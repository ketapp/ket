//! Where ket's own command is: beside the app, or beside a development
//! build — what terminals put on their PATH and what an automation's
//! command is run through.

use std::path::{Path, PathBuf};

/// The CLI shipped beside the app, or beside a development `ket-ui` binary.
fn path_for(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if bin.file_name().is_some_and(|name| name == "MacOS")
        && bin
            .parent()?
            .file_name()
            .is_some_and(|name| name == "Contents")
    {
        Some(bin.parent()?.join("Resources/bin/ket"))
    } else {
        Some(bin.join("ket"))
    }
}

/// The `ket` command beside this build, when it is there.
pub(crate) fn path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| path_for(&exe))
        .filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_cli_in_app_and_development_layouts() {
        assert_eq!(
            path_for(Path::new("/Applications/ket.app/Contents/MacOS/ket")),
            Some(PathBuf::from(
                "/Applications/ket.app/Contents/Resources/bin/ket"
            ))
        );
        assert_eq!(
            path_for(Path::new("/checkout/target/debug/ket-ui")),
            Some(PathBuf::from("/checkout/target/debug/ket"))
        );
    }
}
