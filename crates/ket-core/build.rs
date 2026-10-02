//! Finds the themes ket ships — one TOML file each under `themes/` — so a new
//! theme is a new file, with nothing to register by hand. See
//! `ket_core::theme` for what a file holds.

use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
    // The folder itself, so a file added or removed is noticed too.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("ket-core/themes is missing")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "toml" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_owned)
        })
        .collect();
    names.sort();
    let mut out = String::from(
        "/// Every theme file ket ships: its name, and its text as written.\n\
         pub(crate) const SOURCES: &[(&str, &str)] = &[\n",
    );
    for name in &names {
        let path = dir.join(format!("{name}.toml"));
        println!("cargo:rerun-if-changed={}", path.display());
        out.push_str(&format!(
            "    ({name:?}, include_str!({:?})),\n",
            path.display().to_string()
        ));
    }
    out.push_str("];\n");
    let target = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("builtin_themes.rs");
    std::fs::write(target, out).expect("could not write the theme list");
}
