//! Markdown, drawn: a note's description as it reads rather than as it was
//! typed.
//!
//! `pulldown-cmark` parses; this turns its events into `div`s for the blocks
//! and one [`StyledText`] per run of inline text, with a [`TextRun`] per
//! stretch of the same style. It is a reader, not an editor — the source is
//! edited as plain text in a composer, and this is what shows when nobody is
//! typing in it.
//!
//! What it draws: headings, paragraphs, emphasis, strong, strikethrough,
//! inline code and fenced or indented code blocks, links, block quotes,
//! ordered, unordered and task lists, and rules. Tables, footnotes and HTML
//! are not parsed as such; they read as the text they are made of, which is
//! still legible and is what the agent the note goes to will see anyway.

use gpui::{
    AnyElement, Div, FontStyle, FontWeight, Hsla, Pixels, SharedString, StrikethroughStyle,
    StyledText, TextRun, UnderlineStyle, div, font, prelude::*, px, relative,
};
use ket_core::theme::Theme;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::RADIUS_SM;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};

/// Line height, as a multiple of the text size: the composer's, so a note
/// does not change its spacing when it is clicked into.
const LINE_HEIGHT: f32 = 1.55;

/// Space between blocks.
const BLOCK_GAP: Pixels = px(8.0);

/// Width of a list item's marker column.
const MARKER_W: Pixels = px(20.0);

/// `source` drawn at `size`, as a column of blocks.
pub(crate) fn markdown(source: &str, size: Pixels, t: &Theme) -> Div {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut doc = Doc {
        t,
        size,
        frames: vec![Frame::new(Kind::Root)],
        inline: Inline::default(),
        style: Style::default(),
        code: None,
    };
    for event in Parser::new_ext(source, options) {
        doc.event(event);
    }
    doc.flush();
    let root = doc
        .frames
        .pop()
        .map(|frame| frame.children)
        .unwrap_or_default();
    div()
        .prose()
        .flex()
        .flex_col()
        .gap(BLOCK_GAP)
        .text_size(size)
        .line_height(relative(LINE_HEIGHT))
        .text_color(paint(t.text.primary))
        .children(root)
}

/// A block still being built, with what it holds so far.
struct Frame {
    kind: Kind,
    children: Vec<AnyElement>,
}

impl Frame {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            children: Vec::new(),
        }
    }
}

/// What a [`Frame`] is.
enum Kind {
    Root,
    Quote,
    /// A list, and the number its next item takes when it is ordered.
    List(Option<u64>),
    /// A list item, and the marker drawn before it.
    Item(SharedString),
    /// A heading of this level, 1 to 6.
    Heading(u8),
}

/// The inline style in force: how deep inside each span the text is.
#[derive(Default, Clone, Copy, PartialEq)]
struct Style {
    strong: u8,
    emphasis: u8,
    strike: u8,
    link: u8,
    code: bool,
}

/// Inline text gathered for the next [`StyledText`], and the style of each
/// stretch of it.
#[derive(Default)]
struct Inline {
    text: String,
    spans: Vec<(usize, Style)>,
}

struct Doc<'a> {
    t: &'a Theme,
    size: Pixels,
    frames: Vec<Frame>,
    inline: Inline,
    style: Style,
    /// A code block's text, while inside one.
    code: Option<String>,
}

impl Doc<'_> {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match self.code.as_mut() {
                Some(code) => code.push_str(&text),
                None => self.push(&text, self.style),
            },
            Event::Code(text) => self.push(
                &text,
                Style {
                    code: true,
                    ..self.style
                },
            ),
            Event::SoftBreak => self.push(" ", self.style),
            Event::HardBreak => self.push("\n", self.style),
            Event::Html(text) | Event::InlineHtml(text) => self.push(&text, self.style),
            Event::Rule => {
                self.flush();
                let rule = div().h(px(1.0)).my(px(4.0)).bg(paint(self.t.border));
                self.child(rule.into_any_element());
            }
            Event::TaskListMarker(done) => {
                let marker = if done { "\u{2611}" } else { "\u{2610}" };
                if let Some(frame) = self
                    .frames
                    .iter_mut()
                    .rev()
                    .find(|frame| matches!(frame.kind, Kind::Item(_)))
                {
                    frame.kind = Kind::Item(marker.into());
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.flush(),
            Tag::Heading { level, .. } => {
                self.flush();
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.frames.push(Frame::new(Kind::Heading(level)));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.frames.push(Frame::new(Kind::Quote));
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                self.frames.push(Frame::new(Kind::List(start)));
            }
            Tag::Item => {
                self.flush();
                let marker: SharedString = match self.frames.last_mut().map(|f| &mut f.kind) {
                    Some(Kind::List(Some(next))) => {
                        let marker = format!("{next}.");
                        *next += 1;
                        marker.into()
                    }
                    _ => "\u{2022}".into(),
                };
                self.frames.push(Frame::new(Kind::Item(marker)));
            }
            Tag::Emphasis => self.style.emphasis += 1,
            Tag::Strong => self.style.strong += 1,
            Tag::Strikethrough => self.style.strike += 1,
            Tag::Link { .. } | Tag::Image { .. } => self.style.link += 1,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.close(|frame, t, size| {
                    let Kind::Heading(level) = frame.kind else {
                        return None;
                    };
                    let scale = match level {
                        1 => 1.35,
                        2 => 1.2,
                        _ => 1.05,
                    };
                    Some(
                        div()
                            .flex()
                            .flex_col()
                            .when(level <= 2, |el| el.mt(px(4.0)))
                            .text_size(size * scale)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(paint(t.text.primary))
                            .children(frame.children)
                            .into_any_element(),
                    )
                });
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.close(|frame, t, _| {
                    Some(
                        div()
                            .flex()
                            .flex_col()
                            .gap(BLOCK_GAP)
                            .pl(px(12.0))
                            .border_l_2()
                            .border_color(paint(t.border))
                            .text_color(paint(t.text.dim))
                            .children(frame.children)
                            .into_any_element(),
                    )
                });
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                let code = code.trim_end_matches('\n').to_owned();
                let block = div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(RADIUS_SM)
                    .bg(paint(self.t.panel))
                    .border_1()
                    .border_color(paint(self.t.border))
                    .font_family(crate::fonts::chrome())
                    .text_size(self.size * 0.86)
                    .line_height(relative(1.45))
                    .overflow_hidden()
                    .child(code);
                self.child(block.into_any_element());
            }
            TagEnd::List(_) => {
                self.flush();
                self.close(|frame, _, _| {
                    Some(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .children(frame.children)
                            .into_any_element(),
                    )
                });
            }
            TagEnd::Item => {
                self.flush();
                self.close(|frame, t, _| {
                    let Kind::Item(marker) = frame.kind else {
                        return None;
                    };
                    Some(
                        div()
                            .flex()
                            .child(
                                div()
                                    .flex_none()
                                    .w(MARKER_W)
                                    .text_color(paint(t.text.dim))
                                    .child(marker),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(4.0))
                                    .children(frame.children),
                            )
                            .into_any_element(),
                    )
                });
            }
            TagEnd::Emphasis => self.style.emphasis = self.style.emphasis.saturating_sub(1),
            TagEnd::Strong => self.style.strong = self.style.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.style.strike = self.style.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => self.style.link = self.style.link.saturating_sub(1),
            _ => {}
        }
    }

    /// Adds `text` in `style` to the inline run being gathered.
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        let at = self.inline.text.len();
        if self
            .inline
            .spans
            .last()
            .is_none_or(|(_, last)| *last != style)
        {
            self.inline.spans.push((at, style));
        }
        self.inline.text.push_str(text);
    }

    /// Adds `element` to the block being built.
    fn child(&mut self, element: AnyElement) {
        if let Some(frame) = self.frames.last_mut() {
            frame.children.push(element);
        }
    }

    /// Pops the block being built, draws it with `draw`, and adds it to the
    /// one around it.
    fn close(&mut self, draw: impl FnOnce(Frame, &Theme, Pixels) -> Option<AnyElement>) {
        if self.frames.len() < 2 {
            return;
        }
        let Some(frame) = self.frames.pop() else {
            return;
        };
        if let Some(element) = draw(frame, self.t, self.size) {
            self.child(element);
        }
    }

    /// Turns the inline text gathered so far into one [`StyledText`] in the
    /// block being built.
    fn flush(&mut self) {
        let Inline { text, spans } = std::mem::take(&mut self.inline);
        if text.trim().is_empty() {
            return;
        }
        let t = self.t;
        let heading = self
            .frames
            .last()
            .is_some_and(|frame| matches!(frame.kind, Kind::Heading(_)));
        let quoted = self
            .frames
            .iter()
            .any(|frame| matches!(frame.kind, Kind::Quote));
        let ink: Hsla = paint(if quoted { t.text.dim } else { t.text.primary }).into();
        let link: Hsla = paint(t.accent).into();
        let code_bg: Hsla = alpha(paint(t.text.dim), 0.16).into();

        let runs = spans
            .iter()
            .enumerate()
            .map(|(index, (start, style))| {
                let end = spans.get(index + 1).map_or(text.len(), |(next, _)| *next);
                let mut face = font(if style.code {
                    crate::fonts::chrome()
                } else {
                    crate::fonts::prose()
                });
                face.weight = if style.strong > 0 || heading {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                };
                if style.emphasis > 0 {
                    face.style = FontStyle::Italic;
                }
                let color = if style.link > 0 { link } else { ink };
                TextRun {
                    len: end - start,
                    font: face,
                    color,
                    background_color: style.code.then_some(code_bg),
                    underline: (style.link > 0).then_some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(link),
                        wavy: false,
                    }),
                    strikethrough: (style.strike > 0).then_some(StrikethroughStyle {
                        thickness: px(1.0),
                        color: Some(color),
                    }),
                }
            })
            .collect();
        let text = StyledText::new(text).with_runs(runs);
        self.child(div().child(text).into_any_element());
    }
}
