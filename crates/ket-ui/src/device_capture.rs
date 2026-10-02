//! Screenshot to agent: the strip over a device tab, the capture behind its
//! button, and the hand-off to the Quick prompt, which sends the picture to
//! an agent in the project with whatever is written beside it.
//!
//! The iPhone's screen is a native view that draws above everything ket
//! draws, so nothing can float over it: the controls sit in a strip above
//! the device instead, the same for the Android tab so the two read alike.
//!
//! The picture comes straight from the device — `simctl io … screenshot` for
//! the iPhone, the emulator's own `getScreenshot` for Android — at the
//! device's full size, with no window chrome and no screen-recording
//! permission. It is written to ket's cache (see `ket_core::capture`) and
//! handed to the agent as a pasted path, which the agents attach as an image.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use gpui::{AnyElement, Context, Div, Pixels, SharedString, div, prelude::*, px};
use ket_core::id::WorktreeId;
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose as _;
use crate::paint::paint;
use crate::ui::button::{button, icon_button};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::toast::Tone;
use crate::ui::tooltip::{Side, tooltip};
use crate::ui::{CAPTION, LABEL};

/// The strip's height: the style guide's strip.
const STRIP_H: Pixels = px(44.0);

/// How long the emulator is given to answer for a screenshot.
const ANDROID_SHOT: Duration = Duration::from_secs(10);

/// Which device tab a screenshot is taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceKind {
    Iphone,
    Android,
}

/// How a screenshot is to be taken, found on the UI thread and taken off it.
pub(crate) enum ShotSource {
    /// `simctl io <udid> screenshot <path>`.
    Iphone { simctl: PathBuf, udid: String },
    /// The emulator's answer to a screenshot it has been asked for.
    Android(Receiver<Result<Vec<u8>, String>>),
}

/// What the strip says about the device on its left.
pub(crate) struct DeviceLine {
    pub(crate) name: SharedString,
    /// "iOS 26.0", where it is known.
    pub(crate) os: Option<SharedString>,
}

/// The strip across the top of a device tab: the device on the left, its
/// hardware buttons and the screenshot button on the right.
pub(crate) fn device_strip(
    line: DeviceLine,
    controls: Vec<AnyElement>,
    capture: AnyElement,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .h(STRIP_H)
        .pl(px(14.0))
        .pr(px(10.0))
        .border_b_1()
        .border_color(paint(t.rule))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_w_0()
                .child(sized_icon(Icon::Smartphone, px(14.0), paint(t.text.dim)))
                .child(
                    div()
                        .text_size(LABEL)
                        .text_color(paint(t.text.primary))
                        .truncate()
                        .child(line.name),
                )
                .children(line.os.map(|os| {
                    div()
                        .prose()
                        .flex_none()
                        .text_size(LABEL)
                        .text_color(paint(t.text.dim))
                        .child(os)
                }))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.0))
                        .ml(px(4.0))
                        .text_size(CAPTION)
                        .text_color(paint(t.status.running))
                        .child(
                            div()
                                .size(px(6.0))
                                .rounded_full()
                                .bg(paint(t.status.running)),
                        )
                        .child("booted"),
                ),
        )
        .child(div().flex_grow())
        .children(controls)
        .child(
            div()
                .flex_none()
                .w(px(1.0))
                .h(px(16.0))
                .mx(px(6.0))
                .bg(paint(t.border)),
        )
        .child(capture)
}

impl Shell {
    /// One hardware button in a device strip, with its name and chord in a
    /// tooltip — to the left, inside the strip: one under it would sit over
    /// the iPhone's screen, which draws over it.
    pub(crate) fn strip_control(
        &self,
        id: &'static str,
        which: Icon,
        tip: &'static str,
        press: impl Fn(&mut Shell, &mut Context<Shell>) + 'static,
        cx: &mut Context<Shell>,
    ) -> AnyElement {
        let t = &self.theme;
        tooltip(
            id,
            icon_button(id, which)
                .bare()
                .small()
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    press(this, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .into_any_element(),
            tip,
            Side::Left,
            self.device_strip_hovered == Some(id),
            t,
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.device_strip_hovered = Some(id);
            } else if this.device_strip_hovered == Some(id) {
                this.device_strip_hovered = None;
            }
            cx.notify();
        }))
        .into_any_element()
    }

    /// The strip's screenshot button.
    pub(crate) fn capture_button(&self, kind: DeviceKind, cx: &mut Context<Shell>) -> AnyElement {
        button("device-capture", "Screenshot to agent")
            .small()
            .leading(Icon::Camera)
            .hint("⌘S")
            .loading_if(self.capturing_device, "Taking screenshot…")
            .render(&self.theme)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.capture_device(kind, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }

    /// The worktree whose tabs hold the device tab of `kind`: where its
    /// screenshot goes unless somebody picks somewhere else.
    pub(crate) fn device_worktree(&self, kind: DeviceKind) -> Option<WorktreeId> {
        self.spaces.iter().find_map(|(id, space)| {
            let holds = match kind {
                #[cfg(target_os = "macos")]
                DeviceKind::Iphone => space.has_ios_simulator(),
                #[cfg(not(target_os = "macos"))]
                DeviceKind::Iphone => false,
                DeviceKind::Android => space.has_android(),
            };
            holds.then(|| id.clone())
        })
    }

    /// Takes a screenshot of the device in the `kind` tab and opens the Quick
    /// prompt with it — or adds it to the one already open.
    pub(crate) fn capture_device(&mut self, kind: DeviceKind, cx: &mut Context<Self>) {
        if self.capturing_device {
            return;
        }
        let source = match kind {
            #[cfg(target_os = "macos")]
            DeviceKind::Iphone => self.ios_simulator_shot(),
            #[cfg(not(target_os = "macos"))]
            DeviceKind::Iphone => None,
            DeviceKind::Android => self.android_shot(),
        };
        let Some(source) = source else {
            self.toast(
                Tone::Info,
                "Start the device first, then take a screenshot",
                cx,
            );
            return;
        };
        self.capturing_device = true;
        let worktree = self.device_worktree(kind);
        let shot = cx.background_executor().spawn(async move { take(source) });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = shot.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.capturing_device = false;
                match result {
                    Ok(path) => {
                        #[cfg(target_os = "macos")]
                        if kind == DeviceKind::Iphone {
                            shell.set_ios_last_shot(path.clone());
                        }
                        shell.open_screenshot_prompt(path, kind, worktree, cx);
                    }
                    Err(why) => shell.toast(
                        Tone::Error,
                        format!("Couldn't take a screenshot: {why}"),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// Takes the screenshot `source` describes and says where it was written.
/// Blocks: off the UI thread.
fn take(source: ShotSource) -> Result<PathBuf, String> {
    match source {
        #[cfg(target_os = "macos")]
        ShotSource::Iphone { simctl, udid } => {
            let path = ket_core::capture::new_screenshot().map_err(|e| e.to_string())?;
            let target = path.to_string_lossy().into_owned();
            crate::ios_simulator::command_text(&simctl, &["io", &udid, "screenshot", &target])?;
            Ok(path)
        }
        #[cfg(not(target_os = "macos"))]
        ShotSource::Iphone { .. } => Err("the iPhone Simulator runs only on a Mac".to_owned()),
        ShotSource::Android(answer) => {
            let png = answer
                .recv_timeout(ANDROID_SHOT)
                .map_err(|_| "the emulator didn't answer".to_owned())??;
            if png.is_empty() {
                return Err("the emulator sent an empty picture".to_owned());
            }
            ket_core::capture::save_screenshot(&png).map_err(|e| e.to_string())
        }
    }
}
