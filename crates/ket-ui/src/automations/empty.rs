//! The Automations dialog before there is an automation: what they are in a
//! sentence, and the one way to make the first.
//!
//! The list and the chosen automation's pane both have nothing to show, so
//! neither is drawn: the dialog is its title, its ✕ and this, centred.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px, relative};

use crate::Shell;
use crate::paint::paint;
use crate::ui::button::{button, icon_button};
use crate::ui::icon::{Icon, sized_icon};

/// The badge's width and height.
const BADGE: gpui::Pixels = px(80.0);

/// The dial drawn inside it.
const DIAL: gpui::Pixels = px(50.0);

impl Shell {
    /// The whole of the dialog while nothing is scheduled.
    pub(super) fn automations_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let closer = icon_button("automations-close", Icon::Close)
            .circle()
            .sized(px(40.0), px(18.0))
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_automations();
                cx.notify();
            }));
        // A timer partway round: the ticks behind the hand lit in the
        // running green, the ones ahead of it faint. Two layers, since an
        // icon is drawn in one colour.
        let dial = div()
            .relative()
            .size(DIAL)
            .flex_none()
            .child(sized_icon(Icon::DialRest, DIAL, paint(t.text.primary)))
            .child(div().absolute().inset_0().child(sized_icon(
                Icon::DialLit,
                DIAL,
                paint(t.status.running),
            )));
        // New automation opens the New worktree dialog, which needs a
        // project to open on.
        let can_add = !self.projects.is_empty();
        let add = button("automations-new", "New automation")
            .primary()
            .large()
            .leading(Icon::Plus)
            .enabled(can_add)
            .render(t)
            .mt(px(32.0))
            .when(can_add, |el| {
                el.on_click(cx.listener(|this, _, _, cx| {
                    this.new_automation(cx);
                    cx.notify();
                }))
            });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt(px(24.0))
                    .pl(px(28.0))
                    .pr(px(24.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(paint(t.text.primary))
                            .child("Automations"),
                    )
                    .child(closer),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .px(px(80.0))
                    .pb(px(48.0))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(BADGE)
                            .mb(px(28.0))
                            .rounded_full()
                            .bg(paint(t.sunken))
                            .border_1()
                            .border_color(paint(t.border))
                            .child(dial),
                    )
                    .child(
                        div()
                            .text_size(px(32.0))
                            .line_height(relative(1.2))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(paint(t.text.primary))
                            .text_center()
                            .child("Nothing scheduled yet"),
                    )
                    .child(
                        div()
                            .mt(px(12.0))
                            .max_w(px(520.0))
                            .text_size(px(18.0))
                            .line_height(relative(1.5))
                            .text_color(paint(t.text.dim))
                            .text_center()
                            .child(
                                "Run a prompt or a command on a schedule, even with ket closed.",
                            ),
                    )
                    .child(add),
            )
            .into_any_element()
    }
}
