//! ket itself: its command list, `doctor`, diagnostics, configuration, the
//! event journal, and taking ket out of the agents.

use clap::Subcommand;
use ket_core::command::{self, Category, Invocation, Registry};
use ket_core::config::Config;
use ket_core::journal::Journal;
use ket_core::{KetError, paths};

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    registry.bind("app.commands", |invocation, context| {
        list_commands(
            invocation.is_set("applicable"),
            invocation.text_of("category"),
            context,
        )
    })?;

    registry.bind("app.doctor", |_, _| doctor())?;

    // The CLI's alone: in the app it is the Help menu's, behind a save panel.
    registry.register(
        command::Command::new(
            "app.diagnostics",
            "Zip logs and versions into a file for a bug report",
            Category::Application,
        )
        .arg(command::Arg::optional("output", command::ArgKind::Path)),
    )?;

    registry.bind("app.diagnostics", |invocation, _| {
        let output = invocation
            .text_of("output")
            .unwrap_or(ket_core::diagnostics::DEFAULT_NAME);
        ket_core::diagnostics::export(std::path::Path::new(output))?;
        println!(
            "wrote {output} — attach it to an issue or email it to {}",
            ket_core::diagnostics::SUPPORT_EMAIL
        );
        Ok(())
    })?;

    // The CLI's alone, like the host's: the app has its own button for this in
    // Settings, and from the palette it would be one keystroke from undoing
    // what the window just installed.
    registry.register(command::Command::new(
        "app.unhook",
        "Take ket's hooks out of every agent, for uninstalling it",
        Category::Application,
    ))?;

    registry.bind("app.unhook", |_, _| uninstall_hooks())?;

    registry.bind("config.path", |_, _| {
        println!("{}", paths::config_file()?.display());
        Ok(())
    })?;

    registry.bind("config.show", |_, _| show_config())?;

    registry.bind("event.follow", |_, _| follow_events(true))?;

    registry.bind("event.list", |_, _| follow_events(false))?;

    Ok(())
}

impl ConfigCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            ConfigCmd::Show => Invocation::new("config.show"),
            ConfigCmd::Path => Invocation::new("config.path"),
        }
    }
}

#[derive(Subcommand, Debug)]
pub(super) enum ConfigCmd {
    /// Print the resolved configuration, defaults included.
    Show,
    /// Print the path ket reads configuration from.
    Path,
}

/// Handles `ket commands`.
///
/// Both self-documenting and a check that the registry is the single source of
/// truth: this prints the same table the palette renders and a keybinding file
/// resolves against, because there is only one of them. `!` marks a command that
/// needs a project or a worktree nothing has selected — it still runs if you
/// name one.
pub(super) fn list_commands(
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

/// Prints the resolved configuration as TOML.
pub(super) fn show_config() -> Result<(), KetError> {
    let config = Config::load()?;
    let text = toml_of(&config)?;
    print!("{text}");
    Ok(())
}

/// Renders a config back to TOML.
pub(super) fn toml_of(config: &Config) -> Result<String, KetError> {
    toml::to_string_pretty(config).map_err(|e| KetError::Config(e.to_string()))
}

/// Takes ket out of every agent and prints a line for each place it looked.
///
/// A place it could not change is still a success for the others, so every
/// line is printed first and the failure is the exit status: the cask that
/// runs this must not stop an uninstall over it, and says so itself.
pub(super) fn uninstall_hooks() -> Result<(), KetError> {
    use ket_core::agent_hooks::Removal;
    let removed = ket_core::agent_hooks::remove_everywhere();
    let mut failed = 0;
    for entry in &removed {
        let place = match entry.detail.as_deref() {
            Some(detail) => format!("{} ({detail})", entry.agent),
            None => entry.agent.clone(),
        };
        match &entry.outcome {
            Removal::Removed => println!("{place}: removed"),
            Removal::NotThere => println!("{place}: not there"),
            Removal::Failed(why) => {
                failed += 1;
                println!("{place}: could not change it: {why}");
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(KetError::Conflict(format!(
            "{n} agent setting(s) still have ket in them"
        ))),
    }
}

/// Prints what `ket doctor` found, failing when anything is missing.
pub(super) fn doctor() -> Result<(), KetError> {
    let report = ket_core::doctor::run()?;
    print!("{}", report.text);
    if report.problems == 0 {
        Ok(())
    } else {
        Err(KetError::Config(format!(
            "{} dependency check(s) failed",
            report.problems
        )))
    }
}

/// How often `--follow` looks for new lines in the journal.
///
/// Fast enough to feel live while watching an agent, slow enough that idling
/// on it costs nothing worth measuring.
pub(super) const FOLLOW_POLL: std::time::Duration = std::time::Duration::from_millis(150);

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
pub(super) fn follow_events(follow: bool) -> Result<(), KetError> {
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

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "doctor"],
    &["ket", "diagnostics"],
    &["ket", "diagnostics", "--output", "/tmp/report.zip"],
    &["ket", "commands"],
    &["ket", "commands", "--applicable", "--category", "worktree"],
    &["ket", "config", "show"],
    &["ket", "config", "path"],
    &["ket", "events"],
    &["ket", "events", "--follow"],
    &["ket", "uninstall-hooks"],
];
