//! The ket host: one process per data directory that owns the terminals, so
//! they outlive the windows that show them (Epic 5b.1).
//!
//! Without it, a terminal lives inside `ket.app` and dies with it — quitting,
//! crashing or rebuilding the app ends every agent session in every project.
//! With it, the app is a client. It asks the host to start a terminal, and
//! draws a copy of it that the host keeps current with checkpoints and output
//! (see `crate::terminal::checkpoint`). When the app goes, the terminal keeps
//! running, and the next window to restore that tab takes it over.
//!
//! One host per data directory, and its socket lives inside that directory.
//! That is what keeps a sandboxed `XDG_DATA_HOME` — AGENTS.md's first rule —
//! from ever reaching the owner's terminals: it gets a host of its own.
//!
//! Hosting is the default: terminal survival cannot depend on remembering an
//! environment switch before launching the app. `KET_HOST=0` is the explicit
//! escape hatch for development that needs an in-process terminal. A host
//! failure fails a terminal open instead of silently creating one that dies
//! with the window.
//!
//! The host is not a separate program. It is whichever ket binary started it
//! — the app or the CLI — run again with [`SERVE_ARG`]. Both binaries check
//! for it first thing in `main`; see [`serve_if_asked`].

use std::path::PathBuf;
use std::time::Duration;

use crate::{KetError, Result, paths};

pub(crate) mod client;
pub mod identity;
mod phones;
mod server;
pub mod wire;

/// The argument that turns a ket binary into the host.
pub const SERVE_ARG: &str = "--ket-host-serve";

/// The environment variable that can opt windows out of the host with `0`.
pub const ENABLE_ENV: &str = "KET_HOST";

/// The environment variable naming the relay the host dials for phones, as a
/// `ws://` or `wss://` URL. Unset, the host serves no phones.
pub const RELAY_ENV: &str = "KET_RELAY_URL";

/// The relay the host should dial, if one is configured.
pub fn relay_url() -> Option<String> {
    std::env::var(RELAY_ENV).ok().filter(|url| !url.is_empty())
}

/// The environment variable naming where the phone app is served as a web
/// page (`http://<this Mac>:8081` from its dev server). Set, the pairing QR
/// code opens that page from a phone's Camera app with the code filled in;
/// unset, the QR code is the bare `ket://pair/…` code, which only the native
/// app can open.
pub const PHONE_WEB_ENV: &str = "KET_PHONE_WEB_URL";

/// What the pairing QR code should say for `code`: a link to the phone's web
/// app that pairs with it, when [`PHONE_WEB_ENV`] names one, else the code.
pub fn pairing_link(code: &str) -> String {
    match std::env::var(PHONE_WEB_ENV) {
        Ok(web) if !web.is_empty() => {
            // Only `:` and `/` in a code are not plain URL characters; the
            // rest is base64url.
            let escaped = code.replace(':', "%3A").replace('/', "%2F");
            format!("{}/pair?code={escaped}", web.trim_end_matches('/'))
        }
        _ => code.to_owned(),
    }
}

/// The name a device on the same network reaches this desktop by, such as
/// `Example-MacBook.local`. Asks the system, so it can take a moment.
pub fn lan_name() -> String {
    phones::lan_host()
}

/// A one-time pairing code from the running host, for a phone to scan. Fails
/// when no host is running, or it has no relay.
pub fn pairing_code() -> Result<String> {
    match client::peek()? {
        Some((stream, _, _)) => client::pairing_code(stream),
        None => Err(host_error("the ket host is not running")),
    }
}

/// When a code from [`pairing_code`] stops working, in Unix seconds. `None`
/// for text that is not a pairing code.
pub fn pairing_expires_at(code: &str) -> Option<u64> {
    ket_remote::Offer::from_code(code)
        .ok()
        .map(|offer| offer.expires_at)
}

/// Whether the running host serves phones — whether it was started with a
/// relay. `None` when no host is running.
pub fn serves_phones() -> Result<Option<bool>> {
    let Some((stream, _, _)) = client::peek()? else {
        return Ok(None);
    };
    client::ask(stream, &wire::Request::Devices, |reply| match reply {
        wire::Reply::Devices { devices } => Some(devices.is_some()),
        _ => None,
    })
    .map(Some)
}

/// How long the host lingers with no terminal running and no window
/// connected, before exiting.
///
/// Exiting when idle is what keeps an old build from running forever: the
/// next window starts a host from its own binary.
pub const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);

/// How long a terminal whose program has exited is kept, final screen and
/// all, for a window to come back to.
pub const EXITED_KEPT: Duration = Duration::from_secs(15 * 60);

/// Whether windows should use the host. On unless explicitly disabled — see
/// the module docs.
pub fn enabled() -> bool {
    std::env::var(ENABLE_ENV).map_or(true, |value| value != "0")
}

/// The directory holding the host's socket, lock and log.
///
/// Its own directory, created owner-only before anything is put in it, so
/// there is never a moment when the socket exists with looser permissions.
pub fn dir() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join("host"))
}

/// Longest socket path the platform takes, less a margin: `sun_path` is 104
/// bytes on macOS and 108 on Linux, terminator included.
const SOCKET_PATH_MAX: usize = 100;

/// Where the host listens.
///
/// Inside [`dir`] when the path fits. A deep data directory — a sandbox under
/// a long temporary path, say — makes it too long to bind, and then it goes in
/// the per-user temporary directory instead, under a name derived from the
/// data directory: still one socket per data directory, just not inside it.
pub fn socket_path() -> Result<PathBuf> {
    let dir = dir()?;
    let inside = dir.join("host.sock");
    if inside.as_os_str().len() < SOCKET_PATH_MAX {
        return Ok(inside);
    }
    use std::os::unix::ffi::OsStrExt;
    Ok(std::env::temp_dir()
        .join(format!("ket-host-{:016x}", fnv(dir.as_os_str().as_bytes())))
        .join("host.sock"))
}

/// FNV-1a: small, and the same in every build, which a hash that names a
/// socket two builds must agree on has to be. `DefaultHasher` promises
/// neither.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Which build of ket this is: the executable and when it was written.
///
/// Not a version number, because the case that matters is a developer
/// rebuilding the same version all day. A host whose build differs from a window's is still
/// spoken to while terminals are running in it — the protocol is what has to
/// match — and asked to make way once it is idle.
pub fn build_id() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let written = std::fs::metadata(&exe)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs());
    format!("{}@{written}", exe.display())
}

/// Runs as the host and exits, if this process was started as one.
///
/// Call it first thing in `main`. Returns only when this process is not the
/// host.
pub fn serve_if_asked() {
    if std::env::args().nth(1).as_deref() != Some(SERVE_ARG) {
        return;
    }
    crate::logging::init("info");
    reset_process();
    let code = match server::run() {
        Ok(()) => 0,
        Err(error) => {
            tracing::error!(%error, "ket host failed");
            1
        }
    };
    std::process::exit(code);
}

/// Undoes what the host's launcher may have left it with, before serving.
///
/// Signals it inherited blocked are unblocked, so the handler the server
/// installs for `SIGTERM` can run — see `server::watch_signals`, which is
/// also what overrides an inherited *ignore*.
///
/// And the open-file limit: a host started under launchd's default of 256
/// runs out at a few dozen terminals, each holding the pty three times over
/// (the master, a reader and a writer), and then cannot open another.
fn reset_process() {
    use nix::sys::signal::{SigSet, SigmaskHow, sigprocmask};
    let _ = sigprocmask(SigmaskHow::SIG_SETMASK, Some(&SigSet::empty()), None);

    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    // macOS refuses a soft limit above `OPEN_MAX` even when the hard limit
    // is unlimited.
    const MOST: u64 = 10_240;
    if let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) {
        let wanted = hard.min(MOST);
        if soft < wanted {
            match setrlimit(Resource::RLIMIT_NOFILE, wanted, hard) {
                Ok(()) => tracing::info!(from = soft, to = wanted, "raised the open-file limit"),
                Err(error) => {
                    tracing::warn!(%error, soft, wanted, "could not raise the open-file limit");
                }
            }
        }
    }
}

pub use wire::{DeviceEntry, DeviceRole};

/// The phones paired with this data directory's host.
///
/// Asked of the running host, which also knows which are connected; read
/// from the identity files when no host is running.
pub fn devices() -> Result<Vec<DeviceEntry>> {
    if let Some((stream, _, _)) = client::peek()?
        && let Some(devices) = client::ask(stream, &wire::Request::Devices, |reply| match reply {
            wire::Reply::Devices { devices } => Some(devices),
            _ => None,
        })?
    {
        return Ok(devices);
    }
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let roles = identity::load_roles().unwrap_or_default();
    Ok(identity::load()?
        .map(|host| {
            host.grants()
                .iter()
                .map(|grant| DeviceEntry {
                    id: b64.encode(&grant.device),
                    name: grant.name.clone(),
                    paired_at: grant.paired_at,
                    connected: false,
                    role: roles.get(&grant.device).copied(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Turns phones on or off in this data directory's host, in place: no
/// restart, and no terminal ends. Turning them on starts a host if none is
/// running — it reads the saved switch as it starts, so the caller saves
/// [`crate::config::PhonesConfig::enabled`] first.
pub fn set_phones(enabled: bool) -> Result<()> {
    let stream = match client::peek()? {
        Some((stream, _, _)) => stream,
        None if enabled => {
            client::connection()?;
            match client::peek()? {
                Some((stream, _, _)) => stream,
                None => return Err(host_error("the host did not start")),
            }
        }
        // No host, and phones off: nothing is serving them.
        None => return Ok(()),
    };
    let error = client::ask(
        stream,
        &wire::Request::Phones { enabled },
        |reply| match reply {
            wire::Reply::Phones { error, .. } => Some(error),
            _ => None,
        },
    )?;
    error.map_or(Ok(()), |error| Err(host_error(error)))
}

pub use wire::PendingPairing;

/// Phones that have paired and are waiting for a person to approve them;
/// none when no host is running.
pub fn pairings() -> Result<(Vec<PendingPairing>, bool)> {
    let Some((stream, _, _)) = client::peek()? else {
        return Ok((Vec::new(), false));
    };
    client::ask(stream, &wire::Request::Pairings, |reply| match reply {
        wire::Reply::Pairings {
            pairings,
            scoped_roles,
        } => Some((pairings, scoped_roles)),
        _ => None,
    })
}

/// Approves or declines a waiting pairing. `false` if it is no longer
/// waiting.
pub fn decide_pairing(id: u64, role: Option<DeviceRole>, scoped_roles: bool) -> Result<bool> {
    let Some((stream, _, _)) = client::peek()? else {
        return Ok(false);
    };
    let request = if scoped_roles {
        wire::Request::DecidePairingRole { id, role }
    } else {
        wire::Request::DecidePairing {
            id,
            approve: role.is_some(),
        }
    };
    client::ask(stream, &request, |reply| match reply {
        wire::Reply::PairingDecided { found } => Some(found),
        _ => None,
    })
}

/// Stops accepting a phone, and ends its session if it has one. `false` if
/// no phone by that id is paired.
pub fn revoke(id: &str) -> Result<bool> {
    if let Some((stream, _, _)) = client::peek()? {
        let request = wire::Request::Revoke { id: id.to_owned() };
        let (found, error) = client::ask(stream, &request, |reply| match reply {
            wire::Reply::Revoked { found, error } => Some((found, error)),
            _ => None,
        })?;
        // A host with no phone side leaves the files to be edited directly.
        if error.is_none() {
            return Ok(found);
        }
    }
    use base64::Engine;
    let Ok(device) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(id.trim()) else {
        return Ok(false);
    };
    let Some(mut host) = identity::load()? else {
        return Ok(false);
    };
    let found = host.revoke(&device);
    if found {
        identity::save(&host)?;
    }
    Ok(found)
}

/// What a running host says about itself.
#[derive(Debug, Clone)]
pub struct Status {
    /// See [`build_id`].
    pub build: String,
    /// Terminals running in it.
    pub live: usize,
}

/// The running host's status, or `None` when no host is listening.
///
/// Never starts one.
pub fn status() -> Result<Option<Status>> {
    Ok(client::peek()?.map(|(_, build, live)| Status { build, live }))
}

/// Asks the running host to exit, which it does only if no terminal is
/// running in it — or, with `force`, after ending every terminal it runs.
/// Returns whether a host was there to ask; an error if it was asked and is
/// still running.
///
/// Through the socket first, not a signal: a sandboxed shell can drop
/// signals to processes outside it and still report them sent, and a host
/// started from one may have been left ignoring them. But the answer is the
/// host's exit, not the request having been written: a busy host has been
/// seen to take the request and keep running. With `force`, a host still up
/// after [`STOP_WAIT`] is sent `SIGTERM`, then `SIGKILL`.
pub fn stop(force: bool) -> Result<bool> {
    let Some((stream, _, _)) = client::peek()? else {
        return Ok(false);
    };
    let pid = client::peer_pid(&stream);
    client::shutdown(stream.try_clone().map_err(host_error)?, force)?;
    if client::exits(&stream, STOP_WAIT) {
        return Ok(true);
    }
    if force && let Some(pid) = pid {
        tracing::warn!(
            pid,
            "ket host: asked to stop and still running; signalling it"
        );
        // A killed host leaves its socket file. Not removed here: a window
        // may already have started the next host at that path, and a stale
        // file is only a connection refused.
        if client::end_process(pid, STOP_WAIT) {
            return Ok(true);
        }
    }
    Err(host_error(match pid {
        Some(pid) => format!("asked to stop, but still running (pid {pid})"),
        None => "asked to stop, but still running".to_owned(),
    }))
}

/// How long [`stop`] gives the host to exit before it does more, or gives up.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// Hook reports the host has passed on since the last call.
///
/// From agents the host is running, which report to it rather than to the
/// window that launched them. A window drains these beside its own
/// listener's; see [`crate::agent_hooks::Listener::drain`].
pub fn take_hook_reports() -> Vec<crate::agent_hooks::HookReport> {
    std::mem::take(
        &mut *client::HOOK_REPORTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// New work a phone asked for: a worktree in `project`, its agent started on
/// `prompt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneWork {
    /// The project's id.
    pub project: String,
    /// What the agent is to do.
    pub prompt: String,
    /// Which agent, by name; `None` for the project's own.
    pub agent: Option<String>,
    /// The backlog note it is, when a phone started one — see
    /// `wire::Reply::StartWork`.
    pub note: Option<String>,
}

/// The work phones have asked this window to start since the last call. The
/// host sends each request to one window only.
pub fn take_phone_work() -> Vec<PhoneWork> {
    std::mem::take(
        &mut *client::PHONE_WORK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// The oldest worktree a phone asked this window to merge. Later requests
/// remain queued until the current desktop confirmation is resolved.
pub fn take_phone_merge() -> Option<String> {
    let mut merges = client::PHONE_MERGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (!merges.is_empty()).then(|| merges.remove(0))
}

/// The host's newest agent status snapshot, if one has arrived since the last
/// call. See [`crate::agent_status`]: for the terminals the host runs, this
/// is the status, and a window reapplying the raw hooks itself would be a
/// second authority drifting from the first.
pub fn take_agent_status() -> Option<crate::agent_status::AgentStatusSnapshot> {
    client::AGENT_STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .1
        .take()
}

/// Tells the host where this window's worktrees are, so it can place a Codex
/// report by its working directory. Kept and sent again on every new
/// connection; sent now only if the list changed and a host is connected.
pub fn publish_worktrees(worktrees: Vec<(String, PathBuf)>) {
    {
        let mut published = client::WORKTREES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *published == worktrees {
            return;
        }
        published.clone_from(&worktrees);
    }
    if let Some(conn) = client::current() {
        let _ = conn.request(&wire::Request::Worktrees { worktrees });
    }
}

/// Tells the host the agents' plan usage, for phones — see
/// [`wire::Request::Usage`]. Kept and sent again on every new connection;
/// sent now only if it changed and a host is connected.
pub fn publish_usage(providers: Vec<crate::rate_limits::ProviderSnapshot>) {
    {
        let mut published = client::USAGE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *published == providers {
            return;
        }
        published.clone_from(&providers);
    }
    if let Some(conn) = client::current() {
        let _ = conn.request(&wire::Request::Usage { providers });
    }
}

/// Tells the host which agents this window can start, for phones — see
/// [`wire::Request::Agents`]. Kept and sent again on every new connection;
/// sent now only if it changed and a host is connected.
pub fn publish_agents(names: Vec<String>) {
    {
        let mut published = client::AGENTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *published == names {
            return;
        }
        published.clone_from(&names);
    }
    if let Some(conn) = client::current() {
        let _ = conn.request(&wire::Request::Agents { names });
    }
}

/// Tells the host the theme this window is drawn in, for phones — see
/// [`wire::Request::Theme`]. Kept and sent again on every new connection;
/// sent now only if it changed and a host is connected.
pub fn publish_theme(theme: crate::theme::Theme) {
    {
        let mut published = client::THEME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *published == Some(theme) {
            return;
        }
        *published = Some(theme);
    }
    if let Some(conn) = client::current() {
        let _ = conn.request(&wire::Request::Theme {
            theme: Box::new(theme),
        });
    }
}

/// Tells the host an interrupt was typed into one of its terminals: `cancel`
/// for `Ctrl-C`, a latched verdict; otherwise Escape's guess. Returns whether
/// a host was connected to hear it.
pub fn interrupt(pane: &str, cancel: bool) -> bool {
    client::current().is_some_and(|conn| {
        conn.request(&wire::Request::Interrupt {
            pane: pane.to_owned(),
            cancel,
        })
        .is_ok()
    })
}

/// Status lines the host has passed on since the last call. See
/// [`take_hook_reports`].
pub fn take_status_lines() -> Vec<crate::agent_hooks::StatusLine> {
    std::mem::take(
        &mut *client::STATUS_LINES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Builds an error about the host, for a person to read.
fn host_error(why: impl std::fmt::Display) -> KetError {
    KetError::io(
        dir().unwrap_or_default(),
        std::io::Error::other(format!("ket host: {why}")),
    )
}
