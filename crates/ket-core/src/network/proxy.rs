//! The proxy itself: a loopback HTTP server that passes each request on to
//! its provider over HTTPS and streams the reply back as it arrives.
//!
//! Nothing it passes is changed but the hop: the token comes off the path,
//! the `Host` becomes the provider's, and the headers that only describe one
//! connection (`Connection`, `Transfer-Encoding` and the rest) are left to
//! each side's own. Credentials pass through as they came and are never
//! written anywhere.
//!
//! **The request is read whole before it goes on**, because what it holds is
//! the thing being inspected. A reply is never held: each frame goes back to
//! the agent the moment it arrives, so the first token comes no later than
//! it would have.
//!
//! **When the provider can't be reached**, the agent gets the provider's own
//! error shape with a 502, which its retries already know what to do with.

use std::convert::Infallible;
use std::net::{Ipv4Addr, TcpListener};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Uri, Version};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};

use super::{Binding, NetworkBindings, Route};

/// The pooled HTTPS client requests go on through.
type Upstream = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

/// What goes back to the agent.
type Reply = Response<BoxBody<Bytes, hyper::Error>>;

/// The running proxy. It lives as long as the host does.
pub struct Proxy {
    port: u16,
}

impl Proxy {
    /// Starts listening on a free loopback port, on a thread of its own.
    ///
    /// Fails when there is no port to have or the system's root certificates
    /// cannot be read: the host then launches agents as it would without it.
    pub fn start(bindings: NetworkBindings) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()?
            .https_only()
            .enable_http1()
            .build();
        std::thread::Builder::new()
            .name("ket-host-network".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::warn!(%error, "ket host: no runtime for the network inspector");
                        return;
                    }
                };
                runtime.block_on(serve(listener, bindings, https));
            })?;
        Ok(Self { port })
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address a pane holding `token` is given in place of its provider's.
    pub fn base(&self, token: &str) -> String {
        format!("http://127.0.0.1:{}/{token}", self.port)
    }
}

/// Accepts connections until the host exits.
async fn serve(
    listener: TcpListener,
    bindings: NetworkBindings,
    https: HttpsConnector<HttpConnector>,
) {
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, "ket host: the network inspector could not listen");
            return;
        }
    };
    let client: Upstream = Client::builder(TokioExecutor::new()).build(https);
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                // Out of file descriptors, most likely; accepting again at
                // once would only spin.
                tracing::warn!(%error, "ket host: the network inspector's accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (bindings, client) = (bindings.clone(), client.clone());
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let (bindings, client) = (bindings.clone(), client.clone());
                async move { Ok::<_, Infallible>(forward(request, &bindings, &client).await) }
            });
            // Kept alive: an agent sends its next request down the same
            // connection, and a new one per request would be a handshake per
            // turn on this side for nothing.
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                tracing::debug!(%error, "network inspector: connection ended");
            }
        });
    }
}

/// Passes one request on, and its reply back.
async fn forward(
    request: Request<Incoming>,
    bindings: &NetworkBindings,
    client: &Upstream,
) -> Reply {
    let (mut parts, body) = request.into_parts();
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", |target| target.as_str());
    let Some((token, rest)) = split_token(target) else {
        return refused();
    };
    let Some(binding) = bindings.lookup(token) else {
        return refused();
    };
    let rest = rest.to_owned();
    let body = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => return unreachable(binding.route, &error.to_string()),
    };
    let uri: Uri = match format!("https://{}{rest}", binding.route.upstream()).parse() {
        Ok(uri) => uri,
        Err(error) => return unreachable(binding.route, &error.to_string()),
    };
    strip_hop_by_hop(&mut parts.headers);
    // The client writes the provider's own, and the length of what it sends.
    parts.headers.remove(header::HOST);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.uri = uri;
    parts.version = Version::HTTP_11;
    let mut seen = Seen {
        binding,
        path: rest.split('?').next().unwrap_or_default().to_owned(),
        request: body.clone(),
        status: 0,
        received: 0,
        started: Instant::now(),
        done: false,
    };
    match client
        .request(Request::from_parts(parts, Full::new(body)))
        .await
    {
        Ok(response) => {
            let (mut parts, body) = response.into_parts();
            strip_hop_by_hop(&mut parts.headers);
            seen.status = parts.status.as_u16();
            // Nothing to come — a 204, or an empty body of known length:
            // hyper never polls it, so it is whole now.
            if body.is_end_stream() {
                seen.finish(true);
            }
            Response::from_parts(parts, Tee { inner: body, seen }.boxed())
        }
        Err(error) => {
            // Said as unreachable instead: whether any of it left is not
            // known, and a request that may not have is not logged as sent.
            seen.done = true;
            unreachable(seen.binding.route, &error.to_string())
        }
    }
}

/// `/<token>/rest` split into the token and `/rest`, which keeps its query.
fn split_token(target: &str) -> Option<(&str, &str)> {
    let target = target.strip_prefix('/')?;
    let end = target.find(['/', '?']).unwrap_or(target.len());
    let (token, rest) = target.split_at(end);
    if token.is_empty() {
        return None;
    }
    Some((token, rest))
}

/// The headers that describe one connection rather than the request, and
/// so stop at this hop: the standard ones, and any the `Connection` header
/// names.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        header::CONNECTION,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        headers.remove(name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

/// A request without a token the host issued: nothing here for it.
fn refused() -> Reply {
    let mut reply = Response::new(empty());
    *reply.status_mut() = StatusCode::NOT_FOUND;
    reply
}

/// The provider could not be reached: said in its own error shape, so the
/// agent shows it and retries as it would any network failure.
fn unreachable(route: Route, why: &str) -> Reply {
    tracing::warn!(upstream = route.upstream(), %why, "network inspector: provider unreachable");
    let message = format!(
        "ket's network inspector could not reach {}: {why}",
        route.upstream()
    );
    let body = serde_json::json!({
        "type": "error",
        "error": { "type": "api_error", "message": message },
    });
    let mut reply = Response::new(full(body.to_string().into()));
    *reply.status_mut() = StatusCode::BAD_GATEWAY;
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    reply
}

fn full(bytes: Bytes) -> BoxBody<Bytes, hyper::Error> {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

fn empty() -> BoxBody<Bytes, hyper::Error> {
    full(Bytes::new())
}

/// One request on its way through: said once its reply has ended, or been
/// cut off.
struct Seen {
    binding: Binding,
    /// The provider's path, without the query.
    path: String,
    /// What was sent, as it was sent.
    request: Bytes,
    status: u16,
    /// Bytes of reply passed back so far.
    received: usize,
    started: Instant,
    done: bool,
}

impl Seen {
    fn finish(&mut self, complete: bool) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        let model = serde_json::from_slice::<serde_json::Value>(&self.request)
            .ok()
            .and_then(|request| request.get("model")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        tracing::info!(
            pane = %self.binding.pane,
            path = %self.path,
            %model,
            status = self.status,
            sent = self.request.len(),
            received = self.received,
            ms = self.started.elapsed().as_millis() as u64,
            complete,
            "network inspector: observed"
        );
    }
}

impl Drop for Seen {
    fn drop(&mut self) {
        // The agent hung up first: still a request that left this Mac.
        self.finish(false);
    }
}

/// A reply's body, passed back frame by frame as it arrives, counted on the
/// way.
struct Tee {
    inner: Incoming,
    seen: Seen,
}

impl Body for Tee {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.seen.received += data.len();
                }
                // A reply of known length ends with its last frame: hyper
                // stops there, and never polls for the `None` after it.
                if this.inner.is_end_stream() {
                    this.seen.finish(true);
                }
            }
            Poll::Ready(Some(Err(_))) => this.seen.finish(false),
            Poll::Ready(None) => this.seen.finish(true),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
