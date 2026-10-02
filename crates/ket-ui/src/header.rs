//! The strip across the top of the window, and the frame the rest sits in.
//!
//! The window is a desk with cards on it: the header, the sidebar, the panes
//! and the right panel are each a rounded card on [`Theme::backdrop`], with a
//! gutter of the desk between them. The header is the first card and the
//! window's title bar — the traffic lights sit inside its left end and its
//! empty run drags the window — and what it carries is a read-out of the
//! selected worktree and of the account: which branch, how much has changed,
//! how many agents are at work, and how much of each plan is spent.
//!
//! Every figure is a label over a value, the value in the chrome's mono so
//! the numbers line up and read as numbers. The quota figures and Economy's
//! savings came up from the status bar, which keeps the key hints.

use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, Pixels, SharedString, Window,
    WindowControlArea, div, prelude::*, px,
};
use ket_core::activity::Signal;
use ket_core::subagents::SubagentState;
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::ui::button::icon_button;
use crate::ui::cluster::icon_cluster;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::{CAPTION, LABEL};

/// The desk showing between two cards, and around the edge of the window.
pub(crate) const GUTTER: Pixels = px(10.0);

/// A card's corner.
pub(crate) const CARD_RADIUS: Pixels = px(12.0);

/// The header card's height.
pub(crate) const HEADER_H: Pixels = px(56.0);

/// Where the traffic lights sit, in window coordinates: inside the header's
/// left end, centred on its height. AppKit places the buttons by their
/// top-left corner, and they are about 12px tall.
pub(crate) const TRAFFIC_LIGHTS: (f32, f32) = (26.0, 32.0);

/// The compact strip's height — see [`Shell::header_compact`].
pub(crate) const COMPACT_H: Pixels = px(32.0);

/// Where the traffic lights sit when the header is the compact strip:
/// centred on its 32px, a little further in.
const TRAFFIC_LIGHTS_COMPACT: (f32, f32) = (22.0, 20.0);

/// Room the traffic lights take at the compact strip's left end.
const COMPACT_LIGHTS_INSET: Pixels = px(74.0);

/// Room the traffic lights take at the header's left end.
const LIGHTS_INSET: Pixels = px(90.0);

/// Space between two figures.
const STAT_GAP: Pixels = px(26.0);

/// Space between two read-outs in the compact strip.
const COMPACT_GAP: Pixels = px(6.0);

/// The hover padding around a clickable figure. The trigger takes it back
/// as negative margin, so its fill spills into the gaps and its figures sit
/// on the same spacing as the plain ones.
pub(crate) const TRIGGER_PAD: Pixels = px(8.0);

/// [`TRIGGER_PAD`] for the compact strip's read-outs.
pub(crate) const COMPACT_TRIGGER_PAD: Pixels = px(2.0);

/// A value's size: bigger than the chrome around it, since the numbers are
/// what the strip is for.
const VALUE: Pixels = px(14.0);

/// How far a card's content stands in from its edge when that content paints
/// all the way to its own edges — a terminal's background, an editor's.
///
/// gpui clips to rectangles, not rounded ones, so a square fill flush with
/// the border covers the card's corners. Four pixels is the least that keeps
/// a square corner inside an 11px inner radius: 11 × (1 − 1/√2) ≈ 3.2.
pub(crate) const CARD_INSET: Pixels = px(4.0);

/// The frame every card shares.
pub(crate) fn card(t: &Theme) -> Div {
    div()
        .bg(paint(t.panel))
        .border_1()
        .border_color(paint(t.rule))
        .rounded(CARD_RADIUS)
        .overflow_hidden()
}

/// One figure: a label in the prose face over a value row in the mono.
pub(crate) fn stat(label: impl Into<SharedString>, value: impl IntoElement, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .prose()
                .text_size(LABEL)
                .text_color(paint(t.text.dim))
                .child(label.into()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(VALUE)
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.primary))
                .child(value),
        )
}

/// A value's quiet trailing words — "6 files", "1h 45m".
pub(crate) fn trail(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .text_size(CAPTION)
        .font_weight(FontWeight::NORMAL)
        .text_color(paint(t.text.dim))
        .child(text.into())
}

/// The hairline standing between the usage and the identity. It sits in the
/// middle of one ordinary gap rather than adding a second.
fn divider(gap: Pixels, height: Pixels, t: &Theme) -> Div {
    div()
        .flex_none()
        .w(px(1.0))
        .h(height)
        .mx(-gap / 2.0)
        .bg(paint(t.rule))
}

/// A run of the header that moves the window.
fn drag_area() -> Div {
    div()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            crate::window_drag::drag_window(window);
        })
}

/// `/Users/me/dev/ket` as `~/dev/ket`.
pub(crate) fn tilde(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text.into_owned(),
    }
}

impl Shell {
    /// The header card.
    pub(crate) fn window_header(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let now = ket_core::now_ms();
        let compact = self.header_compact;
        // The lights follow the header's height. Moved only when it changes:
        // AppKit relays the title bar out on every move.
        if self.lights_placed.get() != Some(compact) {
            let (x, y) = if compact {
                TRAFFIC_LIGHTS_COMPACT
            } else {
                TRAFFIC_LIGHTS
            };
            window.set_traffic_light_position(gpui::point(px(x), px(y)));
            self.lights_placed.set(Some(compact));
        }
        let selected = self.selection.and_then(|selection| {
            let project = self.projects.get(selection.project)?;
            Some((project, project.worktrees.get(selection.worktree)?))
        });

        // Which worktree, in which project, where. The mark and these two
        // lines are the title bar's own, so they drag the window.
        let (place, branch): (SharedString, SharedString) = match selected {
            Some((project, node)) => (
                format!("{} · {}", project.name, tilde(&node.path)).into(),
                node.label(),
            ),
            None => ("ket".into(), "No worktree".into()),
        };
        // A project launching with its own command says so between its name
        // and the path, in the tint every other place it shows wears — see
        // `crate::agent_override`.
        let launches = selected.and_then(|(project, node)| {
            Some((
                project.name.clone(),
                project.override_summary()?,
                tilde(&node.path),
            ))
        });
        let branch_label = branch.clone();
        // Lit while its release card is open — see `crate::release`.
        let card_open = self.release_hover.open;
        let logo = drag_area()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(32.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(paint(t.border))
                    .when(card_open, |el| el.bg(paint(t.hover)))
                    .child(sized_icon(Icon::KetMark, px(13.0), paint(t.status.running))),
            );
        let identity = drag_area()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .max_w(px(260.0))
                    .child(match launches {
                        None => div()
                            .prose()
                            .truncate()
                            .text_size(LABEL)
                            .text_color(paint(t.text.dim))
                            .child(place),
                        Some((name, command, path)) => div()
                            .prose()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .min_w_0()
                            .text_size(LABEL)
                            .text_color(paint(t.text.dim))
                            .child(div().flex_none().child(name))
                            .child(div().flex_none().child("·"))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(crate::fonts::chrome())
                                    .text_size(px(11.5))
                                    .text_color(crate::agent_override::hue(t))
                                    .child(command),
                            )
                            .child(div().flex_none().child("·"))
                            .child(div().min_w_0().truncate().child(path)),
                    })
                    .child(
                        div()
                            .truncate()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(branch),
                    ),
            );

        // What the selected worktree has changed against `HEAD`. A clean
        // checkout says so rather than showing zeroes, and one the sweep has
        // not read yet says nothing it does not know.
        let changes = selected.map(|(_, node)| {
            let value = match (node.dirty, node.line_changes) {
                (false, _) => div()
                    .text_color(paint(t.text.dim))
                    .child("clean")
                    .into_any_element(),
                (true, Some((added, removed))) => div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_color(paint(t.diff.added))
                            .child(format!("+{added}")),
                    )
                    .child(
                        div()
                            .text_color(paint(t.diff.removed))
                            .child(format!("−{removed}")),
                    )
                    .child(trail(
                        match node.changes {
                            1 => "1 file".to_owned(),
                            n => format!("{n} files"),
                        },
                        t,
                    ))
                    .into_any_element(),
                (true, None) => trail(
                    match node.changes {
                        1 => "1 file".to_owned(),
                        n => format!("{n} files"),
                    },
                    t,
                )
                .into_any_element(),
            };
            match compact {
                true => crate::ui::chip::readout(t).child(value),
                false => stat("Changes", value, t),
            }
        });

        // Across every project, not just the selected one: the strip is the
        // one place that answers "is anything still working" without
        // scrolling the sidebar.
        let (mut working, mut blocked) = (0usize, 0usize);
        for node in self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
        {
            match self.activity.signal(&node.id, now) {
                Signal::Working => working += 1,
                Signal::Blocked => blocked += 1,
                _ => {}
            }
            // Agents, not worktrees: each subagent a lead has out counts on
            // its own, so a lead idling while three of them work reads 3.
            for subagent in self.activity.subagents(&node.id, now) {
                match subagent.state {
                    SubagentState::Working => working += 1,
                    SubagentState::Blocked => blocked += 1,
                    SubagentState::Idle => {}
                }
            }
        }
        let agents_value = div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .child(crate::ui::chip::status_dot(
                paint(if working > 0 {
                    t.status.running
                } else {
                    t.border
                }),
                if working > 0 {
                    crate::ui::chip::Motion::Travel
                } else {
                    crate::ui::chip::Motion::Still
                },
                px(7.0),
            ))
            .child(
                div()
                    .when(working == 0, |el| el.text_color(paint(t.text.dim)))
                    .child(format!("{working} running")),
            )
            .when(blocked > 0, |el| {
                el.child(
                    div()
                        .text_size(CAPTION)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(paint(t.status.attention))
                        .child(format!("{blocked} waiting")),
                )
            });
        let agents = match compact {
            true => crate::ui::chip::readout(t).child(agents_value),
            false => stat("Agents", agents_value, t),
        };

        let usage = self.usage_stats(compact, window, cx);
        let merge = self.merge_status(compact, now, window, cx);
        let economy = self.economy_status(compact, window, cx);

        // Compact, the mark stands alone and the place is one read-out: the
        // branch, led by its glyph. The project and path are a hover away in
        // the sidebar, and the strip is for figures.
        let (logo, identity) = match compact {
            true => (
                drag_area()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h_full()
                    .px(px(8.0))
                    .child(sized_icon(Icon::KetMark, px(12.0), paint(t.status.running))),
                drag_area().child(
                    crate::ui::chip::readout(t)
                        .child(sized_icon(Icon::GitBranch, px(13.0), paint(t.text.dim)))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .max_w(px(220.0))
                                .truncate()
                                .child(branch_label.clone()),
                        ),
                ),
            ),
            false => (logo, identity),
        };
        let logo = self.release_mark(logo.into_any_element(), window, cx);

        let inset = match (window.is_fullscreen(), compact) {
            // Fullscreen hides the traffic lights, so the room kept for them
            // goes back to the content rather than sitting empty.
            (true, _) => px(12.0),
            (false, true) => COMPACT_LIGHTS_INSET,
            (false, false) => LIGHTS_INSET,
        };
        let cluster = icon_cluster("header-panels");
        let cluster = if compact { cluster.small() } else { cluster };
        // Each switch says what it will do, and the chord that does it too:
        // the header is where a person learns the layout keys. Right-aligned
        // because the cluster ends a few pixels from the window's edge, and a
        // label centred on the last button would run off it.
        let hovered = self.hovered_header_action;
        let tip = |id: &'static str, cell: AnyElement, label: SharedString| {
            crate::ui::tooltip::tooltip(
                id,
                cell,
                label,
                crate::ui::tooltip::Side::BottomEnd,
                hovered == Some(id),
                t,
            )
            .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
                if *is_hovered {
                    this.hovered_header_action = Some(id);
                } else if this.hovered_header_action == Some(id) {
                    this.hovered_header_action = None;
                }
                cx.notify();
            }))
            .into_any_element()
        };
        let tip = &tip;
        let sidebar_label: SharedString = format!(
            "{} sidebar ({})",
            if self.sidebar_open { "Hide" } else { "Show" },
            crate::shortcuts::SIDEBAR_KEYS
        )
        .into();
        let panel_label: SharedString = format!(
            "{} right panel ({})",
            if self.panel_open { "Hide" } else { "Show" },
            crate::shortcuts::PANEL_KEYS
        )
        .into();

        card(t)
            .id("window-header")
            .flex()
            .flex_none()
            .items_center()
            .when(compact, |el| {
                el.gap(COMPACT_GAP)
                    .h(COMPACT_H)
                    .rounded(px(9.0))
                    .pr(px(3.0))
            })
            .when(!compact, |el| el.gap(STAT_GAP).h(HEADER_H).pr(px(10.0)))
            .pl(inset)
            // The account's quota first, beside the mark: it is the figure
            // that decides whether there is room to start anything else.
            .child(logo)
            .child(usage)
            .child(match compact {
                true => divider(COMPACT_GAP, px(16.0), t),
                false => divider(STAT_GAP, px(28.0), t),
            })
            .child(identity)
            .children(changes)
            .child(agents)
            .child(merge)
            .child(economy)
            .child(drag_area().flex_1().h_full())
            // The header's own fold, then the two panels' switches: one
            // cluster of the window's layout, each lit while its thing is
            // folded or open.
            .child(
                cluster
                    .cell(
                        icon_button(
                            "header-compact",
                            if compact {
                                Icon::UnfoldVertical
                            } else {
                                Icon::FoldVertical
                            },
                        )
                        .pressed(compact)
                        .indicator(compact),
                        |cell| {
                            tip(
                                "header-compact-tip",
                                cell.on_click(cx.listener(|this, _, _, cx| {
                                    this.header_compact = !this.header_compact;
                                    this.persist_view();
                                    cx.notify();
                                }))
                                .into_any_element(),
                                if compact {
                                    "Expand header".into()
                                } else {
                                    "Compact header".into()
                                },
                            )
                        },
                    )
                    .cell(
                        icon_button("header-sidebar", Icon::Sidebar)
                            .pressed(self.sidebar_open)
                            .indicator(self.sidebar_open),
                        |cell| {
                            tip(
                                "header-sidebar-tip",
                                cell.on_click(cx.listener(|this, _, _, cx| {
                                    this.dispatch("shell.toggle_sidebar", cx);
                                    cx.notify();
                                }))
                                .into_any_element(),
                                sidebar_label,
                            )
                        },
                    )
                    .cell(
                        icon_button("header-panel", Icon::PanelRight)
                            .pressed(self.panel_open)
                            .indicator(self.panel_open),
                        |cell| {
                            tip(
                                "header-panel-tip",
                                cell.on_click(cx.listener(|this, _, _, cx| {
                                    this.dispatch("shell.toggle_panel", cx);
                                    cx.notify();
                                }))
                                .into_any_element(),
                                panel_label,
                            )
                        },
                    )
                    .render(t),
            )
            .into_any_element()
    }
}

/// The Merge popup's width.
const MERGE_POPUP_W: Pixels = px(340.0);

impl Shell {
    /// The header's Merge figure: how many worktrees have finished work
    /// waiting, and — pressed — the list of them, each a way to merge it.
    ///
    /// It used to be a card docked at the foot of the sidebar. The question
    /// it answers — what is finished and waiting on me — is about the whole
    /// window rather than any one project, which is what the header is for;
    /// and the card took sidebar height exactly when there was most to list.
    /// With nothing ready it still says so, quietly, rather than leaving a
    /// gap in the strip that opens and closes as work lands.
    fn merge_status(
        &self,
        compact: bool,
        now: u64,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let ready = self.ready_to_merge(now);
        let count = ready.len();
        let ink = if count > 0 {
            t.text.primary
        } else {
            t.text.dim
        };

        let face = match compact {
            true => crate::ui::chip::readout(t)
                .text_color(paint(ink))
                .child(sized_icon(Icon::GitMerge, px(13.0), paint(t.text.dim)))
                .child(count.to_string()),
            false => stat(
                "Merge",
                div().text_color(paint(ink)).child(match count {
                    0 => "none".to_owned(),
                    n => format!("{n} ready"),
                }),
                t,
            ),
        };
        if count == 0 {
            return face.into_any_element();
        }

        let trigger = crate::ui::button::pressable("merge-status", t)
            .when(compact, |el| {
                el.p(COMPACT_TRIGGER_PAD).mx(-COMPACT_TRIGGER_PAD)
            })
            .when(!compact, |el| {
                el.px(TRIGGER_PAD).mx(-TRIGGER_PAD).py(px(5.0))
            })
            .child(face)
            .on_click(cx.listener(|this, _, _, cx| {
                if this.popup == Some(crate::PopupKind::Merge) {
                    this.popup = None;
                } else {
                    // A popup is the sole non-modal overlay; see the usage
                    // figure beside this one.
                    this.close_palette(cx);
                    this.close_finder(cx);
                    this.menu = None;
                    this.project_menu = None;
                    this.worktree_menu = None;
                    this.popup = Some(crate::PopupKind::Merge);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element();

        let content = (self.popup == Some(crate::PopupKind::Merge)).then(|| {
            let rows = ready.into_iter().filter_map(|(pi, wi)| {
                let project = self.projects.get(pi)?;
                let node = project.worktrees.get(wi)?;
                // One at a time, the rule `request_merge_worktree` enforces:
                // a second merge started while the first is in flight would
                // be two processes writing one index.
                let busy = self.merging.is_some() || node.in_progress.is_some();
                let id = ("merge-ready", pi * 1000 + wi);
                let row = match busy {
                    true => crate::ui::row::heading_row(id, t),
                    false => crate::ui::row::sized_row(id, false, crate::ui::ROW_H, t),
                };
                // The counts are read on the status tick, so a worktree that
                // has only just become dirty is listed before its numbers
                // arrive. It says so rather than claiming a zero.
                let counts = match node.line_changes {
                    Some((added, removed)) => div()
                        .flex()
                        .flex_none()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_color(paint(t.diff.added))
                                .child(format!("+{added}")),
                        )
                        .child(
                            div()
                                .text_color(paint(t.diff.removed))
                                .child(format!("\u{2212}{removed}")),
                        ),
                    None => div()
                        .flex_none()
                        .text_color(paint(t.text.dim))
                        .child("\u{2026}"),
                };
                Some(
                    row.child(sized_icon(Icon::GitMerge, px(13.0), paint(t.text.dim)))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_baseline()
                                .gap(px(8.0))
                                .child(div().truncate().child(node.label()))
                                .child(
                                    div()
                                        .prose()
                                        .flex_none()
                                        .text_size(CAPTION)
                                        .text_color(paint(t.text.dim))
                                        .child(project.name.clone()),
                                ),
                        )
                        .child(counts)
                        .when(!busy, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.popup = None;
                                this.request_merge_worktree(pi, wi, cx);
                                cx.notify();
                            }))
                        })
                        .into_any_element(),
                )
            });
            div()
                .flex()
                .flex_col()
                .p(px(5.0))
                .gap(px(1.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(28.0))
                        .px(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .text_size(LABEL)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Ready to merge"),
                        )
                        .child(
                            div()
                                .text_size(CAPTION)
                                .text_color(paint(t.text.dim))
                                .child(count.to_string()),
                        ),
                )
                .children(rows)
                .into_any_element()
        });

        crate::ui::popup::anchored(
            "merge-popup",
            trigger,
            content,
            crate::ui::popup::Placement::BelowStart,
            MERGE_POPUP_W,
            window,
            t,
        )
    }
}
