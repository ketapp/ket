//! CLI output can contain credentials. Only allowlisted names and status
//! labels leave this module; neither stdout nor stderr enters an error.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Maximum bytes read from either CLI stream.
const OUTPUT_LIMIT: u64 = 256 * 1024;
/// Maximum duration of one CLI check.
const CHECK_LIMIT: Duration = Duration::from_secs(12);

/// One configured MCP server and the CLI's safe summary of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// Server name, sanitized for display.
    pub name: String,
    /// A status selected from ket's fixed vocabulary.
    pub status: &'static str,
}

/// One agent's check result.
#[derive(Debug, Clone)]
pub enum Check {
    /// The CLI returned a readable list, possibly empty.
    Servers(Vec<Server>),
    /// A safe, actionable failure description.
    Error(&'static str),
}

/// Results of the two independent CLI checks.
#[derive(Debug, Clone)]
pub struct Report {
    /// Claude Code's configured servers and reported connection state.
    pub claude: Check,
    /// Codex's configured servers and authentication state.
    pub codex: Check,
}

/// Checks both agents from the selected checkout. Call only for a user action.
pub fn check(worktree: &Path) -> Report {
    Report {
        claude: run(
            "claude",
            &["mcp", "list"],
            worktree,
            CHECK_LIMIT,
            parse_claude,
        ),
        codex: run(
            "codex",
            &["mcp", "list", "--json"],
            worktree,
            CHECK_LIMIT,
            parse_codex,
        ),
    }
}

fn run(
    program: &str,
    args: &[&str],
    worktree: &Path,
    limit: Duration,
    parse: fn(&str) -> Result<Vec<Server>, &'static str>,
) -> Check {
    let mut child = match Command::new(program)
        .args(args)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Check::Error("CLI not found on PATH. Install it, then check again.");
        }
        Err(_) => return Check::Error("Could not start the CLI in this worktree."),
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (out_send, out_recv) = std::sync::mpsc::channel();
    let (err_send, err_recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = out_send.send(read_bounded(stdout));
    });
    std::thread::spawn(move || {
        let _ = err_send.send(read_bounded(stderr));
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => break None,
        }
    };
    if status.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    if status.is_none() {
        return Check::Error("Check timed out. Try the command in a terminal for details.");
    }
    let stdout = out_recv
        .recv_timeout(Duration::from_millis(100))
        .ok()
        .flatten();
    let stderr = err_recv
        .recv_timeout(Duration::from_millis(100))
        .ok()
        .flatten();
    if stdout.is_none() || stderr.is_none() {
        return Check::Error("CLI output exceeded the size limit or could not be read.");
    }
    if !status.is_some_and(|status| status.success()) {
        return Check::Error("CLI check failed. Run the command in a terminal for details.");
    }
    let Some(stdout) = stdout.and_then(|bytes| String::from_utf8(bytes).ok()) else {
        return Check::Error("CLI returned unreadable output.");
    };
    match parse(&stdout) {
        Ok(servers) => Check::Servers(servers),
        Err(error) => Check::Error(error),
    }
}

fn read_bounded(pipe: impl Read) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= OUTPUT_LIMIT).then_some(bytes)
}

fn safe_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()
        && name.len() <= 100
        && name.chars().all(|c| {
            !c.is_control() && !matches!(c, ':' | '/' | '\\' | '?' | '#' | '=' | '&' | '@' | '%')
        }))
    .then(|| name.to_owned())
}

fn parse_claude(output: &str) -> Result<Vec<Server>, &'static str> {
    let mut servers = Vec::new();
    for line in output.lines() {
        let Some((name, detail)) = line.split_once(':') else {
            continue;
        };
        let Some(name) = safe_name(name) else {
            continue;
        };
        let status = if detail.contains("Needs authentication") {
            "Needs authentication"
        } else if detail.contains("Pending approval") {
            "Pending approval"
        } else if detail.contains("Disabled") {
            "Disabled"
        } else if detail.contains("Connected") {
            "Connected when checked"
        } else if detail.contains("Failed to connect") {
            "Failed to connect"
        } else if detail.contains("not configured") {
            "Not configured"
        } else {
            continue;
        };
        servers.push(Server { name, status });
    }
    if servers.is_empty()
        && !output.contains("No MCP servers configured")
        && !output.trim().is_empty()
    {
        return Err("Could not read Claude's server list. Run claude mcp list in a terminal.");
    }
    Ok(servers)
}

fn parse_codex(output: &str) -> Result<Vec<Server>, &'static str> {
    let entries: serde_json::Value = serde_json::from_str(output).map_err(
        |_| "Could not read Codex's server list. Run codex mcp list --json in a terminal.",
    )?;
    let entries = entries
        .as_array()
        .ok_or("Codex returned an unexpected server list.")?;
    let mut servers = Vec::new();
    for entry in entries {
        let Some(name) = entry
            .get("name")
            .and_then(|name| name.as_str())
            .and_then(safe_name)
        else {
            continue;
        };
        let status = if entry.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            "Disabled"
        } else {
            match entry.get("auth_status").and_then(|v| v.as_str()) {
                Some("not_logged_in") => "Needs authentication",
                Some("logged_in") => "Authenticated in CLI",
                Some("unknown") => "Authentication unknown",
                _ => "Configured",
            }
        };
        servers.push(Server { name, status });
    }
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_keeps_only_names_and_fixed_statuses() {
        let parsed = parse_claude("Checking MCP server health…\nclaude.ai Claude Docs: https://api.anthropic.com/v1/pages/mcp - ✔ Connected\ndb: ! Needs authentication\noff: ⊘ Disabled for this project\nbad: ✘ Failed to connect: token=secret\n").unwrap();
        assert_eq!(parsed[0].name, "claude.ai Claude Docs");
        assert_eq!(
            parsed.iter().map(|s| s.status).collect::<Vec<_>>(),
            [
                "Connected when checked",
                "Needs authentication",
                "Disabled",
                "Failed to connect"
            ]
        );
        assert!(!format!("{parsed:?}").contains("secret"));
    }

    #[test]
    fn codex_discards_raw_configuration_and_secrets() {
        let parsed = parse_codex(r#"[{"name":"my server","enabled":true,"auth_status":"not_logged_in","transport":{"env":{"TOKEN":"secret"}}},{"name":"off","enabled":false,"env":{"KEY":"secret"}}]"#).unwrap();
        assert_eq!(parsed[0].name, "my server");
        assert_eq!(parsed[0].status, "Needs authentication");
        assert_eq!(parsed[1].status, "Disabled");
        assert!(!format!("{parsed:?}").contains("secret"));
        let error = parse_codex("secret malformed").unwrap_err();
        assert!(!error.contains("secret"));
    }

    #[test]
    fn names_allow_unicode_but_not_url_or_secret_delimiters() {
        assert_eq!(safe_name("  Café 文档  ").as_deref(), Some("Café 文档"));
        for name in [
            "token=secret",
            "https://example",
            "name@example",
            "x\\y",
            "x\ny",
            "x?y",
            "x#y",
            "x&y",
            "x%y",
        ] {
            assert!(safe_name(name).is_none(), "{name}");
        }
    }

    #[test]
    fn empty_and_malformed_lists() {
        assert!(parse_codex("[]").unwrap().is_empty());
        assert!(
            parse_claude("No MCP servers configured.")
                .unwrap()
                .is_empty()
        );
        assert!(parse_claude("secret unexpected output").is_err());
    }

    #[test]
    fn missing_cli_and_timeout_have_safe_errors() {
        let cwd = std::env::current_dir().unwrap();
        assert!(matches!(
            run(
                "ket-mcp-nonexistent-cli",
                &[],
                &cwd,
                CHECK_LIMIT,
                parse_claude
            ),
            Check::Error("CLI not found on PATH. Install it, then check again.")
        ));
        assert!(matches!(
            run(
                "sh",
                &["-c", "exec sleep 2"],
                &cwd,
                Duration::from_millis(10),
                parse_claude
            ),
            Check::Error("Check timed out. Try the command in a terminal for details.")
        ));
    }

    #[test]
    fn command_uses_selected_worktree_as_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.display().to_string();
        let script = format!(
            "test \"$PWD\" = '{}' && printf '[]'",
            expected.replace('\'', "'\\''")
        );
        assert!(
            matches!(run("sh", &["-c", &script], &cwd, CHECK_LIMIT, parse_codex), Check::Servers(servers) if servers.is_empty())
        );
    }

    #[test]
    fn failed_cli_never_echoes_stderr() {
        let cwd = std::env::current_dir().unwrap();
        let result = run(
            "sh",
            &["-c", "printf 'token=secret' >&2; exit 1"],
            &cwd,
            CHECK_LIMIT,
            parse_codex,
        );
        assert!(matches!(result, Check::Error(message) if !message.contains("secret")));
    }

    #[test]
    fn observed_server_survives_absence_from_current_configuration() {
        let mut history = crate::usage::History::load_from(
            std::env::temp_dir().join(format!("ket-mcp-usage-test-{}", std::process::id())),
        );
        let worktree = crate::id::WorktreeId::new("selected");
        let other = crate::id::WorktreeId::new("other");
        history.note_tool_call("one", &worktree, 0, Some("retired"), 1);
        history.note_tool_call("two", &worktree, 0, Some("retired"), 2);
        history.note_tool_call("three", &other, 0, Some("other-server"), 3);
        history.note_tool_call("four", &worktree, 0, None, 4);
        assert_eq!(
            history.mcp_calls_for_worktree(&worktree).get("retired"),
            Some(&2)
        );
        assert_eq!(history.mcp_calls_for_worktree(&worktree).len(), 1);
        assert_eq!(
            history.mcp_calls_for_worktree(&other).get("other-server"),
            Some(&1)
        );
        assert!(parse_codex("[]").unwrap().is_empty());
    }
}
