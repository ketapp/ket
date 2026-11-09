//! The relay (Epic 5b.3): where a phone and a ket host meet.
//!
//! Both dial it over WebSockets. A host registers the id from its pairing
//! codes; a phone asks for that id; the relay gives the pair a connection id
//! and forwards frames between them — Noise ciphertext, which it cannot read,
//! in frames it can only route. It keeps no record of what it forwarded and
//! logs only counts and connection events.
//!
//! It runs inside `ket-host`, which calls [`serve`] for its own id when
//! phones are turned on, or as the `ket-relay` binary, for any host that
//! proves it owns the id it asks for. There are no accounts and no admission
//! tickets. A host's id comes from its static key, and it shows it holds
//! that key by answering a challenge; a phone pins its host's key, so a
//! relay routing for the wrong host gets nobody anything but a failed
//! handshake.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
pub use ket_relay_protocol::Control as RelayControl;
use ket_relay_protocol::{
    Control, Frame, Kind, MAX_FRAME, REFUSED_BACKPRESSURE, REFUSED_HOST_OFFLINE, REFUSED_IDENTITY,
    REFUSED_PROTOCOL, proof_message,
};
use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::rand::{SecureRandom, SystemRandom};
use ring::{digest, hmac};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// Most bytes queued for one socket before it is cut off. A phone on a poor
/// connection that cannot keep up is disconnected and reconnects — it
/// resumes from its cursor, or takes a fresh checkpoint — rather than the
/// relay buffering without bound on its behalf.
const QUEUE_LIMIT: usize = 8 * 1024 * 1024;

/// How long a relay that has been told to stop waits for its sockets to see
/// their last frames out, before it returns anyway.
const DRAIN: Duration = Duration::from_secs(5);

/// How much one relay takes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Sockets open at once, from everyone. A phone and a host each need
    /// one; past this, new sockets are closed on accept.
    pub sockets: usize,
    /// Sockets open at once from any one address.
    pub sockets_per_address: usize,
    /// Phones connected to any one host at once; the next is refused with
    /// [`REFUSED_BACKPRESSURE`].
    pub phones_per_host: usize,
    /// Bytes queued for every socket together. A socket a frame would take
    /// past this is cut off as one that fell behind on its own is.
    pub queued: usize,
    /// Whether sockets from this machine come through a proxy — Caddy, in
    /// front of a relay for many hosts — whose `X-Forwarded-For` says
    /// whose they really are, for the per-address limits. Taken only from
    /// loopback: from anywhere else, the header is anyone's to write.
    pub forwarded: bool,
}

impl Limits {
    /// A host's own relay, on its Mac, for its own phones.
    pub const OWN: Self = Self {
        sockets: 256,
        sockets_per_address: 32,
        phones_per_host: 32,
        queued: 256 * 1024 * 1024,
        forwarded: false,
    };

    /// ket's relay, for many Macs, behind a proxy on the same machine.
    pub const SHARED: Self = Self {
        sockets: 16_384,
        sockets_per_address: 32,
        phones_per_host: 16,
        queued: 512 * 1024 * 1024,
        forwarded: true,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::OWN
    }
}

/// How long a socket has to finish its WebSocket upgrade and say what it is.
/// Anything slower is someone holding a socket open, not a phone.
const OPENING_TIMEOUT: Duration = Duration::from_secs(10);

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

/// A frame queued for a socket. It holds its bytes against the socket's
/// limit and everyone's from the moment it is queued, and gives them back
/// when it is dropped — sent, or thrown away with a socket that has gone —
/// so the counts can neither leak nor run below zero.
struct Queued {
    bytes: Vec<u8>,
    len: usize,
    queued: Arc<AtomicUsize>,
    everyone: Arc<AtomicUsize>,
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.queued.fetch_sub(self.len, Ordering::AcqRel);
        self.everyone.fetch_sub(self.len, Ordering::AcqRel);
    }
}

/// One connected socket, as others send to it.
#[derive(Clone)]
struct Peer {
    id: u64,
    tx: mpsc::UnboundedSender<Queued>,
    queued: Arc<AtomicUsize>,
    /// Bytes queued for every socket on the relay, against
    /// [`Limits::queued`].
    everyone: Arc<AtomicUsize>,
    everyone_limit: usize,
    /// Set once it has fallen too far behind: nothing more is queued for it,
    /// and its writer closes the socket once what is queued has gone.
    cut: Arc<AtomicBool>,
}

impl Peer {
    /// `bytes`, counted against this socket and everyone, and the counts
    /// with them.
    fn hold(&self, bytes: Vec<u8>) -> (Queued, usize, usize) {
        let len = bytes.len();
        let queued = self.queued.fetch_add(len, Ordering::AcqRel) + len;
        let everyone = self.everyone.fetch_add(len, Ordering::AcqRel) + len;
        let held = Queued {
            bytes,
            len,
            queued: self.queued.clone(),
            everyone: self.everyone.clone(),
        };
        (held, queued, everyone)
    }

    /// Queues a frame. `false` when the socket has gone or fallen too far
    /// behind, in which case it has been told and cut off.
    fn send(&self, frame: &Frame) -> bool {
        if self.cut.load(Ordering::Acquire) {
            return false;
        }
        let Ok(bytes) = frame.encode() else {
            return false;
        };
        let (held, queued, everyone) = self.hold(bytes);
        if queued > QUEUE_LIMIT || everyone > self.everyone_limit {
            // Not queued, so given back.
            drop(held);
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
                    let _ = self.tx.send(self.hold(refusal).0);
                }
            }
            return false;
        }
        // A socket that has gone hands the frame back, and it is given back.
        self.tx.send(held).is_ok()
    }

    /// Queues `frame` as the socket's last: its writer sends what is queued,
    /// this, and then closes the socket.
    fn last(&self, frame: &Frame) {
        self.cut.store(true, Ordering::Release);
        if let Ok(bytes) = frame.encode() {
            let _ = self.tx.send(self.hold(bytes).0);
        }
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

/// Counts a socket against the limits while it is open: against the total
/// from accept, and against its address once that is known — at accept, or
/// from the proxy's header when it came through one.
struct Open {
    relay: Arc<Relay>,
    address: Option<IpAddr>,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.relay.sockets.fetch_sub(1, Ordering::AcqRel);
        let Some(address) = self.address else {
            return;
        };
        let mut per = lock(&self.relay.per_address);
        if let Some(count) = per.get_mut(&address) {
            *count -= 1;
            if *count == 0 {
                per.remove(&address);
            }
        }
    }
}

#[derive(Default)]
struct Relay {
    /// The one host this relay serves, and the key it must claim it with,
    /// when it is a host's own — see [`serve`]. `None` routes for any host
    /// that proves its id.
    only: Option<Own>,
    limits: Limits,
    /// Set once the relay has been told to stop: nothing new is let in.
    stopping: AtomicBool,
    /// Says the same to every socket's reader, so each lets its writer see
    /// its last frame out and close it rather than waiting on the far end.
    stop: tokio::sync::watch::Sender<bool>,
    /// Bytes queued for every socket together — see [`Limits::queued`].
    queued: Arc<AtomicUsize>,
    hosts: Mutex<HashMap<[u8; 16], Peer>>,
    connections: Mutex<HashMap<u64, Connection>>,
    next_peer: AtomicU64,
    next_connection: AtomicU64,
    /// Open sockets, and per address — see [`Limits::sockets`].
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

    /// Counts a refusal, and says so now and then: counts and reasons only,
    /// never whose it was.
    fn refused(counter: &AtomicU64, reason: &'static str) {
        let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_power_of_two() {
            tracing::warn!(count, reason, "relay refused connection");
        }
    }

    /// Lets a socket from `address` count against that address: its rate of
    /// new sockets, and how many it has open. `false`, counted and said,
    /// when either is past its limit.
    fn admit(&self, address: IpAddr) -> bool {
        if !self.allow_attempt(address) {
            Relay::refused(&self.refused_rate, "address rate limit");
            return false;
        }
        let mut per = lock(&self.per_address);
        let count = per.entry(address).or_default();
        if *count >= self.limits.sockets_per_address {
            drop(per);
            Relay::refused(&self.refused_address, "address socket limit");
            return false;
        }
        *count += 1;
        true
    }

    /// Ends every connection, and every socket that has said what it is,
    /// each with a last frame that says so.
    fn close_everything(&self) {
        let ended: Vec<(u64, Connection)> = lock(&self.connections).drain().collect();
        for (connection, ended) in ended {
            ended.phone.last(&Control::Closed.frame(connection));
            if let Some(host) = lock(&self.hosts)
                .get(&ended.host)
                .filter(|host| host.id == ended.host_peer)
            {
                host.send(&Control::Closed.frame(connection));
            }
        }
        let hosts: Vec<Peer> = lock(&self.hosts).values().cloned().collect();
        for host in hosts {
            host.last(&Control::Closed.frame(0));
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
/// With `only` unset, any host can register, by
/// [`ket_relay_protocol::Control::Identify`] and the proof the relay then
/// asks for — see [`proven`].
///
/// `only` is set when the relay runs inside a host, on the Mac: it then
/// routes for that host alone, and only the host itself can register it — a
/// [`ket_relay_protocol::Control::Claim`] with its key, from this Mac, and
/// not while it is already registered — so nothing else, on the network or
/// on the Mac, can take its place or be connected through it to another.
pub async fn serve(listener: TcpListener, only: Option<Own>) {
    serve_until(
        listener,
        only,
        Limits::OWN,
        None,
        std::future::pending::<()>(),
    )
    .await;
}

/// [`serve`], with `limits`, a health check on `health` when one is given,
/// and until `stop` resolves: then nothing new is accepted, every phone and
/// host is sent a last frame that says its connection has ended, and this
/// returns once their sockets have closed, or after a few seconds whatever
/// is left.
///
/// The health check answers any request with `200` and the number of hosts
/// and phones connected. Nothing else: no ids, no addresses.
pub async fn serve_until(
    listener: TcpListener,
    only: Option<Own>,
    limits: Limits,
    health: Option<TcpListener>,
    stop: impl std::future::Future<Output = ()>,
) {
    let relay = Arc::new(Relay {
        only,
        limits,
        ..Relay::default()
    });
    if let Some(health) = health {
        tokio::spawn(answer_health(health, relay.clone()));
    }
    tokio::select! {
        () = accept(&relay, &listener) => {}
        () = stop => {}
    }
    drop(listener);
    relay.stopping.store(true, Ordering::Release);
    relay.close_everything();
    // `send_replace`, so a socket still finishing its upgrade sees it too.
    relay.stop.send_replace(true);
    let deadline = Instant::now() + DRAIN;
    while relay.sockets.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tracing::info!(
        left = relay.sockets.load(Ordering::Acquire),
        "relay stopped"
    );
}

async fn accept(relay: &Arc<Relay>, listener: &TcpListener) {
    loop {
        let Ok((stream, address)) = listener.accept().await else {
            continue;
        };
        let address = address.ip();
        // Through the proxy, the address is the proxy's, and the socket is
        // counted against the one its header names once it has been read.
        let proxied = relay.limits.forwarded && address.is_loopback();
        if !proxied && !relay.allow_attempt(address) {
            Relay::refused(&relay.refused_rate, "address rate limit");
            continue;
        }
        // Counted before anything is spent on it; over a limit, dropped.
        if relay.sockets.fetch_add(1, Ordering::AcqRel) >= relay.limits.sockets {
            relay.sockets.fetch_sub(1, Ordering::AcqRel);
            Relay::refused(&relay.refused_global, "global socket limit");
            continue;
        }
        let mut open = Open {
            relay: relay.clone(),
            address: None,
        };
        if !proxied {
            let mut per = lock(&relay.per_address);
            let count = per.entry(address).or_default();
            if *count >= relay.limits.sockets_per_address {
                drop(per);
                drop(open);
                Relay::refused(&relay.refused_address, "address socket limit");
                continue;
            }
            *count += 1;
            open.address = Some(address);
        }
        // Every frame here is small and someone is waiting on it — a
        // keystroke, its echo. Nagle's algorithm would hold each one back for
        // the last one's acknowledgement, which on macOS adds up to 200 ms a
        // hop.
        let _ = stream.set_nodelay(true);
        let relay = relay.clone();
        tokio::spawn(async move {
            if let Err(error) = socket(&relay, stream, address, &mut open).await {
                tracing::debug!(%error, "relay socket ended");
            }
            drop(open);
        });
    }
}

/// Answers each connection on `listener` with the relay's health, and
/// closes it.
async fn answer_health(listener: TcpListener, relay: Arc<Relay>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let relay = relay.clone();
        tokio::spawn(async move {
            // The request is read only so the answer does not race it.
            let mut request = [0u8; 1024];
            let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut request)).await;
            let hosts = lock(&relay.hosts).len();
            let phones = lock(&relay.connections).len();
            let body = format!("ok hosts={hosts} phones={phones}\n");
            let answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

/// The address a proxy's `X-Forwarded-For` names: its last entry, the one
/// the proxy itself wrote.
fn forwarded_for(
    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
) -> Option<IpAddr> {
    request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .next_back()
        .and_then(|entry| entry.trim().parse().ok())
}

async fn socket(
    relay: &Relay,
    stream: TcpStream,
    address: IpAddr,
    open: &mut Open,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    // Through the proxy, the socket counts against the address its header
    // names, checked before the upgrade is answered.
    let proxied = open.address.is_none();
    let mut admitted = None;
    // The shape tungstenite's handshake callback takes, large error and all.
    #[allow(clippy::result_large_err)]
    let check = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        if !proxied {
            return Ok(response);
        }
        let from = forwarded_for(request).unwrap_or(address);
        if !relay.admit(from) {
            let mut refusal = ErrorResponse::new(None);
            *refusal.status_mut() =
                tokio_tungstenite::tungstenite::http::StatusCode::TOO_MANY_REQUESTS;
            return Err(refusal);
        }
        admitted = Some(from);
        Ok(response)
    };
    let upgraded = tokio::time::timeout(
        OPENING_TIMEOUT,
        tokio_tungstenite::accept_hdr_async_with_config(stream, check, Some(config)),
    )
    .await;
    if admitted.is_some() {
        open.address = admitted;
    }
    let socket = upgraded??;
    if relay.stopping.load(Ordering::Acquire) {
        return Ok(());
    }
    let (mut sink, mut incoming) = socket.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<Queued>();
    let cut = Arc::new(AtomicBool::new(false));
    let peer = Peer {
        id: relay.next_peer.fetch_add(1, Ordering::Relaxed),
        tx,
        queued: Arc::new(AtomicUsize::new(0)),
        everyone: relay.queued.clone(),
        everyone_limit: relay.limits.queued,
        cut: cut.clone(),
    };
    // Whatever is still queued when this ends is dropped with `rx`, and
    // gives its bytes back.
    let writer = tokio::spawn(async move {
        while let Some(mut frame) = rx.recv().await {
            let bytes = std::mem::take(&mut frame.bytes);
            drop(frame);
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
            // Behind a proxy every socket is from loopback, so none is.
            && address.is_loopback()
            && !relay.limits.forwarded
    };
    let mut stopping = relay.stop.subscribe();
    let serving = async {
        match control {
            // A relay for any host that shows it holds the key its id comes
            // from.
            Some(Control::Identify { host, public }) if relay.only.is_none() => {
                if proven(&peer, host, &public, &mut incoming).await {
                    serve_host(relay, &peer, host, &mut incoming).await;
                }
            }
            Some(Control::Claim { host, key }) if may_claim(&host, &key) => {
                serve_host(relay, &peer, host, &mut incoming).await;
            }
            Some(Control::Connect { host }) if admitted(&host) => {
                serve_phone(relay, &peer, host, &mut incoming).await;
            }
            other => refuse_opening(&peer, other.as_ref()),
        }
    };
    tokio::select! {
        () = serving => {}
        _ = stopping.wait_for(|stopped| *stopped) => {}
    }
    drop(peer);
    let _ = writer.await;
    Ok(())
}

/// Answers a socket that opened with something the relay will not take.
fn refuse_opening(peer: &Peer, control: Option<&Control>) {
    match control {
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

/// A host's id from its static public key: the first sixteen bytes of the
/// key's SHA-256, as `ket_remote::host_id` makes it.
fn host_id(public: &[u8; 32]) -> [u8; 16] {
    let digest = digest::digest(&digest::SHA256, public);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_ref()[..16]);
    id
}

/// Whether a host that says it is `host`, with `public` the key its id comes
/// from, can show it holds that key: a fresh nonce and a one-time X25519
/// key go out, and the answer has to be the HMAC-SHA256 of the proof
/// message keyed by the agreement only the key's holder can reach. Told why
/// when it cannot.
async fn proven(peer: &Peer, host: [u8; 16], public: &[u8; 32], incoming: &mut Incoming) -> bool {
    let refuse = |reason| {
        peer.send(&Control::Refused { reason }.frame(0));
        false
    };
    if host_id(public) != host {
        return refuse(REFUSED_IDENTITY);
    }
    let random = SystemRandom::new();
    let mut nonce = [0u8; 32];
    let Ok(ephemeral) = EphemeralPrivateKey::generate(&X25519, &random) else {
        return refuse(REFUSED_PROTOCOL);
    };
    let (Ok(()), Ok(ephemeral_public)) = (random.fill(&mut nonce), ephemeral.compute_public_key())
    else {
        return refuse(REFUSED_PROTOCOL);
    };
    let Ok(ephemeral_public) = <[u8; 32]>::try_from(ephemeral_public.as_ref()) else {
        return refuse(REFUSED_PROTOCOL);
    };
    peer.send(
        &Control::Challenge {
            nonce,
            ephemeral: ephemeral_public,
        }
        .frame(0),
    );

    let answer = tokio::time::timeout(OPENING_TIMEOUT, next_frame(incoming))
        .await
        .ok()
        .flatten()
        .filter(|frame| frame.kind == Kind::Control)
        .and_then(|frame| Control::parse(&frame.body).ok());
    let Some(Control::Prove { tag }) = answer else {
        return refuse(REFUSED_PROTOCOL);
    };
    let message = proof_message(&nonce, &host);
    // ring refuses an agreement that comes out all zeros, which a low-order
    // `public` would give anyone.
    let checks = agreement::agree_ephemeral(
        ephemeral,
        &UnparsedPublicKey::new(&X25519, public),
        |shared| hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, shared), &message, &tag).is_ok(),
    );
    if checks != Ok(true) {
        return refuse(REFUSED_IDENTITY);
    }
    true
}

async fn serve_host(relay: &Relay, peer: &Peer, host: [u8; 16], incoming: &mut Incoming) {
    {
        let mut hosts = lock(&relay.hosts);
        // A host's own relay keeps the registration it has while it lives;
        // see `socket`. Elsewhere a reconnecting host, which has just proved
        // it holds the id's key, replaces its last one.
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
    peer.send(&Control::Registered.frame(0));
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
    {
        let mut connections = lock(&relay.connections);
        // One host's phones, however many dial it, are a bounded share.
        let already = connections.values().filter(|c| c.host == host).count();
        if already >= relay.limits.phones_per_host {
            drop(connections);
            peer.send(
                &Control::Refused {
                    reason: REFUSED_BACKPRESSURE,
                }
                .frame(0),
            );
            return;
        }
        connections.insert(
            connection,
            Connection {
                phone: peer.clone(),
                host,
                host_peer: host_peer.id,
            },
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(everyone_limit: usize) -> (Peer, mpsc::UnboundedReceiver<Queued>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let peer = Peer {
            id: 1,
            tx,
            queued: Arc::new(AtomicUsize::new(0)),
            everyone: Arc::new(AtomicUsize::new(0)),
            everyone_limit,
            cut: Arc::new(AtomicBool::new(false)),
        };
        (peer, rx)
    }

    fn counts(peer: &Peer) -> (usize, usize) {
        (
            peer.queued.load(Ordering::Acquire),
            peer.everyone.load(Ordering::Acquire),
        )
    }

    #[test]
    fn the_refusal_that_cuts_a_socket_off_is_counted_like_any_frame() {
        let (peer, mut rx) = peer(100);
        assert!(!peer.send(&Frame::data(1, vec![0; 200])));
        // Only the refusal is queued, and it is counted.
        let refusal = rx.try_recv().unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(counts(&peer), (refusal.len, refusal.len));
        drop(refusal);
        assert_eq!(counts(&peer), (0, 0));
    }

    #[test]
    fn frames_thrown_away_with_a_socket_give_their_bytes_back() {
        let (peer, rx) = peer(usize::MAX);
        for _ in 0..3 {
            assert!(peer.send(&Frame::data(1, vec![0; 10])));
        }
        assert_ne!(counts(&peer), (0, 0));
        drop(rx);
        assert_eq!(counts(&peer), (0, 0));
        // Sent to after it has gone: not queued, and not counted.
        assert!(!peer.send(&Frame::data(1, vec![0; 10])));
        peer.last(&Control::Closed.frame(1));
        assert_eq!(counts(&peer), (0, 0));
    }
}
