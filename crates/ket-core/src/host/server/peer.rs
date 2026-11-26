//! Who is on the other end of the host's socket: a ket window, or some other
//! program the same user runs — see [`same_program`].

use std::os::unix::net::UnixStream;

/// Whether the process on the other end of `stream` is running this very
/// executable: a ket window, which is where a person approves a pairing and
/// turns phones on and off.
///
/// The socket's mode keeps other users out; this is what keeps out other
/// programs the same user runs. The peer's process id comes from the socket
/// (`LOCAL_PEERPID`) and its executable's path from the kernel, not from
/// anything the peer says about itself. A program that can take over a ket
/// window — inject code into an unhardened build — is past this; that is the
/// operating system's to stop.
#[cfg(target_os = "macos")]
pub(super) fn same_program(stream: &UnixStream) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::LocalPeerPid};
    let Ok(pid) = getsockopt(stream, LocalPeerPid) else {
        return false;
    };
    let Ok(peer) = libproc::proc_pid::pidpath(pid) else {
        return false;
    };
    let canonical = |path: &std::path::Path| path.canonicalize().ok();
    let mine = std::env::current_exe().ok().and_then(|exe| canonical(&exe));
    mine.is_some() && canonical(std::path::Path::new(&peer)) == mine
}

/// Linux exposes the peer PID and UID on Unix sockets. Resolve that PID through
/// `/proc` and compare the executable, failing closed if either check fails.
#[cfg(target_os = "linux")]
pub(super) fn same_program(stream: &UnixStream) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};

    let Ok(credentials) = getsockopt(stream, PeerCredentials) else {
        return false;
    };
    let Ok(peer) = std::fs::read_link(format!("/proc/{}/exe", credentials.pid())) else {
        return false;
    };
    let canonical = |path: &std::path::Path| path.canonicalize().ok();
    let mine = std::env::current_exe().ok().and_then(|exe| canonical(&exe));
    mine.is_some() && canonical(&peer) == mine
}

/// Platforms without a native peer executable identity fail closed.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn same_program(_stream: &UnixStream) -> bool {
    false
}
