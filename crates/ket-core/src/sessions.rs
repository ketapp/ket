//! Reading each agent's own record of what it has done.
//!
//! ket cannot be the only place a session is known. A person runs `claude` in a
//! ket terminal, or in iTerm, or resumes one inside the agent with `/resume`;
//! in none of those cases did ket mint an id or see a lifecycle. Any design
//! where ket only knows about sessions it started leaves the sidebar blank in
//! exactly the case people hit first — which is what happened.
//!
//! So sessions are *discovered* rather than only recorded: every agent already
//! keeps its own durable store, and this module reads them. The shapes have
//! nothing in common, which is why this is a trait with one implementation per
//! agent rather than a function with a `match`:
//!
//! | agent    | store                                             |
//! |----------|---------------------------------------------------|
//! | claude   | `~/.claude/projects/<slugged cwd>/<uuid>.jsonl`    |
//! | codex    | `~/.codex/sessions/<y>/<m>/<d>/rollout-*.jsonl`    |
//! | opencode | `~/.local/share/opencode/opencode.db` (SQLite)     |
//! | grok     | `~/.grok/sessions/<encoded cwd>/<id>/summary.json` |
//! | gemini   | not implemented — see [`GeminiSessions`]           |
//!
//! **Credentials are never touched.** Two of these stores sit beside secrets:
//! `~/.claude/.credentials.json` and `~/.codex/auth.json` are siblings of the
//! session directories, and opencode keeps a `credential` table in the *same
//! database* as its sessions. Every reader here names exactly what it wants —
//! a glob that only matches session files, a `SELECT` with explicit columns —
//! and nothing here reads a whole directory or a whole database.
//!
//! **The work is bounded.** This runs on a poll, so a project with hundreds of
//! sessions must not stall the window: file counts are capped by
//! [`MAX_SESSIONS_PER_SOURCE`], and the readers take a session's metadata from
//! the first record and its modification time rather than parsing a transcript
//! that may be megabytes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

/// Most sessions any one source will return for one directory.
///
/// A cap rather than a limit that fails: past this the newest are kept, which
/// is what a list ordered by recency would have shown anyway.
pub const MAX_SESSIONS_PER_SOURCE: usize = 50;

/// Most bytes read from a session file while looking for its metadata.
///
/// The interesting record is the first one. A transcript can be megabytes, and
/// reading one to learn its title would make a poll cost more than the window.
const METADATA_BYTES: usize = 64 * 1024;

/// A session an agent recorded in its own store.
///
/// Deliberately the small common shape rather than a union of everything each
/// agent knows: this is what a sidebar row and a dashboard card need, and a
/// type that grew a field per agent would push the differences back out to
/// every consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSession {
    /// Which agent recorded it, e.g. `"claude"`.
    pub agent: String,
    /// The agent's own identifier, and what its resume flag expects.
    pub id: String,
    /// What the agent calls it, when it names its sessions at all.
    pub title: Option<String>,
    /// A title explicitly set with the agent's rename command.
    #[serde(default)]
    pub explicit_title: Option<String>,
    /// The directory it ran in, which is how it maps to a worktree.
    pub directory: PathBuf,
    /// When it was last written to.
    pub updated_at_ms: u64,
}

/// One agent's session store.
///
/// Implementations are read-only and must tolerate a store that is absent,
/// half-written or from a newer version of the agent: an agent nobody has
/// installed is the normal case, not an error.
pub trait SessionSource: Send + Sync {
    /// The agent's name, matching [`crate::config::AgentSpec::name`].
    fn agent(&self) -> &str;

    /// Sessions this agent recorded for any of `directories`.
    ///
    /// The whole set at once, deliberately. A per-directory call would make
    /// each source re-walk its store once per worktree on every poll — a
    /// `sqlite3` spawn and a full rollout walk per row, several times a
    /// minute. One pass, filtered, is the difference between a poll that costs
    /// nothing and one that shows up in a profile.
    ///
    /// An empty vector means "none", never "could not tell" — a source that
    /// cannot read its store says so with `Err`.
    fn sessions_for(&self, directories: &[&Path]) -> Result<Vec<DiscoveredSession>>;

    /// The arguments that resume `session`, appended to the agent's command.
    ///
    /// Here rather than in [`crate::config::AgentSpec`] because it is a fact
    /// about the agent's CLI, not a preference: a user editing their config
    /// should not have to know that Claude spells it `--resume` and OpenCode
    /// does not spell it the same way.
    fn resume_args(&self, session: &str) -> Vec<String>;

    /// Sessions this agent is running in `directory` at this moment.
    ///
    /// Defaults to none, which is the honest answer for a source that has no
    /// way to ask. A source that *can* ask should, because a record on disk
    /// says nothing about whether the agent still owns it — and handing
    /// `--resume` a session the agent is currently running is not a no-op, it
    /// is a process that prints a refusal and exits.
    fn live_in(&self, _directory: &Path) -> Vec<LiveSession> {
        Vec::new()
    }

    /// Every session this agent is running anywhere, with its directory.
    ///
    /// Separate from [`SessionSource::live_in`] because the caller is
    /// different in kind: `live_in` answers one question about one directory
    /// at the moment somebody clicks, and this answers "what is happening
    /// everywhere" on a poll, where asking once per worktree would be a
    /// process spawn per row per tick.
    fn live(&self) -> Vec<LiveSession> {
        Vec::new()
    }

    /// The arguments that open a session the agent is already running.
    ///
    /// `None` when the agent offers no way in — for an interactive session
    /// somebody else has on screen there is nothing to do but start fresh.
    fn attach_args(&self, _live: &LiveSession) -> Option<Vec<String>> {
        None
    }
}

/// How an agent is holding a session that is running right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveKind {
    /// Running detached, with no terminal attached to it. Openable.
    Background,
    /// Someone already has it on screen somewhere else.
    Interactive,
}

/// A session an agent is running at this moment.
///
/// Distinct from [`DiscoveredSession`], which is a *record* — something on
/// disk that can be resumed. A session can be both, and when it is, resuming
/// the record is exactly the wrong move: the agent owns that conversation
/// right now and will refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    /// The agent's own identifier, matching [`DiscoveredSession::id`].
    pub id: String,
    /// The short handle the agent's CLI takes, where it differs from `id`.
    pub handle: Option<String>,
    /// How it is being held.
    pub kind: LiveKind,
    /// Where it is running, which is how it maps to a worktree.
    pub directory: PathBuf,
    /// Whether it is working at this moment, when the agent will say.
    ///
    /// The one fact that cannot be had any other way. ket watching a terminal
    /// learns that `claude` is a running process and stops there — a session
    /// waiting at its prompt and one halfway through a refactor look exactly
    /// alike from outside. `None` means the agent did not say, which is not
    /// the same as "not busy" and must not be shown as one.
    pub busy: Option<bool>,
}

/// Every agent store ket knows how to read.
///
/// Sources are held as trait objects so a new agent is a new file and one line
/// here, and so a consumer never learns which store an answer came from.
pub struct Sessions {
    sources: Vec<Box<dyn SessionSource>>,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Sessions {
    /// The shipped set of sources.
    pub fn new() -> Self {
        Self {
            sources: vec![
                Box::new(ClaudeSessions::default()),
                Box::new(CodexSessions::default()),
                Box::new(OpencodeSessions::default()),
                Box::new(GrokSessions::default()),
                Box::new(GeminiSessions),
            ],
        }
    }

    /// The shipped set as `agent` will see it once launched: Claude's store
    /// is only the config directory `agent`'s own line points at.
    ///
    /// What picking a session to *resume* needs. [`Self::new`] reads every
    /// directory a project's command may use, and a session from one of the
    /// others is an id the launched `claude` has never heard of — it exits
    /// with "no conversation found" instead of opening anything.
    pub fn for_launch(agent: &crate::config::AgentSpec) -> Self {
        let claude = if agent.name == "claude" {
            let dir = claude_config_dir_seen_by(&agent.launch_line(&[]))
                .unwrap_or_else(|| home().join(".claude"));
            ClaudeSessions::at(dir.join("projects"))
        } else {
            ClaudeSessions::default()
        };
        Self {
            sources: vec![
                Box::new(claude),
                Box::new(CodexSessions::default()),
                Box::new(OpencodeSessions::default()),
                Box::new(GrokSessions::default()),
                Box::new(GeminiSessions),
            ],
        }
    }

    /// A set with exactly these sources, for a caller that wants to narrow it.
    pub fn with_sources(sources: Vec<Box<dyn SessionSource>>) -> Self {
        Self { sources }
    }

    /// Every session any agent recorded for any of `directories`, bucketed by
    /// directory and newest first within each.
    ///
    /// One pass per store, however many directories are asked about — see
    /// [`SessionSource::sessions_for`]. A source that fails is skipped rather
    /// than failing the whole scan: one agent with an unreadable store must
    /// not blank the sidebar for the two that are working.
    pub fn scan(&self, directories: &[&Path]) -> BTreeMap<PathBuf, Vec<DiscoveredSession>> {
        let mut buckets: BTreeMap<PathBuf, Vec<DiscoveredSession>> = BTreeMap::new();

        for source in &self.sources {
            let started = std::time::Instant::now();
            let result = source.sessions_for(directories);
            if std::env::var_os("KET_FRAME_TRACE").is_some_and(|v| v != "0")
                && started.elapsed() >= std::time::Duration::from_millis(4)
            {
                eprintln!(
                    "tick  {:>6.2}ms      scan:{}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    source.agent()
                );
            }
            match result {
                Ok(sessions) => {
                    for session in sessions {
                        buckets
                            .entry(session.directory.clone())
                            .or_default()
                            .push(session);
                    }
                }
                Err(e) => {
                    tracing::debug!(agent = source.agent(), %e, "session store unreadable");
                }
            }
        }

        for sessions in buckets.values_mut() {
            sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at_ms));
        }
        buckets
    }

    /// Every session every agent is running right now, across all of them.
    ///
    /// One call per source, not per directory — see [`SessionSource::live`].
    /// A source that cannot answer contributes nothing rather than failing the
    /// sweep, on the same terms as [`Self::scan`].
    pub fn live(&self) -> Vec<LiveSession> {
        self.sources
            .iter()
            .flat_map(|source| source.live())
            .collect()
    }

    /// How to resume `session`, or `None` if no source claims that agent.
    pub fn resume_args(&self, agent: &str, session: &str) -> Option<Vec<String>> {
        self.sources
            .iter()
            .find(|source| source.agent() == agent)
            .map(|source| source.resume_args(session))
    }

    /// How to get `session` back on screen, given what is running right now.
    ///
    /// The distinction [`Self::resume_args`] cannot make on its own, and the
    /// reason clicking a worktree could open a pane that died on the spot:
    ///
    /// - **Nothing is holding it.** Resume the record, as before.
    /// - **The agent has it in the background.** Attach, which is what the
    ///   agent's own refusal tells you to do. Resuming it exits non-zero.
    /// - **Someone has it open elsewhere.** `None` — start a fresh session
    ///   rather than fight another window for the same conversation.
    pub fn open_args(&self, agent: &str, directory: &Path, session: &str) -> Option<Vec<String>> {
        let source = self.sources.iter().find(|source| source.agent() == agent)?;

        match source
            .live_in(directory)
            .into_iter()
            .find(|live| live.id == session)
        {
            Some(live) if live.kind == LiveKind::Background => source.attach_args(&live),
            Some(_) => None,
            None => Some(source.resume_args(session)),
        }
    }
}

// ---- claude -----------------------------------------------------------------

/// Claude Code: one JSONL file per session, in a directory named for the cwd.
pub struct ClaudeSessions {
    /// `~/.claude/projects`, overridable so this is testable without a home.
    ///
    /// Several when a project launches `claude` with a command of its own
    /// that points it at another config directory — see
    /// [`claude_config_dirs`]. A worktree's transcripts are in whichever one
    /// its sessions ran under, and can be in more than one.
    roots: Vec<PathBuf>,
}

impl Default for ClaudeSessions {
    fn default() -> Self {
        Self {
            roots: claude_config_dirs()
                .into_iter()
                .map(|dir| dir.join("projects"))
                .collect(),
        }
    }
}

impl SessionSource for ClaudeSessions {
    fn agent(&self) -> &str {
        "claude"
    }

    fn sessions_for(&self, directories: &[&Path]) -> Result<Vec<DiscoveredSession>> {
        let mut all = Vec::new();
        for directory in directories {
            all.extend(self.for_one(directory)?);
        }
        Ok(all)
    }

    fn resume_args(&self, session: &str) -> Vec<String> {
        vec!["--resume".to_owned(), session.to_owned()]
    }

    /// Asked of `claude` itself rather than inferred from the store.
    ///
    /// Nothing on disk distinguishes a transcript whose process is still alive
    /// from one whose process is long gone, and that difference is the whole
    /// question here. `claude agents --json` answers it directly, and is the
    /// same listing the agent's own refusal message points at.
    fn live_in(&self, directory: &Path) -> Vec<LiveSession> {
        claude_agents(Some(directory))
    }

    fn live(&self) -> Vec<LiveSession> {
        claude_agents(None)
    }

    fn attach_args(&self, live: &LiveSession) -> Option<Vec<String>> {
        // `attach` takes the short handle, not the full session id.
        let handle = live.handle.clone()?;
        Some(vec!["attach".to_owned(), handle])
    }
}

/// Asks `claude` what it is running, for one directory or for all of them.
///
/// Nothing on disk answers this. A transcript looks identical whether its
/// process is mid-turn, sitting at a prompt, or long dead, and that difference
/// is the whole question — so it is asked of the agent, which is the only
/// thing that knows. The same listing its own refusal message points at.
fn claude_agents(directory: Option<&Path>) -> Vec<LiveSession> {
    let mut command = std::process::Command::new("claude");
    command.arg("agents").arg("--json");
    if let Some(directory) = directory {
        command.arg("--cwd").arg(directory);
    }

    let Ok(output) = command.output() else {
        // Not installed, or not on this `PATH`. Not knowing is the default
        // answer, and every caller treats it as "no idea" rather than "no".
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();

    rows.into_iter()
        .filter_map(|row| {
            Some(LiveSession {
                id: row.get("sessionId")?.as_str()?.to_owned(),
                // The short handle `attach` and `stop` take, which only a
                // background session has.
                handle: row.get("id").and_then(|id| id.as_str()).map(str::to_owned),
                kind: match row.get("kind").and_then(|k| k.as_str()) {
                    Some("background") => LiveKind::Background,
                    _ => LiveKind::Interactive,
                },
                directory: row
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                // Anything the agent does not spell out stays unknown: a
                // status this does not recognise is not evidence of idleness.
                busy: match row.get("status").and_then(|s| s.as_str()) {
                    Some("busy") => Some(true),
                    Some("idle") => Some(false),
                    _ => None,
                },
            })
        })
        .collect()
}

impl ClaudeSessions {
    /// Sessions read from a specific `projects` directory.
    pub fn at(root: PathBuf) -> Self {
        Self { roots: vec![root] }
    }

    /// One directory's sessions. Claude files them per cwd, so there is
    /// nothing to gain from looking at several at once.
    fn for_one(&self, directory: &Path) -> Result<Vec<DiscoveredSession>> {
        // The directory name is the cwd with its separators flattened, and
        // the cwd is the *resolved* one: on a Mac `/var` is a symlink to
        // `/private/var`, so a worktree under `/var/folders/...` is recorded
        // by the agent under `-private-var-folders-...`. Slugging the path ket
        // was handed finds nothing, every time, for any worktree below a
        // symlink.
        //
        // Both spellings of the separator rule are tried because it is the
        // agent's rule, not ket's; a wrong guess costs nothing.
        let resolved = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        let candidates: Vec<String> = [resolved.as_path(), directory]
            .into_iter()
            .flat_map(|path| {
                let text = path.to_string_lossy().into_owned();
                // Underscore too, and it is not obvious: a temp path like
                // `…x0_00000gn…` is filed by the agent as `…x0-00000gn…`, so a
                // slug that flattens only `/` and `.` misses by one character
                // and finds nothing at all. Every separator the agent folds
                // has to be folded here or the whole lookup silently fails.
                [
                    text.replace(['/', '.', '_'], "-"),
                    text.replace(['/', '.'], "-"),
                    text.replace('/', "-"),
                ]
            })
            .collect();

        // The first spelling that exists, in each root that has one.
        let dirs: Vec<PathBuf> = self
            .roots
            .iter()
            .filter_map(|root| {
                candidates
                    .iter()
                    .map(|slug| root.join(slug))
                    .find(|path| path.is_dir())
            })
            .collect();

        // Newest first and capped *before* any file is opened for its title:
        // each title costs two tail reads and a parse, and a project's
        // directory holds every transcript it has ever had.
        let mut found = Vec::new();
        for dir in dirs {
            let entries = std::fs::read_dir(&dir).map_err(|e| crate::KetError::io(&dir, e))?;
            for entry in entries.flatten() {
                let path = entry.path();
                // Only session transcripts. The same directory holds other
                // things, and a blanket read of it is how a reader ends up
                // somewhere it was never meant to be.
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };

                found.push((id.to_owned(), modified_ms(&entry), path));
            }
        }

        found.sort_by_key(|(_, updated_at_ms, _)| std::cmp::Reverse(*updated_at_ms));
        found.truncate(MAX_SESSIONS_PER_SOURCE);

        let sessions = found
            .into_iter()
            .map(|(id, updated_at_ms, path)| {
                let explicit_title = custom_title(&path);
                DiscoveredSession {
                    agent: "claude".to_owned(),
                    id,
                    // The agent's own name for the conversation where it has
                    // written one, and the opening line only where it has not.
                    title: explicit_title
                        .clone()
                        .or_else(|| ai_title(&path))
                        .or_else(|| first_user_line(&path)),
                    explicit_title,
                    directory: directory.to_path_buf(),
                    updated_at_ms,
                }
            })
            .collect();

        Ok(sessions)
    }
}

// ---- codex ------------------------------------------------------------------

/// Codex: dated rollout files, whose first record names the cwd they ran in.
pub struct CodexSessions {
    /// `~/.codex/sessions`.
    root: PathBuf,
}

impl Default for CodexSessions {
    fn default() -> Self {
        Self {
            root: codex_sessions_dir(),
        }
    }
}

/// Codex's dated store of rollouts, `~/.codex/sessions`.
pub(crate) fn codex_sessions_dir() -> PathBuf {
    home().join(".codex").join("sessions")
}

impl CodexSessions {
    /// Sessions read from a specific `sessions` directory.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }
}

impl SessionSource for CodexSessions {
    fn agent(&self) -> &str {
        "codex"
    }

    fn sessions_for(&self, directories: &[&Path]) -> Result<Vec<DiscoveredSession>> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }

        // One walk, then match every rollout's cwd against the whole set. The
        // store is years of dated directories; walking it once per worktree is
        // what makes this expensive, not walking it at all.
        // Both spellings, for the same symlink reason as above.
        let wanted: std::collections::HashSet<String> = directories
            .iter()
            .flat_map(|d| {
                let resolved = std::fs::canonicalize(d).unwrap_or_else(|_| d.to_path_buf());
                [
                    d.to_string_lossy().into_owned(),
                    resolved.to_string_lossy().into_owned(),
                ]
            })
            .collect();

        // Rollouts are filed under year/month/day, so the newest live in the
        // last directory at each level. Walking newest-first and stopping at
        // the cap keeps this bounded on a store with years of history in it.
        let mut files = Vec::new();
        collect_rollouts(&self.root, &mut files, 4);
        files.sort_by_key(|(_, at)| std::cmp::Reverse(*at));

        let mut sessions = Vec::new();
        let cap = MAX_SESSIONS_PER_SOURCE * directories.len().max(1);

        for (path, at) in files {
            if sessions.len() >= cap {
                break;
            }

            let Some((cwd, id)) = rollout_identity(&path) else {
                continue;
            };
            if !wanted.contains(cwd.as_str()) {
                continue;
            }

            sessions.push(DiscoveredSession {
                agent: "codex".to_owned(),
                id,
                title: None,
                explicit_title: None,
                directory: PathBuf::from(cwd),
                updated_at_ms: at,
            });
        }

        // Titles live in a separate index, keyed by the same id. Absent or
        // unreadable, the sessions are still perfectly usable without them.
        let titles = codex_titles();
        for session in &mut sessions {
            session.title = titles.get(&session.id).cloned();
            session.explicit_title = session.title.clone();
        }

        Ok(sessions)
    }

    fn resume_args(&self, session: &str) -> Vec<String> {
        vec!["resume".to_owned(), session.to_owned()]
    }
}

/// The `cwd` and session `id` a rollout file's first record names.
///
/// Remembered for the life of the process, keyed by path. A rollout's first
/// record is written once and never changes, and every scan otherwise reads
/// up to [`METADATA_BYTES`] from each file, newest first, until enough match
/// a worktree. On a store full of other projects' sessions that was most of
/// the store, every thirty seconds. Only a successful read is kept, so a file
/// still being created is looked at again next time.
fn rollout_identity(path: &Path) -> Option<(String, String)> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, PoisonError};

    static SEEN: OnceLock<Mutex<HashMap<PathBuf, (String, String)>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Mutex::default);

    if let Some(hit) = seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
    {
        return Some(hit.clone());
    }

    // The first record carries `cwd` and `id`; the rest is transcript and is
    // never read.
    let record = first_record(path)?;
    let payload = record.get("payload")?.as_object()?;
    let cwd = payload.get("cwd").and_then(|c| c.as_str()).unwrap_or("");
    let id = payload.get("id")?.as_str()?;
    let identity = (cwd.to_owned(), id.to_owned());

    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.to_path_buf(), identity.clone());
    Some(identity)
}

/// Every `rollout-*.jsonl` under `dir`, with its modification time.
fn collect_rollouts(dir: &Path, out: &mut Vec<(PathBuf, u64)>, depth: usize) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollouts(&path, out, depth - 1);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        {
            out.push((path, modified_ms(&entry)));
        }
    }
}

/// Most bytes one [`CodexRollout::poll`] will read.
///
/// Only the newest `token_count` record matters, because its counts are
/// cumulative — so a poll that has fallen a long way behind reads the tail
/// rather than the whole backlog, and a rollout that grew by megabytes while
/// ket was closed costs one bounded read rather than a pause.
const TAIL_BYTES: u64 = 256 * 1024;

/// Follows one Codex rollout, reading only what has been appended since the
/// last look.
///
/// Codex publishes no status line, which is the whole reason this exists:
/// everything ket knows about a Claude session's spending arrives on a payload
/// Claude draws several times a second, and the nearest equivalent Codex offers
/// is the file it is already writing its transcript into. The `token_count`
/// records in it carry cumulative token counts and a quota reading — see
/// [`crate::usage::cache_from_codex`] for what is read out of one, and for the
/// arithmetic that is not the obvious one.
///
/// Tailing rather than re-reading: a rollout is a transcript and reaches
/// megabytes, and re-parsing it on every poll would make watching a session
/// cost more than the session.
#[derive(Debug, Clone, Default)]
pub struct CodexRollout {
    /// The file being followed.
    path: PathBuf,
    /// How far into it the last poll read.
    offset: u64,
    /// Codex's own id for the session, from the file's first record.
    session: Option<String>,
    /// Where it is working, from the same place.
    cwd: Option<String>,
    /// The model and effort in force, from the newest `turn_context`.
    ///
    /// **Retained rather than re-read.** These are written once per turn, so a
    /// poll that catches no new turn sees neither — and a card that forgot the
    /// model every few seconds would flicker. The tailer holds one session, so
    /// holding that session's own settings is the natural place for them.
    model: Option<String>,
    /// See [`CodexRollout::model`].
    effort: Option<String>,
}

impl CodexRollout {
    /// Follows the newest rollout Codex has written for `directory`.
    ///
    /// `None` when Codex has never run there — which is every worktree whose
    /// agent is Claude, so this is an ordinary answer rather than a failure.
    pub fn for_directory(directory: &Path) -> Option<Self> {
        Self::under(&home().join(".codex").join("sessions"), directory)
    }

    /// [`CodexRollout::for_directory`] against an explicit store, so this is
    /// exercisable without a home directory.
    pub fn under(root: &Path, directory: &Path) -> Option<Self> {
        if !root.is_dir() {
            return None;
        }

        // Both spellings, for the same symlink reason `sessions_for` gives:
        // `/tmp` and `/private/tmp` are the same directory and not the same
        // string, and a rollout records whichever one it was started under.
        let resolved = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        let wanted = [
            directory.to_string_lossy().into_owned(),
            resolved.to_string_lossy().into_owned(),
        ];

        let mut files = Vec::new();
        collect_rollouts(root, &mut files, 4);
        files.sort_by_key(|(_, at)| std::cmp::Reverse(*at));

        files
            .into_iter()
            .find(|(path, _)| {
                first_record(path)
                    .and_then(|record| {
                        record
                            .get("payload")?
                            .get("cwd")?
                            .as_str()
                            .map(str::to_owned)
                    })
                    .is_some_and(|cwd| wanted.contains(&cwd))
            })
            .map(|(path, _)| {
                let meta = first_record(&path).and_then(|record| record.get("payload").cloned());
                let text = |key: &str| {
                    meta.as_ref()
                        .and_then(|payload| payload.get(key))
                        .and_then(serde_json::Value::as_str)
                        .filter(|found| !found.is_empty())
                        .map(str::to_owned)
                };
                Self {
                    session: text("id").or_else(|| text("session_id")),
                    cwd: text("cwd"),
                    path,
                    offset: 0,
                    model: None,
                    effort: None,
                }
            })
    }

    /// Codex's own id for this session.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// The directory it is working in.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// The model in force as of the last turn seen.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The reasoning effort in force as of the last turn seen.
    pub fn effort(&self) -> Option<&str> {
        self.effort.as_deref()
    }

    /// The file being followed.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The newest `token_count` payload appended since the last poll.
    ///
    /// `None` when nothing has been written since — the common case, because
    /// this is polled on the same tick as everything else and a session spends
    /// most of its time between requests.
    ///
    /// The *newest* rather than all of them: the counts these carry are
    /// cumulative, so an earlier record in the same batch is a value that has
    /// already been superseded.
    pub fn poll(&mut self) -> Option<serde_json::Value> {
        use std::io::{Read, Seek, SeekFrom};

        let mut file = std::fs::File::open(&self.path).ok()?;
        let end = file.metadata().ok()?.len();

        // Shorter than where we left off: the file was truncated, or a store
        // someone tidied put a different session behind the same name. Reading
        // from a stale offset would splice two files together, so start again.
        if end < self.offset {
            self.offset = 0;
        }
        if end == self.offset {
            return None;
        }

        let mut start = self.offset;
        let mut landed_mid_line = false;
        if end - start > TAIL_BYTES {
            start = end - TAIL_BYTES;
            landed_mid_line = true;
        }

        file.seek(SeekFrom::Start(start)).ok()?;
        // Bytes, not a string: a seek to a byte offset can land inside a
        // multi-byte character, and `read_to_string` would fail the whole poll
        // over one that is about to be discarded anyway.
        let mut bytes = Vec::new();
        file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
        self.offset = end;

        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines();
        if landed_mid_line {
            lines.next();
        }

        // Backwards, stopping at the first of each: a batch of appended lines
        // costs one JSON parse per record type rather than one per line, and
        // the `contains` is a cheap way to not parse the transcript records
        // that make up most of the file. Newest wins for both — a token count
        // is cumulative, and a turn's settings supersede the last turn's.
        let mut tokens = None;
        let mut turn = None;
        for line in lines.rev() {
            let wants_tokens = tokens.is_none() && line.contains("token_count");
            let wants_turn = turn.is_none() && line.contains("turn_context");
            if !wants_tokens && !wants_turn {
                continue;
            }
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let kind = record.get("type").and_then(serde_json::Value::as_str);
            let Some(payload) = record.get("payload") else {
                continue;
            };
            if wants_turn && kind == Some("turn_context") {
                turn = Some(payload.clone());
            }
            if wants_tokens
                && payload.get("type").and_then(serde_json::Value::as_str) == Some("token_count")
            {
                tokens = Some(payload.clone());
            }
            if tokens.is_some() && turn.is_some() {
                break;
            }
        }

        if let Some(turn) = turn {
            let text = |key: &str| {
                turn.get(key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|found| !found.is_empty())
                    .map(str::to_owned)
            };
            // Only overwrite what the turn actually named: a later turn that
            // omits one of these has not unset it.
            if let Some(model) = text("model") {
                self.model = Some(model);
            }
            if let Some(effort) = text("effort") {
                self.effort = Some(effort);
            }
        }

        tokens
    }
}

/// Session titles from codex's index, by id.
fn codex_titles() -> BTreeMap<String, String> {
    let path = home().join(".codex").join("session_index.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };

    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|record| {
            let id = record.get("id")?.as_str()?.to_owned();
            let name = record.get("thread_name")?.as_str()?.to_owned();
            Some((id, name))
        })
        .collect()
}

// ---- opencode ---------------------------------------------------------------

/// OpenCode: one SQLite database, holding sessions *and* credentials.
///
/// Read through the `sqlite3` binary rather than by linking a SQLite crate.
/// Two reasons, and the second is the important one: it keeps a C dependency
/// out of `ket-core` for one query, and it makes the read a literal statement
/// naming four columns of one table — so there is no code path here that
/// could reach the `credential` table even by mistake.
pub struct OpencodeSessions {
    /// The database file.
    database: PathBuf,
}

impl Default for OpencodeSessions {
    fn default() -> Self {
        Self {
            database: opencode_database(),
        }
    }
}

/// OpenCode's database, `~/.local/share/opencode/opencode.db`.
pub(crate) fn opencode_database() -> PathBuf {
    home()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db")
}

impl OpencodeSessions {
    /// Sessions read from a specific database file.
    pub fn at(database: PathBuf) -> Self {
        Self { database }
    }
}

impl SessionSource for OpencodeSessions {
    fn agent(&self) -> &str {
        "opencode"
    }

    fn sessions_for(&self, directories: &[&Path]) -> Result<Vec<DiscoveredSession>> {
        if !self.database.is_file() || directories.is_empty() {
            return Ok(Vec::new());
        }

        // One statement for every directory, so the poll costs one spawn
        // rather than one per worktree.
        let list = directories
            .iter()
            .map(|d| format!("'{}'", d.to_string_lossy().replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");

        // Read-only, explicit columns, one table, parameter passed as an
        // argument rather than pasted into the statement.
        let output = std::process::Command::new("sqlite3")
            .arg("-readonly")
            .arg("-json")
            .arg(&self.database)
            .arg(format!(
                "SELECT id, title, directory, time_updated FROM session \
                 WHERE directory IN ({list}) ORDER BY time_updated DESC LIMIT {};",
                MAX_SESSIONS_PER_SOURCE * directories.len()
            ))
            .output()
            .map_err(|e| crate::KetError::io(&self.database, e))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).unwrap_or_default();

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(DiscoveredSession {
                    agent: "opencode".to_owned(),
                    id: row.get("id")?.as_str()?.to_owned(),
                    title: row.get("title").and_then(|t| t.as_str()).map(str::to_owned),
                    explicit_title: None,
                    directory: PathBuf::from(row.get("directory")?.as_str()?),
                    updated_at_ms: row
                        .get("time_updated")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default(),
                })
            })
            .collect())
    }

    fn resume_args(&self, session: &str) -> Vec<String> {
        vec!["--session".to_owned(), session.to_owned()]
    }
}

// ---- gemini -----------------------------------------------------------------

/// Gemini CLI: a slot, not an implementation.
///
/// It is not installed on any machine this has been developed against, so
/// where it keeps its sessions and how it resumes one are both unknown. A
/// source that confidently returns nothing is honest; one that guessed at a
/// path and a flag would be a bug that only appears on someone else's machine.
pub struct GeminiSessions;

impl SessionSource for GeminiSessions {
    fn agent(&self) -> &str {
        "gemini"
    }

    fn sessions_for(&self, _directories: &[&Path]) -> Result<Vec<DiscoveredSession>> {
        Ok(Vec::new())
    }

    fn resume_args(&self, _session: &str) -> Vec<String> {
        Vec::new()
    }
}

// ---- grok -------------------------------------------------------------------

/// Grok: one directory per session under its cwd's directory.
///
/// `$GROK_HOME/sessions/<cwd>/<id>/`, where `<cwd>` is the resolved working
/// directory run through JavaScript's `encodeURIComponent` — `/private/tmp/x`
/// is filed as `%2Fprivate%2Ftmp%2Fx`. Each session directory holds a
/// transcript and a `summary.json` naming it. Checked against Grok 1.0.44 on
/// 2026-09-29, as was `--resume <id>`.
///
/// Only `summary.json` is read. The sessions directory's parent also holds
/// Grok's `auth.json`, so nothing here lists or opens anything above a cwd's
/// own directory.
///
/// A cwd whose encoded name is over 255 bytes is filed under a shortened
/// name this does not reproduce, so a very deep worktree finds nothing.
pub struct GrokSessions {
    /// The `sessions` directory.
    root: PathBuf,
}

impl Default for GrokSessions {
    fn default() -> Self {
        Self {
            root: grok_sessions_dir(),
        }
    }
}

/// Grok's `sessions` directory: `$GROK_HOME/sessions`, else
/// `~/.grok/sessions`.
pub(crate) fn grok_sessions_dir() -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home().join(".grok"), PathBuf::from)
        .join("sessions")
}

impl GrokSessions {
    /// Sessions read from a specific `sessions` directory.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    fn for_one(&self, directory: &Path) -> Result<Vec<DiscoveredSession>> {
        // Grok files the resolved cwd; the given one is tried as well in case
        // a later version stops resolving it.
        let resolved = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        let dir = [resolved.as_path(), directory]
            .into_iter()
            .map(|path| {
                self.root
                    .join(encode_uri_component(&path.to_string_lossy()))
            })
            .find(|path| path.is_dir());
        let Some(dir) = dir else {
            return Ok(Vec::new());
        };

        let mut found = Vec::new();
        let entries = std::fs::read_dir(&dir).map_err(|e| crate::KetError::io(&dir, e))?;
        for entry in entries.flatten() {
            let summary = entry.path().join("summary.json");
            let Ok(meta) = std::fs::metadata(&summary) else {
                continue;
            };
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let updated_at_ms = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_millis() as u64)
                .unwrap_or_default();
            found.push((id, updated_at_ms, summary));
        }

        found.sort_by_key(|(_, updated_at_ms, _)| std::cmp::Reverse(*updated_at_ms));
        found.truncate(MAX_SESSIONS_PER_SOURCE);

        Ok(found
            .into_iter()
            .map(|(id, updated_at_ms, summary)| {
                let summary: Option<serde_json::Value> = std::fs::read(&summary)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                let field = |key: &str| {
                    summary
                        .as_ref()
                        .and_then(|s| s.get(key))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                };
                DiscoveredSession {
                    agent: "grok".to_owned(),
                    id,
                    title: field("generated_title").or_else(|| field("session_summary")),
                    explicit_title: None,
                    directory: directory.to_path_buf(),
                    updated_at_ms,
                }
            })
            .collect())
    }
}

impl SessionSource for GrokSessions {
    fn agent(&self) -> &str {
        "grok"
    }

    fn sessions_for(&self, directories: &[&Path]) -> Result<Vec<DiscoveredSession>> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for directory in directories {
            sessions.extend(self.for_one(directory)?);
        }
        Ok(sessions)
    }

    fn resume_args(&self, session: &str) -> Vec<String> {
        vec!["--resume".to_owned(), session.to_owned()]
    }
}

/// `text` as JavaScript's `encodeURIComponent` would write it: every byte
/// outside `A-Z a-z 0-9 - _ . ! ~ * ' ( )` as an uppercase `%XX`.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ---- shared -----------------------------------------------------------------

/// Where Claude Code keeps its state, as the agent ket launches will see it.
///
/// `~/.claude` unless something says otherwise — the rule the agent itself
/// follows, except that the agent reads `CLAUDE_CONFIG_DIR` from *its*
/// environment and ket used to read it from its own. Getting this wrong is
/// invisible and total: ket looked in `~/.claude` while every agent it
/// launched was reading and writing `~/.claude-personal`, so no session was
/// ever found and no hook ever ran. Nothing errored, because an empty
/// directory and a directory of sessions look the same to code that only
/// counts what it finds. [`configured_claude_config_dir`] is where the answer
/// comes from now.
pub fn claude_config_dir() -> PathBuf {
    configured_claude_config_dir().unwrap_or_else(|| home().join(".claude"))
}

/// The directory an explicit setting names, if anything does.
///
/// Asked of the login shell rather than read from ket's environment. ket
/// launches an agent by typing its configured line at a real prompt (see
/// [`crate::shell`]), and that line can carry its own `CLAUDE_CONFIG_DIR`:
/// `claude-personal` as an alias for `CLAUDE_CONFIG_DIR=~/.claude-personal
/// claude` is the shape that bit. A variable exported to ket reaches the
/// shell too, so it is still honoured — the shell reports it unless the line
/// overrides it. Only when the shell cannot be asked does ket's own
/// environment stand alone, which is the old rule and the right fallback.
///
/// The line is whatever the configured `claude` agent launches with, or
/// `claude` when nothing configures one. The answer is cached with the other
/// shell probes for the life of the process and forgotten with them by
/// [`crate::shell::clear_probe_cache`]. A first call is a whole shell
/// startup, so a thread that must not stall should not be the first to ask.
///
/// `None` is the default install: nothing exported, nothing in the line.
pub fn configured_claude_config_dir() -> Option<PathBuf> {
    let line = crate::config::Config::load()
        .ok()
        .and_then(|config| config.agent("claude").map(|agent| agent.launch_line(&[])))
        .unwrap_or_else(|| "claude".to_owned());
    claude_config_dir_seen_by(&line)
}

/// Every directory Claude Code keeps state in for a session ket launches:
/// [`claude_config_dir`] first, then each one a project's own `claude`
/// command points at (see [`crate::project::ProjectSettings::agents`]).
///
/// What lets a project that runs `claude-work` while every other runs
/// `claude-personal` still have its sessions found and its hooks installed:
/// without it, ket looks in one account's directory while that project's
/// agents write to another's — the same invisible, total failure
/// [`claude_config_dir`] describes. Each line is one cached shell probe.
pub fn claude_config_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![claude_config_dir()];
    let lines = crate::store::Store::open_default()
        .and_then(|store| store.load())
        .map(|state| {
            state
                .project_settings
                .values()
                .filter_map(|settings| settings.agents.get("claude"))
                .map(|launch| crate::shell::line(&launch.command, &launch.args))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for line in lines {
        let dir = claude_config_dir_seen_by(&line).unwrap_or_else(|| home().join(".claude"));
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// The `CLAUDE_CONFIG_DIR` a session started by typing `line` would see.
fn claude_config_dir_seen_by(line: &str) -> Option<PathBuf> {
    // The agent's own nesting guard is set for the probe so that, should the
    // line reach a real `claude` after all — a function wrapping a script,
    // say — it refuses to start rather than opening a session on a pipe.
    crate::shell::variable_seen_by(line, "claude", "CLAUDE_CONFIG_DIR", &[("CLAUDECODE", "1")])
        .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok())
        .map(|dir| dir.trim().to_owned())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// The user's home directory, or the current directory if there is none.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A directory entry's modification time in epoch milliseconds.
fn modified_ms(entry: &std::fs::DirEntry) -> u64 {
    entry
        .metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// The first JSON record in a JSONL file, reading no more than
/// [`METADATA_BYTES`].
fn first_record(path: &Path) -> Option<serde_json::Value> {
    let head = read_head(path)?;
    let line = head.lines().next()?;
    serde_json::from_str(line).ok()
}

/// The first thing a person typed, as a session's title.
///
/// Best effort: an agent that records no user turn in its first pages simply
/// has no title, which reads better than a title invented from a system prompt.
fn first_user_line(path: &Path) -> Option<String> {
    let head = read_head(path)?;

    for line in head.lines() {
        let record: serde_json::Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => continue,
        };

        if record.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }

        // Not everything filed as a user turn was typed by one. The agent
        // records its own scaffolding the same way — the caveat above a
        // command's output, the `<command-name>` a slash command expands to —
        // and a row titled "Caveat: The messages below were generated by…" is
        // what happens when they are taken at face value.
        if record.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
            continue;
        }

        let text = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .or_else(|| record.get("content").and_then(|c| c.as_str()))?;

        let text = text.trim();
        if text.starts_with('<') {
            continue;
        }

        let trimmed: String = text.chars().take(120).collect();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    None
}

/// The name Claude gave a conversation, where it has named one.
///
/// Claude writes an `ai-title` record — its own summary of what the session
/// turned out to be about — and rewrites it as the conversation moves on.
/// That is a far better row label than the first thing anybody typed, which
/// is what a title had to be before this: "implement the next free" names the
/// prompt, not the session.
///
/// Read from the *end* of the file, which is the whole reason this is not one
/// line inside [`first_user_line`]. These records are appended, so the head
/// this module reads for metadata never contains one, and the newest is the
/// last rather than the first.
fn ai_title(path: &Path) -> Option<String> {
    let tail = read_tail(path)?;

    // Backwards: the file holds every title the session has had, and the one
    // worth showing is the one it has now.
    for line in tail.lines().rev() {
        let record: serde_json::Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => continue,
        };

        if record.get("type").and_then(|t| t.as_str()) != Some("ai-title") {
            continue;
        }

        let title: String = record
            .get("aiTitle")
            .and_then(|t| t.as_str())?
            .trim()
            .chars()
            .take(120)
            .collect();

        if !title.is_empty() {
            return Some(title);
        }
    }

    None
}

/// The name set by Claude's `/rename` command, when present.
fn custom_title(path: &Path) -> Option<String> {
    let tail = read_tail(path)?;

    for line in tail.lines().rev() {
        let record: serde_json::Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => continue,
        };
        if record.get("type").and_then(|t| t.as_str()) != Some("custom-title") {
            continue;
        }
        let title: String = record
            .get("customTitle")
            .and_then(|title| title.as_str())?
            .trim()
            .chars()
            .take(120)
            .collect();
        if !title.is_empty() {
            return Some(title);
        }
    }

    None
}

/// The last [`METADATA_BYTES`] of a file, as text.
///
/// The first line is dropped when the file is longer than that, because a
/// read that starts mid-file starts mid-line and half a JSON record parses as
/// nothing anyway.
fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let from = length.saturating_sub(METADATA_BYTES as u64);
    file.seek(SeekFrom::Start(from)).ok()?;

    let mut buffer = Vec::new();
    file.take(METADATA_BYTES as u64)
        .read_to_end(&mut buffer)
        .ok()?;
    let text = String::from_utf8_lossy(&buffer).into_owned();

    if from == 0 {
        return Some(text);
    }
    text.split_once('\n').map(|(_, rest)| rest.to_owned())
}

/// The first [`METADATA_BYTES`] of a file, as text.
fn read_head(path: &Path) -> Option<String> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let mut head = Vec::new();
    file.take(METADATA_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(String::from_utf8_lossy(&head).into_owned())
}
