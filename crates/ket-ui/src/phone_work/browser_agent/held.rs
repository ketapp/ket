//! What an action has pressed and not let go of yet — a key, a mouse button
//! — so that it is let go of all the same when the action is cut off between
//! the two.
//!
//! Chromium answers `Input.dispatchKeyEvent` and `Input.dispatchMouseEvent`
//! once the page has handled the event. A key down or a mouse press whose
//! handler opens a JavaScript dialog is not answered until the dialog closes,
//! and the dialog opening ends the action (see `dialog`), so the key up or
//! the mouse release after it is never sent: the page is left with a key held
//! or a button down — a drag that never ends, a shortcut's modifier still
//! down.
//!
//! So each press is noted in the same turn it is sent, with no `await`
//! between them — a press noted is a press sent — and its release clears it
//! the same way. What is still noted when the action ends is let go of, in
//! the order the action would have: at once if the action ended on its own,
//! failing between the two; once the dialog has closed if a dialog stopped
//! it. Then the page handles input again, and the release lands after the
//! press's own handler has finished, as it would have.

use ket_chromium::Cdp;
use ket_chromium::protocol::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    MouseButton,
};

use super::act::{Point, mouse_event};
use super::keys::{Key, Modifier};
use super::typing::key_event;

/// What an action has pressed and not yet let go of, first pressed first.
#[derive(Debug, Default)]
pub(super) struct Held {
    presses: Vec<Press>,
}

/// A press not yet let go of.
#[derive(Debug, Clone, PartialEq)]
enum Press {
    /// A key, a modifier among them.
    Key(Key),
    /// A mouse button, pressed at a point, as the `clicks`th click.
    Mouse {
        at: Point,
        button: MouseButton,
        clicks: i64,
    },
}

/// The event that lets go of a press.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Release {
    /// A key up.
    Key(DispatchKeyEventParams),
    /// A mouse release.
    Mouse(DispatchMouseEventParams),
}

impl Release {
    /// Sends it to the page, not waiting for it to be taken: the page takes
    /// it after whatever was sent before it.
    pub(super) fn send(self, cdp: &Cdp) {
        match self {
            Self::Key(up) => cdp.send(up),
            Self::Mouse(release) => cdp.send(release),
        }
    }
}

impl Held {
    /// Key event `kind` for `key` is about to be sent: a key down holds the
    /// key, a key up lets go of it.
    pub(super) fn key(&mut self, kind: &DispatchKeyEventType, key: &Key) {
        match kind {
            DispatchKeyEventType::KeyDown | DispatchKeyEventType::RawKeyDown => {
                self.presses.push(Press::Key(key.clone()));
            }
            DispatchKeyEventType::KeyUp => {
                self.clear(|press| matches!(press, Press::Key(held) if held == key));
            }
            DispatchKeyEventType::Char => {}
        }
    }

    /// Mouse event `kind` with `button` at `at`, the `clicks`th click, is
    /// about to be sent: a press holds the button, a release lets go of it.
    pub(super) fn mouse(
        &mut self,
        kind: &DispatchMouseEventType,
        at: Point,
        button: &MouseButton,
        clicks: i64,
    ) {
        match kind {
            DispatchMouseEventType::MousePressed => self.presses.push(Press::Mouse {
                at,
                button: button.clone(),
                clicks,
            }),
            DispatchMouseEventType::MouseReleased => self.clear(
                |press| matches!(press, Press::Mouse { button: held, .. } if held == button),
            ),
            DispatchMouseEventType::MouseMoved | DispatchMouseEventType::MouseWheel => {}
        }
    }

    /// Lets go of the latest press `which` picks.
    fn clear(&mut self, which: impl Fn(&Press) -> bool) {
        if let Some(at) = self.presses.iter().rposition(which) {
            self.presses.remove(at);
        }
    }

    /// The events that let go of everything still held, latest first, as
    /// the action would have sent them — a key up with the modifiers still
    /// held under it — leaving nothing held.
    pub(super) fn releases(&mut self) -> Vec<Release> {
        let mut releases = Vec::new();
        while let Some(press) = self.presses.pop() {
            let modifiers = self
                .presses
                .iter()
                .filter_map(|press| match press {
                    Press::Key(key) => Modifier::of(key),
                    Press::Mouse { .. } => None,
                })
                .fold(0, |held, modifier| held | modifier.bit());
            releases.push(match press {
                Press::Key(key) => {
                    Release::Key(key_event(DispatchKeyEventType::KeyUp, &key, modifiers, &[]))
                }
                Press::Mouse { at, button, clicks } => Release::Mouse(mouse_event(
                    DispatchMouseEventType::MouseReleased,
                    at,
                    button,
                    0,
                    clicks,
                )),
            });
        }
        releases
    }
}

#[cfg(test)]
mod tests {
    use super::super::keys::chord;
    use super::*;

    const AT: Point = Point {
        x: 60.0,
        y: 40.0,
        height: 20.0,
    };

    /// What `releases` sends, near enough to read: `up a 4`, `release Left 2`.
    fn sent(held: &mut Held) -> Vec<String> {
        held.releases()
            .into_iter()
            .map(|release| match release {
                Release::Key(up) => {
                    assert_eq!(up.r#type, DispatchKeyEventType::KeyUp);
                    format!(
                        "up {} {}",
                        up.key.unwrap_or_default(),
                        up.modifiers.unwrap_or_default()
                    )
                }
                Release::Mouse(release) => {
                    assert_eq!(release.r#type, DispatchMouseEventType::MouseReleased);
                    assert_eq!((release.x, release.y), (AT.x, AT.y));
                    assert_eq!(release.buttons, Some(0));
                    format!(
                        "release {:?} {}",
                        release.button.expect("a button"),
                        release.click_count.unwrap_or_default()
                    )
                }
            })
            .collect()
    }

    #[test]
    fn a_chord_cut_off_after_its_key_down_lets_go_of_the_key_then_its_modifiers() {
        // ⌘⇧Z: Meta down, Shift down, Z down — and a dialog.
        let redo = chord("Meta+Shift+Z").unwrap();
        let mut held = Held::default();
        for modifier in &redo.modifiers {
            held.key(&DispatchKeyEventType::RawKeyDown, &modifier.key());
        }
        held.key(&DispatchKeyEventType::RawKeyDown, &redo.key);
        // As `chord` would have: Z up with both held, Shift up with Meta,
        // Meta up with nothing.
        assert_eq!(sent(&mut held), ["up Z 12", "up Shift 4", "up Meta 0"]);
        assert!(held.releases().is_empty(), "nothing is let go of twice");
    }

    #[test]
    fn what_was_let_go_of_before_the_cut_is_not_let_go_of_again() {
        let select_all = chord("Meta+A").unwrap();
        let meta = select_all.modifiers[0].key();
        let mut held = Held::default();
        held.key(&DispatchKeyEventType::RawKeyDown, &meta);
        held.key(&DispatchKeyEventType::RawKeyDown, &select_all.key);
        held.key(&DispatchKeyEventType::KeyUp, &select_all.key);
        // Cut off as Meta was about to go up.
        assert_eq!(sent(&mut held), ["up Meta 0"]);

        // A whole chord, sent: nothing left.
        held.key(&DispatchKeyEventType::RawKeyDown, &meta);
        held.key(&DispatchKeyEventType::RawKeyDown, &select_all.key);
        held.key(&DispatchKeyEventType::KeyUp, &select_all.key);
        held.key(&DispatchKeyEventType::KeyUp, &meta);
        assert!(held.releases().is_empty());

        // A key that types goes down as one `keyDown`, and is held the same.
        let mut typing = Held::default();
        let enter = chord("Enter").unwrap().key;
        typing.key(&DispatchKeyEventType::KeyDown, &enter);
        assert_eq!(sent(&mut typing), ["up Enter 0"]);
    }

    #[test]
    fn a_click_cut_off_after_its_press_lets_go_of_the_button_where_it_went_down() {
        let mut held = Held::default();
        held.mouse(
            &DispatchMouseEventType::MouseMoved,
            AT,
            &MouseButton::None,
            0,
        );
        held.mouse(
            &DispatchMouseEventType::MousePressed,
            AT,
            &MouseButton::Left,
            1,
        );
        assert_eq!(sent(&mut held), ["release Left 1"]);

        // A double click whose first click went through, cut off at the
        // second press: the second click's release.
        for kind in [
            DispatchMouseEventType::MousePressed,
            DispatchMouseEventType::MouseReleased,
        ] {
            held.mouse(&kind, AT, &MouseButton::Left, 1);
        }
        held.mouse(
            &DispatchMouseEventType::MousePressed,
            AT,
            &MouseButton::Left,
            2,
        );
        assert_eq!(sent(&mut held), ["release Left 2"]);

        // A right click, sent whole: nothing left; the wheel holds nothing.
        for kind in [
            DispatchMouseEventType::MousePressed,
            DispatchMouseEventType::MouseReleased,
            DispatchMouseEventType::MouseWheel,
        ] {
            held.mouse(&kind, AT, &MouseButton::Right, 1);
        }
        assert!(held.releases().is_empty());
    }
}
