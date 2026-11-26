//! A phone moving a terminal's task to another agent: asked of the window
//! showing the terminal, which opens the agent and types the brief into it
//! — see `crate::handoff` for the brief and `host/phones/switch.rs` for the
//! phone's side. The same round trip as Start fresh: a [`Reply`] to the
//! window, a [`Request::SwitchResult`] back naming the new tab, and the
//! host finds that tab's terminal to answer the phone with.
//!
//! [`Request::SwitchResult`]: super::Request::SwitchResult

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::{FIX_CHECKS_LOOK, Host, Out, Reply, WAKE_WAIT, Woken, lock};

/// How long the host waits for a window to say how a switch went: the
/// window waits for Codex to start for two minutes, and on an agent's
/// startup question as long as the person takes. Answered within it, the
/// phone is told the new terminal, even asking again after a dropped link.
const ANSWER_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(10 * 60)
};

/// Longest a switch the host stopped waiting for holds its tab.
const HELD: Duration = Duration::from_secs(30 * 60);

/// What a window answered: the new tab's pane key, or why there is none.
type Switched = std::result::Result<String, String>;

/// The switches waiting on a window — a field of [`Host`].
#[derive(Default)]
pub(super) struct Switches {
    waiting: Mutex<BTreeMap<u64, mpsc::Sender<Switched>>>,
    /// Each tab's switch still in a window's hands, by its pane key. One at
    /// a time, so one the host stopped waiting for can't be asked again
    /// while the window may still finish it — two new agents, and the old
    /// tab closed twice.
    held: Mutex<BTreeMap<String, Held>>,
    next: AtomicU64,
}

/// What a phone asked the terminal's task to move to.
pub(in crate::host) struct Switch {
    /// The agent to open.
    pub(in crate::host) agent: String,
    /// Its model and effort; `None` for its own default.
    pub(in crate::host) model: Option<String>,
    pub(in crate::host) effort: Option<String>,
    /// The brief, its first message.
    pub(in crate::host) text: String,
    /// In a new tab beside the old one, rather than in its place.
    pub(in crate::host) beside: bool,
    /// The session the old tab held as the phone asked, where known.
    pub(in crate::host) session: Option<String>,
}

impl Host {
    /// Moves the task of the tab `key` names to another agent, for a phone:
    /// asks the window showing it, waits for that window to name the tab the
    /// new agent runs in — up to [`ANSWER_WAIT`] — then finds its terminal.
    /// The new terminal's id, or why not.
    pub(in crate::host) fn switch_agent(&self, key: &str, switch: Switch) -> Woken {
        let Some(out) = self.window_showing(key) else {
            return Err("that tab isn't open in a ket window on your Mac".to_owned());
        };
        let req = self.switches.next.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut held = lock(&self.switches.held);
            held.retain(|_, held| held.standing());
            if held.contains_key(key) {
                return Err(STILL_SWITCHING.to_owned());
            }
            let asked = Held {
                req,
                out: out.clone(),
                at: Instant::now(),
                in_place: !switch.beside,
            };
            held.insert(key.to_owned(), asked);
        }
        let (sender, receiver) = mpsc::channel();
        lock(&self.switches.waiting).insert(req, sender);
        if !out.reply(&Reply::SwitchAgent {
            req,
            key: key.to_owned(),
            agent: switch.agent,
            model: switch.model,
            effort: switch.effort,
            text: switch.text,
            beside: switch.beside,
            session: switch.session,
        }) {
            lock(&self.switches.waiting).remove(&req);
            self.switches.release(req);
            return Err("the ket window closed before it could switch".to_owned());
        }
        let deadline = Instant::now() + ANSWER_WAIT;
        let named = loop {
            match receiver.recv_timeout(FIX_CHECKS_LOOK) {
                Ok(answer) => break answer,
                Err(_) if !out.alive.load(Ordering::Acquire) => {
                    self.switches.release(req);
                    break Err("the ket window closed before it switched".to_owned());
                }
                // Still the window's: held until it answers, see `held`.
                Err(_) if Instant::now() >= deadline => break Err(STILL_SWITCHING.to_owned()),
                Err(_) => {}
            }
        };
        lock(&self.switches.waiting).remove(&req);
        let opened = named?;
        // The window names the tab once the agent has the brief, so its
        // terminal is open; looked for again while the host files it.
        let deadline = Instant::now() + WAKE_WAIT;
        while Instant::now() < deadline {
            let found = self.listing().into_iter().find(|(_, key, terminal)| {
                key.as_deref() == Some(opened.as_str()) && !terminal.is_closed()
            });
            if let Some((id, _, _)) = found {
                return Ok(id);
            }
            std::thread::sleep(FIX_CHECKS_LOOK);
        }
        Err("the new agent didn't start on your Mac".to_owned())
    }

    /// Whether the tab `key` names is being replaced by a switch still in a
    /// window's hands: its terminal may close at any moment.
    pub(in crate::host) fn replacing(&self, key: &str) -> bool {
        lock(&self.switches.held)
            .get(key)
            .is_some_and(|held| held.in_place && held.standing())
    }

    /// A window's answer to [`Reply::SwitchAgent`].
    pub(super) fn finish_switch(&self, req: u64, key: Option<String>, error: Option<String>) {
        let answer = key.ok_or_else(|| {
            error.unwrap_or_else(|| "ket on your Mac didn't switch agents".to_owned())
        });
        self.switches.release(req);
        if let Some(waiting) = lock(&self.switches.waiting).remove(&req) {
            let _ = waiting.send(answer);
        }
    }
}

/// A tab's switch in a window's hands.
struct Held {
    req: u64,
    /// The window asked.
    out: Arc<Out>,
    /// When.
    at: Instant,
    /// In the tab's place, rather than beside it.
    in_place: bool,
}

impl Held {
    /// Whether the window may still finish it: it is open, and it was asked
    /// within [`HELD`].
    fn standing(&self) -> bool {
        self.out.alive.load(Ordering::Acquire) && self.at.elapsed() < HELD
    }
}

/// Why a second switch of a tab is refused while its first is under way.
const STILL_SWITCHING: &str =
    "ket on your Mac is still starting the new agent: its tab opens once the agent is ready";

impl std::fmt::Debug for Switches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Switches").finish_non_exhaustive()
    }
}

impl Switches {
    /// Lets the tab switch `req` held be switched again.
    fn release(&self, req: u64) {
        lock(&self.held).retain(|_, held| held.req != req);
    }
}

#[cfg(test)]
mod tests;
