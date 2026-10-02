//! Keeping the backlog in order: a search and the notes' tags narrow the
//! list, a note's tags are added and taken off in the open note, and a
//! waiting note is dragged — or moved with ⌥ and an arrow — to where it
//! belongs among the others.

use gpui::{
    AnyElement, BoxShadow, Context, Pixels, Point, Render, SharedString, Window, div, point,
    prelude::*, px,
};
use ket_core::backlog::{Backlog, Priority, normalised_tags};
use ket_core::theme::Theme;

use super::{PRIORITY_MARK, priority_mark_sized};
use crate::Shell;
use crate::fonts::Prose;
use crate::input::{Style, TextInput, text_field};
use crate::paint::{alpha, paint};
use crate::ui::button::pressable;
use crate::ui::chip::{kbd, tag, tinted_tag};
use crate::ui::icon::Icon;

/// How wide the search field is: room for a few words, the rest of the line
/// left to the tags.
const SEARCH_W: Pixels = px(300.0);

/// The search field's height: a step under the composer above it, so the
/// two read as the note being written and the list being looked through.
const SEARCH_H: Pixels = px(32.0);

/// Most tags offered beside the search: the most used ones.
const FILTER_TAGS: usize = 10;

/// How wide the note under the pointer is while it is dragged.
const GHOST_W: Pixels = px(420.0);

/// Where a dragged note would land.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Drop {
    /// Just before this waiting note, taking its priority.
    Before(String),
    /// After the last waiting note of this priority.
    End(Priority),
}

/// A waiting note picked up to be dropped elsewhere in the list.
#[derive(Clone)]
pub(crate) struct NoteDrag {
    /// The note's id.
    pub(super) id: String,
    /// Its title, drawn under the pointer.
    pub(super) title: SharedString,
    /// Its priority, whose mark goes with it.
    pub(super) priority: Priority,
    /// The theme. The thing under the pointer is its own view, with no
    /// access to the shell's.
    pub(super) theme: Theme,
    /// Where in the row the pointer went down: gpui draws the view with its
    /// top-left there, so it starts where the row is.
    pub(super) grab: Point<Pixels>,
}

impl Render for NoteDrag {
    /// The note's mark and title, which is all the thing moving has to say:
    /// the row itself stays where it was until the drop lands.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .w(GHOST_W)
            .px(px(12.0))
            .py(px(8.0))
            .rounded(crate::ui::RADIUS_LG)
            .bg(paint(t.elevated))
            .border_1()
            .border_color(paint(t.border))
            .shadow(vec![BoxShadow {
                color: crate::paint::shadow(0.5, &t),
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(12.0),
                spread_radius: px(0.0),
            }])
            .child(priority_mark_sized(self.priority, PRIORITY_MARK, false, &t))
            .child(
                div()
                    .prose()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.0))
                    .text_color(paint(t.text.primary))
                    .child(self.title.clone()),
            )
    }
}

/// A line written into the capture field, its `#tags` taken out: the words
/// left are the title, and the tags are kept as a note keeps them.
pub(super) fn split_tags(line: &str) -> (String, Vec<String>) {
    let mut words = Vec::new();
    let mut tags = Vec::new();
    for word in line.split_whitespace() {
        match word.strip_prefix('#') {
            Some(name) if !name.trim_start_matches('#').is_empty() => tags.push(name.to_owned()),
            _ => words.push(word),
        }
    }
    (words.join(" "), normalised_tags(tags))
}

/// The tags the waiting notes carry, the most used first, then by name.
fn tags_in_use(notes: &[ket_core::backlog::Note]) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for tag in notes
        .iter()
        .filter(|note| note.done.is_none())
        .flat_map(|note| note.tags.iter())
    {
        match counts
            .iter_mut()
            .find(|(seen, _)| seen.to_lowercase() == tag.to_lowercase())
        {
            Some((_, count)) => *count += 1,
            None => counts.push((tag.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.into_iter().map(|(tag, _)| tag).collect()
}

impl Shell {
    /// The line under the capture field: the search, and the tags in use,
    /// each a press away from narrowing the list to it. `None` while there
    /// is nothing to narrow.
    pub(super) fn backlog_filters(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        if dialog.notes.is_empty() {
            return None;
        }
        let t = &self.theme;
        let search = text_field(
            &dialog.search,
            "backlog-search",
            false,
            Style::new(t, self.caret.visible).leading(Icon::Search),
            window,
            cx,
        )
        .w(SEARCH_W)
        .h(SEARCH_H)
        .flex_none()
        .child(kbd("\u{2318}F", crate::fonts::chrome(), t))
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(dialog) = this.backlog.as_mut() {
                dialog.body_active = false;
                dialog.capture_body_active = false;
            }
            cx.notify();
        }));
        let attention = paint(t.status.attention);
        let chips = tags_in_use(&dialog.notes)
            .into_iter()
            .take(FILTER_TAGS)
            .enumerate()
            .map(|(index, name)| {
                let on = dialog
                    .tag_filter
                    .as_ref()
                    .is_some_and(|wanted| wanted.to_lowercase() == name.to_lowercase());
                let label = format!("#{name}");
                let chip = if on {
                    tinted_tag(label, attention)
                } else {
                    tag(label, t)
                };
                pressable(("backlog-filter-tag", index), t)
                    .p(px(2.0))
                    .child(chip.font_family(crate::fonts::chrome()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(dialog) = this.backlog.as_mut() {
                            dialog.tag_filter = if on { None } else { Some(name.clone()) };
                        }
                        cx.notify();
                    }))
            });
        // How the list is ordered, said once where the search is: a list
        // that can be dragged does not look sorted, and this says it is.
        let order = div()
            .flex_none()
            .prose()
            .text_size(crate::ui::CAPTION)
            .text_color(alpha(paint(t.text.dim), 0.72))
            .child("Sorted by priority, then your order");
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.0))
                .pt(px(2.0))
                .child(search)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(px(4.0))
                        .overflow_hidden()
                        .children(chips),
                )
                .child(order)
                .into_any_element(),
        )
    }

    /// Brings out the field a tag is typed into on the open note.
    pub(super) fn open_backlog_tag_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_ref() else {
            return;
        };
        let field = match dialog.tag_input.clone() {
            Some(field) => field,
            None => {
                let field = TextInput::new("tag", cx);
                self.watch_field(&field, cx);
                field
            }
        };
        window.focus(field.read(cx).focus_handle());
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.body_active = false;
            dialog.tag_input = Some(field);
        }
    }

    /// The same field, brought out from a menu that has no window to hand:
    /// it takes the keyboard as it is first drawn.
    pub(super) fn open_backlog_tag_field(&mut self, cx: &mut Context<Self>) {
        if self
            .backlog
            .as_ref()
            .is_none_or(|dialog| dialog.selected.is_none())
        {
            return;
        }
        let field = TextInput::new("tag", cx);
        self.watch_field(&field, cx);
        field.update(cx, |input, _| input.request_focus());
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.body_active = false;
            dialog.tag_input = Some(field);
        }
    }

    /// Adds what the tag field holds to the open note, and empties it for
    /// another.
    pub(super) fn add_backlog_tag(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(field) = dialog.tag_input.clone() else {
            return;
        };
        let typed = field.read(cx).text();
        let mut tags = dialog.tags.clone();
        tags.extend(typed.split([',', ' ']).map(str::to_owned));
        dialog.tags = normalised_tags(tags);
        field.update(cx, |input, _| input.clear());
    }

    /// Takes the open note's last tag off: Backspace in an empty tag field.
    pub(super) fn drop_last_backlog_tag(&mut self) {
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.tags.pop();
        }
    }

    /// Takes `name` off the open note.
    pub(super) fn remove_backlog_tag(&mut self, name: &str) {
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.tags.retain(|tag| tag != name);
        }
    }

    /// Puts the note `id` where `drop` says, and the list in the order that
    /// leaves.
    pub(super) fn move_backlog_note(&mut self, id: &str, drop: Drop) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let moved = match drop {
            Drop::Before(before) if before == id => return,
            Drop::Before(before) => Backlog::reorder(&dialog.project, id, Some(&before)),
            Drop::End(priority) => {
                // The end of another priority: the note takes that priority
                // first, then goes to the end of it.
                let Some(mut note) = dialog.saved(id).cloned() else {
                    return;
                };
                let moved = if note.priority == priority {
                    Ok(())
                } else {
                    note.priority = priority;
                    Backlog::save_note(&dialog.project, &note).map(|_| ())
                };
                moved.and_then(|()| Backlog::reorder(&dialog.project, id, None))
            }
        };
        match moved {
            Ok(backlog) => {
                dialog.take(backlog);
                dialog.cursor = Some(id.to_owned());
                dialog.error = None;
            }
            Err(error) => dialog.error = Some(error.to_string()),
        }
    }

    /// ⌥↑ and ⌥↓: moves the arrows' row one place up or down among the
    /// notes of its priority, as they are shown.
    pub(super) fn nudge_backlog_note(&mut self, down: bool, cx: &mut Context<Self>) {
        let query = self.backlog_query(cx);
        let Some(dialog) = self.backlog.as_ref() else {
            return;
        };
        let Some(id) = dialog.cursor.clone() else {
            return;
        };
        let open = dialog.open_rows(&query);
        let Some(at) = open.iter().position(|note| note.id == id) else {
            return;
        };
        let priority = open[at].priority;
        let group: Vec<&str> = open
            .iter()
            .filter(|note| note.priority == priority)
            .map(|note| note.id.as_str())
            .collect();
        let Some(place) = group.iter().position(|other| *other == id) else {
            return;
        };
        let drop = match down {
            false if place == 0 => return,
            false => Drop::Before(group[place - 1].to_owned()),
            true if place + 1 >= group.len() => return,
            true => match group.get(place + 2) {
                Some(next) => Drop::Before((*next).to_owned()),
                None => Drop::End(priority),
            },
        };
        self.move_backlog_note(&id, drop);
    }
}

#[cfg(test)]
mod tests {
    use super::split_tags;

    fn split(line: &str) -> (String, Vec<String>) {
        split_tags(line)
    }

    #[test]
    fn hash_words_in_a_captured_line_become_its_tags() {
        assert_eq!(
            split("Fix relay #phone drops #Relay"),
            (
                "Fix relay drops".to_owned(),
                vec!["phone".to_owned(), "Relay".to_owned()]
            )
        );
    }

    #[test]
    fn a_lone_hash_and_a_hash_inside_a_word_stay_in_the_title() {
        assert_eq!(
            split("Port the C# client # later"),
            ("Port the C# client # later".to_owned(), Vec::new())
        );
    }

    #[test]
    fn a_line_of_only_tags_has_no_title() {
        assert_eq!(
            split("#ui #ui #phone"),
            (String::new(), vec!["ui".to_owned(), "phone".to_owned()])
        );
    }
}
