//! A viewer for image files opened from a project.
//!
//! The pane draws nothing of its own: gpui's asset system reads and decodes
//! the file off the UI thread, caches the frames per path, and wakes this
//! view when they are ready. Opening an image tab therefore has no file I/O
//! as a side effect, and ket-ui gains no filesystem dependency.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, Context, ElementId, RenderImage, SharedString, Window, div, prelude::*, px,
};

use crate::Shell;
use crate::paint::paint;
use crate::tabs::{Tab, TabKind};

/// File suffixes routed to the image viewer.
///
/// These are the raster formats gpui decodes through the `image` crate. SVG
/// is deliberately absent: it is source text people edit, so it stays in the
/// editor. The decoder still probes the actual bytes; a suffix never makes
/// malformed data safe.
pub(crate) fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "ico")
    )
}

/// The viewer's caption: `"PNG · 1024×768"`, from the extension a reader
/// already sees in the tab and the size once the decoder has reported it.
fn format_caption(path: &Path, size: Option<(u32, u32)>) -> String {
    let format = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "IMAGE".to_owned());
    match size {
        Some((width, height)) => format!("{format} · {width}×{height}"),
        None => format,
    }
}

/// The pictures behind this session's image tabs, by the tab's path.
///
/// gpui's asset cache keeps a decoded image for as long as the process runs;
/// nothing evicts it, and the texture it was painted from stays in the
/// window's atlas with it. A 20-megapixel photo is some 80 MB of pixels, so a
/// folder of them browsed one tab at a time would grow the app with every
/// open and give nothing back on close. An entry appears the moment a pane
/// asks for its file — `None` until the decode lands — so that
/// [`Shell::prune_images`] can hand everything back once the tab is gone,
/// including a load that was still in flight.
#[derive(Default)]
pub(crate) struct ImageFrames(HashMap<SharedString, Option<Arc<RenderImage>>>);

/// The asset-cache key for an image tab's path: a `Path`, never the key
/// string, since a `&str` source is looked up among the binary's embedded
/// assets rather than on disk.
fn resource(path: &str) -> gpui::Resource {
    gpui::Resource::Path(Arc::from(Path::new(path)))
}

impl Shell {
    /// Forgets every picture whose tab is no longer open anywhere, and frees
    /// its texture. Run at the top of each frame rather than where tabs
    /// close: a tab goes in several places — its own ✕, Close Pane, a
    /// worktree being removed — and only the frame has the window in hand,
    /// which gpui takes out of its map for the length of an event, so a drop
    /// made during one would miss the very window showing the picture.
    pub(crate) fn prune_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.images.0.is_empty() {
            return;
        }
        let open: Vec<SharedString> = self
            .spaces
            .values()
            .flat_map(|space| space.all_tabs())
            .filter_map(|tab| match &tab.kind {
                TabKind::Image(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        let gone: Vec<SharedString> = self
            .images
            .0
            .keys()
            .filter(|key| !open.contains(key))
            .cloned()
            .collect();
        for key in gone {
            // The cache entry first, whether or not it had finished loading:
            // a tab closed mid-decode drops the result when the decode lands.
            cx.remove_asset::<gpui::ImgResourceLoader>(&resource(&key));
            if let Some(Some(render)) = self.images.0.remove(&key) {
                cx.drop_image(render, Some(window));
            }
        }
    }

    /// Opens or focuses the dedicated viewer for an image file.
    ///
    /// Focusing an open tab re-reads the file: opening it is exactly when a
    /// reader would notice the picture had changed on disk, the same way the
    /// editor refreshes a buffer. The old frames stay up until the new ones
    /// land, so the tab does not blink to "Loading…".
    pub(crate) fn open_image_tab(&mut self, path: PathBuf, cx: &mut gpui::App) {
        let Some(worktree_id) = self.selected_id() else {
            return;
        };
        let key: SharedString = path.display().to_string().into();
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".to_owned());
        let kind = TabKind::Image(key.clone());
        let space = self.spaces.entry(worktree_id).or_default();
        if space.root.find_tab(&kind).is_some() {
            cx.remove_asset::<gpui::ImgResourceLoader>(&resource(&key));
        }
        space.open_tab(Tab {
            title: title.into(),
            kind,
            renamed: false,
            pinned: false,
        });
        self.persist_layout();
    }

    /// Renders one image tab: the picture scaled to fit, with its name,
    /// format and pixel size underneath.
    pub(crate) fn image_pane(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let file = Path::new(path);
        let title = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());
        let key: SharedString = path.to_owned().into();

        // `use_asset` starts the load the first time and notifies this view
        // once it finishes, so a pane drawn while loading redraws by itself.
        let loaded = window.use_asset::<gpui::ImgResourceLoader>(&resource(path), cx);
        let held = self.images.0.entry(key.clone()).or_insert(None);

        let centred = || {
            div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(paint(t.text.dim))
        };

        let render = match loaded {
            // Still decoding. A re-read of a file already on screen keeps
            // showing what it had rather than a blank pane.
            None => match held {
                Some(render) => render.clone(),
                None => return centred().child("Loading…").into_any_element(),
            },
            Some(Err(error)) => {
                return centred()
                    .text_color(paint(t.status.failed))
                    .child(format!("could not open {title}: {error}"))
                    .into_any_element();
            }
            Some(Ok(render)) => {
                // A re-read's new pixels replace the old; the old texture
                // goes with them, which this frame can do because it has the
                // window.
                let stale = held
                    .replace(render.clone())
                    .filter(|old| !Arc::ptr_eq(old, &render));
                if let Some(old) = stale {
                    cx.drop_image(old, Some(window));
                }
                render
            }
        };

        let size = render.size(0);
        let caption = format_caption(file, Some((size.width.0 as u32, size.height.0 as u32)));

        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .text_color(paint(t.text.primary))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .p(px(16.0))
                    .overflow_hidden()
                    .child(
                        // An id, because gpui advances an animated GIF's or
                        // WebP's frames only for an element it can keep
                        // state for.
                        gpui::img(gpui::ImageSource::Render(render))
                            .id((ElementId::from("image-tab"), key))
                            .size_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .pb(px(14.0))
                    .text_size(px(11.5))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                    .child(div().text_color(paint(t.text.dim)).child(caption)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_suffixes_are_images_regardless_of_case() {
        for name in [
            "a.png", "b.JPG", "c.jpeg", "d.gif", "e.WebP", "f.bmp", "g.tiff", "h.ico",
        ] {
            assert!(is_image_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn source_and_text_files_are_not_images() {
        // SVG is text somebody edits, so it belongs to the editor.
        for name in ["a.svg", "b.rs", "c.md", "Makefile", "d.png.bak"] {
            assert!(!is_image_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn a_dotfile_named_like_a_format_has_no_extension() {
        // `.png` is a hidden file called "png", not a picture with no name.
        assert!(!is_image_path(Path::new(".png")));
        assert!(!is_image_path(Path::new("/repo/.webp")));
    }

    #[test]
    fn both_tiff_spellings_are_images() {
        assert!(is_image_path(Path::new("scan.tif")));
        assert!(is_image_path(Path::new("scan.tiff")));
    }

    #[test]
    fn the_caption_upper_cases_whatever_suffix_the_file_wears() {
        // The tab shows the suffix as typed, so the caption normalises it
        // rather than inventing a canonical name for the format.
        assert_eq!(format_caption(Path::new("a.jpeg"), None), "JPEG");
        assert_eq!(format_caption(Path::new("a.Jpg"), None), "JPG");
    }

    #[test]
    fn the_caption_names_the_format_and_the_size_once_known() {
        let path = Path::new("/tmp/shot.png");
        assert_eq!(format_caption(path, None), "PNG");
        assert_eq!(format_caption(path, Some((1024, 768))), "PNG · 1024×768");
        assert_eq!(format_caption(Path::new("noext"), None), "IMAGE");
    }
}
