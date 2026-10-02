//! What the shell can actually run, and how a command reaches the screen.
//!
//! `ket-cli` binds the registry's commands to functions that print. The shell
//! binds the same commands to functions that *act*, which raises a problem the
//! CLI does not have: a registry handler is
//! `Fn(&Invocation, &Context) -> Result<()>` and cannot borrow the `Shell`, so a
//! command that changes what is on screen has nowhere to write to.
//!
//! The answer is an [`Outbox`]. A handler posts an [`Intent`]; the shell drains
//! it after dispatch and applies it. That keeps **one** dispatch path — the
//! registry — rather than growing a second one beside it, which is the failure
//! the registry exists to prevent.
//!
//! Two deliberate absences, both reported rather than silently missing:
//!
//! - **Commands needing free text are unbound.** `worktree.create` wants a
//!   branch name and the palette has no input prompt yet. `Registry::resolve`
//!   already refuses them with a clear message naming the argument, so an
//!   unbound command says what it needs instead of failing obscurely.
//! - **`worktree.collapse` is unbound on purpose.** It merges one attempt and
//!   *deletes the others*. Reachable from a fuzzy list in two keystrokes with no
//!   confirmation, that is a way to lose an afternoon's work to a typo. It wants
//!   a confirmation step before it gets a palette entry.

use std::path::Path;
use std::sync::{Arc, Mutex};

use ket_core::command::{Category, Command, Invocation, Registry};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::workspace::Workspace;
use ket_core::{KetError, Result};

use crate::tabs::Toward;

/// Something a command asked the shell to do once it had run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Intent {
    /// Re-read projects and worktrees from core.
    Reload,
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// Show or hide the right panel.
    TogglePanel,
    /// Divide the focused pane, carrying its active tab into the new one on
    /// the right.
    SplitRight,
    /// The same, with the new pane below.
    SplitDown,
    /// Remove the focused pane if another pane can take focus.
    ClosePane,
    /// Move focus to the pane that lies this way, if one does.
    FocusPane(Toward),
    /// Say something in the content area.
    Say(String),
    /// Ask for a folder and register the repository containing it.
    AddProject,
    /// Open the global settings view.
    OpenSettings,
    /// Open the settings dialog for a project.
    OpenProjectSettings(ProjectId),
    /// Ask before removing a project.
    ///
    /// The handler never removes directly: `project.remove` is one fuzzy
    /// match away in the palette, and a project's worktrees go with it.
    ConfirmRemoveProject(ProjectId),
}

/// Where handlers post intents for the shell to pick up.
///
/// Shared and locked because a handler must be `Send + Sync` to live in the
/// registry, and cannot hold a reference to the shell that owns it.
pub(crate) type Outbox = Arc<Mutex<Vec<Intent>>>;

/// Posts an intent, ignoring a poisoned lock.
///
/// A poisoned outbox means a handler panicked while holding it. Losing one
/// intent is not worth taking the window down over, and the operation the
/// handler performed has already happened.
fn post(outbox: &Outbox, intent: Intent) {
    if let Ok(mut queue) = outbox.lock() {
        queue.push(intent);
    }
}

/// The worktree an invocation names, or the one in view.
///
/// `Registry::resolve` has already filled the argument in from the context, so
/// by the time a handler runs this is present for any command that needs it.
fn worktree_of(invocation: &Invocation) -> Result<WorktreeId> {
    invocation
        .require_text("worktree")
        .map(|id| WorktreeId::new(id.to_owned()))
}

/// Opens a workspace, or reports why it could not.
///
/// One per invocation rather than one held for the shell's lifetime. That is
/// spike-grade and matches what the rest of the shell currently does; holding a
/// single `Workspace` is Epic 5's own cleanup, not this file's.
fn workspace() -> Result<Workspace> {
    Workspace::open()
}

/// A shell command's body: does the work, says what the shell should do next.
///
/// Boxed because the bindings below have different closure types and are
/// registered through one helper; named because an inline `Box<dyn Fn(..)>` in
/// that helper's signature is unreadable.
type ShellHandler = Box<dyn Fn(&Invocation) -> Result<Intent> + Send + Sync>;

/// Builds the registry the shell dispatches through.
///
/// Returns the registry and the outbox its handlers write to.
pub(crate) fn registry() -> (Registry, Outbox) {
    let outbox: Outbox = Arc::new(Mutex::new(Vec::new()));
    let mut registry = Registry::builtin();

    // Commands that exist only in the shell. The `panes` field on `Command` was
    // added for exactly this and had no user until now — core never interprets a
    // pane name, it only compares it.
    let shell_commands = [
        Command::new(
            "shell.toggle_sidebar",
            "Toggle the sidebar",
            Category::Application,
        ),
        Command::new(
            "shell.toggle_panel",
            "Toggle the right panel",
            Category::Application,
        ),
        Command::new("shell.refresh", "Refresh projects", Category::Application),
        Command::new(
            "shell.split_right",
            "Split pane right",
            Category::Application,
        ),
        Command::new("shell.split_down", "Split pane down", Category::Application),
        Command::new("shell.close_pane", "Close pane", Category::Application),
        // Moving between panes by keyboard. Without these a split arrangement
        // is one the mouse has to be picked up to get around, which rather
        // defeats splitting a window in an editor.
        Command::new(
            "shell.focus_pane_left",
            "Focus pane to the left",
            Category::Application,
        ),
        Command::new(
            "shell.focus_pane_right",
            "Focus pane to the right",
            Category::Application,
        ),
        Command::new(
            "shell.focus_pane_up",
            "Focus pane above",
            Category::Application,
        ),
        Command::new(
            "shell.focus_pane_down",
            "Focus pane below",
            Category::Application,
        ),
    ];
    for command in shell_commands {
        // A duplicate id here is a mistake in this file, not a runtime state.
        registry
            .register(command)
            .expect("shell command ids are unique and well formed");
    }

    bind(&mut registry, &outbox);
    (registry, outbox)
}

/// Binds every command the shell can carry out.
fn bind(registry: &mut Registry, outbox: &Outbox) {
    let mut wire = |id: &str, handler: ShellHandler| {
        let outbox = Arc::clone(outbox);
        // A bind failure means an id in this list is not in the registry, which
        // is a mistake here rather than a runtime condition — and
        // `every_shell_command_is_bound_or_deliberately_not` is the test that
        // turns it into a failure instead of a panic.
        registry
            .bind(id, move |invocation, _| {
                let intent = handler(invocation)?;
                post(&outbox, intent);
                Ok(())
            })
            .expect("shell binds only ids the registry has");
    };

    wire(
        "shell.toggle_sidebar",
        Box::new(|_| Ok(Intent::ToggleSidebar)),
    );
    wire("shell.toggle_panel", Box::new(|_| Ok(Intent::TogglePanel)));
    wire("shell.refresh", Box::new(|_| Ok(Intent::Reload)));
    wire("config.show", Box::new(|_| Ok(Intent::OpenSettings)));
    wire("shell.split_right", Box::new(|_| Ok(Intent::SplitRight)));
    wire("shell.split_down", Box::new(|_| Ok(Intent::SplitDown)));
    wire("shell.close_pane", Box::new(|_| Ok(Intent::ClosePane)));
    wire(
        "shell.focus_pane_left",
        Box::new(|_| Ok(Intent::FocusPane(Toward::Left))),
    );
    wire(
        "shell.focus_pane_right",
        Box::new(|_| Ok(Intent::FocusPane(Toward::Right))),
    );
    wire(
        "shell.focus_pane_up",
        Box::new(|_| Ok(Intent::FocusPane(Toward::Up))),
    );
    wire(
        "shell.focus_pane_down",
        Box::new(|_| Ok(Intent::FocusPane(Toward::Down))),
    );

    wire(
        "worktree.open",
        Box::new(|invocation| {
            let id = worktree_of(invocation)?;
            workspace()?.open_worktree(&id)?;
            Ok(Intent::Reload)
        }),
    );

    wire(
        "worktree.close",
        Box::new(|invocation| {
            let id = worktree_of(invocation)?;
            workspace()?.close_worktree(&id)?;
            Ok(Intent::Reload)
        }),
    );

    wire(
        "worktree.provision",
        Box::new(|invocation| {
            let id = worktree_of(invocation)?;
            let report = workspace()?.provision_worktree(&id)?;
            Ok(Intent::Say(format!(
                "provisioned in {}ms",
                report.duration_ms
            )))
        }),
    );

    wire(
        "worktree.prune",
        Box::new(|_| {
            let dropped = workspace()?.prune()?;
            Ok(Intent::Say(match dropped.len() {
                0 => "nothing to prune".to_owned(),
                n => format!("pruned {n} worktree(s)"),
            }))
        }),
    );

    wire(
        "worktree.remove",
        Box::new(|invocation| {
            let id = worktree_of(invocation)?;
            // Never forced from the palette. Core refuses a removal that would
            // discard work, and that refusal is the safety here — a `--force`
            // reachable by fuzzy search is a way to lose an agent's afternoon.
            workspace()?.remove_worktree(&id, false, true)?;
            Ok(Intent::Reload)
        }),
    );

    wire(
        "worktree.status",
        Box::new(|invocation| {
            let id = worktree_of(invocation)?;
            let status = workspace()?.worktree_status(&id)?;
            Ok(Intent::Say(format!(
                "{} changed file(s)",
                status.total_changes
            )))
        }),
    );

    wire("worktree.diff", Box::new(|_| Ok(Intent::Reload)));

    wire(
        "project.add",
        Box::new(|invocation| match invocation.text_of("path") {
            // A path was named — a client that already knows the folder.
            Some(path) => {
                workspace()?.add_project(Path::new(path))?;
                Ok(Intent::Reload)
            }
            // The palette names nothing; the shell asks with the OS picker.
            None => Ok(Intent::AddProject),
        }),
    );

    wire(
        "project.settings.set",
        Box::new(|invocation| {
            let id = ProjectId::new(invocation.require_text("project")?.to_owned());
            Ok(Intent::OpenProjectSettings(id))
        }),
    );

    wire(
        "project.remove",
        Box::new(|invocation| {
            let id = ProjectId::new(invocation.require_text("project")?.to_owned());
            Ok(Intent::ConfirmRemoveProject(id))
        }),
    );
}

/// Command ids the shell deliberately leaves unbound, and why.
///
/// Kept as data rather than as prose so a test can assert the set is exactly
/// this — an unbound command that nobody decided to leave unbound is the thing
/// worth catching.
pub(crate) const DELIBERATELY_UNBOUND: &[(&str, &str)] = &[
    (
        "worktree.create",
        "needs a branch name; no input prompt yet",
    ),
    ("agent.run", "needs a prompt; no input prompt yet"),
    (
        "worktree.merge",
        "reached from the row's own button, which knows which worktree",
    ),
    (
        "worktree.collapse",
        "discards the other attempts; wants a confirmation step first",
    ),
    ("worktree.list", "the sidebar already shows this"),
    ("project.list", "the sidebar already shows this"),
    (
        "session.list",
        "belongs in the agent dashboard, once it exists",
    ),
    (
        "session.reap",
        "belongs in the agent dashboard, once it exists",
    ),
    ("app.commands", "the palette itself is this"),
    ("app.doctor", "prints a report; wants a pane to print into"),
    ("config.path", "prints a report; wants a pane to print into"),
    (
        "event.follow",
        "wants an event pane, which is Epic 5 proper",
    ),
    ("event.list", "wants an event pane, which is Epic 5 proper"),
    ("backlog.add", "the Backlog dialog does this"),
    ("backlog.list", "the Backlog dialog shows this"),
    ("backlog.done", "the Backlog dialog does this"),
    ("backlog.reopen", "the Backlog dialog does this"),
    ("backlog.remove", "the Backlog dialog does this"),
    ("backlog.tag", "the Backlog dialog does this"),
    ("backlog.move", "the Backlog dialog does this"),
    ("backlog.store", "Project settings does this"),
];

/// Why `id` is unbound, if it deliberately is.
pub(crate) fn unbound_reason(id: &str) -> Option<&'static str> {
    DELIBERATELY_UNBOUND
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, why)| *why)
}

/// Turns a dispatch failure into something worth reading.
///
/// A command that is unbound on purpose says so and says why, rather than
/// reporting the registry's generic "has no implementation here".
pub(crate) fn explain(id: &str, error: &KetError) -> String {
    match unbound_reason(id) {
        Some(why) => format!("{id}: {why}"),
        None => format!("{error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_bound_or_deliberately_not() {
        let (registry, _) = registry();

        for id in registry.unbound() {
            assert!(
                unbound_reason(id.as_str()).is_some(),
                "{id} is unbound and nobody decided that: bind it, or add it to \
                 DELIBERATELY_UNBOUND with the reason"
            );
        }
    }

    #[test]
    fn nothing_claims_to_be_unbound_that_is_not() {
        let (registry, _) = registry();
        let unbound: Vec<String> = registry
            .unbound()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();

        for (id, _) in DELIBERATELY_UNBOUND {
            assert!(
                unbound.contains(&(*id).to_owned()),
                "{id} is listed as deliberately unbound but is bound, or does not exist"
            );
        }
    }

    #[test]
    fn the_shells_own_commands_are_registered() {
        let (registry, _) = registry();

        assert!(registry.get("shell.toggle_sidebar").is_some());
        assert!(registry.get("shell.refresh").is_some());
        assert!(registry.get("shell.split_right").is_some());
        assert!(registry.get("shell.split_down").is_some());
        assert!(registry.get("shell.close_pane").is_some());
    }

    #[test]
    fn a_handler_posts_its_intent() {
        let (registry, outbox) = registry();
        let context = ket_core::command::Context::default();

        registry
            .dispatch(&Invocation::new("shell.toggle_sidebar"), &context)
            .expect("toggle dispatches");

        let queue = outbox.lock().expect("outbox");
        assert_eq!(queue.as_slice(), &[Intent::ToggleSidebar]);
    }
}
