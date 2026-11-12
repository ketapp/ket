//! Telling people when a newer ket is out.
//!
//! A release build checks its channel on the schedule chosen in Settings →
//! General (see `ket_core::update`), the first time thirty seconds after
//! launch so it never holds up startup. A newer build is announced once, as
//! an alert with Download, and shown in Settings and the release card until
//! this build is replaced. A failed check is logged and waits for the next
//! scheduled one. Dev builds never check.
//!
//! The schedule is the shell's, and there is one shell: ket opens a second
//! window only once the first has gone. What a reopened window must not
//! repeat — the launch check — is remembered for the process, and the rest
//! is in `ket_core::update::Record`, on disk.

use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Task, WeakEntity, div, prelude::*, px};
use ket_core::build_info::{self, Channel};
use ket_core::config::UpdateCheck;
use ket_core::update::{self, DOWNLOAD_URL, Record, Status};

use crate::Shell;
use crate::preferences::{Entry, setting};
use crate::toasts::Alert;
use crate::ui::banner::banner;
use crate::ui::button::button;
use crate::ui::toast::Tone;

/// How long after launch the first check waits, so it never holds up startup.
const LAUNCH_DELAY: Duration = Duration::from_secs(30);

/// The longest the schedule sleeps before asking the clock again. The
/// schedule is by the calendar, and a Mac asleep stops the clock a timer
/// runs on, so one week-long timer would fire days late.
const WAKE_EVERY: Duration = Duration::from_secs(60 * 60);

/// When this process started looking, for [`LAUNCH_DELAY`].
static LAUNCHED: OnceLock<Instant> = OnceLock::new();

/// Whether this launch has checked: [`UpdateCheck::OnLaunch`] checks once per
/// launch, not once per window.
static CHECKED_THIS_LAUNCH: AtomicBool = AtomicBool::new(false);

/// What the shell knows about updates.
#[derive(Default)]
pub(crate) struct Updates {
    /// How often to check, from Settings.
    every: UpdateCheck,
    /// What earlier checks found, kept on disk.
    record: Record,
    /// A check is under way.
    checking: bool,
    /// The last check this run made failed.
    failed: bool,
    /// When this run last tried, answered or not.
    tried_at_ms: Option<u64>,
    /// The wait for the next scheduled check. Replacing it cancels the old.
    next: Option<Task<()>>,
}

/// Whether this build can check at all.
fn checks() -> bool {
    build_info::CHANNEL != Channel::Dev
}

impl Shell {
    /// Reads what earlier checks found and starts the schedule. Called once,
    /// as the shell loads.
    pub(crate) fn start_update_checks(&mut self, every: UpdateCheck, cx: &mut Context<Self>) {
        if !checks() {
            return;
        }
        LAUNCHED.get_or_init(Instant::now);
        self.updates.record = Record::load();
        self.set_update_check(every, cx);
    }

    /// Follows a change to how often to check.
    pub(crate) fn set_update_check(&mut self, every: UpdateCheck, cx: &mut Context<Self>) {
        self.updates.every = every;
        self.schedule_update_check(cx);
    }

    /// How long until the next scheduled check, or `None` for none.
    fn update_wait(&self) -> Option<Duration> {
        let updates = &self.updates;
        let last = updates.record.checked_at_ms.max(updates.tried_at_ms);
        let wait = update::next_check(
            updates.every,
            last,
            ket_core::now_ms(),
            CHECKED_THIS_LAUNCH.load(Ordering::Relaxed),
        )?;
        let since_launch = LAUNCHED.get_or_init(Instant::now).elapsed();
        Some(wait.max(LAUNCH_DELAY.saturating_sub(since_launch)))
    }

    /// Waits for the next scheduled check, in place of any wait already
    /// running.
    fn schedule_update_check(&mut self, cx: &mut Context<Self>) {
        let Some(mut wait) = self.update_wait() else {
            self.updates.next = None;
            return;
        };
        self.updates.next = Some(cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            while !wait.is_zero() {
                cx.background_executor().timer(wait.min(WAKE_EVERY)).await;
                match shell.update(cx, |shell, _| shell.update_wait()) {
                    Ok(Some(left)) => wait = left,
                    _ => return,
                }
            }
            let _ = shell.update(cx, |shell, cx| shell.check_for_update(cx));
        }));
    }

    /// Asks the channel now, off the window's thread. Settings' Check now,
    /// and the schedule when a check is due.
    pub(crate) fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if !checks() || self.updates.checking {
            return;
        }
        self.updates.checking = true;
        self.updates.tried_at_ms = Some(ket_core::now_ms());
        CHECKED_THIS_LAUNCH.store(true, Ordering::Relaxed);
        let work = cx.background_executor().spawn(async { update::check() });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let found = work.await;
            let _ = shell.update(cx, |shell, cx| shell.apply_update_check(found, cx));
        })
        .detach();
        cx.notify();
    }

    /// Keeps what a check found, says so once if it is new, and waits for
    /// the next one.
    fn apply_update_check(&mut self, found: ket_core::Result<Status>, cx: &mut Context<Self>) {
        let updates = &mut self.updates;
        updates.checking = false;
        updates.failed = found.is_err();
        let mut announce = None;
        match found {
            Ok(status) => {
                let record = &mut updates.record;
                record.checked_at_ms = Some(ket_core::now_ms());
                record.available = match status {
                    Status::UpToDate => None,
                    Status::Available(release) => {
                        if release.build > record.announced {
                            record.announced = release.build;
                            announce = Some(release.version.clone());
                        }
                        Some(release)
                    }
                };
                if let Err(e) = record.save() {
                    tracing::warn!(%e, "could not save what the update check found");
                }
            }
            Err(e) => tracing::warn!(%e, "update check failed"),
        }
        if let Some(version) = announce {
            self.alert(
                Tone::Info,
                format!("ket {version} is available"),
                format!("You have {}.", build_info::VERSION),
                Alert {
                    subject: "update-available".into(),
                    action: Some((
                        "Download".into(),
                        Rc::new(|_: &mut Shell, cx: &mut Context<Shell>| {
                            cx.open_url(DOWNLOAD_URL);
                        }),
                    )),
                },
                cx,
            );
        }
        self.schedule_update_check(cx);
        cx.notify();
    }

    /// Settings → General's row under Check for updates: what the last
    /// check found, Download when there is something newer, and Check now.
    pub(crate) fn update_status_setting(&self, cx: &mut Context<Self>) -> Entry {
        let t = &self.theme;
        let updates = &self.updates;
        let newer = updates.record.newer();
        let said = match (checks(), newer, updates.record.checked_at_ms) {
            (false, _, _) => "Updates are off for development builds.".to_owned(),
            (true, Some(release), _) => format!("ket {} is available.", release.version),
            _ if updates.failed => "Could not check for updates.".to_owned(),
            (true, None, Some(at)) => format!("Up to date \u{b7} checked {}", ago(at)),
            (true, None, None) => "Not checked yet.".to_owned(),
        };
        let download = newer.map(|_| {
            button("updates-download", "Download")
                .small()
                .primary()
                .render(t)
                .on_click(|_, _, cx| cx.open_url(DOWNLOAD_URL))
        });
        let check_now = button("updates-check-now", "Check now")
            .small()
            .enabled(checks())
            .loading_if(updates.checking, "Checking\u{2026}")
            .render(t)
            .when(checks(), |el| {
                el.on_click(cx.listener(|this, _, _, cx| this.check_for_update(cx)))
            });
        let controls = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .children(download)
            .child(check_now)
            .into_any_element();
        setting(
            &format!("ket {}", build_info::display()),
            &said,
            controls,
            t,
        )
    }

    /// The release card's line when a newer ket is out, with Download.
    pub(crate) fn release_update_line(&self) -> Option<AnyElement> {
        let release = self.updates.record.newer()?;
        let t = &self.theme;
        Some(
            div()
                .mx(px(12.0))
                .mt(px(12.0))
                .child(
                    banner(
                        "release-update",
                        Tone::Info,
                        format!("ket {} is available", release.version),
                        t,
                    )
                    .items_center()
                    .child(
                        button("release-download", "Download")
                            .small()
                            .primary()
                            .render(t)
                            .on_click(|_, _, cx| cx.open_url(DOWNLOAD_URL)),
                    ),
                )
                .into_any_element(),
        )
    }
}

/// `just now`, or `3h 12m ago`.
fn ago(at_ms: u64) -> String {
    let age = ket_core::now_ms().saturating_sub(at_ms);
    if age < 60_000 {
        return "just now".to_owned();
    }
    format!("{} ago", crate::status_bar::compact_duration(age))
}
