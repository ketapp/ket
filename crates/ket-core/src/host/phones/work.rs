//! What a phone asks of the work itself: starting it, a session in a
//! worktree, a worktree merged, pinned or deleted, and a tab woken or
//! started fresh.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use ket_remote::proto::{self, envelope::Payload};

use super::backlog::no_project;
use super::requests::{FixAsk, Fixes, ack, envelope, error, phone_project};
use super::session::Phone;
use super::typing::Typed;
use super::{Event, lock};
use crate::host::server::{Host, NotDeleted};
use crate::id::WorktreeId;

/// Deletes of worktrees, kept the same way as [`super::pulls::FIXES`]:
/// a phone whose connection dropped while the window was deleting asks again
/// under the same key, and is told how it went rather than being told the
/// worktree is not there.
static DELETES: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

/// Wakes of asleep tabs, kept the same way as [`DELETES`]: a phone whose
/// connection dropped while the window was waking asks again under the same
/// key, and is told the terminal rather than waking a second time.
static WAKES: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

/// Start freshes, kept the same way as [`WAKES`]: a phone whose connection
/// dropped while the window was starting a fresh session asks again under
/// the same key, and is told the fresh terminal rather than starting another.
static FRESHES: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

pub(super) fn start_work_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    work: &proto::StartWork,
) -> Option<proto::Envelope> {
    let prompt = work.prompt.trim();
    if prompt.is_empty() {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "say what the agent is to do",
        ));
    }
    let known = phone
        .phones
        .places()
        .projects
        .iter()
        .any(|project| project.id == work.project_id);
    if !known {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "no such project on this desktop",
        ));
    }
    let agent = Some(work.agent.trim().to_owned()).filter(|agent| !agent.is_empty());
    let base = Some(work.base.trim().to_owned()).filter(|value| !value.is_empty());
    let model = Some(work.model.trim().to_owned()).filter(|value| !value.is_empty());
    let effort = Some(work.effort.trim().to_owned()).filter(|value| !value.is_empty());
    let options = start_options(host, phone, &work.project_id);
    if let Some(base) = &base
        && !options.local_branches.contains(base)
    {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that base branch no longer exists",
        ));
    }
    // A phone from before 1.14 could name an agent before the window
    // had published its runnable list. Preserve that old request's
    // desktop fallback when it carries no new launch choices.
    if !(options.agents.is_empty() && model.is_none() && effort.is_none())
        && let Err(why) = validate_launch(
            &options,
            agent.as_deref(),
            model.as_deref(),
            effort.as_deref(),
        )
    {
        return Some(error(id, proto::ErrorCode::Unspecified, why));
    }
    if host.start_phone_session(
        work.project_id.clone(),
        prompt.to_owned(),
        agent,
        base,
        model,
        effort,
        None,
        false,
    ) {
        ack(id)
    } else {
        Some(error(
            id,
            proto::ErrorCode::NotFound,
            "open ket on your Mac to start work from here",
        ))
    }
}

pub(super) fn start_options_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    ask: &proto::GetStartOptions,
) -> Option<proto::Envelope> {
    if phone_project(phone, &ask.project_id).is_none() {
        return Some(no_project(id));
    }
    Some(envelope(
        id,
        Payload::StartOptions(start_options(host, phone, &ask.project_id)),
    ))
}

pub(super) fn start_session_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    session: &proto::StartSession,
) -> Option<proto::Envelope> {
    let places = phone.phones.places();
    let Some(place) = places.get(&session.worktree_id) else {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "no such worktree on this desktop",
        ));
    };
    if !place.path.is_dir() {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that worktree's directory is missing",
        ));
    }
    let agent = Some(session.agent.trim().to_owned()).filter(|name| !name.is_empty());
    let model = Some(session.model.trim().to_owned()).filter(|value| !value.is_empty());
    let effort = Some(session.effort.trim().to_owned()).filter(|value| !value.is_empty());
    let options = start_options(host, phone, &place.project);
    if let Err(why) = validate_launch(
        &options,
        agent.as_deref(),
        model.as_deref(),
        effort.as_deref(),
    ) {
        return Some(error(id, proto::ErrorCode::Unspecified, why));
    }
    if host.start_phone_session(
        place.project.clone(),
        session.prompt.trim().to_owned(),
        agent,
        None,
        model,
        effort,
        Some(place.id.clone()),
        session.agent.is_empty(),
    ) {
        ack(id)
    } else {
        Some(error(
            id,
            proto::ErrorCode::NotFound,
            "open ket on your Mac to start a session from here",
        ))
    }
}

pub(super) fn merge_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    merge: &proto::MergeWorktree,
) -> Option<proto::Envelope> {
    let places = phone.phones.places();
    match places.get(&merge.worktree_id) {
        None => Some(error(
            id,
            proto::ErrorCode::NotFound,
            "no such worktree on this desktop",
        )),
        Some(place) if place.base.is_none() => Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "the repository's own checkout has nothing to merge into",
        )),
        Some(_) if host.merge_work(merge.worktree_id.clone()) => ack(id),
        Some(_) => Some(error(
            id,
            proto::ErrorCode::NotFound,
            "open ket on your Mac to merge from here",
        )),
    }
}

pub(super) fn pin_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    pin: &proto::PinWorktree,
) -> Option<proto::Envelope> {
    let places = phone.phones.places();
    match places.get(&pin.worktree_id) {
        None => Some(error(
            id,
            proto::ErrorCode::NotFound,
            "no such worktree on this desktop",
        )),
        // No record carries a pin for it, as the desktop's menu says.
        Some(place) if place.base.is_none() => Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "the repository's own checkout can't be pinned",
        )),
        Some(_) => {
            let pinned = crate::workspace::Workspace::open().and_then(|workspace| {
                workspace
                    .set_worktree_pinned(&WorktreeId::new(pin.worktree_id.as_str()), pin.pinned)
            });
            match pinned {
                Ok(()) => {
                    phone.phones.forget_places();
                    host.worktrees_changed();
                    ack(id)
                }
                Err(why) => Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    &format!(
                        "could not {} it: {why}",
                        if pin.pinned { "pin" } else { "unpin" }
                    ),
                )),
            }
        }
    }
}

/// Renames a worktree as the desktop's worktree menu does: the name ket
/// shows, not its branch or folder. The repository's own checkout has no
/// record to carry one, as the menu's greyed-out Rename says.
pub(super) fn rename_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    rename: &proto::RenameWorktree,
) -> Option<proto::Envelope> {
    let places = phone.phones.places();
    match places.get(&rename.worktree_id) {
        None => Some(error(
            id,
            proto::ErrorCode::NotFound,
            "no such worktree on this desktop",
        )),
        Some(place) if place.base.is_none() => Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "the repository's own checkout can't be renamed",
        )),
        Some(_) => {
            let renamed = crate::workspace::Workspace::open().and_then(|workspace| {
                workspace
                    .set_worktree_name(&WorktreeId::new(rename.worktree_id.as_str()), &rename.name)
            });
            match renamed {
                Ok(()) => {
                    phone.phones.forget_places();
                    host.worktrees_changed();
                    ack(id)
                }
                Err(why) => Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    &format!("could not rename it: {why}"),
                )),
            }
        }
    }
}

/// The choices ket knows how to pass to each interactive agent: Claude's and
/// Codex's are what the desktop's dialog and the chat's model sheet offer,
/// so the three never disagree.
fn launch_choices(agent: &str) -> (Vec<String>, Vec<String>) {
    let ids = |choices: Vec<crate::agent_model::Choice>| {
        choices.into_iter().map(|choice| choice.id).collect()
    };
    match agent {
        "claude" | "codex" => crate::agent_model::settings(agent, None).map_or_else(
            || (Vec::new(), Vec::new()),
            |settings| (ids(settings.models), ids(settings.efforts)),
        ),
        "grok" => (
            vec!["grok-4.6".to_owned()],
            ["low", "medium", "high", "xhigh"]
                .map(str::to_owned)
                .to_vec(),
        ),
        _ => (Vec::new(), Vec::new()),
    }
}

fn start_options(host: &Host, phone: &Phone<'_>, project: &str) -> proto::StartOptions {
    let places = phone.phones.places();
    let primary = places
        .worktrees
        .iter()
        .find(|place| place.project == project && place.base.is_none());
    let (default_base, local_branches) = primary.map_or_else(
        || (String::new(), Vec::new()),
        |place| {
            (
                place.branch.clone(),
                crate::git::Git::new(&place.path)
                    .local_branches()
                    .unwrap_or_default(),
            )
        },
    );
    let agents = host
        .agents()
        .into_iter()
        .map(|agent| {
            let (models, efforts) = launch_choices(&agent);
            proto::AgentStartOptions {
                agent,
                models,
                efforts,
            }
        })
        .collect();
    proto::StartOptions {
        project_id: project.to_owned(),
        default_base,
        local_branches,
        agents,
    }
}

fn validate_launch(
    options: &proto::StartOptions,
    agent: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> std::result::Result<(), &'static str> {
    let Some(agent) = agent else {
        return if model.is_none() && effort.is_none() {
            Ok(())
        } else {
            Err("choose an agent before choosing a model or effort")
        };
    };
    let Some(choices) = options.agents.iter().find(|choices| choices.agent == agent) else {
        return Err("that agent is no longer available on this desktop");
    };
    if model.is_some_and(|value| !choices.models.iter().any(|item| item == value)) {
        return Err("that model is not available for this agent");
    }
    if effort.is_some_and(|value| !choices.efforts.iter().any(|item| item == value)) {
        return Err("that effort is not available for this agent");
    }
    Ok(())
}

/// Hands a phone's delete to a window — see [`Host::delete_work`] — off
/// this thread, since a delete takes seconds; the answer goes to whoever is
/// waiting for it then: see [`DELETES`].
/// Wakes an asleep tab for a phone — see `crate::host::server::Host::wake`
/// — answering with the terminal it becomes. A tab whose key is already a
/// running terminal is answered at once: a retry after the wake went
/// through, or a tab the desktop woke itself.
pub(super) fn wake_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    wake: &proto::WakeTerminal,
) -> Option<proto::Envelope> {
    let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
    if lock(&WAKES).again(&ask, id, feed) {
        return None;
    }
    // Only a terminal still running: one that exited before its sleep
    // reached the host has the key too, and is nothing to be woken into.
    if let Some((terminal_id, _, _)) = host.listing().into_iter().find(|(_, key, terminal)| {
        key.as_deref() == Some(wake.key.as_str()) && !terminal.is_closed()
    }) {
        return Some(envelope(
            id,
            Payload::TerminalWoken(proto::TerminalWoken { terminal_id }),
        ));
    }
    if !host
        .asleep_tabs()
        .iter()
        .any(|(_, tab)| tab.key == wake.key && tab.worktree == wake.worktree_id)
    {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that tab isn't asleep on this desktop",
        ));
    }
    {
        let mut wakes = lock(&WAKES);
        if wakes.again(&ask, id, feed) {
            return None;
        }
        if !wakes.start(ask.clone(), &wake.key, id, feed) {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "ket is already waking this tab",
            ));
        }
    }
    let key = wake.key.clone();
    let host = Arc::clone(host);
    let waking = ask.clone();
    let spawned = std::thread::Builder::new()
        .name("ket-host-wake".into())
        .spawn(move || {
            let answer = match host.wake(&key) {
                Ok(terminal_id) => Payload::TerminalWoken(proto::TerminalWoken { terminal_id }),
                Err(message) => Payload::Error(proto::Error {
                    code: proto::ErrorCode::Unspecified as i32,
                    message,
                }),
            };
            lock(&WAKES).finish(waking, &key, answer);
        });
    match spawned {
        Ok(_) => None,
        Err(why) => {
            lock(&WAKES).forget(&ask, &wake.key);
            Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("the Mac couldn't start waking the tab: {why}"),
            ))
        }
    }
}

/// Starts a terminal whose session went cold on a new conversation for a
/// phone, in place — see `crate::host::server::Host::start_fresh` —
/// answering with the terminal it is in. Refused unless it is still the cold spell the phone was shown and
/// the agent is idle: a prompt sent since may not have reached the
/// transcript yet, and a session at work is not cold.
pub(super) fn fresh_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    fresh: &proto::StartFresh,
) -> Option<proto::Envelope> {
    let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
    if lock(&FRESHES).again(&ask, id, feed) {
        return None;
    }
    if !host.controls(fresh.terminal_id, phone.holder) {
        return Some(error(
            id,
            proto::ErrorCode::NotController,
            "take control of this terminal first",
        ));
    }
    let Some(key) = host.key(fresh.terminal_id) else {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "that terminal has closed",
        ));
    };
    // Busy by its own report. A session just resumed still reads Starting
    // until its first turn, so that passes here; the window, which can tell
    // a started agent from a starting one, checks again before acting.
    let status = host.agent_status();
    let pane = status.panes.iter().find(|pane| pane.pane == key);
    let busy = pane.is_some_and(|pane| {
        use crate::event::AgentState;
        matches!(
            pane.state,
            AgentState::Thinking | AgentState::ExecutingTool | AgentState::AwaitingPermission
        )
    });
    if busy {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "it's working — start fresh once this turn is done",
        ));
    }
    let session = pane
        .and_then(|pane| pane.session.clone())
        .or_else(|| host.bound_session(&key));
    let last = session
        .as_deref()
        .filter(|session| *session == fresh.session)
        .and_then(crate::conversation::claude_transcript)
        .and_then(|path| crate::cold::claude_last_call(&path));
    if last.is_none_or(|last| last.at_ms != fresh.since_ms) {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "that session has been used since — it isn't cold any more",
        ));
    }
    {
        let mut freshes = lock(&FRESHES);
        if freshes.again(&ask, id, feed) {
            return None;
        }
        if !freshes.start(ask.clone(), &key, id, feed) {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "ket is already starting this one fresh",
            ));
        }
    }
    let host = Arc::clone(host);
    let starting = ask.clone();
    let tab = key.clone();
    let session = fresh.session.clone();
    let since_ms = fresh.since_ms;
    let typing = phone.phones.typing.clone();
    let terminal_id = fresh.terminal_id;
    let spawned = std::thread::Builder::new()
        .name("ket-host-fresh".into())
        .spawn(move || {
            // What phones type into it waits while the window types its
            // `/clear` and the new session starts: a message sent meanwhile
            // goes in after, to the new session, rather than into the
            // command. Released when this thread is done with it.
            let (_release, released) = mpsc::channel();
            if let Some(terminal) = host.find(terminal_id) {
                let _ = typing.files(terminal_id, terminal, Typed::Hold(released));
            }
            let answer = match host.start_fresh(&tab, &session, since_ms) {
                Ok(terminal_id) => {
                    super::conversation::remember_fresh_conversation(&host, terminal_id, &session);
                    Payload::FreshStarted(proto::FreshStarted { terminal_id })
                }
                Err(message) => Payload::Error(proto::Error {
                    code: proto::ErrorCode::Unspecified as i32,
                    message,
                }),
            };
            lock(&FRESHES).finish(starting, &tab, answer);
        });
    match spawned {
        Ok(_) => None,
        Err(why) => {
            lock(&FRESHES).forget(&ask, &key);
            Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("the Mac couldn't start fresh: {why}"),
            ))
        }
    }
}

pub(super) fn delete_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    delete: &proto::DeleteWorktree,
) -> Option<proto::Envelope> {
    let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
    if lock(&DELETES).again(&ask, id, feed) {
        return None;
    }
    let places = phone.phones.places();
    match places.get(&delete.worktree_id) {
        None => {
            return Some(error(
                id,
                proto::ErrorCode::NotFound,
                "no such worktree on this desktop",
            ));
        }
        // It is the repository: deleting it is not a worktree's delete.
        Some(place) if place.base.is_none() => {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "the repository's own checkout can't be deleted",
            ));
        }
        Some(_) => {}
    }
    if !host.has_window() {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "open ket on your Mac to delete a worktree from here",
        ));
    }
    {
        let mut deletes = lock(&DELETES);
        if deletes.again(&ask, id, feed) {
            return None;
        }
        if !deletes.start(ask.clone(), &delete.worktree_id, id, feed) {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "ket is already deleting this worktree",
            ));
        }
    }
    let worktree = delete.worktree_id.clone();
    let force = delete.force;
    let host = Arc::clone(host);
    let deleting = ask.clone();
    let spawned = std::thread::Builder::new()
        .name("ket-host-delete-worktree".into())
        .spawn(move || {
            let answer = match host.delete_work(worktree.clone(), force) {
                Ok(kept) => Payload::WorktreeDeleted(proto::WorktreeDeleted {
                    kept_branch: kept.unwrap_or_default(),
                }),
                // Only an unforced refusal is one forcing would get past.
                Err(NotDeleted::Refused(message)) if !force => Payload::Error(proto::Error {
                    code: proto::ErrorCode::WouldLoseWork as i32,
                    message,
                }),
                Err(NotDeleted::Refused(message) | NotDeleted::Failed(message)) => {
                    Payload::Error(proto::Error {
                        code: proto::ErrorCode::Unspecified as i32,
                        message,
                    })
                }
            };
            lock(&DELETES).finish(deleting, &worktree, answer);
        });
    match spawned {
        Ok(_) => None,
        Err(why) => {
            lock(&DELETES).forget(&ask, &delete.worktree_id);
            Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("could not start deleting it: {why}"),
            ))
        }
    }
}
