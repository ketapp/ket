//! The host's side of the socket.
//!
//! Plain threads, like the rest of the terminal code: one accepting, one per
//! connected window reading its requests, one per terminal a window is
//! watching, and one for housekeeping. A window watching a terminal gets a
//! checkpoint first and then its output as it arrives; when it falls behind
//! the replay buffer, or the screen is resized or cleared, it gets a fresh
//! checkpoint instead.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::wire::{self, Exit, Frame, Reply, Request, Running, WorkSource};
use super::{EXITED_KEPT, IDLE_EXIT, host_error};

use crate::Result;
use crate::agent_hooks::{self, HookReport, Listener, StatusLine};
use crate::agent_status::{CancelSource, StatusStore};
use crate::terminal::{LocalTerminal, OutputCursor, Since, StopOutcome, TerminalSize};
#[cfg(test)]
use housekeeping::pass_on_hooks;
use housekeeping::{forward_hooks, housekeeping, send_hooks};

mod housekeeping;

/// How often housekeeping runs: foregrounds, reaping, idleness.
const TICK: Duration = Duration::from_secs(1);

/// How often the network inspector's logs are swept of what has outlived
/// [`crate::network::store::KEPT`].
const NETWORK_SWEEP: Duration = Duration::from_secs(60 * 60);

/// How often hook reports are passed on to windows.
const HOOK_TICK: Duration = Duration::from_millis(200);

/// How long a phone's Fix checks with agent waits, after its logs are read,
/// for the window to say the agent has the message — long enough to set up a
/// worktree and start its agent, most times. Past it the phone is told the
/// agent is still starting, not that it failed: the window carries on, and
/// asking again would give the agent the message twice.
const FIX_CHECKS_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(60)
};

/// How often a phone's fix, waiting on a window, looks to see the window is
/// still there.
const FIX_CHECKS_LOOK: Duration = Duration::from_millis(250);

/// How long a phone's review waits for a window to say the agent has it. A
/// window takes one within a quarter of a second, so this is for one that is
/// busy, or too old to know the request.
const REVIEW_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(10)
};

/// How much sooner than [`REVIEW_WAIT`] a window has to take a review: one
/// that takes it later drops it, so a phone told it did not go can send it
/// again without the agent getting it twice. The rest is for the answer's
/// way back.
const REVIEW_GRACE: Duration = Duration::from_millis(1500);

/// How long a phone's delete waits for the window to say the worktree is
/// gone. A delete walks the checkout and asks git twice, which takes seconds
/// on a big tree; past this the phone is told it did not hear back.
const DELETE_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(120)
};

/// How long a phone's wake waits for the window to open the tab's terminal.
/// Waking resumes an agent, which reads its session back first; past this
/// the phone is told it did not hear back, and the tab wakes regardless.
const WAKE_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(45)
};

/// How long `ket browser` waits for the window to say what the page did. The
/// window gives a load ten seconds; a screenshot of a long page, or a slow
/// evaluation, can take a few more. The CLI waits a little longer than this
/// in all, so it hears why from the host rather than giving up first.
const BROWSER_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(20)
};

/// What a window says of `ket browser`'s request: what the page did, or why
/// not.
pub(super) type Browsed = std::result::Result<wire::BrowserAnswer, String>;

/// A `ket browser` request waiting for its answer: the window it went to,
/// the only connection whose answer counts, and where the answer goes.
type BrowserWaiter = (Arc<Out>, mpsc::Sender<Browsed>);

/// What a phone's wake comes to: the terminal the tab is now, or why not.
pub(super) type Woken = std::result::Result<u64, String>;

/// What a window says of a phone's Start fresh: the fresh tab's key, or why
/// not.
type Freshened = std::result::Result<String, String>;

/// What a window says of a phone's delete: the branch it kept, or why not.
pub(super) type Deleted = std::result::Result<Option<String>, String>;

/// A phone's request waiting on a window: the window it went to, the only
/// connection whose answer counts — see [`answered`] — and where the answer
/// goes.
type Waiter<T> = (Arc<Out>, mpsc::Sender<T>);

/// A phone's wake waiting on a window: a [`Waiter`] that keeps the tab's key.
type WakeWaiter = (Arc<Out>, (String, mpsc::Sender<Woken>));

/// Why a phone's delete did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotDeleted {
    /// The window tried, and git refused — for an unforced delete, because
    /// work would be lost.
    Refused(String),
    /// It never got that far: no window, or none that answered.
    Failed(String),
}

/// How a window took a phone's failed checks — see [`Host::fix_checks`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Handed {
    /// The worktree's agent has the message.
    Given,
    /// No agent was running, and the one the window is starting has not
    /// taken the message yet. It will once it is up.
    Starting,
    /// It could not be given, and why.
    Failed(String),
}

/// Hook reports kept while no window is connected, oldest dropped first.
const BACKLOG_REPORTS: usize = 2000;

/// Status lines kept the same way. Only the latest few matter: each one
/// replaces the reading before it.
const BACKLOG_LINES: usize = 32;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every connected window, by connection serial — see `Host::outs`.
type Outs = Arc<Mutex<Vec<(u64, Arc<Out>)>>>;

/// Writes to one window. Shared by every thread with something to tell it.
struct Out {
    stream: Mutex<UnixStream>,
    alive: AtomicBool,
    /// Set once the window has been welcomed. Nothing addressed to every
    /// window goes to one before that: its first frame has to be the
    /// welcome, or it cannot tell which host it reached.
    greeted: AtomicBool,
    /// Whether the other end is a ket window rather than another program,
    /// such as the CLI — see `same_program`. Only a window can be handed
    /// work to start or merge, or the hook traffic kept for one.
    window: bool,
}

impl Out {
    /// Sends one frame; a window that has gone is marked so, not an error.
    fn send(&self, frame: &[u8]) -> bool {
        if !self.alive.load(Ordering::Acquire) {
            return false;
        }
        if wire::send(&mut *lock(&self.stream), frame).is_err() {
            self.alive.store(false, Ordering::Release);
            return false;
        }
        true
    }

    fn reply(&self, reply: &Reply) -> bool {
        wire::json(reply).is_ok_and(|frame| self.send(&frame))
    }
}

/// A window watching one terminal.
struct Watcher {
    out: Arc<Out>,
    /// Set when the window lets go of the terminal, or goes away.
    detached: Arc<AtomicBool>,
    /// Which connection, so it can be detached when that connection ends.
    conn: u64,
}

impl Watcher {
    fn watching(&self) -> bool {
        !self.detached.load(Ordering::Acquire) && self.out.alive.load(Ordering::Acquire)
    }
}

/// One terminal the host owns.
struct Entry {
    terminal: Arc<LocalTerminal>,
    key: Option<String>,
    watchers: Vec<Watcher>,
    /// When its program was seen to have finished.
    closed_at: Option<Instant>,
    /// Killed on a window's say-so: reaped as soon as it has closed.
    killed: bool,
    /// The foreground last sent, so only changes are.
    foreground: Option<Running>,
    /// Which phone session controls input, and until when — see
    /// [`Host::acquire`]. `None` is the desktop, which never needs one.
    lease: Option<(u64, Instant)>,
    /// The size the desktop last gave it, kept while a phone has it fitted
    /// to the phone's screen, so the desktop's comes back when the phone
    /// closes it. See [`Host::fit`].
    window_size: Option<TerminalSize>,
    /// The phone session that has it resized to its own screen now, if one
    /// does.
    fitted: Option<u64>,
    /// When a window last typed into it, milliseconds. A permission prompt
    /// reported before that may already have been answered at the desktop —
    /// see [`Host::answer_prompt`].
    typed_at_ms: u64,
    /// The prompt whose answer ket last typed from a phone. The screen
    /// still shows it until the agent reads the keys, so this is what keeps
    /// a second phone, or a retry, from typing the answer again — into
    /// whatever comes next. See [`Host::respond_prompt`].
    typed_answer: Option<u64>,
}

/// Why [`Host::answer_prompt`] pressed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnswerRefused {
    /// No such terminal, or it has closed.
    Gone,
    /// Its agent is not waiting on a prompt now, or somebody typed into it at
    /// the desktop since the prompt was reported.
    Answered,
    /// It is waiting on a different prompt from the one named.
    Changed,
    /// No prompt was named — a phone older than the check.
    Unnamed,
    /// ket does not know this agent's keys for this prompt, or — for a
    /// question or a plan — its hook is not being held to answer through.
    Unknown,
    /// The answer does not fit what the prompt asked.
    Unfit,
    /// The prompt on screen has no option for that answer: Grok offers no
    /// Always allow for a chained command.
    NotOffered,
}

impl Entry {
    /// Puts the desktop's size back after a phone had the terminal fitted.
    fn unfit(&mut self) {
        if self.fitted.take().is_none() {
            return;
        }
        if let Some(size) = self.window_size {
            let _ = self.terminal.resize(size);
        }
    }
}

/// How long a phone's control of a terminal lasts without being renewed.
pub(super) const LEASE: Duration = Duration::from_secs(30);

pub(super) struct Host {
    terminals: Mutex<BTreeMap<u64, Entry>>,
    next_id: AtomicU64,
    next_conn: AtomicU64,
    clients: AtomicUsize,
    /// When the host last had a window or a running terminal.
    busy_at: Mutex<Instant>,
    build: String,
    /// Every connected window, for what is not about one terminal. Shared
    /// with the network inspector's log thread, which tells the windows of
    /// each request it writes down.
    outs: Outs,
    /// Hook traffic that arrived with no window to hear it.
    backlog: Mutex<(VecDeque<HookReport>, VecDeque<StatusLine>)>,
    /// Where agents report, with credentials bound to terminal pane keys.
    hook_bindings: agent_hooks::HookBindings,
    /// The network inspector's proxy, once a terminal has asked for it —
    /// see [`crate::network`].
    network: Mutex<Option<Arc<crate::network::Proxy>>>,
    /// The proxy's tokens, each bound to the pane it was issued for.
    network_bindings: crate::network::NetworkBindings,
    /// The host listener's address and compatibility endpoint token.
    hooks: (u16, String),
    /// Questions and plans whose hooks the host's listener is holding open,
    /// to be answered from a phone — see [`agent_hooks::Waiters`].
    prompts: Arc<agent_hooks::Waiters>,
    /// The phone side, while phones are on — see `super::phones`. Turned on
    /// and off in place by [`Request::Phones`], without a restart.
    phones: Mutex<Option<Arc<super::phones::Phones>>>,
    /// What the agents in the host's terminals have said about themselves.
    ///
    /// The host runs them, so the host is the authority on them — windows
    /// read snapshots of this rather than each reapplying the same hooks and
    /// drifting apart. See [`crate::agent_status`].
    status: Mutex<StatusStore>,
    /// Each connected window's worktrees, by connection. Merged into
    /// `status` — see [`Host::merge_worktrees`].
    registries: Mutex<BTreeMap<u64, Vec<(String, PathBuf)>>>,
    /// Each connected window's asleep tabs, by connection, for phones — see
    /// [`Request::Asleep`]. Kept by connection so a wake goes to the window
    /// that has the tab.
    asleep: Mutex<BTreeMap<u64, Vec<wire::AsleepTab>>>,
    /// Phone wakes waiting for the window asked to open a terminal under the
    /// tab's key, or to say why it won't.
    wake_waiting: Mutex<BTreeMap<u64, WakeWaiter>>,
    next_wake: AtomicU64,
    /// Phone Start freshes waiting for the window to name the tab it started
    /// fresh, or to say why it didn't.
    fresh_waiting: Mutex<BTreeMap<u64, mpsc::Sender<Freshened>>>,
    next_fresh: AtomicU64,
    /// The agents' plan usage, as a window last reported it, for phones. The
    /// host fetches none itself: a window already does, and a second fetcher
    /// would double the processes that costs.
    usage: Mutex<Vec<crate::rate_limits::ProviderSnapshot>>,
    /// The agents a window can start, as one last reported them, for phones.
    /// The host detects none itself: which are enabled is the window's
    /// settings, and detecting them shells out.
    agents: Mutex<Vec<String>>,
    /// The theme a window last said it is drawn in, for phones. `None` until
    /// one has.
    theme: Mutex<Option<crate::theme::Theme>>,
    /// The pull requests worktrees' branches are open as, with their checks,
    /// as a window last read them from GitHub — by worktree id, for phones.
    /// Like `usage`, the host asks GitHub nothing itself.
    pull_requests: Mutex<Vec<(String, crate::github::PullRequestStatus)>>,
    /// Phone requests waiting for a window to report that the agent started.
    fix_checks_waiting: Mutex<BTreeMap<u64, Waiter<std::result::Result<(), String>>>>,
    next_fix_checks: AtomicU64,
    /// Phone reviews waiting for a window to say the agent has them.
    review_waiting: Mutex<BTreeMap<u64, Waiter<std::result::Result<(), String>>>>,
    next_review: AtomicU64,
    /// Phone deletes waiting for a window to say the worktree is gone.
    delete_waiting: Mutex<BTreeMap<u64, Waiter<Deleted>>>,
    next_delete: AtomicU64,
    /// `ket browser` requests waiting for a window to say what the page did,
    /// each with the window it went to — see [`Host::finish_browser`].
    browser_waiting: Mutex<BTreeMap<u64, BrowserWaiter>>,
    next_browser: AtomicU64,
}

impl Host {
    fn live(&self) -> usize {
        lock(&self.terminals)
            .values()
            .filter(|entry| !entry.terminal.is_closed())
            .count()
    }

    /// One terminal, by the host's id for it.
    pub(super) fn find(&self, id: u64) -> Option<Arc<LocalTerminal>> {
        terminal(self, id)
    }

    /// The phone side, while phones are on.
    fn phones(&self) -> Option<Arc<super::phones::Phones>> {
        lock(&self.phones).clone()
    }

    /// Turns phones on or off in place. On runs the relay on the Mac at the
    /// configured port — or dials `KET_RELAY_URL`, when that is set — and
    /// links to the relay elsewhere the config names, and off ends every
    /// phone's session and stops listening; no terminal is touched either
    /// way. Asked to turn on while on, it takes up a change to the relay
    /// elsewhere. Returns whether phones are on afterwards.
    fn set_phones(self: &Arc<Self>, enabled: bool) -> Result<bool> {
        let mut slot = lock(&self.phones);
        let config = crate::config::Config::load().unwrap_or_default().phones;
        match (enabled, slot.is_some()) {
            (true, false) => {
                let relay = match super::relay_url() {
                    Some(url) => super::phones::Relay::External(url),
                    None => super::phones::Relay::Local { port: config.port },
                };
                *slot = Some(super::phones::start(self.clone(), relay)?);
            }
            (false, true) => {
                if let Some(phones) = slot.take() {
                    phones.stop();
                }
            }
            _ => {}
        }
        // The relay elsewhere as the config has it now, linked, changed or
        // dropped in place (#142). None beside a development relay.
        if let Some(phones) = slot.as_ref() {
            let hosted = super::relay_url()
                .is_none()
                .then(|| config.hosted_relay())
                .flatten();
            phones.set_hosted(self, hosted);
            phones.set_push_config(&config);
        }
        Ok(slot.is_some())
    }

    /// Gives phone session `holder` control of a terminal's input for
    /// [`LEASE`], or extends it. `false` while another phone holds a lease
    /// that has not lapsed.
    pub(super) fn acquire(&self, id: u64, holder: u64) -> bool {
        let mut terminals = lock(&self.terminals);
        let Some(entry) = terminals.get_mut(&id) else {
            return false;
        };
        let now = Instant::now();
        match entry.lease {
            Some((other, until)) if other != holder && until > now => false,
            _ => {
                entry.lease = Some((holder, now + LEASE));
                true
            }
        }
    }

    /// Whether phone session `holder` controls a terminal's input now.
    pub(super) fn controls(&self, id: u64, holder: u64) -> bool {
        lock(&self.terminals)
            .get(&id)
            .and_then(|entry| entry.lease)
            .is_some_and(|(who, until)| who == holder && until > Instant::now())
    }

    /// Gives up phone session `holder`'s control of a terminal, if it has it.
    pub(super) fn release(&self, id: u64, holder: u64) {
        if let Some(entry) = lock(&self.terminals).get_mut(&id)
            && entry.lease.is_some_and(|(who, _)| who == holder)
        {
            entry.lease = None;
        }
    }

    /// Resizes a terminal to a phone's screen, for phone session `holder`,
    /// whether or not it has control: a phone that fits watches at its own
    /// size too. The desktop's size is kept and comes back when that phone
    /// closes the terminal or goes — see [`Host::unfit`]. `false` while
    /// another phone has control, whose screen it is then.
    pub(super) fn fit(&self, id: u64, holder: u64, size: TerminalSize) -> bool {
        let mut terminals = lock(&self.terminals);
        let Some(entry) = terminals.get_mut(&id) else {
            return false;
        };
        if entry
            .lease
            .is_some_and(|(who, until)| who != holder && until > Instant::now())
        {
            return false;
        }
        if entry.fitted.is_none() {
            entry.window_size.get_or_insert(entry.terminal.size());
        }
        entry.fitted = Some(holder);
        let _ = entry.terminal.resize(size);
        true
    }

    /// Gives a terminal the desktop's size back, if phone session `holder`
    /// is the one that fitted it.
    pub(super) fn unfit(&self, id: u64, holder: u64) {
        if let Some(entry) = lock(&self.terminals).get_mut(&id)
            && entry.fitted == Some(holder)
        {
            entry.unfit();
        }
    }

    /// The key terminal `id` was filed under, if it has one.
    pub(super) fn key(&self, id: u64) -> Option<String> {
        lock(&self.terminals)
            .get(&id)
            .and_then(|entry| entry.key.clone())
    }

    /// The keys of the terminals still running, which are the panes the
    /// status store can file a report under.
    fn pane_keys(&self) -> HashSet<String> {
        lock(&self.terminals)
            .values()
            .filter(|entry| !entry.killed && !entry.terminal.is_closed())
            .filter_map(|entry| entry.key.clone())
            .collect()
    }

    /// Sends every window the status store as it is now.
    fn broadcast_status(&self) {
        let reply = Reply::AgentStatus {
            snapshot: Box::new(lock(&self.status).snapshot()),
        };
        let Ok(frame) = wire::json(&reply) else {
            return;
        };
        for out in greeted(&self.outs) {
            out.send(&frame);
        }
    }

    /// The status snapshot phones project into their own wire format.
    pub(super) fn agent_status(&self) -> crate::agent_status::AgentStatusSnapshot {
        lock(&self.status).snapshot()
    }

    /// The session resumed in pane `pane` when it was opened, if one was.
    pub(super) fn bound_session(&self, pane: &str) -> Option<String> {
        lock(&self.status).bound_session(pane)
    }

    /// The agents' plan usage, as a window last reported it.
    /// The theme a window last said it is drawn in.
    pub(super) fn theme(&self) -> Option<crate::theme::Theme> {
        *lock(&self.theme)
    }

    pub(super) fn usage(&self) -> Vec<crate::rate_limits::ProviderSnapshot> {
        lock(&self.usage).clone()
    }

    /// The agents a window can start, as one last reported them.
    pub(super) fn agents(&self) -> Vec<String> {
        lock(&self.agents).clone()
    }

    /// The pull request `worktree`'s branch is open as, as a window last
    /// read it.
    pub(super) fn pull_request(&self, worktree: &str) -> Option<crate::github::PullRequestStatus> {
        lock(&self.pull_requests)
            .iter()
            .find(|(id, _)| id == worktree)
            .map(|(_, status)| status.clone())
    }

    /// Takes a pull request a phone just opened for `worktree`, so phones
    /// see it now rather than once a window reads GitHub again — unless a
    /// window has read one there already. The next read a window sends
    /// replaces the lot, with its checks.
    pub(super) fn remember_pull_request(
        &self,
        worktree: &str,
        status: crate::github::PullRequestStatus,
    ) {
        let mut open = lock(&self.pull_requests);
        if !open.iter().any(|(id, _)| id == worktree) {
            open.push((worktree.to_owned(), status));
        }
    }

    /// Replaces `worktree`'s pull request with one a phone just merged,
    /// updated, readied or reopened, so phones see that now.
    pub(super) fn replace_pull_request(
        &self,
        worktree: &str,
        status: crate::github::PullRequestStatus,
    ) {
        let mut open = lock(&self.pull_requests);
        if let Some(existing) = open.iter_mut().find(|(id, _)| id == worktree) {
            existing.1 = status;
        } else {
            open.push((worktree.to_owned(), status));
        }
    }

    /// The network inspector's proxy, started the first time a terminal asks
    /// for it. `None` when it cannot start: the terminal then goes straight
    /// to its provider, as if it had not asked.
    fn network_proxy(&self) -> Option<Arc<crate::network::Proxy>> {
        let mut proxy = lock(&self.network);
        if proxy.is_none() {
            // Each request written down goes to every window, as the hook
            // reports do, so a Network panel adds the row as it lands.
            let outs = self.outs.clone();
            let told: crate::network::Told = Arc::new(move |worktree: &str, summary| {
                let reply = Reply::NetworkRequest {
                    worktree: worktree.to_owned(),
                    summary: Box::new(summary.clone()),
                };
                let Ok(frame) = wire::json(&reply) else {
                    return;
                };
                for out in windows(&outs) {
                    out.send(&frame);
                }
            });
            match crate::network::Proxy::start(self.network_bindings.clone(), told) {
                Ok(started) => {
                    tracing::info!(port = started.port(), "network inspector listening");
                    *proxy = Some(Arc::new(started));
                }
                Err(error) => tracing::warn!(%error, "network inspector could not start"),
            }
        }
        proxy.clone()
    }

    /// Points the agent `spec` launches at the network inspector, when it is
    /// one ket can inspect and nothing already sends it elsewhere.
    fn inspect(&self, spec: &mut crate::terminal::TerminalSpec) {
        let Some(pane) = spec.key.clone() else {
            return;
        };
        let agent = pane.split('|').nth(1).unwrap_or_default().to_owned();
        let Some(route) = crate::network::route_for(&agent) else {
            return;
        };
        if crate::network::sent_elsewhere(route, spec) {
            tracing::info!(%pane, "network inspector: the agent is sent elsewhere already; left as it is");
            return;
        }
        let Some(proxy) = self.network_proxy() else {
            return;
        };
        let token = self.network_bindings.bind(crate::network::Binding {
            pane: pane.clone(),
            agent,
            route,
        });
        if !crate::network::point(route, &proxy.base(&token), spec) {
            self.network_bindings.forget(&token);
            tracing::info!(%pane, "network inspector: no command line to point the agent from; left as it is");
        }
    }

    /// Forgets the panes of terminals that have gone, and tells the windows.
    fn clear_panes(&self, keys: &[String]) {
        self.hook_bindings.unbind(keys);
        self.network_bindings.unbind(keys);
        let changed = {
            let mut status = lock(&self.status);
            keys.iter()
                .fold(false, |changed, key| status.clear(key) | changed)
        };
        if changed {
            self.broadcast_status();
        }
    }

    /// A cancel verdict for the terminal the host calls `id` — a phone's
    /// interrupt, which reaches the host without passing a window.
    pub(super) fn cancel_terminal(&self, id: u64, source: CancelSource) {
        let Some(key) = lock(&self.terminals).get(&id).and_then(|e| e.key.clone()) else {
            return;
        };
        if lock(&self.status).cancel(&key, source, crate::now_ms()) {
            self.broadcast_status();
        }
    }

    /// The terminal the host calls `id` is in front of someone on a phone:
    /// what its agent last said is read.
    pub(super) fn seen_terminal(&self, id: u64) {
        let Some(key) = lock(&self.terminals).get(&id).and_then(|e| e.key.clone()) else {
            return;
        };
        if lock(&self.status).seen(&key) {
            self.broadcast_status();
        }
    }

    /// Marks `worktree` unread for every window and phone, or takes the mark
    /// off — see [`crate::agent_status::StatusStore::mark_unread`].
    pub(super) fn mark_unread(&self, worktree: &str, unread: bool) {
        if lock(&self.status).mark_unread(worktree, unread) {
            self.broadcast_status();
        }
    }

    /// Answers the permission prompt terminal `id` is showing — only if it
    /// is still prompt `question`, and nobody has typed into the terminal at
    /// the desktop since it was reported — by pressing the keys `keys_for`
    /// gives for its agent and tool.
    ///
    /// The check and the keys are one step: both locks are held from reading
    /// the prompt to writing the keys, so no hook report can replace the
    /// prompt and no desktop keystroke can answer it in between, and a key
    /// meant for one prompt cannot land in the next, or in the agent's input
    /// line. Terminals before status, the order the host takes them in
    /// everywhere.
    ///
    /// What remains is the agent itself moving on inside its TUI without a
    /// keystroke from anyone — a timeout — before its hook says so; nothing
    /// outside the agent can close that.
    ///
    /// Keys fixed per agent and tool; phones answer through
    /// [`Host::answer_prompt_on_screen`], which this is the fixed case of.
    #[cfg(test)]
    pub(super) fn answer_prompt(
        &self,
        id: u64,
        question: u64,
        keys_for: impl Fn(&str, &str) -> Option<&'static [u8]>,
    ) -> std::result::Result<(), AnswerRefused> {
        self.answer_prompt_on_screen(id, question, |agent, tool, _| {
            keys_for(agent, tool)
                .map(<[u8]>::to_vec)
                .ok_or(AnswerRefused::Unknown)
        })
    }

    /// [`Host::answer_prompt`], for keys that can depend on what the screen
    /// shows: `keys_for` is also given the bottom
    /// [`crate::screen::DIALOG_ROWS`] rows, read under the same locks if it
    /// asks for them — Grok's menu, whose digits change with the prompt.
    pub(super) fn answer_prompt_on_screen(
        &self,
        id: u64,
        question: u64,
        keys_for: impl FnOnce(
            &str,
            &str,
            &dyn Fn() -> Vec<String>,
        ) -> std::result::Result<Vec<u8>, AnswerRefused>,
    ) -> std::result::Result<(), AnswerRefused> {
        let terminals = lock(&self.terminals);
        let entry = terminals.get(&id).ok_or(AnswerRefused::Gone)?;
        let mut status = lock(&self.status);
        let (key, pane) = showing(entry, &status, question)?;
        let current = pane.question.as_ref().ok_or(AnswerRefused::Changed)?;
        let rows = || {
            entry
                .terminal
                .with_term(|term| crate::screen::bottom_rows(term, crate::screen::DIALOG_ROWS))
        };
        let keys = keys_for(&pane.agent, &current.tool, &rows)?;
        // Nothing to press is nothing known to press.
        if keys.is_empty() {
            return Err(AnswerRefused::Unknown);
        }
        // The key pressed is the prompt answered: the pane moves on now
        // rather than when the agent next reports, so a second tap finds
        // nothing to answer and types nothing. Escape declines it.
        let allowed = keys != [0x1b];
        let sent = entry.terminal.send_input(keys);
        let moved = sent.is_ok() && status.answered(&key, question, allowed, crate::now_ms());
        drop(status);
        drop(terminals);
        if moved {
            self.broadcast_status();
        }
        sent.map_err(|_| AnswerRefused::Gone)
    }

    /// Answers the question or plan terminal `id` is showing — only if it is
    /// still prompt `question`, on the same terms as
    /// [`Host::answer_prompt`] — with what `decide` makes of it, through the
    /// prompt's held hook rather than keys.
    ///
    /// The locks are held throughout for the same reason: the check and the
    /// answer are one step.
    pub(super) fn respond_prompt(
        &self,
        id: u64,
        question: u64,
        decide: impl FnOnce(&agent_hooks::PermissionQuestion) -> Option<agent_hooks::Decision>,
    ) -> std::result::Result<(), AnswerRefused> {
        let mut terminals = lock(&self.terminals);
        let entry = terminals.get_mut(&id).ok_or(AnswerRefused::Gone)?;
        let status = lock(&self.status);
        let (key, pane) = showing(entry, &status, question)?;
        let current = pane.question.as_ref().ok_or(AnswerRefused::Changed)?;
        if current.prompt.is_none() {
            return Err(AnswerRefused::Unknown);
        }
        // Typed already: the agent has the keys, and has not said so yet.
        if entry.typed_answer == Some(question) {
            return Err(AnswerRefused::Answered);
        }
        // A dialog read off the screen has no hook to answer through: its
        // option's number and Enter go to the terminal — while the screen
        // still shows it, read again now rather than trusted from a tick ago.
        if let Some(agent_hooks::Prompt::Dialog { title, options }) = &current.prompt {
            let Some(agent_hooks::Decision::Pick(at)) = decide(current) else {
                return Err(AnswerRefused::Unfit);
            };
            if at >= options.len() {
                return Err(AnswerRefused::Unfit);
            }
            let rows = entry
                .terminal
                .with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
            // The same dialog, option for option: a number means nothing
            // against a list that has changed since the phone saw it.
            let dialog = match crate::screen::codex_dialog(&rows) {
                Some(dialog)
                    if dialog.title == *title
                        && dialog.options == *options
                        && current.subject.as_deref() == Some(dialog.question.as_str()) =>
                {
                    dialog
                }
                _ => return Err(AnswerRefused::Changed),
            };
            entry
                .terminal
                .send_input(dialog.keys(at))
                .map_err(|_| AnswerRefused::Gone)?;
            entry.typed_answer = Some(question);
            return Ok(());
        }
        // Codex's question form: its hook cannot answer, so each question's
        // option number is typed in turn — a number picks and moves on, and
        // the last one sends. Only from the form's first question with
        // nothing answered, read now: from anywhere else the numbers would
        // land on the wrong questions.
        if current.tool == agent_hooks::CODEX_ASK_TOOL {
            let Some(agent_hooks::Prompt::Ask { questions }) = &current.prompt else {
                return Err(AnswerRefused::Unknown);
            };
            if !agent_hooks::codex_answerable(questions) {
                return Err(AnswerRefused::Unknown);
            }
            let Some(agent_hooks::Decision::Picks(picks)) = decide(current) else {
                return Err(AnswerRefused::Unfit);
            };
            let rows = entry
                .terminal
                .with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
            if !crate::screen::codex_user_input_from_start(&rows, questions.len()) {
                return Err(AnswerRefused::Changed);
            }
            let keys: String = picks.iter().map(|at| (at + 1).to_string()).collect();
            entry
                .terminal
                .send_input(keys.into_bytes())
                .map_err(|_| AnswerRefused::Gone)?;
            entry.typed_answer = Some(question);
            return Ok(());
        }
        let decision = decide(current).ok_or(AnswerRefused::Unfit)?;
        self.prompts
            .resolve(&key, current, &decision)
            .map_err(|why| match why {
                agent_hooks::Unresolved::NotHeld => AnswerRefused::Unknown,
                agent_hooks::Unresolved::Unfit => AnswerRefused::Unfit,
            })
    }

    /// Reads Codex panes waiting on a prompt, idle, or not yet heard from
    /// for dialogs, including folder access and hook review before the
    /// first session hook.
    /// Returns whether any status changed.
    ///
    /// Codex only, and never at work: the status is scanned once, and no
    /// terminal is touched unless its key names Codex and its status allows
    /// it. Read the visible screen under the parser's lock: startup dialogs
    /// can sit above empty rows rather than at the foot of the screen.
    fn read_codex_dialogs(&self) -> bool {
        let status = lock(&self.status).snapshot();
        let screens: Vec<(String, Arc<LocalTerminal>)> = lock(&self.terminals)
            .values()
            .filter_map(|entry| {
                let key = entry.key.as_ref()?;
                let pane = status.panes.iter().find(|pane| pane.pane == *key);
                let agent =
                    pane.map_or_else(|| key.split('|').nth(1), |pane| Some(pane.agent.as_str()))?;
                if !crate::agents::harness(agent).eq_ignore_ascii_case("codex")
                    || entry.terminal.is_closed()
                    || pane.is_some_and(|pane| {
                        !matches!(
                            pane.state,
                            crate::event::AgentState::AwaitingPermission
                                | crate::event::AgentState::Idle
                        )
                    })
                {
                    return None;
                }
                Some((key.clone(), entry.terminal.clone()))
            })
            .collect();
        let now = crate::now_ms();
        let mut changed = false;
        for (key, terminal) in screens {
            let rows = terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
            let dialog = crate::screen::codex_dialog(&rows);
            let mut status = lock(&self.status);
            if status.get(&key).is_none() {
                // Asked at startup, before any hook can report a session.
                if let Some(dialog) = dialog.filter(|dialog| {
                    dialog.title == "Folder access" || dialog.title == crate::screen::HOOK_REVIEW
                }) {
                    let mut parts = key.split('|');
                    if let (Some(worktree), Some(agent)) = (parts.next(), parts.next()) {
                        changed |= status.startup_dialog(&key, worktree, agent, dialog, now);
                    }
                }
            } else {
                changed |= status.screen_dialog(&key, dialog, now);
                changed |= status.codex_subagent_screen(&key, &rows, now);
            }
        }
        changed
    }

    /// Whether the question or plan in `pane` can be answered from a phone:
    /// its hook is held, and still listening.
    pub(super) fn prompt_held(
        &self,
        pane: &str,
        question: &agent_hooks::PermissionQuestion,
    ) -> bool {
        self.prompts.waiting(pane, question)
    }

    /// Lets go of held hooks whose prompt the pane is no longer showing —
    /// answered at the desktop, or replaced — so each one's script ends and
    /// prints nothing, rather than an answer arriving late. A hook whose
    /// script died while the status still showed its prompt is the prompt
    /// being dismissed unanswered, which Claude never reports; the status is
    /// told so the row stops saying "waiting". Returns whether it changed.
    fn release_prompts(&self) -> bool {
        if self.prompts.is_empty() {
            return false;
        }
        let status = lock(&self.status).snapshot();
        let dismissed = self.prompts.release_unless(|key, asked| {
            status.panes.iter().any(|pane| {
                pane.pane == key
                    && pane.state == crate::event::AgentState::AwaitingPermission
                    && [pane.question.as_ref(), pane.covered.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|question| {
                            question.tool == asked.tool && question.subject == asked.subject
                        })
            })
        });
        if dismissed.is_empty() {
            return false;
        }
        let now = crate::now_ms();
        let mut status = lock(&self.status);
        dismissed.iter().fold(false, |changed, (pane, asked)| {
            status.prompt_gone(pane, asked, now) || changed
        })
    }

    /// Hands new work from a phone to one window, which creates the worktree
    /// and starts its agent. `false` when no window is open to do it.
    pub(super) fn start_work(
        &self,
        project: String,
        prompt: String,
        agent: Option<String>,
        note: Option<String>,
        from: WorkSource,
    ) -> bool {
        self.hand_work(
            project, prompt, agent, note, from, None, None, None, None, None, false,
        )
    }

    /// Hands a phone's new work or fresh session to one desktop window.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_phone_session(
        &self,
        project: String,
        prompt: String,
        agent: Option<String>,
        base: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        worktree: Option<String>,
        shell: bool,
    ) -> bool {
        self.hand_work(
            project,
            prompt,
            agent,
            None,
            WorkSource::Phone,
            None,
            base,
            model,
            effort,
            worktree,
            shell,
        )
    }

    /// [`Host::start_work`], never handed back to `asker`: a terminal's
    /// request comes over a connection that said hello as a window does, and
    /// one run from the host's own executable passes for a window too.
    #[allow(clippy::too_many_arguments)]
    fn hand_work(
        &self,
        project: String,
        prompt: String,
        agent: Option<String>,
        note: Option<String>,
        from: WorkSource,
        asker: Option<&Arc<Out>>,
        base: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        worktree: Option<String>,
        shell: bool,
    ) -> bool {
        let Some(out) = windows(&self.outs)
            .into_iter()
            .find(|out| asker.is_none_or(|asker| !Arc::ptr_eq(out, asker)))
        else {
            return false;
        };
        let Ok(frame) = wire::json(&Reply::StartWork {
            project,
            prompt,
            agent,
            note,
            from,
            base,
            model,
            effort,
            worktree,
            shell,
            automation: None,
        }) else {
            return false;
        };
        out.send(&frame);
        true
    }

    /// The window showing the tab `key` names: only that one can open a tab
    /// beside it.
    fn window_showing(&self, key: &str) -> Option<Arc<Out>> {
        let open = windows(&self.outs);
        lock(&self.terminals)
            .values()
            .filter(|entry| !entry.killed && !entry.terminal.is_closed())
            .filter(|entry| entry.key.as_deref() == Some(key))
            .flat_map(|entry| entry.watchers.iter())
            .filter(|watcher| watcher.watching())
            .find_map(|watcher| {
                open.iter()
                    .find(|out| Arc::ptr_eq(out, &watcher.out))
                    .cloned()
            })
    }

    /// Starts the tab `key` names on a new conversation in place, for a
    /// phone: asks the window showing it, waits for that window to name the
    /// tab once its agent has reported the new session, then finds its
    /// terminal — all within [`WAKE_WAIT`], since a fresh session starts an
    /// agent as a wake does.
    /// `session` and `since_ms` are the cold spell the phone was shown, which
    /// the window checks is still the tab's. The fresh terminal's id, or why
    /// not.
    pub(super) fn start_fresh(&self, key: &str, session: &str, since_ms: u64) -> Woken {
        let Some(out) = self.window_showing(key) else {
            return Err("that tab isn't open in a ket window on your Mac".to_owned());
        };
        let req = self.next_fresh.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.fresh_waiting).insert(req, sender);
        if !out.reply(&Reply::StartFresh {
            req,
            key: key.to_owned(),
            session: session.to_owned(),
            since_ms,
        }) {
            lock(&self.fresh_waiting).remove(&req);
            return Err("the ket window closed before it could start fresh".to_owned());
        }
        let deadline = Instant::now() + WAKE_WAIT;
        let named = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(answer) => break answer,
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    break Err("the ket window closed before it started fresh".to_owned());
                }
                Err(_) if Instant::now() >= deadline => {
                    break Err("ket on your Mac didn't say whether it started fresh".to_owned());
                }
                Err(_) => {}
            }
        };
        lock(&self.fresh_waiting).remove(&req);
        let fresh = named?;
        // Its agent has started, so its terminal is open; looked for again
        // while the host files it, which can trail the window by a moment.
        while Instant::now() < deadline {
            let opened = self.listing().into_iter().find(|(_, key, terminal)| {
                key.as_deref() == Some(fresh.as_str()) && !terminal.is_closed()
            });
            if let Some((id, _, _)) = opened {
                return Ok(id);
            }
            std::thread::sleep(FIX_CHECKS_LOOK);
        }
        Err("the fresh session didn't start on your Mac".to_owned())
    }

    /// A window's answer to [`Reply::StartFresh`].
    pub(super) fn finish_fresh(&self, req: u64, answer: Freshened) {
        if let Some(waiting) = lock(&self.fresh_waiting).remove(&req) {
            let _ = waiting.send(answer);
        }
    }

    /// Hands a phone's merge to one window, which runs ket's own merge.
    /// `false` when no window is open to do it.
    pub(super) fn merge_work(&self, worktree: String) -> bool {
        let Some(out) = windows(&self.outs).into_iter().next() else {
            return false;
        };
        let Ok(frame) = wire::json(&Reply::MergeWork { worktree }) else {
            return false;
        };
        out.send(&frame);
        true
    }

    /// Whether a window is open to hand a phone's work to.
    pub(super) fn has_window(&self) -> bool {
        !windows(&self.outs).is_empty()
    }

    /// Tells every window to read its sidebar again: something other than a
    /// window — a phone — changed the worktrees in ket's store.
    pub(super) fn worktrees_changed(&self) {
        let Ok(frame) = wire::json(&Reply::WorktreesChanged) else {
            return;
        };
        for out in windows(&self.outs) {
            out.send(&frame);
        }
    }

    /// Hands a phone's delete to one window, which deletes the worktree as
    /// its own dialog does and closes its terminals and tabs, and waits up to
    /// [`DELETE_WAIT`] for it to say how that went.
    pub(super) fn delete_work(
        &self,
        worktree: String,
        force: bool,
    ) -> std::result::Result<Option<String>, NotDeleted> {
        let failed = |why: &str| Err(NotDeleted::Failed(why.to_owned()));
        let Some(out) = windows(&self.outs).into_iter().next() else {
            return failed("open ket on your Mac to delete a worktree from here");
        };
        let req = self.next_delete.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.delete_waiting).insert(req, (out.clone(), sender));
        if !out.reply(&Reply::DeleteWork {
            req,
            worktree,
            force,
        }) {
            lock(&self.delete_waiting).remove(&req);
            return failed("the ket window closed before it could delete the worktree");
        }
        let deadline = Instant::now() + DELETE_WAIT;
        let deleted = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(answer) => break answer.map_err(NotDeleted::Refused),
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    break failed("the ket window closed before it said the worktree was deleted");
                }
                Err(_) if Instant::now() >= deadline => {
                    break failed("ket on your Mac didn't say whether the worktree was deleted");
                }
                Err(_) => {}
            }
        };
        lock(&self.delete_waiting).remove(&req);
        deleted
    }

    /// Delivers `from`'s answer to the phone's waiting delete, when `from` is
    /// the window it went to — see [`answered`].
    fn finish_delete_work(&self, req: u64, from: &Arc<Out>, answer: Deleted) {
        if let Some(waiting) = answered(&self.delete_waiting, req, from, "delete") {
            let _ = waiting.send(answer);
        }
    }

    /// The asleep tabs every window has reported, each with the connection
    /// that reported it, less any whose key a running terminal has: a
    /// window's report can lag its own wake by a tick. The first window to
    /// report a key has it.
    pub(super) fn asleep_tabs(&self) -> Vec<(u64, wire::AsleepTab)> {
        let live = self.pane_keys();
        let mut seen = HashSet::new();
        lock(&self.asleep)
            .iter()
            .flat_map(|(conn, tabs)| tabs.iter().map(move |tab| (*conn, tab.clone())))
            .filter(|(_, tab)| !live.contains(&tab.key) && seen.insert(tab.key.clone()))
            .collect()
    }

    /// Wakes the asleep tab `key` for a phone: asks the window that reported
    /// it, then waits up to [`WAKE_WAIT`] for a terminal to open under that
    /// key — the window's own wake counts — or for the window to say why
    /// none will. The terminal's id, or why not.
    pub(super) fn wake(&self, key: &str) -> Woken {
        let owner = lock(&self.asleep)
            .iter()
            .find(|(_, tabs)| tabs.iter().any(|tab| tab.key == key))
            .map(|(conn, _)| *conn);
        let out = owner.and_then(|owner| {
            lock(&self.outs)
                .iter()
                .find(|(conn, _)| *conn == owner)
                .map(|(_, out)| out.clone())
        });
        let Some(out) = out.filter(|out| out.window && out.alive.load(Ordering::Acquire)) else {
            return Err("open ket on your Mac to wake this tab".to_owned());
        };
        let req = self.next_wake.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.wake_waiting).insert(req, (out.clone(), (key.to_owned(), sender)));
        if !out.reply(&Reply::Wake {
            req,
            key: key.to_owned(),
        }) {
            lock(&self.wake_waiting).remove(&req);
            return Err("the ket window closed before it could wake the tab".to_owned());
        }
        let deadline = Instant::now() + WAKE_WAIT;
        let woken = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(answer) => break answer,
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    break Err("the ket window closed before the tab woke".to_owned());
                }
                Err(_) if Instant::now() >= deadline => {
                    break Err("ket on your Mac didn't say whether the tab woke".to_owned());
                }
                Err(_) => {}
            }
        };
        lock(&self.wake_waiting).remove(&req);
        woken
    }

    /// `from`'s answer to [`Reply::Wake`], why the tab stays asleep, when
    /// `from` is the window it went to — see [`answered`].
    fn finish_wake(&self, req: u64, from: &Arc<Out>, answer: Woken) {
        if let Some((_, waiting)) = answered(&self.wake_waiting, req, from, "wake") {
            let _ = waiting.send(answer);
        }
    }

    /// The window to hand work for `worktree`'s agent to: one showing an
    /// agent's terminal there, since only the window with its tab can type
    /// into it — another would say none is running, or start a second — and
    /// the first window when none is.
    fn window_for(&self, worktree: &str) -> Option<Arc<Out>> {
        let open = windows(&self.outs);
        let agent_here = |key: &str| {
            let mut parts = key.splitn(3, '|');
            parts.next() == Some(worktree)
                && parts
                    .next()
                    .is_some_and(|agent| !agent.is_empty() && agent != "shell")
        };
        let showing = lock(&self.terminals)
            .values()
            .filter(|entry| !entry.killed && !entry.terminal.is_closed())
            .filter(|entry| entry.key.as_deref().is_some_and(agent_here))
            .flat_map(|entry| entry.watchers.iter())
            .filter(|watcher| watcher.watching())
            .find_map(|watcher| {
                open.iter()
                    .find(|out| Arc::ptr_eq(out, &watcher.out))
                    .cloned()
            });
        showing.or_else(|| open.into_iter().next())
    }

    /// Hands a phone's failed checks to one window and waits for the agent
    /// to take the message, for starting it to fail, or for the window to
    /// go — up to [`FIX_CHECKS_WAIT`], after which the agent is still
    /// starting.
    pub(super) fn fix_checks(&self, worktree: String, prompt: String) -> Handed {
        let Some(out) = self.window_for(&worktree) else {
            return Handed::Failed("open ket on your Mac to fix checks from here".into());
        };
        let req = self.next_fix_checks.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.fix_checks_waiting).insert(req, (out.clone(), sender));
        if !out.reply(&Reply::FixChecks {
            req,
            worktree,
            prompt,
        }) {
            lock(&self.fix_checks_waiting).remove(&req);
            return Handed::Failed("the ket window closed before it could take the checks".into());
        }
        let deadline = Instant::now() + FIX_CHECKS_WAIT;
        let handed = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(Ok(())) => break Handed::Given,
                Ok(Err(why)) => break Handed::Failed(why),
                // Gone, its agent most likely with it.
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    break Handed::Failed(
                        "the ket window closed before the agent took the checks".into(),
                    );
                }
                Err(_) if Instant::now() >= deadline => break Handed::Starting,
                Err(_) => {}
            }
        };
        lock(&self.fix_checks_waiting).remove(&req);
        handed
    }

    /// Delivers `from`'s answer to the phone's waiting request, when `from`
    /// is the window it went to — see [`answered`].
    fn finish_fix_checks(&self, req: u64, from: &Arc<Out>, error: Option<String>) {
        if let Some(waiting) = answered(&self.fix_checks_waiting, req, from, "fix") {
            let _ = waiting.send(error.map_or(Ok(()), Err));
        }
    }

    /// Hands a phone's review to the window showing the worktree's agent —
    /// see [`Host::window_for`] — which writes it into one message for the
    /// agent, and waits up to [`REVIEW_WAIT`] for the window to say the agent
    /// has it. `Err` says why not, as the phone is told it.
    pub(super) fn send_review(
        &self,
        worktree: String,
        base: Option<String>,
        comments: Vec<wire::PhoneReviewComment>,
    ) -> std::result::Result<(), String> {
        let Some(out) = self.window_for(&worktree) else {
            return Err("open ket on your Mac to send a review from here".into());
        };
        let req = self.next_review.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.review_waiting).insert(req, (out.clone(), sender));
        let taken_by = REVIEW_WAIT.saturating_sub(REVIEW_GRACE);
        let until_ms =
            crate::now_ms().saturating_add(u64::try_from(taken_by.as_millis()).unwrap_or(u64::MAX));
        if !out.reply(&Reply::SendReview {
            req,
            worktree,
            base,
            comments,
            until_ms,
        }) {
            lock(&self.review_waiting).remove(&req);
            return Err("the ket window closed before it could take the review".into());
        }
        let deadline = Instant::now() + REVIEW_WAIT;
        let sent = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(answer) => break answer,
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    break Err("the ket window closed before it took the review".into());
                }
                // Too late for the window to send it now: see `until_ms`.
                Err(_) if Instant::now() >= deadline => {
                    break Err(
                        "ket on your Mac didn't answer in time, so the review wasn't sent".into(),
                    );
                }
                Err(_) => {}
            }
        };
        lock(&self.review_waiting).remove(&req);
        sent
    }

    /// Delivers `from`'s answer to the phone's waiting review, when `from` is
    /// the window it went to — see [`answered`].
    fn finish_send_review(&self, req: u64, from: &Arc<Out>, error: Option<String>) {
        if let Some(waiting) = answered(&self.review_waiting, req, from, "review") {
            let _ = waiting.send(error.map_or(Ok(()), Err));
        }
    }

    /// Hands `ket browser`'s request to the window showing the worktree's
    /// agent — see [`Host::window_for`] — or else to any window but the
    /// connection that asked, and waits up to [`BROWSER_WAIT`] for what the
    /// page did. `Err` says why not, as the terminal prints it.
    fn browser(&self, worktree: String, action: wire::BrowserAction, asker: &Arc<Out>) -> Browsed {
        // A CLI run from the host's own executable passes for a window.
        let other = |out: &Arc<Out>| !Arc::ptr_eq(out, asker);
        let Some(out) = self
            .window_for(&worktree)
            .filter(other)
            .or_else(|| windows(&self.outs).into_iter().find(other))
        else {
            return Err("no ket window is open to run the browser in".into());
        };
        let req = self.next_browser.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.browser_waiting).insert(req, (out.clone(), sender));
        if !out.reply(&Reply::BrowserAction {
            req,
            worktree,
            action,
        }) {
            lock(&self.browser_waiting).remove(&req);
            return Err("the ket window closed before it could run the browser".into());
        }
        let deadline = Instant::now() + BROWSER_WAIT;
        let answer = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(answer) => break answer,
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    break Err("the ket window closed before the browser answered".into());
                }
                Err(_) if Instant::now() >= deadline => {
                    break Err("the ket window didn't answer in time".into());
                }
                Err(_) => {}
            }
        };
        lock(&self.browser_waiting).remove(&req);
        answer
    }

    /// Delivers the answer connection `from` gave to `ket browser`'s waiting
    /// request — when `from` is the window it went to. Request numbers are
    /// easy to guess, so any other connection, another program that said
    /// hello among them, is ignored: its answer would be a forged page
    /// handed to the terminal, and the real window's then dropped.
    fn finish_browser(&self, req: u64, from: &Arc<Out>, answer: Browsed) {
        let mut waiting = lock(&self.browser_waiting);
        let Some((asked, _)) = waiting.get(&req) else {
            return;
        };
        if !Arc::ptr_eq(asked, from) {
            tracing::debug!(
                req,
                "ket host: ignored a browser answer from another connection"
            );
            return;
        }
        if let Some((_, sender)) = waiting.remove(&req) {
            let _ = sender.send(answer);
        }
    }

    /// An ambiguous Escape sent to a terminal by a phone.
    pub(super) fn escape_terminal(&self, id: u64) {
        let Some(key) = self.key(id) else {
            return;
        };
        if lock(&self.status).escape(&key, crate::now_ms()) {
            self.broadcast_status();
        }
    }

    /// Recomputes where the worktrees are from every window's list.
    ///
    /// The union by id. Two windows naming different paths for one id is a
    /// disagreement the host cannot settle, so that id places nothing until
    /// they agree — a report left where it said it was beats one moved to
    /// the wrong row.
    fn merge_worktrees(&self) {
        let merged: Vec<(String, PathBuf)> = {
            let registries = lock(&self.registries);
            let mut paths: BTreeMap<&str, Vec<&PathBuf>> = BTreeMap::new();
            for (id, path) in registries.values().flatten() {
                let seen = paths.entry(id.as_str()).or_default();
                if !seen.contains(&path) {
                    seen.push(path);
                }
            }
            paths
                .into_iter()
                .filter_map(|(id, paths)| match paths.as_slice() {
                    [path] => Some((id.to_owned(), (*path).clone())),
                    _ => {
                        tracing::warn!(
                            worktree = id,
                            "windows disagree on where this worktree is; not placing reports by it"
                        );
                        None
                    }
                })
                .collect()
        };
        lock(&self.status).set_worktrees(merged);
    }

    /// Every terminal the host has, with the key it was filed under.
    pub(super) fn listing(&self) -> Vec<(u64, Option<String>, Arc<LocalTerminal>)> {
        lock(&self.terminals)
            .iter()
            .filter(|(_, entry)| !entry.killed)
            .map(|(&id, entry)| (id, entry.key.clone(), entry.terminal.clone()))
            .collect()
    }
}

/// The prompt `entry`'s pane is showing, with the key it is filed under —
/// only while it is still prompt `question` and nobody has typed at the
/// desktop since it was reported.
fn showing(
    entry: &Entry,
    status: &StatusStore,
    question: u64,
) -> std::result::Result<(String, crate::agent_status::PaneAgentStatus), AnswerRefused> {
    let key = entry.key.clone().ok_or(AnswerRefused::Gone)?;
    let pane = status
        .snapshot()
        .panes
        .into_iter()
        .find(|pane| pane.pane == key && pane.state == crate::event::AgentState::AwaitingPermission)
        .ok_or(AnswerRefused::Answered)?;
    let current = pane.question.as_ref().ok_or(AnswerRefused::Changed)?;
    if question == 0 {
        return Err(AnswerRefused::Unnamed);
    }
    if current.id != question {
        return Err(AnswerRefused::Changed);
    }
    // Typed at the desktop after the prompt was last reported: that
    // keystroke may be the answer, and a second would go to whatever the
    // agent shows next.
    if entry.typed_at_ms >= pane.at_ms {
        return Err(AnswerRefused::Answered);
    }
    Ok((key, pane))
}

/// Whether `bytes` is nothing but reports a terminal sends with nobody at
/// the keyboard: mouse motion and wheel reports, in SGR (`ESC [ < b;x;y M`)
/// or the legacy encoding (`ESC [ M` and three bytes), and focus in and out
/// (`ESC [ I`, `ESC [ O`). A button press or release is not one of them: a
/// click can answer a prompt. Empty input is not either.
fn only_passive_reports(bytes: &[u8]) -> bool {
    // Motion sets bit 32 of the button code, the wheel bit 64.
    let passive = |code: u32| code & (32 | 64) != 0;
    let mut rest = bytes;
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        if let Some(after) = rest
            .strip_prefix(b"\x1b[I")
            .or_else(|| rest.strip_prefix(b"\x1b[O"))
        {
            rest = after;
        } else if let Some(after) = rest.strip_prefix(b"\x1b[<") {
            let Some(end) = after.iter().position(|&b| b == b'M' || b == b'm') else {
                return false;
            };
            let body = &after[..end];
            if !body.iter().all(|&b| b.is_ascii_digit() || b == b';') {
                return false;
            }
            let code = body
                .split(|&b| b == b';')
                .next()
                .and_then(|code| std::str::from_utf8(code).ok())
                .and_then(|code| code.parse::<u32>().ok());
            if !code.is_some_and(passive) {
                return false;
            }
            rest = &after[end + 1..];
        } else if let Some(after) = rest.strip_prefix(b"\x1b[M") {
            let [code, _, _, tail @ ..] = after else {
                return false;
            };
            if !passive(u32::from(code.wrapping_sub(32))) {
                return false;
            }
            rest = tail;
        } else {
            return false;
        }
    }
    true
}

/// A host with terminals and nothing else — no socket, no hook listener,
/// phones off — for `super::phones`' tests.
#[cfg(test)]
impl Host {
    pub(super) fn for_tests() -> Arc<Self> {
        Arc::new(Self {
            terminals: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            next_conn: AtomicU64::new(1),
            clients: AtomicUsize::new(0),
            busy_at: Mutex::new(Instant::now()),
            build: String::new(),
            outs: Arc::new(Mutex::new(Vec::new())),
            backlog: Mutex::new((VecDeque::new(), VecDeque::new())),
            hooks: (0, String::new()),
            prompts: Arc::default(),
            phones: Mutex::new(None),
            status: Mutex::new(StatusStore::new(crate::now_ms())),
            hook_bindings: agent_hooks::HookBindings::default(),
            network: Mutex::new(None),
            network_bindings: crate::network::NetworkBindings::default(),
            registries: Mutex::new(BTreeMap::new()),
            asleep: Mutex::new(BTreeMap::new()),
            wake_waiting: Mutex::new(BTreeMap::new()),
            next_wake: AtomicU64::new(1),
            fresh_waiting: Mutex::new(BTreeMap::new()),
            next_fresh: AtomicU64::new(1),
            usage: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            theme: Mutex::new(None),
            pull_requests: Mutex::new(Vec::new()),
            fix_checks_waiting: Mutex::new(BTreeMap::new()),
            next_fix_checks: AtomicU64::new(1),
            review_waiting: Mutex::new(BTreeMap::new()),
            next_review: AtomicU64::new(1),
            delete_waiting: Mutex::new(BTreeMap::new()),
            next_delete: AtomicU64::new(1),
            browser_waiting: Mutex::new(BTreeMap::new()),
            next_browser: AtomicU64::new(1),
        })
    }

    /// Files `terminal` the way a window's `Open` does, and returns its id.
    pub(super) fn file_for_tests(&self, terminal: LocalTerminal) -> u64 {
        self.file_keyed_for_tests(terminal, None)
    }

    /// [`Self::file_for_tests`] under `key` — `worktree|agent|nonce`, as a
    /// window names a pane — so it is listed in its worktree.
    pub(super) fn file_keyed_for_tests(&self, terminal: LocalTerminal, key: Option<String>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.terminals).insert(
            id,
            Entry {
                terminal: Arc::new(terminal),
                key,
                watchers: Vec::new(),
                closed_at: None,
                killed: false,
                foreground: None,
                lease: None,
                window_size: None,
                fitted: None,
                typed_at_ms: 0,
                typed_answer: None,
            },
        );
        id
    }

    /// Takes an agent's hook report as the hook listener does.
    pub(super) fn report_for_tests(&self, report: &agent_hooks::HookReport) -> bool {
        lock(&self.status).report(report, |_| true)
    }

    /// Takes pull requests as a window's `Request::PullRequests` does.
    pub(super) fn pull_requests_for_tests(
        &self,
        pull_requests: Vec<(String, crate::github::PullRequestStatus)>,
    ) {
        *lock(&self.pull_requests) = pull_requests;
    }

    /// A window, greeted: what the host sends it can be read from the
    /// stream.
    pub(super) fn window_for_tests(&self) -> UnixStream {
        let (a, b) = UnixStream::pair().expect("a socket pair");
        let out = Arc::new(Out {
            stream: Mutex::new(a),
            alive: AtomicBool::new(true),
            greeted: AtomicBool::new(true),
            window: true,
        });
        lock(&self.outs).push((u64::MAX, out));
        b
    }

    /// [`Host::finish_fix_checks`] from the window [`Self::window_for_tests`]
    /// made.
    pub(super) fn finish_fix_checks_for_tests(&self, req: u64, error: Option<String>) {
        self.finish_fix_checks(req, &self.test_window(), error);
    }

    /// [`Host::finish_delete_work`] from the window [`Self::window_for_tests`]
    /// made.
    pub(super) fn finish_delete_work_for_tests(&self, req: u64, answer: Deleted) {
        self.finish_delete_work(req, &self.test_window(), answer);
    }

    fn test_window(&self) -> Arc<Out> {
        lock(&self.outs)
            .iter()
            .find(|(conn, _)| *conn == u64::MAX)
            .map(|(_, out)| out.clone())
            .expect("a window from window_for_tests")
    }
}

/// Runs the host until it is idle long enough, or asked to go.
pub(super) fn run() -> Result<()> {
    let dir = super::dir()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(host_error)?;

    // One host per data directory. A second one started in a race finds the
    // lock held and leaves quietly; its window connects to the first.
    let lock_file = std::fs::File::create(dir.join("host.lock")).map_err(host_error)?;
    if lock_file.try_lock().is_err() {
        tracing::info!("another ket host is running here; leaving");
        return Ok(());
    }

    let socket = super::socket_path()?;
    if let Some(parent) = socket.parent()
        && parent != dir
    {
        // The short path in the temporary directory: owner-only, and ours.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(host_error)?;
        use std::os::unix::fs::MetadataExt;
        let owner = |path: &std::path::Path| std::fs::metadata(path).map(|meta| meta.uid());
        if owner(parent).map_err(host_error)? != owner(&dir).map_err(host_error)? {
            return Err(host_error(format!("{} is not ours", parent.display())));
        }
    }
    // The host's own hook listener. An agent it runs outlives the window
    // that launched it, so it cannot report to that window's listener: it
    // reports here, and every window hears it. Start this before publishing
    // the host socket so a client can never connect to a host that cannot
    // uphold that ownership.
    let token = format!("host-{}", agent_hooks::new_token());
    let hook_bindings = agent_hooks::HookBindings::default();
    let hook_listener = Listener::start_bound(hook_bindings.clone()).map_err(host_error)?;
    agent_hooks::publish_host_endpoint(hook_listener.port(), &token).map_err(host_error)?;

    // Whatever is there belongs to a host that has died: the lock says so.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).map_err(host_error)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(host_error)?;
    tracing::info!(
        socket = %socket.display(),
        version = %crate::build_info::display(),
        build = %super::build_id(),
        "ket host listening"
    );

    let host = Arc::new(Host {
        terminals: Mutex::new(BTreeMap::new()),
        next_id: AtomicU64::new(1),
        next_conn: AtomicU64::new(1),
        clients: AtomicUsize::new(0),
        busy_at: Mutex::new(Instant::now()),
        build: super::build_id(),
        outs: Arc::new(Mutex::new(Vec::new())),
        backlog: Mutex::new((VecDeque::new(), VecDeque::new())),
        hook_bindings,
        network: Mutex::new(None),
        network_bindings: crate::network::NetworkBindings::default(),
        hooks: (hook_listener.port(), token),
        prompts: hook_listener.waiters(),
        phones: Mutex::new(None),
        // A new host is a new epoch: whatever a window held from the last
        // one describes terminals that died with it.
        status: Mutex::new(StatusStore::new(crate::now_ms())),
        registries: Mutex::new(BTreeMap::new()),
        asleep: Mutex::new(BTreeMap::new()),
        wake_waiting: Mutex::new(BTreeMap::new()),
        next_wake: AtomicU64::new(1),
        fresh_waiting: Mutex::new(BTreeMap::new()),
        next_fresh: AtomicU64::new(1),
        usage: Mutex::new(Vec::new()),
        agents: Mutex::new(Vec::new()),
        theme: Mutex::new(None),
        pull_requests: Mutex::new(Vec::new()),
        fix_checks_waiting: Mutex::new(BTreeMap::new()),
        next_fix_checks: AtomicU64::new(1),
        review_waiting: Mutex::new(BTreeMap::new()),
        next_review: AtomicU64::new(1),
        delete_waiting: Mutex::new(BTreeMap::new()),
        next_delete: AtomicU64::new(1),
        browser_waiting: Mutex::new(BTreeMap::new()),
        next_browser: AtomicU64::new(1),
    });

    watch_signals(host.clone());

    // Phones as they were left: the saved switch, or a relay named in the
    // environment for development.
    let phones_on = super::relay_url().is_some()
        || crate::config::Config::load()
            .unwrap_or_default()
            .phones
            .enabled;
    if phones_on && let Err(error) = host.set_phones(true) {
        tracing::warn!(%error, "ket host: phones unavailable");
    }

    {
        let host = host.clone();
        std::thread::Builder::new()
            .name("ket-host-hooks".into())
            .spawn(move || forward_hooks(&host, &hook_listener))
            .map_err(host_error)?;
    }

    {
        let host = host.clone();
        let socket = socket.clone();
        std::thread::Builder::new()
            .name("ket-host-tick".into())
            .spawn(move || housekeeping(&host, &socket))
            .map_err(host_error)?;
    }

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let host = host.clone();
        let _ = std::thread::Builder::new()
            .name("ket-host-conn".into())
            .spawn(move || serve_connection(&host, stream));
    }
    Ok(())
}

/// The windows that are connected and have been welcomed.
fn greeted(outs: &Mutex<Vec<(u64, Arc<Out>)>>) -> Vec<Arc<Out>> {
    lock(outs)
        .iter()
        .filter(|(_, out)| out.alive.load(Ordering::Acquire) && out.greeted.load(Ordering::Acquire))
        .map(|(_, out)| out.clone())
        .collect()
}

/// Of [`greeted`], the ones that are ket windows: what work and hook traffic
/// are handed to. The CLI says hello too, and a request it makes must not
/// come back to it.
fn windows(outs: &Mutex<Vec<(u64, Arc<Out>)>>) -> Vec<Arc<Out>> {
    greeted(outs).into_iter().filter(|out| out.window).collect()
}

/// Takes request `req` out of `waiting`, each entry kept beside the window
/// it went to, for connection `from`'s answer — when `from` is that window.
/// Request numbers count up from 1, so any other connection, another
/// program that said hello among them, is ignored and the request left
/// waiting: its answer would be a forgery handed to the phone, and the real
/// window's then dropped.
fn answered<V>(
    waiting: &Mutex<BTreeMap<u64, (Arc<Out>, V)>>,
    req: u64,
    from: &Arc<Out>,
    what: &str,
) -> Option<V> {
    let mut waiting = lock(waiting);
    if !Arc::ptr_eq(&waiting.get(&req)?.0, from) {
        tracing::debug!(
            req,
            "ket host: ignored a {what} answer from another connection"
        );
        return None;
    }
    waiting.remove(&req).map(|(_, kept)| kept)
}

/// Talks to one window until it goes.
fn serve_connection(host: &Arc<Host>, stream: UnixStream) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    // Whether the other end is a ket window — this host's own executable —
    // rather than anything else the same user runs. Checked once, as it
    // connects; see `window_only`.
    let window = same_program(&stream);
    let out = Arc::new(Out {
        stream: Mutex::new(write_half),
        alive: AtomicBool::new(true),
        greeted: AtomicBool::new(false),
        window,
    });
    let conn = host.next_conn.fetch_add(1, Ordering::Relaxed);
    host.clients.fetch_add(1, Ordering::AcqRel);
    lock(&host.outs).push((conn, out.clone()));
    let mut reader = io::BufReader::new(stream);
    // Nothing is served before a `Hello` on the host's own protocol: a
    // connection that has not said which protocol it speaks is not one of
    // ket's windows, and gets nothing — no pairing code, no terminal, no
    // keystroke delivered.
    let mut spoke = false;

    while let Ok(frame) = wire::read(&mut reader) {
        if !spoke {
            let hello = match &frame {
                Frame::Json(body) => serde_json::from_slice::<Request>(body)
                    .ok()
                    .filter(|request| matches!(request, Request::Hello { .. })),
                _ => None,
            };
            let Some(hello) = hello else {
                tracing::warn!("ket host: a connection asked for something before saying hello");
                break;
            };
            if !handle(host, conn, &out, hello) {
                break;
            }
            spoke = true;
            continue;
        }
        match frame {
            Frame::Json(body) => match serde_json::from_slice::<Request>(&body) {
                Ok(request) if !window && refuse_outsider(&out, &request) => {}
                Ok(request) => {
                    if !handle(host, conn, &out, request) {
                        break;
                    }
                }
                Err(error) => tracing::warn!(%error, "ket host: unreadable request"),
            },
            Frame::Input { id, bytes } => {
                // A keystroke at the desktop takes control back from any
                // phone that had it, then and there. Not the size: a phone
                // fitting it keeps it until it closes the terminal. Nor the
                // pointer passing over a program that tracks it, or the
                // window gaining focus: those reports are sent with nobody
                // typing, and counting them refused a phone's answer as
                // "already answered" whenever the mouse moved on the Mac.
                let typed = !only_passive_reports(&bytes);
                let terminal = lock(&host.terminals).get_mut(&id).map(|entry| {
                    if typed {
                        entry.lease = None;
                        entry.typed_at_ms = crate::now_ms();
                    }
                    entry.terminal.clone()
                });
                if let Some(terminal) = terminal {
                    let _ = terminal.send_input(bytes);
                }
            }
            Frame::Output { .. } | Frame::Checkpoint { .. } => {}
        }
    }

    // The window is gone: everything it was watching is now unwatched, and
    // can be adopted by the next window to ask.
    out.alive.store(false, Ordering::Release);
    for entry in lock(&host.terminals).values_mut() {
        for watcher in entry.watchers.iter().filter(|w| w.conn == conn) {
            watcher.detached.store(true, Ordering::Release);
        }
    }
    lock(&host.outs).retain(|(id, _)| *id != conn);
    if lock(&host.registries).remove(&conn).is_some() {
        host.merge_worktrees();
    }
    lock(&host.asleep).remove(&conn);
    host.clients.fetch_sub(1, Ordering::AcqRel);
}

/// Handles one request. `false` ends the connection.
/// Whether the process on the other end of `stream` is running this very
/// executable: a ket window, which is where a person approves a pairing and
/// turns phones on and off.
///
/// The socket's mode keeps other users out; this is what keeps out other
/// programs the same user runs. The peer's process id comes from the socket
/// (`LOCAL_PEERPID`) and its executable's path from the kernel, not from
/// anything the peer says about itself. A program that can take over a ket
/// window — inject code into an unhardened build — is past this; that is the
/// operating system's to stop.
#[cfg(target_os = "macos")]
fn same_program(stream: &UnixStream) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::LocalPeerPid};
    let Ok(pid) = getsockopt(stream, LocalPeerPid) else {
        return false;
    };
    let Ok(peer) = libproc::proc_pid::pidpath(pid) else {
        return false;
    };
    let canonical = |path: &std::path::Path| path.canonicalize().ok();
    let mine = std::env::current_exe().ok().and_then(|exe| canonical(&exe));
    mine.is_some() && canonical(std::path::Path::new(&peer)) == mine
}

/// Linux exposes the peer PID and UID on Unix sockets. Resolve that PID through
/// `/proc` and compare the executable, failing closed if either check fails.
#[cfg(target_os = "linux")]
fn same_program(stream: &UnixStream) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};

    let Ok(credentials) = getsockopt(stream, PeerCredentials) else {
        return false;
    };
    let Ok(peer) = std::fs::read_link(format!("/proc/{}/exe", credentials.pid())) else {
        return false;
    };
    let canonical = |path: &std::path::Path| path.canonicalize().ok();
    let mine = std::env::current_exe().ok().and_then(|exe| canonical(&exe));
    mine.is_some() && canonical(&peer) == mine
}

/// Platforms without a native peer executable identity fail closed.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn same_program(_stream: &UnixStream) -> bool {
    false
}

/// Answers, and refuses, a request only a ket window may make, from a
/// connection that is not one — approving a pairing above all: without this
/// any program the user runs could let its own phone in. `false` for
/// anything else, which is served as usual. Minting a pairing code is not
/// here: `ket host pair` does it, and a code pairs nothing until a window
/// approves the phone. Nor is revoking one, which only ever takes access
/// away and is worth being able to do from a terminal — `ket host revoke`.
fn refuse_outsider(out: &Arc<Out>, request: &Request) -> bool {
    const WHY: &str = "only a ket window can do that";
    let reply = match request {
        Request::DecidePairing { .. } | Request::DecidePairingRole { .. } => {
            Reply::PairingDecided { found: false }
        }
        Request::Phones { .. } => Reply::Phones {
            serving: false,
            error: Some(WHY.into()),
        },
        _ => return false,
    };
    tracing::warn!("ket host: refused a window's request from another program");
    out.reply(&reply);
    true
}

fn handle(host: &Arc<Host>, conn: u64, out: &Arc<Out>, request: Request) -> bool {
    match request {
        Request::Hello { build, .. } => {
            if build != host.build {
                tracing::info!(window = %build, "a window from another build connected");
            }
            out.reply(&Reply::Welcome {
                protocol: wire::PROTOCOL,
                build: host.build.clone(),
                live: host.live(),
            });
            // Status first, whole: a window that has just arrived sees what
            // every other window sees, not whatever hooks happen to come
            // next. Taken under the status lock and greeted before it is
            // released, so no change can fall between this snapshot and the
            // broadcasts the window will now receive.
            {
                let status = lock(&host.status);
                out.reply(&Reply::AgentStatus {
                    snapshot: Box::new(status.snapshot()),
                });
                out.greeted.store(true, Ordering::Release);
            }
            // What agents reported while no window was listening — for usage
            // and rollouts; the snapshot above already holds their status.
            // Kept for a window: the CLI skips hook traffic, so handing it
            // there would lose it.
            if out.window {
                let (reports, lines) = std::mem::take(&mut *lock(&host.backlog));
                send_hooks(out, &Vec::from(reports), &Vec::from(lines));
            }
            true
        }
        Request::Open { req, mut spec } => {
            // A window that wants hook reports from this agent gave it its
            // own listener's address. The host's replaces it: the window may
            // not be there when the agent next reports.
            let resumed_session = spec.env.remove(agent_hooks::RESUME_SESSION_ENV);
            if spec.env.contains_key(agent_hooks::PORT_ENV) {
                let (port, token) = &host.hooks;
                spec.env
                    .insert(agent_hooks::PORT_ENV.to_owned(), port.to_string());
                if let Some(pane) = spec.key.as_deref() {
                    let bound_token = host.hook_bindings.bind(pane, resumed_session.as_deref());
                    spec.env
                        .insert(agent_hooks::PANE_ENV.to_owned(), pane.to_owned());
                    spec.env
                        .insert(agent_hooks::TOKEN_ENV.to_owned(), bound_token);
                } else {
                    // A terminal without a stable pane key cannot report a
                    // prompt that a phone could safely answer.
                    spec.env
                        .insert(agent_hooks::TOKEN_ENV.to_owned(), token.clone());
                }
            }
            // Taken out whatever happens: it is a word to the host, not the
            // agent's.
            let inspected = spec.env.remove(crate::network::LOG_ENV).is_some();
            let adopted = spec
                .adopt
                .then(|| adoptable(host, spec.key.as_deref()))
                .flatten();
            // A terminal adopted is already running with the environment it
            // started with; only a new one can be pointed at the proxy.
            if inspected && adopted.is_none() {
                host.inspect(&mut spec);
            }
            if spec.adopt_only && adopted.is_none() {
                out.reply(&Reply::Failed {
                    req,
                    error: "No live terminal is available to adopt".to_owned(),
                });
                return true;
            }
            let (id, terminal, adopted) = match adopted {
                Some((id, terminal)) => (id, terminal, true),
                None => match LocalTerminal::open(&spec) {
                    Ok(terminal) => {
                        let id = host.next_id.fetch_add(1, Ordering::Relaxed);
                        let terminal = Arc::new(terminal);
                        lock(&host.terminals).insert(
                            id,
                            Entry {
                                terminal: terminal.clone(),
                                key: spec.key.clone(),
                                watchers: Vec::new(),
                                closed_at: None,
                                killed: false,
                                foreground: None,
                                lease: None,
                                window_size: None,
                                fitted: None,
                                typed_at_ms: 0,
                                typed_answer: None,
                            },
                        );
                        (id, terminal, false)
                    }
                    Err(error) => {
                        out.reply(&Reply::Failed {
                            req,
                            error: error.to_string(),
                        });
                        return true;
                    }
                },
            };
            if let (Some(session), Some(pane)) = (resumed_session, spec.key.as_deref()) {
                lock(&host.status).bind_session(&session, pane);
            }
            // The reply goes before the pump starts, so the window knows the
            // id before the checkpoint for it arrives.
            out.reply(&Reply::Opened {
                req,
                id,
                adopted,
                pid: terminal.pid(),
            });
            // A phone waiting for this key's tab to wake has its terminal,
            // when this is the window asked to wake it: any other
            // connection can name the key.
            if let Some(key) = spec.key.as_deref() {
                for (asked, (woken, waiting)) in lock(&host.wake_waiting).values() {
                    if woken == key && Arc::ptr_eq(asked, out) {
                        let _ = waiting.send(Ok(id));
                    }
                }
            }
            watch(host, conn, out, id, terminal);
            true
        }
        Request::Resize { id, size } => {
            // Kept either way; applied unless a phone has the terminal fitted
            // to its screen, in which case it is what comes back afterwards.
            if let Some(entry) = lock(&host.terminals).get_mut(&id) {
                entry.window_size = Some(size);
                if entry.fitted.is_none() {
                    let _ = entry.terminal.resize(size);
                }
            }
            true
        }
        Request::Clear { id } => {
            if let Some(terminal) = terminal(host, id) {
                terminal.clear();
            }
            true
        }
        Request::Kill { id } => {
            let key = lock(&host.terminals).get_mut(&id).and_then(|entry| {
                entry.killed = true;
                entry.terminal.kill();
                entry.key.clone()
            });
            if let Some(key) = key {
                host.clear_panes(&[key]);
            }
            true
        }
        Request::Stop { req, id, grace_ms } => {
            let (outcome, key) = match lock(&host.terminals).get_mut(&id) {
                Some(entry)
                    if !entry.killed
                        && !entry.terminal.is_closed()
                        && !entry.lease.is_some_and(|(_, until)| until > Instant::now()) =>
                {
                    entry.killed = true;
                    let key = entry.key.clone();
                    entry
                        .terminal
                        .stop(std::time::Duration::from_millis(grace_ms.min(2_000)));
                    (StopOutcome::Stopped, key)
                }
                Some(entry)
                    if entry.lease.is_some_and(|(_, until)| until > Instant::now())
                        && !entry.killed
                        && !entry.terminal.is_closed() =>
                {
                    (StopOutcome::Controlled, None)
                }
                _ => (StopOutcome::Gone, None),
            };
            if let Some(key) = key {
                host.clear_panes(&[key]);
            }
            out.reply(&Reply::Stopped { req, outcome });
            true
        }
        Request::Worktrees { worktrees } => {
            lock(&host.registries).insert(conn, worktrees);
            host.merge_worktrees();
            true
        }
        Request::Asleep { tabs } => {
            // Only a ket window's: any other program could list tabs that
            // aren't there, on the phone and as the one asked to wake them.
            if out.window {
                lock(&host.asleep).insert(conn, tabs);
            } else {
                tracing::debug!(
                    conn,
                    "ket host: asleep tabs from a connection that isn't a window, ignored"
                );
            }
            true
        }
        Request::WakeResult { req, error } => {
            if let Some(why) = error {
                host.finish_wake(req, out, Err(why));
            }
            true
        }
        Request::FreshResult { req, key, error } => {
            let answer = key.ok_or_else(|| {
                error.unwrap_or_else(|| "ket on your Mac didn't start fresh".to_owned())
            });
            host.finish_fresh(req, answer);
            true
        }
        Request::Usage { providers } => {
            *lock(&host.usage) = providers;
            true
        }
        Request::Agents { names } => {
            *lock(&host.agents) = names;
            true
        }
        Request::PullRequests { pull_requests } => {
            *lock(&host.pull_requests) = pull_requests;
            true
        }
        Request::FixChecksResult { req, error } => {
            host.finish_fix_checks(req, out, error);
            true
        }
        Request::SendReviewResult { req, error } => {
            host.finish_send_review(req, out, error);
            true
        }
        Request::DeleteWorkResult {
            req,
            error,
            kept_branch,
        } => {
            host.finish_delete_work(req, out, error.map_or(Ok(kept_branch), Err));
            true
        }
        Request::StartWork {
            project,
            prompt,
            agent,
            note,
        } => {
            let taken = host.hand_work(
                project,
                prompt,
                agent,
                note,
                WorkSource::Cli,
                Some(out),
                None,
                None,
                None,
                None,
                false,
            );
            out.reply(&Reply::WorkTaken { taken });
            true
        }
        Request::Browser {
            req,
            worktree,
            action,
        } => {
            // Waited for here, on this connection's own thread: the CLI asks
            // one thing and waits for it, so there is nothing else to read.
            let result = host.browser(worktree, action, out);
            out.reply(&Reply::BrowserAnswer { req, result });
            true
        }
        Request::BrowserResult { req, result } => {
            host.finish_browser(req, out, result);
            true
        }
        Request::Theme { theme } => {
            *lock(&host.theme) = Some(*theme);
            true
        }
        Request::Interrupt { pane, cancel } => {
            let now = crate::now_ms();
            let changed = {
                let mut status = lock(&host.status);
                if cancel {
                    status.cancel(&pane, CancelSource::CtrlC, now)
                } else {
                    status.escape(&pane, now)
                }
            };
            if changed {
                host.broadcast_status();
            }
            true
        }
        Request::Seen { pane } => {
            if lock(&host.status).seen(&pane) {
                host.broadcast_status();
            }
            true
        }
        Request::MarkUnread { worktree, unread } => {
            host.mark_unread(&worktree, unread);
            true
        }
        Request::Detach { id } => {
            if let Some(entry) = lock(&host.terminals).get(&id) {
                for watcher in entry.watchers.iter().filter(|w| w.conn == conn) {
                    watcher.detached.store(true, Ordering::Release);
                }
            }
            true
        }
        Request::PairingCode => {
            let reply = match host.phones() {
                Some(phones) => Reply::PairingCode {
                    code: Some(phones.pairing_code()),
                    error: None,
                },
                None => Reply::PairingCode {
                    code: None,
                    error: Some("phones are turned off".into()),
                },
            };
            out.reply(&reply);
            true
        }
        Request::Devices => {
            let devices = host.phones().map(|phones| {
                phones
                    .devices()
                    .into_iter()
                    .map(|d| wire::DeviceEntry {
                        id: d.id,
                        name: d.name,
                        paired_at: d.paired_at,
                        connected: d.connected,
                        role: d.role,
                        push: d.push,
                    })
                    .collect()
            });
            let hosted = host.phones().and_then(|phones| phones.hosted_relay());
            out.reply(&Reply::Devices { devices, hosted });
            true
        }
        Request::Pairings => {
            let pairings = host
                .phones()
                .map(|phones| phones.pairings())
                .unwrap_or_default();
            out.reply(&Reply::Pairings {
                pairings,
                scoped_roles: true,
            });
            true
        }
        Request::DecidePairing { id, approve } => {
            // A window from before roles has one Approve, which is full
            // access — the same as every approval now.
            let role = approve.then_some(wire::DeviceRole::Administrator);
            let found = host.phones().is_some_and(|phones| phones.decide(id, role));
            out.reply(&Reply::PairingDecided { found });
            true
        }
        Request::DecidePairingRole { id, role } => {
            let found = host.phones().is_some_and(|phones| phones.decide(id, role));
            out.reply(&Reply::PairingDecided { found });
            true
        }
        Request::Revoke { id } => {
            let reply = match host.phones().map(|phones| phones.revoke(&id)) {
                Some(Ok(found)) => Reply::Revoked { found, error: None },
                Some(Err(error)) => Reply::Revoked {
                    found: false,
                    error: Some(error.to_string()),
                },
                None => Reply::Revoked {
                    found: false,
                    error: Some("the host has no phone side".into()),
                },
            };
            out.reply(&reply);
            true
        }
        Request::TestPush { id } => {
            // On this connection's own thread: the post can take seconds.
            let error = match host.phones() {
                Some(phones) => phones.test_push(&id).err(),
                None => Some("Devices are off on this Mac.".into()),
            };
            out.reply(&Reply::PushTested { error });
            true
        }
        Request::Phones { enabled } => {
            let reply = match host.set_phones(enabled) {
                Ok(serving) => Reply::Phones {
                    serving,
                    error: None,
                },
                Err(error) => Reply::Phones {
                    serving: host.phones().is_some(),
                    error: Some(error.to_string()),
                },
            };
            out.reply(&reply);
            true
        }
        Request::Shutdown { force: true } => end(host, "asked to end"),
        Request::Shutdown { force: false } => {
            if host.live() == 0 {
                tracing::info!("asked to make way for another build; exiting");
                if let Ok(socket) = super::socket_path() {
                    let _ = std::fs::remove_file(socket);
                }
                std::process::exit(0);
            }
            true
        }
    }
}

/// Ends every terminal the host runs, and the host with them.
fn end(host: &Host, why: &str) -> ! {
    let ending: Vec<_> = lock(&host.terminals)
        .values()
        .map(|entry| entry.terminal.clone())
        .collect();
    tracing::info!(
        terminals = ending.len(),
        "{why}; ending every terminal and exiting"
    );
    for terminal in ending {
        terminal.kill();
    }
    if let Ok(socket) = super::socket_path() {
        let _ = std::fs::remove_file(socket);
    }
    std::process::exit(0);
}

/// Ends the host, terminals and all, on `SIGTERM`, `SIGINT` or `SIGQUIT`.
///
/// A handler rather than the default action, for two reasons. The default
/// left the terminals to notice their pty had gone; this ends them. And a
/// handler replaces a disposition the host *inherited*: started from a shell
/// that ignores `SIGTERM` — an agent's tool runner is one — the host could
/// otherwise only be ended with `SIGKILL`, and two such hosts outlived their
/// sandboxes by a day.
///
/// Installed before this returns, so a signal sent the moment the socket
/// appears is not missed.
fn watch_signals(host: Arc<Host>) {
    use tokio::signal::unix::{SignalKind, signal};
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!(%error, "ket host: signals not watched");
            return;
        }
    };
    let watched = {
        let _entered = runtime.enter();
        [
            SignalKind::terminate(),
            SignalKind::interrupt(),
            SignalKind::quit(),
        ]
        .into_iter()
        .map(signal)
        .collect::<std::io::Result<Vec<_>>>()
    };
    let mut watched = match watched {
        Ok(watched) => watched,
        Err(error) => {
            tracing::warn!(%error, "ket host: signals not watched");
            return;
        }
    };
    std::thread::spawn(move || {
        runtime.block_on(async {
            let [terminate, interrupt, quit] = watched.as_mut_slice() else {
                return;
            };
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
                _ = quit.recv() => {}
            }
        });
        end(&host, "signalled");
    });
}

fn terminal(host: &Host, id: u64) -> Option<Arc<LocalTerminal>> {
    lock(&host.terminals)
        .get(&id)
        .map(|entry| entry.terminal.clone())
}

/// A running terminal filed under `key` that no window is watching.
///
/// The oldest first, so tabs restored in order adopt terminals in the order
/// they were opened.
fn adoptable(host: &Host, key: Option<&str>) -> Option<(u64, Arc<LocalTerminal>)> {
    let key = key?;
    lock(&host.terminals)
        .iter()
        .find(|(_, entry)| {
            entry.key.as_deref() == Some(key)
                && !entry.terminal.is_closed()
                && !entry.killed
                && !entry.watchers.iter().any(Watcher::watching)
        })
        .map(|(&id, entry)| (id, entry.terminal.clone()))
}

/// Starts sending one terminal to one window.
fn watch(host: &Arc<Host>, conn: u64, out: &Arc<Out>, id: u64, terminal: Arc<LocalTerminal>) {
    let detached = Arc::new(AtomicBool::new(false));
    if let Some(entry) = lock(&host.terminals).get_mut(&id) {
        entry.watchers.push(Watcher {
            out: out.clone(),
            detached: detached.clone(),
            conn,
        });
    }
    let out = out.clone();
    let _ = std::thread::Builder::new()
        .name("ket-host-pump".into())
        .spawn(move || pump(id, &terminal, &out, &detached));
}

/// Sends a terminal's screen and then its output to one window, until the
/// window lets go or the program finishes.
fn pump(id: u64, terminal: &LocalTerminal, out: &Out, detached: &AtomicBool) {
    let mut frames = terminal.frames();
    frames.borrow_and_update();
    let Some(mut cursor) = send_checkpoint(id, terminal, out) else {
        return;
    };

    loop {
        if detached.load(Ordering::Acquire) {
            return;
        }
        match terminal.output_since(cursor) {
            Since::Output {
                bytes,
                cursor: next,
            } => {
                if !bytes.is_empty() && !out.send(&wire::output(id, &bytes)) {
                    return;
                }
                cursor = next;
            }
            Since::Reset => match send_checkpoint(id, terminal, out) {
                Some(next) => cursor = next,
                None => return,
            },
        }

        if terminal.is_closed() {
            // The last frame after EOF has been sent above. The exit status
            // turns up once the reader has reaped the child, which is a
            // moment later.
            let deadline = Instant::now() + Duration::from_secs(1);
            while terminal.exit_status().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            out.reply(&Reply::Closed {
                id,
                status: terminal.exit_status().as_ref().map(Exit::from),
            });
            return;
        }

        if futures::executor::block_on(frames.changed()).is_err() {
            return;
        }
    }
}

fn send_checkpoint(id: u64, terminal: &LocalTerminal, out: &Out) -> Option<OutputCursor> {
    let checkpoint = terminal.checkpoint();
    out.send(&wire::checkpoint(
        id,
        checkpoint.size,
        checkpoint.scrollback,
        &checkpoint.ansi,
    ))
    .then_some(checkpoint.cursor)
}

#[cfg(test)]
mod tests;
