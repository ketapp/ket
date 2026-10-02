//! Mapping keys to command ids, as data.
//!
//! [`crate::command::Registry`] is the single source of truth for what ket can
//! do; this module only ever *points at* a command by id. It never carries a
//! command's logic and never invents one — a binding that names an id the
//! registry does not have is a mistake in the keybinding file, not a silent
//! no-op, which is why [`Keymap::validate`] exists and why loading a keymap
//! always calls it.
//!
//! Four ideas, each earning its place:
//!
//! - **Bindings are data in `~/.config/ket/keybindings.toml`, never compiled
//!   in.** A person who wants `ctrl+j` instead of `ctrl+n` edits a file; they
//!   do not fork ket.
//! - **A [`Chord`] is a sequence of one or more [`Keystroke`]s.** `ctrl+k` and
//!   `ctrl+k ctrl+p` are the same *kind* of thing — the latter is simply two
//!   keystrokes instead of one — which is exactly what makes the second
//!   keystroke able to swallow the first: see the conflict note below.
//! - **A [`Binding`] is scoped to a context** — `"global"`, `"terminal"`,
//!   `"diff"`, whatever a client's panes are named. Contexts are opaque
//!   strings here, the same choice [`crate::command::Command::panes`] makes,
//!   for the same reason: a pane taxonomy is UI knowledge this crate does not
//!   own (invariant 3). `"global"` is not special syntax, only a convention
//!   both this module's defaults and [`Keymap::resolve`]'s fallback rely on.
//! - **The user's file overlays the shipped defaults; it does not replace
//!   them.** [`Keymap::from_toml_str`] merges a parsed file onto
//!   [`Keymap::builtin`] one `(context, chord)` key at a time, so binding one
//!   key does not cost every other binding — the mistake a config format that
//!   just deserializes into the final struct would make silently.
//!
//! One subtlety drives most of [`Keymap::validate`]: **a chord that is a
//! strict prefix of another chord makes the shorter one unreachable.** If
//! `ctrl+k` fires a command immediately, `ctrl+k ctrl+p` can never be
//! reached — the first keystroke already committed. If instead `ctrl+k`
//! waits to see whether `ctrl+p` follows, then `ctrl+k` alone can never fire.
//! Either way one binding loses, silently, the moment both exist in bindings
//! reachable from the same context — and that is true whether both bindings
//! sit in the same named context or one of them is inherited from
//! `"global"`. Rather than pick a resolution rule and let the loser vanish
//! without a word, this is rejected at load time as [`KetError::Conflict`],
//! naming both commands so the person who wrote the file can see the actual
//! collision.
//!
//! The TOML shape is deliberately an array of tables —
//! `[[binding]]`, not a `chord = command` map — even though the map reads
//! shorter. A map cannot represent two bindings for the same key in the same
//! context at all: TOML itself would reject the duplicate key, or a
//! `HashMap`-shaped deserialize would silently keep the last one. Both are
//! the opposite of what a conflict-reporting loader needs, which is for that
//! exact mistake to reach [`Keymap::from_toml_str`] as data so it can be
//! turned into a named error instead of a parse failure with no command
//! names in it or a silent last-write-wins.

use std::fmt;

use serde::Deserialize;

use crate::command::{CommandId, Registry};
use crate::{KetError, Result};

/// The context every client falls back to when a more specific one has no
/// binding for a chord.
///
/// Not enforced syntax — a context is an opaque string, like a pane name —
/// only a convention [`Keymap::builtin`] and [`Keymap::effective_bindings`]
/// both rely on.
pub const GLOBAL_CONTEXT: &str = "global";

/// Named, non-printing keys a [`Keystroke`] can carry besides a single
/// character.
///
/// `f1` through `f24` are handled separately, by parsing the digits after a
/// leading `f`, rather than being listed here — twenty-four more strings
/// would swamp the list without covering anything a formula does not already
/// cover exactly.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "escape",
    "esc",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
];

/// Whether `key` (already lowercased) is a name [`Keystroke::parse`] accepts.
///
/// A single non-whitespace character is always accepted — that is most of
/// the keyboard — plus the named keys above. Whitespace is excluded from the
/// single-character case specifically because [`Chord::parse`] splits a
/// chord's text on whitespace: a literal space key has to be spelled
/// `space`, or it would be indistinguishable from the boundary between two
/// keystrokes.
fn is_known_key(key: &str) -> bool {
    if let Some(rest) = key.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
    {
        return (1..=24).contains(&n);
    }

    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => !c.is_whitespace(),
        _ => NAMED_KEYS.contains(&key),
    }
}

/// One physical key combination: modifiers plus a single named key.
///
/// The unit a [`Chord`] is built from. A `Keystroke` is what fires when
/// *this exact* combination is pressed; a [`Chord`] is what fires after a
/// sequence of them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Keystroke {
    /// Control held.
    pub ctrl: bool,
    /// Alt (or macOS Option) held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
    /// Command (or Super, or Meta) held.
    pub cmd: bool,
    /// The key itself, lowercased: `"k"`, `"up"`, `"f5"`.
    pub key: String,
}

impl Keystroke {
    /// Parses one keystroke, e.g. `ctrl+shift+n` or `f5`.
    ///
    /// Modifiers may appear in any order and either case — `ctrl+shift+n`
    /// and `Shift+Ctrl+N` parse to the same [`Keystroke`] — because the
    /// person typing a keybindings file should not have to remember a
    /// canonical order for something the keyboard does not have one for.
    ///
    /// An unrecognised modifier or key name is rejected with the offending
    /// text quoted in the error, rather than the keystroke being dropped:
    /// a binding that silently vanished because of a typo is far harder to
    /// notice than one that refuses to load.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = |detail: &str| KetError::Config(format!("not a keystroke: {text:?} ({detail})"));

        if text.trim().is_empty() {
            return Err(bad("empty"));
        }

        let parts: Vec<&str> = text.split('+').collect();
        let (key_part, modifier_parts) = parts.split_last().expect("split always yields >=1");

        let mut keystroke = Keystroke {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            key: String::new(),
        };

        for part in modifier_parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => keystroke.ctrl = true,
                "alt" | "option" => keystroke.alt = true,
                "shift" => keystroke.shift = true,
                "cmd" | "meta" | "super" | "command" => keystroke.cmd = true,
                other => return Err(bad(&format!("unknown modifier: {other:?}"))),
            }
        }

        let key = key_part.to_ascii_lowercase();
        if key.is_empty() || !is_known_key(&key) {
            return Err(bad(&format!("unknown key: {key:?}")));
        }
        keystroke.key = key;

        Ok(keystroke)
    }
}

impl fmt::Display for Keystroke {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Fixed order regardless of how the source text wrote them, so two
        // keystrokes that parsed equal also print equal.
        if self.ctrl {
            write!(f, "ctrl+")?;
        }
        if self.alt {
            write!(f, "alt+")?;
        }
        if self.shift {
            write!(f, "shift+")?;
        }
        if self.cmd {
            write!(f, "cmd+")?;
        }
        f.write_str(&self.key)
    }
}

/// A sequence of one or more [`Keystroke`]s bound as a unit.
///
/// Most chords are a single keystroke; a genuine chord like `ctrl+k ctrl+p`
/// is simply one with more than one. Keeping them the same type is what lets
/// [`Chord::is_prefix_of`] compare a single-keystroke binding against a
/// multi-keystroke one — the comparison the prefix-conflict rule in the
/// module docs depends on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord(Vec<Keystroke>);

impl Chord {
    /// Parses a whitespace-separated sequence of keystrokes, e.g.
    /// `"ctrl+k ctrl+p"`.
    pub fn parse(text: &str) -> Result<Self> {
        let keystrokes = text
            .split_whitespace()
            .map(Keystroke::parse)
            .collect::<Result<Vec<_>>>()?;

        if keystrokes.is_empty() {
            return Err(KetError::Config(format!("empty chord: {text:?}")));
        }

        Ok(Chord(keystrokes))
    }

    /// A chord consisting of a single keystroke.
    ///
    /// What a client reaches for when it is building a chord up one
    /// keypress at a time: hold the first keystroke as a one-long `Chord`,
    /// [`resolve`](Keymap::resolve) it, and [`push`](Chord::push) another
    /// keystroke on if the answer is [`Resolution::Pending`].
    pub fn single(keystroke: Keystroke) -> Self {
        Chord(vec![keystroke])
    }

    /// Appends another keystroke, extending the sequence.
    pub fn push(&mut self, keystroke: Keystroke) {
        self.0.push(keystroke);
    }

    /// The keystrokes, in order.
    pub fn keystrokes(&self) -> &[Keystroke] {
        &self.0
    }

    /// How many keystrokes this chord holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false: a [`Chord`] cannot be built with zero keystrokes,
    /// neither by [`Chord::parse`] nor by [`Chord::single`]. Provided
    /// alongside [`Chord::len`] because clippy requires it of anything with
    /// a `len`, and it does describe a real (if unreachable) property.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `self` is a strict prefix of `other` — the same keystrokes,
    /// in order, with `other` holding at least one more.
    ///
    /// This is the relation [`Keymap::validate`] rejects between two
    /// reachable bindings, and the one [`Keymap::resolve`] uses to decide
    /// that a chord in progress should wait rather than fail outright: see
    /// the module docs for why a prefix relation is a conflict rather than
    /// an ordinary precedence rule.
    pub fn is_prefix_of(&self, other: &Chord) -> bool {
        self.0.len() < other.0.len() && self.0 == other.0[..self.0.len()]
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, keystroke) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{keystroke}")?;
        }
        Ok(())
    }
}

/// One chord bound to a command within a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The context this binding is active in — `"global"`, or a pane name.
    pub context: String,
    /// The chord that fires [`Binding::command`].
    pub chord: Chord,
    /// The command id this chord invokes. Checked against a
    /// [`Registry`] by [`Keymap::validate`], never assumed valid.
    pub command: CommandId,
}

/// What pressing a chord does, given how much of it has been typed so far.
///
/// The three cases a client's key-handling loop needs to distinguish: fire
/// something, wait for more keys and show that it is waiting, or give up and
/// let the keystroke fall through to whatever it would ordinarily do (a
/// terminal pane inserting the literal character, say). Collapsing `Pending`
/// into `NoMatch` would make a client unable to tell "nothing is bound here"
/// from "something is bound, but you are not done typing it" — exactly the
/// distinction that decides whether the next keystroke gets swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// This exact chord is bound in this context (or inherited from
    /// [`GLOBAL_CONTEXT`]): run this command and reset.
    Fired(CommandId),
    /// This chord is a strict prefix of at least one reachable binding: keep
    /// listening, and swallow the keystroke that produced this chord so it
    /// does not also do whatever it would do unbound.
    Pending,
    /// Nothing reachable from this context starts this way: give up.
    NoMatch,
}

/// The full set of key-to-command bindings a client resolves keys against.
///
/// Built by layering a parsed keybindings file over [`Keymap::builtin`] —
/// see [`Keymap::from_toml_str`] — never by deserializing a file directly
/// into this shape, which is what guarantees the override-not-replace
/// property described in the module docs.
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    /// The bindings ket ships, active until a user's file overrides one.
    ///
    /// Deliberately modest: it exists to prove the mechanism (a chord, a
    /// context-specific override, several plain single-key bindings) against
    /// commands that really are in [`Registry::builtin`], not to be a
    /// finished keymap for a shell that does not exist yet.
    pub fn builtin() -> Self {
        // (context, chord text, command id). `ctrl+w` appearing in both
        // "global" and "terminal" is the context-precedence example this
        // module exists for: the same chord means something else with a
        // terminal focused, and that is not a conflict — see
        // `effective_bindings` for why an exact match at a more specific
        // context shadows the global one instead of colliding with it.
        const DEFAULTS: &[(&str, &str, &str)] = &[
            (GLOBAL_CONTEXT, "ctrl+k ctrl+p", "app.commands"),
            (GLOBAL_CONTEXT, "ctrl+n", "worktree.create"),
            (GLOBAL_CONTEXT, "ctrl+l", "worktree.list"),
            (GLOBAL_CONTEXT, "ctrl+w", "worktree.close"),
            (GLOBAL_CONTEXT, "ctrl+d", "worktree.diff"),
            (GLOBAL_CONTEXT, "ctrl+shift+a", "agent.run"),
            (GLOBAL_CONTEXT, "cmd+,", "config.show"),
            (GLOBAL_CONTEXT, "ctrl+shift+backspace", "worktree.remove"),
            ("terminal", "ctrl+w", "worktree.status"),
            ("diff", "ctrl+shift+c", "worktree.collapse"),
        ];

        let bindings = DEFAULTS
            .iter()
            .map(|(context, chord, command)| Binding {
                context: (*context).to_owned(),
                chord: Chord::parse(chord).expect("shipped default chord parses"),
                command: CommandId::new(*command),
            })
            .collect();

        Keymap { bindings }
    }

    /// Loads `~/.config/ket/keybindings.toml`, overlaid on [`Keymap::builtin`]
    /// and validated against `registry`.
    pub fn load(registry: &Registry) -> Result<Self> {
        Self::load_from(&crate::paths::keybindings_file()?, registry)
    }

    /// Loads a keybindings file from a specific path.
    ///
    /// A missing file yields the shipped defaults, unmodified — matching how
    /// [`crate::config::Config::load_from`] and [`crate::theme::Theme::load_from`]
    /// both treat absence as "nothing to override" rather than an error.
    pub fn load_from(path: &std::path::Path, registry: &Registry) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml_str(&text, registry),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let keymap = Keymap::builtin();
                keymap.validate(registry)?;
                Ok(keymap)
            }
            Err(e) => Err(KetError::io(path, e)),
        }
    }

    /// Parses a keybindings file and overlays it on [`Keymap::builtin`].
    ///
    /// Two checks happen before the overlay, and one after, each catching a
    /// different way a file could go wrong:
    ///
    /// 1. Every chord and command id must parse — an unparseable chord fails
    ///    the whole load rather than being skipped (see the module docs).
    /// 2. The file's *own* bindings must not collide with each other on
    ///    `(context, chord)` — checked before merging, because merging by
    ///    that same key is what implements "override", and if two of the
    ///    user's own entries shared a key the merge would just keep
    ///    whichever came last. That is the exact last-write-wins outcome
    ///    this module exists to avoid, so it is caught first instead.
    /// 3. After the file is layered over the defaults, [`Keymap::validate`]
    ///    checks the *merged* result for unknown command ids and for the
    ///    prefix relation described in the module docs — both of which can
    ///    just as easily arise between a user binding and a default one as
    ///    within the user's file alone.
    pub fn from_toml_str(text: &str, registry: &Registry) -> Result<Self> {
        let raw: RawKeymap =
            toml::from_str(text).map_err(|e| KetError::Config(format!("keybindings: {e}")))?;

        let mut overrides = Vec::with_capacity(raw.binding.len());
        for entry in raw.binding {
            overrides.push(Binding {
                context: entry.context,
                chord: Chord::parse(&entry.chord)?,
                command: CommandId::new(entry.command),
            });
        }

        reject_duplicate_keys(&overrides)?;

        let mut bindings = Keymap::builtin().bindings;
        for binding in overrides {
            match bindings.iter_mut().find(|existing| {
                existing.context == binding.context && existing.chord == binding.chord
            }) {
                Some(slot) => *slot = binding,
                None => bindings.push(binding),
            }
        }

        let keymap = Keymap { bindings };
        keymap.validate(registry)?;
        Ok(keymap)
    }

    /// Every binding, defaults and overrides together, in no particular
    /// order.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Checks every binding's command id against `registry`, then checks the
    /// whole set for the two ways bindings collide.
    ///
    /// Run automatically by every loading path; exposed on its own so a
    /// client that mutates a [`Keymap`] after loading it — the shell binding
    /// a new command at runtime, say — can re-check before trusting the
    /// result.
    pub fn validate(&self, registry: &Registry) -> Result<()> {
        for binding in &self.bindings {
            if registry.get(binding.command.as_str()).is_none() {
                return Err(KetError::Conflict(format!(
                    "keybinding {} in {} names an unknown command: {}",
                    binding.chord, binding.context, binding.command
                )));
            }
        }

        reject_duplicate_keys(&self.bindings)?;

        let mut contexts: Vec<&str> = self
            .bindings
            .iter()
            .map(|binding| binding.context.as_str())
            .collect();
        contexts.push(GLOBAL_CONTEXT);
        contexts.sort_unstable();
        contexts.dedup();

        for context in contexts {
            let effective = self.effective_bindings(context);
            for (index, a) in effective.iter().enumerate() {
                for b in effective.iter().skip(index + 1) {
                    if a.chord.is_prefix_of(&b.chord) || b.chord.is_prefix_of(&a.chord) {
                        return Err(KetError::Conflict(format!(
                            "in {context}, {} conflicts with {}: one is a prefix of the \
                             other, which would make the shorter one unreachable ({} vs {})",
                            a.chord, b.chord, a.command, b.command
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    /// Which command fires for `chord` in `context`, if any.
    ///
    /// A client accumulates keystrokes into a [`Chord`] one at a time and
    /// calls this after each one. [`Resolution::Pending`] means keep
    /// accumulating and swallow the keystroke; [`Resolution::NoMatch`] means
    /// give up and let it fall through; [`Resolution::Fired`] means run the
    /// command and start the next chord from nothing.
    pub fn resolve(&self, context: &str, chord: &Chord) -> Resolution {
        let effective = self.effective_bindings(context);

        if let Some(binding) = effective.iter().find(|binding| &binding.chord == chord) {
            return Resolution::Fired(binding.command.clone());
        }

        if effective
            .iter()
            .any(|binding| chord.is_prefix_of(&binding.chord))
        {
            return Resolution::Pending;
        }

        Resolution::NoMatch
    }

    /// Bindings that extend `prefix` by exactly one more keystroke.
    ///
    /// What a client renders as a hint while [`Keymap::resolve`] has
    /// answered [`Resolution::Pending`] — "ctrl+k, then p for the palette"
    /// and whatever else could follow.
    pub fn continuations(&self, context: &str, prefix: &Chord) -> Vec<&Binding> {
        self.effective_bindings(context)
            .into_iter()
            .filter(|binding| prefix.is_prefix_of(&binding.chord))
            .collect()
    }

    /// The bindings a chord is checked against while focus is in `context`:
    /// this context's own bindings, plus [`GLOBAL_CONTEXT`]'s, minus any
    /// global chord this context already binds itself.
    ///
    /// That subtraction is what makes rebinding `ctrl+w` in `"terminal"`
    /// alone — without touching the global `ctrl+w` — an ordinary,
    /// conflict-free override rather than two bindings racing for the same
    /// chord: the more specific context wins outright, the same way CSS
    /// specificity or a `PATH` search does, rather than the two being
    /// merged. [`Keymap::validate`]'s prefix check runs over exactly this
    /// set, which is why it catches a prefix collision between a
    /// context-specific chord and a global one, not only two chords in the
    /// same named context.
    fn effective_bindings(&self, context: &str) -> Vec<&Binding> {
        let own: Vec<&Binding> = self
            .bindings
            .iter()
            .filter(|binding| binding.context == context)
            .collect();

        if context == GLOBAL_CONTEXT {
            return own;
        }

        let mut effective = own.clone();
        for binding in self
            .bindings
            .iter()
            .filter(|binding| binding.context == GLOBAL_CONTEXT)
        {
            if !own.iter().any(|local| local.chord == binding.chord) {
                effective.push(binding);
            }
        }
        effective
    }
}

/// Fails with both command ids named if any two bindings in `bindings` share
/// a `(context, chord)` key.
///
/// Shared by [`Keymap::from_toml_str`] (checking a file's own entries before
/// they are merged) and [`Keymap::validate`] (checking the merged result as
/// a backstop) — see the doc comments on each call site for why both checks
/// exist rather than one.
fn reject_duplicate_keys(bindings: &[Binding]) -> Result<()> {
    for (index, a) in bindings.iter().enumerate() {
        for b in bindings.iter().skip(index + 1) {
            if a.context == b.context && a.chord == b.chord {
                return Err(KetError::Conflict(format!(
                    "{} in {} is bound to both {} and {}",
                    a.chord, a.context, a.command, b.command
                )));
            }
        }
    }
    Ok(())
}

/// One `[[binding]]` table as it appears in `keybindings.toml`.
///
/// An array of tables, not a `chord = command` map: see the module docs for
/// why the map shape cannot represent (and therefore cannot report) the
/// exact mistake this module most needs to catch.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    /// Defaults to [`GLOBAL_CONTEXT`], since most bindings are not
    /// pane-specific and should not have to say so.
    #[serde(default = "default_context")]
    context: String,
    chord: String,
    command: String,
}

fn default_context() -> String {
    GLOBAL_CONTEXT.to_owned()
}

/// The top level of `keybindings.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawKeymap {
    binding: Vec<RawBinding>,
}
