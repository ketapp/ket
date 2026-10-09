//! A project's own agent command, drawn.
//!
//! A project can launch an agent with a command of its own — `claude-work`
//! where every other project runs `claude-personal` (see
//! [`ket_core::project::ProjectSettings::agents`]). Getting that wrong is a
//! session on the wrong account, so everything the command reaches wears one
//! tint: the project's group in the sidebar, its marks, the header, the tab
//! and pane of a session it launched, the agent picker and the agent's card
//! in Settings. One hue, so a glance anywhere says "this is not the default".

use gpui::{Rgba, SharedString};
use ket_core::config::AgentLaunch;
use ket_core::theme::Theme;

use crate::paint::paint;

/// The tint.
///
/// The theme's own terminal cyan rather than a new chrome token: no other
/// state in the chrome spends it, and every theme already picks one that
/// reads on its own ground — `#3fd2e0` on the dark desk, a deep teal on the
/// light one.
pub(crate) fn hue(t: &Theme) -> Rgba {
    paint(t.terminal.ansi.cyan)
}

/// What a strip has room for: the command's first word, `claude-work`.
///
/// The word is the part that names the account or wrapper; its arguments are
/// in Project Settings and on the agent's card, where there is room to read
/// them.
pub(crate) fn short(launch: &AgentLaunch) -> SharedString {
    launch
        .command
        .split_whitespace()
        .next()
        .unwrap_or(&launch.command)
        .to_owned()
        .into()
}

/// The whole line, the way the shell is given it.
pub(crate) fn line(launch: &AgentLaunch) -> SharedString {
    ket_core::shell::line(&launch.command, &launch.args).into()
}

// ---- the sidebar's glyph and its card ---------------------------------------
//
// A project with its own command wears a launch glyph beside its name in the
// sidebar (B Inline on the agent override marker canvas, dialled down to B2
// Wash, 2026-10-07). The glyph says *that* something is different; resting
// the pointer on it opens a card that says *what*: each overridden agent,
// the command it runs here and the one it would have run. The card opens and
// closes on the same timers as the header's release card, so the gap between
// glyph and card can be crossed.

use std::time::Duration;

use gpui::{AnyElement, Context, FontWeight, Pixels, Window, div, prelude::*, px};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::alpha;
use crate::tree::ProjectNode;
use crate::ui::RADIUS_SM;
use crate::ui::agent::{agent_label, mark};
use crate::ui::button::button;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::popup::{self, Placement};
use crate::ui::{ICON_GAP, WELL_XS};

/// The card's width.
const POPUP_W: Pixels = px(280.0);

/// The glyph's size, and the well it sits in when its card is open.
const GLYPH: Pixels = px(14.0);
const GLYPH_WELL: Pixels = px(20.0);

/// The glyph sits this far from the name — a step inside the row's usual
/// icon gap, so it reads as a suffix to the name rather than the next thing
/// along the row.
const GLYPH_GAP: Pixels = px(6.0);

/// How long the pointer rests on the glyph before the card opens.
const OPEN_DELAY: Duration = Duration::from_millis(250);

/// How long the card waits after the pointer leaves it and the glyph.
const CLOSE_DELAY: Duration = Duration::from_millis(200);

/// Where the pointer is, and which project's card is open.
#[derive(Debug, Default)]
pub(crate) struct OverrideHover {
    /// The project whose glyph is under the pointer.
    over_glyph: Option<usize>,
    /// Whether the pointer is on the open card.
    over_card: bool,
    /// The project whose card is showing — also what lights its glyph.
    pub(crate) open: Option<usize>,
    /// Bumped on every change, so a timer started for an earlier one does
    /// nothing when it fires.
    epoch: u64,
}

impl Shell {
    /// The glyph on project `pi`'s heading, with the card behind it.
    pub(crate) fn override_glyph(
        &self,
        pi: usize,
        project: &ProjectNode,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let tint = hue(t);
        let open = self.override_hover.open == Some(pi);
        let id = project.id.clone();

        let glyph = div()
            .id(("override-glyph", pi))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(GLYPH_WELL)
            .ml(GLYPH_GAP - ICON_GAP)
            .rounded(RADIUS_SM)
            .cursor_pointer()
            .when(open, |el| el.bg(alpha(tint, 0.16)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let hover = &mut this.override_hover;
                if *hovered {
                    hover.over_glyph = Some(pi);
                } else if hover.over_glyph == Some(pi) {
                    hover.over_glyph = None;
                }
                this.settle_override_hover(cx);
            }))
            // A click goes straight to where the command is set, and not to
            // the heading under it, which would fold the project.
            .on_click(cx.listener(move |this, _, _, cx| {
                this.override_hover = OverrideHover::default();
                this.open_project_settings(&id, crate::projects::Field::Command(0), cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(sized_icon(Icon::Launch, GLYPH, tint))
            .into_any_element();

        let card = open.then(|| self.override_card(pi, project, cx));
        popup::hover_anchored(
            ("override-card", pi),
            glyph,
            card,
            Placement::BelowStart,
            POPUP_W,
            window,
            t,
        )
    }

    /// Opens or closes the card once the pointer has stayed put.
    fn settle_override_hover(&mut self, cx: &mut Context<Self>) {
        let hover = &mut self.override_hover;
        hover.epoch += 1;
        let wanted = hover.over_glyph.or(hover.open.filter(|_| hover.over_card));
        if wanted == hover.open {
            return;
        }
        let epoch = hover.epoch;
        let delay = if wanted.is_some() {
            OPEN_DELAY
        } else {
            CLOSE_DELAY
        };
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            cx.background_executor().timer(delay).await;
            let _ = shell.update(cx, |shell, cx| {
                let hover = &mut shell.override_hover;
                if hover.epoch == epoch {
                    hover.open = hover.over_glyph.or(hover.open.filter(|_| hover.over_card));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The card's body: one row per overridden agent, and the way to the
    /// dialog that sets them.
    fn override_card(
        &self,
        pi: usize,
        project: &ProjectNode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let tint = hue(t);
        let dim = paint(t.text.dim);
        let faint = paint(t.text.dim.mix(t.elevated, 0.3));
        let id = project.id.clone();

        // What each agent would have run: the command in Settings. Read once
        // per open card, not per frame — the card is the only reader.
        let workspace = Workspace::open().ok();
        let config = workspace.as_ref().map(Workspace::config);

        let head = div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .pt(px(12.0))
            .px(px(12.0))
            .pb(px(4.0))
            .child(
                div()
                    .prose()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Own agent command"),
            )
            .child(
                div()
                    .prose()
                    .text_size(px(11.5))
                    .line_height(px(15.0))
                    .text_color(dim)
                    .child(format!(
                        "Sessions in {} launch with this, not the one in Settings.",
                        project.name
                    )),
            );

        let rows = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pt(px(6.0))
            .px(px(8.0))
            .pb(px(10.0))
            .children(project.agent_overrides.iter().map(|(agent, launch)| {
                let replaced = config
                    .as_ref()
                    .and_then(|config| config.agent(agent))
                    .map(|spec| spec.launch_line(&[]));
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .p(px(6.0))
                    .rounded(crate::ui::RADIUS_MD)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(WELL_XS)
                            .rounded(RADIUS_SM)
                            .bg(paint(t.hover))
                            .child(mark(agent, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .prose()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(agent_label(agent)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(tint)
                                    .truncate()
                                    .child(line(launch)),
                            )
                            .children(replaced.map(|replaced| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_size(px(11.0))
                                    .text_color(faint)
                                    .child("was")
                                    .child(
                                        div().min_w_0().truncate().line_through().child(replaced),
                                    )
                            })),
                    )
            }));

        let footer = div()
            .flex()
            .items_center()
            .justify_end()
            .px(px(8.0))
            .py(px(6.0))
            .border_t_1()
            .border_color(paint(t.rule))
            .child(
                button(("override-settings", pi), "Project settings")
                    .link()
                    .small()
                    .trailing(Icon::ChevronRight)
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.override_hover = OverrideHover::default();
                        this.open_project_settings(&id, crate::projects::Field::Command(0), cx);
                        cx.notify();
                    })),
            );

        div()
            .id(("override-card-body", pi))
            .flex()
            .flex_col()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.override_hover.over_card = *hovered;
                this.settle_override_hover(cx);
            }))
            .child(head)
            .child(rows)
            .child(footer)
            .into_any_element()
    }
}
