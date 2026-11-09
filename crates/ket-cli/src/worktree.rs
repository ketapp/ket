//! Worktrees: creating, listing and opening them, and the ways out of one —
//! merge, pull request, collapse.

use clap::Subcommand;
use ket_core::KetError;
use ket_core::backlog::Backlog;
use ket_core::command::{Invocation, Registry};
use ket_core::id::WorktreeId;
use ket_core::status::{ChangeKind, WorktreeStatus};
use ket_core::workspace::{CollapseOptions, MergeError, PullRequestOptions, Workspace};
use std::collections::BTreeMap;

use crate::cwd;

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    registry.bind("worktree.close", |invocation, _| {
        close_worktree(invocation.require_text("worktree")?)
    })?;

    registry.bind("worktree.collapse", |invocation, _| {
        collapse_worktree(
            invocation.require_text("worktree")?,
            invocation.is_set("force"),
            invocation.is_set("keep"),
            invocation.is_set("keep_losers"),
        )
    })?;

    registry.bind("worktree.merge", |invocation, _| {
        merge_worktree(
            invocation.require_text("worktree")?,
            invocation.is_set("force"),
        )
    })?;

    registry.bind("worktree.pull_request", |invocation, _| {
        open_pull_request(
            invocation.require_text("worktree")?,
            invocation.text_of("title"),
            invocation.text_of("body"),
            invocation.is_set("draft"),
        )
    })?;

    registry.bind("worktree.create", |invocation, _| {
        create_worktree(
            invocation.require_text("project")?,
            invocation.require_text("branch")?,
            invocation.text_of("base"),
            invocation.is_set("no_provision"),
        )
    })?;

    registry.bind("worktree.diff", |invocation, _| {
        diff_worktree(
            invocation.require_text("worktree")?,
            invocation.is_set("stat"),
        )
    })?;

    registry.bind("worktree.list", |invocation, _| {
        list_worktrees(invocation.text_of("project"))
    })?;

    registry.bind("worktree.open", |invocation, _| {
        open_worktree(invocation.require_text("worktree")?)
    })?;

    registry.bind("worktree.provision", |invocation, _| {
        provision_worktree(invocation.require_text("worktree")?)
    })?;

    registry.bind("worktree.prune", |_, _| prune_worktrees())?;

    registry.bind("worktree.remove", |invocation, _| {
        remove_worktree(
            invocation.require_text("worktree")?,
            invocation.is_set("force"),
            invocation.is_set("delete_branch"),
        )
    })?;

    registry.bind("worktree.status", |invocation, _| {
        worktree_status(invocation.require_text("worktree")?)
    })?;

    Ok(())
}

impl WorktreeCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            WorktreeCmd::Add {
                branch,
                project,
                base,
                no_provision,
            } => Invocation::new("worktree.create")
                .text("branch", branch)
                .maybe_text("project", project)
                .maybe_text("base", base)
                .flag("no_provision", no_provision),

            WorktreeCmd::List { project } => {
                Invocation::new("worktree.list").maybe_text("project", project)
            }

            WorktreeCmd::Rm {
                worktree,
                force,
                delete_branch,
            } => Invocation::new("worktree.remove")
                .text("worktree", worktree)
                .flag("force", force)
                .flag("delete_branch", delete_branch),

            WorktreeCmd::Status { worktree } => {
                Invocation::new("worktree.status").maybe_text("worktree", worktree)
            }
            WorktreeCmd::Open { worktree } => {
                Invocation::new("worktree.open").maybe_text("worktree", worktree)
            }
            WorktreeCmd::Close { worktree } => {
                Invocation::new("worktree.close").maybe_text("worktree", worktree)
            }
            WorktreeCmd::Prune => Invocation::new("worktree.prune"),
            WorktreeCmd::Provision { worktree } => {
                Invocation::new("worktree.provision").maybe_text("worktree", worktree)
            }
        }
    }
}

#[derive(Subcommand, Debug)]
pub(super) enum WorktreeCmd {
    /// Create a worktree on a new branch.
    Add {
        /// Branch to create. May contain `/`.
        branch: String,
        /// Project id or name. Defaults to the current directory's project,
        /// then the most recently used.
        #[arg(long, short)]
        project: Option<String>,
        /// Base ref. Defaults to the project's default branch.
        #[arg(long, short)]
        base: Option<String>,
        /// Skip provisioning. The worktree will not be usable by an agent
        /// until `ket worktree provision` has run.
        #[arg(long)]
        no_provision: bool,
    },
    /// List worktrees.
    List {
        /// Restrict to one project. Without it, every project is listed.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Remove a worktree.
    ///
    /// Always named, never taken from the current directory: this can throw work
    /// away, and a destructive command should not guess.
    Rm {
        /// Worktree id.
        worktree: String,
        /// Remove even with uncommitted changes.
        #[arg(long)]
        force: bool,
        /// Also delete the branch.
        #[arg(long)]
        delete_branch: bool,
    },
    /// Show changed files and how far the branch has diverged from its base.
    Status {
        /// Worktree id. Defaults to the worktree the current directory is in.
        worktree: Option<String>,
    },
    /// Mark a worktree open in its project, and give it focus.
    ///
    /// Per-project state, remembered across restarts, so returning to a project
    /// restores what you had open rather than starting over.
    Open {
        /// Worktree id. Defaults to the worktree the current directory is in.
        worktree: Option<String>,
    },
    /// Mark a worktree closed. The worktree itself is untouched.
    Close {
        /// Worktree id. Defaults to the worktree the current directory is in.
        worktree: Option<String>,
    },
    /// Drop registry entries whose directories are gone.
    Prune,
    /// Re-run provisioning for an existing worktree.
    Provision {
        /// Worktree id. Defaults to the worktree the current directory is in.
        worktree: Option<String>,
    },
}

/// Handles `ket worktree add`.
///
/// `project` arrives already decided — named on the command line, or filled in
/// by the registry from wherever the shell is standing.
pub(super) fn create_worktree(
    project: &str,
    branch: &str,
    base: Option<&str>,
    no_provision: bool,
) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let resolved = workspace.resolve_project(Some(project), &cwd())?;

    let (created, prepared) =
        workspace.create_worktree_prepared(&resolved.id, branch, base, None)?;
    let (worktree, report) = if no_provision {
        (created, None)
    } else {
        workspace.provision_new(created)
    };

    workspace.touch_project(&resolved.id)?;
    println!("{}  {}", worktree.id, worktree.path.display());
    // What the base went through is not an error and not the result, so it
    // goes where notes go.
    if let Some(notice) = prepared.notice() {
        eprintln!("ket: {notice}");
    }

    match report {
        Some(report) => {
            println!("provisioned in {}ms", report.duration_ms);
            warn_about_submodules(&report.uninitialised_submodules);
            if report.degraded_to_full_copy() {
                eprintln!(
                    "ket: copy-on-write unavailable on this volume; \
                     provisioning fell back to a full copy"
                );
            }
        }
        None if !no_provision => {
            // create_and_provision keeps the worktree on a provisioning failure,
            // since the checkout is real and may hold work.
            eprintln!(
                "ket: worktree created but NOT provisioned; \
                 run `ket worktree provision {}` before starting an agent",
                worktree.id
            );
        }
        None => {}
    }

    Ok(())
}

/// Handles `ket worktree provision`.
pub(super) fn provision_worktree(worktree: &str) -> Result<(), KetError> {
    let report = Workspace::open()?.provision_worktree(&WorktreeId::new(worktree))?;

    for outcome in &report.directories {
        println!("  {:<20} {:?}", outcome.path, outcome.action);
    }
    for file in &report.files {
        println!("  copied {file}");
    }
    println!("provisioned in {}ms", report.duration_ms);
    warn_about_submodules(&report.uninitialised_submodules);
    Ok(())
}

/// Handles `ket worktree list`.
pub(super) fn list_worktrees(project: Option<&str>) -> Result<(), KetError> {
    let workspace = Workspace::open()?;

    let filter = match project {
        Some(name) => Some(workspace.resolve_project(Some(name), &cwd())?.id),
        None => None,
    };

    let worktrees = workspace.worktrees(filter.as_ref())?;
    if worktrees.is_empty() {
        eprintln!("ket: no worktrees");
        return Ok(());
    }

    // Focus is per project, so listing across projects needs one lookup per
    // project rather than one global answer.
    let mut focus: BTreeMap<String, ket_core::store::ProjectState> = BTreeMap::new();
    for worktree in &worktrees {
        if !focus.contains_key(worktree.project_id.as_str()) {
            focus.insert(
                worktree.project_id.to_string(),
                workspace.project_state(&worktree.project_id)?,
            );
        }
    }

    for worktree in worktrees {
        // `!` missing on disk, `~` present but not provisioned — an agent
        // started in a `~` worktree fails on its first command.
        let ready = match (worktree.exists(), worktree.is_provisioned()) {
            (false, _) => '!',
            (true, false) => '~',
            (true, true) => ' ',
        };

        let state = focus.get(worktree.project_id.as_str());
        let open = match state {
            Some(state) if state.active.as_ref() == Some(&worktree.id) => '*',
            Some(state) if state.open.contains(&worktree.id) => '+',
            _ => ' ',
        };

        println!(
            "{ready}{open} {:<28} {:<24} {}",
            worktree.id,
            worktree.branch,
            worktree.path.display()
        );
    }
    Ok(())
}

/// Handles `ket worktree status`.
pub(super) fn worktree_status(worktree: &str) -> Result<(), KetError> {
    let status = Workspace::open()?.worktree_status(&WorktreeId::new(worktree))?;
    print_status(&status);
    Ok(())
}

/// Handles `ket worktree open`.
pub(super) fn open_worktree(worktree: &str) -> Result<(), KetError> {
    let id = WorktreeId::new(worktree);
    Workspace::open()?.open_worktree(&id)?;
    println!("opened {id}");
    Ok(())
}

/// Handles `ket worktree close`.
pub(super) fn close_worktree(worktree: &str) -> Result<(), KetError> {
    let id = WorktreeId::new(worktree);
    Workspace::open()?.close_worktree(&id)?;
    println!("closed {id}");
    Ok(())
}

/// Handles `ket worktree rm`.
pub(super) fn remove_worktree(
    worktree: &str,
    force: bool,
    delete_branch: bool,
) -> Result<(), KetError> {
    let id = WorktreeId::new(worktree);
    Workspace::open()?.remove_worktree(&id, force, delete_branch)?;
    println!("removed {id}");
    Ok(())
}

/// Handles `ket worktree prune`.
pub(super) fn prune_worktrees() -> Result<(), KetError> {
    let pruned = Workspace::open()?.prune()?;
    if pruned.is_empty() {
        eprintln!("ket: nothing to prune");
    }
    for id in pruned {
        println!("pruned {id}");
    }
    Ok(())
}

/// Warns that a fresh worktree's submodules are empty.
///
/// `git worktree add` does not populate them, so a project that needs its
/// submodules hands the agent a build failure about a missing directory rather
/// than about the task. Saying so at provision time is much cheaper than the
/// agent working it out.
pub(super) fn warn_about_submodules(paths: &[String]) {
    if paths.is_empty() {
        return;
    }

    eprintln!(
        "ket: {} uninitialised submodule(s): {}",
        paths.len(),
        paths.join(", ")
    );
    eprintln!(
        "ket: add `post_command = [\"git\", \"submodule\", \"update\", \"--init\", \
         \"--recursive\"]` to the project's provision config if agents need them; then run \
         `ket project set <project> --automation trust`"
    );
}

/// One character per change kind, following `git status`'s vocabulary.
pub(super) fn change_char(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::TypeChange => 'T',
        ChangeKind::Untracked => '?',
        ChangeKind::Conflicted => 'U',
    }
}

/// Prints a worktree's changed files and its divergence from its base.
pub(super) fn print_status(status: &WorktreeStatus) {
    for file in &status.files {
        let where_ = if file.staged { "staged" } else { "worktree" };
        println!("{} {where_:<8} {}", change_char(file.kind), file.path);
    }

    if status.truncated {
        println!(
            "... showing {} of {} change(s)",
            status.files.len(),
            status.total_changes
        );
    }

    match &status.tracking {
        Some(tracking) => println!(
            "{} change(s); {} ahead, {} behind {}",
            status.total_changes, tracking.ahead, tracking.behind, tracking.base
        ),
        // The base ref is gone — a deleted branch, or a pruned remote. Worth
        // saying, since its absence is why there are no numbers.
        None => println!(
            "{} change(s); base ref no longer resolves",
            status.total_changes
        ),
    }
}

/// Handles `ket collapse`.
pub(super) fn collapse_worktree(
    worktree: &str,
    force: bool,
    keep: bool,
    keep_losers: bool,
) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let report = workspace.collapse(
        &WorktreeId::new(worktree),
        CollapseOptions {
            force,
            keep_winner: keep,
            keep_losers,
        },
    )?;

    if report.already_merged {
        println!("{} was already in {}", report.merged, report.into);
    } else {
        println!("merged {} into {}", report.merged, report.into);
    }

    for id in &report.discarded {
        println!("discarded {id}");
    }

    if report.winner_kept {
        println!("kept {}", report.merged);
    }

    Ok(())
}

/// Handles `ket merge`.
pub(super) fn merge_worktree(worktree: &str, force: bool) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let report = match workspace.merge_worktree(&WorktreeId::new(worktree), force) {
        Ok(report) => report,
        // The two failures read the same to `?` and differently to a person:
        // one of them has a way forward printed on it.
        Err(MergeError::Waivable(why)) => {
            return Err(KetError::Conflict(format!(
                "{why}. Pass --force to merge anyway"
            )));
        }
        Err(MergeError::Failed(e)) => return Err(e),
    };

    match &report.committed {
        Some(commit) if commit.files > 0 => {
            println!("committed {} file(s) in {}", commit.files, report.worktree);
        }
        _ => println!("nothing to commit in {}", report.worktree),
    }

    if report.already_merged {
        println!("{} was already in {}", report.branch, report.into);
    } else {
        println!("merged {} into {}", report.branch, report.into);
    }

    if report.removed {
        println!("removed {}", report.worktree);
    }
    if let Some(why) = &report.remove_failed {
        eprintln!(
            "ket: merged, but could not remove {}: {why}",
            report.worktree
        );
    }

    Ok(())
}

/// Handles `ket pr`.
pub(super) fn open_pull_request(
    worktree: &str,
    title: Option<&str>,
    body: Option<&str>,
    draft: bool,
) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let id = WorktreeId::new(worktree);

    // Whatever was not given is drafted the way the app drafts it: from the
    // backlog note the branch came from, else the branch itself.
    let (title, body) = match (title, body) {
        (Some(title), Some(body)) => (title.to_owned(), body.to_owned()),
        _ => {
            let project = workspace
                .worktrees(None)?
                .into_iter()
                .find(|w| w.id == id)
                .map(|w| w.project_id)
                .ok_or_else(|| KetError::UnknownWorktree(worktree.to_owned()))?;
            let notes = Backlog::load(&project).map(|b| b.notes).unwrap_or_default();
            let draft = workspace.pull_request_draft(&id, &notes)?;
            (
                title.map_or(draft.title, str::to_owned),
                body.map_or(draft.body, str::to_owned),
            )
        }
    };

    let report = workspace.open_pull_request(&id, &PullRequestOptions { title, body, draft })?;

    match &report.committed {
        Some(commit) if commit.files > 0 => {
            println!("committed {} file(s) in {}", commit.files, report.worktree);
        }
        _ => println!("nothing to commit in {}", report.worktree),
    }
    println!("pushed {} to {}", report.branch, report.remote);
    let pr = &report.pull_request;
    if report.existed {
        println!("#{} was already open: {}", pr.number, pr.url);
    } else {
        println!("opened #{} into {}: {}", pr.number, report.base, pr.url);
    }

    Ok(())
}

/// Handles `ket diff`.
///
/// Writes to stdout in `git diff` format so the output pipes into everything
/// that already reads diffs — `delta`, `git apply`, a pager — rather than
/// inventing a format nothing else understands.
pub(super) fn diff_worktree(worktree: &str, stat: bool) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let diff = workspace.worktree_diff(&WorktreeId::new(worktree))?;

    if diff.is_empty() {
        eprintln!("ket: no changes against {}", diff.base);
        return Ok(());
    }

    if stat {
        let mut added_total = 0;
        let mut removed_total = 0;

        for file in &diff.files {
            let (added, removed) = file.line_counts();
            added_total += added;
            removed_total += removed;

            if file.binary {
                println!("{:>12}  {}", "binary", file.path);
            } else {
                println!(
                    "{:>5} {:<6}  {}",
                    format!("+{added}"),
                    format!("-{removed}"),
                    file.path
                );
            }
        }

        println!(
            "\n{} file(s), +{} -{} against {}",
            diff.total_files, added_total, removed_total, diff.base
        );
    } else {
        print!("{}", diff.to_unified());
    }

    if diff.truncated {
        eprintln!(
            "ket: showing {} of {} changed files; the rest were dropped to stay inside the budget",
            diff.files.len(),
            diff.total_files
        );
    }

    Ok(())
}

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &[
        "ket",
        "collapse",
        "wt-1",
        "--force",
        "--keep",
        "--keep-losers",
    ],
    &["ket", "merge", "wt-1", "--force"],
    &["ket", "pr", "wt-1"],
    &[
        "ket",
        "pr",
        "wt-1",
        "--title",
        "Fix it",
        "--body",
        "Closes #1",
        "--draft",
    ],
    &["ket", "diff", "wt-1"],
    &["ket", "diff", "--stat"],
    &["ket", "worktree", "add", "fix", "--base", "main"],
    &["ket", "worktree", "add", "fix", "--no-provision"],
    &["ket", "worktree", "list"],
    &[
        "ket",
        "worktree",
        "rm",
        "wt-1",
        "--force",
        "--delete-branch",
    ],
    &["ket", "worktree", "status"],
    &["ket", "worktree", "open", "wt-1"],
    &["ket", "worktree", "close"],
    &["ket", "worktree", "prune"],
    &["ket", "worktree", "provision"],
];
