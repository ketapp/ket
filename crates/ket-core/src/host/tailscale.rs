//! Whether Tailscale is on this Mac, and if not what is in the way: what
//! Settings › Devices says under "Away from home" (#88).
//!
//! A tailnet address on the Mac's interfaces is the answer when there is
//! one. Without one, Tailscale's own command-line tool says why — signed
//! out, or turned off — and no tool at all means it is not installed. The
//! tool is asked with a short deadline, for a few hundred bytes about this
//! Mac only.

use std::io::Read;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where Tailscale is, and how it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tailscale {
    /// No Tailscale app or command-line tool on this Mac.
    NotInstalled,
    /// Installed, and not connected: turned off, starting, or not answering.
    Off,
    /// Installed and running, waiting for someone to sign in or for the
    /// tailnet's admin to approve this Mac.
    SignedOut,
    /// Connected, at this address on the tailnet.
    On(Ipv4Addr),
}

/// How long the command-line tool has to answer.
const DEADLINE: Duration = Duration::from_secs(2);

/// Where the command-line tool is: inside the app from Tailscale's site or
/// the App Store, or on its own from Homebrew.
const TOOLS: &[&str] = &[
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/opt/homebrew/bin/tailscale",
    "/usr/local/bin/tailscale",
];

/// This Mac's Tailscale, now. Asks the system, so it can take a moment.
pub fn state() -> Tailscale {
    if let Some(ip) = super::tailnet_ip() {
        return Tailscale::On(ip);
    }
    let tools: Vec<PathBuf> = TOOLS.iter().map(PathBuf::from).collect();
    let Some(tool) = tools.iter().find(|path| path.exists()) else {
        return Tailscale::NotInstalled;
    };
    match ask(tool) {
        Some(status) => from_status(&status),
        // Installed, and its service is not there to answer.
        None => Tailscale::Off,
    }
}

/// What `tailscale status --json` says, as a state.
pub fn from_status(json: &str) -> Tailscale {
    let Ok(status) = serde_json::from_str::<serde_json::Value>(json) else {
        return Tailscale::Off;
    };
    match status.get("BackendState").and_then(|state| state.as_str()) {
        Some("Running") => status
            .get("TailscaleIPs")
            .and_then(|ips| ips.as_array())
            .into_iter()
            .flatten()
            .filter_map(|ip| ip.as_str()?.parse::<Ipv4Addr>().ok())
            .next()
            .map_or(Tailscale::Off, Tailscale::On),
        Some("NeedsLogin" | "NeedsMachineAuth") => Tailscale::SignedOut,
        _ => Tailscale::Off,
    }
}

/// The tool's status for this Mac alone, or `None` when it fails or does not
/// answer by [`DEADLINE`].
fn ask(tool: &Path) -> Option<String> {
    let mut child = Command::new(tool)
        .args(["status", "--json", "--peers=false"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read as it writes, so a long answer cannot fill the pipe and stall it.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).ok()?;
        Some(out)
    });
    let deadline = Instant::now() + DEADLINE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // Not waited for: anything the tool started may still hold
                // the pipe, and the reader ends when that lets go.
                return None;
            }
        }
    };
    // A tool that is installed but whose service is stopped exits non-zero
    // and still says nothing useful.
    if !status.success() {
        return None;
    }
    reader.join().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_tailscale_is_on_at_its_ipv4_address() {
        let json =
            r#"{"BackendState":"Running","TailscaleIPs":["fd7a:115c:a1e0::1","100.101.102.103"]}"#;
        assert_eq!(
            from_status(json),
            Tailscale::On(Ipv4Addr::new(100, 101, 102, 103))
        );
    }

    #[test]
    fn one_waiting_to_sign_in_or_be_approved_is_signed_out() {
        for state in ["NeedsLogin", "NeedsMachineAuth"] {
            let json = format!(r#"{{"BackendState":"{state}","TailscaleIPs":[]}}"#);
            assert_eq!(from_status(&json), Tailscale::SignedOut, "{state}");
        }
    }

    #[test]
    fn anything_else_is_off() {
        for json in [
            r#"{"BackendState":"Stopped"}"#,
            r#"{"BackendState":"Starting"}"#,
            r#"{"BackendState":"NoState"}"#,
            r#"{"BackendState":"Running","TailscaleIPs":["fd7a:115c:a1e0::1"]}"#,
            "failed to connect to local tailscaled",
        ] {
            assert_eq!(from_status(json), Tailscale::Off, "{json}");
        }
    }

    #[test]
    fn a_tool_that_answers_is_read_and_one_that_hangs_is_cut_off() {
        let dir = std::env::temp_dir().join(format!("ket-tailscale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let answers = write("answers", r#"echo '{"BackendState":"NeedsLogin"}'"#);
        assert_eq!(
            ask(&answers).map(|out| from_status(&out)),
            Some(Tailscale::SignedOut)
        );
        let fails = write("fails", "echo 'failed to connect'; exit 1");
        assert_eq!(ask(&fails), None);
        let hangs = write("hangs", "sleep 30");
        let started = Instant::now();
        assert_eq!(ask(&hangs), None);
        assert!(started.elapsed() < DEADLINE + Duration::from_secs(2));
        std::fs::remove_dir_all(&dir).ok();
    }
}
