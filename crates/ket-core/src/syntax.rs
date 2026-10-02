//! Colouring source code, one line at a time.
//!
//! A lexer, not a parser. It knows comments, strings, numbers, keywords and
//! punctuation, and that is the whole of it — there is no grammar here, no
//! scope resolution, and no way to tell a function call from a variable that
//! happens to be followed by a bracket. That is a deliberate stopping point
//! rather than a first step: those distinctions need a parser per language, and
//! `tree-sitter` is the plan for when the editor pane becomes somewhere people
//! live rather than somewhere they tweak a line.
//!
//! What that buys is no dependency and no build time. The alternative for this
//! much colour is a crate that carries a regex engine and a couple of megabytes
//! of syntax definitions, which is a large thing to own for an approximation —
//! and it would still be an approximation.
//!
//! # Lines, and what carries between them
//!
//! The editor is virtualised: it builds only the rows on screen, so row 400 is
//! highlighted without row 399 ever being looked at. But a block comment or a
//! triple-quoted string is open at the end of one line and still open at the
//! start of the next, and a highlighter that forgets that paints the rest of
//! the file as code.
//!
//! So the unit here is *one line plus what was open when it started*:
//! [`highlight_line`] takes a [`State`] and returns the state the next line
//! begins in. A caller that wants to highlight an arbitrary line runs
//! [`opening_states`] once over the document to learn where every line starts,
//! which is one pass of the same lexer and cheap enough to redo on an edit.
//!
//! # Adding a language
//!
//! A row in [`LANGUAGES`]. Nothing else — the rules are data, in the same shape
//! `ket_core::surface::PROFILES` uses for editors, because a branch per
//! language is how a highlighter becomes something nobody wants to touch.

use std::ops::Range;

/// What a run of characters is, as far as a lexer can tell.
///
/// Six kinds, matching `ket_core::theme::SyntaxTokens` one for one — see its
/// documentation for why the palette stops here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Token {
    /// A language keyword.
    Keyword,
    /// A string or character literal, quotes included.
    Str,
    /// A comment of any shape.
    Comment,
    /// A numeric literal.
    Number,
    /// A name that reads like a type: it begins with a capital.
    Kind,
    /// Brackets, separators, operators.
    Punctuation,
}

/// What was still open at the end of the previous line.
///
/// `Copy` and two words wide on purpose: the editor keeps one of these per line
/// of the document, so a file of a hundred thousand lines costs a megabyte at
/// most and nothing that has to be freed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// Ordinary code.
    #[default]
    Code,
    /// Inside a block comment. The `usize` is the nesting depth, because Rust's
    /// block comments nest and C's do not — a language that does not nest never
    /// counts past one.
    BlockComment(usize),
    /// Inside a string that survives a newline — a Python triple quote, a
    /// TOML multi-line literal. The `u8` is which quote character opened it,
    /// so `"""` does not close on `'''`.
    MultilineString(u8),
}

/// One language's rules, as data.
#[derive(Debug, Clone, Copy)]
pub struct Language {
    /// What it is called, for a caller that wants to say.
    pub name: &'static str,
    /// Extensions that select it, lowercase and without the dot.
    pub extensions: &'static [&'static str],
    /// Exact filenames that select it, for the files that have no extension.
    pub filenames: &'static [&'static str],
    /// What starts a comment that runs to the end of the line.
    pub line_comment: &'static [&'static str],
    /// The opening and closing of a comment that spans lines.
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Whether its block comments nest, as Rust's do.
    pub nested_block_comment: bool,
    /// Quote characters that open a string.
    pub quotes: &'static [u8],
    /// Whether a string may be opened with three of its quote character and
    /// run over newlines until three more.
    pub triple_quotes: bool,
    /// Whether a backslash inside a string escapes the next character.
    pub escapes: bool,
    /// Words drawn as keywords. Sorted is not required; these are short lists
    /// and the lookup is a linear scan over a handful of candidates.
    pub keywords: &'static [&'static str],
    /// Keywords this language shares with a family, checked alongside its own.
    ///
    /// Two slices rather than one long list per language: half of what makes a
    /// keyword in TypeScript also makes one in Java and C#, and a row that
    /// repeats [`C_LIKE`] is a row somebody will update in one place and not
    /// the others.
    pub shared: &'static [&'static str],
}

/// The keywords shared by the whole C-descended family, so the rows below are
/// differences rather than copies.
const C_LIKE: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "extends",
    "false",
    "finally",
    "for",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
];

/// Every language ket colours.
///
/// Ordered by how likely somebody is to open one in a pane whose whole purpose
/// is a quick change, which is also roughly the order the first row matched
/// wins — see [`language_for`].
pub static LANGUAGES: &[Language] = &[
    Language {
        name: "Rust",
        extensions: &["rs"],
        filenames: &[],
        line_comment: &["//"],
        block_comment: Some(("/*", "*/")),
        // The one language here whose block comments nest, and the reason
        // `State::BlockComment` counts rather than holding a flag.
        nested_block_comment: true,
        quotes: b"\"",
        triple_quotes: false,
        escapes: true,
        keywords: &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        shared: &[],
    },
    Language {
        name: "TypeScript",
        extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        filenames: &[],
        line_comment: &["//"],
        block_comment: Some(("/*", "*/")),
        nested_block_comment: false,
        quotes: b"\"'`",
        triple_quotes: false,
        escapes: true,
        keywords: &[
            "abstract",
            "any",
            "as",
            "async",
            "await",
            "declare",
            "delete",
            "export",
            "extends",
            "from",
            "function",
            "get",
            "keyof",
            "namespace",
            "never",
            "private",
            "protected",
            "public",
            "readonly",
            "set",
            "type",
            "undefined",
            "unknown",
            "yield",
        ],
        shared: C_LIKE,
    },
    Language {
        name: "Python",
        extensions: &["py", "pyi"],
        filenames: &[],
        line_comment: &["#"],
        block_comment: None,
        nested_block_comment: false,
        quotes: b"\"'",
        // The reason `State::MultilineString` carries a quote character: a
        // docstring opened with `"""` is not closed by `'''`.
        triple_quotes: true,
        escapes: true,
        keywords: &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "True", "try", "while", "with", "yield",
        ],
        shared: &[],
    },
    Language {
        name: "Go",
        extensions: &["go"],
        filenames: &[],
        line_comment: &["//"],
        block_comment: Some(("/*", "*/")),
        nested_block_comment: false,
        quotes: b"\"`",
        triple_quotes: false,
        escapes: true,
        keywords: &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
            "nil",
            "true",
            "false",
        ],
        shared: &[],
    },
    Language {
        name: "Shell",
        extensions: &["sh", "bash", "zsh", "fish"],
        filenames: &[".bashrc", ".zshrc", ".profile", ".bash_profile", ".zshenv"],
        line_comment: &["#"],
        block_comment: None,
        nested_block_comment: false,
        quotes: b"\"'",
        triple_quotes: false,
        escapes: true,
        keywords: &[
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if",
            "in", "local", "read", "return", "then", "until", "while",
        ],
        shared: &[],
    },
    Language {
        name: "TOML",
        extensions: &["toml"],
        filenames: &["Cargo.lock"],
        line_comment: &["#"],
        block_comment: None,
        nested_block_comment: false,
        quotes: b"\"'",
        triple_quotes: true,
        escapes: true,
        keywords: &["true", "false"],
        shared: &[],
    },
    Language {
        name: "JSON",
        extensions: &["json", "jsonc"],
        filenames: &[],
        // Not JSON's, but every `.json` anybody edits by hand is really JSONC,
        // and a comment drawn as a syntax error helps nobody.
        line_comment: &["//"],
        block_comment: Some(("/*", "*/")),
        nested_block_comment: false,
        quotes: b"\"",
        triple_quotes: false,
        escapes: true,
        keywords: &["true", "false", "null"],
        shared: &[],
    },
    Language {
        name: "YAML",
        extensions: &["yaml", "yml"],
        filenames: &[],
        line_comment: &["#"],
        block_comment: None,
        nested_block_comment: false,
        quotes: b"\"'",
        triple_quotes: false,
        escapes: true,
        keywords: &["true", "false", "null", "yes", "no"],
        shared: &[],
    },
    Language {
        name: "C",
        extensions: &[
            "c", "h", "cc", "cpp", "hpp", "cxx", "m", "mm", "java", "cs", "swift",
        ],
        filenames: &[],
        line_comment: &["//"],
        block_comment: Some(("/*", "*/")),
        nested_block_comment: false,
        quotes: b"\"'",
        triple_quotes: false,
        escapes: true,
        keywords: C_LIKE,
        shared: &[],
    },
];

/// The language a filename selects, if ket colours it.
///
/// Exact filenames beat extensions: `Cargo.lock` is TOML whatever `.lock`
/// might otherwise suggest. Matching is case-insensitive because a `.RS` and a
/// `README.MD` are the same files as their lowercase spellings.
pub fn language_for(path: &str) -> Option<&'static Language> {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();

    if let Some(found) = LANGUAGES.iter().find(|language| {
        language
            .filenames
            .iter()
            .any(|known| known.to_ascii_lowercase() == name)
    }) {
        return Some(found);
    }

    let extension = name.rsplit_once('.').map(|(_, ext)| ext)?;
    LANGUAGES
        .iter()
        .find(|language| language.extensions.contains(&extension))
}

/// Where every line of `text` begins, for a caller that highlights lines out of
/// order.
///
/// One entry per line, so `states[n]` is the state line `n` starts in and
/// `states[0]` is always [`State::Code`]. The editor draws rows it has never
/// drawn before as somebody scrolls, and a state computed only while walking
/// forwards would be a state it does not have.
pub fn opening_states(text: &str, language: &Language) -> Vec<State> {
    let mut state = State::Code;
    let mut states = Vec::new();
    for line in text.split('\n') {
        states.push(state);
        state = highlight_line(line, state, language).1;
    }
    states
}

/// One line's highlighted spans, in order and never overlapping.
pub type Spans = Vec<(Range<usize>, Token)>;

/// One pass of the lexer over the whole document, returning every line's
/// opening state and its highlighted spans in character offsets.
///
/// The same lex [`opening_states`] already runs to learn where lines start —
/// this just keeps what it finds instead of discarding it, so a caller that
/// draws the same unedited lines on every scrolled frame looks each one up
/// instead of lexing it again.
pub fn highlighted_lines(text: &str, language: &Language) -> (Vec<State>, Vec<Spans>) {
    let mut state = State::Code;
    let mut states = Vec::new();
    let mut spans = Vec::new();
    for line in text.split('\n') {
        states.push(state);
        let (line_spans, next) = highlighted_line(line, state, language);
        spans.push(line_spans);
        state = next;
    }
    (states, spans)
}

/// One line's highlighted spans in character offsets, and what the next line
/// starts in — the step [`highlighted_lines`] repeats, for a caller that
/// re-lexes only the lines an edit touched.
pub fn highlighted_line(line: &str, start: State, language: &Language) -> (Spans, State) {
    let (byte_spans, next) = highlight_line(line, start, language);
    (char_spans(line, byte_spans), next)
}

/// Converts one line's spans from byte offsets, which is what the lexer
/// tracks, to character offsets, which is what a row's columns are.
fn char_spans(line: &str, byte_spans: Spans) -> Spans {
    if byte_spans.is_empty() {
        return Vec::new();
    }
    let char_of: std::collections::BTreeMap<usize, usize> = line
        .char_indices()
        .enumerate()
        .map(|(index, (byte, _))| (byte, index))
        .chain(std::iter::once((line.len(), line.chars().count())))
        .collect();
    byte_spans
        .into_iter()
        .filter_map(|(range, token)| {
            let start = *char_of.get(&range.start)?;
            let end = *char_of.get(&range.end)?;
            Some((start..end, token))
        })
        .collect()
}

/// Colours one line, and says what the next line starts in.
///
/// Ranges are byte offsets into `line` and never overlap. Only what is coloured
/// is returned: plain code has no token and no range, because the editor paints
/// it in the pane's own text colour and a run saying so is a run it would have
/// to merge away again.
pub fn highlight_line(
    line: &str,
    start: State,
    language: &Language,
) -> (Vec<(Range<usize>, Token)>, State) {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut state = start;
    let mut i = 0usize;

    // Whatever was open at the end of the previous line is closed first, and
    // everything up to the close is that token — not this line's business to
    // re-decide.
    match state {
        State::BlockComment(depth) => {
            let (end, next) = finish_block_comment(bytes, 0, depth, language);
            push(&mut spans, 0..end, Token::Comment);
            state = next;
            i = end;
        }
        State::MultilineString(quote) => {
            let (end, next) = finish_multiline_string(bytes, 0, quote);
            push(&mut spans, 0..end, Token::Str);
            state = next;
            i = end;
        }
        State::Code => {}
    }
    if state != State::Code {
        return (spans, state);
    }

    while i < bytes.len() {
        let rest = &line[i..];

        // A comment to the end of the line beats everything after it.
        if let Some(marker) = language
            .line_comment
            .iter()
            .find(|marker| rest.starts_with(**marker))
        {
            let _ = marker;
            push(&mut spans, i..bytes.len(), Token::Comment);
            return (spans, State::Code);
        }

        if let Some((open, _)) = language.block_comment
            && rest.starts_with(open)
        {
            let from = i;
            let (end, next) = finish_block_comment(bytes, i + open.len(), 1, language);
            push(&mut spans, from..end, Token::Comment);
            state = next;
            i = end;
            if state != State::Code {
                return (spans, state);
            }
            continue;
        }

        let byte = bytes[i];

        if language.quotes.contains(&byte) {
            let from = i;
            let triple = language.triple_quotes
                && bytes.len() >= i + 3
                && bytes[i + 1] == byte
                && bytes[i + 2] == byte;
            let (end, next) = if triple {
                finish_multiline_string(bytes, i + 3, byte)
            } else {
                (
                    finish_string(bytes, i + 1, byte, language.escapes),
                    State::Code,
                )
            };
            push(&mut spans, from..end, Token::Str);
            state = next;
            i = end;
            if state != State::Code {
                return (spans, state);
            }
            continue;
        }

        if byte.is_ascii_digit() {
            let from = i;
            // One run for the whole literal: `0xff`, `1_000`, `3.14e-2`. A
            // lexer that stopped at the `.` would paint two numbers and a
            // separator where a person reads one value.
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            push(&mut spans, from..i, Token::Number);
            continue;
        }

        if byte.is_ascii_alphabetic() || byte == b'_' {
            let from = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &line[from..i];
            if language.keywords.contains(&word) || language.shared.contains(&word) {
                push(&mut spans, from..i, Token::Keyword);
            } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                // As far as a lexer can honestly go towards "this is a type":
                // a leading capital. It catches `String` and `Ok`, and it also
                // catches a constant, which is the price of not having a
                // parser.
                push(&mut spans, from..i, Token::Kind);
            }
            continue;
        }

        if byte.is_ascii_punctuation() {
            let from = i;
            while i < bytes.len()
                && bytes[i].is_ascii_punctuation()
                && !language.quotes.contains(&bytes[i])
                && !starts_comment(&line[i..], language)
            {
                i += 1;
            }
            // The guards above can stop before consuming anything — a quote or
            // a comment marker. Leaving the loop without advancing would spin.
            if i == from {
                i += 1;
                continue;
            }
            push(&mut spans, from..i, Token::Punctuation);
            continue;
        }

        // Whitespace, or a character outside ASCII — skipped whole. Stepping
        // one byte into a multi-byte character left `i` off a character
        // boundary, and slicing `line` there at the top of the loop panicked:
        // an `é` in an identifier or a `→` in code crashed the app.
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }

    (spans, State::Code)
}

/// Whether a comment of either shape starts here.
fn starts_comment(rest: &str, language: &Language) -> bool {
    language
        .line_comment
        .iter()
        .any(|marker| rest.starts_with(*marker))
        || language
            .block_comment
            .is_some_and(|(open, _)| rest.starts_with(open))
}

/// Walks to the end of a block comment, or to the end of the line.
fn finish_block_comment(
    bytes: &[u8],
    mut i: usize,
    mut depth: usize,
    language: &Language,
) -> (usize, State) {
    let Some((open, close)) = language.block_comment else {
        return (bytes.len(), State::Code);
    };
    let (open, close) = (open.as_bytes(), close.as_bytes());

    while i < bytes.len() {
        if bytes[i..].starts_with(close) {
            depth -= 1;
            i += close.len();
            if depth == 0 {
                return (i, State::Code);
            }
            continue;
        }
        if language.nested_block_comment && bytes[i..].starts_with(open) {
            depth += 1;
            i += open.len();
            continue;
        }
        i += 1;
    }
    (bytes.len(), State::BlockComment(depth))
}

/// Walks to the closing triple quote, or to the end of the line.
fn finish_multiline_string(bytes: &[u8], mut i: usize, quote: u8) -> (usize, State) {
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote
            && bytes.len() >= i + 3
            && bytes[i + 1] == quote
            && bytes[i + 2] == quote
        {
            return (i + 3, State::Code);
        }
        i += 1;
    }
    (bytes.len(), State::MultilineString(quote))
}

/// Walks to the closing quote, or to the end of the line.
///
/// A single-quoted string that reaches the end of the line is *closed* there
/// rather than carried over: an apostrophe in a comment-free line of prose is
/// far more common than a string somebody forgot to close, and carrying it
/// would paint the rest of the file.
fn finish_string(bytes: &[u8], mut i: usize, quote: u8, escapes: bool) -> usize {
    while i < bytes.len() {
        if escapes && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// Adds a span, dropping the empty ones a boundary case can produce.
fn push(spans: &mut Vec<(Range<usize>, Token)>, range: Range<usize>, token: Token) {
    if range.start < range.end {
        spans.push((range, token));
    }
}
