//! Phones, through the relay (Epic 5b.3).
//!
//! With `KET_RELAY_URL` set, the host dials the relay, registers the id from
//! its pairing codes, and serves every phone the relay connects to it: a Noise
//! handshake (`ket-remote`) — a pairing, or a reconnect from a phone it has
//! granted — then the phone's view of the host: the terminals, one
//! terminal's screen and output, keystrokes and signals back.
//!
//! The host only ever dials out. The relay forwards ciphertext by connection
//! id, and everything above the handshake happens here and on the phone.
//!
//! One thread holds the relay socket, in a small tokio runtime of its own;
//! each phone gets a thread that owns its Noise session, fed by one channel
//! that carries both the phone's frames and what the host has to send it — so
//! the session is only ever touched by one thread and needs no lock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use futures::{SinkExt, StreamExt};
use ket_relay_protocol::{Control, Frame, Kind};
use ket_remote::proto::{self, envelope::Payload};
use prost::Message;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::host_error;
use super::server::{AnswerRefused, Host};
use super::wire::DeviceRole;
use crate::Result;
use crate::activity::{Signal, Tracker};
use crate::agent_hooks::{Decision, Prompt};
use crate::agent_status::CancelSource;
use crate::event::AgentState;
use crate::id::WorktreeId;
use crate::rate_limits::{ProviderSnapshot, SnapshotStatus};
use crate::subagents::SubagentState;
use crate::terminal::{LocalTerminal, Since, TerminalSize};

/// How long a phone gets to finish its handshake and say hello, from when
/// the relay opened its connection. Past it, the connection is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Handshakes in flight at once. Anyone on the Wi-Fi can open a connection
/// through the relay, and each one is a thread until it has proved itself;
/// past this, new ones are closed straight away. A real phone that meets the
/// limit reconnects a moment later.
const MAX_PENDING_HANDSHAKES: usize = 8;

/// Bytes a connection may send before its handshake has let it in. A whole
/// handshake and a hello are well under a kilobyte; past this it is closed
/// rather than queued.
const PREADMIT_BYTES: usize = 64 * 1024;

/// How long a pairing code works. Short: a code that has been photographed is
/// a way in until it expires or is used — though since 1.6 a pairing also
/// has to be approved on the desktop, see [`APPROVAL_TIMEOUT`].
pub const PAIRING_TTL: Duration = Duration::from_secs(3 * 60);

/// How long a phone that has paired waits for a person to approve it on the
/// desktop before it is turned away.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Envelopes kept per phone for it to resume from, and their total size. A
/// phone that reconnects within them picks up where it was; one that fell
/// further behind gets a fresh snapshot.
const RESUME_ENVELOPES: usize = 8192;
/// See [`RESUME_ENVELOPES`].
const RESUME_BYTES: usize = 8 * 1024 * 1024;

/// How often the host looks for a change worth a new snapshot: a terminal
/// opened or ended, an agent's state moved.
const SNAPSHOT_TICK: Duration = Duration::from_secs(1);

/// How long ket's store is trusted for project and worktree names before it
/// is read again. Names change when somebody renames a row, which is rare;
/// reading the file every tick is not worth it.
const PLACES_TTL: Duration = Duration::from_secs(5);

/// How soon the store is read again when a terminal names a worktree it did
/// not have: a worktree just made, whose row would otherwise sit under "This
/// desktop" for the rest of [`PLACES_TTL`] before moving to its project.
const PLACES_RECHECK: Duration = Duration::from_secs(1);

/// How long a worktree's figures — changes and commits ahead — are trusted
/// before git is asked again. Each read is a status walk per worktree, so it
/// is done less often than names are, and only while a phone is connected.
const FIGURES_TTL: Duration = Duration::from_secs(15);

/// How long each project's count of open backlog notes is trusted before the
/// files are read again. A phone's own edits clear it at once; the desktop's
/// show within this.
const BACKLOG_TTL: Duration = Duration::from_secs(5);

/// Idempotency keys remembered per phone. A retry lands within moments of
/// the original, so a short memory is enough to make it a no-op.
const SEEN_KEYS: usize = 1024;

/// Cost units available to one paired device for expensive requests,
/// refilling at [`REQUEST_REFILL`] a second; kept per device rather than per
/// connection, so reconnecting cannot reset it.
///
/// Sized to the phone's own use with room to spare. At a burst of 20 and one
/// a second, the conversation's poll alone — every 0.7–1.5 s while an agent
/// works — spent it within half a minute, and the refusals that followed
/// landed on terminal subscribes too, which left terminals blank.
const REQUEST_BURST: u32 = 60;

/// Units returned to a device's budget each second.
const REQUEST_REFILL: u32 = 5;
const MAX_SUBSCRIPTIONS: usize = 8;

struct RequestBudget {
    units: u32,
    updated: Instant,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            units: REQUEST_BURST,
            updated: Instant::now(),
        }
    }
}

impl RequestBudget {
    fn take(&mut self, cost: u32) -> bool {
        let now = Instant::now();
        let seconds = u32::try_from(now.duration_since(self.updated).as_secs()).unwrap_or(u32::MAX);
        let replenished = seconds.saturating_mul(REQUEST_REFILL);
        self.units = self.units.saturating_add(replenished).min(REQUEST_BURST);
        if seconds > 0 {
            self.updated = now;
        }
        if self.units < cost {
            return false;
        }
        self.units -= cost;
        true
    }
}

/// What the host keeps for one paired phone across its reconnects.
#[derive(Default)]
struct Device {
    /// The last envelope sequence number sent to it.
    seq: u64,
    /// Recent envelopes, encoded, for it to resume from.
    sent: VecDeque<(u64, Vec<u8>)>,
    sent_bytes: usize,
    /// Recent idempotency keys, oldest first, and the same as a set.
    seen: VecDeque<String>,
    seen_set: HashSet<String>,
    requests: RequestBudget,
    refused_requests: u64,
}

impl Device {
    /// Whether a phone that has everything up to `seq` can carry on from
    /// what is kept, rather than start over.
    fn can_resume(&self, seq: u64) -> bool {
        seq <= self.seq
            && (seq == self.seq
                || self
                    .sent
                    .front()
                    .is_some_and(|(first, _)| *first <= seq + 1))
    }

    fn remember(&mut self, seq: u64, bytes: &[u8]) {
        self.sent.push_back((seq, bytes.to_vec()));
        self.sent_bytes += bytes.len();
        while self.sent.len() > RESUME_ENVELOPES || self.sent_bytes > RESUME_BYTES {
            let Some((_, dropped)) = self.sent.pop_front() else {
                break;
            };
            self.sent_bytes -= dropped.len();
        }
    }

    /// Whether a request under `key` has already been applied.
    fn applied(&self, key: &str) -> bool {
        self.seen_set.contains(key)
    }

    /// Remembers that a request under `key` was applied — only once it was,
    /// so a retry of one that was refused is tried again rather than told it
    /// succeeded.
    fn record(&mut self, key: &str) {
        if !self.seen_set.insert(key.to_owned()) {
            return;
        }
        self.seen.push_back(key.to_owned());
        if self.seen.len() > SEEN_KEYS
            && let Some(old) = self.seen.pop_front()
        {
            self.seen_set.remove(&old);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// This host run's epoch, for phones: when it started, in milliseconds. A
/// phone that sees it change knows every cursor it held is void.
fn epoch() -> u64 {
    static EPOCH: OnceLock<u64> = OnceLock::new();
    *EPOCH.get_or_init(crate::now_ms)
}

/// Where the host's relay is.
#[derive(Debug, Clone)]
pub(super) enum Relay {
    /// Run by the host itself, on every interface, on this port — phones on
    /// the local network (the default, and the only way for users).
    Local {
        /// See [`crate::config::PhonesConfig::port`].
        port: u16,
    },
    /// Somewhere else, dialled at this URL: `KET_RELAY_URL`, for development.
    External(String),
}

impl Relay {
    /// What the host's own link dials.
    fn dial(&self) -> String {
        match self {
            Self::Local { port } => format!("ws://127.0.0.1:{port}"),
            Self::External(url) => url.clone(),
        }
    }

    /// What a pairing code tells a phone to dial. Worked out per code, so a
    /// code made after the Mac changes network carries where it is now.
    fn advertised(&self) -> String {
        match self {
            Self::Local { port } => format!("ws://{}:{port}", lan_host()),
            Self::External(url) => url.clone(),
        }
    }
}

/// The name a phone on the same network reaches this Mac by: its Bonjour
/// name, which survives a new network or DHCP lease; else its address on
/// the network; else the loopback name, which reaches nothing but says so.
pub(super) fn lan_host() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return format!("{name}.local");
        }
    }
    lan_ip().map_or_else(|| "localhost".to_owned(), |ip| ip.to_string())
}

/// This machine's address on the network its default route uses. A UDP
/// socket "connected" to a documentation address sends nothing; it only
/// makes the system pick the interface it would use.
fn lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// The host's phone side: its identity, the relay it dials, and how many
/// phones are connected.
pub(super) struct Phones {
    identity: Mutex<ket_remote::Host>,
    /// Authority by device key. Every device that connects is given full
    /// access and written here as such; one paired before roles existed is
    /// absent until it next connects.
    roles: Mutex<super::identity::Roles>,
    relay: Relay,
    /// Set once phones are turned off: the snapshot thread and the relay
    /// thread both end on it.
    stopped: AtomicBool,
    stop: tokio::sync::watch::Sender<bool>,
    connected: AtomicUsize,
    /// Per phone, by public key: what it can resume from, and what it has
    /// already asked for. For this run of the host — a new epoch starts over.
    devices: Mutex<HashMap<Vec<u8>, Device>>,
    /// Names each phone session, for leases.
    next_session: AtomicU64,
    /// Phones connected now, by public key, and the channel that ends each
    /// session: how revoking a phone cuts it off then and there.
    live: Mutex<HashMap<Vec<u8>, Vec<Session>>>,
    /// A projection of the host's canonical agent-status snapshot into the
    /// activity vocabulary the phone protocol uses.
    activity: Mutex<Tracker>,
    /// Projects and worktrees from ket's store, and when they were read.
    places: Mutex<Option<(Instant, Arc<Places>)>>,
    /// Each worktree's figures by id, and when they were read.
    figures: Mutex<Option<(Instant, FigureMap)>>,
    /// Each project's open backlog notes by id, and when they were counted.
    backlogs: Mutex<Option<(Instant, BacklogCounts)>>,
    /// The last snapshot sent to every phone, encoded, to tell a change.
    last_snapshot: Mutex<Vec<u8>>,
    /// Handshakes in flight — see [`MAX_PENDING_HANDSHAKES`].
    pending: AtomicUsize,
    /// Phones that have paired and wait for a person to approve them.
    approvals: Mutex<Vec<Approval>>,
    /// Names each approval, for deciding it.
    next_approval: AtomicU64,
    /// The key this host claims its own relay with, made fresh each time
    /// phones are turned on; `None` for a relay elsewhere, which is not told
    /// it. See [`ket_relay::Own`].
    claim: Option<[u8; 32]>,
}

/// What ket's store says about the projects and worktrees the host's
/// terminals belong to: their names, and the order a sidebar shows them in.
#[derive(Default)]
struct Places {
    /// Projects, in the sidebar's order.
    projects: Vec<ProjectPlace>,
    worktrees: Vec<Place>,
}

struct ProjectPlace {
    id: String,
    /// The name a sidebar shows: the one somebody gave it, else the folder's.
    name: String,
    /// The colour chosen for it, `#rrggbb`, or empty for ket's default.
    color: String,
}

struct Place {
    id: String,
    project: String,
    name: String,
    branch: String,
    /// Where it is checked out, for its figures.
    path: std::path::PathBuf,
    /// The branch it was cut from; `None` for a repository's own checkout.
    base: Option<String>,
}

/// Every worktree's figures, by id.
type FigureMap = Arc<HashMap<String, Figures>>;

/// Open backlog notes by project id — see [`BACKLOG_TTL`].
type BacklogCounts = Arc<HashMap<String, u32>>;

/// A worktree's figures, as the desktop's sidebar and merge dialog count them.
#[derive(Clone, Copy, Default)]
struct Figures {
    files_changed: u32,
    lines_added: u32,
    lines_removed: u32,
    commits_ahead: u32,
}

impl Figures {
    /// Reads them from git. Anything that cannot be read is zero: a figure is
    /// a nicety, and a missing checkout or a deleted base is ordinary.
    fn read(place: &Place) -> Self {
        let Ok(status) = crate::status::of_worktree(&place.path, place.base.as_deref()) else {
            return Self::default();
        };
        let mut paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        // Only a dirty tree is worth a diff; a clean one has no lines to count.
        let (lines_added, lines_removed) = if status.total_changes > 0 {
            crate::git::Git::new(&place.path)
                .line_changes()
                .unwrap_or_default()
        } else {
            (0, 0)
        };
        Self {
            files_changed: u32::try_from(paths.len()).unwrap_or(u32::MAX),
            lines_added,
            lines_removed,
            commits_ahead: status.tracking.map_or(0, |tracking| tracking.ahead),
        }
    }
}

impl Places {
    /// Reads ket's store, or nothing if it cannot be read: names are a
    /// nicety, and a phone still gets every terminal without them.
    fn read() -> Self {
        let Ok(state) = crate::store::Store::open_default().and_then(|store| store.load()) else {
            return Self::default();
        };
        // The sidebar's own order, so a phone lists everything where the
        // desktop does.
        let projects = crate::workspace::sidebar_projects(&state);
        let mut worktrees: Vec<Place> = Vec::new();
        for project in &projects {
            // The repository's own checkout, under the id a window gives it.
            worktrees.push(Place {
                id: format!("primary-{}", project.id),
                project: project.id.to_string(),
                name: project.default_base.clone(),
                branch: project.default_base.clone(),
                path: project.root.clone(),
                base: None,
            });
        }
        for worktree in crate::workspace::sidebar_worktrees(&state) {
            worktrees.push(Place {
                id: worktree.id.to_string(),
                project: worktree.project_id.to_string(),
                name: worktree
                    .name
                    .clone()
                    .or_else(|| worktree.agent_title.clone())
                    .unwrap_or_else(|| worktree.branch.clone()),
                branch: worktree.branch,
                path: worktree.path,
                base: Some(worktree.base),
            });
        }
        let settings = state.project_settings;
        Self {
            projects: projects
                .into_iter()
                .map(|p| {
                    let chosen = settings.get(&p.id);
                    ProjectPlace {
                        id: p.id.to_string(),
                        name: chosen
                            .and_then(|s| s.display_name.clone())
                            .unwrap_or(p.name),
                        color: chosen.and_then(|s| s.color.clone()).unwrap_or_default(),
                    }
                })
                .collect(),
            worktrees,
        }
    }

    fn get(&self, id: &str) -> Option<&Place> {
        self.worktrees.iter().find(|place| place.id == id)
    }
}

/// A phone waiting for a person to approve its pairing.
struct Approval {
    id: u64,
    name: String,
    code: String,
    asked: Instant,
    /// `Some` once somebody has approved a role or declined.
    decision: Option<Option<DeviceRole>>,
}

/// One live phone session: its lease holder number, and the channel that
/// ends it.
type Session = (u64, mpsc::Sender<Event>);

/// A paired phone, as the device list shows it.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// The phone's public key, base64url: stable, and what revoking names.
    pub id: String,
    /// What it called itself when it paired.
    pub name: String,
    /// Unix seconds.
    pub paired_at: u64,
    /// Whether it is connected now.
    pub connected: bool,
    /// Its stored authority. `None` for a device paired before roles
    /// existed, which is given full access when it next connects.
    pub role: Option<DeviceRole>,
}

impl Phones {
    /// The figures of every worktree in `places` — see [`FIGURES_TTL`].
    fn figures(&self, places: &Places) -> FigureMap {
        let mut figures = lock(&self.figures);
        match &*figures {
            Some((at, read)) if at.elapsed() < FIGURES_TTL => read.clone(),
            _ => {
                let read: FigureMap = Arc::new(
                    places
                        .worktrees
                        .iter()
                        .map(|place| (place.id.clone(), Figures::read(place)))
                        .collect(),
                );
                *figures = Some((Instant::now(), read.clone()));
                read
            }
        }
    }

    /// Each project's open backlog notes — see [`BACKLOG_TTL`].
    fn backlog_counts(&self, places: &Places) -> BacklogCounts {
        let mut backlogs = lock(&self.backlogs);
        match &*backlogs {
            Some((at, read)) if at.elapsed() < BACKLOG_TTL => read.clone(),
            _ => {
                let read: BacklogCounts = Arc::new(
                    places
                        .projects
                        .iter()
                        .map(|project| {
                            let id = crate::id::ProjectId::from(project.id.as_str());
                            let open = crate::backlog::Backlog::load(&id)
                                .map(|backlog| backlog.open_count())
                                .unwrap_or(0);
                            (project.id.clone(), u32::try_from(open).unwrap_or(u32::MAX))
                        })
                        .collect(),
                );
                *backlogs = Some((Instant::now(), read.clone()));
                read
            }
        }
    }

    /// Recounts the backlogs on the next snapshot, after a phone changed one.
    fn forget_backlogs(&self) {
        *lock(&self.backlogs) = None;
    }

    fn places(&self) -> Arc<Places> {
        self.places_knowing(&[])
    }

    /// [`Phones::places`], read again sooner when one of `worktrees` is not
    /// in it — see [`PLACES_RECHECK`].
    fn places_knowing(&self, worktrees: &[&str]) -> Arc<Places> {
        let mut places = lock(&self.places);
        match &*places {
            Some((at, read))
                if at.elapsed() < PLACES_TTL
                    && (at.elapsed() < PLACES_RECHECK
                        || worktrees.iter().all(|id| read.get(id).is_some())) =>
            {
                read.clone()
            }
            _ => {
                let read = Arc::new(Places::read());
                *places = Some((Instant::now(), read.clone()));
                read
            }
        }
    }

    /// Sends every connected phone a new snapshot, if anything in it changed
    /// since the last one.
    fn tick(&self, host: &Host) {
        let feeds: Vec<mpsc::Sender<Event>> = lock(&self.live)
            .values()
            .flatten()
            .map(|(_, feed)| feed.clone())
            .collect();
        if feeds.is_empty() {
            return;
        }
        let snapshot = snapshot(host, self);
        let encoded = snapshot.encode_to_vec();
        {
            let mut last = lock(&self.last_snapshot);
            if *last == encoded {
                return;
            }
            *last = encoded;
        }
        for feed in feeds {
            let _ = feed.send(Event::Snapshot(Box::new(snapshot.clone())));
        }
    }

    /// A one-time code to pair a phone with, good for [`PAIRING_TTL`].
    pub(super) fn pairing_code(&self) -> String {
        let relay = self.relay.advertised();
        lock(&self.identity).offer(&relay, PAIRING_TTL).to_code()
    }

    /// Turns the phone side off: the relay stops listening, the link to it
    /// drops, and every phone's session ends. Terminals are not touched.
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.stop.send(true);
        for (_, sessions) in lock(&self.live).drain() {
            for (_, feed) in sessions {
                let _ = feed.send(Event::Closed);
            }
        }
        tracing::info!("phones turned off");
    }

    /// Phones connected now.
    pub(super) fn connected(&self) -> usize {
        self.connected.load(Ordering::Acquire)
    }

    /// Every phone this host has paired with.
    pub(super) fn devices(&self) -> Vec<DeviceInfo> {
        // The identity before `live`, the order `revoke` and a phone being
        // let in take them in.
        let identity = lock(&self.identity);
        let roles = lock(&self.roles);
        let live = lock(&self.live);
        identity
            .grants()
            .iter()
            .map(|grant| DeviceInfo {
                id: B64.encode(&grant.device),
                name: grant.name.clone(),
                paired_at: grant.paired_at,
                connected: live.get(&grant.device).is_some_and(|s| !s.is_empty()),
                role: roles.get(&grant.device).copied(),
            })
            .collect()
    }

    /// Phones waiting for a person to approve their pairing, oldest first.
    pub(super) fn pairings(&self) -> Vec<super::wire::PendingPairing> {
        lock(&self.approvals)
            .iter()
            .filter(|approval| approval.decision.is_none())
            .map(|approval| super::wire::PendingPairing {
                id: approval.id,
                name: approval.name.clone(),
                code: approval.code.clone(),
                expires_in: APPROVAL_TIMEOUT
                    .saturating_sub(approval.asked.elapsed())
                    .as_secs(),
            })
            .collect()
    }

    /// Approves or declines a waiting pairing. `false` if none by that id is
    /// waiting.
    pub(super) fn decide(&self, id: u64, role: Option<DeviceRole>) -> bool {
        lock(&self.approvals)
            .iter_mut()
            .find(|approval| approval.id == id && approval.decision.is_none())
            .map(|approval| approval.decision = Some(role))
            .is_some()
    }

    /// Stops accepting a phone and ends its sessions. `false` if no phone by
    /// that id is paired.
    pub(super) fn revoke(&self, id: &str) -> Result<bool> {
        let Ok(device) = B64.decode(id.trim()) else {
            return Ok(false);
        };
        // The grant and the live sessions go under one hold of the identity:
        // a phone lets itself in under the same lock (see `session`), so it
        // is either in `live` by now and ended here, or it finds its grant
        // gone and is turned away.
        let identity = {
            let mut identity = lock(&self.identity);
            if !identity.revoke(&device) {
                return Ok(false);
            }
            lock(&self.roles).remove(&device);
            for (_, feed) in lock(&self.live).remove(&device).unwrap_or_default() {
                let _ = feed.send(Event::Closed);
            }
            identity
        };
        super::identity::save_with_roles(&identity, &lock(&self.roles))?;
        drop(identity);
        tracing::info!("revoked a phone");
        Ok(true)
    }
}

/// Loads the identity, starts the relay when it is the host's own, and
/// dials it, reconnecting whenever the link drops — until [`Phones::stop`].
pub(super) fn start(host: Arc<Host>, relay: Relay) -> Result<Arc<Phones>> {
    let identity = super::identity::load_or_create()?;
    let roles = super::identity::load_roles()?;
    // Bound here rather than on the relay's thread, so a port already in use
    // is this call's error, said where phones are being turned on.
    let listener = match relay {
        Relay::Local { port } => {
            let listener = std::net::TcpListener::bind(("0.0.0.0", port))
                .map_err(|e| host_error(format!("port {port} is in use: {e}")))?;
            listener.set_nonblocking(true).map_err(host_error)?;
            Some((listener, identity.id))
        }
        Relay::External(_) => None,
    };
    let claim = matches!(relay, Relay::Local { .. }).then(ket_remote::secret);
    let (stop, mut stopping) = tokio::sync::watch::channel(false);
    let phones = Arc::new(Phones {
        identity: Mutex::new(identity),
        roles: Mutex::new(roles),
        relay,
        stopped: AtomicBool::new(false),
        stop,
        connected: AtomicUsize::new(0),
        devices: Mutex::new(HashMap::new()),
        next_session: AtomicU64::new(1),
        live: Mutex::new(HashMap::new()),
        activity: Mutex::new(Tracker::new(Vec::<String>::new())),
        places: Mutex::new(None),
        figures: Mutex::new(None),
        backlogs: Mutex::new(None),
        last_snapshot: Mutex::new(Vec::new()),
        pending: AtomicUsize::new(0),
        approvals: Mutex::new(Vec::new()),
        next_approval: AtomicU64::new(1),
        claim,
    });
    let (ticking, watched) = (phones.clone(), host.clone());
    std::thread::Builder::new()
        .name("ket-host-snapshots".into())
        .spawn(move || {
            while !ticking.stopped.load(Ordering::Acquire) {
                std::thread::sleep(SNAPSHOT_TICK);
                ticking.tick(&watched);
            }
        })
        .map_err(host_error)?;
    let link = phones.clone();
    std::thread::Builder::new()
        .name("ket-host-relay".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::warn!(%error, "ket host: no runtime for the relay link");
                    return;
                }
            };
            runtime.block_on(async {
                // The relay, when it is ours: routing for this host alone.
                let relay = async {
                    match listener.map(|(l, id)| (tokio::net::TcpListener::from_std(l), id)) {
                        Some((Ok(listener), id)) => {
                            let own = claim.map(|key| ket_relay::Own { host: id, key });
                            ket_relay::serve(listener, own).await;
                        }
                        Some((Err(error), _)) => {
                            tracing::warn!(%error, "ket host: the relay could not listen");
                            std::future::pending::<()>().await;
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    () = relay => {}
                    () = link_forever(&host, &link) => {}
                    _ = stopping.wait_for(|stopped| *stopped) => {}
                }
            });
            // Dropping the runtime here drops every socket it held.
        })
        .map_err(host_error)?;
    Ok(phones)
}

async fn link_forever(host: &Arc<Host>, phones: &Arc<Phones>) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match link(host, phones).await {
            Ok(()) => tracing::info!("relay link closed"),
            Err(error) => {
                tracing::warn!(%error, relay = %phones.relay.dial(), "relay link failed");
            }
        }
        // A link that held for a while earns a fast retry.
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// What a phone's thread hears: a frame from the phone, something to send
/// the phone, or that the connection is over.
enum Event {
    Frame(Vec<u8>),
    Send(Box<proto::Envelope>),
    /// The host's picture changed; sent on only if it differs from the last
    /// one this phone was given.
    Snapshot(Box<proto::HostSnapshot>),
    Closed,
}

type Link = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn link(host: &Arc<Host>, phones: &Arc<Phones>) -> Link {
    // Nagle off, for the reason the relay turns it off on its side: every
    // frame is a keystroke or an echo someone is waiting on.
    let (socket, _) =
        tokio_tungstenite::connect_async_with_config(phones.relay.dial(), None, true).await?;
    let (mut sink, mut incoming) = socket.split();
    let host_id = lock(&phones.identity).id;
    // Our own relay is claimed with the key it was started with; a relay
    // elsewhere is only told the id.
    let register = match phones.claim {
        Some(key) => Control::Claim { host: host_id, key },
        None => Control::Register { host: host_id },
    };
    sink.send(WsMessage::Binary(register.frame(0).encode()?.into()))
        .await?;
    tracing::info!(relay = %phones.relay.dial(), "registered with the relay");

    let (out, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<Frame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            let Ok(bytes) = frame.encode() else { continue };
            if sink.send(WsMessage::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    let mut connections: HashMap<u64, Opened> = HashMap::new();
    while let Some(message) = incoming.next().await {
        let bytes = match message? {
            WsMessage::Binary(bytes) => bytes,
            WsMessage::Close(_) => break,
            _ => continue,
        };
        let frame = Frame::decode(&bytes)?;
        let connection = frame.connection;
        match frame.kind {
            Kind::Control => match Control::parse(&frame.body) {
                Ok(Control::Opened) => {
                    // Unproven connections are a thread each: past the
                    // limit, closed before one is started.
                    if phones.pending.fetch_add(1, Ordering::AcqRel) >= MAX_PENDING_HANDSHAKES {
                        phones.pending.fetch_sub(1, Ordering::AcqRel);
                        let _ = out.send(Control::Closed.frame(connection));
                        continue;
                    }
                    let (tx, rx) = mpsc::channel();
                    let admitted = Arc::new(AtomicBool::new(false));
                    connections.insert(
                        connection,
                        Opened {
                            tx: tx.clone(),
                            admitted: admitted.clone(),
                            unproven_bytes: 0,
                        },
                    );
                    let pipe = RelayPipe {
                        connection,
                        out: out.clone(),
                        events: rx,
                        feed: tx,
                        deadline: Instant::now() + HANDSHAKE_TIMEOUT,
                        admitted,
                    };
                    let (serving, serving_phones) = (host.clone(), phones.clone());
                    let spawned = std::thread::Builder::new()
                        .name("ket-host-phone".into())
                        .spawn(move || serve_phone(&serving, &serving_phones, pipe));
                    if spawned.is_err() {
                        phones.pending.fetch_sub(1, Ordering::AcqRel);
                        connections.remove(&connection);
                        let _ = out.send(Control::Closed.frame(connection));
                    }
                }
                Ok(Control::Closed) => {
                    if let Some(link) = connections.remove(&connection) {
                        let _ = link.tx.send(Event::Closed);
                    }
                }
                _ => {}
            },
            Kind::Data => {
                let Some(link) = connections.get_mut(&connection) else {
                    continue;
                };
                // Before its handshake lets it in, a connection gets a
                // handshake's worth of bytes and no more.
                if !link.admitted.load(Ordering::Acquire) {
                    link.unproven_bytes += frame.body.len();
                    if link.unproven_bytes > PREADMIT_BYTES {
                        let _ = link.tx.send(Event::Closed);
                        connections.remove(&connection);
                        let _ = out.send(Control::Closed.frame(connection));
                        continue;
                    }
                }
                if link.tx.send(Event::Frame(frame.body)).is_err() {
                    connections.remove(&connection);
                }
            }
        }
    }
    for link in connections.values() {
        let _ = link.tx.send(Event::Closed);
    }
    writer.abort();
    Ok(())
}

/// The relay link's side of one phone connection.
struct Opened {
    tx: mpsc::Sender<Event>,
    /// Set once the handshake has let the phone in.
    admitted: Arc<AtomicBool>,
    /// What it has sent while it was not, against [`PREADMIT_BYTES`].
    unproven_bytes: usize,
}

/// One phone's connection through the relay, as a [`ket_remote::Pipe`].
struct RelayPipe {
    connection: u64,
    out: tokio::sync::mpsc::UnboundedSender<Frame>,
    events: mpsc::Receiver<Event>,
    /// For the pumps sending this phone output: they post to the same
    /// channel the phone's frames arrive on.
    feed: mpsc::Sender<Event>,
    /// When the handshake must be over by: one deadline from when the relay
    /// opened the connection, not a fresh wait per frame.
    deadline: Instant,
    /// Shared with the relay link — see [`Opened::admitted`].
    admitted: Arc<AtomicBool>,
}

fn broken() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the phone has gone")
}

impl ket_remote::Pipe for RelayPipe {
    fn send(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
        self.out
            .send(Frame::data(self.connection, frame))
            .map_err(|_| broken())
    }

    /// Bounded, because it is only used during the handshake: a phone that
    /// connects and says nothing is dropped, not waited on.
    fn recv(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(Event::Frame(frame)) => return Ok(frame),
                Ok(Event::Send(_) | Event::Snapshot(_)) => {}
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => return Err(broken()),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "the phone did not finish its handshake",
                    ));
                }
            }
        }
    }
}

/// Counts a phone as connected while it lives.
struct Connected<'a>(&'a AtomicUsize);

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve_phone(host: &Arc<Host>, phones: &Phones, pipe: RelayPipe) {
    let connection = pipe.connection;
    let out = pipe.out.clone();
    if let Err(error) = session(host, phones, pipe) {
        tracing::info!(connection, %error, "phone session ended");
    }
    let _ = out.send(Control::Closed.frame(connection));
}

/// One phone's session: its Noise session, which device it is, and its name
/// for leases.
struct Phone<'a> {
    session: ket_remote::Session<RelayPipe>,
    device: Vec<u8>,
    holder: u64,
    role: DeviceRole,
    phones: &'a Phones,
}

impl Phone<'_> {
    /// Sends an envelope, numbered, and keeps it for the phone to resume from.
    fn send(&mut self, mut envelope: proto::Envelope) -> Result<()> {
        let bytes = {
            let mut devices = lock(&self.phones.devices);
            let device = devices.entry(self.device.clone()).or_default();
            device.seq += 1;
            envelope.seq = device.seq;
            let bytes = envelope.encode_to_vec();
            device.remember(envelope.seq, &bytes);
            bytes
        };
        self.send_encoded(&bytes)
    }

    fn send_encoded(&mut self, bytes: &[u8]) -> Result<()> {
        for frame in self.session.seal(bytes).map_err(host_error)? {
            ket_remote::Pipe::send(self.session.pipe_mut(), frame).map_err(host_error)?;
        }
        Ok(())
    }
}

/// Counts a handshake as in flight until it is dropped.
struct Pending<'a>(&'a AtomicUsize);

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn session(host: &Arc<Host>, phones: &Phones, mut pipe: RelayPipe) -> Result<()> {
    let pending = Pending(&phones.pending);
    let feed = pipe.feed.clone();
    let admitted = pipe.admitted.clone();

    // The waits on the phone happen with nothing held: a connection that says
    // nothing used to hold the identity — every other handshake, every
    // pairing code, the device list and revocation — until it timed out.
    let first = ket_remote::Pipe::recv(&mut pipe).map_err(host_error)?;
    let answered = lock(&phones.identity).respond(&first);
    let handshake = match answered {
        Ok(handshake) => handshake,
        Err(rejection) => {
            if let Some(refusal) = rejection.refusal() {
                let _ = ket_remote::Pipe::send(&mut pipe, refusal);
            }
            return Err(host_error(rejection.error));
        }
    };
    let (device, paired_now) = (handshake.device.clone(), handshake.paired_now);
    let code = ket_remote::confirmation_code(&handshake.hash);
    let mut session = handshake.finish(pipe).map_err(host_error)?;
    let first = session.recv().map_err(host_error)?;
    let mut phone = Phone {
        session,
        device,
        holder: phones.next_session.fetch_add(1, Ordering::Relaxed),
        // Replaced below before any request is handled.
        role: DeviceRole::Viewer,
        phones,
    };
    // A pairing is granted only for a phone whose first message is a hello
    // this host understands, not merely one that decrypted.
    let hello = proto::Envelope::decode(first.as_slice()).map_err(host_error)?;
    if let Err(refusal) = ket_remote::check_version(&hello) {
        phone.send(envelope(0, Payload::Error(refusal)))?;
        return Ok(());
    }
    let Some(Payload::Hello(hello)) = hello.payload else {
        return Err(host_error("a phone's first message was not a hello"));
    };
    // A scanned code is not enough: a person approves the phone on the
    // desktop, comparing the code it shows with the phone's.
    if paired_now && approved(phones, &mut phone, &hello.device_name, &code)?.is_none() {
        phone.send(error(
            0,
            proto::ErrorCode::Declined,
            "the pairing was not approved on the desktop",
        ))?;
        return Ok(());
    }
    // Every paired device has full access. The desktop's approval is the
    // trust decision, and the owner chose one Approve over a choice of
    // narrower grants (2026-10-01), so whatever role the approval or the
    // file carries, a device that is let in can do what the desktop can.
    let role = DeviceRole::Administrator;
    phone.role = role;

    // Let in and counted live under one hold of the identity, which is what
    // `Phones::revoke` holds too: a phone revoked while its handshake was in
    // flight finds its grant gone here, and one let in is in `live` before a
    // revocation can look.
    {
        let mut identity = lock(&phones.identity);
        identity
            .admit(&phone.device, paired_now)
            .map_err(host_error)?;
        if paired_now {
            identity.name_device(&phone.device, &hello.device_name);
            lock(&phones.roles).insert(phone.device.clone(), role);
            super::identity::save_with_roles(&identity, &lock(&phones.roles))?;
            tracing::info!(device = %hello.device_name, "paired a phone");
        } else if lock(&phones.roles).get(&phone.device) != Some(&role) {
            // Paired before roles existed, or given a narrower one while they
            // were offered: brought up to full access, and the file says so.
            // After `admit`, so only a device that is still paired is written.
            lock(&phones.roles).insert(phone.device.clone(), role);
            super::identity::save_with_roles(&identity, &lock(&phones.roles))?;
        }
        lock(&phones.live)
            .entry(phone.device.clone())
            .or_default()
            .push((phone.holder, feed.clone()));
    }
    admitted.store(true, Ordering::Release);
    drop(pending);
    phones.connected.fetch_add(1, Ordering::AcqRel);
    let _connected = Connected(&phones.connected);
    // Whatever way this ends — the phone leaving, or an error from here on —
    // its terminals are unfitted, its control released, and it leaves `live`.
    let mut held = Held {
        host,
        phones,
        device: phone.device.clone(),
        holder: phone.holder,
        subscriptions: HashMap::new(),
        leased: HashSet::new(),
    };

    // Resume where the phone was, if this is the same run of the host and
    // what it missed is still kept; otherwise it starts over from a snapshot.
    let missed: Option<Vec<Vec<u8>>> = (hello.resume_epoch == epoch())
        .then(|| {
            let devices = lock(&phones.devices);
            let device = devices.get(&phone.device)?;
            device.can_resume(hello.resume_seq).then(|| {
                device
                    .sent
                    .iter()
                    .filter(|(seq, _)| *seq > hello.resume_seq)
                    .map(|(_, bytes)| bytes.clone())
                    .collect()
            })
        })
        .flatten();
    phone.send(envelope(
        0,
        Payload::Welcome(proto::Welcome {
            host_name: host_name(),
            host_epoch: epoch(),
            resumed: missed.is_some(),
        }),
    ))?;
    for bytes in missed.unwrap_or_default() {
        phone.send_encoded(&bytes)?;
    }
    // A fresh picture either way: while no phone was connected no snapshots
    // were sent, so what a resumed phone missed does not include them.
    let first = snapshot(host, phones);
    let mut shown = first.encode_to_vec();
    phone.send(envelope(0, Payload::Snapshot(first)))?;

    loop {
        let event = phone.session.pipe_mut().events.recv();
        match event {
            Ok(Event::Frame(frame)) => {
                let Some(message) = phone.session.open(&frame).map_err(host_error)? else {
                    continue;
                };
                let request = proto::Envelope::decode(message.as_slice()).map_err(host_error)?;
                let reply = handle(
                    host,
                    &phone,
                    &request,
                    &feed,
                    &mut held.subscriptions,
                    &mut held.leased,
                );
                if let Some(reply) = reply {
                    phone.send(reply)?;
                }
            }
            Ok(Event::Send(envelope)) => phone.send(*envelope)?,
            Ok(Event::Snapshot(snapshot)) => {
                let encoded = snapshot.encode_to_vec();
                if encoded != shown {
                    shown = encoded;
                    phone.send(envelope(0, Payload::Snapshot(*snapshot)))?;
                }
            }
            Ok(Event::Closed) | Err(_) => return Ok(()),
        }
    }
}

/// Waits for a person to approve a phone that has just paired. The phone is
/// told to wait, with the code; the desktop's Devices pane shows the same
/// code and the phone's name, and decides. `false` when declined, when it
/// lapses, or when the phone gives up.
fn approved(
    phones: &Phones,
    phone: &mut Phone<'_>,
    name: &str,
    code: &str,
) -> Result<Option<DeviceRole>> {
    let id = phones.next_approval.fetch_add(1, Ordering::Relaxed);
    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    lock(&phones.approvals).push(Approval {
        id,
        name: if name.trim().is_empty() {
            "A phone".to_owned()
        } else {
            name
        },
        code: code.to_owned(),
        asked: Instant::now(),
        decision: None,
    });
    let forget = |phones: &Phones| lock(&phones.approvals).retain(|a| a.id != id);
    tracing::info!(approval = id, "a phone is waiting for approval");
    if let Err(error) = phone.send(envelope(
        0,
        Payload::PairingPending(proto::PairingPending {
            code: code.to_owned(),
            timeout_seconds: u32::try_from(APPROVAL_TIMEOUT.as_secs()).unwrap_or(u32::MAX),
        }),
    )) {
        forget(phones);
        return Err(error);
    }
    let deadline = Instant::now() + APPROVAL_TIMEOUT;
    loop {
        let decision = lock(&phones.approvals)
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.decision);
        if let Some(Some(decision)) = decision {
            forget(phones);
            return Ok(decision);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            forget(phones);
            return Ok(None);
        }
        // The phone going away ends the wait; anything it sends meanwhile
        // is dropped — it has nothing to ask until it is let in.
        match phone
            .session
            .pipe_mut()
            .events
            .recv_timeout(left.min(Duration::from_millis(250)))
        {
            Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                forget(phones);
                return Ok(None);
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// What a phone's session holds on the host, let go when the session ends
/// however it ends — a malformed frame from a paired phone used to return
/// straight past this and leave a terminal fitted to it for good.
struct Held<'a> {
    host: &'a Arc<Host>,
    phones: &'a Phones,
    device: Vec<u8>,
    holder: u64,
    subscriptions: HashMap<u64, Arc<AtomicBool>>,
    leased: HashSet<u64>,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        for (&terminal, stop) in &self.subscriptions {
            stop.store(true, Ordering::Release);
            self.host.unfit(terminal, self.holder);
        }
        // A phone that has gone does not keep control until its lease lapses.
        for &terminal in &self.leased {
            self.host.release(terminal, self.holder);
        }
        let mut live = lock(&self.phones.live);
        if let Some(sessions) = live.get_mut(&self.device) {
            sessions.retain(|(holder, _)| *holder != self.holder);
            if sessions.is_empty() {
                live.remove(&self.device);
            }
        }
    }
}

/// What the phone calls this Mac: the name it has in System Settings.
///
/// `scutil` rather than a system call, the way ket reaches the platform
/// elsewhere; asked once per run.
fn host_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::process::Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Mac".to_owned())
    })
    .clone()
}

fn envelope(request_id: u64, payload: Payload) -> proto::Envelope {
    let mut envelope = ket_remote::envelope(request_id, payload);
    envelope.host_epoch = epoch();
    envelope
}

fn error(request_id: u64, code: proto::ErrorCode, message: &str) -> proto::Envelope {
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
/// [`super::server::LEASE`]. Interrupting and ending what runs in a
/// terminal do not: stopping an agent is always a paired phone's to do.
/// Any request carrying an idempotency key the host has already applied is
/// acknowledged and not applied again, which is what makes a retry after a
/// reconnect safe.
fn handle(
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
    if mutation && succeeded {
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
        // A first read takes the recent end of a transcript; the poll after
        // it reads on from a cursor, which is a seek and usually nothing.
        Payload::GetConversation(read) if read.after == 0 => Some(3),
        Payload::GetConversation(_) => Some(1),
        Payload::StartWork(_) | Payload::MergeWorktree(_) | Payload::StartNote(_) => Some(10),
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
        Payload::MergeWorktree(_) => "merge_worktree",
        Payload::StartNote(_) => "start_note",
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
        Payload::StartWork(_) | Payload::MergeWorktree(_) | Payload::StartNote(_) => {
            DeviceRole::Operator
        }
        Payload::Input(_)
        | Payload::Resize(_)
        | Payload::Lease(_)
        | Payload::Signal(_)
        | Payload::Answer(_)
        | Payload::EditBacklog(_) => DeviceRole::Controller,
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
            if is_interrupt(&input.bytes) {
                match input.bytes.as_slice() {
                    [0x03] => host.cancel_terminal(input.terminal_id, CancelSource::Phone),
                    [0x1b] => host.escape_terminal(input.terminal_id),
                    _ => {}
                }
            }
            let _ = terminal.send_input(input.bytes.clone());
            ack(id)
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
            if host.start_work(work.project_id.clone(), prompt.to_owned(), agent, None) {
                ack(id)
            } else {
                Some(error(
                    id,
                    proto::ErrorCode::NotFound,
                    "open ket on your Mac to start work from here",
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
fn decision(
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
                })
                .collect(),
            next: chunk.next,
            fresh: chunk.fresh,
        },
        Err(error) => unsupported(format!("couldn't read the session: {error}")),
    }
}

/// The keys that answer an agent's permission prompt with `choice`, or `None`
/// when ket does not know that agent's menu well enough to press them.
///
/// Claude Code draws a numbered menu and takes the number: `1` allows,
/// `2` allows and stops asking where the prompt offers that, and Escape
/// declines. A plan to approve or a question to answer is not a permission,
/// and its `1` means something else, so those are left to the terminal.
/// Codex: see [`codex_keys`]; OpenCode: [`opencode_keys`]; Grok:
/// [`grok_keys`]. Any other agent
/// is not answered until its menu is checked.
fn answer_keys(agent: &str, tool: &str, choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    if agent.eq_ignore_ascii_case("codex") {
        return codex_keys(tool, choice);
    }
    if agent.eq_ignore_ascii_case("opencode") {
        return opencode_keys(choice);
    }
    if agent.eq_ignore_ascii_case("grok") {
        return grok_keys(tool, choice);
    }
    if !agent.eq_ignore_ascii_case("claude") || matches!(tool, "ExitPlanMode" | "AskUserQuestion") {
        return None;
    }
    match choice {
        Choice::AllowOnce => Some(b"1"),
        Choice::AllowAlways if offers_always(tool) => Some(b"2"),
        Choice::Deny => Some(&[0x1b]),
        _ => None,
    }
}

/// Codex's approval menus, as Codex CLI 0.159's TUI draws them with its
/// default keymap (`[tui.keymap.approval]` in `~/.codex/config.toml` can
/// rebind them). Each option takes a letter: `y` accepts,
/// `a` accepts for the rest of the session. Escape is the menu's cancel,
/// checked before any option, so it works whatever the menu offers; it
/// cancels the request and ends the turn, as Claude Code's Escape does.
///
/// Its hooks name the prompt `Bash` for a command (a network request
/// included), `apply_patch` for a file edit and `request_permissions` for
/// more sandbox access — each with a `y` option. `a` is pressed only for a
/// file edit, whose menu always has it; a command's menu offers it only
/// sometimes. `write_stdin` and MCP tools are left to the terminal.
fn codex_keys(tool: &str, choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    if !matches!(tool, "Bash" | "apply_patch" | "request_permissions") {
        return None;
    }
    match choice {
        Choice::AllowOnce => Some(b"y"),
        Choice::AllowAlways if tool == "apply_patch" => Some(b"a"),
        Choice::Deny => Some(&[0x1b]),
        _ => None,
    }
}

/// OpenCode's permission prompt, as its TUI draws it in 1.18.33
/// (`tui/src/routes/session/permission.tsx`): three buttons — Allow once,
/// Allow always, Reject — with Allow once selected when it opens. Enter
/// takes the selected one; Escape rejects, which ends the turn. The same
/// dialog for every tool.
///
/// No Allow always: it is a move right, then Enter, then Enter again on a
/// confirmation, and which button is selected can change under a mouse. Only
/// the main session's prompts reach here — ket's plugin leaves a subagent's
/// out, whose Reject opens a feedback box instead. Keybinds for this dialog
/// live in OpenCode's `tui.json`; Enter and the arrows cannot be rebound.
fn opencode_keys(choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    match choice {
        Choice::AllowOnce => Some(b"\r"),
        Choice::Deny => Some(&[0x1b]),
        _ => None,
    }
}

/// Grok's permission menu, as Grok 1.0.44's TUI draws it: numbered options,
/// each taken by its digit with no Enter. `3` allows once and `4` rejects
/// and ends the turn. `2` always allows that command (a shell command), every
/// edit for the session (an edit or a new file), or that domain for the
/// project (a fetch). The menu can open on `1`, which turns off every prompt
/// for the session — never pressed. Escape does not decline: it moves focus
/// off the menu and leaves the prompt open.
///
/// Checked for a shell command, an edit, a new file and a fetch — Grok's
/// `run_terminal_command`, `search_replace`, `write` and `web_fetch`, which
/// ket's hook reader spells as Claude's names. Any other tool, an MCP tool
/// among them, is left to the terminal.
fn grok_keys(tool: &str, choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    if !matches!(tool, "Bash" | "Edit" | "Write" | "WebFetch") {
        return None;
    }
    match choice {
        Choice::AllowOnce => Some(b"3"),
        Choice::AllowAlways => Some(b"2"),
        Choice::Deny => Some(b"4"),
        _ => None,
    }
}

/// Whether Claude Code's prompt for `tool` has the three-line menu whose
/// second line stops asking. Every other prompt's `2` is not that.
fn offers_always(tool: &str) -> bool {
    matches!(
        tool,
        "Bash" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "WebFetch"
    )
}

/// Whether a keystroke asks the program to stop: Ctrl-C, or a bare Escape,
/// which is how Claude Code and Codex cancel a turn. The same test a window
/// makes on its own keystrokes.
pub(super) fn is_interrupt(bytes: &[u8]) -> bool {
    matches!(bytes, [0x03] | [0x1b])
}

/// The host's terminals, grouped by worktree and project, with what each
/// worktree's agent is doing.
fn snapshot(host: &Host, phones: &Phones) -> proto::HostSnapshot {
    let listing = host.listing();
    // Every worktree a terminal is filed under, so one made a moment ago is
    // placed in its project now rather than a few seconds from now.
    let named: Vec<&str> = listing
        .iter()
        .filter_map(|(_, key, _)| key.as_deref())
        .map(|key| key.split('|').next().unwrap_or_default())
        .filter(|worktree| !worktree.is_empty())
        .collect();
    let places = phones.places_knowing(&named);
    let figures = phones.figures(&places);
    let backlogs = phones.backlog_counts(&places);
    let now = crate::now_ms();
    let mut worktrees: Vec<proto::WorktreeSummary> = Vec::new();
    // Which terminal each pane is, for naming the one a prompt is in.
    let mut terminal_of: HashMap<String, u64> = HashMap::new();
    for (id, key, terminal) in listing.iter().cloned() {
        if let Some(key) = &key {
            terminal_of.insert(key.clone(), id);
        }
        // Keys are `worktree|agent|nonce`; a terminal without one is still
        // listed, under no worktree.
        let mut parts = key.as_deref().unwrap_or_default().splitn(3, '|');
        let worktree = parts.next().unwrap_or_default().to_owned();
        let agent = parts
            .next()
            .filter(|a| *a != "shell")
            .unwrap_or_default()
            .to_owned();
        let summary = proto::TerminalSummary {
            id,
            title: terminal.title().unwrap_or_else(|| {
                if agent.is_empty() {
                    "Terminal".into()
                } else {
                    agent.clone()
                }
            }),
            agent,
            closed: terminal.is_closed(),
        };
        match worktrees.iter_mut().find(|w| w.id == worktree) {
            Some(existing) => existing.terminals.push(summary),
            None => {
                let place = places.get(&worktree);
                let figure = figures.get(&worktree).copied().unwrap_or_default();
                worktrees.push(proto::WorktreeSummary {
                    name: place.map_or_else(|| worktree.clone(), |p| p.name.clone()),
                    branch: place.map(|p| p.branch.clone()).unwrap_or_default(),
                    base: place.and_then(|p| p.base.clone()).unwrap_or_default(),
                    files_changed: figure.files_changed,
                    lines_added: figure.lines_added,
                    lines_removed: figure.lines_removed,
                    commits_ahead: figure.commits_ahead,
                    id: worktree,
                    terminals: vec![summary],
                    ..Default::default()
                });
            }
        }
    }

    let status = host.agent_status();
    {
        let mut activity = lock(&phones.activity);
        activity.accept_snapshot(status.clone());
        for worktree in &mut worktrees {
            if worktree.id.is_empty() {
                continue;
            }
            let id = WorktreeId::new(worktree.id.clone());
            let now_doing = activity.activity(&id, now);
            worktree.agent_state = now_doing.detail();
            worktree.agent = now_doing.agent().unwrap_or_default().to_owned();
            worktree.subagents = activity
                .subagents(&id, now)
                .into_iter()
                .map(|subagent| proto::SubagentSummary {
                    id: subagent.id,
                    label: subagent.label,
                    model: subagent.model.unwrap_or_default(),
                    state: match subagent.state {
                        SubagentState::Working => proto::subagent_summary::State::Working,
                        SubagentState::Blocked => proto::subagent_summary::State::Blocked,
                        SubagentState::Idle => proto::subagent_summary::State::Idle,
                    } as i32,
                    started_at_ms: subagent.started_at_ms,
                })
                .collect();
            worktree.activity = match activity.signal(&id, now) {
                Signal::Quiet => proto::Activity::Quiet,
                Signal::Running => proto::Activity::Running,
                Signal::Working => proto::Activity::Working,
                Signal::Merging => proto::Activity::Merging,
                Signal::Blocked => proto::Activity::Blocked,
                Signal::Failed => proto::Activity::Failed,
            } as i32;
            // The latest prompt among the worktree's panes, while one waits.
            worktree.question = status
                .panes
                .iter()
                .filter(|pane| {
                    pane.worktree == worktree.id && pane.state == AgentState::AwaitingPermission
                })
                .max_by_key(|pane| pane.at_ms)
                .and_then(|pane| {
                    let question = pane.question.as_ref()?;
                    // The terminal the prompt is in, which the answer must
                    // name. Without one — or before the prompt has an id —
                    // it can be read on the phone but not answered.
                    let terminal = terminal_of.get(&pane.pane).copied();
                    let bound = terminal.is_some() && question.id != 0;
                    // A question or a plan is answered through its held
                    // hook, so it can be while one is held.
                    let answerable = match question.prompt {
                        Some(_) => bound && host.prompt_held(&pane.pane, question),
                        None => {
                            bound
                                && answer_keys(
                                    &pane.agent,
                                    &question.tool,
                                    proto::answer::Choice::AllowOnce,
                                )
                                .is_some()
                        }
                    };
                    Some(proto::Question {
                        tool: question.tool.clone(),
                        subject: question.subject.clone().unwrap_or_default(),
                        answerable,
                        always: bound
                            && answer_keys(
                                &pane.agent,
                                &question.tool,
                                proto::answer::Choice::AllowAlways,
                            )
                            .is_some(),
                        id: question.id,
                        terminal_id: terminal.unwrap_or_default(),
                        // Nothing to weigh: it asks for no access at all.
                        risk: question
                            .prompt
                            .is_none()
                            .then(|| risk_read(question))
                            .flatten(),
                        asks: match &question.prompt {
                            Some(Prompt::Ask { questions }) => questions
                                .iter()
                                .map(|asked| proto::Asked {
                                    question: asked.question.clone(),
                                    header: asked.header.clone(),
                                    choices: asked
                                        .options
                                        .iter()
                                        .map(|option| proto::asked::Choice {
                                            label: option.label.clone(),
                                            description: option.description.clone(),
                                        })
                                        .collect(),
                                    multi_select: asked.multi_select,
                                })
                                .collect(),
                            _ => Vec::new(),
                        },
                        plan: matches!(question.prompt, Some(Prompt::Plan)),
                    })
                });
        }
    }

    // Projects in the order a sidebar has them, then anything ket's store
    // does not know, under the Mac itself.
    let mut projects: Vec<proto::ProjectSummary> = places
        .projects
        .iter()
        .map(|project| proto::ProjectSummary {
            id: project.id.clone(),
            name: project.name.clone(),
            worktrees: Vec::new(),
            color: project.color.clone(),
            backlog: backlogs.get(&project.id).copied().unwrap_or(0),
        })
        .collect();
    let mut elsewhere = Vec::new();
    for worktree in worktrees {
        let project = places
            .get(&worktree.id)
            .and_then(|place| projects.iter_mut().find(|p| p.id == place.project));
        match project {
            Some(project) => project.worktrees.push(worktree),
            None => elsewhere.push(worktree),
        }
    }
    // Every project the sidebar has stays, running or not: the phone lists
    // them as the desktop does, starts new work in any of them, and reaches
    // each one's backlog.
    for project in &mut projects {
        project.worktrees.sort_by_key(|w| {
            places
                .worktrees
                .iter()
                .position(|place| place.id == w.id)
                .unwrap_or(usize::MAX)
        });
    }
    if !elsewhere.is_empty() {
        projects.push(proto::ProjectSummary {
            id: String::new(),
            name: "This desktop".into(),
            worktrees: elsewhere,
            color: String::new(),
            backlog: 0,
        });
    }
    proto::HostSnapshot {
        projects,
        palette: host.theme().as_ref().map(palette),
        usage: host.usage().iter().map(provider_usage).collect(),
        agents: host.agents(),
    }
}

/// The desktop's theme, as the colours a phone draws with.
fn palette(theme: &crate::theme::Theme) -> proto::Palette {
    let hex = |color: crate::theme::Color| color.to_hex();
    proto::Palette {
        light: theme.appearance == crate::theme::Appearance::Light,
        backdrop: hex(theme.backdrop),
        surface: hex(theme.surface),
        sunken: hex(theme.sunken),
        elevated: hex(theme.elevated),
        hover: hex(theme.hover),
        selection: hex(theme.selection),
        border: hex(theme.border),
        rule: hex(theme.rule),
        accent: hex(theme.accent),
        on_accent: hex(theme.on_accent),
        text: hex(theme.text.primary),
        dim: hex(theme.text.dim),
        marker: hex(theme.marker),
        running: hex(theme.status.running),
        attention: hex(theme.status.attention),
        failed: hex(theme.status.failed),
        merging: hex(theme.status.merging),
        added: hex(theme.diff.added),
        removed: hex(theme.diff.removed),
        modified: hex(theme.diff.modified),
        quota_warm: hex(theme.quota.warm),
        quota_hot: hex(theme.quota.hot),
        quota_critical: hex(theme.quota.critical),
    }
}

/// One agent's plan usage, for a phone.
/// A risk read and its confidence.
type RiskRead = (crate::jev::Risk, f32);

/// Jev's risk reads, by prompt id: `None` while one is being asked for.
static RISKS: OnceLock<Mutex<HashMap<u64, Option<RiskRead>>>> = OnceLock::new();

/// Most prompts remembered before the oldest reads are dropped.
const RISKS_KEPT: usize = 256;

/// A prompt's risk read, when Jev's beta is on and has answered.
///
/// The first snapshot to see a prompt asks for its read on a thread of its
/// own and goes out without it; the snapshot tick after the answer carries
/// it. Each prompt is asked about once, whatever the answer.
fn risk_read(question: &crate::agent_hooks::PermissionQuestion) -> Option<proto::Risk> {
    if question.id == 0 {
        return None;
    }
    let risks = RISKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut known = lock(risks);
    if let Some(read) = known.get(&question.id) {
        return read.map(|(risk, confidence)| proto::Risk {
            level: risk.word().to_owned(),
            confidence,
        });
    }
    if known.len() >= RISKS_KEPT {
        known.clear();
    }
    known.insert(question.id, None);
    drop(known);
    let (id, tool, subject) = (
        question.id,
        question.tool.clone(),
        question.subject.clone().unwrap_or_default(),
    );
    let asked = std::thread::Builder::new()
        .name("ket-host-jev".into())
        .spawn(move || {
            let Ok(config) = crate::config::Config::load() else {
                return;
            };
            let read = crate::jev::risk_of(&config.jev, &tool, &subject);
            if read.is_some() {
                lock(risks).insert(id, read);
            }
        });
    if let Err(e) = asked {
        tracing::info!("jev: could not ask for a risk read: {e}");
    }
    None
}

fn provider_usage(snapshot: &ProviderSnapshot) -> proto::ProviderUsage {
    let (status, reason) = match &snapshot.status {
        SnapshotStatus::Fresh => (proto::UsageStatus::Fresh, String::new()),
        SnapshotStatus::Stale => (proto::UsageStatus::Stale, String::new()),
        SnapshotStatus::Unavailable { reason } => (proto::UsageStatus::Unavailable, reason.clone()),
        SnapshotStatus::Unsupported => (proto::UsageStatus::Unsupported, String::new()),
    };
    // Being turned away outranks whatever else there is to say about a
    // provider, and is the one thing about it a phone must not miss.
    let reason = match snapshot.limited_at_ms {
        Some(_) => "Rate limited".to_owned(),
        None => reason,
    };
    proto::ProviderUsage {
        provider: snapshot.provider.label().to_owned(),
        status: status as i32,
        windows: snapshot
            .windows
            .iter()
            .map(|window| proto::UsageWindow {
                name: window.name.clone(),
                used_percent: window.used_percent,
                resets_at_ms: window.resets_at_ms.unwrap_or_default(),
            })
            .collect(),
        plan: snapshot.plan.clone().unwrap_or_default(),
        fetched_at_ms: snapshot.fetched_at_ms.unwrap_or_default(),
        reason,
    }
}

/// Sends one terminal to one phone: a checkpoint, then its output, until
/// the phone lets go or the program ends.
fn pump(id: u64, terminal: &LocalTerminal, feed: &mpsc::Sender<Event>, stop: &AtomicBool) {
    let post = |payload: Payload| {
        let mut envelope = envelope(0, payload);
        envelope.stream_id = id;
        feed.send(Event::Send(Box::new(envelope))).is_ok()
    };
    let checkpoint = || {
        let checkpoint = terminal.checkpoint();
        let posted = post(Payload::Checkpoint(proto::TerminalCheckpoint {
            terminal_id: id,
            epoch: checkpoint.cursor.epoch,
            through_seq: checkpoint.cursor.seq,
            cols: u32::from(checkpoint.size.cols),
            rows: u32::from(checkpoint.size.rows),
            scrollback: checkpoint.scrollback as u32,
            ansi: checkpoint.ansi,
        }));
        posted.then_some(checkpoint.cursor)
    };

    let mut frames = terminal.frames();
    frames.borrow_and_update();
    let Some(mut cursor) = checkpoint() else {
        return;
    };
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        match terminal.output_since(cursor) {
            Since::Output {
                bytes,
                cursor: next,
            } => {
                if !bytes.is_empty()
                    && !post(Payload::Chunk(proto::TerminalChunk {
                        terminal_id: id,
                        epoch: cursor.epoch,
                        seq: cursor.seq,
                        bytes,
                    }))
                {
                    return;
                }
                cursor = next;
            }
            Since::Reset => {
                if !post(Payload::Reset(proto::TerminalReset { terminal_id: id })) {
                    return;
                }
                match checkpoint() {
                    Some(next) => cursor = next,
                    None => return,
                }
            }
        }
        if terminal.is_closed() {
            let status = terminal.exit_status();
            post(Payload::Closed(proto::TerminalClosed {
                terminal_id: id,
                exit_code: status.as_ref().map(portable_pty::ExitStatus::exit_code),
                signal: status
                    .as_ref()
                    .and_then(|s| s.signal().map(str::to_owned))
                    .unwrap_or_default(),
            }));
            return;
        }
        if futures::executor::block_on(frames.changed()).is_err() {
            return;
        }
    }
}

/// The phone side end to end, lightly: a real relay on loopback, the host's
/// link to it, and a phone speaking the protocol through it — pairing,
/// who is let back in, and what a phone can do to a terminal. Kept to a few
/// tests while phones are a prototype.
///
/// Each test runs again in a child process with a throwaway
/// `XDG_DATA_HOME`: pairing and revoking write the host's identity and
/// device list to the data directory, and a test cannot point its own
/// process somewhere else (see `crate::paths`).
#[cfg(test)]
mod tests {
    use std::net::TcpStream;
    use std::process::Command;

    use ket_relay_protocol::REFUSED_HOST_OFFLINE;
    use ket_remote::{Keypair, Offer};
    use tokio_tungstenite::tungstenite::{self, WebSocket, stream::MaybeTlsStream};

    use super::*;
    use crate::terminal::TerminalSpec;

    /// Longest any one step waits before the test calls it stuck.
    const WAIT: Duration = Duration::from_secs(10);

    /// Set in the child a test runs in.
    const SANDBOXED: &str = "KET_PHONES_TEST_SANDBOX";

    /// In the parent: runs test `name` again in a child whose data directory
    /// is a temporary one, checks it passed, and returns `true`. In that
    /// child: `false`, and the test goes on.
    fn rerun_sandboxed(name: &str) -> bool {
        if std::env::var_os(SANDBOXED).is_some() {
            assert!(std::env::var_os("XDG_DATA_HOME").is_some_and(|dir| !dir.is_empty()));
            return false;
        }
        let data = std::env::temp_dir().join(format!("ket-phones-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).expect("a data directory");
        let out = Command::new(std::env::current_exe().expect("this test binary"))
            .arg(format!("host::phones::tests::{name}"))
            .args(["--exact", "--test-threads=1", "--nocapture"])
            .env(SANDBOXED, "1")
            .env("XDG_DATA_HOME", &data)
            .env_remove(super::super::RELAY_ENV)
            .output()
            .expect("the test runs again");
        let _ = std::fs::remove_dir_all(&data);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        true
    }

    /// A host with phones on, linked to a relay of its own on a free
    /// loopback port.
    struct Rig {
        host: Arc<Host>,
        phones: Arc<Phones>,
        relay: String,
    }

    fn rig() -> Rig {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let relay = format!("ws://{}", listener.local_addr().expect("its address"));
        listener
            .set_nonblocking(true)
            .expect("a nonblocking listener");
        let host = Host::for_tests();
        let phones = Arc::new(Phones {
            identity: Mutex::new(ket_remote::Host::generate().expect("a host identity")),
            roles: Mutex::new(HashMap::new()),
            relay: Relay::External(relay.clone()),
            stopped: AtomicBool::new(false),
            stop: tokio::sync::watch::channel(false).0,
            connected: AtomicUsize::new(0),
            devices: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            live: Mutex::new(HashMap::new()),
            activity: Mutex::new(Tracker::new(Vec::<String>::new())),
            places: Mutex::new(None),
            figures: Mutex::new(None),
            backlogs: Mutex::new(None),
            last_snapshot: Mutex::new(Vec::new()),
            pending: AtomicUsize::new(0),
            approvals: Mutex::new(Vec::new()),
            next_approval: AtomicU64::new(1),
            claim: None,
        });
        let (serving, linked) = (host.clone(), phones.clone());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
                tokio::select! {
                    () = ket_relay::serve(listener, None) => {}
                    () = link_forever(&serving, &linked) => {}
                }
            });
        });
        Rig {
            host,
            phones,
            relay,
        }
    }

    /// A phone's connection through the relay.
    struct PhonePipe {
        socket: WebSocket<MaybeTlsStream<TcpStream>>,
        connection: u64,
    }

    impl PhonePipe {
        /// Connects to host `id`. The relay does not acknowledge a host's
        /// registration, so a phone that arrives first is refused; this tries
        /// again until the host is there.
        fn connect(relay: &str, id: [u8; 16]) -> Self {
            let deadline = Instant::now() + WAIT;
            loop {
                let (mut socket, _) = tungstenite::connect(relay).expect("the relay");
                if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
                    stream.set_read_timeout(Some(WAIT)).expect("a read timeout");
                }
                let connect = Control::Connect { host: id }
                    .frame(0)
                    .encode()
                    .expect("a frame");
                socket.send(WsMessage::binary(connect)).expect("sent");
                let frame = loop {
                    if let WsMessage::Binary(bytes) = socket.read().expect("the relay answers") {
                        break Frame::decode(&bytes).expect("a frame");
                    }
                };
                match Control::parse(&frame.body) {
                    Ok(Control::Connected) => {
                        return Self {
                            socket,
                            connection: frame.connection,
                        };
                    }
                    Ok(Control::Refused {
                        reason: REFUSED_HOST_OFFLINE,
                    }) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    other => panic!("{other:?}"),
                }
            }
        }
    }

    impl ket_remote::Pipe for PhonePipe {
        fn send(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
            let bytes = Frame::data(self.connection, frame)
                .encode()
                .map_err(std::io::Error::other)?;
            self.socket
                .send(WsMessage::binary(bytes))
                .map_err(std::io::Error::other)
        }

        /// The next data frame; an error once the connection is closed.
        fn recv(&mut self) -> std::io::Result<Vec<u8>> {
            loop {
                match self.socket.read().map_err(std::io::Error::other)? {
                    WsMessage::Binary(bytes) => {
                        let frame = Frame::decode(&bytes).map_err(std::io::Error::other)?;
                        if frame.kind == Kind::Data {
                            return Ok(frame.body);
                        }
                        if Control::parse(&frame.body) == Ok(Control::Closed) {
                            return Err(broken());
                        }
                    }
                    WsMessage::Close(_) => return Err(broken()),
                    _ => {}
                }
            }
        }
    }

    type PhoneSession = ket_remote::Session<PhonePipe>;

    fn hello(resume_epoch: u64, resume_seq: u64) -> proto::Envelope {
        ket_remote::envelope(
            0,
            Payload::Hello(proto::Hello {
                device_name: "Test phone".into(),
                resume_epoch,
                resume_seq,
                ..Default::default()
            }),
        )
    }

    /// Pairs `device` with a fresh code from the host and says hello. Not
    /// yet approved.
    fn pair(rig: &Rig, device: &Keypair) -> PhoneSession {
        let offer = Offer::from_code(&rig.phones.pairing_code()).expect("a pairing code");
        let pipe = PhonePipe::connect(&offer.relay, offer.host_id);
        let mut session = ket_remote::pair(device, &offer, pipe).expect("the handshake");
        session.send_envelope(&hello(0, 0)).expect("hello");
        session
    }

    /// Reconnects `device` to the rig's host and says `hello`.
    fn resume(
        rig: &Rig,
        device: &Keypair,
        hello: &proto::Envelope,
    ) -> ket_remote::Result<PhoneSession> {
        let (id, public) = {
            let identity = lock(&rig.phones.identity);
            (identity.id, identity.public())
        };
        let pipe = PhonePipe::connect(&rig.relay, id);
        let mut session = ket_remote::resume(device, &id, &public, pipe)?;
        session.send_envelope(hello)?;
        Ok(session)
    }

    fn next(session: &mut PhoneSession) -> proto::Envelope {
        session.recv_envelope().expect("an envelope from the host")
    }

    /// Decides the one pairing waiting, once it is, checking it shows the
    /// code the phone was given.
    fn decide(rig: &Rig, code: &str, approve: bool) {
        let deadline = Instant::now() + WAIT;
        let waiting = loop {
            if let [waiting] = rig.phones.pairings().as_slice() {
                break waiting.clone();
            }
            assert!(Instant::now() < deadline, "no pairing waited for approval");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(waiting.code, code);
        assert_eq!(waiting.name, "Test phone");
        let role = approve.then_some(DeviceRole::Administrator);
        assert!(rig.phones.decide(waiting.id, role));
    }

    /// The code a phone that has just paired is asked to wait with.
    fn pending(session: &mut PhoneSession) -> String {
        match next(session).payload {
            Some(Payload::PairingPending(pending)) => pending.code,
            other => panic!("the phone was not asked to wait for approval: {other:?}"),
        }
    }

    /// A phone paired and approved, past its welcome and first snapshot,
    /// and the sequence number of that snapshot.
    fn paired(rig: &Rig, device: &Keypair) -> (PhoneSession, u64) {
        let mut session = pair(rig, device);
        let code = pending(&mut session);
        decide(rig, &code, true);
        assert!(matches!(
            next(&mut session).payload,
            Some(Payload::Welcome(_))
        ));
        let snapshot = next(&mut session);
        assert!(matches!(snapshot.payload, Some(Payload::Snapshot(_))));
        (session, snapshot.seq)
    }

    /// Sends a request under its own idempotency key and reads the answer.
    fn ask(session: &mut PhoneSession, id: u64, payload: Payload) -> proto::Envelope {
        let mut request = ket_remote::envelope(id, payload);
        request.idempotency_key = format!("test-{id}");
        session.send_envelope(&request).expect("the request");
        next(session)
    }

    fn is_ack(reply: &proto::Envelope) -> bool {
        matches!(reply.payload, Some(Payload::Ack(_)))
    }

    fn error_code(reply: &proto::Envelope) -> Option<proto::ErrorCode> {
        match &reply.payload {
            Some(Payload::Error(error)) => proto::ErrorCode::try_from(error.code).ok(),
            _ => None,
        }
    }

    /// `command` in a terminal the rig's host owns.
    fn terminal(rig: &Rig, command: &str, args: &[&str]) -> (u64, Arc<LocalTerminal>) {
        let spec = TerminalSpec {
            command: command.into(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cwd: std::env::temp_dir(),
            env: std::collections::BTreeMap::new(),
            env_remove: Vec::new(),
            size: TerminalSize::new(80, 24),
            key: None,
            adopt: false,
        };
        let id = rig
            .host
            .file_for_tests(LocalTerminal::open(&spec).expect("a terminal"));
        (id, rig.host.find(id).expect("filed"))
    }

    fn eventually(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_phone_pairs_through_the_relay_once_the_desktop_approves() {
        if rerun_sandboxed("a_phone_pairs_through_the_relay_once_the_desktop_approves") {
            return;
        }
        let rig = rig();
        let mut session = pair(&rig, &Keypair::generate().unwrap());
        let code = pending(&mut session);
        // A scanned code is not enough on its own.
        assert!(rig.phones.devices().is_empty());

        decide(&rig, &code, true);
        let Some(Payload::Welcome(welcome)) = next(&mut session).payload else {
            panic!("no welcome");
        };
        assert_eq!(welcome.host_epoch, epoch());
        assert!(!welcome.resumed);
        assert!(matches!(
            next(&mut session).payload,
            Some(Payload::Snapshot(_))
        ));

        let devices = rig.phones.devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Test phone");
        assert!(devices[0].connected);
        let saved = crate::host::identity::load()
            .unwrap()
            .expect("a saved identity");
        assert_eq!(saved.grants().len(), 1);
    }

    #[test]
    fn a_declined_pairing_is_turned_away_and_not_granted() {
        if rerun_sandboxed("a_declined_pairing_is_turned_away_and_not_granted") {
            return;
        }
        let rig = rig();
        let device = Keypair::generate().unwrap();
        let mut session = pair(&rig, &device);
        let code = pending(&mut session);
        decide(&rig, &code, false);

        assert_eq!(
            error_code(&next(&mut session)),
            Some(proto::ErrorCode::Declined)
        );
        assert!(session.recv().is_err(), "the connection stayed open");
        assert!(rig.phones.devices().is_empty());
        assert!(matches!(
            resume(&rig, &device, &hello(0, 0)),
            Err(ket_remote::Error::Refused)
        ));
    }

    #[test]
    fn a_phone_the_host_never_granted_is_refused() {
        if rerun_sandboxed("a_phone_the_host_never_granted_is_refused") {
            return;
        }
        let rig = rig();
        assert!(matches!(
            resume(&rig, &Keypair::generate().unwrap(), &hello(0, 0)),
            Err(ket_remote::Error::Refused)
        ));

        // Nor is a code good twice: the first use spends it.
        let offer = Offer::from_code(&rig.phones.pairing_code()).unwrap();
        let first = PhonePipe::connect(&offer.relay, offer.host_id);
        let _first = ket_remote::pair(&Keypair::generate().unwrap(), &offer, first).unwrap();
        let second = PhonePipe::connect(&offer.relay, offer.host_id);
        assert!(matches!(
            ket_remote::pair(&Keypair::generate().unwrap(), &offer, second),
            Err(ket_remote::Error::Refused)
        ));
        assert!(rig.phones.devices().is_empty());
    }

    #[test]
    fn a_granted_phone_reconnects_and_resumes_without_asking_again() {
        if rerun_sandboxed("a_granted_phone_reconnects_and_resumes_without_asking_again") {
            return;
        }
        let rig = rig();
        let device = Keypair::generate().unwrap();
        let (session, seq) = paired(&rig, &device);
        drop(session);
        eventually("the first session did not end", || {
            rig.phones.connected() == 0
        });

        let mut session = resume(&rig, &device, &hello(epoch(), seq)).expect("let back in");
        let Some(Payload::Welcome(welcome)) = next(&mut session).payload else {
            panic!("a reconnect was not welcomed straight away");
        };
        assert!(welcome.resumed);
        assert!(rig.phones.pairings().is_empty());
        assert_eq!(rig.phones.devices().len(), 1);
    }

    #[test]
    fn a_keystroke_from_a_phone_reaches_the_terminal_once_it_has_control() {
        if rerun_sandboxed("a_keystroke_from_a_phone_reaches_the_terminal_once_it_has_control") {
            return;
        }
        let rig = rig();
        let (id, terminal) = terminal(&rig, "/bin/cat", &[]);
        let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
        let input = |bytes: &[u8]| {
            Payload::Input(proto::Input {
                terminal_id: id,
                bytes: bytes.to_vec(),
            })
        };

        let refused = ask(&mut session, 1, input(b"ket-too-soon\r"));
        assert_eq!(error_code(&refused), Some(proto::ErrorCode::NotController));

        let lease = Payload::Lease(proto::Lease {
            terminal_id: id,
            action: proto::lease::Action::Acquire as i32,
        });
        assert!(is_ack(&ask(&mut session, 2, lease)));
        assert!(is_ack(&ask(&mut session, 3, input(b"ket-from-a-phone\r"))));

        let screen = || String::from_utf8_lossy(&terminal.checkpoint().ansi).into_owned();
        eventually("the keystrokes never reached the terminal", || {
            screen().contains("ket-from-a-phone")
        });
        assert!(!screen().contains("ket-too-soon"));
        terminal.kill();
    }

    #[test]
    fn a_terminate_signal_from_a_phone_ends_the_program() {
        if rerun_sandboxed("a_terminate_signal_from_a_phone_ends_the_program") {
            return;
        }
        let rig = rig();
        let (id, terminal) = terminal(&rig, "/bin/sleep", &["30"]);
        let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

        let signal = Payload::Signal(proto::Signal {
            terminal_id: id,
            kind: proto::signal::Kind::Terminate as i32,
        });
        assert!(is_ack(&ask(&mut session, 1, signal)));
        eventually("the program outlived the signal", || terminal.is_closed());
    }

    #[test]
    fn malformed_input_from_a_phone_ends_only_its_own_session() {
        if rerun_sandboxed("malformed_input_from_a_phone_ends_only_its_own_session") {
            return;
        }
        let rig = rig();
        let id = lock(&rig.phones.identity).id;

        // Not a handshake at all: dropped, without a refusal.
        let mut stranger = PhonePipe::connect(&rig.relay, id);
        ket_remote::Pipe::send(&mut stranger, b"not a ket handshake".to_vec()).unwrap();
        assert!(ket_remote::Pipe::recv(&mut stranger).is_err());

        // A paired phone whose message decrypts but is not an envelope.
        let device = Keypair::generate().unwrap();
        let (mut session, _) = paired(&rig, &device);
        session.send(&[0xff; 11]).unwrap();
        assert!(
            session.recv().is_err(),
            "the session outlived a malformed message"
        );
        eventually("the session did not end", || rig.phones.connected() == 0);

        // The host still serves that phone.
        let mut session = resume(&rig, &device, &hello(0, 0)).expect("let back in");
        assert!(matches!(
            next(&mut session).payload,
            Some(Payload::Welcome(_))
        ));
    }

    #[test]
    fn a_revoked_phone_is_cut_off_and_cannot_reconnect() {
        if rerun_sandboxed("a_revoked_phone_is_cut_off_and_cannot_reconnect") {
            return;
        }
        let rig = rig();
        let device = Keypair::generate().unwrap();
        let (mut session, _) = paired(&rig, &device);

        let id = rig.phones.devices()[0].id.clone();
        assert!(rig.phones.revoke(&id).unwrap());
        assert!(
            session.recv().is_err(),
            "the revoked phone's session stayed open"
        );
        assert!(rig.phones.devices().is_empty());
        assert!(matches!(
            resume(&rig, &device, &hello(0, 0)),
            Err(ket_remote::Error::Refused)
        ));
    }

    #[test]
    fn a_response_fits_only_what_was_asked() {
        use crate::agent_hooks::{Asked, Offered, PermissionQuestion};
        let option = |label: &str| Offered {
            label: label.to_owned(),
            description: String::new(),
        };
        let asked = PermissionQuestion {
            tool: "AskUserQuestion".to_owned(),
            subject: None,
            id: 1,
            prompt: Some(Prompt::Ask {
                questions: vec![
                    Asked {
                        question: "Colour?".to_owned(),
                        header: String::new(),
                        options: vec![option("Red"), option("Blue")],
                        multi_select: false,
                    },
                    Asked {
                        question: "Toppings?".to_owned(),
                        header: String::new(),
                        options: vec![option("Cheese"), option("Ham")],
                        multi_select: true,
                    },
                ],
            }),
        };
        let pick = |choices: &[u32], other: &str| proto::respond::Pick {
            choices: choices.to_vec(),
            other: other.to_owned(),
        };
        let respond = |picks: Vec<proto::respond::Pick>| proto::Respond {
            picks,
            ..Default::default()
        };

        assert_eq!(
            decision(&asked, &respond(vec![pick(&[1], ""), pick(&[1, 0, 1], "")])),
            Some(Decision::Answers(vec![
                "Blue".to_owned(),
                "Cheese, Ham".to_owned()
            ]))
        );
        // Written instead of chosen: the words are the answer.
        assert_eq!(
            decision(&asked, &respond(vec![pick(&[], " Green "), pick(&[0], "")])),
            Some(Decision::Answers(vec![
                "Green".to_owned(),
                "Cheese".to_owned()
            ]))
        );
        for misfit in [
            vec![pick(&[0], "")],
            vec![pick(&[0, 1], ""), pick(&[0], "")],
            vec![pick(&[], ""), pick(&[0], "")],
            vec![pick(&[2], ""), pick(&[0], "")],
        ] {
            assert_eq!(decision(&asked, &respond(misfit)), None);
        }

        let plan = PermissionQuestion {
            tool: "ExitPlanMode".to_owned(),
            prompt: Some(Prompt::Plan),
            ..asked.clone()
        };
        let verdict = |approve: bool, feedback: &str| proto::Respond {
            approve,
            feedback: feedback.to_owned(),
            ..Default::default()
        };
        assert_eq!(decision(&plan, &verdict(true, "")), Some(Decision::Approve));
        assert_eq!(
            decision(&plan, &verdict(false, "Add tests")),
            Some(Decision::KeepPlanning("Add tests".to_owned()))
        );
        assert_eq!(decision(&plan, &verdict(false, " ")), None);

        let permission = PermissionQuestion {
            tool: "Bash".to_owned(),
            prompt: None,
            ..asked
        };
        assert_eq!(decision(&permission, &verdict(true, "")), None);
    }
}
