//! The command registry, driven the way a client drives it.
//!
//! The unit tests beside `ket_core::command` check the registry's own rules.
//! This checks the shape a client actually uses: core supplies the catalogue,
//! the client binds handlers that do real work, and dispatch — with arguments
//! filled in from what is selected rather than named — produces a worktree that
//! `git` itself agrees exists. That is `ket-cli`'s entire structure, exercised
//! without `ket-cli`, and it is what makes the registry load-bearing rather than
//! a table nobody dispatches through.

use std::sync::Arc;

use ket_core::KetError;
use ket_core::command::{Arg, ArgKind, Category, Command, Context, Invocation, Registry};
use ket_core::id::ProjectId;
use ket_core::store::Store;
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, git, init_repo};

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

/// A registry bound the way a client binds one: core's catalogue, the client's
/// implementations of the parts it supports.
fn bound(workspace: &Arc<Workspace>) -> Registry {
    let mut registry = Registry::builtin();

    let engine = Arc::clone(workspace);
    registry
        .bind("project.add", move |invocation, _| {
            let path = invocation.text_of("path").unwrap_or(".");
            engine.add_project(std::path::Path::new(path)).map(|_| ())
        })
        .expect("project.add is a registered command");

    let engine = Arc::clone(workspace);
    registry
        .bind("worktree.create", move |invocation, _| {
            engine
                .create_worktree(
                    &ProjectId::new(invocation.require_text("project")?),
                    invocation.require_text("branch")?,
                    invocation.text_of("base"),
                    None,
                )
                .map(|_| ())
        })
        .expect("worktree.create is a registered command");

    registry
}

#[test]
fn dispatch_creates_a_worktree_git_agrees_exists() {
    let sandbox = Sandbox::new("command-dispatch");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let engine = Arc::new(workspace(&sandbox));
    let registry = bound(&engine);

    registry
        .dispatch(
            &Invocation::new("project.add").text("path", repo.to_string_lossy()),
            &Context::default(),
        )
        .expect("project.add");

    // The project is now what is selected, so the branch is the only thing the
    // caller has to name — exactly what a palette would send.
    let project = engine.projects().unwrap().into_iter().next().unwrap();
    let context = Context {
        project: Some(project.id.clone()),
        ..Context::default()
    };

    registry
        .dispatch(
            &Invocation::new("worktree.create").text("branch", "729-fix"),
            &context,
        )
        .expect("worktree.create");

    let worktree = engine.worktrees(None).unwrap().into_iter().next().unwrap();
    assert!(worktree.path.is_dir());
    assert!(
        git(&repo, &["worktree", "list"]).contains(&worktree.path.display().to_string()),
        "git does not know about the worktree the registry created"
    );
}

#[test]
fn a_worktree_command_is_offered_only_once_a_worktree_is_selected() {
    let sandbox = Sandbox::new("command-applicable");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let engine = Arc::new(workspace(&sandbox));
    let registry = bound(&engine);

    let project = engine.add_project(&repo).unwrap();
    let worktree = engine
        .create_worktree(&project.id, "729-fix", None, None)
        .unwrap();

    let offered = |context: &Context| {
        registry
            .applicable(context)
            .iter()
            .any(|command| command.id.as_str() == "worktree.status")
    };

    assert!(!offered(&Context::default()));
    assert!(offered(&Context {
        worktree: Some(worktree.id.clone()),
        ..Context::default()
    }));
}

#[test]
fn a_command_a_client_did_not_implement_is_an_error_not_a_panic() {
    // A client is free to support a subset — a palette with no terminal has no
    // use for `event.follow`. Everything it did implement keeps working.
    let sandbox = Sandbox::new("command-partial");
    let engine = Arc::new(workspace(&sandbox));
    let registry = bound(&engine);

    let result = registry.dispatch(&Invocation::new("worktree.prune"), &Context::default());
    assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    assert!(!registry.unbound().is_empty());
}

#[test]
fn a_registered_command_joins_the_same_listing_as_the_built_ins() {
    // The plugin path, without a plugin: a command registered from outside is
    // indistinguishable from a built-in to everything that reads the registry,
    // which is what lets the UI need no plugin-specific code.
    let mut registry = Registry::builtin();
    let before = registry.len();

    registry
        .register(
            Command::new("acme.ship", "Ship it", Category::Review)
                .arg(Arg::required("worktree", ArgKind::Worktree)),
        )
        .unwrap();

    assert_eq!(registry.len(), before + 1);
    assert!(registry.get("acme.ship").is_some());

    let review: Vec<&str> = registry
        .in_category(Category::Review)
        .iter()
        .map(|command| command.id.as_str())
        .collect();
    assert!(review.contains(&"acme.ship"));

    // And it obeys the same context rules as everything else.
    assert!(
        !registry
            .get("acme.ship")
            .unwrap()
            .applies_in(&Context::default())
    );
}

#[test]
fn the_catalogue_is_the_same_no_matter_who_asks_for_it() {
    // Two clients, two registries, one list of things ket can do. If this ever
    // stops holding, the CLI and the palette have started to disagree about what
    // ket is capable of.
    let mine: Vec<String> = Registry::builtin()
        .list()
        .map(|command| command.id.as_str().to_owned())
        .collect();
    let yours: Vec<String> = Registry::builtin()
        .list()
        .map(|command| command.id.as_str().to_owned())
        .collect();

    assert_eq!(mine, yours);
    assert!(mine.iter().all(|id| id.contains('.')));
}
