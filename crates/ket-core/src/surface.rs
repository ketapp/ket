//! The editing surface: handing a file to the editor the user already runs.
//!
//! ket's core loop is *reading*. Three agents work in parallel worktrees and a
//! person reads three diffs and keeps one. Editing is the rarer act, and when it
//! comes it is better served by the editor that person has already configured —
//! their keymap, their language servers, their muscle memory — than by anything
//! ket could grow in the time available. `gpui` ships no text editor widget, and
//! writing one is a multi-year project, so external handoff is not a fallback
//! here: it is the surface, and it has to be good enough to use every day.
//!
//! Three pieces make that true:
//!
//! 1. **Precise handoff.** Not "open the file" but "open the file *here*", at a
//!    line and column taken from a diff hunk. Every editor spells that
//!    differently, so the spelling lives in [`PROFILES`] as data rather than in
//!    a branch per editor — see [`EditorProfile`] for the three rules that turn
//!    one template into all four argument shapes in the wild.
//! 2. **Noticing.** Once the file is open in someone else's process, ket is no
//!    longer the only writer — and it never was, because an agent is writing
//!    into the same worktree at the same time. [`FileWatcher`] is what keeps a
//!    view honest without a manual refresh.
//! 3. **Not clobbering.** When ket does write a file, it writes it only if the
//!    file is still the one it read. See [`write_if_unchanged`].
//!
//! [`EditorSurface`] exists with a single implementation on purpose. A native
//! editor pane may arrive later; when it does, it should be a second
//! implementation and a different [`SurfaceCaps`], not a rewrite of every caller.
//!
//! **On invariant 4 — every state is escapable.** Nothing here waits on an
//! editor. A missing `$KET_EDITOR`, a binary that is not on `PATH`, an editor
//! that never exits: all of them return promptly, with a message that says what
//! to set. The one thing this module must never do is block a UI thread on
//! somebody else's process.

use std::ffi::{OsStr, OsString};
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use crate::{KetError, Result, now_ms};

/// Environment variables consulted for the editor command, most specific first.
///
/// `$KET_EDITOR` is checked first so a person can point ket at a GUI editor
/// without disturbing the `$EDITOR` their shell tools use — those two wants
/// genuinely differ, since `$EDITOR` is very often a terminal editor and ket
/// spawns detached (see [`ProcessSpawner`]).
pub const EDITOR_VARS: [&str; 3] = ["KET_EDITOR", "VISUAL", "EDITOR"];

/// Reads an environment variable. Injected so the editor can be resolved in
/// tests without mutating process-wide state — under a parallel test runner that
/// is a race, and in edition 2024 it requires `unsafe`, which this workspace
/// forbids. Same reasoning as [`crate::paths`].
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// What an editing surface can actually do.
///
/// Read by the shell to decide what to draw, rather than by asking "is this the
/// external one?" — which is the question that would have to be revisited at
/// every call site the day a second surface exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceCaps {
    /// Whether editing happens inside ket's own window.
    ///
    /// `false` means handing off to another process, which in turn means ket
    /// cannot know about unsaved buffers and must watch the filesystem instead.
    pub embedded: bool,
    /// Whether this surface can change a file at all.
    ///
    /// A future preview-only pane would set this `false`; the UI uses it to
    /// decide whether to offer an edit affordance rather than offering one that
    /// fails.
    pub editable: bool,
    /// Whether the surface renders diffs itself.
    ///
    /// `false` for [`ExternalSurface`], and that is the load-bearing fact behind
    /// [`crate::diff`] existing: ket renders the review, the editor renders the
    /// edit.
    pub diffs: bool,
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

/// A place in a file: which file, and optionally where in it.
///
/// Lines and columns are 1-based, matching every editor in [`PROFILES`] and
/// matching unified diff hunk headers, which is where these numbers come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Absolute path to the file.
    ///
    /// Absolute deliberately: the editor is spawned without a working directory
    /// of ket's choosing, so a relative path would resolve against whatever the
    /// process happened to be started in and silently open the wrong file. That
    /// is refused rather than guessed — see [`ExternalSurface::command_for`].
    pub path: PathBuf,
    /// 1-based line, if known.
    pub line: Option<u32>,
    /// 1-based column, if known.
    ///
    /// Always `None` when [`Position::line`] is `None`: no editor understands a
    /// column without a line, and every template shape in [`PROFILES`] writes
    /// the column *after* the line, so a column alone would render as a
    /// dangling separator.
    pub col: Option<u32>,
}

impl Position {
    /// A whole file, with no position inside it.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            line: None,
            col: None,
        }
    }

    /// A file inside a worktree, named by its worktree-relative path.
    ///
    /// The convenience that keeps [`Position::path`] absolute: `status` and
    /// `diff` both report relative paths, and joining them at the boundary is
    /// better than teaching every editor profile about a working directory.
    pub fn in_worktree(root: impl AsRef<Path>, rel: impl AsRef<Path>) -> Self {
        Self::file(root.as_ref().join(rel))
    }

    /// Sets the line. `0` is clamped to `1`.
    ///
    /// A zero comes from a 0-based converter, and the editors here split on what
    /// they do with it — some open at the top, some reject the argument. Clamping
    /// makes the whole table behave the same way.
    #[must_use]
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line.max(1));
        self
    }

    /// Sets the column. `0` is clamped to `1`. Ignored when no line is set.
    #[must_use]
    pub fn with_column(mut self, col: u32) -> Self {
        if self.line.is_some() {
            self.col = Some(col.max(1));
        }
        self
    }

    /// Whether the slot has a value to substitute.
    fn has(&self, slot: Slot) -> bool {
        match slot {
            Slot::Path => true,
            Slot::Line => self.line.is_some(),
            Slot::Col => self.col.is_some(),
        }
    }
}

// ---------------------------------------------------------------------------
// The surface trait
// ---------------------------------------------------------------------------

/// Somewhere a person can be shown, and possibly change, a file.
///
/// Object-safe, and kept with one implementation on purpose: it is the seam a
/// native editor pane would slot into later without touching its callers.
pub trait EditorSurface {
    /// What this surface can do.
    fn caps(&self) -> SurfaceCaps;

    /// A short human-readable name, for error messages and status lines.
    fn describe(&self) -> String;

    /// Brings `at` in front of the user.
    ///
    /// Returns once the handoff has been made, not once the person is done
    /// editing. Waiting for an editor to exit is how a UI thread wedges.
    fn reveal(&self, at: &Position) -> Result<()>;

    /// Watches `root` so changes made outside this surface still show up.
    ///
    /// Defaulted rather than required: an embedded editor knows about its own
    /// writes but still shares the worktree with an agent, so it wants the same
    /// watcher. Overriding is for a surface that has a better source of truth.
    fn watch(&self, root: &Path) -> Result<FileWatcher> {
        FileWatcher::watch(root)
    }
}

// ---------------------------------------------------------------------------
// Editor profiles
// ---------------------------------------------------------------------------

/// A placeholder an argument template can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `{path}`
    Path,
    /// `{line}`
    Line,
    /// `{col}`
    Col,
}

impl Slot {
    /// The placeholder spelling, without braces.
    fn parse(name: &str) -> Option<Self> {
        match name {
            "path" => Some(Slot::Path),
            "line" => Some(Slot::Line),
            "col" => Some(Slot::Col),
            _ => None,
        }
    }
}

/// How one editor wants a place in a file spelled on its command line.
///
/// The four shapes in the wild are a suffix (`path:line:col`), a prefixed Ex
/// command (`+line path`), a flag pair (`--line N --column M path`), and a flag
/// plus a suffix (`--goto path:line:col`). Writing one template per editor and
/// three rules for rendering it covers all four, which is why this is a table
/// and not a branch per editor: adding an editor is adding a row.
///
/// The rules, applied to [`EditorProfile::template`] with `{path}`, `{line}` and
/// `{col}` substituted:
///
/// 1. An argument holding placeholders of which *none* has a value is dropped
///    entirely. `+{line}` disappears when no line is known; `--line` does too,
///    by rule 3.
/// 2. In an argument that survives, trailing unset placeholders and the
///    separator literals (`:` and `,`) that led to them are trimmed.
///    `{path}:{line}:{col}` with only a line becomes `path:line`.
/// 3. A flag-shaped argument — one starting with `-` and holding no
///    placeholders — is dropped when the argument after it was dropped, because
///    a flag without its value is worse than no flag. Applied right to left, so
///    `--line {line} --column {col}` collapses correctly from either end.
///
/// An editor with no row is not an error: it gets [`PATH_ONLY`] and opens the
/// file. Losing the cursor position is a smaller failure than refusing to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorProfile {
    /// Matched against the executable's file stem, lowercased.
    ///
    /// The stem rather than the whole command, so an absolute path to the binary
    /// — which is what `$KET_EDITOR` holds when the editor is not on `PATH` —
    /// still matches.
    pub program: &'static str,
    /// What the editor calls itself, for a menu row or a status line.
    ///
    /// `code` is a command; "VS Code" is what a person is looking for in a
    /// list. The table is the only place that knows both, so the mapping lives
    /// here rather than being guessed from the program name at a call site —
    /// `subl` is "Sublime Text" and no rule turns one into the other.
    pub label: &'static str,
    /// Argument template, appended after any arguments the user themselves put
    /// in `$KET_EDITOR`.
    pub template: &'static [&'static str],
    /// Whether it opens a window of its own.
    ///
    /// `false` for the editors that want a terminal — vim, nano, kak. ket
    /// spawns detached with stdio closed (see [`ProcessSpawner`]), so offering
    /// one of those as a place to open a file would start a process with
    /// nowhere to draw. They stay in the table because `$KET_EDITOR` may well
    /// name one, wrapped in a terminal the user chose; what this flag decides
    /// is whether ket may offer it *unwrapped*.
    pub gui: bool,
}

/// The profile used when nothing else matches: open the file, no position.
pub const PATH_ONLY: EditorProfile = EditorProfile {
    program: "",
    label: "your editor",
    template: &["{path}"],
    // Unknown, and assumed to want a window: this is what an editor ket has
    // never heard of gets, and `$KET_EDITOR` naming a terminal editor is
    // already the user saying they know what they are launching.
    gui: true,
};

/// Known editors and how each wants a position.
///
/// Sourced from each editor's own CLI documentation. Where an editor has forks
/// that kept the same CLI (the VS Code family) the forks get their own rows
/// rather than a prefix match, because a prefix match would silently claim
/// unrelated programs.
pub static PROFILES: &[EditorProfile] = &[
    // VS Code and forks: `code --goto path:line:col`.
    EditorProfile {
        program: "code",
        label: "VS Code",
        template: &["--goto", "{path}:{line}:{col}"],
        gui: true,
    },
    EditorProfile {
        program: "code-insiders",
        label: "VS Code Insiders",
        template: &["--goto", "{path}:{line}:{col}"],
        gui: true,
    },
    EditorProfile {
        program: "codium",
        label: "VSCodium",
        template: &["--goto", "{path}:{line}:{col}"],
        gui: true,
    },
    EditorProfile {
        program: "cursor",
        label: "Cursor",
        template: &["--goto", "{path}:{line}:{col}"],
        gui: true,
    },
    EditorProfile {
        program: "windsurf",
        label: "Windsurf",
        template: &["--goto", "{path}:{line}:{col}"],
        gui: true,
    },
    // Bare suffix: `zed path:line:col`.
    EditorProfile {
        program: "zed",
        label: "Zed",
        template: &["{path}:{line}:{col}"],
        gui: true,
    },
    EditorProfile {
        program: "subl",
        label: "Sublime Text",
        template: &["{path}:{line}:{col}"],
        gui: true,
    },
    EditorProfile {
        program: "hx",
        label: "Helix",
        template: &["{path}:{line}:{col}"],
        gui: false,
    },
    // Vim family: `nvim +line path`. There is no column in this shape, and the
    // alternatives that do have one (`-c "call cursor(l,c)"`) are worth less
    // than a predictable command line.
    EditorProfile {
        program: "nvim",
        label: "Neovim",
        template: &["+{line}", "{path}"],
        gui: false,
    },
    EditorProfile {
        program: "vim",
        label: "Vim",
        template: &["+{line}", "{path}"],
        gui: false,
    },
    EditorProfile {
        program: "vi",
        label: "Vi",
        template: &["+{line}", "{path}"],
        gui: false,
    },
    EditorProfile {
        program: "gvim",
        label: "GVim",
        template: &["+{line}", "{path}"],
        gui: true,
    },
    EditorProfile {
        program: "bbedit",
        label: "BBEdit",
        template: &["+{line}", "{path}"],
        gui: true,
    },
    // Prefixed with a separator before the column.
    EditorProfile {
        program: "emacsclient",
        label: "Emacs",
        template: &["+{line}:{col}", "{path}"],
        gui: false,
    },
    EditorProfile {
        program: "micro",
        label: "Micro",
        template: &["+{line}:{col}", "{path}"],
        gui: false,
    },
    EditorProfile {
        program: "kak",
        label: "Kakoune",
        template: &["+{line}:{col}", "{path}"],
        gui: false,
    },
    // `nano` uses a comma where the others use a colon. This single character is
    // the reason the table holds templates rather than a "supports column" flag.
    EditorProfile {
        program: "nano",
        label: "Nano",
        template: &["+{line},{col}", "{path}"],
        gui: false,
    },
    // TextMate: a flag whose value carries both numbers.
    EditorProfile {
        program: "mate",
        label: "TextMate",
        template: &["-l", "{line}:{col}", "{path}"],
        gui: true,
    },
    // JetBrains: separate flags, which is the shape rule 3 exists for.
    EditorProfile {
        program: "idea",
        label: "IntelliJ IDEA",
        template: &["--line", "{line}", "--column", "{col}", "{path}"],
        gui: true,
    },
    EditorProfile {
        program: "pycharm",
        label: "PyCharm",
        template: &["--line", "{line}", "--column", "{col}", "{path}"],
        gui: true,
    },
    EditorProfile {
        program: "webstorm",
        label: "WebStorm",
        template: &["--line", "{line}", "--column", "{col}", "{path}"],
        gui: true,
    },
    EditorProfile {
        program: "rustrover",
        label: "RustRover",
        template: &["--line", "{line}", "--column", "{col}", "{path}"],
        gui: true,
    },
    EditorProfile {
        program: "goland",
        label: "GoLand",
        template: &["--line", "{line}", "--column", "{col}", "{path}"],
        gui: true,
    },
    EditorProfile {
        program: "clion",
        label: "CLion",
        template: &["--line", "{line}", "--column", "{col}", "{path}"],
        gui: true,
    },
];

/// The profile for an editor binary, matched on its file stem.
///
/// Returns `None` rather than [`PATH_ONLY`] so a caller can tell "ket knows this
/// editor" from "ket will open the file and lose the line", which is worth
/// saying out loud in a doctor command.
pub fn profile_for(program: &Path) -> Option<&'static EditorProfile> {
    let stem = program
        .file_stem()
        .or_else(|| program.file_name())?
        .to_string_lossy()
        .to_lowercase();
    PROFILES.iter().find(|p| p.program == stem)
}

/// An editor ket found on this machine, and what starts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledEditor {
    /// Its row in [`PROFILES`] — the name to show and the template to fill.
    pub profile: &'static EditorProfile,
    /// The executable, as found. Either a name on the login shell's `PATH` or
    /// an absolute path into an application bundle.
    pub program: PathBuf,
    /// Its application's icon as PNG, read from the bundle the executable
    /// lives in — see [`crate::app_icon`] — at [`ICON_PX`] square or the next
    /// size up. `None` for an editor found on `PATH` outside any bundle, which
    /// is how the JetBrains launchers and a Toolbox install arrive.
    pub icon: Option<Vec<u8>>,
}

/// Where an editor keeps its command-line helper inside a macOS application
/// bundle.
///
/// Every one of these editors ships the helper and asks the user to run an
/// "install the shell command" step to get it onto `PATH`, and most people
/// never do. Looking inside the bundle is what makes ket's menu list the
/// editors somebody actually has rather than the ones they happen to have
/// symlinked — on this author's machine, `PATH` alone found none of them.
///
/// Bundle names are exact, because they are what the vendor ships and a fuzzy
/// match would claim unrelated applications — the same reasoning that gives the
/// VS Code forks their own rows in [`PROFILES`] instead of a prefix match.
///
/// The JetBrains editors are deliberately absent: their bundle is named for the
/// edition (`IntelliJ IDEA.app`, `IntelliJ IDEA CE.app`, `IntelliJ IDEA
/// Ultimate.app`, and Toolbox installs elsewhere entirely), and guessing wrong
/// is worse than the `PATH` lookup they already answer to — which is the way
/// JetBrains' own documentation tells people to set them up.
#[cfg(target_os = "macos")]
static BUNDLED_CLIS: &[(&str, &str, &str)] = &[
    (
        "code",
        "Visual Studio Code.app",
        "Contents/Resources/app/bin/code",
    ),
    (
        "code-insiders",
        "Visual Studio Code - Insiders.app",
        "Contents/Resources/app/bin/code-insiders",
    ),
    (
        "codium",
        "VSCodium.app",
        "Contents/Resources/app/bin/codium",
    ),
    ("cursor", "Cursor.app", "Contents/Resources/app/bin/cursor"),
    (
        "windsurf",
        "Windsurf.app",
        "Contents/Resources/app/bin/windsurf",
    ),
    ("zed", "Zed.app", "Contents/MacOS/cli"),
    (
        "subl",
        "Sublime Text.app",
        "Contents/SharedSupport/bin/subl",
    ),
    ("mate", "TextMate.app", "Contents/Resources/mate"),
    ("bbedit", "BBEdit.app", "Contents/Helpers/bbedit_tool"),
];

/// The square an editor's icon is read at: a 16pt menu mark on a 2x display.
///
/// Pixels rather than points, because that is what an `.icns` is indexed by.
/// Fixed here rather than asked for by the menu so the lookup can happen once,
/// on the warm-up thread, with the rest of the discovery.
const ICON_PX: u32 = 32;

/// The windowed editors this machine can actually launch, in [`PROFILES`] order.
///
/// For a menu that offers somewhere to open a file. Only the editors that open
/// a window of their own, because ket spawns detached with no terminal to lend
/// a `nano`; and only the ones that are really here, looked for in two places:
///
/// 1. The login shell's `PATH` — see [`crate::shell::login_path`] for why that
///    is not the `PATH` ket itself inherits.
/// 2. Inside the application bundles named by [`BUNDLED_CLIS`], which is where
///    the helper lives until somebody runs the editor's "install the shell
///    command" step.
///
/// Table order rather than alphabetical: [`PROFILES`] groups editors by the
/// shape of their command line, which keeps the VS Code family together, and a
/// menu whose order changes when a machine gains an editor is a menu people
/// stop being able to aim at from memory.
///
/// Cached and returned by reference: the first call pays one shell startup for
/// the `PATH`, a handful of `stat`s and one icon file read per editor found,
/// and the menu that asks for this is opened from a right-click. Warm it off
/// the window's thread — see the shell's own startup — or the first opening of
/// that menu pays for it.
pub fn installed_editors() -> &'static [InstalledEditor] {
    static CACHE: OnceLock<Vec<InstalledEditor>> = OnceLock::new();
    CACHE.get_or_init(|| {
        PROFILES
            .iter()
            .filter(|profile| profile.gui)
            .filter_map(|profile| {
                let program = crate::shell::find_on_login_path(profile.program)
                    .or_else(|| bundled_cli(profile.program))?;
                let icon = crate::app_icon::bundle_of(&program)
                    .and_then(|bundle| crate::app_icon::icon_png(&bundle, ICON_PX));
                Some(InstalledEditor {
                    profile,
                    program,
                    icon,
                })
            })
            .collect()
    })
}

/// The command-line helper `program` ships inside its application bundle.
#[cfg(target_os = "macos")]
fn bundled_cli(program: &str) -> Option<PathBuf> {
    // Both roots: `/Applications` is where an installer puts an editor, and
    // `~/Applications` is where a person who cannot write to the first one — or
    // who simply prefers it — puts the same editor.
    let roots = [
        Some(PathBuf::from("/Applications")),
        crate::paths::home()
            .ok()
            .map(|home| home.join("Applications")),
    ];
    let roots: Vec<PathBuf> = roots.into_iter().flatten().collect();
    BUNDLED_CLIS
        .iter()
        .filter(|(name, _, _)| *name == program)
        .flat_map(|(_, bundle, inside)| {
            roots.iter().map(move |root| root.join(bundle).join(inside))
        })
        .find(|candidate| candidate.is_file())
}

#[cfg(not(target_os = "macos"))]
fn bundled_cli(_program: &str) -> Option<PathBuf> {
    None
}

// ---------------------------------------------------------------------------
// The file manager
// ---------------------------------------------------------------------------

/// What this platform calls the thing that shows a file in its folder.
///
/// `None` where ket has no reliable way to reveal a *file* — as opposed to
/// opening it, which is a different act and the editors' job. A row that cannot
/// be drawn honestly is not drawn: see [`reveal_in_file_manager`].
pub const FILE_MANAGER: Option<&str> = if cfg!(target_os = "macos") {
    Some("Finder")
} else {
    None
};

/// The process that would reveal `path` in [`FILE_MANAGER`].
///
/// `open -R` selects the file inside its folder rather than opening it with
/// whatever application owns the extension — the distinction between "show me
/// where this is" and "open this", and the reason this is not just `open`.
pub fn file_manager_command(path: &Path) -> Option<EditorCommand> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    Some(EditorCommand {
        program: OsString::from("/usr/bin/open"),
        args: vec![OsString::from("-R"), path.as_os_str().to_owned()],
    })
}

/// Reveals `path` in the platform's file manager.
///
/// Detached and never waited on, exactly as an editor handoff is — see
/// [`ProcessSpawner`] for why both halves of that matter.
pub fn reveal_in_file_manager(path: &Path) -> Result<()> {
    let Some(command) = file_manager_command(path) else {
        return Err(KetError::Config(
            "ket has no file manager to reveal this in on this platform".to_owned(),
        ));
    };
    spawn_detached(&command).map_err(|e| {
        KetError::Config(format!(
            "could not show {} in {}: {e}",
            path.display(),
            FILE_MANAGER.unwrap_or("the file manager"),
        ))
    })
}

// ---------------------------------------------------------------------------
// Template rendering
// ---------------------------------------------------------------------------

/// One piece of a parsed template argument.
enum Piece<'a> {
    /// Text copied through as-is.
    Literal(&'a str),
    /// A placeholder to substitute.
    Slot(Slot),
}

/// Splits a template argument into literals and placeholders.
///
/// An unrecognised `{...}` stays literal: an editor whose real command line
/// contains braces should not have them eaten by a typo in a placeholder name.
fn parse_template(arg: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = arg;

    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after
            .find('}')
            .and_then(|close| Slot::parse(&after[..close]).map(|slot| (slot, &after[close + 1..])))
        {
            Some((slot, tail)) => {
                if open > 0 {
                    pieces.push(Piece::Literal(&rest[..open]));
                }
                pieces.push(Piece::Slot(slot));
                rest = tail;
            }
            None => {
                // Not a placeholder. Keep the brace and carry on past it.
                pieces.push(Piece::Literal(&rest[..=open]));
                rest = after;
            }
        }
    }

    if !rest.is_empty() {
        pieces.push(Piece::Literal(rest));
    }
    pieces
}

/// Renders one template argument, or `None` if it should be dropped.
///
/// Rules 1 and 2 from [`EditorProfile`].
fn render_arg(template: &str, at: &Position) -> Option<OsString> {
    let pieces = parse_template(template);

    let mut saw_slot = false;
    let mut saw_value = false;
    for piece in &pieces {
        if let Piece::Slot(slot) = piece {
            saw_slot = true;
            saw_value |= at.has(*slot);
        }
    }
    // Rule 1: placeholders, none of them filled.
    if saw_slot && !saw_value {
        return None;
    }

    // Rule 2: trim trailing unset placeholders and the separators leading to them.
    let mut end = pieces.len();
    while end > 0 {
        match &pieces[end - 1] {
            Piece::Slot(slot) if !at.has(*slot) => end -= 1,
            Piece::Literal(text)
                if !text.is_empty() && text.chars().all(|c| c == ':' || c == ',') =>
            {
                end -= 1;
            }
            _ => break,
        }
    }

    let mut out = OsString::new();
    for piece in &pieces[..end] {
        match piece {
            Piece::Literal(text) => out.push(text),
            Piece::Slot(Slot::Path) => out.push(at.path.as_os_str()),
            Piece::Slot(Slot::Line) => {
                if let Some(line) = at.line {
                    out.push(line.to_string());
                }
            }
            Piece::Slot(Slot::Col) => {
                if let Some(col) = at.col {
                    out.push(col.to_string());
                }
            }
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

/// Renders a whole template into arguments.
///
/// Rule 3 from [`EditorProfile`] lives here, and runs right to left so a run of
/// flags collapses as a unit.
fn render(template: &[String], at: &Position) -> Vec<OsString> {
    let mut rendered: Vec<Option<OsString>> = template
        .iter()
        .map(|arg| render_arg(arg, at))
        .collect::<Vec<_>>();

    for i in (0..rendered.len()).rev() {
        let dropped_value = i + 1 < rendered.len() && rendered[i + 1].is_none();
        let flag_shaped = template[i].starts_with('-')
            && !parse_template(&template[i])
                .iter()
                .any(|p| matches!(p, Piece::Slot(_)));

        if rendered[i].is_some() && dropped_value && flag_shaped {
            rendered[i] = None;
        }
    }

    rendered.into_iter().flatten().collect()
}

/// Whether a command line holds any of ket's placeholders.
fn has_placeholder(words: &[String]) -> bool {
    words.iter().any(|w| {
        parse_template(w)
            .iter()
            .any(|p| matches!(p, Piece::Slot(_)))
    })
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// The exact process ket would start to reveal a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    /// Program to execute. Never run through a shell.
    pub program: OsString,
    /// Arguments, in order.
    pub args: Vec<OsString>,
}

impl EditorCommand {
    /// The full argv, program first, for logging and for test assertions.
    ///
    /// Lossy because it is for humans. The real spawn uses the `OsString`s.
    pub fn argv_lossy(&self) -> Vec<String> {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }
}

/// Starts a process.
///
/// This is a seam rather than a direct `Command::spawn` so that tests can assert
/// on the argv ket *would* run. Launching a real editor from a test is slow,
/// depends on what happens to be installed, opens windows on the developer's
/// screen, and is simply impossible on a headless machine — and the argv is the
/// whole substance of [`ExternalSurface`], so it is the thing worth checking.
pub trait Spawner: Send + Sync + std::fmt::Debug {
    /// Starts `command` and returns without waiting for it to finish.
    fn spawn(&self, command: &EditorCommand) -> Result<()>;
}

/// The real spawner: starts the editor detached from ket.
///
/// Detached means stdio is closed and the child is never waited on by the
/// caller. Both halves are deliberate:
///
/// - **Closed stdio.** ket is heading for a GUI application with no terminal to
///   lend. A terminal editor needs its own terminal, and the way to have one is
///   to say so — `KET_EDITOR='wezterm start -- nvim'` — rather than to have ket
///   hand over a tty it may not have.
/// - **Never waited on by the caller.** A [`EditorSurface::reveal`] that blocks
///   until the editor exits is a wedged UI. A background thread reaps the child
///   instead, so a long editing session leaves no zombie either.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessSpawner;

impl Spawner for ProcessSpawner {
    fn spawn(&self, command: &EditorCommand) -> Result<()> {
        // The common failure, and the one that must never look like a hang: a
        // name that is not on PATH. Reported as configuration, because that is
        // what the person has to change.
        spawn_detached(command).map_err(|e| {
            KetError::Config(format!(
                "could not start editor `{}`: {e}. Set ${} to a command on your PATH.",
                command.program.to_string_lossy(),
                EDITOR_VARS[0],
            ))
        })
    }
}

/// Starts `command` with stdio closed and reaps it on a thread of its own.
///
/// The mechanism behind [`ProcessSpawner`], shared with
/// [`reveal_in_file_manager`] so there is one place that knows how ket starts
/// something it is not going to wait for. The `io::Error` comes back bare: what
/// advice to attach to it depends on where the command came from, and only the
/// caller knows that — `$KET_EDITOR` is the wrong thing to mention to somebody
/// whose Finder did not open.
fn spawn_detached(command: &EditorCommand) -> std::io::Result<()> {
    use std::process::Stdio;

    let mut child = std::process::Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// The external surface
// ---------------------------------------------------------------------------

/// Handing files to the editor named by `$KET_EDITOR`.
///
/// Resolved once, at construction, so a misconfigured editor is a startup
/// failure with a message rather than a surprise the first time somebody clicks
/// a line in a diff.
#[derive(Debug, Clone)]
pub struct ExternalSurface {
    program: OsString,
    /// Arguments the user put in `$KET_EDITOR` themselves, e.g. `--wait`.
    leading: Vec<String>,
    /// Position template — from a profile, or from the user's own placeholders.
    template: Vec<String>,
    /// `Some` when a row in [`PROFILES`] matched.
    profile: Option<&'static EditorProfile>,
    /// Whether `template` came from the user rather than from the table.
    custom: bool,
    spawner: Arc<dyn Spawner>,
}

impl ExternalSurface {
    /// Resolves the editor from the real environment.
    pub fn from_env() -> Result<Self> {
        Self::with_lookup(&|key| std::env::var_os(key), Arc::new(ProcessSpawner))
    }

    /// Resolves the editor from an injected environment.
    pub fn with_lookup(lookup: EnvLookup<'_>, spawner: Arc<dyn Spawner>) -> Result<Self> {
        for var in EDITOR_VARS {
            let Some(value) = lookup(var) else { continue };
            let Ok(text) = value.into_string() else {
                return Err(KetError::Config(format!(
                    "${var} is not valid UTF-8; ket will not guess at the bytes of a \
                     command it is about to run"
                )));
            };
            if text.trim().is_empty() {
                continue;
            }
            return Self::from_command_line(&text, spawner);
        }

        Err(KetError::Config(format!(
            "no editor configured: set ${} to the command that opens your editor \
             (for example `code`, `zed`, or `nvim`). Tried {}.",
            EDITOR_VARS[0],
            EDITOR_VARS.join(", "),
        )))
    }

    /// Builds a surface from an editor command line.
    ///
    /// If the command line contains any of `{path}`, `{line}` or `{col}` it is
    /// taken as a complete template and no profile is consulted — the same
    /// convention as `config.editor.open_command`, so the escape hatch for an
    /// editor ket has never heard of is the one already documented there.
    pub fn from_command_line(line: &str, spawner: Arc<dyn Spawner>) -> Result<Self> {
        let mut words = split_command_line(line);
        if words.is_empty() {
            return Err(KetError::Config(format!(
                "${} is empty; set it to the command that opens your editor",
                EDITOR_VARS[0],
            )));
        }

        let program = OsString::from(words.remove(0));

        if has_placeholder(&words) {
            return Ok(Self {
                program,
                leading: Vec::new(),
                template: words,
                profile: None,
                custom: true,
                spawner,
            });
        }

        let profile = profile_for(Path::new(&program));
        let template = profile
            .unwrap_or(&PATH_ONLY)
            .template
            .iter()
            .map(|s| (*s).to_owned())
            .collect();

        Ok(Self {
            program,
            leading: words,
            template,
            profile,
            custom: false,
            spawner,
        })
    }

    /// A surface for an editor binary ket located itself.
    ///
    /// [`Self::from_command_line`] is for a *command line* someone wrote, and
    /// pays for that: it splits on whitespace and honours quotes, so a path
    /// handed to it has to be quoted first. Every macOS editor keeps its
    /// command-line helper inside a bundle whose name has a space in it — see
    /// [`installed_editors`] — and quoting a path back into a string only to
    /// split it again is a round trip with nothing to gain and a quoting bug
    /// to lose. The profile is matched on the file stem exactly as it would
    /// have been, so what runs is the same either way.
    pub fn for_program(program: impl Into<OsString>, spawner: Arc<dyn Spawner>) -> Self {
        let program = program.into();
        let profile = profile_for(Path::new(&program));
        let template = profile
            .unwrap_or(&PATH_ONLY)
            .template
            .iter()
            .map(|s| (*s).to_owned())
            .collect();

        Self {
            program,
            leading: Vec::new(),
            template,
            profile,
            custom: false,
            spawner,
        }
    }

    /// The profile that matched, if ket recognised the editor.
    pub fn profile(&self) -> Option<&'static EditorProfile> {
        self.profile
    }

    /// Whether a position can be handed over, or only a path.
    ///
    /// `false` means an unrecognised editor with no template of its own: it will
    /// still open the file, it just cannot be told where to put the cursor. Worth
    /// surfacing, because the fix is one environment variable.
    pub fn knows_positions(&self) -> bool {
        self.custom || self.profile.is_some()
    }

    /// The exact command that [`ExternalSurface::reveal`] would run.
    ///
    /// Public because it is the testable half, and because a doctor command
    /// showing the argv is a far better answer to "why did nothing open?" than
    /// an exit status.
    pub fn command_for(&self, at: &Position) -> Result<EditorCommand> {
        if !at.path.is_absolute() {
            return Err(KetError::Path {
                what: "file to reveal",
                why: format!(
                    "{} is relative; the editor is not started in ket's directory, so a \
                     relative path would open a different file",
                    at.path.display()
                ),
            });
        }

        let mut args: Vec<OsString> = self.leading.iter().map(OsString::from).collect();
        args.extend(render(&self.template, at));

        Ok(EditorCommand {
            program: self.program.clone(),
            args,
        })
    }
}

impl EditorSurface for ExternalSurface {
    fn caps(&self) -> SurfaceCaps {
        SurfaceCaps {
            // Somebody else's window.
            embedded: false,
            editable: true,
            // ket renders the review; the editor renders the edit.
            diffs: false,
        }
    }

    fn describe(&self) -> String {
        let name = Path::new(&self.program)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.to_string_lossy().into_owned());

        if self.knows_positions() {
            name
        } else {
            format!("{name} (file only)")
        }
    }

    fn reveal(&self, at: &Position) -> Result<()> {
        self.spawner.spawn(&self.command_for(at)?)
    }
}

/// Splits a command line the way a shell would, minus the shell.
///
/// `$KET_EDITOR` is a command line and not a program name: `code --wait` and
/// `"/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl"` are both
/// things people really put in it, and the second one has a space in it on every
/// Mac. Doing the splitting here means the string never reaches a shell, so a
/// path with a space works and a value with a `;` in it cannot become a second
/// command.
///
/// Lenient about an unterminated quote — it closes at the end of the string —
/// because the alternative failure, "could not start editor", already says
/// exactly what is wrong and where.
fn split_command_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    // Inside double quotes a backslash escapes only itself and
                    // the quote, as in POSIX sh; anything else is two literals.
                    match chars.next() {
                        Some(n @ ('"' | '\\')) => current.push(n),
                        Some(n) => {
                            current.push(c);
                            current.push(n);
                        }
                        None => current.push(c),
                    }
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        current.push(n);
                        started = true;
                    }
                }
                c if c.is_whitespace() => {
                    if started {
                        out.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            },
        }
    }

    if started {
        out.push(current);
    }
    out
}

// ---------------------------------------------------------------------------
// Watching
// ---------------------------------------------------------------------------

/// Prefix of the temporary file [`write_if_unchanged`] renames into place.
///
/// Public because [`FileWatcher`] filters it out and anything else watching the
/// same tree should too: a half-written temporary is not a change worth waking a
/// UI for, and it is gone by the time anything could look at it.
pub const TEMP_PREFIX: &str = ".ket-tmp.";

/// Filesystem changes held for a consumer that has not looked yet.
///
/// Bounded on purpose. An agent running
/// `npm install` inside a watched worktree produces tens of thousands of events
/// in a second, and queueing all of them to redraw a file list once is the
/// unbounded buffer that bound exists to prevent. Past the cap events are
/// dropped and [`FileWatcher::overflowed`] says so — which is the honest signal,
/// because the right response to a flood is one full refresh rather than 40,000
/// incremental ones.
pub const MAX_PENDING_CHANGES: usize = 4096;

/// What happened to a path.
///
/// A hint, not a fact. macOS coalesces FSEvents flags, so a plain write to an
/// existing file can arrive as `Created`; only the path is reliable. Anything
/// that must be exact should re-stat the path — see [`FileStamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeKind {
    /// The path appeared, or was renamed into place.
    Created,
    /// The path's contents or metadata changed.
    Modified,
    /// The path went away, or was renamed elsewhere.
    Removed,
}

/// One observed change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Absolute path that changed.
    pub path: PathBuf,
    /// What appears to have happened.
    pub kind: FileChangeKind,
    /// Milliseconds since the Unix epoch, as [`crate::now_ms`] reports it.
    pub at_ms: u64,
}

/// Watches a tree so that writes ket did not make still show up.
///
/// There are always at least two writers: the agent working in the worktree, and
/// the editor the person has the same file open in. Neither tells ket anything.
/// Without this every view in the shell would need a manual refresh, and a stale
/// diff during a review is exactly the failure that makes a review tool
/// untrustworthy.
///
/// Dropping the watcher stops the watch.
pub struct FileWatcher {
    /// Held for its `Drop`: releasing it ends the watch.
    _watcher: notify::RecommendedWatcher,
    rx: Receiver<FileChange>,
    overflowed: Arc<AtomicBool>,
    root: PathBuf,
}

impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher")
            .field("root", &self.root)
            .field("overflowed", &self.overflowed())
            .finish_non_exhaustive()
    }
}

impl FileWatcher {
    /// Starts watching `root`.
    ///
    /// A directory is watched recursively, a file on its own. Anything under a
    /// `.git` directory is filtered out: a worktree's index, lock files and
    /// reflogs change constantly and none of it is a change to a file anybody is
    /// editing, so passing it through would mean the queue is full of noise
    /// precisely when an agent is busy.
    pub fn watch(root: &Path) -> Result<Self> {
        // Canonicalise so the paths in events compare equal to the ones callers
        // hold. On macOS the temp dir is reached through /var, a symlink to
        // /private/var, and FSEvents reports the real one.
        let root = root.canonicalize().map_err(|e| KetError::io(root, e))?;

        let (tx, rx) = sync_channel::<FileChange>(MAX_PENDING_CHANGES);
        let overflowed = Arc::new(AtomicBool::new(false));

        let sink = Sink {
            tx,
            overflowed: Arc::clone(&overflowed),
        };

        let mut watcher = notify::recommended_watcher(move |event| sink.accept(event))
            .map_err(|e| watch_error(&root, e))?;

        let mode = if root.is_dir() {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(&root, mode)
            .map_err(|e| watch_error(&root, e))?;

        Ok(Self {
            _watcher: watcher,
            rx,
            overflowed,
            root,
        })
    }

    /// The canonicalised path being watched.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether events have been dropped since the last [`FileWatcher::drain`].
    ///
    /// `true` means the queued changes are a subset and the correct response is a
    /// full refresh, not a per-file update.
    pub fn overflowed(&self) -> bool {
        self.overflowed.load(Ordering::Relaxed)
    }

    /// The next change, or `None` if none is queued right now.
    pub fn try_next(&self) -> Option<FileChange> {
        self.rx.try_recv().ok()
    }

    /// The next change, waiting up to `timeout`.
    ///
    /// Bounded rather than blocking forever: a watcher that never fires must not
    /// be able to hold a thread (invariant 4).
    pub fn next_within(&self, timeout: Duration) -> Option<FileChange> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Every queued change, one entry per path, newest state first-seen order.
    ///
    /// Coalesced because a single save is several filesystem events — a write, a
    /// rename, a permissions touch — and a consumer wants "this file changed"
    /// once. Clears the overflow flag, so the caller that sees it is the one
    /// responsible for the full refresh.
    pub fn drain(&self) -> Vec<FileChange> {
        let mut order: Vec<PathBuf> = Vec::new();
        let mut latest: std::collections::HashMap<PathBuf, FileChange> =
            std::collections::HashMap::new();

        while let Ok(change) = self.rx.try_recv() {
            if latest.insert(change.path.clone(), change.clone()).is_none() {
                order.push(change.path);
            }
        }

        self.overflowed.store(false, Ordering::Relaxed);
        order
            .into_iter()
            .filter_map(|path| latest.remove(&path))
            .collect()
    }
}

/// Where the watcher's callback puts what it sees.
///
/// Separate from [`FileWatcher`] because the callback runs on `notify`'s own
/// thread and outlives nothing else.
struct Sink {
    tx: SyncSender<FileChange>,
    overflowed: Arc<AtomicBool>,
}

impl Sink {
    fn accept(&self, event: notify::Result<notify::Event>) {
        // A watcher error is not worth tearing the watch down for — notify
        // reports transient ones — but it does mean events were missed, which is
        // exactly what the overflow flag means to a consumer.
        let Ok(event) = event else {
            self.overflowed.store(true, Ordering::Relaxed);
            return;
        };

        let Some(kind) = classify(event.kind) else {
            return;
        };

        let at_ms = now_ms();
        for path in event.paths {
            if is_noise(&path) {
                continue;
            }
            let change = FileChange { path, kind, at_ms };
            if self.tx.try_send(change).is_err() {
                self.overflowed.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// Maps a `notify` event kind onto ket's three, or `None` to ignore it.
fn classify(kind: notify::EventKind) -> Option<FileChangeKind> {
    use notify::EventKind as K;
    use notify::event::{ModifyKind, RenameMode};

    match kind {
        K::Create(_) => Some(FileChangeKind::Created),
        K::Remove(_) => Some(FileChangeKind::Removed),
        // A rename is a removal on one side and a creation on the other. `Both`
        // carries two paths in one event and there is no way to say which is
        // which per path, so it is reported as a modification of both — the
        // consumer re-reads either way.
        K::Modify(ModifyKind::Name(RenameMode::From)) => Some(FileChangeKind::Removed),
        K::Modify(ModifyKind::Name(RenameMode::To)) => Some(FileChangeKind::Created),
        K::Modify(_) => Some(FileChangeKind::Modified),
        K::Any => Some(FileChangeKind::Modified),
        // Opening and reading a file is not a change, and an editor opening a
        // file would otherwise report one on every keystroke on some platforms.
        K::Access(_) => None,
        K::Other => None,
    }
}

/// Whether a path is churn rather than a change somebody wants to see.
fn is_noise(path: &Path) -> bool {
    path.components().any(|c| {
        let name = c.as_os_str();
        name == OsStr::new(".git") || name.to_str().is_some_and(|n| n.starts_with(TEMP_PREFIX))
    })
}

/// Turns a `notify` failure into a [`KetError`].
///
/// Mapped onto [`KetError::Io`] rather than given a variant of its own: the path
/// is the part a person needs, `notify`'s own kinds are platform detail, and a
/// watch failure is genuinely an I/O failure — a directory that vanished, or a
/// per-process descriptor limit.
fn watch_error(path: &Path, error: notify::Error) -> KetError {
    KetError::io(path, std::io::Error::other(error))
}

// ---------------------------------------------------------------------------
// Writing without clobbering
// ---------------------------------------------------------------------------

/// A file as it was when ket last read it.
///
/// The point of the third field is that mtime alone is not enough. Two writes
/// inside one filesystem timestamp tick that happen to produce the same length
/// are indistinguishable, and "an editor saved the file twice in quick
/// succession" is not a rare event — it is what autosave does. A content hash
/// makes the comparison exact for the cost of a read ket has already paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    /// Size in bytes.
    pub len: u64,
    /// Modification time in milliseconds since the epoch, when the filesystem
    /// reports one.
    pub modified_ms: Option<u64>,
    /// A hash of the contents.
    ///
    /// In-memory only, never persisted or compared across runs, so the hash
    /// function is an implementation detail and carries no stability promise.
    pub digest: u64,
}

impl FileStamp {
    /// Stamps contents that have already been read.
    pub fn of_bytes(bytes: &[u8], modified_ms: Option<u64>) -> Self {
        let mut hasher = DefaultHasher::new();
        hasher.write(bytes);
        Self {
            len: bytes.len() as u64,
            modified_ms,
            digest: hasher.finish(),
        }
    }

    /// Stamps the file at `path` as it is right now.
    pub fn of(path: &Path) -> Result<Self> {
        Ok(read(path)?.1)
    }

    /// Whether `path`'s length and modification time still match this stamp,
    /// without reading the file.
    ///
    /// The cheap half of the comparison, for something that looks at an open
    /// file often and expects it to be unchanged nearly every time: one
    /// `stat` instead of a read and a hash of every byte. Deliberately
    /// one-sided. `true` means there is nothing here worth re-reading;
    /// `false` is only ever a prompt to take the exact comparison — a fresh
    /// [`FileStamp::of`] against this one — which is what the digest is for.
    ///
    /// A filesystem that reports no modification time, or reports one in
    /// whole seconds, makes this say `false` more often than it strictly
    /// needs to, and that costs a read rather than a wrong answer. The case
    /// it cannot see is the one the digest exists for and is the reason
    /// nothing that must be exact may use this alone: a second write inside
    /// the same millisecond that happens to land on the same length.
    /// [`write_if_unchanged`] does not use it.
    pub fn metadata_matches(&self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let modified = modified_ms_of(&meta);
        modified.is_some() && modified == self.modified_ms && meta.len() == self.len
    }
}

/// Modification time of `path`, in milliseconds, if the filesystem has one.
pub(crate) fn modified_ms(path: &Path) -> Option<u64> {
    modified_ms_of(&std::fs::metadata(path).ok()?)
}

/// The same, from metadata already in hand — so a caller that has just
/// stat-ed a file does not stat it again.
fn modified_ms_of(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Reads a file together with the stamp that identifies this version of it.
///
/// Bytes rather than a `String`: this is the same path a diff viewer uses, and
/// refusing to open a file because one byte is not UTF-8 is not acceptable
/// behaviour for a tool that shows you what an agent did.
pub fn read(path: &Path) -> Result<(Vec<u8>, FileStamp)> {
    let bytes = std::fs::read(path).map_err(|e| KetError::io(path, e))?;
    let stamp = FileStamp::of_bytes(&bytes, modified_ms(path));
    Ok((bytes, stamp))
}

/// Writes `contents` to `path`, but only if the file is still the one that was
/// read.
///
/// The conflict this prevents is concrete and happens daily in ket's own use:
/// the file is open in the user's editor *and* an agent is working in the same
/// worktree. If either of them wrote since ket read, an unconditional write
/// silently destroys their work — and because the editor still holds the old
/// buffer, the person would not find out until much later.
///
/// Returns [`KetError::Conflict`] rather than writing when the file changed or
/// was deleted. The caller's job is then to reload and decide, which is a choice
/// only a person can make.
///
/// The write itself goes to a temporary file in the same directory and is
/// renamed into place, so an interrupted write cannot leave a truncated source
/// file behind, and a watcher sees one change rather than a file that is briefly
/// empty. The original's permissions are carried over — a rename would otherwise
/// silently un-execute a script.
pub fn write_if_unchanged(path: &Path, expected: &FileStamp, contents: &[u8]) -> Result<FileStamp> {
    let stamp = match read(path) {
        Ok((_, stamp)) => stamp,
        // A file that vanished is usually an agent that decided to delete it.
        // Recreating it from a buffer read before that decision would silently
        // undo the change under review.
        Err(KetError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(KetError::Conflict(format!(
                "{} was deleted since it was read; nothing was written",
                path.display()
            )));
        }
        Err(e) => return Err(e),
    };

    if stamp != *expected {
        return Err(KetError::Conflict(format!(
            "{} changed on disk since it was read; reload before saving",
            path.display()
        )));
    }

    let parent = path.parent().ok_or_else(|| KetError::Path {
        what: "directory of the file to write",
        why: format!("{} has no parent", path.display()),
    })?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let temp = parent.join(format!("{TEMP_PREFIX}{}.{name}", std::process::id()));

    let write = || -> Result<()> {
        std::fs::write(&temp, contents).map_err(|e| KetError::io(&temp, e))?;
        if let Ok(meta) = std::fs::metadata(path) {
            // Best effort: a filesystem that cannot carry permissions across is
            // not a reason to refuse the write.
            let _ = std::fs::set_permissions(&temp, meta.permissions());
        }
        std::fs::rename(&temp, path).map_err(|e| KetError::io(path, e))
    };

    match write() {
        Ok(()) => Ok(FileStamp::of_bytes(contents, modified_ms(path))),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spawner that records instead of running anything.
    #[derive(Debug, Default)]
    struct Recorder(std::sync::Mutex<Vec<Vec<String>>>);

    impl Spawner for Recorder {
        fn spawn(&self, command: &EditorCommand) -> Result<()> {
            self.0.lock().unwrap().push(command.argv_lossy());
            Ok(())
        }
    }

    fn surface(command_line: &str) -> ExternalSurface {
        ExternalSurface::from_command_line(command_line, Arc::new(Recorder::default())).unwrap()
    }

    fn argv(command_line: &str, at: &Position) -> Vec<String> {
        surface(command_line).command_for(at).unwrap().argv_lossy()
    }

    #[test]
    fn vs_code_takes_the_position_as_a_suffix_after_goto() {
        let at = Position::file("/w/src/main.rs")
            .with_line(12)
            .with_column(4);
        assert_eq!(argv("code", &at), ["code", "--goto", "/w/src/main.rs:12:4"]);
    }

    #[test]
    fn zed_takes_the_position_as_a_bare_suffix() {
        let at = Position::file("/w/src/main.rs")
            .with_line(12)
            .with_column(4);
        assert_eq!(argv("zed", &at), ["zed", "/w/src/main.rs:12:4"]);
    }

    #[test]
    fn neovim_takes_the_line_as_an_ex_command_before_the_path() {
        let at = Position::file("/w/src/main.rs")
            .with_line(12)
            .with_column(4);
        assert_eq!(argv("nvim", &at), ["nvim", "+12", "/w/src/main.rs"]);
    }

    #[test]
    fn a_missing_column_trims_the_separator_that_would_have_led_to_it() {
        let at = Position::file("/w/a.rs").with_line(9);
        assert_eq!(argv("code", &at), ["code", "--goto", "/w/a.rs:9"]);
        assert_eq!(argv("zed", &at), ["zed", "/w/a.rs:9"]);
        assert_eq!(argv("emacsclient", &at), ["emacsclient", "+9", "/w/a.rs"]);
        assert_eq!(argv("nano", &at), ["nano", "+9", "/w/a.rs"]);
    }

    #[test]
    fn a_missing_line_leaves_only_the_path() {
        let at = Position::file("/w/a.rs");
        assert_eq!(argv("code", &at), ["code", "--goto", "/w/a.rs"]);
        assert_eq!(argv("zed", &at), ["zed", "/w/a.rs"]);
        assert_eq!(argv("nvim", &at), ["nvim", "/w/a.rs"]);
        assert_eq!(argv("emacsclient", &at), ["emacsclient", "/w/a.rs"]);
        assert_eq!(argv("mate", &at), ["mate", "/w/a.rs"]);
    }

    #[test]
    fn a_flag_is_dropped_along_with_the_value_it_introduces() {
        // The shape rule 3 exists for: `--column` without a column is worse than
        // no flag, and JetBrains editors reject it outright.
        let full = Position::file("/w/a.rs").with_line(3).with_column(7);
        assert_eq!(
            argv("idea", &full),
            ["idea", "--line", "3", "--column", "7", "/w/a.rs"]
        );

        let line_only = Position::file("/w/a.rs").with_line(3);
        assert_eq!(argv("idea", &line_only), ["idea", "--line", "3", "/w/a.rs"]);

        let bare = Position::file("/w/a.rs");
        assert_eq!(argv("idea", &bare), ["idea", "/w/a.rs"]);
    }

    #[test]
    fn an_unknown_editor_opens_the_file_rather_than_failing() {
        let at = Position::file("/w/a.rs").with_line(3).with_column(7);
        let s = surface("acme");
        assert!(!s.knows_positions());
        assert_eq!(
            s.command_for(&at).unwrap().argv_lossy(),
            ["acme", "/w/a.rs"]
        );
        assert_eq!(s.describe(), "acme (file only)");
    }

    #[test]
    fn the_editor_is_matched_by_the_binarys_name_not_its_path() {
        let at = Position::file("/w/a.rs").with_line(3);
        assert_eq!(
            argv("/opt/homebrew/bin/zed", &at),
            ["/opt/homebrew/bin/zed", "/w/a.rs:3"]
        );
    }

    #[test]
    fn a_users_own_arguments_survive_in_front_of_the_template() {
        let at = Position::file("/w/a.rs").with_line(3);
        assert_eq!(
            argv("code --wait --new-window", &at),
            ["code", "--wait", "--new-window", "--goto", "/w/a.rs:3"]
        );
    }

    #[test]
    fn placeholders_in_the_editor_variable_replace_the_profile_entirely() {
        let at = Position::file("/w/a.rs").with_line(3).with_column(7);
        assert_eq!(
            argv("acme --file {path} --line {line} --col {col}", &at),
            ["acme", "--file", "/w/a.rs", "--line", "3", "--col", "7"]
        );

        let bare = Position::file("/w/a.rs");
        assert_eq!(
            argv("acme --file {path} --line {line} --col {col}", &bare),
            ["acme", "--file", "/w/a.rs"]
        );
    }

    #[test]
    fn a_quoted_path_with_a_space_stays_one_argument() {
        let editor = "\"/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl\" --wait";
        let at = Position::file("/w/a.rs").with_line(3);
        assert_eq!(
            argv(editor, &at),
            [
                "/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl",
                "--wait",
                "/w/a.rs:3",
            ]
        );
    }

    #[test]
    fn splitting_handles_quotes_escapes_and_runs_of_whitespace() {
        assert_eq!(split_command_line("code  --wait"), ["code", "--wait"]);
        assert_eq!(split_command_line("'a b' c"), ["a b", "c"]);
        assert_eq!(split_command_line("a\\ b"), ["a b"]);
        assert_eq!(split_command_line("\"a\\\"b\""), ["a\"b"]);
        assert_eq!(split_command_line("   "), Vec::<String>::new());
        // Unterminated quote closes at the end rather than erroring.
        assert_eq!(split_command_line("code \"a b"), ["code", "a b"]);
    }

    #[test]
    fn a_column_without_a_line_is_dropped_rather_than_rendered_as_a_dangling_separator() {
        let at = Position::file("/w/a.rs").with_column(7);
        assert_eq!(at.col, None);
        assert_eq!(argv("zed", &at), ["zed", "/w/a.rs"]);
    }

    #[test]
    fn a_zero_line_is_clamped_because_every_editor_here_is_one_based() {
        let at = Position::file("/w/a.rs").with_line(0).with_column(0);
        assert_eq!((at.line, at.col), (Some(1), Some(1)));
    }

    #[test]
    fn a_relative_path_is_refused_rather_than_resolved_against_an_unknown_directory() {
        let at = Position::file("src/main.rs").with_line(3);
        assert!(matches!(
            surface("code").command_for(&at),
            Err(KetError::Path { .. })
        ));
    }

    #[test]
    fn revealing_spawns_exactly_the_command_that_was_computed() {
        // The seam: what `reveal` runs is what `command_for` says it runs.
        let recorder = Arc::new(Recorder::default());
        let surface =
            ExternalSurface::from_command_line("code --wait", Arc::clone(&recorder) as Arc<_>)
                .unwrap();
        let at = Position::file("/w/a.rs").with_line(3).with_column(7);

        surface.reveal(&at).unwrap();

        assert_eq!(
            recorder.0.lock().unwrap().as_slice(),
            [surface.command_for(&at).unwrap().argv_lossy()]
        );
    }

    #[test]
    fn the_external_surface_is_not_embedded_and_does_not_render_diffs() {
        // Both are why `crate::diff` exists and why the shell draws the review.
        let caps = surface("code").caps();
        assert_eq!(
            caps,
            SurfaceCaps {
                embedded: false,
                editable: true,
                diffs: false,
            }
        );
    }

    #[test]
    fn a_worktree_relative_path_is_joined_onto_the_root() {
        let at = Position::in_worktree("/w", "src/main.rs");
        assert_eq!(at.path, Path::new("/w/src/main.rs"));
    }

    #[test]
    fn every_profile_renders_a_command_for_every_amount_of_position() {
        // The table is data, and the rules must hold for all of it: whatever a
        // row says, the path always survives.
        for profile in PROFILES {
            let s = surface(profile.program);
            for at in [
                Position::file("/w/a.rs"),
                Position::file("/w/a.rs").with_line(4),
                Position::file("/w/a.rs").with_line(4).with_column(2),
            ] {
                let argv = s.command_for(&at).unwrap().argv_lossy();
                assert!(
                    argv.iter().any(|a| a.contains("/w/a.rs")),
                    "{}: lost the path: {argv:?}",
                    profile.program
                );
                assert!(
                    !argv.iter().any(|a| a.contains('{')),
                    "{}: unsubstituted placeholder: {argv:?}",
                    profile.program
                );
            }
        }
    }

    #[test]
    fn no_two_profiles_claim_the_same_program() {
        let mut seen = std::collections::BTreeSet::new();
        for profile in PROFILES {
            assert!(
                seen.insert(profile.program),
                "duplicate: {}",
                profile.program
            );
        }
    }

    #[test]
    fn braces_that_are_not_placeholders_are_left_alone() {
        let at = Position::file("/w/a.rs").with_line(3);
        assert_eq!(
            argv("acme {path} {nope}", &at),
            ["acme", "/w/a.rs", "{nope}"]
        );
    }

    #[test]
    fn git_churn_and_our_own_temporaries_are_not_reported_as_changes() {
        assert!(is_noise(Path::new("/w/.git/index.lock")));
        assert!(is_noise(Path::new("/w/.git")));
        assert!(is_noise(Path::new("/w/.ket-tmp.900.main.rs")));
        assert!(!is_noise(Path::new("/w/src/main.rs")));
        assert!(!is_noise(Path::new("/w/gitignore")));
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        use notify::event::AccessKind;
        assert_eq!(classify(notify::EventKind::Access(AccessKind::Any)), None);
    }

    /// A throwaway directory, removed when the guard drops.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ket-surface-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // macOS puts the temp dir behind /private; canonicalise so paths
            // compared against what the watcher reports are the same ones.
            let dir = dir.canonicalize().unwrap_or(dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_stamp_of_bytes_matches_a_stamp_taken_from_disk() {
        let dir = TempDir::new("stamp");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"hello").unwrap();

        let from_disk = FileStamp::of(&path).unwrap();
        let from_bytes = FileStamp::of_bytes(b"hello", from_disk.modified_ms);
        assert_eq!(from_disk, from_bytes);
    }

    #[test]
    fn file_stamp_of_missing_file_is_an_error() {
        let dir = TempDir::new("stamp-missing");
        assert!(FileStamp::of(&dir.0.join("nope.txt")).is_err());
    }

    #[test]
    fn different_contents_never_share_a_digest() {
        let a = FileStamp::of_bytes(b"one", None);
        let b = FileStamp::of_bytes(b"two", None);
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn metadata_matches_is_false_once_the_file_is_gone() {
        let dir = TempDir::new("stamp-gone");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        let stamp = FileStamp::of(&path).unwrap();

        std::fs::remove_file(&path).unwrap();
        assert!(!stamp.metadata_matches(&path));
    }

    #[test]
    fn metadata_matches_is_false_without_a_modification_time() {
        let stamp = FileStamp {
            len: 5,
            modified_ms: None,
            digest: 0,
        };
        let dir = TempDir::new("stamp-no-mtime");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        assert!(!stamp.metadata_matches(&path));
    }

    #[test]
    fn write_if_unchanged_writes_when_the_stamp_still_matches() {
        let dir = TempDir::new("write-ok");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"before").unwrap();
        let stamp = FileStamp::of(&path).unwrap();

        let new_stamp = write_if_unchanged(&path, &stamp, b"after").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"after");
        assert_eq!(new_stamp, FileStamp::of(&path).unwrap());
    }

    #[test]
    fn write_if_unchanged_refuses_when_the_file_changed_since_it_was_read() {
        let dir = TempDir::new("write-changed");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"before").unwrap();
        let stamp = FileStamp::of(&path).unwrap();

        // Someone else writes in between.
        std::fs::write(&path, b"someone else's edit").unwrap();

        let err = write_if_unchanged(&path, &stamp, b"after").expect_err("should refuse");
        assert!(matches!(err, KetError::Conflict(_)));
        assert_eq!(std::fs::read(&path).unwrap(), b"someone else's edit");
    }

    #[test]
    fn write_if_unchanged_refuses_when_the_file_was_deleted() {
        let dir = TempDir::new("write-deleted");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"before").unwrap();
        let stamp = FileStamp::of(&path).unwrap();

        std::fs::remove_file(&path).unwrap();

        let err = write_if_unchanged(&path, &stamp, b"after").expect_err("should refuse");
        assert!(matches!(err, KetError::Conflict(_)));
        assert!(err.to_string().contains("deleted"));
    }

    #[test]
    fn write_if_unchanged_leaves_no_temp_file_behind_on_success() {
        let dir = TempDir::new("write-no-temp");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"before").unwrap();
        let stamp = FileStamp::of(&path).unwrap();

        write_if_unchanged(&path, &stamp, b"after").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(TEMP_PREFIX))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "left a temp file behind: {leftovers:?}"
        );
    }

    #[test]
    fn read_returns_bytes_and_a_matching_stamp() {
        let dir = TempDir::new("read");
        let path = dir.0.join("file.txt");
        std::fs::write(&path, b"contents").unwrap();

        let (bytes, stamp) = read(&path).unwrap();
        assert_eq!(bytes, b"contents");
        assert_eq!(stamp, FileStamp::of(&path).unwrap());
    }

    #[test]
    fn read_of_a_missing_file_is_an_error() {
        let dir = TempDir::new("read-missing");
        assert!(read(&dir.0.join("nope.txt")).is_err());
    }

    #[test]
    fn a_watcher_reports_a_new_file_as_a_change() {
        let dir = TempDir::new("watch-create");
        let watcher = FileWatcher::watch(&dir.0).unwrap();
        assert_eq!(watcher.root(), dir.0.as_path());
        assert!(!watcher.overflowed());

        std::fs::write(dir.0.join("new.txt"), b"hi").unwrap();

        let change = watcher.next_within(Duration::from_secs(5));
        assert!(change.is_some(), "expected a change to be reported");
    }

    #[test]
    fn a_watcher_ignores_git_internals() {
        let dir = TempDir::new("watch-git");
        std::fs::create_dir(dir.0.join(".git")).unwrap();
        let watcher = FileWatcher::watch(&dir.0).unwrap();

        std::fs::write(dir.0.join(".git").join("index.lock"), b"x").unwrap();
        std::fs::write(dir.0.join("real.txt"), b"hi").unwrap();

        // Whatever arrives, it must be the real file, never the git-internal one.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_real = false;
        while std::time::Instant::now() < deadline {
            match watcher.try_next() {
                Some(change) => {
                    assert!(
                        !change.path.starts_with(dir.0.join(".git")),
                        "reported a git-internal path: {change:?}"
                    );
                    if change.path == dir.0.join("real.txt") {
                        saw_real = true;
                        break;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        assert!(saw_real, "never saw the real file reported");
    }

    #[test]
    fn drain_collects_every_pending_change_at_once() {
        let dir = TempDir::new("watch-drain");
        let watcher = FileWatcher::watch(&dir.0).unwrap();

        std::fs::write(dir.0.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.0.join("b.txt"), b"b").unwrap();

        // Give the watcher time to observe both before draining.
        let _ = watcher.next_within(Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(200));

        let drained = watcher.drain();
        assert!(!drained.is_empty());
        assert!(watcher.try_next().is_none(), "drain must take everything");
    }

    #[test]
    fn a_file_change_debug_names_its_path_and_kind() {
        let change = FileChange {
            path: PathBuf::from("/w/file.txt"),
            kind: FileChangeKind::Modified,
            at_ms: 0,
        };
        assert!(format!("{change:?}").contains("file.txt"));
    }

    #[test]
    fn profile_for_matches_the_file_stem_case_insensitively() {
        assert_eq!(
            profile_for(Path::new("/usr/local/bin/CODE")).unwrap().label,
            "VS Code"
        );
        assert_eq!(profile_for(Path::new("zed")).unwrap().label, "Zed");
        assert!(profile_for(Path::new("not-a-real-editor")).is_none());
    }

    #[test]
    fn editor_command_argv_lossy_puts_the_program_first() {
        let command = EditorCommand {
            program: OsString::from("code"),
            args: vec![OsString::from("--goto"), OsString::from("/w/a.rs:1:1")],
        };
        assert_eq!(command.argv_lossy(), ["code", "--goto", "/w/a.rs:1:1"]);
    }

    #[test]
    fn knows_positions_is_true_for_a_matched_profile_or_a_custom_template() {
        assert!(surface("code").knows_positions());
        assert!(surface("some-unknown-thing {path}:{line}").knows_positions());
        assert!(!surface("some-unknown-thing").knows_positions());
    }

    #[test]
    fn describe_notes_file_only_for_an_editor_with_no_position_template() {
        assert_eq!(surface("code").describe(), "code");
        assert_eq!(
            surface("some-unknown-thing").describe(),
            "some-unknown-thing (file only)"
        );
    }

    #[test]
    fn for_program_matches_the_profile_from_an_unquoted_bundle_path() {
        let s = ExternalSurface::for_program(
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
            Arc::new(Recorder::default()),
        );
        assert_eq!(s.profile().unwrap().label, "VS Code");
    }

    #[test]
    fn from_command_line_refuses_an_empty_string() {
        let err = ExternalSurface::from_command_line("   ", Arc::new(Recorder::default()))
            .expect_err("should refuse");
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn with_lookup_skips_a_blank_variable_and_falls_through_to_the_next() {
        let lookup = |key: &str| -> Option<OsString> {
            match key {
                "KET_EDITOR" => Some(OsString::from("  ")),
                "EDITOR" => Some(OsString::from("zed")),
                _ => None,
            }
        };
        let s = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default())).unwrap();
        assert_eq!(s.profile().unwrap().label, "Zed");
    }

    #[test]
    fn with_lookup_refuses_non_utf8_and_names_the_variable() {
        use std::os::unix::ffi::OsStringExt;
        let lookup = |key: &str| -> Option<OsString> {
            (key == "KET_EDITOR").then(|| OsString::from_vec(vec![0xff, 0xfe]))
        };
        let err = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default()))
            .expect_err("should refuse");
        assert!(err.to_string().contains("KET_EDITOR"));
    }

    #[test]
    fn with_lookup_names_every_variable_it_tried_when_none_is_set() {
        let lookup = |_: &str| -> Option<OsString> { None };
        let err = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default()))
            .expect_err("should refuse");
        for var in EDITOR_VARS {
            assert!(err.to_string().contains(var), "{err} missing {var}");
        }
    }

    #[test]
    fn split_command_line_closes_an_unterminated_quote_at_the_end() {
        assert_eq!(split_command_line("code \"a b"), ["code", "a b"]);
    }

    #[test]
    fn split_command_line_of_only_whitespace_is_empty() {
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn classify_maps_every_notify_event_kind() {
        use notify::EventKind as K;
        use notify::event::{ModifyKind, RenameMode};

        assert_eq!(
            classify(K::Create(notify::event::CreateKind::Any)),
            Some(FileChangeKind::Created)
        );
        assert_eq!(
            classify(K::Remove(notify::event::RemoveKind::Any)),
            Some(FileChangeKind::Removed)
        );
        assert_eq!(
            classify(K::Modify(ModifyKind::Name(RenameMode::From))),
            Some(FileChangeKind::Removed)
        );
        assert_eq!(
            classify(K::Modify(ModifyKind::Name(RenameMode::To))),
            Some(FileChangeKind::Created)
        );
        assert_eq!(
            classify(K::Modify(ModifyKind::Data(notify::event::DataChange::Any))),
            Some(FileChangeKind::Modified)
        );
        assert_eq!(classify(K::Any), Some(FileChangeKind::Modified));
        assert_eq!(classify(K::Access(notify::event::AccessKind::Any)), None);
        assert_eq!(classify(K::Other), None);
    }

    #[test]
    fn is_noise_flags_a_temp_file_and_a_deeply_nested_git_path() {
        assert!(is_noise(Path::new("/w/.ket-tmp.123.foo")));
        assert!(is_noise(Path::new("/w/.git/refs/heads/main")));
        assert!(!is_noise(Path::new("/w/src/main.rs")));
    }

    #[test]
    fn file_manager_command_reveals_with_open_dash_r() {
        let command = file_manager_command(Path::new("/w/a.rs")).unwrap();
        assert_eq!(command.argv_lossy(), ["/usr/bin/open", "-R", "/w/a.rs"]);
    }

    #[test]
    fn a_fresh_watcher_reports_its_canonicalised_root_and_no_pending_changes() {
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir =
            Temp(std::env::temp_dir().join(format!("surface-watch-root-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();

        let watcher = FileWatcher::watch(&dir.0).unwrap();
        assert_eq!(watcher.root(), dir.0.canonicalize().unwrap());
        assert!(!watcher.overflowed());
        assert!(watcher.try_next().is_none());
        assert!(watcher.next_within(Duration::from_millis(50)).is_none());
    }
}
