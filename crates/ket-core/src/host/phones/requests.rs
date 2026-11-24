//! What a phone asks for: who may ask it, what it costs, that a retry is
//! not applied twice, and doing it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ket_remote::proto::{self, envelope::Payload};

use super::backlog::{backlog_from_phone, edit_from_phone, start_note_from_phone};
use super::conversation::{agent_session_from_phone, conversation_from_phone};
use super::keys::{OFFER_FOCUS, answer_on_screen, answers_offer, is_interrupt};
use super::pulls::{
    PrAction, address_comments_from_phone, draft_from_phone, file_change, fix_checks_from_phone,
    open_pull_request_from_phone, review_from_phone, start_pr_action,
};
use super::session::{Phone, pump};
use super::typing::{Typed, as_typed};
use super::work::{
    delete_from_phone, fresh_from_phone, merge_from_phone, pin_from_phone, rename_from_phone,
    start_options_from_phone, start_session_from_phone, start_work_from_phone, wake_from_phone,
};
use super::{Event, MAX_ATTACHMENTS, MAX_SUBSCRIPTIONS, epoch, lock};
use crate::agent_hooks::{Decision, Prompt};
use crate::agent_status::CancelSource;
use crate::host::server::{AnswerRefused, Host};
use crate::host::wire::DeviceRole;
use crate::terminal::TerminalSize;

/// How many finished fixes are remembered with their answers, for phones
/// that ask again after a reconnect. The same for reviews and pull requests.
const FIXES_KEPT: usize = 64;

/// A device, and the idempotency key it asked under.
pub(super) type FixAsk = (Vec<u8>, String);

pub(super) struct Fixes {
    pub(super) asks: BTreeMap<FixAsk, Fix>,
    /// The worktrees one is going in: another ask for one, under a new key,
    /// is refused meanwhile.
    pub(super) going: BTreeSet<String>,
    /// The finished asks, oldest first — see [`FIXES_KEPT`].
    pub(super) finished: VecDeque<FixAsk>,
}

pub(super) enum Fix {
    /// Being done; each request waiting for the answer, and the session
    /// feed it goes to.
    Going(Vec<(u64, mpsc::Sender<Event>)>),
    /// Done, and how it went.
    Done(Box<Payload>),
}

impl Fixes {
    /// Takes an ask under a key seen before: answers it on `feed` if it is
    /// done, or puts it among those waiting. `false` for a new ask.
    pub(super) fn again(&mut self, ask: &FixAsk, id: u64, feed: &mpsc::Sender<Event>) -> bool {
        match self.asks.get_mut(ask) {
            Some(Fix::Going(waiting)) => waiting.push((id, feed.clone())),
            Some(Fix::Done(answer)) => {
                let _ = feed.send(Event::Send(Box::new(envelope(id, (**answer).clone()))));
            }
            None => return false,
        }
        true
    }

    /// Starts `ask` on `worktree`. `false` when one is going there already.
    pub(super) fn start(
        &mut self,
        ask: FixAsk,
        worktree: &str,
        id: u64,
        feed: &mpsc::Sender<Event>,
    ) -> bool {
        if !self.going.insert(worktree.to_owned()) {
            return false;
        }
        self.asks.insert(ask, Fix::Going(vec![(id, feed.clone())]));
        true
    }

    /// Ends `ask` with `answer`: sent to every request waiting for it, and
    /// kept for any that ask again.
    pub(super) fn finish(&mut self, ask: FixAsk, worktree: &str, answer: Payload) {
        self.going.remove(worktree);
        if let Some(Fix::Going(waiting)) = self
            .asks
            .insert(ask.clone(), Fix::Done(Box::new(answer.clone())))
        {
            for (id, feed) in waiting {
                let _ = feed.send(Event::Send(Box::new(envelope(id, answer.clone()))));
            }
        }
        self.finished.push_back(ask);
        while self.finished.len() > FIXES_KEPT {
            if let Some(old) = self.finished.pop_front() {
                self.asks.remove(&old);
            }
        }
    }

    /// Forgets `ask`, which could not be started.
    pub(super) fn forget(&mut self, ask: &FixAsk, worktree: &str) {
        self.asks.remove(ask);
        self.going.remove(worktree);
    }
}

pub(super) fn envelope(request_id: u64, payload: Payload) -> proto::Envelope {
    let mut envelope = ket_remote::envelope(request_id, payload);
    envelope.host_epoch = epoch();
    envelope
}

pub(super) fn error(request_id: u64, code: proto::ErrorCode, message: &str) -> proto::Envelope {
    envelope(
        request_id,
        Payload::Error(proto::Error {
            code: code as i32,
            message: message.to_owned(),
        }),
    )
}

pub(super) fn ack(request_id: u64) -> Option<proto::Envelope> {
    (request_id != 0).then(|| envelope(request_id, Payload::Ack(proto::Ack {})))
}

/// Answers one request from a phone, if it wants an answer.
///
/// Typing and resizing need the terminal's lease — see
/// [`crate::host::server::LEASE`]. Interrupting and ending what runs in a
/// terminal do not: stopping an agent is always a paired phone's to do.
/// Any request carrying an idempotency key the host has already applied is
/// acknowledged and not applied again, which is what makes a retry after a
/// reconnect safe.
pub(super) fn handle(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    subscriptions: &mut HashMap<u64, Arc<AtomicBool>>,
    leased: &mut HashSet<u64>,
) -> Option<proto::Envelope> {
    let id = request.request_id;
    let payload = request.payload.as_ref()?;
    let mutation = matches!(
        payload,
        Payload::Input(_)
            | Payload::Resize(_)
            | Payload::Signal(_)
            | Payload::Lease(_)
            | Payload::RevokeDevice(_)
            | Payload::Answer(_)
            | Payload::Respond(_)
            | Payload::StartWork(_)
            | Payload::MergeWorktree(_)
            | Payload::EditBacklog(_)
            | Payload::EditAutomation(_)
            | Payload::StartNote(_)
            | Payload::FixChecks(_)
            | Payload::SendFiles(_)
            | Payload::SendReview(_)
            | Payload::OpenPullRequest(_)
            | Payload::MergePullRequest(_)
            | Payload::UpdatePullRequest(_)
            | Payload::ReadyPullRequest(_)
            | Payload::ReopenPullRequest(_)
            | Payload::AddressComments(_)
            | Payload::PinWorktree(_)
            | Payload::RenameWorktree(_)
            | Payload::DeleteWorktree(_)
            | Payload::OpenAgentSession(_)
            | Payload::StartSession(_)
            | Payload::WakeTerminal(_)
            | Payload::SetMode(_)
            | Payload::SetModel(_)
            | Payload::StartFresh(_)
    );
    let key = request.idempotency_key.as_str();
    if mutation {
        // Every mutation names itself, so a retry of one that went through is
        // a no-op. Refused requests are not remembered.
        if key.is_empty() {
            return Some(error(
                id,
                proto::ErrorCode::StaleRequest,
                "a change needs an idempotency key",
            ));
        }
        let applied = lock(&phone.phones.devices)
            .get(&phone.device)
            .is_some_and(|device| device.applied(key));
        if applied {
            return ack(id);
        }
    }

    if let Some(cost) = expensive_cost(payload) {
        let (allowed, refusals) = {
            let mut devices = lock(&phone.phones.devices);
            let device = devices.entry(phone.device.clone()).or_default();
            let allowed = device.requests.take(cost);
            if !allowed {
                device.refused_requests = device.refused_requests.saturating_add(1);
            }
            (allowed, device.refused_requests)
        };
        if !allowed {
            if refusals.is_power_of_two() {
                tracing::warn!(
                    operation = payload_name(payload),
                    refusals,
                    "phone rate limited"
                );
            }
            return Some(error(
                id,
                proto::ErrorCode::Backpressure,
                "too many expensive requests; try again shortly",
            ));
        }
    }

    let reply = apply(host, phone, request, feed, subscriptions, leased);
    let succeeded = reply
        .as_ref()
        .is_none_or(|reply| !matches!(reply.payload, Some(Payload::Error(_))));
    // A fix of checks, a review, a pull request and the actions on one keep
    // their own records, with their answers: see `FIXES`, `REVIEWS`, `PULLS`,
    // `PR_ACTIONS` and `COMMENTS` in `pulls`, and `DELETES`, `WAKES` and
    // `FRESHES` in `work`.
    let kept_here = !matches!(
        payload,
        Payload::FixChecks(_)
            | Payload::SendReview(_)
            | Payload::OpenPullRequest(_)
            | Payload::MergePullRequest(_)
            | Payload::UpdatePullRequest(_)
            | Payload::ReadyPullRequest(_)
            | Payload::ReopenPullRequest(_)
            | Payload::AddressComments(_)
            | Payload::DeleteWorktree(_)
            | Payload::WakeTerminal(_)
            | Payload::StartFresh(_)
    );
    if mutation && succeeded && kept_here {
        lock(&phone.phones.devices)
            .entry(phone.device.clone())
            .or_default()
            .record(key);
    }
    reply
}

fn expensive_cost(payload: &Payload) -> Option<u32> {
    match payload {
        Payload::Subscribe(_) | Payload::GetChanges(_) => Some(3),
        Payload::GetDiff(_) => Some(4),
        // Files written to disk, and a hand-over that takes seconds.
        Payload::SendFiles(_) => Some(3),
        // A first read takes the recent end of a transcript; the poll after
        // it reads on from a cursor, which is a seek and usually nothing.
        Payload::GetConversation(read) if read.after == 0 => Some(3),
        Payload::GetConversation(_) => Some(1),
        // Typed into an agent, as a message is: cheap here, but not free.
        Payload::SendReview(_) => Some(1),
        // Keys pressed through agent view's screens, then a message.
        Payload::OpenAgentSession(_) => Some(2),
        // `git status` in the worktree and the backlog read from disk.
        Payload::GetPullRequestDraft(_) => Some(3),
        // Each reads the failed jobs' logs from GitHub on the desktop.
        // Opening a pull request commits, pushes and asks GitHub twice.
        Payload::StartWork(_)
        | Payload::StartSession(_)
        | Payload::MergeWorktree(_)
        | Payload::StartNote(_)
        | Payload::FixChecks(_)
        | Payload::OpenPullRequest(_)
        | Payload::MergePullRequest(_)
        | Payload::UpdatePullRequest(_)
        | Payload::ReadyPullRequest(_)
        | Payload::ReopenPullRequest(_)
        | Payload::AddressComments(_) => Some(10),
        // A delete walks the checkout and asks git twice.
        Payload::DeleteWorktree(_) => Some(10),
        // Resumes an agent on the desktop.
        Payload::WakeTerminal(_) => Some(5),
        // Ends a session on the desktop and starts another.
        Payload::StartFresh(_) => Some(5),
        // Keys pressed into an agent, each waited on.
        Payload::SetMode(_) => Some(2),
        // Commands typed into an agent, each confirmed.
        Payload::SetModel(_) => Some(2),
        // One write to ket's store.
        Payload::PinWorktree(_) | Payload::RenameWorktree(_) | Payload::EditAutomation(_) => {
            Some(1)
        }
        _ => None,
    }
}

fn payload_name(payload: &Payload) -> &'static str {
    match payload {
        Payload::Subscribe(_) => "subscribe",
        Payload::GetChanges(_) => "get_changes",
        Payload::GetDiff(_) => "get_diff",
        Payload::GetConversation(_) => "get_conversation",
        Payload::StartWork(_) => "start_work",
        Payload::StartSession(_) => "start_session",
        Payload::GetStartOptions(_) => "get_start_options",
        Payload::MergeWorktree(_) => "merge_worktree",
        Payload::StartNote(_) => "start_note",
        Payload::FixChecks(_) => "fix_checks",
        Payload::SendFiles(_) => "send_files",
        Payload::SendReview(_) => "send_review",
        Payload::GetPullRequestDraft(_) => "get_pull_request_draft",
        Payload::OpenPullRequest(_) => "open_pull_request",
        Payload::MergePullRequest(_) => "merge_pull_request",
        Payload::UpdatePullRequest(_) => "update_pull_request",
        Payload::ReadyPullRequest(_) => "ready_pull_request",
        Payload::ReopenPullRequest(_) => "reopen_pull_request",
        Payload::AddressComments(_) => "address_comments",
        Payload::PinWorktree(_) => "pin_worktree",
        Payload::RenameWorktree(_) => "rename_worktree",
        Payload::GetAutomations(_) => "get_automations",
        Payload::EditAutomation(_) => "edit_automation",
        Payload::DeleteWorktree(_) => "delete_worktree",
        Payload::WakeTerminal(_) => "wake_terminal",
        Payload::SetMode(_) => "set_mode",
        Payload::SetModel(_) => "set_model",
        Payload::StartFresh(_) => "start_fresh",
        Payload::MarkSeen(_) => "mark_seen",
        Payload::MarkUnread(_) => "mark_unread",
        Payload::RegisterPush(_) => "register_push",
        Payload::UnregisterPush(_) => "unregister_push",
        Payload::OpenAgentSession(_) => "open_agent_session",
        _ => "other",
    }
}

/// Does what one request asks — see [`handle`], which keeps retries of a
/// change from applying it twice.
fn apply(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    subscriptions: &mut HashMap<u64, Arc<AtomicBool>>,
    leased: &mut HashSet<u64>,
) -> Option<proto::Envelope> {
    let id = request.request_id;
    let payload = request.payload.as_ref()?;
    let required = match payload {
        Payload::ListDevices(_) | Payload::RevokeDevice(_) => DeviceRole::Administrator,
        // Opening a pull request pushes the branch, as a merge lands it.
        Payload::StartWork(_)
        | Payload::StartSession(_)
        | Payload::MergeWorktree(_)
        | Payload::StartNote(_)
        | Payload::OpenPullRequest(_)
        | Payload::MergePullRequest(_)
        | Payload::UpdatePullRequest(_)
        | Payload::ReadyPullRequest(_)
        | Payload::ReopenPullRequest(_)
        | Payload::AddressComments(_)
        // A worktree's place and name in the sidebar, and the worktree
        // itself.
        | Payload::PinWorktree(_)
        | Payload::RenameWorktree(_)
        | Payload::DeleteWorktree(_)
        // Starting an automation now is starting work; pausing or deleting
        // one changes what work starts.
        | Payload::EditAutomation(_) => DeviceRole::Operator,
        // Fixing checks and a review are messages typed into the
        // worktree's agent, which a controller may type itself.
        Payload::Input(_)
        | Payload::SendFiles(_)
        | Payload::OpenAgentSession(_)
        | Payload::WakeTerminal(_)
        | Payload::StartFresh(_)
        | Payload::SetMode(_)
        | Payload::SetModel(_)
        | Payload::Resize(_)
        | Payload::Lease(_)
        | Payload::Signal(_)
        | Payload::Answer(_)
        | Payload::EditBacklog(_)
        | Payload::FixChecks(_)
        | Payload::SendReview(_) => DeviceRole::Controller,
        _ => DeviceRole::Viewer,
    };
    if !phone.role.allows(required) {
        return Some(error(
            id,
            proto::ErrorCode::Unauthorized,
            "this device's role does not allow that operation",
        ));
    }
    let holder = phone.holder;
    let no_control = || {
        Some(error(
            id,
            proto::ErrorCode::NotController,
            "take control of this terminal first",
        ))
    };

    match payload {
        Payload::Subscribe(subscribe) => {
            let Some(terminal) = host.find(subscribe.terminal_id) else {
                return Some(error(id, proto::ErrorCode::NotFound, "no such terminal"));
            };
            if !subscriptions.contains_key(&subscribe.terminal_id)
                && subscriptions.len() >= MAX_SUBSCRIPTIONS
            {
                return Some(error(
                    id,
                    proto::ErrorCode::Backpressure,
                    "too many terminal subscriptions",
                ));
            }
            let stop = Arc::new(AtomicBool::new(false));
            if let Some(old) = subscriptions.insert(subscribe.terminal_id, stop.clone()) {
                old.store(true, Ordering::Release);
            }
            let (feed, terminal_id) = (feed.clone(), subscribe.terminal_id);
            let _ = std::thread::Builder::new()
                .name("ket-host-phone-pump".into())
                .spawn(move || pump(terminal_id, &terminal, &feed, &stop));
            None
        }
        Payload::Unsubscribe(unsubscribe) => {
            if let Some(stop) = subscriptions.remove(&unsubscribe.terminal_id) {
                stop.store(true, Ordering::Release);
            }
            // The desktop's size back once the phone stops looking.
            host.unfit(unsubscribe.terminal_id, holder);
            None
        }
        Payload::Lease(lease) => match proto::lease::Action::try_from(lease.action) {
            Ok(proto::lease::Action::Acquire | proto::lease::Action::Renew) => {
                if host.acquire(lease.terminal_id, holder) {
                    leased.insert(lease.terminal_id);
                    ack(id)
                } else {
                    Some(error(
                        id,
                        proto::ErrorCode::NotController,
                        "another device is in control of this terminal",
                    ))
                }
            }
            Ok(proto::lease::Action::Release) => {
                host.release(lease.terminal_id, holder);
                leased.remove(&lease.terminal_id);
                ack(id)
            }
            _ => None,
        },
        Payload::Input(input) => {
            if !host.controls(input.terminal_id, holder) {
                return no_control();
            }
            let terminal = host.find(input.terminal_id)?;
            // An interrupt goes straight in, as a Signal does: stopping the
            // agent does not wait behind a message still being handed over.
            if is_interrupt(&input.bytes) {
                match input.bytes.as_slice() {
                    [0x03] => host.cancel_terminal(input.terminal_id, CancelSource::Phone),
                    [0x1b] => host.escape_terminal(input.terminal_id),
                    _ => {}
                }
                let _ = terminal.send_input(input.bytes.clone());
            } else {
                let rows =
                    || terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
                if answers_offer(&input.bytes, &rows) {
                    phone
                        .phones
                        .typing
                        .keys(input.terminal_id, &terminal, OFFER_FOCUS.to_vec());
                }
                phone.phones.typing.keys(
                    input.terminal_id,
                    &terminal,
                    as_typed(&terminal, input.bytes.clone()),
                );
            }
            ack(id)
        }
        // Photos and files with a message. Kept before anything is typed, so
        // a file that cannot be leaves the agent's input as it was; then
        // handed over in the terminal's turn, off this thread, since waiting
        // for an agent to show its images would stop this session reading
        // the phone. See [`Typing`].
        Payload::SendFiles(message) => {
            if !host.controls(message.terminal_id, holder) {
                return no_control();
            }
            let Some(terminal) = host.find(message.terminal_id) else {
                return Some(error(id, proto::ErrorCode::NotFound, "no such terminal"));
            };
            if message.attachments.len() > MAX_ATTACHMENTS {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    &format!("send at most {MAX_ATTACHMENTS} files at a time"),
                ));
            }
            if message.text.trim().is_empty() && message.attachments.is_empty() {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "write something or attach a file first",
                ));
            }
            let files: Vec<(&str, &[u8])> = message
                .attachments
                .iter()
                .map(|file| (file.name.as_str(), file.data.as_slice()))
                .collect();
            let kept = match crate::paths::phone_uploads_dir()
                .and_then(|root| crate::attachments::keep(&root, &files))
            {
                Ok(kept) => kept,
                Err(why) => {
                    tracing::warn!(%why, "could not keep a phone's files");
                    return Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        "the Mac couldn't save the files",
                    ));
                }
            };
            let feed = feed.clone();
            let job = Typed::Files {
                text: message.text.clone(),
                files: kept,
                done: Box::new(move |handed| {
                    let reply = match handed {
                        Ok(()) => ack(id),
                        Err(why) => {
                            tracing::warn!(%why, "could not hand a phone's files to its terminal");
                            Some(error(
                                id,
                                proto::ErrorCode::NotFound,
                                "the terminal closed before the message reached it",
                            ))
                        }
                    };
                    if let Some(reply) = reply {
                        let _ = feed.send(Event::Send(Box::new(reply)));
                    }
                }),
            };
            match phone
                .phones
                .typing
                .files(message.terminal_id, terminal, job)
            {
                Ok(_) => None,
                Err(_) => Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "the Mac is too busy to take the files right now",
                )),
            }
        }
        // Fits the terminal to the phone while it has control; the desktop's
        // size comes back when that control ends — see `Host::fit`. Bounded
        // below, so a zero or tiny size cannot wedge a program's layout.
        Payload::Resize(resize) => {
            let size = TerminalSize::new(
                resize.cols.clamp(20, u32::from(u16::MAX)) as u16,
                resize.rows.clamp(5, u32::from(u16::MAX)) as u16,
            );
            if !host.fit(resize.terminal_id, holder, size) {
                return Some(error(
                    id,
                    proto::ErrorCode::NotController,
                    "another device is in control of this terminal",
                ));
            }
            ack(id)
        }
        Payload::ListDevices(_) => {
            let devices = phone
                .phones
                .devices()
                .into_iter()
                .map(|device| proto::Device {
                    this_device: device.id == B64.encode(&phone.device),
                    id: device.id,
                    name: device.name,
                    paired_at: device.paired_at,
                    connected: device.connected,
                    push: device.push,
                })
                .collect();
            Some(envelope(
                id,
                Payload::Devices(proto::DeviceList { devices }),
            ))
        }
        Payload::RevokeDevice(revoke) => match phone.phones.revoke(&revoke.id) {
            Ok(true) => ack(id),
            Ok(false) => Some(error(id, proto::ErrorCode::NotFound, "no such device")),
            Err(e) => Some(error(id, proto::ErrorCode::Unspecified, &e.to_string())),
        },
        Payload::Signal(signal) => {
            let terminal = host.find(signal.terminal_id)?;
            match proto::signal::Kind::try_from(signal.kind) {
                Ok(proto::signal::Kind::Interrupt) => {
                    // A verdict before the byte: the phone asked for the turn
                    // to stop, and no window saw the keystroke to say so.
                    host.cancel_terminal(signal.terminal_id, CancelSource::Phone);
                    let _ = terminal.send_input(vec![0x03]);
                }
                Ok(proto::signal::Kind::Terminate) => terminal.kill(),
                _ => {}
            }
            ack(id)
        }
        Payload::MarkSeen(seen) => {
            host.seen_terminal(seen.terminal_id);
            ack(id)
        }
        Payload::MarkUnread(mark) => {
            // Only a worktree this desktop has: the host keeps a mark until it
            // is taken off, and sends every one to every window.
            if mark.unread && phone.phones.places().get(&mark.worktree_id).is_none() {
                return Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "no such worktree on this desktop",
                ));
            }
            host.mark_unread(&mark.worktree_id, mark.unread);
            ack(id)
        }
        // Only ever about the phone asking: the session's own device key,
        // never one named in the payload.
        Payload::RegisterPush(register) => {
            match super::pushes::Registration::from_proto(register, crate::now_ms() / 1000) {
                Ok(registration) => match phone.phones.set_push(&phone.device, registration) {
                    Ok(()) => ack(id),
                    Err(e) => Some(error(id, proto::ErrorCode::Unspecified, &e.to_string())),
                },
                Err(why) => Some(error(id, proto::ErrorCode::Unspecified, why)),
            }
        }
        Payload::UnregisterPush(_) => match phone.phones.set_push(&phone.device, None) {
            Ok(()) => ack(id),
            Err(e) => Some(error(id, proto::ErrorCode::Unspecified, &e.to_string())),
        },
        Payload::StartWork(work) => start_work_from_phone(host, phone, id, work),
        Payload::GetStartOptions(ask) => start_options_from_phone(host, phone, id, ask),
        Payload::StartSession(session) => start_session_from_phone(host, phone, id, session),
        Payload::GetBacklog(ask) => backlog_from_phone(phone, id, ask),
        Payload::GetAutomations(_) => super::automations::automations_from_phone(phone, id),
        Payload::EditAutomation(edit) => {
            super::automations::edit_automation_from_phone(phone, id, edit)
        }
        Payload::EditBacklog(edit) => edit_from_phone(phone, id, edit),
        Payload::StartNote(start) => start_note_from_phone(host, phone, id, start),
        Payload::GetChanges(ask) => {
            let places = phone.phones.places();
            let reply = match places.get(&ask.worktree_id) {
                Some(place) => match crate::review::changes(&place.path, place.base.as_deref()) {
                    Ok(files) => proto::Changes {
                        worktree_id: ask.worktree_id.clone(),
                        files: files.into_iter().map(file_change).collect(),
                        base: place.base.clone().unwrap_or_default(),
                        error: String::new(),
                    },
                    Err(error) => proto::Changes {
                        worktree_id: ask.worktree_id.clone(),
                        error: error.to_string(),
                        ..Default::default()
                    },
                },
                None => proto::Changes {
                    worktree_id: ask.worktree_id.clone(),
                    error: "no such worktree on this desktop".to_owned(),
                    ..Default::default()
                },
            };
            Some(envelope(id, Payload::Changes(reply)))
        }
        Payload::GetDiff(ask) => {
            let places = phone.phones.places();
            let found = places
                .get(&ask.worktree_id)
                .ok_or_else(|| "no such worktree on this desktop".to_owned())
                .and_then(|place| {
                    crate::review::diff(&place.path, place.base.as_deref(), &ask.path)
                        .map_err(|error| error.to_string())
                });
            let reply = match found {
                Ok((text, truncated)) => proto::Diff {
                    worktree_id: ask.worktree_id.clone(),
                    path: ask.path.clone(),
                    text,
                    truncated,
                    error: String::new(),
                },
                Err(error) => proto::Diff {
                    worktree_id: ask.worktree_id.clone(),
                    path: ask.path.clone(),
                    error,
                    ..Default::default()
                },
            };
            Some(envelope(id, Payload::Diff(reply)))
        }
        Payload::MergeWorktree(merge) => merge_from_phone(host, phone, id, merge),
        Payload::FixChecks(fix) => fix_checks_from_phone(host, phone, request, feed, id, fix),
        Payload::SendReview(review) => review_from_phone(host, phone, request, feed, id, review),
        Payload::GetPullRequestDraft(ask) => draft_from_phone(phone, id, ask),
        Payload::OpenPullRequest(open) => {
            open_pull_request_from_phone(host, phone, request, feed, id, open)
        }
        Payload::MergePullRequest(merge) => start_pr_action(
            host,
            phone,
            request,
            feed,
            id,
            &merge.worktree_id,
            PrAction::Merge(merge.method),
        ),
        Payload::UpdatePullRequest(update) => start_pr_action(
            host,
            phone,
            request,
            feed,
            id,
            &update.worktree_id,
            PrAction::Update,
        ),
        Payload::ReadyPullRequest(ready) => start_pr_action(
            host,
            phone,
            request,
            feed,
            id,
            &ready.worktree_id,
            PrAction::Ready,
        ),
        Payload::ReopenPullRequest(reopen) => start_pr_action(
            host,
            phone,
            request,
            feed,
            id,
            &reopen.worktree_id,
            PrAction::Reopen,
        ),
        Payload::AddressComments(ask) => {
            address_comments_from_phone(host, phone, request, feed, id, &ask.worktree_id)
        }
        Payload::PinWorktree(pin) => pin_from_phone(host, phone, id, pin),
        Payload::RenameWorktree(rename) => rename_from_phone(host, phone, id, rename),
        Payload::WakeTerminal(wake) => wake_from_phone(host, phone, request, feed, id, wake),
        Payload::DeleteWorktree(delete) => {
            delete_from_phone(host, phone, request, feed, id, delete)
        }
        Payload::GetSnippets(_) => {
            let snippets = crate::snippets::Snippets::load()
                .map(|saved| saved.items)
                .unwrap_or_default()
                .into_iter()
                .map(|snippet| proto::Snippet {
                    name: snippet.name,
                    body: snippet.body,
                })
                .collect();
            Some(envelope(
                id,
                Payload::Snippets(proto::SnippetList { snippets }),
            ))
        }
        Payload::GetConversation(ask) => conversation_from_phone(host, phone, id, ask),
        // The agent put in a mode: its own key, pressed on the terminal's
        // typing turn until it reports the mode — see `crate::agent_mode`.
        Payload::SetMode(set) => {
            if !host.controls(set.terminal_id, holder) {
                return no_control();
            }
            let Some(terminal) = host.find(set.terminal_id) else {
                return Some(error(id, proto::ErrorCode::NotFound, "no such terminal"));
            };
            let Some(key) = host.key(set.terminal_id) else {
                return Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "that terminal has closed",
                ));
            };
            let agent = key.split('|').nth(1).unwrap_or_default().to_owned();
            let session = host
                .agent_status()
                .panes
                .iter()
                .find(|pane| pane.pane == key)
                .and_then(|pane| pane.session.clone())
                .or_else(|| host.bound_session(&key));
            let feed = feed.clone();
            let job = Typed::Mode {
                agent,
                session,
                target: set.mode.clone(),
                done: Box::new(move |outcome| {
                    let reply = match outcome {
                        Ok(()) => ack(id),
                        Err(why) => Some(error(id, proto::ErrorCode::Unspecified, &why)),
                    };
                    if let Some(reply) = reply {
                        let _ = feed.send(Event::Send(Box::new(reply)));
                    }
                }),
            };
            match phone.phones.typing.files(set.terminal_id, terminal, job) {
                Ok(_) => None,
                Err(_) => Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "the Mac is too busy to switch that right now",
                )),
            }
        }
        // The agent switched to another model or effort: its own commands,
        // typed on the terminal's typing turn and each confirmed on screen —
        // see `crate::agent_model`. Only while it is idle: a command typed
        // mid-turn waits behind the turn, or lands in a prompt.
        Payload::StartFresh(fresh) => fresh_from_phone(host, phone, request, feed, id, fresh),
        Payload::SetModel(set) => {
            if !host.controls(set.terminal_id, holder) {
                return no_control();
            }
            let Some(terminal) = host.find(set.terminal_id) else {
                return Some(error(id, proto::ErrorCode::NotFound, "no such terminal"));
            };
            let Some(key) = host.key(set.terminal_id) else {
                return Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "that terminal has closed",
                ));
            };
            let agent = key.split('|').nth(1).unwrap_or_default().to_owned();
            let status = host.agent_status();
            let pane = status.panes.iter().find(|pane| pane.pane == key);
            // Busy as the tab says it, on the phone and in the sidebar, not
            // by the agent's last word alone. A session nobody has given a
            // turn yet, or one just compacted, last said "starting" and sits
            // at its prompt; a cancelled turn says nothing, so its last word
            // stays "thinking" until the report lapses.
            let doing = {
                let mut activity = lock(&phone.phones.activity);
                activity.accept_snapshot(status.clone());
                activity.pane(&key, crate::now_ms())
            };
            if doing.is_some_and(|(_, signal)| signal != crate::activity::Signal::Quiet) {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    &format!(
                        "{} is busy — change it once this turn is done",
                        crate::agents::harness(&agent)
                    ),
                ));
            }
            let session = pane
                .and_then(|pane| pane.session.clone())
                .or_else(|| host.bound_session(&key));
            let feed = feed.clone();
            let job = Typed::Model {
                agent,
                session,
                model: set.model.clone(),
                effort: set.effort.clone(),
                done: Box::new(move |outcome| {
                    let reply = match outcome {
                        Ok(()) => ack(id),
                        Err(why) => Some(error(id, proto::ErrorCode::Unspecified, &why)),
                    };
                    if let Some(reply) = reply {
                        let _ = feed.send(Event::Send(Box::new(reply)));
                    }
                }),
            };
            match phone.phones.typing.files(set.terminal_id, terminal, job) {
                Ok(_) => None,
                Err(_) => Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "the Mac is too busy to switch that right now",
                )),
            }
        }
        Payload::OpenAgentSession(open) => agent_session_from_phone(host, phone, feed, id, open),
        Payload::Answer(answer) => {
            let choice = proto::answer::Choice::try_from(answer.choice).ok()?;
            // The very prompt the phone showed, checked and answered in one
            // step on the host — see `Host::answer_prompt`.
            let refused = host.answer_prompt_on_screen(
                answer.terminal_id,
                answer.question_id,
                |agent, tool, rows| answer_on_screen(agent, tool, choice, rows),
            );
            match refused {
                Ok(()) => ack(id),
                Err(why) => Some(refusal(id, why)),
            }
        }
        Payload::Respond(respond) => {
            // As an Answer is, but through the prompt's held hook — see
            // `Host::respond_prompt`.
            let refused = host.respond_prompt(respond.terminal_id, respond.question_id, |asked| {
                decision(asked, respond)
            });
            match refused {
                Ok(()) => ack(id),
                Err(why) => Some(refusal(id, why)),
            }
        }
        _ => None,
    }
}

/// The project a phone named, when this desktop has it.
pub(super) fn phone_project(phone: &Phone<'_>, id: &str) -> Option<crate::id::ProjectId> {
    phone
        .phones
        .places()
        .projects
        .iter()
        .any(|project| !id.is_empty() && project.id == id)
        .then(|| crate::id::ProjectId::from(id))
}

/// What a person's Respond decides about the prompt it names, or `None` when
/// it does not fit what the prompt asked: a pick for every question, one
/// choice where only one is allowed, choices that exist.
pub(super) fn decision(
    asked: &crate::agent_hooks::PermissionQuestion,
    respond: &proto::Respond,
) -> Option<Decision> {
    match asked.prompt.as_ref()? {
        // Codex's form takes one option's number per question, and nothing
        // typed: words go in through its notes, which keys cannot reach
        // safely.
        Prompt::Ask { questions } if asked.tool == crate::agent_hooks::CODEX_ASK_TOOL => {
            if respond.picks.len() != questions.len() {
                return None;
            }
            questions
                .iter()
                .zip(&respond.picks)
                .map(|(question, pick)| {
                    let [at] = pick.choices.as_slice() else {
                        return None;
                    };
                    let at = *at as usize;
                    (pick.other.trim().is_empty() && at < question.options.len()).then_some(at)
                })
                .collect::<Option<Vec<_>>>()
                .map(Decision::Picks)
        }
        Prompt::Ask { questions } => {
            if respond.picks.len() != questions.len() {
                return None;
            }
            questions
                .iter()
                .zip(&respond.picks)
                .map(|(question, pick)| {
                    let other = pick.other.trim();
                    if !other.is_empty() {
                        return Some(other.to_owned());
                    }
                    let mut chosen: Vec<usize> =
                        pick.choices.iter().map(|&at| at as usize).collect();
                    chosen.sort_unstable();
                    chosen.dedup();
                    if chosen.is_empty() || (!question.multi_select && chosen.len() > 1) {
                        return None;
                    }
                    let labels = chosen
                        .into_iter()
                        .map(|at| question.options.get(at).map(|option| option.label.as_str()))
                        .collect::<Option<Vec<_>>>()?;
                    // Joined as Claude's own dialog joins a multi-select.
                    Some(labels.join(", "))
                })
                .collect::<Option<Vec<_>>>()
                .map(Decision::Answers)
        }
        // One question, one choice: the option's number is what gets typed.
        Prompt::Dialog { options, .. } => {
            let [pick] = respond.picks.as_slice() else {
                return None;
            };
            let [at] = pick.choices.as_slice() else {
                return None;
            };
            let at = *at as usize;
            (at < options.len()).then_some(Decision::Pick(at))
        }
        Prompt::Plan => match respond.approve {
            true => Some(Decision::Approve),
            false => {
                let feedback = respond.feedback.trim();
                (!feedback.is_empty()).then(|| Decision::KeepPlanning(feedback.to_owned()))
            }
        },
    }
}

/// The error a refused Answer or Respond goes back with.
fn refusal(id: u64, why: AnswerRefused) -> proto::Envelope {
    let (code, message) = match why {
        AnswerRefused::Gone => (proto::ErrorCode::NotFound, "that terminal has closed"),
        AnswerRefused::Answered => (
            proto::ErrorCode::NotFound,
            "that prompt has already been answered",
        ),
        AnswerRefused::Changed => (
            proto::ErrorCode::NotFound,
            "that prompt has changed — look again before answering",
        ),
        AnswerRefused::Unnamed => (
            proto::ErrorCode::Unspecified,
            "update ket on this phone to answer prompts from it",
        ),
        AnswerRefused::Unknown => (
            proto::ErrorCode::Unspecified,
            "ket cannot answer this prompt from a phone — answer it in the terminal",
        ),
        AnswerRefused::Unfit => (
            proto::ErrorCode::Unspecified,
            "that answer doesn't fit the question — look again before answering",
        ),
        AnswerRefused::NotOffered => (
            proto::ErrorCode::Unspecified,
            "this prompt offers no always allow — approve it once, or answer it in the terminal",
        ),
    };
    error(id, code, message)
}
