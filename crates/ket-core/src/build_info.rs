//! Which release of ket this is: version, channel, commit, build number.
//!
//! The version is the workspace's, from `Cargo.toml`. Everything else is
//! stamped in by the release script through the environment at build time —
//! `KET_CHANNEL`, `KET_COMMIT`, `KET_BUILD_NUMBER`, `KET_BUILD_DATE` — and is
//! absent from an everyday `cargo run`, which is a dev build.
//!
//! Read with `option_env!` rather than computed by a build script on purpose:
//! cargo tracks those variables, so changing one rebuilds, and nothing else
//! does. A build script asking git for the commit would rebuild ket-core, and
//! with it ket-ui, on every checkpoint ket commits.
//!
//! Not to be confused with [`crate::host::build_id`], which tells two builds
//! of the same version apart so a rebuilt window can replace an idle host.

/// The release this build belongs to: the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Which stream of releases a build comes from, and so which it updates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Built from a checkout. Never updates itself.
    Dev,
    /// Releases ahead of stable, for people who asked for them.
    Beta,
    /// Releases.
    Stable,
}

impl Channel {
    /// What it is called in `KET_CHANNEL` and in what ket shows.
    pub const fn name(self) -> &'static str {
        match self {
            Channel::Dev => "dev",
            Channel::Beta => "beta",
            Channel::Stable => "stable",
        }
    }
}

/// This build's channel: `KET_CHANNEL`, or [`Channel::Dev`] without one. Any
/// other value fails the build rather than shipping as a channel nothing
/// serves.
pub const CHANNEL: Channel = match option_env!("KET_CHANNEL") {
    None => Channel::Dev,
    Some(name) => match name.as_bytes() {
        b"dev" => Channel::Dev,
        b"beta" => Channel::Beta,
        b"stable" => Channel::Stable,
        _ => panic!("KET_CHANNEL must be dev, beta or stable"),
    },
};

/// The short commit a release was cut from, when the release script said.
pub const COMMIT: Option<&str> = option_env!("KET_COMMIT");

/// The day a release was built, `YYYY-MM-DD`, when the release script said.
pub const BUILD_DATE: Option<&str> = option_env!("KET_BUILD_DATE");

/// A number that only goes up from one release to the next — what an update
/// is compared by, and macOS's `CFBundleVersion`. Zero for a dev build.
pub const BUILD_NUMBER: u64 = match option_env!("KET_BUILD_NUMBER") {
    None => 0,
    Some(digits) => parse_build_number(digits.as_bytes()),
};

const fn parse_build_number(digits: &[u8]) -> u64 {
    assert!(!digits.is_empty(), "KET_BUILD_NUMBER is empty");
    let mut number: u64 = 0;
    let mut at = 0;
    while at < digits.len() {
        let digit = digits[at];
        assert!(
            digit.is_ascii_digit(),
            "KET_BUILD_NUMBER must be decimal digits"
        );
        number = number * 10 + (digit - b'0') as u64;
        at += 1;
    }
    number
}

/// How ket names this build to a person: `0.4.2 (a1b2c3d)`, `0.4.2 beta
/// (a1b2c3d)`, or `0.4.2-dev`.
pub fn display() -> String {
    let mut shown = match CHANNEL {
        Channel::Dev => format!("{VERSION}-dev"),
        Channel::Beta => format!("{VERSION} beta"),
        Channel::Stable => VERSION.to_owned(),
    };
    if let Some(commit) = COMMIT {
        shown.push_str(&format!(" ({commit})"));
    }
    shown
}
