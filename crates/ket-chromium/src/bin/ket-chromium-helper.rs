//! Chromium's subprocesses for ket: the binary inside `ket Helper.app` and
//! its GPU, Renderer, Plugin and Alerts variants in ket.app's
//! `Contents/Frameworks`. Everything it does is
//! [`ket_chromium::helper_main`].

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(ket_chromium::helper_main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("ket's Chromium helper only runs on macOS");
    std::process::exit(1);
}
