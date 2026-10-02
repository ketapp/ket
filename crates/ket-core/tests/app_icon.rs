//! Reading an application bundle's icon: finding the bundle from a program
//! path, and pulling a PNG out of its `.icns`.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::Path;

use ket_core::app_icon::{bundle_of, icon_png};

mod common;
use common::Sandbox;

fn write_plist(bundle: &Path, icon_file: &str) {
    let contents = bundle.join("Contents");
    fs::create_dir_all(&contents).unwrap();
    fs::write(
        contents.join("Info.plist"),
        format!(
            "<?xml version=\"1.0\"?>\n<plist><dict>\n<key>CFBundleIconFile</key>\n<string>{icon_file}</string>\n</dict></plist>\n"
        ),
    )
    .unwrap();
}

/// A minimal `.icns` with one chunk: `kind` holding `payload` after the PNG
/// magic, or not, depending on `is_png`.
fn write_icns(path: &Path, kind: &[u8; 4], side_payload: &[u8], is_png: bool) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut body = Vec::new();
    if is_png {
        body.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    }
    body.extend_from_slice(side_payload);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"icns");
    let chunk_len = 8 + body.len();
    let total_len = 8 + 8 + body.len();
    bytes.extend_from_slice(&(total_len as u32).to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(&(chunk_len as u32).to_be_bytes());
    bytes.extend_from_slice(&body);

    fs::write(path, bytes).unwrap();
}

#[test]
fn bundle_of_finds_the_app_directory_a_program_lives_inside() {
    let sandbox = Sandbox::new("icon-bundle-of");
    let bundle = sandbox.path("Example.app");
    let binary = bundle.join("Contents/MacOS/example");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"#!/bin/sh\n").unwrap();

    assert_eq!(bundle_of(&binary), Some(bundle.canonicalize().unwrap()));
}

#[test]
fn bundle_of_is_none_for_a_program_outside_any_bundle() {
    let sandbox = Sandbox::new("icon-bundle-of-none");
    let binary = sandbox.path("bin/plain");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"#!/bin/sh\n").unwrap();

    assert_eq!(bundle_of(&binary), None);
}

#[test]
fn bundle_of_is_none_for_a_program_that_does_not_exist() {
    let sandbox = Sandbox::new("icon-bundle-of-missing");
    assert_eq!(bundle_of(&sandbox.path("nowhere")), None);
}

#[test]
fn icon_png_reads_the_icon_named_in_the_plist() {
    let sandbox = Sandbox::new("icon-named-in-plist");
    let bundle = sandbox.path("Example.app");
    write_plist(&bundle, "AppIcon");
    write_icns(
        &bundle.join("Contents/Resources/AppIcon.icns"),
        b"icp4",
        b"fake-png-bytes",
        true,
    );

    let png = icon_png(&bundle, 16).unwrap();
    assert!(png.ends_with(b"fake-png-bytes"));
}

#[test]
fn icon_png_accepts_a_plist_name_that_already_has_an_extension() {
    let sandbox = Sandbox::new("icon-plist-name-with-ext");
    let bundle = sandbox.path("Example.app");
    write_plist(&bundle, "AppIcon.icns");
    write_icns(
        &bundle.join("Contents/Resources/AppIcon.icns"),
        b"icp4",
        b"payload",
        true,
    );

    assert!(icon_png(&bundle, 16).is_some());
}

#[test]
fn icon_png_falls_back_to_a_file_named_after_the_bundle() {
    let sandbox = Sandbox::new("icon-fallback-bundle-name");
    let bundle = sandbox.path("Example.app");
    // No Info.plist at all.
    fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    write_icns(
        &bundle.join("Contents/Resources/Example.icns"),
        b"icp4",
        b"payload",
        true,
    );

    assert!(icon_png(&bundle, 16).is_some());
}

#[test]
fn icon_png_is_none_when_nothing_is_there() {
    let sandbox = Sandbox::new("icon-nothing");
    let bundle = sandbox.path("Empty.app");
    fs::create_dir_all(bundle.join("Contents")).unwrap();

    assert!(icon_png(&bundle, 16).is_none());
}

#[test]
fn icon_png_picks_the_smallest_size_at_least_as_big_as_asked() {
    let sandbox = Sandbox::new("icon-picks-size");
    let bundle = sandbox.path("Example.app");
    write_plist(&bundle, "AppIcon");
    let icns = bundle.join("Contents/Resources/AppIcon.icns");
    fs::create_dir_all(icns.parent().unwrap()).unwrap();

    // Build an icns with three PNG chunks at 16, 32 and 128.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"icns");
    bytes.extend_from_slice(&0u32.to_be_bytes()); // total length patched below

    let mut chunk = |kind: &[u8; 4], tag: &[u8]| {
        let mut body = b"\x89PNG\r\n\x1a\n".to_vec();
        body.extend_from_slice(tag);
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
        bytes.extend_from_slice(&body);
    };
    chunk(b"icp4", b"16px"); // 16
    chunk(b"icp5", b"32px"); // 32
    chunk(b"ic07", b"128px"); // 128

    let total_len = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&total_len.to_be_bytes());
    fs::write(&icns, bytes).unwrap();

    // Asking for 20 should pick 32, the smallest that is at least 20.
    let png = icon_png(&bundle, 20).unwrap();
    assert!(png.ends_with(b"32px"), "{png:?}");
}

#[test]
fn icon_png_falls_back_to_the_largest_when_nothing_is_big_enough() {
    let sandbox = Sandbox::new("icon-falls-back-to-largest");
    let bundle = sandbox.path("Example.app");
    write_plist(&bundle, "AppIcon");
    write_icns(
        &bundle.join("Contents/Resources/AppIcon.icns"),
        b"icp4",
        b"only-16px",
        true,
    );

    // Nothing is >= 512, so the only one there wins regardless.
    let png = icon_png(&bundle, 512).unwrap();
    assert!(png.ends_with(b"only-16px"));
}

#[test]
fn icon_png_skips_a_chunk_whose_body_is_not_actually_png() {
    let sandbox = Sandbox::new("icon-skips-non-png");
    let bundle = sandbox.path("Example.app");
    write_plist(&bundle, "AppIcon");
    write_icns(
        &bundle.join("Contents/Resources/AppIcon.icns"),
        // icp4 can be JPEG 2000; this body has no PNG magic.
        b"icp4",
        b"not-a-png-payload",
        false,
    );

    assert!(icon_png(&bundle, 16).is_none());
}

#[test]
fn icon_png_is_none_for_a_file_that_is_not_an_icns_at_all() {
    let sandbox = Sandbox::new("icon-not-icns");
    let bundle = sandbox.path("Example.app");
    write_plist(&bundle, "AppIcon");
    fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    fs::write(
        bundle.join("Contents/Resources/AppIcon.icns"),
        b"not an icns file",
    )
    .unwrap();

    assert!(icon_png(&bundle, 16).is_none());
}
