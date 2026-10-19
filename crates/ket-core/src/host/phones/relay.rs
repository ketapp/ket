//! The host's link to the relay: one socket, a connection id per phone, and
//! the pipe each phone's thread reads its frames from.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use ket_relay_protocol::{Control, Frame, Kind};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::session::serve_phone;
use super::{Event, HANDSHAKE_TIMEOUT, MAX_PENDING_HANDSHAKES, PREADMIT_BYTES, Phones, lock};
use crate::Result;
use crate::host::server::Host;

pub(super) async fn link_forever(host: &Arc<Host>, phones: &Arc<Phones>) {
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
