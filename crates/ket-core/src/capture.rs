//! Screenshots of a device tab's screen, and recordings of a browser tab,
//! kept where an agent can read them.
//!
//! A screenshot goes to an agent as a file the agent opens itself — pasted
//! into its terminal as a path, which Claude Code, Codex and the rest turn
//! into an attached image. So it has to outlive the paste: it is written to
//! ket's cache rather than a temporary file, and kept until enough newer ones
//! have been taken to push it out. A recording is kept the same way, its
//! video and the stills beside it going together.
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

/// How many recordings are kept, each with its stills; the oldest go as new
/// ones are made. Fewer than screenshots: a video runs to megabytes.
const RECORDINGS_KEPT: usize = 20;

/// What every recording's files start with: `recording-<ms>-<n>.mp4`, and
/// its stills `recording-<ms>-<n>-<k>.png`.
const RECORDING_PREFIX: &str = "recording-";

/// How recent a recording is spared by pruning however many came after it,
/// in milliseconds: one started this recently may still be being written.
/// Longer than the longest recording `ket browser record` allows, ten
/// minutes, and the time to finish its file.
const RECORDING_SPARED_MS: u64 = 15 * 60 * 1000;

/// A path for a new screenshot, its folder made: where `simctl` can write one.
pub fn new_screenshot() -> Result<PathBuf> {
    let dir = crate::paths::cache_dir()?.join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
    prune(&dir, PREFIX, KEPT, u64::MAX);
    static TAKEN: AtomicU32 = AtomicU32::new(0);
    let taken = TAKEN.fetch_add(1, Ordering::Relaxed);
    Ok(dir.join(format!("{PREFIX}{}-{taken}.png", crate::now_ms())))
}

/// A path for a new recording's MP4, its folder made, with room for it: past
/// the newest [`RECORDINGS_KEPT`], the oldest recordings go, stills and all.
/// Its stills go beside it, named for it.
pub fn new_recording() -> Result<PathBuf> {
    let dir = crate::paths::cache_dir()?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
    let spared = crate::now_ms().saturating_sub(RECORDING_SPARED_MS);
    prune(&dir, RECORDING_PREFIX, RECORDINGS_KEPT, spared);
    static MADE: AtomicU32 = AtomicU32::new(0);
    let made = MADE.fetch_add(1, Ordering::Relaxed);
    Ok(dir.join(format!("{RECORDING_PREFIX}{}-{made}.mp4", crate::now_ms())))
}

/// Writes a PNG somebody already has the bytes of — the Android emulator's —
/// and says where.
pub fn save_screenshot(png: &[u8]) -> Result<PathBuf> {
    let path = new_screenshot()?;
    std::fs::write(&path, png).map_err(|e| KetError::io(&path, e))?;
    Ok(path)
}

/// Removes the oldest captures in `dir` past `kept`, making room for one
/// more. A capture is every file named `<prefix><ms>-<n>…`: one for a
/// screenshot, a video and its stills for a recording. One taken at or after
/// `spared_from` (milliseconds since the epoch) stays whatever its age in
/// the order: a recording that may still be being written. Best effort: a
/// file that will not go is left for next time.
fn prune(dir: &Path, prefix: &str, kept: usize, spared_from: u64) {
    let kept = kept.max(1);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let files: Vec<((u64, u64), PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let mut parts = name.to_str()?.strip_prefix(prefix)?.split('-');
            let taken = parts.next()?.parse::<u64>().ok()?;
            let n = parts
                .next()
                .and_then(|n| n.split('.').next()?.parse::<u64>().ok())
                .unwrap_or(0);
            Some(((taken, n), entry.path()))
        })
        .collect();
    let mut captures: Vec<(u64, u64)> = files.iter().map(|(capture, _)| *capture).collect();
    captures.sort_unstable();
    captures.dedup();
    if captures.len() < kept {
        return;
    }
    // Everything from the oldest capture up to the newest of those going.
    let newest_going = captures[captures.len() - kept];
    for (capture, path) in files {
        if capture <= newest_going && capture.0 < spared_from {
            let _ = std::fs::remove_file(path);
        }
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
        prune(&d.0, PREFIX, KEPT, u64::MAX);
        assert_eq!(count(&d.0), KEPT - 1);
    }

    #[test]
    fn at_the_limit_the_oldest_go_to_make_room_for_one_more() {
        let d = dir("over");
        for taken in 0..(KEPT as u64 + 5) {
            shot(&d.0, 1_000 + taken);
        }
        std::fs::write(d.0.join("notes.txt"), b"not a screenshot").unwrap();
        prune(&d.0, PREFIX, KEPT, u64::MAX);
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
    fn a_recording_goes_with_its_stills_when_it_is_among_the_oldest() {
        let d = dir("recordings");
        let recording = |taken: u64, stills: u32| {
            let stem = format!("{RECORDING_PREFIX}{taken}-3");
            std::fs::write(d.0.join(format!("{stem}.mp4")), b"mp4").unwrap();
            for still in 1..=stills {
                std::fs::write(d.0.join(format!("{stem}-{still}.png")), b"png").unwrap();
            }
        };
        for taken in 0..(RECORDINGS_KEPT as u64 + 2) {
            recording(1_000 + taken, 6);
        }
        std::fs::write(d.0.join("notes.txt"), b"not a recording").unwrap();
        prune(&d.0, RECORDING_PREFIX, RECORDINGS_KEPT, u64::MAX);
        // Room for the recording about to be made: three went, whole.
        assert_eq!(count(&d.0), (RECORDINGS_KEPT - 1) * 7 + 1);
        for gone in 1_000..1_003 {
            assert!(
                !d.0.join(format!("{RECORDING_PREFIX}{gone}-3.mp4")).exists(),
                "{gone}"
            );
            assert!(
                !d.0.join(format!("{RECORDING_PREFIX}{gone}-3-6.png"))
                    .exists(),
                "{gone}"
            );
        }
        assert!(d.0.join(format!("{RECORDING_PREFIX}1003-3-1.png")).exists());
    }

    #[test]
    fn a_recording_still_being_made_is_spared_however_many_follow_it() {
        let d = dir("recordings-spared");
        // A long one started first, still being written, then a run of short
        // ones after it that would push it out.
        std::fs::write(d.0.join(format!("{RECORDING_PREFIX}5000-0.mp4")), b"mp4").unwrap();
        for taken in 0..(RECORDINGS_KEPT as u64 + 2) {
            let name = format!("{RECORDING_PREFIX}{}-1.mp4", 6_000 + taken);
            std::fs::write(d.0.join(name), b"mp4").unwrap();
        }
        prune(&d.0, RECORDING_PREFIX, RECORDINGS_KEPT, 5_000);
        assert!(d.0.join(format!("{RECORDING_PREFIX}5000-0.mp4")).exists());
    }

    #[test]
    fn a_missing_directory_is_nothing_to_prune() {
        prune(
            Path::new("/nonexistent/ket/screenshots"),
            PREFIX,
            KEPT,
            u64::MAX,
        );
    }
}
