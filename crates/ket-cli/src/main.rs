//! `ket` — the command line interface.
//!
//! A first-class client of `ket-core`, not a debug harness for it. It stays
//! useful after the native shell exists, and it is what makes headless
//! operation possible without a redesign.
//!
//! Epic 4 is complete: `doctor`, `config`, `project`, `worktree`, `run`, `ps`,
//! `diff`, `collapse`, and `events --follow`.
//!
//! Since Epic 10 there is no dispatch table here. Every subcommand resolves to a
//! named command in `ket_core::command`'s registry and runs through it, so the
//! CLI, the palette, and a keybinding file all reach the same list of things ket
//! can do. `ket commands` prints that list.
//!
//! Each area — `agent`, `app`, `backlog`, `browser`, `filter`, `host`,
//! `project`, `worktree` — is a module holding its subcommands' argv, how they
//! become invocations, its implementations and its share of the argv list the
//! tests check the registry against.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use ket_core::command::{self, Invocation, Registry};
use ket_core::workspace::Workspace;
use ket_core::{KetError, logging};

mod agent;
mod app;
mod backlog;
mod browser;
mod filter;
mod host;
mod project;
mod worktree;

use app::ConfigCmd;
use backlog::BacklogCmd;
use browser::BrowserCmd;
use filter::FilterArgs;
use host::HostCmd;
use project::ProjectCmd;
use worktree::WorktreeCmd;

#[derive(Parser, Debug)]
#[command(
    name = "ket",
    version = ket_core::build_info::display(),
    about = "Run coding agents in parallel across isolated git worktrees (beta)"
)]
struct Cli {
    /// Logging filter. Overrides the `KET_LOG` environment variable.
    #[arg(long, global = true, value_name = "FILTER")]
    log: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Check that everything ket depends on is present.
    Doctor,

    /// Zip logs, `ket doctor` and versions into a file for a bug report.
    ///
    /// Credentials, tokens and pairing codes are taken out first. The same
    /// bundle as Help → Export Diagnostics in the app, for when the app will
    /// not open.
    Diagnostics {
        /// Where to write the zip. Defaults to `ket-diagnostics.zip` here.
        #[arg(long, short, value_name = "PATH")]
        output: Option<PathBuf>,
    },

    /// List every command ket can perform.
    ///
    /// The same registry the palette and a keybinding file resolve against, so
    /// this is documentation and a check that there is only one of them.
    Commands {
        /// Only the commands that apply where you are standing.
        #[arg(long)]
        applicable: bool,
        /// Restrict to one category.
        #[arg(long, value_name = "CATEGORY")]
        category: Option<String>,
    },

    /// Inspect or stop the ket host, which runs terminals for the app.
    Host {
        #[command(subcommand)]
        cmd: HostCmd,
    },

    /// Inspect ket's configuration.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },

    /// Merge one attempt into its base and discard the rest.
    ///
    /// The end of the parallel-agent loop: several worktrees worked the same
    /// task, one won. Only committed work is merged, so this refuses a winner
    /// with uncommitted changes rather than half-merging it.
    ///
    /// The winner is always named, never taken from the current directory: this
    /// discards the other attempts, and a destructive command should not guess.
    Collapse {
        /// Worktree id of the attempt to keep.
        worktree: String,
        /// Merge even when the winner or the checkout is dirty.
        #[arg(long)]
        force: bool,
        /// Keep the winning worktree instead of removing it once merged.
        #[arg(long)]
        keep: bool,
        /// Leave the losing worktrees in place.
        #[arg(long)]
        keep_losers: bool,
    },

    /// Commit a worktree's work and merge it into its base.
    ///
    /// The everyday end of one worktree: whatever is uncommitted is committed,
    /// and the branch is merged into the base it was branched from. Unlike
    /// `collapse`, the other worktrees are left exactly where they are.
    ///
    /// The commit message comes from `merge.commit_template` in the config, and
    /// what happens to the worktree afterwards from `merge.keep_after_merge`.
    Merge {
        /// Worktree id to commit and merge.
        worktree: String,
        /// Merge even when the primary checkout has uncommitted changes.
        #[arg(long)]
        force: bool,
    },

    /// Commit a worktree's work, push its branch and open a pull request.
    ///
    /// The other way out of a worktree: the same commit `merge` makes, then
    /// the branch goes to the repository's remote as the GitHub account the
    /// project picked, and a pull request into its base is opened with `gh`.
    /// A pull request the branch already has open is pushed to and reported
    /// instead of doubled. The primary checkout is not touched.
    ///
    /// Without `--title`, the title is the backlog note the branch was started
    /// from, or the branch name made readable; without `--body`, it is
    /// `Closes #142` for a branch named for an issue.
    Pr {
        /// Worktree id to push and open a pull request for.
        worktree: String,
        /// The pull request's title.
        #[arg(long)]
        title: Option<String>,
        /// The pull request's description.
        #[arg(long)]
        body: Option<String>,
        /// Open it as a draft.
        #[arg(long)]
        draft: bool,
    },

    /// Print what a worktree changed, as a unified diff.
    ///
    /// The comparison runs against the worktree's base and includes uncommitted
    /// work, so an agent's changes show up whether or not it committed them.
    Diff {
        /// Worktree id. Defaults to the worktree the current directory is in.
        worktree: Option<String>,
        /// Summarise per file instead of printing hunks.
        #[arg(long)]
        stat: bool,
    },

    /// Stream ket's event bus as JSON lines on stdout.
    Events {
        /// Keep the stream open until interrupted.
        #[arg(long)]
        follow: bool,
    },

    /// Register and manage projects.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },

    /// Notes on work not started yet.
    Backlog {
        #[command(subcommand)]
        cmd: BacklogCmd,
    },

    /// Create and manage worktrees.
    Worktree {
        #[command(subcommand)]
        cmd: WorktreeCmd,
    },

    /// Run an agent against a worktree.
    Run {
        /// Agent to run. Defaults to the project's preferred agent, then the
        /// first configured one.
        agent: Option<String>,
        /// Worktree id to run in. Defaults to the one the current directory is in.
        #[arg(long, short)]
        on: Option<String>,
        /// The prompt. Use `-` to read it from stdin.
        #[arg(long, short)]
        prompt: String,
    },

    /// Start work in a new worktree, as a tab in the ket window.
    ///
    /// The open ket window makes the worktree the way its own dialogs do,
    /// opens the agent there and hands it the prompt, so the work shows in
    /// the sidebar like any other. Returns once the window has it; `ket
    /// worktree list` shows the worktree once it exists. Unlike `ket run`,
    /// which runs an agent here and waits for it, this needs ket open.
    Start {
        /// What the agent is to do. Use `-` to read it from stdin.
        prompt: String,
        /// Agent to run. Defaults to the project's preferred agent.
        #[arg(long, short)]
        agent: Option<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },

    /// Drive this worktree's browser tab in the ket window.
    ///
    /// For an agent checking its work in a page: the worktree's Chromium tab
    /// in ket — the one in front, else the latest — which `open` makes when
    /// there is none. Needs ket open. Results go to stdout, tersely; why
    /// something could not be done goes to stderr.
    ///
    /// To use a page as a person would: `snapshot -i` lists what can be
    /// acted on, each with a ref like `@e3`; `click @e3`, `fill @e3 'text'`
    /// and `press Enter` act with real mouse and keyboard events; `wait`,
    /// `screenshot` and `console` show how it went. An alert, confirm or
    /// prompt the page opens is said at once; `dialog accept` or `dialog
    /// dismiss` answers it.
    Browser {
        /// Worktree id. Defaults to the agent's own (`KET_WORKTREE_ID`), then
        /// the worktree the current directory is in.
        #[arg(long, short, global = true)]
        worktree: Option<String>,
        #[command(subcommand)]
        cmd: BrowserCmd,
    },

    /// Run a command and print its output trimmed, the way an Economy
    /// level's filter trims it for an agent: a successful run loses its
    /// progress and per-test lines, a failed run is printed whole, and the
    /// exit status is the command's own.
    Filter(FilterArgs),

    /// Print the full output of a command the filter trimmed, by the id in
    /// its trailer.
    Recall {
        /// The id, or the end of it.
        id: String,
    },

    /// List agent sessions across every project.
    Ps {
        /// Drop finished and abandoned records instead of listing them.
        #[arg(long)]
        reap: bool,
    },

    /// Take ket's hooks and status line out of every agent, for uninstalling it.
    ///
    /// Only ket's own entries go, and Claude's status line gets back whatever
    /// ket displaced. The Homebrew cask runs this before it deletes ket.app; a
    /// ket window still open puts them back the next time it starts.
    UninstallHooks,
}

fn main() -> ExitCode {
    // The CLI can be the ket host too — see `ket_core::host`.
    ket_core::host::serve_if_asked();
    // And the hook that trims a command's output — see `ket_core::filter`.
    ket_core::filter::serve_if_asked();
    // Likewise the permission hook, which rules every tool call an agent
    // makes and so has to answer in milliseconds — see `ket_core::permissions`.
    ket_core::permissions::serve_if_asked();

    let cli = Cli::parse();
    // `filter` and `recall` print for a model to read, and a stray warning
    // on stderr would land in its context beside the command's own.
    let quiet = matches!(cli.command, Cmd::Filter { .. } | Cmd::Recall { .. });
    logging::init(
        cli.log
            .as_deref()
            .unwrap_or(if quiet { "error" } else { "info" }),
    );

    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ket: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a subcommand, through the registry rather than around it.
///
/// clap parses the argv shape, [`Cmd::invocation`] turns it into a registry
/// invocation, and the registry resolves that — filling in what the current
/// directory implies — before calling whatever this crate bound to the command.
/// There is deliberately no second dispatch table here to drift out of step with
/// the one the palette will read.
fn run(command: Cmd) -> Result<(), KetError> {
    registry()?.dispatch(&command.invocation(), &context())
}

impl Cmd {
    /// Turns parsed argv into a registry invocation.
    ///
    /// This is where the CLI's shape stops mattering. A switch that changes
    /// *what happens* selects a different command — `--reap` is `session.reap`,
    /// `--follow` is `event.follow` — while a switch that changes how the same
    /// result is rendered stays an argument, as `--stat` does.
    fn invocation(self) -> Invocation {
        match self {
            Cmd::Doctor => Invocation::new("app.doctor"),
            Cmd::Diagnostics { output } => Invocation::new("app.diagnostics").maybe_text(
                "output",
                output.map(|path| path.to_string_lossy().into_owned()),
            ),

            Cmd::Host { cmd } => cmd.invocation(),

            Cmd::Commands {
                applicable,
                category,
            } => Invocation::new("app.commands")
                .flag("applicable", applicable)
                .maybe_text("category", category),

            Cmd::Config { cmd } => cmd.invocation(),

            Cmd::Collapse {
                worktree,
                force,
                keep,
                keep_losers,
            } => Invocation::new("worktree.collapse")
                .text("worktree", worktree)
                .flag("force", force)
                .flag("keep", keep)
                .flag("keep_losers", keep_losers),

            Cmd::Merge { worktree, force } => Invocation::new("worktree.merge")
                .text("worktree", worktree)
                .flag("force", force),

            Cmd::Pr {
                worktree,
                title,
                body,
                draft,
            } => Invocation::new("worktree.pull_request")
                .text("worktree", worktree)
                .maybe_text("title", title)
                .maybe_text("body", body)
                .flag("draft", draft),

            Cmd::Diff { worktree, stat } => Invocation::new("worktree.diff")
                .maybe_text("worktree", worktree)
                .flag("stat", stat),

            Cmd::Events { follow } => {
                Invocation::new(if follow { "event.follow" } else { "event.list" })
            }

            Cmd::Project { cmd } => cmd.invocation(),

            Cmd::Backlog { cmd } => cmd.invocation(),

            Cmd::Worktree { cmd } => cmd.invocation(),

            Cmd::Run { agent, on, prompt } => Invocation::new("agent.run")
                .maybe_text("worktree", on)
                .text("prompt", prompt)
                .maybe_text("agent", agent),

            Cmd::Start {
                prompt,
                agent,
                project,
            } => Invocation::new("work.start")
                .maybe_text("project", project)
                .text("prompt", prompt)
                .maybe_text("agent", agent),

            Cmd::Ps { reap } => Invocation::new(if reap { "session.reap" } else { "session.list" }),

            Cmd::UninstallHooks => Invocation::new("app.unhook"),

            Cmd::Browser { worktree, cmd } => cmd
                .invocation()
                .maybe_text("worktree", worktree.or_else(agent_worktree)),

            Cmd::Filter(args) => args.invocation(),
            Cmd::Recall { id } => Invocation::new("shell.recall").text("id", id),
        }
    }
}

/// Core's catalogue of commands, with this crate's implementations attached.
///
/// Each area's module attaches its own, and registers the commands that are
/// the CLI's alone: a new command touches its module, not this function.
///
/// `bind` refuses an id core does not know, so a typo here fails the test below
/// rather than leaving a subcommand that quietly stops working; and
/// `Registry::unbound` coming back empty is what proves the CLI implements
/// everything the palette will offer. Both are asserted in `tests`.
fn registry() -> Result<Registry, KetError> {
    let mut registry = Registry::builtin();
    agent::register(&mut registry)?;
    app::register(&mut registry)?;
    backlog::register(&mut registry)?;
    browser::register(&mut registry)?;
    filter::register(&mut registry)?;
    host::register(&mut registry)?;
    project::register(&mut registry)?;
    worktree::register(&mut registry)?;

    Ok(registry)
}

/// What the CLI has selected: the worktree the current directory sits in, and
/// the project that worktree — or the current directory — belongs to.
///
/// Best-effort, and silent when it cannot answer. `ket doctor` is what you run
/// when ket itself is broken, and it must not fail because the state file would
/// not load, so an unanswerable context is simply empty and every command that
/// needs nothing selected carries on working.
///
/// The shell fills in the other two fields; a terminal has no focused pane and
/// no session in view.
fn context() -> command::Context {
    let cwd = cwd();

    let Ok(workspace) = Workspace::open() else {
        return command::Context::default();
    };

    let worktree = workspace
        .worktrees(None)
        .ok()
        .and_then(|worktrees| worktrees.into_iter().find(|w| cwd.starts_with(&w.path)));

    let project = match &worktree {
        Some(worktree) => Some(worktree.project_id.clone()),
        None => workspace.resolve_project(None, &cwd).ok().map(|p| p.id),
    };

    command::Context {
        project,
        worktree: worktree.map(|w| w.id),
        session: None,
        pane: None,
    }
}

/// The current directory, or `/` if it cannot be determined.
///
/// A deleted or unreadable cwd is not worth failing a command over — project
/// resolution simply falls through to recency.
fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// The worktree an agent was started in, from the environment ket gives its
/// terminal — ahead of the current directory, which the agent may have left.
fn agent_worktree() -> Option<String> {
    std::env::var(ket_core::agent_hooks::WORKTREE_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ket_core::id::WorktreeId;

    /// Every way of reaching a command from argv, each module's own list.
    ///
    /// Exhaustive on purpose: this list is what the tests below check the
    /// registry against in both directions, so a subcommand missing from it is a
    /// subcommand nobody proved resolves to anything.
    const ARGV: &[&[&[&str]]] = &[
        agent::ARGV,
        app::ARGV,
        backlog::ARGV,
        browser::ARGV,
        filter::ARGV,
        host::ARGV,
        project::ARGV,
        worktree::ARGV,
    ];

    /// Every entry of [`ARGV`], whichever module it is in.
    fn argv() -> impl Iterator<Item = &'static [&'static str]> {
        ARGV.iter().flat_map(|list| list.iter().copied())
    }

    /// A context with everything a built-in command can ask to have filled in.
    fn selection() -> command::Context {
        command::Context {
            project: Some(ket_core::id::ProjectId::new("proj-a")),
            worktree: Some(WorktreeId::new("wt-1")),
            ..command::Context::default()
        }
    }

    pub(crate) fn invocation_for(argv: &[&str]) -> Invocation {
        Cli::try_parse_from(argv)
            .unwrap_or_else(|e| panic!("{argv:?}: {e}"))
            .command
            .invocation()
    }

    #[test]
    fn the_cli_implements_every_command_in_the_registry() {
        // The single-source-of-truth check. A command core knows about and the
        // CLI does not implement is a hole in `ket commands` output, which is
        // supposed to be a list of things that work.
        let registry = registry().expect("every bind names a registered command");
        let unbound = registry.unbound();
        assert!(unbound.is_empty(), "no implementation for {unbound:?}");
    }

    #[test]
    fn every_subcommand_resolves_to_a_registered_command() {
        let registry = registry().unwrap();

        for argv in argv() {
            let invocation = invocation_for(argv);
            registry
                .resolve(&invocation, &selection())
                .unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        }
    }

    #[test]
    fn every_command_in_the_registry_is_reachable_from_argv() {
        let registry = registry().unwrap();

        let reached: std::collections::BTreeSet<String> = argv()
            .map(|argv| invocation_for(argv).id.as_str().to_owned())
            .collect();

        let unreachable: Vec<&str> = registry
            .list()
            .map(|command| command.id.as_str())
            .filter(|id| !reached.contains(*id))
            .collect();

        assert!(
            unreachable.is_empty(),
            "no argv reaches {unreachable:?}; a command nobody can run is not a command"
        );
    }

    #[test]
    fn a_switch_that_changes_what_happens_selects_a_different_command() {
        assert_eq!(invocation_for(&["ket", "ps"]).id.as_str(), "session.list");
        assert_eq!(
            invocation_for(&["ket", "ps", "--reap"]).id.as_str(),
            "session.reap"
        );
        assert_eq!(invocation_for(&["ket", "events"]).id.as_str(), "event.list");
        assert_eq!(
            invocation_for(&["ket", "events", "--follow"]).id.as_str(),
            "event.follow"
        );
    }

    #[test]
    fn a_switch_that_changes_rendering_stays_an_argument() {
        let invocation = invocation_for(&["ket", "diff", "wt-1", "--stat"]);
        assert_eq!(invocation.id.as_str(), "worktree.diff");
        assert!(invocation.is_set("stat"));
    }

    #[test]
    fn an_unnamed_worktree_comes_from_the_selection() {
        let registry = registry().unwrap();
        let resolved = registry
            .resolve(
                &invocation_for(&["ket", "worktree", "status"]),
                &selection(),
            )
            .unwrap();

        assert_eq!(resolved.text_of("worktree"), Some("wt-1"));
    }

    #[test]
    fn a_named_worktree_wins_over_the_selection() {
        let registry = registry().unwrap();
        let resolved = registry
            .resolve(
                &invocation_for(&["ket", "worktree", "status", "wt-other"]),
                &selection(),
            )
            .unwrap();

        assert_eq!(resolved.text_of("worktree"), Some("wt-other"));
    }

    #[test]
    fn an_unnamed_worktree_with_nothing_selected_is_an_error_not_a_panic() {
        let registry = registry().unwrap();
        let result = registry.resolve(
            &invocation_for(&["ket", "worktree", "status"]),
            &command::Context::default(),
        );
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn a_named_worktree_wins_over_the_agents_own_for_the_browser() {
        let invocation = invocation_for(&["ket", "browser", "tabs", "--worktree", "wt-2"]);
        assert_eq!(invocation.id.as_str(), "browser.tabs");
        assert_eq!(invocation.text_of("worktree"), Some("wt-2"));
    }

    #[test]
    fn destructive_commands_still_demand_a_name() {
        // `rm` and `collapse` throw work away. Both refuse to be parsed without
        // an explicit worktree, so neither can act on wherever you happen to be.
        assert!(Cli::try_parse_from(["ket", "worktree", "rm"]).is_err());
        assert!(Cli::try_parse_from(["ket", "collapse"]).is_err());
    }
}
