//! The dropdown menu — one component, every menu.
//!
//! The design is the one the `+` button's menu follows: a floating panel with
//! a search row at the top, items in sections separated by hairlines, a hover
//! and a keyboard selection that share one colour, and each item's keybinding
//! drawn right-aligned in the platform's own script. The first user is the
//! tab strip's `+` button; anything else that needs a menu builds its entries
//! and gets all of this for free rather than growing a second menu beside
//! this one.
//!
//! Three deliberate limits, each matching a rule the shell already keeps:
//!
//! - **The menu never interprets its items.** An item carries a payload the
//!   call site understands; the menu filters, draws, and hands the payload
//!   back. What an item *does* is the opener's business, which is why
//!   [`MenuItem`] is generic over its action.
//! - **Filtering is the palette's matcher, not a new one.** Two fuzzy scorers
//!   in one shell would drift; [`crate::palette::score`] is the single
//!   implementation, and the menu keeps its own order rather than ranking —
//!   a menu is curated, the palette is discovered.
//! - **Chords come from `ket-core`'s parser.** A hint is a [`Chord`] the
//!   call site parsed, so what the menu draws and what the shell dispatches
//!   can never disagree about what a chord is.
//!
//! Keyboard operation, because a menu that needs the mouse is a menu a
//! keyboard-only user cannot use: arrows move, Enter runs, Escape closes,
//! typing filters — and an item's own chord runs it while the menu is open,
//! so the hint is a promise the menu keeps rather than decoration.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnchoredPositionMode, Animation, AnimationExt, AnyElement, BoxShadow, Context, Corner,
    ElementId, Image, ImageSource, KeyDownEvent, Keystroke, MouseButton, Pixels, Point, Rgba,
    ScrollHandle, SharedString, Stateful, Styled, anchored, deferred, div, ease_out_quint, img,
    point, prelude::*, px,
};
use ket_core::keybinding::Chord;
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::ui::icon::{Icon, WELL_GLYPH, WELL_MARK, icon_well, lift_well, sized_icon};

/// Height of one row.
///
/// Fixed rather than derived from the text, because a menu whose rows change
/// height with their content reads as ragged — and because an absolutely
/// positioned panel with no stated height is at the mercy of whatever its
/// container does about stretching, which is what made the first version of
/// this menu draw rows a couple of hundred pixels tall.
const ROW_HEIGHT: Pixels = px(28.0);

/// Height of a row that carries a subtitle beneath its label.
///
/// A menu that gives every one of its rows a subtitle is not the ragged menu
/// [`ROW_HEIGHT`]'s doc comment warns about — every row in it agrees on being
/// tall, so the effect is a taller menu, not an uneven one.
const TALL_ROW_HEIGHT: Pixels = px(38.0);

/// A subtitle's size, small enough to read as commentary under its label
/// rather than a second label.
const SUBTITLE_SIZE: Pixels = px(9.5);

/// Padding above the first row and below the last.
const PANEL_PAD: Pixels = px(5.0);

/// Left and right padding inside a row.
const ROW_PAD_X: Pixels = px(8.0);

/// The icon column, fixed so every label starts at the same x.
const ICON_COLUMN: Pixels = px(20.0);

/// Gap between the icon column and the label.
const ICON_GAP: Pixels = px(8.0);

/// The panel's corner radius.
///
/// Taken down from 16 when the tab strip gave up its rounded slab, and to
/// `RADIUS_LG` with Subdued: a menu rounded further than that beside a square
/// strip and a sidebar of hairline rows reads
/// as borrowed from another application.
const PANEL_RADIUS: Pixels = super::RADIUS_LG;

/// A row's corner radius. Rows are pills inset by [`PANEL_PAD`] rather than
/// bands running to the panel's edge — the inset is what makes the panel read
/// as an object floating over the window instead of a hole cut in it.
///
/// Small enough now that the highlight reads as a marked row rather than a
/// capsule sitting on one; the inset it keeps is what the comment above is
/// about, and that has not changed.
const ROW_RADIUS: Pixels = super::RADIUS_SM;

/// Label size: the style guide's menu row, 12.5 in the prose face. A shade
/// over the chrome's body size — these are the words a person is aiming a
/// mouse at.
const LABEL_SIZE: Pixels = px(12.5);

/// The keybinding hint, quieter than its label and set in the chrome's mono,
/// because a chord is something you would quote.
const HINT_SIZE: Pixels = px(10.5);

/// A section heading, and the caption a context menu puts above its rows.
const HEADING_SIZE: Pixels = px(11.0);

/// Air between an opener and the menu hanging under it.
const MENU_GAP: Pixels = px(6.0);

/// How strongly a plain glyph is drawn until the pointer is on its row.
///
/// The label is what a person reads down a menu; the icons are what they
/// recognise once they know where they are going. A plain glyph is drawn in
/// full ink and held at this, which rests it at about the dim ink the style
/// guide asks for and still lets it land the moment the row is under the
/// pointer. Provider marks, application pictures and a destructive row's red
/// are not held back: their colour is what the row carries.
const ICON_REST_OPACITY: f32 = 0.6;

/// How strongly a disabled row is drawn, all of it at once.
///
/// One fade over the whole row rather than one per part: faded separately,
/// the well's fill outweighed a glyph and a label that had each been dimmed
/// on their own. The same strength as a disabled button's label.
const DISABLED_OPACITY: f32 = 0.55;

/// How long the panel takes to arrive.
///
/// Long enough to be seen and short enough never to stand between a person
/// and the row they are already reaching for. 110ms was the first try and it
/// was over before the eye found the panel: the entrance has to outlast the
/// saccade that goes looking for it, or it may as well not run.
const ENTRANCE: Duration = Duration::from_millis(190);

/// How far the panel travels on the way in.
///
/// It rises into place rather than growing or tilting: gpui gives a div
/// opacity and layout, and no transform (`with_transformation` is `svg`-only
/// in this version), so there is no scale or perspective to be had without
/// drawing the panel as something it is not. Vertical travel alone: widening
/// the panel as it lands was tried and reads as the menu resizing rather than
/// approaching, which is a different and worse impression. Six pixels read as
/// nothing, so this is the distance that actually says *arrived from
/// somewhere*.
const ENTRANCE_RISE: Pixels = px(14.0);

/// The caption row above the items, and a section's heading — shorter than a
/// pickable row because each is a label rather than a target.
const HEADER_HEIGHT: Pixels = px(24.0);

/// How close to the window edge an anchored menu may sit.
const WINDOW_MARGIN: Pixels = px(8.0);

/// The tallest an *inline* dropdown's rows may grow before they scroll.
///
/// Short of seven plain rows, or a little under six tall ones. A dropdown
/// hangs below the control that opened it and grows in one direction only, so
/// a long one has to stop somewhere; this is where, and the sliver of the next
/// row it leaves is the cue that there is more below.
///
/// A menu opened **at a pointer** is not bounded by this at all — see
/// [`OpenMenu::view_with_footer`]. It can be placed above the pointer as
/// easily as below, so there is no direction for it to run out of, and a
/// context menu that scrolls is one whose last rows read as missing rather
/// than as below. That is not a hypothetical: the tab menu grew to eleven rows
/// and this cap fell one pixel past the end of Copy Path.
const MAX_ROWS_HEIGHT: Pixels = px(280.0);

/// Draws a dropdown's real trigger and, when open, its panel above the rest
/// of the current surface.
///
/// The returned element is the only click target the caller needs. Menu
/// panels stop mouse-down propagation themselves, so clicks on options do not
/// also toggle the trigger underneath them.
pub(crate) fn dropdown(
    id: impl Into<ElementId>,
    trigger: impl IntoElement,
    panel: Option<AnyElement>,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .child(trigger)
        .children(panel.map(|panel| deferred(panel).with_priority(1)))
}

/// One pickable row of a menu.
///
/// Generic over the action because the menu renders and filters but never
/// interprets — see the module docs for why that seam is where it is.
pub(crate) struct MenuItem<T> {
    /// What running this item does. Opaque to the menu; handed back on run.
    pub(crate) action: T,
    /// The row's label, which is also what the query filters against.
    pub(crate) title: SharedString,
    /// A second line under the label, drawn smaller and dimmer.
    ///
    /// Presence, not content, decides the row's height — see
    /// [`TALL_ROW_HEIGHT`] — so an opener giving every item a subtitle gets a
    /// uniformly taller menu rather than a ragged one.
    pub(crate) subtitle: Option<SharedString>,
    /// A leading icon, drawn dim in a fixed column so labels align.
    pub(crate) icon: Option<Icon>,
    /// An explicit tint for the leading icon.
    ///
    /// Most menu icons are quiet chrome, but a provider mark carries useful
    /// identity in its colour. Keeping that choice on the item lets selectors
    /// use the shared menu without redrawing a one-off list.
    pub(crate) icon_colour: Option<Rgba>,
    /// A picture in place of the icon: an application's own mark, read from
    /// its bundle rather than drawn by ket — see `ket_core::app_icon`. Wins
    /// over `icon` when both are set, and sits in the same column at the same
    /// size, faded at rest the same way.
    pub(crate) image: Option<Arc<Image>>,
    /// The chord that runs this item, which the row draws as its hint and
    /// the keyboard handler honours while the menu is open.
    pub(crate) chord: Option<Chord>,
    /// A short figure drawn where a chord hint would be, and its tint.
    ///
    /// For a picker whose rows carry a measurement — what a level saved, say
    /// — that reads best lined up down the right edge rather than folded into
    /// each subtitle. A chord wins when an item has both: the hint there is a
    /// promise the keyboard keeps, and a figure is only commentary.
    pub(crate) detail: Option<(SharedString, Option<Rgba>)>,
    /// Whether the item can be picked.
    ///
    /// A disabled row is drawn dim and takes neither a click, the keyboard
    /// selection, nor its own chord. Shown rather than hidden because a menu
    /// whose shape changes with context is a menu people stop being able to
    /// aim at from memory.
    pub(crate) enabled: bool,
    /// Whether picking it destroys something, which colours the label.
    pub(crate) danger: bool,
    /// Whether it leads to a submenu, which draws a trailing chevron.
    ///
    /// Only the chevron: the menu draws the promise, and opening the second
    /// level is the opener's business, the same way running an item is.
    pub(crate) submenu: bool,
    /// Whether this item is the value currently held by its opener.
    ///
    /// Keyboard focus is transient; a check is not. A selector needs to show
    /// both without making the hovered row pretend it is the saved choice.
    pub(crate) checked: bool,
}

impl<T> MenuItem<T> {
    /// A plain enabled row.
    pub(crate) fn new(action: T, title: impl Into<SharedString>) -> Self {
        Self {
            action,
            title: title.into(),
            subtitle: None,
            icon: None,
            icon_colour: None,
            image: None,
            chord: None,
            detail: None,
            enabled: true,
            danger: false,
            submenu: false,
            checked: false,
        }
    }

    /// Gives it a second line under the label.
    pub(crate) fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Gives it a leading icon.
    pub(crate) fn icon(mut self, which: Icon) -> Self {
        self.icon = Some(which);
        self
    }

    /// Gives the leading icon a meaningful colour instead of chrome's dim tint.
    pub(crate) fn tinted_icon(mut self, which: Icon, colour: Rgba) -> Self {
        self.icon = Some(which);
        self.icon_colour = Some(colour);
        self
    }

    /// Gives it a picture for a mark instead of an icon.
    pub(crate) fn image(mut self, image: Arc<Image>) -> Self {
        self.image = Some(image);
        self
    }

    /// Marks this row as the value currently selected by its opener.
    pub(crate) fn checked(mut self) -> Self {
        self.checked = true;
        self
    }

    /// Gives it a keybinding hint, which the open menu also honours.
    pub(crate) fn chord(mut self, chord: Chord) -> Self {
        self.chord = Some(chord);
        self
    }

    /// Gives it a trailing figure, tinted when `colour` is given and in the
    /// hint's quiet ink otherwise.
    pub(crate) fn detail(mut self, text: impl Into<SharedString>, colour: Option<Rgba>) -> Self {
        self.detail = Some((text.into(), colour));
        self
    }

    /// Marks it destructive.
    pub(crate) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Marks it unpickable.
    pub(crate) fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Marks it as leading somewhere.
    pub(crate) fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }
}

/// One slot in a menu's column: a pickable row, a hairline between
/// sections, or a section's name.
pub(crate) enum MenuEntry<T> {
    /// A row.
    Item(MenuItem<T>),
    /// A section boundary. Drawn only when items survive on both sides of it.
    Separator,
    /// A dim name over the items after it, when a section needs one. Starts
    /// a section of its own, and goes with it when the query empties it.
    Heading(SharedString),
}

/// One run of surviving items, and the heading it was given, if any.
struct Section<'a, T> {
    heading: Option<&'a SharedString>,
    items: Vec<&'a MenuItem<T>>,
}

/// What one key event did to an open menu.
#[derive(Debug)]
pub(crate) enum MenuKey<T> {
    /// No menu is open, or the event is not the menu's to take.
    Ignored,
    /// Handled; the caller should redraw.
    Consumed,
    /// Escape: the caller should close the menu.
    Close,
    /// An item asked to run. The caller owns what that means.
    Run(T),
}

/// An open menu: what it shows, what has been typed, and where the keyboard
/// selection sits.
pub(crate) struct OpenMenu<T> {
    /// The call site's name for itself, so an opener can tell whether the
    /// open menu is *its* menu — the `+` button this pane owns, say.
    origin: SharedString,
    /// The search row's placeholder, when the menu shows one at all.
    ///
    /// `None` draws no search row. Typing still filters — a short curated
    /// menu behaves like a native one, where type-ahead narrows the list and
    /// no box appears to explain that it did.
    placeholder: Option<SharedString>,
    /// The panel's width. Menus are short lists, so the width is chosen by
    /// the opener to fit its labels rather than measured per item.
    width: Pixels,
    /// A caption above the first row, naming what the menu acts on.
    ///
    /// Distinct from a section's [`MenuEntry::Heading`]: a context menu
    /// answers "what am I doing this to" once, and the answer is the same for
    /// every row in it.
    header: Option<SharedString>,
    /// Where to put the panel, when it belongs at the pointer.
    ///
    /// `None` hangs it below whatever relative container the opener deferred
    /// it from — the `+` button's menu. `Some` anchors it at a window
    /// position and snaps it back inside the edges, which is what a
    /// right-click menu needs.
    at: Option<Point<Pixels>>,
    /// How far below the opener an inline dropdown hangs — the gap alone, not
    /// the opener's height.
    ///
    /// `AnchoredPositionMode::Local` resolves to `natural origin + offset`,
    /// and the panel's natural origin is already *under* the trigger: both
    /// `dropdown` and the tab strip's `+` defer it as the second child of a
    /// block container, so it lays out after the trigger before anything here
    /// applies. An offset carrying the trigger's height on top of that counts
    /// the trigger twice, which is what put every menu in the shell roughly a
    /// button's height too low.
    offset: Point<Pixels>,
    /// The menu's rows in order, separators included.
    entries: Vec<MenuEntry<T>>,
    /// What has been typed into the search row.
    query: String,
    /// Index into the *visible* items — separators never occupy a selection
    /// slot, so filtering cannot leave the selection pointing at a hairline.
    selected: usize,
    /// Whether that selection is worth drawing yet.
    ///
    /// A menu opens with `selected` at zero so Enter has something to run, but
    /// painting that on arrival puts a highlight under the first row before
    /// anyone has chosen anything — the pointer is somewhere else entirely,
    /// and the row it lands on is the one the eye reads as chosen. So the
    /// highlight waits for evidence: a key that moves the selection, or an
    /// opener that deliberately started on a value (see [`Self::selected`]).
    show_selection: bool,
    /// The rows' own scroll position, kept across renders — a fresh handle
    /// every frame would forget the scroll the moment the mouse wheel moved
    /// it.
    scroll: ScrollHandle,
    /// Whether the rows run their full length rather than scrolling past
    /// [`MAX_ROWS_HEIGHT`] — see [`Self::uncapped`].
    uncapped: bool,
}

impl<T: Clone + 'static> OpenMenu<T> {
    /// A menu freshly opened: nothing typed, first item selected.
    pub(crate) fn new(
        origin: SharedString,
        placeholder: Option<SharedString>,
        width: Pixels,
        entries: Vec<MenuEntry<T>>,
    ) -> Self {
        Self {
            origin,
            placeholder,
            width,
            header: None,
            at: None,
            offset: Point {
                x: px(0.0),
                y: MENU_GAP,
            },
            entries,
            query: String::new(),
            selected: 0,
            show_selection: false,
            scroll: ScrollHandle::new(),
            uncapped: false,
        }
    }

    /// Starts keyboard navigation on a visible item chosen by the opener.
    ///
    /// A selector opens on its saved value, so Enter immediately confirms it
    /// instead of unexpectedly replacing it with the first row.
    pub(crate) fn selected(mut self, selected: usize) -> Self {
        self.selected = selected.min(self.visible_count().saturating_sub(1));
        self.show_selection = true;
        self
    }

    /// Puts a caption above the rows.
    pub(crate) fn with_header(mut self, header: impl Into<SharedString>) -> Self {
        self.header = Some(header.into());
        self
    }

    /// Lets a menu hung below its opener run to its full length instead of
    /// scrolling past [`MAX_ROWS_HEIGHT`]. For a short, curated list whose
    /// length is known — the `+` menu — where a scrolled-off last row reads
    /// as a row that is not there.
    pub(crate) fn uncapped(mut self) -> Self {
        self.uncapped = true;
        self
    }

    /// Anchors the panel at a window position rather than below its opener.
    pub(crate) fn at(mut self, at: Point<Pixels>) -> Self {
        self.at = Some(at);
        self
    }

    /// Positions an inline menu inside its opener's relative container.
    pub(crate) fn offset(mut self, left: Pixels, top: Pixels) -> Self {
        self.offset = Point { x: left, y: top };
        self
    }

    /// Whether this menu was opened by `origin` — the opener's name for
    /// itself, matched as a plain string because the menu is generic over
    /// its action and cannot know what an opener's identity looks like.
    pub(crate) fn opened_by(&self, origin: &str) -> bool {
        self.origin.as_ref() == origin
    }

    /// The sections the current query leaves behind: runs of matching items
    /// in entry order, with separators collapsed wherever a side of them
    /// emptied out.
    ///
    /// Order is kept rather than ranked — a menu is curated, and its order
    /// is part of what its opener decided. Public to the crate so an opener
    /// can assert what its menu offers, which is the only reading anyone
    /// outside this module has ever needed.
    pub(crate) fn sections(&self) -> Vec<Vec<&MenuItem<T>>> {
        self.titled_sections()
            .into_iter()
            .map(|section| section.items)
            .collect()
    }

    /// [`Self::sections`], each with the heading it was given. A heading
    /// whose items all filtered out goes with them.
    fn titled_sections(&self) -> Vec<Section<'_, T>> {
        let mut sections = Vec::new();
        let mut current = Section {
            heading: None,
            items: Vec::new(),
        };

        for entry in &self.entries {
            match entry {
                MenuEntry::Separator | MenuEntry::Heading(_) => {
                    let heading = match entry {
                        MenuEntry::Heading(heading) => Some(heading),
                        _ => None,
                    };
                    let done = std::mem::replace(
                        &mut current,
                        Section {
                            heading,
                            items: Vec::new(),
                        },
                    );
                    if !done.items.is_empty() {
                        sections.push(done);
                    }
                }
                MenuEntry::Item(item) => {
                    if crate::palette::score(&self.query, &item.title).is_some() {
                        current.items.push(item);
                    }
                }
            }
        }
        if !current.items.is_empty() {
            sections.push(current);
        }

        sections
    }

    /// How many items the query leaves pickable.
    fn visible_count(&self) -> usize {
        self.sections().iter().map(Vec::len).sum()
    }

    /// Where the top of one row sits, measured from the panel's own top edge.
    ///
    /// For an opener hanging a second level off one of its rows: a submenu has
    /// to line up with the row that summoned it, and the only thing that knows
    /// how tall this menu's chrome and rows are is this menu. Deliberately next
    /// to [`Self::view`] — these are the same numbers, and they have to move
    /// together.
    ///
    /// Measured rather than reported by the layout because gpui hands element
    /// bounds back during paint, which is a frame too late for a menu that has
    /// to be positioned as it opens.
    ///
    /// A row past the end of the list answers with the panel's top, which is
    /// where a menu with nothing in it would want its second level anyway.
    pub(crate) fn row_top(&self, wanted: usize) -> Pixels {
        let mut top = PANEL_PAD;
        if self.header.is_some() {
            top += HEADER_HEIGHT;
        }
        if self.placeholder.is_some() {
            top += ROW_HEIGHT;
        }

        let mut index = 0usize;
        for (section_index, section) in self.titled_sections().iter().enumerate() {
            if section_index > 0 {
                // The hairline and the air either side of it: `h(1)` with
                // `my(4)` in `view`.
                top += px(9.0);
            }
            if section.heading.is_some() {
                top += HEADER_HEIGHT;
            }
            for item in &section.items {
                if index == wanted {
                    return top;
                }
                top += if item.subtitle.is_some() {
                    TALL_ROW_HEIGHT
                } else {
                    ROW_HEIGHT
                };
                index += 1;
            }
        }
        PANEL_PAD
    }

    /// The action under the keyboard selection, if the query left one.
    ///
    /// A disabled row still occupies a selection slot — it is visible, so
    /// skipping it would make the arrow keys jump in a way the eye cannot
    /// explain — but picking it does nothing.
    fn selected_action(&self) -> Option<T> {
        self.sections()
            .iter()
            .flatten()
            .nth(self.selected)
            .filter(|item| item.enabled)
            .map(|item| item.action.clone())
    }

    /// Handles one key event while open.
    ///
    /// A modified keystroke that matches a visible item's chord runs that
    /// item — the hint on the row is a promise the menu honours, not
    /// decoration. Modified keystrokes that match nothing are consumed all
    /// the same: the menu is the topmost thing on screen and a stray chord
    /// falling through to the shell underneath it is how a menu comes to
    /// drive two things at once.
    pub(crate) fn key(&mut self, event: &KeyDownEvent) -> MenuKey<T> {
        let keystroke = &event.keystroke;

        match keystroke.key.as_str() {
            "escape" => return MenuKey::Close,
            "enter" => {
                return match self.selected_action() {
                    Some(action) => MenuKey::Run(action),
                    None => MenuKey::Consumed,
                };
            }
            "down" => {
                let last = self.visible_count().saturating_sub(1);
                self.selected = (self.selected + 1).min(last);
                self.show_selection = true;
                return MenuKey::Consumed;
            }
            "up" => {
                self.selected = self.selected.saturating_sub(1);
                self.show_selection = true;
                return MenuKey::Consumed;
            }
            "backspace" => {
                self.query.pop();
                self.selected = 0;
                self.show_selection = true;
                return MenuKey::Consumed;
            }
            _ => {}
        }

        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            for item in self.sections().iter().flatten() {
                if item.enabled
                    && let Some(chord) = &item.chord
                    && chord_matches(chord, keystroke)
                {
                    return MenuKey::Run(item.action.clone());
                }
            }
            return MenuKey::Consumed;
        }

        // `key_char` rather than `key`: the former is what was actually
        // typed, which is the difference between a US layout and every other
        // one — the same rule the palette follows.
        if let Some(typed) = keystroke.key_char.as_deref()
            && !typed.is_empty()
        {
            self.query.push_str(typed);
            // Filtering makes the first match the thing Enter runs, so it has
            // to be visible from here on.
            self.selected = 0;
            self.show_selection = true;
            return MenuKey::Consumed;
        }

        MenuKey::Ignored
    }

    /// The panel: search row, then the surviving sections, positioned below
    /// whatever relative container the opener deferred it from.
    ///
    /// `on_run` is the seam where the menu hands a picked action back — the
    /// one thing the menu cannot do itself, because it never knew what the
    /// action meant.
    ///
    /// `on_dismiss` closes this same menu. It fires for a click that lands on
    /// the panel's own chrome rather than a row — the gap a sibling control
    /// occluded when this menu opened over it, so the click that missed an
    /// item still reads as "away" rather than doing nothing.
    pub(crate) fn view(
        &self,
        theme: &Theme,
        cx: &mut Context<Shell>,
        on_run: impl Fn(&mut Shell, &T, &mut Context<Shell>) + Clone + 'static,
        on_dismiss: impl Fn(&mut Shell, &mut Context<Shell>) + Clone + 'static,
    ) -> AnyElement {
        self.view_with_hover(theme, cx, on_run, on_dismiss, |_, _, _| {})
    }

    /// [`OpenMenu::view`], plus a word each time the pointer comes to rest on
    /// a row.
    ///
    /// For an opener with a branch to hang off one of its rows: a native
    /// submenu opens when the pointer reaches its row and closes when the
    /// pointer moves on to a sibling, and neither of those is a click. Called
    /// on entering a row only — leaving one is entering the next, and the
    /// panel as a whole has no useful "left" — and never for a disabled row,
    /// which the pointer crosses as if it were chrome. Whether an entry is
    /// worth a redraw is the opener's call, made inside `on_hover`.
    pub(crate) fn view_with_hover(
        &self,
        theme: &Theme,
        cx: &mut Context<Shell>,
        on_run: impl Fn(&mut Shell, &T, &mut Context<Shell>) + Clone + 'static,
        on_dismiss: impl Fn(&mut Shell, &mut Context<Shell>) + Clone + 'static,
        on_hover: impl Fn(&mut Shell, &T, &mut Context<Shell>) + Clone + 'static,
    ) -> AnyElement {
        let t = theme;
        let selected = self.selected;
        let show_selection = self.show_selection;

        // Only when the opener asked for one. See `placeholder`.
        let query_line = self.placeholder.clone().map(|placeholder| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(ROW_HEIGHT)
                .px(ROW_PAD_X)
                .text_size(LABEL_SIZE)
                .text_color(paint(if self.query.is_empty() {
                    t.text.dim
                } else {
                    t.text.primary
                }))
                .child(if self.query.is_empty() {
                    placeholder
                } else {
                    SharedString::from(self.query.clone())
                })
        });

        let sections = self.titled_sections();
        let mut rows = Vec::new();
        let mut index = 0usize;

        for (section_index, section) in sections.iter().enumerate() {
            if section_index > 0 {
                rows.push(
                    div()
                        .flex_none()
                        .h(px(1.0))
                        .my(px(4.0))
                        .mx(px(8.0))
                        // The selected fill, not the card's rule: on the
                        // float's lighter ground the rule all but vanishes.
                        .bg(paint(t.selection))
                        .into_any_element(),
                );
            }
            if let Some(heading) = section.heading {
                rows.push(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(HEADER_HEIGHT)
                        .px(ROW_PAD_X)
                        .text_size(HEADING_SIZE)
                        .text_color(paint(t.text.dim))
                        .child(heading.clone())
                        .into_any_element(),
                );
            }
            for item in &section.items {
                let action = item.action.clone();
                let on_run = on_run.clone();
                let hover_action = item.action.clone();
                let on_hover = on_hover.clone();
                let hint = item
                    .chord
                    .as_ref()
                    .map(|chord| (glyphs(chord), None))
                    .or_else(|| item.detail.clone());
                let row = index;
                index += 1;

                // Disabled wins over danger: a destructive row that cannot be
                // picked should not be shouting in red about it.
                let label_colour = if !item.enabled {
                    t.text.dim
                } else if item.danger {
                    t.status.failed
                } else {
                    t.text.primary
                };
                let enabled = item.enabled;
                let submenu = item.submenu;
                let checked = item.checked;
                let subtitle = item.subtitle.clone();
                let row_height = if subtitle.is_some() {
                    TALL_ROW_HEIGHT
                } else {
                    ROW_HEIGHT
                };

                let label = if let Some(subtitle) = subtitle {
                    div()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_color(paint(label_colour))
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(SUBTITLE_SIZE)
                                .text_color(paint(t.text.dim))
                                .child(subtitle),
                        )
                } else {
                    div()
                        .text_color(paint(label_colour))
                        .child(item.title.clone())
                };

                let row_group = SharedString::from(format!("menu-row-{row}"));
                let lit = row == selected && enabled && show_selection;

                // A plain glyph is drawn in full ink and faded at rest;
                // opacity rather than a second colour because `icon` bakes
                // its tint into the SVG mask when the element is built (see
                // `ui::icon`), so there is nothing for a hover rule to
                // recolour — but the wrapper it sits in can simply be faded.
                // A mark or a picture keeps its colour: see
                // `ICON_REST_OPACITY`.
                // A destructive row's glyph takes its label's red at full
                // strength, and in a well the well takes a wash of it.
                // A disabled row drops any tint, draws its glyph in its
                // label's ink, and is faded as a whole (`DISABLED_OPACITY`);
                // neither the glyph nor its well answers the pointer.
                let danger = item.danger && item.enabled;
                let icon_colour = item.icon_colour.filter(|_| enabled);
                let glyph_only = item.image.is_none() && item.icon_colour.is_none();
                let plain = glyph_only && !danger;
                let glyph_size = if glyph_only { WELL_GLYPH } else { WELL_MARK };
                let glyph = match (&item.image, item.icon) {
                    (Some(image), _) => Some(
                        img(ImageSource::Image(image.clone()))
                            .size(glyph_size)
                            .flex_none()
                            .into_any_element(),
                    ),
                    (None, Some(which)) => Some(
                        sized_icon(
                            which,
                            glyph_size,
                            icon_colour.unwrap_or_else(|| {
                                paint(if !enabled {
                                    t.text.dim
                                } else if danger {
                                    t.status.failed
                                } else {
                                    t.text.primary
                                })
                            }),
                        )
                        .into_any_element(),
                    ),
                    (None, None) => None,
                };
                let has_glyph = glyph.is_some();
                let glyph = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .when(enabled && plain && !lit, |el| {
                        el.opacity(ICON_REST_OPACITY)
                            .group_hover(row_group.clone(), |style| style.opacity(1.0))
                    })
                    .children(glyph);
                // The well lifts with its row when it is a plain one; a
                // tinted one keeps its colour.
                let tint = icon_colour.or_else(|| danger.then(|| paint(t.status.failed)));

                rows.push(
                    div()
                        .id(("menu-row", row))
                        .group(row_group.clone())
                        .flex()
                        // Every row states its height. Without this the panel
                        // is laid out by whatever its container believes about
                        // stretching, and a three-item menu can fill a window.
                        .flex_none()
                        .h(row_height)
                        .items_center()
                        .gap(ICON_GAP)
                        .px(ROW_PAD_X)
                        .rounded(ROW_RADIUS)
                        .text_size(LABEL_SIZE)
                        // Hover and the keyboard selection share one fill, so
                        // the pointer and the arrows never disagree about
                        // which row is lit.
                        .when(enabled, |el| {
                            el.cursor_pointer()
                                .hover(|style| style.bg(paint(t.selection)))
                        })
                        .when(lit, |el| el.bg(paint(t.selection)))
                        .when(!enabled, |el| el.opacity(DISABLED_OPACITY))
                        // Stops the mouse-down here rather than letting it
                        // reach the panel's own handler below, which reads a
                        // click that never got this far as a click on empty
                        // chrome and dismisses the menu before the row's
                        // `on_click` gets a mouse-up to pair it with.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(if has_glyph {
                            icon_well(tint, lit, t)
                                .when(enabled && tint.is_none(), |el| {
                                    el.group_hover(row_group.clone(), |style| lift_well(style, t))
                                })
                                .child(glyph)
                        } else {
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .w(ICON_COLUMN)
                                .child(glyph)
                        })
                        .child(label)
                        .child(div().flex_grow())
                        .children(hint.map(|(hint, colour)| {
                            div()
                                .flex_none()
                                .font_family(crate::fonts::chrome())
                                .text_size(HINT_SIZE)
                                .text_color(colour.unwrap_or_else(|| paint(t.text.dim)))
                                .child(hint)
                        }))
                        .when(checked, |el| {
                            el.child(div().flex_none().child(sized_icon(
                                Icon::Check,
                                px(14.0),
                                paint(t.accent),
                            )))
                        })
                        // The chevron says there is another level. Drawing it
                        // without wiring one would be a lie, so an opener that
                        // sets `submenu` owes the reader that second level.
                        .when(submenu, |el| {
                            el.child(div().flex_none().child(sized_icon(
                                Icon::ChevronRight,
                                px(14.0),
                                paint(t.text.dim),
                            )))
                        })
                        // No handler at all when disabled, rather than a
                        // handler that returns early: a row that cannot be
                        // picked should not swallow the click either.
                        .when(enabled, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                on_run(this, &action, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }))
                        })
                        .when(enabled, |el| {
                            el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered {
                                    on_hover(this, &hover_action, cx);
                                }
                            }))
                        })
                        .into_any_element(),
                );
            }
        }

        if index == 0 {
            rows.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(ROW_HEIGHT)
                    .px(ROW_PAD_X)
                    .text_size(LABEL_SIZE)
                    .text_color(paint(t.text.dim))
                    .child("No matches")
                    .into_any_element(),
            );
        }

        // Names what the menu acts on, above everything else in it.
        let header = self.header.clone().map(|header| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(HEADER_HEIGHT)
                .px(ROW_PAD_X)
                .text_size(HEADING_SIZE)
                .text_color(paint(t.text.dim))
                .child(header)
        });

        let panel = div()
            .id("menu")
            .prose()
            // A column that hugs its rows, stated rather than inherited.
            .flex()
            .flex_col()
            .flex_none()
            .w(self.width)
            .p(PANEL_PAD)
            .overflow_hidden()
            .rounded(PANEL_RADIUS)
            .border_1()
            .border_color(paint(t.border))
            // The float ground, a step lighter than the cards it hangs over:
            // the style guide's surface for everything that floats. The
            // border and the two-layer shadow below hold its edge on the
            // dark desk.
            .bg(paint(t.elevated))
            .shadow(vec![
                // The cast: long, soft, and well clear of the panel.
                BoxShadow {
                    color: crate::paint::shadow(0.55, t),
                    offset: point(px(0.0), px(12.0)),
                    blur_radius: px(32.0),
                    spread_radius: px(0.0),
                },
                // A tight one under the edge, so the panel keeps a definite
                // bottom instead of dissolving into its own cast shadow.
                BoxShadow {
                    color: crate::paint::shadow(0.35, t),
                    offset: point(px(0.0), px(2.0)),
                    blur_radius: px(6.0),
                    spread_radius: px(0.0),
                },
            ])
            // Nothing behind an open menu reacts to a click on it.
            .occlude()
            .children(header)
            .children(query_line)
            .child(
                // `min_h_0` and `max_h` together are what let this shrink to
                // its cap instead of the flex child refusing to shrink below
                // its content — see `crate::ui::picker::results`, which the
                // same pair was copied from.
                div()
                    .id("menu-rows")
                    .flex()
                    .flex_col()
                    .min_h_0()
                    // Only where the menu has one direction to grow in. See
                    // `MAX_ROWS_HEIGHT`, and the placement at the end of this
                    // function.
                    .when(self.at.is_none() && !self.uncapped, |el| {
                        el.max_h(MAX_ROWS_HEIGHT)
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                    })
                    .children(rows),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    on_dismiss(this, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            // The entrance. A menu's panel is only built while it is open, so
            // it mounts fresh every time and gets a new clock for the fade
            // without anything here having to reset one — the same property
            // `ui::tooltip` leans on. Quint ease-out so almost all of the
            // travel is over in the first few frames: the panel should be
            // *there* by the time the eye arrives, with the tail of the
            // animation doing nothing but settling.
            .with_animation(
                SharedString::from(format!("menu-entrance-{}", self.origin)),
                Animation::new(ENTRANCE).with_easing(ease_out_quint()),
                move |el, delta| el.opacity(delta).mt(ENTRANCE_RISE * (1.0 - delta)),
            );

        match self.at {
            // At the pointer, and above it when there is no room below.
            //
            // `anchored()`'s own default — switch which corner the panel hangs
            // from, rather than sliding it back inside the window. Sliding is
            // what a panel that must keep its position does; a context menu has
            // no position to keep beyond "next to what was clicked", and going
            // up from the pointer is what every menu on this platform does. It
            // is also why these are not capped: flipping means a menu near the
            // bottom of the window shows every row instead of scrolling the
            // last of them out of sight.
            Some(at) => anchored()
                .position(at)
                .anchor(Corner::TopLeft)
                .child(panel)
                .into_any_element(),
            // Below its opener inside the relative container it deferred
            // from — but still snapped to the window, so a field near the
            // bottom of a tall panel does not open a menu that runs off the
            // screen with no way to reach the rows past the edge.
            None => anchored()
                .position_mode(AnchoredPositionMode::Local)
                .position(self.offset)
                .anchor(Corner::TopLeft)
                .snap_to_window_with_margin(WINDOW_MARGIN)
                .child(panel)
                .into_any_element(),
        }
    }
}

/// Whether an event is exactly this single-keystroke chord.
///
/// Multi-keystroke chords are never matched while a menu is open: they are
/// sequences, and honouring them means tracking a half-typed prefix against
/// the search row eating the first keystroke as text. Hints still draw them;
/// only the in-menu shortcut is single-keystroke.
fn chord_matches(chord: &Chord, keystroke: &Keystroke) -> bool {
    let [stroke] = chord.keystrokes() else {
        return false;
    };
    let m = &keystroke.modifiers;
    stroke.ctrl == m.control
        && stroke.alt == m.alt
        && stroke.shift == m.shift
        && stroke.cmd == m.platform
        && stroke.key == keystroke.key.to_lowercase()
}

/// A chord drawn the way macOS draws it: Apple's modifier order, then the
/// key, uppercased when it is a single letter — `ctrl+shift+b` becomes
/// `⇧⌃B`, which is what a person reading a menu row expects to see.
#[cfg(target_os = "macos")]
/// The symbol macOS prints for a named key, or the name itself.
///
/// A hint reading `⌘⇧⌫` is one a person can match against the key caps in
/// front of them; `cmd+shift+backspace` is one they have to translate. Only
/// the keys that have a conventional symbol are mapped — inventing one for a
/// key that has none would be worse than spelling it out.
fn key_symbol(key: &str) -> &str {
    match key {
        "backspace" => "⌫",
        "delete" => "⌦",
        "enter" => "⏎",
        "tab" => "⇥",
        "escape" | "esc" => "⎋",
        "space" => "␣",
        "up" => "↑",
        "down" => "↓",
        "left" => "←",
        "right" => "→",
        "home" => "↖",
        "end" => "↘",
        "pageup" => "⇞",
        "pagedown" => "⇟",
        other => other,
    }
}

pub(crate) fn glyphs(chord: &Chord) -> SharedString {
    let mut out = String::new();
    for stroke in chord.keystrokes() {
        if !out.is_empty() {
            out.push(' ');
        }
        if stroke.shift {
            out.push('⇧');
        }
        if stroke.ctrl {
            out.push('⌃');
        }
        if stroke.alt {
            out.push('⌥');
        }
        if stroke.cmd {
            out.push('⌘');
        }
        if stroke.key.chars().count() == 1 {
            out.push_str(&stroke.key.to_uppercase());
        } else {
            out.push_str(key_symbol(&stroke.key));
        }
    }
    out.into()
}

/// A chord drawn as its canonical text, which is how the other platforms
/// already write bindings.
#[cfg(not(target_os = "macos"))]
pub(crate) fn glyphs(chord: &Chord) -> SharedString {
    chord.to_string().into()
}

impl Shell {
    /// Routes one key event to the open menu, if any.
    ///
    /// Returns whether the menu took it. Runs picked actions through the
    /// opener's own handler rather than a dispatch of the menu's devising —
    /// the menu is generic over its action and has nowhere to send one.
    pub(crate) fn menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        let outcome = menu.key(event);

        match outcome {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.menu = None,
            MenuKey::Run(action) => self.run_menu_action(action, cx),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    /// A menu for the filtering and keyboard tests: two file-ish items, a
    /// section break, one terminal-ish item with a chord.
    fn sample() -> OpenMenu<&'static str> {
        OpenMenu::new(
            "test".into(),
            Some("Search…".into()),
            px(264.0),
            vec![
                MenuEntry::Item(MenuItem::new("file", "New File…").icon(Icon::File)),
                MenuEntry::Item(MenuItem::new("folder", "Open Folder").icon(Icon::Folder)),
                MenuEntry::Separator,
                MenuEntry::Item(
                    MenuItem::new("terminal", "New Terminal")
                        .icon(Icon::Terminal)
                        .chord(Chord::parse("ctrl+`").unwrap()),
                ),
            ],
        )
    }

    fn key_down(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers,
                key: key.to_owned(),
                key_char: key_char.map(ToOwned::to_owned),
            },
            is_held: false,
        }
    }

    fn ctrl(key: &str) -> KeyDownEvent {
        key_down(
            key,
            None,
            Modifiers {
                control: true,
                ..Default::default()
            },
        )
    }

    fn typed(text: &str) -> KeyDownEvent {
        key_down(
            text,
            Some(text),
            Modifiers {
                shift: text.chars().all(|c| c.is_uppercase()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn an_empty_query_leaves_every_section() {
        let menu = sample();

        let sections = menu.sections();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].len(), 2);
        assert_eq!(sections[1].len(), 1);
    }

    #[test]
    fn a_query_that_matches_one_section_collapses_the_separator() {
        let mut menu = sample();
        menu.key(&typed("t"));
        menu.key(&typed("e"));
        menu.key(&typed("r"));
        menu.key(&typed("m"));

        let sections = menu.sections();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].len(), 1);
        assert_eq!(sections[0][0].action, "terminal");
    }

    #[test]
    fn a_query_that_matches_nothing_leaves_nothing_pickable() {
        let mut menu = sample();
        menu.key(&typed("z"));
        menu.key(&typed("z"));

        assert!(menu.sections().is_empty());
        assert_eq!(menu.visible_count(), 0);
        assert_eq!(menu.selected_action(), None);
    }

    #[test]
    fn enter_runs_the_selected_item() {
        let mut menu = sample();

        match menu.key(&key_down("enter", None, Modifiers::default())) {
            MenuKey::Run(action) => assert_eq!(action, "file"),
            other => panic!("enter did not run the first item: {other:?}"),
        }
    }

    #[test]
    fn arrows_walk_the_visible_items_in_order() {
        let mut menu = sample();
        menu.key(&key_down("down", None, Modifiers::default()));
        menu.key(&key_down("down", None, Modifiers::default()));

        match menu.key(&key_down("enter", None, Modifiers::default())) {
            MenuKey::Run(action) => assert_eq!(action, "terminal"),
            other => panic!("selection did not reach the third item: {other:?}"),
        }

        menu.key(&key_down("up", None, Modifiers::default()));
        match menu.key(&key_down("enter", None, Modifiers::default())) {
            MenuKey::Run(action) => assert_eq!(action, "folder"),
            other => panic!("up did not step back: {other:?}"),
        }
    }

    #[test]
    fn arrows_clamp_at_both_ends() {
        let mut menu = sample();
        for _ in 0..5 {
            menu.key(&key_down("up", None, Modifiers::default()));
        }
        match menu.key(&key_down("enter", None, Modifiers::default())) {
            MenuKey::Run(action) => assert_eq!(action, "file"),
            other => panic!("up fell off the top: {other:?}"),
        }

        let mut menu = sample();
        for _ in 0..5 {
            menu.key(&key_down("down", None, Modifiers::default()));
        }
        match menu.key(&key_down("enter", None, Modifiers::default())) {
            MenuKey::Run(action) => assert_eq!(action, "terminal"),
            other => panic!("down fell off the bottom: {other:?}"),
        }
    }

    #[test]
    fn typing_resets_the_selection_to_the_first_match() {
        let mut menu = sample();
        menu.key(&key_down("down", None, Modifiers::default()));
        menu.key(&typed("f"));

        match menu.key(&key_down("enter", None, Modifiers::default())) {
            MenuKey::Run(action) => assert_eq!(action, "file"),
            other => panic!("typing did not reset the selection: {other:?}"),
        }
    }

    #[test]
    fn backspace_eats_the_query_one_character_at_a_time() {
        let mut menu = sample();
        menu.key(&typed("c"));
        menu.key(&typed("h"));
        assert_eq!(menu.query, "ch");

        menu.key(&key_down("backspace", None, Modifiers::default()));
        assert_eq!(menu.query, "c");

        menu.key(&key_down("backspace", None, Modifiers::default()));
        assert_eq!(menu.query, "");
        assert_eq!(menu.visible_count(), 3);
    }

    #[test]
    fn escape_closes() {
        let mut menu = sample();
        assert!(matches!(
            menu.key(&key_down("escape", None, Modifiers::default())),
            MenuKey::Close
        ));
    }

    #[test]
    fn an_items_chord_runs_it_while_the_menu_is_open() {
        let mut menu = sample();

        match menu.key(&ctrl("`")) {
            MenuKey::Run(action) => assert_eq!(action, "terminal"),
            other => panic!("ctrl-backtick did not run the terminal item: {other:?}"),
        }
    }

    #[test]
    fn a_modified_key_that_matches_no_item_is_consumed_rather_than_passed_through() {
        let mut menu = sample();

        assert!(matches!(menu.key(&ctrl("x")), MenuKey::Consumed));
    }

    #[test]
    fn an_unmodified_key_the_search_row_cannot_take_is_ignored() {
        let mut menu = sample();

        // `key_char` is what the search row types; an event without one and
        // without modifiers is not text and not a command — a raw `tab`, say.
        assert!(matches!(
            menu.key(&key_down("tab", None, Modifiers::default())),
            MenuKey::Ignored
        ));
    }

    #[test]
    fn chords_draw_in_the_platforms_own_script() {
        let plain = Chord::parse("ctrl+`").unwrap();
        let modified = Chord::parse("cmd+shift+t").unwrap();
        let sequence = Chord::parse("ctrl+k ctrl+p").unwrap();

        #[cfg(target_os = "macos")]
        {
            assert_eq!(glyphs(&plain).as_ref(), "⌃`");
            assert_eq!(glyphs(&modified).as_ref(), "⇧⌘T");
            assert_eq!(glyphs(&sequence).as_ref(), "⌃K ⌃P");
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(glyphs(&plain).as_ref(), "ctrl+`");
            assert_eq!(glyphs(&modified).as_ref(), "shift+cmd+t");
            assert_eq!(glyphs(&sequence).as_ref(), "ctrl+k ctrl+p");
        }
    }

    #[test]
    fn a_menu_identifies_its_opener() {
        let menu = sample();

        assert!(menu.opened_by("test"));
        assert!(!menu.opened_by("other"));
    }
}
