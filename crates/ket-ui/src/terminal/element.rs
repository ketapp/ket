//! The terminal grid as a `gpui` element.
//!
//! `ket_core::terminal::Terminal` owns the pty, the parser and the grid —
//! everything this file does is read that grid and turn it into paint. It
//! is a real [`Element`] rather than a tree of `div`s, because a terminal is
//! not laid out, it is *plotted*: every cell has a known pixel rectangle,
//! and the fastest and most faithful way to draw ten thousand of them is to
//! compute those rectangles once and issue quads and shaped text runs
//! directly.
//!
//! **Only the visible screen is ever read.** `Term::renderable_content`'s
//! `display_iter` walks exactly `rows * cols` cells — the current screen, at
//! whatever scrollback offset the display is at — never the scrollback
//! behind it, which can be ten thousand lines.
//!
//! **Paint order**, back to front, in [`Element::paint`]:
//!
//! 1. the pane background, which also covers the padding;
//! 2. cell backgrounds, merged into one quad per contiguous run of the same
//!    colour on a row, and skipped entirely for the default colour;
//! 3. find matches, then the selection, translucent so the cell colours
//!    still show through;
//! 4. a filled block cursor, drawn *under* its glyph so the glyph inverts;
//! 5. text, one shaped run per contiguous stretch of matching style;
//! 6. every other cursor shape — beam, underline, and the hollow block an
//!    unfocused terminal shows — which sit on top of text;
//! 7. text being composed by an input method, over a patch of background.
//!
//! **Cell alignment.** Runs of plain ASCII are shaped as one string and
//! painted at the run's first cell, with every glyph forced to the cell
//! width so the shaper's own advances cannot drift off the grid. Anything
//! else — a wide character, a glyph with combining marks, an emoji or a
//! box-drawing character that the font falls back for — is shaped on its
//! own and placed at its own cell, since the fallback font's advance is not
//! ours to force. Ligatures are disabled: a ligature across two cells has
//! no single cell to live in.

use std::sync::Arc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, FocusHandle, Font, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, ShapedLine,
    SharedString, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill, outline, point,
    px, relative, size,
};

use std::ops::Range;

use crate::terminal::TerminalId;
use ket_core::terminal::alacritty_terminal::grid::{Dimensions, Row as GridRow};
use ket_core::terminal::alacritty_terminal::index::{Column, Point as GridPoint};
use ket_core::terminal::alacritty_terminal::selection::SelectionRange;
use ket_core::terminal::alacritty_terminal::term::cell::{Cell as GridCell, Flags};
use ket_core::terminal::alacritty_terminal::term::color::Colors as Palette;
use ket_core::terminal::alacritty_terminal::term::{TermDamage, TermMode};
use ket_core::terminal::alacritty_terminal::vte::ansi::CursorShape;
use ket_core::terminal::{Terminal, TerminalSize};
use ket_core::theme::Theme;

use super::TerminalView;
use super::colors::cell_colors;
use super::find::visible_matches;
use super::mouse::{Cell, Geometry};
use super::row_link_spans;
use crate::Shell;
use crate::paint::paint;

/// Glyph size. Terminal.app's default is a point smaller; Zed's a pixel or
/// two larger.
/// Fallback point size, used only if the shell hands over nothing sane.
///
/// The real value is [`crate::Shell`]'s `theme_config.code_font_size`, which is
/// a setting; this exists so the element still draws if that is ever zero.
const FALLBACK_FONT_SIZE: Pixels = px(12.);
/// Row height as a multiple of the font size. Every terminal sits between
/// 1.2 and 1.35; the shell's default text style is 1.618, which is what made
/// the prototype read as double-spaced.
const LINE_HEIGHT: f32 = 1.3;
/// Space between the pane's edge and the first column.
const PAD_X: Pixels = px(8.);
/// Space between the pane's top edge and the first row.
const PAD_TOP: Pixels = px(4.);
/// Width of the beam cursor and height of the underline cursor.
const THIN_CURSOR: Pixels = px(2.);
/// How much of the cell colours show through the selection.
const SELECTION_ALPHA: f32 = 0.35;
/// How strongly a find match, and the one navigation is on, wash their cells.
const MATCH_ALPHA: f32 = 0.22;
const CURRENT_MATCH_ALPHA: f32 = 0.6;

/// The terminal grid element for one worktree's terminal.
///
/// Built fresh every frame by `Shell::terminal_pane`; nothing here outlives
/// a frame. What the element learns about where it landed is handed back to
/// the pane's own view so mouse events, which reach the shell rather than the
/// element, can be mapped to cells.
pub(crate) struct TerminalElement {
    /// The shell, for routing mouse events and the input method. Touched only
    /// during *paint*, which is outside the window's accessed-entity tracking
    /// — see `TerminalView`, whose whole contract is that nothing `Shell` owns
    /// is read while the frame is being built.
    shell: Entity<Shell>,
    /// The pane this draws, for the state that *is* read while building it:
    /// the grid cache, and the geometry written back out.
    view: Entity<TerminalView>,
    /// Which terminal this is, in the shell's map.
    id: TerminalId,
    /// The grid to draw.
    term: Arc<Terminal>,
    /// The window's keyboard focus, which the text-input handler follows.
    focus: FocusHandle,
    /// The typeface, chosen once by the shell — see `crate::fonts`.
    family: SharedString,
    /// Point size for the grid, from the user's settings.
    font_size: Pixels,
    /// Resolved colours.
    theme: Theme,
    /// The shell-side state that changes how this frame draws.
    state: FrameState,
}

/// What the shell knows about a terminal that the element needs for one
/// frame: the per-terminal UI state in `TerminalHandle`, minus the handle.
pub(crate) struct FrameState {
    /// Whether this terminal's pane is the keyboard target. Combined with
    /// window focus at paint time to decide whether the cursor is solid.
    pub(crate) focused: bool,
    /// The blink phase, when the program asked for a blinking cursor.
    pub(crate) blink_visible: bool,
    /// Text an input method is still composing.
    pub(crate) marked: Option<String>,
    /// Whether the pointer is over a link, so the cursor becomes a hand.
    /// Which link it is does not matter: every link on screen is underlined
    /// whether the pointer is on it or not.
    pub(crate) hovered_link: bool,
}

/// The two entities a pane's element talks to, together because a call site
/// that needs one always needs the other.
pub(crate) struct Owners {
    /// Routing for mouse and the input method, at paint time only.
    pub(crate) shell: Entity<Shell>,
    /// The pane itself, for the grid cache and the geometry.
    pub(crate) view: Entity<TerminalView>,
}

/// The typeface and point size the grid draws with, together, because they
/// are chosen together and neither is meaningful alone.
pub(crate) struct GridFont {
    /// The monospace family, chosen once by the shell — see `crate::fonts`.
    pub(crate) family: SharedString,
    /// Point size, from the user's settings.
    pub(crate) size: Pixels,
}

impl TerminalElement {
    /// Builds the element for one frame.
    pub(crate) fn new(
        owners: Owners,
        id: TerminalId,
        term: Arc<Terminal>,
        focus: FocusHandle,
        font: GridFont,
        theme: Theme,
        state: FrameState,
    ) -> Self {
        let GridFont { family, size } = font;
        let Owners { shell, view } = owners;
        Self {
            shell,
            view,
            id,
            term,
            font_size: if size > px(0.) {
                size
            } else {
                FALLBACK_FONT_SIZE
            },
            focus,
            family,
            theme,
            state,
        }
    }

    /// The terminal's font at a given weight and slant.
    fn font(family: &SharedString, bold: bool, italic: bool) -> Font {
        Font {
            family: family.clone(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: None,
            weight: if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        }
    }
}

/// Everything [`Element::paint`] needs, computed once in
/// [`Element::prepaint`] while the grid is locked and the text system is
/// shaping.
pub(crate) struct Layout {
    /// Where the grid landed.
    geometry: Geometry,
    /// The element's mouse target.
    hitbox: Hitbox,
    /// The pane background.
    background: Hsla,
    /// Every screen row's backgrounds and text, top to bottom.
    rows: Vec<Arc<Row>>,
    /// The selection, as one rectangle per row.
    selection: Vec<CellRect>,
    /// Find matches on screen, the current one last so it draws on top.
    matches: Vec<CellRect>,
    /// The cursor, when it is on screen.
    cursor: Option<CursorLayout>,
    /// Text an input method is composing, shaped and ready to draw at the
    /// cursor.
    marked: Option<ShapedLine>,
    /// Whether this terminal is the keyboard target *and* the window is
    /// active — the condition for a solid cursor and for text input.
    active: bool,
}

/// A run of `cells` cells on one row, painted in one colour.
#[derive(Clone)]
struct CellRect {
    /// First cell.
    cell: Cell,
    /// How many cells the rectangle spans.
    cells: usize,
    /// The colour.
    color: Hsla,
}

/// One shaped run of text, positioned by its first cell.
///
/// Carries its own colour and decorations rather than leaving them inside
/// the `ShapedLine`: the run is painted glyph by glyph in [`paint_run`], not
/// through `ShapedLine::paint`, and that needs them to hand.
#[derive(Clone)]
struct TextLayout {
    /// Where the run starts.
    cell: Cell,
    /// The shaped glyphs.
    line: ShapedLine,
    /// The colour every glyph in the run is drawn in.
    color: Hsla,
    /// The run's underline, if it has one.
    underline: Option<UnderlineStyle>,
    /// The run's strikethrough, if it has one.
    strikethrough: Option<StrikethroughStyle>,
}

/// The cursor as it will be painted.
#[derive(Clone)]
struct CursorLayout {
    /// The cell (or two, over a wide character) the cursor occupies.
    bounds: Bounds<Pixels>,
    /// Which of the shapes to draw. Never `Hidden`.
    shape: CursorShape,
    /// The cursor colour.
    color: Hsla,
    /// Whether to draw it this frame: false during the off phase of a blink.
    visible: bool,
}

/// One screen row's share of the grid walk: its merged backgrounds and its
/// shaped text.
///
/// Held behind an `Arc` in [`GridCache`], so a row the program did not touch
/// is handed to the next frame as it stands rather than copied.
#[derive(Default)]
pub(crate) struct Row {
    /// Non-default cell backgrounds, merged into one rectangle per colour run.
    rects: Vec<CellRect>,
    /// Shaped text, positioned by its first cell.
    runs: Vec<TextLayout>,
}

/// What decides how *every* row is drawn. A change to any of it and the
/// whole screen is walked again.
#[derive(Clone, PartialEq)]
pub(crate) struct GridKey {
    /// How far the view is scrolled back. Scrolling moves every row at once
    /// (alacritty reports it as full damage too).
    display_offset: usize,
    /// Where the grid landed and how big a cell is.
    geometry: Geometry,
    /// Colours and point size, which the program's damage knows nothing about.
    theme: Theme,
    font_size: Pixels,
}

/// What the cursor's own row was drawn with. A filled block inverts the glyph
/// under it, so the blink phase and the focus reach that row's *runs* and
/// not merely the cursor — but only that row, which alacritty reports as
/// damaged on every read anyway.
#[derive(Clone, Copy, PartialEq)]
struct CursorKey {
    blink_visible: bool,
    active: bool,
    /// Whether the program asked for a blinking cursor at all, which decides
    /// whether the phase above is consulted.
    blinking: bool,
}

/// The grid walk, kept per row so a frame redoes only the rows that changed.
///
/// The walk visits every cell it is given, resolves its colours and link
/// state, merges cells into runs and shapes them. Doing that for the whole
/// screen on every change was most of a frame with an agent working in view:
/// its spinner and status line rewrite a few rows a few dozen times a second,
/// and every other row was walked and shaped again to produce what it
/// produced last time.
///
/// alacritty records which lines each write touched, and ket is its only
/// reader. So a frame asks it for that damage, walks exactly those rows, and
/// lays them over the rows it already has. The invariant is simple: damage is
/// reset only in the same lock that walks every row it names, so what is
/// cached is always what the grid holds. A hidden terminal's damage just
/// accumulates until it is next drawn.
pub(crate) struct GridCache {
    /// What every row was drawn under.
    key: GridKey,
    /// What the cursor's row was drawn under.
    cursor_key: CursorKey,
    /// The backend's repaint counter when this was last brought up to date.
    /// Equal counter and equal keys: nothing to do, not even read the damage.
    frame: u64,
    /// One entry per screen row.
    rows: Vec<Arc<Row>>,
    /// The cursor, when it is on screen.
    cursor: Option<CursorLayout>,
}

/// What a frame's pass under the grid lock decided.
enum Walk {
    /// Nothing that feeds the grid has moved: draw last frame's rows.
    Reuse,
    /// These rows, walked but not yet shaped.
    Rows {
        /// Whether every row was walked, so nothing cached is kept.
        full: bool,
        /// Each walked row, by index, with its backgrounds and text runs.
        rows: Vec<(usize, Vec<CellRect>, Vec<RunSpec>)>,
        cursor: Option<CursorLayout>,
    },
}

/// What every cell in a walk is resolved against.
struct WalkStyle<'a> {
    theme: &'a Theme,
    palette: &'a Palette,
    /// The pane background: a cell in it gets no rectangle of its own.
    background: Hsla,
    /// Where the cursor is, if on screen.
    cursor: Option<Cell>,
    /// Whether the cursor is a solid block this frame, which inverts the glyph
    /// under it.
    filled_cursor: bool,
}

/// Walks one screen row into background rectangles and unshaped text runs.
///
/// `links` are this row's link spans, underlined as the runs are built.
fn walk_row(
    cells: &GridRow<GridCell>,
    row: usize,
    cols: usize,
    style: &WalkStyle<'_>,
    links: &[Range<usize>],
) -> (Vec<CellRect>, Vec<RunSpec>) {
    let mut rects = Vec::new();
    let mut specs = Vec::new();
    let mut open: Option<RunSpec> = None;
    // A background rect still being extended.
    let mut open_rect: Option<CellRect> = None;

    for col in 0..cols {
        let cell = &cells[Column(col)];
        let at = Cell { row, col };

        // The spacer half of a wide character carries no glyph of its own
        // and shares its neighbour's background.
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            if let Some(rect) = open_rect.as_mut() {
                rect.cells += 1;
            }
            continue;
        }

        let colors = cell_colors(style.theme, style.palette, cell);
        let is_cursor = style.cursor == Some(at);

        // Background.
        let bg: Hsla = paint(colors.bg).into();
        if bg != style.background {
            match open_rect.as_mut() {
                Some(rect) if rect.color == bg => rect.cells += 1,
                _ => {
                    if let Some(rect) = open_rect.take() {
                        rects.push(rect);
                    }
                    open_rect = Some(CellRect {
                        cell: at,
                        cells: 1,
                        color: bg,
                    });
                }
            }
        } else if let Some(rect) = open_rect.take() {
            rects.push(rect);
        }

        // Foreground.
        let mut fg: Hsla = paint(colors.fg).into();
        if is_cursor && style.filled_cursor {
            fg = style.background;
        }
        let is_link = links.iter().any(|cols| cols.contains(&col));
        let underline = if cell.flags.intersects(Flags::ALL_UNDERLINES) {
            Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(paint(colors.underline).into()),
                wavy: cell.flags.contains(Flags::UNDERCURL),
            })
        } else if is_link {
            Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(fg),
                wavy: false,
            })
        } else {
            None
        };
        let strikethrough = cell
            .flags
            .contains(Flags::STRIKEOUT)
            .then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(fg),
            });
        let run_style = RunStyle {
            color: fg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline,
            strikethrough,
        };

        let ch = cell.c;
        let zerowidth = cell.zerowidth();
        let plain = ch.is_ascii_graphic() || ch == ' ';
        let simple = plain && zerowidth.is_none() && !cell.flags.contains(Flags::WIDE_CHAR);

        // A blank with nothing drawn on it extends an open run (it will be
        // trimmed if nothing follows) and never starts one.
        if ch == ' ' && simple && underline.is_none() && strikethrough.is_none() {
            if let Some(spec) = open.as_mut() {
                if spec.style == run_style || spec.force_width {
                    spec.text.push(' ');
                    spec.trailing_blanks += 1;
                } else if let Some(spec) = open.take() {
                    specs.push(spec);
                }
            }
            continue;
        }

        if !simple {
            // On its own cell, shaped on its own: see the module docs.
            if let Some(spec) = open.take() {
                specs.push(spec);
            }
            let mut spec = RunSpec::new(at, run_style, false);
            spec.text.push(ch);
            if let Some(marks) = zerowidth {
                spec.text.extend(marks);
            }
            specs.push(spec);
            continue;
        }

        match open.as_mut() {
            Some(spec) if spec.style == run_style && spec.force_width => {
                spec.text.push(ch);
                spec.trailing_blanks = 0;
            }
            _ => {
                if let Some(spec) = open.take() {
                    specs.push(spec);
                }
                let mut spec = RunSpec::new(at, run_style, true);
                spec.text.push(ch);
                open = Some(spec);
            }
        }
    }
    if let Some(spec) = open.take() {
        specs.push(spec);
    }
    if let Some(rect) = open_rect.take() {
        rects.push(rect);
    }
    (rects, specs)
}

/// One run of text as gathered from the grid, before shaping.
///
/// Gathered under the grid lock, shaped after it is released: shaping is
/// the slow half, and holding the lock through it would stall the parser
/// thread for every frame.
struct RunSpec {
    /// Where the run starts.
    cell: Cell,
    /// The characters, in order, with any combining marks appended to the
    /// cell they belong to.
    text: String,
    /// What every cell in the run shares.
    style: RunStyle,
    /// Whether every glyph can be pinned to the cell width — true only for
    /// runs of plain ASCII, see the module docs.
    force_width: bool,
    /// Blank cells at the end of the run, trimmed before shaping.
    trailing_blanks: usize,
}

/// The style a run of cells shares.
#[derive(Clone, PartialEq)]
struct RunStyle {
    color: Hsla,
    bold: bool,
    italic: bool,
    underline: Option<UnderlineStyle>,
    strikethrough: Option<StrikethroughStyle>,
}

impl RunSpec {
    fn new(cell: Cell, style: RunStyle, force_width: bool) -> Self {
        Self {
            cell,
            text: String::new(),
            style,
            force_width,
            trailing_blanks: 0,
        }
    }

    /// Shapes the run, minus any trailing blanks, or nothing if that leaves
    /// it empty.
    fn shape(
        mut self,
        window: &Window,
        family: &SharedString,
        cell_width: Pixels,
        font_size: Pixels,
    ) -> Option<TextLayout> {
        for _ in 0..self.trailing_blanks {
            self.text.pop();
        }
        if self.text.is_empty() {
            return None;
        }
        let run = TextRun {
            len: self.text.len(),
            font: TerminalElement::font(family, self.style.bold, self.style.italic),
            color: self.style.color,
            background_color: None,
            underline: self.style.underline,
            strikethrough: self.style.strikethrough,
        };
        let line = window.text_system().shape_line(
            self.text.into(),
            font_size,
            &[run],
            self.force_width.then_some(cell_width),
        );
        Some(TextLayout {
            cell: self.cell,
            line,
            color: self.style.color,
            underline: self.style.underline,
            strikethrough: self.style.strikethrough,
        })
    }
}

/// Draws one run's glyphs and decorations, inside the grid's layer.
///
/// What `ShapedLine::paint` does, minus the part that cost the most: it opens
/// a layer of its own for every line it paints, and a layer is an insert into
/// the scene's bounds tree, searched against everything already drawn. A
/// terminal is hundreds of runs, and the grid repaints whenever its program
/// writes — an agent's status line is a few dozen times a second — so that
/// insert was a third of every frame. A run here is always one line, one
/// style and left-aligned, which is all of `paint_line` this needs.
fn paint_run(run: &TextLayout, origin: Point<Pixels>, line_height: Pixels, window: &mut Window) {
    let layout = &*run.line;
    let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
    let baseline = origin.y + padding_top + layout.ascent;

    for shaped in &layout.runs {
        for glyph in &shaped.glyphs {
            let at = point(origin.x + glyph.position.x, baseline);
            // A glyph that cannot be rasterised is left out, as
            // `ShapedLine::paint` would; nothing else in the frame depends on
            // it.
            let _ = if glyph.is_emoji {
                window.paint_emoji(at, shaped.font_id, glyph.id, layout.font_size)
            } else {
                window.paint_glyph(at, shaped.font_id, glyph.id, layout.font_size, run.color)
            };
        }
    }

    // Placed where `paint_line` places them, so a decorated run looks the
    // same as it did when gpui drew it.
    if let Some(underline) = &run.underline {
        let style = UnderlineStyle {
            color: Some(underline.color.unwrap_or(run.color)),
            ..*underline
        };
        let at = point(origin.x, baseline + layout.descent * 0.618);
        window.paint_underline(at, layout.width, &style);
    }
    if let Some(strikethrough) = &run.strikethrough {
        let style = StrikethroughStyle {
            color: Some(strikethrough.color.unwrap_or(run.color)),
            ..*strikethrough
        };
        let at = point(
            origin.x,
            origin.y + (layout.ascent * 0.5 + padding_top + layout.ascent) * 0.5,
        );
        window.paint_strikethrough(at, layout.width, &style);
    }
}

/// Rows of the selection on screen, one rectangle each.
fn selection_rects(
    selection: Option<SelectionRange>,
    display_offset: usize,
    geometry: &Geometry,
    color: Hsla,
) -> Vec<CellRect> {
    let Some(range) = selection else {
        return Vec::new();
    };
    let to_row = |point: GridPoint| point.line.0 + display_offset as i32;
    let first = to_row(range.start);
    let last = to_row(range.end);
    let mut rects = Vec::new();
    for row in first.max(0)..=last.min(geometry.rows as i32 - 1) {
        let (start, end) = if range.is_block {
            (range.start.column.0, range.end.column.0)
        } else {
            let start = if row == first {
                range.start.column.0
            } else {
                0
            };
            let end = if row == last {
                range.end.column.0
            } else {
                geometry.cols - 1
            };
            (start, end)
        };
        let start = start.min(geometry.cols - 1);
        let end = end.min(geometry.cols - 1);
        if end < start {
            continue;
        }
        rects.push(CellRect {
            cell: Cell {
                row: row as usize,
                col: start,
            },
            cells: end - start + 1,
            color,
        });
    }
    rects
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Layout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let _span = crate::frametrace::span("terminal:prepaint");
        let theme = self.theme;
        let active = self.state.focused && window.is_window_active();

        // Metrics. The cell width is the font's advance for a typical glyph,
        // snapped to a device pixel so a row of them cannot accumulate a
        // fractional drift; the row height is a whole pixel for the same
        // reason.
        let scale = window.scale_factor().max(1.);
        let snap = |value: Pixels| px((f32::from(value) * scale).round() / scale);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&Self::font(&self.family, false, false));
        let cell_width = text_system
            .advance(font_id, self.font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(px(8.));
        let cell_width = snap(cell_width);
        let line_height = px((f32::from(self.font_size) * LINE_HEIGHT).round());

        let origin = point(
            snap(bounds.origin.x + PAD_X),
            snap(bounds.origin.y + PAD_TOP),
        );
        let available = size(bounds.size.width - PAD_X * 2., bounds.size.height - PAD_TOP);
        // Two columns at least: `alacritty_terminal` misbehaves with a wide
        // character in a one-column grid.
        let cols = ((f32::from(available.width) / f32::from(cell_width)).floor() as usize).max(2);
        let rows = ((f32::from(available.height) / f32::from(line_height)).floor() as usize).max(1);
        let geometry = Geometry {
            origin,
            cell_width,
            line_height,
            cols,
            rows,
        };

        // Idempotent when unchanged — see `Terminal::resize` — so calling it
        // on every frame costs nothing once the pane has settled at a size.
        let _ = self.term.resize(TerminalSize::new(
            cols.min(u16::MAX as usize) as u16,
            rows.min(u16::MAX as usize) as u16,
        ));

        let background: Hsla = paint(theme.terminal.background).into();
        let cursor_color: Hsla = paint(theme.terminal.cursor).into();
        let mut selection_color = paint(theme.accent);
        selection_color.a = SELECTION_ALPHA;
        let selection_color: Hsla = selection_color.into();
        let mut match_color = paint(theme.accent);
        match_color.a = MATCH_ALPHA;
        let mut current_color = paint(theme.accent);
        current_color.a = CURRENT_MATCH_ALPHA;
        let (match_color, current_color): (Hsla, Hsla) = (match_color.into(), current_color.into());
        // Borrowed out of the view for the frame: the search needs it
        // mutably, and under the grid's lock.
        let mut find = self.view.update(cx, |view, _| view.find.take());

        // Last frame's inputs, to decide how much of the walk to redo. The
        // keys alone: the rows are only touched once that is decided.
        let cached = self.view.read(cx).grid.as_ref().map(|cache| {
            (
                cache.key.clone(),
                cache.cursor_key,
                cache.frame,
                cache.rows.len(),
            )
        });

        // Everything under the lock is bounded by the screen size, never by
        // scrollback — see the module docs — and, when only a few rows
        // changed, by those rows.
        let (key, cursor_key, frame, screen_rows, selection, matches, blinking, walk) =
            self.term.with_term(|term| {
                let display_offset = term.grid().display_offset();
                let blinking = term.cursor_style().blinking;
                let screen_rows = term.screen_lines();
                let frame = self.term.frame();
                let key = GridKey {
                    display_offset,
                    geometry,
                    theme,
                    font_size: self.font_size,
                };
                let cursor_key = CursorKey {
                    blink_visible: self.state.blink_visible,
                    active,
                    blinking,
                };
                // Whether the cached rows still line up with the screen at
                // all. When they do not, damage is beside the point.
                let stale = cached
                    .as_ref()
                    .is_none_or(|(k, _, _, rows)| *k != key || *rows != screen_rows);
                let unchanged = !stale
                    && cached
                        .as_ref()
                        .is_some_and(|(_, c, f, _)| *c == cursor_key && *f == frame);

                let walk = if unchanged {
                    Walk::Reuse
                } else {
                    // Read and reset together: every row named here is walked
                    // below, under this same lock. See `GridCache`.
                    let damaged: Option<Vec<usize>> = match term.damage() {
                        TermDamage::Full => None,
                        TermDamage::Partial(lines) => Some(
                            lines
                                .map(|line| line.line)
                                .filter(|row| *row < screen_rows)
                                .collect(),
                        ),
                    };
                    term.reset_damage();
                    let (full, walk_rows) = match damaged {
                        Some(rows) if !stale => (false, rows),
                        _ => (true, (0..screen_rows).collect()),
                    };

                    let term_ref = &*term;
                    let content = term_ref.renderable_content();
                    let grid = term_ref.grid();
                    let cols = grid.columns();

                    // The cursor first, because a filled block inverts the
                    // glyph under it and the walk needs to know which cell.
                    let cursor_row = content.cursor.point.line.0 + display_offset as i32;
                    let cursor_on_screen = (0..rows as i32).contains(&cursor_row)
                        && content.cursor.point.column.0 < cols;
                    let cursor_shape = match content.cursor.shape {
                        CursorShape::Hidden => None,
                        _ if !active => Some(CursorShape::HollowBlock),
                        shape => Some(shape),
                    };
                    let cursor_cell = cursor_on_screen.then_some(Cell {
                        row: cursor_row as usize,
                        col: content.cursor.point.column.0,
                    });
                    let filled_cursor = cursor_shape == Some(CursorShape::Block)
                        && (!blinking || self.state.blink_visible);
                    let cursor_wide = cursor_on_screen
                        && grid[content.cursor.point].flags.contains(Flags::WIDE_CHAR);

                    let style = WalkStyle {
                        theme: &theme,
                        palette: content.colors,
                        background,
                        cursor: cursor_cell,
                        filled_cursor,
                    };
                    let rows = walk_rows
                        .into_iter()
                        .map(|row| {
                            let links = row_link_spans(term_ref, display_offset, row);
                            let line = Cell { row, col: 0 }.to_grid(display_offset).line;
                            let (rects, specs) = walk_row(&grid[line], row, cols, &style, &links);
                            (row, rects, specs)
                        })
                        .collect();

                    let cursor = match (cursor_cell, cursor_shape) {
                        (Some(cell), Some(shape)) => Some(CursorLayout {
                            bounds: geometry.cell_bounds(cell, if cursor_wide { 2 } else { 1 }),
                            shape,
                            color: cursor_color,
                            visible: !blinking || self.state.blink_visible || !active,
                        }),
                        _ => None,
                    };
                    Walk::Rows { full, rows, cursor }
                };

                // Every frame, walked or not: the mouse writes the selection
                // straight into `Term`, and it costs a rectangle per row.
                let selection = selection_rects(
                    term.renderable_content().selection,
                    display_offset,
                    &geometry,
                    selection_color,
                );
                // Every frame too, over the screen only — see `find`.
                let mut matches = Vec::new();
                let mut current = Vec::new();
                if let Some(find) = find.as_mut() {
                    for found in visible_matches(term, &mut find.regex) {
                        let is_current = find.current.as_ref() == Some(&found);
                        let range = SelectionRange::new(*found.start(), *found.end(), false);
                        let rects = selection_rects(
                            Some(range),
                            display_offset,
                            &geometry,
                            if is_current {
                                current_color
                            } else {
                                match_color
                            },
                        );
                        if is_current {
                            current.extend(rects);
                        } else {
                            matches.extend(rects);
                        }
                    }
                }
                matches.extend(current);
                (
                    key,
                    cursor_key,
                    frame,
                    screen_rows,
                    selection,
                    matches,
                    blinking,
                    walk,
                )
            });
        if find.is_some() {
            self.view.update(cx, |view, _| view.find = find);
        }

        // Shaped outside the lock, as ever, and only for the rows walked.
        let (rows, cursor) = match walk {
            Walk::Reuse => match self.view.read(cx).grid.as_ref() {
                Some(cache) => (cache.rows.clone(), cache.cursor.clone()),
                // Gone between the key read and here — impossible in
                // practice, but an empty pane is the wrong way to find out.
                None => (Vec::new(), None),
            },
            Walk::Rows {
                full,
                rows: walked,
                cursor,
            } => {
                let mut rows = if full {
                    Vec::new()
                } else {
                    self.view
                        .read(cx)
                        .grid
                        .as_ref()
                        .map(|cache| cache.rows.clone())
                        .unwrap_or_default()
                };
                rows.resize_with(screen_rows, Arc::default);
                for (row, rects, specs) in walked {
                    let runs = specs
                        .into_iter()
                        .filter_map(|spec| {
                            spec.shape(window, &self.family, cell_width, self.font_size)
                        })
                        .collect();
                    rows[row] = Arc::new(Row { rects, runs });
                }
                self.view.update(cx, |view, _| {
                    view.grid = Some(GridCache {
                        key,
                        cursor_key,
                        frame,
                        rows: rows.clone(),
                        cursor: cursor.clone(),
                    });
                });
                (rows, cursor)
            }
        };

        let marked = self
            .state
            .marked
            .as_ref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                let fg: Hsla = paint(theme.terminal.foreground).into();
                let run = TextRun {
                    len: text.len(),
                    font: Self::font(&self.family, false, false),
                    color: fg,
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(fg),
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(text.clone().into(), self.font_size, &[run], None)
            });

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        // Hand the shell what it needs to map mouse events and to place the
        // input method's candidate window; and start or stop the blink
        // timer to match what the program asked for.
        let cursor_bounds = cursor.as_ref().map(|cursor| cursor.bounds);
        self.view.update(cx, |view, cx| {
            view.geometry = Some(geometry);
            view.cursor_bounds = cursor_bounds;
            view.set_blinking(blinking && active, cx);
        });

        Layout {
            geometry,
            hitbox,
            background,
            rows,
            selection,
            matches,
            cursor,
            marked,
            active,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _span = crate::frametrace::span("terminal:paint");
        let geometry = layout.geometry;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // One layer for the whole grid. Everything in it shares one place
            // in the scene's draw order, so each quad and glyph is an append
            // rather than an insert into the scene's bounds tree — and gpui
            // still draws a layer's quads before its glyphs, which is the
            // order backgrounds, selection and a block cursor need.
            window.paint_layer(bounds, |window| {
                window.paint_quad(fill(bounds, layout.background));

                for rect in layout.rows.iter().flat_map(|row| &row.rects) {
                    window.paint_quad(fill(
                        geometry.cell_bounds(rect.cell, rect.cells),
                        rect.color,
                    ));
                }
                for rect in layout.matches.iter().chain(&layout.selection) {
                    window.paint_quad(fill(
                        geometry.cell_bounds(rect.cell, rect.cells),
                        rect.color,
                    ));
                }

                if let Some(cursor) = &layout.cursor
                    && cursor.visible
                    && cursor.shape == CursorShape::Block
                {
                    window.paint_quad(fill(cursor.bounds, cursor.color));
                }

                for run in layout.rows.iter().flat_map(|row| &row.runs) {
                    let origin = geometry.cell_bounds(run.cell, 1).origin;
                    paint_run(run, origin, geometry.line_height, window);
                }
            });

            if let Some(cursor) = &layout.cursor
                && cursor.visible
            {
                match cursor.shape {
                    CursorShape::Block | CursorShape::Hidden => {}
                    CursorShape::HollowBlock => {
                        window.paint_quad(outline(cursor.bounds, cursor.color, BorderStyle::Solid));
                    }
                    CursorShape::Beam => {
                        let bounds = Bounds {
                            origin: cursor.bounds.origin,
                            size: size(THIN_CURSOR, cursor.bounds.size.height),
                        };
                        window.paint_quad(fill(bounds, cursor.color));
                    }
                    CursorShape::Underline => {
                        let bounds = Bounds {
                            origin: point(
                                cursor.bounds.origin.x,
                                cursor.bounds.origin.y + cursor.bounds.size.height - THIN_CURSOR,
                            ),
                            size: size(cursor.bounds.size.width, THIN_CURSOR),
                        };
                        window.paint_quad(fill(bounds, cursor.color));
                    }
                }
            }

            if let (Some(marked), Some(cursor)) = (&layout.marked, &layout.cursor) {
                let origin = cursor.bounds.origin;
                let patch = Bounds {
                    origin,
                    size: size(marked.width, geometry.line_height),
                };
                window.paint_quad(fill(patch, layout.background));
                let _ = marked.paint(origin, geometry.line_height, window, cx);
            }

            self.register_mouse_listeners(&layout.hitbox, window);

            let style = if self.state.hovered_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            };
            window.set_cursor_style(style, &layout.hitbox);

            if layout.active {
                window.handle_input(
                    &self.focus,
                    ElementInputHandler::new(bounds, self.shell.clone()),
                    cx,
                );
            }
        });
    }
}

impl TerminalElement {
    /// Routes the window's mouse events to the shell for this terminal.
    ///
    /// Registered on the window rather than as element handlers so a drag
    /// that leaves the pane keeps extending the selection and a release
    /// outside it still ends one. Each handler checks the hitbox itself
    /// where that matters.
    fn register_mouse_listeners(&self, hitbox: &Hitbox, window: &mut Window) {
        let shell = self.shell.clone();
        let id = self.id;
        let target = hitbox.clone();
        let focus = self.focus.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !target.is_hovered(window) {
                return;
            }
            // A press in the grid takes the keyboard back from the find
            // strip's field, the way a press in any text area would.
            if !focus.is_focused(window) {
                window.focus(&focus);
            }
            shell.update(cx, |shell, cx| shell.terminal_mouse_down(id, event, cx));
        });

        let shell = self.shell.clone();
        let id = self.id;
        let target = hitbox.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = target.is_hovered(window);
            shell.update(cx, |shell, cx| {
                shell.terminal_mouse_move(id, event, hovered, cx);
            });
        });

        let shell = self.shell.clone();
        let id = self.id;
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            shell.update(cx, |shell, cx| shell.terminal_mouse_up(id, event, cx));
        });

        let shell = self.shell.clone();
        let id = self.id;
        let target = hitbox.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !target.should_handle_scroll(window) {
                return;
            }
            shell.update(cx, |shell, cx| shell.terminal_scroll(id, event, cx));
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Which modes matter to the element's own behaviour, re-exported so the
/// shell's handlers and this file agree on the names.
pub(crate) fn mode_of(term: &Terminal) -> TermMode {
    term.with_term(|term| *term.mode())
}
