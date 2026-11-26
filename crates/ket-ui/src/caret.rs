//! The blink behind every text caret in the shell — one for all of them, so
//! two fields never blink out of step.

use gpui::Context;

use crate::Shell;

/// The blink behind every text caret in the shell.
///
/// A caret that does not blink reads as a rule drawn between two characters
/// rather than as an insertion point.
#[derive(Default)]
pub(crate) struct Caret {
    /// Whether the caret is in its visible phase.
    pub(crate) visible: bool,
    /// The timer flipping the phase. `None` while no field has the keyboard.
    task: Option<gpui::Task<()>>,
}

/// How long each phase of the caret's blink lasts: macOS's own interval, and
/// the one the terminal cursor already blinks at.
const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(530);

impl Shell {
    /// Starts or stops the blink to match.
    pub(crate) fn caret_blink(&mut self, on: bool, cx: &mut Context<Self>) {
        match (on, self.caret.task.is_some()) {
            (true, false) => {
                self.caret.visible = true;
                self.caret.task = Some(cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
                    loop {
                        cx.background_executor().timer(CARET_BLINK).await;
                        let alive = this.update(cx, |shell, cx| {
                            shell.caret.visible = !shell.caret.visible;
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                }));
            }
            (false, true) => {
                self.caret.task = None;
                self.caret.visible = true;
            }
            _ => {}
        }
    }

    /// Shows the caret and starts its phase over — what typing does to a caret
    /// everywhere else, so a character never lands during the half of the
    /// blink where the caret is not drawn.
    pub(crate) fn caret_wake(&mut self, cx: &mut Context<Self>) {
        self.caret.visible = true;
        if self.caret.task.is_some() {
            self.caret.task = None;
            self.caret_blink(true, cx);
        }
    }
}
