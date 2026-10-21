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
//!
//! Here: the phone side as a whole — turning it on and off, pairing, the
//! devices it knows. The link to the relay is `relay`; one phone's session,
//! `session`; what a phone asks for, `requests`; the picture it is sent,
//! `snapshot`, built from what `places` reads of ket's store; and the keys
//! that answer an agent's prompt, `keys`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ket_remote::proto;
use prost::Message;

use super::host_error;
use super::server::Host;
use super::wire::DeviceRole;
use crate::Result;
use crate::activity::Tracker;

mod keys;
mod places;
mod relay;
mod requests;
mod session;
mod snapshot;
#[cfg(test)]
mod tests;
mod typing;

use places::{BacklogCounts, FigureMap, Places};
use relay::link_forever;
use snapshot::snapshot;
use typing::Typing;

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

/// The most files one message from a phone carries.
const MAX_ATTACHMENTS: usize = 10;

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

    /// Addresses a paired phone may try. The pairing URL stays separate.
    fn addresses(&self) -> Vec<String> {
        match self {
            Self::Local { port } => {
                let mut addresses = vec![format!("ws://{}:{port}", lan_host())];
                if let Some(ip) = tailnet_ip() {
                    addresses.push(format!("ws://{ip}:{port}"));
                }
                addresses
            }
            Self::External(url) => vec![url.clone()],
        }
    }
}

/// Picks an IPv4 address in the Tailscale address range from interface IPs.
fn tailnet_ip_from(
    addresses: impl IntoIterator<Item = std::net::Ipv4Addr>,
) -> Option<std::net::Ipv4Addr> {
    addresses.into_iter().find(|ip| {
        let octets = ip.octets();
        octets[0] == 100 && (64..=127).contains(&octets[1])
    })
}

/// This machine's current Tailscale IPv4 interface address, if present.
pub(super) fn tailnet_ip() -> Option<std::net::Ipv4Addr> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let interfaces = nix::ifaddrs::getifaddrs().ok()?;
        tailnet_ip_from(interfaces.filter_map(|interface| {
            interface
                .address?
                .as_sockaddr_in()
                .map(|address| address.ip())
        }))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
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
    /// What phones have asked to be typed, waiting its turn in each terminal.
    typing: Typing,
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
        typing: Typing::default(),
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
