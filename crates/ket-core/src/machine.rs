//! Facts about this machine that a local model has to fit inside, and that
//! what ket costs is measured against: how much memory it has, how many cores,
//! and how much disk is free where Ollama keeps models.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// How many cores the system runs work on — every one, efficiency cores
/// included, which is what CPU figures counted per core add up to. At least
/// one, however little ket can tell.
pub fn cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, usize::from))
}

/// Total physical memory in bytes, read once — it does not change while ket
/// runs. `None` where ket cannot tell.
pub fn total_memory_bytes() -> Option<u64> {
    static TOTAL: OnceLock<Option<u64>> = OnceLock::new();
    *TOTAL.get_or_init(read_total_memory)
}

#[cfg(target_os = "macos")]
fn read_total_memory() -> Option<u64> {
    let output = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn read_total_memory() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 = meminfo
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_total_memory() -> Option<u64> {
    None
}

/// Where Ollama keeps models: `OLLAMA_MODELS`, or `~/.ollama/models`.
pub fn ollama_models_dir() -> Option<PathBuf> {
    match std::env::var_os("OLLAMA_MODELS").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => crate::paths::home()
            .ok()
            .map(|home| home.join(".ollama").join("models")),
    }
}

/// Bytes free on the volume holding `path`, asked of the nearest folder that
/// exists — Ollama's models folder is only made by the first pull.
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|dir| dir.exists())?;
    free_at(existing)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn free_at(dir: &Path) -> Option<u64> {
    let stats = nix::sys::statvfs::statvfs(dir).ok()?;
    #[allow(clippy::unnecessary_cast)]
    Some(stats.blocks_available() as u64 * stats.fragment_size() as u64)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn free_at(_dir: &Path) -> Option<u64> {
    None
}
