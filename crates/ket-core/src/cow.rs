//! Copy-on-write directory cloning.
//!
//! Provisioning a worktree means materialising `node_modules` and friends into
//! it. Doing that with a byte-for-byte copy is the difference between worktree
//! creation being instant and being something you avoid.
//!
//! Measured on this machine, a 20,000-file / 78 MB tree:
//!
//! | Method | Time |
//! |---|---|
//! | `clonefile(2)` on the directory | 293 ms |
//! | `cp -c -R` (per-file clone) | 3,084 ms |
//! | `cp -R` (full copy) | 5,545 ms |
//!
//! The gap is structural, not incidental. `cp -c -R` walks the tree and clones
//! each file individually, so it stays O(number of files); `clonefile(2)` on a
//! directory clones the whole hierarchy in one syscall. A real `node_modules`
//! has 50k+ files, where that is the difference between well under a second and
//! most of a minute.
//!
//! There is no safe crate for this. `reflink-copy` handles files, not directory
//! trees. So this module holds the workspace's only `unsafe`, in one binding.

use std::ffi::{CString, c_char, c_int};
use std::path::Path;
use std::process::Command;

/// How a directory ended up being materialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneMethod {
    /// Copy-on-write. Near-instant, and blocks are shared until written to.
    CopyOnWrite,
    /// A real byte-for-byte copy, because copy-on-write was unavailable.
    FullCopy,
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{CString, c_char, c_int};

    // SAFETY: `clonefile` is a stable macOS system call, available since 10.12,
    // declared in <sys/clonefile.h>. Declaring it here rather than depending on
    // `libc` keeps the dependency count at zero for this path; the signature is
    // taken directly from the header.
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn clonefile(src: *const c_char, dst: *const c_char, flags: u32) -> c_int;
    }

    /// Calls `clonefile(2)`.
    ///
    /// `dst` must not already exist — the syscall fails with `EEXIST` rather
    /// than merging into it, which is the behaviour we want.
    pub fn clonefile_dir(src: &CString, dst: &CString) -> std::io::Result<()> {
        // SAFETY: both pointers come from `CString`s that outlive the call and
        // are guaranteed NUL-terminated with no interior NULs. `clonefile` reads
        // them and does not retain them. A flags value of 0 is the documented
        // default. The return value is checked, and `errno` is read immediately
        // via `last_os_error` on failure.
        #[allow(unsafe_code)]
        let rc = unsafe { clonefile(src.as_ptr(), dst.as_ptr(), 0) };

        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// Converts a path to a C string, rejecting interior NUL bytes.
fn c_path(path: &Path) -> std::io::Result<CString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "clonefile is unix-only",
        ))
    }
}

/// Materialises `src` at `dst`, preferring copy-on-write.
///
/// `dst` must not exist. Returns which method was actually used, so callers can
/// warn when a clone silently degraded into a full copy — on a network mount or
/// a non-APFS volume, that is the difference between fast and unusable.
pub fn clone_directory(src: &Path, dst: &Path) -> std::io::Result<CloneMethod> {
    if !src.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a directory", src.display()),
        ));
    }

    if dst.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", dst.display()),
        ));
    }

    #[cfg(target_os = "macos")]
    {
        match (c_path(src), c_path(dst)) {
            (Ok(s), Ok(d)) => match sys::clonefile_dir(&s, &d) {
                Ok(()) => return Ok(CloneMethod::CopyOnWrite),
                Err(e) => {
                    // Cross-device, non-APFS, or a network mount. Fall through
                    // to a copy rather than failing the whole provision.
                    tracing::debug!(%e, src = %src.display(), "clonefile unavailable; copying");
                }
            },
            _ => {
                tracing::debug!("path not representable as a C string; copying");
            }
        }
    }

    full_copy(src, dst)?;
    Ok(CloneMethod::FullCopy)
}

/// Falls back to `cp`, requesting a reflink where the platform supports one.
fn full_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut command = Command::new("cp");

    #[cfg(target_os = "linux")]
    command.arg("--reflink=auto");

    // `-R` recurses; `-p` preserves modes and timestamps so build tools do not
    // decide everything is stale and rebuild it.
    let output = command.args(["-Rp"]).arg(src).arg(dst).output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "cp failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sandbox(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ket-cow-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clones_a_directory_tree() {
        let root = sandbox("basic");
        let src = root.join("src");
        fs::create_dir_all(src.join("nested/deep")).unwrap();
        fs::write(src.join("a.txt"), b"alpha").unwrap();
        fs::write(src.join("nested/deep/b.txt"), b"beta").unwrap();

        let dst = root.join("dst");
        clone_directory(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(dst.join("nested/deep/b.txt")).unwrap(), b"beta");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_clone_is_independent_of_its_source() {
        // Copy-on-write must behave like a copy, not a link: writing to one side
        // must not be visible from the other. Two agents in two worktrees
        // installing packages concurrently depends on this.
        let root = sandbox("independent");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("shared.txt"), b"original").unwrap();

        let dst = root.join("dst");
        clone_directory(&src, &dst).unwrap();

        fs::write(dst.join("shared.txt"), b"modified").unwrap();
        assert_eq!(fs::read(src.join("shared.txt")).unwrap(), b"original");

        fs::write(src.join("new.txt"), b"added").unwrap();
        assert!(!dst.join("new.txt").exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refuses_an_existing_destination() {
        // Merging into a half-provisioned directory would produce a tree that
        // looks complete but is not.
        let root = sandbox("exists");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        let err = clone_directory(&src, &dst).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_source_is_an_error() {
        let root = sandbox("missing");
        let err = clone_directory(&root.join("nope"), &root.join("dst")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_symlinked_source_directory_is_handled() {
        // Some checkouts symlink node_modules to a shared store.
        let root = sandbox("symlink-src");
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("f.txt"), b"content").unwrap();

        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let dst = root.join("dst");
        clone_directory(&link, &dst).unwrap();
        assert_eq!(fs::read(dst.join("f.txt")).unwrap(), b"content");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn on_macos_a_local_clone_actually_uses_copy_on_write() {
        // If this ever regresses to FullCopy on APFS, provisioning gets ~10x
        // slower with no other visible symptom, so assert the method itself.
        if !cfg!(target_os = "macos") {
            return;
        }

        let root = sandbox("method");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f.txt"), b"x").unwrap();

        let method = clone_directory(&src, &root.join("dst")).unwrap();
        assert_eq!(method, CloneMethod::CopyOnWrite);

        fs::remove_dir_all(&root).ok();
    }
}
