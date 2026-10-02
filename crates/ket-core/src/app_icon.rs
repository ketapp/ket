//! The icon an application ships, for drawing beside its name.
//!
//! A menu offering to open a file in VS Code, Cursor or Zed is easier to aim
//! at when each row wears the mark a person already knows from their Dock.
//! Those marks are the vendors' own, so ket does not draw them: it reads them,
//! when they are wanted, from the application bundle already installed on this
//! machine — where Finder's own *Open With* menu takes them from. An editor
//! with no bundle to read gets no icon, and the caller draws a generic one.
//!
//! macOS only, and deliberately without a plist or icon library. A bundle is
//! `Something.app/Contents/Info.plist` naming an `.icns` under
//! `Contents/Resources`; the plist is read for that one key with a string
//! search, and an `.icns` is a flat run of typed chunks with eight bytes of
//! header each, nearly all of them PNG since 10.7.

use std::path::{Path, PathBuf};

/// The `.app` directory `program` lives inside, if any.
///
/// Symlinks are followed first: `/usr/local/bin/code` is a link into the
/// bundle, and the bundle is where the icon is.
pub fn bundle_of(program: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(program).ok()?;
    real.ancestors()
        .find(|dir| {
            dir.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        })
        .map(Path::to_path_buf)
}

/// The bundle's icon as PNG bytes, at the smallest size that is at least
/// `want` pixels square.
///
/// `want` is pixels, not points: a 16pt menu mark on a 2x display wants 32.
/// Larger than asked rather than smaller when nothing fits exactly, because a
/// mark scaled down keeps its shape and one scaled up does not.
///
/// `None` when the bundle names no icon, the file is missing, or nothing in it
/// is PNG — an old bundle whose chunks are JPEG 2000 or raw ARGB, which ket
/// has no decoder for and does not want one.
#[cfg(target_os = "macos")]
pub fn icon_png(bundle: &Path, want: u32) -> Option<Vec<u8>> {
    let contents = bundle.join("Contents");
    let resources = contents.join("Resources");

    let plist = std::fs::read_to_string(contents.join("Info.plist")).ok();
    let named = plist.as_deref().and_then(icon_file_name).map(|name| {
        let mut path = resources.join(name);
        // `CFBundleIconFile` may name the file with or without its extension;
        // both are common and both are the same file.
        if path.extension().is_none() {
            path.set_extension("icns");
        }
        path
    });
    // A bundle whose plist is binary — which ket does not parse — or names no
    // icon usually still ships one called after the application.
    let after_bundle = bundle
        .file_stem()
        .map(|stem| resources.join(stem).with_extension("icns"));

    [named, after_bundle]
        .into_iter()
        .flatten()
        .find_map(|path| std::fs::read(path).ok())
        .and_then(|bytes| icns_png(&bytes, want))
}

/// No bundles to read on other platforms.
#[cfg(not(target_os = "macos"))]
pub fn icon_png(_bundle: &Path, _want: u32) -> Option<Vec<u8>> {
    None
}

/// The value of `CFBundleIconFile` in an XML property list.
///
/// A dictionary's value follows its key, so the first `<string>` after the key
/// is the one. Binary plists are not understood and answer `None`; third-party
/// editors ship XML, and Apple's own applications are not editors ket offers.
#[cfg(target_os = "macos")]
fn icon_file_name(plist: &str) -> Option<&str> {
    let rest = &plist[plist.find("<key>CFBundleIconFile</key>")?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = start + rest[start..].find("</string>")?;
    let name = rest[start..end].trim();
    (!name.is_empty()).then_some(name)
}

/// The eight bytes every PNG starts with.
#[cfg(target_os = "macos")]
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The `.icns` chunk types whose payload can be a PNG, and the square each
/// holds.
///
/// The `@2x` types are listed by their pixels, not their points: `ic11` is the
/// 16pt mark drawn at 32 pixels, and the 32 is what choosing one is about.
/// `icp4` and `icp5` are sometimes JPEG 2000 instead, which is why every
/// candidate is checked against [`PNG_MAGIC`] rather than trusted by type.
#[cfg(target_os = "macos")]
const PNG_KINDS: [(&[u8; 4], u32); 11] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
];

/// The PNG inside an `.icns` file closest to `want` pixels square from above.
///
/// The container is a magic `icns`, a total length, then chunks of a four-byte
/// type, a four-byte big-endian length that counts its own header, and the
/// payload. Walked to the end rather than trusting the table of contents chunk,
/// which not every writer emits.
#[cfg(target_os = "macos")]
fn icns_png(bytes: &[u8], want: u32) -> Option<Vec<u8>> {
    if bytes.len() < 8 || &bytes[..4] != b"icns" {
        return None;
    }

    let mut found: Vec<(u32, &[u8])> = Vec::new();
    let mut offset = 8;
    while offset + 8 <= bytes.len() {
        let kind = &bytes[offset..offset + 4];
        let len = u32::from_be_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        // A length shorter than its own header, or one running past the end,
        // is a file ket does not understand; what was read so far still counts.
        if len < 8 || offset + len > bytes.len() {
            break;
        }
        let body = &bytes[offset + 8..offset + len];
        offset += len;

        let side = PNG_KINDS
            .iter()
            .find(|(name, _)| *name == kind)
            .map(|(_, side)| *side);
        if let Some(side) = side
            && body.starts_with(PNG_MAGIC)
        {
            found.push((side, body));
        }
    }

    let best = found
        .iter()
        .filter(|(side, _)| *side >= want)
        .min_by_key(|(side, _)| *side)
        .or_else(|| found.iter().max_by_key(|(side, _)| *side))?;
    Some(best.1.to_vec())
}
