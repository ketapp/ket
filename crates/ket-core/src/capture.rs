//! Screenshots of a device tab's screen, kept where an agent can read them.
//!
//! A screenshot goes to an agent as a file the agent opens itself — pasted
//! into its terminal as a path, which Claude Code, Codex and the rest turn
//! into an attached image. So it has to outlive the paste: it is written to
//! ket's cache rather than a temporary file, and kept until enough newer ones
//! have been taken to push it out.
//!
//! The path is absolute and has no spaces: an agent takes a pasted path as an
//! image only when the whole paste is that one path.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{KetError, Result};

/// How many screenshots are kept; the oldest go as new ones are taken.
const KEPT: usize = 100;

/// What every screenshot's file name starts with, so pruning touches only
/// these.
const PREFIX: &str = "screenshot-";

/// A path for a new screenshot, its folder made: where `simctl` can write one.
pub fn new_screenshot() -> Result<PathBuf> {
    let dir = crate::paths::cache_dir()?.join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
    prune(&dir);
    static TAKEN: AtomicU32 = AtomicU32::new(0);
    let taken = TAKEN.fetch_add(1, Ordering::Relaxed);
    Ok(dir.join(format!("{PREFIX}{}-{taken}.png", crate::now_ms())))
}

/// Writes a PNG somebody already has the bytes of — the Android emulator's —
/// and says where.
pub fn save_screenshot(png: &[u8]) -> Result<PathBuf> {
    let path = new_screenshot()?;
    std::fs::write(&path, png).map_err(|e| KetError::io(&path, e))?;
    Ok(path)
}

/// Removes the oldest screenshots past [`KEPT`], making room for one more.
/// Best effort: a file that will not go is left for next time.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut shots: Vec<(u64, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let taken = name
                .to_str()?
                .strip_prefix(PREFIX)?
                .split('-')
                .next()?
                .parse::<u64>()
                .ok()?;
            Some((taken, entry.path()))
        })
        .collect();
    if shots.len() < KEPT {
        return;
    }
    shots.sort_unstable_by_key(|(taken, _)| *taken);
    let excess = shots.len() + 1 - KEPT;
    for (_, path) in shots.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(tag: &str) -> Dir {
        let path = std::env::temp_dir().join(format!(
            "ket-capture-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }

    fn shot(dir: &Path, taken: u64) {
        std::fs::write(dir.join(format!("{PREFIX}{taken}-0.png")), b"png").unwrap();
    }

    fn count(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    #[test]
    fn under_the_limit_nothing_is_pruned() {
        let d = dir("under");
        for taken in 0..(KEPT as u64 - 1) {
            shot(&d.0, taken);
        }
        prune(&d.0);
        assert_eq!(count(&d.0), KEPT - 1);
    }

    #[test]
    fn at_the_limit_the_oldest_go_to_make_room_for_one_more() {
        let d = dir("over");
        for taken in 0..(KEPT as u64 + 5) {
            shot(&d.0, 1_000 + taken);
        }
        std::fs::write(d.0.join("notes.txt"), b"not a screenshot").unwrap();
        prune(&d.0);
        // Room for the screenshot about to be taken; other files untouched.
        assert_eq!(count(&d.0), KEPT);
        assert!(d.0.join("notes.txt").exists());
        assert!(
            !d.0.join(format!("{PREFIX}1000-0.png")).exists(),
            "oldest gone"
        );
        assert!(
            d.0.join(format!("{PREFIX}{}-0.png", 1_000 + KEPT as u64 + 4))
                .exists()
        );
    }

    #[test]
    fn a_missing_directory_is_nothing_to_prune() {
        prune(Path::new("/nonexistent/ket/screenshots"));
    }
}
