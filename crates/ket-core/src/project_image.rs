//! A project's badge image: a favicon or logo drawn on its badge in place of
//! the glyph or letter.
//!
//! The file a person picks is copied in, the way a backlog note's files are
//! (see [`crate::backlog`]), so the badge survives the original being moved,
//! renamed or deleted. The project remembers the copy's name in
//! [`ProjectSettings::image`], never the original path:
//!
//! ```text
//! <data_dir>/project-images/<project-id>/<content-hash>.<ext>
//! ```
//!
//! The name comes from the file's contents, so a different picture always
//! has a different path, and an image cache keyed by path — gpui's is —
//! never goes on showing the old one after a change. The same picture picked
//! twice is the same file.
//!
//! What is accepted is what a badge wants and the shell can draw from a file:
//! PNG, JPEG, GIF, WebP, SVG and ICO, of at most [`MAX_IMAGE_BYTES`]. They are
//! told apart by their contents, not their extension, which is how gpui's
//! `img` decides how to decode a file too: a `.png` that is really a BMP
//! would not be drawn as a PNG, so it is not accepted as one.
//!
//! Choosing one takes two steps, to fit a dialog that holds its changes until
//! Save. [`Workspace::import_project_image`] copies the file in and returns
//! its name; the name goes into [`ProjectSettings::image`], and
//! [`Workspace::set_project_settings`] writes it with everything else. That
//! write is what deletes the copies a project no longer names — the one just
//! replaced, every one when the image is cleared, and any left by a Save that
//! failed halfway — and [`Workspace::remove_project`] deletes the project's
//! directory.
//!
//! [`ProjectSettings::image`]: crate::project::ProjectSettings::image
//! [`Workspace::import_project_image`]: crate::workspace::Workspace::import_project_image
//! [`Workspace::set_project_settings`]: crate::workspace::Workspace::set_project_settings
//! [`Workspace::remove_project`]: crate::workspace::Workspace::remove_project

use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use crate::id::ProjectId;
use crate::{KetError, Result};

/// Largest image accepted, in bytes: 1 MiB.
///
/// A favicon is a few kilobytes and a logo exported for the web rarely more
/// than a couple of hundred; a badge is drawn at the size of a letter. The
/// cap is there for the file picked by mistake — a photo straight off a
/// camera, a video — which would otherwise be copied into the data directory
/// and decoded on every launch.
pub const MAX_IMAGE_BYTES: u64 = 1024 * 1024;

/// The kinds of image a badge can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// Portable Network Graphics.
    Png,
    /// JPEG.
    Jpeg,
    /// GIF. A badge draws an animated one's first frame.
    Gif,
    /// WebP. A badge draws an animated one's first frame.
    WebP,
    /// SVG, as text. Compressed `.svgz` is not accepted.
    Svg,
    /// A Windows icon file, the classic `favicon.ico`.
    Ico,
}

impl ImageFormat {
    /// Every format accepted, in the order an error lists them.
    pub const ALL: [Self; 6] = [
        Self::Png,
        Self::Jpeg,
        Self::Gif,
        Self::WebP,
        Self::Svg,
        Self::Ico,
    ];

    /// Its name, as a sentence says it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Svg => "SVG",
            Self::Ico => "ICO",
        }
    }

    /// The extension a copy is given, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Svg => "svg",
            Self::Ico => "ico",
        }
    }

    /// Which format `bytes` hold, from their contents alone.
    ///
    /// The raster signatures are the ones the `image` crate's `guess_format`
    /// matches, which is what gpui decodes by; anything it does not match,
    /// gpui hands to its SVG renderer, so an SVG is recognised by its root
    /// element instead.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
        const JPEG: &[u8] = &[0xff, 0xd8, 0xff];
        const ICO: &[u8] = &[0, 0, 1, 0];

        if bytes.starts_with(PNG) {
            Some(Self::Png)
        } else if bytes.starts_with(JPEG) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some(Self::WebP)
        } else if bytes.starts_with(ICO) {
            Some(Self::Ico)
        } else if is_svg(bytes) {
            Some(Self::Svg)
        } else {
            None
        }
    }
}

/// Whether `bytes` are an SVG document: UTF-8 text whose root element is
/// `<svg>`, after any declaration, comments and doctype.
///
/// UTF-8 because that is all gpui's SVG renderer reads. The root element and
/// not just an `<svg` somewhere, so an HTML page with an inline icon in it is
/// not taken for one.
fn is_svg(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let Ok(mut rest) = std::str::from_utf8(bytes) else {
        return false;
    };
    loop {
        rest = rest.trim_start();
        let end = if let Some(after) = rest.strip_prefix("<?") {
            after.find("?>").map(|end| (after, end + 2))
        } else if let Some(after) = rest.strip_prefix("<!--") {
            after.find("-->").map(|end| (after, end + 3))
        } else if let Some(after) = rest.strip_prefix("<!") {
            // A doctype, perhaps with declarations of its own in brackets,
            // which may hold a `>` of their own.
            let close = after.find('>');
            let end = match after.find('[') {
                Some(open) if close.is_none_or(|close| open < close) => after[open..]
                    .find(']')
                    .and_then(|shut| after[open + shut..].find('>').map(|gt| open + shut + gt)),
                _ => close,
            };
            end.map(|end| (after, end + 1))
        } else {
            break;
        };
        let Some((after, end)) = end else {
            return false;
        };
        rest = &after[end..];
    }
    rest.strip_prefix("<svg").is_some_and(|after| {
        after.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
    })
}

/// Checks that `source` is an image a badge can show, without copying it.
///
/// For a dialog to call the moment a file is picked, so a file that will be
/// refused is refused then, next to the picker, rather than at Save. The
/// error names the file and says why in words a person can act on: it is
/// missing, a folder, empty, over [`MAX_IMAGE_BYTES`], or not one of
/// [`ImageFormat::ALL`].
pub fn check(source: &Path) -> Result<ImageFormat> {
    read(source).map(|(_, format)| format)
}

/// Reads `source` whole, refusing what [`check`] refuses.
fn read(source: &Path) -> Result<(Vec<u8>, ImageFormat)> {
    let named = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.display().to_string());
    let refuse = |why: String| KetError::Conflict(format!("{named} {why}"));

    let mut file = std::fs::File::open(source).map_err(|e| {
        KetError::Conflict(match e.kind() {
            std::io::ErrorKind::NotFound => format!("{named} is not there any more"),
            _ => format!("could not read {named}: {e}"),
        })
    })?;
    let metadata = file
        .metadata()
        .map_err(|e| KetError::Conflict(format!("could not read {named}: {e}")))?;
    if metadata.is_dir() {
        return Err(refuse("is a folder, not an image".to_owned()));
    }
    if !metadata.is_file() {
        return Err(refuse("is not a file".to_owned()));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(refuse(too_large(metadata.len())));
    }

    // Bounded even after the size check: a file can grow between the two.
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.by_ref()
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| KetError::Conflict(format!("could not read {named}: {e}")))?;
    let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if read > MAX_IMAGE_BYTES {
        return Err(refuse(too_large(read)));
    }
    if bytes.is_empty() {
        return Err(refuse("is empty".to_owned()));
    }

    let format = ImageFormat::sniff(&bytes).ok_or_else(|| {
        refuse(format!(
            "is not an image a badge can show: it has to be a {} file",
            accepted()
        ))
    })?;
    Ok((bytes, format))
}

/// "PNG, JPEG, … or ICO".
fn accepted() -> String {
    let names: Vec<&str> = ImageFormat::ALL.iter().map(|f| f.name()).collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

/// "is 3.4 MB; …", for a file over the cap.
fn too_large(bytes: u64) -> String {
    // Megabytes to one place, rounded up, so a file just over the cap never
    // reads as exactly at it.
    let tenths = (bytes * 10).div_ceil(1024 * 1024);
    format!(
        "is {}.{} MB; a project image can be at most {} MB",
        tenths / 10,
        tenths % 10,
        MAX_IMAGE_BYTES / (1024 * 1024)
    )
}

/// Whether `name` is a single, ordinary file name: what a copy is called,
/// and so all [`ProjectSettings::image`] may hold. Never a path, never `..`,
/// never hidden.
///
/// [`ProjectSettings::image`]: crate::project::ProjectSettings::image
pub(crate) fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && name.len() <= 255
}

/// The directory `project`'s copies are kept in under `root`, or `None` for
/// an id that is not a plain name and so must not become a path.
fn project_dir(root: &Path, project: &ProjectId) -> Option<PathBuf> {
    is_plain_name(project.as_str()).then(|| root.join(project.as_str()))
}

/// Where the copy called `name` is kept for `project` under `root`.
///
/// No I/O: cheap enough to call every frame. `None` for a name that is not a
/// plain file name, which nothing ket writes is. A file deleted by hand still
/// has a path; it is the drawing that fails, and the badge falls back to its
/// glyph or letter.
pub fn path_in(root: &Path, project: &ProjectId, name: &str) -> Option<PathBuf> {
    if !is_plain_name(name) {
        return None;
    }
    project_dir(root, project).map(|dir| dir.join(name))
}

/// Copies `source` in as a candidate image for `project` under `root`, and
/// returns the name it is kept under.
///
/// Nothing names it until settings are written with it, and the next write
/// that does not deletes it again.
pub(crate) fn import_in(root: &Path, project: &ProjectId, source: &Path) -> Result<String> {
    let (bytes, format) = read(source)?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let hash: String = digest.as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let name = format!("{hash}.{}", format.extension());

    let dir = project_dir(root, project)
        .ok_or_else(|| KetError::Config(format!("project id {project} cannot name a directory")))?;
    let target = dir.join(&name);
    if target.is_file() {
        // The same picture, already here.
        return Ok(name);
    }
    std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
    // Written beside the target and renamed over it, so a copy cut short is
    // never taken for the image.
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, &bytes).map_err(|e| KetError::io(&temporary, e))?;
    std::fs::rename(&temporary, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        KetError::io(&target, e)
    })?;
    Ok(name)
}

/// Deletes every copy `project` has under `root` but `keep` — and the
/// directory itself when `keep` is `None`.
///
/// Logged rather than returned: it follows a write that has already
/// succeeded, and a file that could not be deleted is tidying left undone,
/// not a reason to report the write as failed.
pub(crate) fn prune_in(root: &Path, project: &ProjectId, keep: Option<&str>) {
    let Some(dir) = project_dir(root, project) else {
        return;
    };
    let Some(keep) = keep else {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(dir = %dir.display(), %e, "could not delete project images"),
        }
        return;
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(dir = %dir.display(), %e, "could not list project images");
            return;
        }
    };
    for entry in entries.flatten() {
        if entry.file_name() == keep {
            continue;
        }
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(e) = removed
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), %e, "could not delete a project image");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_accepted_format_is_told_by_its_contents() {
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(ImageFormat::sniff(b"GIF89a\x01\0"), Some(ImageFormat::Gif));
        assert_eq!(ImageFormat::sniff(b"GIF87a\x01\0"), Some(ImageFormat::Gif));
        assert_eq!(
            ImageFormat::sniff(b"RIFF\x24\0\0\0WEBPVP8 "),
            Some(ImageFormat::WebP)
        );
        assert_eq!(
            ImageFormat::sniff(&[0, 0, 1, 0, 1, 0]),
            Some(ImageFormat::Ico)
        );
        assert_eq!(
            ImageFormat::sniff(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#),
            Some(ImageFormat::Svg)
        );
        let extensions: Vec<&str> = ImageFormat::ALL.iter().map(|f| f.extension()).collect();
        assert_eq!(extensions, ["png", "jpg", "gif", "webp", "svg", "ico"]);
    }

    #[test]
    fn an_svg_is_found_after_its_prolog() {
        let svg = "\u{feff}<?xml version=\"1.0\"?>\n<!-- made by hand -->\n\
            <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \
            \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\" [ <!ENTITY a \"b\"> ]>\n\
            <svg viewBox=\"0 0 16 16\"><rect width=\"16\" height=\"16\"/></svg>";
        assert_eq!(ImageFormat::sniff(svg.as_bytes()), Some(ImageFormat::Svg));
        assert_eq!(ImageFormat::sniff(b"<svg>"), Some(ImageFormat::Svg));
    }

    #[test]
    fn what_a_badge_cannot_show_is_not_taken_for_an_image() {
        // Formats gpui could decode but a badge does not take.
        assert_eq!(ImageFormat::sniff(b"BM\x36\0\0\0"), None);
        assert_eq!(ImageFormat::sniff(b"II*\0"), None);
        // Text that merely mentions an SVG.
        assert_eq!(
            ImageFormat::sniff(b"<!doctype html><html><body><svg></svg></body></html>"),
            None
        );
        assert_eq!(ImageFormat::sniff(b"<svgfoo/>"), None);
        assert_eq!(ImageFormat::sniff(b"hello"), None);
        assert_eq!(ImageFormat::sniff(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(ImageFormat::sniff(b""), None);
        // Gzipped SVG, which a badge does not take either.
        assert_eq!(ImageFormat::sniff(&[0x1f, 0x8b, 0x08]), None);
        // Not UTF-8.
        assert_eq!(ImageFormat::sniff(b"<svg>\xff</svg>"), None);
        // A prolog that never closes.
        assert_eq!(ImageFormat::sniff(b"<?xml <svg/>"), None);
    }

    #[test]
    fn only_plain_file_names_are_names() {
        assert!(is_plain_name("0123456789abcdef.png"));
        for bad in [
            "",
            ".",
            "..",
            "../x.png",
            "a/b.png",
            "/etc/passwd",
            "a\\b.png",
            ".hidden.png",
        ] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
        assert!(!is_plain_name(&"x".repeat(256)));
    }

    #[test]
    fn a_path_is_never_built_from_a_name_that_is_not_plain() {
        let root = Path::new("/data/project-images");
        let id = ProjectId::new("acme-0123456789ab".to_owned());
        assert_eq!(
            path_in(root, &id, "0123456789abcdef.png"),
            Some(PathBuf::from(
                "/data/project-images/acme-0123456789ab/0123456789abcdef.png"
            ))
        );
        assert_eq!(path_in(root, &id, "../../state.json"), None);
        assert_eq!(
            path_in(root, &ProjectId::new("..".to_owned()), "a.png"),
            None
        );
    }

    #[test]
    fn a_file_over_the_cap_says_how_large_it_is() {
        assert_eq!(
            too_large(MAX_IMAGE_BYTES + 1),
            "is 1.1 MB; a project image can be at most 1 MB"
        );
        assert_eq!(
            too_large(3 * 1024 * 1024 + 400 * 1024),
            "is 3.4 MB; a project image can be at most 1 MB"
        );
    }
}
