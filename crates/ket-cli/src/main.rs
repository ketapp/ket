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

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use std::sync::Arc;

use ket_core::backlog::{Backlog, Done, Note, normalised_tags, repo_dir};
use ket_core::command::{self, Category, Invocation, Registry};
use ket_core::config::{Config, Transport};
use ket_core::event::{Envelope, Event, SessionOutcome};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::journal::Journal;
use ket_core::status::{ChangeKind, WorktreeStatus};
use ket_core::workspace::{CollapseOptions, MergeError, Workspace};
use ket_core::{KetError, logging, paths};

#[derive(Parser, Debug)]
#[command(
    name = "ket",
    version = ket_core::build_info::display(),
    about = "Run coding agents in parallel across isolated git worktrees"
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

    /// List agent sessions across every project.
    Ps {
        /// Drop finished and abandoned records instead of listing them.
        #[arg(long)]
        reap: bool,
    },
}

#[derive(Subcommand, Debug)]
enum HostCmd {
    /// Say whether a host is running for this data directory, and what in.
    Status,
    /// Ask the host to exit. It only does if no terminal is running in it.
    Stop {
        /// End every terminal running in it, then exit. Through the host's
        /// socket, so it works where signals to the host do not arrive.
        #[arg(long)]
        force: bool,
    },
    /// Print a one-time code for pairing a phone, good for ten minutes.
    Pair,
    /// List the phones paired with this Mac.
    Devices,
    /// Stop accepting a phone, and disconnect it if it is connected.
    Revoke {
        /// The phone's id, from `ket host devices`.
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Print the resolved configuration, defaults included.
    Show,
    /// Print the path ket reads configuration from.
    Path,
}

#[derive(Subcommand, Debug)]
enum ProjectCmd {
    /// Register the repository containing a path.
    Add {
        /// Any path inside the repository. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// List registered projects, most recently used first.
    List,
    /// Deregister a project. The repository itself is untouched.
    Rm {
        /// Project id or name.
        project: String,
        /// Remove even while ket-managed worktrees remain.
        #[arg(long)]
        force: bool,
    },
    /// Change how a project is shown and what it defaults to.
    Set {
        /// Project id or name.
        project: String,
        /// Name shown in the sidebar. An empty string restores the directory name.
        #[arg(long)]
        name: Option<String>,
        /// Badge colour as `#rrggbb`. An empty string restores the default.
        #[arg(long)]
        color: Option<String>,
        /// Icon, stored verbatim. An empty string clears it.
        #[arg(long)]
        icon: Option<String>,
        /// Branch new worktrees are based on.
        #[arg(long)]
        base: Option<String>,
        /// Agent used when none is named. An empty string clears it.
        #[arg(long)]
        agent: Option<String>,
        /// Whether worktrees ket did not create are shown: `hide` or `show`.
        #[arg(long, value_parser = ["hide", "show"])]
        discovered: Option<String>,
        /// Whether committed hooks and post-command may run: `trust` or `deny`.
        #[arg(long, value_parser = ["trust", "deny"])]
        automation: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum BacklogCmd {
    /// Add a note to a project's backlog.
    Add {
        /// What the list shows, and the first line an agent is told.
        title: String,
        /// The description: what an agent is told, after the title.
        #[arg(long, short)]
        body: Option<String>,
        /// A tag to find it by. Repeat it for more than one.
        #[arg(long = "tag", short, value_name = "TAG")]
        tags: Vec<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// List a project's backlog notes, most pressing first.
    List {
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
        /// Include notes already marked done.
        #[arg(long)]
        all: bool,
        /// Only the notes with this tag.
        #[arg(long, short)]
        tag: Option<String>,
        /// Only the notes holding every word of this, in the title, the
        /// description or a tag. `#word` asks for a tag.
        #[arg(long, short)]
        search: Option<String>,
    },
    /// Add tags to a note, or take them off.
    Tag {
        /// Note id, from `ket backlog list`.
        note: String,
        /// The tags. A leading `#` is dropped.
        #[arg(required = true)]
        tags: Vec<String>,
        /// Take these tags off instead.
        #[arg(long)]
        remove: bool,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Move a note in the list: before another, or to the end of its
    /// priority.
    ///
    /// Moved before a note of another priority, it takes that priority.
    Move {
        /// Note id, from `ket backlog list`.
        note: String,
        /// The note to put it before. Without it, the note goes to the end of
        /// its priority.
        #[arg(long)]
        before: Option<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Keep a project's backlog privately or in its repository.
    ///
    /// `repo` keeps it in `.ket/backlog/` in the repository, a file per note,
    /// to be committed and shared with a team; `private` keeps it in ket's
    /// data directory, as a new project does. The notes and their files are
    /// moved across, and a note on both sides keeps its latest edit.
    Store {
        /// `private` or `repo`.
        #[arg(value_parser = ["private", "repo"])]
        location: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Mark a note done by hand — for work that happened somewhere else.
    Done {
        /// Note id, from `ket backlog list`.
        note: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Put a done note back among the open ones.
    Reopen {
        /// Note id, from `ket backlog list --all`.
        note: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Delete a note and its files.
    Rm {
        /// Note id, from `ket backlog list`.
        note: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum WorktreeCmd {
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

fn main() -> ExitCode {
    // The CLI can be the ket host too — see `ket_core::host`.
    ket_core::host::serve_if_asked();

    let cli = Cli::parse();
    logging::init(cli.log.as_deref().unwrap_or("info"));

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

            Cmd::Host { cmd } => match cmd {
                HostCmd::Status => Invocation::new("host.status"),
                HostCmd::Stop { force } => Invocation::new("host.stop").flag("force", force),
                HostCmd::Pair => Invocation::new("host.pair"),
                HostCmd::Devices => Invocation::new("host.devices"),
                HostCmd::Revoke { id } => Invocation::new("host.revoke").text("id", id),
            },

            Cmd::Commands {
                applicable,
                category,
            } => Invocation::new("app.commands")
                .flag("applicable", applicable)
                .maybe_text("category", category),

            Cmd::Config { cmd } => match cmd {
                ConfigCmd::Show => Invocation::new("config.show"),
                ConfigCmd::Path => Invocation::new("config.path"),
            },

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

            Cmd::Diff { worktree, stat } => Invocation::new("worktree.diff")
                .maybe_text("worktree", worktree)
                .flag("stat", stat),

            Cmd::Events { follow } => {
                Invocation::new(if follow { "event.follow" } else { "event.list" })
            }

            Cmd::Project { cmd } => match cmd {
                ProjectCmd::Add { path } => {
                    Invocation::new("project.add").text("path", path.to_string_lossy())
                }
                ProjectCmd::List => Invocation::new("project.list"),
                ProjectCmd::Rm { project, force } => Invocation::new("project.remove")
                    .text("project", project)
                    .flag("force", force),
                ProjectCmd::Set {
                    project,
                    name,
                    color,
                    icon,
                    base,
                    agent,
                    discovered,
                    automation,
                } => Invocation::new("project.settings.set")
                    .text("project", project)
                    .maybe_text("name", name)
                    .maybe_text("color", color)
                    .maybe_text("icon", icon)
                    .maybe_text("base", base)
                    .maybe_text("agent", agent)
                    .maybe_text("discovered", discovered)
                    .maybe_text("automation", automation),
            },

            Cmd::Backlog { cmd } => match cmd {
                BacklogCmd::Add {
                    title,
                    body,
                    tags,
                    project,
                } => Invocation::new("backlog.add")
                    .maybe_text("project", project)
                    .text("title", title)
                    .maybe_text("body", body)
                    .maybe_text("tags", (!tags.is_empty()).then(|| lines(&tags))),
                BacklogCmd::List {
                    project,
                    all,
                    tag,
                    search,
                } => Invocation::new("backlog.list")
                    .maybe_text("project", project)
                    .flag("all", all)
                    .maybe_text("tag", tag)
                    .maybe_text("search", search),
                BacklogCmd::Done { note, project } => Invocation::new("backlog.done")
                    .maybe_text("project", project)
                    .text("note", note),
                BacklogCmd::Reopen { note, project } => Invocation::new("backlog.reopen")
                    .maybe_text("project", project)
                    .text("note", note),
                BacklogCmd::Rm { note, project } => Invocation::new("backlog.remove")
                    .maybe_text("project", project)
                    .text("note", note),
                BacklogCmd::Tag {
                    note,
                    tags,
                    remove,
                    project,
                } => Invocation::new("backlog.tag")
                    .maybe_text("project", project)
                    .text("note", note)
                    .text("tags", lines(&tags))
                    .flag("remove", remove),
                BacklogCmd::Move {
                    note,
                    before,
                    project,
                } => Invocation::new("backlog.move")
                    .maybe_text("project", project)
                    .text("note", note)
                    .maybe_text("before", before),
                BacklogCmd::Store { location, project } => Invocation::new("backlog.store")
                    .maybe_text("project", project)
                    .text("location", location),
            },

            Cmd::Worktree { cmd } => match cmd {
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
            },

            Cmd::Run { agent, on, prompt } => Invocation::new("agent.run")
                .maybe_text("worktree", on)
                .text("prompt", prompt)
                .maybe_text("agent", agent),

            Cmd::Ps { reap } => Invocation::new(if reap { "session.reap" } else { "session.list" }),
        }
    }
}

/// Core's catalogue of commands, with this crate's implementations attached.
///
/// `bind` refuses an id core does not know, so a typo here fails the test below
/// rather than leaving a subcommand that quietly stops working; and
/// `Registry::unbound` coming back empty is what proves the CLI implements
/// everything the palette will offer. Both are asserted in `tests`.
fn registry() -> Result<Registry, KetError> {
    let mut registry = Registry::builtin();

    registry.bind("agent.run", |invocation, _| {
        run_agent(
            invocation.text_of("agent"),
            invocation.require_text("worktree")?,
            invocation.require_text("prompt")?,
        )
    })?;
    registry.bind("app.commands", |invocation, context| {
        list_commands(
            invocation.is_set("applicable"),
            invocation.text_of("category"),
            context,
        )
    })?;
    registry.bind("app.doctor", |_, _| doctor())?;

    // The host's commands are the CLI's alone: from inside the app, stopping
    // the process that runs its terminals is not a thing to offer.
    registry.register(command::Command::new(
        "host.status",
        "Show whether the ket host is running",
        Category::Application,
    ))?;
    registry.register(
        command::Command::new(
            "host.stop",
            "Stop the ket host, if nothing is running in it",
            Category::Application,
        )
        .arg(command::Arg::flag("force")),
    )?;
    registry.register(command::Command::new(
        "host.pair",
        "Print a one-time code for pairing a phone",
        Category::Application,
    ))?;
    registry.register(command::Command::new(
        "host.devices",
        "List the phones paired with this Mac",
        Category::Application,
    ))?;
    registry.register(
        command::Command::new(
            "host.revoke",
            "Stop accepting a phone, and disconnect it",
            Category::Application,
        )
        .arg(command::Arg::required("id", command::ArgKind::Text)),
    )?;
    registry.bind("host.devices", |_, _| {
        let devices = ket_core::host::devices()?;
        if devices.is_empty() {
            println!("no phones paired — `ket host pair` makes a code");
        }
        for device in devices {
            println!(
                "{}  {}{}",
                device.id,
                if device.name.is_empty() {
                    "(unnamed)"
                } else {
                    &device.name
                },
                if device.connected {
                    "  · connected"
                } else {
                    ""
                }
            );
        }
        Ok(())
    })?;
    registry.bind("host.revoke", |invocation, _| {
        let id = invocation.require_text("id")?;
        if ket_core::host::revoke(id)? {
            println!("revoked");
            Ok(())
        } else {
            Err(KetError::Conflict(format!("no phone {id} is paired")))
        }
    })?;
    registry.bind("host.pair", |_, _| {
        println!("{}", ket_core::host::pairing_code()?);
        Ok(())
    })?;
    registry.bind("host.status", |_, _| {
        match ket_core::host::status()? {
            Some(status) => println!(
                "running: {} terminal(s)\nbuild:   {}\nsocket:  {}",
                status.live,
                status.build,
                ket_core::host::socket_path()?.display()
            ),
            None => println!("not running"),
        }
        Ok(())
    })?;
    registry.bind("host.stop", |invocation, _| {
        let force = invocation.is_set("force");
        match ket_core::host::status()? {
            None => println!("not running"),
            Some(status) if status.live > 0 && !force => {
                return Err(KetError::Conflict(format!(
                    "the host has {} terminal(s) running; it stays until they end \
                     (--force ends them)",
                    status.live
                )));
            }
            Some(status) => {
                ket_core::host::stop(force)?;
                match status.live {
                    0 => println!("stopped"),
                    n => println!("stopped, ending {n} terminal(s)"),
                }
            }
        }
        Ok(())
    })?;
    registry.bind("config.path", |_, _| {
        println!("{}", paths::config_file()?.display());
        Ok(())
    })?;
    registry.bind("config.show", |_, _| show_config())?;
    registry.bind("event.follow", |_, _| follow_events(true))?;
    registry.bind("event.list", |_, _| follow_events(false))?;
    registry.bind("project.add", |invocation, _| {
        add_project(invocation.text_of("path").unwrap_or("."))
    })?;
    registry.bind("project.list", |_, _| list_projects())?;
    registry.bind("project.remove", |invocation, _| {
        remove_project(
            invocation.require_text("project")?,
            invocation.is_set("force"),
        )
    })?;
    registry.bind("project.settings.set", |invocation, _| {
        set_project_settings(invocation)
    })?;
    registry.bind("backlog.add", |invocation, _| {
        backlog_add(
            invocation.text_of("project"),
            invocation.require_text("title")?,
            invocation.text_of("body"),
            invocation.text_of("tags"),
        )
    })?;
    registry.bind("backlog.list", |invocation, _| {
        backlog_list(
            invocation.text_of("project"),
            invocation.is_set("all"),
            invocation.text_of("tag"),
            invocation.text_of("search"),
        )
    })?;
    registry.bind("backlog.done", |invocation, _| {
        backlog_done(
            invocation.text_of("project"),
            invocation.require_text("note")?,
        )
    })?;
    registry.bind("backlog.reopen", |invocation, _| {
        backlog_reopen(
            invocation.text_of("project"),
            invocation.require_text("note")?,
        )
    })?;
    registry.bind("backlog.remove", |invocation, _| {
        backlog_remove(
            invocation.text_of("project"),
            invocation.require_text("note")?,
        )
    })?;
    registry.bind("backlog.tag", |invocation, _| {
        backlog_tag(
            invocation.text_of("project"),
            invocation.require_text("note")?,
            invocation.require_text("tags")?,
            invocation.is_set("remove"),
        )
    })?;
    registry.bind("backlog.move", |invocation, _| {
        backlog_move(
            invocation.text_of("project"),
            invocation.require_text("note")?,
            invocation.text_of("before"),
        )
    })?;
    registry.bind("backlog.store", |invocation, _| {
        backlog_store(
            invocation.text_of("project"),
            invocation.require_text("location")?,
        )
    })?;
    registry.bind("session.list", |_, _| list_sessions())?;
    registry.bind("session.reap", |_, _| reap_sessions())?;
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

/// Handles `ket commands`.
///
/// Both self-documenting and a check that the registry is the single source of
/// truth: this prints the same table the palette renders and a keybinding file
/// resolves against, because there is only one of them. `!` marks a command that
/// needs a project or a worktree nothing has selected — it still runs if you
/// name one.
fn list_commands(
    applicable_only: bool,
    category: Option<&str>,
    context: &command::Context,
) -> Result<(), KetError> {
    // Listing needs the catalogue, not the handlers, and handing the registry a
    // reference to itself so that `app.commands` could read it is not worth
    // arranging for that.
    let registry = Registry::builtin();

    let wanted = match category {
        Some(name) => Some(Category::parse(name).ok_or_else(|| {
            KetError::Conflict(format!(
                "unknown category {name}; try one of: {}",
                Category::ALL
                    .iter()
                    .map(Category::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?),
        None => None,
    };

    let mut shown = 0usize;

    for command in registry.list() {
        if wanted.is_some_and(|wanted| wanted != command.category) {
            continue;
        }

        let applies = command.applies_in(context);
        if applicable_only && !applies {
            continue;
        }

        let mark = if applies { ' ' } else { '!' };
        println!(
            "{mark} {:<20} {:<12} {}",
            command.id, command.category, command.title
        );

        let signature = command.signature();
        if !signature.is_empty() {
            println!("{:36}{signature}", "");
        }

        shown += 1;
    }

    println!("\n{shown} command(s)");
    Ok(())
}

/// Handles `ket run`.
///
/// Streams the session's events as they happen rather than printing a transcript
/// at the end: watching an agent work is most of what makes running three of
/// them in parallel bearable.
fn run_agent(agent: Option<&str>, worktree: &str, prompt: &str) -> Result<(), KetError> {
    let agent = agent.map(str::to_owned);
    let worktree = worktree.to_owned();

    let prompt = if prompt == "-" {
        std::io::read_to_string(std::io::stdin())
            .map_err(|e| KetError::io("stdin", e))?
            .trim()
            .to_owned()
    } else {
        prompt.to_owned()
    };

    let runtime = tokio::runtime::Runtime::new().map_err(|e| KetError::io("tokio runtime", e))?;

    runtime.block_on(async move {
        let workspace = Arc::new(Workspace::open()?);
        let id = WorktreeId::new(worktree);

        let events = workspace.bus().subscribe();
        let printer = tokio::spawn(render(events, Arc::clone(&workspace)));

        let mut session = tokio::spawn({
            let workspace = Arc::clone(&workspace);
            let agent = agent.clone();
            async move { workspace.run_agent(&id, agent.as_deref(), &prompt).await }
        });

        let outcome = tokio::select! {
            finished = &mut session => finished,
            _ = tokio::signal::ctrl_c() => {
                // Ask, then still wait for the session to wind down. Dropping
                // the task here would leave an agent process running with
                // nothing watching it.
                eprintln!("\nket: stopping the agent...");
                workspace.cancel_all();
                (&mut session).await
            }
        }
        .map_err(|e| KetError::Agent {
            agent: agent.unwrap_or_else(|| "agent".to_owned()),
            why: format!("session task failed: {e}"),
        })??;

        printer.abort();

        match outcome {
            SessionOutcome::Completed => {
                println!("\ndone");
                Ok(())
            }
            SessionOutcome::Cancelled => {
                eprintln!("\nket: cancelled");
                Ok(())
            }
            SessionOutcome::Failed { why } => Err(KetError::Agent {
                agent: "session".to_owned(),
                why,
            }),
        }
    })
}

/// Prints a session's events, answering permission requests on the terminal.
async fn render(mut events: tokio::sync::broadcast::Receiver<Envelope>, workspace: Arc<Workspace>) {
    use std::io::Write as _;

    loop {
        let envelope = match events.recv().await {
            Ok(envelope) => envelope,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                eprintln!("ket: dropped {missed} event(s); the agent is outrunning the terminal");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };

        match envelope.event {
            // Output arrives in chunks that do not end on line boundaries, so
            // it is written and flushed rather than printed as lines.
            Event::AgentOutput { chunk, .. } => {
                print!("{chunk}");
                let _ = std::io::stdout().flush();
            }

            Event::AgentStateChanged { to, .. } => {
                tracing::debug!(?to, "agent state");
            }

            Event::AgentPermissionRequested {
                session_id,
                request_id,
                tool,
                title,
            } => {
                let allowed = ask(&tool, &title).await;
                workspace.answer_permission(&session_id, &request_id, allowed);
            }

            Event::AgentPermissionResolved {
                allowed, by_policy, ..
            } if by_policy => {
                eprintln!(
                    "ket: {} by policy",
                    if allowed { "allowed" } else { "denied" }
                );
            }

            _ => {}
        }
    }
}

/// Asks the terminal whether an agent may proceed.
///
/// Anything but an explicit yes is a no. A permission prompt answered by a
/// stray newline should not grant anything.
async fn ask(tool: &str, title: &str) -> bool {
    eprintln!("\nket: the agent wants to {tool}: {title}");
    eprint!("ket: allow? [y/N] ");

    let line = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await;

    matches!(
        line.map(|r| r.map(|l| l.trim().to_ascii_lowercase())),
        Ok(Ok(answer)) if answer == "y" || answer == "yes"
    )
}

/// Handles `ket ps --reap`.
fn reap_sessions() -> Result<(), KetError> {
    let dropped = Workspace::open()?.reap_sessions()?;
    println!("reaped {dropped} session record(s)");
    Ok(())
}

/// Handles `ket ps`.
fn list_sessions() -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let sessions = workspace.sessions(None)?;
    if sessions.is_empty() {
        eprintln!("ket: no sessions");
        return Ok(());
    }

    for session in sessions {
        println!(
            "{:<28} {:<10} {:<28} {:?}",
            session.id, session.agent, session.worktree_id, session.status
        );
    }
    Ok(())
}

/// The current directory, or `/` if it cannot be determined.
///
/// A deleted or unreadable cwd is not worth failing a command over — project
/// resolution simply falls through to recency.
fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// Handles `ket project ...`.
fn add_project(path: &str) -> Result<(), KetError> {
    let project = Workspace::open()?.add_project(std::path::Path::new(path))?;
    println!("{}  {}", project.id, project.root.display());
    Ok(())
}

/// Handles `ket project list`.
fn list_projects() -> Result<(), KetError> {
    let workspace = Workspace::open()?;

    let projects = workspace.projects()?;
    if projects.is_empty() {
        eprintln!("ket: no projects registered; run `ket project add`");
        return Ok(());
    }

    let worktrees = workspace.worktrees(None)?;
    for project in projects {
        let count = worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .count();
        // Availability is worth showing: projects outlive their directories, and
        // a moved repo should be visible, not silent.
        let mark = if project.is_available() { ' ' } else { '!' };
        println!(
            "{mark} {:<28} {:<3} {}",
            project.id,
            count,
            project.root.display()
        );
    }
    Ok(())
}

/// Handles `ket project rm`.
fn remove_project(project: &str, force: bool) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let resolved = workspace.resolve_project(Some(project), &cwd())?;
    workspace.remove_project(&resolved.id, force)?;
    println!("removed {}", resolved.id);
    Ok(())
}

/// The note `id` refers to, or a clear error — `Backlog`'s own mutators treat
/// an unknown id as a no-op, which would otherwise have the CLI print a
/// success message for nothing happening.
fn find_note(project: &ProjectId, id: &str) -> Result<(), KetError> {
    let backlog = Backlog::load(project)?;
    if backlog.notes.iter().any(|note| note.id == id) {
        Ok(())
    } else {
        Err(KetError::Conflict(format!("no backlog note {id}")))
    }
}

/// Several values as one registry argument, a line each — an argument is
/// text or a switch, and a tag never holds a line break.
fn lines(values: &[String]) -> String {
    values.join("\n")
}

/// A note's tags as `list` prints them after its title: `  #one #two`, or
/// nothing.
fn tag_list(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let tags: Vec<String> = tags.iter().map(|tag| format!("#{tag}")).collect();
    format!("  {}", tags.join(" "))
}

/// Handles `ket backlog add`.
fn backlog_add(
    project: Option<&str>,
    title: &str,
    body: Option<&str>,
    tags: Option<&str>,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let existing = Backlog::load(&resolved.id)?;
    let mut note = Note::new(&existing.notes);
    note.title = title.to_owned();
    note.body = body.unwrap_or_default().to_owned();
    note.tags = tags.map_or_else(Vec::new, |tags| normalised_tags(tags.lines()));
    Backlog::save_note(&resolved.id, &note)?;
    println!("{}  {}{}", note.id, note.title, tag_list(&note.tags));
    Ok(())
}

/// Handles `ket backlog list`.
///
/// In the order the backlog is shown — most pressing first, then as moved —
/// and the done notes after, most recently done first.
fn backlog_list(
    project: Option<&str>,
    all: bool,
    tag: Option<&str>,
    search: Option<&str>,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::load(&resolved.id)?;
    // `--tag` is a `#tag` in the search, spelt the way a tag is kept.
    let mut query = search.unwrap_or_default().to_owned();
    for tag in normalised_tags(tag) {
        query.push_str(" #");
        query.push_str(&tag);
    }
    let done = if all {
        backlog.sorted_done()
    } else {
        Vec::new()
    };
    let notes: Vec<&Note> = backlog
        .sorted_open()
        .into_iter()
        .chain(done)
        .filter(|note| note.matches(&query))
        .collect();

    if notes.is_empty() {
        if query.trim().is_empty() {
            eprintln!("ket: no backlog notes; run `ket backlog add \"<title>\"`");
        } else {
            eprintln!("ket: no backlog notes match");
        }
        return Ok(());
    }
    for note in notes {
        let mark = if note.done.is_some() { 'x' } else { ' ' };
        println!(
            "{mark} {:<10} {}{}",
            note.id,
            note.title,
            tag_list(&note.tags)
        );
    }
    Ok(())
}

/// Handles `ket backlog tag`.
fn backlog_tag(
    project: Option<&str>,
    note: &str,
    tags: &str,
    remove: bool,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    let given = normalised_tags(tags.lines());
    let backlog = Backlog::update(&resolved.id, |backlog| {
        let Some(saved) = backlog.notes.iter_mut().find(|saved| saved.id == note) else {
            return;
        };
        let tags = if remove {
            let gone: Vec<String> = given.iter().map(|tag| tag.to_lowercase()).collect();
            let mut kept = saved.tags.clone();
            kept.retain(|tag| !gone.contains(&tag.to_lowercase()));
            kept
        } else {
            normalised_tags(saved.tags.iter().chain(&given))
        };
        if tags != saved.tags {
            saved.tags = tags;
            saved.updated_ms = ket_core::now_ms();
        }
    })?;
    let kept = backlog
        .notes
        .iter()
        .find(|saved| saved.id == note)
        .map(|saved| saved.tags.clone())
        .unwrap_or_default();
    if !remove {
        let folded: Vec<String> = kept.iter().map(|tag| tag.to_lowercase()).collect();
        if given
            .iter()
            .any(|tag| !folded.contains(&tag.to_lowercase()))
        {
            eprintln!(
                "ket: a note keeps at most {} tags; the rest were not added",
                ket_core::backlog::MAX_TAGS
            );
        }
    }
    if kept.is_empty() {
        println!("{note}  no tags");
    } else {
        println!("{note}{}", tag_list(&kept));
    }
    Ok(())
}

/// Handles `ket backlog move`.
fn backlog_move(project: Option<&str>, note: &str, before: Option<&str>) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::reorder(&resolved.id, note, before)?;
    let priority = backlog
        .notes
        .iter()
        .find(|saved| saved.id == note)
        .map(|saved| saved.priority.name())
        .unwrap_or_default();
    match before {
        Some(other) => println!("moved {note} before {other}, in {priority}"),
        None => println!("moved {note} to the end of {priority}"),
    }
    Ok(())
}

/// Handles `ket backlog store`.
fn backlog_store(project: Option<&str>, location: &str) -> Result<(), KetError> {
    let in_repo = match location {
        "repo" => true,
        "private" => false,
        other => {
            return Err(KetError::Config(format!(
                "store must be `private` or `repo`, not {other:?}"
            )));
        }
    };
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::set_location(&resolved.id, in_repo)?;
    let count = backlog.notes.len();
    if in_repo {
        println!(
            "{count} note(s) in {} — commit it to share it",
            repo_dir(&resolved.root).display()
        );
    } else {
        println!("{count} note(s), kept privately");
    }
    Ok(())
}

/// Handles `ket backlog done`.
fn backlog_done(project: Option<&str>, note: &str) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    Backlog::mark_done(&resolved.id, note, Done::by_hand())?;
    println!("done {note}");
    Ok(())
}

/// Handles `ket backlog reopen`.
fn backlog_reopen(project: Option<&str>, note: &str) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    Backlog::reopen(&resolved.id, note)?;
    println!("reopened {note}");
    Ok(())
}

/// Handles `ket backlog rm`.
fn backlog_remove(project: Option<&str>, note: &str) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    Backlog::remove(&resolved.id, note)?;
    println!("removed {note}");
    Ok(())
}

/// Handles `ket project set`.
///
/// Presentation fields go through one `ProjectSettings` write; `base` and
/// `agent` have their own setters because they are checked against the
/// repository and the config respectively, and a failed check must not
/// half-apply the rest. Presentation is written first for the same reason —
/// it cannot fail on anything but the store itself.
fn set_project_settings(invocation: &Invocation) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let resolved = workspace.resolve_project(Some(invocation.require_text("project")?), &cwd())?;

    let mut settings = workspace.project_settings(&resolved.id)?;
    let mut touched = false;
    // An explicit empty string clears a field; an absent flag leaves it alone.
    let mut apply = |slot: &mut Option<String>, value: Option<&str>| {
        if let Some(value) = value {
            *slot = Some(value.to_owned());
            touched = true;
        }
    };
    apply(&mut settings.display_name, invocation.text_of("name"));
    apply(&mut settings.color, invocation.text_of("color"));
    apply(&mut settings.icon, invocation.text_of("icon"));
    match invocation.text_of("discovered") {
        Some("hide") => {
            settings.hide_discovered_worktrees = true;
            touched = true;
        }
        Some("show") => {
            settings.hide_discovered_worktrees = false;
            touched = true;
        }
        Some(other) => {
            return Err(KetError::Config(format!(
                "discovered must be `hide` or `show`, not {other:?}"
            )));
        }
        None => {}
    }
    if touched {
        workspace.set_project_settings(&resolved.id, settings)?;
    }

    if let Some(base) = invocation.text_of("base") {
        workspace.set_default_base(&resolved.id, base)?;
    }
    if let Some(agent) = invocation.text_of("agent") {
        let agent = (!agent.trim().is_empty()).then_some(agent);
        workspace.set_preferred_agent(&resolved.id, agent)?;
    }
    match invocation.text_of("automation") {
        Some("trust") => workspace.set_automation_trusted(&resolved.id, true)?,
        Some("deny") => workspace.set_automation_trusted(&resolved.id, false)?,
        Some(other) => {
            return Err(KetError::Config(format!(
                "automation must be `trust` or `deny`, not {other:?}"
            )));
        }
        None => {}
    }

    let project = workspace.resolve_project(Some(resolved.id.as_str()), &cwd())?;
    let settings = workspace.project_settings(&project.id)?;
    println!(
        "{}  name={}  color={}  icon={}  base={}  agent={}  discovered={}  automation={}",
        project.id,
        settings.name_for(&project),
        settings.color.as_deref().unwrap_or("-"),
        settings.icon.as_deref().unwrap_or("-"),
        project.default_base,
        project.preferred_agent.as_deref().unwrap_or("-"),
        if settings.hide_discovered_worktrees {
            "hide"
        } else {
            "show"
        },
        if workspace.automation_trusted(&project.id)? {
            "trust"
        } else {
            "deny"
        },
    );
    Ok(())
}

/// Handles `ket worktree add`.
///
/// `project` arrives already decided — named on the command line, or filled in
/// by the registry from wherever the shell is standing.
fn create_worktree(
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
fn provision_worktree(worktree: &str) -> Result<(), KetError> {
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
fn list_worktrees(project: Option<&str>) -> Result<(), KetError> {
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
fn worktree_status(worktree: &str) -> Result<(), KetError> {
    let status = Workspace::open()?.worktree_status(&WorktreeId::new(worktree))?;
    print_status(&status);
    Ok(())
}

/// Handles `ket worktree open`.
fn open_worktree(worktree: &str) -> Result<(), KetError> {
    let id = WorktreeId::new(worktree);
    Workspace::open()?.open_worktree(&id)?;
    println!("opened {id}");
    Ok(())
}

/// Handles `ket worktree close`.
fn close_worktree(worktree: &str) -> Result<(), KetError> {
    let id = WorktreeId::new(worktree);
    Workspace::open()?.close_worktree(&id)?;
    println!("closed {id}");
    Ok(())
}

/// Handles `ket worktree rm`.
fn remove_worktree(worktree: &str, force: bool, delete_branch: bool) -> Result<(), KetError> {
    let id = WorktreeId::new(worktree);
    Workspace::open()?.remove_worktree(&id, force, delete_branch)?;
    println!("removed {id}");
    Ok(())
}

/// Handles `ket worktree prune`.
fn prune_worktrees() -> Result<(), KetError> {
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
fn warn_about_submodules(paths: &[String]) {
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
fn change_char(kind: ChangeKind) -> char {
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
fn print_status(status: &WorktreeStatus) {
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

/// Prints the resolved configuration as TOML.
fn show_config() -> Result<(), KetError> {
    let config = Config::load()?;
    let text = toml_of(&config)?;
    print!("{text}");
    Ok(())
}

/// Renders a config back to TOML.
fn toml_of(config: &Config) -> Result<String, KetError> {
    toml::to_string_pretty(config).map_err(|e| KetError::Config(e.to_string()))
}

/// Locates an executable on `PATH` without running it.
///
/// Deliberately does not spawn the command to test it: `ket doctor` should be
/// safe to run at any time, and executing three different agent binaries to see
/// whether they exist is not that.
fn find_on_path(command: &str) -> Option<PathBuf> {
    if command.contains(std::path::MAIN_SEPARATOR) {
        let direct = PathBuf::from(command);
        return direct.is_file().then_some(direct);
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        candidate.is_file().then_some(candidate)
    })
}

/// Reports on everything ket needs in order to work.
///
/// This directly serves an Epic 3 adversarial case — "ACP adapter not installed,
/// `node` missing" — by making that condition visible before an agent run fails
/// halfway through with a confusing error.
fn doctor() -> Result<(), KetError> {
    let config = Config::load()?;
    let mut problems = 0usize;

    println!("paths");
    println!("  config   {}", paths::config_file()?.display());
    println!("  data     {}", paths::data_dir()?.display());
    println!("  worktrees {}", config.worktrees_dir()?.display());

    println!("\nrequired");
    problems += report("git", find_on_path("git"), true);

    println!("\nagents");
    for agent in &config.agents {
        let disabled = config.agent.disabled.contains(&agent.name);
        let found = find_on_path(&agent.command);
        let label = format!("{} ({:?})", agent.name, agent.transport);
        problems += report(&label, found, false);

        // ACP agents are launched through a runner; without it the transport
        // cannot work at all, so surface that separately.
        if agent.transport == Transport::Acp && agent.command == "npx" {
            problems += report(
                "  ↳ node (for the ACP adapter)",
                find_on_path("node"),
                false,
            );
        }

        // The adapter above is only half the story: a pane types the agent's
        // *interactive* command at a shell prompt, which may be an alias or a
        // function rather than anything on `PATH`. Asked of the same shell the
        // pane will use, so this line answers the question a pane answers.
        let launch = agent.launch_command();
        if launch != agent.command {
            let word = launch.split_whitespace().next().unwrap_or(launch);
            problems += report(
                &format!("  ↳ {word} (interactive command)"),
                ket_core::shell::probe(word),
                false,
            );
        }

        if disabled {
            println!("  · {:<28} switched off in settings", agent.name);
        }
    }

    if problems == 0 {
        println!("\nall good");
        Ok(())
    } else {
        println!("\n{problems} problem(s) found");
        Err(KetError::Config(format!(
            "{problems} dependency check(s) failed"
        )))
    }
}

/// Prints one doctor line, returning 1 when it represents a problem.
fn report(label: &str, found: Option<PathBuf>, required: bool) -> usize {
    match found {
        Some(path) => {
            println!("  ✓ {label:<28} {}", path.display());
            0
        }
        None if required => {
            println!("  ✗ {label:<28} not found on PATH");
            1
        }
        None => {
            println!("  ! {label:<28} not found on PATH");
            1
        }
    }
}

/// Handles `ket collapse`.
fn collapse_worktree(
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
fn merge_worktree(worktree: &str, force: bool) -> Result<(), KetError> {
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

/// Handles `ket diff`.
///
/// Writes to stdout in `git diff` format so the output pipes into everything
/// that already reads diffs — `delta`, `git apply`, a pager — rather than
/// inventing a format nothing else understands.
fn diff_worktree(worktree: &str, stat: bool) -> Result<(), KetError> {
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

/// How often `--follow` looks for new lines in the journal.
///
/// Fast enough to feel live while watching an agent, slow enough that idling
/// on it costs nothing worth measuring.
const FOLLOW_POLL: std::time::Duration = std::time::Duration::from_millis(150);

/// Streams the event journal as JSON lines.
///
/// Reads the journal on disk rather than subscribing to a bus, because the bus
/// is a `tokio::sync::broadcast` channel that reaches only its own process —
/// and the whole point of this command is watching a `ket run` in *another*
/// terminal. See `ket_core::journal`.
///
/// Without `--follow` it prints what the journal holds and exits. With it, it
/// starts at the end and streams what happens next, which is what you want when
/// the question is "what is my agent doing right now". Ctrl-C leaves at any
/// time: invariant 4, and a stream you cannot leave is the first place that
/// rule gets broken.
fn follow_events(follow: bool) -> Result<(), KetError> {
    use std::io::{Seek, SeekFrom};

    let path = paths::events_file()?;

    if !follow {
        for envelope in Journal::at(&path).read_all() {
            match serde_json::to_string(&envelope) {
                Ok(line) => println!("{line}"),
                Err(e) => tracing::error!(%e, "failed to serialise envelope"),
            }
        }
        return Ok(());
    }

    // Start at the end: `--follow` is for what happens next.
    let mut offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    eprintln!("ket: following {} — Ctrl-C to exit", path.display());

    loop {
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        // A shrinking file means the journal rotated; the lines we had not read
        // are in the rotated generation, and starting over beats skipping ahead
        // into the middle of a line.
        if len < offset {
            offset = 0;
        }

        if len > offset {
            match std::fs::File::open(&path) {
                Ok(mut file) => {
                    if file.seek(SeekFrom::Start(offset)).is_ok() {
                        let mut text = String::new();
                        if std::io::Read::read_to_string(&mut file, &mut text).is_ok() {
                            // Only whole lines: a partial final line means
                            // something is mid-append, and it will be complete
                            // by the next poll.
                            let complete = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
                            for line in text[..complete].lines() {
                                println!("{line}");
                            }
                            offset += complete as u64;
                        }
                    }
                }
                Err(e) => tracing::debug!(%e, "event journal not readable yet"),
            }
        }

        std::thread::sleep(FOLLOW_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every way of reaching a command from argv.
    ///
    /// Exhaustive on purpose: this list is what the tests below check the
    /// registry against in both directions, so a subcommand missing from it is a
    /// subcommand nobody proved resolves to anything.
    const ARGV: &[&[&str]] = &[
        &["ket", "doctor"],
        &["ket", "commands"],
        &["ket", "commands", "--applicable", "--category", "worktree"],
        &["ket", "config", "show"],
        &["ket", "config", "path"],
        &[
            "ket",
            "collapse",
            "wt-1",
            "--force",
            "--keep",
            "--keep-losers",
        ],
        &["ket", "merge", "wt-1", "--force"],
        &["ket", "diff", "wt-1"],
        &["ket", "diff", "--stat"],
        &["ket", "events"],
        &["ket", "events", "--follow"],
        &["ket", "project", "add"],
        &["ket", "project", "list"],
        &["ket", "project", "rm", "proj-a", "--force"],
        &[
            "ket",
            "project",
            "set",
            "proj-a",
            "--name",
            "Ready Set Reading",
            "--color",
            "#eab308",
            "--discovered",
            "hide",
        ],
        &["ket", "backlog", "add", "fix the thing"],
        &[
            "ket",
            "backlog",
            "add",
            "fix the thing",
            "--body",
            "details",
        ],
        &["ket", "backlog", "list"],
        &["ket", "backlog", "list", "--all"],
        &["ket", "backlog", "done", "note-1"],
        &["ket", "backlog", "reopen", "note-1"],
        &["ket", "backlog", "rm", "note-1"],
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
        &["ket", "run", "claude", "--prompt", "fix the flaky test"],
        &["ket", "ps"],
        &["ket", "ps", "--reap"],
        &["ket", "host", "status"],
        &["ket", "host", "stop"],
        &["ket", "host", "stop", "--force"],
        &["ket", "host", "pair"],
        &["ket", "host", "devices"],
        &["ket", "host", "revoke", "phone-1"],
    ];

    /// A context with everything a built-in command can ask to have filled in.
    fn selection() -> command::Context {
        command::Context {
            project: Some(ket_core::id::ProjectId::new("proj-a")),
            worktree: Some(WorktreeId::new("wt-1")),
            ..command::Context::default()
        }
    }

    fn invocation_for(argv: &[&str]) -> Invocation {
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

        for argv in ARGV {
            let invocation = invocation_for(argv);
            registry
                .resolve(&invocation, &selection())
                .unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        }
    }

    #[test]
    fn every_command_in_the_registry_is_reachable_from_argv() {
        let registry = registry().unwrap();

        let reached: std::collections::BTreeSet<String> = ARGV
            .iter()
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
    fn destructive_commands_still_demand_a_name() {
        // `rm` and `collapse` throw work away. Both refuse to be parsed without
        // an explicit worktree, so neither can act on wherever you happen to be.
        assert!(Cli::try_parse_from(["ket", "worktree", "rm"]).is_err());
        assert!(Cli::try_parse_from(["ket", "collapse"]).is_err());
    }
}
