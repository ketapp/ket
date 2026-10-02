//! The backlog's list: the waiting notes grouped by priority, the open note
//! drawn in its row's place, then the done ones behind a heading that shows
//! or hides them.

use gpui::{
    AnyElement, ClickEvent, Context, DragMoveEvent, FontWeight, MouseButton, Window, div, point,
    prelude::*, px,
};
use ket_core::backlog::{Done, Note, Priority};

use super::organise::{Drop, NoteDrag};
use super::{
    BODY_EDITOR, BODY_HEIGHT, BODY_INSET, BODY_TEXT, COMPOSER_GUTTER, OPEN_PAD, OPEN_PRIORITY,
    OPEN_TITLE, PRIORITY_MARK, ROW_DETAIL, ROW_FIGURE, ROW_MENU, ROW_TITLE, age,
    priority_mark_sized,
};
use crate::Shell;
use crate::fonts::Prose;
use crate::input::{Style, text_field, text_line};
use crate::paint::{alpha, paint};
use crate::phone_work::branch_seeded;
use crate::snippets::preview;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{caption, removable, tag, tinted_tag};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::markdown::markdown;
use crate::ui::menu::dropdown;
use crate::ui::row::{block_row, sized_row};
use crate::ui::textarea::textarea;
use crate::ui::toggle::check_box;
use crate::ui::tooltip::{Side, tooltip};

/// Most tags a row shows before the rest are counted.
const ROW_TAGS: usize = 3;

/// The gap between rows, which a drop's insertion line is centred on.
const ROW_GAP: gpui::Pixels = px(2.0);

/// A priority's heading over its notes.
const GROUP_TEXT: gpui::Pixels = px(12.5);

/// The column at a row's right edge: its figures at rest, its actions on
/// the row the pointer or the arrows are on. One width for both, so nothing
/// on the row moves when one gives way to the other.
const ROW_END_W: gpui::Pixels = px(200.0);

/// The ⋯ button's side, which the row's menu hangs from.
pub(super) const MORE_W: gpui::Pixels = crate::ui::WELL_SM;

/// The right-hand column's height, whatever it holds — figures at rest,
/// buttons under the pointer, Delete's question: the buttons' height, so a
/// one-line row is as tall at rest as when its buttons show, and nothing
/// jumps as the pointer crosses the list.
const ROW_END_H: gpui::Pixels = crate::ui::WELL_SM;

/// A row's corner.
const ROW_RADIUS: gpui::Pixels = px(10.0);

/// Where a row sits among the waiting notes, for what dragging over it
/// means.
struct Place {
    /// The waiting note drawn next in the same priority, which a drop on
    /// this row's lower half lands before. `None` for the last of its
    /// priority, where a drop lands at the end of it.
    after: Option<String>,
}

impl Shell {
    /// The open note's description, built ahead of the dialog because the
    /// editor it draws needs the shell mutably.
    pub(super) fn backlog_body_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        dialog.selected.as_ref()?;
        let active = dialog.body_active && self.focus.is_focused(window);
        self.set_composer_caret(BODY_EDITOR, active);
        let source = self.composer_text(BODY_EDITOR);
        let empty = source.is_empty();
        // Written as markdown and read as it: drawn whenever nobody is
        // typing in it, and the source again the moment somebody clicks in.
        let body = if active || empty {
            self.editor_pane(BODY_EDITOR, None, window, cx)
        } else {
            div()
                .id("backlog-body-read")
                .size_full()
                .pl(COMPOSER_GUTTER)
                .pr(px(12.0))
                .pb(px(12.0))
                .overflow_y_scroll()
                .child(markdown(&source, BODY_TEXT, &self.theme))
                .into_any_element()
        };
        let t = &self.theme;
        let area = textarea("backlog-body", body, BODY_HEIGHT).active(active);
        let area = if empty {
            area.placeholder(
                "The detail: what to change, where, and how to tell it worked",
                BODY_TEXT,
            )
        } else {
            area
        };
        Some(
            area.render(t)
                .flex_none()
                .mx(BODY_INSET)
                .mt(px(8.0))
                .cursor_text()
                .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.body_active = true;
                    }
                    window.focus(&this.focus);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The queue: the waiting notes under a heading for each priority, the
    /// open one drawn in place; then the done ones behind a heading that
    /// shows or hides them.
    pub(super) fn backlog_list(
        &self,
        body: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(dialog) = self.backlog.as_ref() else {
            return div().into_any_element();
        };
        let t = &self.theme;
        let query = dialog.search.read(cx).text();
        let mut body = body;
        let mut rows: Vec<AnyElement> = Vec::new();
        let done = dialog.done_count();
        let waiting = dialog.notes.len() - done;
        let open = dialog.open_rows(&query);
        let fresh = dialog.fresh.as_ref().map(|note| note.id.as_str());
        let mut heading: Option<Priority> = None;
        let mut done_drawn = 0;
        for (index, note) in dialog.rows(&query).into_iter().enumerate() {
            let waits = note.done.is_none() && Some(note.id.as_str()) != fresh;
            if waits && heading != Some(note.priority) {
                heading = Some(note.priority);
                let count = open
                    .iter()
                    .filter(|other| other.priority == note.priority)
                    .count();
                let first = rows.is_empty();
                rows.push(self.backlog_group_heading(note.priority, count, first));
            }
            if note.done.is_some() {
                done_drawn += 1;
            }
            if dialog.selected.as_deref() == Some(note.id.as_str()) {
                if let Some(open) = self.backlog_open_note(note, body.take(), window, cx) {
                    rows.push(open);
                }
                continue;
            }
            let place = waits.then(|| {
                let at = open.iter().position(|other| other.id == note.id);
                Place {
                    after: at
                        .and_then(|at| open.get(at + 1))
                        .filter(|next| next.priority == note.priority)
                        .map(|next| next.id.clone()),
                }
            });
            rows.push(self.backlog_row(index, note, place, cx));
        }
        if waiting == 0 && dialog.fresh.is_none() {
            rows.insert(
                0,
                caption(
                    format!(
                        "Nothing waiting. Write one above, drop files here, or press {} \
                         anywhere in ket.",
                        crate::shortcuts::CAPTURE_KEYS
                    ),
                    t,
                )
                .px(px(12.0))
                .py(px(10.0))
                .into_any_element(),
            );
        } else if open.is_empty() && dialog.fresh.is_none() && dialog.narrowed(&query) {
            rows.insert(
                0,
                caption("No waiting notes match.", t)
                    .px(px(12.0))
                    .py(px(10.0))
                    .into_any_element(),
            );
        }
        if done > 0 {
            let shown = dialog.show_done;
            // Above the done rows, which are the last ones when shown.
            let at = rows.len() - done_drawn;
            rows.insert(
                at,
                sized_row("backlog-done", false, px(32.0), t)
                    .mt(px(6.0))
                    .gap(px(6.0))
                    .text_color(alpha(paint(t.text.dim), 0.72))
                    .child(sized_icon(
                        if shown {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        px(12.0),
                        alpha(paint(t.text.dim), 0.72),
                    ))
                    .child(div().prose().text_size(crate::ui::CAPTION).child("Done"))
                    .child(
                        div()
                            .font_family(crate::fonts::chrome())
                            .text_size(ROW_FIGURE)
                            .child(done.to_string()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.backlog.as_mut() {
                            dialog.show_done = !dialog.show_done;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        div()
            .id("backlog-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(ROW_GAP)
            .overflow_y_scroll()
            .children(rows)
            .on_drop::<NoteDrag>(cx.listener(|this, drag: &NoteDrag, _, cx| {
                let target = this.backlog.as_mut().and_then(|dialog| dialog.drop.take());
                if let Some(target) = target {
                    this.move_backlog_note(&drag.id, target);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// The line that heads a priority's notes: its mark, its name and how
    /// many it holds, then a rule to the edge.
    fn backlog_group_heading(&self, priority: Priority, count: usize, first: bool) -> AnyElement {
        let t = &self.theme;
        let faint = alpha(paint(t.text.dim), 0.72);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(9.0))
            .px(px(16.0))
            .pt(if first { px(6.0) } else { px(18.0) })
            .pb(px(8.0))
            .child(
                div()
                    .prose()
                    .text_size(GROUP_TEXT)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(paint(t.text.dim))
                    .child(priority.name()),
            )
            .child(
                div()
                    .font_family(crate::fonts::chrome())
                    .text_size(ROW_FIGURE)
                    .text_color(faint)
                    .child(count.to_string()),
            )
            .child(div().flex_1().h(px(1.0)).bg(alpha(paint(t.border), 0.45)))
            .into_any_element()
    }

    /// One note in the queue: its title and tags over its first line, or
    /// the branch it went to once it is done, then its figures and its
    /// actions. A waiting note's `place` is where it sits among the others,
    /// for dragging; a waiting row can be picked up and dropped elsewhere.
    fn backlog_row(
        &self,
        index: usize,
        note: &Note,
        place: Option<Place>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let Some(dialog) = self.backlog.as_ref() else {
            return div().into_any_element();
        };
        let cursor = dialog.cursor.as_deref() == Some(note.id.as_str());
        let hovered = dialog.hovered.as_deref() == Some(note.id.as_str());
        let menu_open = dialog.row_menu.as_deref() == Some(note.id.as_str());
        let confirming = dialog.confirming_delete.as_deref() == Some(note.id.as_str());
        let picking = !dialog.picked.is_empty();
        let picked = dialog.picked.contains(&note.id);
        let faint = alpha(paint(t.text.dim), 0.72);
        let title = match note.title.trim() {
            "" => "Untitled".to_owned(),
            title => title.to_owned(),
        };
        let detail = match &note.done {
            Some(Done { branch: None, .. }) => div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .min_w_0()
                .child(sized_icon(Icon::CircleCheck, ROW_FIGURE, faint))
                .child(
                    div()
                        .text_size(ROW_DETAIL)
                        .text_color(faint)
                        .child("Marked done"),
                )
                .into_any_element(),
            Some(Done {
                branch: Some(branch),
                ..
            }) => div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .min_w_0()
                .child(sized_icon(Icon::GitBranch, ROW_FIGURE, faint))
                .child(
                    div()
                        .font_family(crate::fonts::chrome())
                        .text_size(ROW_FIGURE)
                        .text_color(faint)
                        .truncate()
                        .child(branch.clone()),
                )
                .into_any_element(),
            None => div()
                .text_size(ROW_DETAIL)
                .line_height(px(19.0))
                .text_color(alpha(paint(t.text.dim), 0.92))
                .truncate()
                .child(preview(&note.body))
                .into_any_element(),
        };
        let has_detail = note.done.is_some() || !note.body.trim().is_empty();
        let when = note
            .done
            .as_ref()
            .map_or(note.updated_ms, |done| done.at_ms);

        let figures = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(12.0))
            .w(ROW_END_W)
            .h(ROW_END_H)
            .pr(px(4.0))
            .font_family(crate::fonts::chrome())
            .text_size(ROW_FIGURE)
            .text_color(faint)
            .when(!note.attachments.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(sized_icon(Icon::Paperclip, ROW_FIGURE, faint))
                        .child(note.attachments.len().to_string()),
                )
            })
            .child(age(when));

        let right = if confirming {
            let id = note.id.clone();
            div()
                .flex()
                .flex_none()
                .h(ROW_END_H)
                .items_center()
                .gap(px(6.0))
                .child(caption("Delete it and its files?", t))
                .child(
                    button(("backlog-delete-cancel", index), "Cancel")
                        .ghost()
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(dialog) = this.backlog.as_mut() {
                                dialog.confirming_delete = None;
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .child(
                    button(("backlog-delete-confirm", index), "Delete")
                        .danger()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_backlog_note(&id);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
        } else if picking {
            // While notes are ticked, a row is something to tick: its own
            // actions would be a second way to act on one of them.
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.0))
                .child(figures)
        } else if note.done.is_none() && (cursor || hovered || menu_open) {
            // The row being looked at offers what can be done with it, in
            // its figures' place: Start, done, and the rest behind ⋯.
            let complete = note.id.clone();
            let start = note.id.clone();
            let more = note.id.clone();
            let open = self.backlog_menu_open(ROW_MENU) && menu_open;
            let panel = if open {
                self.backlog_row_menu_panel(cx)
            } else {
                None
            };
            let trigger = icon_button(("backlog-row-more", index), Icon::Ellipsis)
                .bare()
                .small()
                .pressed(open)
                .render(t);
            // Each says what it does and the chord that does it, the way the
            // header's buttons do; ⋯'s goes quiet while its menu is open.
            let tip_on = |which: &'static str| {
                dialog
                    .row_tip
                    .as_ref()
                    .is_some_and(|(id, tip)| id == &note.id && *tip == which)
            };
            let tip = |which: &'static str,
                       cell: AnyElement,
                       label: &'static str,
                       side: Side,
                       cx: &mut Context<Self>| {
                let owner = note.id.clone();
                tooltip(
                    (which, index),
                    cell,
                    label,
                    side,
                    tip_on(which) && !(which == "more" && open),
                    t,
                )
                .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                    let Some(dialog) = this.backlog.as_mut() else {
                        return;
                    };
                    let here = Some((owner.clone(), which));
                    if *over {
                        dialog.row_tip = here;
                    } else if dialog.row_tip == here {
                        dialog.row_tip = None;
                    }
                    cx.notify();
                }))
                .into_any_element()
            };
            let start_button = button(("backlog-row-start", index), "Start")
                .small()
                .leading(Icon::Play)
                .enabled(!dialog.starting)
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start_backlog_note(start.clone(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .into_any_element();
            let done_button = icon_button(("backlog-row-done", index), Icon::CircleCheck)
                .bare()
                .small()
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.complete_backlog_note(complete.clone(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .into_any_element();
            let start_tip = tip(
                "start",
                start_button,
                "Start in a new worktree (\u{21e7}\u{2318}\u{21b5})",
                Side::Top,
                cx,
            );
            let done_tip = tip("done", done_button, "Mark done (\u{2318}D)", Side::Top, cx);
            let more_tip = tip(
                "more",
                trigger.into_any_element(),
                "More: open, priority, tag, delete",
                Side::TopEnd,
                cx,
            );
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_end()
                .gap(px(2.0))
                .w(ROW_END_W)
                .h(ROW_END_H)
                .child(div().mr(px(6.0)).child(start_tip))
                .child(done_tip)
                .child(
                    dropdown(("backlog-row-more-menu", index), more_tip, panel)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.toggle_backlog_row_menu(&more, window);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
        } else {
            div().flex().flex_none().child(figures)
        };

        // Always drawn, done or not; a done note's is faded with its title.
        let mark = priority_mark_sized(note.priority, PRIORITY_MARK, note.done.is_some(), t);
        let tags = note.tags.iter().take(ROW_TAGS).map(|tag| {
            tinted_tag(format!("#{tag}"), paint(t.status.attention))
                .font_family(crate::fonts::chrome())
        });
        let more = note.tags.len().saturating_sub(ROW_TAGS);

        let id = note.id.clone();
        let tickable = place.is_some();
        let drop = cx.has_active_drag().then(|| dialog.drop.clone()).flatten();
        let line_above = drop
            .as_ref()
            .is_some_and(|drop| *drop == Drop::Before(note.id.clone()));
        let line_below = place.as_ref().is_some_and(|place| {
            place.after.is_none() && drop.as_ref() == Some(&Drop::End(note.priority))
        });
        // Selected is a fill, nothing more: the rail a list row draws for
        // the place it is on would say the same thing twice here.
        let hover_id = note.id.clone();
        let row = block_row(("backlog-row", index), false, t)
            .pl(px(16.0))
            .pr(px(12.0))
            .py(if has_detail { px(12.0) } else { px(14.0) })
            .rounded(ROW_RADIUS)
            .when(cursor || picked || menu_open, |el| {
                el.bg(alpha(paint(t.selection), 0.5))
            })
            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                let Some(dialog) = this.backlog.as_mut() else {
                    return;
                };
                let was = dialog.hovered.clone();
                if *over {
                    dialog.hovered = Some(hover_id.clone());
                } else if dialog.hovered.as_deref() == Some(hover_id.as_str()) {
                    dialog.hovered = None;
                }
                // Off the row is off its buttons too, though they may have
                // gone before they could say so.
                if !*over
                    && dialog
                        .row_tip
                        .as_ref()
                        .is_some_and(|(id, _)| id == &hover_id)
                {
                    dialog.row_tip = None;
                }
                if dialog.hovered != was {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(15.0))
                    .when(picking && tickable, |el| el.child(check_box(picked, t)))
                    .child(mark)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(ROW_TITLE)
                                            .line_height(px(21.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .when(note.done.is_some(), |el| {
                                                el.text_color(paint(t.text.dim))
                                            })
                                            .truncate()
                                            .child(title.clone()),
                                    )
                                    .children(tags)
                                    .when(more > 0, |el| el.child(tag(format!("+{more}"), t))),
                            )
                            .when(has_detail, |el| el.child(detail)),
                    )
                    .child(right),
            )
            .when(line_above, |el| {
                el.child(crate::tree::insertion_line(false, ROW_GAP, px(0.0), t))
            })
            .when(line_below, |el| {
                el.child(crate::tree::insertion_line(true, ROW_GAP, px(0.0), t))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let keys = event.modifiers();
                if tickable && (keys.platform || keys.control) {
                    this.toggle_backlog_pick(&id);
                } else if tickable && keys.shift {
                    this.pick_backlog_run(&id, cx);
                } else if tickable && picking {
                    this.toggle_backlog_pick(&id);
                } else {
                    this.select_backlog_note(Some(id.clone()), Some(window), cx);
                }
                cx.notify();
            }));
        let Some(place) = place else {
            return row.into_any_element();
        };

        // A waiting row can be picked up. `on_drag` starts only once the
        // pointer moves with the button down, so a click still opens it.
        let drag = NoteDrag {
            id: note.id.clone(),
            title: title.into(),
            priority: note.priority,
            theme: *t,
            grab: point(px(0.0), px(0.0)),
        };
        let own = note.id.clone();
        let priority = note.priority;
        row.on_drag(drag, |drag, grab, _, cx| {
            let mut drag = drag.clone();
            drag.grab = grab;
            cx.new(|_| drag)
        })
        .on_drag_move::<NoteDrag>(cx.listener(
            move |this, event: &DragMoveEvent<NoteDrag>, _, cx| {
                let bounds = event.bounds;
                let pointer = event.event.position;
                if pointer.y < bounds.top() || pointer.y > bounds.bottom() {
                    return;
                }
                let below = pointer.y > bounds.center().y;
                let target = match (below, &place.after) {
                    (false, _) => Drop::Before(own.clone()),
                    (true, Some(next)) => Drop::Before(next.clone()),
                    (true, None) => Drop::End(priority),
                };
                if let Some(dialog) = this.backlog.as_mut()
                    && dialog.drop.as_ref() != Some(&target)
                {
                    dialog.drop = Some(target);
                    cx.notify();
                }
            },
        ))
        .into_any_element()
    }

    /// The open note, in its row's place: its title, tags, description and
    /// files, what Start will make, and Save and Start — or, once it is
    /// done, where it went.
    fn backlog_open_note(
        &self,
        note: &Note,
        body: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        let input = dialog.title.as_ref()?;
        let t = &self.theme;
        let done = note.done.clone();

        let title = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(OPEN_PAD)
            .pt(px(14.0))
            .text_size(OPEN_TITLE)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(paint(if done.is_some() {
                t.text.dim
            } else {
                t.text.primary
            }))
            .child(
                text_line(
                    input,
                    "backlog-title",
                    Style::new(t, self.caret.visible),
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.body_active = false;
                    }
                    cx.notify();
                })),
            )
            .child(self.backlog_priority_picker(OPEN_PRIORITY, dialog.priority, cx));

        let tags = done.is_none().then(|| {
            let chips: Vec<_> = dialog
                .tags
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let name = name.clone();
                    removable(format!("#{name}"), t)
                        .id(("backlog-tag", index))
                        .font_family(crate::fonts::chrome())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_backlog_tag(&name);
                            cx.notify();
                        }))
                })
                .collect();
            let adder = match dialog.tag_input.as_ref() {
                Some(field) => text_field(
                    field,
                    "backlog-tag-input",
                    false,
                    Style::new(t, self.caret.visible).leading(Icon::Tag),
                    window,
                    cx,
                )
                .w(px(180.0))
                .h(px(28.0))
                .into_any_element(),
                None => button("backlog-tag-add", "Tag")
                    .ghost()
                    .small()
                    .leading(Icon::Tag)
                    .render(t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_backlog_tag_input(window, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            };
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .px(OPEN_PAD)
                .pt(px(8.0))
                .children(chips)
                .child(adder)
        });

        let status = done.as_ref().map(|done| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(OPEN_PAD)
                .pt(px(6.0))
                .map(|el| match &done.branch {
                    Some(branch) => el
                        .child(tag("Started", t))
                        .child(caption(format!("in {branch}"), t)),
                    None => el.child(tag("Done", t)),
                })
        });

        let chips = note.attachments.iter().enumerate().map(|(index, name)| {
            let name = name.clone();
            removable(name.clone(), t)
                .id(("backlog-file", index))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.detach_from_backlog_note(&name);
                    cx.notify();
                }))
        });
        let files = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .px(OPEN_PAD)
            .pt(px(10.0))
            .children(chips)
            .child(
                button("backlog-attach", "Attach")
                    .ghost()
                    .small()
                    .leading(Icon::Paperclip)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.choose_backlog_files(cx);
                        cx.notify();
                    })),
            )
            .when(note.attachments.is_empty(), |el| {
                el.child(
                    caption("or drop files here. Images reach the agent as images.", t)
                        .text_color(alpha(paint(t.text.dim), 0.72)),
                )
            });

        // What Start will make, and what its agent will read — only while
        // there is still something to start.
        let (options, brief) = match done {
            Some(_) => (None, None),
            None => (
                Some(self.backlog_start_options(cx)),
                dialog
                    .preview
                    .then(|| self.backlog_brief(note, input.read(cx).text())),
            ),
        };

        let delete = note.id.clone();
        let confirming = dialog.confirming_delete.as_deref() == Some(note.id.as_str());
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(52.0))
            .px(px(10.0));
        let actions = if confirming {
            actions
                .child(caption("Delete this note and its files?", t).pl(px(6.0)))
                .child(div().flex_grow())
                .child(
                    button("backlog-delete-cancel", "Cancel")
                        .ghost()
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(dialog) = this.backlog.as_mut() {
                                dialog.confirming_delete = None;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    button("backlog-delete-confirm", "Delete")
                        .danger()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_backlog_note(&delete);
                            cx.notify();
                        })),
                )
        } else {
            let actions = actions
                .child(
                    icon_button("backlog-delete", Icon::Trash)
                        .bare()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(dialog) = this.backlog.as_mut() {
                                dialog.confirming_delete = Some(delete.clone());
                            }
                            cx.notify();
                        })),
                )
                .child(div().flex_grow())
                .child(
                    button(
                        "backlog-cancel",
                        if done.is_some() { "Close" } else { "Cancel" },
                    )
                    .ghost()
                    .render(t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_backlog_note(window);
                        cx.notify();
                    })),
                );
            match &done {
                Some(done) => {
                    let exists = done
                        .worktree
                        .as_ref()
                        .is_some_and(|worktree| self.position_of(worktree).is_some());
                    let started = done.worktree.is_some();
                    actions
                        .child(
                            button("backlog-reopen", "Reopen")
                                .leading(Icon::RotateCcw)
                                .render(t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reopen_backlog_note();
                                    cx.notify();
                                })),
                        )
                        .when(started, |el| {
                            el.child(
                                button(
                                    "backlog-goto",
                                    if exists {
                                        "Open worktree"
                                    } else {
                                        "Worktree removed"
                                    },
                                )
                                .primary()
                                .leading(Icon::GitBranch)
                                .enabled(exists)
                                .render(t)
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.open_backlog_worktree(cx);
                                        cx.notify();
                                    },
                                )),
                            )
                        })
                }
                None => {
                    let typed = input.read(cx).text();
                    let words = [typed.trim(), note.body.trim()]
                        .into_iter()
                        .find(|part| !part.is_empty())
                        .unwrap_or_default();
                    let start = note.id.clone();
                    let complete = note.id.clone();
                    actions
                        .child(
                            button("backlog-done", "Mark done")
                                .ghost()
                                .leading(Icon::CircleCheck)
                                .render(t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.complete_backlog_note(complete.clone(), cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            button("backlog-start", "Start in worktree")
                                .leading(Icon::Play)
                                .detail(branch_seeded(words, "backlog", dialog.branch_seed))
                                .hint("\u{21e7}\u{2318}\u{21b5}")
                                .loading_if(dialog.starting, "Creating the worktree\u{2026}")
                                .render(t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_backlog_note(start.clone(), cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            button("backlog-save", "Save")
                                .primary()
                                .leading(Icon::Check)
                                .hint("\u{2318}\u{21b5}")
                                .render(t)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_backlog_note(window, cx);
                                    cx.notify();
                                })),
                        )
                }
            }
        };

        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .my(px(2.0))
                .rounded(crate::ui::RADIUS_LG)
                .bg(paint(t.sunken))
                .border_1()
                .border_color(paint(t.border))
                .child(title)
                .children(tags)
                .children(status)
                .children(body)
                .child(files)
                .children(options)
                .children(brief)
                .child(actions)
                .into_any_element(),
        )
    }
}
