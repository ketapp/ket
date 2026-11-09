//! A page's DevTools protocol, in process: through CEF's
//! `send_dev_tools_message` and a message observer, not a remote-debugging
//! port anything on the machine could drive ket's pages through. Commands
//! are answered by id and events handed on; `chromiumoxide_cdp` types the
//! messages. No particular async runtime: the waits are oneshot channels and
//! the timeout a delayed task on CEF's thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chromiumoxide_types::{CallId, Command, MethodCall};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};

use crate::{Error, engine, failed};

/// How long a command may go unanswered; a slow page's navigation fits.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How far a slow listener may fall behind before it misses events.
const EVENT_BACKLOG: usize = 1024;

/// An event from a page's DevTools protocol — `Page.loadEventFired` and the
/// like, for whichever domains have been enabled.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    /// The event's name, `Domain.event`.
    pub method: String,
    /// Its parameters, as the protocol sent them.
    pub params: Value,
}

/// A page's DevTools protocol. Cloned freely and used from any thread; it
/// stops working when the page closes.
#[derive(Clone)]
pub struct Cdp {
    shared: Arc<Shared>,
}

struct Shared {
    page: u64,
    next: AtomicUsize,
    pending: Mutex<HashMap<usize, Answer>>,
    /// Gone once the page has closed, which ends every listener.
    events: Mutex<Option<broadcast::Sender<CdpEvent>>>,
}

type Answer = oneshot::Sender<Result<Value, String>>;

/// What the page sends: an answer, with the `id` it answers, or an event.
#[derive(Deserialize)]
struct Incoming {
    id: Option<usize>,
    result: Option<Value>,
    error: Option<chromiumoxide_types::Error>,
    method: Option<String>,
    params: Option<Value>,
}

impl Cdp {
    pub(crate) fn new(page: u64) -> Self {
        Self {
            shared: Arc::new(Shared {
                page,
                next: AtomicUsize::new(1),
                pending: Mutex::new(HashMap::new()),
                events: Mutex::new(Some(broadcast::channel(EVENT_BACKLOG).0)),
            }),
        }
    }

    /// Sends `command` to the page and waits for its answer, typed. A page
    /// still being created gets it once it exists.
    pub async fn call<C: Command>(&self, command: C) -> Result<C::Response, Error> {
        let method = command.identifier();
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        let call = MethodCall {
            id: CallId::new(id),
            method: method.clone(),
            session_id: None,
            params: serde_json::to_value(&command).map_err(failed)?,
        };
        let message = serde_json::to_vec(&call).map_err(failed)?;
        let (answer, answered) = oneshot::channel();
        lock(&self.shared.pending).insert(id, answer);

        let page = self.shared.page;
        let cdp = self.clone();
        if !engine::on_ui(None, move || {
            crate::browser::send_devtools(page, id, message, &cdp)
        }) {
            lock(&self.shared.pending).remove(&id);
            return Err(gone());
        }
        let cdp = self.clone();
        engine::on_ui(Some(CALL_TIMEOUT), move || {
            cdp.answer(id, Err(format!("was not answered in {CALL_TIMEOUT:?}")));
        });

        match answered.await {
            Ok(Ok(value)) => C::response_from_value(value)
                .map_err(|e| failed(format!("{method} answered oddly: {e}"))),
            Ok(Err(why)) => Err(failed(format!("{method}: {why}"))),
            Err(_) => Err(gone()),
        }
    }

    /// Sends `command` to the page without waiting for its answer, which is
    /// dropped when it comes: for what is sent many times a second — a
    /// screencast's acknowledgements — where [`call`](Self::call)'s answer
    /// and timeout would each be one more thing waiting. A page still being
    /// created gets it once it exists; if the page has closed, or Chromium
    /// refuses the message, nothing says so.
    pub fn send<C: Command>(&self, command: C) {
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        let params = match serde_json::to_value(&command) {
            Ok(params) => params,
            Err(error) => {
                tracing::debug!(%error, "a DevTools command that would not encode");
                return;
            }
        };
        let call = MethodCall {
            id: CallId::new(id),
            method: command.identifier(),
            session_id: None,
            params,
        };
        let Ok(message) = serde_json::to_vec(&call) else {
            return;
        };
        let page = self.shared.page;
        let cdp = self.clone();
        engine::on_ui(None, move || {
            crate::browser::send_devtools(page, id, message, &cdp)
        });
    }

    /// The page's events from now on, for whichever domains a command has
    /// enabled. Ends when the page closes; a listener that falls far behind
    /// is told how many it missed.
    pub fn events(&self) -> broadcast::Receiver<CdpEvent> {
        match &*lock(&self.shared.events) {
            Some(events) => events.subscribe(),
            None => broadcast::channel(1).1,
        }
    }

    /// Answers call `id`, if it is still waiting.
    pub(crate) fn answer(&self, id: usize, result: Result<Value, String>) {
        let waiting = lock(&self.shared.pending).remove(&id);
        if let Some(waiting) = waiting {
            let _ = waiting.send(result);
        }
    }

    /// A message from the page, as CEF's observer hands it over.
    pub(crate) fn receive(&self, message: &[u8]) {
        let incoming: Incoming = match serde_json::from_slice(message) {
            Ok(incoming) => incoming,
            Err(error) => {
                tracing::debug!(%error, "an unreadable DevTools message");
                return;
            }
        };
        if let Some(id) = incoming.id {
            self.answer(
                id,
                match incoming.error {
                    Some(error) => Err(error.message),
                    None => Ok(incoming.result.unwrap_or(Value::Null)),
                },
            );
        } else if let Some(method) = incoming.method {
            let event = CdpEvent {
                method,
                params: incoming.params.unwrap_or(Value::Null),
            };
            if let Some(events) = &*lock(&self.shared.events) {
                // No listener is not a failure.
                let _ = events.send(event);
            }
        }
    }

    /// The page has closed: whatever is waiting is told so, and listeners
    /// end.
    pub(crate) fn close(&self) {
        lock(&self.shared.pending).clear();
        lock(&self.shared.events).take();
    }
}

fn gone() -> Error {
    failed("the page has closed")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_find_their_calls_and_events_their_listeners() {
        let cdp = Cdp::new(1);
        let mut events = cdp.events();
        let mut calls = [1, 2, 3].map(|id| {
            let (answer, answered) = oneshot::channel();
            lock(&cdp.shared.pending).insert(id, answer);
            answered
        });
        cdp.receive(br#"{"id":2,"error":{"code":-32601,"message":"no such method"}}"#);
        cdp.receive(br#"{"id":1,"result":{"frameId":"F"}}"#);
        cdp.receive(br#"{"method":"Page.loadEventFired","params":{"t":1}}"#);
        cdp.receive(b"not json");
        let [first, second, third] = &mut calls;
        assert_eq!(
            first.try_recv().ok(),
            Some(Ok(serde_json::json!({"frameId": "F"})))
        );
        assert_eq!(second.try_recv().ok(), Some(Err("no such method".into())));
        let event = events.try_recv().ok();
        assert_eq!(
            event.map(|event| event.method).as_deref(),
            Some("Page.loadEventFired")
        );
        // What is only sent waits for no answer: Chromium's, when it comes,
        // finds nothing to answer.
        cdp.send(chromiumoxide_cdp::cdp::browser_protocol::page::ScreencastFrameAckParams::new(1));
        assert_eq!(lock(&cdp.shared.pending).len(), 1);
        cdp.receive(br#"{"id":4,"result":{}}"#);
        assert_eq!(lock(&cdp.shared.pending).len(), 1);
        // Closing the page answers what is left and ends every listener.
        cdp.close();
        assert!(third.try_recv().is_err());
        assert!(events.try_recv().is_err());
        assert!(cdp.events().try_recv().is_err());
    }
}
