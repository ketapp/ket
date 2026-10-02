//! Settings → Devices: pair an iPhone or iPad with this desktop, and see and revoke the
//! devices already paired (Epic 5b).
//!
//! The same three things `ket host pair`, `ket host devices` and `ket host
//! revoke` do, through the same functions in `ket_core::host`. Every one of
//! them is a round trip over the host's socket that can wait seconds on a
//! host that is busy or wedged, so none runs on the window's thread: each
//! goes to the background executor and comes back as a [`Snapshot`], the
//! way the merge and clear-build dialogs do their work.
//!
//! Phones are one switch at the top of the pane, off until turned on. On,
//! the ket host runs the relay itself on the Mac and phones on the same
//! Wi-Fi reach it — no restart, and no terminal ends; off, every phone is
//! disconnected and nothing listens. Paired phones are listed either way:
//! with no host to ask, `devices` reads the identity files, and revoking
//! edits them.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, ClipboardItem, Context, Pixels, SharedString, Task, WeakEntity, Window, div,
    prelude::*, px,
};
use ket_core::host::{self, DeviceEntry, DeviceRole, PendingPairing};
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::preferences::{Entry, Section};
use crate::ui::banner::banner;
use crate::ui::button::button;
use crate::ui::chip::{Motion, caption, status_dot, tag, tinted_tag};
use crate::ui::field::error;
use crate::ui::icon::{Icon, icon};
use crate::ui::popup::{self, Placement};
use crate::ui::qr::{QrModules, qr_code};
use crate::ui::row::sized_row;
use crate::ui::toast::Tone;
use crate::ui::toggle::switch;
use crate::ui::{CAPTION, CONTROL_H, ICON_GAP, LABEL, PAD_X, RADIUS_MD, RADIUS_SM, TITLE};

/// How often the pane wakes while it is showing, to swap a code that is
/// about to run out for a fresh one.
const TICK: Duration = Duration::from_secs(1);

/// Every this many ticks, the paired phones are asked for again, so a phone
/// connecting or dropping shows without reopening the pane — and a phone
/// that just scanned the code shows up as paired.
const REFRESH_TICKS: u32 = 4;

/// How long before a code runs out it is swapped for a fresh one, so a scan
/// never lands on a code with seconds left.
const RENEW_BEFORE: u64 = 20;

/// How long an approved device has to arrive in the list before the pane
/// stops saying it is connecting and says it did not, in seconds.
const CONNECT_TIMEOUT: u64 = 30;

/// How long after pairing a device that is not connected yet reads as
/// connecting rather than as not connected, in seconds: the phone closes the
/// pairing connection and dials back in, which takes a moment.
const SETTLING: u64 = 60;

/// The most room the code may take, in the Devices group and the Pair a
/// device popover alike. It is drawn at the largest whole-pixel module size
/// that fits, so it lands a little under this.
///
/// Sized for the usual code: about 180 characters with a LAN relay, which is
/// QR version 10 — 57 modules, 65 with the quiet zone — at four pixels a
/// module, which a phone held at arm's length reads first time.
const QR: Pixels = px(260.0);

/// The Pair a device popover's inset.
const POPOVER_PAD: Pixels = px(16.0);

/// How wide the Pair a device popover is: the code, edge to edge inside it.
const POPOVER_W: Pixels = px(260.0 + 2.0 * 16.0);

/// The Phones pane's state, kept on the settings view and dropped with it.
#[derive(Default)]
pub(crate) struct PhonesPane {
    /// What the host said about phones, once it has answered.
    host: Option<HostState>,
    /// The code on show.
    pairing: Option<Pairing>,
    /// Why the last attempt at a code failed.
    pairing_error: Option<String>,
    /// The paired phones, newest first.
    devices: Vec<DeviceEntry>,
    /// Phones that have paired and wait for this desktop to approve them.
    pairings: Vec<PendingPairing>,
    /// Whether the running host accepts role-bearing pairing decisions.
    scoped_roles: bool,
    /// The pairing being approved or declined now.
    deciding: Option<u64>,
    /// A device just approved, by name, and when (Unix seconds): the pane
    /// says it is connecting until it shows up in the list.
    connecting: Option<(String, u64)>,
    /// Why they could not be listed.
    devices_error: Option<String>,
    /// The phone whose Revoke was pressed once and is asking to be sure.
    confirming: Option<String>,
    /// The phone being revoked now.
    revoking: Option<String>,
    /// Whether a fetch is on its way, so the ticker does not stack another.
    fetching: bool,
    /// Whether the switch has been thrown and the host has not answered yet.
    switching: bool,
    /// Whether the Pair a device popover is open.
    pairing_open: bool,
    /// This desktop's name on the network, once asked.
    desktop: Option<String>,
    /// Wakes the pane every [`TICK`] while the settings view is open.
    /// Dropping it — closing the view — stops it.
    ticker: Option<Task<()>>,
}

/// Whether this window can pair a phone, and if not, why not.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostState {
    /// Phones are on: the host is serving them.
    Serving,
    /// Phones are off — the host is not serving them, or is not running.
    Off,
    /// This window was started with `KET_HOST=0`, so it has no host to
    /// serve phones from.
    NoHost,
    /// Something answered on the socket but the exchange failed.
    Unreachable(String),
}

/// A pairing code and what the pane draws from it.
struct Pairing {
    /// The `ket://pair/…` text: what the QR code encodes, and what the
    /// phone's paste field and `ket://pair?code=` link both accept.
    code: String,
    /// Unix seconds.
    expires_at: u64,
    /// `None` only if the code is too long for any QR version.
    modules: Option<Arc<QrModules>>,
}

/// One round of asking the host, done off the window's thread.
struct Snapshot {
    host: HostState,
    /// Present when a code was asked for and the host could make one.
    pairing: Option<Result<Pairing, String>>,
    devices: Result<Vec<DeviceEntry>, String>,
    /// Phones waiting for approval; none when the host is not serving.
    pairings: Vec<PendingPairing>,
    scoped_roles: bool,
    /// This desktop's name on the network, asked along with a code.
    desktop: Option<String>,
}

/// Asks the host everything the pane shows. Blocks; run it in the
/// background.
fn fetch(want_code: bool) -> Snapshot {
    let host = match host::serves_phones() {
        Ok(Some(true)) => HostState::Serving,
        Ok(Some(false)) => HostState::Off,
        Ok(None) if host::enabled() => HostState::Off,
        Ok(None) => HostState::NoHost,
        Err(why) => HostState::Unreachable(why.to_string()),
    };
    let pairing = (want_code && host == HostState::Serving).then(|| {
        host::pairing_code()
            .map(|code| Pairing {
                expires_at: host::pairing_expires_at(&code).unwrap_or(0),
                // Encoded here too: it runs Reed-Solomon and scores eight
                // masks, which is no work for the window's thread either.
                modules: QrModules::encode(&host::pairing_link(&code)),
                code,
            })
            .map_err(|why| why.to_string())
    });
    let devices = host::devices()
        .map(|mut devices| {
            devices.sort_by_key(|device| std::cmp::Reverse(device.paired_at));
            devices
        })
        .map_err(|why| why.to_string());
    let (pairings, scoped_roles) = if host == HostState::Serving {
        host::pairings().unwrap_or_default()
    } else {
        (Vec::new(), false)
    };
    Snapshot {
        host,
        pairing,
        devices,
        pairings,
        scoped_roles,
        desktop: want_code.then(host::lan_name),
    }
}

/// Unix seconds now.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A phone's name, or what to call one that gave none.
fn phone_name(device: &DeviceEntry) -> String {
    if device.name.trim().is_empty() {
        "Unnamed device".to_owned()
    } else {
        device.name.clone()
    }
}

/// "Paired 3d 4h ago", from Unix seconds.
fn paired_ago(paired_at: u64) -> String {
    let age = now().saturating_sub(paired_at);
    if age < 60 {
        return "Paired just now".to_owned();
    }
    format!(
        "Paired {} ago",
        crate::status_bar::compact_duration(age * 1_000)
    )
}

/// Whether a code is out, or close enough that it should be swapped.
fn expiring(expires_at: u64) -> bool {
    expires_at.saturating_sub(now()) <= RENEW_BEFORE
}

impl Shell {
    /// The pane's state, when the settings view is open.
    fn phones_pane(&mut self) -> Option<&mut PhonesPane> {
        self.preferences.as_mut().map(|view| &mut view.phones)
    }

    /// Call after the settings view lands on a section. Starts the pane's
    /// ticker and asks the host for the pane's contents, if the section is
    /// Phones; otherwise does nothing.
    pub(crate) fn phones_shown(&mut self, cx: &mut Context<Self>) {
        if self
            .preferences
            .as_ref()
            .is_none_or(|view| view.section != Section::Devices)
        {
            return;
        }
        let want_code = {
            let Some(pane) = self.phones_pane() else {
                return;
            };
            if pane.ticker.is_none() {
                pane.ticker = Some(cx.spawn(tick));
            }
            pane.pairing.is_none()
        };
        self.refresh_phones(want_code, cx);
    }

    /// Asks the host again, in the background. A code is asked for only
    /// when `want_code`, because each one is a fresh one-time offer.
    fn refresh_phones(&mut self, want_code: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.phones_pane() else {
            return;
        };
        if pane.fetching {
            return;
        }
        pane.fetching = true;
        let work = cx
            .background_executor()
            .spawn(async move { fetch(want_code) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let snapshot = work.await;
            let _ = shell.update(cx, |shell, cx| shell.apply_phones(snapshot, cx));
        })
        .detach();
    }

    /// Puts a fetch's answer on the pane.
    fn apply_phones(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let Some(pane) = self.phones_pane() else {
            return;
        };
        pane.fetching = false;

        // A phone that was not in the last list and is in this one has just
        // paired — with the code on screen, most likely, which is spent now.
        let known = pane.host.is_some() && pane.devices_error.is_none();
        let newcomer = match &snapshot.devices {
            Ok(devices) if known => devices
                .iter()
                .find(|device| !pane.devices.iter().any(|old| old.id == device.id))
                .map(phone_name),
            _ => None,
        };
        match snapshot.devices {
            Ok(devices) => {
                pane.devices = devices;
                pane.devices_error = None;
            }
            Err(why) => pane.devices_error = Some(why),
        }
        if pane
            .confirming
            .as_ref()
            .is_some_and(|id| !pane.devices.iter().any(|device| &device.id == id))
        {
            pane.confirming = None;
        }

        if let Some(desktop) = snapshot.desktop {
            pane.desktop = Some(desktop);
        }
        // A phone that has scanned the code waits here, and the code it used
        // is spent: the pane shows the phone to approve in its place.
        if !snapshot.pairings.is_empty() && pane.pairings.is_empty() {
            pane.pairing = None;
        }
        pane.pairings = snapshot.pairings;
        pane.scoped_roles = snapshot.scoped_roles;
        let serving = snapshot.host == HostState::Serving;
        pane.host = Some(snapshot.host);
        match snapshot.pairing {
            Some(Ok(pairing)) => {
                pane.pairing = Some(pairing);
                pane.pairing_error = None;
            }
            Some(Err(why)) => pane.pairing_error = Some(why),
            None => {}
        }
        // A code from a host that has since gone, or lost its relay, pairs
        // nothing; showing it would invite a scan that cannot work.
        if !serving {
            pane.pairing = None;
            pane.pairing_open = false;
        }
        let spent = newcomer.is_some() && pane.pairing.take().is_some();
        if newcomer.is_some() {
            pane.pairing_open = false;
            pane.connecting = None;
        }

        if let Some(name) = newcomer {
            self.toast_detail(Tone::Success, "Device paired", name, cx);
        }
        if spent {
            self.refresh_phones(true, cx);
        }
        cx.notify();
    }

    /// Turns phones on or off: saved first, so a host started to serve them
    /// reads the switch already thrown, then told to the running host in the
    /// background. No terminal ends either way.
    fn set_phones_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable || view.phones.switching {
            return;
        }
        view.config.phones.enabled = enabled;
        view.error = view.config.save().err().map(|error| error.to_string());
        view.phones.switching = true;
        view.phones.pairing = None;
        view.phones.pairing_error = None;
        view.phones.pairing_open = false;

        let work = cx
            .background_executor()
            .spawn(async move { host::set_phones(enabled).map_err(|why| why.to_string()) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Some(pane) = shell.phones_pane() {
                    pane.switching = false;
                }
                if let Err(why) = outcome {
                    let title = if enabled {
                        "Could not turn Devices on"
                    } else {
                        "Could not turn Devices off"
                    };
                    shell.toast_detail(Tone::Error, title, why, cx);
                }
                shell.refresh_phones(enabled, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Throws the code away and asks for another.
    fn new_pairing_code(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.phones_pane() else {
            return;
        };
        pane.pairing = None;
        pane.pairing_error = None;
        self.refresh_phones(true, cx);
    }

    /// Puts the code's text on the clipboard, for pasting into the phone.
    fn copy_pairing_code(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self
            .phones_pane()
            .and_then(|pane| pane.pairing.as_ref())
            .map(|pairing| pairing.code.clone())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.toast(Tone::Success, "Pairing code copied", cx);
    }

    /// Revokes a phone, in the background.
    fn revoke_phone(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(pane) = self.phones_pane() else {
            return;
        };
        if pane.revoking.is_some() {
            return;
        }
        let name = pane
            .devices
            .iter()
            .find(|device| device.id == id)
            .map_or_else(|| "Device".to_owned(), phone_name);
        pane.confirming = None;
        pane.revoking = Some(id.clone());

        let work = cx
            .background_executor()
            .spawn(async move { host::revoke(&id).map_err(|why| why.to_string()) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Some(pane) = shell.phones_pane() {
                    pane.revoking = None;
                }
                match outcome {
                    Ok(true) => shell.toast_detail(Tone::Success, "Device revoked", name, cx),
                    Ok(false) => {
                        shell.toast_detail(Tone::Info, "That device was not paired", name, cx)
                    }
                    Err(why) => shell.toast_detail(Tone::Error, "Could not revoke", why, cx),
                }
                shell.refresh_phones(false, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Approves or declines a phone waiting to pair. Approved, it has full
    /// access, and the pane says it is connecting until it is in the list.
    /// Either way a fresh code follows, the one it scanned being spent.
    fn decide_pairing(&mut self, id: u64, approve: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.phones_pane() else {
            return;
        };
        if pane.deciding.is_some() {
            return;
        }
        let name = pane
            .pairings
            .iter()
            .find(|pairing| pairing.id == id)
            .map_or_else(|| "A phone".to_owned(), |pairing| pairing.name.clone());
        let scoped_roles = pane.scoped_roles;
        pane.deciding = Some(id);
        let role = approve.then_some(DeviceRole::Administrator);
        let work = cx.background_executor().spawn(async move {
            host::decide_pairing(id, role, scoped_roles).map_err(|why| why.to_string())
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Some(pane) = shell.phones_pane() {
                    pane.deciding = None;
                    pane.pairings.retain(|pairing| pairing.id != id);
                    if approve && matches!(outcome, Ok(true)) {
                        pane.connecting = Some((name.clone(), now()));
                    }
                }
                match outcome {
                    // Approved, the phone arrives in the list in a moment and
                    // says so itself; declined, it is told.
                    Ok(true) if !approve => {
                        shell.toast_detail(Tone::Info, "Pairing declined", name, cx);
                    }
                    Ok(true) => {}
                    Ok(false) => {
                        shell.toast_detail(Tone::Info, "That phone stopped waiting", name, cx)
                    }
                    Err(why) => shell.toast_detail(Tone::Error, "Could not decide", why, cx),
                }
                shell.refresh_phones(true, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A phone waiting to pair: its name, the code it shows, and the choice.
    fn approval_view(
        &self,
        pairing: &PendingPairing,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = pairing.id;
        let busy = self
            .preferences
            .as_ref()
            .is_some_and(|view| view.phones.deciding == Some(id));
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.0))
            .child(banner(
                "phones-approve",
                Tone::Warning,
                format!(
                    "“{}” wants to pair with this desktop. Approve it only if the phone shows \
                     the same code.",
                    pairing.name
                ),
                t,
            ))
            .child(
                div()
                    .text_size(px(28.0))
                    .text_color(paint(t.text.primary))
                    .child(pairing.code.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        button("phones-approve-yes", "Approve")
                            .primary()
                            .loading_if(busy, "Approving…")
                            .render(t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_pairing(id, true, cx);
                            })),
                    )
                    .child(
                        button("phones-approve-no", "Decline")
                            .enabled(!busy)
                            .render(t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_pairing(id, false, cx);
                            })),
                    )
                    .child(caption(format!("Waiting {} s more", pairing.expires_in), t)),
            )
            .into_any_element()
    }

    /// Backs out of a Revoke that is asking to be sure, or closes the Pair a
    /// device popover. Returns whether either was open, so Escape can do this
    /// before it closes the settings view.
    pub(crate) fn cancel_phone_prompt(&mut self) -> bool {
        self.phones_pane().is_some_and(|pane| {
            pane.confirming.take().is_some() | std::mem::take(&mut pane.pairing_open)
        })
    }

    /// The Devices pane: this desktop, the devices paired with it, and a way
    /// to pair another.
    ///
    /// Only the headings are found by a search. What is under them is live —
    /// a code and a device list the pane fetches while it is the one showing
    /// — so a result offers the heading, and its section heading opens the
    /// pane where those are current.
    pub(crate) fn phone_settings(
        &self,
        t: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let pane = &view.phones;
        let first = pane.devices.is_empty() && pane.devices_error.is_none();

        // What the section is for is the pane header's line, so the pane
        // opens on the desktop's own row.
        let mut out = vec![Entry::setting(
            "Desktop turn on enable relay wifi network listening Devices iPhone iPad pair \
                 phone tablet mobile remote",
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(group_label("Desktop", None, t))
                .child(group_box(t).child(self.desktop_row(pane, t, cx))),
        )];

        let trigger = (!first && pane.host == Some(HostState::Serving)).then(|| {
            let open = pane.pairing_open;
            let button = button("phones-pair", "Pair a device")
                .leading(Icon::Plus)
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    let Some(pane) = this.phones_pane() else {
                        return;
                    };
                    pane.pairing_open = true;
                    let want_code = pane.pairing.is_none();
                    if want_code {
                        this.refresh_phones(true, cx);
                    }
                    cx.notify();
                }))
                .into_any_element();
            let content = open.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(POPOVER_PAD)
                    .child(self.pairing_block(pane, true, t, cx))
                    // A press anywhere else puts it away; the trigger only
                    // opens, so this closing first never fights it.
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if let Some(pane) = this.phones_pane() {
                            pane.pairing_open = false;
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            });
            popup::anchored(
                "phones-pair-popup",
                button,
                content,
                Placement::BelowEnd,
                POPOVER_W,
                window,
                t,
            )
        });

        let count = (!pane.devices.is_empty()).then_some(pane.devices.len());
        let body = if let Some(why) = &pane.devices_error {
            error(why.clone(), t).into_any_element()
        } else if first {
            // Nothing paired yet: the code is the page, not a button away.
            group_box(t)
                .p(px(20.0))
                .child(self.pairing_block(pane, false, t, cx))
                .into_any_element()
        } else {
            group_box(t)
                .children(
                    pane.devices
                        .iter()
                        .map(|device| self.device_row(pane, device, t, cx)),
                )
                .into_any_element()
        };
        out.push(Entry::setting(
            "Devices paired revoke disconnect iPhone iPad pairing code QR scan",
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .pt(px(16.0))
                .child(group_label("Devices", count, t).children(trigger))
                .child(body),
        ));

        out.push(Entry::layout(div().pt(px(16.0)).child(banner(
            "phones-note",
            Tone::Info,
            "Only devices you pair can connect, and only on this Wi-Fi. Everything between \
             them is end-to-end encrypted, and revoking one disconnects it at once.",
            t,
        ))));
        out
    }

    /// This desktop: its name on the network, whether devices can reach it,
    /// and the switch that decides.
    fn desktop_row(&self, pane: &PhonesPane, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.preferences.as_ref() else {
            return div().into_any_element();
        };
        // The switch shows what was chosen; a host still serving devices from
        // a relay named in its environment shows as on too.
        let on = view.config.phones.enabled || pane.host == Some(HostState::Serving);
        let writable = view.writable && !pane.switching;
        let control = switch("phones-enabled", on, t).when(writable, |el| {
            el.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_phones_enabled(!on, cx);
                    cx.notify();
                }))
        });

        let state = if pane.switching {
            caption(
                if on {
                    "Turning on…"
                } else {
                    "Turning off…"
                },
                t,
            )
            .into_any_element()
        } else if pane.host == Some(HostState::Serving) {
            tinted_tag("Listening", paint(t.status.running)).into_any_element()
        } else {
            tag("Off", t).into_any_element()
        };

        let name = pane
            .desktop
            .clone()
            .unwrap_or_else(|| "This desktop".to_owned());
        let (stem, domain) = match name.strip_suffix(".local") {
            Some(stem) => (stem.to_owned(), ".local"),
            None => (name, ""),
        };

        div()
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .h(px(40.0))
            .px(PAD_X)
            .child(icon(Icon::Monitor, paint(t.text.primary)))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .truncate()
                    .font_family(self.font_family.clone())
                    .text_size(LABEL)
                    .text_color(paint(t.text.primary))
                    .child(stem)
                    .child(div().text_color(paint(t.text.dim)).child(domain)),
            )
            .child(div().flex_1())
            .child(state)
            .child(control)
            .into_any_element()
    }

    /// The code, or why there is none. `stacked` sets the code above its
    /// words, for the popover once something is paired; otherwise it sits
    /// beside the steps, as the whole of the Devices group.
    fn pairing_block(
        &self,
        pane: &PhonesPane,
        stacked: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &pane.host {
            None => working("Asking the ket host…".to_owned(), CAPTION, t).into_any_element(),
            _ if pane.switching => working("One moment…".to_owned(), CAPTION, t).into_any_element(),
            Some(HostState::Off) => caption(
                "Turn on the desktop above, then scan the code that appears here.",
                t,
            )
            .into_any_element(),
            Some(HostState::NoHost) => banner(
                "phones-no-host",
                Tone::Info,
                "Devices reach this desktop through the ket host, and this window was \
                 started with KET_HOST=0. Start ket without it to pair a device."
                    .to_owned(),
                t,
            )
            .into_any_element(),
            Some(HostState::Unreachable(why)) => div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(10.0))
                .child(banner(
                    "phones-unreachable",
                    Tone::Error,
                    format!("Could not reach the ket host: {why}"),
                    t,
                ))
                .child(
                    button("phones-retry", "Try again")
                        .leading(Icon::Refresh)
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_phones(true, cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
            Some(HostState::Serving) if !pane.pairings.is_empty() => {
                self.approval_view(&pane.pairings[0], t, cx)
            }
            // Approved, and on its way: said rather than going straight back
            // to a fresh code, which read as nothing having happened.
            Some(HostState::Serving) if pane.connecting.is_some() => {
                let name = pane
                    .connecting
                    .as_ref()
                    .map_or("the device", |(name, _)| name.as_str());
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(working(format!("Connecting “{name}”…"), LABEL, t))
                    .child(caption(
                        "Approved. The device is finishing on its side and appears here \
                         in a few seconds.",
                        t,
                    ))
                    .into_any_element()
            }
            Some(HostState::Serving) => match (&pane.pairing, &pane.pairing_error) {
                (Some(pairing), _) => self.pairing_code_view(pairing, stacked, t, cx),
                (None, Some(why)) => div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(10.0))
                    .child(banner(
                        "phones-code-failed",
                        Tone::Error,
                        format!("The host could not make a code: {why}"),
                        t,
                    ))
                    .child(
                        button("phones-new-code", "Try again")
                            .leading(Icon::Refresh)
                            .primary()
                            .render(t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_pairing_code(cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
                (None, None) => working("Making a code…".to_owned(), CAPTION, t).into_any_element(),
            },
        }
    }

    /// The QR code beside what to do with it, or above it when `stacked`.
    /// The code is the same size either way.
    ///
    /// No countdown and no New code: the pane swaps in a fresh code shortly
    /// before this one runs out, and again once one is used, so the square on
    /// screen is always one that pairs.
    fn pairing_code_view(
        &self,
        pairing: &Pairing,
        stacked: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let picture = match &pairing.modules {
            Some(modules) => div()
                .flex_none()
                .child(qr_code(modules.clone(), QR, t))
                .into_any_element(),
            None => div()
                .flex_none()
                .w(QR)
                .child(caption(
                    "This code is too long to draw. Copy it instead.",
                    t,
                ))
                .into_any_element(),
        };

        let copy = button("phones-copy", "Copy code")
            .leading(Icon::Copy)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.copy_pairing_code(cx);
                cx.notify();
            }));

        if stacked {
            return div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .prose()
                                .text_size(LABEL)
                                .text_color(paint(t.text.primary))
                                .child("Pair a device"),
                        )
                        .child(caption(
                            "Scan with the camera on your iPhone or iPad. One code pairs \
                             one device.",
                            t,
                        )),
                )
                .child(picture)
                .child(working(
                    "Waiting for a device to scan…".to_owned(),
                    CAPTION,
                    t,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(copy)
                        .child(caption("No camera? Paste it into ket.", t)),
                )
                .into_any_element();
        }

        let step = |n: usize, text: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_none()
                        .size(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(RADIUS_SM)
                        .bg(paint(t.hover))
                        .font_family(self.font_family.clone())
                        .text_size(CAPTION)
                        .text_color(paint(t.text.primary))
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .prose()
                        .text_size(LABEL)
                        .text_color(paint(t.text.primary))
                        .child(text),
                )
        };

        let words = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .prose()
                            .text_size(TITLE)
                            .text_color(paint(t.text.primary))
                            .child("Pair your iPhone or iPad"),
                    )
                    .child(caption(
                        "Scan this code with the device's camera. It shows up here once \
                             it is paired.",
                        t,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(step(1, "Install ket on your iPhone or iPad"))
                    .child(step(2, "Open the Camera and point it at this code"))
                    .child(step(3, "Tap the link — ket opens, paired")),
            )
            .child(working(
                "Waiting for a device to scan…".to_owned(),
                CAPTION,
                t,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(copy)
                    .child(caption("No camera? Paste it into ket on the device.", t)),
            );

        div()
            .flex()
            .items_center()
            .gap(px(28.0))
            .child(picture)
            .child(words.flex_1().min_w_0())
            .into_any_element()
    }

    /// One paired device: what it is, when it paired, whether it is
    /// connected, and Revoke — which asks once before it does anything.
    fn device_row(
        &self,
        pane: &PhonesPane,
        device: &DeviceEntry,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = device.id.clone();
        let revoking = pane.revoking.as_deref() == Some(id.as_str());
        let confirming = pane.confirming.as_deref() == Some(id.as_str());
        let kind = device_kind(device);

        // Just paired and not in yet: the phone is dialling back in, which
        // is a moment's work, not a device that is away.
        let settling = !device.connected
            && pane.host == Some(HostState::Serving)
            && now().saturating_sub(device.paired_at) < SETTLING;
        let state = if device.connected {
            tinted_tag("Connected", paint(t.status.running)).into_any_element()
        } else if settling {
            working("Connecting…".to_owned(), CAPTION, t).into_any_element()
        } else {
            tag("Not connected", t).into_any_element()
        };
        let actions = if revoking {
            button(key("phones-revoking", &id), "Revoking…")
                .danger()
                .enabled(false)
                .render(t)
                .into_any_element()
        } else if confirming {
            let confirm_id = id.clone();
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(caption("Revoke this device?", t))
                .child(
                    button(key("phones-revoke-cancel", &id), "Cancel")
                        .ghost()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_phone_prompt();
                            cx.notify();
                        })),
                )
                .child(
                    button(key("phones-revoke-confirm", &id), "Revoke")
                        .danger()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.revoke_phone(confirm_id.clone(), cx);
                        })),
                )
                .into_any_element()
        } else {
            let ask_id = id.clone();
            button(key("phones-revoke", &id), "Revoke")
                .danger()
                .enabled(pane.revoking.is_none())
                .render(t)
                .when(pane.revoking.is_none(), |el| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(pane) = this.phones_pane() {
                            pane.confirming = Some(ask_id.clone());
                        }
                        cx.notify();
                    }))
                })
                .into_any_element()
        };

        let detail = match kind {
            Some((_, kind)) => format!("{kind} · {}", paired_ago(device.paired_at).to_lowercase()),
            None => paired_ago(device.paired_at),
        };

        sized_row(key("phones-device", &id), false, px(40.0), t)
            .cursor_default()
            .child(icon(
                kind.map_or(Icon::Smartphone, |(mark, _)| mark),
                paint(t.text.dim),
            ))
            .child(
                div()
                    .flex_none()
                    .font_family(self.font_family.clone())
                    .child(phone_name(device)),
            )
            .child(caption(detail, t).truncate().min_w_0())
            .child(div().flex_1())
            .child(state)
            .child(actions)
            .into_any_element()
    }
}

/// A line saying something is under way: a breathing dot before the words,
/// so a wait reads as one rather than as a page that stopped.
fn working(text: String, size: Pixels, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(status_dot(paint(t.status.running), Motion::Travel, px(6.0)))
        .child(
            div()
                .prose()
                .text_size(size)
                .text_color(paint(t.text.dim))
                .child(text),
        )
}

/// What kind of device this is, from the name it gave: iOS names a device
/// after its model unless its owner renamed it. `None` when the name does
/// not say.
fn device_kind(device: &DeviceEntry) -> Option<(Icon, &'static str)> {
    let name = device.name.to_lowercase();
    if name.contains("ipad") {
        Some((Icon::Tablet, "iPad"))
    } else if name.contains("iphone") {
        Some((Icon::Smartphone, "iPhone"))
    } else {
        None
    }
}

/// A group's label: "Desktop", "Devices" and its count, and whatever the
/// caller puts at the trailing end.
fn group_label(text: &'static str, count: Option<usize>, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .min_h(CONTROL_H)
        .text_size(LABEL)
        .text_color(paint(t.text.dim))
        .child(text)
        .children(count.map(|count| caption(count.to_string(), t)))
        .child(div().flex_1())
}

/// The bordered box a group's rows sit in — the settings view's card.
fn group_box(t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .p(px(4.0))
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(paint(t.border))
        .bg(paint(t.elevated))
}

/// An element id for one of a phone's buttons. Named for the phone, because
/// the rows are not stateful and their buttons share one id scope.
fn key(what: &str, id: &str) -> SharedString {
    format!("{what}-{id}").into()
}

/// Whether a pairing is in motion, so the pane asks the host every tick: a
/// code on show — the whole group before anything is paired, or the popover
/// — a phone waiting to be approved, or one connecting.
fn watching(pane: &PhonesPane) -> bool {
    let code_on_show = pane.pairing.is_some() && (pane.devices.is_empty() || pane.pairing_open);
    let settling = pane
        .devices
        .iter()
        .any(|device| !device.connected && now().saturating_sub(device.paired_at) < SETTLING);
    code_on_show || !pane.pairings.is_empty() || pane.connecting.is_some() || settling
}

/// The pane's clock: swaps a code about to run out while the Devices
/// section is showing, and asks the host again every [`REFRESH_TICKS`].
/// Stops when the settings view closes, because that drops the task.
async fn tick(shell: WeakEntity<Shell>, cx: &mut gpui::AsyncApp) {
    let mut ticks = 0u32;
    loop {
        cx.background_executor().timer(TICK).await;
        let showing = shell.update(cx, |shell, cx| {
            let Some(view) = shell.preferences.as_ref() else {
                return false;
            };
            if view.section != Section::Devices {
                return true;
            }
            ticks = ticks.wrapping_add(1);
            let stale = view
                .phones
                .pairing
                .as_ref()
                .is_some_and(|pairing| expiring(pairing.expires_at));
            // An approval that never turned into a device: say so, rather
            // than leave the pane saying it is connecting forever.
            let lapsed = view
                .phones
                .connecting
                .as_ref()
                .filter(|(_, since)| now().saturating_sub(*since) >= CONNECT_TIMEOUT)
                .map(|(name, _)| name.clone());
            if let Some(name) = lapsed {
                if let Some(pane) = shell.phones_pane() {
                    pane.connecting = None;
                }
                shell.toast_detail(
                    Tone::Error,
                    format!("“{name}” didn't finish pairing"),
                    "Scan a fresh code on the device to try again.",
                    cx,
                );
            }
            let Some(view) = shell.preferences.as_ref() else {
                return false;
            };
            // Every tick while a pairing is in motion — a code on show, a
            // phone to approve, one connecting — so each step shows within a
            // second rather than on the slower round.
            if stale || watching(&view.phones) || ticks.is_multiple_of(REFRESH_TICKS) {
                let want_code = stale || view.phones.pairing.is_none();
                shell.refresh_phones(want_code, cx);
            }
            true
        });
        if !matches!(showing, Ok(true)) {
            return;
        }
    }
}
