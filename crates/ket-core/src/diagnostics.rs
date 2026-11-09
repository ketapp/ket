//! Diagnostics for a bug report: one zip a person attaches to an issue or
//! emails to [`SUPPORT_EMAIL`].
//!
//! What goes in is what someone answering the report would otherwise ask
//! for: ket's version and the Mac's, `ket doctor`, the resolved config, the
//! sidebar's state, the host's and Chromium's logs, and ket's recent crash
//! reports. Every file passes through [`redact`] on its way in, which takes
//! out credentials, tokens and pairing codes and writes the home directory
//! as `~`.
//!
//! What never goes in, redacted or not: the phone identity and device list
//! (`remote/`), the network inspector's captures, which hold whole requests
//! headers and all, usage history, and any other application's files.
//!
//! The zip is made by the system's own tool — `ditto` on a Mac, `zip`
//! elsewhere — the way [`crate::feedback`] reaches the network through
//! `curl`, rather than by an archive library linked for this one file.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::{Captures, Regex};

use crate::{KetError, Result, build_info, paths};

/// Where a bundle can be emailed.
pub const SUPPORT_EMAIL: &str = "hello@ketapp.dev";

/// What a bundle is called unless the person names it.
pub const DEFAULT_NAME: &str = "ket-diagnostics.zip";

/// The folder inside the zip, so unzipping it never scatters files.
const FOLDER: &str = "ket-diagnostics";

/// How much of the end of each log goes in. The recent end is the part a
/// report is about, and a log a week old can run to many megabytes.
const LOG_TAIL: u64 = 2 * 1024 * 1024;

/// The most crash reports that go in, newest first.
const MAX_CRASHES: usize = 10;

/// How old a crash report may be and still go in.
const CRASH_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What a removed value is replaced with.
const REDACTED: &str = "[redacted]";

/// Builds a bundle and writes it to `dest`, replacing whatever is there.
///
/// Blocks for as long as reading the logs and zipping take — a second or
/// two — so call it off the UI thread. Anything that cannot be read is noted
/// in the bundle's README rather than failing the export: a bundle with a
/// missing log is still worth sending.
pub fn export(dest: &Path) -> Result<()> {
    let stage = std::env::temp_dir().join(format!(
        "ket-diagnostics-{}-{}",
        std::process::id(),
        crate::now_ms()
    ));
    let result = stage_bundle(&stage.join(FOLDER)).and_then(|()| zip(&stage, dest));
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Writes every file of the bundle into `dir`.
fn stage_bundle(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir.join("logs")).map_err(io_error)?;
    let home = paths::home().ok();
    let mut bundle = Bundle {
        dir,
        home: home.as_deref(),
        missing: Vec::new(),
    };

    bundle.text("system.txt", Ok(system()));
    bundle.text("doctor.txt", crate::doctor::run().map(|report| report.text));
    bundle.text("config.toml", config_toml());
    bundle.text(
        "state.json",
        paths::data_dir().and_then(|dir| read(&dir.join("state.json"))),
    );

    if let Ok(host) = crate::host::dir() {
        bundle.log("logs/host.log", &host.join("host.log"));
    }
    if let Ok(roots) = paths::chromium_roots() {
        for root in roots {
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("chromium");
            let log = root.join("chromium.log");
            if log.is_file() {
                bundle.log(&format!("logs/{name}.log"), &log);
            }
        }
    }
    if let Some(home) = home.as_deref() {
        let crashes = crash_reports(&home.join("Library/Logs/DiagnosticReports"));
        if !crashes.is_empty() {
            std::fs::create_dir_all(dir.join("crashes")).map_err(io_error)?;
        }
        for path in crashes {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                bundle.text(&format!("crashes/{name}"), read(&path));
            }
        }
    }

    // Redacted too: an error can quote the line it failed on, and a config
    // that will not parse is often the reason for the report.
    let readme = redact(&readme(&bundle.missing), bundle.home);
    std::fs::write(dir.join("README.txt"), readme).map_err(io_error)
}

/// The resolved config as TOML, with what may be secret taken out by where
/// it sits before [`redact`] looks for it by name: an agent's environment is
/// whatever its owner chose to put there, keys and connection strings
/// included, and the telemetry headers are usually an `Authorization` bearer.
/// The names stay; the values go.
fn config_toml() -> Result<String> {
    let mut config = crate::config::Config::load()?;
    for agent in &mut config.agents {
        for value in agent.env.values_mut() {
            *value = REDACTED.to_owned();
        }
    }
    if let Some(headers) = config.telemetry.headers.as_mut() {
        *headers = REDACTED.to_owned();
    }
    toml::to_string_pretty(&config).map_err(|e| KetError::Config(e.to_string()))
}

/// The bundle being written, and what could not go in.
struct Bundle<'a> {
    dir: &'a Path,
    home: Option<&'a Path>,
    missing: Vec<String>,
}

impl Bundle<'_> {
    /// Writes `content`, redacted, as `name` — or notes why it is missing.
    fn text(&mut self, name: &str, content: Result<String>) {
        let written = content.and_then(|text| {
            std::fs::write(self.dir.join(name), redact(&text, self.home)).map_err(io_error)
        });
        if let Err(e) = written {
            self.missing.push(format!("{name}: {e}"));
        }
    }

    /// Writes the end of the log at `path` as `name`.
    fn log(&mut self, name: &str, path: &Path) {
        self.text(name, tail(path, LOG_TAIL));
    }
}

/// What the bundle holds, for whoever opens it — the person sending it
/// first of all.
fn readme(missing: &[String]) -> String {
    let mut text = format!(
        "ket diagnostics\n\
         \n\
         Attach this zip to an issue at https://github.com/ketapp/ket/issues\n\
         or email it to {SUPPORT_EMAIL} with what went wrong.\n\
         \n\
         system.txt   ket's version, macOS's, and this Mac's model\n\
         doctor.txt   what `ket doctor` reports\n\
         config.toml  ket's settings as it resolved them\n\
         state.json   your projects and worktrees as the sidebar shows them\n\
         logs/        the most recent part of ket's logs\n\
         crashes/     ket's crash reports from the last 30 days, if any\n\
         \n\
         Passwords, API keys, tokens and phone pairing codes were replaced with\n\
         {REDACTED}, and your home folder is written as ~. Project and worktree\n\
         names and paths are kept, because most bugs are about them; read the\n\
         files before sending if any of them are private.\n\
         \n\
         Not included: the keys that pair phones, the network inspector's\n\
         captures, usage history, and other applications' files.\n"
    );
    if !missing.is_empty() {
        text.push_str("\nCould not be included:\n");
        for line in missing {
            text.push_str(&format!("  {line}\n"));
        }
    }
    text
}

/// ket's version, the OS's, and the machine's.
fn system() -> String {
    let mut lines = vec![
        format!("ket          {}", build_info::display()),
        format!("channel      {}", build_info::CHANNEL.name()),
    ];
    if let Some(date) = build_info::BUILD_DATE {
        lines.push(format!("built        {date}"));
    }
    lines.push(format!("os           {}", crate::feedback::system()));
    #[cfg(target_os = "macos")]
    {
        if let Some(build) = output("sw_vers", &["-buildVersion"]) {
            lines.push(format!("os build     {build}"));
        }
        if let Some(model) = output("sysctl", &["-n", "hw.model"]) {
            lines.push(format!("model        {model}"));
        }
        if let Some(bytes) =
            output("sysctl", &["-n", "hw.memsize"]).and_then(|bytes| bytes.parse::<u64>().ok())
        {
            lines.push(format!("memory       {} GB", bytes / (1024 * 1024 * 1024)));
        }
    }
    if let Some(git) = output("git", &["--version"]) {
        lines.push(format!("git          {git}"));
    }
    if let Ok(shell) = std::env::var("SHELL") {
        lines.push(format!("shell        {shell}"));
    }
    lines.push(format!("exported     {} (Unix ms)", crate::now_ms()));
    lines.join("\n") + "\n"
}

/// The trimmed stdout of `program args`, when it ran and said something.
fn output(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// ket's crash reports in `dir`: at most [`MAX_CRASHES`], none older than
/// [`CRASH_AGE`], ket's own processes before Chromium's helpers and newest
/// first within each.
///
/// A report is named for its process — `ket-ui-…`, `ket Helper (Renderer)-…`
/// for Chromium's helpers — so ket's are the ones whose name is `ket` and
/// then something other than a letter: not another app that starts with it.
/// A helper crashing takes one browser tab with it and tends to come in
/// bursts; the window or the host crashing is the report that matters, and a
/// burst of helpers must not push it out.
fn crash_reports(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(is_ket_crash_report))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            let age = now.duration_since(modified).unwrap_or_default();
            (age <= CRASH_AGE).then(|| (modified, entry.path()))
        })
        .collect();
    let helper = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("ket Helper"))
    };
    found.sort_by(|a, b| helper(&a.1).cmp(&helper(&b.1)).then(b.0.cmp(&a.0)));
    found
        .into_iter()
        .take(MAX_CRASHES)
        .map(|(_, path)| path)
        .collect()
}

/// Whether `name` is the file name of one of ket's crash reports.
fn is_ket_crash_report(name: &str) -> bool {
    let report = name.ends_with(".ips") || name.ends_with(".crash");
    let ours = name
        .strip_prefix("ket")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|next| !next.is_alphanumeric());
    report && ours
}

/// Reads a whole text file, lossily.
fn read(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| file_error(path, e))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The last `max` bytes of the file at `path`, from the first whole line.
fn tail(path: &Path, max: u64) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| file_error(path, e))?;
    let len = file.metadata().map_err(|e| file_error(path, e))?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| file_error(path, e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| file_error(path, e))?;
    if start > 0
        && let Some(newline) = bytes.iter().position(|&b| b == b'\n')
    {
        bytes.drain(..=newline);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Zips the folder inside `stage` into `dest`.
fn zip(stage: &Path, dest: &Path) -> Result<()> {
    // Written beside the destination and moved over it, so a failed export
    // leaves no half a zip under the name the person chose.
    let partial = dest.with_extension("zip.partial");
    let _ = std::fs::remove_file(&partial);
    #[cfg(target_os = "macos")]
    let status = Command::new("ditto")
        .args([
            "-c",
            "-k",
            "--norsrc",
            "--noextattr",
            "--noqtn",
            "--keepParent",
        ])
        .arg(stage.join(FOLDER))
        .arg(&partial)
        .status();
    #[cfg(not(target_os = "macos"))]
    let status = Command::new("zip")
        .args(["-q", "-r"])
        .arg(&partial)
        .arg(FOLDER)
        .current_dir(stage)
        .status();
    match status {
        Ok(status) if status.success() => {
            std::fs::rename(&partial, dest).map_err(|e| file_error(dest, e))
        }
        Ok(status) => {
            let _ = std::fs::remove_file(&partial);
            Err(KetError::Config(format!(
                "could not make the zip ({status})"
            )))
        }
        Err(e) => Err(KetError::Config(format!("could not make the zip: {e}"))),
    }
}

/// Removes what must not leave the machine from `text`, and writes `home`
/// as `~`.
///
/// Deliberately broad: a value under any key that names a key, token, secret,
/// password or cookie goes, along with anything shaped like a known
/// provider's token wherever it appears. A report missing an innocent value
/// costs a question; one carrying a live token costs the person far more.
pub fn redact(text: &str, home: Option<&Path>) -> String {
    static ANSI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("valid regex"));
    // A pairing code is a whole `ket://pair/…` link, and a way in to this
    // Mac until it expires.
    static PAIRING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"ket://pair/[^\s"'<>]+"#).expect("valid regex"));
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}").expect("valid regex")
    });
    // Anthropic, OpenAI, GitHub, Slack and AWS keys, and JWTs.
    static SHAPED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\b(sk-ant-[\w-]{8,}|sk-[\w-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_\w{20,}|xox[abprs]-[\w-]{10,}|AKIA[0-9A-Z]{16}|eyJ[\w-]{8,}\.[\w-]{8,}\.[\w-]{8,})",
        )
        .expect("valid regex")
    });
    // `api_key = "…"`, `"token": "…"`, `password=…`: the key's name decides.
    // A name with `secret`, `password` or `private` anywhere in it counts —
    // `AWS_SECRET_ACCESS_KEY` — while the other words must end it, so
    // `tokens = 5` and `token_limit` stay. A TOML multi-line string is taken
    // whole, so a PEM key does not survive past its opening quotes.
    static KEYED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)(["']?\b[\w.-]*(?:(?:api|access|signing|session)[_-]?key|apikey|token|credentials?|cookie|authorization|auth|passwd|(?:secret|password|private)[\w-]*)["']?\s*[:=]\s*)("""[\s\S]*?"""|'''[\s\S]*?'''|"[^"]*"|'[^']*'|[^\s,;&}\]]+)"#,
        )
        .expect("valid regex")
    });
    static QUERY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)([?&](?:code|key|sig|signature|state)=)[^&\s"'#]+"#).expect("valid regex")
    });

    let text = ANSI.replace_all(text, "");
    let text = PAIRING.replace_all(&text, format!("ket://pair/{REDACTED}"));
    let text = SCHEME.replace_all(&text, format!("$1 {REDACTED}"));
    let text = SHAPED.replace_all(&text, REDACTED);
    let text = KEYED.replace_all(&text, |caps: &Captures<'_>| {
        let value = &caps[2];
        let quote = [r#"""""#, "'''", "\"", "'"]
            .into_iter()
            .find(|q| value.len() >= 2 * q.len() && value.starts_with(q))
            .unwrap_or("");
        if value.len() <= 2 * quote.len() {
            // Empty: nothing to hide, and saying so helps the reader.
            caps[0].to_owned()
        } else {
            format!("{}{quote}{REDACTED}{quote}", &caps[1])
        }
    });
    let text = QUERY.replace_all(&text, format!("${{1}}{REDACTED}"));
    match home.and_then(Path::to_str).filter(|home| home.len() > 1) {
        Some(home) => text.replace(home, "~"),
        None => text.into_owned(),
    }
}

fn file_error(path: &Path, source: std::io::Error) -> KetError {
    KetError::Io {
        path: path.to_owned(),
        source,
    }
}

fn io_error(e: std::io::Error) -> KetError {
    KetError::Config(format!("could not write the bundle: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_codes_and_tokens_are_removed() {
        // The tokens are put together here so the source holds nothing
        // shaped like one for a secret scanner to stop on.
        let text = format!(
            "pair with ket://pair/AbC123_-xyz to connect\n\
             Authorization: Bearer abcdefghijklmnop\n\
             key {}api03-abcdefghijklmnop and {}abcdefghijklmnopqrstuvwxyz\n",
            "sk-ant-", "ghp_"
        );
        let out = redact(&text, None);
        assert!(!out.contains("AbC123"), "{out}");
        assert!(!out.contains("abcdefghijklmnop"), "{out}");
        assert!(!out.contains("ghp_"), "{out}");
        assert!(out.contains("ket://pair/[redacted]"), "{out}");
    }

    #[test]
    fn values_under_secret_keys_are_removed_and_others_kept() {
        let text = "\
            api_key = \"hunter2hunter2\"\n\
            \"token\": \"abc.def\"\n\
            password=swordfish other=1\n\
            tool_output_token_limit = 4000\n\
            cold_resume_min_tokens = 100000\n\
            tokenReduction = 2\n\
            api_key = \"\"\n";
        let out = redact(text, None);
        assert!(out.contains("api_key = \"[redacted]\""), "{out}");
        assert!(out.contains("\"token\": \"[redacted]\""), "{out}");
        assert!(out.contains("password=[redacted] other=1"), "{out}");
        assert!(out.contains("tool_output_token_limit = 4000"), "{out}");
        assert!(out.contains("cold_resume_min_tokens = 100000"), "{out}");
        assert!(out.contains("tokenReduction = 2"), "{out}");
        assert!(out.contains("api_key = \"\""), "{out}");
    }

    #[test]
    fn names_with_secret_inside_and_multi_line_values_are_removed() {
        let text = "\
            AWS_SECRET_ACCESS_KEY = \"wJalrXUtnFEMIexample\"\n\
            PRIVATE_KEY = \"\"\"\n-----BEGIN KEY-----\nMIIEvQIBADAN\n-----END KEY-----\"\"\"\n\
            after = 1\n";
        let out = redact(text, None);
        assert!(
            out.contains("AWS_SECRET_ACCESS_KEY = \"[redacted]\""),
            "{out}"
        );
        assert!(
            out.contains("PRIVATE_KEY = \"\"\"[redacted]\"\"\"\nafter = 1"),
            "{out}"
        );
        assert!(!out.contains("MIIEvQIBADAN"), "{out}");
    }

    #[test]
    fn the_readme_is_redacted_with_the_errors_it_quotes() {
        let missing =
            ["config.toml: config: session_cookie = \"live-cookie\" unexpected".to_owned()];
        let out = redact(&readme(&missing), None);
        assert!(!out.contains("live-cookie"), "{out}");
    }

    #[test]
    fn url_secrets_colour_codes_and_home_go() {
        let text =
            "\x1b[2mGET\x1b[0m https://example.com/cb?code=s3cr3t&page=2 in /home/someone/repo\n";
        let out = redact(text, Some(Path::new("/home/someone")));
        assert_eq!(
            out,
            "GET https://example.com/cb?code=[redacted]&page=2 in ~/repo\n"
        );
    }

    #[test]
    fn only_kets_crash_reports_are_picked() {
        assert!(is_ket_crash_report("ket-ui-2026-10-29-203428.ips"));
        assert!(is_ket_crash_report(
            "ket Helper (Renderer)-2026-10-30-144122.ips"
        ));
        assert!(is_ket_crash_report("ket-host-2026-10-30.crash"));
        assert!(!is_ket_crash_report("ketchup-2026-10-30.ips"));
        assert!(!is_ket_crash_report("ket-ui-2026-10-30.diag"));
        assert!(!is_ket_crash_report("Finder-2026-10-30.ips"));
    }

    #[test]
    fn a_tail_starts_on_a_whole_line() {
        let dir = std::env::temp_dir().join(format!("ket-diag-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log");
        std::fs::write(&path, "first line\nsecond line\nthird\n").unwrap();
        assert_eq!(tail(&path, 14).unwrap(), "third\n");
        assert_eq!(
            tail(&path, 1000).unwrap(),
            "first line\nsecond line\nthird\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
