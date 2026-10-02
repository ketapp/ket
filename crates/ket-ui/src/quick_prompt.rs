//! Global prompt composer and destination picker.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AsyncApp, Context, Div, FocusHandle, KeyDownEvent, MouseButton, Pixels,
    SharedString, Stateful, WeakEntity, Window, div, prelude::*, px, relative,
};
use ket_core::activity::Signal;
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::device_capture::DeviceKind;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::snippets::{SnippetMenu, SnippetPick};
use crate::ui::banner::banner;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::kbd;
use crate::ui::dialog::{card, centered, header};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;
use crate::ui::textarea::textarea;
use crate::ui::toast::Tone;
use crate::ui::tooltip::{Side, tooltip};
use crate::ui::{CAPTION, LABEL, RADIUS_LG};
use crate::voice;

/// Wide, because the prompt is the dialog: everything else on it is a
/// default somebody changes now and then.
const DIALOG_WIDTH: Pixels = px(880.0);

/// The prompt's box, footer row included. Tall enough for a paragraph
/// before it scrolls.
const EDITOR_HEIGHT: Pixels = px(452.0);

/// The mark before an inline trigger's value.
const INLINE_MARK: Pixels = px(14.0);

/// The project and worktree menus. Wider than the inline triggers they hang
/// from, which are only as wide as their values.
const MENU_WIDTH: Pixels = px(240.0);

/// The column of screenshots beside the draft.
const SHOTS_W: Pixels = px(132.0);

/// One screenshot alone: a phone's shape at the column's width.
const SHOT_H: Pixels = px(286.0);

/// Two or more, in pairs.
const SHOT_SMALL_W: Pixels = px(62.0);
const SHOT_SMALL_H: Pixels = px(134.0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuickTarget {
    AnyFree,
    NewWorktree,
    Existing(WorktreeId),
}

pub(crate) struct QuickPrompt {
    project: ProjectId,
    target: QuickTarget,
    project_focus: FocusHandle,
    target_focus: FocusHandle,
    project_menu: Option<OpenMenu<ProjectId>>,
    target_menu: Option<OpenMenu<QuickTarget>>,
    /// The snippet picker, while it is open. See [`crate::snippets`].
    snippet_menu: Option<SnippetMenu>,
    error: Option<SharedString>,
    focus: FocusHandle,
    /// Voice input, while the microphone is being asked for, listened to or
    /// finished with.
    voice: Option<Voice>,
    /// The glyph button under the pointer, whose tooltip is showing.
    hovered: Option<&'static str>,
    /// Screenshots sent with the prompt — see `device_capture`.
    attachments: Vec<PathBuf>,
    /// The device tab they came from, which ⌘S takes another from.
    source: Option<DeviceKind>,
}

/// Dictation into the draft, from the mic button to the last word.
struct Voice {
    /// The microphone and the recogniser. `None` while the permissions are
    /// still being asked for.
    dictation: Option<voice::Dictation>,
    /// What the recogniser has heard, written from its own thread.
    heard: voice::Transcript,
    /// Where in the draft the words go.
    start: usize,
    /// Whether a space goes before them, to keep off the word before.
    spaced: bool,
    /// How many characters dictation has written at `start` so far.
    written: usize,
    /// The transcript as last written, so a poll with nothing new writes
    /// nothing.
    shown: String,
    /// When the microphone was closed, if it has been. The recogniser still
    /// owes a final result then, and this is how long it has been owed.
    stopped: Option<Instant>,
}

impl Voice {
    /// Whether the microphone is open, or about to be.
    fn listening(&self) -> bool {
        self.stopped.is_none()
    }
}

/// How often the dialog reads what the recogniser has heard. Fast enough that
/// words appear as they are said, and far cheaper than a frame.
const VOICE_POLL: Duration = Duration::from_millis(60);

/// How long a closed microphone waits for the recogniser's final result
/// before settling for what it has.
const VOICE_SETTLE: Duration = Duration::from_secs(3);

impl Shell {
    pub(crate) fn open_quick_prompt(&mut self, cx: &mut Context<Self>) {
        let project = self
            .selection
            .and_then(|selection| self.projects.get(selection.project))
            .or_else(|| self.projects.first())
            .map(|project| project.id.clone());
        let Some(project) = project else {
            self.toast(Tone::Info, "Add a project before sending a prompt", cx);
            return;
        };

        self.popup = None;
        self.menu = None;
        self.open_quick_prompt_editor();
        self.quick_prompt = Some(QuickPrompt {
            project,
            target: QuickTarget::AnyFree,
            project_focus: cx.focus_handle(),
            target_focus: cx.focus_handle(),
            project_menu: None,
            target_menu: None,
            snippet_menu: None,
            error: None,
            focus: cx.focus_handle(),
            voice: None,
            hovered: None,
            attachments: Vec::new(),
            source: None,
        });
    }

    /// Whether the Quick prompt is open with screenshots from the `kind` tab.
    pub(crate) fn quick_prompt_has_shots(&self, kind: DeviceKind) -> bool {
        self.quick_prompt
            .as_ref()
            .is_some_and(|dialog| dialog.source == Some(kind) && !dialog.attachments.is_empty())
    }

    /// Opens the Quick prompt with a device's screenshot, sending to the
    /// worktree the device tab is in — or adds the screenshot to the prompt
    /// already open.
    pub(crate) fn open_screenshot_prompt(
        &mut self,
        shot: PathBuf,
        kind: DeviceKind,
        worktree: Option<WorktreeId>,
        cx: &mut Context<Self>,
    ) {
        if self.quick_prompt.is_none() {
            self.open_quick_prompt(cx);
            let home = worktree.and_then(|id| {
                let project = self
                    .projects
                    .iter()
                    .find(|project| project.worktrees.iter().any(|w| w.id == id))?;
                Some((project.id.clone(), id))
            });
            if let (Some(dialog), Some((project, id))) = (self.quick_prompt.as_mut(), home) {
                dialog.project = project;
                dialog.target = QuickTarget::Existing(id);
            }
        }
        if let Some(dialog) = self.quick_prompt.as_mut() {
            dialog.attachments.push(shot);
            dialog.source = Some(kind);
            dialog.error = None;
        }
    }

    fn close_quick_prompt(&mut self) {
        self.quick_prompt = None;
        self.close_quick_prompt_editor();
    }

    fn toggle_quick_project_menu(&mut self) {
        let Some(dialog) = self.quick_prompt.as_mut() else {
            return;
        };
        if dialog.project_menu.take().is_some() {
            return;
        }
        let selected = self
            .projects
            .iter()
            .position(|project| project.id == dialog.project)
            .unwrap_or(0);
        let entries = self
            .projects
            .iter()
            .map(|project| {
                let item = MenuItem::new(project.id.clone(), project.name.clone());
                MenuEntry::Item(if project.id == dialog.project {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        dialog.project_menu = Some(
            OpenMenu::new("quick-project".into(), None, MENU_WIDTH, entries).selected(selected),
        );
        dialog.target_menu = None;
        dialog.snippet_menu = None;
    }

    fn toggle_quick_target_menu(&mut self) {
        let Some(dialog) = self.quick_prompt.as_mut() else {
            return;
        };
        if dialog.target_menu.take().is_some() {
            return;
        }
        let mut entries = vec![
            MenuEntry::Item(MenuItem::new(QuickTarget::AnyFree, "Any free")),
            MenuEntry::Item(MenuItem::new(QuickTarget::NewWorktree, "New worktree")),
            MenuEntry::Separator,
        ];
        if let Some(project) = self.projects.iter().find(|p| p.id == dialog.project) {
            entries.extend(project.worktrees.iter().map(|worktree| {
                let label: SharedString = if worktree.primary {
                    format!("Main · {}", worktree.label()).into()
                } else {
                    worktree.label()
                };
                let target = QuickTarget::Existing(worktree.id.clone());
                let item = MenuItem::new(target.clone(), label);
                MenuEntry::Item(if target == dialog.target {
                    item.checked()
                } else {
                    item
                })
            }));
        }
        let selected = match &dialog.target {
            QuickTarget::AnyFree => 0,
            QuickTarget::NewWorktree => 1,
            QuickTarget::Existing(id) => self
                .projects
                .iter()
                .find(|p| p.id == dialog.project)
                .and_then(|p| p.worktrees.iter().position(|w| &w.id == id))
                .map_or(0, |index| index + 2),
        };
        dialog.target_menu = Some(
            OpenMenu::new("quick-target".into(), None, MENU_WIDTH, entries).selected(selected),
        );
        dialog.project_menu = None;
        dialog.snippet_menu = None;
    }

    /// Opens the snippet picker, or closes it. Read from disk each time —
    /// see [`crate::snippets`].
    fn toggle_quick_snippet_menu(&mut self, cx: &mut Context<Self>) {
        if self
            .quick_prompt
            .as_mut()
            .is_none_or(|dialog| dialog.snippet_menu.take().is_some())
        {
            return;
        }
        let snippets = self.load_snippets(cx);
        // Settings replace this dialog, and a draft closed under someone is
        // a draft lost: the way there waits until there is nothing to lose.
        let blocked = (!self.quick_prompt_text().trim().is_empty())
            .then_some("Send or clear the prompt first");
        if let Some(dialog) = self.quick_prompt.as_mut() {
            dialog.snippet_menu = Some(SnippetMenu::new("quick-snippets", snippets, true, blocked));
            dialog.project_menu = None;
            dialog.target_menu = None;
        }
    }

    /// Runs a pick from the snippet picker: the snippet goes in at the caret,
    /// over any selection. The draft keeps the keyboard throughout — the
    /// picker never took it — so what was inserted can be finished at once.
    fn run_quick_snippet(&mut self, pick: SnippetPick, cx: &mut Context<Self>) {
        let Some(open) = self
            .quick_prompt
            .as_mut()
            .and_then(|dialog| dialog.snippet_menu.take())
        else {
            return;
        };
        if pick == SnippetPick::Manage {
            self.close_quick_prompt();
            self.manage_snippets(cx);
            return;
        }
        if let Some(snippet) = open.picked(pick) {
            self.composer_insert(crate::editor::QUICK_PROMPT_EDITOR, &snippet.body);
        }
    }

    fn quick_snippet_menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let action = self.quick_prompt.as_mut().and_then(|dialog| {
            dialog
                .snippet_menu
                .as_mut()
                .map(|open| open.menu.key(event))
        });
        let Some(action) = action else {
            return false;
        };
        match action {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(dialog) = self.quick_prompt.as_mut() {
                    dialog.snippet_menu = None;
                }
                true
            }
            MenuKey::Run(pick) => {
                self.run_quick_snippet(pick, cx);
                true
            }
        }
    }

    fn quick_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let action = self
            .quick_prompt
            .as_mut()
            .and_then(|dialog| dialog.project_menu.as_mut().map(|menu| menu.key(event)));
        let Some(action) = action else {
            return false;
        };
        match action {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(dialog) = self.quick_prompt.as_mut() {
                    dialog.project_menu = None;
                }
                true
            }
            MenuKey::Run(project) => {
                if let Some(dialog) = self.quick_prompt.as_mut() {
                    dialog.project = project;
                    dialog.target = QuickTarget::AnyFree;
                    dialog.project_menu = None;
                    dialog.error = None;
                }
                true
            }
        }
    }

    fn quick_target_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let action = self
            .quick_prompt
            .as_mut()
            .and_then(|dialog| dialog.target_menu.as_mut().map(|menu| menu.key(event)));
        let Some(action) = action else {
            return false;
        };
        match action {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(dialog) = self.quick_prompt.as_mut() {
                    dialog.target_menu = None;
                }
                true
            }
            MenuKey::Run(target) => {
                if let Some(dialog) = self.quick_prompt.as_mut() {
                    dialog.target = target;
                    dialog.target_menu = None;
                    dialog.error = None;
                }
                true
            }
        }
    }

    pub(crate) fn quick_prompt_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.quick_prompt.is_none() {
            return false;
        }
        if self.quick_snippet_menu_key(event, cx)
            || self.quick_target_menu_key(event)
            || self.quick_menu_key(event)
        {
            return true;
        }
        let key = &event.keystroke;
        if key.key == "/" && (key.modifiers.platform || key.modifiers.control) {
            self.toggle_quick_snippet_menu(cx);
            return true;
        }
        let another = self.quick_prompt.as_ref().and_then(|dialog| dialog.source);
        if let Some(kind) = another
            && key.key == "s"
            && key.modifiers.platform
            && !key.modifiers.shift
        {
            self.capture_device(kind, cx);
            return true;
        }
        let listening = self
            .quick_prompt
            .as_ref()
            .and_then(|dialog| dialog.voice.as_ref())
            .is_some_and(Voice::listening);
        if listening && key.key == "escape" {
            // The first Escape stops listening and keeps what was heard; the
            // dialog is the second one's to close.
            self.stop_quick_voice();
            return true;
        }
        // Anything else typed ends dictation where it stands, so the words
        // still arriving cannot land on top of what is being typed.
        if let Some(dialog) = self.quick_prompt.as_mut() {
            dialog.voice = None;
        }
        if key.key == "escape" {
            self.close_quick_prompt();
        } else if key.key == "enter" && (key.modifiers.platform || key.modifiers.control) {
            self.send_quick_prompt(cx);
        } else {
            self.quick_prompt_editor_key(key, cx);
        }
        true
    }

    /// The mic button: starts listening, or stops.
    fn toggle_quick_voice(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.quick_prompt.as_ref() else {
            return;
        };
        match &dialog.voice {
            Some(voice) if voice.listening() => self.stop_quick_voice(),
            // Still settling from the last time: that one's words stay, and
            // the next begins after them.
            _ => self.start_quick_voice(cx),
        }
    }

    fn start_quick_voice(&mut self, cx: &mut Context<Self>) {
        let Some((start, spaced)) = self.quick_prompt_dictation_start() else {
            return;
        };
        let Some(dialog) = self.quick_prompt.as_mut() else {
            return;
        };
        let heard = voice::Transcript::default();
        dialog.error = None;
        dialog.voice = Some(Voice {
            dictation: None,
            heard: heard.clone(),
            start,
            spaced,
            written: 0,
            shown: String::new(),
            stopped: None,
        });

        cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let access = voice::authorize().await;
            let listening = shell
                .update(cx, |shell, cx| {
                    let started = shell.begin_quick_voice(&heard, access);
                    cx.notify();
                    started
                })
                .unwrap_or(false);
            if !listening {
                return;
            }
            loop {
                cx.background_executor().timer(VOICE_POLL).await;
                let more = shell
                    .update(cx, |shell, cx| shell.poll_quick_voice(&heard, cx))
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// Opens the microphone once the permissions are in, if the dictation
    /// that asked for them is still the one the dialog is waiting on.
    fn begin_quick_voice(&mut self, heard: &voice::Transcript, access: Result<(), String>) -> bool {
        let Some(dialog) = self.quick_prompt.as_mut() else {
            return false;
        };
        let Some(voice) = dialog
            .voice
            .as_mut()
            .filter(|voice| Arc::ptr_eq(&voice.heard, heard) && voice.listening())
        else {
            return false;
        };
        match access.and_then(|()| voice::Dictation::start(heard.clone())) {
            Ok(dictation) => {
                voice.dictation = Some(dictation);
                true
            }
            Err(why) => {
                dialog.voice = None;
                dialog.error = Some(why.into());
                false
            }
        }
    }

    /// Closes the microphone. The words already said still arrive.
    fn stop_quick_voice(&mut self) {
        let Some(voice) = self
            .quick_prompt
            .as_mut()
            .and_then(|dialog| dialog.voice.as_mut())
        else {
            return;
        };
        match voice.dictation.as_mut() {
            Some(dictation) => {
                dictation.stop();
                voice.stopped = Some(Instant::now());
            }
            // Stopped while the permissions were still being asked for:
            // there is nothing to wait for.
            None => {
                if let Some(dialog) = self.quick_prompt.as_mut() {
                    dialog.voice = None;
                }
            }
        }
    }

    /// Writes whatever the recogniser has heard since the last poll into the
    /// draft. Returns whether to keep polling.
    fn poll_quick_voice(&mut self, heard: &voice::Transcript, cx: &mut Context<Self>) -> bool {
        let Some(voice) = self
            .quick_prompt
            .as_ref()
            .and_then(|dialog| dialog.voice.as_ref())
            .filter(|voice| Arc::ptr_eq(&voice.heard, heard))
        else {
            return false;
        };
        let (text, done, error) = match heard.lock() {
            Ok(heard) => (heard.text.clone(), heard.done, heard.error.clone()),
            Err(_) => (String::new(), true, None),
        };
        let settled = voice
            .stopped
            .is_some_and(|stopped| stopped.elapsed() >= VOICE_SETTLE);
        let (start, written, fresh) = (voice.start, voice.written, text != voice.shown);
        let words = if voice.spaced && !text.is_empty() {
            format!(" {text}")
        } else {
            text.clone()
        };

        if fresh {
            let written = self.quick_prompt_dictate(start, written, &words);
            if let Some(voice) = self
                .quick_prompt
                .as_mut()
                .and_then(|dialog| dialog.voice.as_mut())
            {
                voice.written = written;
                voice.shown = text;
            }
            cx.notify();
        }
        if done || settled {
            if let Some(dialog) = self.quick_prompt.as_mut() {
                dialog.voice = None;
                if let Some(why) = error {
                    dialog.error = Some(why.into());
                }
            }
            cx.notify();
            return false;
        }
        true
    }

    fn send_quick_prompt(&mut self, cx: &mut Context<Self>) {
        let prompt = self.quick_prompt_text();
        if prompt.trim().is_empty() {
            if let Some(dialog) = self.quick_prompt.as_mut() {
                dialog.error = Some("Enter a prompt".into());
            }
            return;
        }
        let Some(dialog) = self.quick_prompt.as_ref() else {
            return;
        };
        let attachments = dialog.attachments.clone();
        if dialog.target == QuickTarget::NewWorktree {
            // A worktree named from the prompt, made and landed in as the
            // Create Worktree dialog would, with the project's own agent
            // started on it — the way a phone or a backlog note starts work.
            let project = dialog.project.clone();
            let branch = crate::phone_work::branch_for(&prompt, "quick-prompt");
            self.close_quick_prompt();
            self.toast(Tone::Info, format!("Starting {branch}…"), cx);
            self.start_work(
                crate::phone_work::NewWork {
                    project,
                    branch,
                    prompt,
                    attachments,
                    agent: None,
                    base: None,
                    economy: None,
                    origin: crate::phone_work::WorkOrigin::QuickPrompt,
                },
                cx,
            );
            return;
        }
        let Some(project) = self.projects.iter().find(|p| p.id == dialog.project) else {
            return;
        };
        let worktree = match &dialog.target {
            QuickTarget::Existing(id) => project
                .worktrees
                .iter()
                .find(|worktree| &worktree.id == id && !worktree.missing),
            QuickTarget::AnyFree => project.worktrees.iter().find(|worktree| {
                !worktree.missing
                    && worktree.in_progress.is_none()
                    && self.activity.signal(&worktree.id, ket_core::now_ms()) == Signal::Quiet
            }),
            QuickTarget::NewWorktree => None,
        };
        let Some(worktree) = worktree else {
            if let Some(dialog) = self.quick_prompt.as_mut() {
                dialog.error = Some("No idle worktree is available in this project".into());
            }
            return;
        };
        let id = worktree.id.clone();
        let label = worktree.label();
        self.close_quick_prompt();
        self.toast(Tone::Info, format!("Prompt sent to {label}"), cx);
        self.send_with_attachments(id, prompt, attachments, cx);
    }

    /// Hands `prompt` to `id`'s agent.
    ///
    /// Typed into the agent's terminal when one is open, where it can be
    /// watched and answered, rather than to a second session nobody can see.
    /// Otherwise run as a session of its own, provisioning the worktree first
    /// if it needs it.
    pub(crate) fn send_prompt_to_worktree(
        &mut self,
        id: WorktreeId,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.agent_terminal(&id) {
            self.submit_to_terminal(terminal, &prompt, cx);
            return;
        }

        let work = cx.background_executor().spawn(async move {
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
                let workspace = Workspace::open().map_err(|e| e.to_string())?;
                let needs_provisioning = workspace
                    .worktrees(None)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|worktree| worktree.id == id)
                    .is_some_and(|worktree| !worktree.is_provisioned());
                if needs_provisioning {
                    workspace
                        .provision_worktree(&id)
                        .map_err(|e| e.to_string())?;
                }
                runtime
                    .block_on(workspace.run_agent(&id, None, &prompt))
                    .map_err(|e| e.to_string())
            })
            .join()
            .map_err(|_| "agent runner panicked".to_owned())?
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = work.await;
            if let Err(why) = result {
                let _ = shell.update(cx, |shell, cx| {
                    shell.toast(Tone::Error, format!("Could not send prompt: {why}"), cx);
                });
            }
        })
        .detach();
    }

    /// The screenshots going with the prompt, as a column beside the draft:
    /// one at the column's width, two or more in pairs. Each can be taken
    /// off again.
    fn quick_prompt_shots(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.quick_prompt.as_ref()?;
        if dialog.attachments.is_empty() {
            return None;
        }
        let t = &self.theme;
        let alone = dialog.attachments.len() == 1;
        let (width, height) = match alone {
            true => (SHOTS_W, SHOT_H),
            false => (SHOT_SMALL_W, SHOT_SMALL_H),
        };
        let thumbs = dialog.attachments.iter().enumerate().map(|(index, path)| {
            div()
                .relative()
                .flex_none()
                .w(width)
                .h(height)
                .overflow_hidden()
                .rounded(RADIUS_LG)
                .border_1()
                .border_color(paint(t.border))
                .bg(paint(t.backdrop))
                .child(
                    gpui::img(path.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .child(
                    div().absolute().top(px(4.0)).right(px(4.0)).child(
                        icon_button(("quick-shot-remove", index), Icon::Close)
                            .small()
                            .render(t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(dialog) = this.quick_prompt.as_mut()
                                    && index < dialog.attachments.len()
                                {
                                    dialog.attachments.remove(index);
                                }
                                cx.stop_propagation();
                                cx.notify();
                            })),
                    ),
                )
        });
        Some(
            div()
                .id("quick-prompt-shots")
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .max_h(EDITOR_HEIGHT - px(70.0))
                .overflow_y_scroll()
                .children(thumbs)
                .into_any_element(),
        )
    }

    pub(crate) fn quick_prompt_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.quick_prompt.as_ref()?;
        let editor = self.quick_prompt_editor_view(window, cx);
        let empty = self.quick_prompt_text().trim().is_empty();
        let dialog = self.quick_prompt.as_ref()?;
        let t = &self.theme;
        let project_name = self
            .projects
            .iter()
            .find(|project| project.id == dialog.project)
            .map(|project| project.name.clone())
            .unwrap_or_default();
        let target_name: SharedString = match &dialog.target {
            QuickTarget::AnyFree => "Any free".into(),
            QuickTarget::NewWorktree => "New worktree".into(),
            QuickTarget::Existing(id) => self
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|worktree| &worktree.id == id)
                .map(|worktree| worktree.label())
                .unwrap_or_default(),
        };

        let project_menu = dialog.project_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, project, cx| {
                    if let Some(dialog) = shell.quick_prompt.as_mut() {
                        dialog.project = project.clone();
                        dialog.target = QuickTarget::AnyFree;
                        dialog.project_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.quick_prompt.as_mut() {
                        dialog.project_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let project_row = dropdown(
            "quick-project-select",
            select("quick-project", project_name)
                .placeholder("Project")
                .leading(sized_icon(Icon::Folder, INLINE_MARK, paint(t.text.dim)))
                .inline()
                .open(dialog.project_menu.is_some())
                .focused(dialog.project_focus.is_focused(window))
                .render(t),
            project_menu,
        )
        .track_focus(&dialog.project_focus)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                if let Some(dialog) = this.quick_prompt.as_ref() {
                    window.focus(&dialog.project_focus);
                }
                this.toggle_quick_project_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );

        let target_menu = dialog.target_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, target, cx| {
                    if let Some(dialog) = shell.quick_prompt.as_mut() {
                        dialog.target = target.clone();
                        dialog.target_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.quick_prompt.as_mut() {
                        dialog.target_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let snippet_menu = dialog.snippet_menu.as_ref().map(|open| {
            open.menu.view(
                t,
                cx,
                |shell, pick, cx| {
                    shell.run_quick_snippet(*pick, cx);
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.quick_prompt.as_mut() {
                        dialog.snippet_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        // The glyph buttons say what they are on hover.
        let hovered = dialog.hovered;
        let tip = |id: &'static str, trigger: Stateful<Div>, label: &'static str, side: Side| {
            tooltip(
                id,
                trigger.into_any_element(),
                label,
                side,
                hovered == Some(id),
                t,
            )
            .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
                if let Some(dialog) = this.quick_prompt.as_mut() {
                    if *is_hovered {
                        dialog.hovered = Some(id);
                    } else if dialog.hovered == Some(id) {
                        dialog.hovered = None;
                    }
                }
                cx.notify();
            }))
        };

        let snippet_row = dropdown(
            "quick-snippets-select",
            tip(
                "quick-snippets",
                icon_button("quick-snippets", Icon::Layers)
                    .bare()
                    .small()
                    .pressed(dialog.snippet_menu.is_some())
                    .render(t),
                "Snippets (⌘/)",
                Side::Top,
            ),
            snippet_menu,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_quick_snippet_menu(cx);
                cx.stop_propagation();
                cx.notify();
            }),
        );

        let target_row = dropdown(
            "quick-target-select",
            select("quick-target", target_name)
                .placeholder("Worktree")
                .leading(sized_icon(Icon::GitBranch, INLINE_MARK, paint(t.text.dim)))
                .mono(self.chrome_family.clone())
                .inline()
                .open(dialog.target_menu.is_some())
                .focused(dialog.target_focus.is_focused(window))
                .render(t),
            target_menu,
        )
        .track_focus(&dialog.target_focus)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                if let Some(dialog) = this.quick_prompt.as_ref() {
                    window.focus(&dialog.target_focus);
                }
                this.toggle_quick_target_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );

        // The prompt has the keyboard unless a picker has taken it, and the
        // box says so the way a focused field does.
        let writing =
            !dialog.project_focus.is_focused(window) && !dialog.target_focus.is_focused(window);
        let listening = dialog.voice.as_ref().is_some_and(Voice::listening);
        let asking = dialog
            .voice
            .as_ref()
            .is_some_and(|voice| voice.dictation.is_none());

        let mic = tip(
            "quick-voice",
            icon_button("quick-voice", Icon::Mic)
                .bare()
                .small()
                .pressed(listening)
                .tint(paint(t.accent))
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_quick_voice(cx);
                    cx.notify();
                })),
            "Dictate",
            Side::Top,
        );

        let close = tip(
            "quick-close",
            icon_button("quick-close", Icon::Close)
                .bare()
                .small()
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_quick_prompt();
                    cx.notify();
                })),
            "Close (esc)",
            Side::Bottom,
        );
        let send = button("quick-send", "Send")
            .primary()
            .small()
            .hint("⌘↵")
            .enabled(!empty)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.send_quick_prompt(cx);
                cx.notify();
            }));
        let divider = || div().flex_none().w(px(1.0)).h(px(16.0)).bg(paint(t.border));
        let word = |text: &'static str| {
            div()
                .flex_none()
                .text_size(LABEL)
                .text_color(paint(t.text.dim))
                .child(text)
        };

        // Everything that shapes the send lives in the well with it, read as
        // one sentence: send to this project, in this worktree — then the
        // snippets and the mic that fill the draft, and the send itself.
        let footer = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.0))
            .child(word("Send to"))
            .child(project_row)
            .child(word("in"))
            .child(target_row)
            .child(div().mx(px(4.0)).child(divider()))
            .child(snippet_row)
            .child(mic)
            .child(div().flex_grow())
            .child(send);
        let shots = self.quick_prompt_shots(cx);
        let prompt = {
            let area = textarea("quick-prompt-draft", editor, EDITOR_HEIGHT)
                .active(writing || listening)
                .footer(footer);
            let area = match shots {
                Some(column) => area.aside(column, SHOTS_W),
                None => area,
            };
            // Drawn over the empty draft rather than in it, so the caret
            // stays at the start of the line with the words after it.
            match empty && !listening {
                true => area.placeholder("What should the agent do?", crate::editor::COMPOSER_TEXT),
                false => area,
            }
            .render(t)
        };
        // The keys the draft answers to, as key caps under the well. Send's
        // chord rides on its button, so it is not repeated here.
        let key = |keys: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(kbd(keys, self.chrome_family.clone(), t))
                .child(what)
        };
        let hints = div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .px(px(2.0))
            .prose()
            .text_size(CAPTION)
            .text_color(paint(t.text.dim));
        let hints = if listening {
            hints
                .child(div().text_color(paint(t.accent)).child(if asking {
                    "Asking for the microphone…"
                } else {
                    "Listening…"
                }))
                .child(key("esc", "stop"))
        } else {
            hints
                .child(key("↵", "new line"))
                .children(
                    dialog
                        .source
                        .is_some()
                        .then(|| key("⌘S", "another screenshot")),
                )
                .child(key("⌘/", "snippets"))
                .child(key("esc", "close"))
        };

        Some(
            centered(
                card("quick-prompt", DIALOG_WIDTH, t)
                    .max_w(relative(0.92))
                    .track_focus(&dialog.focus)
                    .child(
                        header(
                            if dialog.attachments.is_empty() {
                                "Quick prompt"
                            } else {
                                "Screenshot to agent"
                            },
                            t,
                        )
                        .child(close),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(prompt)
                            .children(
                                dialog
                                    .error
                                    .clone()
                                    .map(|why| banner("quick-error", Tone::Error, why, t)),
                            )
                            .child(hints),
                    ),
            )
            .into_any_element(),
        )
    }
}
