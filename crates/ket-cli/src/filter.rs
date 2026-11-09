//! `ket filter` and `ket recall`: the hook that trims a command's output.

use clap::Args;
use ket_core::KetError;
use ket_core::command::{self, Category, Invocation, Registry};

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    // The filter's commands are the CLI's alone too: they print for whoever
    // ran them, and there is nobody in a palette to print to.
    registry.register(
        command::Command::new(
            "shell.filter",
            "Run a command and print its output trimmed",
            Category::Agent,
        )
        .arg(command::Arg::required("command", command::ArgKind::Text))
        .arg(command::Arg::flag("keep")),
    )?;

    registry.register(
        command::Command::new(
            "shell.recall",
            "Print the full output the filter trimmed",
            Category::Agent,
        )
        .arg(command::Arg::required("id", command::ArgKind::Text)),
    )?;

    registry.bind("shell.filter", |invocation, _| {
        let code = ket_core::filter::run(
            invocation.require_text("command")?,
            invocation.is_set("keep"),
        );
        if code == 0 {
            Ok(())
        } else {
            std::process::exit(code)
        }
    })?;

    registry.bind("shell.recall", |invocation, _| {
        print!(
            "{}",
            ket_core::filter::recall(invocation.require_text("id")?)?
        );
        Ok(())
    })?;

    Ok(())
}

/// The shell command `ket filter` runs, as the words after `--`.
#[derive(Args, Debug)]
pub(super) struct FilterArgs {
    /// Keep the full output for `ket recall`. Otherwise nothing is written.
    #[arg(long)]
    keep: bool,
    /// The command, run through your login shell. Exits with its status.
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

impl FilterArgs {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        Invocation::new("shell.filter")
            .flag("keep", self.keep)
            .text(
                "command",
                self.command
                    .iter()
                    .map(|word| ket_core::shell::quote(word))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
    }
}

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "filter", "--keep", "--", "cargo", "build"],
    &["ket", "recall", "a1b2c3"],
];
