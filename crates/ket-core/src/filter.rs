//! Trimming a successful command's output before the model reads it.
//!
//! A build prints a line per crate it compiles, a test run a line per test
//! that passed, an install a progress bar — and every one of those lines is
//! paid for as fresh input once and as a cache read on every turn after. An
//! Economy level can ask for them to go: Claude Code's `PostToolUse` hook may
//! answer with `updatedToolOutput`, which replaces what the model reads
//! without touching what ran.
//!
//! That last clause is the whole design. The command runs exactly as the
//! agent wrote it, its exit status is exactly what it was, and nothing here
//! is consulted until the tool has returned. Then four gates, in order:
//!
//! 1. the run **succeeded**. Claude Code only sends `PostToolUse` for a
//!    shell command that did; one that exited non-zero fires
//!    `PostToolUseFailure` instead, with no output in it to trim. So the
//!    event is the proof, and `exit_code`, `interrupted` and `truncated` are
//!    checked besides when the payload carries them — the reference
//!    documents all three, and the 2.1 series sends only `interrupted`. A
//!    failed run is shown whole, always: what a failure says is the one
//!    thing a filter must never guess at;
//! 2. the command is one of the few in [`RECIPES`], each of which knows what
//!    its program's noise looks like. Anything else is left alone;
//! 3. the trim is worth it: enough output to matter, cut by enough to matter;
//! 4. when the reader asked for an archive, the **full output is written
//!    away first**, so the trimmed text can name where the rest is and
//!    `ket recall` can bring it back. No archive then, no trim.
//!
//! The archive is off unless `agent.command_archive = true`: the agent's own
//! transcript already holds every tool result whole, so a second copy on
//! disk — command output can carry whatever a command printed, tokens and
//! environment dumps included — is only worth keeping for a reader who wants
//! `ket recall`. The trailer says which: the transcript, or the id and path.
//! An archive that is kept is written owner-only and swept after a day.
//!
//! Every path that is not all four prints nothing, and the hook script turns
//! nothing into `{}` — the same answer it gives when ket is not installed at
//! all. See [`hook_decision`] and the script in [`crate::agent_hooks`].
//!
//! The filter runs as a ket subprocess (`ket --filter-hook`, see
//! [`serve_if_asked`]) rather than as a call to the running window: the
//! model waits on this hook, so it has to finish in milliseconds and has to
//! work whether or not ket is open. A level whose filter quietly stopped
//! when the window closed would be a level that lies.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{KetError, Result};

/// The argv[1] that makes a ket binary answer a hook and exit.
pub const HOOK_ARG: &str = "--filter-hook";

/// The variable a launch sets to the binary the hook script should run,
/// when the session's level asks for its output filtered. Unset, the script
/// never filters — see [`crate::worktree::economy_launch`].
pub const ENV: &str = "KET_COMMAND_FILTER";

/// The variable a launch sets, to anything, when the reader asked for the
/// full output of every trimmed command to be kept for `ket recall` —
/// `agent.command_archive` in config. Unset, nothing is written to disk.
pub const ARCHIVE_ENV: &str = "KET_COMMAND_ARCHIVE";

/// How long the trimmed text may be, in characters.
///
/// Under the 10,000 that Claude Code documents for a hook's other strings
/// before spilling them to a file — a replacement that spilled would hand
/// the model a path in place of the output it asked for.
pub const TRIM_TARGET: usize = 8_000;

/// Output shorter than this is not worth replacing: the trailer naming the
/// archive costs lines of its own.
const MIN_RAW: usize = 1_500;

/// A trim has to cut at least this share of the output to be worth the
/// trailer and the archive.
const MIN_SAVING: f64 = 0.2;

/// Most bytes of hook payload the subprocess will read.
const MAX_STDIN: usize = 2 * 1024 * 1024;

/// How long an archived output is kept: a recall is wanted minutes after
/// the trim, in the same session, and almost never the next day.
const KEEP_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// Lines kept from the end of an over-long output, so a summary printed
/// last survives the head-biased cut.
const TAIL_LINES: usize = 12;

/// Most bytes those tail lines may take between them: a line in the tail
/// can be as long as a line anywhere, and twelve of them must not be the
/// whole budget.
const TAIL_BYTES: usize = TRIM_TARGET / 4;

/// Room the omission marker takes in the cut.
const MARKER_ROOM: usize = 48;

/// Runs as the filter hook and exits, if this process was started as one.
///
/// Call it first thing in `main`, beside [`crate::host::serve_if_asked`].
/// Returns only when this process is not the hook. Prints nothing on any
/// failure: the script treats silence as "leave the output alone".
pub fn serve_if_asked() {
    if std::env::args().nth(1).as_deref() != Some(HOOK_ARG) {
        return;
    }
    use std::io::Read;
    let mut payload = String::new();
    let read = std::io::stdin()
        .take(MAX_STDIN as u64)
        .read_to_string(&mut payload)
        .is_ok();
    if read
        && let Some(decision) = serde_json::from_str(&payload)
            .ok()
            .as_ref()
            .and_then(hook_decision)
    {
        println!("{}", decision.shown_bytes);
        println!("{}", decision.json);
    }
    std::process::exit(0);
}

/// What the hook should answer: how many bytes the model will read, and the
/// JSON that replaces the tool's result with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The replacement's length in bytes, for the record the hook reports.
    pub shown_bytes: u64,
    /// The `hookSpecificOutput` document, one line.
    pub json: String,
}

/// Whether this process was asked to keep what it trims — see [`ARCHIVE_ENV`].
fn archive_asked() -> bool {
    std::env::var_os(ARCHIVE_ENV).is_some_and(|value| !value.is_empty())
}

/// The hook's answer for one `PostToolUse` payload, or `None` to leave the
/// output as it is — see the module notes for the gates.
pub fn hook_decision(payload: &serde_json::Value) -> Option<Decision> {
    let keep = archive_asked().then(|| archive_dir().ok()).flatten();
    hook_decision_with(payload, keep.as_deref())
}

/// [`hook_decision`], told where to keep the full output, if anywhere.
fn hook_decision_with(payload: &serde_json::Value, keep: Option<&Path>) -> Option<Decision> {
    fn text(value: Option<&serde_json::Value>) -> &str {
        value.and_then(|v| v.as_str()).unwrap_or("")
    }
    if text(payload.get("hook_event_name")) != "PostToolUse"
        || text(payload.get("tool_name")) != "Bash"
    {
        return None;
    }
    let input = payload.get("tool_input")?;
    if input.get("run_in_background").and_then(|b| b.as_bool()) == Some(true) {
        return None;
    }
    let response = payload.get("tool_response")?;
    // Refused on what the payload says, never on what it leaves out: a
    // `PostToolUse` is a success by the event's own contract, and a
    // payload without `exit_code` — which is every one Claude Code 2.1
    // sends — is not a failure.
    let flag = |name: &str| response.get(name).and_then(|b| b.as_bool()) == Some(true);
    let failed = response
        .get("exit_code")
        .and_then(|c| c.as_i64())
        .is_some_and(|code| code != 0);
    if failed || flag("interrupted") || flag("truncated") {
        return None;
    }
    let command = text(input.get("command"));
    let stdout = text(response.get("stdout"));
    let stderr = text(response.get("stderr"));
    let id = text(payload.get("tool_use_id"));
    let session = text(payload.get("session_id"));
    if id.is_empty() || session.is_empty() {
        return None;
    }

    let filtered = apply(command, stdout, stderr)?;
    // Archive before answering, when asked to: a trim the reader wanted to
    // be able to undo and cannot is a trim that should not happen.
    let kept = match keep {
        Some(root) => Some(archive_in(root, session, id, stdout, stderr).ok()?),
        None => None,
    };
    let shown = with_trailer(&filtered, id, kept.as_deref());
    // The replacement is the tool's own result shape with the text swapped,
    // not a bare string: for the shell tool Claude Code drops a string here
    // without a word, and only an object like the one it produced —
    // `stdout`, `stderr`, `interrupted` and whatever else it sent — is
    // taken. Everything the trim kept from stderr is already in `stdout`
    // under its own heading, so `stderr` goes out empty.
    let mut replacement = response.clone();
    if let Some(fields) = replacement.as_object_mut() {
        fields.insert(
            "stdout".to_owned(),
            serde_json::Value::String(shown.clone()),
        );
        fields.insert(
            "stderr".to_owned(),
            serde_json::Value::String(String::new()),
        );
    }
    let document = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": replacement,
        }
    });
    Some(Decision {
        shown_bytes: shown.len() as u64,
        json: document.to_string(),
    })
}

/// A command's output, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filtered {
    /// The trimmed text, stdout then stderr, without the trailer.
    pub text: String,
    /// Lines in, counting both streams.
    pub lines_in: usize,
    /// Lines out.
    pub lines_out: usize,
    /// Bytes in, counting both streams.
    pub bytes_in: usize,
}

/// `stdout` and `stderr` of a successful `command`, trimmed by its recipe —
/// or `None` when the command has no recipe, the output is too short to
/// bother with, or the recipe did not take enough off.
///
/// The pure part, shared by the hook and by `ket filter --`.
pub fn apply(command: &str, stdout: &str, stderr: &str) -> Option<Filtered> {
    let recipe = recipe_for(command)?;
    let bytes_in = stdout.len() + stderr.len();
    if bytes_in < MIN_RAW {
        return None;
    }
    let stdout = crate::conversation::strip_ansi(stdout);
    let stderr = crate::conversation::strip_ansi(stderr);
    let lines_in = stdout.lines().count() + stderr.lines().count();

    let mut out = recipe.shape.trim(&stdout);
    let err = recipe.shape.trim(&stderr);
    if !err.trim().is_empty() {
        if !out.trim().is_empty() {
            out.push('\n');
        }
        out.push_str("--- stderr ---\n");
        out.push_str(&err);
    }
    let text = cap(&out);
    let saved = 1.0 - text.len() as f64 / bytes_in as f64;
    if saved < MIN_SAVING {
        return None;
    }
    Some(Filtered {
        lines_out: text.lines().count(),
        text,
        lines_in,
        bytes_in,
    })
}

/// The trimmed text with the line that says what happened to it, and where
/// the rest went: the archive, by id and path, when one was kept — the path
/// as well as the id, since the agent's own file tools can open a path
/// without `ket` on its `PATH` — else the session's own transcript, which
/// holds every tool result whole.
fn with_trailer(filtered: &Filtered, id: &str, archived: Option<&Path>) -> String {
    let per = crate::usage::BYTES_PER_TOKEN.max(1) as usize;
    let rest = match archived {
        Some(path) => format!("full output: ket recall {id} \u{b7} {}", path.display()),
        None => "full output in the session transcript".to_owned(),
    };
    format!(
        "{}\n[ket: {} lines \u{2192} {} \u{b7} \u{2248}{} \u{2192} {} tokens \u{b7} {rest}]",
        filtered.text.trim_end(),
        thousands(filtered.lines_in),
        thousands(filtered.lines_out),
        short(filtered.bytes_in / per),
        short(filtered.text.len() / per),
    )
}

/// A count with thousands separators.
fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A token count at trailer width: `0.4k`, `9.2k`, `120k`.
fn short(count: usize) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=99_999 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{}k", count / 1_000),
    }
}

// ---- the archive --------------------------------------------------------------

/// Where archived outputs live: one directory per session under the cache.
fn archive_dir() -> Result<PathBuf> {
    Ok(crate::paths::cache_dir()?.join("filter"))
}

/// Writes the full output away and returns where.
///
/// Under the cache rather than the data directory: it is derived from a
/// command the agent already ran, deleting it costs nothing, and
/// [`sweep`] does exactly that after a day. Owner-only, directory and
/// file, the way the hook spool is: a command's output is whatever the
/// command printed.
fn archive_in(root: &Path, session: &str, id: &str, stdout: &str, stderr: &str) -> Result<PathBuf> {
    // Whatever writes also prunes, so a reader who only ever runs the CLI
    // gets the same day's retention as one who opens ket.
    sweep_in(root, std::time::SystemTime::now());
    let dir = root.join(safe(session));
    std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
    owner_only(root, 0o700)?;
    owner_only(&dir, 0o700)?;
    let path = dir.join(format!("{}.txt", safe(id)));
    let mut body = String::with_capacity(stdout.len() + stderr.len() + 16);
    body.push_str(stdout);
    if !stderr.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("--- stderr ---\n");
        body.push_str(stderr);
    }
    std::fs::write(&path, body).map_err(|e| KetError::io(&path, e))?;
    owner_only(&path, 0o600)?;
    Ok(path)
}

/// Sets `mode` on `path`, where the platform has modes.
fn owner_only(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| KetError::io(path, e))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// An id as a file name: the characters ids are made of, and nothing that
/// could walk out of the directory.
fn safe(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect()
}

/// The full output archived under `id`, or the one archived id `id` is the
/// end of — a trailer's id is long, and a person retyping it stops early.
pub fn recall(id: &str) -> Result<String> {
    recall_in(&archive_dir()?, id)
}

/// [`recall`] under `root`.
fn recall_in(root: &Path, id: &str) -> Result<String> {
    let wanted = safe(id);
    if wanted.is_empty() {
        return Err(KetError::Config("recall needs an output id".to_owned()));
    }
    let mut found: Option<PathBuf> = None;
    for session in std::fs::read_dir(root).map_err(|e| KetError::io(root, e))? {
        let session = session.map_err(|e| KetError::io(root, e))?.path();
        let Ok(entries) = std::fs::read_dir(&session) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem == wanted || stem.ends_with(&wanted) {
                if found.is_some() {
                    return Err(KetError::Conflict(format!(
                        "more than one archived output ends in {wanted}; give more of the id"
                    )));
                }
                found = Some(path);
            }
        }
    }
    let path = found.ok_or_else(|| {
        KetError::Config(format!(
            "no archived output {wanted}: archives are kept for a day"
        ))
    })?;
    std::fs::read_to_string(&path).map_err(|e| KetError::io(&path, e))
}

/// Removes archives older than [`KEEP_FOR`]. Nothing waits on this; a
/// failure is a directory that is swept next time.
pub fn sweep() {
    if let Ok(root) = archive_dir() {
        sweep_in(&root, std::time::SystemTime::now());
    }
}

/// [`sweep`] under `root`, as of `now`.
fn sweep_in(root: &Path, now: std::time::SystemTime) {
    let Ok(sessions) = std::fs::read_dir(root) else {
        return;
    };
    for session in sessions.flatten() {
        let path = session.path();
        let stale = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > KEEP_FOR);
        if stale {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

// ---- running a command for `ket filter --` ------------------------------------

/// What `ket filter` ran: the exit code to exit with, and what to print on
/// each stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// The command's own exit code, always.
    pub code: i32,
    /// For stdout: the trimmed text with its trailer, or the raw stdout.
    pub out: String,
    /// For stderr: empty when trimmed (it is folded into `out`), else raw.
    pub err: String,
}

/// Runs `line` through the login shell, prints its output trimmed by the
/// same gates the hook applies — or whole, when they do not pass — and
/// returns the exit code to exit with, which is always the command's own.
/// `keep` archives the full output for `ket recall`, as the hook does when
/// the reader asked.
pub fn run(line: &str, keep: bool) -> i32 {
    let keep = keep.then(|| archive_dir().ok()).flatten();
    let ran = run_in(line, keep.as_deref());
    print!("{}", ran.out);
    eprint!("{}", ran.err);
    ran.code
}

/// [`run`] without the printing, archiving under `keep` when given.
fn run_in(line: &str, keep: Option<&Path>) -> Ran {
    let output = std::process::Command::new(crate::shell::login_shell())
        .args(["-lc", line])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Ran {
                code: 127,
                out: String::new(),
                err: format!("ket filter: could not run the shell: {error}\n"),
            };
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(128);
    let raw = || Ran {
        code,
        out: stdout.clone(),
        err: stderr.clone(),
    };
    let Some(filtered) = (code == 0).then(|| apply(line, &stdout, &stderr)).flatten() else {
        return raw();
    };
    let id = format!("cli-{}", crate::now_ms());
    let kept = match keep {
        Some(root) => match archive_in(root, "cli", &id, &stdout, &stderr) {
            Ok(path) => Some(path),
            // Asked to keep and could not: no archive, no trim, the same
            // rule as the hook.
            Err(_) => return raw(),
        },
        None => None,
    };
    Ran {
        code,
        out: format!("{}\n", with_trailer(&filtered, &id, kept.as_deref())),
        err: String::new(),
    }
}

// ---- recipes ------------------------------------------------------------------

/// What a program's noise looks like, and so what to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A compiler driving a build: progress per crate, diagnostics in
    /// blank-line-separated blocks, a summary at the end.
    Build,
    /// `cargo test`: a build, then a line per test that passed.
    CargoTest,
    /// pytest: a line or a dot per test that passed, a ruled summary.
    Pytest,
    /// jest and vitest: a tick per test that passed, `PASS` per file.
    Jest,
    /// `go test`: `=== RUN` and `--- PASS` per test, `ok` per package.
    GoTest,
    /// A package manager installing: progress, then what it added.
    Install,
    /// A listing: files, matches, log lines. Repeats collapse, long lists
    /// are cut.
    Listing,
    /// A diff. Nothing is deduplicated — a repeated `}` is content — and
    /// only the length is bounded.
    Diff,
}

/// One program ket knows how to quieten.
struct Recipe {
    /// The program, as the first word of the command names it.
    program: &'static str,
    /// The subcommands it applies to; empty for every invocation.
    subcommands: &'static [&'static str],
    shape: Shape,
}

/// The vetted table. Short on purpose: a recipe is a claim that ket knows
/// what this program's output means, and each one is checked by hand.
const RECIPES: &[Recipe] = &[
    Recipe {
        program: "cargo",
        subcommands: &["build", "check", "clippy", "doc", "fmt", "run"],
        shape: Shape::Build,
    },
    Recipe {
        program: "cargo",
        subcommands: &["test", "nextest"],
        shape: Shape::CargoTest,
    },
    Recipe {
        program: "pytest",
        subcommands: &[],
        shape: Shape::Pytest,
    },
    Recipe {
        program: "jest",
        subcommands: &[],
        shape: Shape::Jest,
    },
    Recipe {
        program: "vitest",
        subcommands: &[],
        shape: Shape::Jest,
    },
    Recipe {
        program: "go",
        subcommands: &["test"],
        shape: Shape::GoTest,
    },
    Recipe {
        program: "go",
        subcommands: &["build", "vet"],
        shape: Shape::Build,
    },
    Recipe {
        program: "npm",
        subcommands: &["install", "ci", "i", "add"],
        shape: Shape::Install,
    },
    Recipe {
        program: "pnpm",
        subcommands: &["install", "i", "add"],
        shape: Shape::Install,
    },
    Recipe {
        program: "yarn",
        subcommands: &["install", "add"],
        shape: Shape::Install,
    },
    Recipe {
        program: "ls",
        subcommands: &[],
        shape: Shape::Listing,
    },
    Recipe {
        program: "find",
        subcommands: &[],
        shape: Shape::Listing,
    },
    Recipe {
        program: "grep",
        subcommands: &[],
        shape: Shape::Listing,
    },
    Recipe {
        program: "rg",
        subcommands: &[],
        shape: Shape::Listing,
    },
    Recipe {
        program: "git",
        subcommands: &["status", "log", "branch", "ls-files"],
        shape: Shape::Listing,
    },
    Recipe {
        program: "git",
        subcommands: &["diff", "show"],
        shape: Shape::Diff,
    },
];

/// The recipe for `command`, if its first program is one ket knows.
///
/// Only the first command of a line: `cargo test | tail` already chose its
/// own trim, and `a && b` is two outputs ket cannot tell apart.
fn recipe_for(command: &str) -> Option<&'static Recipe> {
    let first = command
        .split(['|', ';', '&'])
        .next()
        .unwrap_or_default()
        .trim();
    if first.len() != command.trim().len() {
        return None;
    }
    let mut words = first.split_whitespace().peekable();
    // Through the wrappers that run a program without being one.
    loop {
        match words.peek().copied() {
            Some(word) if word.contains('=') => {
                words.next();
            }
            Some("sudo" | "env" | "nohup" | "nice" | "time" | "command") => {
                words.next();
                while words.peek().is_some_and(|w| w.starts_with('-')) {
                    words.next();
                }
            }
            Some("timeout") => {
                words.next();
                while words.peek().is_some_and(|w| w.starts_with('-')) {
                    words.next();
                }
                words.next();
            }
            _ => break,
        }
    }
    let program = words.next()?;
    let program = program.rsplit('/').next().unwrap_or(program);
    let subcommand = words.find(|word| !word.starts_with('-'));
    RECIPES.iter().find(|recipe| {
        recipe.program == program
            && (recipe.subcommands.is_empty()
                || subcommand.is_some_and(|sub| recipe.subcommands.contains(&sub)))
    })
}

impl Shape {
    /// `text` with this shape's noise taken out.
    fn trim(self, text: &str) -> String {
        match self {
            Shape::Build => build(text),
            Shape::CargoTest => cargo_test(&build(text)),
            Shape::Pytest => pytest(text),
            Shape::Jest => jest(text),
            Shape::GoTest => go_test(text),
            Shape::Install => install(text),
            Shape::Listing => dedupe(text),
            Shape::Diff => text.to_owned(),
        }
    }
}

/// Whether a build line is progress: what cargo and go print per unit of
/// work, which says nothing once the work is done.
fn is_build_progress(line: &str) -> bool {
    const PROGRESS: [&str; 14] = [
        "Compiling ",
        "Checking ",
        "Downloading ",
        "Downloaded ",
        "Updating ",
        "Locking ",
        "Adding ",
        "Removing ",
        "Blocking ",
        "Fresh ",
        "Running ",
        "Documenting ",
        "Building ",
        "Dirty ",
    ];
    let line = line.trim_start();
    PROGRESS.iter().any(|word| line.starts_with(word))
}

/// A build's output as its diagnostics and its summary.
///
/// Blocks rather than lines: a diagnostic is a header and the lines under
/// it, separated from the next by a blank line, and the lines under it are
/// what says where. A block that is only progress goes; the rest stay,
/// identical blocks counted once.
fn build(text: &str) -> String {
    let blocks: Vec<String> = text
        .split("\n\n")
        .map(|block| {
            block
                .lines()
                .filter(|line| !is_build_progress(line))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|block| !block.trim().is_empty())
        .collect();
    counted(blocks, "\n\n")
}

/// `cargo test`'s per-test lines as one count per block of them.
fn cargo_test(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let (mut passed, mut ignored) = (0usize, 0usize);
    let flush = |out: &mut Vec<String>, passed: &mut usize, ignored: &mut usize| {
        if *passed > 0 || *ignored > 0 {
            let mut line = format!("{} passed", *passed);
            if *ignored > 0 {
                line.push_str(&format!(", {} ignored", *ignored));
            }
            out.push(line);
        }
        *passed = 0;
        *ignored = 0;
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test ") && trimmed.ends_with("... ok") {
            passed += 1;
        } else if trimmed.starts_with("test ") && trimmed.ends_with("... ignored") {
            ignored += 1;
        } else if trimmed.starts_with("test ")
            && trimmed.contains(" ... ")
            && !trimmed.contains("FAILED")
            && !trimmed.contains("panicked")
        {
            // `bench`, `ignored, reason` and the like: counted as passed
            // rather than kept, since the run as a whole exited 0.
            passed += 1;
        } else {
            flush(&mut out, &mut passed, &mut ignored);
            out.push(line.to_owned());
        }
    }
    flush(&mut out, &mut passed, &mut ignored);
    out.join("\n")
}

/// pytest's per-test lines and progress dots go; the ruled summary and
/// anything that was not a pass stay.
fn pytest(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            let dots = !trimmed.is_empty()
                && trimmed.chars().all(|c| {
                    c == '.'
                        || c == 's'
                        || c == 'x'
                        || c == ' '
                        || c == '['
                        || c == ']'
                        || c == '%'
                        || c.is_ascii_digit()
                });
            !(trimmed.contains(" PASSED") || trimmed.contains(" SKIPPED") || dots)
        })
        .map(str::to_owned)
        .collect();
    counted(lines, "\n")
}

/// jest's and vitest's ticks and `PASS` lines go; failures, errors and the
/// totals stay.
fn jest(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with('\u{2713}')
                || trimmed.starts_with('\u{2714}')
                || trimmed.starts_with("PASS ")
                || trimmed.starts_with("\u{221a}"))
        })
        .map(str::to_owned)
        .collect();
    counted(lines, "\n")
}

/// `go test`'s per-test and per-package passes go.
fn go_test(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with("=== RUN")
                || trimmed.starts_with("=== PAUSE")
                || trimmed.starts_with("=== CONT")
                || trimmed.starts_with("--- PASS")
                || trimmed == "PASS"
                || trimmed.starts_with("ok  ")
                || trimmed.starts_with("ok \t"))
        })
        .map(str::to_owned)
        .collect();
    counted(lines, "\n")
}

/// An install's progress goes; what it added, warned about and deprecated
/// stays.
fn install(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|line| {
            let lower = line.trim().to_ascii_lowercase();
            if lower.is_empty() {
                return false;
            }
            let keep = [
                "added ",
                "removed ",
                "changed ",
                "warn",
                "err",
                "deprecated",
                "vulnerab",
                "done in",
                "packages:",
                "peer",
                "funding",
                "audited",
                "up to date",
            ];
            keep.iter().any(|word| lower.contains(word))
                && !lower.starts_with("progress")
                && !lower.contains("resolving")
                && !lower.contains("fetching")
                && !lower.contains("downloading")
        })
        .map(str::to_owned)
        .collect();
    counted(lines, "\n")
}

/// Exact repeats collapsed to one line with a count.
fn dedupe(text: &str) -> String {
    counted(text.lines().map(str::to_owned).collect(), "\n")
}

/// `items` with each run of identical neighbours — and any item already
/// seen — reduced to one copy and a `(×N)`, joined by `sep`.
fn counted(items: Vec<String>, sep: &str) -> String {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for item in items {
        match seen.iter_mut().find(|(known, _)| *known == item) {
            Some((_, count)) => *count += 1,
            None => seen.push((item, 1)),
        }
    }
    seen.into_iter()
        .map(|(item, count)| {
            if count > 1 {
                format!("{item}  (\u{d7}{count})")
            } else {
                item
            }
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// `text` cut to [`TRIM_TARGET`]: the head, a line saying what was left out,
/// and the last [`TAIL_LINES`] lines, which is where a summary tends to be.
///
/// Never longer than the target, whatever the lines look like: the tail is
/// bounded by bytes as well as lines, and a single line too long for the
/// budget is cut rather than kept whole.
fn cap(text: &str) -> String {
    if text.len() <= TRIM_TARGET {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.lines().collect();
    // The tail, taken from the end while both bounds hold.
    let mut tail_at = lines.len();
    let mut tail_len = 0;
    while tail_at > 0 && lines.len() - tail_at < TAIL_LINES {
        let next = lines[tail_at - 1].len() + 1;
        if tail_len + next > TAIL_BYTES {
            break;
        }
        tail_len += next;
        tail_at -= 1;
    }
    let budget = TRIM_TARGET.saturating_sub(tail_len + MARKER_ROOM);
    let mut head: Vec<&str> = Vec::new();
    let mut used = 0;
    for line in &lines[..tail_at] {
        if used + line.len() + 1 > budget {
            break;
        }
        used += line.len() + 1;
        head.push(line);
    }
    let omitted = tail_at - head.len();
    let mut out = head.join("\n");
    out.push_str(&format!("\n\u{2026} {omitted} lines omitted \u{2026}\n"));
    out.push_str(&lines[tail_at..].join("\n"));
    // A line longer than the whole budget leaves the head empty and the
    // marker alone would not say what it was; cut the text itself instead.
    if out.len() > TRIM_TARGET {
        let mut end = TRIM_TARGET - MARKER_ROOM;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\n\u{2026} cut \u{2026}");
    }
    out
}

#[cfg(test)]
mod tests {
    //! The pure parts: which commands get a recipe, what each recipe keeps,
    //! and which payloads the hook refuses before it touches the disk. The
    //! archive and the subprocess are exercised by hand — see
    //! the manual recipe kept outside this repository.

    use super::*;

    fn payload(exit_code: i64) -> serde_json::Value {
        let stdout: String = (0..60)
            .map(|i| format!("test module::case_{i} ... ok\n"))
            .collect();
        serde_json::json!({
            "hook_event_name": "PostToolUse",
            "session_id": "sess",
            "tool_use_id": "toolu_01",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_response": {
                "stdout": stdout,
                "stderr": "",
                "exit_code": exit_code,
                "interrupted": false,
                "truncated": false,
            },
        })
    }

    // ---- recipes -------------------------------------------------------------

    #[test]
    fn a_known_program_and_subcommand_get_their_recipe() {
        assert_eq!(
            recipe_for("cargo test --lib").map(|r| r.shape),
            Some(Shape::CargoTest)
        );
        assert_eq!(
            recipe_for("cargo clippy -- -D warnings").map(|r| r.shape),
            Some(Shape::Build)
        );
        assert_eq!(
            recipe_for("git diff HEAD~1").map(|r| r.shape),
            Some(Shape::Diff)
        );
        assert_eq!(
            recipe_for("rg foo src").map(|r| r.shape),
            Some(Shape::Listing)
        );
    }

    #[test]
    fn wrappers_and_paths_are_seen_through() {
        assert_eq!(
            recipe_for("sudo -n /usr/bin/go test ./...").map(|r| r.shape),
            Some(Shape::GoTest)
        );
        assert_eq!(
            recipe_for("RUST_LOG=debug timeout 30 cargo check").map(|r| r.shape),
            Some(Shape::Build)
        );
    }

    #[test]
    fn an_unknown_program_or_a_compound_line_gets_no_recipe() {
        assert!(recipe_for("uname -a").is_none());
        assert!(recipe_for("cargo publish").is_none());
        assert!(recipe_for("cargo test | tail -5").is_none());
        assert!(recipe_for("cargo build && cargo test").is_none());
    }

    #[test]
    fn a_build_drops_progress_and_keeps_diagnostics_whole() {
        let out = build(
            "   Compiling a v0.1.0\n   Compiling b v0.1.0\n\nwarning: unused `x`\n --> src/a.rs:1:5\n\n    Finished dev\n",
        );
        assert!(!out.contains("Compiling"));
        assert!(out.contains("warning: unused `x`\n --> src/a.rs:1:5"));
        assert!(out.contains("Finished dev"));
    }

    #[test]
    fn cargo_test_collapses_passes_and_keeps_the_summary() {
        let out = cargo_test(
            "running 3 tests\ntest a ... ok\ntest b ... ignored\ntest c ... ok\n\ntest result: ok. 2 passed; 0 failed; 1 ignored\n",
        );
        assert!(out.contains("2 passed, 1 ignored"));
        assert!(!out.contains("test a ... ok"));
        assert!(out.contains("test result: ok. 2 passed"));
    }

    #[test]
    fn a_panic_line_in_a_green_run_is_kept() {
        let out = cargo_test("test a ... ok\nthread 'x' panicked at src/a.rs\ntest b ... ok\n");
        assert!(out.contains("panicked"));
    }

    #[test]
    fn repeats_are_counted_once() {
        assert_eq!(
            counted(vec!["a".into(), "a".into(), "b".into()], "\n"),
            "a  (\u{d7}2)\nb"
        );
    }

    #[test]
    fn a_diff_is_never_deduplicated() {
        let diff = "+}\n+}\n+}\n";
        assert_eq!(Shape::Diff.trim(diff), diff);
    }

    #[test]
    fn the_cap_holds_against_long_lines_anywhere() {
        // Twelve long lines at the end would once have been kept whole.
        let long = "x".repeat(3_000);
        let text: String = (0..40)
            .map(|i| format!("line {i}\n"))
            .chain(std::iter::repeat_n(format!("{long}\n"), 12))
            .collect();
        assert!(cap(&text).len() <= TRIM_TARGET);
        // One line longer than the whole budget.
        let giant = format!("{}\nend\n", "y".repeat(20_000));
        let out = cap(&giant);
        assert!(out.len() <= TRIM_TARGET);
        assert!(out.ends_with("\u{2026} cut \u{2026}") || out.ends_with("end"));
    }

    #[test]
    fn the_cap_keeps_the_head_and_the_tail() {
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        let out = cap(&text);
        assert!(out.len() <= TRIM_TARGET);
        assert!(out.starts_with("line 0\n"));
        assert!(out.contains("lines omitted"));
        assert!(out.trim_end().ends_with("line 1999"));
    }

    // ---- apply ---------------------------------------------------------------

    #[test]
    fn short_output_is_not_worth_trimming() {
        assert!(apply("cargo test", "test a ... ok\n", "").is_none());
    }

    #[test]
    fn a_trim_that_saves_too_little_is_refused() {
        let text: String = (0..100)
            .map(|i| format!("unique line number {i}\n"))
            .collect();
        assert!(apply("ls", &text, "").is_none());
    }

    #[test]
    fn a_noisy_run_is_trimmed_and_counted() {
        let stdout: String = (0..100)
            .map(|i| format!("test module::case_{i} ... ok\n"))
            .collect();
        let filtered = apply("cargo test", &stdout, "").expect("trimmed");
        assert_eq!(filtered.lines_in, 100);
        assert_eq!(filtered.text.trim(), "100 passed");
        assert_eq!(filtered.bytes_in, stdout.len());
    }

    #[test]
    fn stderr_is_kept_under_its_own_rule() {
        let stdout: String = (0..100)
            .map(|i| format!("test module::case_{i} ... ok\n"))
            .collect();
        let filtered = apply("cargo test", &stdout, "warning: something\n").expect("trimmed");
        assert!(filtered.text.contains("--- stderr ---\nwarning: something"));
    }

    // ---- the hook's gates ------------------------------------------------------

    #[test]
    fn a_failed_run_is_never_touched() {
        assert!(hook_decision(&payload(1)).is_none());
    }

    #[test]
    fn a_payload_without_an_exit_code_is_the_success_the_event_says_it_is() {
        // What Claude Code 2.1 actually sends: stdout, stderr, interrupted
        // and nothing else.
        let mut real = payload(0);
        real["tool_response"] = serde_json::json!({
            "stdout": real["tool_response"]["stdout"],
            "stderr": "",
            "interrupted": false,
            "isImage": false,
            "noOutputExpected": false,
        });
        assert!(hook_decision_with(&real, None).is_some());
    }

    #[test]
    fn an_interrupted_or_already_truncated_run_is_left_alone() {
        let mut interrupted = payload(0);
        interrupted["tool_response"]["interrupted"] = serde_json::json!(true);
        assert!(hook_decision(&interrupted).is_none());
        let mut truncated = payload(0);
        truncated["tool_response"]["truncated"] = serde_json::json!(true);
        assert!(hook_decision(&truncated).is_none());
    }

    #[test]
    fn a_background_command_and_other_tools_are_left_alone() {
        let mut background = payload(0);
        background["tool_input"]["run_in_background"] = serde_json::json!(true);
        assert!(hook_decision(&background).is_none());
        let mut read = payload(0);
        read["tool_name"] = serde_json::json!("Read");
        assert!(hook_decision(&read).is_none());
        let mut pre = payload(0);
        pre["hook_event_name"] = serde_json::json!("PreToolUse");
        assert!(hook_decision(&pre).is_none());
    }

    #[test]
    fn a_command_with_no_recipe_is_left_alone() {
        let mut other = payload(0);
        other["tool_input"]["command"] = serde_json::json!("uname -a");
        assert!(hook_decision(&other).is_none());
    }

    #[test]
    fn the_trailer_names_the_archive_when_there_is_one_and_the_transcript_when_not() {
        let filtered = Filtered {
            text: "3 passed".to_owned(),
            lines_in: 1_842,
            lines_out: 1,
            bytes_in: 36_800,
        };
        let kept = with_trailer(&filtered, "toolu_01", Some(Path::new("/tmp/x.txt")));
        assert!(kept.ends_with("full output: ket recall toolu_01 \u{b7} /tmp/x.txt]"));
        assert!(kept.contains("1,842 lines \u{2192} 1"));
        assert!(kept.contains("\u{2248}9.2k \u{2192} 2 tokens"));
        let not = with_trailer(&filtered, "toolu_01", None);
        assert!(not.ends_with("full output in the session transcript]"));
        assert!(!not.contains("recall"));
    }

    #[test]
    fn without_an_archive_a_green_run_is_trimmed_and_nothing_touches_the_disk() {
        let decision = hook_decision_with(&payload(0), None).expect("a decision");
        let document: serde_json::Value = serde_json::from_str(&decision.json).expect("json");
        let replacement = &document["hookSpecificOutput"]["updatedToolOutput"];
        // The tool's own shape, text swapped: a bare string is ignored.
        let shown = replacement["stdout"].as_str().expect("the replacement");
        assert_eq!(replacement["stderr"], "");
        assert_eq!(replacement["interrupted"], false);
        assert_eq!(
            document["hookSpecificOutput"]["hookEventName"],
            "PostToolUse"
        );
        assert!(shown.starts_with("60 passed"));
        assert!(shown.ends_with("full output in the session transcript]"));
        assert_eq!(decision.shown_bytes, shown.len() as u64);
    }

    // ---- the archive, under a root of the test's own -------------------------

    /// A cache root of the test's own, removed when it finishes.
    struct Root(PathBuf);

    impl Root {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("ket-filter-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn an_archive_is_written_owner_only_and_recalled_by_id_or_its_tail() {
        let root = Root::new("archive");
        let path = archive_in(&root.0, "sess-1", "toolu_01ABCDEF", "out\n", "warn\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "out\n--- stderr ---\nwarn\n"
        );
        #[cfg(unix)]
        {
            assert_eq!(mode(&root.0), 0o700);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
            assert_eq!(mode(&path), 0o600);
        }
        assert_eq!(
            recall_in(&root.0, "toolu_01ABCDEF").unwrap(),
            "out\n--- stderr ---\nwarn\n"
        );
        assert_eq!(
            recall_in(&root.0, "CDEF").unwrap(),
            "out\n--- stderr ---\nwarn\n"
        );
    }

    #[test]
    fn recall_refuses_an_ambiguous_tail_and_names_the_retention_for_an_unknown_id() {
        let root = Root::new("recall");
        archive_in(&root.0, "s", "cli-1000", "a", "").unwrap();
        archive_in(&root.0, "s", "cli-2000", "b", "").unwrap();
        let ambiguous = recall_in(&root.0, "000").unwrap_err().to_string();
        assert!(ambiguous.contains("more than one"), "{ambiguous}");
        let unknown = recall_in(&root.0, "nope").unwrap_err().to_string();
        assert!(unknown.contains("kept for a day"), "{unknown}");
        assert!(recall_in(&root.0, "../etc").is_err());
    }

    #[test]
    fn an_id_is_reduced_to_a_file_name() {
        assert_eq!(safe("toolu_01-AB/../x"), "toolu_01-ABx");
    }

    #[test]
    fn the_sweep_removes_sessions_older_than_a_day_and_keeps_the_rest() {
        let root = Root::new("sweep");
        archive_in(&root.0, "old", "a", "x", "").unwrap();
        archive_in(&root.0, "new", "b", "y", "").unwrap();
        let later = std::time::SystemTime::now() + KEEP_FOR + Duration::from_secs(60);
        // Everything was written just now, so as of `later` it is all stale.
        sweep_in(&root.0, later);
        assert!(!root.0.join("old").exists());
        assert!(!root.0.join("new").exists());
        // And as of now nothing is.
        archive_in(&root.0, "fresh", "c", "z", "").unwrap();
        sweep_in(&root.0, std::time::SystemTime::now());
        assert!(root.0.join("fresh").exists());
    }

    #[test]
    fn with_a_root_the_hook_archives_first_and_names_the_path() {
        let root = Root::new("hook-keep");
        let decision = hook_decision_with(&payload(0), Some(&root.0)).expect("a decision");
        let document: serde_json::Value = serde_json::from_str(&decision.json).unwrap();
        let shown = document["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        let path = root.0.join("sess").join("toolu_01.txt");
        assert!(path.exists());
        assert!(shown.contains("full output: ket recall toolu_01"));
        assert!(shown.ends_with(&format!("{}]", path.display())));
    }

    #[test]
    fn a_root_that_cannot_be_written_means_no_trim() {
        let root = Root::new("hook-unwritable");
        // A file where the archive directory would go.
        let blocked = root.0.join("blocked");
        std::fs::write(&blocked, "").unwrap();
        assert!(hook_decision_with(&payload(0), Some(&blocked)).is_none());
    }

    // ---- `ket filter --` -------------------------------------------------------

    #[test]
    fn the_runner_keeps_the_commands_exit_code_and_leaves_a_failure_whole() {
        let ran = run_in("echo failing; exit 3", None);
        assert_eq!(ran.code, 3);
        assert_eq!(ran.out, "failing\n");
    }

    #[test]
    fn the_runner_leaves_a_command_with_no_recipe_whole() {
        let ran = run_in("uname -a", None);
        assert_eq!(ran.code, 0);
        assert!(!ran.out.contains("[ket:"));
    }

    #[test]
    fn the_runner_trims_a_recipe_and_archives_only_when_asked() {
        let bare = run_in("ls -la /usr/bin", None);
        assert_eq!(bare.code, 0);
        assert!(
            bare.out
                .ends_with("full output in the session transcript]\n"),
            "{}",
            bare.out
        );

        let root = Root::new("runner-keep");
        let kept = run_in("ls -la /usr/bin", Some(&root.0));
        assert!(
            kept.out.contains("full output: ket recall cli-"),
            "{}",
            kept.out
        );
        assert!(root.0.join("cli").read_dir().unwrap().next().is_some());
    }
}
