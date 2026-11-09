//! WebSockets, which Codex talks to OpenAI over before it tries HTTPS:
//! passed on byte for byte, and read on the way.
//!
//! The handshake goes on as the agent asked for it, its offer to compress
//! included, and the provider's answer comes back as it was given. From then
//! on the proxy only copies: each side gets exactly the bytes the other
//! sent. A copy of what the agent sends is read as frames, for its messages
//! — each `response.create` is one request, as a POST of it would be — and a
//! copy of what comes back for the event that ends each reply, which is
//! where its usage is.
//!
//! What cannot be read — a message past [`MESSAGE_KEPT`], one that does not
//! inflate — costs the log the rest of that socket, never the socket.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};

use flate2::{Decompress, FlushDecompress};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::http::request::Parts;
use hyper::{Request, Response, StatusCode, Uri, Version};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::protocol::frame::FrameHeader;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};

use super::proxy::{self, Reply, Seen, Upstream};
use super::{Binding, Observed, Target};

/// The most of one message held while its frames arrive.
const MESSAGE_KEPT: usize = 64 * 1024 * 1024;

/// Whether the agent asked for a WebSocket.
pub(super) fn asked(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Asks the provider for the socket the agent asked for, at `rest`, and
/// once both ends are open, joins them.
pub(super) async fn forward(
    mut parts: Parts,
    rest: &str,
    binding: Binding,
    target: Target,
    client: &Upstream,
    log: mpsc::Sender<Observed>,
) -> Reply {
    let Some(agent) = parts.extensions.remove::<hyper::upgrade::OnUpgrade>() else {
        return proxy::refused();
    };
    let uri: Uri = match format!("https://{}{rest}", target.host).parse() {
        Ok(uri) => uri,
        Err(error) => return proxy::unreachable(binding.route, target, &error.to_string()),
    };
    proxy::strip_hop_by_hop(&mut parts.headers);
    // Said only to this hop, and so taken off with the rest; but what asks
    // the provider, in turn, for a socket of its own.
    parts
        .headers
        .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    parts
        .headers
        .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    parts.headers.remove(header::HOST);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.uri = uri;
    parts.version = Version::HTTP_11;
    let mut response = match client
        .request(Request::from_parts(parts, Full::new(Bytes::new())))
        .await
    {
        Ok(response) => response,
        Err(error) => return proxy::unreachable(binding.route, target, &error.to_string()),
    };
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        // Turned down: the agent hears it as the provider said it, and goes
        // over HTTPS instead.
        let (mut parts, body) = response.into_parts();
        proxy::strip_hop_by_hop(&mut parts.headers);
        return Response::from_parts(parts, body.boxed());
    }
    let provider = hyper::upgrade::on(&mut response);
    let deflate = response
        .headers()
        .get(header::SEC_WEBSOCKET_EXTENSIONS)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("permessage-deflate"));
    let talk = Talk {
        binding,
        target,
        path: rest.to_owned(),
        log,
        waiting: VecDeque::new(),
    };
    tokio::spawn(async move {
        match tokio::join!(agent, provider) {
            (Ok(agent), Ok(provider)) => {
                relay(TokioIo::new(agent), TokioIo::new(provider), deflate, talk).await;
            }
            (Err(error), _) | (_, Err(error)) => {
                tracing::debug!(%error, "network inspector: a socket did not open");
            }
        }
    });
    // The provider's answer as it gave it — `Upgrade`, `Connection` and the
    // rest: they are this hop's now.
    let (parts, _) = response.into_parts();
    Response::from_parts(parts, proxy::empty())
}

/// Copies each end's bytes to the other until either goes, reading the
/// copies on the way.
async fn relay(
    agent: impl AsyncRead + AsyncWrite + Unpin,
    provider: impl AsyncRead + AsyncWrite + Unpin,
    deflate: bool,
    talk: Talk,
) {
    let talk = Mutex::new(talk);
    let (mut agent_read, mut agent_write) = tokio::io::split(agent);
    let (mut provider_read, mut provider_write) = tokio::io::split(provider);
    let sent = pump(
        &mut agent_read,
        &mut provider_write,
        Frames::new(deflate),
        |message| lock(&talk).sent(message),
    );
    let received = pump(
        &mut provider_read,
        &mut agent_write,
        Frames::new(deflate),
        |message| lock(&talk).received(&message),
    );
    // Either end gone is the socket gone: nothing more can reach it, and a
    // request still waiting was cut off.
    tokio::select! {
        () = sent => {}
        () = received => {}
    }
}

/// Passes `from`'s bytes to `to` as they come, and each message they make
/// to `said`.
async fn pump(
    from: &mut (impl AsyncRead + Unpin),
    to: &mut (impl AsyncWrite + Unpin),
    mut frames: Frames,
    mut said: impl FnMut(Vec<u8>),
) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match from.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if to.write_all(&buffer[..read]).await.is_err() || to.flush().await.is_err() {
            return;
        }
        frames.feed(&buffer[..read], &mut said);
    }
    let _ = to.shutdown().await;
}

fn lock(talk: &Mutex<Talk>) -> MutexGuard<'_, Talk> {
    talk.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One direction's bytes, read into whole messages.
struct Frames {
    /// What has come of a frame not yet whole.
    pending: Vec<u8>,
    /// The message so far, from its frames' payloads.
    message: Vec<u8>,
    /// Whether it was compressed, as its first frame said.
    compressed: bool,
    /// When the two ends agreed to compress: one context for the whole
    /// socket, which its messages are compressed against in turn.
    inflate: Option<Decompress>,
    /// Given up on: everything from here on goes by unread.
    lost: bool,
}

impl Frames {
    fn new(deflate: bool) -> Self {
        Self {
            pending: Vec::new(),
            message: Vec::new(),
            compressed: false,
            inflate: deflate.then(|| Decompress::new(false)),
            lost: false,
        }
    }

    fn feed(&mut self, bytes: &[u8], said: &mut impl FnMut(Vec<u8>)) {
        if self.lost {
            return;
        }
        self.pending.extend_from_slice(bytes);
        let mut at = 0;
        loop {
            let mut cursor = Cursor::new(&self.pending[at..]);
            let (header, length) = match FrameHeader::parse(&mut cursor) {
                Ok(Some(parsed)) => parsed,
                Ok(None) => break,
                Err(error) => return self.lose(&error.to_string()),
            };
            let Some(length) = usize::try_from(length)
                .ok()
                .filter(|length| self.message.len() + length <= MESSAGE_KEPT)
            else {
                return self.lose("a message too large to read");
            };
            let start = at + cursor.position() as usize;
            if self.pending.len() - start < length {
                break;
            }
            let payload = &mut self.pending[start..start + length];
            if let Some(mask) = header.mask {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            match header.opcode {
                // Close, ping and pong: between a message's frames, never
                // part of one.
                OpCode::Control(_) => {}
                OpCode::Data(Data::Continue) => self.message.extend_from_slice(payload),
                OpCode::Data(_) => {
                    self.message.clear();
                    self.message.extend_from_slice(payload);
                    self.compressed = header.rsv1;
                }
            }
            at = start + length;
            if header.is_final && matches!(header.opcode, OpCode::Data(_)) {
                let message = std::mem::take(&mut self.message);
                match self.whole(message) {
                    Some(message) => said(message),
                    None => return self.lose("a message that did not inflate"),
                }
            }
        }
        self.pending.drain(..at);
    }

    /// `message` as it was before it was compressed, if it was.
    fn whole(&mut self, mut message: Vec<u8>) -> Option<Vec<u8>> {
        if !self.compressed {
            return Some(message);
        }
        let inflate = self.inflate.as_mut()?;
        // Each message is the next stretch of one deflate stream, sent
        // without the empty block that ends a stretch.
        message.extend_from_slice(&[0, 0, 0xff, 0xff]);
        let mut out = Vec::with_capacity(message.len() * 4);
        let mut used = 0;
        loop {
            if out.len() == out.capacity() {
                out.reserve(out.capacity().max(64 * 1024));
            }
            let (before_in, before_out) = (inflate.total_in(), out.len());
            inflate
                .decompress_vec(&message[used..], &mut out, FlushDecompress::Sync)
                .ok()?;
            used += (inflate.total_in() - before_in) as usize;
            if out.len() > MESSAGE_KEPT {
                return None;
            }
            let stuck = inflate.total_in() == before_in && out.len() == before_out;
            if (used == message.len() && out.len() < out.capacity()) || stuck {
                break;
            }
        }
        Some(out)
    }

    fn lose(&mut self, why: &str) {
        tracing::debug!(%why, "network inspector: the rest of a socket goes unread");
        self.lost = true;
        self.pending = Vec::new();
        self.message = Vec::new();
    }
}

/// What a reply event is, by its `type`.
#[derive(Deserialize)]
struct Event<'a> {
    #[serde(rename = "type", borrow)]
    kind: std::borrow::Cow<'a, str>,
    /// An `error` event's HTTP status, when it gives one.
    status: Option<u16>,
}

/// One socket's requests, each waiting for the event that ends its reply.
struct Talk {
    binding: Binding,
    target: Target,
    path: String,
    log: mpsc::Sender<Observed>,
    /// Oldest first: the provider answers them in the order they came.
    waiting: VecDeque<Seen>,
}

impl Talk {
    /// A message the agent sent.
    fn sent(&mut self, message: Vec<u8>) {
        // What asks for a reply. Anything else said on the socket is not a
        // request of its own.
        let asks = serde_json::from_slice::<Event>(&message)
            .is_ok_and(|event| event.kind == "response.create");
        if !asks {
            tracing::debug!(pane = %self.binding.pane, "network inspector: a socket message that asks for nothing; not logged");
            return;
        }
        self.waiting.push_back(Seen::new(
            self.binding.clone(),
            self.target,
            &self.path,
            Bytes::from(message),
            self.log.clone(),
        ));
    }

    /// A message from the provider: more of the oldest request's reply, or
    /// its end.
    fn received(&mut self, message: &[u8]) {
        let Some(seen) = self.waiting.front_mut() else {
            return;
        };
        seen.received += message.len();
        // A socket has no status of its own after it opens: a reply that
        // began is one that was answered.
        if seen.status == 0 {
            seen.status = 200;
        }
        let Ok(event) = serde_json::from_slice::<Event>(message) else {
            return;
        };
        let status = match event.kind.as_ref() {
            "response.completed" | "response.incomplete" => 200,
            "response.failed" => 500,
            "error" => event.status.unwrap_or(500),
            _ => return,
        };
        let Some(mut seen) = self.waiting.pop_front() else {
            return;
        };
        seen.status = status;
        // Kept as a stream's last event, which is how its usage is read.
        seen.keep(b"data: ");
        seen.keep(message);
        seen.keep(b"\n");
        seen.finish(true);
    }
}
