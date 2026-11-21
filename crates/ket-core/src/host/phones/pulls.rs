//! What a phone asks of a worktree's pull request: opening it, merging,
//! updating, readying and reopening it, its failed checks and its review
//! comments given to the agent, and a review of the diff sent.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use ket_remote::proto::{self, envelope::Payload};

use super::places::{Place, Places};
use super::requests::{FixAsk, Fixes, envelope, error};
use super::session::Phone;
use super::{Event, lock};
use crate::github::{MergeMethod, PullRequestStatus};
use crate::host::server::{Handed, Host};
use crate::id::{ProjectId, WorktreeId};

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

/// Address comments with agent, kept the same way as [`FIXES`].
static COMMENTS: Mutex<Fixes> = Mutex::new(Fixes {
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

/// Merge, update, ready and reopen, kept the same way as [`PULLS`].
static PR_ACTIONS: Mutex<Fixes> = Mutex::new(Fixes {
    asks: BTreeMap::new(),
    going: BTreeSet::new(),
    finished: VecDeque::new(),
});

pub(super) fn fix_checks_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    fix: &proto::FixChecks,
) -> Option<proto::Envelope> {
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

/// Written into one message by a window, as the desktop's own Send to
/// agent does, with the same refusals. A window takes it within a
/// quarter of a second, but one that is busy is waited for longer,
/// off this thread — see `Host::send_review` — and the answer goes to
/// whoever is waiting for it then: see `REVIEWS`.
pub(super) fn review_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    review: &proto::SendReview,
) -> Option<proto::Envelope> {
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

pub(super) fn address_comments_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    worktree: &str,
) -> Option<proto::Envelope> {
    let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
    if lock(&COMMENTS).again(&ask, id, feed) {
        return None;
    }
    let places = phone.phones.places();
    let (place, status) = match pr_place(&places, host, worktree) {
        Ok(found) => found,
        Err((code, why)) => return Some(error(id, code, why)),
    };
    if !status.can_address_comments() {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "this pull request has no review comments to address",
        ));
    }
    if !host.has_window() {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "open ket on your Mac to address comments from here",
        ));
    }
    {
        let mut comments = lock(&COMMENTS);
        if comments.again(&ask, id, feed) {
            return None;
        }
        if !comments.start(ask.clone(), worktree, id, feed) {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "ket is already giving this pull request's comments to the agent",
            ));
        }
    }
    let project = ProjectId::new(place.project.clone());
    let owned = worktree.to_owned();
    let host = Arc::clone(host);
    let running = ask.clone();
    let spawned = std::thread::Builder::new()
        .name("ket-host-address-comments".into())
        .spawn(move || {
            let answer = address_comments(&host, &project, owned.clone(), &status);
            lock(&COMMENTS).finish(running, &owned, answer);
        });
    match spawned {
        Ok(_) => None,
        Err(why) => {
            lock(&COMMENTS).forget(&ask, worktree);
            Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("could not start reading the comments: {why}"),
            ))
        }
    }
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

fn pr_place<'a>(
    places: &'a super::places::Places,
    host: &Host,
    worktree: &str,
) -> std::result::Result<
    (&'a super::places::Place, PullRequestStatus),
    (proto::ErrorCode, &'static str),
> {
    match places.get(worktree) {
        None => Err((
            proto::ErrorCode::NotFound,
            "no such worktree on this desktop",
        )),
        Some(place) if place.base.is_none() => Err((
            proto::ErrorCode::Unspecified,
            "the repository's own checkout has no pull request of its own",
        )),
        Some(_) => match host.pull_request(worktree) {
            None => Err((
                proto::ErrorCode::NotFound,
                "ket on your Mac has not found a pull request for this worktree",
            )),
            Some(status) => Ok((places.get(worktree).expect("still there"), status)),
        },
    }
}

fn address_comments(
    host: &Host,
    project: &ProjectId,
    worktree: String,
    status: &PullRequestStatus,
) -> Payload {
    let failed = |message: String| {
        Payload::Error(proto::Error {
            code: proto::ErrorCode::Unspecified as i32,
            message,
        })
    };
    let handed = crate::checks::checkout(project).and_then(|at| {
        crate::address_comments::hand_over(&at, &WorktreeId::new(worktree.as_str()), status)
    });
    let prompt = match handed {
        Ok(prompt) => prompt,
        Err(why) => {
            tracing::warn!(%why, "could not read review comments for a phone");
            return failed(format!(
                "could not read #{}'s review comments: {why}",
                status.number
            ));
        }
    };
    match host.fix_checks(worktree, prompt) {
        Handed::Given => Payload::CommentsHandedOver(proto::CommentsHandedOver { starting: false }),
        Handed::Starting => {
            Payload::CommentsHandedOver(proto::CommentsHandedOver { starting: true })
        }
        Handed::Failed(why) => failed(why),
    }
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

pub(super) fn draft_from_phone(
    phone: &Phone<'_>,
    id: u64,
    ask: &proto::GetPullRequestDraft,
) -> Option<proto::Envelope> {
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

pub(super) fn open_pull_request_from_phone(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    open: &proto::OpenPullRequest,
) -> Option<proto::Envelope> {
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
            state: crate::github::PullRequestState::Open,
            base: report.base.clone(),
            behind: false,
            unresolved_comments: 0,
            merge_methods: crate::github::MergeMethods::default(),
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

#[derive(Clone, Copy)]
pub(super) enum PrAction {
    Merge(i32),
    Update,
    Ready,
    Reopen,
}

pub(super) fn start_pr_action(
    host: &Arc<Host>,
    phone: &Phone<'_>,
    request: &proto::Envelope,
    feed: &mpsc::Sender<Event>,
    id: u64,
    worktree: &str,
    action: PrAction,
) -> Option<proto::Envelope> {
    let ask: FixAsk = (phone.device.clone(), request.idempotency_key.clone());
    if lock(&PR_ACTIONS).again(&ask, id, feed) {
        return None;
    }
    let places = phone.phones.places();
    let (place, status) = match pr_place(&places, host, worktree) {
        Ok(found) => found,
        Err((code, why)) => return Some(error(id, code, why)),
    };
    if let Err(why) = action.allowed_for(&status) {
        return Some(error(id, proto::ErrorCode::Unspecified, why));
    }
    {
        let mut actions = lock(&PR_ACTIONS);
        if actions.again(&ask, id, feed) {
            return None;
        }
        if !actions.start(ask.clone(), worktree, id, feed) {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "ket is already changing this pull request",
            ));
        }
    }
    let project = ProjectId::new(place.project.clone());
    let owned = worktree.to_owned();
    let host = Arc::clone(host);
    let running = ask.clone();
    let spawned = std::thread::Builder::new()
        .name("ket-host-pr-action".into())
        .spawn(move || {
            let answer = run_pr_action(&host, &project, &owned, status, action);
            lock(&PR_ACTIONS).finish(running, &owned, answer);
        });
    match spawned {
        Ok(_) => None,
        Err(why) => {
            lock(&PR_ACTIONS).forget(&ask, worktree);
            Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("could not start: {why}"),
            ))
        }
    }
}

impl PrAction {
    fn allowed_for(self, status: &PullRequestStatus) -> std::result::Result<(), &'static str> {
        match self {
            Self::Merge(_) if !status.can_merge() => Err("this pull request cannot be merged yet"),
            Self::Update if !status.can_update() => {
                Err("this pull request's branch is not behind its base")
            }
            Self::Ready if !status.can_ready() => Err("this pull request is not a draft"),
            Self::Reopen if !status.can_reopen() => Err("this pull request is not closed"),
            _ => Ok(()),
        }
    }
}

fn run_pr_action(
    host: &Host,
    project: &ProjectId,
    worktree: &str,
    status: PullRequestStatus,
    action: PrAction,
) -> Payload {
    let failed = |message: String| {
        Payload::Error(proto::Error {
            code: proto::ErrorCode::Unspecified as i32,
            message,
        })
    };
    let at = match crate::checks::checkout(project) {
        Ok(at) => at,
        Err(why) => return failed(why.to_string()),
    };
    let method = match action {
        PrAction::Merge(raw) => match pick_merge_method(raw, status.merge_methods) {
            Ok(method) => Some(method),
            Err(why) => return failed(why.to_owned()),
        },
        _ => None,
    };
    let ran = match action {
        PrAction::Merge(_) => {
            crate::github::merge_pull_request(&at, status.number, method.expect("picked"))
        }
        PrAction::Update => crate::github::update_branch(&at, status.number),
        PrAction::Ready => crate::github::mark_ready(&at, status.number),
        PrAction::Reopen => crate::github::reopen_pull_request(&at, status.number),
    };
    if let Err(why) = ran {
        tracing::info!(worktree, %why, "a phone's pull request action was refused");
        return failed(why.to_string());
    }
    let mut next = crate::github::pull_request(&at, status.number).unwrap_or_else(|_| {
        let mut next = status.clone();
        match action {
            PrAction::Merge(_) => next.state = crate::github::PullRequestState::Merged,
            PrAction::Ready => next.draft = false,
            PrAction::Reopen => next.state = crate::github::PullRequestState::Open,
            PrAction::Update => next.behind = false,
        }
        next
    });
    next.merge_methods = status.merge_methods;
    host.replace_pull_request(worktree, next.clone());
    match action {
        PrAction::Merge(_) => Payload::PullRequestMerged(proto::PullRequestMerged {
            number: next.number,
            url: next.url,
            method: match method.expect("picked") {
                MergeMethod::Merge => proto::PullRequestMergeMethod::MergeCommit as i32,
                MergeMethod::Squash => proto::PullRequestMergeMethod::Squash as i32,
                MergeMethod::Rebase => proto::PullRequestMergeMethod::Rebase as i32,
            },
        }),
        PrAction::Update => Payload::PullRequestUpdated(proto::PullRequestUpdated {
            number: next.number,
        }),
        PrAction::Ready => Payload::PullRequestReadied(proto::PullRequestReadied {
            number: next.number,
        }),
        PrAction::Reopen => Payload::PullRequestReopened(proto::PullRequestReopened {
            number: next.number,
        }),
    }
}

fn pick_merge_method(
    raw: i32,
    allowed: crate::github::MergeMethods,
) -> std::result::Result<MergeMethod, &'static str> {
    let method = match proto::PullRequestMergeMethod::try_from(raw) {
        Ok(proto::PullRequestMergeMethod::Unspecified) | Err(_) => allowed
            .default_method()
            .ok_or("this repository allows no merge methods")?,
        Ok(proto::PullRequestMergeMethod::MergeCommit) => MergeMethod::Merge,
        Ok(proto::PullRequestMergeMethod::Squash) => MergeMethod::Squash,
        Ok(proto::PullRequestMergeMethod::Rebase) => MergeMethod::Rebase,
    };
    if allowed.allows(method) {
        Ok(method)
    } else {
        Err("that merge method is not allowed on this repository")
    }
}

/// A changed file, on the phone protocol.
pub(super) fn file_change(file: crate::review::FileChange) -> proto::FileChange {
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
