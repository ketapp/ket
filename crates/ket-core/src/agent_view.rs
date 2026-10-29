//! Claude Code's agent view (`claude agents`), as ket sees it from a tab.
//!
//! Agent view is one terminal holding many background sessions. Each runs
//! under Claude's own daemon, whose hooks carry no tab's identity, so ket
//! hears nothing from them — the tab looks like one empty session. What ket
//! has instead:
//!
//! - **The screen.** Which of agent view's screens is up — its list, its
//!   search, or one session's conversation — read off the rows, as Claude
//!   Code 2.1.288 draws them. See [`showing`].
//! - **`claude agents --json`**, which lists the background sessions under a
//!   folder by name, state and session id without a terminal. See
//!   [`sessions`].
//! - **Its keys.** ctrl+f searches the list, ↓ and ↑ move its `❯`, Enter
//!   opens the session `❯` is on, ← goes back to the list from a session
//!   with nothing typed in it. So a message for one session goes in by opening that session first,
//!   the way a person at the Mac would — see [`open`] — and never into
//!   whichever one happens to be up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::terminal::LocalTerminal;

/// Which of agent view's screens a terminal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showing {
    /// The list of sessions, with its box for starting a new one.
    List,
    /// The list, narrowed by ctrl+f's search.
    Find,
    /// One session's conversation.
    Session,
}

/// What agent view's list says over its sessions:
/// `0 awaiting input · 1 working · 2 completed`, or in its simple view the
/// counts it has, `1 needs you · 2 working · 1 idle`, or `nothing running`.
fn is_summary(row: &str) -> bool {
    if row.contains("awaiting input ·") && row.contains("working ·") && row.contains("completed")
    {
        return true;
    }
    let row = row.trim();
    !row.is_empty()
        && row.split(" · ").all(|part| {
            part == "nothing running"
                || part.split_once(' ').is_some_and(|(count, what)| {
                    count.bytes().all(|b| b.is_ascii_digit())
                        && matches!(what, "needs you" | "working" | "idle")
                })
        })
}

/// Whether `title` is the one agent view gives its terminal:
/// `2 awaiting input · claude agents`, or `claude agents`.
pub fn is_agent_view_title(title: &str) -> bool {
    title.trim().ends_with("claude agents")
}

/// A rule agent view draws over and under its input box.
fn is_rule(row: &str) -> bool {
    let row = row.trim();
    row.chars().count() >= 20 && row.chars().all(|c| c == '─')
}

/// The last few rows with anything on them, where Claude's footer and its
/// ← hint are — with a status line, or a mode, drawn about them.
fn foot(rows: &[String]) -> impl Iterator<Item = &String> {
    rows.iter()
        .rev()
        .filter(|row| !row.trim().is_empty())
        .take(4)
}

/// Whether `rows` are an ordinary Claude session's, not agent view's: its
/// footer offers ← to agent view — `← for agents`, `← 2 agents`, `← 1
/// done` — where a session opened from agent view offers `← to go back`.
fn plain_session(rows: &[String]) -> bool {
    foot(rows).any(|row| {
        row.split("← ")
            .skip(1)
            .any(|after| !after.starts_with("to go back"))
    })
}

/// Which of agent view's screens `rows` — the whole screen, top to bottom —
/// show, if any.
pub fn showing(rows: &[String]) -> Option<Showing> {
    if foot(rows).any(|row| row.contains("← to go back")) {
        return Some(Showing::Session);
    }
    if !rows.iter().any(|row| is_summary(row)) {
        return None;
    }
    if rows.iter().any(|row| row.trim_start().starts_with('⌕')) {
        return Some(Showing::Find);
    }
    Some(Showing::List)
}

/// Whether terminal `id` is in agent view: showing one of its screens now,
/// or having shown one and either still run by the same foreground process
/// group, `foreground`, or still drawing Claude's input box between its
/// rules. A session open in agent view draws that session's own screens — at
/// work, or asking a permission with no input box at all — so the screen
/// alone would say "not agent view" for as long as it shows them; the
/// process that drew the list is still there until agent view is quit.
///
/// Agent view also titles its terminal — see [`is_agent_view_title`] — which
/// its simple view, drawing no box and no rules, still does; and a terminal
/// whose foreground process was started as `claude agents`, `started_as`,
/// is agent view whatever its screen or title say.
pub fn in_agent_view(
    id: u64,
    rows: &[String],
    title: &str,
    foreground: Option<i32>,
    started_as: bool,
) -> bool {
    let mut seen = seen();
    if started_as || showing(rows).is_some() || (is_agent_view_title(title) && !plain_session(rows))
    {
        if seen.insert(id, foreground).is_none() {
            tracing::info!(
                terminal = id,
                by_command = started_as,
                by_title = is_agent_view_title(title),
                "terminal is showing Claude's agent view"
            );
        }
        return true;
    }
    // ← from an ordinary session opens agent view in the same process, and
    // ← to go back leaves it: the foreground alone can't say which is up.
    if let Some(drew) = seen.get(&id).filter(|_| !plain_session(rows))
        && ((drew.is_some() && *drew == foreground)
            || rows.iter().filter(|row| is_rule(row)).count() >= 2)
    {
        return true;
    }
    seen.remove(&id);
    false
}

/// The terminals found in agent view, with the foreground process group that
/// drew it — see [`in_agent_view`].
fn seen() -> std::sync::MutexGuard<'static, HashMap<u64, Option<i32>>> {
    static SEEN: OnceLock<Mutex<HashMap<u64, Option<i32>>>> = OnceLock::new();
    SEEN.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Whether `command` — a process's arguments, as the terminal's foreground
/// has them — is Claude Code.
pub fn runs_claude(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|program| program.rsplit('/').next())
        .is_some_and(|program| program == "claude")
}

/// Whether `command` is Claude Code started as agent view — `claude agents`
/// — which needs no reading of the screen to know.
pub fn runs_agent_view(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let program = words.next().and_then(|program| program.rsplit('/').next());
    program == Some("claude") && words.next() == Some("agents")
}

/// Forgets terminal `id`, which has closed.
pub fn forget(id: u64) {
    seen().remove(&id);
}

/// The list row agent view has selected, and whether it is a session's.
///
/// Agent view marks the selection by painting the row's background — as
/// Claude Code 2.1.288 draws it, `48;2;55;55;55` — never with a mark: the
/// only `❯` on its screen is its input box's prompt. Searched between the
/// list's summary and the rule over that box. A session's row is told from
/// a group heading by the footer, which offers "enter to open" only while a
/// session is selected; a heading's says "enter to collapse".
fn selected(terminal: &LocalTerminal) -> Option<(String, bool)> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::vte::ansi::{Color, NamedColor};

    terminal.with_term(|term| {
        let grid = term.grid();
        let rows = crate::screen::bottom_rows(term, usize::MAX);
        let from = rows
            .iter()
            .position(|row| is_summary(row))
            .map_or(0, |at| at + 1);
        let to = rows
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, row)| is_rule(row))
            .map_or(rows.len(), |(at, _)| at);
        let painted = (from..to).find(|&row| {
            let line = Line(i32::try_from(row).unwrap_or(i32::MAX));
            (0..grid.columns().min(8)).any(|col| {
                !matches!(
                    grid[line][Column(col)].bg,
                    Color::Named(NamedColor::Background)
                )
            })
        })?;
        let session = foot(&rows).any(|row| row.contains("enter to open"));
        Some((rows[painted].trim().to_owned(), session))
    })
}

/// Whether list row `row` is session `name`'s and none of `others`'. Whole,
/// the name is on the row and no longer name holding it is — "plan" and
/// "plan (2)". Too wide for the row, it is cut short with `…`: what the row
/// keeps of it must be the start of this name and of no other.
fn is_row_of(row: &str, name: &str, others: &[String]) -> bool {
    if row.contains(name) {
        return !others
            .iter()
            .any(|other| other != name && other.contains(name) && row.contains(other.as_str()));
    }
    let Some((kept, _)) = row.split_once('…') else {
        return false;
    };
    // The longest end of what the row keeps that starts the name: the row
    // leads with its marks.
    let shown = kept
        .char_indices()
        .map(|(at, _)| &kept[at..])
        .find(|tail| !tail.trim().is_empty() && name.starts_with(tail.trim_start()))
        .map(str::trim_start);
    shown.is_some_and(|shown| {
        shown.chars().count() >= MIN_SHOWN
            && !others
                .iter()
                .any(|other| other != name && other.starts_with(shown))
    })
}

/// The least of a name a cut-short row must show to be told apart by.
const MIN_SHOWN: usize = 8;

/// A word of `name` to narrow the list by. Agent view reads a search's first
/// word as a name to match within names and the rest as words to find
/// anywhere, and some words as filters of their own — `a:`, `#123`, a
/// link — so one plain word is typed, and the session picked from what it
/// leaves by its whole name. `None` when no word is plain.
fn search_word(name: &str) -> Option<&str> {
    name.split_whitespace()
        .filter(|word| {
            word.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
        .max_by_key(|word| word.chars().count())
}

/// What is typed in the input box between the last two rules on `rows`,
/// without its `❯`. A list's box shows its placeholder when empty.
fn typed(rows: &[String]) -> Option<String> {
    let rules: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| is_rule(row))
        .map(|(at, _)| at)
        .collect();
    let [.., above, below] = rules.as_slice() else {
        return None;
    };
    let text = rows[above + 1..*below]
        .iter()
        .map(|row| row.trim().trim_start_matches('❯').trim())
        .collect::<Vec<_>>()
        .join(" ");
    Some(text.trim().to_owned())
}

/// The placeholder in agent view's list box.
const PLACEHOLDER: &str = "describe a task for a new session";

/// A background session agent view lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundSession {
    /// Claude's session id: its transcript's name.
    pub session: String,
    /// The name agent view lists it under, and searches for.
    pub name: String,
    /// What it is doing.
    pub state: SessionState,
    /// When it was started, in milliseconds since the epoch.
    pub started_at_ms: i64,
    /// When its transcript was last written: when it last did anything. The
    /// start, when there is no transcript to ask.
    pub active_at_ms: i64,
    /// What it is doing, or waiting for, in a few words — empty for one
    /// that is done. See [`crate::conversation::claude_last_step`].
    pub doing: String,
    /// The short id `claude attach` takes.
    pub handle: String,
    /// Whether a process is running it now: the listing names its pid. One
    /// without is continued by `claude --resume … --bg`, with no screen.
    pub live: bool,
    /// The Claude config directory that lists it: the account it runs under.
    pub config_dir: Option<PathBuf>,
}

/// What a background session is doing, in the order a list shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SessionState {
    /// Waiting on a permission or a question.
    NeedsYou,
    /// At work on a turn.
    Working,
    /// Finished its turn.
    Done,
}

/// How long a listing is kept before it is asked for again.
const LISTING_TTL: Duration = Duration::from_secs(4);

/// How long `claude agents --json` may take before it is given up on.
const LISTING_TIMEOUT: Duration = Duration::from_secs(8);

struct Listing {
    at: Option<Instant>,
    refreshing: bool,
    sessions: Vec<BackgroundSession>,
}

fn listings() -> &'static Mutex<HashMap<PathBuf, Listing>> {
    static LISTINGS: OnceLock<Mutex<HashMap<PathBuf, Listing>>> = OnceLock::new();
    LISTINGS.get_or_init(Mutex::default)
}

/// The background sessions agent view lists under `cwd`, as last read.
///
/// Never waits: asking costs a `claude` process, so the answer is the last
/// one, and a listing older than [`LISTING_TTL`] is read again off this
/// thread for the next time. Empty until the first read is in.
pub fn sessions(cwd: &Path) -> Vec<BackgroundSession> {
    listing(cwd).0
}

/// [`sessions`], and whether a read has come in yet: until one has, none
/// listed means none known, not none there.
pub fn listing(cwd: &Path) -> (Vec<BackgroundSession>, bool) {
    let mut all = listings().lock().unwrap_or_else(PoisonError::into_inner);
    let listing = all.entry(cwd.to_owned()).or_insert(Listing {
        at: None,
        refreshing: false,
        sessions: Vec::new(),
    });
    let stale = listing.at.is_none_or(|at| at.elapsed() >= LISTING_TTL);
    if stale && !listing.refreshing {
        listing.refreshing = true;
        let cwd = cwd.to_owned();
        let spawned = std::thread::Builder::new()
            .name("ket-agent-view-listing".into())
            .spawn(move || {
                let read = read_sessions(&cwd);
                let mut all = listings().lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(listing) = all.get_mut(&cwd) {
                    listing.refreshing = false;
                    listing.at = Some(Instant::now());
                    if let Some(read) = read {
                        listing.sessions = read;
                    }
                }
            });
        if spawned.is_err() {
            listing.refreshing = false;
        }
    }
    (listing.sessions.clone(), listing.at.is_some())
}

/// Asks every Claude config directory ket knows of for its background
/// sessions under `cwd`. `None` when none could be asked at all, so a
/// failed read keeps the last listing rather than emptying it.
fn read_sessions(cwd: &Path) -> Option<Vec<BackgroundSession>> {
    let claude = crate::shell::find_on_login_path("claude")?;
    let mut found: Vec<BackgroundSession> = Vec::new();
    let mut asked = false;
    for dir in crate::sessions::claude_config_dirs() {
        let Some(json) = run_listing(&claude, &dir, cwd) else {
            continue;
        };
        asked = true;
        for mut session in parse_listing(&json) {
            session.config_dir = Some(dir.clone());
            resolve(&dir, &mut session);
            merge(&mut found, session);
        }
    }
    for session in &mut found {
        describe(session);
    }
    found.sort_by(|a, b| {
        a.state
            .cmp(&b.state)
            .then(b.active_at_ms.cmp(&a.active_at_ms))
    });
    // `--all` reaches back as far as Claude keeps sessions: the oldest
    // finished ones go first.
    found.truncate(MAX_LISTED);
    asked.then_some(found)
}

/// The most sessions a listing keeps.
const MAX_LISTED: usize = 200;

/// Points `session` at the transcript it is written in.
///
/// A background session has two ids: its job's, and the one its
/// conversation is written under, which a resume can make another. Claude's
/// listing names the second while a process runs the session and the first
/// once none does, and no transcript is named by the first after a resume —
/// "the session's transcript isn't there yet". The job's own record under
/// `dir/jobs/<short id>/state.json` keeps the second, `resumeSessionId`; a
/// session whose listed id names no transcript takes it, when it names one.
/// That id is what the conversation is read by and `--resume` takes.
fn resolve(dir: &Path, session: &mut BackgroundSession) {
    let written = |id: &str| crate::conversation::claude_transcript(id).is_some();
    if let Some(id) = written_under(dir, &session.handle, &session.session, written) {
        session.session = id;
    }
}

/// [`resolve`]'s answer: the id session `listed`, job `handle`, is written
/// under, when it isn't `listed` itself — read from the job's record under
/// `dir`, and taken only if `written` finds a transcript by it.
fn written_under(
    dir: &Path,
    handle: &str,
    listed: &str,
    written: impl Fn(&str) -> bool,
) -> Option<String> {
    if handle.is_empty() || written(listed) {
        return None;
    }
    let record = dir.join("jobs").join(handle).join("state.json");
    let resumed = std::fs::read(&record)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|state| state.get("resumeSessionId")?.as_str().map(str::to_owned))?;
    (resumed != listed && written(&resumed)).then_some(resumed)
}

/// Adds `session` to `found`, one entry per id. A continued session can be
/// listed twice under one id, the old finished entry beside the one running
/// it now: the running one is the session.
fn merge(found: &mut Vec<BackgroundSession>, session: BackgroundSession) {
    match found
        .iter_mut()
        .find(|known| known.session == session.session)
    {
        Some(known) if session.live && !known.live => *known = session,
        Some(_) => {}
        None => found.push(session),
    }
}

/// Fills in when `session` last did anything, from its transcript's time,
/// and — for one not done — what it is doing or waiting for, from the
/// transcript's end. A finished session's end is not read: a listing is
/// read every few seconds, and most of it is finished sessions.
fn describe(session: &mut BackgroundSession) {
    let Some(path) = crate::conversation::claude_transcript(&session.session) else {
        return;
    };
    if let Some(modified) = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
    {
        session.active_at_ms = i64::try_from(modified.as_millis()).unwrap_or(i64::MAX);
    }
    if session.state == SessionState::Done {
        return;
    }
    let Some(step) = crate::conversation::claude_last_step(&path) else {
        return;
    };
    session.doing = match session.state {
        SessionState::NeedsYou if !step.answered => step.asking,
        SessionState::NeedsYou => "Waiting on you".to_owned(),
        _ => step.doing,
    };
}

/// `claude agents --json --cwd <cwd>` under config directory `dir`, waited
/// for at most [`LISTING_TIMEOUT`].
fn run_listing(claude: &Path, dir: &Path, cwd: &Path) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new(claude)
        // `--all`: a finished session with no process left is still on
        // agent view's list, under Completed.
        .args(["agents", "--json", "--all", "--cwd"])
        .arg(cwd)
        .current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read as it is written: a listing bigger than the pipe holds would
    // otherwise leave `claude` blocked on writing it until the timeout.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).ok().map(|_| out)
    });
    let since = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if since.elapsed() >= LISTING_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    reader.join().ok().flatten()
}

/// The background sessions in a `claude agents --json` listing. Its
/// interactive sessions are other Claude terminals, not agent view's.
fn parse_listing(json: &str) -> Vec<BackgroundSession> {
    let Ok(serde_json::Value::Array(all)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    all.iter()
        .filter(|entry| entry["kind"].as_str() == Some("background"))
        .filter_map(|entry| {
            let session = entry["sessionId"].as_str()?.to_owned();
            // `state` is agent view's own reading: working, blocked, done,
            // failed or stopped. Older listings had only `status`.
            let state = match entry["state"].as_str() {
                Some("blocked") => SessionState::NeedsYou,
                Some("working") => SessionState::Working,
                Some(_) => SessionState::Done,
                None if entry["status"].as_str() == Some("busy") => SessionState::Working,
                None => SessionState::Done,
            };
            let started_at_ms = match &entry["startedAt"] {
                serde_json::Value::String(at) => at.parse().unwrap_or_default(),
                at => at.as_i64().unwrap_or_default(),
            };
            Some(BackgroundSession {
                name: entry["name"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&session)
                    .to_owned(),
                session,
                state,
                started_at_ms,
                active_at_ms: started_at_ms,
                doing: String::new(),
                handle: entry["id"].as_str().unwrap_or_default().to_owned(),
                live: !entry["pid"].is_null(),
                config_dir: None,
            })
        })
        .collect()
}

/// Why a session could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The terminal shows none of agent view's screens: not agent view at
    /// all, or a session asking a permission, which a key could answer.
    NotShowing,
    /// Something is typed in agent view's box on the Mac, which the keys
    /// would lose or send.
    Typed,
    /// The list showed no session by that name.
    Missing,
    /// Another session has the very same name, so its row can't be told
    /// from this one's.
    SameName,
    /// Agent view did not get where it was asked to in time.
    Stuck,
    /// The terminal took no more input.
    Gone,
    /// Claude refused or failed, in its words or ours.
    Failed(String),
}

impl Refused {
    /// What a phone is told.
    pub fn reason(&self) -> String {
        match self {
            Self::NotShowing => {
                "agent view on your Mac is showing something that needs an answer there first"
                    .to_owned()
            }
            Self::Typed => {
                "something is typed in agent view on your Mac — send or clear it there first"
                    .to_owned()
            }
            Self::SameName => {
                "agent view lists another session with the same name — open it in the \
                 terminal, or rename one"
                    .to_owned()
            }
            Self::Missing => {
                "agent view on your Mac doesn't show that session — open it in the terminal"
                    .to_owned()
            }
            Self::Stuck => "agent view didn't open the session — try it in the terminal".to_owned(),
            Self::Gone => "the terminal closed".to_owned(),
            Self::Failed(why) => why.clone(),
        }
    }
}

/// How long each step waits for agent view to draw its next screen.
const STEP_WAIT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(80);
/// How long a search is given to narrow the list once typed.
const SEARCH_SETTLE: Duration = Duration::from_millis(400);

fn screen(terminal: &LocalTerminal) -> Vec<String> {
    terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX))
}

/// Waits until `terminal` shows `want`, for at most [`STEP_WAIT`].
fn wait_for(terminal: &LocalTerminal, want: Showing) -> Result<(), Refused> {
    let since = Instant::now();
    loop {
        if showing(&screen(terminal)) == Some(want) {
            return Ok(());
        }
        if since.elapsed() >= STEP_WAIT {
            return Err(Refused::Stuck);
        }
        std::thread::sleep(POLL);
    }
}

fn press(terminal: &LocalTerminal, keys: &[u8]) -> Result<(), Refused> {
    terminal
        .send_input(keys.to_vec())
        .map_err(|_| Refused::Gone)
}

/// Opens the session named `name` in the agent view `terminal` shows: back
/// to the list if a session is up, a search by one word of the name, then
/// down the list and back up until `❯` is on that session's row, and Enter.
/// `others` are the other sessions' names, which can hold `name`. Blocks
/// while agent view draws each step, so it runs on the terminal's typing
/// worker, never a reading thread.
pub fn open(terminal: &LocalTerminal, name: &str, others: &[String]) -> Result<(), Refused> {
    if others.iter().any(|other| other == name) {
        return Err(Refused::SameName);
    }
    to_list(terminal)?;
    open_from_list(terminal, name, others)
}

/// Brings agent view back to its list, with nothing typed in its box: ←
/// from a session with nothing typed in it, Esc out of a search. Refused
/// when something is typed, which a key would lose or send.
fn to_list(terminal: &LocalTerminal) -> Result<(), Refused> {
    let rows = screen(terminal);
    let drafted =
        |rows: &[String]| typed(rows).is_some_and(|text| !text.is_empty() && text != PLACEHOLDER);
    match showing(&rows).ok_or(Refused::NotShowing)? {
        Showing::Session => {
            if typed(&rows).is_some_and(|text| !text.is_empty()) {
                return Err(Refused::Typed);
            }
            press(terminal, b"\x1b[D")?;
            wait_for(terminal, Showing::List)?;
            // The list keeps what was being written there for a new session.
            if drafted(&screen(terminal)) {
                return Err(Refused::Typed);
            }
        }
        Showing::Find => {
            press(terminal, b"\x1b")?;
            wait_for(terminal, Showing::List)?;
        }
        Showing::List => {
            if drafted(&rows) {
                return Err(Refused::Typed);
            }
        }
    }
    Ok(())
}

/// [`open`], from agent view's list.
fn open_from_list(terminal: &LocalTerminal, name: &str, others: &[String]) -> Result<(), Refused> {
    let searched = match search_word(name) {
        Some(word) => {
            press(terminal, &[0x06])?;
            wait_for(terminal, Showing::Find)?;
            press(terminal, word.as_bytes())?;
            std::thread::sleep(SEARCH_SETTLE);
            true
        }
        None => false,
    };
    // Down to the foot of the list, then up to its head: wherever the
    // selection was. Every press is counted, and a list that shows no
    // selection after one is left alone: a key that moves nothing visible
    // must not be pressed again and again.
    let mut presses = 0;
    for key in [b"\x1b[B", b"\x1b[A"] {
        let mut last: Option<String> = None;
        while presses < MAX_PRESSES {
            let Some((row, session)) = selected(terminal) else {
                if presses > 0 {
                    break;
                }
                // Nothing selected yet: one press to put the selection in
                // the list, and no more if that shows none.
                press(terminal, key)?;
                presses += 1;
                std::thread::sleep(STEP_SETTLE);
                continue;
            };
            if session && is_row_of(&row, name, others) {
                press(terminal, b"\r")?;
                return wait_for(terminal, Showing::Session);
            }
            if last.as_deref() == Some(row.as_str()) {
                break;
            }
            last = Some(row);
            press(terminal, key)?;
            presses += 1;
            std::thread::sleep(STEP_SETTLE);
        }
    }
    if searched {
        let _ = press(terminal, b"\x1b");
    }
    Err(Refused::Missing)
}

/// The most keys [`open`] presses moving through the list, down and up:
/// a search by one word leaves a few rows, and the whole of a long list is
/// not walked from a phone.
const MAX_PRESSES: usize = 60;
/// How long agent view is given to move its selection a row.
const STEP_SETTLE: Duration = Duration::from_millis(120);

/// The `CLAUDE_CONFIG_DIR` process `pid` runs under — the account a
/// terminal's agent view lists — or `None` for Claude's default.
pub fn config_dir_of(pid: i32) -> Option<PathBuf> {
    let output = std::process::Command::new("ps")
        .args(["-E", "-ww", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find_map(|word| word.strip_prefix("CLAUDE_CONFIG_DIR="))
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// The variables a Claude session leaves for what it runs, which a Claude
/// ket starts must not inherit: nesting guards, and the one that turns off
/// transcript saving — see the claude agent in `config.rs`.
const INHERITED: [&str; 4] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_CHILD_SESSION",
];

/// How long `claude --bg` is given to start a session.
const START_WAIT: Duration = Duration::from_secs(30);

/// `CLAUDE_CONFIG_DIR` for a Claude run under config directory `dir`:
/// unset for Claude's default `~/.claude`, which a tab launched without the
/// variable uses too. Set to the default by name, Claude reads its trust
/// and settings from `~/.claude/.claude.json` instead of `~/.claude.json`,
/// finds the checkout untrusted there, and refuses to start.
fn config_dir_env(dir: Option<&Path>) -> Option<&Path> {
    dir.filter(|dir| *dir != crate::sessions::home().join(".claude"))
}

/// Starts a background session in `cwd` doing `task`: `claude --bg`, under
/// the account `config_dir` names. With `resume`, that session's id, the
/// task goes to it and it carries on in the background, under the same id.
/// Agent view lists it from there. No screen is read or typed into.
pub fn start(
    config_dir: Option<&Path>,
    cwd: &Path,
    task: &str,
    resume: Option<&str>,
) -> Result<(), Refused> {
    let claude = crate::shell::find_on_login_path("claude")
        .ok_or_else(|| Refused::Failed("claude isn't on this Mac's PATH".to_owned()))?;
    let mut command = std::process::Command::new(claude);
    if let Some(session) = resume {
        command.arg("--resume").arg(session);
    }
    command
        .arg("--bg")
        .arg(task)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    for name in INHERITED {
        command.env_remove(name);
    }
    if let Some(dir) = config_dir_env(config_dir) {
        command.env("CLAUDE_CONFIG_DIR", dir);
    }
    let mut child = command
        .spawn()
        .map_err(|e| Refused::Failed(format!("couldn't start claude: {e}")))?;
    // Waited for at most START_WAIT: a stalled start must not hold the
    // terminal's typing queue, and the phone, for ever.
    let since = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if since.elapsed() >= START_WAIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Refused::Failed(
                    "claude took too long to start the session".to_owned(),
                ));
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(Refused::Failed(format!("couldn't start claude: {e}"))),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut said = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        use std::io::Read;
        let _ = stderr.read_to_string(&mut said);
    }
    let said = said.trim().to_owned();
    Err(Refused::Failed(if said.is_empty() {
        "claude didn't start the session".to_owned()
    } else {
        said
    }))
}

/// How long [`send`] waits for `claude attach` to draw the session — an old
/// session's attach reads its whole transcript first — and then for the
/// message to reach its transcript.
const ATTACH_WAIT: Duration = Duration::from_secs(30);
const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// A rule of a session's input box: a row of `─`, plain, or with the
/// session's name set into it, as the top rule of an attached session's box
/// is drawn (`──── design canvas directions ─`).
fn is_box_rule(row: &str) -> bool {
    let row = row.trim();
    row.starts_with('─') && row.ends_with('─') && row.chars().filter(|c| *c == '─').count() >= 20
}

/// The name set into the top rule of an attached session's input box, the
/// lowest one on `rows`: which background session the terminal shows.
///
/// A session agent view opens says so in its footer, but one Claude
/// attaches a terminal to by itself — `claude` started in a folder whose
/// session it had moved to the background — offers `← for agents`, as an
/// ordinary session does. Its box still carries the session's name.
pub fn attached_name(rows: &[String]) -> Option<String> {
    rows.iter().rev().find_map(|row| {
        if !is_box_rule(row) {
            return None;
        }
        let name = row.trim().trim_matches('─').trim();
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// Of the background sessions listed under `cwd`, the one named `name` as
/// [`attached_name`] read it: whole, or cut short with `…`. Only one the
/// name can mean: two of the same name, or two a cut-short name starts,
/// say nothing about which is up.
pub fn named(cwd: &Path, name: &str) -> Option<BackgroundSession> {
    let only = |mut matching: std::vec::IntoIter<BackgroundSession>| {
        let found = matching.next()?;
        matching.next().is_none().then_some(found)
    };
    let listed = sessions(cwd);
    let whole: Vec<_> = listed
        .iter()
        .filter(|listed| listed.name == name)
        .cloned()
        .collect();
    if !whole.is_empty() {
        return only(whole.into_iter());
    }
    let start = name.strip_suffix('…')?.trim_end();
    if start.is_empty() {
        return None;
    }
    only(
        listed
            .into_iter()
            .filter(|listed| listed.name.starts_with(start))
            .collect::<Vec<_>>()
            .into_iter(),
    )
}

/// Sends `text` to background session `session` by its id.
///
/// A session no process is running is continued with `claude --resume <id>
/// --bg <text>`: nothing is read off a screen or typed into one, and it
/// carries on under the same id. One with a process — or one Claude refuses
/// to resume, because the daemon still holds it — is attached in a terminal
/// of the host's own that no window shows, the message typed there, and the
/// terminal closed once the message is in the session's transcript. Nothing
/// in agent view's list is read or moved through, and the session runs on
/// when the attach is closed.
pub fn send(session: &BackgroundSession, cwd: &Path, text: &str) -> Result<(), Refused> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    if !session.live {
        match start(
            session.config_dir.as_deref(),
            cwd,
            text,
            Some(&session.session),
        ) {
            Ok(()) => return Ok(()),
            Err(why) => tracing::info!(
                session = session.session,
                why = why.reason(),
                "claude didn't resume the session in the background; attaching instead"
            ),
        }
    }
    attach_and_type(session, cwd, text)
}

/// [`send`] through `claude attach`.
fn attach_and_type(session: &BackgroundSession, cwd: &Path, text: &str) -> Result<(), Refused> {
    use alacritty_terminal::term::TermMode;

    if session.handle.is_empty() {
        return Err(Refused::Missing);
    }
    let claude = crate::shell::find_on_login_path("claude")
        .ok_or_else(|| Refused::Failed("claude isn't on this Mac's PATH".to_owned()))?;
    let mut env = std::collections::BTreeMap::new();
    env.insert("TERM".to_owned(), "xterm-256color".to_owned());
    if let Some(dir) = config_dir_env(session.config_dir.as_deref()) {
        env.insert(
            "CLAUDE_CONFIG_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        );
    }
    let spec = crate::terminal::TerminalSpec {
        command: claude.to_string_lossy().into_owned(),
        args: vec!["attach".to_owned(), session.handle.clone()],
        cwd: cwd.to_owned(),
        env,
        env_remove: INHERITED.iter().map(|name| (*name).to_owned()).collect(),
        size: crate::terminal::TerminalSize::new(120, 40),
        key: None,
        adopt: false,
        adopt_only: false,
    };
    // Where the transcript ends before anything is typed: delivery is a
    // new record from there on, never an older one that says the same.
    let before = crate::conversation::claude_transcript(&session.session)
        .and_then(|path| std::fs::metadata(path).ok())
        .map_or(0, |meta| meta.len());
    let terminal = LocalTerminal::open(&spec)
        .map_err(|e| Refused::Failed(format!("couldn't open the session: {e}")))?;
    let sent = (|| {
        // Drawn: its input box between two rules.
        let since = Instant::now();
        loop {
            let rows = screen(&terminal);
            if rows.iter().filter(|row| is_box_rule(row)).count() >= 2 {
                break;
            }
            if terminal.is_closed() {
                return Err(Refused::Failed(
                    "claude closed the session it was asked to open".to_owned(),
                ));
            }
            if since.elapsed() >= ATTACH_WAIT {
                let shown: Vec<&str> = rows
                    .iter()
                    .map(|row| row.trim())
                    .filter(|row| !row.is_empty())
                    .collect();
                let shown = shown[shown.len().saturating_sub(6)..].join(" ⏎ ");
                tracing::warn!(
                    session = session.session,
                    handle = session.handle,
                    screen = shown,
                    "claude attach didn't draw the session's input box in time"
                );
                return Err(Refused::Stuck);
            }
            std::thread::sleep(POLL);
        }
        std::thread::sleep(SEARCH_SETTLE);
        let bracketed = terminal.with_term(|term| term.mode().contains(TermMode::BRACKETED_PASTE));
        press(&terminal, &crate::attachments::paste(text, bracketed))?;
        std::thread::sleep(crate::attachments::SUBMIT_DELAY);
        press(&terminal, b"\r")?;
        let since = Instant::now();
        while since.elapsed() < DELIVERY_WAIT {
            if delivered(&session.session, before, text) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        Err(Refused::Failed(
            "the session didn't take the message — try it in the terminal".to_owned(),
        ))
    })();
    terminal.kill();
    sent
}

/// Whether session `session`'s transcript, from byte `from` on, has a
/// record of the person saying `text`. Read whole from `from`, however long
/// the record.
fn delivered(session: &str, from: u64, text: &str) -> bool {
    crate::conversation::claude_transcript(session)
        .is_some_and(|path| delivered_in(&path, from, text))
}

/// [`delivered`], in the transcript at `path`.
fn delivered_in(path: &Path, from: u64, text: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    // A transcript that shrank was rewritten: read it all.
    let from = if from > length { 0 } else { from };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return false;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    let wanted: String = text.chars().take(60).collect();
    bytes.split(|&b| b == b'\n').any(|line| {
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            return false;
        };
        if record.get("type").and_then(|t| t.as_str()) != Some("user") {
            return false;
        }
        let said = match &record["message"]["content"] {
            serde_json::Value::String(said) => said.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        said.contains(&wanted)
    })
}

/// Opens session `name` in `terminal`'s agent view, then types `text` to it
/// and Enter, as a person would, if there is any text.
pub fn open_and_type(
    terminal: &LocalTerminal,
    name: &str,
    others: &[String],
    text: &str,
) -> Result<(), Refused> {
    use alacritty_terminal::term::TermMode;

    let text = text.trim();
    // No name: a new session, started from the list's own box with `text`.
    if name.is_empty() {
        if text.is_empty() {
            return Err(Refused::Missing);
        }
        to_list(terminal)?;
    } else {
        open(terminal, name, others)?;
    }
    if text.is_empty() {
        return Ok(());
    }
    let bracketed = terminal.with_term(|term| term.mode().contains(TermMode::BRACKETED_PASTE));
    press(terminal, &crate::attachments::paste(text, bracketed))?;
    std::thread::sleep(crate::attachments::SUBMIT_DELAY);
    press(terminal, b"\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn a_claude_agents_process_is_agent_view_by_its_command_line() {
        assert!(runs_agent_view("/opt/homebrew/bin/claude agents"));
        assert!(runs_agent_view("claude agents --json"));
        assert!(!runs_agent_view("claude --resume 1234"));
        assert!(!runs_agent_view("node agents"));
        assert!(runs_claude("claude"));
        assert!(!runs_claude("/usr/bin/node claude"));
    }

    #[test]
    fn agent_view_is_known_by_the_title_it_gives_its_terminal() {
        assert!(is_agent_view_title("claude agents"));
        assert!(is_agent_view_title("2 awaiting input · claude agents"));
        assert!(!is_agent_view_title("claude"));
        assert!(!is_agent_view_title("✳ Fixing the build"));
    }

    #[test]
    fn the_lists_summary_row_is_recognised_in_both_its_forms() {
        assert!(is_summary("0 awaiting input · 0 working · 1 completed"));
        assert!(is_summary("  1 needs you · 2 working · 1 idle"));
        assert!(is_summary("nothing running"));
        assert!(!is_summary("working on it"));
        assert!(!is_summary(""));
    }

    #[test]
    fn each_of_agent_views_screens_is_told_apart() {
        let list = rows(&[
            "Claude Code v2.1.288",
            "0 awaiting input · 0 working · 1 completed",
            "Completed",
            "  ● plan",
            "────────────────────────",
            "❯ describe a task for a new session",
            "────────────────────────",
        ]);
        assert_eq!(showing(&list), Some(Showing::List));
        let find = rows(&[
            "0 awaiting input · 0 working · 1 completed",
            "  ● plan",
            "────────────────────────",
            "⌕ n:plan",
            "────────────────────────",
        ]);
        assert_eq!(showing(&find), Some(Showing::Find));
        let session = rows(&[
            "plan",
            "I did the thing.",
            "",
            "← to go back · esc to interrupt",
        ]);
        assert_eq!(showing(&session), Some(Showing::Session));
        let plain = rows(&["✳ Working…", "", "← for agents"]);
        assert_eq!(showing(&plain), None);
        assert!(plain_session(&plain));
        assert!(plain_session(&rows(&["", "← 2 agents"])));
        assert!(!plain_session(&session));
    }

    #[test]
    fn agent_view_sticks_to_its_process_until_a_plain_session_shows() {
        let id = 90_001;
        let list = rows(&["0 awaiting input · 1 working · 0 completed"]);
        assert!(in_agent_view(id, &list, "", Some(42), false));
        // A session at work draws none of agent view's screens; same process.
        assert!(in_agent_view(
            id,
            &rows(&["thinking…"]),
            "",
            Some(42),
            false
        ));
        // Another process in the foreground: no longer agent view.
        assert!(!in_agent_view(
            id,
            &rows(&["thinking…"]),
            "",
            Some(43),
            false
        ));
        // ← for agents is the ordinary session agent view was left for.
        assert!(in_agent_view(id, &list, "", Some(42), false));
        assert!(!in_agent_view(
            id,
            &rows(&["", "← for agents"]),
            "",
            Some(42),
            false
        ));
        // By title alone, and by command line alone.
        assert!(in_agent_view(
            90_002,
            &rows(&["anything"]),
            "claude agents",
            None,
            false
        ));
        assert!(in_agent_view(90_003, &rows(&[]), "", None, true));
        assert!(!in_agent_view(
            90_004,
            &rows(&["anything"]),
            "claude",
            None,
            false
        ));
    }

    #[test]
    fn a_sessions_row_is_told_from_its_longer_siblings_and_from_one_cut_short() {
        let others = vec!["plan (2)".to_owned(), "planning".to_owned()];
        assert!(is_row_of("❯ ● plan  3 min", "plan", &others));
        assert!(!is_row_of("❯ ● plan (2)  3 min", "plan", &others));
        assert!(!is_row_of("❯ ● planning  3 min", "plan", &others));
        // Cut short by the row's width: enough of the start, and no sibling
        // that starts the same way.
        let long = "share to comms feature plan";
        assert!(is_row_of(
            "❯ ● share to comms fe…",
            long,
            &["other thing".to_owned()]
        ));
        assert!(!is_row_of(
            "❯ ● share to comms fe…",
            long,
            &["share to comms feature plan (2)".to_owned()]
        ));
        assert!(!is_row_of("❯ ● share…", long, &[]));
        assert!(!is_row_of("❯ ● other thing…", long, &[]));
    }

    #[test]
    fn a_search_uses_one_plain_word_of_the_name() {
        assert_eq!(search_word("fix the build #123"), Some("build"));
        assert_eq!(search_word("plan"), Some("plan"));
        assert_eq!(search_word("#123 a:x"), None);
    }

    #[test]
    fn a_listing_is_read_by_claudes_own_state() {
        let json = r#"[
          {"kind":"interactive","sessionId":"i1","name":"me","status":"busy"},
          {"kind":"background","sessionId":"b1","name":"one","state":"blocked","status":"idle","startedAt":1},
          {"kind":"background","sessionId":"b2","name":"two","state":"working","startedAt":"2"},
          {"kind":"background","sessionId":"b3","name":"","state":"failed","startedAt":3},
          {"kind":"background","sessionId":"b4","status":"busy","startedAt":4}
        ]"#;
        let listed = parse_listing(json);
        let states: Vec<(&str, SessionState)> = listed
            .iter()
            .map(|session| (session.session.as_str(), session.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("b1", SessionState::NeedsYou),
                ("b2", SessionState::Working),
                ("b3", SessionState::Done),
                ("b4", SessionState::Working),
            ]
        );
        assert_eq!(listed[2].name, "b3");
        assert_eq!(listed[1].started_at_ms, 2);
    }

    /// A config directory holding job `handle`'s record, `state` as JSON.
    fn jobs(name: &str, handle: &str, state: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ket-agent-view-jobs-{name}-{}", std::process::id()));
        let job = dir.join("jobs").join(handle);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(job.join("state.json"), state).unwrap();
        dir
    }

    #[test]
    fn a_resumed_session_is_read_under_the_id_its_job_record_names() {
        // Listed under its job's id once nothing runs it, its conversation
        // written under another since a resume.
        let dir = jobs(
            "resumed",
            "4578b120",
            r#"{"state":"done","sessionId":"4578b120-b093","resumeSessionId":"71348dec-7c15"}"#,
        );
        let written = |id: &str| id == "71348dec-7c15";
        assert_eq!(
            written_under(&dir, "4578b120", "4578b120-b093", written),
            Some("71348dec-7c15".to_owned())
        );
    }

    #[test]
    fn a_listed_id_with_a_transcript_is_kept() {
        let dir = jobs(
            "kept",
            "49341863",
            r#"{"sessionId":"49341863-3234","resumeSessionId":"83ac82b3-b465"}"#,
        );
        // Listed under the id it is written under, as a running one is.
        let written = |id: &str| id == "83ac82b3-b465";
        assert_eq!(
            written_under(&dir, "49341863", "83ac82b3-b465", written),
            None
        );
    }

    #[test]
    fn nothing_is_taken_that_names_no_transcript_either() {
        // Neither id written: a conversation Claude never kept.
        let dir = jobs(
            "neither",
            "93dd32dc",
            r#"{"sessionId":"93dd32dc-36e0","resumeSessionId":"93dd32dc-36e0"}"#,
        );
        assert_eq!(
            written_under(&dir, "93dd32dc", "93dd32dc-36e0", |_| false),
            None
        );
        // A record naming an id with no transcript.
        let dir = jobs("unwritten", "aaaa1111", r#"{"resumeSessionId":"bbbb2222"}"#);
        assert_eq!(
            written_under(&dir, "aaaa1111", "aaaa1111-x", |_| false),
            None
        );
        // No record, an unreadable one, one without the field, no handle.
        let written = |id: &str| id == "cccc3333";
        assert_eq!(written_under(&dir, "nojob", "nojob-x", written), None);
        let dir = jobs("garbled", "dddd4444", "not json");
        assert_eq!(written_under(&dir, "dddd4444", "dddd4444-x", written), None);
        let dir = jobs("fieldless", "eeee5555", r#"{"state":"done"}"#);
        assert_eq!(written_under(&dir, "eeee5555", "eeee5555-x", written), None);
        assert_eq!(written_under(&dir, "", "x", written), None);
    }

    #[test]
    fn a_session_is_live_when_the_listing_names_its_pid() {
        let json = r#"[
          {"kind":"background","sessionId":"b1","name":"one","state":"done","startedAt":1},
          {"kind":"background","id":"b2","sessionId":"b2","name":"two","state":"done","status":"idle","pid":"4242","startedAt":2}
        ]"#;
        let listed = parse_listing(json);
        assert!(!listed[0].live);
        assert_eq!(listed[0].handle, "");
        assert!(listed[1].live);
        assert_eq!(listed[1].handle, "b2");
    }

    fn listed(session: &str, live: bool, state: SessionState) -> BackgroundSession {
        BackgroundSession {
            session: session.to_owned(),
            name: session.to_owned(),
            state,
            started_at_ms: 0,
            active_at_ms: 0,
            doing: String::new(),
            handle: session.to_owned(),
            live,
            config_dir: None,
        }
    }

    #[test]
    fn a_continued_session_listed_twice_is_the_running_one() {
        let mut found = Vec::new();
        merge(&mut found, listed("b1", false, SessionState::Done));
        merge(&mut found, listed("b2", false, SessionState::Done));
        // The old finished entry, then the one running it now.
        merge(&mut found, listed("b1", true, SessionState::Working));
        assert_eq!(found.len(), 2);
        assert!(found[0].live);
        assert_eq!(found[0].state, SessionState::Working);
        // The other way round, the running one is kept.
        merge(&mut found, listed("b1", false, SessionState::Done));
        assert!(found[0].live);
        // Two finished entries: the first is kept.
        let mut again = vec![listed("b3", false, SessionState::Done)];
        let mut other = listed("b3", false, SessionState::Done);
        other.name = "later".to_owned();
        merge(&mut again, other);
        assert_eq!(again[0].name, "b3");
    }

    #[test]
    fn claudes_default_config_dir_is_never_named_in_the_environment() {
        let home = crate::sessions::home();
        assert_eq!(config_dir_env(None), None);
        assert_eq!(config_dir_env(Some(&home.join(".claude"))), None);
        let personal = home.join(".claude-personal");
        assert_eq!(config_dir_env(Some(&personal)), Some(personal.as_path()));
    }

    #[test]
    fn an_attached_sessions_input_box_is_found_by_its_labelled_rule() {
        // As Claude Code 2.1.288 draws an attached session, 120 columns:
        // the top rule carries the session's name.
        let top = format!("{} design canvas directions ─", "─".repeat(93));
        let bottom = "─".repeat(120);
        assert!(is_box_rule(&top));
        assert!(is_box_rule(&bottom));
        assert!(!is_rule(&top), "the labelled rule is not a plain one");
        assert!(is_rule(&bottom));
        let screen = rows(&[
            "  Attaching…",
            "",
            &top,
            "❯ ",
            &bottom,
            "  ⏵⏵ auto mode on · ← for agents",
        ]);
        assert_eq!(screen.iter().filter(|row| is_box_rule(row)).count(), 2);
        assert_eq!(screen.iter().filter(|row| is_rule(row)).count(), 1);
        // Not a rule: too short, text, a rule that ends in text.
        assert!(!is_box_rule("─────"));
        assert!(!is_box_rule("Ran 2 shell commands"));
        assert!(!is_box_rule(&format!("{} done", "─".repeat(30))));
        assert!(!is_box_rule(""));
    }

    fn transcript(name: &str, records: &[serde_json::Value]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ket-agent-view-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn user(said: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"user","message":{"role":"user","content":said}})
    }

    #[test]
    fn a_message_is_delivered_only_as_a_record_written_after_the_send() {
        use std::io::Write;

        let older = user(serde_json::json!("Reply with only the word ok."));
        let path = transcript("delivered", &[older]);
        let before = std::fs::metadata(&path).unwrap().len();
        // The same words from earlier are not this send.
        assert!(!delivered_in(&path, before, "Reply with only the word ok."));
        assert!(delivered_in(&path, 0, "Reply with only the word ok."));
        // Written after the send: found, in a string or in parts, and by
        // its first 60 characters.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let reply = serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}});
        writeln!(file, "{reply}").unwrap();
        let parts = user(
            serde_json::json!([{"type":"text","text":"Now rebase onto main and rerun the checks"}]),
        );
        writeln!(file, "{parts}").unwrap();
        assert!(delivered_in(
            &path,
            before,
            "Now rebase onto main and rerun the checks"
        ));
        assert!(!delivered_in(&path, before, "Something never sent"));
        // An assistant's words are not the person's.
        assert!(!delivered_in(&path, before, "ok"));
        // A transcript shorter than where the send started was rewritten:
        // read whole.
        assert!(delivered_in(&path, u64::MAX, "Now rebase onto main"));
        assert!(!delivered_in(
            Path::new("/nonexistent/session.jsonl"),
            0,
            "anything"
        ));
    }
}
