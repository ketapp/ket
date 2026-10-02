//! The card behind the header's ket mark: which build this is, and what is
//! in it.
//!
//! Resting the pointer on the mark opens it; there is no click, because the
//! mark is part of the title bar and a press there moves the window. It stays
//! open while the pointer is on the mark or the card, so the gap between the
//! two can be crossed, and closes a moment after it leaves both.
//!
//! The notes are the compiled-in `CHANGELOG.md` (`ket_core::changelog`): a
//! release shows its own section, a dev build shows what has landed under
//! Unreleased. Update state joins the card with the updater; until then only
//! a dev build has one to give.
//! Chosen as A Card on the release popover canvas, 2026-10-01.

use std::time::Duration;

use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, Pixels, Window, div, prelude::*, px, relative,
};
use ket_core::build_info::{self, Channel};
use ket_core::changelog::{self, Release, UNRELEASED};

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::preferences::Section;
use crate::ui::LABEL;
use crate::ui::banner::banner;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{caption, tag, tinted_tag};
use crate::ui::icon::Icon;
use crate::ui::popup::{self, Placement};
use crate::ui::toast::Tone;

/// The card's width.
const POPUP_W: Pixels = px(344.0);

/// Space inside the card's edges.
const PAD: Pixels = px(14.0);

/// The footer's height, the same as the Economy card's.
const FOOTER_H: Pixels = px(34.0);

/// How long the pointer rests on the mark before the card opens: long
/// enough that passing over it on the way to the traffic lights does not.
const OPEN_DELAY: Duration = Duration::from_millis(250);

/// How long the card waits after the pointer leaves, so the gap between the
/// mark and the card can be crossed.
const CLOSE_DELAY: Duration = Duration::from_millis(200);

/// Past this the notes scroll rather than the card growing down the window.
const NOTES_MAX_H: Pixels = px(300.0);

/// Every release's notes, on the website.
const CHANGELOG_URL: &str = "https://ketapp.dev/changelog/";

/// Where the pointer is, and whether the card is open.
#[derive(Debug, Default)]
pub(crate) struct ReleaseHover {
    over_mark: bool,
    over_card: bool,
    /// Whether the card is showing — also what lights the mark.
    pub(crate) open: bool,
    /// Bumped on every change, so a timer started for an earlier one does
    /// nothing when it fires.
    epoch: u64,
}

impl Shell {
    /// The header's mark, with the card behind it.
    pub(crate) fn release_mark(
        &self,
        mark: AnyElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let card = self.release_hover.open.then(|| self.release_card(cx));
        // Full height, so the card opens below the header rather than over
        // its bottom edge.
        div()
            .id("ket-mark")
            .flex()
            .flex_none()
            .h_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.release_hover.over_mark = *hovered;
                this.settle_release_hover(cx);
            }))
            .child(popup::hover_anchored(
                "release-card",
                mark,
                card,
                Placement::BelowStart,
                POPUP_W,
                window,
                t,
            ))
            .into_any_element()
    }

    /// Opens or closes the card once the pointer has stayed put.
    fn settle_release_hover(&mut self, cx: &mut Context<Self>) {
        let hover = &mut self.release_hover;
        hover.epoch += 1;
        let wanted = hover.over_mark || hover.over_card;
        if wanted == hover.open {
            return;
        }
        let epoch = hover.epoch;
        let delay = if wanted { OPEN_DELAY } else { CLOSE_DELAY };
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            cx.background_executor().timer(delay).await;
            let _ = shell.update(cx, |shell, cx| {
                let hover = &mut shell.release_hover;
                if hover.epoch == epoch {
                    hover.open = hover.over_mark || hover.over_card;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Closes the card now, for an action taken from it.
    fn close_release_card(&mut self) {
        self.release_hover = ReleaseHover::default();
    }

    /// The card's body.
    fn release_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let dim = paint(t.text.dim);
        let dev = build_info::CHANNEL == Channel::Dev;

        let version = match dev {
            true => format!("{}-dev", build_info::VERSION),
            false => build_info::VERSION.to_owned(),
        };
        let channel = match build_info::CHANNEL {
            Channel::Dev => tinted_tag("Dev", paint(t.marker)),
            Channel::Beta => tag("Beta", t),
            Channel::Stable => tag("Stable", t),
        };
        let meta = match dev {
            true => ["Built from a checkout".to_owned()]
                .into_iter()
                .chain(build_info::COMMIT.map(str::to_owned))
                .collect::<Vec<_>>(),
            false => build_info::BUILD_DATE
                .map(long_date)
                .into_iter()
                .chain(build_info::COMMIT.map(str::to_owned))
                .collect(),
        }
        .join(" \u{b7} ");

        let head = div()
            .flex()
            .items_start()
            .gap(px(8.0))
            .pt(PAD)
            .pl(PAD)
            .pr(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .prose()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("ket"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(version),
                            )
                            .child(channel),
                    )
                    .when(!meta.is_empty(), |el| {
                        el.child(div().text_size(px(11.5)).text_color(dim).child(meta))
                    }),
            )
            .child(
                icon_button("release-copy", Icon::Copy)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let shown = format!("ket {}", build_info::display());
                        cx.write_to_clipboard(ClipboardItem::new_string(shown.clone()));
                        this.toast_detail(Tone::Info, "Copied version", shown, cx);
                        cx.notify();
                    })),
            );

        // The updater's place. Until it exists, only a dev build has a state
        // to report, and it is that there is nothing to report.
        let status = dev.then(|| {
            div().mx(px(12.0)).mt(px(12.0)).child(banner(
                "release-updates-off",
                Tone::Info,
                "Updates are off in dev builds",
                t,
            ))
        });

        let (title, notes) = match dev {
            true => {
                let since = changelog::release(build_info::VERSION)
                    .map(|_| format!(" \u{b7} since {}", build_info::VERSION))
                    .unwrap_or_default();
                (
                    format!("{UNRELEASED}{since}"),
                    changelog::release(UNRELEASED),
                )
            }
            false => (
                format!("What\u{2019}s new in {}", build_info::VERSION),
                changelog::release(build_info::VERSION),
            ),
        };
        let notes = div()
            .flex()
            .flex_col()
            .px(PAD)
            .pt(px(14.0))
            .child(
                div()
                    .prose()
                    .pb(px(6.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(dim)
                    .child(title),
            )
            .child(
                div()
                    .id("release-notes")
                    .max_h(NOTES_MAX_H)
                    .overflow_y_scroll()
                    .child(release_notes(notes, dev, t)),
            );

        let footer = div()
            .flex()
            .items_center()
            .h(FOOTER_H)
            .mt(px(12.0))
            .pl(PAD)
            .pr(px(10.0))
            .border_t_1()
            .border_color(paint(t.border))
            .child(
                button("release-changelog", "Full changelog")
                    .link()
                    .small()
                    .trailing(Icon::ExternalLink)
                    .render(t)
                    .on_click(|_, _, cx| cx.open_url(CHANGELOG_URL)),
            )
            .child(div().flex_grow())
            .child(
                icon_button("release-settings", Icon::Sliders)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_release_card();
                        this.open_preferences_at(Section::General, cx);
                        cx.notify();
                    })),
            );

        div()
            .id("release-card-body")
            .flex()
            .flex_col()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.release_hover.over_card = *hovered;
                this.settle_release_hover(cx);
            }))
            .child(head)
            .children(status)
            .child(notes)
            .child(footer)
            .into_any_element()
    }
}

/// A release's notes, a caption over each group's lines.
fn release_notes(release: Option<Release>, dev: bool, t: &ket_core::theme::Theme) -> AnyElement {
    let empty = match dev {
        true => "Nothing yet.",
        false => "No notes for this release.",
    };
    let Some(release) = release.filter(|release| !release.is_empty()) else {
        return caption(empty, t).prose().into_any_element();
    };
    let faint = paint(t.text.dim.mix(t.elevated, 0.3));
    div()
        .prose()
        .flex()
        .flex_col()
        .children(
            release
                .groups
                .into_iter()
                .filter(|group| !group.items.is_empty())
                .enumerate()
                .map(|(index, group)| {
                    div()
                        .flex()
                        .flex_col()
                        .when(index > 0, |el| el.pt(px(8.0)))
                        .when(!group.kind.is_empty(), |el| {
                            el.child(
                                div()
                                    .pb(px(2.0))
                                    .text_size(px(11.5))
                                    .text_color(faint)
                                    .child(group.kind),
                            )
                        })
                        .children(group.items.into_iter().map(|item| {
                            div()
                                .flex()
                                .gap(px(8.0))
                                .py(px(2.0))
                                .text_size(LABEL)
                                .line_height(relative(1.4))
                                .child(div().flex_none().text_color(faint).child("\u{2013}"))
                                .child(div().flex_1().min_w_0().child(item))
                        }))
                }),
        )
        .into_any_element()
}

/// `2026-10-01` as `1 Oct 2026`, or as written if it is not a date.
fn long_date(date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = date.splitn(3, '-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return date.to_owned();
    };
    match (
        month
            .parse::<usize>()
            .ok()
            .and_then(|m| MONTHS.get(m.wrapping_sub(1))),
        day.parse::<u32>(),
    ) {
        (Some(month), Ok(day)) => format!("{day} {month} {year}"),
        _ => date.to_owned(),
    }
}
