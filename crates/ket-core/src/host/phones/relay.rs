//! The host's link to the relay: one socket, a connection id per phone, and
//! the pipe each phone's thread reads its frames from.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use ket_relay_protocol::{Control, Frame, Kind};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream, tungstenite};

use super::session::serve_phone;
use super::{Event, HANDSHAKE_TIMEOUT, MAX_PENDING_HANDSHAKES, PREADMIT_BYTES, Phones, lock};
use crate::Result;
use crate::host::server::Host;

/// A relay the host links to: where it is, how the host takes its place
/// there, and how the link is doing.
pub(super) struct Target {
    pub(super) url: String,
    /// The key the host claims its own relay with, made fresh each time
    /// phones are turned on; `None` for a relay elsewhere, which is not told
    /// it and is proved to instead. See [`ket_relay::Own`].
    pub(super) claim: Option<[u8; 32]>,
    pub(super) status: Arc<LinkStatus>,
}

/// How a link is doing, for Settings to say.
#[derive(Default)]
pub(super) struct LinkStatus {
    /// Registered with the relay now.
    pub(super) linked: AtomicBool,
    /// Why the last attempt failed.
    pub(super) error: std::sync::Mutex<Option<String>>,
}

pub(super) async fn link_forever(host: &Arc<Host>, phones: &Arc<Phones>, target: &Target) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        let outcome = link(host, phones, target).await;
        target.status.linked.store(false, Ordering::Release);
        match outcome {
            Ok(()) => tracing::info!(relay = %target.url, "relay link closed"),
            Err(error) => {
                tracing::warn!(%error, relay = %target.url, "relay link failed");
                *lock(&target.status.error) = Some(error.to_string());
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

type Link = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// The host's socket to a relay.
pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Dials the relay at `url`: `ws://` to the host's own on this Mac, or
/// `wss://` to one elsewhere, whose certificate is checked against the
/// operating system's root certificates — or against `connector`'s, when a
/// test brings a certificate of its own. Nagle off, for the reason the relay
/// turns it off on its side: every frame is a keystroke or an echo someone
/// is waiting on.
pub(super) async fn dial(
    url: &str,
    connector: Option<Connector>,
) -> Result<Socket, tungstenite::Error> {
    let (socket, _) =
        tokio_tungstenite::connect_async_tls_with_config(url, None, true, connector).await?;
    Ok(socket)
}

/// How long a relay has to confirm the host's registration, from the first
/// frame the host sends it.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);

type Sink = futures::stream::SplitSink<Socket, WsMessage>;
type Incoming = futures::stream::SplitStream<Socket>;

/// Takes this host's place on the relay: our own is claimed with the key it
/// was started with; one elsewhere is told the host's id and key, and the
/// host proves it holds the key (#141). Done once the relay says so.
async fn register(
    phones: &Phones,
    claim: Option<[u8; 32]>,
    sink: &mut Sink,
    incoming: &mut Incoming,
) -> Link {
    async fn send(sink: &mut Sink, control: Control) -> Link {
        sink.send(WsMessage::Binary(control.frame(0).encode()?.into()))
            .await?;
        Ok(())
    }
    let (host, public) = {
        let identity = lock(&phones.identity);
        (identity.id, identity.public())
    };
    match claim {
        Some(key) => send(sink, Control::Claim { host, key }).await?,
        None => {
            send(sink, Control::Identify { host, public }).await?;
            let Control::Challenge { nonce, ephemeral } = next_control(incoming).await? else {
                return Err("the relay answered with something other than a challenge".into());
            };
            let tag = lock(&phones.identity).prove(&nonce, &ephemeral)?;
            send(sink, Control::Prove { tag }).await?;
        }
    }
    match next_control(incoming).await? {
        Control::Registered => Ok(()),
        Control::Refused { reason } => {
            Err(format!("the relay refused this host's registration (reason {reason})").into())
        }
        _ => Err("the relay answered with something other than a registration".into()),
    }
}

/// The next control message from the relay, skipping anything that is not a
/// frame.
async fn next_control(
    incoming: &mut Incoming,
) -> Result<Control, Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let bytes = match incoming.next().await {
            Some(message) => match message? {
                WsMessage::Binary(bytes) => bytes,
                WsMessage::Close(_) => return Err("the relay closed the link".into()),
                _ => continue,
            },
            None => return Err("the relay closed the link".into()),
        };
        let frame = Frame::decode(&bytes)?;
        if frame.kind == Kind::Control {
            return Ok(Control::parse(&frame.body)?);
        }
    }
}

async fn link(host: &Arc<Host>, phones: &Arc<Phones>, target: &Target) -> Link {
    let socket = dial(&target.url, None).await?;
    let (mut sink, mut incoming) = socket.split();
    tokio::time::timeout(
        REGISTRATION_TIMEOUT,
        register(phones, target.claim, &mut sink, &mut incoming),
    )
    .await??;
    target.status.linked.store(true, Ordering::Release);
    *lock(&target.status.error) = None;
    tracing::info!(relay = %target.url, "registered with the relay");

    let (out, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<Frame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            let Ok(bytes) = frame.encode() else { continue };
            if sink.send(WsMessage::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    let mut connections = Connections::default();
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
    writer.abort();
    Ok(())
}

/// The phones that came in over one link, by the relay's connection id.
/// However the link ends — the relay closing it, an error, or the link
/// being stopped mid-await — each is told its connection has gone, so no
/// session outlives the socket it came through.
#[derive(Default)]
struct Connections(HashMap<u64, Opened>);

impl std::ops::Deref for Connections {
    type Target = HashMap<u64, Opened>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Connections {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Connections {
    fn drop(&mut self) {
        for (_, link) in self.0.drain() {
            let _ = link.tx.send(Event::Closed);
        }
    }
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
pub(super) struct RelayPipe {
    pub(super) connection: u64,
    pub(super) out: tokio::sync::mpsc::UnboundedSender<Frame>,
    pub(super) events: mpsc::Receiver<Event>,
    /// For the pumps sending this phone output: they post to the same
    /// channel the phone's frames arrive on.
    pub(super) feed: mpsc::Sender<Event>,
    /// When the handshake must be over by: one deadline from when the relay
    /// opened the connection, not a fresh wait per frame.
    pub(super) deadline: Instant,
    /// Shared with the relay link — see [`Opened::admitted`].
    pub(super) admitted: Arc<AtomicBool>,
}

pub(super) fn broken() -> std::io::Error {
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
