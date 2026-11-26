//! The listener the hook script posts to, and the prompts it holds open
//! until someone answers them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use super::decode::{GrokTools, decode_envelope, permission_question};
use super::script::new_token;
use super::{HookEvent, HookReport, PermissionQuestion};
use crate::{KetError, Result};

// ---- the listener -----------------------------------------------------------

/// Receives hook reports on loopback.
///
/// A hand-rolled listener rather than an HTTP crate: this speaks to a script
/// ket itself wrote, the entire protocol is "one POST with a JSON body", and
/// an HTTP stack is a large dependency and a large attack surface for that.
/// It binds to `127.0.0.1:0` — loopback so nothing off this machine can reach
/// it, port zero so ket never fights another process for a fixed number.
pub struct Listener {
    /// The port to hand agents in [`PORT_ENV`].
    port: u16,
    /// Reports as they arrive.
    reports: std::sync::mpsc::Receiver<HookReport>,
    /// Status line payloads as they arrive, kept apart from the reports
    /// because they answer a different question — see [`ClaudeStatusLine`] —
    /// and are read by a different part of the app.
    statuslines: std::sync::mpsc::Receiver<StatusLine>,
    /// Questions and plans whose hooks are waiting to be answered.
    waiters: std::sync::Arc<Waiters>,
    /// Dropped to stop the accept loop.
    _shutdown: Shutdown,
}

/// Closes the listener when the [`Listener`] goes.
struct Shutdown(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Listener {
    /// Starts listening, and tells you which port to hand out.
    ///
    /// `token` is minted by the caller and must appear in a report for it to be
    /// believed: loopback keeps other machines out, not other processes.
    pub fn start(token: String) -> Result<Self> {
        Self::start_with_auth(HookAuth::Legacy(token))
    }

    /// Starts the host listener with credentials bound to individual panes.
    pub fn start_bound(bindings: HookBindings) -> Result<Self> {
        Self::start_with_auth(HookAuth::Bound(bindings))
    }

    fn start_with_auth(auth: HookAuth) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| KetError::io("127.0.0.1:0", e))?;
        let port = listener
            .local_addr()
            .map_err(|e| KetError::io("127.0.0.1:0", e))?
            .port();
        // So the accept loop can notice it has been told to stop.
        listener
            .set_nonblocking(true)
            .map_err(|e| KetError::io("127.0.0.1:0", e))?;

        let (send, reports) = std::sync::mpsc::channel();
        let (send_status, statuslines) = std::sync::mpsc::channel();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stopped.clone();
        let waiters = std::sync::Arc::new(Waiters::default());
        let parked = waiters.clone();

        std::thread::Builder::new()
            .name("ket-agent-hooks".to_owned())
            .spawn(move || {
                let mut grok_tools = GrokTools::new();
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => match read_post(stream, &auth, &mut grok_tools) {
                            Some(Post::Report(report)) => {
                                if send.send(*report).is_err() {
                                    return;
                                }
                            }
                            Some(Post::StatusLine(payload)) => {
                                if send_status.send(payload).is_err() {
                                    return;
                                }
                            }
                            Some(Post::Await(pane, waiter)) => parked.park(pane, waiter),
                            None => {}
                        },
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(40));
                        }
                        // A failed accept is one lost report, not a reason to
                        // stop reporting for the rest of the session.
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
                    }
                }
            })
            .map_err(|e| KetError::io("ket-agent-hooks", e))?;

        Ok(Self {
            port,
            reports,
            statuslines,
            waiters,
            _shutdown: Shutdown(stopped),
        })
    }

    /// The port agents should post to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The questions and plans held open on this listener, for whatever
    /// answers them.
    pub fn waiters(&self) -> std::sync::Arc<Waiters> {
        self.waiters.clone()
    }

    /// Every report that has arrived since the last call.
    ///
    /// Drained rather than blocking: the shell asks on its own tick, and a
    /// hook must never be able to make the window wait.
    pub fn drain(&self) -> Vec<HookReport> {
        self.reports.try_iter().collect()
    }

    /// Every status line payload that has arrived since the last call.
    ///
    /// Payloads still raw, because this module's job is the wire and not the
    /// meaning of what came over it:
    /// [`crate::rate_limits::snapshot_from_statusline`] is what knows a quota
    /// block when it sees one, and [`crate::usage::from_statusline`] the
    /// session's own numbers.
    pub fn drain_statuslines(&self) -> Vec<StatusLine> {
        self.statuslines.try_iter().collect()
    }
}

/// What arrived on one connection.
pub(super) enum Post {
    /// A lifecycle report from an agent's hook.
    Report(Box<HookReport>),
    /// A status line payload, whole and uninterpreted.
    StatusLine(StatusLine),
    /// A question or a plan's hook, to hold open until it is answered — see
    /// [`Waiters`]. The pane it came from, and the connection.
    Await(String, Waiter),
}

/// Where a script posts a question or a plan it waits on an answer for.
const AWAIT_PATH: &str = "/hook/await";

/// How long a hook just parked is safe from [`Waiters::release_unless`].
///
/// Its report reaches the status store on the host's next tick, a moment
/// after it parks; until then its pane still looks like it is not waiting.
const PARK_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The `PermissionRequest` hooks held open for a question or a plan, by pane.
///
/// Claude Code runs that hook alongside its own dialog and takes whatever
/// decision the hook prints, so a hook that waits is a way to answer the
/// prompt without a single keystroke: the answers themselves, in Claude's
/// words, rather than keys into a menu that changes with the session's mode.
/// A hook that prints nothing leaves the dialog to the terminal.
///
/// Claude does not stop the hook when the prompt is answered at the desktop
/// — checked on 2.1.281 — so letting go is ket's to do, once the pane's
/// status has moved on: see [`Waiters::release_unless`].
#[derive(Default)]
pub struct Waiters {
    parked: std::sync::Mutex<std::collections::HashMap<String, Waiter>>,
}

/// One held hook.
pub struct Waiter {
    stream: std::net::TcpStream,
    /// The prompt, as its report told the status store: matched against the
    /// store's before answering, so an answer cannot reach a later prompt.
    asked: PermissionQuestion,
    /// The tool's input, which the decision hands back with the answers in.
    input: serde_json::Value,
    parked_at: std::time::Instant,
}

impl Waiter {
    /// Hands the hook `body` and closes it. An empty body says nothing, which
    /// the script answers with `{}`.
    fn send(mut self, body: &str) -> bool {
        use std::io::Write;
        let reply = match body.is_empty() {
            true => "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_owned(),
            false => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        };
        let sent = self.stream.write_all(reply.as_bytes()).is_ok();
        let _ = self.stream.flush();
        sent
    }

    /// Whether the script is still there to hear an answer: Claude kills it
    /// when its timeout runs out, and when the session ends.
    fn alive(&self) -> bool {
        if self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut byte = [0_u8; 1];
        let open = match self.stream.peek(&mut byte) {
            Ok(0) => false,
            Ok(_) => true,
            Err(e) => e.kind() == std::io::ErrorKind::WouldBlock,
        };
        let _ = self.stream.set_nonblocking(false);
        open
    }

    /// Whether this is the hook for `question`.
    fn holds(&self, question: &PermissionQuestion) -> bool {
        self.asked.tool == question.tool && self.asked.subject == question.subject
    }
}

impl Waiters {
    /// Holds `waiter` for `pane`, letting go of any older one there: a pane
    /// shows one prompt at a time.
    fn park(&self, pane: String, waiter: Waiter) {
        let old = lock_waiters(&self.parked).insert(pane, waiter);
        if let Some(old) = old {
            old.send("");
        }
    }

    /// Whether no hook is held at all.
    pub fn is_empty(&self) -> bool {
        lock_waiters(&self.parked).is_empty()
    }

    /// Whether `pane`'s hook for `question` is held and still listening —
    /// whether an answer from a phone can reach it.
    pub fn waiting(&self, pane: &str, question: &PermissionQuestion) -> bool {
        lock_waiters(&self.parked)
            .get(pane)
            .is_some_and(|waiter| waiter.holds(question) && waiter.alive())
    }

    /// Answers `pane`'s prompt with `decision`, when its hook for `question`
    /// is held and the decision fits what it asked.
    pub fn resolve(
        &self,
        pane: &str,
        question: &PermissionQuestion,
        decision: &Decision,
    ) -> std::result::Result<(), Unresolved> {
        let mut parked = lock_waiters(&self.parked);
        let waiter = parked
            .get(pane)
            .filter(|waiter| waiter.holds(question) && waiter.alive())
            .ok_or(Unresolved::NotHeld)?;
        let body = decision
            .claude_output(&waiter.input)
            .ok_or(Unresolved::Unfit)?;
        let waiter = parked.remove(pane).ok_or(Unresolved::NotHeld)?;
        match waiter.send(&body) {
            true => Ok(()),
            false => Err(Unresolved::NotHeld),
        }
    }

    /// Lets go of every held hook `keep` does not want, once it has had
    /// [`PARK_GRACE`] to be reported, and of every one whose script has
    /// gone. `keep` is asked with the pane and the prompt the hook holds.
    ///
    /// Returns the pane and prompt of each hook let go because its script
    /// had gone while `keep` still wanted it. Claude kills the script when
    /// the dialog closes, answered or dismissed, and sends no event for a
    /// dismissal — so a dead script under a prompt the status still shows is
    /// the only word that the prompt is no longer up. Hooks let go only
    /// because `keep` said no are not reported: the status already moved.
    pub fn release_unless(
        &self,
        keep: impl Fn(&str, &PermissionQuestion) -> bool,
    ) -> Vec<(String, PermissionQuestion)> {
        let mut parked = lock_waiters(&self.parked);
        if parked.is_empty() {
            return Vec::new();
        }
        let mut dismissed = Vec::new();
        let gone: Vec<String> = parked
            .iter()
            .filter(|(pane, waiter)| {
                if waiter.parked_at.elapsed() < PARK_GRACE {
                    return false;
                }
                let alive = waiter.alive();
                let wanted = keep(pane, &waiter.asked);
                if !alive && wanted {
                    dismissed.push(((*pane).clone(), waiter.asked.clone()));
                }
                !alive || !wanted
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        for pane in gone {
            if let Some(waiter) = parked.remove(&pane) {
                waiter.send("");
            }
        }
        dismissed
    }
}

fn lock_waiters<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why [`Waiters::resolve`] could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    /// No hook is held for that prompt, or it has stopped listening.
    NotHeld,
    /// The decision does not fit what the prompt asked.
    Unfit,
}

/// What a person decided about a [`Prompt`](super::Prompt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// An `AskUserQuestion`'s answers, one per question in order: the labels
    /// picked, joined as Claude joins them, or what was typed instead.
    Answers(Vec<String>),
    /// Go ahead with the plan, in the mode the session had before planning.
    Approve,
    /// Not yet: keep planning, with this to go on.
    KeepPlanning(String),
    /// Which of a screen-read dialog's options to press, counted from zero.
    /// Typed, never handed to a hook — see [`Prompt::Dialog`](super::Prompt::Dialog).
    Pick(usize),
    /// One option for each of Codex's questions, in order, counted from
    /// zero. Typed into its form, never handed to a hook — see
    /// [`CODEX_ASK_TOOL`](super::CODEX_ASK_TOOL).
    Picks(Vec<usize>),
}

impl Decision {
    /// Claude Code's `PermissionRequest` output for this decision about a
    /// prompt raised with `input`, or `None` when it does not fit.
    ///
    /// Allowing either tool takes `updatedInput`: Claude refuses a bare allow
    /// for a tool that needs a person. The input goes back as it came, with
    /// the answers added under each question's own text.
    pub fn claude_output(&self, input: &serde_json::Value) -> Option<String> {
        let decision = match self {
            Self::Answers(answers) => {
                let questions = input.get("questions")?.as_array()?;
                if questions.len() != answers.len() {
                    return None;
                }
                let mut named = serde_json::Map::new();
                for (question, answer) in questions.iter().zip(answers) {
                    let text = question.get("question")?.as_str()?;
                    if answer.trim().is_empty() {
                        return None;
                    }
                    named.insert(text.to_owned(), serde_json::Value::from(answer.as_str()));
                }
                let mut updated = input.clone();
                updated
                    .as_object_mut()?
                    .insert("answers".to_owned(), serde_json::Value::Object(named));
                serde_json::json!({ "behavior": "allow", "updatedInput": updated })
            }
            Self::Approve => {
                input.as_object()?;
                serde_json::json!({ "behavior": "allow", "updatedInput": input })
            }
            Self::KeepPlanning(feedback) => {
                let feedback = feedback.trim();
                if feedback.is_empty() {
                    return None;
                }
                serde_json::json!({ "behavior": "deny", "message": feedback, "interrupt": false })
            }
            // Keys, not a hook's answer.
            Self::Pick(_) | Self::Picks(_) => return None,
        };
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": decision,
            }
        }))
        .ok()
    }
}

/// One status line payload, and which worktree drew it.
///
/// The worktree is carried because two different things are read out of the
/// same payload and they are not both global: the quota in it belongs to the
/// account, but the context and cost in it belong to *that session* — see
/// [`crate::usage::from_statusline`] — and a reading with no worktree on it
/// could only be shown against all of them or none.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusLine {
    /// The worktree the session is in, from `KET_WORKTREE_ID`. `None` for a
    /// session ket launched before it set that, or one launched by hand.
    pub worktree: Option<String>,
    /// Which terminal it was launched into, from [`PANE_ENV`](super::PANE_ENV).
    ///
    /// The worktree alone cannot answer "which of these is spending this":
    /// every pane in a worktree is launched with the same worktree id, so
    /// without this two agents side by side are one reading that flickers
    /// between them. `None` from a session started before ket set the
    /// variable, or by hand in a shell ket did not open.
    pub pane: Option<String>,
    /// Which agent drew it, from `KET_AGENT_NAME`.
    pub agent: Option<String>,
    /// The payload itself, exactly as Claude wrote it.
    pub payload: serde_json::Value,
}

/// Most bytes read from one hook post.
///
/// A tool payload can carry a whole file's contents. ket reads the envelope,
/// not the transcript, and a hook that could make the window allocate without
/// bound is a hook that can take the window down.
const MAX_BODY: usize = 256 * 1024;

/// Per-terminal hook credentials. The token, rather than the hook's claimed
/// pane field, determines which terminal a report can affect.
#[derive(Clone, Default)]
pub struct HookBindings {
    state: Arc<Mutex<HookBindingState>>,
}

#[derive(Default)]
struct HookBindingState {
    panes: HashMap<String, String>,
    sessions: HashMap<String, String>,
}

fn lock_bindings(mutex: &Mutex<HookBindingState>) -> MutexGuard<'_, HookBindingState> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HookBindings {
    /// Issues or resumes a token bound to `pane`.
    pub fn bind(&self, pane: &str, resume_session: Option<&str>) -> String {
        let mut state = lock_bindings(&self.state);
        let token = resume_session
            .and_then(|session| state.sessions.get(session))
            .filter(|token| state.panes.get(*token).is_none_or(|bound| bound == pane))
            .cloned()
            .unwrap_or_else(new_token);
        state.panes.insert(token.clone(), pane.to_owned());
        token
    }

    /// Stops accepting credentials assigned to the closed panes. Session
    /// associations remain for a later explicit resume in this host run.
    pub fn unbind(&self, panes: &[String]) {
        let mut state = lock_bindings(&self.state);
        state
            .panes
            .retain(|_, pane| !panes.iter().any(|closed| closed == pane));
    }

    fn pane(&self, token: &str) -> Option<String> {
        lock_bindings(&self.state).panes.get(token).cloned()
    }

    fn remember_session(&self, token: &str, report: &HookReport) {
        if report.event != HookEvent::SessionStart {
            return;
        }
        let Some(session) = report
            .session
            .as_deref()
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            return;
        };
        let mut state = lock_bindings(&self.state);
        if state.sessions.len() >= 1024 && !state.sessions.contains_key(session) {
            return;
        }
        // First association wins, so later reports cannot move a known
        // session's credential to another pane.
        state
            .sessions
            .entry(session.to_owned())
            .or_insert_with(|| token.to_owned());
    }
}

#[derive(Clone)]
enum HookAuth {
    Legacy(String),
    Bound(HookBindings),
}

/// Checks `token` against `auth`. `Some((pane, token))`, where `pane` is the
/// pane a bound token is verified to belong to, or `None` for a legacy token
/// — which carries no pane concept at all, bound or otherwise. `None`
/// overall when `token` matches neither.
fn authenticate(auth: &HookAuth, token: &str) -> Option<(Option<String>, String)> {
    match auth {
        HookAuth::Legacy(expected) if token == expected => Some((None, token.to_owned())),
        HookAuth::Bound(bindings) => bindings
            .pane(token)
            .map(|pane| (Some(pane), token.to_owned())),
        _ => None,
    }
}

/// Reads one request and turns it into whatever it carried.
fn read_post(
    mut stream: std::net::TcpStream,
    auth: &HookAuth,
    grok_tools: &mut GrokTools,
) -> Option<Post> {
    use std::io::{Read, Write};

    // Accepted from a non-blocking listener, which on macOS makes it
    // non-blocking too: the first read that found nothing waiting would end
    // the request, and a body that came in more than one piece was dropped.
    stream.set_nonblocking(false).ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .ok()?;

    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.len() > MAX_BODY {
                    break;
                }
                // The body starts after the blank line; once we have all of
                // what the headers promised, there is nothing more to wait for.
                if let Some((head, body)) = split_request(&buffer)
                    && body.len() >= content_length(head)
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // A hook that waits for its answer is answered later, by whoever has it.
    if split_request(&buffer).is_some_and(|(head, _)| target(head) == AWAIT_PATH) {
        return awaited(stream, &buffer, auth);
    }

    let parsed = split_request(&buffer).and_then(|(head, body)| {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .map(|envelope| (head, envelope))
    });
    let accepted = parsed.as_ref().and_then(|(_, envelope)| {
        envelope
            .get("token")?
            .as_str()
            .and_then(|t| authenticate(auth, t))
    });
    let refused = parsed
        .as_ref()
        .is_some_and(|(_, envelope)| envelope.is_object() && accepted.is_none());
    let status: &[u8] = if refused {
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"
    } else {
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
    };
    let _ = stream.write_all(status);
    let _ = stream.flush();
    if refused {
        return None;
    }

    let (head, mut envelope) = parsed?;
    let (trusted_pane, token) = accepted?;
    if let Some(pane) = trusted_pane {
        envelope["pane"] = serde_json::Value::String(pane);
    }
    let post = decode_envelope(&envelope, target(head) == "/statusline", grok_tools)?;
    if let (HookAuth::Bound(bindings), Post::Report(report)) = (auth, &post) {
        bindings.remember_session(&token, report);
    }
    Some(post)
}

/// A question or a plan's hook, to hold open — or, when it is not one ket can
/// answer, answered at once with nothing, which leaves the prompt to the
/// terminal.
///
/// Not reported: the script has already reported the same payload to
/// `/hook`, and this is only the wait.
fn awaited(mut stream: std::net::TcpStream, buffer: &[u8], auth: &HookAuth) -> Option<Post> {
    use std::io::Write;
    let parked = (|| {
        let (_, body) = split_request(buffer)?;
        let envelope: serde_json::Value = serde_json::from_slice(body).ok()?;
        // Claude's hook and OpenCode's plugin both wait here; the other
        // agents answer their prompts with keys, not through a hook.
        let agent = envelope.get("agent").and_then(|a| a.as_str())?;
        if !matches!(
            crate::agents::harness(agent).to_ascii_lowercase().as_str(),
            "claude" | "opencode"
        ) {
            return None;
        }
        let token = envelope.get("token")?.as_str()?;
        // Any token `authenticate` recognises may park a wait. A bound one
        // parks it under the pane it belongs to, as `/hook` files its report
        // there: the envelope's own claim would let one pane's hook stand in
        // for another's prompt. A legacy token carries no pane, so it still
        // goes by the claim.
        let (trusted, _) = authenticate(auth, token)?;
        let pane = match trusted {
            Some(pane) => pane,
            None => envelope
                .get("pane")
                .and_then(|p| p.as_str())
                .filter(|p| !p.is_empty())?
                .to_owned(),
        };
        let payload = envelope.get("payload")?;
        if payload.get("hook_event_name").and_then(|e| e.as_str()) != Some("PermissionRequest") {
            return None;
        }
        let asked = permission_question(payload)?;
        asked.prompt.as_ref()?;
        Some((pane, asked, payload.get("tool_input")?.clone()))
    })();
    let Some((pane, asked, input)) = parked else {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
        return None;
    };
    // Held until the answer, with no read timeout left to cut it short.
    let _ = stream.set_read_timeout(None);
    Some(Post::Await(
        pane,
        Waiter {
            stream,
            asked,
            input,
            parked_at: std::time::Instant::now(),
        },
    ))
}

/// The path a request was posted to, or `/hook` when the line cannot be read.
///
/// The default matters: a script from an older ket posts to `/hook` without
/// ket ever having to ask what it is.
fn target(head: &[u8]) -> String {
    String::from_utf8_lossy(head)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/hook")
        .to_owned()
}

/// Splits a request into its headers and whatever body has arrived.
fn split_request(buffer: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = buffer.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some((&buffer[..at], &buffer[at + 4..]))
}

/// The body length the headers promise, or zero when they do not say.
fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0)
}
