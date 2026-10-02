//! The shell's icons, drawn rather than typed.
//!
//! Every glyph in the chrome used to be a literal — `⟳`, `⋯`, `▾`, `✕`, `◧` —
//! which meant each one was drawn by whichever installed font happened to
//! resolve that codepoint, at whatever weight and optical size that font gave
//! it. Two of them landed in different faces from the text beside them, none
//! of them matched a stroke weight, and none would have matched a new UI face.
//!
//! These are thirty marks from **Lucide** (ISC; the licence sits beside
//! them in `assets/icons`), compiled into the binary and rendered by `gpui` as
//! a mask tinted with the element's own text colour. They are drawn on a 24px
//! grid at a 2px stroke, which lands at about 1.33px once the chrome draws
//! them at [`ICON`] — close to the hand-drawn set they replace, and consistent
//! in a way one pass of hand-drawing was not. Phosphor's filled marks were
//! tried here first and read too heavy against a hairline chrome.
//!
//! More are not from Lucide: the agent providers' own marks, and the Apple and
//! Android marks on the simulator entries, which come from Simple Icons (see
//! `NOTICE.md`). Logos are filled silhouettes on the same grid. A logo redrawn at a
//! hairline to match a stroke set stops being the thing anyone recognises,
//! which is the whole reason a row carries one. So is the primary checkout's
//! boxed `p`, which no set has.
//!
//! They are vendored with `currentColor` rewritten to a literal. `gpui`
//! rasterises through `resvg` with no colour context, so a mark left as
//! `currentColor` resolves to nothing and draws blank — the same silent
//! failure as an uncoloured `Svg`, and the reason [`icon`] takes its colour
//! as an argument.

use std::borrow::Cow;

use gpui::{AssetSource, Div, Pixels, Result, Rgba, SharedString, Styled, Svg, div, px, svg};
use ket_core::theme::Theme;

use super::{ICON, RADIUS_SM};
use crate::paint::{alpha, paint};

/// An icon well's side.
pub(crate) const ICON_WELL: Pixels = px(20.0);

/// A plain glyph's size inside a well: a step under [`ICON`] so the well
/// keeps a margin round it.
pub(crate) const WELL_GLYPH: Pixels = px(14.0);

/// A provider mark's size inside a well. A filled silhouette reads heavier
/// than a stroke at the same size — see `ui::agent::MARK` — so it takes a
/// further step down.
pub(crate) const WELL_MARK: Pixels = px(12.0);

/// How much of a mark's own colour tints the well it sits in.
const WELL_TINT: f32 = 0.14;

/// One icon in the set.
///
/// Deliberately a closed enum rather than a path string: an icon that does not
/// exist should not compile, and the asset source below can only answer for
/// what is listed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    /// An expanded disclosure.
    ChevronDown,
    /// A collapsed disclosure, and a submenu's "there is more this way".
    ChevronRight,
    /// The previous one of something: a diff's change above.
    ChevronUp,
    /// A diff between two versions of a file.
    GitCompare,
    /// Add: a project, a worktree, a tab.
    Plus,
    /// Dismiss: a dialog, a tab.
    Close,
    /// Reload the tree, or the quota reading. One arrow on an open arc, not
    /// Lucide's two chasing arrows, which read as a recycling mark.
    Refresh,
    /// A ticked checkbox.
    Check,
    /// Find.
    Search,
    /// Find *inside* files — lines with a lens over them. Distinct from
    /// [`Icon::Search`], which already leads the explorer's name filter a
    /// few pixels below where this one sits; two magnifiers stacked read as
    /// one control.
    TextSearch,
    /// A terminal pane or session.
    Terminal,
    /// A file, in a tab or a menu row.
    File,
    /// A page with lines on it: the `+` menu's new file. Drawn here on a
    /// 16px grid at a true 1.5 stroke rather than taken from Lucide, like
    /// [`Icon::TerminalWindow`] and [`Icon::BrowserWindow`] beside it — the
    /// three share one frame and read as a set at [`ICON`].
    FileLines,
    /// A window with a prompt in it: the `+` menu's new terminal.
    TerminalWindow,
    /// A window with a title bar and an address line: the `+` menu's new
    /// browser. Deliberately not [`Icon::ExternalLink`], which means leaving
    /// the app — a browser tab stays in it.
    BrowserWindow,
    /// Delete, always paired with a danger colour.
    Trash,
    /// Clear something a command puts back — build output, and nothing that
    /// was written by hand. Deliberately not [`Icon::Trash`]: a bin means the
    /// thing is gone, and the whole argument for clearing build output being
    /// a primary action rather than a destructive one is that it is not.
    Eraser,
    /// Speak instead of type — the quick prompt's voice input.
    Mic,
    /// Leaves the app — Reveal in Finder, and the like.
    ExternalLink,
    /// A source file's angle brackets: an editor with no mark of its own to
    /// show, in a list where the others wear theirs.
    Code,
    /// Show or hide the sidebar.
    Sidebar,
    /// Show or hide the right panel — the sidebar's mirror.
    PanelRight,
    /// Settings.
    Sliders,
    /// A worktree, which is a branch with a checkout.
    GitBranch,
    /// A branch rejoining the line it came from.
    GitMerge,
    /// A commit on a line — the merge dialog's first step.
    GitCommit,
    /// Edit in place — a worktree's Update row.
    Pencil,
    /// A list of states to move something into.
    List,
    /// Copy to the clipboard.
    Copy,
    /// Keep this at the top.
    Pin,
    /// Take this off the top.
    PinOff,
    /// The primary checkout's mark on a worktree row: a `p` in a box, whose
    /// outline is drawn lighter so the letter reads first.
    Primary,
    /// Notifications. Nothing draws it since the sidebar's placeholder bell
    /// came out, and it stays for when notifications are real.
    #[allow(dead_code)]
    Bell,
    /// Group several things.
    Layers,
    /// The sidebar sorted by status: three square pads, each on a trace cut
    /// shorter than the one above it. Drawn here rather than taken from
    /// Lucide, like [`Icon::ViewProject`] beside it — the pair share one
    /// vocabulary, square pads for worktrees and a round via for a project.
    ViewStatus,
    /// The sidebar grouped by project: a round via with a trunk running down
    /// from it, elbowing out to two square pads.
    ViewProject,
    /// ket's own mark, the bar and chevron from the app icon. A filled
    /// silhouette like the provider marks, for the same reason: it is a logo.
    KetMark,
    /// Suspend.
    Moon,
    /// A phone: the Settings section that pairs one.
    Smartphone,
    /// A tablet, among the paired devices.
    Tablet,
    /// This desktop, the one the paired devices reach.
    Monitor,
    /// Something kept safe: the branch a worktree delete leaves behind when
    /// it holds unmerged commits.
    ShieldCheck,
    /// The asterisk an agent spins while it works.
    Asterisk,
    /// A directory the file tree has not opened.
    Folder,
    /// One it has.
    FolderOpen,
    /// Add a project: a folder with a plus in it.
    FolderPlus,
    /// Make a bar shorter: two chevrons closing on a line.
    FoldVertical,
    /// Let it back out: the chevrons parting.
    UnfoldVertical,
    /// Start playback. Filled, like [`Icon::Pause`]: the two sit on the
    /// audio player's primary button, which is a solid accent fill, and a
    /// hairline triangle there reads as a smudge rather than a control.
    Play,
    /// Suspend playback.
    Pause,
    /// Step the audio player's position back a fixed increment.
    RotateCcw,
    /// Step the audio player's position forward a fixed increment.
    RotateCw,
    /// Money kept: the status bar's savings total.
    PiggyBank,

    // ---- the four tones a toast can take ----------------------------------
    //
    // A shape per tone, so the news does not arrive in colour alone: a reader
    // who cannot tell the green rail from the amber one still gets a tick, a
    // triangle or a cross.
    /// Something worth knowing that needed nothing.
    Info,
    /// It worked.
    CircleCheck,
    /// It worked, but not the whole way.
    TriangleAlert,
    /// It did not work.
    CircleX,

    // ---- provider marks ----------------------------------------------------
    //
    // Not Lucide, and not a stroke set: an agent's mark is a logo, and a logo
    // redrawn at a hairline stops being the thing people recognise. These are
    // drawn as filled silhouettes on the same 24px grid, so they sit level
    // with the rest at [`ICON`] without matching their weight.
    /// Claude — the radial burst.
    Claude,
    /// Codex — the six-petal knot.
    Codex,
    /// OpenCode — the shell prompt.
    OpenCode,
    /// Gemini — the four-point spark.
    Gemini,
    /// Grok — the slashed ring.
    Grok,
    /// Apple — the bitten fruit, for the iPhone Simulator entry.
    Apple,
    /// Android — the bugdroid head, for the Android Emulator entry.
    Android,
    /// A project's backlog: a checked box beside lines.
    ListTodo,
    /// A file attached to something.
    Paperclip,

    // ---- the settings rail -------------------------------------------------
    //
    // Drawn here, one mark per section, so no two sections share a glyph and
    // none borrows a mark that means something else in the chrome. Duotone:
    // the setting a mark sits in is drawn at 38% opacity and the part that
    // names the section at full, which survives `gpui`'s single-colour mask
    // because the mask is the SVG's alpha.
    /// Agents: a satellite on an orbit round a core, the motif of
    /// `orbit.rs`. The orbit is cut round the satellite so the two do not
    /// merge at [`ICON`].
    Orbit,
    /// Devices: this Mac and a phone paired to it.
    LaptopPhone,
    /// Economy: a gauge with its needle set low.
    Gauge,
    /// Snippets: a message with a bookmark on it — a prompt kept to reuse.
    MessageBookmark,
    /// General: [`Icon::KetMark`] inset to 14 of the 24 units, since a
    /// filled mark at full size outweighs the strokes around it.
    AppMark,
    /// Appearance: a theme swatch, light over dark.
    Swatch,
    /// Editor: lines of text and the caret that edits them.
    TextCursor,
    /// Terminal: a prompt and its block cursor.
    Prompt,
    /// Merge: a worktree elbowing back into its trunk, drawn like the
    /// sidebar tree.
    MergeInto,
    /// Keybindings: the command key.
    Command,
    /// A device's screen, taken as a picture.
    Camera,
    /// A device's on-screen keyboard.
    Keyboard,
    /// A device's lock.
    Lock,
    /// Back, as a device's back button.
    ArrowLeft,
    /// Forward, beside [`Icon::ArrowLeft`] in a browser's toolbar.
    ArrowRight,
    /// Feedback: a speech bubble, the status bar's way to write to the team.
    MessageSquare,
    /// A backlog note's priority: the first of three bars. Drawn over
    /// [`Icon::PriorityTrack`], which shows the bars that are not lit.
    PriorityLow,
    /// The first two of three bars.
    PriorityMedium,
    /// All three bars.
    PriorityHigh,
    /// A filled square with an exclamation mark cut out of it: not a fourth
    /// bar, so it stands out of a list of them.
    PriorityUrgent,
    /// All three priority bars, drawn faint under the lit ones so a mark
    /// always shows its whole scale.
    PriorityTrack,
    /// Showing what something will be: a backlog note's brief, as its agent
    /// will be handed it.
    Eye,
    /// A label on a note, and the button that adds one.
    Tag,
    /// Kept in the repository: a backlog stored beside the code.
    BookMarked,
    /// More: the actions a row keeps behind a button rather than on it.
    Ellipsis,
}

impl Icon {
    /// The asset path this icon is registered under.
    fn asset(self) -> &'static str {
        match self {
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::ChevronUp => "icons/chevron-up.svg",
            Icon::GitCompare => "icons/git-compare.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::Close => "icons/close.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Check => "icons/check.svg",
            Icon::Search => "icons/search.svg",
            Icon::TextSearch => "icons/text-search.svg",
            Icon::Terminal => "icons/terminal.svg",
            Icon::File => "icons/file.svg",
            Icon::FileLines => "icons/file-lines.svg",
            Icon::TerminalWindow => "icons/terminal-window.svg",
            Icon::BrowserWindow => "icons/browser-window.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::Eraser => "icons/eraser.svg",
            Icon::Mic => "icons/mic.svg",
            Icon::ExternalLink => "icons/external-link.svg",
            Icon::Code => "icons/code.svg",
            Icon::Sidebar => "icons/sidebar.svg",
            Icon::PanelRight => "icons/panel-right.svg",
            Icon::Sliders => "icons/sliders.svg",
            Icon::GitBranch => "icons/git-branch.svg",
            Icon::GitMerge => "icons/git-merge.svg",
            Icon::GitCommit => "icons/git-commit.svg",
            Icon::Pencil => "icons/pencil.svg",
            Icon::List => "icons/list.svg",
            Icon::Copy => "icons/copy.svg",
            Icon::Pin => "icons/pin.svg",
            Icon::PinOff => "icons/pin-off.svg",
            Icon::Primary => "icons/primary.svg",
            Icon::Bell => "icons/bell.svg",
            Icon::PiggyBank => "icons/piggy-bank.svg",
            Icon::Layers => "icons/layers.svg",
            Icon::ViewStatus => "icons/view-status.svg",
            Icon::ViewProject => "icons/view-project.svg",
            Icon::KetMark => "icons/ket-mark.svg",
            Icon::Moon => "icons/moon.svg",
            Icon::Smartphone => "icons/smartphone.svg",
            Icon::Tablet => "icons/tablet.svg",
            Icon::Monitor => "icons/monitor.svg",
            Icon::ShieldCheck => "icons/shield-check.svg",
            Icon::Asterisk => "icons/asterisk.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::FolderOpen => "icons/folder-open.svg",
            Icon::FolderPlus => "icons/folder-plus.svg",
            Icon::FoldVertical => "icons/fold-vertical.svg",
            Icon::UnfoldVertical => "icons/unfold-vertical.svg",
            Icon::Play => "icons/play.svg",
            Icon::Pause => "icons/pause.svg",
            Icon::RotateCcw => "icons/rotate-ccw.svg",
            Icon::RotateCw => "icons/rotate-cw.svg",
            Icon::Info => "icons/info.svg",
            Icon::CircleCheck => "icons/circle-check.svg",
            Icon::TriangleAlert => "icons/triangle-alert.svg",
            Icon::CircleX => "icons/circle-x.svg",
            Icon::Claude => "icons/agent-claude.svg",
            Icon::Codex => "icons/agent-codex.svg",
            Icon::OpenCode => "icons/agent-opencode.svg",
            Icon::Gemini => "icons/agent-gemini.svg",
            Icon::Grok => "icons/agent-grok.svg",
            Icon::Apple => "icons/apple-logo.svg",
            Icon::Android => "icons/android-logo.svg",
            Icon::ListTodo => "icons/list-todo.svg",
            Icon::Paperclip => "icons/paperclip.svg",
            Icon::Orbit => "icons/orbit.svg",
            Icon::LaptopPhone => "icons/laptop-phone.svg",
            Icon::Gauge => "icons/gauge.svg",
            Icon::MessageBookmark => "icons/message-bookmark.svg",
            Icon::AppMark => "icons/app-mark.svg",
            Icon::Swatch => "icons/swatch.svg",
            Icon::TextCursor => "icons/text-cursor.svg",
            Icon::Prompt => "icons/prompt.svg",
            Icon::MergeInto => "icons/merge-into.svg",
            Icon::Command => "icons/command.svg",
            Icon::Camera => "icons/camera.svg",
            Icon::Keyboard => "icons/keyboard.svg",
            Icon::Lock => "icons/lock.svg",
            Icon::ArrowLeft => "icons/arrow-left.svg",
            Icon::ArrowRight => "icons/arrow-right.svg",
            Icon::MessageSquare => "icons/message-square.svg",
            Icon::PriorityLow => "icons/priority-low.svg",
            Icon::PriorityMedium => "icons/priority-medium.svg",
            Icon::PriorityHigh => "icons/priority-high.svg",
            Icon::PriorityUrgent => "icons/priority-urgent.svg",
            Icon::PriorityTrack => "icons/priority-track.svg",
            Icon::Ellipsis => "icons/ellipsis.svg",
            Icon::Eye => "icons/eye.svg",
            Icon::Tag => "icons/tag.svg",
            Icon::BookMarked => "icons/book-marked.svg",
        }
    }
}

/// Draws `which` at the standard [`ICON`] size, in `colour`.
///
/// The colour is an argument rather than something inherited, and that is the
/// whole point of the signature. `gpui` rasterises an SVG to an alpha mask and
/// tints it with the element's *own* `style.text.color`; a `text_color` set on
/// the parent does not reach it. An icon built without one is not a
/// differently-coloured icon, it is an invisible one — which is exactly how
/// every chevron, the branch marks and both sidebar buttons came to be missing
/// while the one icon that passed its own colour rendered fine.
pub(crate) fn icon(which: Icon, colour: Rgba) -> Svg {
    sized_icon(which, ICON, colour)
}

/// Draws `which` at a size of the caller's choosing.
///
/// For the places that are not 16px: a menu's submenu chevron, a tab's close
/// button, and the explorer tree's disclosure marks.
pub(crate) fn sized_icon(which: Icon, size: Pixels, colour: Rgba) -> Svg {
    svg()
        .path(which.asset())
        .size(size)
        .flex_none()
        .text_color(colour)
}

/// The icons, compiled in.
///
/// Registered with `Application::with_assets` before the window opens. Without
/// it `gpui` has no asset source at all and every `svg()` silently draws
/// nothing — the same class of quiet failure the bundled fonts exist to avoid.
pub(crate) struct Assets;

/// The bytes for every path [`Icon::asset`] can return.
const FILES: [(&str, &[u8]); 91] = [
    (
        "icons/chevron-down.svg",
        include_bytes!("../../assets/icons/chevron-down.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../../assets/icons/chevron-right.svg"),
    ),
    (
        "icons/chevron-up.svg",
        include_bytes!("../../assets/icons/chevron-up.svg"),
    ),
    (
        "icons/git-compare.svg",
        include_bytes!("../../assets/icons/git-compare.svg"),
    ),
    (
        "icons/plus.svg",
        include_bytes!("../../assets/icons/plus.svg"),
    ),
    (
        "icons/close.svg",
        include_bytes!("../../assets/icons/close.svg"),
    ),
    (
        "icons/refresh.svg",
        include_bytes!("../../assets/icons/refresh.svg"),
    ),
    (
        "icons/check.svg",
        include_bytes!("../../assets/icons/check.svg"),
    ),
    (
        "icons/search.svg",
        include_bytes!("../../assets/icons/search.svg"),
    ),
    (
        "icons/text-search.svg",
        include_bytes!("../../assets/icons/text-search.svg"),
    ),
    (
        "icons/terminal.svg",
        include_bytes!("../../assets/icons/terminal.svg"),
    ),
    (
        "icons/file.svg",
        include_bytes!("../../assets/icons/file.svg"),
    ),
    (
        "icons/file-lines.svg",
        include_bytes!("../../assets/icons/file-lines.svg"),
    ),
    (
        "icons/terminal-window.svg",
        include_bytes!("../../assets/icons/terminal-window.svg"),
    ),
    (
        "icons/browser-window.svg",
        include_bytes!("../../assets/icons/browser-window.svg"),
    ),
    (
        "icons/trash.svg",
        include_bytes!("../../assets/icons/trash.svg"),
    ),
    (
        "icons/external-link.svg",
        include_bytes!("../../assets/icons/external-link.svg"),
    ),
    (
        "icons/code.svg",
        include_bytes!("../../assets/icons/code.svg"),
    ),
    (
        "icons/sidebar.svg",
        include_bytes!("../../assets/icons/sidebar.svg"),
    ),
    (
        "icons/panel-right.svg",
        include_bytes!("../../assets/icons/panel-right.svg"),
    ),
    (
        "icons/sliders.svg",
        include_bytes!("../../assets/icons/sliders.svg"),
    ),
    (
        "icons/git-branch.svg",
        include_bytes!("../../assets/icons/git-branch.svg"),
    ),
    (
        "icons/git-merge.svg",
        include_bytes!("../../assets/icons/git-merge.svg"),
    ),
    (
        "icons/git-commit.svg",
        include_bytes!("../../assets/icons/git-commit.svg"),
    ),
    (
        "icons/pencil.svg",
        include_bytes!("../../assets/icons/pencil.svg"),
    ),
    (
        "icons/list.svg",
        include_bytes!("../../assets/icons/list.svg"),
    ),
    (
        "icons/copy.svg",
        include_bytes!("../../assets/icons/copy.svg"),
    ),
    (
        "icons/pin.svg",
        include_bytes!("../../assets/icons/pin.svg"),
    ),
    (
        "icons/pin-off.svg",
        include_bytes!("../../assets/icons/pin-off.svg"),
    ),
    (
        "icons/primary.svg",
        include_bytes!("../../assets/icons/primary.svg"),
    ),
    (
        "icons/bell.svg",
        include_bytes!("../../assets/icons/bell.svg"),
    ),
    (
        "icons/piggy-bank.svg",
        include_bytes!("../../assets/icons/piggy-bank.svg"),
    ),
    (
        "icons/layers.svg",
        include_bytes!("../../assets/icons/layers.svg"),
    ),
    (
        "icons/view-status.svg",
        include_bytes!("../../assets/icons/view-status.svg"),
    ),
    (
        "icons/view-project.svg",
        include_bytes!("../../assets/icons/view-project.svg"),
    ),
    (
        "icons/ket-mark.svg",
        include_bytes!("../../assets/icons/ket-mark.svg"),
    ),
    (
        "icons/moon.svg",
        include_bytes!("../../assets/icons/moon.svg"),
    ),
    (
        "icons/smartphone.svg",
        include_bytes!("../../assets/icons/smartphone.svg"),
    ),
    (
        "icons/tablet.svg",
        include_bytes!("../../assets/icons/tablet.svg"),
    ),
    (
        "icons/monitor.svg",
        include_bytes!("../../assets/icons/monitor.svg"),
    ),
    (
        "icons/shield-check.svg",
        include_bytes!("../../assets/icons/shield-check.svg"),
    ),
    (
        "icons/asterisk.svg",
        include_bytes!("../../assets/icons/asterisk.svg"),
    ),
    (
        "icons/folder.svg",
        include_bytes!("../../assets/icons/folder.svg"),
    ),
    (
        "icons/folder-open.svg",
        include_bytes!("../../assets/icons/folder-open.svg"),
    ),
    (
        "icons/folder-plus.svg",
        include_bytes!("../../assets/icons/folder-plus.svg"),
    ),
    (
        "icons/fold-vertical.svg",
        include_bytes!("../../assets/icons/fold-vertical.svg"),
    ),
    (
        "icons/unfold-vertical.svg",
        include_bytes!("../../assets/icons/unfold-vertical.svg"),
    ),
    (
        "icons/play.svg",
        include_bytes!("../../assets/icons/play.svg"),
    ),
    (
        "icons/pause.svg",
        include_bytes!("../../assets/icons/pause.svg"),
    ),
    (
        "icons/rotate-ccw.svg",
        include_bytes!("../../assets/icons/rotate-ccw.svg"),
    ),
    (
        "icons/rotate-cw.svg",
        include_bytes!("../../assets/icons/rotate-cw.svg"),
    ),
    (
        "icons/eraser.svg",
        include_bytes!("../../assets/icons/eraser.svg"),
    ),
    (
        "icons/mic.svg",
        include_bytes!("../../assets/icons/mic.svg"),
    ),
    (
        "icons/info.svg",
        include_bytes!("../../assets/icons/info.svg"),
    ),
    (
        "icons/circle-check.svg",
        include_bytes!("../../assets/icons/circle-check.svg"),
    ),
    (
        "icons/triangle-alert.svg",
        include_bytes!("../../assets/icons/triangle-alert.svg"),
    ),
    (
        "icons/circle-x.svg",
        include_bytes!("../../assets/icons/circle-x.svg"),
    ),
    (
        "icons/agent-claude.svg",
        include_bytes!("../../assets/icons/agent-claude.svg"),
    ),
    (
        "icons/agent-codex.svg",
        include_bytes!("../../assets/icons/agent-codex.svg"),
    ),
    (
        "icons/agent-opencode.svg",
        include_bytes!("../../assets/icons/agent-opencode.svg"),
    ),
    (
        "icons/agent-gemini.svg",
        include_bytes!("../../assets/icons/agent-gemini.svg"),
    ),
    (
        "icons/agent-grok.svg",
        include_bytes!("../../assets/icons/agent-grok.svg"),
    ),
    (
        "icons/apple-logo.svg",
        include_bytes!("../../assets/icons/apple-logo.svg"),
    ),
    (
        "icons/android-logo.svg",
        include_bytes!("../../assets/icons/android-logo.svg"),
    ),
    (
        "icons/list-todo.svg",
        include_bytes!("../../assets/icons/list-todo.svg"),
    ),
    (
        "icons/paperclip.svg",
        include_bytes!("../../assets/icons/paperclip.svg"),
    ),
    (
        "icons/orbit.svg",
        include_bytes!("../../assets/icons/orbit.svg"),
    ),
    (
        "icons/laptop-phone.svg",
        include_bytes!("../../assets/icons/laptop-phone.svg"),
    ),
    (
        "icons/gauge.svg",
        include_bytes!("../../assets/icons/gauge.svg"),
    ),
    (
        "icons/message-bookmark.svg",
        include_bytes!("../../assets/icons/message-bookmark.svg"),
    ),
    (
        "icons/app-mark.svg",
        include_bytes!("../../assets/icons/app-mark.svg"),
    ),
    (
        "icons/swatch.svg",
        include_bytes!("../../assets/icons/swatch.svg"),
    ),
    (
        "icons/text-cursor.svg",
        include_bytes!("../../assets/icons/text-cursor.svg"),
    ),
    (
        "icons/prompt.svg",
        include_bytes!("../../assets/icons/prompt.svg"),
    ),
    (
        "icons/merge-into.svg",
        include_bytes!("../../assets/icons/merge-into.svg"),
    ),
    (
        "icons/command.svg",
        include_bytes!("../../assets/icons/command.svg"),
    ),
    (
        "icons/camera.svg",
        include_bytes!("../../assets/icons/camera.svg"),
    ),
    (
        "icons/keyboard.svg",
        include_bytes!("../../assets/icons/keyboard.svg"),
    ),
    (
        "icons/lock.svg",
        include_bytes!("../../assets/icons/lock.svg"),
    ),
    (
        "icons/arrow-left.svg",
        include_bytes!("../../assets/icons/arrow-left.svg"),
    ),
    (
        "icons/arrow-right.svg",
        include_bytes!("../../assets/icons/arrow-right.svg"),
    ),
    (
        "icons/message-square.svg",
        include_bytes!("../../assets/icons/message-square.svg"),
    ),
    (
        "icons/priority-low.svg",
        include_bytes!("../../assets/icons/priority-low.svg"),
    ),
    (
        "icons/priority-medium.svg",
        include_bytes!("../../assets/icons/priority-medium.svg"),
    ),
    (
        "icons/priority-high.svg",
        include_bytes!("../../assets/icons/priority-high.svg"),
    ),
    (
        "icons/priority-urgent.svg",
        include_bytes!("../../assets/icons/priority-urgent.svg"),
    ),
    (
        "icons/priority-track.svg",
        include_bytes!("../../assets/icons/priority-track.svg"),
    ),
    (
        "icons/ellipsis.svg",
        include_bytes!("../../assets/icons/ellipsis.svg"),
    ),
    (
        "icons/eye.svg",
        include_bytes!("../../assets/icons/eye.svg"),
    ),
    (
        "icons/tag.svg",
        include_bytes!("../../assets/icons/tag.svg"),
    ),
    (
        "icons/book-marked.svg",
        include_bytes!("../../assets/icons/book-marked.svg"),
    ),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

/// A small rounded square for an icon to sit in: sunken behind a plain
/// glyph, lifting to the border step while `lit`; `tint`, faint, behind a
/// glyph drawn in a colour of its own — a provider's mark, a destructive
/// action.
///
/// Every [`crate::ui::menu::OpenMenu`] row sets its icon in one of these.
/// Size the glyph with [`WELL_GLYPH`]
/// or [`WELL_MARK`]. A `Div`, so a caller can add a hover rule: a menu row
/// lifts its well with the row with [`lift_well`].
pub(crate) fn icon_well(tint: Option<Rgba>, lit: bool, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(ICON_WELL)
        .rounded(RADIUS_SM)
        .bg(match tint {
            Some(colour) => alpha(colour, WELL_TINT),
            None => paint(if lit { t.border } else { t.sunken }),
        })
}

/// What a plain well lifts to under the pointer: the same step `lit` gives
/// it, for a caller's hover rule.
pub(crate) fn lift_well(style: gpui::StyleRefinement, t: &Theme) -> gpui::StyleRefinement {
    style.bg(paint(t.border))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon the enum can name has bytes registered under its path.
    ///
    /// The failure this catches is silent at runtime: `gpui` draws nothing for
    /// an asset it cannot find, so a typo'd path is an invisible icon rather
    /// than an error.
    #[test]
    fn every_icon_resolves_to_a_registered_asset() {
        // Every variant, in declaration order. A new icon goes here too, or
        // the count below says so.
        let every = [
            Icon::ChevronDown,
            Icon::ChevronRight,
            Icon::ChevronUp,
            Icon::GitCompare,
            Icon::Plus,
            Icon::Close,
            Icon::Refresh,
            Icon::Check,
            Icon::Search,
            Icon::TextSearch,
            Icon::Terminal,
            Icon::File,
            Icon::FileLines,
            Icon::TerminalWindow,
            Icon::BrowserWindow,
            Icon::Trash,
            Icon::Eraser,
            Icon::Mic,
            Icon::ExternalLink,
            Icon::Code,
            Icon::Sidebar,
            Icon::PanelRight,
            Icon::Sliders,
            Icon::GitBranch,
            Icon::GitMerge,
            Icon::GitCommit,
            Icon::Pencil,
            Icon::List,
            Icon::Copy,
            Icon::Pin,
            Icon::PinOff,
            Icon::Primary,
            Icon::Bell,
            Icon::Layers,
            Icon::ViewStatus,
            Icon::ViewProject,
            Icon::KetMark,
            Icon::Moon,
            Icon::Smartphone,
            Icon::Tablet,
            Icon::Monitor,
            Icon::ShieldCheck,
            Icon::Asterisk,
            Icon::Folder,
            Icon::FolderOpen,
            Icon::FolderPlus,
            Icon::FoldVertical,
            Icon::UnfoldVertical,
            Icon::Play,
            Icon::Pause,
            Icon::RotateCcw,
            Icon::RotateCw,
            Icon::PiggyBank,
            Icon::Info,
            Icon::CircleCheck,
            Icon::TriangleAlert,
            Icon::CircleX,
            Icon::Claude,
            Icon::Codex,
            Icon::OpenCode,
            Icon::Gemini,
            Icon::Grok,
            Icon::Apple,
            Icon::Android,
            Icon::ListTodo,
            Icon::Paperclip,
            Icon::Orbit,
            Icon::LaptopPhone,
            Icon::Gauge,
            Icon::MessageBookmark,
            Icon::AppMark,
            Icon::Swatch,
            Icon::TextCursor,
            Icon::Prompt,
            Icon::MergeInto,
            Icon::Command,
            Icon::Camera,
            Icon::Keyboard,
            Icon::Lock,
            Icon::ArrowLeft,
            Icon::ArrowRight,
            Icon::MessageSquare,
            Icon::PriorityLow,
            Icon::PriorityMedium,
            Icon::PriorityHigh,
            Icon::PriorityUrgent,
            Icon::PriorityTrack,
            Icon::Ellipsis,
            Icon::Eye,
            Icon::Tag,
            Icon::BookMarked,
        ];
        assert_eq!(
            every.len(),
            FILES.len(),
            "an icon has no asset, or vice versa"
        );

        for which in every {
            let found = Assets.load(which.asset()).unwrap();
            assert!(
                found.is_some(),
                "{:?} has no bytes: {}",
                which,
                which.asset()
            );
        }
    }

    #[test]
    fn the_registered_bytes_are_svg() {
        for (name, bytes) in FILES {
            let text = std::str::from_utf8(bytes).expect("icons are text");
            assert!(text.starts_with("<svg"), "{name} is not an svg");
        }
    }

    #[test]
    fn an_unknown_path_is_absent_rather_than_an_error() {
        // gpui asks for things that may not exist; a miss is not a failure.
        assert!(Assets.load("icons/no-such-icon.svg").unwrap().is_none());
    }
}
