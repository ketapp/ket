//! The relay (Epic 5b.3): where a phone and a ket host meet.
//!
//! Both dial it over WebSockets. A host registers the id from its pairing
//! codes; a phone asks for that id; the relay gives the pair a connection id
//! and forwards frames between them — Noise ciphertext, which it cannot read,
//! in frames it can only route. It keeps no record of what it forwarded and
//! logs only counts and connection events.
//!
//! Phones reach their Mac over the local network only (decided 2026-09-28),
//! so this runs on the Mac: inside `ket-host`, which calls [`serve`] for its
//! own id when phones are turned on, or as the `ket-relay` binary for
//! development. There are no accounts and no admission tickets — the pairing
//! keys are the authentication, and a phone pins its host's key, so a relay
//! routing for the wrong host gets nobody anything but a failed handshake.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
pub use ket_relay_protocol::Control as RelayControl;
use ket_relay_protocol::{
    Control, Frame, Kind, MAX_FRAME, REFUSED_BACKPRESSURE, REFUSED_HOST_OFFLINE, REFUSED_PROTOCOL,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// Most bytes queued for one socket before it is cut off. A phone on a poor
/// connection that cannot keep up is disconnected and reconnects — it
/// resumes from its cursor, or takes a fresh checkpoint — rather than the
/// relay buffering without bound on its behalf.
const QUEUE_LIMIT: usize = 8 * 1024 * 1024;

/// How long a socket has to finish its WebSocket upgrade and say what it is.
/// Anything slower is someone holding a socket open, not a phone.
const OPENING_TIMEOUT: Duration = Duration::from_secs(10);

/// Sockets open at once, from everyone, and from any one address. A phone
/// and a host each need one; past these, new sockets are closed on accept.
const MAX_SOCKETS: usize = 256;
const MAX_SOCKETS_PER_ADDRESS: usize = 32;

/// New TCP connections allowed per address: a short reconnect burst, then
/// two per second. The table is bounded so distinct source addresses cannot
/// turn the limiter itself into a memory sink.
const ATTEMPT_BURST: u32 = 16;
const ATTEMPT_REFILL: Duration = Duration::from_millis(500);
const MAX_RATE_ADDRESSES: usize = 4096;
const RATE_IDLE: Duration = Duration::from_secs(10 * 60);

struct AttemptRate {
    tokens: u32,
    updated: Instant,
}

impl AttemptRate {
    fn new() -> Self {
        Self {
            tokens: ATTEMPT_BURST,
            updated: Instant::now(),
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        let intervals = now.duration_since(self.updated).as_millis() / ATTEMPT_REFILL.as_millis();
        let replenished = u32::try_from(intervals).unwrap_or(u32::MAX);
        self.tokens = self.tokens.saturating_add(replenished).min(ATTEMPT_BURST);
        if replenished > 0 {
            self.updated = now;
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One connected socket, as others send to it.
#[derive(Clone)]
struct Peer {
    id: u64,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    /// Set once it has fallen too far behind: nothing more is queued for it,
    /// and its writer closes the socket once what is queued has gone.
    cut: Arc<AtomicBool>,
}

impl Peer {
    /// Queues a frame. `false` when the socket has gone or fallen too far
    /// behind, in which case it has been told and cut off.
    fn send(&self, frame: &Frame) -> bool {
        if self.cut.load(Ordering::Acquire) {
            return false;
        }
        let Ok(bytes) = frame.encode() else {
            return false;
        };
        let queued = self.queued.fetch_add(bytes.len(), Ordering::AcqRel) + bytes.len();
        if queued > QUEUE_LIMIT {
            // Not queued, so not counted.
            self.queued.fetch_sub(bytes.len(), Ordering::AcqRel);
            if !self.cut.swap(true, Ordering::AcqRel) {
                tracing::info!(peer = self.id, "cut off: too far behind");
                // One more small frame to say why; the writer then ends the
                // socket rather than leaving it open and silent.
                if let Ok(refusal) = (Control::Refused {
                    reason: REFUSED_BACKPRESSURE,
                })
                .frame(0)
                .encode()
                {
                    let _ = self.tx.send(refusal);
                }
            }
            return false;
        }
        self.tx.send(bytes).is_ok()
    }
}

/// A phone and the host it is connected to.
struct Connection {
    phone: Peer,
    host: [u8; 16],
    /// Which registration of the host: a host that reconnects is a new peer,
    /// and the old one's connections are over.
    host_peer: u64,
}

/// Counts a socket against the limits while it is open.
struct Open {
    relay: Arc<Relay>,
    address: IpAddr,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.relay.sockets.fetch_sub(1, Ordering::AcqRel);
        let mut per = lock(&self.relay.per_address);
        if let Some(count) = per.get_mut(&self.address) {
            *count -= 1;
            if *count == 0 {
                per.remove(&self.address);
            }
        }
    }
}

#[derive(Default)]
struct Relay {
    /// The one host this relay serves, and the key it must claim it with,
    /// when it is a host's own — see [`serve`]. `None` routes for any host
    /// that registers.
    only: Option<Own>,
    hosts: Mutex<HashMap<[u8; 16], Peer>>,
    connections: Mutex<HashMap<u64, Connection>>,
    next_peer: AtomicU64,
    next_connection: AtomicU64,
    /// Open sockets, and per address — see [`MAX_SOCKETS`].
    sockets: AtomicUsize,
    per_address: Mutex<HashMap<IpAddr, usize>>,
    attempts: Mutex<HashMap<IpAddr, AttemptRate>>,
    refused_rate: AtomicU64,
    refused_global: AtomicU64,
    refused_address: AtomicU64,
}

impl Relay {
    fn allow_attempt(&self, address: IpAddr) -> bool {
        let mut attempts = lock(&self.attempts);
        if attempts.len() >= MAX_RATE_ADDRESSES && !attempts.contains_key(&address) {
            attempts.retain(|_, rate| rate.updated.elapsed() < RATE_IDLE);
            if attempts.len() >= MAX_RATE_ADDRESSES {
                return false;
            }
        }
        attempts
            .entry(address)
            .or_insert_with(AttemptRate::new)
            .allow()
    }

    fn refused(counter: &AtomicU64, address: IpAddr, reason: &'static str) {
        let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_power_of_two() {
            tracing::warn!(%address, count, reason, "relay refused connection");
        }
    }
}

/// The host a relay belongs to, when it runs inside one.
#[derive(Clone, Copy)]
pub struct Own {
    /// The host's id.
    pub host: [u8; 16],
    /// A secret the host made when it started the relay and claims it with —
    /// see [`ket_relay_protocol::Control::Claim`].
    pub key: [u8; 32],
}

/// Compares two keys in time that does not depend on where they differ.
fn same_key(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Accepts sockets on `listener` and routes between hosts and phones until
/// the future is dropped.
///
/// `only` is set when the relay runs inside a host, on the Mac: it then
/// routes for that host alone, and only the host itself can register it — a
/// [`ket_relay_protocol::Control::Claim`] with its key, from this Mac, and
/// not while it is already registered — so nothing else, on the network or
/// on the Mac, can take its place or be connected through it to another.
pub async fn serve(listener: TcpListener, only: Option<Own>) {
    let relay = Arc::new(Relay {
        only,
        ..Relay::default()
    });
    loop {
        let Ok((stream, address)) = listener.accept().await else {
            continue;
        };
        let address = address.ip();
        if !relay.allow_attempt(address) {
            Relay::refused(&relay.refused_rate, address, "address rate limit");
            continue;
        }
        // Counted before anything is spent on it; over a limit, dropped.
        if relay.sockets.fetch_add(1, Ordering::AcqRel) >= MAX_SOCKETS {
            relay.sockets.fetch_sub(1, Ordering::AcqRel);
            Relay::refused(&relay.refused_global, address, "global socket limit");
            continue;
        }
        {
            let mut per = lock(&relay.per_address);
            let count = per.entry(address).or_default();
            if *count >= MAX_SOCKETS_PER_ADDRESS {
                drop(per);
                relay.sockets.fetch_sub(1, Ordering::AcqRel);
                Relay::refused(&relay.refused_address, address, "address socket limit");
                continue;
            }
            *count += 1;
        }
        let open = Open {
            relay: relay.clone(),
            address,
        };
        // Every frame here is small and someone is waiting on it — a
        // keystroke, its echo. Nagle's algorithm would hold each one back for
        // the last one's acknowledgement, which on macOS adds up to 200 ms a
        // hop.
        let _ = stream.set_nodelay(true);
        let relay = relay.clone();
        tokio::spawn(async move {
            let _open = open;
            if let Err(error) = socket(&relay, stream, address).await {
                tracing::debug!(%address, %error, "relay socket ended");
            }
        });
    }
}

async fn socket(
    relay: &Relay,
    stream: TcpStream,
    address: IpAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let socket = tokio::time::timeout(
        OPENING_TIMEOUT,
        tokio_tungstenite::accept_async_with_config(stream, Some(config)),
    )
    .await??;
    let (mut sink, mut incoming) = socket.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let queued = Arc::new(AtomicUsize::new(0));
    let cut = Arc::new(AtomicBool::new(false));
    let peer = Peer {
        id: relay.next_peer.fetch_add(1, Ordering::Relaxed),
        tx,
        queued: queued.clone(),
        cut: cut.clone(),
    };
    let writer = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            queued.fetch_sub(bytes.len(), Ordering::AcqRel);
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
            // Cut off, and the refusal that says so sent: nothing more is
            // queued behind it, so the socket ends here.
            if cut.load(Ordering::Acquire) && rx.is_empty() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // The first frame says what this socket is, and has to come promptly.
    let first = tokio::time::timeout(OPENING_TIMEOUT, next_frame(&mut incoming))
        .await
        .ok()
        .flatten();
    let control = first
        .as_ref()
        .filter(|frame| frame.kind == Kind::Control)
        .and_then(|frame| Control::parse(&frame.body).ok());
    let admitted = |host: &[u8; 16]| relay.only.is_none_or(|own| own.host == *host);
    // A host's own relay takes its registration from the host alone: from
    // this Mac, and — checked as it is taken, in `serve_host` — only while no
    // registration is live. A second one would otherwise replace the host
    // and take every phone connection with it.
    let may_claim = |host: &[u8; 16], key: &[u8; 32]| {
        relay
            .only
            .is_some_and(|own| own.host == *host && same_key(&own.key, key))
            && address.is_loopback()
    };
    match control {
        // A development relay, for any host.
        Some(Control::Register { host }) if relay.only.is_none() => {
            serve_host(relay, &peer, host, &mut incoming).await;
        }
        Some(Control::Claim { host, key }) if may_claim(&host, &key) => {
            serve_host(relay, &peer, host, &mut incoming).await;
        }
        Some(Control::Connect { host }) if admitted(&host) => {
            serve_phone(relay, &peer, host, &mut incoming).await;
        }
        // A host this relay is not for is, to a phone, one that is not here.
        Some(Control::Connect { .. }) => {
            peer.send(
                &Control::Refused {
                    reason: REFUSED_HOST_OFFLINE,
                }
                .frame(0),
            );
        }
        _ => {
            peer.send(
                &Control::Refused {
                    reason: REFUSED_PROTOCOL,
                }
                .frame(0),
            );
        }
    }
    drop(peer);
    let _ = writer.await;
    Ok(())
}

type Incoming = futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<TcpStream>>;

/// The next frame, or `None` when the socket has ended or sent nonsense.
async fn next_frame(incoming: &mut Incoming) -> Option<Frame> {
    loop {
        match incoming.next().await? {
            Ok(Message::Binary(bytes)) => return Frame::decode(&bytes).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            // Pings are answered by the library; anything else is ignored.
            Ok(_) => {}
        }
    }
}

async fn serve_host(relay: &Relay, peer: &Peer, host: [u8; 16], incoming: &mut Incoming) {
    {
        let mut hosts = lock(&relay.hosts);
        // A host's own relay keeps the registration it has while it lives;
        // see `socket`. A development relay lets a reconnecting host replace
        // its last one.
        if relay.only.is_some() && hosts.contains_key(&host) {
            drop(hosts);
            peer.send(
                &Control::Refused {
                    reason: REFUSED_PROTOCOL,
                }
                .frame(0),
            );
            return;
        }
        if let Some(old) = hosts.insert(host, peer.clone()) {
            tracing::info!(old = old.id, new = peer.id, "host re-registered");
        }
    }
    tracing::info!(peer = peer.id, "host registered");

    while let Some(frame) = next_frame(incoming).await {
        match frame.kind {
            Kind::Data => {
                let phone = lock(&relay.connections)
                    .get(&frame.connection)
                    .filter(|c| c.host_peer == peer.id)
                    .map(|c| c.phone.clone());
                if let Some(phone) = phone
                    && !phone.send(&frame)
                {
                    close(relay, frame.connection);
                }
            }
            Kind::Control => {
                if Control::parse(&frame.body) == Ok(Control::Closed) {
                    let owned = lock(&relay.connections)
                        .get(&frame.connection)
                        .is_some_and(|c| c.host_peer == peer.id);
                    if owned {
                        close(relay, frame.connection);
                    }
                }
            }
        }
    }

    // Gone: its phones are disconnected, and its registration removed —
    // unless it has already been replaced by a newer one.
    {
        let mut hosts = lock(&relay.hosts);
        if hosts.get(&host).is_some_and(|p| p.id == peer.id) {
            hosts.remove(&host);
        }
    }
    let orphaned: Vec<u64> = lock(&relay.connections)
        .iter()
        .filter(|(_, c)| c.host_peer == peer.id)
        .map(|(&id, _)| id)
        .collect();
    for connection in orphaned {
        close(relay, connection);
    }
    tracing::info!(peer = peer.id, "host left");
}

async fn serve_phone(relay: &Relay, peer: &Peer, host: [u8; 16], incoming: &mut Incoming) {
    let Some(host_peer) = lock(&relay.hosts).get(&host).cloned() else {
        peer.send(
            &Control::Refused {
                reason: REFUSED_HOST_OFFLINE,
            }
            .frame(0),
        );
        return;
    };
    let connection = relay.next_connection.fetch_add(1, Ordering::Relaxed) + 1;
    lock(&relay.connections).insert(
        connection,
        Connection {
            phone: peer.clone(),
            host,
            host_peer: host_peer.id,
        },
    );
    host_peer.send(&Control::Opened.frame(connection));
    peer.send(&Control::Connected.frame(connection));
    tracing::info!(connection, "phone connected");

    while let Some(frame) = next_frame(incoming).await {
        if frame.kind != Kind::Data {
            continue;
        }
        // Routed by the connection the relay assigned, whatever the phone
        // wrote in the frame — and only ever to the host peer that accepted
        // it, never to whatever holds the registration now.
        if !lock(&relay.connections).contains_key(&connection) {
            break;
        }
        if !host_peer.send(&Frame::data(connection, frame.body)) {
            break;
        }
    }
    close(relay, connection);
    tracing::info!(connection, "phone left");
}

/// Ends a connection: both sides are told, and the phone is let go.
fn close(relay: &Relay, connection: u64) {
    let Some(ended) = lock(&relay.connections).remove(&connection) else {
        return;
    };
    ended.phone.send(&Control::Closed.frame(connection));
    if let Some(host) = lock(&relay.hosts)
        .get(&ended.host)
        .filter(|h| h.id == ended.host_peer)
    {
        host.send(&Control::Closed.frame(connection));
    }
}
