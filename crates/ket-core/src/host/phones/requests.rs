//! What a phone asks for: who may ask it, what it costs, that a retry is
//! not applied twice, and doing it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ket_remote::proto::{self, envelope::Payload};

use super::keys::{answer_keys, is_interrupt};
use super::places::{Place, Places};
use super::session::{Phone, pump};
use super::typing::Typed;
use super::{Event, MAX_ATTACHMENTS, MAX_SUBSCRIPTIONS, epoch, lock};
use crate::agent_hooks::{Decision, Prompt};
use crate::agent_status::CancelSource;
use crate::github::PullRequestStatus;
use crate::host::server::{AnswerRefused, Handed, Host};
use crate::host::wire::DeviceRole;
use crate::id::{ProjectId, WorktreeId};
use crate::terminal::TerminalSize;

/// How many finished fixes are remembered with their answers, for phones
/// that ask again after a reconnect. The same for reviews and pull requests.
const FIXES_KEPT: usize = 64;

/// The most comments one review sends, and the most text in them all: a
/// message typed into an agent, not a file transfer.
const MAX_REVIEW_COMMENTS: usize = 50;
const MAX_REVIEW_BYTES: usize = 64 * 1024;

/// The most a phone's pull request title and description carry, in bytes:
/// a bound on one request, not GitHub's own limits, which `gh` reports.
const MAX_PULL_REQUEST_TITLE: usize = 1024;
const MAX_PULL_REQUEST_BODY: usize = 64 * 1024;

/// Every Fix checks with agent phones have asked for, by the device and the
/// idempotency key each was asked under: going, with who is waiting for its
/// answer, or that answer. Kept here rather than with a session, which a
/// phone that loses its connection while it waits does not come back to:
/// asking again under the same key, it is told how its fix went rather than
/// giving the agent the message twice.
static FIXES: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

/// Every review phones have asked to send, kept the same way as [`FIXES`]:
/// a phone whose connection dropped while the window was sending it asks
/// again under the same key, and is told how it went rather than giving
/// the agent the review twice.
static REVIEWS: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

/// Every pull request phones have asked to open, kept the same way as
/// [`FIXES`]: a phone whose connection dropped during the push asks again
/// under the same key, and is told how it went rather than pushing twice.
static PULLS: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

/// A device, and the idempotency key it asked under.
type FixAsk = (Vec<u8>, String);

struct Fixes {
    asks: BTreeMap<FixAsk, Fix>,
    /// The worktrees one is going in: another ask for one, under a new key,
    /// is refused meanwhile.
    going: BTreeSet<String>,
    /// The finished asks, oldest first — see [`FIXES_KEPT`].
    finished: VecDeque<FixAsk>,
}

enum Fix {
    /// Being done; each request waiting for the answer, and the session
    /// feed it goes to.
    Going(Vec<(u64, mpsc::Sender<Event>)>),
    /// Done, and how it went.
    Done(Box<Payload>),
}

impl Fixes {
    /// Takes an ask under a key seen before: answers it on `feed` if it is
    /// done, or puts it among those waiting. `false` for a new ask.
    fn again(&mut self, ask: &FixAsk, id: u64, feed: &mpsc::Sender<Event>) -> bool {
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
    fn start(&mut self, ask: FixAsk, worktree: &str, id: u64, feed: &mpsc::Sender<Event>) -> bool {
        if !self.going.insert(worktree.to_owned()) {
            return false;
        }
        self.asks.insert(ask, Fix::Going(vec![(id, feed.clone())]));
        true
    }

    /// Ends `ask` with `answer`: sent to every request waiting for it, and
    /// kept for any that ask again.
    fn finish(&mut self, ask: FixAsk, worktree: &str, answer: Payload) {
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
    fn forget(&mut self, ask: &FixAsk, worktree: &str) {
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

fn ack(request_id: u64) -> Option<proto::Envelope> {
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
            | Payload::StartNote(_)
            | Payload::FixChecks(_)
            | Payload::SendFiles(_)
            | Payload::SendReview(_)
            | Payload::OpenPullRequest(_)
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
    // A fix of checks, a review and a pull request keep their own records,
    // with their answers: see `FIXES`, `REVIEWS` and `PULLS`.
    let kept_here = !matches!(
        payload,
        Payload::FixChecks(_) | Payload::SendReview(_) | Payload::OpenPullRequest(_)
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
        // `git status` in the worktree and the backlog read from disk.
        Payload::GetPullRequestDraft(_) => Some(3),
        // Each reads the failed jobs' logs from GitHub on the desktop.
        // Opening a pull request commits, pushes and asks GitHub twice.
        Payload::StartWork(_)
        | Payload::StartSession(_)
        | Payload::MergeWorktree(_)
        | Payload::StartNote(_)
        | Payload::FixChecks(_)
        | Payload::OpenPullRequest(_) => Some(10),
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
        | Payload::OpenPullRequest(_) => DeviceRole::Operator,
        // Fixing checks and a review are messages typed into the
        // worktree's agent, which a controller may type itself.
        Payload::Input(_)
        | Payload::SendFiles(_)
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
                phone
                    .phones
                    .typing
                    .keys(input.terminal_id, &terminal, input.bytes.clone());
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
        Payload::StartWork(work) => {
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
        Payload::GetStartOptions(ask) => {
            if phone_project(phone, &ask.project_id).is_none() {
                return Some(no_project(id));
            }
            Some(envelope(
                id,
                Payload::StartOptions(start_options(host, phone, &ask.project_id)),
            ))
        }
        Payload::StartSession(session) => {
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
        Payload::GetBacklog(ask) => {
            let Some(project) = phone_project(phone, &ask.project_id) else {
                return Some(no_project(id));
            };
            Some(match crate::backlog::Backlog::load(&project) {
                Ok(backlog) => backlog_reply(id, &ask.project_id, backlog),
                Err(why) => error(
                    id,
                    proto::ErrorCode::Unspecified,
                    &format!("could not read the backlog: {why}"),
                ),
            })
        }
        Payload::EditBacklog(edit) => {
            let Some(project) = phone_project(phone, &edit.project_id) else {
                return Some(no_project(id));
            };
            let edited = edit_backlog(&project, edit);
            phone.phones.forget_backlogs();
            Some(match edited {
                Ok(backlog) => backlog_reply(id, &edit.project_id, backlog),
                Err((code, why)) => error(id, code, &why),
            })
        }
        Payload::StartNote(start) => {
            let Some(project) = phone_project(phone, &start.project_id) else {
                return Some(no_project(id));
            };
            let note = match crate::backlog::Backlog::load(&project) {
                Ok(backlog) => backlog
                    .notes
                    .into_iter()
                    .find(|note| note.id == start.note_id),
                Err(why) => {
                    return Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        &format!("could not read the backlog: {why}"),
                    ));
                }
            };
            let Some(note) = note else {
                return Some(no_note(id));
            };
            if note.done.is_some() {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "that note has already been started or done",
                ));
            }
            // What a window from before `note` existed starts on: the words
            // the desktop's own Start hands the agent, without the files.
            let words: Vec<&str> = [note.title.trim(), note.body.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            if words.is_empty() {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "write what the work is first",
                ));
            }
            let started = host.start_work(
                start.project_id.clone(),
                words.join("\n\n"),
                None,
                Some(note.id),
                crate::host::wire::WorkSource::Phone,
            );
            if started {
                ack(id)
            } else {
                Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "open ket on your Mac to start work from here",
                ))
            }
        }
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
        Payload::MergeWorktree(merge) => {
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
        Payload::FixChecks(fix) => {
            let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
            if lock(&FIXES).again(&ask, id, feed) {
                return None;
            }
            let places = phone.phones.places();
            let open = host.pull_request(&fix.worktree_id);
            let (place, open) = match (places.get(&fix.worktree_id), open) {
                (None, _) => {
                    return Some(error(
                        id,
                        proto::ErrorCode::NotFound,
                        "no such worktree on this desktop",
                    ));
                }
                (Some(_), None) => {
                    return Some(error(
                        id,
                        proto::ErrorCode::NotFound,
                        "ket on your Mac has not found a pull request for this worktree",
                    ));
                }
                (Some(place), Some(open)) => (place, open),
            };
            if open.failed().next().is_none() {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "none of this pull request's checks have failed",
                ));
            }
            // Checked first, so as not to read logs nobody can be given.
            if !host.has_window() {
                return Some(no_window_to_fix(id));
            }
            {
                let mut fixes = lock(&FIXES);
                // Asked again while the above was looked at.
                if fixes.again(&ask, id, feed) {
                    return None;
                }
                if !fixes.start(ask.clone(), &fix.worktree_id, id, feed) {
                    return Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        "ket is already giving this pull request's failed checks to the agent",
                    ));
                }
            }
            // Reading the logs takes `gh` a few seconds a job, and starting
            // an agent longer, which this session's terminals do not wait
            // for; the phone does. The answer goes to whoever is waiting for
            // it when it comes — see `FIXES`.
            let project = ProjectId::new(place.project.clone());
            let worktree = fix.worktree_id.clone();
            let host = Arc::clone(host);
            let fixing = ask.clone();
            let spawned = std::thread::Builder::new()
                .name("ket-host-fix-checks".into())
                .spawn(move || {
                    let answer = fix_checks(&host, &project, worktree.clone(), &open);
                    lock(&FIXES).finish(fixing, &worktree, answer);
                });
            match spawned {
                Ok(_) => None,
                Err(why) => {
                    lock(&FIXES).forget(&ask, &fix.worktree_id);
                    Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        &format!("could not start reading the checks: {why}"),
                    ))
                }
            }
        }
        // Written into one message by a window, as the desktop's own Send to
        // agent does, with the same refusals. A window takes it within a
        // quarter of a second, but one that is busy is waited for longer,
        // off this thread — see `Host::send_review` — and the answer goes to
        // whoever is waiting for it then: see `REVIEWS`.
        Payload::SendReview(review) => {
            let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
            if lock(&REVIEWS).again(&ask, id, feed) {
                return None;
            }
            let places = phone.phones.places();
            let Some(place) = places.get(&review.worktree_id) else {
                return Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "no such worktree on this desktop",
                ));
            };
            if let Err(why) = review_fits(&review.comments) {
                return Some(error(id, proto::ErrorCode::Unspecified, &why));
            }
            if !host.has_window() {
                return Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "open ket on your Mac to send a review from here",
                ));
            }
            {
                let mut reviews = lock(&REVIEWS);
                if reviews.again(&ask, id, feed) {
                    return None;
                }
                if !reviews.start(ask.clone(), &review.worktree_id, id, feed) {
                    return Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        "a review is already on its way to this worktree's agent",
                    ));
                }
            }
            let worktree = review.worktree_id.clone();
            let base = place.base.clone();
            let comments = review
                .comments
                .iter()
                .map(|comment| crate::host::wire::PhoneReviewComment {
                    rel: comment.path.clone(),
                    lines: comment.lines.clone(),
                    text: comment.text.clone(),
                    comment: comment.comment.trim().to_owned(),
                })
                .collect();
            let host = Arc::clone(host);
            let sending = ask.clone();
            let spawned = std::thread::Builder::new()
                .name("ket-host-send-review".into())
                .spawn(move || {
                    let answer = match host.send_review(worktree.clone(), base, comments) {
                        Ok(()) => Payload::Ack(proto::Ack {}),
                        Err(message) => Payload::Error(proto::Error {
                            code: proto::ErrorCode::Unspecified as i32,
                            message,
                        }),
                    };
                    lock(&REVIEWS).finish(sending, &worktree, answer);
                });
            match spawned {
                Ok(_) => None,
                Err(why) => {
                    lock(&REVIEWS).forget(&ask, &review.worktree_id);
                    Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        &format!("could not start sending the review: {why}"),
                    ))
                }
            }
        }
        Payload::GetPullRequestDraft(ask) => {
            let places = phone.phones.places();
            let place = match pull_request_place(&places, &ask.worktree_id) {
                Ok(place) => place,
                Err((code, why)) => return Some(error(id, code, why)),
            };
            // The note the branch was started from names it best; a backlog
            // that will not load leaves the branch to name it.
            let notes = crate::backlog::Backlog::load(&ProjectId::new(place.project.clone()))
                .map(|backlog| backlog.notes)
                .unwrap_or_default();
            let drafted = crate::workspace::Workspace::open().and_then(|workspace| {
                workspace.pull_request_draft(&WorktreeId::new(ask.worktree_id.as_str()), &notes)
            });
            Some(match drafted {
                Ok(draft) => envelope(
                    id,
                    Payload::PullRequestDraft(proto::PullRequestDraft {
                        worktree_id: ask.worktree_id.clone(),
                        branch: draft.branch,
                        base: draft.base,
                        remote: draft.remote.unwrap_or_default(),
                        title: draft.title,
                        body: draft.body,
                        changes: u32::try_from(draft.changes).unwrap_or(u32::MAX),
                        existing_number: draft.existing.as_ref().map_or(0, |link| link.number),
                        existing_url: draft.existing.map(|link| link.url).unwrap_or_default(),
                    }),
                ),
                Err(why) => error(id, proto::ErrorCode::Unspecified, &why.to_string()),
            })
        }
        Payload::OpenPullRequest(open) => {
            let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
            if lock(&PULLS).again(&ask, id, feed) {
                return None;
            }
            let places = phone.phones.places();
            if let Err((code, why)) = pull_request_place(&places, &open.worktree_id) {
                return Some(error(id, code, why));
            }
            let title = open.title.trim();
            if title.is_empty() {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "a pull request needs a title",
                ));
            }
            if title.len() > MAX_PULL_REQUEST_TITLE || open.body.len() > MAX_PULL_REQUEST_BODY {
                return Some(error(
                    id,
                    proto::ErrorCode::Unspecified,
                    "that title or description is too long to send",
                ));
            }
            {
                let mut pulls = lock(&PULLS);
                // Asked again while the above was looked at.
                if pulls.again(&ask, id, feed) {
                    return None;
                }
                if !pulls.start(ask.clone(), &open.worktree_id, id, feed) {
                    return Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        "this worktree's branch is already being pushed",
                    ));
                }
            }
            // Committing runs the repository's hooks and pushing talks to
            // the remote, which this session's terminals do not wait for;
            // the phone does. The answer goes to whoever is waiting for it
            // when it comes — see `PULLS`.
            let worktree = open.worktree_id.clone();
            let options = crate::workspace::PullRequestOptions {
                title: title.to_owned(),
                body: open.body.clone(),
                draft: open.draft,
            };
            let host = Arc::clone(host);
            let opening = ask.clone();
            let spawned = std::thread::Builder::new()
                .name("ket-host-open-pull-request".into())
                .spawn(move || {
                    let answer = open_pull_request(&host, &worktree, &options);
                    lock(&PULLS).finish(opening, &worktree, answer);
                });
            match spawned {
                Ok(_) => None,
                Err(why) => {
                    lock(&PULLS).forget(&ask, &open.worktree_id);
                    Some(error(
                        id,
                        proto::ErrorCode::Unspecified,
                        &format!("could not start pushing: {why}"),
                    ))
                }
            }
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
        Payload::GetConversation(ask) => Some(envelope(
            id,
            Payload::Conversation(conversation(host, ask.terminal_id, ask.after)),
        )),
        Payload::Answer(answer) => {
            let choice = proto::answer::Choice::try_from(answer.choice).ok()?;
            // The very prompt the phone showed, checked and answered in one
            // step on the host — see `Host::answer_prompt`.
            let refused =
                host.answer_prompt(answer.terminal_id, answer.question_id, |agent, tool| {
                    answer_keys(agent, tool, choice)
                });
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
fn phone_project(phone: &Phone<'_>, id: &str) -> Option<crate::id::ProjectId> {
    phone
        .phones
        .places()
        .projects
        .iter()
        .any(|project| !id.is_empty() && project.id == id)
        .then(|| crate::id::ProjectId::from(id))
}

/// The choices ket knows how to pass to each interactive agent. Aliases are
/// preferred where an agent maintains one, so a model release need not make
/// the phone's saved choice stale.
fn launch_choices(agent: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match agent {
        "claude" => (&["sonnet", "opus", "haiku"], &["low", "medium", "high"]),
        "codex" => (
            &["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol", "gpt-6-luna"],
            &["low", "medium", "high", "xhigh"],
        ),
        "grok" => (&["grok-4.6"], &["low", "medium", "high", "xhigh"]),
        _ => (&[], &[]),
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
                models: models.iter().map(|value| (*value).to_owned()).collect(),
                efforts: efforts.iter().map(|value| (*value).to_owned()).collect(),
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

/// A phone's Fix checks with agent, off the session's thread: reads what
/// `open`'s failed checks printed, as the desktop's own menu item does, and
/// hands the message to a window to type into `worktree`'s agent. The
/// answer says how that went, so a phone is not told its fix is on the way
/// when the logs could not be read or the agent would not start.
fn fix_checks(
    host: &Host,
    project: &ProjectId,
    worktree: String,
    open: &PullRequestStatus,
) -> Payload {
    let failed = |message: String| {
        Payload::Error(proto::Error {
            code: proto::ErrorCode::Unspecified as i32,
            message,
        })
    };
    let handed = crate::checks::checkout(project).and_then(|at| {
        let logs = crate::checks::logs_dir()?;
        crate::checks::hand_over(&at, &WorktreeId::new(worktree.as_str()), open, &logs)
    });
    let prompt = match handed {
        Ok(prompt) => prompt,
        Err(why) => {
            tracing::warn!(%why, "could not read failed checks for a phone");
            return failed(format!(
                "could not read #{}'s failed checks: {why}",
                open.number
            ));
        }
    };
    match host.fix_checks(worktree, prompt) {
        Handed::Given => Payload::ChecksHandedOver(proto::ChecksHandedOver { starting: false }),
        Handed::Starting => Payload::ChecksHandedOver(proto::ChecksHandedOver { starting: true }),
        Handed::Failed(why) => failed(why),
    }
}

/// The worktree a phone's pull request is for, or why there is none to
/// open: no worktree by that id, or a repository's own checkout, which has
/// no branch of its own to open one from.
fn pull_request_place<'a>(
    places: &'a Places,
    worktree: &str,
) -> std::result::Result<&'a Place, (proto::ErrorCode, &'static str)> {
    match places.get(worktree) {
        None => Err((
            proto::ErrorCode::NotFound,
            "no such worktree on this desktop",
        )),
        Some(place) if place.base.is_none() => Err((
            proto::ErrorCode::Unspecified,
            "the repository's own checkout has no branch of its own to open a pull request from",
        )),
        Some(place) => Ok(place),
    }
}

/// A phone's pull request, off the session's thread: ket's own commit,
/// push and pull request, as the desktop's Pull request… runs them, with no
/// window needed. One opened is put among the pull requests phones are
/// shown at once, rather than when a window next reads GitHub.
fn open_pull_request(
    host: &Host,
    worktree: &str,
    options: &crate::workspace::PullRequestOptions,
) -> Payload {
    let opened = crate::workspace::Workspace::open()
        .and_then(|workspace| workspace.open_pull_request(&WorktreeId::new(worktree), options));
    let report = match opened {
        Ok(report) => report,
        Err(why) => {
            tracing::info!(worktree, %why, "a phone's pull request was refused");
            return Payload::Error(proto::Error {
                code: proto::ErrorCode::Unspecified as i32,
                message: why.to_string(),
            });
        }
    };
    host.remember_pull_request(
        worktree,
        PullRequestStatus {
            number: report.pull_request.number,
            url: report.pull_request.url.clone(),
            branch: report.branch.clone(),
            // One already open may be a draft or not; the window's next read
            // says which.
            draft: options.draft && !report.existed,
            review: crate::github::Review::None,
            checks: Vec::new(),
        },
    );
    Payload::PullRequestOpened(proto::PullRequestOpened {
        number: report.pull_request.number,
        url: report.pull_request.url,
        branch: report.branch,
        base: report.base,
        remote: report.remote,
        existed: report.existed,
        committed: report
            .committed
            .map_or(0, |commit| u32::try_from(commit.files).unwrap_or(u32::MAX)),
    })
}

/// Whether a phone's review is one to send: some comments, each saying
/// something, and not more of them than [`MAX_REVIEW_COMMENTS`] or
/// [`MAX_REVIEW_BYTES`] allow. Why not, when it is not.
fn review_fits(comments: &[proto::ReviewComment]) -> std::result::Result<(), String> {
    if comments.is_empty() {
        return Err("write a comment on the diff first".to_owned());
    }
    if comments.len() > MAX_REVIEW_COMMENTS {
        return Err(format!(
            "a review sends at most {MAX_REVIEW_COMMENTS} comments at a time"
        ));
    }
    if comments
        .iter()
        .any(|comment| comment.comment.trim().is_empty())
    {
        return Err("write what should change in every comment first".to_owned());
    }
    let bytes: usize = comments
        .iter()
        .map(|comment| {
            comment.path.len() + comment.lines.len() + comment.text.len() + comment.comment.len()
        })
        .sum();
    if bytes > MAX_REVIEW_BYTES {
        return Err("the review is too long to send at once; send fewer comments".to_owned());
    }
    Ok(())
}

fn no_window_to_fix(id: u64) -> proto::Envelope {
    error(
        id,
        proto::ErrorCode::NotFound,
        "open ket on your Mac to fix checks from here",
    )
}

fn no_project(id: u64) -> proto::Envelope {
    error(
        id,
        proto::ErrorCode::NotFound,
        "no such project on this desktop",
    )
}

fn no_note(id: u64) -> proto::Envelope {
    error(
        id,
        proto::ErrorCode::NotFound,
        "no such note in the backlog",
    )
}

/// A project's backlog, as a phone is sent it.
fn backlog_reply(id: u64, project: &str, backlog: crate::backlog::Backlog) -> proto::Envelope {
    let notes = backlog
        .notes
        .into_iter()
        .map(|note| proto::BacklogNote {
            done_ms: note.done.as_ref().map_or(0, |done| done.at_ms),
            branch: note.done.and_then(|done| done.branch).unwrap_or_default(),
            id: note.id,
            title: note.title,
            body: note.body,
            files: note.attachments,
            created_ms: note.created_ms,
            updated_ms: note.updated_ms,
            priority: wire_priority(note.priority).into(),
        })
        .collect();
    envelope(
        id,
        Payload::Backlog(proto::Backlog {
            project_id: project.to_owned(),
            notes,
        }),
    )
}

/// A note's priority as a phone is sent it.
fn wire_priority(priority: crate::backlog::Priority) -> proto::BacklogPriority {
    use crate::backlog::Priority;
    match priority {
        Priority::Low => proto::BacklogPriority::Low,
        Priority::Medium => proto::BacklogPriority::Medium,
        Priority::High => proto::BacklogPriority::High,
        Priority::Urgent => proto::BacklogPriority::Urgent,
    }
}

/// The priority a phone asked for, or `None` when it named none — an older
/// phone, or a level this desktop does not know.
fn asked_priority(wire: i32) -> Option<crate::backlog::Priority> {
    use crate::backlog::Priority;
    match proto::BacklogPriority::try_from(wire).ok()? {
        proto::BacklogPriority::Unspecified => None,
        proto::BacklogPriority::Low => Some(Priority::Low),
        proto::BacklogPriority::Medium => Some(Priority::Medium),
        proto::BacklogPriority::High => Some(Priority::High),
        proto::BacklogPriority::Urgent => Some(Priority::Urgent),
    }
}

/// Applies one phone's change to a note — see [`proto::EditBacklog`].
fn edit_backlog(
    project: &crate::id::ProjectId,
    edit: &proto::EditBacklog,
) -> std::result::Result<crate::backlog::Backlog, (proto::ErrorCode, String)> {
    use crate::backlog::{Backlog, Done, Note};
    use proto::edit_backlog::Action;

    let failed = |why: crate::KetError| (proto::ErrorCode::Unspecified, why.to_string());
    let saved = Backlog::load(project).map_err(failed)?;
    let action = Action::try_from(edit.action).unwrap_or(Action::Unspecified);
    let exists = saved.notes.iter().any(|note| note.id == edit.note_id);
    // Every change but a new note names one that is there: the library
    // would otherwise do nothing, or — for a save — add it.
    let new = action == Action::Save && edit.note_id.is_empty();
    if !new && !exists {
        return Err((
            proto::ErrorCode::NotFound,
            "no such note in the backlog".to_owned(),
        ));
    }
    match action {
        Action::Save => {
            let title = edit.title.trim();
            let body = edit.body.trim();
            if title.is_empty() && body.is_empty() {
                return Err((
                    proto::ErrorCode::Unspecified,
                    "write what the work is first".to_owned(),
                ));
            }
            // An existing note starts as saved, so what a phone is not sent —
            // its tags — is saved back as it was rather than cleared.
            let mut note = match new {
                true => Note::new(&saved.notes),
                false => saved
                    .notes
                    .iter()
                    .find(|note| note.id == edit.note_id)
                    .cloned()
                    .unwrap_or_default(),
            };
            title.clone_into(&mut note.title);
            body.clone_into(&mut note.body);
            // A phone that names no priority leaves the note's as it was.
            if let Some(priority) = asked_priority(edit.priority) {
                note.priority = priority;
            }
            Backlog::save_note(project, &note).map_err(failed)
        }
        Action::Done => Backlog::mark_done(project, &edit.note_id, Done::by_hand()).map_err(failed),
        Action::Reopen => Backlog::reopen(project, &edit.note_id).map_err(failed),
        Action::Remove => Backlog::remove(project, &edit.note_id).map_err(failed),
        Action::Unspecified => Err((
            proto::ErrorCode::Unspecified,
            "not a change this desktop knows; update ket".to_owned(),
        )),
    }
}

/// What a person's Respond decides about the prompt it names, or `None` when
/// it does not fit what the prompt asked: a pick for every question, one
/// choice where only one is allowed, choices that exist.
pub(super) fn decision(
    asked: &crate::agent_hooks::PermissionQuestion,
    respond: &proto::Respond,
) -> Option<Decision> {
    match asked.prompt.as_ref()? {
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
    };
    error(id, code, message)
}

/// A changed file, on the phone protocol.
fn file_change(file: crate::review::FileChange) -> proto::FileChange {
    let (added, removed) = file.lines.unwrap_or_default();
    proto::FileChange {
        path: file.path,
        added,
        removed,
        status: match file.status {
            crate::review::Status::Modified => proto::file_change::Status::Modified,
            crate::review::Status::Added => proto::file_change::Status::Added,
            crate::review::Status::Deleted => proto::file_change::Status::Deleted,
        } as i32,
        binary: file.lines.is_none(),
    }
}

/// What the agent in terminal `terminal_id` has said since cursor `after` of
/// its session — see `crate::conversation`: the session its hooks named for
/// the terminal's pane, for each agent ket can read.
fn conversation(host: &Host, terminal_id: u64, after: u64) -> proto::Conversation {
    let unsupported = |reason: String| proto::Conversation {
        terminal_id,
        supported: false,
        reason,
        ..Default::default()
    };
    let Some(key) = host.key(terminal_id) else {
        return unsupported("that terminal has closed".to_owned());
    };
    let status = host.agent_status();
    let Some(pane) = status.panes.iter().find(|pane| pane.pane == key) else {
        return unsupported("no agent has reported from this terminal yet".to_owned());
    };
    if !crate::conversation::Transcript::readable(&pane.agent) {
        return unsupported(format!(
            "{}'s sessions can't be read as a conversation yet",
            pane.agent
        ));
    }
    let Some(session) = pane.session.clone() else {
        return unsupported("the agent hasn't said which session this is yet".to_owned());
    };
    let Some(transcript) = crate::conversation::Transcript::locate(&pane.agent, &session) else {
        return unsupported("the session's transcript isn't there yet".to_owned());
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
