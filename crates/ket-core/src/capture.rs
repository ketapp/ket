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
