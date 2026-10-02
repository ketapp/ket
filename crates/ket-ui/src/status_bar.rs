//! The strip along the bottom of the window.
//!
//! Agent quota, which is what it was asked for: how much of each provider's
//! plan is used and when it resets. It is a strip of *sections* rather than a
//! fixed row, so what else lands here later — branch, background work — is a
//! section rather than a rewrite.
//!
//! Session usage lived here for a while and has gone to the sidebar — see
//! `crate::usage_card`. It never belonged: a quota is an account-wide fact and
//! reads fine on a strip addressed to nothing in particular, while what one
//! session is spending is about a row, and a row is a thing you can point at.
//!
//! **What it must never do is show a number it does not have.** A provider whose
//! fetch failed reports [`SnapshotStatus::Unavailable`], and rendering that as
//! `0%` would say "plenty of headroom" about a provider ket cannot currently see.
//! `ket-core` went to some trouble to keep those four states distinct; the whole
//! benefit is lost if the bar collapses them back into a percentage.

use gpui::{
    AnyElement, Context, Div, FontFeatures, FontWeight, Pixels, SharedString, Window, div,
    prelude::*, px,
};
use ket_core::rate_limits::{Provider, ProviderSnapshot, SnapshotStatus};
use ket_core::theme::{Color, Theme};

use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::ui::CAPTION;
use crate::ui::agent;
use crate::ui::button::icon_button;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::popup::{self, Placement};
use crate::ui::progress::progress;
use crate::{PopupKind, Shell};

/// The shell's periodic tick: how often the bar re-reads the quota cache, and
/// the cadence of everything else `Shell::poll_tick` keeps current.
///
/// The cache refreshes on its own thread; this only decides how soon the window
/// notices. Ten seconds rather than five: each tick stamps every worktree's git
/// directory, reads the session store and refreshes open editors, and none of
/// what it keeps current is something a person watches change second by
/// second. The selected worktree's files are watched rather than ticked — see
/// `crate::git_watch`.
pub(crate) const POLL: std::time::Duration = std::time::Duration::from_secs(10);

impl Shell {
    /// Shows or hides the status bar.
    ///
    /// The quota refresh keeps running either way: the figures it feeds are
    /// in the header now, which is always on screen.
    ///
    /// Rebuilds the menu bar too, since the View menu's item says which of
    /// the two it will do.
    pub(crate) fn set_status_bar(&mut self, open: bool, cx: &mut Context<Self>) {
        self.status_bar_open = open;
        // The feedback popover hangs off the bar; with the bar gone it would
        // be a form nobody can see, still taking the keys.
        if !open {
            self.dismiss_feedback();
        }
        cx.set_menus(crate::app_menus(open));
        cx.notify();
    }
}

/// One reading on the strip, reduced to what fits on a single line.
struct Section {
    /// Whose reading it is. The provider rather than its name, because the
    /// strip draws the mark and the mark is looked up from the provider —
    /// see [`crate::ui::agent::provider`].
    provider: Provider,
    /// `"15%"`, or a word when there is no percentage to give.
    ///
    /// Without " used": a percentage on a quota strip is already a share of
    /// something spent, and the word was the third thing in every section and
    /// the least of them.
    headline: SharedString,
    /// `"4h 41m"` until the window resets, when a reset is known.
    resets_in: Option<SharedString>,
    /// Which window the figure is for — `"Session"`, `"Weekly"` — when there
    /// is a figure.
    span: Option<SharedString>,
    /// Whether the reading is old enough to say so.
    stale: bool,
    /// How full the window is, for the ramp; `None` when there is no number.
    used: Option<f32>,
    /// A provider with no quota turning requests away — Grok, when its API
    /// has told it to slow down. The loudest thing the strip can say, in
    /// the ramp's last colour, with no number beside it.
    limited: bool,
}

/// Renders a duration the way a status bar should: two units, no seconds.
///
/// `16d 9h`, `4h 41m`, `12m`. Seconds on a quota that resets in four hours is
/// noise that changes every tick and tells nobody anything.
pub(crate) fn compact_duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    let (days, rest) = (minutes / 1_440, minutes % 1_440);
    let (hours, mins) = (rest / 60, rest % 60);

    match (days, hours, mins) {
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// The window a person actually cares about: whichever is closest to its limit.
///
/// A provider can report several concurrent windows — a session one and a weekly
/// one, say — and the binding constraint is the one that will stop you first.
/// Showing the *first* window instead would be showing whichever the provider
/// happened to list first.
fn tightest(snapshot: &ProviderSnapshot) -> Option<&ket_core::rate_limits::RateWindow> {
    snapshot
        .windows
        .iter()
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
}

/// Reduces a snapshot to a line, or `None` if it has nothing to say.
///
/// The only event a window has is its reset, and the only colour on the line
/// is the percentage's ramp.
fn section(snapshot: &ProviderSnapshot, now_ms: u64) -> Option<Section> {
    let provider = snapshot.provider;

    // Being turned away outranks any share of an allowance: a Grok at 3% of
    // its week that the API has told to slow down is stopped all the same,
    // and "3%" would say the opposite.
    if snapshot.limited_at_ms.is_some() {
        return Some(Section {
            provider,
            headline: "rate limited".into(),
            resets_in: None,
            span: None,
            stale: false,
            used: None,
            limited: true,
        });
    }

    match &snapshot.status {
        // A provider with no quota concept is omitted rather than shown as a
        // permanent "n/a" — a row that can only ever say nothing is noise in a
        // strip this small. OpenCode reports this until an OpenCode Go key is
        // configured — pasted into settings or set on `OPENCODE_API_KEY` —
        // because bring-your-own-key really has no plan limit; once one is it
        // reports windows like anyone else and lands in the strip through the
        // arm below, with no case here for it.
        SnapshotStatus::Unsupported => None,

        // Named, but never with a number. "Unavailable" and "0% used" must not
        // look alike.
        SnapshotStatus::Unavailable { .. } => Some(Section {
            provider,
            headline: "unavailable".into(),
            resets_in: None,
            span: None,
            stale: true,
            used: None,
            limited: false,
        }),

        status => {
            let window = tightest(snapshot)?;

            Some(Section {
                provider,
                headline: format!("{:.0}%", window.used_percent).into(),
                resets_in: resets_in(window, now_ms),
                span: Some(window.name.clone().into()),
                stale: matches!(status, SnapshotStatus::Stale),
                used: Some(used_percent(window)),
                limited: false,
            })
        }
    }
}

// ---- popup metrics ---------------------------------------------------------
//
// The popup is chrome, not a screen. Its first drawing was laid out like a
// settings page: 460px wide, 18px gutters, a 36px segmented control, and three
// stacked elements per quota window — a label line, a full-width bar, and a
// reset line. That put a panel taller than the editor over the window in order
// to say six numbers, and none of it was at the density of the menus it opens
// beside. These metrics compressed that to the scale of the rest of the
// chrome: one line per quota, with the bar as the thing the eye lands on.
//
// The Detailed view has since gone back to a label/bar/stats block per
// window — see `usage_window` — because a reset time read as a fourth column
// squeezed into 304px, and it is worth more as a line of its own than as a
// number sharing a row with three others.
//
// And back once more. The Detailed view is one line per window again, at a
// panel wide enough for it: the reset time that would not fit as a fourth
// column in 304px fits in 560px with room to spare, and a window read left to
// right — name, bar, figure, reset — is the shape a person scans a list of
// them in. The Compact view that sat beside it is gone: at one line per
// window the Detailed view is short enough on its own.

/// The panel's width.
///
/// Wide, and allowed to be: this is a panel opened to read numbers off, not a
/// menu, and it sits over a terminal that is wider still. The width is what
/// lets a window be one line — a name column, a bar long enough to
/// read a length off, a figure and a reset, each in its own column — where
/// 304px forced the three numbers to stack.
const POPUP_W: Pixels = px(560.0);

/// The panel's gutter, and the inset of every row inside it.
const PAD: Pixels = px(16.0);

/// The header, which holds the title, the reading's age and refresh.
const HEADER_H: Pixels = px(44.0);

/// A provider's heading line: mark, name, plan.
const PROVIDER_H: Pixels = px(34.0);

/// A window bar's thickness.
///
/// Continuous and six pixels tall, so it reads as a length rather than as a
/// slab on a row this short. It has a figure beside it to be exact for it.
const BAR_H: Pixels = px(6.0);

/// A window row's height, which sets the rhythm of a provider's list.
const DETAIL_ROW_H: Pixels = px(30.0);

/// The gap between a window row's columns.
const ROW_GAP: Pixels = px(16.0);

/// The window-name column, so every bar starts at the
/// same x down the panel. Wide enough for `Weekly Fable`.
const NAME_COL: Pixels = px(116.0);

/// The figure column, right-aligned, wide enough for `100%`
/// at [`ROW_TEXT`].
const FIGURE_COL: Pixels = px(44.0);

/// The trailing column — `Resets in 23d 10h` — right-aligned
/// so the reset times line up down the panel. Spelled out on every row
/// rather than headed once: a column head is one more thing to look up to.
const TRAIL_COL: Pixels = px(124.0);

/// The panel's title.
const TITLE_TEXT: Pixels = px(14.0);

/// A provider's name.
const PROVIDER_TEXT: Pixels = px(13.5);

/// A window's name and figure: the things read off a row.
const ROW_TEXT: Pixels = px(13.0);

/// A window's reset.
const RESET_TEXT: Pixels = px(12.5);

/// Metadata: the reading's age, a plan, a reason. A step under [`ROW_TEXT`],
/// because these are things a person reads once and then stops seeing.
const META: Pixels = px(12.0);

/// The header's quota bar: long enough that a tenth of it is a visible
/// step. Both ends are rounded, which keeps a 4% fill reading as a mark on a
/// track rather than a sliver clipped off the end of one.
const HEADER_BAR_W: Pixels = px(56.0);

/// Its thickness.
const HEADER_BAR_H: Pixels = px(4.0);

/// The compact header's quota bar, in a read-out.
const COMPACT_BAR_W: Pixels = px(32.0);

/// Where a quota starts to be worth a glance.
const WARM: f32 = 70.0;

/// Where it starts to be worth planning around.
const HOT: f32 = 80.0;

/// Where it is about to stop.
const CRITICAL: f32 = 90.0;

/// The colour a quota at `used` percent is highlighted in, if it is.
///
/// `None` below [`WARM`]: a quota that is fine is not news, and a bar or a
/// figure with nothing to say keeps the ink everything else around it uses.
/// Past that the three steps of [`ket_core::theme::QuotaTokens`], which
/// start earlier and climb finer than the two-step `attention`/`failed`
/// ramp this used to borrow — see that type for why. `status.running`'s
/// green is deliberately not in the ramp: green already means *a session is
/// working* everywhere else in the shell, and a green quota bar would be a
/// second, unrelated meaning for one colour.
fn pressure(used: f32, t: &Theme) -> Option<Color> {
    if used >= CRITICAL {
        Some(t.quota.critical)
    } else if used >= HOT {
        Some(t.quota.hot)
    } else if used >= WARM {
        Some(t.quota.warm)
    } else {
        None
    }
}

/// A window's usage as a number safe to draw with.
///
/// A provider that reported a NaN would otherwise become a meter of
/// indeterminate width rather than a visible zero.
fn used_percent(window: &ket_core::rate_limits::RateWindow) -> f32 {
    if window.used_percent.is_finite() {
        window.used_percent.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// A window's bar in the panel: continuous, with a fill coloured by
/// [`pressure`]. What a bar adds beside a figure is *length*; the figure on
/// the same line is exact for it.
fn bar(used: f32, t: &Theme) -> Div {
    progress(used / 100.0)
        .grow()
        .height(BAR_H)
        .fill(paint(pressure(used, t).unwrap_or(t.accent)))
        .render(t)
}

/// A quota's length on the strip: [`bar`] at the strip's scale. The same
/// instrument as the panel's, which is the point — the strip and the panel it
/// opens should not be two different pictures of one number.
fn strip_bar(used: f32, width: Pixels, height: Pixels, t: &Theme) -> Div {
    progress(used / 100.0)
        .width(width)
        .height(height)
        .fill(paint(pressure(used, t).unwrap_or(t.accent)))
        .render(t)
}

/// The provider's mark, dimmed for a provider that has nothing to report.
///
/// A provider with no quota concept keeps a row in the panel so that its
/// absence from the status bar is explained, not because it has news. Drawing
/// its mark at full strength gives a nothing the same pull as a quota about to
/// run out. This is where an unconfigured OpenCode sits; once it is reporting
/// windows it is no longer quiet and is drawn like the others.
fn provider_mark(snapshot: &ProviderSnapshot, t: &Theme) -> gpui::Svg {
    let (which, tint) = agent::provider(snapshot.provider.label(), t);
    let tint = match quiet(snapshot) {
        true => alpha(tint, 0.45),
        false => tint,
    };
    sized_icon(which, agent::MARK, tint)
}

/// Whether a provider has nothing at all to report: no quota, and no spend
/// or refusal to show in its place.
fn quiet(snapshot: &ProviderSnapshot) -> bool {
    matches!(snapshot.status, SnapshotStatus::Unsupported)
        && snapshot.spend.is_empty()
        && snapshot.limited_at_ms.is_none()
}

/// A single quiet line, for everything a provider says that is not a number.
fn note(text: impl Into<SharedString>, t: &Theme) -> AnyElement {
    div()
        .truncate()
        .text_size(META)
        .text_color(paint(t.text.dim))
        .child(text.into())
        .into_any_element()
}

/// Digits set to one width, so a column of figures lines up by its digits
/// and a reset ticking from `9m` to `10m` does not shift the text beside it.
fn tabular(mut el: Div) -> Div {
    el.text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])));
    el
}

/// A reset, as the two units a status bar shows.
fn resets_in(window: &ket_core::rate_limits::RateWindow, now_ms: u64) -> Option<SharedString> {
    window
        .resets_at_ms
        .filter(|reset| *reset > now_ms)
        .map(|reset| SharedString::from(compact_duration(reset - now_ms)))
}

/// How old a reading is, as a person says it.
fn age(fetched_ms: u64, now_ms: u64) -> String {
    let age = now_ms.saturating_sub(fetched_ms);
    if age < 60_000 {
        "just now".to_owned()
    } else {
        format!("{} ago", compact_duration(age))
    }
}

/// The plan a reading was taken on, as a word: `plus` reads as `Plus`.
///
/// The account is deliberately not shown. A provider reports it as a UUID, and
/// thirty-six characters of hex beside a provider's name identify something
/// that never changes between two readings of the same panel.
fn plan_label(plan: &str) -> SharedString {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()).into(),
        None => SharedString::default(),
    }
}

/// One reported rate window, as a single line: name, bar, figure, reset.
///
/// A window read left to right — what it is, how full, the number, when it
/// turns over — is the order a person scans a list of them in. The reset is
/// spelled out as `Resets in …` on every row rather than headed once as a
/// column: a column head is one more thing to look up to.
fn usage_window(window: &ket_core::rate_limits::RateWindow, now_ms: u64, t: &Theme) -> AnyElement {
    let used = used_percent(window);

    // The reset, always, as `section` does it, and never coloured: the
    // figure carries the ramp and nothing else here competes with it.
    let trailing = resets_in(window, now_ms).map(|reset| format!("Resets in {reset}"));

    // A qualified window — `Weekly (Fable)` — as its name and a dimmer
    // qualifier, so the column reads as `Session`, `Weekly`, `Weekly`
    // down its left edge and the brackets stop competing with the words.
    let (base, qualifier) = match window
        .name
        .strip_suffix(')')
        .and_then(|name| name.split_once(" ("))
    {
        Some((base, qualifier)) => (base.to_owned(), Some(qualifier.to_owned())),
        None => (window.name.to_string(), None),
    };

    div()
        .flex()
        .items_center()
        .gap(ROW_GAP)
        .h(DETAIL_ROW_H)
        .child(
            div()
                .flex()
                .flex_none()
                .gap(px(4.0))
                .w(NAME_COL)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(ROW_TEXT)
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.primary))
                .child(base)
                .children(qualifier.map(|qualifier| {
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(paint(t.text.dim))
                        .child(qualifier)
                })),
        )
        .child(bar(used, t))
        .child(
            tabular(div())
                .flex()
                .flex_none()
                .w(FIGURE_COL)
                .justify_end()
                .text_size(ROW_TEXT)
                .font_weight(FontWeight::SEMIBOLD)
                // The figure takes the ramp's colour once the ramp has
                // something to say, and only then: a row where every figure
                // is coloured has no way left to point at one of them.
                .text_color(paint(pressure(used, t).unwrap_or(t.text.primary)))
                .child(format!("{used:.0}%")),
        )
        .child(
            tabular(div())
                .flex()
                .flex_none()
                .w(TRAIL_COL)
                .justify_end()
                .text_size(RESET_TEXT)
                .text_color(paint(t.text.dim))
                .children(trailing.map(|trailing| div().truncate().child(trailing))),
        )
        .into_any_element()
}

/// A count of tokens as a person reads one: `850`, `51k`, `1.2M`.
fn token_count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{}k", tokens / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// Dollars at the precision they are worth: cents, and none below a hundred
/// dollars' worth of them.
fn dollars(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "<$0.01".to_owned()
    } else if usd >= 100.0 {
        format!("${usd:.0}")
    } else {
        format!("${usd:.2}")
    }
}

/// One period of spend, on a window row's columns: the period, what went
/// through, and what it cost where that is known.
fn spend_row(spend: &ket_core::rate_limits::Spend, t: &Theme) -> AnyElement {
    let through = match spend.sessions {
        0 => "Nothing".to_owned(),
        1 => format!("{} tokens · 1 session", token_count(spend.tokens)),
        n => format!("{} tokens · {n} sessions", token_count(spend.tokens)),
    };
    div()
        .flex()
        .items_center()
        .gap(ROW_GAP)
        .h(DETAIL_ROW_H)
        .child(
            div()
                .flex_none()
                .w(NAME_COL)
                .text_size(ROW_TEXT)
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.primary))
                .child(format!("Last {}", spend.period)),
        )
        .child(
            tabular(div())
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(RESET_TEXT)
                .text_color(paint(t.text.dim))
                .child(through),
        )
        .child(
            tabular(div())
                .flex()
                .flex_none()
                .w(TRAIL_COL)
                .justify_end()
                .text_size(ROW_TEXT)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(paint(t.text.primary))
                // Unpriced is not free: a dash, not `$0.00`.
                .child(match (spend.sessions, spend.cost_usd) {
                    (0, _) => String::new(),
                    (_, Some(usd)) => dollars(usd),
                    (_, None) => "—".to_owned(),
                }),
        )
        .into_any_element()
}

/// What a provider says besides its windows: that it is being turned away,
/// and what its sessions have spent. Nothing, for every provider but Grok.
fn extras(snapshot: &ProviderSnapshot, now_ms: u64, t: &Theme) -> Option<AnyElement> {
    if snapshot.spend.is_empty() && snapshot.limited_at_ms.is_none() {
        return None;
    }
    let unpriced = snapshot
        .spend
        .iter()
        .any(|spend| spend.sessions > 0 && spend.cost_usd.is_none());
    Some(
        div()
            .flex()
            .flex_col()
            .children(snapshot.limited_at_ms.map(|limited| {
                div()
                    .flex()
                    .items_center()
                    .h(DETAIL_ROW_H)
                    .text_size(ROW_TEXT)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(paint(t.quota.critical))
                    .child(format!(
                        "Rate limited {} — requests are being refused",
                        age(limited, now_ms)
                    ))
            }))
            .children(snapshot.spend.iter().map(|spend| spend_row(spend, t)))
            .when(unpriced, |extras| {
                extras.child(note("— : cost is left off subscription traffic", t))
            })
            .into_any_element(),
    )
}

/// One provider section, including honest no-data states
/// instead of percentages inferred from absence.
fn usage_provider(snapshot: ProviderSnapshot, now_ms: u64, last: bool, t: &Theme) -> AnyElement {
    let quiet = quiet(&snapshot);
    // A stale reading says so beside the name, in the attention colour. A
    // fresh one says nothing here: the header's `Updated` covers it, and three
    // copies of one age down the panel were three things to read past.
    let stale_age = match (&snapshot.status, snapshot.fetched_at_ms) {
        (SnapshotStatus::Stale, Some(fetched)) => Some(age(fetched, now_ms)),
        _ => None,
    };
    let body = match &snapshot.status {
        SnapshotStatus::Fresh | SnapshotStatus::Stale if snapshot.windows.is_empty() => {
            note("No windows reported", t)
        }
        SnapshotStatus::Fresh | SnapshotStatus::Stale => div()
            .flex()
            .flex_col()
            .children(
                snapshot
                    .windows
                    .iter()
                    .map(|window| usage_window(window, now_ms, t)),
            )
            .into_any_element(),
        SnapshotStatus::Unavailable { reason } => note(format!("Unavailable — {reason}"), t),
        SnapshotStatus::Unsupported if quiet => note("No usage reporting", t),
        SnapshotStatus::Unsupported => note("No quota published", t),
    };
    let extras = extras(&snapshot, now_ms, t);

    div()
        .flex()
        .flex_col()
        .px(PAD)
        .pt(px(6.0))
        .pb(px(10.0))
        .when(!last, |section| {
            section.border_b_1().border_color(paint(t.border))
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(PROVIDER_H)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(18.0))
                        .child(provider_mark(&snapshot, t)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(PROVIDER_TEXT)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(paint(if quiet { t.text.dim } else { t.text.primary }))
                        .child(snapshot.provider.label()),
                )
                .children(
                    snapshot
                        .plan
                        .as_deref()
                        .map(|plan| crate::ui::chip::tag(plan_label(plan), t).text_size(px(11.5))),
                )
                .child(div().flex_grow())
                .children(stale_age.map(|stale_age| {
                    div()
                        .flex_none()
                        .text_size(META)
                        .text_color(paint(t.status.attention))
                        .child(stale_age)
                })),
        )
        .child(body)
        .children(extras)
        .into_any_element()
}

/// A header's two- or three-way switch, sized to sit beside a popup's title
/// rather than to occupy a band of its own.
///
/// The Savings popup's period, drawn at the size a popup header carries. `pick` is handed the
/// chosen value and owns whatever choosing it means, persisting included.
pub(crate) fn segmented<V: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(&'static str, V)],
    active: V,
    pick: fn(&mut Shell, V),
    t: &Theme,
    cx: &mut Context<Shell>,
) -> AnyElement {
    crate::ui::group::button_group(id)
        .small()
        .children(options.iter().enumerate().map(|(index, &(label, value))| {
            crate::ui::group::segment((id, index), label)
                .selected(active == value)
                .on_click(cx.listener(move |this, _, _, cx| {
                    pick(this, value);
                    cx.stop_propagation();
                    cx.notify();
                }))
        }))
        .render(t)
}

impl Shell {
    /// The quota figures in the window's header: one label-over-value per
    /// provider, the whole run one trigger for the Usage popup below it.
    pub(crate) fn usage_stats(
        &self,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let now = ket_core::now_ms();

        let sections: Vec<Section> = Provider::ALL
            .into_iter()
            .filter_map(|provider| section(&self.rate_limits.snapshot(provider), now))
            .collect();

        let summary = crate::ui::button::pressable("quota-summary", t)
            .when(compact, |el| {
                el.gap(px(6.0))
                    .p(crate::header::COMPACT_TRIGGER_PAD)
                    .mx(-crate::header::COMPACT_TRIGGER_PAD)
            })
            .when(!compact, |el| {
                el.gap(px(26.0))
                    .px(crate::header::TRIGGER_PAD)
                    .mx(-crate::header::TRIGGER_PAD)
                    .py(px(5.0))
            })
            .when(sections.is_empty(), |summary| {
                summary.child(crate::header::stat(
                    "Usage",
                    crate::header::trail("no readings", t),
                    t,
                ))
            })
            .children(sections.into_iter().map(|section| {
                // Stale readings are dimmed rather than hidden: an old number is
                // still worth more than nothing, as long as it does not claim to
                // be current. A quota climbing towards its limit takes the
                // ramp's colour, and stale wins over the ramp: an old number is
                // not evidence of anything urgent, and dim is what says "old".
                let colour = if section.limited {
                    t.quota.critical
                } else if section.stale {
                    t.text.dim
                } else {
                    section
                        .used
                        .and_then(|used| pressure(used, t))
                        .unwrap_or(t.text.primary)
                };
                let label = match &section.span {
                    Some(span) => format!("{} · {span}", section.provider.label()),
                    None => section.provider.label().to_owned(),
                };
                let value = div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_color(paint(colour))
                    .child(match section.used.is_some() {
                        // A figure: mono, because it is a value to line up
                        // and quote rather than something to read.
                        true => div().child(section.headline.clone()),
                        // A word — "unavailable" — so it takes the
                        // proportional face, like every other word here.
                        false => div()
                            .prose()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(paint(if section.limited { colour } else { t.text.dim }))
                            .child(section.headline.clone()),
                    })
                    .children(
                        section
                            .used
                            .map(|used| strip_bar(used, HEADER_BAR_W, HEADER_BAR_H, t)),
                    )
                    .children(
                        section
                            .resets_in
                            .clone()
                            .map(|resets| crate::header::trail(resets, t)),
                    );
                match compact {
                    // A read-out led by the provider's own mark in place of
                    // its name, the figure, a short bar and the reset.
                    true => {
                        let (mark, tint) = agent::provider(section.provider.label(), t);
                        let tint =
                            match (section.used.is_some() && !section.stale) || section.limited {
                                true => tint,
                                false => alpha(tint, 0.5),
                            };
                        crate::ui::chip::readout(t)
                            .text_color(paint(colour))
                            .child(sized_icon(mark, px(12.0), tint))
                            .child(section.headline.clone())
                            .children(
                                section
                                    .used
                                    .map(|used| strip_bar(used, COMPACT_BAR_W, HEADER_BAR_H, t)),
                            )
                            .children(section.resets_in.clone().map(|resets| {
                                div()
                                    .text_size(px(11.0))
                                    .text_color(paint(t.text.dim))
                                    .child(resets)
                            }))
                    }
                    false => crate::header::stat(label, value, t),
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.popup == Some(PopupKind::Usage) {
                    this.popup = None;
                } else {
                    // A popup is the sole non-modal overlay. Closing the text
                    // pickers through their own paths also clears their query.
                    this.close_palette(cx);
                    this.close_finder(cx);
                    this.menu = None;
                    this.project_menu = None;
                    this.worktree_menu = None;
                    this.popup = Some(PopupKind::Usage);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element();

        let popup_content = (self.popup == Some(PopupKind::Usage)).then(|| {
            let providers: Vec<ProviderSnapshot> = Provider::ALL
                .into_iter()
                .map(|provider| self.rate_limits.snapshot(provider))
                // Grok with nothing to say is Grok switched off in settings,
                // or not on this Mac: a row for it would be about an agent
                // the person does not use. OpenCode keeps its quiet row,
                // which is there to explain a missing key.
                .filter(|snapshot| !(snapshot.provider == Provider::Grok && quiet(snapshot)))
                .collect();
            // The newest reading, for the header. A provider whose own
            // reading has gone stale says so on its own line.
            let updated = providers
                .iter()
                .filter_map(|snapshot| snapshot.fetched_at_ms)
                .max()
                .map(|fetched| format!("Updated {}", age(fetched, now)));
            let last = providers.len();
            let body = div().flex().flex_col().children(
                providers
                    .into_iter()
                    .enumerate()
                    .map(|(index, snapshot)| usage_provider(snapshot, now, index + 1 == last, t)),
            );
            div()
                .flex()
                .flex_col()
                // The panel is words and figures to read off, not
                // identifiers, so it is set in the prose face throughout.
                .prose()
                // Title, the reading's age and refresh on one line. The
                // `Usage / All agents` breadcrumb that once sat beside the
                // title named a hierarchy the panel does not have.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .h(HEADER_H)
                        .pl(PAD)
                        .pr(px(8.0))
                        .border_b_1()
                        .border_color(paint(t.border))
                        .child(
                            div()
                                .flex_none()
                                .text_size(TITLE_TEXT)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Usage"),
                        )
                        .child(div().flex_grow())
                        .children(updated.map(|updated| {
                            tabular(div())
                                .flex_none()
                                .text_size(META)
                                .text_color(paint(t.text.dim))
                                .child(updated)
                        }))
                        .child(
                            icon_button("popup-quota-refresh", Icon::Refresh)
                                .bare()
                                .small()
                                .render(t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // Open state is deliberately untouched: the
                                    // poll redraws these same rows in place.
                                    this.rate_limits.refresh_all();
                                    cx.stop_propagation();
                                    cx.notify();
                                })),
                        ),
                )
                .child(body)
                .into_any_element()
        });

        popup::anchored(
            "usage-popup",
            summary,
            popup_content,
            Placement::BelowStart,
            POPUP_W,
            window,
            t,
        )
    }

    /// The window's status bar: the key hints on the left, `feedback` — the
    /// bubble that opens the feedback popover, see `crate::feedback` — on the
    /// right. It sits on the desk rather than in a card of its own — it is
    /// the window's footnote, not a region.
    ///
    /// Quota refresh used to sit where the bubble is; it is in the usage
    /// popover's header, beside the reading it refreshes.
    ///
    /// Named apart from the editor pane's own status line, which is a
    /// different thing at a different scope.
    pub(crate) fn window_status_bar(&self, feedback: AnyElement) -> AnyElement {
        // Drawn straight on the desk, which is not always darker than the ink.
        let t = &self.theme.on_backdrop();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(18.0))
            .h(px(24.0))
            .px(px(6.0))
            .text_size(CAPTION)
            .text_color(paint(t.text.dim))
            .children(crate::shortcuts::FOOTER_HINTS.iter().map(|&(keys, words)| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().text_color(paint(t.text.primary)).child(keys))
                    .child(div().prose().child(words))
            }))
            .child(div().flex_grow())
            .child(feedback)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ket_core::rate_limits::RateWindow;

    fn window(name: &str, used: f32, resets_at_ms: Option<u64>) -> RateWindow {
        RateWindow {
            name: name.to_owned(),
            used_percent: used,
            resets_at_ms,
            window_minutes: None,
        }
    }

    fn fresh(windows: Vec<RateWindow>) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: Provider::Claude,
            status: SnapshotStatus::Fresh,
            windows,
            plan: None,
            account: None,
            fetched_at_ms: Some(1_000),
            spend: Vec::new(),
            limited_at_ms: None,
        }
    }

    #[test]
    fn durations_lose_the_units_nobody_reads() {
        assert_eq!(compact_duration(45 * 60_000), "45m");
        assert_eq!(compact_duration((4 * 60 + 41) * 60_000), "4h 41m");
        assert_eq!(compact_duration((16 * 1_440 + 9 * 60) * 60_000), "16d 9h");
    }

    #[test]
    fn a_duration_under_a_minute_does_not_render_as_empty() {
        assert_eq!(compact_duration(30_000), "0m");
    }

    #[test]
    fn the_tightest_window_is_the_one_shown() {
        // A provider reports several concurrent windows; the binding constraint
        // is whichever stops you first, not whichever it listed first.
        let snapshot = fresh(vec![
            window("Session", 12.0, None),
            window("Weekly", 88.0, None),
            window("Fable", 2.0, None),
        ]);

        assert_eq!(tightest(&snapshot).unwrap().name, "Weekly");
    }

    #[test]
    fn a_failed_fetch_is_never_rendered_as_a_percentage() {
        // The whole reason core keeps four states apart.
        let snapshot = ProviderSnapshot::unavailable(Provider::Codex, "timed out");
        let section = section(&snapshot, 0).expect("still named");

        assert_eq!(section.headline, "unavailable");
        assert!(!section.headline.contains('%'));
    }

    #[test]
    fn a_provider_with_no_quota_concept_is_omitted_entirely() {
        // A row that can only ever say "n/a" is noise in a strip this small.
        let snapshot = ProviderSnapshot::unsupported(Provider::OpenCode);
        assert!(section(&snapshot, 0).is_none());
    }

    #[test]
    fn a_reset_already_in_the_past_is_dropped_rather_than_underflowing() {
        // Clock skew and a snapshot taken before a reset both produce this, and
        // `at - now` on unsigned integers would panic in debug and wrap in
        // release.
        let snapshot = fresh(vec![window("Session", 10.0, Some(500))]);
        let section = section(&snapshot, 1_000).expect("section");

        assert!(section.resets_in.is_none());
    }

    #[test]
    fn a_stale_reading_is_marked_rather_than_hidden() {
        let mut snapshot = fresh(vec![window("Session", 40.0, None)]);
        snapshot.status = SnapshotStatus::Stale;

        let section = section(&snapshot, 0).expect("section");
        assert!(section.stale);
        assert_eq!(section.headline, "40%");
    }

    #[test]
    fn a_fresh_provider_with_no_windows_yet_says_nothing() {
        assert!(section(&fresh(Vec::new()), 0).is_none());
    }
}
