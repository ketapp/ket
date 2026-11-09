//! `ket browser wait`: a look at the page every tenth of a second until what
//! was asked for is so — an element shown or gone, text on the page, the
//! address, the load — or the time runs out, which says what never
//! happened.
//!
//! A look the page doesn't answer, or not in time — a renderer busy with a
//! long script, say — says nothing either way. An element is gone only once
//! a look has seen it go: out of the document, hidden, or its ref naming
//! nothing in the page's document now, or naming a node Chromium no longer
//! has. A wait whose last look went unanswered says it couldn't tell.

use std::time::{Duration, Instant};

use gpui::FutureExt as _;
use ket_chromium::protocol::js_protocol::runtime::EvaluateParams;
use ket_core::host::wire::{BrowserTarget, BrowserWait};
use serde_json::Value;

use super::act::{Unresolved, stale};
use super::{Answer, LOAD_LOOK, LOOK_WAIT, Page};

/// Whether an element — `this` — is on the page with a box of some size and
/// not hidden: what a person could see, scrolled to it.
const VISIBLE: &str = r"function () {
    const el = this.nodeType === Node.ELEMENT_NODE ? this : this.parentElement;
    if (!this.isConnected || !el) return false;
    const box = el.getBoundingClientRect();
    return box.width > 0 && box.height > 0 && getComputedStyle(el).visibility !== 'hidden';
}";

/// The least time a look is given, however near the wait's end: what a page
/// that answers at all answers in.
const LOOK_LEAST: Duration = Duration::from_millis(500);

/// What Chromium says when it has no node to hand over for a ref
/// (`DOM.resolveNode`): the node collected, or its document gone.
const NO_NODE: &[&str] = &[
    "No node with given id found",
    "No node found for given backend id",
    "does not belong to the document",
];

/// What one look at an element found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// On the page, and visible.
    Shown,
    /// Not visible: hidden, of no size or out of the document — or, for a
    /// selector, nothing matches it.
    Hidden,
    /// Gone for good: its ref names nothing in the page's document now, or
    /// a node the page no longer has.
    Removed,
    /// The page didn't answer, or not in time: nothing is known.
    Unknown,
}

impl Page<'_> {
    /// Waits up to `timeout_ms` for `until`, then says where the page is,
    /// or what never happened.
    pub(super) async fn wait(&self, until: BrowserWait, timeout_ms: u64) -> Answer {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            // Each look short of the wait's end, so a page that never
            // answers is said to have not answered rather than cut off.
            let left = deadline.saturating_duration_since(Instant::now());
            let look = self
                .holds(&until)
                .with_timeout(LOOK_WAIT.min(left.max(LOOK_LEAST)), self.executor)
                .await;
            self.let_go();
            // A look that ran out of time is one the page didn't answer.
            let holds = look.unwrap_or(Ok(None))?;
            if holds == Some(true) {
                return self.here(None).await;
            }
            if Instant::now() >= deadline {
                let url = match until {
                    BrowserWait::Url(_) => self.spot().await.map(|spot| spot.url),
                    _ => None,
                };
                return Err(never(&until, timeout_ms, url.as_deref(), holds.is_some()));
            }
            self.executor.timer(LOAD_LOOK).await;
        }
    }

    /// Whether `until` is so now; `None` when the page didn't answer a look
    /// at an element. Fails when it never can be: a selector that is not
    /// one, a ref to show whose node has gone. Text, the address or the load
    /// the page could not say — it is between documents, say — is a no for
    /// now.
    async fn holds(&self, until: &BrowserWait) -> Result<Option<bool>, String> {
        Ok(match until {
            BrowserWait::Shown(target) => match self.look(target).await? {
                Seen::Removed => return Err(stale(target)),
                seen => verdict(false, seen),
            },
            BrowserWait::Gone(target) => verdict(true, self.look(target).await?),
            BrowserWait::Text(text) => {
                let text = serde_json::to_string(text).map_err(|error| error.to_string())?;
                let found = self
                    .value(&format!(
                        "!!document.body && document.body.innerText.includes({text})"
                    ))
                    .await;
                Some(found == Some(Value::Bool(true)))
            }
            BrowserWait::Url(part) => Some(
                self.spot()
                    .await
                    .is_some_and(|spot| spot.url.contains(part.as_str())),
            ),
            BrowserWait::Load => {
                Some(self.value("document.readyState").await == Some(Value::from("complete")))
            }
        })
    }

    /// What a look at `target` finds. Fails only for a selector that is not
    /// one.
    async fn look(&self, target: &BrowserTarget) -> Result<Seen, String> {
        let selector = match target {
            BrowserTarget::Ref(n) => {
                return Ok(match self.resolve(*n).await {
                    Ok((object, _)) => seen(self.call_on(&object, VISIBLE, Vec::new()).await),
                    Err(why) => unresolved(why),
                });
            }
            BrowserTarget::Selector(selector) => selector,
        };
        let literal = serde_json::to_string(selector).map_err(|error| error.to_string())?;
        let mut look = EvaluateParams::new(format!(
            "((el) => !!el && ({VISIBLE}).call(el))(document.querySelector({literal}))"
        ));
        look.return_by_value = Some(true);
        look.object_group = self.objects.group();
        let Ok(looked) = self.ask(look).await else {
            return Ok(Seen::Unknown);
        };
        if looked.exception_details.is_some() {
            return Err(format!("{target} is not a CSS selector"));
        }
        Ok(seen(Ok(looked.result.value.unwrap_or(Value::Null))))
    }
}

/// What [`VISIBLE`]'s answer says of an element.
fn seen(answer: Result<Value, String>) -> Seen {
    match answer {
        Ok(Value::Bool(true)) => Seen::Shown,
        Ok(Value::Bool(false)) => Seen::Hidden,
        // The page couldn't run it: its document going, say. The next look
        // finds out.
        _ => Seen::Unknown,
    }
}

/// What a ref's node not being had says of it: that it has gone, only if
/// the page said so.
fn unresolved(why: Unresolved) -> Seen {
    match why {
        Unresolved::Stale => Seen::Removed,
        Unresolved::Refused(why) if NO_NODE.iter().any(|said| why.contains(said)) => Seen::Removed,
        Unresolved::Unplaced | Unresolved::Refused(_) => Seen::Unknown,
    }
}

/// Whether an element waited for to be `gone`, else shown, is so, by what a
/// look at it `seen`; `None` when the look can't say.
fn verdict(gone: bool, seen: Seen) -> Option<bool> {
    match seen {
        Seen::Unknown => None,
        Seen::Shown => Some(!gone),
        Seen::Hidden | Seen::Removed => Some(gone),
    }
}

/// What the terminal is told when `until` was not so in `timeout_ms`, the
/// page being at `url` — the last look having been `answered` or not.
fn never(until: &BrowserWait, timeout_ms: u64, url: Option<&str>, answered: bool) -> String {
    let after = span(timeout_ms);
    match until {
        BrowserWait::Shown(target) => format!("{target} didn't show up in {after}"),
        // Not "still there": nobody saw it there.
        BrowserWait::Gone(target) if !answered => {
            format!("couldn't tell whether {target} went: the page didn't answer")
        }
        BrowserWait::Gone(target) => format!("{target} was still there after {after}"),
        BrowserWait::Text(text) => format!("\"{text}\" didn't show up on the page in {after}"),
        BrowserWait::Url(part) => match url {
            Some(url) => {
                format!("the address didn't come to have \"{part}\" in {after}; it's {url}")
            }
            None => format!("the address didn't come to have \"{part}\" in {after}"),
        },
        BrowserWait::Load => format!("the page was still loading after {after}"),
    }
}

/// `ms` as a person says it: `10s`, `1.5s`.
fn span(ms: u64) -> String {
    if ms.is_multiple_of(1_000) {
        format!("{}s", ms / 1_000)
    } else {
        format!("{:.1}s", ms as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_that_runs_out_says_what_never_happened() {
        let save = BrowserTarget::Selector("#save".to_owned());
        assert_eq!(
            never(&BrowserWait::Shown(save.clone()), 10_000, None, true),
            "\"#save\" didn't show up in 10s"
        );
        assert_eq!(
            never(&BrowserWait::Gone(BrowserTarget::Ref(4)), 1_500, None, true),
            "@e4 was still there after 1.5s"
        );
        assert_eq!(
            never(&BrowserWait::Text("Saved".to_owned()), 5_000, None, true),
            "\"Saved\" didn't show up on the page in 5s"
        );
        assert_eq!(
            never(
                &BrowserWait::Url("/dashboard".to_owned()),
                10_000,
                Some("http://localhost:3000/login"),
                true
            ),
            "the address didn't come to have \"/dashboard\" in 10s; it's \
             http://localhost:3000/login"
        );
        assert_eq!(
            never(&BrowserWait::Load, 500, None, true),
            "the page was still loading after 0.5s"
        );
    }

    #[test]
    fn an_element_is_gone_only_once_a_look_sees_it_go() {
        // What a ref's node not being had says.
        let refused = |why: &str| unresolved(Unresolved::Refused(why.to_owned()));
        assert_eq!(unresolved(Unresolved::Stale), Seen::Removed);
        assert_eq!(
            refused("DOM.resolveNode: No node with given id found"),
            Seen::Removed
        );
        assert_eq!(
            refused("DOM.resolveNode: Node with given id does not belong to the document"),
            Seen::Removed
        );
        // The page not saying where it is, or not answering: unknown.
        assert_eq!(unresolved(Unresolved::Unplaced), Seen::Unknown);
        assert_eq!(
            refused("DOM.resolveNode: was not answered in 30s"),
            Seen::Unknown
        );
        assert_eq!(refused("the page has closed"), Seen::Unknown);
        // What a look at the node says.
        assert_eq!(seen(Ok(Value::Bool(true))), Seen::Shown);
        assert_eq!(seen(Ok(Value::Bool(false))), Seen::Hidden);
        assert_eq!(seen(Ok(Value::Null)), Seen::Unknown);
        assert_eq!(
            seen(Err(
                "Runtime.callFunctionOn: Cannot find context with specified id".to_owned()
            )),
            Seen::Unknown
        );
        // Gone: hidden or removed is, shown isn't, unknown can't say.
        assert_eq!(verdict(true, Seen::Hidden), Some(true));
        assert_eq!(verdict(true, Seen::Removed), Some(true));
        assert_eq!(verdict(true, Seen::Shown), Some(false));
        assert_eq!(verdict(true, Seen::Unknown), None);
        // Shown: the other way round.
        assert_eq!(verdict(false, Seen::Shown), Some(true));
        assert_eq!(verdict(false, Seen::Hidden), Some(false));
        assert_eq!(verdict(false, Seen::Unknown), None);
        // Run out with the last look unanswered, a wait for it to go says so.
        assert_eq!(
            never(
                &BrowserWait::Gone(BrowserTarget::Ref(4)),
                10_000,
                None,
                false
            ),
            "couldn't tell whether @e4 went: the page didn't answer"
        );
    }
}
