//! Level packs: the token-reduction table, fetched rather than compiled in.
//!
//! ket ships five levels in [`crate::worktree`]. A *pack* is the same table as
//! a JSON document, so the wording an agent is actually handed can be authored
//! once and shipped to machines that are not this one — which is the whole
//! point, and also the whole risk, because the instruction in a pack lands in
//! every agent's context at session start.
//!
//! ## Where a pack comes from
//!
//! One URL, configured once at setup, pointing at a repository the people
//! shipping the pack control. Never a URL the reader of the pack supplies:
//! that would turn a config file into an instruction channel aimed at their own
//! agents. Pinning the URL to a tag or a commit — a GitHub raw URL bakes the
//! SHA into the path — makes a bad publish "one machine is a version behind"
//! rather than "everyone updated at once".
//!
//! ## Why this shells out instead of linking an HTTP client
//!
//! Nothing else in `ket-core` opens a socket, and this module deliberately does
//! not become the first thing that does. The house precedent is already set
//! twice: [`crate::sessions`] reads OpenCode's database through the `sqlite3`
//! binary to keep a C dependency out of the crate, and [`crate::rate_limits`]
//! gets both providers' quota by running their own CLIs rather than
//! reimplementing their APIs. One JSON document over HTTPS does not earn a
//! TLS stack, a connection pool and their transitive trees in a crate that
//! currently has none of them — `curl` is present on every machine ket runs on,
//! ket's own hook script already uses it, and the arguments below are passed as
//! argv rather than through a shell, so a URL cannot become a command.
//!
//! ## What a failure does
//!
//! Nothing, loudly. A fetch that fails, a document that will not parse, a
//! schema from a future ket — each of them leaves the previously cached pack in
//! place, and with no cached pack the built-in table is what a launch uses.
//! There is no state in which a pack problem stops a worktree opening, because
//! the cost of that failure is somebody unable to work and the cost of the
//! alternative is a level reading "no reduction" for an hour.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{KetError, Result};
use crate::worktree::{EconomyPolicy, TokenReduction};

/// The document shape this ket understands.
///
/// Bumped only when the shape changes in a way an older ket could misread — a
/// new required field, or an existing one changing meaning. Adding an optional
/// field does not need it, because an older ket ignores what it does not know
/// and a newer one defaults it.
pub const SCHEMA_VERSION: u32 = 2;

/// Provider-specific launch settings in a schema-v2 pack.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackPolicy {
    /// Reasoning effort.
    #[serde(default)]
    pub effort: Option<String>,
    /// Main model.
    #[serde(default)]
    pub model: Option<String>,
    /// Prompt-cache lifetime.
    #[serde(default)]
    pub cache_ttl: Option<String>,
    /// Automatic compaction threshold or mode.
    #[serde(default)]
    pub autocompact: Option<String>,
    /// Subagent model.
    #[serde(default)]
    pub subagent_model: Option<String>,
    /// Subagent cache lifetime.
    #[serde(default)]
    pub subagent_cache_ttl: Option<String>,
    /// Whether agent teams are enabled.
    #[serde(default)]
    pub agent_teams: Option<bool>,
    /// Cross-session message policy.
    #[serde(default)]
    pub cross_session_inbound: Option<String>,
    /// Response verbosity.
    #[serde(default)]
    pub verbosity: Option<String>,
    /// Reasoning-summary mode.
    #[serde(default)]
    pub reasoning_summary: Option<String>,
}

impl From<&PackPolicy> for EconomyPolicy {
    fn from(policy: &PackPolicy) -> Self {
        Self {
            effort: policy.effort.clone(),
            model: policy.model.clone(),
            cache_ttl: policy.cache_ttl.clone(),
            autocompact: policy.autocompact.clone(),
            subagent_model: policy.subagent_model.clone(),
            subagent_cache_ttl: policy.subagent_cache_ttl.clone(),
            agent_teams: policy.agent_teams,
            cross_session_inbound: policy.cross_session_inbound.clone(),
            verbosity: policy.verbosity.clone(),
            reasoning_summary: policy.reasoning_summary.clone(),
        }
    }
}

/// How long to wait on the network before giving up.
///
/// Short on purpose. Nothing waits on this — the fetch happens on a background
/// thread and a launch reads the disk — so a longer timeout buys nothing except
/// a thread parked on a dead host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The whole fetch's ceiling, connection included.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

/// Most bytes a pack may be.
///
/// A pack is a handful of levels and their wording. Anything past this is not
/// a pack, and reading it into memory before finding that out is how a bad URL
/// becomes an allocation.
const MAX_BYTES: u64 = 256 * 1024;

/// One level, as a pack writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackLevel {
    /// Stable across pack versions, and what a usage record is compared by.
    pub id: String,
    /// How the picker names it.
    pub label: String,
    /// The picker's one-line explanation.
    pub description: String,
    /// What a sidebar row flags it as, if anything.
    #[serde(default)]
    pub tag: Option<String>,
    /// The sentence the agent is handed at session start.
    #[serde(default)]
    pub instruction: Option<String>,
    /// The policy this level asks for. All optional, and all absent means a
    /// level that changes wording and nothing else.
    #[serde(default)]
    pub effort: Option<String>,
    /// See [`PackLevel::effort`].
    #[serde(default)]
    pub model: Option<String>,
    /// See [`PackLevel::effort`].
    #[serde(default)]
    pub cache_ttl: Option<String>,
    /// See [`PackLevel::effort`].
    #[serde(default)]
    pub autocompact: Option<String>,
    /// The model a subagent runs on, when this level names one.
    ///
    /// A separate field from [`PackLevel::model`] on purpose: a subagent runs
    /// tests, reads documentation and greps logs, and the cheapest model that
    /// can do that is rarely the one the conversation itself wants.
    #[serde(default)]
    pub subagent_model: Option<String>,
    /// A subagent's cache lifetime. Subagents fall outside the main
    /// conversation's TTL bucket and get five minutes even on a subscription.
    #[serde(default)]
    pub subagent_cache_ttl: Option<String>,
    /// Whether agent teams are available at all under this level.
    ///
    /// `Some(false)` and `None` are the same absence to a launch — the variable
    /// is simply not set — but they are different statements in a pack, and
    /// the explicit one is worth being able to write.
    #[serde(default)]
    pub agent_teams: Option<bool>,
    /// Whether another session's messages may interrupt this one — `hold` or
    /// `refuse`. Background spend a reader never asked for; see
    /// [`crate::worktree::launch_settings`].
    #[serde(default)]
    pub cross_session_inbound: Option<String>,
    /// How much the model writes — `low`, `medium`, `high`. Codex only; see
    /// [`crate::worktree::TokenReduction::verbosity`].
    #[serde(default)]
    pub verbosity: Option<String>,
    /// Whether reasoning summaries are emitted — `auto`, `concise`, `detailed`,
    /// `none`. Codex only.
    #[serde(default)]
    pub reasoning_summary: Option<String>,
    /// Claude Code settings (schema v2).
    #[serde(default)]
    pub claude: PackPolicy,
    /// Codex settings (schema v2).
    #[serde(default)]
    pub codex: PackPolicy,
    /// OpenCode settings (schema v2).
    #[serde(default)]
    pub opencode: PackPolicy,
    /// Grok settings (schema v2). Grok takes `model` and `effort`; nothing
    /// else in a policy applies to it.
    #[serde(default)]
    pub grok: PackPolicy,
}

/// A whole pack.
///
/// `deny_unknown_fields` on purpose: a key this ket does not know is either a
/// typo or a document from a newer schema that forgot to say so, and both are
/// better refused than half-read. The [`SCHEMA_VERSION`] gate is what a
/// *deliberate* future shape passes through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pack {
    /// The document shape — see [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Which pack this is, across every version of it.
    pub pack_id: String,
    /// Which version of that pack, bumped on every publish.
    pub pack_version: u32,
    /// A human name, for wherever a person sees the pack named.
    #[serde(default)]
    pub name: Option<String>,
    /// When it was published, ISO 8601.
    #[serde(default)]
    pub published_at: Option<String>,
    /// The levels, in the picker's order: least reduction first.
    pub levels: Vec<PackLevel>,
    /// Which level's `id` a worktree gets when nothing has chosen otherwise.
    pub default_level: String,
}

impl Pack {
    /// The levels as the rest of ket reads them.
    ///
    /// The `level` number is assigned from position rather than carried in the
    /// document: it is an index into *this* pack and means nothing outside it,
    /// which is exactly why a record is compared by [`PackLevel::id`] instead.
    /// Counting down from the end matches the built-in table's own order —
    /// least reduction first, and the deepest level numbered zero.
    pub fn levels(&self) -> Vec<TokenReduction> {
        let last = self.levels.len().saturating_sub(1);
        self.levels
            .iter()
            .enumerate()
            .map(|(index, level)| TokenReduction {
                level: (last - index) as u8,
                id: level.id.clone(),
                label: level.label.clone(),
                description: level.description.clone(),
                tag: level.tag.clone(),
                instruction: level.instruction.clone(),
                effort: level.effort.clone(),
                model: level.model.clone(),
                cache_ttl: level.cache_ttl.clone(),
                autocompact: level.autocompact.clone(),
                subagent_model: level.subagent_model.clone(),
                subagent_cache_ttl: level.subagent_cache_ttl.clone(),
                agent_teams: level.agent_teams,
                cross_session_inbound: level.cross_session_inbound.clone(),
                verbosity: level.verbosity.clone(),
                reasoning_summary: level.reasoning_summary.clone(),
                claude: (&level.claude).into(),
                codex: (&level.codex).into(),
                opencode: (&level.opencode).into(),
                grok: (&level.grok).into(),
            })
            .collect()
    }

    /// The `level` number this pack's default corresponds to.
    ///
    /// Falls back to the least-reducing level when `defaultLevel` names an id
    /// the pack does not contain — a pack that contradicts itself should open
    /// worktrees at "no reduction", never at whatever happened to be first.
    pub fn default_level(&self) -> u8 {
        self.levels()
            .into_iter()
            .find(|level| level.id == self.default_level)
            .map(|level| level.level)
            .unwrap_or_else(|| self.levels.len().saturating_sub(1) as u8)
    }
}

/// Reads a pack, checking the schema before anything else.
///
/// The gate runs against the raw document rather than after deserialising it,
/// so a shape this ket has never seen is refused *as a version* rather than as
/// a list of fields it could not understand. That distinction is what a person
/// reading the log needs: "this ket is too old for that pack" is actionable and
/// "unknown field `foo`" is a puzzle.
pub fn parse(bytes: &[u8]) -> Result<Pack> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(KetError::Config(format!(
            "pack is larger than {MAX_BYTES} bytes"
        )));
    }

    let document: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| KetError::Config(format!("pack is not valid JSON: {e}")))?;

    let schema = document
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| KetError::Config("pack has no schemaVersion".to_owned()))?;
    if schema == 0 || schema > u64::from(SCHEMA_VERSION) {
        return Err(KetError::Config(format!(
            "pack uses schema {schema}, and this ket understands {SCHEMA_VERSION} — upgrade ket \
             to use it"
        )));
    }

    let pack: Pack = serde_json::from_value(document)
        .map_err(|e| KetError::Config(format!("pack could not be read: {e}")))?;

    if pack.levels.is_empty() {
        return Err(KetError::Config("pack declares no levels".to_owned()));
    }
    if pack.levels.len() > usize::from(u8::MAX) + 1 {
        return Err(KetError::Config(
            "pack declares more than 256 levels".to_owned(),
        ));
    }
    if pack.pack_id.trim().is_empty() {
        return Err(KetError::Config("pack id is blank".to_owned()));
    }
    if pack.default_level.trim().is_empty() {
        return Err(KetError::Config("pack default level is blank".to_owned()));
    }
    if pack.levels.iter().any(|level| level.id.trim().is_empty()) {
        return Err(KetError::Config("pack has a blank level id".to_owned()));
    }
    if !pack
        .levels
        .iter()
        .any(|level| level.id == pack.default_level)
    {
        return Err(KetError::Config(format!(
            "pack default level `{}` does not exist",
            pack.default_level
        )));
    }
    // Ids are what records are compared by, so two levels sharing one would
    // silently merge two different settings in every later comparison.
    let mut ids: Vec<&str> = pack.levels.iter().map(|level| level.id.as_str()).collect();
    ids.sort_unstable();
    let unique = ids.len();
    ids.dedup();
    if ids.len() != unique {
        return Err(KetError::Config(
            "pack has two levels with the same id".to_owned(),
        ));
    }
    for level in &pack.levels {
        validate_policy(
            level.verbosity.as_deref(),
            level.reasoning_summary.as_deref(),
            level.cross_session_inbound.as_deref(),
            level.autocompact.as_deref(),
        )?;
        for policy in [&level.claude, &level.codex, &level.opencode, &level.grok] {
            validate_policy(
                policy.verbosity.as_deref(),
                policy.reasoning_summary.as_deref(),
                policy.cross_session_inbound.as_deref(),
                policy.autocompact.as_deref(),
            )?;
        }
    }

    Ok(pack)
}

fn validate_policy(
    verbosity: Option<&str>,
    reasoning_summary: Option<&str>,
    cross_session_inbound: Option<&str>,
    autocompact: Option<&str>,
) -> Result<()> {
    if verbosity.is_some_and(|value| !matches!(value, "low" | "medium" | "high")) {
        return Err(KetError::Config(
            "pack verbosity must be low, medium, or high".to_owned(),
        ));
    }
    if reasoning_summary
        .is_some_and(|value| !matches!(value, "auto" | "concise" | "detailed" | "none"))
    {
        return Err(KetError::Config(
            "pack reasoning summary must be auto, concise, detailed, or none".to_owned(),
        ));
    }
    if cross_session_inbound.is_some_and(|value| !matches!(value, "accept" | "hold" | "refuse")) {
        return Err(KetError::Config(
            "pack cross-session policy must be accept, hold, or refuse".to_owned(),
        ));
    }
    if autocompact.is_some_and(|value| {
        value != "auto" && value.parse::<u64>().ok().is_none_or(|tokens| tokens == 0)
    }) {
        return Err(KetError::Config(
            "pack autocompact must be auto or a positive token count".to_owned(),
        ));
    }
    Ok(())
}

/// Where the last good pack is kept.
///
/// The cache directory rather than the data directory: this is a copy of
/// something that lives elsewhere and can be fetched again, which is precisely
/// what that directory is for. Losing it costs one fetch.
pub fn cache_path() -> Result<PathBuf> {
    Ok(crate::paths::cache_dir()?.join("pack.json"))
}

/// The cached pack, or nothing.
///
/// Every failure is `None`: no file yet, a half-written one, a document from a
/// newer ket. The caller's fallback is the built-in table, and a pack problem
/// must never be the reason a worktree will not open — see this module's own
/// header, and [`crate::usage::History::load`], which makes the same bargain
/// for the same reason.
pub fn load() -> Option<Pack> {
    let path = cache_path().ok()?;
    let bytes = std::fs::read(&path).ok()?;
    match parse(&bytes) {
        Ok(pack) => Some(pack),
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "cached pack unusable; using built-in levels");
            None
        }
    }
}

/// Writes a pack to the cache, atomically.
///
/// Temp file then rename, in the same directory so the rename cannot cross a
/// filesystem — the shape [`crate::usage::History::save`] already uses. A
/// process killed mid-write leaves the previous pack intact rather than a
/// truncated one for the next launch to choke on.
pub fn store(bytes: &[u8]) -> Result<()> {
    store_at(&cache_path()?, bytes)
}

/// [`store`], to an explicit path.
fn store_at(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
    }

    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| KetError::io(&temp, e))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        KetError::io(path, e)
    })
}

/// Fetches a pack document over HTTPS.
///
/// Through `curl`, for the reasons in this module's header. The arguments go
/// as argv and never through a shell, so a URL carrying shell metacharacters is
/// a URL and not a command.
///
/// Redirects are followed but bounded: a raw-content URL redirects once or
/// twice by design, and an unbounded chain is a way to make a fetch take
/// forever without ever failing.
pub fn fetch(url: &str) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        return Err(KetError::Config(format!(
            "pack URL must be https, got {url}"
        )));
    }

    let output = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        // Anything but success is a failure here, not a body to parse: an HTML
        // error page is still bytes, and without this it would reach the parser
        // as a malformed pack rather than as the 404 it is.
        .arg("--fail")
        .arg("--location")
        .arg("--max-redirs")
        .arg("3")
        .arg("--connect-timeout")
        .arg(CONNECT_TIMEOUT.as_secs().to_string())
        .arg("--max-time")
        .arg(TOTAL_TIMEOUT.as_secs().to_string())
        .arg("--max-filesize")
        .arg(MAX_BYTES.to_string())
        .arg(url)
        .output()
        .map_err(|e| KetError::Config(format!("could not run curl: {e}")))?;

    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.trim();
        return Err(KetError::Config(format!(
            "fetching the pack failed: {}",
            if reason.is_empty() {
                "curl reported no reason"
            } else {
                reason
            }
        )));
    }

    Ok(output.stdout)
}

/// Where a newly fetched pack waits until somebody adopts it.
///
/// Beside the active one rather than over it. A refresh that overwrote the file
/// a launch reads would make every publish an adoption — every agent
/// would take new wording the next time they opened a worktree, with nobody
/// having chosen it. Whoever ships the pack should be able to publish without
/// that being the same act as deploying.
pub fn staged_path() -> Result<PathBuf> {
    Ok(crate::paths::cache_dir()?.join("pack.staged.json"))
}

/// The pack waiting to be adopted, if one is and it is not already in force.
///
/// `active` is what this run loaded, so a staged copy of the same version — the
/// ordinary case, a refresh finding nothing new — reads as nothing waiting.
pub fn staged(active: Option<&Pack>) -> Option<Pack> {
    let bytes = std::fs::read(staged_path().ok()?).ok()?;
    let pack = parse(&bytes).ok()?;
    match active {
        Some(active)
            if active.pack_id == pack.pack_id && active.pack_version == pack.pack_version =>
        {
            None
        }
        _ => Some(pack),
    }
}

/// Adopts the staged pack, which takes effect at the next launch.
///
/// A rename rather than a copy, so there is no moment where both files claim to
/// be current. Nothing about the running session changes — see
/// [`crate::worktree::activate_pack`] for why the table in force does not move
/// while ket is open.
pub fn promote() -> Result<()> {
    let staged = staged_path()?;
    let active = cache_path()?;
    // Verified once more on the way in: the file has been on disk since the
    // fetch, and adopting something that no longer parses would leave the next
    // launch with no pack rather than the one it had.
    let bytes = std::fs::read(&staged).map_err(|e| KetError::io(&staged, e))?;
    parse(&bytes)?;
    std::fs::rename(&staged, &active).map_err(|e| KetError::io(&active, e))
}

/// Fetches, verifies and caches in one step.
///
/// Verified *before* it is stored, so the cache only ever holds a document that
/// parsed. A cache that could hold a broken pack would turn one bad publish
/// into a permanent one, since the next launch reads the cache rather than the
/// network.
pub fn refresh(url: &str) -> Result<Pack> {
    let bytes = fetch(url)?;
    let pack = parse(&bytes)?;

    // Staged, not adopted. The first pack ever fetched is the exception: there
    // is nothing in force for it to differ from, and asking somebody to adopt
    // the only pack they have is a question with one answer.
    if cache_path().is_ok_and(|path| path.exists()) {
        store_at(&staged_path()?, &bytes)?;
    } else {
        store(&bytes)?;
    }
    Ok(pack)
}

/// How long a temp file is left alone before it counts as abandoned.
///
/// Generously longer than any write takes, because the one thing this must not
/// do is delete the temp file of a *live* write in another ket — the rename
/// would then fail and that process would lose its fetch. A day is far past any
/// plausible in-flight write and far short of leaving litter around for ever.
const TEMP_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Removes temp files left behind by a process that died mid-write.
///
/// [`store_at`] writes `pack.tmp.<pid>` and renames it into place, so a process
/// killed between the two leaves the previous pack correctly intact — verified
/// by SIGKILLing one at eighty different offsets — and a stray temp file beside
/// it. Nothing read those strays, but nothing removed them either.
///
/// Every failure is ignored on purpose. This is housekeeping: a directory that
/// cannot be read, or a file that will not delete because another user owns it,
/// is not a reason to trouble anybody, and the next launch will try again.
pub fn sweep_temps() {
    let Ok(dir) = crate::paths::cache_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        // Both this module's temp files: the active cache's and the staged
        // one's. `pack.json` and `pack.staged.json` themselves do not match,
        // because neither contains `.tmp.`.
        if !name.starts_with("pack.") || !name.contains(".tmp.") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .and_then(|at| at.elapsed().map_err(std::io::Error::other))
            .is_ok_and(|age| age > TEMP_STALE_AFTER);
        if stale {
            tracing::debug!(path = %path.display(), "removing an abandoned pack temp file");
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `path` is the cache this module writes.
///
/// Exposed so a caller can name the file in a message without rebuilding the
/// path and getting it subtly wrong.
pub fn is_cache(path: &Path) -> bool {
    cache_path().is_ok_and(|cache| cache == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `store_at` takes an explicit path, so it is testable directly without
    // touching the real `$XDG_CACHE_HOME`/`$HOME` that `cache_path` and
    // `staged_path` resolve against — see this crate's `paths` module on why
    // those are not mocked by mutating process-wide env.

    #[test]
    fn store_at_writes_the_file_and_leaves_no_temp_behind() {
        let dir = std::env::temp_dir().join(format!("ket-pack-store-at-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("pack.json");

        store_at(&path, b"hello").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        let temps: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(temps.is_empty(), "a successful write leaves no temp file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_at_creates_missing_parent_directories() {
        let dir =
            std::env::temp_dir().join(format!("ket-pack-store-at-mkdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("pack.json");

        store_at(&path, b"data").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"data");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_at_overwrites_an_existing_file_atomically() {
        let dir = std::env::temp_dir().join(format!(
            "ket-pack-store-at-overwrite-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("pack.json");

        store_at(&path, b"first").unwrap();
        store_at(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_cache_recognises_only_the_real_cache_path() {
        assert!(is_cache(&cache_path().unwrap()));
        assert!(!is_cache(Path::new("/definitely/not/the/pack/cache.json")));
    }
}
