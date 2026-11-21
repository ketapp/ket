//! Both project dialogs' left column: how the project looks.
//!
//! The badge as the sidebar will show it, then the name, the icon, the
//! picture that can stand in for the icon, and the colour.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Div, EntityId, PathPromptOptions, Pixels, SharedString, Window, div,
    prelude::*, px,
};
use ket_core::project_image::ImageFormat;
use ket_core::theme::Color;

use super::{CONTROL_W, ITEM_GAP, Tip, stacked};
use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::projects::badge::{SWATCH_GAP, badge_initial, colour_name, glyph_words};
use crate::projects::default_color;
use crate::projects::form::{Field, ImageDraft, ProjectForm};
use crate::ui::button::icon_button;
use crate::ui::chip::image_badge;
use crate::ui::field::field;
use crate::ui::glyph::Glyph;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::popup::{Placement, anchored};
use crate::ui::rule::rule;
use crate::ui::section::heading;
use crate::ui::select::select;
use crate::ui::{ICON, ICON_GAP, LABEL};

/// The icon panel's padding, and the gap between its parts.
const PANEL_PAD: Pixels = px(8.0);

/// How much of the icon's item is left while a picture is shown in its
/// place: there, and still working, but plainly not what the badge wears.
const SET_ASIDE: f32 = 0.45;

/// The image field's inset after its remove button, which sits closer to
/// the edge than text does so the button's square is centred in the field's
/// height.
const IMAGE_FIELD_END: Pixels = px(4.0);

/// The picked file's name, in the mono, a step under the field's text.
const IMAGE_NAME: Pixels = px(12.0);

impl Shell {
    /// The left column: the badge as the sidebar will show it, then the
    /// name, the icon, the picture and the colour that make it.
    pub(super) fn appearance(
        &self,
        form: &ProjectForm,
        root: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = &self.theme;
        let shown = form.tip;
        let colour = form.colour(cx);
        let image = form.image_shown();

        let preview = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(image_badge(
                image.clone(),
                colour.unwrap_or_else(default_color),
                badge_initial(form, cx),
                t,
            ))
            .child(heading(
                "Appearance",
                "How this project shows up in the sidebar.",
                t,
            ));
        let name = stacked(
            self.item_label("Name", Tip::Name, shown, cx),
            self.form_field(form, Field::Name, false, None, window, cx),
        );
        // Dimmed rather than taken away: the icon is what the badge wears
        // again once the picture is removed. The panel it opens is drawn in
        // a later pass, which the dimming does not reach.
        let icon_tip = if image.is_some() {
            Tip::IconSetAside
        } else {
            Tip::Icon
        };
        let icon = stacked(
            self.item_label("Icon", icon_tip, shown, cx),
            self.icon_select(form, window, cx),
        )
        .when(image.is_some(), |el| el.opacity(SET_ASIDE));
        let picture = stacked(
            self.item_label("Image", Tip::Image, shown, cx),
            self.image_field(form, root, image, colour, cx),
        );
        let named = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .mt(px(2.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(LABEL)
                    .text_color(paint(t.text.primary))
                    .child(colour_name(colour)),
            )
            .child(self.hex_field(form, colour, window, cx));
        let swatches = div()
            .flex()
            .flex_col()
            .gap(SWATCH_GAP)
            .children(self.swatch_rows(colour, cx));
        let colour =
            stacked(self.item_label("Colour", Tip::Colour, shown, cx), swatches).child(named);

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(CONTROL_W)
            .gap(ITEM_GAP)
            .child(preview)
            .child(name)
            .child(icon)
            .child(picture)
            .child(colour)
    }

    /// The image's field: "Choose an image…" with none, else the picture,
    /// its file's name and the button that takes it off. A press anywhere
    /// else in it asks for a file — the first, or one to replace it.
    fn image_field(
        &self,
        form: &ProjectForm,
        root: Option<&Path>,
        image: Option<Arc<Path>>,
        colour: Option<Color>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let trigger = match image {
            None => field("project-image", "", "Choose an image\u{2026}")
                .leading(Icon::Image)
                .render(t),
            Some(image) => {
                let named = image_name(&image, root);
                let thumbnail = image_badge(
                    Some(image),
                    colour.unwrap_or_else(default_color),
                    badge_initial(form, cx),
                    t,
                );
                let body = div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(ICON_GAP)
                    .child(thumbnail)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(self.font_family.clone())
                            .text_size(IMAGE_NAME)
                            .text_color(paint(t.text.primary))
                            .child(named.clone()),
                    )
                    .child(self.remove_image(form.tip, cx))
                    .into_any_element();
                field("project-image", named, "")
                    .body(body)
                    .render(t)
                    .pr(IMAGE_FIELD_END)
            }
        };
        trigger
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.choose_project_image(cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The image field's ✕, which takes the picture off the draft.
    fn remove_image(&self, shown: Option<Tip>, cx: &mut Context<Self>) -> AnyElement {
        let button = icon_button("project-image-remove", Icon::Close)
            .bare()
            .small()
            .render(&self.theme)
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(form) = this.project_form_mut() {
                    form.image = ImageDraft::Cleared;
                    form.error = None;
                    // The button goes with the picture, so the pointer
                    // leaving it is never reported.
                    if form.tip == Some(Tip::RemoveImage) {
                        form.tip = None;
                    }
                }
                // The press is the button's, not the field's around it.
                cx.stop_propagation();
                cx.notify();
            }));
        self.with_project_tip(Tip::RemoveImage, button.into_any_element(), shown, cx)
    }

    /// Asks for a picture with the system's file prompt, and takes it as the
    /// draft once core's checks pass — read off the window's thread, since
    /// a check reads the file whole. A file refused stays out of the draft,
    /// with core's reason on the dialog's error line.
    fn choose_project_image(&mut self, cx: &mut Context<Self>) {
        // The form it was asked for, known again by its name field: the
        // same dialog, not one opened since.
        let Some(form) = self.project_form().map(|form| form.name.entity_id()) else {
            return;
        };
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let checked = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { ket_core::project_image::check(&path) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.take_project_image(form, path, checked, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Takes a picked file into the draft of the form it was asked for, or
    /// its refusal onto the error line. Dropped when that form is no longer
    /// the one open: the prompt does not hold the window, and the dialog can
    /// close behind it.
    fn take_project_image(
        &mut self,
        asked: EntityId,
        path: PathBuf,
        checked: ket_core::Result<ImageFormat>,
        cx: &mut App,
    ) {
        let Some(form) = self
            .project_form_mut()
            .filter(|form| form.name.entity_id() == asked)
        else {
            return;
        };
        match checked {
            Ok(_) => {
                let path: Arc<Path> = Arc::from(path);
                // gpui keeps a picture by its path, so a file picked before
                // and changed since would be drawn as it was.
                cx.remove_asset::<gpui::ImgResourceLoader>(&gpui::Resource::Path(path.clone()));
                form.image = ImageDraft::Picked(path);
                form.error = None;
            }
            Err(e) => form.error = Some(e.to_string().into()),
        }
    }

    /// The icon's dropdown: a trigger showing the mark the badge wears, and
    /// under it, while open, the panel that picks one.
    fn icon_select(
        &self,
        form: &ProjectForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let (glyph, value) = icon_choice(form, cx);
        let mut trigger = select("project-icon", value).open(form.palette_open);
        if let Some(glyph) = glyph {
            trigger = trigger.leading(sized_icon(Icon::Glyph(glyph), ICON, paint(t.text.primary)));
        }
        let trigger = trigger
            .render(t)
            .w_full()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(form) = this.project_form_mut() {
                    if form.palette_open {
                        form.close_panel(window, cx);
                    } else {
                        form.open_panel(cx);
                    }
                }
                cx.notify();
            }));
        let panel = form.palette_open.then(|| self.icon_panel(form, window, cx));
        anchored(
            "project-icon-panel",
            trigger.into_any_element(),
            panel,
            Placement::BelowStart,
            CONTROL_W,
            window,
            t,
        )
    }

    /// The icon panel: the filter, the glyphs it leaves, and under a rule
    /// the field for a letter or an emoji instead.
    fn icon_panel(
        &self,
        form: &ProjectForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .gap(PANEL_PAD)
            .p(PANEL_PAD)
            .prose()
            .child(self.form_field(
                form,
                Field::IconFilter,
                false,
                Some(Icon::Search),
                window,
                cx,
            ))
            .child(self.glyph_grid(form, cx))
            .child(rule(t))
            .child(self.form_field(form, Field::Icon, false, Some(Icon::TextCursor), window, cx))
            .into_any_element()
    }
}

/// What the icon trigger shows: the glyph and its name, or the letter or
/// emoji typed — which outranks a glyph, as it does on the badge — or, with
/// neither, the initial the badge falls back to.
fn icon_choice(form: &ProjectForm, cx: &App) -> (Option<Glyph>, SharedString) {
    let typed = form.icon.read(cx).text();
    if !typed.trim().is_empty() {
        return (None, typed.trim().to_owned().into());
    }
    match form.glyph {
        Some(glyph) => (Some(glyph), glyph_words(glyph).into()),
        None => (None, badge_initial(form, cx)),
    }
}

/// What the image's field calls the picture at `path`: a file inside the
/// project by its path there — `public/favicon.svg` — and any other by its
/// name. The copy a project already has is named by ket, for its contents.
fn image_name(path: &Path, root: Option<&Path>) -> SharedString {
    let named = root
        .and_then(|root| path.strip_prefix(root).ok())
        .filter(|inside| !inside.as_os_str().is_empty())
        .or_else(|| path.file_name().map(Path::new))
        .unwrap_or(path);
    named.display().to_string().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_image_is_named_by_where_it_is() {
        let root = Path::new("/work/acme");
        // Inside the project: its path there, as the repository names it.
        assert_eq!(
            image_name(Path::new("/work/acme/public/favicon.svg"), Some(root)),
            "public/favicon.svg"
        );
        // Anywhere else: the file's name alone.
        assert_eq!(
            image_name(Path::new("/tmp/downloads/logo.png"), Some(root)),
            "logo.png"
        );
        // A sibling whose name only starts like the project's is not in it.
        assert_eq!(
            image_name(Path::new("/work/acme-site/logo.png"), Some(root)),
            "logo.png"
        );
        // ket's own copy goes by the name it is kept under.
        assert_eq!(
            image_name(
                Path::new("/data/project-images/acme-0123456789ab/0123456789abcdef.png"),
                Some(root)
            ),
            "0123456789abcdef.png"
        );
        // A project being added before its folder is chosen: the name alone.
        assert_eq!(
            image_name(Path::new("/work/acme/public/favicon.svg"), None),
            "favicon.svg"
        );
    }
}
