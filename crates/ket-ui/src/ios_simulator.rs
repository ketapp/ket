//! Proof of concept for embedding an already-booted iPhone Simulator.
//!
//! SimulatorKit and CoreSimulator are private Apple frameworks. The safe half
//! of this module owns discovery and tab lifecycle; `private` is the sole
//! adapter allowed to load those frameworks or message their Objective-C API.
//! An iPhone is booted only from the tab's own Start button; nothing is
//! created, shut down, installed to or launched here.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{AnyElement, ClipboardItem, Context, SharedString, Window, canvas, div, prelude::*, px};
use ket_core::theme::Theme;
use serde::Deserialize;

use crate::Shell;
use crate::device_capture::DeviceKind;
use crate::fonts::Prose as _;
use crate::handset::{Model, Stage, Turn, booting, waiting};
use crate::paint::paint;
use crate::tabs::{PaneId, Tab, TabKind};
use crate::ui::button::{Button, button};
use crate::ui::chip::{caption, tag};
use crate::ui::dialog;
use crate::ui::field::field;
use crate::ui::icon::{Icon, icon};
use crate::ui::row::sized_row;
use crate::ui::{LABEL, RADIUS_MD};
use private::Control;

const APPLE_DEVELOPER_TEAM: &str = "59GAB85EFG";
/// The SimulatorKit builds ket's private Swift calls are known to match,
/// each with the Xcode that ships it. Every symbol `private` loads, with the
/// same mangling, is in both (checked 09-29). A new Xcode is a new row, once
/// it has been checked the same way.
const SUPPORTED_SIMULATOR_KITS: &[(&str, &str)] = &[("1005.2", "27.0"), ("955.7", "26.6")];
/// CoreSimulator is installed once per Mac — by the newest Xcode — not per
/// Xcode, so it has a list of its own.
const SUPPORTED_CORE_SIMULATORS: &[&str] = &["1171.7"];
/// Where SimulatorKit sits inside an Xcode's Contents: Xcode 27 moved it to
/// SharedFrameworks; Xcode 26 keeps it in the developer directory.
const SIMULATOR_KITS: &[&str] = &[
    "SharedFrameworks/SimulatorKit.framework/Versions/A/SimulatorKit",
    "Developer/Library/PrivateFrameworks/SimulatorKit.framework/Versions/A/SimulatorKit",
];
const CORE_SIMULATOR: &str =
    "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Versions/A/CoreSimulator";
/// Xcode's page in the Mac App Store.
const XCODE_APP_STORE: &str = "macappstore://apps.apple.com/app/id497799835";
static NEXT_DISCOVERY_GENERATION: AtomicU64 = AtomicU64::new(1);

/// State for the sole process-local simulator tab.
pub(crate) struct IosSimulatorHandle {
    state: State,
    shown: bool,
    discovery_generation: u64,
    /// The phone attached, for what is asked of it by name — a screenshot.
    device: Option<Discovery>,
    /// The last screenshot taken, drawn in the phone's place while a dialog
    /// over the tab has the native view hidden.
    last_shot: Option<PathBuf>,
    /// The phone drawn while the tab waits, and how far it has turned.
    turn: Cell<Turn>,
}

enum State {
    Discovering,
    /// Booting the named iPhone from the tab's Start button.
    Starting(SharedString),
    Ready(Discovery),
    Attached(Rc<private::NativeSimulator>),
    Failed(Problem),
}

/// Why the tab cannot show a phone, told as what to do about it. The error
/// underneath goes in `detail`, for a bug report; it is never the headline.
#[derive(Clone)]
struct Problem {
    title: SharedString,
    why: SharedString,
    steps: Vec<Step>,
    action: Option<Action>,
    /// Whether trying again could help. It cannot on an Intel Mac.
    retry: bool,
    detail: Option<SharedString>,
    /// The iPhones that could be started here, drawn as a list in place of
    /// steps. Boxed, as it is rare and the error path carries a `Problem`.
    picker: Option<Box<Picker>>,
}

/// The installed iPhones, the one used last first, and which is picked.
#[derive(Clone)]
struct Picker {
    phones: Vec<Phone>,
    picked: usize,
}

/// An installed iPhone the tab could start.
#[derive(Clone)]
struct Phone {
    boot: Boot,
    /// "iOS 26.0", from the runtime it is listed under.
    runtime: SharedString,
    /// Whether this is the one used most recently.
    last_used: bool,
}

#[derive(Clone)]
struct Step {
    text: SharedString,
    /// A command to run in Terminal, shown with a Copy button.
    command: Option<SharedString>,
}

/// The one thing a problem's primary button does.
#[derive(Clone)]
enum Action {
    GetXcode,
    OpenXcode,
    OpenSimulator,
}

#[derive(Clone)]
struct Boot {
    simctl: PathBuf,
    udid: String,
    name: SharedString,
}

impl Action {
    fn label(&self) -> SharedString {
        match self {
            Self::GetXcode => "Get Xcode".into(),
            Self::OpenXcode => "Open Xcode".into(),
            Self::OpenSimulator => "Open Simulator".into(),
        }
    }

    /// Opens what the button names.
    fn open(&self) {
        let args: &[&str] = match self {
            Self::GetXcode => &[XCODE_APP_STORE],
            Self::OpenXcode => &["-a", "Xcode"],
            Self::OpenSimulator => &["-a", "Simulator"],
        };
        if let Err(error) = Command::new("/usr/bin/open").args(args).status() {
            tracing::warn!(%error, "could not open {}", self.label());
        }
    }
}

impl Problem {
    fn new(title: &str, why: impl Into<SharedString>) -> Self {
        Self {
            title: SharedString::new(title),
            why: why.into(),
            steps: Vec::new(),
            action: None,
            retry: true,
            detail: None,
            picker: None,
        }
    }

    fn steps(mut self, steps: impl IntoIterator<Item = Step>) -> Self {
        self.steps.extend(steps);
        self
    }

    fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

fn step(text: &str) -> Step {
    Step {
        text: SharedString::new(text),
        command: None,
    }
}

fn run_in_terminal(text: &str, command: String) -> Step {
    Step {
        text: SharedString::new(text),
        command: Some(command.into()),
    }
}

const RETRY: &str = "Come back here and click Check again.";

#[derive(Clone)]
struct Discovery {
    developer_dir: PathBuf,
    core_simulator: PathBuf,
    simulator_kit: PathBuf,
    udid: String,
    name: String,
    /// "iOS 26.0", from the runtime it is listed under.
    os: SharedString,
}

#[derive(Deserialize)]
struct SimctlList {
    devices: HashMap<String, Vec<SimctlDevice>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimctlDevice {
    name: String,
    udid: String,
    state: String,
    device_type_identifier: String,
    #[serde(default)]
    is_available: bool,
    #[serde(default)]
    last_used_at: String,
}

impl Shell {
    /// Opens or focuses the one simulator tab allowed in this process.
    pub(crate) fn open_ios_simulator_tab_in(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(worktree) = self
            .spaces
            .iter()
            .find_map(|(id, space)| space.has_ios_simulator().then(|| id.clone()))
        {
            if self.selected_id().as_ref() != Some(&worktree)
                && let Some(selection) = self.locate(&worktree)
            {
                self.select(selection, cx);
            }
            if let Some(space) = self.spaces.get_mut(&worktree) {
                space.focus_ios_simulator();
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
                    title: "iPhone Simulator".into(),
                    renamed: false,
                    pinned: false,
                    kind: TabKind::IosSimulator,
                },
            )
        });
        if opened {
            self.ios_simulator = Some(IosSimulatorHandle {
                state: State::Discovering,
                shown: false,
                discovery_generation: 0,
                device: None,
                last_shot: None,
                turn: Cell::default(),
            });
            self.discover_ios_simulator(cx);
        }
    }

    fn discover_ios_simulator(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.ios_simulator.as_mut() {
            handle.state = State::Discovering;
            handle.shown = false;
            handle.discovery_generation = NEXT_DISCOVERY_GENERATION.fetch_add(1, Ordering::Relaxed);
        } else {
            return;
        }
        let generation = self
            .ios_simulator
            .as_ref()
            .map_or(0, |handle| handle.discovery_generation);
        let discovery = cx.background_executor().spawn(async { discover() });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = discovery.await;
            let _ = shell.update(cx, |shell, cx| {
                let Some(handle) = shell.ios_simulator.as_mut() else {
                    return;
                };
                if handle.discovery_generation != generation {
                    return;
                }
                handle.state = match result {
                    Ok(found) => State::Ready(found),
                    Err(problem) => State::Failed(problem),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Boots an iPhone, waits until it has finished starting, and then
    /// looks again — which finds it booted and attaches.
    fn start_ios_simulator(&mut self, boot: Boot, cx: &mut Context<Self>) {
        let Some(handle) = self.ios_simulator.as_mut() else {
            return;
        };
        handle.state = State::Starting(boot.name.clone());
        handle.shown = false;
        let generation = NEXT_DISCOVERY_GENERATION.fetch_add(1, Ordering::Relaxed);
        handle.discovery_generation = generation;
        cx.notify();
        let simctl = boot.simctl.clone();
        let udid = boot.udid.clone();
        // `bootstatus -b` boots the device if it is not running, and returns
        // only once it has finished — `boot` alone returns while it starts.
        let started = cx
            .background_executor()
            .spawn(async move { command_text(&simctl, &["bootstatus", &udid, "-b"]) });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = started.await;
            let _ = shell.update(cx, |shell, cx| {
                let Some(handle) = shell.ios_simulator.as_mut() else {
                    return;
                };
                if handle.discovery_generation != generation {
                    return;
                }
                match result {
                    Ok(_) => shell.discover_ios_simulator(cx),
                    Err(error) => {
                        handle.state = State::Failed(could_not_start(&boot.name, error));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn ensure_ios_simulator(&mut self, window: &mut Window) {
        let should_show = self
            .space()
            .is_some_and(|space| space.ios_simulator_active())
            && !self.browser_occluded();
        let Some(handle) = self.ios_simulator.as_mut() else {
            return;
        };
        let State::Ready(found) = &handle.state else {
            return;
        };
        let found = found.clone();
        handle.state = match private::NativeSimulator::attach(window, &found) {
            Ok(native) => {
                native.set_visible(should_show);
                handle.shown = should_show;
                handle.device = Some(found);
                State::Attached(Rc::new(native))
            }
            Err(error) => State::Failed(could_not_attach(error)),
        };
    }

    /// Drops the private native surface when its ephemeral tab disappears.
    pub(crate) fn prune_ios_simulator(&mut self) {
        let live = self.spaces.values().any(|space| space.has_ios_simulator());
        if !live {
            self.ios_simulator = None;
        }
    }

    /// Keeps the native child behind tabs and overlays that GPUI must draw.
    pub(crate) fn sync_ios_simulator_visibility(&mut self, cx: &mut Context<Self>) {
        let should_show = self
            .space()
            .is_some_and(|space| space.ios_simulator_active())
            && !self.browser_occluded();
        let Some(handle) = self.ios_simulator.as_mut() else {
            return;
        };
        if handle.shown == should_show {
            return;
        }
        if let State::Attached(native) = &handle.state {
            native.set_visible(should_show);
        }
        handle.shown = should_show;
        let _ = cx;
    }

    /// Simulator.app's hardware chords, while the phone has the keyboard:
    /// ⌘K on-screen keyboard, ⇧⌘K the Mac's keyboard, ⇧⌘H Home, ⌘L Lock,
    /// ⌥⌘H Siri, ⌘↑ / ⌘↓ volume, ⌘← / ⌘→ rotate. `false` for anything else,
    /// or when the phone does not have the keyboard, so the chord goes on to
    /// do what it does elsewhere — ⌘K the palette, notably.
    pub(crate) fn ios_simulator_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &keystroke.modifiers;
        if !m.platform || m.control || m.function {
            return false;
        }
        // ⌘S, as in Simulator.app — here a screenshot sent to an agent. Only
        // while the phone has the keyboard: beside an editor in a split, ⌘S
        // is still the editor's save.
        if keystroke.key == "s"
            && !m.shift
            && !m.alt
            && self
                .attached_ios_simulator()
                .is_some_and(private::NativeSimulator::has_keyboard)
        {
            self.capture_device(crate::device_capture::DeviceKind::Iphone, cx);
            return true;
        }
        let control = match (keystroke.key.as_str(), m.shift, m.alt) {
            ("k", false, false) => Control::SoftwareKeyboard,
            ("k", true, false) => Control::HardwareKeyboard,
            ("h", true, false) => Control::Home,
            ("h", false, true) => Control::Siri,
            ("l", false, false) => Control::Lock,
            ("up", false, false) => Control::VolumeUp,
            ("down", false, false) => Control::VolumeDown,
            ("left", false, false) => Control::RotateLeft,
            ("right", false, false) => Control::RotateRight,
            _ => return false,
        };
        let Some(handle) = self.ios_simulator.as_ref() else {
            return false;
        };
        let State::Attached(native) = &handle.state else {
            return false;
        };
        if !handle.shown || !native.has_keyboard() {
            return false;
        }
        if let Err(error) = native.control(control) {
            tracing::warn!(%error, ?control, "iPhone Simulator control failed");
            self.note = Some(format!("Could not {}: {error}", control.doing()).into());
        }
        if matches!(control, Control::RotateLeft | Control::RotateRight) {
            // SimDisplayView may take a moment to swap its size; fit it
            // again once it has, as well as on this frame.
            cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let _ = shell.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        true
    }

    /// Prevents render-time focus repair from stealing keys from SimulatorKit
    /// — only while the phone has them, so that with the phone beside a
    /// terminal in a split, ket's own focus still gets repaired.
    pub(crate) fn ios_simulator_owns_focus(&self) -> bool {
        self.attached_ios_simulator()
            .is_some_and(private::NativeSimulator::has_keyboard)
    }

    /// Takes the keyboard back from the phone, if it has it. Every click on
    /// something ket draws comes here: in a split the phone stays on
    /// screen, so hiding it — which also hands the keyboard back — never
    /// happens, and AppKit does not move first responder to GPUI's view on
    /// a click either.
    pub(crate) fn release_ios_simulator_keyboard(&self) -> bool {
        self.attached_ios_simulator()
            .is_some_and(private::NativeSimulator::release_keyboard)
    }

    /// After ket's focused pane moves from the keyboard: the phone takes
    /// the keyboard when its pane is the focused one, and gives it back
    /// otherwise.
    pub(crate) fn follow_ios_simulator_pane_focus(&self) {
        let Some(native) = self.attached_ios_simulator() else {
            return;
        };
        if self
            .space()
            .is_some_and(|space| space.ios_simulator_focused())
        {
            native.take_keyboard();
        } else {
            native.release_keyboard();
        }
    }

    /// How to take the attached phone's screenshot, when one is attached.
    pub(crate) fn ios_simulator_shot(&self) -> Option<crate::device_capture::ShotSource> {
        let handle = self.ios_simulator.as_ref()?;
        let State::Attached(_) = &handle.state else {
            return None;
        };
        let device = handle.device.as_ref()?;
        Some(crate::device_capture::ShotSource::Iphone {
            simctl: device.developer_dir.join("usr/bin/simctl"),
            udid: device.udid.clone(),
        })
    }

    /// Keeps `path` to draw in the phone's place while it is hidden.
    pub(crate) fn set_ios_last_shot(&mut self, path: PathBuf) {
        if let Some(handle) = self.ios_simulator.as_mut() {
            handle.last_shot = Some(path);
        }
    }

    /// Presses one of the phone's hardware buttons from the strip.
    fn press_ios_control(&mut self, control: Control, cx: &mut Context<Self>) {
        let Some(handle) = self.ios_simulator.as_ref() else {
            return;
        };
        let State::Attached(native) = &handle.state else {
            return;
        };
        if let Err(error) = native.control(control) {
            tracing::warn!(%error, ?control, "iPhone Simulator control failed");
            self.note = Some(format!("Could not {}: {error}", control.doing()).into());
        }
        if matches!(control, Control::RotateLeft | Control::RotateRight) {
            cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let _ = shell.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    fn attached_ios_simulator(&self) -> Option<&private::NativeSimulator> {
        let handle = self.ios_simulator.as_ref()?;
        match &handle.state {
            State::Attached(native) if handle.shown => Some(native),
            _ => None,
        }
    }

    /// Renders status UI and reserves the rectangle occupied by SimDisplayView.
    pub(crate) fn ios_simulator_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_ios_simulator(window);
        let theme = self.theme;
        let Some(handle) = self.ios_simulator.as_ref() else {
            return message(&theme, "This simulator tab is no longer available.");
        };
        match &handle.state {
            State::Discovering => waiting(
                &handle.turn,
                Model::Iphone,
                Stage::Looking,
                "Looking for an iPhone",
                "simctl · booted simulators",
                &theme,
            ),
            State::Starting(name) => waiting(
                &handle.turn,
                Model::Iphone,
                Stage::Starting,
                format!("Starting {name}"),
                booting(&handle.turn),
                &theme,
            ),
            State::Ready(found) => waiting(
                &handle.turn,
                Model::Iphone,
                Stage::Connecting,
                "Connecting to the screen",
                format!("{} · {}", found.name, found.os),
                &theme,
            ),
            State::Failed(problem) => problem_view(&problem.clone(), &theme, cx),
            State::Attached(native) => {
                let native = native.clone();
                let line = crate::device_capture::DeviceLine {
                    name: handle
                        .device
                        .as_ref()
                        .map(|device| SharedString::from(device.name.clone()))
                        .unwrap_or_else(|| "iPhone".into()),
                    os: handle.device.as_ref().map(|device| device.os.clone()),
                };
                // Hidden under the screenshot's own prompt, the phone leaves
                // a hole; the screenshot fills it. Under anything else — a
                // menu, the palette — an old picture would be a lie, so the
                // hole stays.
                let stand_in = (!handle.shown && self.quick_prompt_has_shots(DeviceKind::Iphone))
                    .then(|| handle.last_shot.clone())
                    .flatten();
                let controls = vec![
                    self.strip_control(
                        "ios-home",
                        Icon::Smartphone,
                        "Home  ⇧⌘H",
                        |shell, cx| shell.press_ios_control(Control::Home, cx),
                        cx,
                    ),
                    self.strip_control(
                        "ios-rotate",
                        Icon::RotateCw,
                        "Rotate  ⌘→",
                        |shell, cx| shell.press_ios_control(Control::RotateRight, cx),
                        cx,
                    ),
                    self.strip_control(
                        "ios-keyboard",
                        Icon::Keyboard,
                        "On-screen keyboard  ⌘K",
                        |shell, cx| shell.press_ios_control(Control::SoftwareKeyboard, cx),
                        cx,
                    ),
                    self.strip_control(
                        "ios-lock",
                        Icon::Lock,
                        "Lock  ⌘L",
                        |shell, cx| shell.press_ios_control(Control::Lock, cx),
                        cx,
                    ),
                ];
                let capture = self.capture_button(crate::device_capture::DeviceKind::Iphone, cx);
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .bg(paint(theme.surface))
                    .child(crate::device_capture::device_strip(
                        line, controls, capture, &theme,
                    ))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                canvas(
                                    move |bounds, _, _| native.set_bounds(bounds),
                                    |_, _, _, _| {},
                                )
                                .size_full(),
                            )
                            .children(stand_in.map(|path| {
                                div().absolute().inset_0().child(
                                    gpui::img(path)
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Contain),
                                )
                            })),
                    )
                    .into_any_element()
            }
        }
    }
}

fn message(theme: &Theme, text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .px_6()
        .prose()
        .child(dialog::body(text, theme))
        .into_any_element()
}

fn discover() -> Result<Discovery, Problem> {
    if cfg!(not(target_arch = "aarch64")) {
        return Err(Problem {
            retry: false,
            ..Problem::new(
                "iPhone Simulator needs an Apple silicon Mac",
                "For now ket can show the simulator only on Macs with Apple silicon. \
                 Apple's Simulator app still works on this Mac.",
            )
            .steps([step(
                "Click Open Simulator to use Apple's Simulator app instead.",
            )])
            .action(Action::OpenSimulator)
        });
    }

    // With only the Command Line Tools active, there is no SimulatorKit
    // beside the developer directory.
    let developer_dir = command_text(Path::new("/usr/bin/xcode-select"), &["-p"])
        .ok()
        .and_then(|dir| PathBuf::from(dir.trim()).canonicalize().ok());
    let simulator_kit = developer_dir
        .as_deref()
        .and_then(Path::parent)
        .and_then(simulator_kit_in);
    let (Some(developer_dir), Some(simulator_kit)) = (developer_dir, simulator_kit) else {
        return Err(missing_xcode());
    };
    let core_simulator = PathBuf::from(CORE_SIMULATOR)
        .canonicalize()
        .map_err(|error| {
            Problem::new(
                "Xcode hasn't finished setting up",
                "Xcode installs the simulator the first time it opens, and that hasn't happened \
             on this Mac yet.",
            )
            .steps([
                step("Click Open Xcode and accept its prompts to install additional components."),
                step("When it's done, come back here and click Check again."),
            ])
            .action(Action::OpenXcode)
            .detail(error.to_string())
        })?;

    for binary in [&simulator_kit, &core_simulator] {
        verify_apple_framework(binary).map_err(|error| {
            Problem::new(
                "Xcode's simulator files couldn't be verified",
                "ket only loads simulator code signed by Apple, and these files didn't pass \
                 that check. A damaged or modified Xcode is the usual cause.",
            )
            .steps([
                step("Click Get Xcode and reinstall it from the App Store."),
                step("Open Xcode once so it can finish setting up."),
                step(RETRY),
            ])
            .action(Action::GetXcode)
            .detail(error)
        })?;
    }
    let versions = framework_version(&simulator_kit)
        .and_then(|kit| framework_version(&core_simulator).map(|core| (kit, core)));
    match versions {
        Ok((kit, core))
            if SUPPORTED_SIMULATOR_KITS
                .iter()
                .any(|(known, _)| *known == kit)
                && SUPPORTED_CORE_SIMULATORS.contains(&core.as_str()) => {}
        Ok((kit, core)) => {
            let kits: Vec<_> = SUPPORTED_SIMULATOR_KITS
                .iter()
                .map(|(kit, _)| *kit)
                .collect();
            return Err(unsupported_xcode(
                &developer_dir,
                format!(
                    "SimulatorKit {kit}, CoreSimulator {core}; ket supports SimulatorKit {} \
                     with CoreSimulator {}",
                    kits.join(" or "),
                    SUPPORTED_CORE_SIMULATORS.join(" or "),
                ),
            ));
        }
        Err(error) => return Err(unsupported_xcode(&developer_dir, error)),
    }

    let simctl = developer_dir.join("usr/bin/simctl");
    let listed = command_text(&simctl, &["list", "--json", "devices", "available"])
        .and_then(|json| {
            serde_json::from_str::<SimctlList>(&json)
                .map_err(|error| format!("simctl returned invalid device data: {error}"))
        })
        .map_err(|error| {
            Problem::new(
                "Couldn't list your simulators",
                "Xcode's simulator service didn't answer. It usually just needs to start once.",
            )
            .steps([
                step("Click Open Simulator and wait for it to open."),
                step(RETRY),
            ])
            .action(Action::OpenSimulator)
            .detail(error)
        })?;
    let mut phones: Vec<(SharedString, SimctlDevice)> = listed
        .devices
        .into_iter()
        .flat_map(|(runtime, devices)| {
            let runtime = runtime_label(&runtime);
            devices
                .into_iter()
                .map(move |device| (runtime.clone(), device))
        })
        .filter(|(_, device)| {
            device.is_available && device.device_type_identifier.contains("iPhone")
        })
        .collect();
    if phones.is_empty() {
        return Err(Problem::new(
            "No iPhone simulators are installed",
            "Xcode is ready, but it doesn't have an iOS simulator to run yet.",
        )
        .steps([
            step("Click Open Xcode, then choose Xcode ▸ Settings ▸ Components."),
            step("Download an iOS simulator. It's a large download, so it can take a while."),
            step("When it finishes, click Check again."),
        ])
        .action(Action::OpenXcode));
    }
    // The one used last leads, and is picked: most likely the one wanted.
    // A phone never used sorts by name, so the list holds still between looks.
    phones.sort_by(|(_, left), (_, right)| {
        right
            .last_used_at
            .cmp(&left.last_used_at)
            .then_with(|| left.name.cmp(&right.name))
    });
    let booted = phones
        .iter()
        .position(|(_, device)| device.state == "Booted");
    let Some(booted) = booted else {
        let phones = phones
            .into_iter()
            .enumerate()
            .map(|(index, (runtime, device))| Phone {
                last_used: index == 0 && !device.last_used_at.is_empty(),
                boot: Boot {
                    simctl: simctl.clone(),
                    udid: device.udid,
                    name: device.name.into(),
                },
                runtime,
            })
            .collect();
        return Err(not_running(phones));
    };
    let (os, phone) = phones.swap_remove(booted);

    Ok(Discovery {
        developer_dir,
        core_simulator,
        simulator_kit,
        udid: phone.udid,
        name: phone.name,
        os,
    })
}

/// No iPhone is booted: every installed one, to pick from and start here.
/// `phones` is never empty where this is called from — Open Simulator is
/// the way out all the same.
fn not_running(phones: Vec<Phone>) -> Problem {
    let problem = Problem::new(
        "No iPhone is running",
        "Pick one to start here. It takes a few seconds, longer the first time.",
    );
    if phones.is_empty() {
        return problem
            .steps([
                step("Click Open Simulator, then choose File ▸ Open Simulator and pick an iPhone."),
                step("Once its home screen appears, click Check again."),
            ])
            .action(Action::OpenSimulator);
    }
    Problem {
        picker: Some(Box::new(Picker { phones, picked: 0 })),
        ..problem
    }
}

/// "iOS 26.0" from `com.apple.CoreSimulator.SimRuntime.iOS-26-0`.
fn runtime_label(runtime: &str) -> SharedString {
    let tail = runtime.rsplit('.').next().unwrap_or(runtime);
    match tail.split_once('-') {
        Some((os, version)) => format!("{os} {}", version.replace('-', ".")).into(),
        None => tail.to_owned().into(),
    }
}

fn could_not_start(name: &str, error: String) -> Problem {
    Problem::new(
        &format!("Couldn't start {name}"),
        "The simulator didn't finish starting this iPhone.",
    )
    .steps([
        step("Click Open Simulator and start the iPhone there (File ▸ Open Simulator)."),
        step("Once its home screen appears, click Check again."),
    ])
    .action(Action::OpenSimulator)
    .detail(error)
}

/// SimulatorKit inside an Xcode's Contents, wherever this Xcode keeps it.
fn simulator_kit_in(contents: &Path) -> Option<PathBuf> {
    SIMULATOR_KITS
        .iter()
        .find_map(|relative| contents.join(relative).canonicalize().ok())
}

/// No usable Xcode is active: either none is installed, or one is and the
/// Mac is pointed at the Command Line Tools instead.
fn missing_xcode() -> Problem {
    let Some(app) = installed_xcode() else {
        return Problem::new(
            "iPhone Simulator needs Xcode",
            "The simulator comes with Xcode, Apple's free developer app, and Xcode isn't \
             installed on this Mac.",
        )
        .steps([
            step("Click Get Xcode to open it in the App Store, and install it."),
            step("Open Xcode once and let it install its components, including an iOS simulator."),
            step(RETRY),
        ])
        .action(Action::GetXcode);
    };
    let path = app.display().to_string();
    let path = if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path
    };
    Problem::new(
        "Xcode isn't selected",
        "Xcode is installed, but this Mac is set to use the Command Line Tools instead, so \
         ket can't find the simulator.",
    )
    .steps([
        run_in_terminal(
            "Open Terminal and run this command. It asks for your Mac's password.",
            format!("sudo xcode-select -s {path}"),
        ),
        step(RETRY),
    ])
}

/// Whether this Mac has an Xcode the tab could use: the active developer
/// directory is one, or one is installed and only not selected — which the
/// tab explains how to fix. File-system reads only, no `xcode-select`: it
/// runs as the + menu opens.
pub(crate) fn xcode_installed() -> bool {
    std::env::var_os("DEVELOPER_DIR")
        .map(PathBuf::from)
        .or_else(|| std::fs::read_link("/var/db/xcode_select_link").ok())
        .is_some_and(|developer| developer.parent().and_then(simulator_kit_in).is_some())
        || installed_xcode().is_some()
}

/// The first Xcode in /Applications, preferring the plain `Xcode.app`.
fn installed_xcode() -> Option<PathBuf> {
    let plain = PathBuf::from("/Applications/Xcode.app");
    if plain.is_dir() {
        return Some(plain);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir("/Applications")
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Xcode") && name.ends_with(".app"))
        })
        .collect();
    found.sort();
    found.pop()
}

/// "Xcode 27.0 or 26.6", from the table.
fn supported_xcodes() -> String {
    let versions: Vec<_> = SUPPORTED_SIMULATOR_KITS
        .iter()
        .map(|(_, xcode)| *xcode)
        .collect();
    format!("Xcode {}", versions.join(" or "))
}

fn unsupported_xcode(developer_dir: &Path, detail: String) -> Problem {
    let this = xcode_version(developer_dir).map_or_else(
        || "a different version".to_owned(),
        |version| format!("Xcode {version}"),
    );
    Problem::new(
        "This version of Xcode isn't supported yet",
        format!(
            "ket's simulator tab works with {}, and this Mac has {this}. Each new Xcode needs \
             a ket update.",
            supported_xcodes()
        ),
    )
    .steps([
        step("Click Open Simulator to use Apple's Simulator app in the meantime."),
        step("Update ket when a new version is out, then click Check again."),
    ])
    .action(Action::OpenSimulator)
    .detail(detail)
}

/// The active Xcode's marketing version — "27.0" — from its Info.plist.
fn xcode_version(developer_dir: &Path) -> Option<String> {
    let plist = developer_dir.parent()?.join("Info.plist");
    command_text(
        Path::new("/usr/bin/plutil"),
        &[
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            plist.to_str()?,
        ],
    )
    .ok()
    .map(|version| version.trim().to_owned())
}

fn could_not_attach(error: String) -> Problem {
    Problem::new(
        "Couldn't show the iPhone's screen",
        "The iPhone is running, but ket couldn't connect to its display.",
    )
    .steps([
        step("Quit Apple's Simulator app, then click Open Simulator to start it again."),
        step("Once the iPhone's home screen appears, click Check again."),
        step(
            "If it keeps happening, use the Simulator app directly for now. This Xcode may \
             have changed something ket relies on.",
        ),
    ])
    .action(Action::OpenSimulator)
    .detail(error)
}

/// A problem, centred in the pane: what is wrong, why, and the steps out.
fn problem_view(problem: &Problem, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    // Built now rather than lazily: each step's Copy button borrows `cx`.
    let steps: Vec<_> = problem
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            div()
                .flex()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_none()
                        .w(px(16.0))
                        .font_family(crate::fonts::chrome())
                        .text_size(LABEL)
                        .text_color(paint(theme.text.dim))
                        .child(format!("{}.", index + 1)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(LABEL)
                                .text_color(paint(theme.text.primary))
                                .child(step.text.clone()),
                        )
                        .children(
                            step.command
                                .clone()
                                .map(|command| command_box(command, theme, cx)),
                        ),
                )
        })
        .collect();

    let mut actions = div().flex().gap(px(8.0));
    if let Some(action) = problem.action.clone() {
        actions = actions.child(
            button("ios-simulator-action", action.label())
                .primary()
                .render(theme)
                .on_click(cx.listener(move |_, _, _, cx| {
                    let action = action.clone();
                    cx.background_executor()
                        .spawn(async move { action.open() })
                        .detach();
                })),
        );
    }
    if problem.retry {
        let retry = check_again();
        actions = actions.child(
            if problem.action.is_none() {
                retry.primary()
            } else {
                retry
            }
            .render(theme)
            .on_click(cx.listener(|this, _, _, cx| {
                this.discover_ios_simulator(cx);
            })),
        );
    }
    // With phones to offer, the list is the way out and the footer carries
    // the other two: a phone started elsewhere, and Apple's app itself.
    let actions = if problem.picker.is_none() {
        actions.into_any_element()
    } else {
        phone_footer(theme, cx)
    };

    // A column, so the content is a flex item along the pane's height and
    // can shrink to fit it: the phone list scrolls inside what is left once
    // everything else has its height, rather than running off the pane.
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .p_6()
        .child(
            // Set in the prose face, as a dialog is: sentences to read. The
            // figures in it ask for the mono themselves.
            div()
                .prose()
                .flex()
                .flex_col()
                .min_h_0()
                .gap(px(18.0))
                .w_full()
                .max_w(px(460.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .gap(px(6.0))
                        .child(dialog::header(problem.title.clone(), theme))
                        .child(dialog::body(problem.why.clone(), theme)),
                )
                .children(
                    (!problem.steps.is_empty())
                        .then(|| div().flex().flex_col().gap(px(12.0)).children(steps)),
                )
                .children(
                    problem
                        .picker
                        .as_deref()
                        .map(|picker| phone_list(picker, theme, cx)),
                )
                .child(div().flex_none().child(actions))
                .children(
                    problem
                        .detail
                        .as_ref()
                        .map(|detail| caption(format!("Details: {detail}"), theme).flex_none()),
                ),
        )
        .into_any_element()
}

/// The button that looks again, for a phone started or a fix made elsewhere.
fn check_again() -> Button {
    button("ios-simulator-retry", "Check again").leading(Icon::Refresh)
}

/// Every installed iPhone, the picked one with its Start button.
fn phone_list(picker: &Picker, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let rows = picker.phones.iter().enumerate().map(|(index, phone)| {
        let picked = index == picker.picked;
        let start = picked.then(|| {
            let boot = phone.boot.clone();
            button("ios-simulator-start", "Start")
                .primary()
                .small()
                .leading(Icon::Play)
                .render(theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start_ios_simulator(boot.clone(), cx);
                }))
        });
        sized_row(
            SharedString::from(format!("ios-simulator-phone-{}", phone.boot.udid)),
            picked,
            px(36.0),
            theme,
        )
        .child(icon(
            Icon::Smartphone,
            paint(if picked {
                theme.text.primary
            } else {
                theme.text.dim
            }),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(phone.boot.name.clone()),
        )
        .children(phone.last_used.then(|| caption("last used", theme)))
        .child(tag(phone.runtime.clone(), theme))
        // A fixed slot, so the tags line up whichever row holds Start.
        .child(
            div()
                .flex()
                .flex_none()
                .justify_end()
                .w(px(64.0))
                .children(start),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(IosSimulatorHandle {
                state: State::Failed(problem),
                ..
            }) = this.ios_simulator.as_mut()
                && let Some(picker) = problem.picker.as_mut()
            {
                picker.picked = index;
            }
            cx.notify();
        }))
    });

    div()
        .id("ios-simulator-phones")
        .flex()
        .flex_col()
        .min_h_0()
        .overflow_y_scroll()
        .gap(px(2.0))
        .p(px(4.0))
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(paint(theme.border))
        .bg(paint(theme.panel))
        .children(rows)
        .into_any_element()
}

/// Under the phone list: look again, or go to Apple's app.
fn phone_footer(theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(caption("Started one in Apple's Simulator?", theme).mr(px(4.0)))
        .child(
            check_again()
                .ghost()
                .small()
                .render(theme)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.discover_ios_simulator(cx);
                })),
        )
        .child(div().flex_1())
        .child(
            button("ios-simulator-open", Action::OpenSimulator.label())
                .ghost()
                .small()
                .leading(Icon::ExternalLink)
                .render(theme)
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.background_executor()
                        .spawn(async { Action::OpenSimulator.open() })
                        .detach();
                })),
        )
        .into_any_element()
}

/// A Terminal command: a read-only field in the chrome's mono, with Copy at
/// its end.
fn command_box(command: SharedString, theme: &Theme, cx: &mut Context<Shell>) -> impl IntoElement {
    field("ios-simulator-command", command.clone(), "")
        .read_only()
        .leading(Icon::Terminal)
        .body(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(crate::fonts::chrome())
                .text_size(LABEL)
                .child(command.clone())
                .into_any_element(),
        )
        .render(theme)
        .pr(px(4.0))
        .child(
            button("ios-simulator-copy", "Copy")
                .ghost()
                .small()
                .leading(Icon::Copy)
                .render(theme)
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
                })),
        )
}

pub(crate) fn command_text(program: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("could not run {}: {error}", program.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("{} failed: {detail}", program.display()));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{} did not return UTF-8: {error}", program.display()))
}

fn verify_apple_framework(binary: &Path) -> Result<(), String> {
    let binary_text = binary
        .to_str()
        .ok_or_else(|| "a framework path is not valid UTF-8".to_owned())?;
    command_text(
        Path::new("/usr/bin/codesign"),
        &["--verify", "--strict", binary_text],
    )?;
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "--verbose=4", binary_text])
        .output()
        .map_err(|error| format!("could not inspect {}: {error}", binary.display()))?;
    let details = String::from_utf8_lossy(&output.stderr);
    if !output.status.success()
        || !details
            .lines()
            .any(|line| line == format!("TeamIdentifier={APPLE_DEVELOPER_TEAM}"))
    {
        return Err(format!(
            "refusing to load {} because it is not signed by Apple",
            binary.display()
        ));
    }
    Ok(())
}

/// A framework's build version — "1005.2" — from its Info.plist.
fn framework_version(binary: &Path) -> Result<String, String> {
    let resources = binary
        .parent()
        .ok_or_else(|| "a framework binary has no version directory".to_owned())?
        .join("Resources/Info.plist");
    let plist = resources
        .to_str()
        .ok_or_else(|| "a framework resource path is not valid UTF-8".to_owned())?;
    command_text(
        Path::new("/usr/bin/plutil"),
        &["-extract", "CFBundleVersion", "raw", plist],
    )
    .map(|version| version.trim().to_owned())
}

mod private {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CStr, c_char, c_void};
    use std::panic::AssertUnwindSafe;
    use std::ptr;
    use std::sync::{Arc, OnceLock};

    use gpui::{Bounds, Pixels, Window};
    use libloading::Library;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool, Method, Sel};
    use objc2::{MainThreadMarker, msg_send, sel};
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    use super::Discovery;

    struct Frameworks {
        _simulator_kit: Library,
        _core_simulator: Library,
    }

    #[derive(Clone, Copy)]
    struct SwiftBridge {
        connect: *const c_void,
        disconnect: *const c_void,
        render_scale_get: *const c_void,
        render_scale_set: *const c_void,
        error_release: unsafe extern "C" fn(*mut c_void),
        /// `IndigoHIDMessageForButton(keyCode, op, target)`: a 192-byte
        /// message from `calloc`, which the HID client frees once sent.
        /// Optional: without it the phone still shows, it just cannot
        /// toggle its keyboard.
        button_message: Option<unsafe extern "C" fn(u32, u32, u32) -> *mut c_void>,
        /// `IndigoHIDMessageForHIDArbitrary(target, usagePage, usage, op)`.
        hid_arbitrary: Option<unsafe extern "C" fn(u32, u32, u32, u32) -> *mut c_void>,
        /// `IndigoHIDGetKeyboardType()`: the Mac keyboard's type, remapped.
        keyboard_type: Option<unsafe extern "C" fn() -> u8>,
        /// `SimDisplayView.deviceRotation`, a `Measurement<NSUnitAngle>`.
        rotation_get: Option<*const c_void>,
        rotation_set: Option<*const c_void>,
    }

    /// Simulator.app's Toggle Software Keyboard is one press of the button
    /// SimulatorHID turns into the Eject key (Consumer page 0x0C, usage
    /// 0xB8), which iOS takes from a hardware keyboard as "show or hide the
    /// on-screen one". The hardware keyboard stays connected.
    const EJECT_BUTTON: u32 = 0x3f0;
    const BUTTON_DOWN: u32 = 1;
    const BUTTON_UP: u32 = 2;
    /// `SimDeviceScreen.buttonTarget` for an iPhone or iPad. It is a
    /// Swift-only getter, so the value is named here instead.
    const HARDWARE_TARGET: u32 = 0x33;
    /// Simulator.app's Device menu, as `IndigoHIDMessageForButton` codes.
    /// SimulatorHID turns them into Consumer Menu (0x40), Power (0x30) and
    /// the Siri usage (0xCF).
    const HOME_BUTTON: u32 = 0x0;
    const LOCK_BUTTON: u32 = 0x1;
    const SIRI_BUTTON: u32 = 0x3f2;
    /// Volume is no button code: Simulator.app sends the Consumer usages
    /// themselves, through `IndigoHIDMessageForHIDArbitrary`.
    const CONSUMER_PAGE: u32 = 0x0C;
    const VOLUME_UP: u32 = 0xE9;
    const VOLUME_DOWN: u32 = 0xEA;

    // Rotation reaches iOS as a GSEvent on the device's PurpleWorkspacePort,
    // a raw 108-byte Mach message — what Simulator.app's own
    // `-[SimDevice(GSEvents) gsEventsSendOrientation:]` builds.
    const MACH_MSG_TYPE_COPY_SEND: u32 = 0x13;
    const GS_MESSAGE_SIZE: u32 = 108;
    const GS_MESSAGE_ID: u32 = 0x7b;
    /// GSEvent type 50, orientation changed, with the from-the-host flag.
    const GS_ORIENTATION_CHANGED: u32 = 50 | 0x2_0000;

    unsafe extern "C" {
        fn mach_msg_send(message: *mut c_void) -> i32;
    }

    /// Simulator.app's hardware controls that ket offers, each on
    /// Simulator.app's own chord — see `Shell::ios_simulator_key`.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Control {
        SoftwareKeyboard,
        HardwareKeyboard,
        Home,
        Lock,
        Siri,
        VolumeUp,
        VolumeDown,
        RotateLeft,
        RotateRight,
    }

    impl Control {
        /// What it does, to finish "Could not …".
        pub(crate) fn doing(self) -> &'static str {
            match self {
                Self::SoftwareKeyboard => "show the iPhone's keyboard",
                Self::HardwareKeyboard => "connect the Mac's keyboard to the iPhone",
                Self::Home => "press Home",
                Self::Lock => "lock the iPhone",
                Self::Siri => "start Siri",
                Self::VolumeUp | Self::VolumeDown => "change the volume",
                Self::RotateLeft | Self::RotateRight => "rotate the iPhone",
            }
        }
    }

    /// The `UIDeviceOrientation` an angle of the display view stands for:
    /// Simulator.app turns left by adding 90° and never wraps the angle.
    fn orientation(degrees: f64) -> u32 {
        match ((degrees.rem_euclid(360.0) / 90.0).round() as i64).rem_euclid(4) {
            0 => 1, // portrait
            1 => 3, // landscape left
            2 => 2, // upside down
            _ => 4, // landscape right
        }
    }

    /// `-[SimDeviceLegacyHIDClient sendWithMessage:freeWhenDone:completionQueue:completion:]`,
    /// called through its implementation: objc2 has no encoding for the
    /// Indigo message pointer, and the signature is validated before use.
    type SendHid = unsafe extern "C-unwind" fn(
        *const AnyObject,
        Sel,
        *mut c_void,
        Bool,
        *const AnyObject,
        *const c_void,
    );

    pub(super) struct NativeSimulator {
        display: Retained<AnyObject>,
        /// A HID client of ket's own for the device, made on first use —
        /// Simulator.app keeps one beside its display view the same way.
        /// Declared before the device and frameworks, so it goes first.
        hid: RefCell<Option<(Retained<AnyObject>, SendHid)>>,
        /// The device's PurpleWorkspacePort, looked up on first rotation.
        workspace: Cell<Option<u32>>,
        /// Whether the Mac's keyboard is connected to the phone. CoreSimulator
        /// has no getter; Simulator.app keeps its own flag, default on.
        hardware_keyboard: Cell<bool>,
        device: Retained<AnyObject>,
        _screen: Retained<AnyObject>,
        bridge: SwiftBridge,
        _frameworks: Arc<Frameworks>,
    }

    impl NativeSimulator {
        pub(super) fn attach(window: &mut Window, found: &Discovery) -> Result<Self, String> {
            let _main_thread = MainThreadMarker::new()
                .ok_or_else(|| "SimulatorKit attachment must run on the main thread".to_owned())?;
            let handle = window.window_handle().map_err(|error| error.to_string())?;
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return Err("the iPhone Simulator tab needs an AppKit window".to_owned());
            };
            let parent = handle.ns_view.as_ptr().cast::<NSView>();
            let found = found.clone();
            objc2::exception::catch(AssertUnwindSafe(move || {
                // SAFETY: Both paths were canonicalized and signature-checked immediately
                // before this main-thread call. Handles stay alive after every object that
                // uses their classes. The AppKit pointer comes from the live GPUI window.
                unsafe { Self::attach_unchecked(parent, &found) }
            }))
            .map_err(|_| {
                "SimulatorKit raised an Objective-C exception while attaching".to_owned()
            })?
        }

        unsafe fn attach_unchecked(parent: *mut NSView, found: &Discovery) -> Result<Self, String> {
            let frameworks = Arc::new(Frameworks {
                // SAFETY: Validated signed framework binaries; kept loaded for object lifetime.
                _core_simulator: unsafe { Library::new(&found.core_simulator) }
                    .map_err(|error| format!("could not load CoreSimulator: {error}"))?,
                // SAFETY: As above. CoreSimulator must be loaded first.
                _simulator_kit: unsafe { Library::new(&found.simulator_kit) }
                    .map_err(|error| format!("could not load SimulatorKit: {error}"))?,
            });
            let bridge = unsafe { SwiftBridge::load(&frameworks._simulator_kit) }?;

            let service = class(c"SimServiceContext")?;
            let display = class(c"_TtC12SimulatorKit14SimDisplayView")?;
            let screen = class(c"_TtC12SimulatorKit15SimDeviceScreen")?;
            require_nsview_subclass(display)?;
            validate_class_method(
                service,
                sel!(sharedServiceContextForDeveloperDir:error:),
                "@",
                &["@", ":", "@", "^@"],
            )?;
            validate_instance_method(
                service,
                sel!(defaultDeviceSetWithError:),
                "@",
                &["@", ":", "^@"],
            )?;
            validate_instance_method(display, sel!(initWithFrame:), "@", &["@", ":", "{CGRect"])?;
            validate_instance_method(display, sel!(setDevice:), "v", &["@", ":", "@"])?;
            validate_instance_method(
                screen,
                sel!(initWithDevice:screenID:),
                "@",
                &["@", ":", "@", "I"],
            )?;
            validate_instance_method(screen, sel!(isDefault), "B", &["@", ":"])?;

            let developer = NSString::from_str(&found.developer_dir.to_string_lossy());
            let mut error: *mut AnyObject = ptr::null_mut();
            // SAFETY: Selector and encodings were validated above.
            let context: Option<Retained<AnyObject>> = unsafe {
                msg_send![service, sharedServiceContextForDeveloperDir: &*developer, error: &mut error]
            };
            let context = context
                .ok_or_else(|| "CoreSimulator did not create a service context".to_owned())?;
            // SAFETY: Selector and encodings were validated above.
            let device_set: Option<Retained<AnyObject>> =
                unsafe { msg_send![&context, defaultDeviceSetWithError: &mut error] };
            let device_set = device_set
                .ok_or_else(|| "CoreSimulator did not return its device set".to_owned())?;
            let device = unsafe { find_device(&device_set, &found.udid) }?
                .ok_or_else(|| format!("the booted iPhone {} disappeared", found.name))?;

            // Xcode 27 identifies an iPhone's integrated PurpleMain display as
            // screen 1. Confirm `isDefault` below before handing it to Swift.
            let allocated_screen: Allocated<AnyObject> = unsafe { msg_send![screen, alloc] };
            let screen_object: Option<Retained<AnyObject>> =
                unsafe { msg_send![allocated_screen, initWithDevice: &*device, screenID: 1_u32] };
            let screen_object = screen_object
                .ok_or_else(|| "SimulatorKit did not create the device screen".to_owned())?;
            let is_default: bool = unsafe { msg_send![&screen_object, isDefault] };
            if !is_default {
                return Err("SimulatorKit screen 1 is not the iPhone's default display".to_owned());
            }

            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            // SAFETY: Display class hierarchy and init signature were validated.
            let allocated: Allocated<AnyObject> = unsafe { msg_send![display, alloc] };
            let display_object: Option<Retained<AnyObject>> =
                unsafe { msg_send![allocated, initWithFrame: frame] };
            let display_object = display_object
                .ok_or_else(|| "SimulatorKit did not create its display view".to_owned())?;
            // SAFETY: setDevice: was validated and device belongs to the same service context.
            unsafe {
                let _: () = msg_send![&display_object, setDevice: &*device];
            }

            let parent = unsafe { parent.as_ref() }
                .ok_or_else(|| "GPUI did not provide an AppKit content view".to_owned())?;
            let view = unsafe { view(&display_object) };
            view.setHidden(true);
            view.setClipsToBounds(true);
            parent.addSubview(view);
            if let Err(error) = keep_mouse_moves() {
                tracing::warn!(%error, "hover may stop after the pointer leaves the phone");
            }
            if let Some(window) = parent.window() {
                window.setAcceptsMouseMovedEvents(true);
            }

            let swift_error = unsafe { bridge.connect(&display_object, &screen_object) };
            if !swift_error.is_null() {
                unsafe { (bridge.error_release)(swift_error) };
                view.removeFromSuperview();
                return Err("SimulatorKit rejected the framebuffer connection".to_owned());
            }

            Ok(Self {
                display: display_object,
                hid: RefCell::new(None),
                workspace: Cell::new(None),
                hardware_keyboard: Cell::new(true),
                device,
                _screen: screen_object,
                bridge,
                _frameworks: frameworks,
            })
        }

        pub(super) fn set_visible(&self, visible: bool) {
            if !visible {
                self.release_keyboard();
            }
            // SAFETY: attach proved this private object is an NSView subclass.
            unsafe { view(&self.display) }.setHidden(!visible);
        }

        /// Hands the keyboard back to GPUI's view if the simulator — or
        /// anything inside it — has it. A focused text field in the phone
        /// makes SimDisplayView the window's first responder, and AppKit
        /// does not move that on when the view is hidden or removed: every
        /// key after it went to a phone no longer on screen, and ket could
        /// not be typed in again until it restarted. The browser tab does
        /// the same through wry's `focus_parent`.
        pub(super) fn release_keyboard(&self) -> bool {
            // SAFETY: attach proved this private object is an NSView subclass.
            let view = unsafe { view(&self.display) };
            let parent = unsafe { view.superview() };
            match (self.has_keyboard(), view.window(), parent) {
                (true, Some(window), Some(parent)) => window.makeFirstResponder(Some(&parent)),
                _ => false,
            }
        }

        /// Gives the phone the keyboard, as a click on it would.
        pub(super) fn take_keyboard(&self) {
            // SAFETY: attach proved this private object is an NSView subclass.
            let view = unsafe { view(&self.display) };
            if let Some(window) = view.window()
                && !view.isHidden()
            {
                window.makeFirstResponder(Some(view));
            }
        }

        /// Whether the simulator, or anything inside it, is the window's
        /// first responder — where the Mac's keys are going.
        pub(super) fn has_keyboard(&self) -> bool {
            // SAFETY: attach proved this private object is an NSView subclass.
            let view = unsafe { view(&self.display) };
            view.window()
                .and_then(|window| window.firstResponder())
                .and_then(|responder| responder.downcast::<NSView>().ok())
                .is_some_and(|responder| responder.isDescendantOf(view))
        }

        /// One of Simulator.app's hardware controls. Buttons are a press —
        /// down, then straight up — as Simulator.app sends them.
        pub(super) fn control(&self, control: Control) -> Result<(), String> {
            match control {
                // ⌘K in Simulator.app: the Eject key, which iOS takes as
                // "show or hide the on-screen keyboard".
                Control::SoftwareKeyboard => self.press(EJECT_BUTTON),
                Control::HardwareKeyboard => self.toggle_hardware_keyboard(),
                Control::Home => self.press(HOME_BUTTON),
                Control::Lock => self.press(LOCK_BUTTON),
                Control::Siri => self.press(SIRI_BUTTON),
                Control::VolumeUp => self.consumer(VOLUME_UP),
                Control::VolumeDown => self.consumer(VOLUME_DOWN),
                Control::RotateLeft => self.rotate(90.0),
                Control::RotateRight => self.rotate(-90.0),
            }
        }

        fn press(&self, button: u32) -> Result<(), String> {
            let build = self
                .bridge
                .button_message
                .ok_or_else(|| "SimulatorKit has no IndigoHIDMessageForButton".to_owned())?;
            for op in [BUTTON_DOWN, BUTTON_UP] {
                // SAFETY: A plain C function of the verified SimulatorKit,
                // taking three integers.
                self.send(unsafe { build(button, op, HARDWARE_TARGET) })?;
            }
            Ok(())
        }

        fn consumer(&self, usage: u32) -> Result<(), String> {
            let build = self
                .bridge
                .hid_arbitrary
                .ok_or_else(|| "SimulatorKit has no IndigoHIDMessageForHIDArbitrary".to_owned())?;
            for op in [BUTTON_DOWN, BUTTON_UP] {
                // SAFETY: As `press`: four integers in, a calloc'd message out.
                self.send(unsafe { build(HARDWARE_TARGET, CONSUMER_PAGE, usage, op) })?;
            }
            Ok(())
        }

        /// Sends one Indigo message, which the HID client then frees.
        fn send(&self, message: *mut c_void) -> Result<(), String> {
            if message.is_null() {
                return Err("SimulatorKit did not build the key press".to_owned());
            }
            let (client, send) = self.hid_client()?;
            let selector = sel!(sendWithMessage:freeWhenDone:completionQueue:completion:);
            objc2::exception::catch(AssertUnwindSafe(|| {
                // SAFETY: The implementation's signature was validated when
                // the client was made. freeWhenDone hands the calloc'd
                // message to the client; no queue, no block.
                unsafe {
                    send(
                        Retained::as_ptr(&client),
                        selector,
                        message,
                        Bool::YES,
                        ptr::null(),
                        ptr::null(),
                    );
                }
            }))
            .map_err(|_| "SimulatorKit raised an exception sending the key".to_owned())
        }

        /// Turns the display view — which resizes itself, so the next frame's
        /// `set_bounds` fits it again — and tells iOS the new orientation.
        fn rotate(&self, by: f64) -> Result<(), String> {
            let degrees = class(c"NSUnitAngle")?;
            // SAFETY: A public Foundation class method returning an object.
            let unit: Option<Retained<AnyObject>> = unsafe { msg_send![degrees, degrees] };
            let unit = unit.ok_or_else(|| "Foundation has no degrees unit".to_owned())?;
            // SAFETY: Symbols from the verified, version-pinned SimulatorKit.
            let current = unsafe { self.bridge.rotation(&self.display) }
                .ok_or_else(|| "SimulatorKit cannot report the rotation".to_owned())?;
            let angle = current + by;
            unsafe { self.bridge.set_rotation(&self.display, unit, angle) }
                .ok_or_else(|| "SimulatorKit cannot rotate the display".to_owned())?;
            self.send_orientation(orientation(angle))
        }

        fn send_orientation(&self, orientation: u32) -> Result<(), String> {
            let port = self.workspace_port()?;
            let mut message = [0_u32; (GS_MESSAGE_SIZE / 4) as usize];
            message[0] = MACH_MSG_TYPE_COPY_SEND; // msgh_bits
            message[1] = GS_MESSAGE_SIZE;
            message[2] = port; // msgh_remote_port
            message[5] = GS_MESSAGE_ID;
            message[6] = GS_ORIENTATION_CHANGED; // at 0x18
            message[18] = 4; // at 0x48: the size of what follows
            message[19] = orientation; // at 0x4c
            // SAFETY: A complete, zero-filled Mach message with no
            // descriptors, sent with a copied send right.
            let result = unsafe { mach_msg_send(message.as_mut_ptr().cast()) };
            if result != 0 {
                return Err(format!("the iPhone refused the orientation ({result:#x})"));
            }
            Ok(())
        }

        fn workspace_port(&self) -> Result<u32, String> {
            if let Some(port) = self.workspace.get() {
                return Ok(port);
            }
            validate_instance_method(
                self.device.class(),
                sel!(lookup:error:),
                "I",
                &["@", ":", "@", "^@"],
            )?;
            let name = NSString::from_str("PurpleWorkspacePort");
            let device = &self.device;
            let port = objc2::exception::catch(AssertUnwindSafe(|| {
                let mut error: *mut AnyObject = ptr::null_mut();
                // SAFETY: lookup:error: was validated just above.
                let port: u32 = unsafe { msg_send![&**device, lookup: &*name, error: &mut error] };
                port
            }))
            .map_err(|_| {
                "CoreSimulator raised an exception finding the iPhone's port".to_owned()
            })?;
            if port == 0 {
                return Err("CoreSimulator did not find the iPhone's workspace port".to_owned());
            }
            self.workspace.set(Some(port));
            Ok(port)
        }

        /// Simulator.app's Connect Hardware Keyboard. Off, iOS shows its
        /// on-screen keyboard for every field.
        fn toggle_hardware_keyboard(&self) -> Result<(), String> {
            let keyboard_type = self
                .bridge
                .keyboard_type
                .ok_or_else(|| "SimulatorKit has no IndigoHIDGetKeyboardType".to_owned())?;
            let selector = sel!(setHardwareKeyboardEnabled:keyboardType:error:);
            validate_instance_method(
                self.device.class(),
                selector,
                "B",
                &["@", ":", "B", "C", "^@"],
            )?;
            let enable = !self.hardware_keyboard.get();
            // SAFETY: A plain C function of the verified SimulatorKit.
            let kind = unsafe { keyboard_type() };
            let device = &self.device;
            let done = objc2::exception::catch(AssertUnwindSafe(|| {
                let mut error: *mut AnyObject = ptr::null_mut();
                // SAFETY: The selector's encoding was validated just above.
                let done: bool = unsafe {
                    msg_send![&**device, setHardwareKeyboardEnabled: enable, keyboardType: kind, error: &mut error]
                };
                done
            }))
            .map_err(|_| "CoreSimulator raised an exception".to_owned())?;
            if !done {
                return Err("CoreSimulator refused".to_owned());
            }
            self.hardware_keyboard.set(enable);
            Ok(())
        }

        fn hid_client(&self) -> Result<(Retained<AnyObject>, SendHid), String> {
            if let Some((client, send)) = &*self.hid.borrow() {
                return Ok((client.clone(), *send));
            }
            let class = class(c"_TtC12SimulatorKit24SimDeviceLegacyHIDClient")?;
            validate_instance_method(
                class,
                sel!(initWithDevice:error:),
                "@",
                &["@", ":", "@", "^@"],
            )?;
            let selector = sel!(sendWithMessage:freeWhenDone:completionQueue:completion:);
            validate_instance_method(
                class,
                selector,
                "v",
                &["@", ":", "^{IndigoHIDMessageStruct", "B", "@", "@?"],
            )?;
            let method = class
                .instance_method(selector)
                .ok_or_else(|| "the HID client cannot send".to_owned())?;
            // SAFETY: The signature was validated just above, argument by
            // argument, against `SendHid`.
            let send: SendHid = unsafe { std::mem::transmute(method.implementation()) };

            let device = &self.device;
            let client = objc2::exception::catch(AssertUnwindSafe(|| {
                let mut error: *mut AnyObject = ptr::null_mut();
                // SAFETY: initWithDevice:error: was validated above; the
                // device belongs to the verified CoreSimulator.
                let allocated: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
                let client: Option<Retained<AnyObject>> =
                    unsafe { msg_send![allocated, initWithDevice: &**device, error: &mut error] };
                client
            }))
            .map_err(|_| "SimulatorKit raised an exception making a HID client".to_owned())?
            .ok_or_else(|| "SimulatorKit did not make a HID client for the iPhone".to_owned())?;
            *self.hid.borrow_mut() = Some((client.clone(), send));
            Ok((client, send))
        }

        pub(super) fn set_bounds(&self, bounds: Bounds<Pixels>) {
            // GPUI coordinates start at the window's top-left; AppKit child
            // frames start at the parent view's bottom-left.
            let view = unsafe { view(&self.display) };
            // SAFETY: The view remains attached while this process-local handle lives.
            let Some(parent) = (unsafe { view.superview() }) else {
                return;
            };
            let height = parent.bounds().size.height;
            let pane_x = f32::from(bounds.origin.x) as f64;
            let pane_width = f32::from(bounds.size.width) as f64;
            let pane_height = f32::from(bounds.size.height) as f64;
            let top = f32::from(bounds.origin.y) as f64;

            // SimDisplayView lays its framebuffer out at `renderScale`; merely
            // assigning the pane's frame leaves that intrinsic child at its
            // previous size and AppKit then cuts it off. Recover the unscaled
            // device size from the current intrinsic size, choose an
            // aspect-preserving fit, and center that native view in the pane.
            let intrinsic: NSSize = unsafe { msg_send![&self.display, intrinsicContentSize] };
            let current_scale = unsafe { self.bridge.render_scale(&self.display) };
            let valid = current_scale.is_finite()
                && current_scale > 0.0
                && intrinsic.width.is_finite()
                && intrinsic.width > 0.0
                && intrinsic.height.is_finite()
                && intrinsic.height > 0.0;
            let (x, width, fitted_height) = if valid {
                let natural_width = intrinsic.width / current_scale;
                let natural_height = intrinsic.height / current_scale;
                let scale = (pane_width / natural_width)
                    .min(pane_height / natural_height)
                    .min(1.0);
                unsafe { self.bridge.set_render_scale(&self.display, scale) };
                let width = natural_width * scale;
                let fitted_height = natural_height * scale;
                (pane_x + (pane_width - width) / 2.0, width, fitted_height)
            } else {
                (pane_x, pane_width, pane_height)
            };
            view.setFrame(NSRect::new(
                NSPoint::new(x, height - top - (pane_height + fitted_height) / 2.0),
                NSSize::new(width, fitted_height),
            ));
        }
    }

    impl Drop for NativeSimulator {
        fn drop(&mut self) {
            self.release_keyboard();
            let view = unsafe { view(&self.display) };
            view.setHidden(true);
            unsafe { self.bridge.disconnect(&self.display) };
            // SAFETY: setDevice: was validated before this object was constructed.
            unsafe {
                let _: () = msg_send![&self.display, setDevice: Option::<&AnyObject>::None];
            }
            view.removeFromSuperview();
        }
    }

    impl SwiftBridge {
        unsafe fn load(library: &Library) -> Result<Self, String> {
            #[cfg(not(target_arch = "aarch64"))]
            {
                let _ = library;
                return Err(
                    "the version-pinned SimulatorKit bridge currently requires Apple silicon"
                        .to_owned(),
                );
            }

            #[cfg(target_arch = "aarch64")]
            {
                const CONNECT: &[u8] = b"$s12SimulatorKit14SimDisplayViewC7connect6screen6inputsyAA0C12DeviceScreenC_AC0j5InputI0VtKFTj\0";
                const DISCONNECT: &[u8] =
                    b"$s12SimulatorKit14SimDisplayViewC10disconnect10completionyyycSg_tFTj\0";
                const RENDER_SCALE_GET: &[u8] =
                    b"$s12SimulatorKit14SimDisplayViewC11renderScale12CoreGraphics7CGFloatVvgTj\0";
                const RENDER_SCALE_SET: &[u8] =
                    b"$s12SimulatorKit14SimDisplayViewC11renderScale12CoreGraphics7CGFloatVvsTj\0";
                // SAFETY: Exact symbols are required only after the framework's
                // signature and version have been verified.
                let connect = unsafe { library.get::<*const c_void>(CONNECT) }
                    .map_err(|error| format!("SimulatorKit connect ABI is unavailable: {error}"))?;
                let disconnect =
                    unsafe { library.get::<*const c_void>(DISCONNECT) }.map_err(|error| {
                        format!("SimulatorKit disconnect ABI is unavailable: {error}")
                    })?;
                let render_scale_get = unsafe { library.get::<*const c_void>(RENDER_SCALE_GET) }
                    .map_err(|error| {
                        format!("SimulatorKit render-scale getter is unavailable: {error}")
                    })?;
                let render_scale_set = unsafe { library.get::<*const c_void>(RENDER_SCALE_SET) }
                    .map_err(|error| {
                        format!("SimulatorKit render-scale setter is unavailable: {error}")
                    })?;
                let error_release = unsafe {
                    library.get::<unsafe extern "C" fn(*mut c_void)>(b"swift_errorRelease\0")
                }
                .map_err(|error| format!("the Swift error runtime is unavailable: {error}"))?;
                let button_message = unsafe {
                    library.get::<unsafe extern "C" fn(u32, u32, u32) -> *mut c_void>(
                        b"IndigoHIDMessageForButton\0",
                    )
                }
                .ok()
                .map(|symbol| *symbol);
                // Optional like the button builder: without them the phone
                // still shows, and only that control reports it is missing.
                let hid_arbitrary = unsafe {
                    library.get::<unsafe extern "C" fn(u32, u32, u32, u32) -> *mut c_void>(
                        b"IndigoHIDMessageForHIDArbitrary\0",
                    )
                }
                .ok()
                .map(|symbol| *symbol);
                let keyboard_type = unsafe {
                    library.get::<unsafe extern "C" fn() -> u8>(b"IndigoHIDGetKeyboardType\0")
                }
                .ok()
                .map(|symbol| *symbol);
                const ROTATION_GET: &[u8] = b"$s12SimulatorKit14SimDisplayViewC14deviceRotation10Foundation11MeasurementVySo11NSUnitAngleCGvgTj\0";
                const ROTATION_SET: &[u8] = b"$s12SimulatorKit14SimDisplayViewC14deviceRotation10Foundation11MeasurementVySo11NSUnitAngleCGvsTj\0";
                let rotation_get = unsafe { library.get::<*const c_void>(ROTATION_GET) }
                    .ok()
                    .map(|symbol| *symbol);
                let rotation_set = unsafe { library.get::<*const c_void>(ROTATION_SET) }
                    .ok()
                    .map(|symbol| *symbol);
                Ok(Self {
                    button_message,
                    hid_arbitrary,
                    keyboard_type,
                    rotation_get,
                    rotation_set,
                    connect: *connect,
                    disconnect: *disconnect,
                    render_scale_get: *render_scale_get,
                    render_scale_set: *render_scale_set,
                    error_release: *error_release,
                })
            }
        }

        #[cfg(target_arch = "aarch64")]
        unsafe fn connect(
            self,
            display: &Retained<AnyObject>,
            screen: &Retained<AnyObject>,
        ) -> *mut c_void {
            let mut error = ptr::null_mut::<c_void>();
            let inputs = usize::MAX;
            // SAFETY: Xcode 27's Swift arm64 ABI passes method `self` in x20,
            // thrown errors in x21, the screen reference in x0, and the
            // resilient ScreenInputDevice value indirectly through x1.
            // Framework version and symbol identity were checked before this call.
            unsafe {
                core::arch::asm!(
                    "blr x16",
                    in("x16") self.connect,
                    in("x20") Retained::as_ptr(display),
                    in("x0") Retained::as_ptr(screen),
                    in("x1") &inputs,
                    inout("x21") error,
                    clobber_abi("C"),
                );
            }
            error
        }

        #[cfg(not(target_arch = "aarch64"))]
        unsafe fn connect(
            self,
            _display: &Retained<AnyObject>,
            _screen: &Retained<AnyObject>,
        ) -> *mut c_void {
            ptr::null_mut()
        }

        #[cfg(target_arch = "aarch64")]
        unsafe fn disconnect(self, display: &Retained<AnyObject>) {
            // A nil Swift closure is two zero words; `self` remains x20.
            unsafe {
                core::arch::asm!(
                    "blr x16",
                    in("x16") self.disconnect,
                    in("x20") Retained::as_ptr(display),
                    in("x0") 0_usize,
                    in("x1") 0_usize,
                    clobber_abi("C"),
                );
            }
        }

        #[cfg(not(target_arch = "aarch64"))]
        unsafe fn disconnect(self, _display: &Retained<AnyObject>) {}

        #[cfg(target_arch = "aarch64")]
        unsafe fn render_scale(self, display: &Retained<AnyObject>) -> f64 {
            let scale: f64;
            unsafe {
                core::arch::asm!(
                    "blr x16",
                    in("x16") self.render_scale_get,
                    in("x20") Retained::as_ptr(display),
                    lateout("d0") scale,
                    clobber_abi("C"),
                );
            }
            scale
        }

        #[cfg(not(target_arch = "aarch64"))]
        unsafe fn render_scale(self, _display: &Retained<AnyObject>) -> f64 {
            1.0
        }

        #[cfg(target_arch = "aarch64")]
        unsafe fn set_render_scale(self, display: &Retained<AnyObject>, scale: f64) {
            unsafe {
                core::arch::asm!(
                    "blr x16",
                    in("x16") self.render_scale_set,
                    in("x20") Retained::as_ptr(display),
                    in("d0") scale,
                    clobber_abi("C"),
                );
            }
        }

        #[cfg(not(target_arch = "aarch64"))]
        unsafe fn set_render_scale(self, _display: &Retained<AnyObject>, _scale: f64) {}

        /// `deviceRotation`'s value in degrees. Swift returns the
        /// `Measurement` indirectly through x8 as `{unit +1, value}`; the
        /// unit is released here.
        #[cfg(target_arch = "aarch64")]
        unsafe fn rotation(self, display: &Retained<AnyObject>) -> Option<f64> {
            let getter = self.rotation_get?;
            let mut measurement = [0_u64; 2];
            // SAFETY: Method `self` in x20 and the indirect result in x8,
            // Xcode 27's Swift arm64 ABI; the symbol was checked on loading.
            unsafe {
                core::arch::asm!(
                    "blr x16",
                    in("x16") getter,
                    in("x20") Retained::as_ptr(display),
                    in("x8") measurement.as_mut_ptr(),
                    clobber_abi("C"),
                );
                drop(Retained::from_raw(measurement[0] as *mut AnyObject));
            }
            Some(f64::from_bits(measurement[1]))
        }

        #[cfg(not(target_arch = "aarch64"))]
        unsafe fn rotation(self, _display: &Retained<AnyObject>) -> Option<f64> {
            None
        }

        /// Sets `deviceRotation`. The `Measurement` goes by address in x0,
        /// and the setter takes ownership of its unit.
        #[cfg(target_arch = "aarch64")]
        unsafe fn set_rotation(
            self,
            display: &Retained<AnyObject>,
            unit: Retained<AnyObject>,
            degrees: f64,
        ) -> Option<()> {
            let setter = self.rotation_set?;
            let measurement = [Retained::into_raw(unit) as u64, degrees.to_bits()];
            // SAFETY: As `rotation`; the unit's +1 passes to Swift.
            unsafe {
                core::arch::asm!(
                    "blr x16",
                    in("x16") setter,
                    in("x20") Retained::as_ptr(display),
                    in("x0") measurement.as_ptr(),
                    clobber_abi("C"),
                );
            }
            Some(())
        }

        #[cfg(not(target_arch = "aarch64"))]
        unsafe fn set_rotation(
            self,
            _display: &Retained<AnyObject>,
            _unit: Retained<AnyObject>,
            _degrees: f64,
        ) -> Option<()> {
            None
        }
    }

    unsafe fn find_device(
        device_set: &AnyObject,
        wanted: &str,
    ) -> Result<Option<Retained<AnyObject>>, String> {
        let class = device_set.class();
        validate_instance_method(class, sel!(devices), "@", &["@", ":"])?;
        let devices: Option<Retained<AnyObject>> = unsafe { msg_send![device_set, devices] };
        let Some(devices) = devices else {
            return Ok(None);
        };
        let count: usize = unsafe { msg_send![&devices, count] };
        for index in 0..count {
            let device: &AnyObject = unsafe { msg_send![&devices, objectAtIndex: index] };
            validate_instance_method(device.class(), sel!(UDID), "@", &["@", ":"])?;
            let uuid: Option<Retained<AnyObject>> = unsafe { msg_send![device, UDID] };
            let Some(uuid) = uuid else { continue };
            let text: Option<Retained<AnyObject>> = unsafe { msg_send![&uuid, UUIDString] };
            let Some(text) = text else { continue };
            if unsafe { object_string(&text) }? == wanted {
                // SAFETY: `device` is live while its owning NSArray is retained.
                return unsafe { Retained::retain(device as *const AnyObject as *mut AnyObject) }
                    .map(Some)
                    .ok_or_else(|| "CoreSimulator returned a null device".to_owned());
            }
        }
        Ok(None)
    }

    unsafe fn object_string(object: &AnyObject) -> Result<&str, String> {
        let bytes: *const c_char = unsafe { msg_send![object, UTF8String] };
        if bytes.is_null() {
            return Err("an Objective-C string had no UTF-8 representation".to_owned());
        }
        unsafe { CStr::from_ptr(bytes) }
            .to_str()
            .map_err(|error| format!("an Objective-C string was not UTF-8: {error}"))
    }

    fn class(name: &'static CStr) -> Result<&'static AnyClass, String> {
        AnyClass::get(name).ok_or_else(|| {
            format!(
                "required private class {} is unavailable",
                name.to_string_lossy()
            )
        })
    }

    fn require_nsview_subclass(class: &AnyClass) -> Result<(), String> {
        let mut next = Some(class);
        while let Some(candidate) = next {
            if candidate.name().to_bytes() == b"NSView" {
                return Ok(());
            }
            next = candidate.superclass();
        }
        Err(format!(
            "{} is not an NSView subclass",
            class.name().to_string_lossy()
        ))
    }

    fn validate_class_method(
        class: &AnyClass,
        selector: Sel,
        result: &str,
        arguments: &[&str],
    ) -> Result<(), String> {
        validate_method(
            class.class_method(selector),
            class,
            selector,
            result,
            arguments,
        )
    }

    fn validate_instance_method(
        class: &AnyClass,
        selector: Sel,
        result: &str,
        arguments: &[&str],
    ) -> Result<(), String> {
        validate_method(
            class.instance_method(selector),
            class,
            selector,
            result,
            arguments,
        )
    }

    fn validate_method(
        method: Option<&Method>,
        class: &AnyClass,
        selector: Sel,
        result: &str,
        arguments: &[&str],
    ) -> Result<(), String> {
        let method = method.ok_or_else(|| {
            format!(
                "{} does not implement {}",
                class.name().to_string_lossy(),
                selector
            )
        })?;
        let actual_result = method.return_type();
        if !encoding_matches(&actual_result, result) || method.arguments_count() != arguments.len()
        {
            return Err(format!(
                "{} has an incompatible {} signature",
                class.name().to_string_lossy(),
                selector
            ));
        }
        for (index, expected) in arguments.iter().enumerate() {
            let actual = method.argument_type(index).ok_or_else(|| {
                format!(
                    "{} has an incomplete {} signature",
                    class.name().to_string_lossy(),
                    selector
                )
            })?;
            if !encoding_matches(&actual, expected) {
                return Err(format!(
                    "{} has an incompatible {} signature",
                    class.name().to_string_lossy(),
                    selector
                ));
            }
        }
        Ok(())
    }

    fn encoding_matches(actual: &CStr, expected: &str) -> bool {
        actual.to_bytes().starts_with(expected.as_bytes())
    }

    unsafe fn view(object: &Retained<AnyObject>) -> &NSView {
        unsafe { &*Retained::as_ptr(object).cast::<NSView>() }
    }

    /// SimulatorKit's digitizer turns the window's mouse-moved events on as
    /// the pointer enters the phone and off as it leaves. GPUI's window gets
    /// every mouse move through that flag, so crossing the phone once killed
    /// every hover in ket until it restarted. GPUI turns the flag on for its
    /// windows and never off, so its window class now ignores off.
    fn keep_mouse_moves() -> Result<(), String> {
        type SetFlag = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Bool);
        static ORIGINAL: OnceLock<SetFlag> = OnceLock::new();

        unsafe extern "C-unwind" fn always_on(this: *mut AnyObject, cmd: Sel, _: Bool) {
            if let Some(original) = ORIGINAL.get() {
                // SAFETY: NSWindow's own implementation, on the window it was
                // sent to, with the selector it was sent as.
                unsafe { original(this, cmd, Bool::YES) };
            }
        }

        if ORIGINAL.get().is_some() {
            return Ok(());
        }
        let selector = sel!(setAcceptsMouseMovedEvents:);
        let method = class(c"NSWindow")?
            .instance_method(selector)
            .ok_or_else(|| "NSWindow has no setAcceptsMouseMovedEvents:".to_owned())?;
        // SAFETY: A public AppKit setter taking one BOOL, as `SetFlag` says.
        let original: SetFlag = unsafe { std::mem::transmute(method.implementation()) };
        let _ = ORIGINAL.set(original);
        let gpui_window = class(c"GPUIWindow")?;
        // SAFETY: GPUIWindow is GPUI's NSWindow subclass and does not define
        // this selector itself; `always_on` has the signature and encoding
        // of the NSWindow method it overrides.
        let added = unsafe {
            let always_on: SetFlag = always_on;
            objc2::ffi::class_addMethod(
                ptr::from_ref(gpui_window).cast_mut(),
                selector,
                std::mem::transmute::<SetFlag, objc2::runtime::Imp>(always_on),
                objc2::ffi::method_getTypeEncoding(method),
            )
        };
        if added.as_bool() {
            Ok(())
        } else {
            Err("GPUIWindow already overrides setAcceptsMouseMovedEvents:".to_owned())
        }
    }
}
