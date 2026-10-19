//! Review comments on a diff, sent to the agent as one message.
//!
//! Lines picked in a diff tab (see [`crate::diff_view`]) can be commented
//! on: ⌘↵ or the header's Comment button opens a composer in the diff itself,
//! as a row under the last line picked that moves the lines after it down —
//! the way a pull request review does it — and what is
//! written there joins the worktree's review, shown as a card in the same
//! place, with Edit and Delete. The review collects
//! in the Git panel's Review section, across files and comparisons, until
//! Send to agent types it into the worktree's agent terminal as one message —
//! the way a reviewer leaves several comments on a pull request and submits
//! them together, rather than interrupting the agent once per line.
//!
//! **Snapshots.** A comment keeps the lines it was about, as text, and which
//! lines they were, as they read when it was written. The agent goes on
//! editing the file, and a comment that pointed at row indices into a diff
//! read since would point at the wrong code; the quoted lines still say what
//! was meant.
//!
//! **In memory only.** Pending comments go with the window. They are a draft
//! of one message, not a record.
//!
//! **From a phone.** A phone keeps its own comments and sends them all at
//! once; the host hands them to a window, which writes them into the same
//! message and refuses them for the same reasons — see
//! [`Shell::send_review_from_phone`].

use gpui::{
    AnyElement, Context, Div, KeyDownEvent, MouseButton, SharedString, Window, div, prelude::*, px,
    relative,
};
use ket_core::activity::Signal;
use ket_core::host::PhoneReview;
use ket_core::id::WorktreeId;
use ket_core::theme::Theme;

use crate::Shell;
use crate::backlog::fence_for;
use crate::diff_view::{DiffKey, DiffPick};
use crate::editor::REVIEW_EDITOR;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::tabs::TabKind;
use crate::terminal::TerminalId;
use crate::ui::button::button;
use crate::ui::chip::{caption, key_hint};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::textarea::textarea;
use crate::ui::toast::Tone;

/// What the comment is typed at.
const DRAFT_TEXT: gpui::Pixels = px(13.0);

/// The composer's height: three or four lines, which is what a comment on a
/// line runs to; a longer one scrolls.
const DRAFT_H: gpui::Pixels = px(116.0);

/// The widest a card under the lines gets, so a comment reads as a note
/// beside the code and not as a banner across a wide pane.
const CARD_W: gpui::Pixels = px(640.0);

/// One comment waiting to be sent.
#[derive(Debug)]
pub(crate) struct ReviewComment {
    /// Which comment, for the Review section's rows.
    pub(crate) id: usize,
    /// The file and the comparison it was read in.
    pub(crate) key: DiffKey,
    /// Which lines, as the diff said when the comment was written:
    /// `lines 12–18`.
    pub(crate) lines: String,
    /// The first and last of the new file's line numbers picked, for marking
    /// them in the diff.
    pub(crate) new_span: Option<(u32, u32)>,
    /// The same of the old file's, which is all a removed line has.
    pub(crate) old_span: Option<(u32, u32)>,
    /// The lines themselves, each with its `+`, `-` or space.
    pub(crate) text: String,
    /// What should change.
    pub(crate) comment: String,
}

impl ReviewComment {
    /// The file's name, without its directory.
    pub(crate) fn name(&self) -> &str {
        self.key.rel.rsplit('/').next().unwrap_or(&self.key.rel)
    }
}

/// A review sent to an agent whose local model was busy: whose it is, and
/// its comments, held until the message goes.
pub(crate) type QueuedReview = (WorktreeId, Vec<ReviewComment>);

/// The composer a comment is written in, while it is open.
pub(crate) struct ReviewDraft {
    /// Whose review it joins.
    worktree: WorktreeId,
    /// The diff the lines were picked in.
    pub(crate) key: DiffKey,
    /// The lines.
    pub(crate) pick: DiffPick,
    /// The comment being edited, when it is one already in the review
    /// rather than a new one: its card gives way to the composer.
    pub(crate) editing: Option<usize>,
    /// Whether keys go to it. A click elsewhere takes them away from a draft
    /// with words in it rather than throwing the words out; a click on the
    /// composer gives them back.
    focused: bool,
    /// Why it could not be added.
    error: Option<SharedString>,
}

/// What a diff is compared against, as a review says it.
fn against(key: &DiffKey) -> String {
    match &key.base {
        Some((base, _)) => format!("against {base}"),
        None => "uncommitted".to_owned(),
    }
}

/// The review as the agent reads it: a line saying whose and against what,
/// then each comment numbered, with its file and lines, the lines fenced as
/// a diff, and what should change under them.
///
/// When the comments were not all read against the same comparison, each
/// says its own rather than the first line saying one for all.
pub(crate) fn review_message(branch: &str, comments: &[ReviewComment]) -> String {
    let shared = comments
        .first()
        .map(|first| &first.key.base)
        .filter(|base| comments.iter().all(|comment| &comment.key.base == *base));
    let mut message = match (shared, comments.first()) {
        (Some(_), Some(first)) => {
            format!("Review comments on {branch} ({}):", against(&first.key))
        }
        _ => format!("Review comments on {branch}:"),
    };
    for (index, comment) in comments.iter().enumerate() {
        let mut place = comment.key.rel.to_string();
        if !comment.lines.is_empty() {
            place.push_str(", ");
            place.push_str(&comment.lines);
        }
        if shared.is_none() {
            place.push_str(&format!(" ({})", against(&comment.key)));
        }
        let text = comment.text.trim_end_matches(['\n', '\r']);
        let fence = fence_for(text);
        message.push_str(&format!(
            "\n\n{}. {place}:\n{fence}diff\n{text}\n{fence}\n{}",
            index + 1,
            comment.comment.trim()
        ));
    }
    message
}

impl Shell {
    /// The worktree whose review a comment on `key` joins: the one checked
    /// out at its root, or else the one whose space holds the tab.
    ///
    /// By root first because a pinned explorer can show a diff of a worktree
    /// other than the selected one, and the comment is about that worktree's
    /// code. A row in the Review section still reopens its diff in the
    /// selected worktree's space, as every diff opened from the Git panel
    /// does.
    pub(crate) fn review_worktree_of(&self, key: &DiffKey) -> Option<WorktreeId> {
        let root = std::path::Path::new(key.root.as_ref());
        self.projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .find(|node| node.path == root)
            .map(|node| node.id.clone())
            .or_else(|| {
                let tab = TabKind::Diff(key.clone());
                self.spaces
                    .iter()
                    .find(|(_, space)| space.root.find_tab(&tab).is_some())
                    .map(|(id, _)| id.clone())
            })
    }

    /// The branch of `id`, or its name when it has none to show.
    fn review_branch(&self, id: &WorktreeId) -> SharedString {
        self.worktree_node(id)
            .map(|node| node.branch.clone())
            .unwrap_or_else(|| id.as_str().to_owned().into())
    }

    /// ⌘↵ or Comment in a diff: opens the composer under the lines picked
    /// in `key`'s tab. Does nothing, and says so by returning `false`, with
    /// nothing picked or no worktree to file the comment under. One draft at
    /// a time: while one is open this only hands it the keys back.
    pub(crate) fn open_review_draft(&mut self, key: DiffKey) -> bool {
        if let Some(draft) = self.review_draft.as_mut() {
            draft.focused = true;
            return true;
        }
        let Some(worktree) = self.review_worktree_of(&key) else {
            return false;
        };
        let Some(pick) = self.diff_selection_of(&key) else {
            return false;
        };
        self.start_review_draft(worktree, key, pick, None, "");
        true
    }

    /// Edit on a comment's card: the composer takes the card's place with
    /// the comment in it, and adding saves over it.
    pub(crate) fn edit_review_comment(&mut self, worktree: &WorktreeId, id: usize) {
        if self.review_draft.is_some() {
            return;
        }
        let Some(comment) = self
            .reviews
            .get(worktree)
            .and_then(|comments| comments.iter().find(|comment| comment.id == id))
        else {
            return;
        };
        let pick = DiffPick {
            text: comment.text.clone(),
            lines: comment.lines.clone(),
            new_span: comment.new_span,
            old_span: comment.old_span,
        };
        let (key, said) = (comment.key.clone(), comment.comment.clone());
        self.start_review_draft(worktree.clone(), key, pick, Some(id), &said);
    }

    fn start_review_draft(
        &mut self,
        worktree: WorktreeId,
        key: DiffKey,
        pick: DiffPick,
        editing: Option<usize>,
        text: &str,
    ) {
        self.popup = None;
        self.menu = None;
        self.open_composer(REVIEW_EDITOR, text);
        self.set_composer_size(REVIEW_EDITOR, DRAFT_TEXT);
        self.review_draft = Some(ReviewDraft {
            worktree,
            key,
            pick,
            editing,
            focused: true,
            error: None,
        });
    }

    /// Closes the composer and lets go of the lines it was for, as a
    /// cancelled comment leaves nothing behind.
    fn close_review_draft(&mut self) {
        if let Some(draft) = self.review_draft.take()
            && draft.editing.is_none()
        {
            self.clear_diff_pick(&draft.key);
        }
        self.close_composer(REVIEW_EDITOR);
    }

    /// A click that missed the composer. An empty draft closes, as nothing
    /// is lost; one with words in it stays where it is and lets go of the
    /// keys, so the click's target gets them. Says whether anything changed.
    pub(crate) fn blur_review_draft(&mut self) -> bool {
        let empty = self.composer_text(REVIEW_EDITOR).trim().is_empty();
        match self.review_draft.as_mut() {
            None => false,
            Some(_) if empty => {
                self.close_review_draft();
                true
            }
            Some(draft) => std::mem::replace(&mut draft.focused, false),
        }
    }

    /// A press in the diff's gutter: an empty composer closes so the press
    /// can pick for a new comment — keeping its lines with `extend`, which
    /// is a ⇧-press running them on. Says whether no composer is left open.
    pub(crate) fn discard_empty_review_draft(&mut self, extend: bool) -> bool {
        if self.review_draft.is_none() {
            return true;
        }
        if !self.composer_text(REVIEW_EDITOR).trim().is_empty() {
            return false;
        }
        if extend {
            self.review_draft = None;
            self.close_composer(REVIEW_EDITOR);
        } else {
            self.close_review_draft();
        }
        true
    }

    /// Whether the composer holds the keys, for [`Shell::typing`].
    pub(crate) fn review_draft_focused(&self) -> bool {
        self.review_draft
            .as_ref()
            .is_some_and(|draft| draft.focused)
    }

    /// Adds the comment to its worktree's review, or saves over the one
    /// being edited, and closes the composer. No toast: the card in its
    /// place is the answer.
    fn add_review_comment(&mut self) {
        let comment = self.composer_text(REVIEW_EDITOR).trim().to_owned();
        let Some(draft) = self.review_draft.as_mut() else {
            return;
        };
        if comment.is_empty() {
            draft.error = Some("Write what should change first.".into());
            return;
        }
        let Some(draft) = self.review_draft.take() else {
            return;
        };
        self.close_composer(REVIEW_EDITOR);
        if let Some(id) = draft.editing {
            if let Some(edited) = self
                .reviews
                .get_mut(&draft.worktree)
                .and_then(|comments| comments.iter_mut().find(|comment| comment.id == id))
            {
                edited.comment = comment;
            }
            return;
        }
        // The lines are taken; what marks them now is the comment's card.
        self.clear_diff_pick(&draft.key);
        self.next_review_id += 1;
        let pick = draft.pick;
        self.reviews
            .entry(draft.worktree)
            .or_default()
            .push(ReviewComment {
                id: self.next_review_id,
                key: draft.key,
                lines: pick.lines,
                new_span: pick.new_span,
                old_span: pick.old_span,
                text: pick.text,
                comment,
            });
    }

    /// Takes one comment out of `id`'s review.
    pub(crate) fn remove_review_comment(&mut self, id: &WorktreeId, comment: usize) {
        if let Some(comments) = self.reviews.get_mut(id) {
            comments.retain(|pending| pending.id != comment);
            if comments.is_empty() {
                self.reviews.remove(id);
            }
        }
    }

    /// Empties `id`'s review without sending it.
    pub(crate) fn clear_review(&mut self, id: &WorktreeId) {
        self.reviews.remove(id);
    }

    /// The terminal `id`'s review is typed into: its agent's. Refused while
    /// the agent is stopped on a permission prompt, where what is typed
    /// could answer it, and when no agent terminal is open: a review is
    /// feedback on a session somebody is watching, so it is not handed to a
    /// headless run the way the quick prompt's is. Why, as the toast and a
    /// phone say it.
    fn review_target(&self, id: &WorktreeId, branch: &str) -> Result<TerminalId, String> {
        if self.activity.signal(id, ket_core::now_ms()) == Signal::Blocked {
            return Err(format!(
                "{branch}'s agent is waiting on a prompt. Answer it first."
            ));
        }
        self.agent_terminal(id)
            .ok_or_else(|| format!("No agent is running in {branch}"))
    }

    /// Send to agent: types `id`'s review into its agent's terminal as one
    /// message, and empties it — see [`Shell::review_target`] for when it
    /// refuses, which leaves the review as it was.
    ///
    /// A local agent whose model is busy has the message queued instead, and
    /// the queue holds the comments with it: should the tab close before it
    /// goes, they come back to the review rather than being lost.
    pub(crate) fn send_review(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let branch = self.review_branch(&id);
        let count = self.reviews.get(&id).map_or(0, Vec::len);
        if count == 0 {
            self.toast(
                Tone::Info,
                format!("Nothing in the review for {branch}"),
                cx,
            );
            return;
        }
        let terminal = match self.review_target(&id, &branch) {
            Ok(terminal) => terminal,
            Err(why) => {
                self.toast(Tone::Warning, why, cx);
                return;
            }
        };
        let Some(comments) = self.reviews.remove(&id) else {
            return;
        };
        let message = review_message(&branch, &comments);
        // The queue says so itself when it holds the message back.
        if !self.submit_or_queue_review(terminal, &message, Some((id, comments)), cx) {
            self.toast(
                Tone::Info,
                format!("Sent {} to {branch}", comments_said(count)),
                cx,
            );
        }
    }

    /// Sends the reviews phones have asked for since the last look. Looked
    /// for on the status wake, not the slower tick, because the phone is
    /// waiting for the answer. Says whether there were any.
    pub(crate) fn take_phone_reviews(&mut self, cx: &mut Context<Self>) -> bool {
        let mut any = false;
        while let Some(review) = ket_core::host::take_phone_review() {
            self.send_review_from_phone(review, cx);
            any = true;
        }
        any
    }

    /// A phone's Send to agent: its comments written into the message the
    /// Review section's own button sends, refused for the same reasons, and
    /// the phone told which — see [`ket_core::host::finish_phone_review`].
    /// The window is left where it is.
    ///
    /// Refused too while a local agent's model is busy, where the desktop's
    /// own button queues the review: the phone keeps its comments only until
    /// it is told they went, and a queued review whose tab closed would come
    /// back here rather than to the phone.
    ///
    /// The comments carry no line numbers to mark in a diff tab: they were
    /// picked in the phone's diff.
    pub(crate) fn send_review_from_phone(&mut self, review: PhoneReview, cx: &mut Context<Self>) {
        // The phone has been told it did not go, and may send it again.
        if ket_core::now_ms() > review.until_ms {
            tracing::warn!(
                req = review.req,
                "dropped a phone's review that came too late"
            );
            return;
        }
        let id = WorktreeId::from(review.worktree.as_str());
        let Some(node) = self.worktree_node(&id) else {
            ket_core::host::finish_phone_review(
                review.req,
                Some("That worktree isn't open in ket on your Mac".to_owned()),
            );
            return;
        };
        let root = SharedString::from(node.path.display().to_string());
        // What a diff tab of the worktree since its base is keyed by, so the
        // message names the base as the desktop does.
        let base = review.base.map(|name| {
            node.base
                .clone()
                .unwrap_or_else(|| (name.clone().into(), name.into()))
        });
        let branch = self.review_branch(&id);
        let terminal = match self.review_target(&id, &branch) {
            Ok(terminal) => terminal,
            Err(why) => {
                ket_core::host::finish_phone_review(review.req, Some(why));
                return;
            }
        };
        if let Some(why) = self.local_queue_wait(terminal) {
            ket_core::host::finish_phone_review(
                review.req,
                Some(format!("{why}. Send again when it's done.")),
            );
            return;
        }
        let comments: Vec<ReviewComment> = review
            .comments
            .into_iter()
            .map(|comment| ReviewComment {
                // Written into the message only, never into this window's
                // review.
                id: 0,
                key: DiffKey {
                    root: root.clone(),
                    rel: comment.rel.into(),
                    base: base.clone(),
                },
                lines: comment.lines,
                new_span: None,
                old_span: None,
                text: comment.text,
                comment: comment.comment,
            })
            .collect();
        let count = comments.len();
        let message = review_message(&branch, &comments);
        let req = review.req;
        // The phone lets go of its comments when it is told they went, so it
        // is told once Enter has gone in, not when the paste has.
        self.submit_now_then(terminal, &message, cx, move |shell, entered, cx| {
            if !entered {
                ket_core::host::finish_phone_review(
                    req,
                    Some(format!(
                        "{branch}'s agent closed before the review reached it"
                    )),
                );
                return;
            }
            ket_core::host::finish_phone_review(req, None);
            shell.toast(
                Tone::Info,
                format!("Sent {} to {branch} from your phone", comments_said(count)),
                cx,
            );
        });
    }

    /// Puts a queued review whose tab closed before it went back in front
    /// of whatever was commented since, and says so.
    pub(crate) fn restore_review(&mut self, review: QueuedReview, cx: &mut Context<Self>) {
        let (id, mut comments) = review;
        if let Some(later) = self.reviews.remove(&id) {
            comments.extend(later);
        }
        self.reviews.insert(id.clone(), comments);
        let branch = self.review_branch(&id);
        self.toast(
            Tone::Warning,
            format!("The review for {branch} wasn't sent: its agent's tab closed"),
            cx,
        );
    }

    /// Keys while the panel is up. It takes every one.
    pub(crate) fn review_draft_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let active = self.active_diff();
        let Some(draft) = self.review_draft.as_mut() else {
            return false;
        };
        if !draft.focused {
            return false;
        }
        // Its tab went to the back by a key the shell answers first: the
        // composer is out of sight, so it does not keep the keyboard.
        if active.as_ref() != Some(&draft.key) {
            draft.focused = false;
            return false;
        }
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        match key.key.as_str() {
            "escape" => self.close_review_draft(),
            "enter" if command => self.add_review_comment(),
            _ => {
                self.composer_key(REVIEW_EDITOR, key, cx);
                if let Some(draft) = self.review_draft.as_mut() {
                    draft.error = None;
                }
            }
        }
        true
    }

    /// The composer, as the diff draws it under the lines: which they are,
    /// the text, and Cancel and Add. No quote of the code — it is the row
    /// above. Keys reach it through [`Shell::review_draft_key`]; a press on
    /// it takes them back after a click elsewhere let them go.
    pub(crate) fn review_draft_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The pane borrows the shell mutably, so it is built before the
        // draft is read.
        let editor = self.editor_pane(REVIEW_EDITOR, None, window, cx);
        let empty = self.composer_text(REVIEW_EDITOR).trim().is_empty();
        let Some(draft) = self.review_draft.as_ref() else {
            return div().into_any_element();
        };
        let t = &self.theme;
        let editing = draft.editing.is_some();

        let cancel = button("review-cancel", "Cancel")
            .ghost()
            .small()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_review_draft();
                cx.notify();
            }));
        let add = button("review-add", if editing { "Save" } else { "Add to review" })
            .primary()
            .small()
            .hint("\u{2318}\u{21b5}")
            .enabled(!empty)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.add_review_comment();
                cx.notify();
            }));
        let note = match &draft.error {
            Some(why) => caption(why.clone(), t).text_color(paint(t.status.failed)),
            None => key_hint("esc", "cancel", t),
        };
        let footer = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.0))
            .child(note)
            .child(div().flex_grow())
            .child(cancel)
            .child(add);
        let well = textarea("review-draft-well", editor, DRAFT_H)
            .active(draft.focused)
            .footer(footer);
        let well = match empty {
            true => well.placeholder("What should change?", DRAFT_TEXT),
            false => well,
        }
        .render(t);

        review_card(t)
            .id("review-draft")
            // Over the root's: a press here is not a click elsewhere.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(draft) = this.review_draft.as_mut() {
                        draft.focused = true;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(card_heading(&draft.pick.lines, t))
            .child(well)
            .into_any_element()
    }

    /// A comment in the review, as the diff draws it under its lines: which
    /// lines, what it says, and Edit and Delete. Empty when the comment has
    /// gone, which the next frame's rows will agree with.
    pub(crate) fn review_comment_card(&self, id: usize, cx: &mut Context<Self>) -> AnyElement {
        let found = self.reviews.iter().find_map(|(worktree, comments)| {
            comments
                .iter()
                .find(|comment| comment.id == id)
                .map(|comment| (worktree.clone(), comment))
        });
        let Some((worktree, comment)) = found else {
            return div().into_any_element();
        };
        let t = &self.theme;
        let owner = worktree.clone();
        // Another comment with words in it keeps the composer, so Edit has
        // nowhere to open; an empty one gives way on the press.
        let free =
            self.review_draft.is_none() || self.composer_text(REVIEW_EDITOR).trim().is_empty();
        let edit = button(("review-edit", id), "Edit")
            .ghost()
            .small()
            .enabled(free)
            .render(t)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_review_comment(&owner, id);
                cx.notify();
            }));
        let delete = button(("review-delete", id), "Delete")
            .ghost()
            .small()
            .render(t)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.remove_review_comment(&worktree, id);
                cx.notify();
            }));
        review_card(t)
            .id(("review-card", id))
            .child(
                card_heading(&comment.lines, t)
                    .child(div().flex_grow())
                    .child(edit)
                    .child(delete),
            )
            .child(
                div()
                    .px(px(2.0))
                    .text_size(DRAFT_TEXT)
                    .line_height(relative(1.5))
                    .text_color(paint(t.text.primary))
                    .child(comment.comment.clone()),
            )
            .into_any_element()
    }
}

/// `3 comments`, `1 comment`.
fn comments_said(count: usize) -> String {
    match count {
        1 => "1 comment".to_owned(),
        _ => format!("{count} comments"),
    }
}

/// The frame both cards share: the shell's card, set into the diff under
/// the code, in the prose face the code rows around it are not set in.
fn review_card(t: &Theme) -> Div {
    crate::header::card(t)
        .prose()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .w_full()
        .max_w(CARD_W)
        .p(px(10.0))
        .text_color(paint(t.text.primary))
}

/// A card's first line: a comment mark and which lines — `Lines 18–20`.
fn card_heading(lines: &str, t: &Theme) -> Div {
    let mut chars = lines.chars();
    let said = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
        None => "The hunk".to_owned(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .min_h(px(26.0))
        .px(px(2.0))
        .child(sized_icon(Icon::MessageSquare, px(12.0), paint(t.accent)))
        .child(caption(said, t))
}

#[cfg(test)]
mod tests {
    use super::{ReviewComment, review_message};
    use crate::diff_view::DiffKey;

    fn comment(rel: &str, lines: &str, text: &str, said: &str) -> ReviewComment {
        ReviewComment {
            id: 0,
            key: DiffKey {
                root: "/repo".into(),
                rel: rel.to_owned().into(),
                base: None,
            },
            lines: lines.to_owned(),
            new_span: None,
            old_span: None,
            text: text.to_owned(),
            comment: said.to_owned(),
        }
    }

    #[test]
    fn a_review_numbers_each_comment_under_its_fenced_lines() {
        let comments = [
            comment(
                "src/main.rs",
                "lines 12\u{2013}13",
                "-let x = 1;\n+let x = 2;\n",
                "Keep it at one.",
            ),
            comment("README.md", "line 4", "+```sh\n", "  Close this fence.  "),
        ];
        assert_eq!(
            review_message("fix-login", &comments),
            "Review comments on fix-login (uncommitted):\n\n\
             1. src/main.rs, lines 12\u{2013}13:\n```diff\n-let x = 1;\n+let x = 2;\n```\nKeep it at one.\n\n\
             2. README.md, line 4:\n````diff\n+```sh\n````\nClose this fence."
        );
    }

    #[test]
    fn comments_read_against_different_bases_each_say_which() {
        let mut since = comment("a.rs", "line 1", "+a", "One.");
        since.key.base = Some(("main".into(), "abc123".into()));
        let comments = [since, comment("b.rs", "line 2", "+b", "Two.")];
        let message = review_message("feature", &comments);
        assert!(
            message.starts_with("Review comments on feature:\n\n1. a.rs, line 1 (against main):")
        );
        assert!(message.contains("\n\n2. b.rs, line 2 (uncommitted):"));
    }
}
