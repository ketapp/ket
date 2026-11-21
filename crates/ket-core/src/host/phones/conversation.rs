//! What a phone asks of an agent's conversation: reading what it has said,
//! and a message for one of the sessions its agent view holds.

use std::sync::mpsc;

use ket_remote::proto::{self, envelope::Payload};

use super::places::Places;
use super::requests::{ack, envelope, error};
use super::session::Phone;
use super::typing::Typed;
use super::{Event, lock};
use crate::host::server::Host;

pub(super) fn conversation_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    ask: &proto::GetConversation,
) -> Option<proto::Envelope> {
    let listed = (!ask.session.is_empty())
        .then(|| agent_view_session(host, &phone.phones.places(), ask.terminal_id, &ask.session))
        .flatten();
    Some(envelope(
        id,
        Payload::Conversation(conversation(
            host,
            &phone.phones.places(),
            ask.terminal_id,
            ask.after,
            &ask.session,
            listed.is_some(),
        )),
    ))
}

/// A message for one of the sessions a terminal's agent view holds:
/// that session opened there first, by the keys a person would press,
/// then the message typed — all in the terminal's typing turn, since
/// each step waits for agent view to draw. See `crate::agent_view`.
pub(super) fn agent_session_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    feed: &mpsc::Sender<Event>,
    id: u64,
    open: &proto::OpenAgentSession,
) -> Option<proto::Envelope> {
    if !host.controls(open.terminal_id, phone.holder) {
        return Some(error(
            id,
            proto::ErrorCode::NotController,
            "take control of this terminal first",
        ));
    }
    let Some(terminal) = host.find(open.terminal_id) else {
        return Some(error(id, proto::ErrorCode::NotFound, "no such terminal"));
    };
    // Sent by the session's id, never through agent view's screen:
    // `claude --bg` for a new one, `claude attach` for one it lists.
    let Some(cwd) = agent_view_cwd(host, &phone.phones.places(), open.terminal_id) else {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that terminal isn't in a checkout ket knows",
        ));
    };
    let session = if open.session.is_empty() {
        if open.text.trim().is_empty() {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "say what the new session is to do",
            ));
        }
        None
    } else {
        let Some((found, _)) = agent_view_session(
            host,
            &phone.phones.places(),
            open.terminal_id,
            &open.session,
        ) else {
            return Some(error(
                id,
                proto::ErrorCode::NotFound,
                "agent view doesn't list that session",
            ));
        };
        if found.state == crate::agent_view::SessionState::NeedsYou {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "it's waiting on an answer in agent view on your Mac — answer it there first",
            ));
        }
        Some(found)
    };
    // A new session runs under the account agent view does.
    let config_dir = terminal
        .foreground_group()
        .and_then(crate::agent_view::config_dir_of);
    let feed = feed.clone();
    let job = Typed::AgentView {
        session,
        cwd,
        config_dir,
        text: open.text.clone(),
        done: Box::new(move |opened| {
            let reply = match opened {
                Ok(()) => ack(id),
                Err(why) => Some(error(id, proto::ErrorCode::Unspecified, &why.reason())),
            };
            if let Some(reply) = reply {
                let _ = feed.send(Event::Send(Box::new(reply)));
            }
        }),
    };
    match phone.phones.typing.files(open.terminal_id, terminal, job) {
        Ok(_) => None,
        Err(_) => Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "the Mac is too busy to type that right now",
        )),
    }
}

/// The checkout the agent view terminal `terminal_id` lists sessions for.
fn agent_view_cwd(host: &Host, places: &Places, terminal_id: u64) -> Option<std::path::PathBuf> {
    let key = host.key(terminal_id)?;
    let worktree = key.split('|').next()?;
    Some(places.get(worktree)?.path.clone())
}

/// Background session `session`, if the agent view terminal `terminal_id`
/// shows lists it for its worktree's checkout, with the names of the others
/// it lists.
fn agent_view_session(
    host: &Host,
    places: &Places,
    terminal_id: u64,
    session: &str,
) -> Option<(crate::agent_view::BackgroundSession, Vec<String>)> {
    let key = host.key(terminal_id)?;
    let worktree = key.split('|').next()?;
    let place = places.get(worktree)?;
    let (wanted, others): (Vec<_>, Vec<_>) = crate::agent_view::sessions(&place.path)
        .into_iter()
        .partition(|listed| listed.session == session);
    let wanted = wanted.into_iter().next()?;
    Some((wanted, others.into_iter().map(|other| other.name).collect()))
}

/// The background session Claude has terminal `terminal_id` attached to,
/// and its transcript — see [`crate::agent_view::attached_name`]. Its own
/// process is the daemon's and reports nothing to the terminal, whose own
/// session is one nothing was ever said in.
fn attached_session(
    host: &Host,
    places: &Places,
    terminal_id: u64,
    key: &str,
) -> Option<(String, crate::conversation::Transcript)> {
    let terminal = host.find(terminal_id)?;
    let rows = terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
    let name = crate::agent_view::attached_name(&rows)?;
    let place = places.get(key.split('|').next()?)?;
    let session = crate::agent_view::named(&place.path, &name)?.session;
    let transcript = crate::conversation::Transcript::locate("claude", &session)?;
    Some((session, transcript))
}

/// What the agent in terminal `terminal_id` has said since cursor `after` of
/// its session — see `crate::conversation`: the session its hooks named for
/// the terminal's pane, for each agent ket can read. `wanted`, when set, is
/// one of the background sessions its agent view lists instead, and
/// `listed` whether it does. A Claude terminal whose own session has nothing
/// to read, attached to one of those sessions, reads that one — see
/// [`attached_session`].
fn conversation(
    host: &Host,
    places: &Places,
    terminal_id: u64,
    after: u64,
    wanted: &str,
    listed: bool,
) -> proto::Conversation {
    let unsupported = |reason: String| proto::Conversation {
        terminal_id,
        supported: false,
        reason,
        ..Default::default()
    };
    let Some(key) = host.key(terminal_id) else {
        return unsupported("that terminal has closed".to_owned());
    };
    // One of the sessions the terminal's agent view holds, which report
    // nothing to the terminal: read by Claude's own id, once it is one the
    // terminal lists, so a phone cannot name any transcript on the Mac.
    if !wanted.is_empty() {
        if !listed {
            return unsupported("agent view doesn't list that session".to_owned());
        }
        let Some(transcript) = crate::conversation::Transcript::locate("claude", wanted) else {
            return unsupported("the session's transcript isn't there yet".to_owned());
        };
        return read_conversation(terminal_id, wanted.to_owned(), &transcript, after);
    }
    // Serialize the lookup with a confirmed Start fresh, so a read that
    // began on the old session cannot put it back after the clear.
    let mut last = lock(&LAST_READ);
    let status = host.agent_status();
    // The agent's own report of which session it is; failing one, the
    // session ket resumed in this terminal when it opened it. OpenCode says
    // nothing on resuming until a message is typed, and a session read from
    // its transcript needs no report.
    let (agent, session) = match status.panes.iter().find(|pane| pane.pane == key) {
        Some(pane) => (pane.agent.clone(), pane.session.clone()),
        None => {
            let agent = key.split('|').nth(1).unwrap_or_default().to_owned();
            let bound = host.bound_session(&key);
            // A Claude terminal attached to a background session may never
            // report at all: read below by the session it shows.
            if agent.is_empty() || (bound.is_none() && crate::agents::harness(&agent) != "claude") {
                return unsupported("no agent has reported from this terminal yet".to_owned());
            }
            (agent, bound)
        }
    };
    let reported = session.is_some();
    if !crate::conversation::Transcript::readable(&agent) {
        return unsupported(format!(
            "{agent}'s sessions can't be read as a conversation yet"
        ));
    }
    // OpenCode's `/clear` and `/new` go to the home screen without a new
    // session, so the pane still names the conversation that just left. The
    // phone starts over on a different session id; a later `session.created`
    // binds the pane to the next one.
    if crate::agents::harness(&agent) == "opencode"
        && let Some(terminal) = host.find(terminal_id)
    {
        let rows = terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
        if crate::screen::opencode_home(&rows) {
            last.remove(&key);
            return proto::Conversation {
                terminal_id,
                supported: true,
                session: crate::screen::OPENCODE_HOME.to_owned(),
                fresh: true,
                ..Default::default()
            };
        }
    }
    let held = session.clone();
    let cleared_session = last.get(&key).and_then(|read| {
        (read.agent == agent
            && Some(&read.session) == session.as_ref()
            && read.transcript.is_none())
        .then_some(read.cleared_session.as_ref())
        .flatten()
    });
    let found = session
        .and_then(|session| {
            crate::conversation::Transcript::locate(&agent, &session).map(|found| (session, found))
        })
        .or_else(|| {
            (crate::agents::harness(&agent) == "claude")
                .then(|| attached_session(host, places, terminal_id, &key))
                .flatten()
                // A stale screen can still name the conversation just
                // cleared. Other attachments are new choices and must show.
                .filter(|(session, _)| Some(session) != cleared_session)
        });
    let (session, transcript) = match found {
        Some((session, transcript)) => {
            last.insert(
                key.clone(),
                LastRead {
                    agent,
                    session: session.clone(),
                    transcript: Some(transcript.clone()),
                    cleared_session: None,
                },
            );
            (session, Some(transcript))
        }
        // A session with nothing to read — not written yet, or one of the
        // agent's own side threads reporting through the same hooks, as
        // Codex's do, or an attached session's name hidden for a moment by
        // a dialog — leaves the chat on the session last read here, rather
        // than putting an error in the middle of it.
        None => match last.get(&key) {
            Some(read)
                if read.agent == agent
                    && (crate::agents::harness(&agent) != "opencode"
                        || held.as_deref() == Some(read.session.as_str())) =>
            {
                (read.session.clone(), read.transcript.clone())
            }
            _ if !reported => {
                return unsupported("no agent has reported from this terminal yet".to_owned());
            }
            _ => return unsupported("the session's transcript isn't there yet".to_owned()),
        },
    };
    drop(last);
    match transcript {
        Some(transcript) => read_conversation(terminal_id, session, &transcript, after),
        None => proto::Conversation {
            terminal_id,
            supported: true,
            session,
            fresh: true,
            ..Default::default()
        },
    }
}

/// Remembers a confirmed Start fresh before answering the phone. An agent
/// may not write its new transcript until the first message: while it has
/// none, the chat must show the empty conversation rather than the old one.
pub(super) fn remember_fresh_conversation(host: &Host, terminal_id: u64, cleared_session: &str) {
    let Some(key) = host.key(terminal_id) else {
        return;
    };
    let status = host.agent_status();
    let Some(pane) = status.panes.iter().find(|pane| pane.pane == key) else {
        return;
    };
    let Some(session) = &pane.session else {
        return;
    };
    lock(&LAST_READ).insert(
        key,
        LastRead {
            agent: pane.agent.clone(),
            session: session.clone(),
            transcript: None,
            cleared_session: Some(cleared_session.to_owned()),
        },
    );
}

/// The agent, session and transcript a pane's conversation was last read
/// from. No transcript means a confirmed Start fresh awaiting its first
/// message, which is read as an empty conversation.
struct LastRead {
    agent: String,
    session: String,
    transcript: Option<crate::conversation::Transcript>,
    /// The cold session an empty fresh conversation replaced. Ignore only
    /// this attachment while the screen catches up after the clear.
    cleared_session: Option<String>,
}

/// For each pane, where its conversation was last read from — see
/// [`conversation`].
static LAST_READ: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, LastRead>>,
> = std::sync::LazyLock::new(Default::default);

/// What `transcript` has said since cursor `after`, for terminal
/// `terminal_id`'s phone.
fn read_conversation(
    terminal_id: u64,
    session: String,
    transcript: &crate::conversation::Transcript,
    after: u64,
) -> proto::Conversation {
    let unsupported = |reason: String| proto::Conversation {
        terminal_id,
        supported: false,
        reason,
        ..Default::default()
    };
    // Only Claude records a queue: another agent's message typed mid-turn
    // shows once it is taken in.
    let queued = transcript.queued();
    let read = transcript.read(after);
    match read {
        Ok(chunk) => proto::Conversation {
            queued,
            terminal_id,
            supported: true,
            reason: String::new(),
            session,
            turns: chunk
                .turns
                .into_iter()
                .map(|turn| proto::Turn {
                    kind: match turn.kind {
                        crate::conversation::TurnKind::User => proto::turn::Kind::User,
                        crate::conversation::TurnKind::Assistant => proto::turn::Kind::Assistant,
                        crate::conversation::TurnKind::Tool => proto::turn::Kind::Tool,
                    } as i32,
                    text: turn.text,
                    tool: turn.tool,
                    images: turn.images,
                })
                .collect(),
            next: chunk.next,
            fresh: chunk.fresh,
        },
        Err(error) => unsupported(format!("couldn't read the session: {error}")),
    }
}
