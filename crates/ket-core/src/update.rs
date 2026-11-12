//! Whether a newer ket is out: the channel's manifest, read and compared.
//!
//! This only reports. It reads two fields of the manifest, the version to
//! show and the build number to compare, and acts on nothing else in it: no
//! URL it names is opened (the download link is always [`DOWNLOAD_URL`]) and
//! nothing is downloaded. That is why the manifest's signature is not checked
//! here. The in-app updater that will download what the manifest names
//! verifies the signature first, before it trusts anything in it.
//!
//! Fetched with the system `curl`, HTTPS only, the way [`crate::pack`]
//! fetches a pack: ket links no TLS stack of its own for this.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::build_info::{self, Channel};
use crate::config::UpdateCheck;
use crate::{KetError, Result};

/// Where a newer ket is downloaded from. Never a URL from the manifest.
pub const DOWNLOAD_URL: &str = "https://ketapp.dev/download/";

/// The channel's manifest, or `None` for a dev build, which never checks.
pub fn manifest_url(channel: Channel) -> Option<&'static str> {
    match channel {
        Channel::Dev => None,
        Channel::Beta => Some("https://ketapp.dev/releases/beta.json"),
        Channel::Stable => Some("https://ketapp.dev/releases/stable.json"),
    }
}

/// A release, as much of one as this check reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// What to call it: `0.2.1`.
    pub version: String,
    /// What it is compared by — see [`build_info::BUILD_NUMBER`].
    pub build: u64,
}

/// The manifest file: the manifest and its signature, which this check
/// leaves to the updater (see the module docs). Unknown fields are ignored,
/// so a later release can add some without breaking this one.
#[derive(Deserialize)]
struct Signed {
    manifest: Release,
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The channel has nothing newer than this build.
    UpToDate,
    /// The channel has a newer build.
    Available(Release),
}

/// Reads a manifest file's version and build number.
pub fn parse(bytes: &[u8]) -> Result<Release> {
    serde_json::from_slice::<Signed>(bytes)
        .map(|signed| signed.manifest)
        .map_err(|e| KetError::Config(format!("the release manifest is unreadable: {e}")))
}

/// Compares the channel's latest release with this build.
pub fn status(latest: Release) -> Status {
    if latest.build > build_info::BUILD_NUMBER {
        Status::Available(latest)
    } else {
        Status::UpToDate
    }
}

/// Asks this build's channel whether there is a newer ket. Blocks for as long
/// as `curl` takes, twenty seconds at most, so run it off the window's thread.
pub fn check() -> Result<Status> {
    let Some(url) = manifest_url(build_info::CHANNEL) else {
        return Err(KetError::Config(
            "a dev build does not check for updates".to_owned(),
        ));
    };
    // Argv, never a shell. `--proto` and `--proto-redir` keep the request and
    // any redirect on HTTPS; `-f` makes an error status a failure rather than
    // a body to parse.
    let output = Command::new("/usr/bin/curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "20",
            url,
        ])
        .output()
        .map_err(|e| KetError::Config(format!("could not run curl: {e}")))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(KetError::Config(format!(
            "fetching {url} failed: {}",
            reason.trim()
        )));
    }
    parse(&output.stdout).map(status)
}

/// How long to wait before the next scheduled check, or `None` when there is
/// none to schedule: a dev build, [`UpdateCheck::Never`], or
/// [`UpdateCheck::OnLaunch`] once this launch has checked.
///
/// `last_ms` is when ket last checked, whether or not that check got an
/// answer, so a failed one waits for the next scheduled time rather than
/// retrying at once.
pub fn next_check(
    every: UpdateCheck,
    last_ms: Option<u64>,
    now_ms: u64,
    checked_this_launch: bool,
) -> Option<Duration> {
    const DAY_MS: u64 = 24 * 60 * 60 * 1000;
    if build_info::CHANNEL == Channel::Dev {
        return None;
    }
    let interval = match every {
        UpdateCheck::Never => return None,
        UpdateCheck::OnLaunch => return (!checked_this_launch).then_some(Duration::ZERO),
        UpdateCheck::Daily => DAY_MS,
        UpdateCheck::Weekly => 7 * DAY_MS,
    };
    let due = last_ms.map_or(0, |last| last.saturating_add(interval));
    Some(Duration::from_millis(due.saturating_sub(now_ms)))
}

/// What ket remembers between launches: when it last heard from the channel,
/// what it heard, and what it has already told the person.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// When a check last got an answer, in milliseconds since the epoch.
    pub checked_at_ms: Option<u64>,
    /// The newer release that answer named, if it named one.
    pub available: Option<Release>,
    /// The build ket last said was available, so it says so once.
    pub announced: u64,
}

impl Record {
    /// The record, or an empty one when there is none or it is unreadable.
    pub fn load() -> Self {
        let Ok(path) = record_path() else {
            return Self::default();
        };
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), %e, "update record unreadable; starting over");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the record, atomically.
    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string(self)
            .map_err(|e| KetError::Config(format!("could not encode the update record: {e}")))?;
        crate::config::write_atomically(&record_path()?, &text, false)
    }

    /// The newer release known, until this build is replaced by it.
    pub fn newer(&self) -> Option<&Release> {
        self.available
            .as_ref()
            .filter(|release| release.build > build_info::BUILD_NUMBER)
    }
}

/// `<cache_dir>/update.json`: regenerable, so the cache rather than the data
/// directory. Losing it costs one check and, at worst, one repeated notice.
fn record_path() -> Result<PathBuf> {
    Ok(crate::paths::cache_dir()?.join("update.json"))
}
