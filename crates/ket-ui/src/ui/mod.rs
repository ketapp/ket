//! The shell's component vocabulary.
//!
//! Before this module every control was built where it was used. That gave six
//! buttons no two of which agreed on height, padding, radius or whether they
//! had a border; five icon buttons, two of which had no hover state and no
//! pointer cursor; three text fields; four dialog scaffolds split across two
//! contradictory houses; and two copies of the same overlay picker. Across
//! 43 click handlers only 18 changed the cursor and only 9 reacted to hover —
//! which is most of what made the window feel unfinished, and none of which is
//! fixable at one call site at a time.
//!
//! So the recipes live here, once. `menu` was already built this way and is
//! the pattern the rest follow: named metrics at the top of the module, one
//! constructor, callers supplying only content and behaviour.
//!
//! Nothing here holds state or reads the workspace. A component takes a
//! `&Theme` and returns an element the caller attaches handlers to.

pub(crate) mod agent;
pub(crate) mod banner;
pub(crate) mod button;
pub(crate) mod chip;
pub(crate) mod cluster;
pub(crate) mod dialog;
pub(crate) mod field;
pub(crate) mod filetype;
pub(crate) mod group;
pub(crate) mod icon;
pub(crate) mod markdown;
pub(crate) mod menu;
pub(crate) mod picker;
pub(crate) mod popup;
pub(crate) mod progress;
pub(crate) mod qr;
pub(crate) mod row;
pub(crate) mod select;
pub(crate) mod textarea;
pub(crate) mod toast;
pub(crate) mod toggle;
pub(crate) mod tooltip;

use gpui::{Pixels, px};

// ---- metrics ---------------------------------------------------------------
//
// One scale, stated once. These are absolute pixels rather than `rem`s
// deliberately: `gpui`'s `px_3`/`text_sm` helpers are relative to the window's
// rem size, which `theme.ui_font_size` moves, and a control whose height
// scales while its icon does not is how the old chrome ended up with a 34px
// menu row holding a 20px icon column and a 12px hint.

/// A button, and anything the eye reads as one.
pub(crate) const CONTROL_H: Pixels = px(32.0);

/// A text field, which is taller than a button because it is aimed at with a
/// caret rather than a click.
pub(crate) const FIELD_H: Pixels = px(36.0);

/// A row in a list: the sidebar, a menu, a picker.
///
/// Rows are rounded fills with no hairline between them, so this is the
/// whole of a row's height — see `ui::row`.
pub(crate) const ROW_H: Pixels = px(30.0);

/// A square icon button.
pub(crate) const WELL: Pixels = px(32.0);

/// A square icon button in a dense strip.
pub(crate) const WELL_SM: Pixels = px(28.0);

/// A square icon button in a cluster of them, where the run of buttons has to
/// read as one control rather than three — the project heading's actions. Four
/// pixels of air around a 16px glyph is the least a button can carry and still
/// be aimed at.
pub(crate) const WELL_XS: Pixels = px(24.0);

/// Chips, badges, and the inside of a well: a segment in a switch, a tab in
/// its track.
///
/// Soft rather than square. The window is cards on a desk now, and a control
/// sitting in a 12px card with a hairline-square corner reads as a piece of
/// an older window pasted in.
pub(crate) const RADIUS_SM: Pixels = px(5.0);

/// Buttons, fields and the tracks of a switch.
pub(crate) const RADIUS_MD: Pixels = px(7.0);

/// Panels that float: menus, dialogs, the picker.
pub(crate) const RADIUS_LG: Pixels = px(10.0);

/// An icon button's corner. The same as [`RADIUS_MD`]: the name is kept so
/// the call sites still say which of the two things they are.
pub(crate) const RADIUS_WELL: Pixels = RADIUS_MD;

/// Every icon in the chrome.
pub(crate) const ICON: Pixels = px(16.0);

/// A control's label, and a menu row's.
///
/// A step down from the 13.5 the chrome took in the sans: a mono sets
/// wider than a proportional face at the same size, and the step is what
/// keeps a branch name like `test-token-reduction` on one line in a 300px
/// sidebar.
pub(crate) const LABEL: Pixels = px(12.5);

/// Secondary text: field labels, hints, the status bar.
pub(crate) const CAPTION: Pixels = px(11.0);

/// What a text field's contents are typed at — its value and, when it is
/// empty, its placeholder.
///
/// One constant for the sidebar's query line and every boxed field, so the
/// explorer's filter cannot drift a size and a half above the search field
/// beside it, which it did while the box took [`LABEL`] and the line took a
/// number of its own. Stated rather than inherited, which is what the sidebar
/// field once did and what nothing else in the chrome does: an unstated size
/// resolves against the window's rem, which `theme.ui_font_size` moves, so
/// the one field grew and shrank while the rows under it held still.
pub(crate) const FIELD_TEXT: Pixels = px(13.0);

/// A dialog's title.
pub(crate) const TITLE: Pixels = px(15.0);

/// Horizontal padding inside a row or a field.
pub(crate) const PAD_X: Pixels = px(10.0);

/// Padding between a dialog's edge and its controls.
///
/// Named because a field with a caret in it has to know how much room its
/// text has before the row is laid out — see `input::TextInput::render`.
pub(crate) const CARD_PAD: Pixels = px(18.0);

/// The gap between an icon and the label it belongs to.
pub(crate) const ICON_GAP: Pixels = px(8.0);
