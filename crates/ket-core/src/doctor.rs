//! `ket doctor`: everything ket needs in order to work, and whether it is
//! there.
//!
//! Here rather than in the CLI so the same report goes into an exported
//! diagnostics bundle — see [`crate::diagnostics`].

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::config::{Config, Transport};
use crate::{Result, paths};

/// What [`run`] found.
#[derive(Debug, Clone)]
pub struct Report {
    /// The report as `ket doctor` prints it, ending in a verdict line.
    pub text: String,
    /// How many checks failed.
    pub problems: usize,
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
pub fn run() -> Result<Report> {
    let config = Config::load()?;
    let mut out = String::new();
    let mut problems = 0usize;

    let _ = writeln!(out, "paths");
    let _ = writeln!(out, "  config   {}", paths::config_file()?.display());
    let _ = writeln!(out, "  data     {}", paths::data_dir()?.display());
    let _ = writeln!(out, "  worktrees {}", config.worktrees_dir()?.display());

    let _ = writeln!(out, "\nrequired");
    problems += line(&mut out, "git", find_on_path("git"), true);

    let _ = writeln!(out, "\nagents");
    for agent in &config.agents {
        let disabled = config.agent.disabled.contains(&agent.name);
        let found = find_on_path(&agent.command);
        let label = format!("{} ({:?})", agent.name, agent.transport);
        problems += line(&mut out, &label, found, false);

        // ACP agents are launched through a runner; without it the transport
        // cannot work at all, so surface that separately.
        if agent.transport == Transport::Acp && agent.command == "npx" {
            problems += line(
                &mut out,
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
            problems += line(
                &mut out,
                &format!("  ↳ {word} (interactive command)"),
                crate::shell::probe(word),
                false,
            );
        }

        if disabled {
            let _ = writeln!(out, "  · {:<28} switched off in settings", agent.name);
        }
    }

    if problems == 0 {
        let _ = writeln!(out, "\nall good");
    } else {
        let _ = writeln!(out, "\n{problems} problem(s) found");
    }
    Ok(Report {
        text: out,
        problems,
    })
}

/// Writes one doctor line, returning 1 when it represents a problem.
fn line(out: &mut String, label: &str, found: Option<PathBuf>, required: bool) -> usize {
    match found {
        Some(path) => {
            let _ = writeln!(out, "  ✓ {label:<28} {}", path.display());
            0
        }
        None if required => {
            let _ = writeln!(out, "  ✗ {label:<28} not found on PATH");
            1
        }
        None => {
            let _ = writeln!(out, "  ! {label:<28} not found on PATH");
            1
        }
    }
}
