//! The fallback transport: an agent driven through a pseudo-terminal.
//!
//! Deliberately first-class rather than an afterthought. ACP coverage across
//! agents is uneven and moves, and an agent ket cannot run at all is worse than
//! an agent ket runs without structured tool calls.
//!
//! What this transport gives up is *observation*, not correctness. A byte stream
//! cannot tell you whether the agent is waiting on the model or running your
//! test suite, so a PTY session reports [`AgentState::Thinking`] for the whole
//! turn and never [`AgentState::ExecutingTool`]. Events are the same shape as
//! ACP's; there are simply fewer of them. Nothing downstream branches on which
//! transport produced them.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::mpsc;

use super::{SessionContext, SessionResult, timed_out};
use crate::KetError;
use crate::config::AgentSpec;
use crate::event::{AgentState, SessionOutcome};

/// Size the pty is opened at.
///
/// Agents that render boxes and progress bars wrap to this. The browser drives
/// real resizes in Epic 6; until there is a terminal on screen there is nothing
/// to take a size from, and 80×24 is what every tool assumes when asked nothing.
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;

/// Bytes read from the pty per call.
const READ_BUFFER: usize = 8 * 1024;

/// Output chunks buffered between the reader thread and the async driver.
///
/// Bounded, so a burst of output applies backpressure to the reader thread
/// rather than growing without limit.
const OUTPUT_QUEUE: usize = 256;

/// Runs one prompt through a pty, streaming output as events.
///
/// Returns when the agent process exits, when `cancel` fires, or when the agent
/// has produced nothing for long enough to count as stalled.
pub async fn run(spec: &AgentSpec, ctx: &Arc<SessionContext>, prompt: &str) -> SessionResult {
    let mut cancel = ctx.cancel_signal();
    let pair = {
        // See `crate::pty_lock` for why this has to be a lock shared with
        // `Terminal::open`: a terminal pane and an agent session can open their
        // ptys at the same instant, and only a lock the two share stops that
        // from racing.
        let _serialised = crate::pty_lock::lock();
        native_pty_system().openpty(PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
    }
    .map_err(|e| agent_error(spec, format!("could not open a pty: {e}")))?;

    let mut command = CommandBuilder::new(&spec.command);
    command.args(&spec.args);
    command.cwd(&ctx.worktree);
    for key in &spec.env_remove {
        command.env_remove(key);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| agent_error(spec, format!("could not spawn {}: {e}", spec.command)))?;

    // The slave handle must go before the child's output can ever reach EOF:
    // while ket holds it open, the master keeps reading an empty stream forever
    // and the session never ends on its own.
    drop(pair.slave);

    let mut killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| agent_error(spec, format!("could not read the pty: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| agent_error(spec, format!("could not write to the pty: {e}")))?;

    // There is no handshake to wait for: a pty agent is usable the moment it
    // exists. ACP's `Starting → Authenticating → Idle` collapses to one step.
    ctx.transition(AgentState::Idle);

    let (tx, mut rx) = mpsc::channel::<String>(OUTPUT_QUEUE);
    std::thread::spawn(move || pump(reader, &tx));

    writeln!(writer, "{prompt}")
        .and_then(|()| writer.flush())
        .map_err(|e| agent_error(spec, format!("could not send the prompt: {e}")))?;

    ctx.transition(AgentState::Thinking);

    // A pty cannot say which state it is in, so the only honest bound is on
    // *silence*, and it resets on every chunk: output is evidence the agent is
    // alive. The limit is the tool one rather than the thinking one, because an
    // agent that is quietly running a test suite must not be killed as though it
    // were a stalled model request — the distinction ACP can make and this
    // transport cannot.
    let idle_limit = Duration::from_secs(ctx.timeouts.executing_tool_secs);

    loop {
        tokio::select! {
            biased;

            () = cancel.cancelled() => {
                let _ = killer.kill();
                ctx.transition(AgentState::Idle);
                return Ok(SessionOutcome::Cancelled);
            }

            received = tokio::time::timeout(idle_limit, rx.recv()) => match received {
                Err(_elapsed) => {
                    let _ = killer.kill();
                    return Err(timed_out(&ctx.id, ctx.state(), idle_limit));
                }
                // EOF: the reader thread saw the pty close, so the agent is done.
                Ok(None) => break,
                Ok(Some(chunk)) => ctx.output(chunk),
            },
        }
    }

    ctx.transition(AgentState::Idle);

    let status = tokio::task::spawn_blocking(move || child.wait())
        .await
        .map_err(|e| agent_error(spec, format!("waiting for the agent panicked: {e}")))?
        .map_err(|e| agent_error(spec, format!("could not wait for the agent: {e}")))?;

    if status.success() {
        Ok(SessionOutcome::Completed)
    } else {
        Ok(SessionOutcome::Failed {
            why: format!("{} exited with {status:?}", spec.name),
        })
    }
}

/// Reads the pty until EOF, forwarding decoded text.
///
/// Runs on its own thread because `portable-pty` is a blocking API. Sends on a
/// bounded channel, so when the consumer falls behind this thread blocks instead
/// of buffering the world.
fn pump(mut reader: Box<dyn Read + Send>, tx: &mpsc::Sender<String>) {
    let mut buffer = [0u8; READ_BUFFER];
    let mut chunker = Utf8Chunker::default();

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if let Some(text) = chunker.push(&buffer[..n])
                    && tx.blocking_send(text).is_err()
                {
                    // The session ended. Nothing is listening.
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!(%e, "pty read ended");
                break;
            }
        }
    }
}

/// Turns a byte stream into text without splitting characters across reads.
///
/// A read boundary lands wherever the kernel put it, which for an agent drawing
/// box characters or emoji is regularly in the middle of one. Decoding each
/// buffer independently would turn those into replacement characters at
/// unpredictable places.
#[derive(Debug, Default)]
struct Utf8Chunker {
    /// Bytes of a character split across reads. Never more than three.
    tail: Vec<u8>,
}

impl Utf8Chunker {
    /// Decodes what it can, keeping any truncated trailing character.
    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.tail.extend_from_slice(bytes);
        let mut out = String::new();

        loop {
            match std::str::from_utf8(&self.tail) {
                Ok(text) => {
                    out.push_str(text);
                    self.tail.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if let Ok(text) = std::str::from_utf8(&self.tail[..valid]) {
                        out.push_str(text);
                    }

                    match e.error_len() {
                        // Genuinely invalid — a raw escape byte, say. Mark it and
                        // carry on rather than discarding everything after it.
                        Some(len) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.tail.drain(..valid + len);
                        }
                        // Truncated: the rest of this character is in the next read.
                        None => {
                            self.tail.drain(..valid);
                            break;
                        }
                    }
                }
            }
        }

        (!out.is_empty()).then_some(out)
    }
}

/// Builds an [`KetError::Agent`] naming the agent that failed.
fn agent_error(spec: &AgentSpec, why: String) -> KetError {
    KetError::Agent {
        agent: spec.name.clone(),
        why,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_straight_through() {
        let mut chunker = Utf8Chunker::default();
        assert_eq!(chunker.push(b"hello").as_deref(), Some("hello"));
    }

    #[test]
    fn a_character_split_across_reads_is_reassembled() {
        // The failure this prevents: an agent drawing a progress bar emits
        // replacement characters at random points, purely because of where the
        // kernel happened to end a read.
        let snowman = "☃".as_bytes();
        let mut chunker = Utf8Chunker::default();

        assert_eq!(chunker.push(&snowman[..1]), None);
        assert_eq!(chunker.push(&snowman[1..2]), None);
        assert_eq!(chunker.push(&snowman[2..]).as_deref(), Some("☃"));
    }

    #[test]
    fn text_before_a_split_character_is_emitted_immediately() {
        // Holding back the whole buffer because its last character is truncated
        // would stall the stream one read behind, which on a slow agent is
        // visible as output arriving in the wrong rhythm.
        let mut chunker = Utf8Chunker::default();
        let mut bytes = b"done ".to_vec();
        bytes.push("é".as_bytes()[0]);

        assert_eq!(chunker.push(&bytes).as_deref(), Some("done "));
        assert_eq!(chunker.push(&["é".as_bytes()[1]]).as_deref(), Some("é"));
    }

    #[test]
    fn invalid_bytes_are_marked_rather_than_swallowing_the_rest() {
        let mut chunker = Utf8Chunker::default();
        let text = chunker.push(b"a\xffb").expect("some output");

        assert!(text.starts_with('a') && text.ends_with('b'), "{text:?}");
        assert!(text.contains(char::REPLACEMENT_CHARACTER), "{text:?}");
    }

    #[test]
    fn the_held_back_tail_can_never_grow() {
        // Otherwise a stream of invalid bytes is an unbounded buffer.
        let mut chunker = Utf8Chunker::default();
        for _ in 0..1000 {
            chunker.push(b"\xff\xfe\xfd");
        }
        assert!(
            chunker.tail.len() < 4,
            "tail grew to {}",
            chunker.tail.len()
        );
    }
}
