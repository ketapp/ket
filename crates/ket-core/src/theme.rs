//! Colour as data, not as constants scattered through the renderer.
//!
//! The Epic 5 spike hardcodes hex values in a `theme` module inside `ket-ui`.
//! That was fine for a spike and is not fine for anything that outlives one:
//! every renderer written against `0x7ee08a` is a renderer nobody can retheme
//! without finding every call site, and the number of call sites only grows.
//! Epic 10 replaces that module with tokens defined here — this file is the
//! foundation it needs to exist first, per invariant 1: state and computation
//! live in `ket-core`, and the shell only turns a token into a `gpui` colour.
//!
//! A theme is a fixed set of **semantic** tokens — `surface`, `text.primary`,
//! `diff.added` — never raw colours at the call site. The distinction is the
//! whole point: a renderer that names `0x7ee08a` cannot be re-themed, one that
//! names `diff.added` can, because the token's meaning outlives whatever colour
//! it currently maps to.
//!
//! Two ideas are kept deliberately separate:
//!
//! - [`Appearance`] is a property *of a theme*: is this a dark palette or a
//!   light one. It exists so a user's TOML file can say "I am a dark theme"
//!   and get the shipped dark theme's tokens for anything it does not override.
//! - [`AppearancePreference`] is a property *of the user*: which of the two
//!   they want, or a third state neither `Appearance` variant can express —
//!   follow whatever macOS is currently set to, and switch when it changes.
//!   Core does not read the OS setting; that is the shell's job. Core only
//!   needs to represent the preference so it can be stored and acted on.

use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{KetError, Result, paths};

/// An RGB colour.
///
/// No alpha: nothing in the token set composites with translucency, and
/// adding a channel nothing uses would just be another way for two colours to
/// look equal and not be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red, 0-255.
    pub r: u8,
    /// Green, 0-255.
    pub g: u8,
    /// Blue, 0-255.
    pub b: u8,
}

impl Color {
    /// Builds a colour from its three channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `self` moved `amount` of the way towards `other`, channel by channel.
    ///
    /// `amount` is clamped to `[0, 1]`: 0 is `self`, 1 is `other`.
    pub fn mix(self, other: Color, amount: f64) -> Color {
        let amount = amount.clamp(0.0, 1.0);
        let channel = |from: u8, to: u8| {
            (f64::from(from) + (f64::from(to) - f64::from(from)) * amount).round() as u8
        };
        Color::new(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
        )
    }

    /// Parses `#rrggbb` (case-insensitive; the `#` is optional).
    ///
    /// Anything else — wrong length, non-hex digits, a `#rrggbbaa` some other
    /// tool might have written — is rejected with the original text in the
    /// message, because a colour that failed to parse is a config error a
    /// person has to go fix, not a mistake to guess around.
    pub fn from_hex(text: &str) -> Result<Self> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        let bad = || KetError::Config(format!("not a colour, want #rrggbb: {text:?}"));

        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }

        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| bad());
        Ok(Self {
            r: channel(0)?,
            g: channel(2)?,
            b: channel(4)?,
        })
    }

    /// Renders as `#rrggbb`, lowercase.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// WCAG relative luminance.
    ///
    /// The formula from the spec: each channel is linearised (sRGB's gamma
    /// curve undone) before being weighted, because contrast is a statement
    /// about light, and 8-bit sRGB values are not linear in light.
    fn relative_luminance(self) -> f64 {
        fn channel(value: u8) -> f64 {
            let c = f64::from(value) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }

        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }
}

impl std::fmt::Display for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

// Colours are stored in TOML as `#rrggbb` strings — the format everyone
// already reads and writes — rather than as three integer fields, which would
// make a theme file three times as long and no more readable.
impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Color::from_hex(&text).map_err(serde::de::Error::custom)
    }
}

/// The contrast ratio between two colours, per WCAG 2's formula.
///
/// Order does not matter — the lighter of the two always ends up as the
/// numerator — and the range is `[1.0, 21.0]`. `1.0` is two identical
/// luminances; `21.0` is black against white, which is why that pair makes a
/// clean fixture for a test.
pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (la, lb) = (a.relative_luminance() + 0.05, b.relative_luminance() + 0.05);
    if la > lb { la / lb } else { lb / la }
}

/// How prominent a token is, which decides the WCAG AA bar it must clear.
///
/// WCAG only defines two thresholds — 4.5:1 for normal text, 3:1 for large
/// text — and non-text UI components (WCAG 1.4.11, focus rings and the like)
/// share that same 3:1 number. Rather than add a third bucket that would
/// carry the identical minimum, accents and non-text chrome are grouped under
/// [`Prominence::Accent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prominence {
    /// Body text: 4.5:1.
    Body,
    /// Large or bold text, and non-text UI components such as a focus ring: 3:1.
    Accent,
}

impl Prominence {
    /// The WCAG AA minimum ratio for this level of prominence.
    pub fn minimum_ratio(self) -> f64 {
        match self {
            Prominence::Body => 4.5,
            Prominence::Accent => 3.0,
        }
    }
}

/// One token pair that failed its WCAG AA minimum.
#[derive(Debug, Clone, PartialEq)]
pub struct ContrastFailure {
    /// The token drawn as foreground, e.g. `"text.primary"`.
    pub foreground: &'static str,
    /// The token it is drawn against, e.g. `"surface"`.
    pub background: &'static str,
    /// The ratio the theme actually achieves.
    pub ratio: f64,
    /// The WCAG AA minimum this pair needed to clear.
    pub required: f64,
}

impl std::fmt::Display for ContrastFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} on {} is {:.2}:1, needs {:.1}:1",
            self.foreground, self.background, self.ratio, self.required
        )
    }
}

/// Every token pair whose contrast matters, and how prominent each is.
///
/// This is the fixed list of pairs a theme is checked against. It is not
/// exhaustive of every possible token combination — nobody draws
/// `diff.hunk_header` text on `panel` — it is the set that corresponds to how
/// `ket-ui` actually composites tokens today.
fn contrast_pairs(theme: &Theme) -> [(&'static str, &'static str, Color, Color, Prominence); 24] {
    [
        (
            "text.primary",
            "surface",
            theme.text.primary,
            theme.surface,
            Prominence::Body,
        ),
        (
            "text.primary",
            "panel",
            theme.text.primary,
            theme.panel,
            Prominence::Body,
        ),
        (
            "text.primary",
            "selection",
            theme.text.primary,
            theme.selection,
            Prominence::Body,
        ),
        (
            "text.dim",
            "surface",
            theme.text.dim,
            theme.surface,
            Prominence::Body,
        ),
        (
            "text.dim",
            "panel",
            theme.text.dim,
            theme.panel,
            Prominence::Body,
        ),
        (
            "text.primary",
            "elevated",
            theme.text.primary,
            theme.elevated,
            Prominence::Body,
        ),
        (
            "text.dim",
            "elevated",
            theme.text.dim,
            theme.elevated,
            Prominence::Body,
        ),
        (
            "text.primary",
            "hover",
            theme.text.primary,
            theme.hover,
            Prominence::Body,
        ),
        (
            "text.dim",
            "hover",
            theme.text.dim,
            theme.hover,
            Prominence::Body,
        ),
        // The one pair that is easy to get wrong by eye: a primary button's
        // label sits on `accent`, and an accent chosen for how it looks
        // against the window is not thereby readable under white text.
        (
            "on_accent",
            "accent",
            theme.on_accent,
            theme.accent,
            Prominence::Body,
        ),
        (
            "diff.added",
            "surface",
            theme.diff.added,
            theme.surface,
            Prominence::Body,
        ),
        (
            "diff.removed",
            "surface",
            theme.diff.removed,
            theme.surface,
            Prominence::Body,
        ),
        (
            "diff.modified",
            "surface",
            theme.diff.modified,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "diff.context",
            "surface",
            theme.diff.context,
            theme.surface,
            Prominence::Body,
        ),
        (
            "diff.hunk_header",
            "surface",
            theme.diff.hunk_header,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "status.running",
            "surface",
            theme.status.running,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "status.attention",
            "surface",
            theme.status.attention,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "status.failed",
            "surface",
            theme.status.failed,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "status.merging",
            "surface",
            theme.status.merging,
            theme.surface,
            Prominence::Accent,
        ),
        // The quota ramp is drawn as a bar and as the figure beside it, both
        // on the panel; `surface` and `panel` are one colour in every shipped
        // theme, so one pair each covers both.
        (
            "quota.warm",
            "surface",
            theme.quota.warm,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "quota.hot",
            "surface",
            theme.quota.hot,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "quota.critical",
            "surface",
            theme.quota.critical,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "focus_ring",
            "surface",
            theme.focus_ring,
            theme.surface,
            Prominence::Accent,
        ),
        (
            "terminal.foreground",
            "terminal.background",
            theme.terminal.foreground,
            theme.terminal.background,
            Prominence::Body,
        ),
    ]
}

/// Every pair in a theme that fails its WCAG AA minimum.
///
/// Empty means the theme passes outright. This is meant to run in a test
/// against every shipped theme — see `tests/theme.rs` — so that a theme
/// failing its own accessibility bar is a build failure, not something a user
/// discovers by squinting at diff output.
pub fn contrast_failures(theme: &Theme) -> Vec<ContrastFailure> {
    contrast_pairs(theme)
        .into_iter()
        .filter_map(|(foreground, background, fg, bg, prominence)| {
            let ratio = contrast_ratio(fg, bg);
            let required = prominence.minimum_ratio();
            (ratio < required).then_some(ContrastFailure {
                foreground,
                background,
                ratio,
                required,
            })
        })
        .collect()
}

/// Whether a theme is a dark palette or a light one.
///
/// A property of the theme itself — see the module docs for why this is kept
/// distinct from [`AppearancePreference`], which is a property of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Light text on a dark ground.
    #[default]
    Dark,
    /// Dark text on a light ground.
    Light,
}

/// Which appearance the user wants the shell to render.
///
/// [`Appearance`] can only ever be `Dark` or `Light` — a theme has to be one
/// or the other to have tokens at all. This type adds the third state a
/// *preference* can be that a theme cannot: follow whatever macOS is
/// currently set to, and switch live when it changes. Core does not read the
/// OS setting — it has no window to receive the notification in — it only
/// stores which of the three the user asked for, so the shell has something
/// to resolve against `NSApplication`'s effective appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearancePreference {
    /// Always dark, regardless of the OS setting.
    ///
    /// The default. Following the OS is the more deferential choice and is one
    /// setting away, but ket is a tool for reading diffs and agent output for
    /// hours, and dark is what it is designed and contrast-checked against.
    /// Shipping `System` meant a light-mode Mac got the light palette on first
    /// run without anyone having chosen it.
    #[default]
    Dark,
    /// Always light, regardless of the OS setting.
    Light,
    /// Whatever macOS is currently set to; follows it live.
    System,
}

impl AppearancePreference {
    /// Resolves the preference to a concrete [`Appearance`].
    ///
    /// `system` is the shell's current read of the OS setting — core cannot
    /// supply it, so the caller does.
    pub fn resolve(self, system: Appearance) -> Appearance {
        match self {
            AppearancePreference::Dark => Appearance::Dark,
            AppearancePreference::Light => Appearance::Light,
            AppearancePreference::System => system,
        }
    }
}

/// Text colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextTokens {
    /// The colour most text is drawn in.
    pub primary: Color,
    /// De-emphasised text: summaries, secondary metadata, placeholders.
    pub dim: Color,
    /// The line under a worktree's name in the sidebar — what its agent is
    /// doing — when it has nothing urgent to say. `None` draws it in `dim`,
    /// as most themes do; a theme with hues to spare can give it one.
    #[serde(default)]
    pub note: Option<Color>,
}

/// Diff-pane colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffTokens {
    /// An added line.
    pub added: Color,
    /// A removed line.
    pub removed: Color,
    /// A line that stands where a different one stood.
    ///
    /// A unified diff has no use for this — it renders a modification as a
    /// removal and an addition — but a rail beside an editor's line numbers
    /// does: the line is still there, and saying "changed" about it is both
    /// shorter and truer than drawing it twice. A third hue rather than a
    /// shade of `added`, because "this line is new" and "this line is not
    /// what it was" are the two things a reader is telling apart.
    ///
    /// Amber, which is where every tool that shows git state puts "modified"
    /// — the colour a changed file's name is drawn in — rather than the blue
    /// some editors use in the gutter alone. Green, amber and red is the set
    /// a reader already knows the meaning of without being told.
    pub modified: Color,
    /// Unchanged context around a change.
    pub context: Color,
    /// The `@@ -a,b +c,d @@` line introducing a hunk.
    pub hunk_header: Color,
}

/// Colours for highlighted source, in the editor pane.
///
/// Six, and deliberately not more. A palette with thirty token kinds in it is
/// one nobody can hold in their head while choosing colours, and the extra
/// twenty-four are distinctions a *reader* does not make — the eye separates
/// "this is prose the compiler ignores" from "this is a literal" from "this is
/// structure", and beyond that it is decoration. The highlighter here is a
/// lexer, not a parser, so it could not honestly fill a larger palette anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntaxTokens {
    /// A language keyword: `fn`, `return`, `if`.
    pub keyword: Color,
    /// A string or character literal, and anything inside one.
    pub string: Color,
    /// A comment, of any shape.
    ///
    /// The one token that should be *quieter* than plain text rather than
    /// louder. A comment lit brighter than the code it explains inverts what
    /// somebody skimming is looking for.
    pub comment: Color,
    /// A numeric literal.
    pub number: Color,
    /// A type name, and the constants that read like one — anything beginning
    /// with a capital, which is as far as a lexer can honestly go.
    pub kind: Color,
    /// Brackets, separators and operators.
    ///
    /// Held back rather than coloured: punctuation is the most common thing on
    /// a line of code and the least worth reading, so this exists to push it
    /// *down* against plain text, not to pick it out.
    pub punctuation: Color,
}

/// Colours for agent-session status, independent of the diff pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusTokens {
    /// A session that is live and making progress.
    pub running: Color,
    /// A session that has stopped and is waiting on a person.
    ///
    /// Distinct from both of the others on purpose: an agent blocked on a
    /// permission prompt is neither working nor broken, and drawing it as
    /// either is how a session sits untouched for an hour.
    pub attention: Color,
    /// A session that ended badly.
    pub failed: Color,
    /// A worktree git is part-way through rewriting.
    ///
    /// Not a session state at all, which is why it is its own hue rather than
    /// a shade of `running`: a merge, rebase or cherry-pick left half-applied
    /// is a fact about the *checkout*, and it outlives whichever agent
    /// started it. Green would say "an agent is making progress here", which
    /// is exactly the wrong thing to say about a tree stopped on a conflict.
    pub merging: Color,
}

/// Colours for a quota climbing towards its limit.
///
/// Three steps rather than a reuse of [`StatusTokens`]: a usage meter used to
/// borrow `attention` and `failed`, on the argument that a quota near its
/// limit wants a person the way a blocked session does. It does — but a
/// ramp is a different instrument from a state. Two steps left the whole
/// stretch from "fine" to "nearly out" one colour, and the first step landed
/// late enough that by the time a meter changed there was little left to
/// plan around. Three steps, starting earlier, is a gradient a person can
/// read the slope of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaTokens {
    /// Past seven tenths: worth a glance. A light yellow.
    pub warm: Color,
    /// Past eight tenths: worth planning around. An orange.
    pub hot: Color,
    /// Past nine tenths: about to stop. A red.
    pub critical: Color,
}

/// The sixteen colours a terminal emulator indexes by number, named rather
/// than kept as an array so a theme file reads as `red = "#..."` instead of
/// `ansi[1] = "#..."`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnsiColors {
    /// Index 0.
    pub black: Color,
    /// Index 1.
    pub red: Color,
    /// Index 2.
    pub green: Color,
    /// Index 3.
    pub yellow: Color,
    /// Index 4.
    pub blue: Color,
    /// Index 5.
    pub magenta: Color,
    /// Index 6.
    pub cyan: Color,
    /// Index 7.
    pub white: Color,
    /// Index 8.
    pub bright_black: Color,
    /// Index 9.
    pub bright_red: Color,
    /// Index 10.
    pub bright_green: Color,
    /// Index 11.
    pub bright_yellow: Color,
    /// Index 12.
    pub bright_blue: Color,
    /// Index 13.
    pub bright_magenta: Color,
    /// Index 14.
    pub bright_cyan: Color,
    /// Index 15.
    pub bright_white: Color,
}

impl AnsiColors {
    /// The colour for a standard terminal colour index.
    ///
    /// Indexed modulo 16: `alacritty_terminal` (and every other emulator)
    /// treats the base 16-colour palette as cyclic for anything outside
    /// 0-15 rather than erroring, and matching that means a renderer built
    /// on this never has to special-case an index it was not expecting.
    pub fn get(&self, index: u8) -> Color {
        match index % 16 {
            0 => self.black,
            1 => self.red,
            2 => self.green,
            3 => self.yellow,
            4 => self.blue,
            5 => self.magenta,
            6 => self.cyan,
            7 => self.white,
            8 => self.bright_black,
            9 => self.bright_red,
            10 => self.bright_green,
            11 => self.bright_yellow,
            12 => self.bright_blue,
            13 => self.bright_magenta,
            14 => self.bright_cyan,
            _ => self.bright_white,
        }
    }
}

/// Terminal-pane colours: the sixteen ANSI slots plus the three the grid
/// itself needs outside of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalTokens {
    /// Default text colour, used where an escape sequence names no colour.
    pub foreground: Color,
    /// The pane's background.
    pub background: Color,
    /// The text cursor.
    pub cursor: Color,
    /// The sixteen indexed colours.
    pub ansi: AnsiColors,
}

/// A complete theme: every token a renderer needs, with nothing optional.
///
/// Constructed by [`Theme::dark`], [`Theme::light`], or by loading a user's
/// TOML file and filling in whatever it left out from one of those two — see
/// [`Theme::load_from`]. There is no other way to build one, which is what
/// guarantees a `Theme` a renderer holds always has every token defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// Whether this is a dark palette or a light one.
    pub appearance: Appearance,
    /// The desk the window's cards sit on: the gutter between the sidebar,
    /// the panes and the panel, and the ground under the header and footer.
    ///
    /// Darker than `surface` in every shipped theme. The cards read as cards
    /// only because this is not their colour; set equal, the gutters vanish
    /// and the window goes back to one flat field with hairlines in it.
    pub backdrop: Color,
    /// The window's background, behind every panel.
    pub surface: Color,
    /// A raised panel's background — the sidebar, say.
    pub panel: Color,
    // The grounds are a deliberate ladder — `backdrop` under the cards,
    // `surface` and `panel` the cards themselves, `sunken` the wells set into
    // them, `elevated` what floats over them — and the gaps between them have
    // to be big enough to see. A well is a fill rather than a border now, so
    // `sunken` equal to the card it sits in is a field nobody can find.
    /// A raised slab: a menu, a dialog, a button at rest.
    ///
    /// Distinct from `panel`, which is the ground a whole *region* sits on. A
    /// menu drawn in `panel` over a `panel` sidebar has no edge at all, which
    /// is why the two cannot be the same token however close they look.
    pub elevated: Color,
    /// A recessed well: the inside of a text field.
    pub sunken: Color,
    /// The background of a row the pointer is over.
    ///
    /// Deliberately lighter than `selection`. Before this token existed the
    /// shell hovered with `selection`, which made a row you were pointing at
    /// and a row you had chosen identical — so nothing appeared to respond.
    pub hover: Color,
    /// The background of a selected row.
    pub selection: Color,
    /// A hairline between two surfaces.
    pub border: Color,
    /// A hairline *within* one surface: between rows of a list, under a
    /// column heading, beside a gutter.
    ///
    /// Fainter than `border`, and separate from it because the two are
    /// answering different questions. `border` says "these are two different
    /// panels"; this says "these are two entries in one list". A single
    /// hairline weight for both made a sidebar of thirty rows read as thirty
    /// panels.
    pub rule: Color,
    /// The rail on the selected row of a list, and nothing else: the one
    /// warm colour the chrome spends, so it only ever means "you are here".
    pub marker: Color,
    /// The fill of a primary action.
    pub accent: Color,
    /// `accent` with the pointer over it.
    pub accent_hover: Color,
    /// Text and icons drawn on `accent`.
    pub on_accent: Color,
    /// The ring drawn around the keyboard-focused element.
    ///
    /// Only that. It was also the primary button's fill, four panel borders,
    /// a selected pill's outline and a hover border before `accent` and
    /// `border` existed — which left the shell no way to show focus at all.
    pub focus_ring: Color,
    /// Text colours.
    pub text: TextTokens,
    /// Diff-pane colours.
    pub diff: DiffTokens,
    /// Agent-session status colours.
    pub status: StatusTokens,
    /// Quota-meter colours — the ramp a usage bar climbs. See [`QuotaTokens`].
    pub quota: QuotaTokens,
    /// Terminal-pane colours.
    pub terminal: TerminalTokens,
    /// Highlighted-source colours, in the editor pane.
    pub syntax: SyntaxTokens,
}

/// The themes ket ships, found when it is built — see `build.rs`.
mod sources {
    include!(concat!(env!("OUT_DIR"), "/builtin_themes.rs"));
}

/// A theme a picker can offer: its file's name, what it is called, and the
/// palette.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeEntry {
    /// The file's name without `.toml`: what the config stores.
    pub name: String,
    /// What a picker calls it — the file's `label`, else its name.
    pub label: String,
    /// The palette itself.
    pub theme: Theme,
    /// Whether ket ships it, rather than it being one of the user's own files.
    pub builtin: bool,
}

/// Every theme ket ships, parsed once: `dark` first, `light` last, the rest
/// by label. A shipped file must set every token — it is a palette, not an
/// override — so one that does not is left out, loudly, and the test over
/// [`Theme::builtins`] fails on it.
fn shipped_themes() -> &'static [ThemeEntry] {
    static THEMES: std::sync::OnceLock<Vec<ThemeEntry>> = std::sync::OnceLock::new();
    THEMES.get_or_init(|| {
        let mut themes: Vec<ThemeEntry> = sources::SOURCES
            .iter()
            .filter_map(|(name, text)| match parse_complete(text) {
                Ok((label, theme)) => Some(ThemeEntry {
                    name: (*name).to_owned(),
                    label: label.unwrap_or_else(|| (*name).to_owned()),
                    theme,
                    builtin: true,
                }),
                Err(e) => {
                    tracing::error!(%e, name, "a shipped theme does not parse");
                    None
                }
            })
            .collect();
        let rank = |name: &str| match name {
            "dark" => 0,
            "light" => 2,
            _ => 1,
        };
        themes.sort_by(|a, b| {
            rank(&a.name)
                .cmp(&rank(&b.name))
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
        themes
    })
}

/// A theme file's text as a TOML table, with its `label` taken out: the one
/// key that names the theme rather than colouring it.
fn split_label(text: &str) -> Result<(Option<String>, toml::Table)> {
    let mut table: toml::Table =
        toml::from_str(text).map_err(|e| KetError::Config(format!("theme: {e}")))?;
    let label = match table.remove("label") {
        Some(toml::Value::String(label)) => Some(label),
        Some(_) => return Err(KetError::Config("theme: label must be text".to_owned())),
        None => None,
    };
    Ok((label, table))
}

/// A shipped theme file: every token set, nothing unknown.
fn parse_complete(text: &str) -> Result<(Option<String>, Theme)> {
    let (label, table) = split_label(text)?;
    let theme = toml::Value::Table(table)
        .try_into::<Theme>()
        .map_err(|e| KetError::Config(format!("theme: {e}")))?;
    Ok((label, theme))
}

impl Theme {
    /// The shipped dark theme, Desk. Used whenever nothing overrides it.
    pub fn dark() -> Self {
        Theme::builtin("dark").expect("ket's dark theme file parses")
    }

    /// The shipped light theme: Desk, in daylight.
    pub fn light() -> Self {
        Theme::builtin("light").expect("ket's light theme file parses")
    }

    /// This theme as seen by what is drawn straight on [`Theme::backdrop`] —
    /// the status bar, which has no card of its own.
    ///
    /// Every shipped theme but one sets its ink against the cards, and the
    /// desk under them is darker still, so the ink reads there too and this
    /// returns the theme unchanged. A theme whose desk is *bright* — Acid's
    /// lime — would leave pale ink on a pale ground, so there the ink turns
    /// dark: the card colour for the words, and the card colour lifted a
    /// third of the way towards the desk for the dim line.
    pub fn on_backdrop(&self) -> Theme {
        if contrast_ratio(self.text.dim, self.backdrop) >= Prominence::Body.minimum_ratio() {
            return *self;
        }
        let mut theme = *self;
        theme.text.primary = self.surface;
        theme.text.dim = self.surface.mix(self.backdrop, 0.32);
        theme
    }

    /// The shipped theme for a given appearance.
    pub fn shipped(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Theme::dark(),
            Appearance::Light => Theme::light(),
        }
    }

    /// Every theme ket ships, in the order a picker lists them: `dark`
    /// first, `light` last, the rest by label. One file each under
    /// `ket-core/themes/`; adding a theme is adding a file.
    pub fn builtins() -> &'static [ThemeEntry] {
        shipped_themes()
    }

    /// One of ket's own themes, by its file's name — or `None` for a name
    /// that is not one of ket's own, which is what lets [`Theme::load`] fall
    /// through to a user's file of the same name.
    pub fn builtin(name: &str) -> Option<Self> {
        // Acqua replaced Azure; a config still naming the old theme gets its
        // successor rather than falling back to `dark`.
        let name = if name == "azure" { "acqua" } else { name };
        shipped_themes()
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.theme)
    }

    /// Every theme there is to pick: ket's own, then the user's files in
    /// `~/.config/ket/themes/`, by label. A user file named like one of
    /// ket's is not listed — [`Theme::load`] would give ket's — and one that
    /// does not parse is left out rather than listed broken.
    pub fn available() -> Vec<ThemeEntry> {
        let mut themes = shipped_themes().to_vec();
        let mut own: Vec<ThemeEntry> = paths::themes_dir()
            .ok()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()? != "toml" {
                    return None;
                }
                let name = path.file_stem()?.to_str()?.to_owned();
                if Theme::builtin(&name).is_some() {
                    return None;
                }
                let text = std::fs::read_to_string(&path).ok()?;
                match Theme::parse_file(&text) {
                    Ok((label, theme)) => Some(ThemeEntry {
                        label: label.unwrap_or_else(|| name.clone()),
                        name,
                        theme,
                        builtin: false,
                    }),
                    Err(e) => {
                        tracing::warn!(%e, path = %path.display(), "a theme file does not parse");
                        None
                    }
                }
            })
            .collect();
        own.sort_by_key(|entry| entry.label.to_lowercase());
        themes.extend(own);
        themes
    }

    /// Loads the named theme: one of [`Theme::builtins`], or otherwise a file
    /// at `~/.config/ket/themes/<name>.toml`.
    pub fn load(name: &str) -> Result<Self> {
        if let Some(theme) = Theme::builtin(name) {
            return Ok(theme);
        }
        Self::load_from(&paths::theme_file(name)?)
    }

    /// Loads a theme from a specific path.
    ///
    /// A missing file yields the shipped dark theme, matching how
    /// [`crate::config::Config::load_from`] treats a missing config: absence
    /// is not an error, since a theme only needs to exist to override
    /// something. A file that exists but does not parse — bad TOML, an
    /// unknown token, a colour that is not `#rrggbb` — is an error, and
    /// [`KetError::Config`] names what was wrong.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml_str(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Theme::dark()),
            Err(e) => Err(KetError::io(path, e)),
        }
    }

    /// Parses a theme from TOML text.
    ///
    /// Every token is optional in the source text: whatever the file does not
    /// set falls back to the shipped theme for its `appearance` (`dark` if
    /// even that is absent), rather than failing to load. A typo in one
    /// colour should cost that one token, not the whole file — and definitely
    /// not stop the app from opening. An unknown key is a different kind of
    /// mistake — a misspelled *token name*, not a missing value — and is
    /// rejected with that name in the error, via `deny_unknown_fields` on
    /// every level of [`PartialTheme`].
    ///
    /// A top-level `label` names the theme for a picker; it colours nothing.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        Theme::parse_file(text).map(|(_, theme)| theme)
    }

    /// A user's theme file: its `label`, if it gives one, and its palette with
    /// whatever it left out filled from the shipped theme.
    fn parse_file(text: &str) -> Result<(Option<String>, Self)> {
        let (label, table) = split_label(text)?;
        let partial: PartialTheme = toml::Value::Table(table)
            .try_into()
            .map_err(|e| KetError::Config(format!("theme: {e}")))?;
        let base = Theme::shipped(partial.appearance.unwrap_or_default());
        Ok((label, partial.resolve(base)))
    }
}

/// The same shape as [`Theme`], but every token is optional.
///
/// This is what a user's file is actually deserialised into: `#[serde(default)]`
/// handles a whole section being absent (no `[terminal]` table at all), and
/// the `Option<Color>` fields handle one token inside a present section being
/// absent. Both cases mean the same thing — fall back to the base theme — and
/// merging happens once, in [`PartialTheme::resolve`], rather than the
/// fallback being reimplemented per field at the call site.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialTheme {
    appearance: Option<Appearance>,
    backdrop: Option<Color>,
    surface: Option<Color>,
    panel: Option<Color>,
    elevated: Option<Color>,
    sunken: Option<Color>,
    hover: Option<Color>,
    selection: Option<Color>,
    border: Option<Color>,
    rule: Option<Color>,
    marker: Option<Color>,
    accent: Option<Color>,
    accent_hover: Option<Color>,
    on_accent: Option<Color>,
    focus_ring: Option<Color>,
    text: PartialTextTokens,
    diff: PartialDiffTokens,
    status: PartialStatusTokens,
    quota: PartialQuotaTokens,
    terminal: PartialTerminalTokens,
    syntax: PartialSyntaxTokens,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialTextTokens {
    primary: Option<Color>,
    dim: Option<Color>,
    note: Option<Color>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialDiffTokens {
    added: Option<Color>,
    removed: Option<Color>,
    modified: Option<Color>,
    context: Option<Color>,
    hunk_header: Option<Color>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialSyntaxTokens {
    keyword: Option<Color>,
    string: Option<Color>,
    comment: Option<Color>,
    number: Option<Color>,
    kind: Option<Color>,
    punctuation: Option<Color>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialStatusTokens {
    running: Option<Color>,
    attention: Option<Color>,
    failed: Option<Color>,
    merging: Option<Color>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialQuotaTokens {
    warm: Option<Color>,
    hot: Option<Color>,
    critical: Option<Color>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialAnsiColors {
    black: Option<Color>,
    red: Option<Color>,
    green: Option<Color>,
    yellow: Option<Color>,
    blue: Option<Color>,
    magenta: Option<Color>,
    cyan: Option<Color>,
    white: Option<Color>,
    bright_black: Option<Color>,
    bright_red: Option<Color>,
    bright_green: Option<Color>,
    bright_yellow: Option<Color>,
    bright_blue: Option<Color>,
    bright_magenta: Option<Color>,
    bright_cyan: Option<Color>,
    bright_white: Option<Color>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialTerminalTokens {
    foreground: Option<Color>,
    background: Option<Color>,
    cursor: Option<Color>,
    ansi: PartialAnsiColors,
}

impl PartialTheme {
    /// Fills every token this file left unset from `base`.
    fn resolve(self, base: Theme) -> Theme {
        Theme {
            appearance: base.appearance,
            backdrop: self.backdrop.unwrap_or(base.backdrop),
            surface: self.surface.unwrap_or(base.surface),
            panel: self.panel.unwrap_or(base.panel),
            elevated: self.elevated.unwrap_or(base.elevated),
            sunken: self.sunken.unwrap_or(base.sunken),
            hover: self.hover.unwrap_or(base.hover),
            selection: self.selection.unwrap_or(base.selection),
            border: self.border.unwrap_or(base.border),
            rule: self.rule.unwrap_or(base.rule),
            marker: self.marker.unwrap_or(base.marker),
            accent: self.accent.unwrap_or(base.accent),
            accent_hover: self.accent_hover.unwrap_or(base.accent_hover),
            on_accent: self.on_accent.unwrap_or(base.on_accent),
            focus_ring: self.focus_ring.unwrap_or(base.focus_ring),
            text: TextTokens {
                primary: self.text.primary.unwrap_or(base.text.primary),
                dim: self.text.dim.unwrap_or(base.text.dim),
                note: self.text.note.or(base.text.note),
            },
            diff: DiffTokens {
                added: self.diff.added.unwrap_or(base.diff.added),
                removed: self.diff.removed.unwrap_or(base.diff.removed),
                modified: self.diff.modified.unwrap_or(base.diff.modified),
                context: self.diff.context.unwrap_or(base.diff.context),
                hunk_header: self.diff.hunk_header.unwrap_or(base.diff.hunk_header),
            },
            status: StatusTokens {
                running: self.status.running.unwrap_or(base.status.running),
                attention: self.status.attention.unwrap_or(base.status.attention),
                failed: self.status.failed.unwrap_or(base.status.failed),
                merging: self.status.merging.unwrap_or(base.status.merging),
            },
            quota: QuotaTokens {
                warm: self.quota.warm.unwrap_or(base.quota.warm),
                hot: self.quota.hot.unwrap_or(base.quota.hot),
                critical: self.quota.critical.unwrap_or(base.quota.critical),
            },
            terminal: TerminalTokens {
                foreground: self.terminal.foreground.unwrap_or(base.terminal.foreground),
                background: self.terminal.background.unwrap_or(base.terminal.background),
                cursor: self.terminal.cursor.unwrap_or(base.terminal.cursor),
                ansi: AnsiColors {
                    black: self.terminal.ansi.black.unwrap_or(base.terminal.ansi.black),
                    red: self.terminal.ansi.red.unwrap_or(base.terminal.ansi.red),
                    green: self.terminal.ansi.green.unwrap_or(base.terminal.ansi.green),
                    yellow: self
                        .terminal
                        .ansi
                        .yellow
                        .unwrap_or(base.terminal.ansi.yellow),
                    blue: self.terminal.ansi.blue.unwrap_or(base.terminal.ansi.blue),
                    magenta: self
                        .terminal
                        .ansi
                        .magenta
                        .unwrap_or(base.terminal.ansi.magenta),
                    cyan: self.terminal.ansi.cyan.unwrap_or(base.terminal.ansi.cyan),
                    white: self.terminal.ansi.white.unwrap_or(base.terminal.ansi.white),
                    bright_black: self
                        .terminal
                        .ansi
                        .bright_black
                        .unwrap_or(base.terminal.ansi.bright_black),
                    bright_red: self
                        .terminal
                        .ansi
                        .bright_red
                        .unwrap_or(base.terminal.ansi.bright_red),
                    bright_green: self
                        .terminal
                        .ansi
                        .bright_green
                        .unwrap_or(base.terminal.ansi.bright_green),
                    bright_yellow: self
                        .terminal
                        .ansi
                        .bright_yellow
                        .unwrap_or(base.terminal.ansi.bright_yellow),
                    bright_blue: self
                        .terminal
                        .ansi
                        .bright_blue
                        .unwrap_or(base.terminal.ansi.bright_blue),
                    bright_magenta: self
                        .terminal
                        .ansi
                        .bright_magenta
                        .unwrap_or(base.terminal.ansi.bright_magenta),
                    bright_cyan: self
                        .terminal
                        .ansi
                        .bright_cyan
                        .unwrap_or(base.terminal.ansi.bright_cyan),
                    bright_white: self
                        .terminal
                        .ansi
                        .bright_white
                        .unwrap_or(base.terminal.ansi.bright_white),
                },
            },
            syntax: SyntaxTokens {
                keyword: self.syntax.keyword.unwrap_or(base.syntax.keyword),
                string: self.syntax.string.unwrap_or(base.syntax.string),
                comment: self.syntax.comment.unwrap_or(base.syntax.comment),
                number: self.syntax.number.unwrap_or(base.syntax.number),
                kind: self.syntax.kind.unwrap_or(base.syntax.kind),
                punctuation: self.syntax.punctuation.unwrap_or(base.syntax.punctuation),
            },
        }
    }
}
