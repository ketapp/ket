//! The terminal pane.
//!
//! `ket_core::terminal::Terminal` owns the pty, the parser and the grid.
//! This module is the shell's side of it: opening a terminal per worktree,
//! routing keys, mouse and text input to the right one, and holding the
//! per-terminal state that lives in the UI rather than the emulator —
//! where the grid was last painted, whether a drag is selecting, what an
//! input method is composing. Painting itself is [`element::TerminalElement`],
//! a real `gpui` element; see its docs for the paint model.
//!
//! The backend already coalesces damage onto a 16ms tick and publishes a
//! frame counter over a `tokio::sync::watch` channel (see `FrameClock` in
//! `ket-core`'s `terminal.rs`); this pane subscribes to that instead of
//! polling, so an idle terminal costs nothing and a busy one costs at most
//! 60 repaints a second regardless of how much the program inside it writes.
//!
//! **Where input comes from.** Two paths, deliberately:
//!
//! - Keys with a meaning of their own — control chords, `Alt` as meta, the
//!   arrows, function and editing keys, `Cmd-C`/`Cmd-V` — arrive as
//!   `KeyDownEvent`s through the window's key handler and are encoded by
//!   [`keys::keystroke_to_bytes`]. Each one stops propagation so macOS does
//!   not *also* interpret it as text.
//! - Plain text arrives through the window's text-input path, as
//!   [`gpui::EntityInputHandler`] calls on the shell, which is what lets
//!   dead keys, `Option`-accents and CJK composition work: the input method
//!   marks text while composing and commits it when done, and the element
//!   draws the marked text at the cursor in the meantime.
//!
//! **Mouse.** A program that asked for mouse events (`htop`, `lazygit`,
//! anything built on a TUI library) gets them, encoded by [`mouse`]; holding
//! `Shift` bypasses that and selects text as usual. Otherwise a drag
//! selects, a double-click selects a word, a triple-click a line, and
//! `Shift`-click extends. Links are underlined wherever they appear —
//! both OSC 8 hyperlinks and plain `http(s)://` text — the pointer turns
//! to a hand over one, and a click offers to open it in a ket browser tab,
//! open it in the system's browser, or copy its URL
//! (`Shell::open_link_menu`); a drag that starts on one selects instead of
//! opening. A program that asked for the mouse owns a plain click there,
//! so `Cmd`-click reaches a link inside one. Selection state lives in
//! `alacritty_terminal`'s own `Term::selection`, which is what makes copy
//! a one-liner and keeps the selection anchored through
//! scrolling and reflow.
//!
//! **What is deliberately not here.** No kitty keyboard protocol, so a
//! modified key that collides with an unmodified one cannot be told apart
//! on the other end. No right-click paste. No copy-on-select.
//! Dim text is approximated by blending toward the background rather than
//! drawn with a fainter weight.

mod colors;
mod element;
mod find;
mod keys;
mod mouse;

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Bounds, ClipboardItem, Context, Entity, EntityInputHandler,
    ExternalPaths, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, ScrollWheelEvent, SharedString, Task, TouchPhase, UTF16Selection,
    WeakEntity, Window, div, prelude::*, px,
};

use ket_core::activity::{Foreground, Signal};
use ket_core::agent_status::{CancelSource, PaneAgentStatus};
use ket_core::agents::{Catalogue, Resolution, ShellProber};
use ket_core::config::{AgentConfig, AgentSpec};
use ket_core::id::WorktreeId;
use ket_core::sessions::Sessions;
use ket_core::terminal::alacritty_terminal::Term;
use ket_core::terminal::alacritty_terminal::event::EventListener;
use ket_core::terminal::alacritty_terminal::grid::{Dimensions, Scroll};
use ket_core::terminal::alacritty_terminal::index::{Column, Point as GridPoint};
use ket_core::terminal::alacritty_terminal::selection::{Selection, SelectionType};
use ket_core::terminal::alacritty_terminal::term::TermMode;

/// Launch choices for one new agent tab, never saved on the worktree.
#[derive(Clone, Default)]
pub(crate) struct SessionLaunch {
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
}
use ket_core::terminal::{StopOutcome, Terminal, TerminalSpec};
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::paint::paint;
use crate::tabs::{PaneId, Tab, TabKind};
use crate::ui::toast::Tone;

use self::element::{FrameState, GridCache, GridFont, Owners, TerminalElement, mode_of};
use self::find::{FindPaint, TerminalFind};
use self::keys::{is_interrupt, keystroke_to_bytes};
use self::mouse::{
    Cell, Geometry, alternate_scroll, button_report, motion_report, wants_mouse, wheel_reports,
};

/// How long each phase of a blinking cursor lasts.
const BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// How long a program must survive before its exit counts as *you quitting it*
/// rather than *it never starting*.
///
/// The two look identical from here — the frame clock stops either way — and
/// they want opposite handling. A shell you `exit` should take its tab with it;
/// a program that fails in its first breath should leave the tab exactly where
/// it is, because whatever it printed on the way out is the only explanation
/// anyone is going to get.
const STARTUP_GRACE: Duration = Duration::from_secs(3);

/// Only automatic sleep protects the selected tab in the focused window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SleepTrigger {
    Manual,
    Automatic,
}

/// The exact session an idle agent can keep through sleep. Manual requests
/// may sleep the selected tab, but still require the agent to be quiet.
fn sleep_session<'a>(
    trigger: SleepTrigger,
    focused: bool,
    agent: &str,
    pane: Option<&'a PaneAgentStatus>,
    signal: Option<Signal>,
) -> Result<&'a str, String> {
    if trigger == SleepTrigger::Automatic && focused {
        return Err("the selected tab is in the focused window".to_owned());
    }
    let pane = pane.ok_or("agent has not reported its state")?;
    if pane.agent != agent
        || pane.state != ket_core::event::AgentState::Idle
        || signal != Some(Signal::Quiet)
    {
        return Err("agent is still working or waiting".to_owned());
    }
    pane.session
        .as_deref()
        .ok_or_else(|| "agent has not reported a session ID".to_owned())
}

/// The CLI shipped beside the app, or beside a development `ket-ui` binary.
fn cli_path_for(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if bin.file_name().is_some_and(|name| name == "MacOS")
        && bin
            .parent()?
            .file_name()
            .is_some_and(|name| name == "Contents")
    {
        Some(bin.parent()?.join("Resources/bin/ket"))
    } else {
        Some(bin.join("ket"))
    }
}

/// Put ket's CLI on the PATH inherited by shells and the agents they start,
/// and tell those agents it is there — see `ket_core::agent_hooks::CLI_NOTE`.
fn expose_cli(spec: &mut TerminalSpec) {
    let Some(cli) = std::env::current_exe()
        .ok()
        .and_then(|exe| cli_path_for(&exe))
        .filter(|path| path.is_file())
    else {
        return;
    };
    let Some(dir) = cli.parent() else {
        return;
    };
    let inherited = spec
        .env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    let mut entries = vec![dir.to_path_buf()];
    if let Some(inherited) = inherited {
        entries.extend(std::env::split_paths(&inherited));
    }
    if let Ok(path) = std::env::join_paths(entries)
        && let Some(path) = path.to_str()
    {
        spec.env.insert("PATH".to_owned(), path.to_owned());
        spec.env.insert(
            ket_core::agent_hooks::CLI_NOTE_ENV.to_owned(),
            ket_core::agent_hooks::CLI_NOTE.to_owned(),
        );
    }
}

/// A running terminal, plus what the pane needs beyond the backend itself.
///
/// Owns the backend behind an `Arc` so the frame-watcher task and a frame's
/// element can each hold their own cheap handle to it without racing the
/// map this lives in. Dropping the handle (closing the last reference)
/// drops the `Terminal`, which kills the pty and joins its parser thread —
/// see `Terminal`'s own `Drop` impl.
/// Identity for one terminal.
///
/// A terminal used to belong to a *worktree*, one apiece, which is why a
/// second one could never be opened and why a dead one was never replaced: the
/// map already had an entry under that key and `ensure_terminal` returned
/// early. Belonging to a tab instead means as many as you like, each closable,
/// and a shell that exits takes only its own tab with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TerminalId(pub(crate) u64);

/// How long [`Shell::submit_to_terminal`] waits between the paste and Enter.
const SUBMIT_DELAY: Duration = ket_core::attachments::SUBMIT_DELAY;

/// The closest two repaints of one pane may come while its program is writing
/// continuously.
///
/// The backend publishes at up to sixty frames a second, and a pane has no
/// partial redraw: every one of those frames rebuilds the whole window and
/// re-submits every glyph in every visible pane. An agent's status line
/// rewrites itself several times a second for as long as it works, so this is
/// a direct multiplier on what a working agent costs. Fifteen still reads as
/// smooth for streaming text — the spinner and the status line are the only
/// things moving — at a quarter of the backend's rate.
///
/// Only a *run* of frames is held back. The first frame after a quiet spell
/// goes out at once, so a keystroke's echo is never the one kept waiting.
const REPAINT_GAP: Duration = Duration::from_millis(66);

/// `text` as a paste: bracketed when the program asked for that, so a
/// multi-line paste is not executed line by line, and with newlines as
/// carriage returns, which is what the Enter key sends.
fn paste_bytes(term: &Terminal, text: &str) -> Vec<u8> {
    ket_core::attachments::paste(text, mode_of(term).contains(TermMode::BRACKETED_PASTE))
}

/// One terminal pane, as a view of its own.
///
/// **Everything `render` and `prepaint` touch lives here, and nothing here is
/// read off `Shell`.** That is the whole point of the type rather than an
/// incidental tidiness: gpui records which entities a view read while it was
/// being built (`App::detect_accessed_entities`) and invalidates the view when
/// any of them is notified. A pane that read one field off `Shell` would be
/// re-rendered every time anything anywhere in the window changed, and the
/// `.cached()` at its embed site would never once hit.
///
/// The state that only event handlers touch — `scroll_px`, `selecting`,
/// `pending_link` — deliberately stays on [`TerminalHandle`]. Handlers run
/// outside the render pass, so reading `Shell` from one costs nothing.
pub(crate) struct TerminalView {
    /// Which terminal this is, in the shell's map.
    pub(crate) id: TerminalId,
    /// The pty, the parser and the grid.
    pub(crate) term: Arc<Terminal>,
    /// The window's keyboard focus, which the text-input handler follows.
    pub(crate) focus: FocusHandle,
    /// The typeface and point size the grid draws with. Pushed by the shell
    /// when the settings change rather than read from it — see the type docs.
    pub(crate) family: SharedString,
    /// See `family`.
    pub(crate) font_size: Pixels,
    /// Resolved colours, pushed the same way and for the same reason.
    pub(crate) theme: Theme,
    /// Whether this pane is the keyboard target. Pushed when the active tab
    /// changes.
    pub(crate) focused: bool,
    /// Where the grid was last painted, for mapping the pointer to cells.
    /// `None` until the first frame.
    pub(crate) geometry: Option<Geometry>,
    /// Where the cursor was last painted, for placing an input method's
    /// candidate window.
    pub(crate) cursor_bounds: Option<Bounds<Pixels>>,
    /// Text an input method is composing and has not committed.
    pub(crate) marked: Option<String>,
    /// The link under a `Cmd`-hover, if any.
    pub(crate) hovered_link: Option<Link>,
    /// The blink phase and the timer driving it.
    pub(crate) blink: Blink,
    /// The last frame's grid work, reused whenever nothing that feeds it has
    /// changed — see [`element::GridCache`]. Held here rather than in the
    /// element because the element is rebuilt from scratch every frame, which
    /// is the very thing this exists to stop paying for.
    pub(crate) grid: Option<GridCache>,
    /// Why the program stopped, when it stopped badly enough to say so.
    pub(crate) exit_note: Option<SharedString>,
    /// The find query and its current match, while the find strip is open.
    pub(crate) find: Option<FindPaint>,
    /// For the handful of things a pane has to tell the window: a dropped
    /// path, a mouse event that moves the selection. Weak, because the shell
    /// owns the pane and not the other way round.
    pub(crate) shell: WeakEntity<Shell>,
}

pub(crate) struct TerminalHandle {
    /// The pty, the parser and the grid.
    pub(crate) term: Arc<Terminal>,
    /// The view that draws it, and holds everything drawing needs.
    pub(crate) view: Entity<TerminalView>,
    /// The agent it was started with, if any — `None` for a blank shell.
    ///
    /// Recorded at spawn time rather than parsed back out of `spec.command`:
    /// closing a tab needs to know whether it is about to kill a session
    /// worth asking about, and a name is cheaper to keep than to reconstruct.
    pub(crate) agent: Option<String>,
    /// The project's own command it was launched with, by first word, when
    /// its project has one for this agent — what tints its tab and its pane.
    /// `None` for a session launched the way Settings says.
    pub(crate) launched_with: Option<SharedString>,
    /// Wakes a repaint on every coalesced frame. Held only to be dropped:
    /// dropping the task cancels the watch loop, which is how closing a
    /// terminal stops its polling rather than leaving it running forever.
    _watch: Task<()>,
    /// Wheel movement not yet worth a whole line, so a trackpad's small
    /// deltas add up instead of being rounded away.
    scroll_px: Pixels,
    /// A link the mouse went down on with `Cmd` held; opened if it comes
    /// back up on the same one.
    pending_link: Option<Link>,
    /// When input was last sent, read by the repaint loop: frames arriving
    /// soon enough to contain its echo skip `REPAINT_GAP`.
    typed_at_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Whether the left button is down and extending a selection.
    selecting: bool,
    /// The find strip, while it is open. See [`find`].
    find: Option<TerminalFind>,
}

/// A tab whose agent process has stopped, but whose exact session can wake.
pub(crate) struct AsleepTerminal {
    pub(crate) agent: String,
    pub(crate) session: String,
    pub(crate) since_ms: u64,
    pub(crate) freed_bytes: u64,
    wake_requested: bool,
    /// The host's request number, when a phone asked for this wake and is
    /// waiting to hear why it failed — see `Shell::wake_failed`.
    wake_req: Option<u64>,
}

/// The cursor's blink, when the program asked for one.
#[derive(Default)]
pub(crate) struct Blink {
    /// Whether the cursor is in its visible phase.
    visible: bool,
    /// The timer flipping the phase. `None` while the cursor is steady.
    task: Option<Task<()>>,
}

/// A link on screen: what it opens and which cells it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    /// The target, as the program gave it or as it appeared in the text.
    uri: String,
    /// The screen row it is on.
    row: usize,
    /// The columns it covers on that row.
    cols: Range<usize>,
}

/// ket's permission rules, told to OpenCode in its inline config: ket has
/// no hook into OpenCode, so OpenCode enforces them itself — see
/// `RuleSet::opencode_permissions`. Read with the project and worktree the
/// hook environment has named, and added after each of `agents`' own rules
/// as well as the config's. A rule file that cannot be read adds nothing,
/// as it rules nothing for the hook.
fn opencode_permissions(
    env: &mut std::collections::BTreeMap<String, String>,
    agents: ket_core::Result<Vec<String>>,
) -> ket_core::Result<()> {
    use ket_core::permissions::{self, RuleSet};
    const KEY: &str = "OPENCODE_CONFIG_CONTENT";
    let path = |key: &str| env.get(key).map(PathBuf::from);
    let worktree = path(permissions::WORKTREE_ROOT_ENV);
    let root = path(permissions::PROJECT_ROOT_ENV);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let project = env.get(permissions::PROJECT_ID_ENV).cloned();
    let Ok(rules) = RuleSet::load(project.as_deref(), root.as_deref(), home.as_deref()) else {
        return Ok(());
    };
    let rules = rules.opencode_permissions(worktree.as_deref(), home.as_deref());
    if rules.is_empty() {
        return Ok(());
    }
    let agents = agents?;
    // A config the person exported themselves keeps everything it says.
    let config = env.get(KEY).cloned().or_else(|| std::env::var(KEY).ok());
    env.insert(
        KEY.to_owned(),
        permissions::with_opencode_permissions(config.as_deref(), rules, &agents),
    );
    Ok(())
}

/// How to pick up where this worktree's agent left off, if it can.
fn resume_args(cwd: &std::path::Path, agent: &AgentSpec) -> Option<(String, Vec<String>)> {
    let sessions = Sessions::for_launch(agent);
    let agent = agent.name.as_str();
    let latest = sessions
        .scan(&[cwd])
        .remove(cwd)?
        .into_iter()
        // Exact: a local agent resumes only its own model's sessions, and a
        // cloud OpenCode none of them — see `OpencodeSessions`.
        .find(|session| session.agent == agent)?;

    // Not `resume_args`: the newest record is often one still running, and
    // resuming one of those exits immediately. See `Sessions::open_args`.
    // Asked of the harness, whose store it is: a local agent's sessions are
    // OpenCode's.
    // A background session — one agent view ran — opens by attaching to it,
    // which is not picking up where this tab left off: a tab relaunched
    // into one shows a conversation nobody had in it. Left out, so the tab
    // starts afresh instead.
    sessions
        .open_args(ket_core::agents::harness(agent), cwd, &latest.id)
        .filter(|args| args.first().map(String::as_str) != Some("attach"))
        .map(|args| (latest.id, args))
}

impl Shell {
    /// Opens (or resumes) the selected worktree's own session: its configured
    /// agent, picking up its last conversation if it left one.
    ///
    /// This is what selecting a worktree row puts you into — a worktree
    /// stands for a branch *with an agent running in it*, so arriving at one
    /// starts that agent the same way leaving and coming back resumes it.
    /// Deliberately distinct from [`Self::open_terminal_tab`], which is a
    /// blank shell regardless of what a worktree is configured to run.
    pub(crate) fn open_worktree_session_tab(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        // The space is created here when it does not exist, because the one
        // time this is called automatically is a worktree's *first* visit —
        // when by definition it has no space yet. Reading the focused pane out
        // of a space that had to already exist is why clicking a worktree
        // stopped opening anything.
        let pane = self.spaces.entry(id).or_default().focused;
        let agent = self.selected_agent();
        self.open_session_tab_in(pane, agent, true, true, None, None, cx);
    }

    /// Opens (or focuses) a blank terminal tab for the selected worktree.
    ///
    /// The app-wide `ctrl+\`` shortcut's own handler; see
    /// [`Self::open_terminal_tab_in`] for why it never carries an agent.
    pub(crate) fn open_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let pane = self.spaces.entry(id).or_default().focused;
        self.open_terminal_tab_in(pane, cx);
    }

    /// Starts the shells a freshly-selected worktree is short of.
    ///
    /// The session tab first, into the focused pane, because that is what a
    /// worktree is *about* — it resumes whatever its agent left behind. Then a
    /// plain shell in each pane a saved layout said had one, so an arrangement
    /// of shells comes back as an arrangement of shells rather than as one
    /// terminal beside a row of empty panes. A pane that has since acquired a
    /// terminal — the focused one, usually, from the session tab above — is
    /// skipped rather than given a second.
    pub(crate) fn open_owed_terminals(
        &mut self,
        needs_terminal: bool,
        owed: &[PaneId],
        cx: &mut Context<Self>,
    ) {
        if needs_terminal {
            self.open_worktree_session_tab(cx);
        }
        let Some(id) = self.selected_id() else {
            return;
        };
        for pane in owed {
            let wants = self
                .spaces
                .get(&id)
                .is_some_and(|space| space.contains_pane(*pane) && !space.pane_has_terminal(*pane));
            if wants {
                // Adopting, unlike New Terminal: with the ket host running,
                // the shell this pane had may still be alive in it.
                self.open_session_tab_in(*pane, None, false, true, None, None, cx);
            }
        }
    }

    /// Brings back the terminal tabs a saved layout had open in the selected
    /// worktree — see [`crate::layout::SavedTerminal`].
    ///
    /// Each asks the ket host for its terminal by key; one the host no longer
    /// has starts afresh in its place, as the same agent resuming its last
    /// session, or as a shell. A pane that has gone since goes to the focused
    /// one.
    pub(crate) fn restore_terminals(
        &mut self,
        restores: Vec<crate::layout::SavedTerminal>,
        cx: &mut Context<Self>,
    ) {
        if restores.is_empty() {
            return;
        }
        let Some(id) = self.selected_id() else {
            return;
        };
        let agents = self.runnable_agents();
        for restore in restores {
            let agent = restore
                .agent
                .as_deref()
                .and_then(|name| agents.iter().find(|spec| spec.name == name).cloned());
            let pane = match self.spaces.get(&id) {
                Some(space) if space.contains_pane(restore.pane) => restore.pane,
                Some(space) => space.focused,
                None => continue,
            };
            if let Some(saved) = restore.asleep.as_ref() {
                let terminal = TerminalId(self.next_terminal);
                self.next_terminal += 1;
                let title = restore.title.clone().unwrap_or_else(|| {
                    restore
                        .agent
                        .clone()
                        .unwrap_or_else(|| "Terminal".to_owned())
                });
                self.terminal_keys.insert(terminal, restore.key.clone());
                self.asleep_terminals.insert(
                    terminal,
                    AsleepTerminal {
                        agent: restore.agent.clone().unwrap_or_default(),
                        session: saved.session.clone(),
                        since_ms: saved.since_ms,
                        freed_bytes: saved.freed_bytes,
                        wake_requested: false,
                        wake_req: None,
                    },
                );
                self.spaces.entry(id.clone()).or_default().open_tab_in(
                    pane,
                    Tab {
                        title: title.into(),
                        kind: TabKind::Terminal(terminal),
                        renamed: restore.title.is_some(),
                        pinned: restore.pinned,
                        preview: false,
                    },
                );
                // The layout may have been saved just before a crash, while
                // the host was still running this process. Adopt by the saved
                // pane key without ever falling back to spawning another one.
                // `cwd` is not used to launch anything in adopt-only mode,
                // so a missing checkout must not prevent host reconciliation.
                let cwd = self
                    .selected_path()
                    .or_else(|| worktree_path(&id))
                    .unwrap_or_else(|| PathBuf::from("/"));
                let mut spec = TerminalSpec::shell(cwd);
                spec.key = Some(restore.key.clone());
                spec.adopt = true;
                spec.adopt_only = true;
                let agent_name = restore.agent.clone();
                self.terminal_notes
                    .insert(terminal, "Checking agent…".into());
                cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
                    let opened = cx
                        .background_executor()
                        .spawn(async move { Terminal::open(&spec) })
                        .await;
                    let _ = this.update(cx, |shell, cx| match opened {
                        Ok(term) => shell.finish_terminal(
                            terminal,
                            "agent".to_owned(),
                            agent_name,
                            None,
                            Ok(term),
                            cx,
                        ),
                        Err(_) => {
                            shell.terminal_notes.remove(&terminal);
                            let wake = shell
                                .asleep_terminals
                                .get(&terminal)
                                .is_some_and(|asleep| asleep.wake_requested);
                            if wake {
                                shell.wake_terminal(terminal, cx);
                            }
                            cx.notify();
                        }
                    });
                })
                .detach();
                continue;
            }
            let resume = agent.is_some();
            self.open_session_tab_in(pane, agent, resume, true, Some(restore), None, cx);
        }
        self.persist_layout();
        self.sync_asleep_tabs();
    }

    /// Tells the host which tabs are asleep, for phones to list and wake —
    /// see `ket_core::host::publish_asleep`. The tabs asleep in this
    /// window, and the agent tabs a saved layout still owes a worktree
    /// nobody has opened since the restart: as good as asleep, with no tab
    /// yet. Those put to sleep say since when and what that freed; one the
    /// host restart stopped knows neither.
    pub(crate) fn sync_asleep_tabs(&self) {
        let mut tabs: Vec<ket_core::host::wire::AsleepTab> = self
            .asleep_terminals
            .iter()
            .filter_map(|(terminal, asleep)| {
                let key = self.terminal_keys.get(terminal)?.clone();
                let worktree = self.worktree_of_terminal(*terminal)?;
                let title = self
                    .spaces
                    .get(&worktree)
                    .and_then(|space| space.tab_title(&TabKind::Terminal(*terminal)))
                    .map(|title| title.to_string())
                    .unwrap_or_else(|| asleep.agent.clone());
                Some(ket_core::host::wire::AsleepTab {
                    key,
                    worktree: worktree.to_string(),
                    agent: asleep.agent.clone(),
                    title,
                    since_ms: asleep.since_ms,
                    freed_bytes: asleep.freed_bytes,
                })
            })
            .collect();
        for (worktree, owed) in &self.owed_restores {
            for restore in owed {
                let Some(agent) = restore.agent.clone().filter(|agent| !agent.is_empty()) else {
                    continue;
                };
                let saved = restore.asleep.as_ref();
                tabs.push(ket_core::host::wire::AsleepTab {
                    key: restore.key.clone(),
                    worktree: worktree.to_string(),
                    title: restore.title.clone().unwrap_or_else(|| {
                        if restore.agent_view {
                            "Agent view".to_owned()
                        } else {
                            agent.clone()
                        }
                    }),
                    agent,
                    since_ms: saved.map_or(0, |saved| saved.since_ms),
                    freed_bytes: saved.map_or(0, |saved| saved.freed_bytes),
                });
            }
        }
        tabs.sort_by(|a, b| a.key.cmp(&b.key));
        ket_core::host::publish_asleep(tabs);
    }

    /// Wakes the asleep tab `key` for a phone — see
    /// `ket_core::host::take_phone_wake`. A tab a saved layout still owes
    /// its worktree is restored first, by opening that worktree, as a click
    /// on it would. The host hears a wake that works from the terminal
    /// opened under the key; only a refusal is answered, with the note the
    /// tab shows.
    pub(crate) fn wake_from_phone(&mut self, req: u64, key: String, cx: &mut Context<Self>) {
        let by_key = |shell: &Self| {
            shell
                .terminal_keys
                .iter()
                .find(|(_, k)| **k == key)
                .map(|(terminal, _)| *terminal)
        };
        let mut terminal = by_key(self);
        if terminal.is_none() {
            let owed = self
                .owed_restores
                .iter()
                .find(|(_, owed)| owed.iter().any(|restore| restore.key == key))
                .map(|(worktree, _)| worktree.clone());
            if let Some(selection) = owed.and_then(|worktree| self.position_of(&worktree)) {
                self.select(selection, cx);
                terminal = by_key(self);
            }
        }
        let Some(terminal) = terminal else {
            ket_core::host::finish_phone_wake(req, "that tab isn't here any more".to_owned());
            return;
        };
        if !self.asleep_terminals.contains_key(&terminal) {
            // Awake already: the host has, or will have, its terminal.
            return;
        }
        if let Some(asleep) = self.asleep_terminals.get_mut(&terminal) {
            asleep.wake_req = Some(req);
        }
        self.wake_terminal(terminal, cx);
    }

    /// A wake that did not happen: the tab says why, and so does the phone
    /// that asked for it, if one did.
    fn wake_failed(&mut self, terminal: TerminalId, why: SharedString, cx: &mut Context<Self>) {
        self.terminal_notes.insert(terminal, why.clone());
        if let Some(req) = self
            .asleep_terminals
            .get_mut(&terminal)
            .and_then(|asleep| asleep.wake_req.take())
        {
            ket_core::host::finish_phone_wake(req, why.to_string());
        }
        cx.notify();
    }

    /// Restart one stopped tab with the session ID it held before sleep.
    pub(crate) fn wake_terminal(&mut self, terminal: TerminalId, cx: &mut Context<Self>) {
        if self
            .terminal_notes
            .get(&terminal)
            .is_some_and(|note| note.as_ref() == "Checking agent…")
        {
            if let Some(asleep) = self.asleep_terminals.get_mut(&terminal) {
                asleep.wake_requested = true;
            }
            return;
        }
        if self.terminals.contains_key(&terminal) {
            if let Some(asleep) = self.asleep_terminals.get_mut(&terminal) {
                asleep.wake_requested = true;
                self.terminal_notes
                    .insert(terminal, "Waking when the agent stops…".into());
                cx.notify();
            }
            return;
        }
        if self
            .terminal_notes
            .get(&terminal)
            .is_some_and(|note| note.as_ref() == "Waking agent…")
        {
            return;
        }
        let Some(asleep) = self.asleep_terminals.get(&terminal) else {
            return;
        };
        let Some(worktree) = self
            .spaces
            .iter()
            .find_map(|(id, space)| space.terminal_ids().contains(&terminal).then(|| id.clone()))
        else {
            return;
        };
        let Some(cwd) = self
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|node| node.id == worktree)
            .map(|node| node.path.clone())
            .or_else(|| worktree_path(&worktree))
        else {
            self.wake_failed(terminal, "Worktree directory is unavailable".into(), cx);
            return;
        };
        let Some(agent) = self
            .runnable_agents()
            .into_iter()
            .find(|agent| agent.name == asleep.agent)
        else {
            self.wake_failed(
                terminal,
                "This agent is unavailable; the tab remains asleep".into(),
                cx,
            );
            return;
        };
        let agent = self.project_agent_spec(&worktree, &agent);
        let session = asleep.session.clone();
        let launched_with = self
            .project_agent_override(&worktree, &agent.name)
            .map(crate::agent_override::short);
        let key = self.terminal_keys.get(&terminal).cloned();
        let pane = self
            .spaces
            .get(&worktree)
            .map_or(PaneId(0), |space| space.focused);
        let size = self.pane_terminal_size(&worktree, pane, cx);
        self.terminal_notes.insert(terminal, "Waking agent…".into());
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    let sessions = Sessions::for_launch(&agent);
                    let recorded = sessions.scan(&[&cwd]).remove(&cwd).is_some_and(|found| {
                        found
                            .iter()
                            .any(|found| found.id == session && found.agent == agent.name)
                    });
                    let resume = recorded
                        .then(|| {
                            sessions.open_args(
                                ket_core::agents::harness(&agent.name),
                                &cwd,
                                &session,
                            )
                        })
                        .flatten()
                        .map(|args| (session, args));
                    let settings = Workspace::open()
                        .ok()
                        .map(|workspace| workspace.config().agent.clone());
                    let opencode_agents = (ket_core::agents::harness(&agent.name) == "opencode")
                        .then(|| {
                            use ket_core::agents::Prober;
                            let program = agent
                                .command
                                .split_whitespace()
                                .next()
                                .unwrap_or("opencode");
                            let inline = agent
                                .env
                                .get("OPENCODE_CONFIG_CONTENT")
                                .cloned()
                                .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
                            match ket_core::agents::ShellProber.find(program) {
                                Some(found) => ket_core::permissions::opencode_agents(
                                    &found,
                                    &agent.command,
                                    &cwd,
                                    inline.as_deref(),
                                ),
                                None => Err(ket_core::KetError::Config(
                                    "OpenCode could not be found to inspect its agents".to_owned(),
                                )),
                            }
                        });
                    (agent, cwd, resume, settings, opencode_agents)
                })
                .await;
            let (agent, cwd, resume, settings, opencode_agents) = prepared;
            let Some(resume) = resume else {
                let _ = this.update(cx, |shell, cx| {
                    shell.wake_failed(
                        terminal,
                        "That session is open elsewhere or cannot be resumed".into(),
                        cx,
                    );
                });
                return;
            };
            let agent_name = agent.name.clone();
            let spec = this
                .update(cx, |shell, _| {
                    if !shell.asleep_terminals.contains_key(&terminal)
                        || !shell.usage_history.can_resume_with_economy(
                            &resume.0,
                            shell.token_reduction_for(&worktree),
                        )
                    {
                        return None;
                    }
                    Some(shell.terminal_spec(
                        &worktree,
                        cwd,
                        Some(agent),
                        Some(resume),
                        settings.as_ref(),
                        terminal,
                        None,
                    ))
                })
                .ok()
                .flatten();
            let Some(mut spec) = spec else {
                let _ = this.update(cx, |shell, cx| {
                    shell.wake_failed(
                        terminal,
                        "Economy prevents resuming this session".into(),
                        cx,
                    );
                });
                return;
            };
            if let Some(agents) = opencode_agents
                && let Err(error) = opencode_permissions(&mut spec.env, agents)
            {
                let _ = this.update(cx, |shell, cx| {
                    shell.wake_failed(
                        terminal,
                        format!("Could not wake OpenCode: {error}").into(),
                        cx,
                    );
                });
                return;
            }
            spec.key = key;
            if let Some(size) = size {
                spec.size = size;
            }
            let command = spec.command.clone();
            let opened = cx
                .background_executor()
                .spawn(async move { Terminal::open(&spec) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.finish_terminal(
                    terminal,
                    command,
                    Some(agent_name),
                    launched_with,
                    opened,
                    cx,
                );
            });
        })
        .detach();
    }

    /// Stop an idle agent while retaining its tab and exact session.
    fn sleep_terminal(
        &mut self,
        terminal: TerminalId,
        trigger: SleepTrigger,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let handle = self
            .terminals
            .get(&terminal)
            .ok_or("terminal is not running")?;
        let agent = handle.agent.clone().ok_or("plain shells cannot sleep")?;
        let term = handle.term.clone();
        if term.is_closed() {
            return Err("terminal has already exited".to_owned());
        }
        let key = self
            .terminal_keys
            .get(&terminal)
            .ok_or("terminal has no session key")?;
        let session = sleep_session(
            trigger,
            self.window_active && self.active_terminal_id() == Some(terminal),
            &agent,
            self.activity.pane_status(key),
            self.activity
                .pane(key, ket_core::now_ms())
                .map(|(_, signal)| signal),
        )?
        .to_owned();
        let resume = Sessions::new().resume_args(ket_core::agents::harness(&agent), &session);
        if resume.is_none_or(|args| args.is_empty()) {
            return Err(format!("{agent} cannot resume a session"));
        }
        let worktree = self
            .spaces
            .iter()
            .find_map(|(id, space)| space.terminal_ids().contains(&terminal).then_some(id))
            .ok_or("terminal has no worktree")?;
        if !self
            .usage_history
            .can_resume_with_economy(&session, self.token_reduction_for(worktree))
        {
            return Err("Economy prevents resuming this session".to_owned());
        }
        self.refresh_overhead();
        let freed_bytes = term
            .pid()
            .and_then(|pid| self.overhead.terminal_memory(pid))
            .unwrap_or(0);
        self.asleep_terminals.insert(
            terminal,
            AsleepTerminal {
                agent,
                session,
                since_ms: ket_core::now_ms(),
                freed_bytes,
                wake_requested: false,
                wake_req: None,
            },
        );
        self.terminal_notes.remove(&terminal);
        self.persist_layout();
        self.sync_asleep_tabs();
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let stopped = cx
                .background_executor()
                .spawn(async move { term.stop(Duration::from_secs(2)) })
                .await;
            if !matches!(stopped, Ok(StopOutcome::Stopped | StopOutcome::Gone)) {
                let _ = this.update(cx, |shell, cx| {
                    // A failed round trip does not prove whether the stop was
                    // applied. Only a live phone lease lets us safely undo
                    // sleep; a closed terminal must keep its saved session.
                    if matches!(stopped, Ok(StopOutcome::Controlled))
                        && shell
                            .terminals
                            .get(&terminal)
                            .is_some_and(|term| !term.term.is_closed())
                    {
                        shell.asleep_terminals.remove(&terminal);
                        shell.persist_layout();
                        shell.sync_asleep_tabs();
                    }
                    let message = match stopped {
                        Ok(StopOutcome::Controlled) => {
                            "Could not put the agent to sleep: the terminal is controlled by a phone"
                        }
                        _ => "Could not confirm that the agent stopped",
                    };
                    shell.toast(Tone::Error, message, cx);
                });
            }
        })
        .detach();
        Ok(())
    }

    /// Apply the menu action to every eligible agent tab in one worktree.
    pub(crate) fn sleep_worktree(&mut self, worktree: &WorktreeId, cx: &mut Context<Self>) {
        let ids = self
            .spaces
            .get(worktree)
            .map_or_else(Vec::new, |space| space.terminal_ids());
        let mut slept = 0;
        let mut reasons = Vec::new();
        let mut skipped = 0;
        for id in ids {
            // Shells are not agents, and must not hide an agent's refusal.
            if !self
                .terminals
                .get(&id)
                .is_some_and(|handle| handle.agent.is_some())
            {
                continue;
            }
            match self.sleep_terminal(id, SleepTrigger::Manual, cx) {
                Ok(()) => slept += 1,
                Err(reason) => {
                    skipped += 1;
                    if !reasons.contains(&reason) {
                        reasons.push(reason);
                    }
                }
            }
        }
        if slept == 0 {
            self.toast_detail(
                Tone::Error,
                "No agent could sleep",
                if reasons.is_empty() {
                    "No agent tab is running".to_owned()
                } else {
                    reasons.join("; ")
                },
                cx,
            );
        } else {
            let message = format!(
                "Putting {slept} agent{} to sleep",
                if slept == 1 { "" } else { "s" }
            );
            if skipped == 0 {
                self.toast(Tone::Info, message, cx);
            } else {
                self.toast_detail(
                    Tone::Warning,
                    message,
                    format!("{skipped} could not sleep: {}", reasons.join("; ")),
                    cx,
                );
            }
        }
    }

    /// Wake every asleep agent tab in one worktree.
    pub(crate) fn wake_worktree(&mut self, worktree: &WorktreeId, cx: &mut Context<Self>) {
        let ids = self
            .spaces
            .get(worktree)
            .map_or_else(Vec::new, |space| space.terminal_ids());
        for id in ids {
            if self.asleep_terminals.contains_key(&id) {
                self.wake_terminal(id, cx);
            }
        }
    }

    /// Check each agent pane independently; a busy neighbour does not hide an idle one.
    pub(crate) fn sweep_idle_agents(&mut self, cx: &mut Context<Self>) {
        let Ok(config) = ket_core::config::Config::load() else {
            return;
        };
        if !config.sleep.enabled() {
            return;
        }
        let now = ket_core::now_ms();
        let threshold = u64::from(config.sleep.after_minutes) * 60_000;
        let ids: Vec<_> = self
            .spaces
            .values()
            .flat_map(|space| space.terminal_ids())
            .collect();
        for id in ids {
            let Some(key) = self.terminal_keys.get(&id) else {
                continue;
            };
            let Some(pane) = self.activity.pane_status(key) else {
                continue;
            };
            if pane.state == ket_core::event::AgentState::Idle
                && now.saturating_sub(pane.at_ms) >= threshold
                && now.saturating_sub(pane.heard_at_ms()) >= threshold
            {
                let _ = self.sleep_terminal(id, SleepTrigger::Automatic, cx);
            }
        }
    }

    /// Opens a new, blank terminal in one named pane. Never an agent.
    ///
    /// Always a *new* tab. Opening used to be idempotent — a second call
    /// focused the terminal already open for this worktree — which is why the
    /// `+` menu could never give you a second shell, and why a shell that had
    /// exited could never be replaced by a live one.
    ///
    /// This is what the `+` menu's New Terminal item dispatches, and it
    /// carries the same `ctrl+\`` chord the row hints at — so the two must
    /// agree on what they do, and what they do is a plain shell. Starting an
    /// agent is [`Self::open_worktree_session_tab`]'s job when you arrive at
    /// a worktree, or [`Self::open_agent_tab_in`]'s when you ask for one
    /// explicitly; "terminal" no longer means either.
    pub(crate) fn open_terminal_tab_in(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        self.open_session_tab_in(pane, None, false, false, None, None, cx);
    }

    /// Opens a fresh session with the named agent in one named pane, never
    /// resumed.
    ///
    /// `agent_name` is one of [`Self::runnable_agents`], picked from the `+`
    /// menu's own list — not necessarily the worktree's configured agent, the
    /// way [`Self::open_worktree_session_tab`]'s is. A name that no longer
    /// resolves (switched off or uninstalled between the menu opening and the
    /// click landing) opens nothing rather than falling back to some other
    /// agent silently.
    pub(crate) fn open_agent_tab_in(
        &mut self,
        pane: PaneId,
        agent_name: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self
            .runnable_agents()
            .into_iter()
            .find(|spec| spec.name == agent_name)
        else {
            return;
        };
        self.open_session_tab_in(pane, Some(agent), false, false, None, None, cx);
    }

    /// A fresh agent tab with one-session model and effort choices.
    pub(crate) fn open_agent_tab_with(
        &mut self,
        pane: PaneId,
        agent_name: &str,
        launch: SessionLaunch,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self
            .runnable_agents()
            .into_iter()
            .find(|spec| spec.name == agent_name)
        else {
            return;
        };
        self.open_session_tab_in(pane, Some(agent), false, false, None, Some(launch), cx);
    }

    /// Shared body for the three openers above; `agent` and
    /// `resume_previous` are what tell them apart.
    ///
    /// `adopt` asks the ket host for the terminal it is running under this
    /// tab's key, when no window is showing it, before starting a new one.
    /// `restored` is a tab a saved layout brought back: its key, the title
    /// somebody typed for it and whether it was pinned. See `ket_core::host`; without the host neither
    /// does anything.
    #[allow(clippy::too_many_arguments)]
    fn open_session_tab_in(
        &mut self,
        pane: PaneId,
        agent: Option<AgentSpec>,
        resume_previous: bool,
        adopt: bool,
        restored: Option<crate::layout::SavedTerminal>,
        session_launch: Option<SessionLaunch>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_id() else {
            return;
        };
        // The selected row's own path. `worktree_path` only knows worktrees
        // core created, which the repository's checkout is not.
        let Some(cwd) = self.selected_path().or_else(|| worktree_path(&id)) else {
            tracing::warn!(worktree = %id, "no directory for the selected row");
            self.note = Some("could not find that worktree's directory".into());
            return;
        };
        // The project's own command, where it has one, from here on: the
        // session to resume is looked for where that command keeps them, and
        // every word the launch adds joins that line, not the configured one.
        let agent = agent.map(|agent| self.project_agent_spec(&id, &agent));
        let launched_with = agent
            .as_ref()
            .and_then(|agent| self.project_agent_override(&id, &agent.name))
            .map(crate::agent_override::short);

        // The tab is named after its agent, if it has one, so a glance at the
        // strip says whose session this is.
        let pinned = restored.as_ref().is_some_and(|restore| restore.pinned);
        // A tab that was showing Claude's agent view comes back as agent
        // view, not as whichever session agent view last listed: that one
        // is the newest in the worktree, and resuming it would attach to a
        // background session instead.
        let agent_view = restored.as_ref().is_some_and(|restore| restore.agent_view);
        let (restored_key, typed_title) =
            restored.map(|restore| (restore.key, restore.title)).unzip();
        let typed_title = typed_title.flatten();
        let title: SharedString = typed_title
            .clone()
            .map(SharedString::from)
            .or_else(|| {
                agent.as_ref().map(|spec| {
                    // A local agent's name is a launch key; it reads as its
                    // model, `Qwen3 0.6B`.
                    if ket_core::local_models::model_tag(&spec.name).is_some() {
                        SharedString::from(crate::ui::agent::agent_label(&spec.name))
                    } else {
                        SharedString::from(spec.name.clone())
                    }
                })
            })
            .unwrap_or_else(|| "Terminal".into());

        // Put the tab on screen before scanning session stores or opening a
        // pty. Both can take long enough to miss a frame on a cold machine.
        let terminal = TerminalId(self.next_terminal);
        self.next_terminal += 1;
        self.terminal_notes
            .insert(terminal, "Starting terminal…".into());
        // Unique to this tab, so a restart can ask for exactly this terminal
        // back, and kept across restarts once it is saved in the layout.
        let key = restored_key.unwrap_or_else(|| {
            format!(
                "{id}|{}|{}-{}",
                agent.as_ref().map_or("shell", |spec| spec.name.as_str()),
                ket_core::now_ms(),
                terminal.0
            )
        });
        self.terminal_keys.insert(terminal, key.clone());

        // The size the pane already draws at, when it has a terminal to say.
        // Spawned at that size, the shell starts at its real width and the
        // host's first checkpoint is the one that gets drawn — instead of a
        // 120×40 grid that the first layout reflows, with a SIGWINCH into a
        // shell still reading its startup files and a second checkpoint
        // round trip before the prompt sits where it should.
        let size = self.pane_terminal_size(&id, pane, cx);

        let space = self.spaces.entry(id.clone()).or_default();
        space.open_tab_in(
            pane,
            Tab {
                title,
                kind: TabKind::Terminal(terminal),
                renamed: typed_title.is_some(),
                pinned,
                preview: false,
            },
        );
        self.persist_layout();
        cx.notify();

        // A plain shell has nothing to look up before it can start: its spec
        // is built here and the pty opened straight from the background, with
        // no trip back to this thread in between. An agent's spec waits on
        // the settings and the session scan, which are read off this thread.
        let shell_spec = agent
            .is_none()
            .then(|| self.terminal_spec(&id, cwd.clone(), None, None, None, terminal, None));
        let scan_cwd = cwd.clone();
        let scan_agent = agent.clone();
        let prepared = cx.background_executor().spawn(async move {
            let settings = scan_agent.as_ref().and_then(|_| {
                Workspace::open()
                    .ok()
                    .map(|workspace| workspace.config().agent.clone())
            });
            let resume = if agent_view {
                // No session: the word that opens agent view, on the line.
                Some((String::new(), vec!["agents".to_owned()]))
            } else {
                resume_previous
                    .then(|| {
                        scan_agent
                            .as_ref()
                            .and_then(|agent| resume_args(&scan_cwd, agent))
                    })
                    .flatten()
            };
            // OpenCode's agents here, so ket's rules can go after each one's
            // own — see `opencode_permissions`.
            let opencode_agents = scan_agent
                .as_ref()
                .filter(|agent| ket_core::agents::harness(&agent.name) == "opencode")
                .map(|agent| {
                    use ket_core::agents::Prober;
                    let program = agent
                        .command
                        .split_whitespace()
                        .next()
                        .unwrap_or("opencode");
                    let inline = agent
                        .env
                        .get("OPENCODE_CONFIG_CONTENT")
                        .cloned()
                        .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
                    match ket_core::agents::ShellProber.find(program) {
                        Some(found) => ket_core::permissions::opencode_agents(
                            &found,
                            &agent.command,
                            &scan_cwd,
                            inline.as_deref(),
                        ),
                        None => Err(ket_core::KetError::Config(
                            "OpenCode could not be found to inspect its agents".to_owned(),
                        )),
                    }
                });
            (settings, resume, opencode_agents)
        });
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let agent_name = agent.as_ref().map(|agent| agent.name.clone());
            let (spec, opencode_agents) = match shell_spec {
                Some(spec) => (Some(spec), None),
                None => {
                    let (settings, resume, opencode_agents) = prepared.await;
                    let spec = this
                        .update(cx, |shell, _| {
                            shell.terminal_notes.contains_key(&terminal).then(|| {
                                shell.terminal_spec(
                                    &id,
                                    cwd,
                                    agent,
                                    resume,
                                    settings.as_ref(),
                                    terminal,
                                    session_launch,
                                )
                            })
                        })
                        .ok()
                        .flatten();
                    (spec, opencode_agents)
                }
            };
            let Some(mut spec) = spec else {
                return;
            };
            if let Some(agents) = opencode_agents
                && let Err(error) = opencode_permissions(&mut spec.env, agents)
            {
                let _ = this.update(cx, |shell, cx| {
                    shell.terminal_notes.insert(
                        terminal,
                        format!("Could not start OpenCode: {error}").into(),
                    );
                    cx.notify();
                });
                return;
            }
            spec.key = Some(key);
            spec.adopt = adopt;
            if let Some(size) = size {
                spec.size = size;
            }

            let command = spec.command.clone();
            let opened = cx
                .background_executor()
                .spawn(async move { Terminal::open(&spec) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.finish_terminal(terminal, command, agent_name, launched_with, opened, cx);
            });
        })
        .detach();
    }

    /// The size the terminals already in `pane` are drawn at, if it has any.
    ///
    /// Tabs in one pane share a size, so a terminal opened there can be
    /// started at it rather than at the default and resized on first layout.
    /// The active tab's geometry is asked for first: a hidden tab is not
    /// painted, so what it remembers can predate the pane's last resize.
    fn pane_terminal_size(
        &self,
        worktree: &WorktreeId,
        pane: PaneId,
        cx: &Context<Self>,
    ) -> Option<ket_core::terminal::TerminalSize> {
        self.spaces
            .get(worktree)?
            .pane_terminals(pane)
            .into_iter()
            .find_map(|id| {
                let geometry = self
                    .terminal_view(id)?
                    .read(cx)
                    .geometry
                    .filter(Geometry::is_valid)?;
                Some(ket_core::terminal::TerminalSize::new(
                    u16::try_from(geometry.cols).ok()?,
                    u16::try_from(geometry.rows).ok()?,
                ))
            })
    }

    /// Resolves the selected agent through the same catalogue as Settings.
    ///
    /// This is deliberately not a scan of `config.agents`: a disabled agent
    /// must not reappear at startup merely because an older worktree recorded
    /// its name. An unavailable configured command opens a plain shell rather
    /// than silently substituting a different account or provider.
    pub(crate) fn selected_agent(&self) -> Option<AgentSpec> {
        let selection = self.selection?;
        let wanted = self
            .projects
            .get(selection.project)?
            .worktrees
            .get(selection.worktree)?
            .agent
            .clone();
        let workspace = Workspace::open().ok()?;
        let config = workspace.config();
        // ShellProber and not PathProber: a pane types its command at a login
        // shell's prompt, so an alias or a function is as runnable as a file on
        // PATH, and a PATH walk would report the agent missing and open a bare
        // shell instead. See `ket_core::shell`.
        let catalogue = Catalogue::detect(config, &ShellProber);

        match catalogue.resolve(
            wanted.as_deref().map(|name| &**name),
            None,
            &config.agent.default,
        ) {
            Resolution::Agent(spec) => Some(*spec),
            Resolution::NoAgent | Resolution::Unavailable(_) => None,
        }
    }

    /// Every agent Settings has enabled and this machine can actually run,
    /// in name order — what the `+` menu's agent rows list from.
    ///
    /// Global, not worktree-specific: which agents exist does not depend on
    /// which worktree is selected, only [`Self::selected_agent`]'s pick among
    /// them does.
    pub(crate) fn runnable_agents(&self) -> Vec<AgentSpec> {
        runnable_specs()
    }

    /// The command the project holding `worktree` launches `agent` with, when
    /// it has one of its own rather than the config's.
    pub(crate) fn project_agent_override(
        &self,
        worktree: &WorktreeId,
        agent: &str,
    ) -> Option<&ket_core::config::AgentLaunch> {
        self.projects
            .iter()
            .find(|project| project.worktrees.iter().any(|node| &node.id == worktree))
            .and_then(|project| project.agent_overrides.get(agent))
    }

    /// `agent` as the project holding `worktree` launches it.
    fn project_agent_spec(&self, worktree: &WorktreeId, agent: &AgentSpec) -> AgentSpec {
        let mut agent = agent.clone();
        if let Some(launch) = self.project_agent_override(worktree, &agent.name) {
            agent.launch = Some(launch.clone());
        }
        agent
    }

    /// Builds the launch specification after background preparation finishes.
    #[allow(clippy::too_many_arguments)]
    fn terminal_spec(
        &self,
        worktree: &WorktreeId,
        cwd: PathBuf,
        agent: Option<AgentSpec>,
        resume: Option<(String, Vec<String>)>,
        settings: Option<&AgentConfig>,
        id: TerminalId,
        session_launch: Option<SessionLaunch>,
    ) -> TerminalSpec {
        // The resume words join the agent's command *line*, not an argument
        // vector: a pane types its command at a shell prompt rather than
        // spawning it, so there is no argv to push onto here.
        //
        // The cache-lifetime flag joins them, on the same line and for the same
        // reason. It is only ever added for a metered account — see
        // `worktree::cache_ttl_for`, which returns nothing at all until some
        // session has said which kind of account this is.
        let level = self.token_reduction_for(worktree);
        let resume = resume.and_then(|(session, args)| {
            self.usage_history
                .can_resume_with_economy(&session, level)
                .then_some((session, args))
        });
        // The session being resumed, for the host to bind to the pane before
        // the agent has reported a thing: a phone reads the session by it,
        // and a Codex report that names the pane saved in the session still
        // lands in this one. Every agent, not Codex alone: OpenCode says
        // nothing on resuming until a message is typed to it.
        let resumed_session = resume
            .as_ref()
            .map(|(session, _)| session.clone())
            .filter(|session| !session.is_empty());
        let mut extra = resume.map(|(_, args)| args).unwrap_or_default();
        let mut economy_env = std::collections::BTreeMap::new();
        if let Some(agent) = agent.as_ref() {
            // The level's own policy sits *under* the reader's config: a pack
            // proposes, a config decides. Every built-in level names nothing at
            // all, so this changes no launch until a pack is in force.
            let level = ket_core::worktree::token_reduction(self.token_reduction_for(worktree));
            let policy = level.policy_for(&agent.name);
            let ttl = ket_core::worktree::cache_ttl_for(
                &agent.name,
                self.usage_history.latest_billing(),
                settings
                    .and_then(|agent| agent.prompt_cache_ttl.as_deref())
                    .or(policy.cache_ttl.as_deref()),
            );
            let limit = settings.and_then(|agent| agent.output_limit);
            let compact = settings
                .and_then(|agent| agent.autocompact.clone())
                .or_else(|| policy.autocompact.clone());
            let launch = ket_core::worktree::economy_launch(
                &agent.name,
                level,
                ttl.as_deref(),
                limit,
                compact.as_deref(),
            );
            let mut economy_args = launch.args;
            if let Some(overrides) = session_launch.as_ref() {
                // Explicit phone choices win over policy flags, without
                // leaving duplicate CLI options for the agent to interpret.
                let mut kept = Vec::new();
                let mut args = economy_args.into_iter();
                while let Some(arg) = args.next() {
                    if (arg == "--model" && overrides.model.is_some())
                        || ((arg == "--effort" || arg == "--reasoning-effort")
                            && overrides.effort.is_some())
                        || (arg == "-c"
                            && overrides.effort.is_some()
                            && args
                                .as_slice()
                                .first()
                                .is_some_and(|next| next.starts_with("model_reasoning_effort=")))
                    {
                        let _ = args.next();
                    } else {
                        kept.push(arg);
                    }
                }
                economy_args = kept;
            }
            extra.extend(economy_args);
            economy_env = launch.env;
            if let Some(overrides) = session_launch.as_ref() {
                if let Some(model) = &overrides.model {
                    extra.extend(["--model".to_owned(), model.clone()]);
                }
                if let Some(effort) = &overrides.effort {
                    match agent.name.as_str() {
                        "codex" => extra
                            .extend(["-c".to_owned(), format!("model_reasoning_effort={effort}")]),
                        "grok" => extra.extend(["--reasoning-effort".to_owned(), effort.clone()]),
                        _ => extra.extend(["--effort".to_owned(), effort.clone()]),
                    }
                }
            }
            // The filter, like the cap: a pack proposes, a config decides.
            match settings.and_then(|agent| agent.command_filter) {
                Some(false) => {
                    economy_env.remove(ket_core::filter::ENV);
                }
                Some(true) => {
                    if let Ok(exe) = std::env::current_exe() {
                        economy_env
                            .insert(ket_core::filter::ENV.to_owned(), exe.display().to_string());
                    }
                }
                None => {}
            }
            // Keeping what the filter trims is the reader's to ask for, and
            // only means anything while the filter is on.
            if settings.and_then(|agent| agent.command_archive) == Some(true)
                && economy_env.contains_key(ket_core::filter::ENV)
            {
                economy_env.insert(ket_core::filter::ARCHIVE_ENV.to_owned(), "1".to_owned());
            }
        }

        let mut spec = match &agent {
            Some(agent) => TerminalSpec::agent_with(cwd, agent, &extra),
            None => TerminalSpec::shell(cwd),
        };

        // What lets the agent's own hooks say which worktree they belong to.
        // Set on a shell as well as on an agent: a person who types `claude`
        // into a plain pane should be reported the same as one ket launched,
        // and the shell passes its environment to whatever it runs.
        let name = agent.as_ref().map(|a| a.name.as_str()).unwrap_or("claude");
        spec.env.extend(self.hook_env(worktree, name, id));
        spec.env.extend(economy_env);
        expose_cli(&mut spec);
        // The network inspector, while it is still being built: asked for
        // by launching the window with `KET_NETWORK_INSPECTOR=1`. The host
        // takes the word out and points the agent at its proxy.
        if agent.is_some() && std::env::var_os("KET_NETWORK_INSPECTOR").is_some() {
            spec.env
                .insert(ket_core::network::LOG_ENV.to_owned(), "1".to_owned());
        }
        if let Some(session) = resumed_session {
            spec.env.insert(
                ket_core::agent_hooks::RESUME_SESSION_ENV.to_owned(),
                session,
            );
        }
        // The level and the sentence that goes with it, both from the one
        // table: the hook script has no idea which level means what, so a note
        // ket does not send is a session that is not asked to do anything
        // differently.
        let reduction = ket_core::worktree::token_reduction(self.token_reduction_for(worktree));
        if agent.is_none() {
            spec.env.insert(
                ket_core::worktree::TOKEN_REDUCTION_ENV.to_owned(),
                reduction.level.to_string(),
            );
            spec.env.insert(
                ket_core::worktree::TOKEN_REDUCTION_NOTE_ENV.to_owned(),
                reduction.instruction.clone().unwrap_or_default(),
            );
        }
        if agent
            .as_ref()
            .is_some_and(|agent| agent.name.eq_ignore_ascii_case("claude"))
        {
            // Where this session's own token and cost metrics go, when a
            // collector is configured. Claude-only for the same reason as the
            // block it sits in: `CLAUDE_CODE_ENABLE_TELEMETRY` is Claude Code's,
            // and the `OTEL_*` variables around it are inert without it — but
            // they are also *not* inert in the general case, since another tool
            // in that pane may read them and start exporting on ket's say-so.
            // Setting them only where they were meant to be read keeps this a
            // decision about the agent ket launched.
            spec.env.extend(
                self.telemetry_env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }

        tracing::debug!(
            worktree = %worktree,
            command = %spec.command,
            args = ?spec.args,
            cwd = %spec.cwd.display(),
            "opening a terminal"
        );

        spec
    }

    /// Attaches a pty opened by the background executor to its waiting tab.
    fn finish_terminal(
        &mut self,
        id: TerminalId,
        command: String,
        agent: Option<String>,
        launched_with: Option<SharedString>,
        opened: ket_core::Result<Terminal>,
        cx: &mut Context<Self>,
    ) {
        // Closing a starting tab removes its note. If the background work wins
        // that race later, the program it started has no tab to live in, so it
        // is ended here. Dropping it is not enough: a hosted terminal only
        // detaches on drop, and the shell would stay running in the host.
        if !self.terminal_notes.contains_key(&id) {
            if let Ok(term) = opened {
                term.kill();
            }
            return;
        }

        let term = match opened {
            Ok(term) => term,
            Err(e) => {
                tracing::warn!(terminal = id.0, error = %e, "terminal refused to open");
                self.wake_failed(id, format!("Could not open terminal: {e}").into(), cx);
                return;
            }
        };
        self.asleep_terminals.remove(&id);
        self.persist_layout();
        self.terminal_notes.remove(&id);
        self.sync_asleep_tabs();

        let mut frames = term.frames();
        let term = Arc::new(term);

        // The only poll in this pane, and not really one: `changed()` blocks
        // until the backend's `FrameClock` actually publishes a frame, so an
        // idle shell wakes this task never and a busy one wakes it at most 60
        // times a second.
        //
        // When it *stops* — which is what a shell exiting looks like from here
        // — the tab usually goes with it. That is what a terminal does
        // everywhere else on the platform, and it is the only way the pane
        // cannot get stuck showing a dead shell.
        //
        // Usually, not always: see [`STARTUP_GRACE`]. A program that dies on
        // startup writes the reason to the very grid this would throw away,
        // and closing the tab turns a message that says exactly what went
        // wrong into an empty pane that says nothing. That is not a
        // hypothetical — `claude --resume` on a session already running in the
        // background exits 1 with instructions on the screen, and every one of
        // those was deleted before it could be read.
        // The view is the pane. It is handed everything drawing needs — the
        // faces, the colours, whether it is the keyboard target — rather than
        // being left to read them off the shell, for the reason in
        // `TerminalView`'s own docs.
        let shell = cx.entity().downgrade();
        let view = cx.new(|_cx| TerminalView {
            id,
            term: term.clone(),
            focus: self.focus.clone(),
            family: self.font_family.clone(),
            font_size: px(f32::from(self.theme_config.code_font_size)),
            theme: self.theme,
            focused: false,
            geometry: None,
            cursor_bounds: None,
            marked: None,
            hovered_link: None,
            blink: Blink {
                visible: true,
                task: None,
            },
            grid: None,
            exit_note: None,
            find: None,
            shell,
        });

        let dying = term.clone();
        let painting = view.downgrade();
        let typed_at_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let recent_input = typed_at_ms.clone();
        let watch = cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let opened = std::time::Instant::now();
            // When this pane last asked to be drawn, for `REPAINT_GAP`.
            let mut painted: Option<std::time::Instant> = None;
            loop {
                if frames.changed().await.is_err() {
                    break;
                }
                // Inside the gap, wait out the rest of it. The repaint after
                // it reads the grid as it is then, so frames that land
                // meanwhile are drawn by it — and are marked seen here, or
                // the next `changed` would fire at once for them and paint
                // the same grid a second time.
                //
                // Unless it arrived soon after a keystroke. While an agent's
                // spinner keeps the pane busy it never has the quiet spell the
                // gap waits for, so every character typed into it waited up to
                // the whole gap before it appeared — the lag of typing into a
                // working session. This is a window rather than a one-shot
                // flag because a spinner frame can arrive between the input
                // and its echo; consuming a flag on that frame would leave the
                // echo throttled. Frames outside the window are still held.
                let typed_at_ms = recent_input.load(std::sync::atomic::Ordering::Acquire);
                let echo_due = typed_at_ms != 0
                    && ket_core::now_ms().saturating_sub(typed_at_ms)
                        <= REPAINT_GAP.as_millis() as u64;
                if !echo_due
                    && let Some(wait) = painted.and_then(|at| REPAINT_GAP.checked_sub(at.elapsed()))
                {
                    cx.background_executor().timer(wait).await;
                    frames.borrow_and_update();
                }
                // Only when somebody can see it. The frame clock ticks for
                // every terminal ket holds open — an agent working in another
                // worktree, a shell behind a tab nobody has forward — and a
                // notify here redraws the *whole window*, because
                // `TerminalElement` is an element inside `Shell::render`
                // rather than a view of its own. So a dozen busy worktrees
                // were a dozen 60Hz full-window repaints driving the frame
                // this reader is trying to type into, none of them drawing a
                // pixel anyone could see.
                //
                // Nothing goes stale by skipping it: the grid lives in the
                // `Terminal`, not in what was last painted, so the redraw that
                // comes with bringing the tab forward draws what it holds now.
                // The grid cache catches up on everything a hidden terminal
                // missed: alacritty keeps collecting damage until the element
                // next reads it (see `element::GridCache`).
                let visible = this.update(cx, |shell, _| shell.terminal_is_visible(id));
                match visible {
                    Ok(true) => {
                        // The *pane*, not the window. This is the 60Hz wake,
                        // and routing it through `Shell` is what made every
                        // terminal frame rebuild every sidebar row: notifying
                        // a view marks that view and its ancestors dirty, so
                        // `Shell::render` still runs — but its other children
                        // are `.cached()` and clean, and gpui reuses their
                        // prepaint instead of building them again.
                        painted = Some(std::time::Instant::now());
                        if painting
                            .update(cx, |_: &mut TerminalView, cx| cx.notify())
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(false) => {}
                    Err(_) => return,
                }
                // The signal that actually fires. Waiting for the frame channel
                // to close cannot work: the pane holds the `Terminal`, so it
                // holds a sender, so `changed()` never errors and this loop
                // never ended — which is why a program that died left a live
                // tab wrapped around a dead pty, drawing whatever the grid held
                // when it went. `closed` is set by the parser immediately
                // before its final frame, so the wake that carries it is the
                // one this arrives on.
                if dying.is_closed() {
                    break;
                }
            }

            // The program is gone, and whatever its agent last said about
            // itself went with it — whether or not the tab stays.
            let _ = this.update(cx, |shell, _| shell.clear_agent_status(id));

            // Sleeping keeps the tab and its saved session, even though its
            // process and renderer have ended.
            if this
                .update(cx, |shell, cx| {
                    if shell.asleep_terminals.contains_key(&id) {
                        let wake = shell
                            .asleep_terminals
                            .get_mut(&id)
                            .is_some_and(|asleep| std::mem::take(&mut asleep.wake_requested));
                        shell.terminals.remove(&id);
                        if wake {
                            shell.wake_terminal(id, cx);
                        }
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
            {
                return;
            }

            // Exited cleanly, or lived long enough to have been used: that is
            // someone quitting their shell, and the tab goes.
            let used = opened.elapsed() >= STARTUP_GRACE;
            let clean = dying.exit_status().is_some_and(|status| status.success());
            if used || clean {
                let _ = this.update(cx, |shell, cx| {
                    shell.close_terminal(id);
                    cx.notify();
                });
                return;
            }

            // Died in its first breath. The tab stays — but the grid cannot
            // be trusted to explain why, because a program that used the
            // alternate screen takes its own error message down with it on the
            // way out, which is exactly what `claude --resume` does. So the
            // pane says it in ket's own words.
            tracing::warn!(
                terminal = id.0,
                status = ?dying.exit_status(),
                "program exited during startup; keeping the tab so its output can be read"
            );
            let code = dying
                .exit_status()
                .map(|status| status.to_string())
                .unwrap_or_else(|| "no exit status".to_owned());
            let _ = this.update(cx, |shell, cx| {
                shell.note = Some(format!("{command} exited immediately ({code})").into());
                cx.notify();
            });
        });

        self.terminals.insert(
            id,
            TerminalHandle {
                term,
                view,
                agent,
                launched_with,
                _watch: watch,
                scroll_px: px(0.),
                pending_link: None,
                selecting: false,
                typed_at_ms,
                find: None,
            },
        );
        cx.notify();
    }

    /// Closes a terminal and the tab showing it.
    ///
    /// Both, always: a handle without a tab is a shell nobody can reach, and a
    /// tab without a handle is the pane that could not be got out of.
    pub(crate) fn close_terminal(&mut self, terminal: TerminalId) {
        self.clear_agent_status(terminal);
        self.asleep_terminals.remove(&terminal);
        self.terminal_notes.remove(&terminal);
        self.terminal_keys.remove(&terminal);
        self.sync_asleep_tabs();
        if let Some(handle) = self.terminals.remove(&terminal) {
            handle.term.kill();
        }

        // Which space holds it is not tracked, and does not need to be: a
        // terminal appears in exactly one tab, and there are a handful of
        // spaces.
        for space in self.spaces.values_mut() {
            if let Some((pane, index)) = space.root.find_tab(&TabKind::Terminal(terminal)) {
                space.close_tab(pane, index);
                break;
            }
        }
        self.persist_layout();
    }

    /// The terminal that owns the keyboard: the focused pane's active tab,
    /// when that tab is a terminal. Read from the worktree dialog too, to
    /// decide which project a chord-opened worktree belongs to.
    pub(crate) fn active_terminal_id(&self) -> Option<TerminalId> {
        match self.space()?.active_tab()?.kind {
            TabKind::Terminal(id) => Some(id),
            _ => None,
        }
    }

    /// Routes one keystroke to the active terminal, and says whether it took
    /// the key.
    ///
    /// Handling is `cx.stop_propagation`, which happens below for the keys
    /// that are encoded here. Redrawing is not the caller's business: a
    /// keystroke's effect on screen is the program's echo, which the pane's
    /// frame watch draws when it arrives, and the few things ket changes
    /// itself are drawn where they change — see `terminal_input`. Notifying
    /// the shell on every key used to rebuild the whole window per character,
    /// before anything had changed.
    ///
    /// A plain printable key is not encoded here at all: it arrives a moment
    /// later through the text-input path (see `replace_text_in_range`).
    ///
    /// Which keys are encoded here and which arrive as text is in the module
    /// docs; once a terminal is focused every key belongs to it except the
    /// `Cmd` chords the window keeps, the same way the palette claims every
    /// key while it is open (see `Shell::palette_key`, checked first in
    /// `main.rs`).
    pub(crate) fn terminal_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.active_terminal_id() else {
            return false;
        };
        if self.asleep_terminals.contains_key(&id) {
            if !event.keystroke.modifiers.platform {
                self.wake_terminal(id, cx);
                cx.stop_propagation();
                return true;
            }
            return false;
        }
        if !self.terminals.contains_key(&id) {
            return false;
        }

        let keystroke = &event.keystroke;
        // The find strip first: while its field has the keyboard, the keys
        // are the field's and not the program's.
        if self.terminal_find_key(id, keystroke, window, cx) {
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        if self
            .terminals
            .get(&id)
            .and_then(|handle| handle.find.as_ref())
            .is_some_and(|find| find.query_focused(window, cx))
        {
            // Text, travelling on to the field's input handler.
            return false;
        }
        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "f" if !keystroke.modifiers.shift => {
                    self.open_terminal_find(id, cx);
                    cx.stop_propagation();
                    cx.notify();
                    return true;
                }
                "c" => {
                    self.terminal_copy(id, cx);
                    cx.stop_propagation();
                    return true;
                }
                "v" => {
                    self.terminal_paste(id, cx);
                    cx.stop_propagation();
                    return true;
                }
                _ => return false,
            }
        }

        let mode = self
            .terminals
            .get(&id)
            .map(|handle| mode_of(&handle.term))
            .unwrap_or(TermMode::NONE);
        if let Some(bytes) = keystroke_to_bytes(keystroke, mode) {
            // Read on the way past rather than acted on: the bytes still go to
            // the program unchanged, and what ket takes from them is only that
            // somebody asked the agent to stop. Here rather than in
            // `terminal_input` so a paste cannot trigger it.
            if is_interrupt(&bytes) {
                self.terminal_interrupted(id, &bytes);
                // The sidebar shows the turn as stopped.
                cx.notify();
            }
            self.terminal_input(id, bytes, cx);
            // Handled here; macOS must not also interpret it as text
            // (`Option-b` would otherwise type `∫` after the `Esc b`).
            cx.stop_propagation();
            return true;
        }
        // Anything else that is printable arrives through the text-input
        // path — see `EntityInputHandler for Shell` below. Nothing has
        // changed yet, so nothing is redrawn yet.
        false
    }

    /// Whether this terminal is on screen right now.
    ///
    /// Selected in a visible leaf *and* in the worktree the sidebar has
    /// selected — a space that is not the current one is not drawn at all, so
    /// every terminal in it is as hidden as one behind a tab.
    fn terminal_is_visible(&self, id: TerminalId) -> bool {
        self.space()
            .is_some_and(|space| space.active_terminal_ids().contains(&id))
    }

    /// `Ctrl-C` into a terminal from outside it — the usage popover's Stop:
    /// the byte, and the verdict a typed one raises, so the row stops saying
    /// "thinking" for a turn that has been stopped.
    pub(crate) fn interrupt_terminal(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        self.terminal_interrupted(id, &[0x03]);
        self.terminal_input(id, vec![0x03], cx);
    }

    /// Tells whoever owns the terminal's agent status that a turn was stopped
    /// by hand: `Ctrl-C` is a latched verdict, a bare Escape an unlatched
    /// guess — see [`ket_core::agent_status`]. The store decides whether
    /// there was in fact a turn to stop.
    ///
    /// The pane is named by the key the hook environment carries in
    /// `KET_PANE_KEY`, because that is what its agent's reports were filed
    /// under. The bare number only for an agent launched before keys
    /// existed, which reports that instead.
    fn terminal_interrupted(&mut self, id: TerminalId, bytes: &[u8]) {
        let cancel = bytes == [0x03];
        let hosted = self
            .terminals
            .get(&id)
            .is_some_and(|handle| handle.term.is_hosted());
        let numeric = id.0.to_string();
        let key = self.terminal_keys.get(&id).cloned();
        for pane in key.iter().chain(std::iter::once(&numeric)) {
            // The host runs this terminal, so its store hears the keystroke.
            // The window's store is tried too for local terminals and for a
            // report still draining from its listener during host adoption.
            if hosted {
                ket_core::host::interrupt(pane, cancel);
            }
            let applied = if cancel {
                self.activity
                    .cancel(pane, CancelSource::CtrlC, ket_core::now_ms())
            } else {
                self.activity.escape(pane, ket_core::now_ms())
            };
            if applied || hosted {
                break;
            }
        }
    }

    /// Forgets what a terminal's agent said about itself, now that the
    /// terminal has closed or its program exited. A terminal the host runs is
    /// forgotten by the host, which saw it end first.
    pub(crate) fn clear_agent_status(&mut self, id: TerminalId) {
        if let Some(key) = self.terminal_keys.get(&id) {
            self.activity.clear_pane(key);
        }
        self.activity.clear_pane(&id.0.to_string());
    }

    /// Sends bytes to the program, as though they had been typed.
    ///
    /// Typing snaps a scrolled-back display to the bottom, drops the
    /// selection, and shows the cursor, matching what typing into a terminal
    /// does everywhere else.
    ///
    /// **Redraws nothing of its own accord.** What a keystroke changes on
    /// screen is the program's echo, and that arrives through the pane's frame
    /// watch a moment later and draws then. Redrawing here as well cost a
    /// whole-window rebuild per character in which nothing had changed yet —
    /// the pane is a child of the shell, so even notifying the pane alone
    /// rebuilds every view above it. Only a change ket makes itself, which no
    /// echo will carry, is drawn now: a scrolled-back view snapping to the
    /// bottom, or a selection going away.
    /// Says whether the terminal took them: not when it has closed.
    fn terminal_input(&mut self, id: TerminalId, bytes: Vec<u8>, cx: &mut Context<Self>) -> bool {
        if self.asleep_terminals.contains_key(&id) {
            self.wake_terminal(id, cx);
            return true;
        }
        let Some(handle) = self.terminals.get_mut(&id) else {
            return false;
        };
        let moved = handle.term.with_term(|term| {
            let scrolled = term.grid().display_offset() > 0;
            if scrolled {
                term.scroll_display(Scroll::Bottom);
            }
            let selected = term.selection.take().is_some();
            scrolled || selected
        });
        handle.view.update(cx, |view, cx| {
            view.blink.visible = true;
            if moved {
                cx.notify();
            }
        });
        handle
            .typed_at_ms
            .store(ket_core::now_ms(), std::sync::atomic::Ordering::Release);
        handle.term.send_input(bytes).is_ok()
    }

    /// `Cmd-K`: empties the active pane, if it is a shell rather than an
    /// agent. Returns whether it did.
    ///
    /// Reported rather than assumed, because the chord is the command
    /// palette's everywhere else in the window and the caller has to know
    /// whether to fall back to it. **An agent's pane is exempt**: its
    /// scrollback is the conversation, and a chord that quietly destroyed a
    /// session's history — which no `Cmd-Z` brings back — is not a clear, it
    /// is a loss. `Ctrl-L` still reaches an agent that implements its own.
    pub(crate) fn clear_active_terminal(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.active_terminal_id() else {
            return false;
        };
        let Some(handle) = self.terminals.get(&id) else {
            return false;
        };
        if handle.agent.is_some() {
            return false;
        }

        handle.term.clear();
        // Then the shell is asked to draw its prompt again, so the pane is
        // left ready rather than blank with a cursor stranded partway down
        // it. Only when nothing else is running: a form feed sent into
        // somebody's `less` or `vim` is a keystroke they did not type, and
        // those repaint themselves anyway.
        if matches!(handle.term.foreground(), Some(Foreground::Shell)) {
            let _ = handle.term.send_input(vec![0x0c]);
        }
        cx.notify();
        true
    }

    /// What is selected in the terminal `id`, if anything is.
    pub(crate) fn terminal_selection(&self, id: TerminalId) -> Option<String> {
        self.terminals
            .get(&id)?
            .term
            .with_term(|term| term.selection_to_string())
            .filter(|text| !text.trim().is_empty())
    }

    /// `Cmd-C`: the selection to the clipboard. Nothing selected, nothing
    /// happens — the interrupt lives on `Ctrl-C`, as everywhere on macOS.
    pub(crate) fn terminal_copy(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        let Some(handle) = self.terminals.get(&id) else {
            return;
        };
        if let Some(text) = handle.term.with_term(|term| term.selection_to_string()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// `Cmd-V`: the clipboard to the program, bracketed when it asked for
    /// that so a multi-line paste is not executed line by line, and with
    /// newlines as carriage returns, which is what the Enter key sends.
    pub(crate) fn terminal_paste(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(handle) = self.terminals.get(&id) else {
            return;
        };
        let bytes = paste_bytes(&handle.term, &text);
        self.terminal_input(id, bytes, cx);
    }

    /// Whether any of `worktree`'s tabs is still opening its terminal. One
    /// that failed to open keeps its note too, and counts until it is closed.
    pub(crate) fn terminals_starting(&self, worktree: &WorktreeId) -> bool {
        self.spaces.get(worktree).is_some_and(|space| {
            space
                .terminal_ids()
                .iter()
                .any(|id| self.terminal_notes.contains_key(id) && !self.terminals.contains_key(id))
        })
    }

    /// The terminal running `worktree`'s agent, if one is open: the tab in
    /// front when that is one, and otherwise the first the space holds.
    pub(crate) fn agent_terminal(&self, worktree: &WorktreeId) -> Option<TerminalId> {
        let space = self.spaces.get(worktree)?;
        let agent = |tab: &Tab| match tab.kind {
            TabKind::Terminal(id)
                if self
                    .terminals
                    .get(&id)
                    .is_some_and(|handle| handle.agent.is_some()) =>
            {
                Some(id)
            }
            _ => None,
        };
        space
            .active_tab()
            .and_then(agent)
            .or_else(|| space.all_tabs().into_iter().find_map(agent))
    }

    /// Pastes `text` into a terminal without pressing Enter: typed into the
    /// agent's input for the person to finish and send.
    pub(crate) fn paste_into_terminal(
        &mut self,
        id: TerminalId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.terminals.get(&id) else {
            return;
        };
        let bytes = paste_bytes(&handle.term, text);
        self.terminal_input(id, bytes, cx);
    }

    /// Pastes `text` into a terminal and presses Enter, as though somebody
    /// had pasted a prompt into the agent's input and submitted it.
    pub(crate) fn submit_to_terminal(
        &mut self,
        id: TerminalId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.submit_to_terminal_then(id, text, cx, |_, _, _| {});
    }

    /// [`Shell::submit_to_terminal`], then `done` once Enter has gone in —
    /// told `false` when the terminal closed before the paste or the Enter
    /// reached it, so the prompt was never submitted. Not called at all if
    /// the window goes first.
    pub(crate) fn submit_to_terminal_then(
        &mut self,
        id: TerminalId,
        text: &str,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Shell, bool, &mut Context<Shell>) + 'static,
    ) {
        let Some(handle) = self.terminals.get(&id) else {
            done(self, false, cx);
            return;
        };
        let bytes = paste_bytes(&handle.term, text);
        if !self.terminal_input(id, bytes, cx) {
            done(self, false, cx);
            return;
        }
        // Enter as a write of its own, a beat later. Sent in the same write,
        // a TUI still taking the paste in can read the carriage return as one
        // more line of it rather than as the submit.
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            cx.background_executor().timer(SUBMIT_DELAY).await;
            let _ = this.update(cx, |shell, cx| {
                let entered = shell.terminal_input(id, b"\r".to_vec(), cx);
                done(shell, entered, cx);
            });
        })
        .detach();
    }

    /// Hands the images among `paths` to the agent in terminal `id`, each as
    /// a bracketed paste of its path — what a drop does, see
    /// [`Self::terminal_drop_paths`]. Returns the paths that could not go
    /// that way, for the caller to name in the prompt instead.
    /// How many image chips — Claude Code's and Codex's "[Image #1]",
    /// OpenCode's "[Image 1]" — the bottom of `id`'s live screen shows: where
    /// an agent's input box is, and where an image pasted into it appears
    /// once the agent has read it. Pasting the prompt before then can lose
    /// its Enter, so a sender waits for the count to rise.
    pub(crate) fn images_shown(&self, id: TerminalId) -> usize {
        let Some(handle) = self.terminals.get(&id) else {
            return 0;
        };
        handle
            .term
            .with_term(|term| ket_core::attachments::images_shown(term))
    }

    pub(crate) fn attach_to_terminal(
        &mut self,
        id: TerminalId,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) -> Vec<std::path::PathBuf> {
        let Some(handle) = self.terminals.get(&id) else {
            return paths.to_vec();
        };
        let bracketed = mode_of(&handle.term).contains(TermMode::BRACKETED_PASTE);
        let (bytes, _, left) = ket_core::attachments::image_pastes(paths, bracketed);
        if !bytes.is_empty() {
            self.terminal_input(id, bytes, cx);
        }
        left
    }

    /// A file dragged from the Finder and dropped on the pane.
    ///
    /// There is no byte for "here is a file" on a pty — only what a shell
    /// would read as typed — so this does what Terminal.app and iTerm2 do:
    /// type each path, quoted, at the cursor. Nothing is submitted; the
    /// paths sit there until Return is pressed, same as a real drop.
    ///
    /// Images are the exception, because a dropped screenshot is almost
    /// never an argument to a command — it is something to show the agent
    /// running in the pane. Claude Code and Codex both pick an image up from
    /// a *bracketed* paste of its path and attach the file itself; a path
    /// typed key by key is just a name on the line, which is what dropping
    /// one here used to produce. So an image goes in bracketed and unquoted:
    /// the quotes would land inside the pasted text and break the
    /// file-exists check those tools run on it.
    ///
    /// Unquoted only holds while the path has nothing a shell would act on,
    /// since the same drop can land at a plain prompt — a path with a `$` or
    /// a `;` in it goes back to being quoted, and is then a name rather than
    /// an attachment. Bracketed paste is also what decides it: a program
    /// that never asked for it would be handed the escapes as literal text.
    fn terminal_drop_paths(
        &mut self,
        id: TerminalId,
        paths: &ExternalPaths,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.terminals.get(&id) else {
            return;
        };
        let bracketed = mode_of(&handle.term).contains(TermMode::BRACKETED_PASTE);
        let paths = paths.paths();
        // A named function rather than a closure: the borrow it hands back
        // outlives the call, which closure lifetime elision will not express.
        fn attach(bracketed: bool, path: &std::path::Path) -> Option<&str> {
            bracketed.then(|| image_paste(path)).flatten()
        }

        let mut bytes = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let next = paths.get(i + 1);
            match attach(bracketed, path) {
                Some(raw) => {
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(raw.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    // A paste is self-delimiting, so two images need nothing
                    // between them — and a space there would be typed into
                    // the agent's prompt. A quoted path following one does
                    // need the gap.
                    if next.is_some_and(|next| attach(bracketed, next).is_none()) {
                        bytes.push(b' ');
                    }
                }
                None => {
                    bytes.extend_from_slice(shell_quote(path).as_bytes());
                    if next.is_some() {
                        bytes.push(b' ');
                    }
                }
            }
        }
        if bytes.is_empty() {
            return;
        }
        self.terminal_input(id, bytes, cx);
    }

    /// Hands every pane the few things it draws with but cannot own: the
    /// colours, the faces, and whether it is the keyboard target.
    ///
    /// Pushed rather than pulled, and only when it actually differs, because
    /// a pane that read any of this off `Shell` would be re-rendered whenever
    /// anything in the window changed — see `TerminalView`. Called once a
    /// frame from `Shell::render`; the comparison is what keeps it from
    /// notifying, and so from defeating the cache it exists to protect.
    pub(crate) fn sync_terminal_views(&mut self, cx: &mut Context<Self>) {
        let theme = self.theme;
        let family = self.font_family.clone();
        let font_size = px(f32::from(self.theme_config.code_font_size));
        // Not while a modal is up. The pane registers the window's text input
        // against the shell's own focus, which a dialog drawn by the shell
        // does not take — so a focused terminal behind the quick prompt got
        // every character typed into it as well.
        let modal = self.modal_open();
        let focused = self
            .space()
            .filter(|_| !modal)
            .and_then(|space| space.active_tab())
            .and_then(|tab| match tab.kind {
                TabKind::Terminal(id) => Some(id),
                _ => None,
            });

        let views: Vec<(TerminalId, Entity<TerminalView>)> = self
            .terminals
            .iter()
            .map(|(id, handle)| (*id, handle.view.clone()))
            .collect();

        for (id, view) in views {
            view.update(cx, |view, cx| {
                let is_focused = focused == Some(id);
                if view.theme == theme
                    && view.family == family
                    && view.font_size == font_size
                    && view.focused == is_focused
                {
                    return;
                }
                view.theme = theme;
                view.family = family.clone();
                view.font_size = font_size;
                view.focused = is_focused;
                cx.notify();
            });
        }
    }

    /// The view behind a terminal, for the handlers that reach into it.
    ///
    /// The element writes the grid cache and the geometry straight onto the
    /// view now, so the three `Shell` methods that used to carry them —
    /// `terminal_grid`, `terminal_grid_store`, `terminal_geometry_changed` —
    /// are gone rather than forwarded: a pane's drawing state passing through
    /// the shell is exactly what made the shell an entity every frame had
    /// read, and so an entity every notify invalidated.
    fn terminal_view(&self, id: TerminalId) -> Option<&Entity<TerminalView>> {
        self.terminals.get(&id).map(|handle| &handle.view)
    }

    /// A press inside the grid: a mouse report if the program wants them,
    /// otherwise the start of a selection, of a click on a link, or of
    /// both — a plain press on a link anchors a selection too, so a drag
    /// off it can still take the text.
    pub(crate) fn terminal_mouse_down(
        &mut self,
        id: TerminalId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self
            .terminal_view(id)
            .map(|view| view.read(cx))
            .and_then(|view| view.geometry.filter(Geometry::is_valid))
        else {
            return;
        };
        let Some(view) = self.terminal_view(id).cloned() else {
            return;
        };
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        let (cell, side) = geometry.cell_at(event.position);
        let (mode, display_offset) = handle
            .term
            .with_term(|term| (*term.mode(), term.grid().display_offset()));

        if event.button == MouseButton::Left
            && let Some(link) = link_at(&handle.term, cell, display_offset)
        {
            handle.pending_link = Some(link);
            // A plain click falls through to the terminal or selection,
            // while the release opens the link if it did not become a drag.
            if event.modifiers.secondary() {
                return;
            }
        }

        if wants_mouse(mode, &event.modifiers) {
            if let Some(report) = button_report(cell, event.button, &event.modifiers, true, mode) {
                let _ = handle.term.send_input(report);
            }
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }
        let kind = match event.click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => return,
        };
        let point = cell.to_grid(display_offset);
        handle.term.with_term(|term| {
            match term.selection.as_mut() {
                // Shift-click extends what is there; without a selection it
                // just starts one, same as a plain click.
                Some(selection) if kind == SelectionType::Simple && event.modifiers.shift => {
                    selection.update(point, side);
                }
                _ => term.selection = Some(Selection::new(kind, point, side)),
            }
        });
        handle.selecting = true;
        view.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// Pointer movement: extends a drag-selection (scrolling when the
    /// pointer is past the grid's edge), reports motion to a program that
    /// asked for it, and tracks the link under a `Cmd`-hover.
    pub(crate) fn terminal_mouse_move(
        &mut self,
        id: TerminalId,
        event: &MouseMoveEvent,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self
            .terminal_view(id)
            .map(|view| view.read(cx))
            .and_then(|view| view.geometry.filter(Geometry::is_valid))
        else {
            return;
        };
        let Some(view) = self.terminal_view(id).cloned() else {
            return;
        };
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        let (cell, side) = geometry.cell_at(event.position);
        let (mode, display_offset) = handle
            .term
            .with_term(|term| (*term.mode(), term.grid().display_offset()));

        if handle.pending_link.is_some() {
            // A press on a link that wanders off it is not a click on it.
            if link_at(&handle.term, cell, display_offset) != handle.pending_link {
                handle.pending_link = None;
            }
            // A plain press also anchored a selection; let the drag below
            // carry it on. A `Cmd`-press did not, and is done here.
            if !handle.selecting {
                return;
            }
        }

        if handle.selecting && event.pressed_button == Some(MouseButton::Left) {
            if wants_mouse(mode, &event.modifiers) {
                return;
            }
            let row = geometry.row_at(event.position);
            let scroll = if row < 0 {
                1
            } else if row >= geometry.rows as i32 {
                -1
            } else {
                0
            };
            handle.term.with_term(|term| {
                if scroll != 0 {
                    term.scroll_display(Scroll::Delta(scroll));
                }
                let point = cell.to_grid(term.grid().display_offset());
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
            });
            view.update(cx, |_, cx| cx.notify());
            cx.notify();
            return;
        }

        if !hovered {
            if view.update(cx, |view, cx| {
                let had = view.hovered_link.take().is_some();
                if had {
                    cx.notify();
                }
                had
            }) {
                cx.notify();
            }
            return;
        }

        if wants_mouse(mode, &event.modifiers)
            && let Some(report) = motion_report(cell, event.pressed_button, &event.modifiers, mode)
        {
            let _ = handle.term.send_input(report);
        }

        let link = link_at(&handle.term, cell, display_offset);
        view.update(cx, |view, cx| {
            if link != view.hovered_link {
                view.hovered_link = link;
                cx.notify();
            }
        });
    }

    /// A release: opens a clicked link, reports the release to a
    /// program that wants it, or ends a selection — dropping one that never
    /// grew past its anchor, so a plain click leaves nothing behind.
    pub(crate) fn terminal_mouse_up(
        &mut self,
        id: TerminalId,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self
            .terminal_view(id)
            .map(|view| view.read(cx))
            .and_then(|view| view.geometry.filter(Geometry::is_valid))
        else {
            return;
        };
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        let (cell, _) = geometry.cell_at(event.position);
        let (mode, display_offset) = handle
            .term
            .with_term(|term| (*term.mode(), term.grid().display_offset()));

        // A press that grew into a selection wanted the text, not the
        // page — so only a release still on the link, with nothing
        // selected, opens it.
        if let Some(link) = handle.pending_link.take() {
            let dragged = handle
                .term
                .with_term(|term| term.selection.as_ref().is_some_and(|s| !s.is_empty()));
            if !dragged && link_at(&handle.term, cell, display_offset).as_ref() == Some(&link) {
                handle.term.with_term(|term| term.selection = None);
                handle.selecting = false;
                let pane = self.space().map(|space| {
                    space
                        .root
                        .find_tab(&TabKind::Terminal(id))
                        .map_or(space.focused, |(pane, _)| pane)
                });
                match pane {
                    Some(pane) => self.open_link_menu(link.uri, pane, event.position),
                    None => cx.open_url(&link.uri),
                }
                cx.notify();
                return;
            }
        }

        if wants_mouse(mode, &event.modifiers) {
            if let Some(report) = button_report(cell, event.button, &event.modifiers, false, mode) {
                let _ = handle.term.send_input(report);
            }
            return;
        }

        if event.button == MouseButton::Left && handle.selecting {
            handle.selecting = false;
            handle.term.with_term(|term| {
                if term.selection.as_ref().is_some_and(Selection::is_empty) {
                    term.selection = None;
                }
            });
            cx.notify();
        }
    }

    /// Wheel and trackpad scrolling, accumulated in pixels so a trackpad's
    /// small deltas add up to lines rather than rounding away.
    ///
    /// Three destinations, in order of who asked: a program that wants
    /// mouse events gets wheel reports; a full-screen program that asked
    /// for alternate scrolling gets arrow keys; otherwise the display moves
    /// through scrollback.
    pub(crate) fn terminal_scroll(
        &mut self,
        id: TerminalId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.terminal_view(id).cloned() else {
            return;
        };
        let Some(geometry) = view.read(cx).geometry.filter(Geometry::is_valid) else {
            return;
        };
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        match event.touch_phase {
            TouchPhase::Started => {
                handle.scroll_px = px(0.);
                return;
            }
            TouchPhase::Ended => return,
            TouchPhase::Moved => {}
        }

        let line_height = f32::from(geometry.line_height);
        let before = (f32::from(handle.scroll_px) / line_height) as i32;
        handle.scroll_px += event.delta.pixel_delta(geometry.line_height).y;
        let after = (f32::from(handle.scroll_px) / line_height) as i32;
        // Keep the remainder small so a change of direction responds at once.
        let height = line_height * geometry.rows as f32;
        handle.scroll_px = px(f32::from(handle.scroll_px) % height);
        let lines = after - before;
        if lines == 0 {
            return;
        }

        let mode = mode_of(&handle.term);
        if wants_mouse(mode, &event.modifiers) {
            let (cell, _) = geometry.cell_at(event.position);
            for report in wheel_reports(cell, lines, &event.modifiers, mode) {
                let _ = handle.term.send_input(report);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !event.modifiers.shift
        {
            let _ = handle.term.send_input(alternate_scroll(lines, mode));
        } else {
            handle
                .term
                .with_term(|term| term.scroll_display(Scroll::Delta(lines)));
            view.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// The terminal pane for the selected worktree, in pane `pane`.
    ///
    /// The pane is a view of its own now, so this hands back the view rather
    /// than building anything — and hands it back **cached**, which is the
    /// half that does the work. Without `.cached` gpui re-renders and
    /// re-prepaints a child view on every window draw regardless of who
    /// notified (`Entity<V>`'s own `Element` impl has no dirty check at all);
    /// with it, a frame the pane did not cause reuses last frame's prepaint
    /// and the grid is not shaped again. See `TerminalView`.
    ///
    /// The style is stated here because a cached view takes its layout from
    /// the embed site rather than from its own render.
    pub(crate) fn terminal_pane(
        &self,
        id: TerminalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(asleep) = self.asleep_terminals.get(&id) {
            let since =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(asleep.since_ms as i64)
                    .map_or_else(
                        || "earlier".to_owned(),
                        |time| {
                            time.with_timezone(&chrono::Local)
                                .format("%H:%M")
                                .to_string()
                        },
                    );
            let message = if asleep.freed_bytes > 0 {
                format!(
                    "Asleep since {since} · freed {} MB",
                    asleep.freed_bytes / 1_000_000
                )
            } else {
                format!("Asleep since {since}")
            };
            return div()
                .flex()
                .flex_col()
                .size_full()
                .justify_center()
                .items_center()
                .gap_3()
                .text_color(paint(self.theme.text.dim))
                .child(message)
                .when_some(self.terminal_notes.get(&id), |element, note| {
                    element.child(note.clone())
                })
                .child(
                    crate::ui::button::button(("wake-terminal", id.0), "Wake")
                        .render(&self.theme)
                        .on_click(cx.listener(move |this, _, _, cx| this.wake_terminal(id, cx))),
                )
                .into_any_element();
        }
        let Some(view) = self.terminal_view(id) else {
            let note = self
                .terminal_notes
                .get(&id)
                .map_or("Terminal unavailable", |note| note.as_ref());
            return terminal_note(&self.theme, note);
        };

        // The find bar, or else the offer to remember a permission answer:
        // one strip at a time, and finding is what the person asked for.
        let bar = self
            .terminal_find_bar(id, window, cx)
            .or_else(|| self.rule_offer_strip(id, cx));
        let mut style = gpui::StyleRefinement::default();
        style.size.width = Some(gpui::relative(1.).into());
        match bar {
            None => {
                style.size.height = Some(gpui::relative(1.).into());
                gpui::AnyView::from(view.clone())
                    .cached(style)
                    .into_any_element()
            }
            // The strip takes its height off the top, and the grid, resized
            // to what is left, keeps every row it had in view.
            Some(bar) => {
                style.flex_grow = Some(1.);
                style.min_size.height = Some(px(0.).into());
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(bar)
                    .child(gpui::AnyView::from(view.clone()).cached(style))
                    .into_any_element()
            }
        }
    }
}

impl TerminalView {
    /// Starts or stops the blink timer to match what the program asked for.
    ///
    /// A steady cursor costs nothing: the timer exists only while a program
    /// has requested blinking and the terminal is the keyboard target.
    pub(crate) fn set_blinking(&mut self, on: bool, cx: &mut Context<Self>) {
        let running = self.blink.task.is_some();
        match (on, running) {
            (true, false) => {
                // Notifies the pane rather than the window. A cursor blinking
                // in one terminal used to redraw every sidebar row twice a
                // second for the rest of the session.
                let blinking = cx.entity().downgrade();
                let task = cx.spawn(
                    async move |_: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
                        loop {
                            cx.background_executor().timer(BLINK_INTERVAL).await;
                            let alive = blinking.update(cx, |view: &mut TerminalView, cx| {
                                view.blink.visible = !view.blink.visible;
                                cx.notify();
                            });
                            if alive.is_err() {
                                break;
                            }
                        }
                    },
                );
                self.blink.task = Some(task);
            }
            (false, true) => {
                self.blink.task = None;
                self.blink.visible = true;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let id = self.id;

        let Some(shell) = self.shell.upgrade() else {
            // The window is going. Nothing to draw it into.
            return div().into_any_element();
        };
        let element = TerminalElement::new(
            Owners {
                shell,
                view: cx.entity(),
            },
            id,
            self.term.clone(),
            self.focus.clone(),
            GridFont {
                family: self.family.clone(),
                size: self.font_size,
            },
            theme,
            FrameState {
                focused: self.focused,
                blink_visible: self.blink.visible,
                marked: self.marked.clone(),
                hovered_link: self.hovered_link.is_some(),
            },
        );

        // A shell you quit takes its tab with it, the way every other terminal
        // on the platform does, and needs no band. A program that *failed to
        // start* is the opposite case and the band exists for it alone: it
        // died in under [`STARTUP_GRACE`], so the tab was kept — and the grid
        // behind it is very likely blank, because a program that drew on the
        // alternate screen took its own error message down with it on the way
        // out. `claude --resume` on a session already running does exactly
        // that: it prints what to do instead, then wipes it.
        let failed = self.term.is_closed()
            && !self
                .term
                .exit_status()
                .is_some_and(|status| status.success());
        let note = self.exit_note.clone();
        let shell = self.shell.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .children(failed.then(|| {
                div()
                    .flex_none()
                    .w_full()
                    .px(px(12.0))
                    .py(px(7.0))
                    .bg(paint(theme.elevated))
                    .border_b_1()
                    .border_color(paint(theme.status.failed))
                    .text_size(px(12.0))
                    .text_color(paint(theme.text.primary))
                    .child(note.unwrap_or_else(|| "the program exited immediately".into()))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .on_drop::<ExternalPaths>(move |paths, _window, cx| {
                        let _ = shell.update(cx, |shell, cx| {
                            shell.terminal_drop_paths(id, paths, cx);
                        });
                    })
                    .child(element),
            )
            .into_any_element()
    }
}

/// Text input from the window's input method, routed to the active
/// terminal. See the module docs for why plain text arrives this way.
impl EntityInputHandler for Shell {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // A terminal has no selection the input method can see, but it has
        // a cursor, and reporting *some* range is what makes the candidate
        // window appear next to it.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let id = self.active_terminal_id()?;
        let marked = self.terminal_view(id)?.read(_cx).marked.clone()?;
        Some(0..marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_terminal_id()
            && let Some(view) = self.terminal_view(id).cloned()
        {
            view.update(cx, |view, cx| {
                view.marked = None;
                cx.notify();
            });
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.active_terminal_id() else {
            return;
        };
        // Only a composition being committed has anything to take off the
        // screen now; the text itself is drawn by its echo — see
        // `terminal_input`.
        if let Some(view) = self.terminal_view(id).cloned() {
            view.update(cx, |view, cx| {
                if view.marked.take().is_some() {
                    cx.notify();
                }
            });
        }
        self.terminal_input(id, text.as_bytes().to_vec(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.active_terminal_id()
            && let Some(view) = self.terminal_view(id).cloned()
        {
            view.update(cx, |view, cx| {
                view.marked = Some(new_text.to_owned());
                cx.notify();
            });
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let id = self.active_terminal_id()?;
        let view = self.terminal_view(id)?.read(_cx);
        let mut bounds = view.cursor_bounds?;
        if let Some(geometry) = view.geometry {
            bounds.origin.x += geometry.cell_width * range_utf16.start as f32;
        }
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// The link under `cell`, if any: an OSC 8 hyperlink the program attached
/// to the cell, or a URL spelled out in the row's text.
fn link_at(term: &Terminal, cell: Cell, display_offset: usize) -> Option<Link> {
    term.with_term(|term| {
        let grid = term.grid();
        let cols = grid.columns();
        if cell.col >= cols || cell.row >= grid.screen_lines() {
            return None;
        }
        let point = cell.to_grid(display_offset);
        let at = |col: usize| &grid[GridPoint::new(point.line, Column(col))];

        if let Some(link) = at(cell.col).hyperlink() {
            let same = |col: usize| {
                at(col)
                    .hyperlink()
                    .is_some_and(|other| other.uri() == link.uri())
            };
            let mut start = cell.col;
            while start > 0 && same(start - 1) {
                start -= 1;
            }
            let mut end = cell.col + 1;
            while end < cols && same(end) {
                end += 1;
            }
            return Some(Link {
                uri: link.uri().to_owned(),
                row: cell.row,
                cols: start..end,
            });
        }

        let chars: Vec<char> = (0..cols).map(|col| at(col).c).collect();
        url_span(&chars, cell.col).map(|span| Link {
            uri: chars[span.clone()].iter().collect(),
            row: cell.row,
            cols: span,
        })
    })
}

/// Every link on one row of the visible screen, as the columns it covers.
///
/// The element underlines these, so a link looks clickable before the
/// pointer reaches it. Same two sources as [`link_at`], and an OSC 8
/// hyperlink wins over text that merely looks like a URL underneath it —
/// the program said where those cells point.
///
/// One row rather than the screen: the element re-walks only the rows the
/// program changed, and a URL here never spans rows, so a row's links are
/// a function of that row alone.
fn row_link_spans<T: EventListener>(
    term: &Term<T>,
    display_offset: usize,
    row: usize,
) -> Vec<Range<usize>> {
    let grid = term.grid();
    let cols = grid.columns();
    let line = Cell { row, col: 0 }.to_grid(display_offset).line;
    let at = |col: usize| &grid[GridPoint::new(line, Column(col))];

    let mut spans: Vec<Range<usize>> = Vec::new();
    let mut col = 0;
    while col < cols {
        let Some(uri) = at(col).hyperlink().map(|link| link.uri().to_owned()) else {
            col += 1;
            continue;
        };
        let start = col;
        col += 1;
        while col < cols && at(col).hyperlink().is_some_and(|next| next.uri() == uri) {
            col += 1;
        }
        spans.push(start..col);
    }

    let chars: Vec<char> = (0..cols).map(|col| at(col).c).collect();
    for span in row_url_spans(&chars) {
        let overlaps = |other: &Range<usize>| span.start < other.end && other.start < span.end;
        if !spans.iter().any(overlaps) {
            spans.push(span);
        }
    }
    spans
}

/// Every URL spelled out in a row of text, left to right.
fn row_url_spans(chars: &[char]) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut col = 0;
    while col < chars.len() {
        if chars[col].is_whitespace() {
            col += 1;
            continue;
        }
        let start = col;
        while col < chars.len() && !chars[col].is_whitespace() {
            col += 1;
        }
        if let Some(span) = url_token(chars, start, col) {
            spans.push(span);
        }
    }
    spans
}

/// The columns of a URL in `chars` that covers column `col`, if that column
/// is inside one.
fn url_span(chars: &[char], col: usize) -> Option<Range<usize>> {
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col + 1;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    url_token(chars, start, end).filter(|span| span.contains(&col))
}

/// The URL inside the whitespace-delimited token `chars[start..end]`, if it
/// is one: a token starting with an `http`, `https` or `file` scheme, minus
/// the punctuation prose tends to wrap around one — the full stop at the end
/// of a sentence, the brackets or quotes of a parenthetical.
fn url_token(chars: &[char], start: usize, end: usize) -> Option<Range<usize>> {
    let (mut start, mut end) = (start, end);
    while start < end && matches!(chars[start], '(' | '[' | '<' | '\'' | '"') {
        start += 1;
    }
    while end > start
        && matches!(
            chars[end - 1],
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '>' | '\'' | '"'
        )
    {
        end -= 1;
    }
    let token: String = chars[start..end].iter().collect();
    let is_url = ["http://", "https://", "file://"]
        .iter()
        .any(|scheme| token.starts_with(scheme) && token.len() > scheme.len());
    is_url.then_some(start..end)
}

/// The path git checked out for `id`, if that worktree still exists.
///
/// Reopens the workspace rather than threading a path through the sidebar's
/// own tree, which does not keep one — see `tree::WorktreeNode`. Spike-grade
/// in the same way `tree.rs`'s own click handler already is; a shell that
/// held the workspace for its lifetime would not need to do this per call.
pub(crate) fn worktree_path(id: &WorktreeId) -> Option<PathBuf> {
    let workspace = Workspace::open().ok()?;
    workspace
        .worktrees(None)
        .ok()?
        .into_iter()
        .find(|worktree| &worktree.id == id)
        .map(|worktree| worktree.path)
}

/// A centred, dimmed line where the grid would be — no worktree selected, or
/// none opened a terminal yet.
/// Quotes a path the way a POSIX shell expects, single-quoted with any
/// embedded quote broken out and escaped — safe regardless of spaces or
/// other shell metacharacters in the name.
fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Whether `path` reaches an agent as an image of its own when it is
/// handed one — see [`Shell::attach_to_terminal`] — rather than as a path in
/// the prompt.
pub(crate) fn pastes_as_image(path: &std::path::Path) -> bool {
    ket_core::attachments::pastes_as_image(path)
}

/// The path as it should be pasted to attach it, or `None` if it is not an
/// image or cannot go in unquoted — see [`Shell::terminal_drop_paths`] and
/// [`ket_core::attachments::image_paste`].
fn image_paste(path: &std::path::Path) -> Option<&str> {
    ket_core::attachments::image_paste(path)
}

fn terminal_note(theme: &Theme, message: &str) -> AnyElement {
    div()
        .flex()
        .size_full()
        .justify_center()
        .items_center()
        .p_6()
        .text_color(paint(theme.text.dim))
        // Its own box, so a long error wraps inside the pane instead of
        // running off both edges of a centred single line.
        .child(
            div()
                .min_w_0()
                .max_w(px(720.))
                .text_center()
                .whitespace_normal()
                .child(message.to_owned()),
        )
        .into_any_element()
}

/// Every agent Settings has enabled and this machine can actually run, in
/// name order — [`Shell::runnable_agents`], for a caller with no shell to
/// hand, such as a background task.
pub(crate) fn runnable_specs() -> Vec<AgentSpec> {
    let Ok(workspace) = Workspace::open() else {
        return Vec::new();
    };
    let catalogue = Catalogue::detect(workspace.config(), &ShellProber);
    catalogue
        .runnable()
        .map(|entry| entry.spec.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_sleep_pane() -> PaneAgentStatus {
        let mut activity = ket_core::activity::Tracker::new(["claude".to_owned()]);
        let report = serde_json::from_value(serde_json::json!({
            "agent": "claude",
            "event": "stop",
            "worktree": "wt",
            "pane": "wt|claude|a",
            "session": "saved-session",
            "mutatingGit": false,
            "atMs": 1000,
        }))
        .unwrap();
        activity.report(&report, |_| true);
        activity.pane_status("wt|claude|a").unwrap().clone()
    }

    #[test]
    fn manual_sleep_allows_the_selected_idle_agent_but_automatic_sleep_protects_it() {
        let pane = idle_sleep_pane();
        let session = |trigger, focused| {
            sleep_session(trigger, focused, "claude", Some(&pane), Some(Signal::Quiet))
        };
        assert_eq!(session(SleepTrigger::Manual, true), Ok("saved-session"));
        assert_eq!(session(SleepTrigger::Manual, false), Ok("saved-session"));
        assert!(session(SleepTrigger::Automatic, true).is_err());
        assert_eq!(session(SleepTrigger::Automatic, false), Ok("saved-session"));
    }

    #[test]
    fn manual_sleep_still_requires_a_matching_idle_agent_and_a_saved_session() {
        fn check(pane: Option<&PaneAgentStatus>, signal: Option<Signal>) -> Result<&str, String> {
            sleep_session(SleepTrigger::Manual, true, "claude", pane, signal)
        }

        let mut pane = idle_sleep_pane();
        assert!(check(None, Some(Signal::Quiet)).is_err());
        assert!(check(Some(&pane), None).is_err());
        // A quiet lead can still have a working or blocked subagent.
        for signal in [Signal::Working, Signal::Blocked] {
            assert!(check(Some(&pane), Some(signal)).is_err());
        }
        pane.agent = "codex".to_owned();
        assert!(check(Some(&pane), Some(Signal::Quiet)).is_err());
        pane.agent = "claude".to_owned();
        for state in [
            ket_core::event::AgentState::Thinking,
            ket_core::event::AgentState::AwaitingPermission,
        ] {
            pane.state = state;
            assert!(check(Some(&pane), Some(Signal::Quiet)).is_err());
        }
        pane.state = ket_core::event::AgentState::Idle;
        pane.session = None;
        assert!(check(Some(&pane), Some(Signal::Quiet)).is_err());
    }

    #[test]
    fn finds_the_cli_in_app_and_development_layouts() {
        assert_eq!(
            cli_path_for(Path::new("/Applications/ket.app/Contents/MacOS/ket")),
            Some(PathBuf::from(
                "/Applications/ket.app/Contents/Resources/bin/ket"
            ))
        );
        assert_eq!(
            cli_path_for(Path::new("/checkout/target/debug/ket-ui")),
            Some(PathBuf::from("/checkout/target/debug/ket"))
        );
    }

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn a_url_is_found_from_any_column_inside_it() {
        let row = chars("see https://example.com/path for details");
        let span = url_span(&row, 4);
        assert_eq!(span, Some(4..28));
        assert_eq!(url_span(&row, 20), Some(4..28));
    }

    #[test]
    fn trailing_prose_punctuation_is_not_part_of_the_url() {
        let row = chars("(https://example.com).");
        assert_eq!(url_span(&row, 5), Some(1..20));
        let uri: String = row[1..20].iter().collect();
        assert_eq!(uri, "https://example.com");
        // The wrapping punctuation itself is not part of the link.
        assert_eq!(url_span(&row, 0), None);
        assert_eq!(url_span(&row, 21), None);
    }

    #[test]
    fn a_column_outside_any_url_finds_nothing() {
        let row = chars("see https://example.com for details");
        assert_eq!(url_span(&row, 0), None);
        assert_eq!(url_span(&row, 3), None);
        assert_eq!(url_span(&row, 30), None);
    }

    #[test]
    fn a_bare_scheme_is_not_a_link() {
        let row = chars("https://");
        assert_eq!(url_span(&row, 2), None);
    }
}
