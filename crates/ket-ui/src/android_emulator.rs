//! The Android Emulator tab: a running emulator's screen, drawn by ket from
//! the frames it streams, with clicks sent back as touches and typing as
//! keys. See `ket_core::android` for how the emulator is found, started and
//! driven — its public gRPC control API, nothing private.
//!
//! Unlike the iPhone Simulator tab there is no native view: the screen is an
//! image ket draws like any other, so menus cover it, splits hold it and the
//! keyboard never leaves ket. What it does share is the shape — a problem
//! screen with steps when there is nothing to show, a list of devices to
//! start when none is running, and the hardware buttons on the same chords.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyElement, Bounds, Context, Entity, MouseButton, ObjectFit, Pixels, Point, Render,
    RenderImage, SharedString, StyledImage, Window, canvas, div, img, prelude::*, px,
};
use ket_core::android::{self, Button, Device, Frame, Running, Status};
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose as _;
use crate::handset::{Model, Stage, Turn, booting, waiting};
use crate::paint::paint;
use crate::tabs::{PaneId, Tab, TabKind};
use crate::ui::button::button;
use crate::ui::chip::caption;
use crate::ui::dialog;
use crate::ui::icon::{Icon, icon};
use crate::ui::row::sized_row;
use crate::ui::{CAPTION, RADIUS_MD};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// The one emulator tab in this window.
pub(crate) struct AndroidHandle {
    state: State,
    /// Which search or start this is, so a stale answer is dropped.
    generation: u64,
    /// The emulator this tab started, stopped when the tab closes. One found
    /// running is left running.
    started: Option<Running>,
    /// The phone drawn while the tab waits, and how far it has turned.
    turn: Cell<Turn>,
}

enum State {
    Looking,
    Starting(SharedString),
    /// Shown, by a view of its own — see [`AndroidScreen`].
    Attached(Rc<Device>, Entity<AndroidScreen>),
    Failed(Problem),
}

/// The device's screen, a view of its own and drawn cached: a new frame —
/// forty a second while something moves — redraws this and nothing else,
/// where notifying the shell would lay out the whole window each time.
pub(crate) struct AndroidScreen {
    device: Rc<Device>,
    theme: Theme,
    /// The frame on screen as gpui draws it, and which frame that was.
    image: Option<(Arc<Frame>, Arc<RenderImage>)>,
    /// Where the screen was last drawn, for placing clicks on it.
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Whether a click is down on the screen — a finger on the glass.
    pressed: bool,
}

/// Why nothing is on screen, and what to do: the iPhone tab's shape.
#[derive(Clone)]
struct Problem {
    title: SharedString,
    why: SharedString,
    steps: Vec<SharedString>,
    /// Virtual devices to start, when none is running.
    devices: Vec<String>,
    detail: Option<SharedString>,
}

impl Problem {
    fn new(title: &str, why: &str) -> Self {
        Self {
            title: SharedString::new(title),
            why: SharedString::new(why),
            steps: Vec::new(),
            devices: Vec::new(),
            detail: None,
        }
    }

    fn steps(mut self, steps: &[&str]) -> Self {
        self.steps = steps.iter().map(|step| SharedString::new(*step)).collect();
        self
    }

    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// What looking for an emulator found.
enum Found {
    Attached(Device),
    Nothing(Problem),
}

/// Finds a running emulator and connects to it, or says why there is none.
/// Blocks; run it in the background.
fn find() -> Found {
    let sdk = match android::sdk() {
        Ok(sdk) => sdk,
        Err(error) => return Found::Nothing(missing(&error)),
    };
    // The newest first; a discovery file left by one that has gone fails to
    // connect and the next is tried.
    for running in android::running() {
        if let Ok(device) = Device::connect(&running) {
            return Found::Attached(device);
        }
    }
    match sdk.devices() {
        Ok(devices) if devices.is_empty() => Found::Nothing(missing(&android::Error::NoDevices)),
        Ok(devices) => Found::Nothing(Problem {
            devices,
            ..Problem::new(
                "No Android device is running",
                "Start one here and it runs without a window of its own, in this tab.",
            )
        }),
        Err(error) => Found::Nothing(
            Problem::new(
                "Couldn't list your Android devices",
                "The emulator didn't answer when ket asked which devices you have.",
            )
            .steps(&[
                "Open Android Studio, then Device Manager, and check your devices are there.",
                "Come back here and click Check again.",
            ])
            .detail(error.to_string()),
        ),
    }
}

/// What to do about a missing SDK, emulator or device.
fn missing(error: &android::Error) -> Problem {
    match error {
        android::Error::NoSdk => Problem::new(
            "The Android emulator needs Android Studio",
            "The emulator comes with Android Studio's SDK, and none was found on this Mac.",
        )
        .steps(&[
            "Install Android Studio from developer.android.com/studio.",
            "Open it once and let it download the SDK and the emulator.",
            "Come back here and click Check again.",
        ]),
        android::Error::NoEmulator(_) => Problem::new(
            "The Android SDK has no emulator",
            "The SDK is installed, but the emulator isn't part of it yet.",
        )
        .steps(&[
            "In Android Studio, open Settings, then Languages & Frameworks, then Android SDK.",
            "Under SDK Tools, tick Android Emulator and click Apply.",
            "Come back here and click Check again.",
        ]),
        android::Error::NoDevices => Problem::new(
            "No Android devices yet",
            "The emulator is installed, but it has no virtual device to run.",
        )
        .steps(&[
            "In Android Studio, open Device Manager and click Create Virtual Device.",
            "Pick a phone and a system image, and finish.",
            "Come back here and click Check again.",
        ]),
        android::Error::Failed(why) => Problem::new(
            "Couldn't reach the Android emulator",
            "Something went wrong talking to it.",
        )
        .steps(&["Click Check again."])
        .detail(why.clone()),
    }
}

impl Shell {
    /// Opens the emulator tab in `pane`, or brings the one that is open to
    /// the front.
    pub(crate) fn open_android_tab_in(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(worktree) = self
            .spaces
            .iter()
            .find_map(|(id, space)| space.has_android().then(|| id.clone()))
        {
            if self.selected_id().as_ref() != Some(&worktree)
                && let Some(selection) = self.locate(&worktree)
            {
                self.select(selection, cx);
            }
            if let Some(space) = self.spaces.get_mut(&worktree) {
                space.focus_android();
            }
            cx.notify();
            return;
        }
        let Some(worktree) = self.selected_id() else {
            return;
        };
        let opened = self.spaces.get_mut(&worktree).is_some_and(|space| {
            space.open_tab_in(
                pane,
                Tab {
                    title: "Android".into(),
                    renamed: false,
                    pinned: false,
                    kind: TabKind::Android,
                },
            )
        });
        if opened {
            self.android = Some(AndroidHandle {
                state: State::Looking,
                generation: 0,
                started: None,
                turn: Cell::default(),
            });
            self.find_android(cx);
        }
    }

    /// Looks for a running emulator again.
    fn find_android(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.android.as_mut() else {
            return;
        };
        handle.state = State::Looking;
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        handle.generation = generation;
        cx.notify();
        let work = cx.background_executor().spawn(async { find() });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let found = work.await;
            let _ = shell.update(cx, |shell, cx| match found {
                Found::Attached(device) => shell.attach_android(generation, device, None, cx),
                Found::Nothing(problem) => {
                    if let Some(handle) = shell.android.as_mut()
                        && handle.generation == generation
                    {
                        handle.state = State::Failed(problem);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Boots `avd` headless and shows it once it is reachable.
    fn start_android(&mut self, avd: String, cx: &mut Context<Self>) {
        let Some(handle) = self.android.as_mut() else {
            return;
        };
        let name = avd.replace('_', " ");
        handle.state = State::Starting(name.clone().into());
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        handle.generation = generation;
        cx.notify();
        let work = cx.background_executor().spawn(async move {
            let sdk = android::sdk()?;
            let running = sdk.boot(&avd)?;
            let device = Device::connect(&running)?;
            Ok::<_, android::Error>((running, device))
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let started = work.await;
            let _ = shell.update(cx, |shell, cx| match started {
                Ok((running, device)) => {
                    shell.attach_android(generation, device, Some(running), cx);
                }
                Err(error) => {
                    if let Some(handle) = shell.android.as_mut()
                        && handle.generation == generation
                    {
                        handle.state = State::Failed(
                            Problem::new(
                                &format!("Couldn't start {name}"),
                                "The emulator didn't finish starting this device.",
                            )
                            .steps(&[
                                "Start it from Android Studio's Device Manager instead.",
                                "Once it is running, click Check again here.",
                            ])
                            .detail(error.to_string()),
                        );
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Shows `device`, and keeps the tab in step with its frames and its
    /// status for as long as it is the one on screen.
    fn attach_android(
        &mut self,
        generation: u64,
        device: Device,
        started: Option<Running>,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.android.as_mut() else {
            // The tab closed while it started: stop what nobody is watching.
            if let Some(running) = started {
                cx.background_executor()
                    .spawn(async move { running.stop() })
                    .detach();
            }
            return;
        };
        if handle.generation != generation {
            return;
        }
        if started.is_some() {
            handle.started = started;
        }
        let device = Rc::new(device);
        let mut frames = device.frames();
        let mut status = device.status();
        let theme = self.theme;
        let screen = cx.new(|_| AndroidScreen {
            device: device.clone(),
            theme,
            image: None,
            drawn: Rc::new(Cell::new(None)),
            pressed: false,
        });
        let watched = screen.downgrade();
        if let Some(handle) = self.android.as_mut() {
            handle.state = State::Attached(device, screen);
        }
        cx.notify();
        // A frame, or Android coming up, redraws the screen alone; the
        // device going away is the tab's to show. So is everything up to the
        // first frame, which the tab draws over the screen.
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let mut waiting = true;
            loop {
                let closed = tokio::select! {
                    changed = frames.changed() => changed.is_err(),
                    changed = status.changed() => changed.is_err(),
                };
                let why = match &*status.borrow() {
                    Status::Ended(why) => Some(why.clone()),
                    _ => None,
                };
                if !closed && why.is_none() {
                    if watched.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                    if waiting {
                        waiting = frames.borrow().is_none();
                        let _ = shell.update(cx, |_, cx| cx.notify());
                    }
                    continue;
                }
                let _ = shell.update(cx, |shell, cx| {
                    if let Some(handle) = shell.android.as_mut()
                        && handle.generation == generation
                    {
                        handle.started = None;
                        handle.state = State::Failed(
                            Problem::new(
                                "The Android device stopped",
                                "The emulator closed, or stopped answering ket.",
                            )
                            .steps(&["Click Check again, or start it again from the list."])
                            .detail(why.unwrap_or_default()),
                        );
                        cx.notify();
                    }
                });
                break;
            }
        })
        .detach();
    }

    /// Stops the emulator the tab started, when the tab has gone.
    pub(crate) fn prune_android(&mut self) {
        if self.spaces.values().any(|space| space.has_android()) {
            return;
        }
        if let Some(handle) = self.android.take()
            && let Some(running) = handle.started
        {
            std::thread::spawn(move || {
                if let Err(error) = running.stop() {
                    tracing::warn!(%error, "could not stop the Android emulator");
                }
            });
        }
    }

    /// The emulator's own chords, and typing, while its pane has focus:
    /// ⇧⌘H Home, ⌘⌫ Back, ⌃⌘H Overview, ⌘L Power, ⌘↑ / ⌘↓ volume — the
    /// iPhone tab's, where it has them. `false` for anything else, which
    /// goes on to ket: ⌘W still closes the tab.
    pub(crate) fn android_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.space().is_some_and(|space| space.android_focused()) {
            return false;
        }
        let Some(AndroidHandle {
            state: State::Attached(device, _),
            ..
        }) = self.android.as_ref()
        else {
            return false;
        };
        let m = &keystroke.modifiers;
        let key = keystroke.key.as_str();
        // ⌘S, as on the iPhone tab: a screenshot sent to an agent.
        if m.platform && key == "s" && !m.shift && !m.control && !m.alt {
            self.capture_device(crate::device_capture::DeviceKind::Android, cx);
            return true;
        }
        if m.platform {
            let button = match (key, m.shift, m.control, m.alt) {
                ("h", true, false, false) => Button::Home,
                ("h", false, true, false) => Button::Overview,
                ("backspace", false, false, false) => Button::Back,
                ("l", false, false, false) => Button::Power,
                ("up", false, false, false) => Button::VolumeUp,
                ("down", false, false, false) => Button::VolumeDown,
                _ => return false,
            };
            device.press(button);
            return true;
        }
        if m.control || m.function {
            return false;
        }
        // Named keys by the names a browser gives them, which the emulator
        // takes; everything printable as text.
        let named = match key {
            "backspace" => Some("Backspace"),
            "delete" => Some("Delete"),
            "enter" => Some("Enter"),
            "tab" => Some("Tab"),
            "escape" => Some("Escape"),
            "left" => Some("ArrowLeft"),
            "right" => Some("ArrowRight"),
            "up" => Some("ArrowUp"),
            "down" => Some("ArrowDown"),
            _ => None,
        };
        if let Some(named) = named {
            device.key(named);
            return true;
        }
        match keystroke.key_char.as_deref() {
            Some(text) if !text.is_empty() && text.chars().all(|c| !c.is_control()) => {
                device.text(text);
                true
            }
            _ => false,
        }
    }

    /// How to take the running device's screenshot, when one is running.
    pub(crate) fn android_shot(&self) -> Option<crate::device_capture::ShotSource> {
        match &self.android.as_ref()?.state {
            State::Attached(device, _) => Some(crate::device_capture::ShotSource::Android(
                device.screenshot(),
            )),
            _ => None,
        }
    }

    /// Presses one of the device's hardware buttons from the strip.
    fn press_android(&mut self, button: Button) {
        if let Some(AndroidHandle {
            state: State::Attached(device, _),
            ..
        }) = self.android.as_ref()
        {
            device.press(button);
        }
    }

    /// The tab's content: the screen, or why there is none.
    pub(crate) fn android_pane(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(handle) = self.android.as_mut() else {
            return message(&theme, "This emulator tab is no longer available.");
        };
        match &handle.state {
            State::Looking => waiting(
                &handle.turn,
                Model::Android,
                Stage::Looking,
                "Looking for an Android device",
                "running emulators",
                &theme,
            ),
            State::Starting(name) => waiting(
                &handle.turn,
                Model::Android,
                Stage::Starting,
                format!("Starting {name}"),
                booting(&handle.turn),
                &theme,
            ),
            State::Failed(problem) => {
                let problem = problem.clone();
                problem_view(&problem, &theme, cx)
            }
            State::Attached(device, screen) => {
                // Until its first frame the phone is still on screen: still
                // starting while Android boots, then reaching the screen.
                let wait = device.frames().borrow().is_none().then(|| {
                    if *device.status().borrow() == Status::Booting {
                        waiting(
                            &handle.turn,
                            Model::Android,
                            Stage::Starting,
                            format!("Starting {}", device.name),
                            booting(&handle.turn),
                            &theme,
                        )
                    } else {
                        waiting(
                            &handle.turn,
                            Model::Android,
                            Stage::Connecting,
                            "Connecting to the screen",
                            format!("{} · Android", device.name),
                            &theme,
                        )
                    }
                });
                let mut style = gpui::StyleRefinement::default();
                style.size.width = Some(gpui::relative(1.).into());
                style.size.height = Some(gpui::relative(1.).into());
                let line = crate::device_capture::DeviceLine {
                    name: device.name.clone().into(),
                    os: Some("Android".into()),
                };
                let screen = gpui::AnyView::from(screen.clone()).cached(style);
                let controls = vec![
                    self.strip_control(
                        "android-back",
                        Icon::ArrowLeft,
                        "Back  ⌘⌫",
                        |shell, _| shell.press_android(Button::Back),
                        cx,
                    ),
                    self.strip_control(
                        "android-home",
                        Icon::Smartphone,
                        "Home  ⇧⌘H",
                        |shell, _| shell.press_android(Button::Home),
                        cx,
                    ),
                    self.strip_control(
                        "android-overview",
                        Icon::Layers,
                        "Overview  ⌃⌘H",
                        |shell, _| shell.press_android(Button::Overview),
                        cx,
                    ),
                    self.strip_control(
                        "android-power",
                        Icon::Lock,
                        "Power  ⌘L",
                        |shell, _| shell.press_android(Button::Power),
                        cx,
                    ),
                ];
                let capture = self.capture_button(crate::device_capture::DeviceKind::Android, cx);
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(crate::device_capture::device_strip(
                        line, controls, capture, &theme,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(screen)
                            .children(wait.map(|wait| {
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .bg(paint(theme.surface))
                                    .child(wait)
                            })),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for AndroidScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let device = self.device.clone();
        let frame = device.frames().borrow().clone();
        let Some(frame) = frame else {
            // Frames are asked for once the pane has a size. Until one comes
            // the tab draws its phone over this.
            return div()
                .flex()
                .size_full()
                .relative()
                .child(frame_sizer(device, self.drawn.clone()))
                .into_any_element();
        };
        // A new frame is a new image; the one it replaces is freed from the
        // GPU, or a streaming screen fills it.
        let current = match &self.image {
            Some((shown, image)) if Arc::ptr_eq(shown, &frame) => image.clone(),
            _ => {
                let image = render_image(&frame);
                if let Some((_, old)) = self.image.replace((frame.clone(), image.clone())) {
                    let _ = window.drop_image(old);
                }
                image
            }
        };
        let drawn = self.drawn.clone();
        let native = device.native;
        let aspect = (frame.width, frame.height);
        let at = move |position: Point<Pixels>| -> Option<(i32, i32)> {
            to_device(drawn.get()?, aspect, native, position)
        };
        let (down, moved, up) = (at.clone(), at.clone(), at);
        div()
            .id("android-screen")
            .flex()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(paint(theme.surface))
            // Not stopped: the click also makes this the focused pane, which
            // is what sends typing to the device.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                    if let Some((x, y)) = down(event.position) {
                        this.pressed = true;
                        this.device.touch(x, y, true);
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, _| {
                    if this.pressed
                        && let Some((x, y)) = moved(event.position)
                    {
                        this.device.touch(x, y, true);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _, _| {
                    if this.pressed {
                        this.pressed = false;
                        let (x, y) = up(event.position).unwrap_or((0, 0));
                        this.device.touch(x, y, false);
                    }
                }),
            )
            .child(img(current).object_fit(ObjectFit::Contain).size_full())
            .child(frame_sizer(device, self.drawn.clone()))
            .into_any_element()
    }
}

/// Measures the pane each frame: remembers where the screen is drawn, for
/// placing clicks, and asks the emulator for frames that size in the
/// display's own pixels — no bigger than it needs, which keeps a stream
/// light.
fn frame_sizer(device: Rc<Device>, drawn: Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            drawn.set(Some(bounds));
            let scale = window.scale_factor();
            let width = (f32::from(bounds.size.width) * scale).round().max(0.0) as u32;
            let height = (f32::from(bounds.size.height) * scale).round().max(0.0) as u32;
            device.frame_size(width, height);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// Where a click at `position` lands on the device, in its own pixels: the
/// frame is drawn contained in `bounds`, centred, at the frame's shape.
/// `None` outside the screen.
fn to_device(
    bounds: Bounds<Pixels>,
    frame: (u32, u32),
    native: (u32, u32),
    position: Point<Pixels>,
) -> Option<(i32, i32)> {
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (fw, fh) = (frame.0 as f32, frame.1 as f32);
    if bw <= 0.0 || bh <= 0.0 || fw <= 0.0 || fh <= 0.0 {
        return None;
    }
    let scale = (bw / fw).min(bh / fh);
    let (w, h) = (fw * scale, fh * scale);
    let left = f32::from(bounds.origin.x) + (bw - w) / 2.0;
    let top = f32::from(bounds.origin.y) + (bh - h) / 2.0;
    let (x, y) = (f32::from(position.x) - left, f32::from(position.y) - top);
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return None;
    }
    let dx = (x / w * native.0 as f32).round() as i32;
    let dy = (y / h * native.1 as f32).round() as i32;
    Some((
        dx.clamp(0, native.0 as i32 - 1),
        dy.clamp(0, native.1 as i32 - 1),
    ))
}

/// A frame as gpui draws it: BGRA already, in one image frame.
fn render_image(frame: &Frame) -> Arc<RenderImage> {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra.clone())
        .unwrap_or_else(|| image::RgbaImage::new(1, 1));
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

fn message(theme: &Theme, text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .px_6()
        .text_sm()
        .text_color(paint(theme.text.dim))
        .child(text.into())
        .into_any_element()
}

/// A problem, centred: what is wrong, why, the steps out, and — when no
/// device is running — the devices to start.
fn problem_view(problem: &Problem, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let steps = problem.steps.iter().enumerate().map(|(index, step)| {
        div()
            .flex()
            .gap(px(10.0))
            .child(
                div()
                    .flex_none()
                    .w(px(16.0))
                    .font_family(crate::fonts::chrome())
                    .text_color(paint(theme.text.dim))
                    .child(format!("{}.", index + 1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(paint(theme.text.primary))
                    .child(step.clone()),
            )
    });
    let devices = (!problem.devices.is_empty()).then(|| {
        let rows = problem.devices.iter().map(|avd| {
            let start = avd.clone();
            sized_row(
                SharedString::from(format!("android-device-{avd}")),
                false,
                px(36.0),
                theme,
            )
            .child(icon(Icon::Smartphone, paint(theme.text.dim)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(avd.replace('_', " ")),
            )
            .child(
                button(SharedString::from(format!("android-start-{avd}")), "Start")
                    .primary()
                    .small()
                    .leading(Icon::Play)
                    .render(theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_android(start.clone(), cx);
                    })),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(4.0))
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(paint(theme.border))
            .bg(paint(theme.panel))
            .children(rows)
    });
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .p_6()
        .child(
            div()
                .prose()
                .flex()
                .flex_col()
                .gap(px(18.0))
                .w_full()
                .max_w(px(460.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(dialog::header(problem.title.clone(), theme))
                        .child(dialog::body(problem.why.clone(), theme)),
                )
                .children(
                    (!problem.steps.is_empty())
                        .then(|| div().flex().flex_col().gap(px(12.0)).children(steps)),
                )
                .children(devices)
                .child(
                    div().child(
                        button("android-check-again", "Check again")
                            .leading(Icon::Refresh)
                            .render(theme)
                            .on_click(cx.listener(|this, _, _, cx| this.find_android(cx))),
                    ),
                )
                .children(problem.detail.clone().map(|detail| {
                    div()
                        .text_size(CAPTION)
                        .child(caption(format!("Details: {detail}"), theme))
                })),
        )
        .into_any_element()
}
