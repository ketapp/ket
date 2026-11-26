//! A phone handing a terminal's task to another agent (protocol 1.39): a
//! preview of the brief — what the new agent would be told, built from the
//! conversation as `crate::handoff` builds it — and then the switch itself,
//! with the brief the phone showed. The window does the switching; see
//! `Host::switch_agent`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, mpsc};

use ket_remote::proto::{self, envelope::Payload};

use super::requests::{FixAsk, Fixes, envelope, error};
use super::session::Phone;
use super::typing::Typed;
use super::{Event, lock};
use crate::handoff::{Depth, Why, brief_of_session};
use crate::host::server::{Host, Switch};

/// The most of a brief a phone may send: a full brief is held to about
/// 24 KB, and this leaves it room to grow before anything is refused.
const MOST_TEXT: usize = 64 * 1024;

/// Switches, kept as Start fresh's are: a phone whose connection dropped
/// while the window was switching asks again under the same key, and is
/// told the new terminal rather than opening another agent.
static SWITCHES: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

/// The agent in terminal `terminal_id` and its session, as its own report
/// says, or as ket resumed it when it opened the tab; with whether that
/// session is stopped at a limit. `Err` says why there is none.
fn session_of(host: &Host, terminal_id: u64) -> Result<(String, String, bool), &'static str> {
    let key = host.key(terminal_id).ok_or("that terminal has closed")?;
    let status = host.agent_status();
    let pane = status.panes.iter().find(|pane| pane.pane == key);
    let agent = pane.map_or_else(
        || key.split('|').nth(1).unwrap_or_default().to_owned(),
        |pane| pane.agent.clone(),
    );
    let session = pane
        .and_then(|pane| pane.session.clone())
        .or_else(|| host.bound_session(&key))
        .ok_or("no agent has reported from this terminal yet")?;
    let limited = pane.is_some_and(|pane| pane.limit.is_some());
    Ok((agent, session, limited))
}

/// Whether terminal `terminal_id` now holds another session than `named`,
/// the one its brief was read from. One a phone didn't name can't be.
fn moved_on(host: &Host, terminal_id: u64, named: &str) -> bool {
    !named.is_empty() && session_of(host, terminal_id).is_ok_and(|(_, holds, _)| holds != named)
}

/// The brief for handing terminal `terminal_id`'s task over at `depth`, and
/// the session it was read from.
fn brief_of(
    host: &Host,
    terminal_id: u64,
    depth: Depth,
) -> Result<(crate::handoff::Brief, String), String> {
    let (agent, session, limited) = session_of(host, terminal_id).map_err(str::to_owned)?;
    let why = if limited { Why::Limit } else { Why::Chosen };
    brief_of_session(&agent, &session, depth, why).map(|brief| (brief, session))
}

fn depth_of(depth: i32) -> Depth {
    match proto::SwitchDepth::try_from(depth) {
        Ok(proto::SwitchDepth::Task) => Depth::Task,
        Ok(proto::SwitchDepth::Full) => Depth::Full,
        _ => Depth::Recent,
    }
}

/// Whether `agent` is one the desktop can start: a phone may only name one
/// the window listed.
fn startable(host: &Host, agent: &str) -> bool {
    !agent.is_empty() && host.agents().iter().any(|listed| listed == agent)
}

/// What a switch to `ask.agent` would hand over.
pub(super) fn preview_from_phone(
    host: &Host,
    id: u64,
    ask: &proto::PreviewSwitch,
) -> Option<proto::Envelope> {
    if !startable(host, &ask.agent) {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that agent isn't one ket on your Mac can start",
        ));
    }
    match brief_of(host, ask.terminal_id, depth_of(ask.depth)) {
        Ok((brief, session)) => Some(envelope(
            id,
            Payload::SwitchPreview(proto::SwitchPreview {
                terminal_id: ask.terminal_id,
                text: brief.text,
                turns: u32::try_from(brief.turns).unwrap_or(u32::MAX),
                approx_tokens: u32::try_from(brief.approx_tokens).unwrap_or(u32::MAX),
                session,
            }),
        )),
        Err(why) => Some(error(id, proto::ErrorCode::Unspecified, &why)),
    }
}

/// Moves terminal `switch.terminal_id`'s task to `switch.agent`, answering
/// with the terminal the new agent runs in once it has the brief.
pub(super) fn switch_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    switch: &proto::SwitchAgent,
) -> Option<proto::Envelope> {
    let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
    if lock(&SWITCHES).again(&ask, id, feed) {
        return None;
    }
    if !host.controls(switch.terminal_id, phone.holder) {
        return Some(error(
            id,
            proto::ErrorCode::NotController,
            "take control of this terminal first",
        ));
    }
    let Some(key) = host.key(switch.terminal_id) else {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that terminal has closed",
        ));
    };
    if !startable(host, &switch.agent) {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that agent isn't one ket on your Mac can start",
        ));
    }
    let text = switch.text.trim();
    if text.is_empty() || text.len() > MOST_TEXT {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "the brief for the new agent is empty or too long",
        ));
    }
    if moved_on(host, switch.terminal_id, &switch.session) {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "that tab has moved on to another session since its brief was read",
        ));
    }
    {
        let mut switches = lock(&SWITCHES);
        if switches.again(&ask, id, feed) {
            return None;
        }
        if !switches.start(ask.clone(), &key, id, feed) {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "ket is already handing this task over",
            ));
        }
    }
    let host = Arc::clone(host);
    let going = ask.clone();
    let tab = key.clone();
    let typing = phone.phones.typing.clone();
    let terminal_id = switch.terminal_id;
    // What the brief was read from, for the window to hold the tab to: the
    // session its preview named — the tab may have moved on while the
    // phone read it — or, from a phone that named none, the one it holds.
    let session = Some(switch.session.clone())
        .filter(|session| !session.is_empty())
        .or_else(|| {
            session_of(&host, terminal_id)
                .ok()
                .map(|(_, session, _)| session)
        });
    let wanted = Switch {
        agent: switch.agent.clone(),
        model: Some(switch.model.clone()).filter(|model| !model.is_empty()),
        effort: Some(switch.effort.clone()).filter(|effort| !effort.is_empty()),
        text: text.to_owned(),
        beside: switch.beside,
        session,
    };
    let spawned = std::thread::Builder::new()
        .name("ket-host-switch".into())
        .spawn(move || {
            // What phones type into the old terminal waits while it is being
            // replaced, rather than landing in an agent on its way out. Keys
            // into one being replaced in place are refused meanwhile (see
            // `Host::replacing`), since it may close before they go in.
            let (_release, released) = mpsc::channel();
            if let Some(terminal) = host.find(terminal_id) {
                let _ = typing.files(terminal_id, terminal, Typed::Hold(released));
            }
            let answer = match host.switch_agent(&tab, wanted) {
                Ok(terminal_id) => Payload::AgentSwitched(proto::AgentSwitched { terminal_id }),
                Err(message) => Payload::Error(proto::Error {
                    code: proto::ErrorCode::Unspecified as i32,
                    message,
                }),
            };
            lock(&SWITCHES).finish(going, &tab, answer);
        });
    match spawned {
        Ok(_) => None,
        Err(why) => {
            lock(&SWITCHES).forget(&ask, &key);
            Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("the Mac couldn't switch agents: {why}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests;
