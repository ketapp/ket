//! Tracing setup.
//!
//! Called once from a binary. Library code never initialises logging — it only
//! emits through `tracing` macros and lets the binary decide where that goes.

use tracing_subscriber::EnvFilter;

/// Initialises tracing, reading its filter from `KET_LOG`.
///
/// Falls back to `default_directive` when the variable is unset or unparseable.
///
/// Diagnostics go to **stderr**, deliberately: stdout carries data — the JSON
/// lines from `ket events`, diffs from `ket diff` — and must stay pipeable.
///
/// ```no_run
/// ket_core::logging::init("info");
/// ```
pub fn init(default_directive: &str) {
    let filter =
        EnvFilter::try_from_env("KET_LOG").unwrap_or_else(|_| EnvFilter::new(default_directive));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();
}
