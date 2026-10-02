//! One display row: the gutter, the rail, and the text drawn as a single
//! shaped line. See the parent module's "How a row is drawn" section.

use std::ops::Range;

use gpui::{
    AnyElement, App, Bounds, ContentMask, DispatchPhase, Element, ElementId, Font, FontFeatures,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ShapedLine, SharedString,
    Style as GpuiStyle, TextRun, WeakEntity, Window, div, fill, point, prelude::*, px, size,
};

use ket_core::diff::LineMark;
use ket_core::theme::Theme;

use crate::Shell;
use crate::paint::paint;
use crate::tabs::PaneId;

use super::geometry::Geometry;
use super::highlight::{Mark, mark_line};
use super::{CARET_WIDTH, GUTTER_GAP};

/// How wide the bar marking a changed line is.
///
/// Drawn in the air that is already between the numbers and the code — see
/// [`GUTTER_GAP`] — rather than in a column of its own, so a file with
/// nothing to say about it looks exactly as it did before this existed.
const MARK_WIDTH: Pixels = px(2.0);

/// How wide the stub marking removed lines is, and how tall.
///
/// Wider than [`MARK_WIDTH`] and only a couple of pixels tall, because it
/// means something different: not "this line", which a bar down the side of a
/// row says, but "the boundary this row sits on", which wants to read as a
/// mark across the rail rather than as a very short line.
const MARK_STUB_WIDTH: Pixels = px(8.0);

/// How tall that stub is. See [`MARK_STUB_WIDTH`].
const MARK_STUB_HEIGHT: Pixels = px(2.0);

/// What one rendered row needs, bundled so [`render_editor_row`] takes two
/// arguments instead of a dozen.
pub(super) struct RowView<'a> {
    pub(super) theme: Theme,
    pub(super) geometry: Geometry,
    /// How far this row's text sits from the pane's left edge.
    pub(super) gutter_width: Pixels,
    pub(super) display_row: usize,
    pub(super) buffer_line: usize,
    /// Char index in the document of this row's first character. What turns
    /// a column under the pointer into a document position.
    pub(super) start: usize,
    pub(super) text: &'a str,
    pub(super) selection: Option<Range<usize>>,
    /// Whether the selection carries on past the end of this row, so the
    /// line break it swallows is drawn as selected rather than as a notch.
    pub(super) selection_runs_on: bool,
    pub(super) matches: &'a [(Range<usize>, bool)],
    /// What the highlighter made of this row, in the row's own columns.
    pub(super) syntax: &'a [(Range<usize>, ket_core::syntax::Token)],
    /// The column the caret sits at, when it is on this row and lit.
    pub(super) cursor: Option<usize>,
    pub(super) current_line: bool,
    pub(super) show_number: bool,
    /// What this row's line does against `HEAD`, when it does anything.
    pub(super) git: Option<LineMark>,
}

/// One display row's text: a `gpui` element rather than a row of styled
/// `div`s. See the `editor` module doc's "How a row is drawn" section for why.
struct RowText {
    shell: WeakEntity<Shell>,
    key: SharedString,
    /// The pane this row is drawn in, focused by a press on it.
    pane: Option<PaneId>,
    /// Char index in the document of this row's first character.
    start: usize,
    text: SharedString,
    /// The row's highlight spans, as byte ranges into `text`, in order and
    /// covering all of it — which is what [`gpui::TextSystem::shape_line`]
    /// wants of its runs.
    marks: Vec<(Range<usize>, Mark)>,
    /// The caret's byte offset into `text`, when it is on this row and lit.
    caret: Option<usize>,
    /// Whether the selection carries on past this row's end.
    runs_on: bool,
    /// One character's advance, for the block that stands in for the line
    /// break a selection swallows. The one thing on this row measured rather
    /// than shaped, because a line break has no glyph to shape.
    space_width: Pixels,
    /// The gutter to this element's left, which its hitbox reaches back over.
    gutter_width: Pixels,
    theme: Theme,
}

/// What [`RowText`]'s prepaint works out and its paint and mouse handling
/// then need.
struct PaintedRow {
    line: ShapedLine,
    hitbox: Hitbox,
}

/// The byte offset of the character boundary nearest `x`, measured from the
/// row's left edge.
///
/// Not [`gpui::LineLayout::closest_index_for_x`], which considers only the
/// boundaries a glyph *starts* at: past the last glyph's start it gives up
/// and returns the row's end, so the whole of a line's final character
/// snapped the cursor past it. The end of the row is a boundary like any
/// other, and it is the one that method leaves out of the comparison.
fn nearest_boundary(line: &ShapedLine, x: Pixels) -> usize {
    let mut nearest = 0;
    let mut closest = x.abs();
    for run in &line.runs {
        for glyph in &run.glyphs {
            let distance = (glyph.position.x - x).abs();
            if distance < closest {
                closest = distance;
                nearest = glyph.index;
            }
        }
    }
    if (line.width - x).abs() < closest {
        nearest = line.len();
    }
    nearest
}

/// The document position under window x, for a row that starts at `start`
/// and was shaped as `line`.
fn position_at(line: &ShapedLine, text: &str, left: Pixels, x: Pixels, start: usize) -> usize {
    let byte = nearest_boundary(line, (x - left).max(px(0.0)));
    start + text.get(..byte).map_or(0, |head| head.chars().count())
}

impl RowText {
    /// Registers this row's own mouse handling.
    ///
    /// Here rather than on a surrounding `div` because the arithmetic needs
    /// the shaped line, and this is the only place it exists.
    fn listen(&self, bounds: Bounds<Pixels>, painted: &PaintedRow, window: &mut Window) {
        let left = bounds.left();
        let start = self.start;

        let press_hitbox = painted.hitbox.clone();
        let press_line = painted.line.clone();
        let press_text = self.text.clone();
        let press_shell = self.shell.clone();
        let press_key = self.key.clone();
        let press_pane = self.pane;
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !press_hitbox.is_hovered(window)
            {
                return;
            }
            let at = position_at(&press_line, &press_text, left, event.position.x, start);
            let _ = press_shell.update(cx, |shell, cx| {
                // The find bar may remain visible while the document is being
                // edited. A click in the document gives the shell's editor
                // surface the keyboard back, so Delete acts on the selection
                // instead of shortening the find query and jumping matches.
                window.focus(&shell.focus);
                // The pane's own press handler never sees this one (see
                // below), so it is asked for here: in a split, the pane
                // clicked into is the one the keys have to go to.
                if let Some(pane) = press_pane {
                    shell.focus_pane(pane);
                }
                shell.editor_mouse_down(press_key.as_ref(), at, event);
                // The caret lit where it lands, as a keystroke lights it: a
                // press in the blink's dark half otherwise moves a caret
                // nobody can see, and reads as a click that did nothing.
                shell.caret_wake(cx);
                cx.notify();
            });
            // The pane behind the rows treats a press it still sees as a
            // click on the empty space past the end of the document.
            cx.stop_propagation();
        });

        let drag_hitbox = painted.hitbox.clone();
        let drag_line = painted.line.clone();
        let drag_text = self.text.clone();
        let drag_shell = self.shell.clone();
        let drag_key = self.key.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.pressed_button != Some(MouseButton::Left)
                || !drag_hitbox.is_hovered(window)
            {
                return;
            }
            let at = position_at(&drag_line, &drag_text, left, event.position.x, start);
            let _ = drag_shell.update(cx, |shell, cx| {
                if shell.editor_drag(drag_key.as_ref(), at) {
                    cx.notify();
                }
            });
        });
    }
}

impl IntoElement for RowText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RowText {
    type RequestLayoutState = ();
    type PrepaintState = PaintedRow;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = GpuiStyle {
            // Whatever the row has left once the gutter has taken its width:
            // an element with no children measures as nothing, so growing
            // from that gives it exactly the remainder.
            flex_grow: 1.0,
            size: gpui::Size {
                height: window.line_height().into(),
                ..GpuiStyle::default().size
            },
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        // The code face and size, taken from what the list cascaded down
        // rather than rebuilt here, so a row cannot be shaped in one font and
        // measured in another.
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        // Ligatures off, as they are in the terminal, and here for a reason
        // beyond taste: a ligature is one glyph standing for two characters,
        // and both the caret and a click work in glyphs. With `->` drawn as
        // an arrow there is no position between the two to put a cursor at,
        // and a cursor the keyboard puts there draws past both of them.
        let font = Font {
            features: FontFeatures::disable_ligatures(),
            ..style.font()
        };

        let line = if self.text.is_empty() {
            ShapedLine::default()
        } else {
            let runs: Vec<TextRun> = self
                .marks
                .iter()
                .map(|(range, mark)| {
                    let (color, background_color) = mark.colors(&self.theme, style.color);
                    TextRun {
                        len: range.len(),
                        font: mark.font(&font),
                        color,
                        background_color,
                        underline: None,
                        strikethrough: None,
                    }
                })
                .collect();
            window
                .text_system()
                .shape_line(self.text.clone(), font_size, &runs, None)
        };

        // The hitbox reaches back over the gutter, which sits immediately to
        // this element's left. A drag that wanders onto the line numbers —
        // which selecting right-to-left does constantly — is still a drag
        // over this row, and without this it lands on no row at all and the
        // selection freezes until the pointer comes back.
        let hitbox = Bounds::new(
            point(bounds.left() - self.gutter_width, bounds.top()),
            size(bounds.size.width + self.gutter_width, bounds.size.height),
        );

        PaintedRow {
            line,
            hitbox: window.insert_hitbox(hitbox, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        painted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = window.line_height();
        let theme = self.theme;
        let runs_on = self.runs_on;
        let space_width = self.space_width;
        let caret = self.caret;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if runs_on {
                // The line break the selection swallows, drawn a character
                // wide, so a selection over several lines reads as a block
                // instead of as fragments with a notch at every line end.
                let selection: Hsla = paint(theme.selection).into();
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.left() + painted.line.width, bounds.top()),
                        size(space_width, bounds.size.height),
                    ),
                    selection,
                ));
            }
            // Two calls, deliberately: `ShapedLine::paint` draws glyphs and
            // nothing else, and the backgrounds its runs carry — which is
            // what the selection and the current find match are — come only
            // from `paint_background`. Painting one without the other draws
            // a selection that highlights nothing.
            painted
                .line
                .paint_background(bounds.origin, line_height, window, cx)
                .ok();
            painted
                .line
                .paint(bounds.origin, line_height, window, cx)
                .ok();
            if let Some(caret) = caret {
                let accent: Hsla = paint(theme.accent).into();
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.left() + painted.line.x_for_index(caret),
                            bounds.top(),
                        ),
                        size(CARET_WIDTH, bounds.size.height),
                    ),
                    accent,
                ));
            }
        });

        self.listen(bounds, painted, window);
    }
}

/// The rail between the line numbers and the code, for one row.
///
/// A line that is there gets a bar the height of the row, so a run of changed
/// lines reads as one block the way the change itself does. Lines that are
/// *not* there have no row to run a bar down, so a removal is a stub on the
/// edge of the row that closed over it — which is the only place a reader
/// could look for it.
fn git_rail(mark: Option<LineMark>, width: Pixels, t: &Theme) -> impl IntoElement {
    let rail = div().w(width).flex_shrink_0().flex().justify_center();
    let Some(mark) = mark else {
        return rail;
    };

    let colour = paint(match mark {
        LineMark::Added => t.diff.added,
        LineMark::Modified => t.diff.modified,
        LineMark::RemovedAbove | LineMark::RemovedBelow => t.diff.removed,
    });
    let stub = || div().w(MARK_STUB_WIDTH).h(MARK_STUB_HEIGHT).bg(colour);

    match mark {
        LineMark::Added | LineMark::Modified => rail.child(div().w(MARK_WIDTH).bg(colour)),
        LineMark::RemovedAbove => rail.items_start().child(stub()),
        LineMark::RemovedBelow => rail.items_end().child(stub()),
    }
}

/// Renders one display row: gutter, the row's text, and the caret if the
/// cursor is on it.
pub(super) fn render_editor_row(
    view: RowView<'_>,
    shell: &WeakEntity<Shell>,
    key: &SharedString,
    pane: Option<PaneId>,
) -> AnyElement {
    let RowView {
        theme: t,
        geometry,
        gutter_width,
        display_row,
        buffer_line,
        start,
        text,
        selection,
        selection_runs_on,
        matches,
        syntax,
        cursor,
        current_line,
        show_number,
        git,
    } = view;

    // `mark_line` counts characters; a shaped line indexes bytes.
    let byte_of: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect();
    let marks: Vec<(Range<usize>, Mark)> = mark_line(byte_of.len() - 1, selection, matches, syntax)
        .into_iter()
        .map(|(range, mark)| (byte_of[range.start]..byte_of[range.end], mark))
        .collect();

    let gutter = if show_number {
        (buffer_line + 1).to_string()
    } else {
        String::new()
    };

    div()
        .id(("editor-row", display_row))
        .flex()
        // A row is laid out as a root of its own — `uniform_list` hands each
        // one the pane's width as available space — so without this it is
        // sized to its contents. [`RowText`] has no contents to measure, only
        // a share of what is left over, so the row would come out exactly as
        // wide as the gutter and the text would be laid out into nothing.
        // Full width is also what makes the current line's highlight run to
        // the edge of the pane rather than stopping where the text does.
        .w_full()
        .when(current_line, |el| el.bg(paint(t.hover)))
        .child(
            div()
                .w(gutter_width)
                .flex_shrink_0()
                .flex()
                .child(
                    div()
                        .flex_grow()
                        .text_right()
                        .pl_1()
                        // The line the caret is on takes the full ink. The wash
                        // behind the row says which line it is from across the
                        // pane; this says it again in the one column somebody
                        // reading a stack trace is actually looking at.
                        .text_color(paint(if current_line {
                            t.text.primary
                        } else {
                            t.text.dim
                        }))
                        .child(gutter),
                )
                // The rail takes exactly the air that was the gutter's right
                // padding, so the numbers sit where they always did and a file
                // with nothing changed in it looks untouched by this.
                .child(git_rail(git, gutter_width.min(GUTTER_GAP), &t)),
        )
        .child(RowText {
            shell: shell.clone(),
            key: key.clone(),
            pane,
            start,
            text: text.to_owned().into(),
            marks,
            caret: cursor.map(|column| byte_of[column]),
            runs_on: selection_runs_on,
            space_width: geometry.cell_width,
            gutter_width,
            theme: t,
        })
        .into_any_element()
}
