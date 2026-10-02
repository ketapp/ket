//! Embeds an `Info.plist` in the bare `ket-ui` binary on macOS.
//!
//! A process that asks macOS for the microphone or for Speech Recognition
//! without a usage description in its `Info.plist` is killed on the spot, and
//! a binary run straight out of `target/` has no bundle to carry one. The
//! linker can put a plist in the executable itself (`__TEXT,__info_plist`),
//! which is where macOS looks for a process with no bundle — so `cargo run`
//! can use voice input the same as `ket.app`, whose own `Info.plist`
//! (`scripts/bundle-macos.sh`) carries the same two keys.
//!
//! The identifier is not ket.app's, so permissions given to a development
//! build are kept apart from the installed app's.

use std::path::PathBuf;

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>dev.ketapp.ket.dev</string>
    <key>CFBundleName</key>
    <string>ket</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>ket listens while you dictate a prompt.</string>
    <key>NSSpeechRecognitionUsageDescription</key>
    <string>ket turns what you dictate into the text of a prompt.</string>
</dict>
</plist>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let plist = out.join("Info.plist");
    std::fs::write(&plist, PLIST).expect("OUT_DIR is writable");
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
