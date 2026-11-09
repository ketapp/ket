//! Running an agent, starting work, and the session list.

use ket_core::KetError;
use ket_core::command::{self, Category, Registry};
use ket_core::event::{Envelope, Event, SessionOutcome};
use ket_core::id::WorktreeId;
use ket_core::workspace::Workspace;
use std::sync::Arc;

use crate::cwd;

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    registry.bind("agent.run", |invocation, _| {
        run_agent(
            invocation.text_of("agent"),
            invocation.require_text("worktree")?,
            invocation.require_text("prompt")?,
        )
    })?;

    // Starting work in the window is the CLI's alone as well: inside the app,
    // the quick prompt and the backlog's Start already do it.
    registry.register(
        command::Command::new(
            "work.start",
            "Start work in a new worktree, as a tab in the ket window",
            Category::Agent,
        )
        .arg(command::Arg::optional("project", command::ArgKind::Project))
        .arg(command::Arg::required("prompt", command::ArgKind::Text))
        .arg(command::Arg::optional("agent", command::ArgKind::Text)),
    )?;

    registry.bind("work.start", |invocation, _| {
        start_work(
            invocation.text_of("project"),
            invocation.require_text("prompt")?,
            invocation.text_of("agent"),
        )
    })?;

    registry.bind("session.list", |_, _| list_sessions())?;

    registry.bind("session.reap", |_, _| reap_sessions())?;

    Ok(())
}

/// Handles `ket run`.
///
/// Streams the session's events as they happen rather than printing a transcript
/// at the end: watching an agent work is most of what makes running three of
/// them in parallel bearable.
pub(super) fn run_agent(agent: Option<&str>, worktree: &str, prompt: &str) -> Result<(), KetError> {
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

/// Handles `ket ps --reap`.
pub(super) fn reap_sessions() -> Result<(), KetError> {
    let dropped = Workspace::open()?.reap_sessions()?;
    println!("reaped {dropped} session record(s)");
    Ok(())
}

/// Handles `ket ps`.
pub(super) fn list_sessions() -> Result<(), KetError> {
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

/// Handles `ket start`: the prompt to the open window, which makes the
/// worktree and starts its agent in a tab.
pub(super) fn start_work(
    project: Option<&str>,
    prompt: &str,
    agent: Option<&str>,
) -> Result<(), KetError> {
    let prompt = if prompt == "-" {
        std::io::read_to_string(std::io::stdin()).map_err(|e| KetError::io("stdin", e))?
    } else {
        prompt.to_owned()
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(KetError::Conflict("say what the agent is to do".to_owned()));
    }
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    ket_core::host::start_work(resolved.id.as_str(), prompt, agent, None)?;
    println!(
        "starting in {} — `ket worktree list` shows the worktree once ket has made it",
        resolved.name
    );
    Ok(())
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

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "start", "fix the flaky test"],
    &[
        "ket",
        "start",
        "fix the flaky test",
        "--agent",
        "codex",
        "--project",
        "ket",
    ],
    &["ket", "run", "claude", "--prompt", "fix the flaky test"],
    &["ket", "ps"],
    &["ket", "ps", "--reap"],
];
