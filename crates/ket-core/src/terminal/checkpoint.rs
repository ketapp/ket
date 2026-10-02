//! Terminal checkpoints: a screen another client can rebuild, and the output
//! that follows it.
//!
//! Everything else in [`super`] assumes the renderer reads the grid in-process.
//! A client in another process — the desktop talking to a host, or a phone —
//! cannot: all it gets is bytes. So a checkpoint is what a *fresh* terminal
//! needs to arrive at the same screen, written as that terminal's own input:
//! escape sequences, from [`serialise`], that rebuild the scrollback, both
//! screens, the cursors, the modes and the pen. ANSI rather than a struct,
//! because the other end is not always alacritty — xterm.js on a phone reads
//! the same bytes.
//!
//! A checkpoint is only useful with the output after it, so [`Stream`] numbers
//! every byte the parser applies. A checkpoint says which byte it is accurate
//! through, and [`Stream::since`] hands out what came after. Both are read under
//! the grid lock, so they can never disagree.
//!
//! Two kinds of parser state live outside the grid and have to travel with a
//! checkpoint, or the first bytes after it are misread:
//!
//! - **An unfinished escape sequence.** Reads split output anywhere, including
//!   halfway through `ESC [ 3 8 ; 5`. The parser holds the first half
//!   privately, so a fresh parser handed only the second half prints it as
//!   text. [`EscapeTracker`] follows the parser closely enough to know where the
//!   unfinished sequence began, and the checkpoint ends with those bytes.
//! - **A synchronized update.** Between `CSI ? 2026 h` and `l` the parser
//!   buffers output instead of applying it, so the grid is behind the stream.
//!   The buffered bytes are always the newest ones, so the checkpoint ends by
//!   opening an update of its own and replaying them into it.
//!
//! Changes that reach the grid without passing through the stream — a resize,
//! or ket's own clear — cannot be replayed, and bump the stream's epoch
//! instead: a client on an older epoch needs a new checkpoint.

use std::collections::VecDeque;
use std::ops::Range;

use alacritty_terminal::grid::{Charsets, Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{
    CharsetIndex, Color, CursorShape, CursorStyle, NamedColor, StandardCharset,
};

use super::TerminalSize;

/// Most output kept for clients catching up after a checkpoint.
///
/// Allocated only for a terminal that has been checkpointed at least once, so
/// the desktop's own panes never pay for it. A client that falls further
/// behind than this takes a new checkpoint rather than the bytes.
pub const REPLAY_BYTES: usize = 1024 * 1024;

/// Most bytes of one unfinished escape sequence carried into a checkpoint.
///
/// Sequences are short, with two exceptions: an OSC 52 clipboard write and a
/// DCS image can run to megabytes. Past this the tail is cut, and the one
/// sequence that straddled the checkpoint arrives on the client damaged.
const MAX_PENDING: usize = 256 * 1024;

/// Begins a synchronized update.
const BEGIN_SYNC: &[u8] = b"\x1b[?2026h";

/// Where a client is in one terminal's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCursor {
    /// Bumped whenever the grid changes outside the byte stream.
    pub epoch: u64,
    /// Bytes applied to the grid since the terminal opened.
    pub seq: u64,
}

/// A screen, as the bytes that rebuild it on a fresh terminal.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    /// The output this checkpoint is accurate through.
    pub cursor: OutputCursor,
    /// The size the receiving terminal must have before applying `ansi`.
    pub size: TerminalSize,
    /// Scrollback lines the receiving terminal must be able to keep.
    pub scrollback: usize,
    /// The escape sequences that rebuild the screen.
    pub ansi: Vec<u8>,
}

/// What a client catching up after a checkpoint gets.
#[derive(Debug)]
pub enum Since {
    /// The output after the client's cursor, and the cursor after it.
    Output {
        /// Raw pty output, to be applied in order.
        bytes: Vec<u8>,
        /// Where the client is once it has applied `bytes`.
        cursor: OutputCursor,
    },
    /// The output is gone or no longer applies: take a new checkpoint.
    Reset,
}

/// The numbered output of one terminal, and the parser state a checkpoint of
/// it needs.
///
/// Updated by the parser thread under the grid lock, immediately after each
/// chunk is applied, which is what keeps [`Stream::cursor`] and the grid in
/// step.
#[derive(Debug, Default)]
pub struct Stream {
    /// See [`OutputCursor::epoch`].
    epoch: u64,
    /// See [`OutputCursor::seq`].
    seq: u64,
    /// Where the parser is inside an escape sequence, if it is.
    escape: EscapeTracker,
    /// The newest bytes, held by the parser in a synchronized update.
    held: Vec<u8>,
    /// Whether a synchronized update is open.
    syncing: bool,
    /// Recent output, once anyone has asked for it.
    replay: Option<VecDeque<u8>>,
}

impl Stream {
    /// An empty stream at the start of a terminal's output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a chunk the parser has just applied.
    ///
    /// `held` is how many of the newest bytes the parser is holding back in a
    /// synchronized update, or `None` when no update is open. The parser's
    /// buffer is always a suffix of the stream, which is what lets this keep a
    /// copy of it without reaching inside the parser.
    pub fn record(&mut self, bytes: &[u8], held: Option<usize>) {
        self.seq += bytes.len() as u64;
        self.escape.advance(bytes);

        match held {
            Some(count) => {
                self.syncing = true;
                self.held.extend_from_slice(bytes);
                let stale = self.held.len().saturating_sub(count);
                self.held.drain(..stale);
            }
            None => {
                self.syncing = false;
                release(&mut self.held);
            }
        }

        if let Some(replay) = &mut self.replay {
            let bytes = &bytes[bytes.len().saturating_sub(REPLAY_BYTES)..];
            let overflow = (replay.len() + bytes.len()).saturating_sub(REPLAY_BYTES);
            replay.drain(..overflow);
            replay.extend(bytes);
        }
    }

    /// Starts keeping output for clients to catch up from.
    pub fn arm(&mut self) {
        self.replay.get_or_insert_with(VecDeque::new);
    }

    /// Declares that the grid changed outside the stream.
    ///
    /// Every client's cursor stops applying, and the output kept so far with
    /// it: none of it can be replayed onto a grid that no longer exists.
    pub fn bump(&mut self) {
        self.epoch += 1;
        if let Some(replay) = &mut self.replay {
            replay.clear();
        }
    }

    /// Where the stream is now.
    pub fn cursor(&self) -> OutputCursor {
        OutputCursor {
            epoch: self.epoch,
            seq: self.seq,
        }
    }

    /// The parser state a checkpoint has to carry, because the grid does not
    /// hold it.
    pub fn tail(&self) -> Tail {
        if self.syncing {
            let mut bytes = BEGIN_SYNC.to_vec();
            bytes.extend_from_slice(&self.held);
            // The held bytes are replayed, and set the character REP repeats
            // for themselves. One REP inside them that refers back past the
            // start of the update is the case this gives up on.
            Tail {
                bytes,
                last_printed: None,
            }
        } else {
            Tail {
                bytes: self.escape.pending().to_vec(),
                last_printed: self.escape.last_printed(),
            }
        }
    }

    /// The output after `from`, if it is still here and still applies.
    pub fn since(&self, from: OutputCursor) -> Since {
        if from.epoch != self.epoch || from.seq > self.seq {
            return Since::Reset;
        }
        let kept = self.replay.as_ref().map_or(0, VecDeque::len) as u64;
        let oldest = self.seq - kept;
        if from.seq < oldest {
            return Since::Reset;
        }

        let skip = (from.seq - oldest) as usize;
        let bytes = self
            .replay
            .as_ref()
            .map(|replay| replay.range(skip..).copied().collect())
            .unwrap_or_default();
        Since::Output {
            bytes,
            cursor: self.cursor(),
        }
    }
}

/// Parser state that lives outside the grid, which a checkpoint ends with.
#[derive(Debug, Clone, Default)]
pub struct Tail {
    /// Bytes to replay after the screen: an unfinished escape sequence, or an
    /// open synchronized update and what it has buffered.
    pub bytes: Vec<u8>,
    /// The last character the parser printed, which is what REP (`CSI n b`)
    /// repeats. `None` when nothing has been printed, or it is not known.
    pub last_printed: Option<char>,
}

/// Empties a buffer, and gives its memory back if it grew large.
fn release(buffer: &mut Vec<u8>) {
    buffer.clear();
    if buffer.capacity() > 64 * 1024 {
        buffer.shrink_to_fit();
    }
}

/// Where the parser is, as far as a checkpoint cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Between sequences.
    #[default]
    Ground,
    /// Inside a UTF-8 character, with this many bytes still to come.
    Utf8(u8),
    /// After `ESC`.
    Escape,
    /// After `ESC` and one or more intermediate bytes.
    EscapeIntermediate,
    /// Inside `ESC [`.
    Csi,
    /// Inside `ESC ]`, which BEL or ST ends.
    Osc,
    /// Inside `ESC P`, `ESC X`, `ESC ^` or `ESC _`, which only ST ends.
    String,
}

/// Follows the parser's state machine closely enough to know whether it is
/// between sequences, and if not, which bytes began the one it is inside.
///
/// Not a parser. It never interprets a sequence, and it only has to agree with
/// `vte` on one question — "is this byte the end of it?" — and the DEC state
/// diagram `vte` implements answers that the same way for both. Control
/// characters inside a sequence are left out of the pending bytes, because the
/// parser has already executed them: replaying them would run them twice.
///
/// It also remembers the last character printed, which the parser keeps for
/// REP and which no grid records.
#[derive(Debug, Default)]
pub struct EscapeTracker {
    /// The parser's state after the last byte.
    state: State,
    /// Bytes of the unfinished sequence, or empty between sequences.
    pending: Vec<u8>,
    /// The last character printed, as the parser remembers it for REP.
    last_printed: Option<char>,
}

impl EscapeTracker {
    /// Follows a chunk of output.
    pub fn advance(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    /// The bytes of the unfinished sequence, or nothing between sequences.
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }

    /// The last character printed, which REP repeats.
    pub fn last_printed(&self) -> Option<char> {
        self.last_printed
    }

    /// Follows one byte.
    fn step(&mut self, byte: u8) {
        match self.state {
            State::Ground => match byte {
                0x1b => self.begin(State::Escape, byte),
                0x20..=0x7e => self.last_printed = Some(char::from(byte)),
                0xc2..=0xdf => self.begin(State::Utf8(1), byte),
                0xe0..=0xef => self.begin(State::Utf8(2), byte),
                0xf0..=0xf4 => self.begin(State::Utf8(3), byte),
                0x80..=0xff => self.last_printed = Some(char::REPLACEMENT_CHARACTER),
                _ => {}
            },
            State::Utf8(left) => {
                if (0x80..=0xbf).contains(&byte) {
                    if left == 1 {
                        self.keep(byte);
                        self.last_printed = Some(
                            std::str::from_utf8(&self.pending)
                                .ok()
                                .and_then(|s| s.chars().next())
                                .unwrap_or(char::REPLACEMENT_CHARACTER),
                        );
                        self.finish();
                    } else {
                        self.keep(byte);
                        self.state = State::Utf8(left - 1);
                    }
                } else {
                    // A broken character: the parser prints a replacement and
                    // reads this byte afresh.
                    self.last_printed = Some(char::REPLACEMENT_CHARACTER);
                    self.finish();
                    self.step(byte);
                }
            }
            // Cancel, and substitute, abandon any sequence at all.
            _ if byte == 0x18 || byte == 0x1a => self.finish(),
            // ESC also ends a string, as the first half of ST. The string has
            // been dispatched by then, so only the ESC is still pending.
            _ if byte == 0x1b => self.begin(State::Escape, byte),
            State::Escape => match byte {
                b'[' => self.enter(State::Csi, byte),
                b']' => self.enter(State::Osc, byte),
                b'P' | b'X' | b'^' | b'_' => self.enter(State::String, byte),
                0x20..=0x2f => self.enter(State::EscapeIntermediate, byte),
                0x30..=0x7e => self.finish(),
                _ => {}
            },
            State::EscapeIntermediate => match byte {
                0x20..=0x2f => self.keep(byte),
                0x30..=0x7e => self.finish(),
                _ => {}
            },
            State::Csi => match byte {
                0x40..=0x7e => self.finish(),
                0x20..=0x3f => self.keep(byte),
                _ => {}
            },
            State::Osc => match byte {
                0x07 => self.finish(),
                _ => self.keep(byte),
            },
            State::String => self.keep(byte),
        }
    }

    /// Starts a new sequence, dropping whatever was pending.
    fn begin(&mut self, state: State, byte: u8) {
        self.pending.clear();
        self.pending.push(byte);
        self.state = state;
    }

    /// Moves to `state` inside the current sequence.
    fn enter(&mut self, state: State, byte: u8) {
        self.keep(byte);
        self.state = state;
    }

    /// Adds a byte to the pending sequence, up to [`MAX_PENDING`].
    fn keep(&mut self, byte: u8) {
        if self.pending.len() < MAX_PENDING {
            self.pending.push(byte);
        }
    }

    /// Back between sequences.
    fn finish(&mut self) {
        self.state = State::Ground;
        release(&mut self.pending);
    }
}

/// What a checkpoint needs from a terminal, copied out from under the lock.
///
/// Copying the grid is a memcpy and a reference count per styled cell;
/// turning it into escape sequences is the slow part — tens of milliseconds
/// for a full scrollback in which every cell has a colour of its own — and the
/// parser and the renderer both wait for as long as the lock is held. So the
/// lock covers this, and [`serialise`] runs after it is released.
pub struct Snapshot {
    mode: TermMode,
    /// The screen being shown.
    grid: Grid<Cell>,
    /// The primary screen, when a full-screen program has the alternate one.
    primary: Option<Grid<Cell>>,
    region: Range<Line>,
    tabs: Vec<bool>,
    colors: Colors,
    cursor_style: CursorStyle,
}

impl Snapshot {
    /// Copies what a checkpoint of `term` needs. Call it under the grid lock.
    pub fn of<T>(term: &Term<T>) -> Self {
        let mode = *term.mode();
        Self {
            mode,
            grid: term.grid().clone(),
            primary: mode
                .contains(TermMode::ALT_SCREEN)
                .then(|| term.inactive_grid().clone()),
            region: term.scroll_region(),
            tabs: (0..term.columns())
                .map(|col| term.is_tab_stop(Column(col)))
                .collect(),
            colors: *term.colors(),
            cursor_style: term.cursor_style(),
        }
    }

    /// The size a client must give its terminal before applying the
    /// checkpoint.
    pub fn size(&self) -> TerminalSize {
        TerminalSize::new(self.grid.columns() as u16, self.grid.screen_lines() as u16)
    }

    /// Lines of scrollback the checkpoint writes.
    pub fn history(&self) -> usize {
        self.primary.as_ref().unwrap_or(&self.grid).history_size()
    }
}

/// Writes the escape sequences that rebuild a snapshot on a fresh terminal of
/// the same size, ending with `tail` — see [`Stream::tail`].
///
/// The order matters throughout, because every step is itself terminal input
/// with side effects on the steps after it:
///
/// 1. Content first, with every mode at its default, so autowrap, origin and
///    insert mode cannot bend it. The primary screen's history is written as
///    ordinary lines, and scrolls into the scrollback the way it did the first
///    time.
/// 2. The primary screen's cursor, and then — if a full-screen program is
///    running — the switch to the alternate screen and its content.
/// 3. The shown screen's saved cursor, and the character REP repeats, which
///    is set by printing it into blank cells and erasing them again.
/// 4. Tab stops and the scroll region, which both move the cursor.
/// 5. The modes, origin last, because it moves the cursor too.
/// 6. The live cursor: position, pending wrap, character sets and pen.
/// 7. Insert, newline and no-autowrap modes, which would have bent step 6.
pub fn serialise(snapshot: &Snapshot, title: Option<&str>, tail: &Tail) -> Vec<u8> {
    let mut out = Writer::default();
    let mode = snapshot.mode;
    let grid = &snapshot.grid;

    // Full reset, so a client can reuse a terminal rather than build a new one.
    out.put("\x1bc");
    out.colors(&snapshot.colors);
    out.cursor_style(snapshot.cursor_style);
    if let Some(title) = title.filter(|t| !t.chars().any(char::is_control)) {
        out.put(&format!("\x1b]2;{title}\x1b\\"));
    }

    if let Some(primary) = &snapshot.primary {
        out.rows(primary);
        // The primary screen's saved cursor needs no writing: entering the
        // alternate screen overwrites it with the live cursor, here and when
        // the program did it.
        out.cursor(primary, 0);
        out.put("\x1b[?1049h\x1b[H");
        // The alternate screen's cursor starts as a copy of the primary's,
        // character sets included; its content must not be mapped by them.
        out.designate(Charsets::default());
    }
    out.rows(grid);
    out.saved_cursor(grid);
    if let Some(c) = tail.last_printed {
        out.last_printed(grid, c);
    }

    out.tabs(&snapshot.tabs);
    let region = snapshot.region.clone();
    if region != (Line(0)..Line(grid.screen_lines() as i32)) {
        out.put(&format!("\x1b[{};{}r", region.start.0 + 1, region.end.0));
    }

    let defaults = TermMode::default();
    for &(flag, code) in PRIVATE_MODES {
        if mode.contains(flag) && !defaults.contains(flag) {
            out.put(&format!("\x1b[?{code}h"));
        } else if !mode.contains(flag) && defaults.contains(flag) {
            out.put(&format!("\x1b[?{code}l"));
        }
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        out.put("\x1b=");
    }
    let origin = if mode.contains(TermMode::ORIGIN) {
        out.put("\x1b[?6h");
        region.start.0
    } else {
        0
    };

    out.cursor(grid, origin);

    if mode.contains(TermMode::INSERT) {
        out.put("\x1b[4h");
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.put("\x1b[20h");
    }
    if !mode.contains(TermMode::LINE_WRAP) {
        out.put("\x1b[?7l");
    }

    out.out.extend_from_slice(&tail.bytes);
    out.out
}

/// Private modes written as plain DECSET/DECRST, against their defaults.
///
/// Missing on purpose: the alternate screen and origin mode, which move things
/// and are ordered by hand in [`serialise`]; line wrap, which has to stay on
/// until the cursor is placed; the keyboard protocol flags, which ket never
/// enables; and vi mode, which is alacritty's own and not a terminal mode.
const PRIVATE_MODES: &[(TermMode, u16)] = &[
    (TermMode::APP_CURSOR, 1),
    (TermMode::SHOW_CURSOR, 25),
    (TermMode::MOUSE_REPORT_CLICK, 1000),
    (TermMode::MOUSE_DRAG, 1002),
    (TermMode::MOUSE_MOTION, 1003),
    (TermMode::FOCUS_IN_OUT, 1004),
    (TermMode::UTF8_MOUSE, 1005),
    (TermMode::SGR_MOUSE, 1006),
    (TermMode::ALTERNATE_SCROLL, 1007),
    (TermMode::URGENCY_HINTS, 1042),
    (TermMode::BRACKETED_PASTE, 2004),
];

/// Cell flags that are style, as opposed to layout.
const STYLE: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

/// Everything SGR and OSC 8 set: what the next printed character looks like.
#[derive(Debug, Clone, PartialEq)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: Flags,
    underline: Option<Color>,
    link: Option<Hyperlink>,
}

impl Pen {
    /// The pen `cell` was written with.
    fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags & STYLE,
            underline: cell.underline_color(),
            link: cell.hyperlink(),
        }
    }
}

impl Default for Pen {
    fn default() -> Self {
        Self::of(&Cell::default())
    }
}

/// The output, and what the terminal reading it will believe at each point.
///
/// Tracking the pen and the character sets is what keeps a checkpoint small:
/// a run of cells in one style costs one SGR, not one per cell.
#[derive(Debug, Default)]
struct Writer {
    out: Vec<u8>,
    pen: Pen,
    charsets: Charsets,
}

impl Writer {
    fn put(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    /// Writes every line of `grid`, scrollback first.
    ///
    /// A soft-wrapped line is written in full and left to wrap on its own,
    /// which is what gives the client the same wrap flag and lets it reflow the
    /// same way. A hard-ended line is trimmed to its last written cell and
    /// ended with CRLF, with the pen reset first: a line that scrolls in takes
    /// the pen's background.
    fn rows(&mut self, grid: &Grid<Cell>) {
        let cols = grid.columns();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let blank = Cell::default();
        let mut continued = false;

        for line in top..=bottom {
            let row = &grid[Line(line)];
            let wrapped = line != bottom && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            let mut end = if wrapped {
                cols
            } else {
                (0..cols)
                    .rev()
                    .find(|&col| row[Column(col)] != blank)
                    .map_or(0, |col| col + 1)
            };
            // A continuation needs at least one character, or nothing wraps
            // into it and the line above loses its wrap flag.
            if continued {
                end = end.max(1);
            }

            for col in 0..end {
                let cell = &row[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    // Recreated by the wide character that owns it.
                    continue;
                }
                self.cell(cell, col, cols);
            }

            if !wrapped {
                self.pen(&Pen::default());
                if continued && end < cols {
                    // The wrap that made this line filled it with the pen of
                    // the character that caused it.
                    self.put("\x1b[K");
                }
                if line != bottom {
                    self.put("\r\n");
                }
            }
            continued = wrapped;
        }
    }

    /// Prints one cell, at column `col` of `cols`, in its own pen.
    ///
    /// A tab is the odd one out: alacritty records `\t` in the cell a tab
    /// started from, so the client has to be made to run a tab from that same
    /// cell. A blank goes down first for the pen, the tab overwrites it, and
    /// the cursor is then put back one column on — a tab ends wherever the tab
    /// stops say, and the next cell is not there.
    fn cell(&mut self, cell: &Cell, col: usize, cols: usize) {
        self.pen(&Pen::of(cell));
        if cell.c == '\t' {
            self.put(&format!(" \x1b[{}G\t", col + 1));
            if col + 1 < cols {
                self.put(&format!("\x1b[{}G", col + 2));
            }
            return;
        }
        let mut buf = [0u8; 4];
        self.put(cell.c.encode_utf8(&mut buf));
        for &c in cell.zerowidth().unwrap_or_default() {
            self.put(c.encode_utf8(&mut buf));
        }
    }

    /// Places the cursor for `grid` and gives it its pen and character sets.
    ///
    /// `origin` is the top of the scroll region when origin mode is on, which
    /// makes every position relative to it. A cursor that has just written the
    /// last column is *past* the line without having left it; the only way to
    /// say that to a terminal is to write that last cell again.
    fn cursor(&mut self, grid: &Grid<Cell>, origin: i32) {
        let cursor = &grid.cursor;
        let line = cursor.point.line.0;
        if cursor.input_needs_wrap {
            let row = &grid[cursor.point.line];
            let mut col = cursor.point.column.0;
            if col > 0 && row[Column(col)].flags.contains(Flags::WIDE_CHAR_SPACER) {
                col -= 1;
            }
            self.goto(line - origin, col);
            self.cell(&row[Column(col)], col, grid.columns());
        } else {
            self.goto(line - origin, cursor.point.column.0);
        }
        self.designate(cursor.charsets);
        self.pen(&Pen::of(&cursor.template));
    }

    /// Writes the saved cursor of the screen being shown, with DECSC.
    fn saved_cursor(&mut self, grid: &Grid<Cell>) {
        let saved = &grid.saved_cursor;
        if *saved == Default::default() {
            return;
        }
        self.goto(saved.point.line.0, saved.point.column.0);
        self.designate(saved.charsets);
        self.pen(&Pen::of(&saved.template));
        self.put("\x1b7");
        // Back to plain ASCII, so nothing printed after this is remapped.
        self.designate(Charsets::default());
    }

    fn goto(&mut self, line: i32, col: usize) {
        self.put(&format!("\x1b[{};{}H", line + 1, col + 1));
    }

    /// Switches to `pen`, writing only what changed.
    fn pen(&mut self, pen: &Pen) {
        if pen.fg != self.pen.fg
            || pen.bg != self.pen.bg
            || pen.flags != self.pen.flags
            || pen.underline != self.pen.underline
        {
            let mut sgr = String::from("\x1b[0");
            for (flag, code) in [
                (Flags::BOLD, "1"),
                (Flags::DIM, "2"),
                (Flags::ITALIC, "3"),
                (Flags::UNDERLINE, "4"),
                (Flags::DOUBLE_UNDERLINE, "4:2"),
                (Flags::UNDERCURL, "4:3"),
                (Flags::DOTTED_UNDERLINE, "4:4"),
                (Flags::DASHED_UNDERLINE, "4:5"),
                (Flags::INVERSE, "7"),
                (Flags::HIDDEN, "8"),
                (Flags::STRIKEOUT, "9"),
            ] {
                if pen.flags.contains(flag) {
                    sgr.push(';');
                    sgr.push_str(code);
                }
            }
            color(&mut sgr, pen.fg, 30, 90, 38);
            color(&mut sgr, pen.bg, 40, 100, 48);
            if let Some(underline) = pen.underline {
                color(&mut sgr, underline, 0, 0, 58);
            }
            sgr.push('m');
            self.put(&sgr);
        }

        if pen.link != self.pen.link {
            match &pen.link {
                Some(link) => {
                    self.put(&format!("\x1b]8;id={};{}\x1b\\", link.id(), link.uri()));
                }
                None => self.put("\x1b]8;;\x1b\\"),
            }
        }

        self.pen = pen.clone();
    }

    /// Designates G0–G3, writing only what changed.
    fn designate(&mut self, charsets: Charsets) {
        for (index, intro) in [
            (CharsetIndex::G0, '('),
            (CharsetIndex::G1, ')'),
            (CharsetIndex::G2, '*'),
            (CharsetIndex::G3, '+'),
        ] {
            if charsets[index] != self.charsets[index] {
                let set = match charsets[index] {
                    StandardCharset::Ascii => 'B',
                    StandardCharset::SpecialCharacterAndLineDrawing => '0',
                };
                self.put(&format!("\x1b{intro}{set}"));
            }
        }
        self.charsets = charsets;
    }

    /// Clears the tab stops and sets them again, if they are not the default.
    fn tabs(&mut self, tabs: &[bool]) {
        if tabs
            .iter()
            .enumerate()
            .all(|(col, &stop)| stop == (col % 8 == 0))
        {
            return;
        }
        self.put("\x1b[3g");
        for (col, _) in tabs.iter().enumerate().filter(|(_, stop)| **stop) {
            self.put(&format!("\x1b[{}G\x1bH", col + 1));
        }
    }

    /// Leaves `c` as the character REP repeats, without changing the screen.
    ///
    /// The parser remembers the last character it *printed*, and the last
    /// thing a checkpoint prints is whatever cell it happened to draw last.
    /// So `c` is printed into three blank cells in a row and erased again:
    /// three, so that a wide character fits and a combining one, which lands
    /// on the cell before, is erased with the rest. A screen with no three
    /// blank cells in a row keeps the wrong character; REP straight after a
    /// checkpoint on such a screen is the case this gives up on.
    fn last_printed(&mut self, grid: &Grid<Cell>, c: char) {
        let blank = Cell::default();
        let cols = grid.columns();
        let spot = (0..grid.screen_lines() as i32).find_map(|line| {
            let row = &grid[Line(line)];
            (0..cols.saturating_sub(2))
                .find(|&col| (col..col + 3).all(|col| row[Column(col)] == blank))
                .map(|col| (line, col))
        });
        let Some((line, col)) = spot else {
            return;
        };
        self.pen(&Pen::default());
        self.goto(line, col + 1);
        let mut buf = [0u8; 4];
        self.put(c.encode_utf8(&mut buf));
        self.goto(line, col);
        self.put("\x1b[3X");
    }

    /// Redefines the palette entries the program changed.
    fn colors(&mut self, colors: &Colors) {
        for index in 0..256 {
            if let Some(rgb) = colors[index] {
                self.put(&format!(
                    "\x1b]4;{index};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    rgb.r, rgb.g, rgb.b
                ));
            }
        }
        for (named, code) in [
            (NamedColor::Foreground, 10),
            (NamedColor::Background, 11),
            (NamedColor::Cursor, 12),
        ] {
            if let Some(rgb) = colors[named] {
                self.put(&format!(
                    "\x1b]{code};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    rgb.r, rgb.g, rgb.b
                ));
            }
        }
    }

    /// Sets the cursor shape, if the program changed it.
    fn cursor_style(&mut self, style: CursorStyle) {
        if style == CursorStyle::default() {
            return;
        }
        let steady = match style.shape {
            CursorShape::Block => 2,
            CursorShape::Underline => 4,
            CursorShape::Beam => 6,
            // Neither has an escape sequence; only alacritty's own
            // configuration and vi mode produce them.
            CursorShape::HollowBlock | CursorShape::Hidden => return,
        };
        let code = if style.blinking { steady - 1 } else { steady };
        self.put(&format!("\x1b[{code} q"));
    }
}

/// Appends one colour to an SGR sequence.
///
/// `base` and `bright` are the first code of the eight normal and eight bright
/// named colours; `extended` is the 38/48/58 introducer. A named colour is
/// written as one, because alacritty stores `SGR 31` and `SGR 38;5;1` as
/// different colours and the client has to arrive at the same one.
fn color(sgr: &mut String, color: Color, base: u16, bright: u16, extended: u16) {
    let code = match color {
        Color::Spec(rgb) => format!("{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => format!("{extended};5;{index}"),
        Color::Named(named) => {
            let index = match named as usize {
                index @ 0..=15 => index,
                // Dim colours only exist in the renderer; the cell holds the
                // normal one. Mapped back in case one ever arrives here.
                index @ 259..=266 => index - 259,
                // The defaults, which the leading reset already selected.
                _ => return,
            };
            if base == 0 {
                format!("{extended};5;{index}")
            } else if index < 8 {
                (base as usize + index).to_string()
            } else {
                (bright as usize + index - 8).to_string()
            }
        }
    };
    sgr.push(';');
    sgr.push_str(&code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::{Event as TermEvent, EventListener};
    use alacritty_terminal::vte::ansi::Processor;

    struct NoopListener;
    impl EventListener for NoopListener {
        fn send_event(&self, _event: TermEvent) {}
    }

    fn term(cols: u16, rows: u16) -> Term<NoopListener> {
        let size = super::super::TerminalSize::new(cols, rows);
        Term::new(super::super::term_config(1000), &size, NoopListener)
    }

    fn feed(term: &mut Term<NoopListener>, bytes: &[u8]) {
        let mut parser: Processor = Processor::new();
        parser.advance(term, bytes);
    }

    /// Reads one visible row as trimmed text, for comparing two terminals'
    /// screens without caring about their exact padding.
    fn row_text(term: &Term<NoopListener>, line: i32) -> String {
        let grid = term.grid();
        let row = &grid[Line(line)];
        (0..grid.columns())
            .map(|col| row[Column(col)].c)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    // ---- EscapeTracker -------------------------------------------------

    #[test]
    fn plain_text_leaves_nothing_pending_and_remembers_the_last_character() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(b"hello");
        assert!(tracker.pending().is_empty());
        assert_eq!(tracker.last_printed(), Some('o'));
    }

    #[test]
    fn an_unfinished_csi_sequence_stays_pending() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(b"before\x1b[3");
        assert_eq!(tracker.pending(), b"\x1b[3");
    }

    #[test]
    fn a_completed_csi_sequence_clears_the_pending_bytes() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(b"\x1b[31mred");
        assert!(tracker.pending().is_empty());
        assert_eq!(tracker.last_printed(), Some('d'));
    }

    #[test]
    fn an_osc_sequence_ends_on_bel() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(b"\x1b]2;title\x07after");
        assert!(tracker.pending().is_empty());
        assert_eq!(tracker.last_printed(), Some('r'));
    }

    #[test]
    fn an_unterminated_osc_sequence_stays_pending() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(b"\x1b]2;still going");
        assert_eq!(tracker.pending(), b"\x1b]2;still going");
    }

    #[test]
    fn a_string_sequence_is_only_ended_by_the_escape_half_of_st() {
        let mut tracker = EscapeTracker::default();
        // ESC P ... ESC \ (a DCS, terminated by ST). Only the ESC that begins
        // ST survives as pending; the rest of the string is dispatched.
        tracker.advance(b"\x1bPsome data\x1b\\");
        assert!(tracker.pending().is_empty());
    }

    #[test]
    fn cancel_and_substitute_abandon_any_open_sequence() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(b"\x1b[3;1");
        tracker.advance(b"\x18");
        assert!(tracker.pending().is_empty());
    }

    #[test]
    fn a_two_byte_utf8_character_is_tracked_as_one_printed_character() {
        let mut tracker = EscapeTracker::default();
        tracker.advance("é".as_bytes());
        assert!(tracker.pending().is_empty());
        assert_eq!(tracker.last_printed(), Some('é'));
    }

    #[test]
    fn a_broken_utf8_continuation_falls_back_to_the_replacement_character_and_resumes() {
        let mut tracker = EscapeTracker::default();
        // A lead byte promising a continuation, followed by plain ASCII.
        tracker.advance(&[0xC3, b'A']);
        assert_eq!(tracker.last_printed(), Some('A'));
        assert!(tracker.pending().is_empty());
    }

    #[test]
    fn an_unfinished_utf8_character_stays_pending() {
        let mut tracker = EscapeTracker::default();
        tracker.advance(&[0xC3]);
        assert_eq!(tracker.pending(), &[0xC3]);
    }

    #[test]
    fn pending_bytes_are_capped_at_max_pending() {
        let mut tracker = EscapeTracker::default();
        let mut sequence = vec![0x1b, b'['];
        sequence.extend(std::iter::repeat_n(b'9', MAX_PENDING + 10));
        tracker.advance(&sequence);
        assert!(tracker.pending().len() <= MAX_PENDING);
    }

    // ---- Stream ----------------------------------------------------------

    #[test]
    fn a_fresh_stream_starts_at_the_origin() {
        let stream = Stream::new();
        assert_eq!(stream.cursor(), OutputCursor { epoch: 0, seq: 0 });
    }

    #[test]
    fn recording_without_a_hold_advances_the_sequence_and_clears_any_held_bytes() {
        let mut stream = Stream::new();
        stream.record(b"hello", Some(3));
        stream.record(b"world", None);
        assert_eq!(stream.cursor().seq, 10);
        let tail = stream.tail();
        // Not syncing any more, so the tail is whatever the escape tracker has
        // pending — nothing, since "world" is plain text.
        assert!(tail.bytes.is_empty());
    }

    #[test]
    fn a_synchronized_update_carries_its_held_bytes_into_the_tail() {
        let mut stream = Stream::new();
        stream.record(b"abcdef", Some(4));
        let tail = stream.tail();
        assert!(tail.bytes.starts_with(BEGIN_SYNC));
        assert!(tail.bytes.ends_with(b"cdef"));
        assert_eq!(tail.last_printed, None);
    }

    #[test]
    fn an_unfinished_escape_sequence_is_the_tail_when_not_syncing() {
        let mut stream = Stream::new();
        stream.record(b"text\x1b[3", None);
        let tail = stream.tail();
        assert_eq!(tail.bytes, b"\x1b[3");
        assert_eq!(tail.last_printed, Some('t'));
    }

    #[test]
    fn bump_advances_the_epoch_and_clears_anything_armed_for_replay() {
        let mut stream = Stream::new();
        stream.arm();
        stream.record(b"before", None);
        stream.bump();
        assert_eq!(stream.cursor().epoch, 1);

        // Caught up to exactly where the bump left the stream: nothing to
        // replay, but not a reset either.
        match stream.since(stream.cursor()) {
            Since::Output { bytes, .. } => assert!(bytes.is_empty()),
            Since::Reset => panic!("the client is exactly caught up"),
        }

        // The replay buffer was cleared by the bump, so a cursor from before
        // it — even one the bump's own epoch would otherwise accept — has
        // fallen out of the window and must reset.
        match stream.since(OutputCursor { epoch: 1, seq: 0 }) {
            Since::Reset => {}
            Since::Output { .. } => panic!("everything before the bump was dropped"),
        }
    }

    #[test]
    fn since_with_a_different_epoch_is_a_reset() {
        let mut stream = Stream::new();
        stream.arm();
        stream.record(b"data", None);
        stream.bump();
        match stream.since(OutputCursor { epoch: 0, seq: 0 }) {
            Since::Reset => {}
            Since::Output { .. } => panic!("a stale epoch must not be trusted"),
        }
    }

    #[test]
    fn since_past_the_current_sequence_is_a_reset() {
        let stream = Stream::new();
        match stream.since(OutputCursor { epoch: 0, seq: 100 }) {
            Since::Reset => {}
            Since::Output { .. } => panic!("the client claims to have seen the future"),
        }
    }

    #[test]
    fn since_older_than_what_replay_still_holds_is_a_reset() {
        let mut stream = Stream::new();
        stream.arm();
        stream.record(b"first chunk of output", None);
        // Ask for everything since before recording began, with nothing
        // trimmed from replay yet, this still succeeds...
        assert!(matches!(
            stream.since(OutputCursor { epoch: 0, seq: 0 }),
            Since::Output { .. }
        ));
        // ...but a client cursor claiming a seq below what replay ever held
        // at all (it never started before 0) can't happen with a real one;
        // instead we simulate replay having trimmed everything by recording
        // past REPLAY_BYTES so the oldest kept byte has moved forward.
        let mut stream = Stream::new();
        stream.arm();
        stream.record(&vec![b'x'; REPLAY_BYTES + 10], None);
        match stream.since(OutputCursor { epoch: 0, seq: 0 }) {
            Since::Reset => {}
            Since::Output { .. } => panic!("seq 0 fell out of the replay window"),
        }
    }

    #[test]
    fn since_returns_exactly_the_bytes_recorded_after_the_cursor() {
        let mut stream = Stream::new();
        stream.arm();
        stream.record(b"one", None);
        let midpoint = stream.cursor();
        stream.record(b"two", None);

        match stream.since(midpoint) {
            Since::Output { bytes, cursor } => {
                assert_eq!(bytes, b"two");
                assert_eq!(cursor, stream.cursor());
            }
            Since::Reset => panic!("the client is well within the replay window"),
        }
    }

    // ---- Snapshot / serialise --------------------------------------------

    #[test]
    fn a_snapshot_reports_the_grids_own_size() {
        let t = term(20, 5);
        let snapshot = Snapshot::of(&t);
        assert_eq!(snapshot.size(), super::super::TerminalSize::new(20, 5));
    }

    #[test]
    fn a_fresh_terminal_has_no_scrollback() {
        let t = term(20, 5);
        let snapshot = Snapshot::of(&t);
        assert_eq!(snapshot.history(), 0);
    }

    #[test]
    fn serialising_and_replaying_a_checkpoint_reproduces_the_screen() {
        let mut original = term(20, 5);
        feed(&mut original, b"hello, checkpoint\r\ntwo lines");

        let snapshot = Snapshot::of(&original);
        let tail = Tail::default();
        let bytes = serialise(&snapshot, None, &tail);

        let mut replayed = term(20, 5);
        feed(&mut replayed, &bytes);

        assert_eq!(row_text(&replayed, 0), "hello, checkpoint");
        assert_eq!(row_text(&replayed, 1), "two lines");
        assert_eq!(
            replayed.grid().cursor.point,
            original.grid().cursor.point,
            "the cursor lands where it was when the checkpoint was taken"
        );
    }

    #[test]
    fn a_clean_title_is_written_and_a_control_carrying_one_is_dropped() {
        let t = term(10, 2);
        let snapshot = Snapshot::of(&t);
        let tail = Tail::default();

        let clean = serialise(&snapshot, Some("my title"), &tail);
        assert!(
            clean.windows(4).any(|w| w == b"]2;m"),
            "a clean title is written as an OSC 2 sequence"
        );

        let dirty = serialise(&snapshot, Some("bad\x07title"), &tail);
        assert!(
            !dirty.windows(4).any(|w| w == b"]2;b"),
            "a title carrying a control character is dropped rather than sent broken"
        );
    }

    #[test]
    fn the_tails_bytes_are_replayed_after_the_screen() {
        let t = term(10, 2);
        let snapshot = Snapshot::of(&t);
        let tail = Tail {
            bytes: b"\x1b[3".to_vec(),
            last_printed: None,
        };
        let bytes = serialise(&snapshot, None, &tail);
        assert!(bytes.ends_with(b"\x1b[3"));
    }

    // ---- color() -----------------------------------------------------------

    #[test]
    fn a_red_foreground_round_trips_through_serialise() {
        let mut original = term(10, 2);
        feed(&mut original, b"\x1b[31mr\x1b[0m");

        let snapshot = Snapshot::of(&original);
        let bytes = serialise(&snapshot, None, &Tail::default());

        let mut replayed = term(10, 2);
        feed(&mut replayed, &bytes);

        let original_cell = &original.grid()[Line(0)][Column(0)];
        let replayed_cell = &replayed.grid()[Line(0)][Column(0)];
        assert_eq!(original_cell.fg, replayed_cell.fg);
        assert_eq!(original_cell.c, 'r');
        assert_eq!(replayed_cell.c, 'r');
    }

    fn round_trip(bytes: &[u8]) -> (Term<NoopListener>, Term<NoopListener>) {
        let mut original = term(20, 5);
        feed(&mut original, bytes);
        let snapshot = Snapshot::of(&original);
        let replay = serialise(&snapshot, None, &Tail::default());
        let mut replayed = term(20, 5);
        feed(&mut replayed, &replay);
        (original, replayed)
    }

    #[test]
    fn the_alternate_screen_and_its_content_round_trip() {
        let (original, replayed) = round_trip(b"primary text\x1b[?1049h\x1b[Halternate text");

        assert!(replayed.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(row_text(&replayed, 0), "alternate text");
        assert_eq!(
            original.mode().contains(TermMode::ALT_SCREEN),
            replayed.mode().contains(TermMode::ALT_SCREEN)
        );
    }

    #[test]
    fn bracketed_paste_mode_round_trips() {
        let (_, replayed) = round_trip(b"\x1b[?2004h");
        assert!(replayed.mode().contains(TermMode::BRACKETED_PASTE));
    }

    #[test]
    fn insert_mode_round_trips() {
        let (_, replayed) = round_trip(b"\x1b[4h");
        assert!(replayed.mode().contains(TermMode::INSERT));
    }

    #[test]
    fn origin_mode_and_the_scroll_region_round_trip() {
        let (original, replayed) = round_trip(b"\x1b[2;4r\x1b[?6h");
        assert!(replayed.mode().contains(TermMode::ORIGIN));
        assert_eq!(replayed.scroll_region(), original.scroll_region());
    }

    #[test]
    fn application_keypad_mode_round_trips() {
        let (_, replayed) = round_trip(b"\x1b=");
        assert!(replayed.mode().contains(TermMode::APP_KEYPAD));
    }

    #[test]
    fn line_wrap_being_off_round_trips() {
        let (_, replayed) = round_trip(b"\x1b[?7l");
        assert!(!replayed.mode().contains(TermMode::LINE_WRAP));
    }

    #[test]
    fn snapshot_history_counts_scrolled_lines() {
        let mut original = term(10, 2);
        assert_eq!(Snapshot::of(&original).history(), 0);

        feed(&mut original, b"a\r\nb\r\nc\r\nd\r\n");
        assert!(Snapshot::of(&original).history() > 0);
    }
}
