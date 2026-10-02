//! Worktrees — one isolated checkout per task.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::{ProjectId, WorktreeId};
use crate::slug;

/// A worktree ket created for a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// Stable identifier, also the worktree's directory name.
    pub id: WorktreeId,
    /// Owning project.
    pub project_id: ProjectId,
    /// The branch checked out here.
    ///
    /// Kept verbatim, including any `/`. This is the authoritative branch name;
    /// [`Worktree::path`] holds only a slug of it.
    pub branch: String,
    /// Optional display name for the worktree row. The git branch is unchanged.
    #[serde(default)]
    pub name: Option<String>,
    /// Last explicit session title imported from an agent.
    ///
    /// Kept separately so a manual rename is not overwritten on every poll;
    /// only a later agent rename replaces it.
    #[serde(default)]
    pub agent_title: Option<String>,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Ref this worktree was branched from, exactly as the caller named it.
    ///
    /// Kept for display and for ahead/behind against a *moving* base: a worktree
    /// branched from `main` should report how far behind `main` it has fallen as
    /// `main` advances, not against wherever `main` happened to be that morning.
    pub base: String,
    /// Commit [`Worktree::base`] resolved to at creation time.
    ///
    /// Bases that do not name a moving branch — a tag, a raw object id, or the
    /// `HEAD` of a repository sitting on a detached checkout — cannot be
    /// re-resolved later and mean something different from inside a linked
    /// worktree, where `HEAD` is the worktree's *own* branch. Freezing the
    /// commit is what keeps ahead/behind honest for those.
    ///
    /// `#[serde(default)]` so state files written before this field existed
    /// still load.
    #[serde(default)]
    pub base_commit: Option<String>,
    /// Which agent this worktree is for — `"claude"`, `"codex"`, `"opencode"`,
    /// `"grok"`.
    ///
    /// Recorded at creation from the project's preferred agent, so the sidebar
    /// can say what a worktree is *for* before anything has run in it. A
    /// session's agent only exists once an agent has started, which is too
    /// late to label the row someone is about to click.
    ///
    /// `None` when no preference applied. `#[serde(default)]` so state files
    /// written before this field existed still load.
    #[serde(default)]
    pub agent: Option<String>,
    /// Creation time, milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    /// When provisioning last completed, or `None` if it has not.
    ///
    /// Set only *after* provisioning succeeds, so an interrupted run leaves a
    /// worktree that is visibly not ready rather than one that looks fine and
    /// fails on the agent's first command. `#[serde(default)]` so state files
    /// written before this field existed still load.
    #[serde(default)]
    pub provisioned_at_ms: Option<u64>,
    /// Repository-relative paths that provisioning created here.
    ///
    /// Recorded rather than recomputed, because the two are not the same thing:
    /// config says `vendor` should be cloned, but a repository that *tracks*
    /// `vendor/` already has one and provisioning left it alone. Tearing down
    /// from config would delete the checkout's own files; tearing down from this
    /// list cannot.
    #[serde(default)]
    pub provisioned_paths: Vec<String>,
    /// How aggressively the agent launched here should cut its own token
    /// usage, from `4` (no reduction) down to `0` (maximum).
    ///
    /// Set from the "Create worktree" dialog and carried as an environment
    /// variable into every agent this worktree launches — see
    /// [`crate::worktree::TOKEN_REDUCTION_ENV`]. `#[serde(default)]` would
    /// read a state file written before this field existed as `0`, which is
    /// maximum reduction rather than the "do nothing different" a missing
    /// value should mean, so the default is spelled out instead.
    #[serde(default = "default_token_reduction")]
    pub token_reduction: u8,
    /// Which level that number *meant*, when a pack was in force.
    ///
    /// Added beside [`Worktree::token_reduction`] rather than replacing it, and
    /// deliberately: the number is an index into whichever table was loaded
    /// when it was written, so it survives a pack that renumbers only if the
    /// name is stored too. Replacing the number outright would have been a
    /// migration of every existing row for a benefit no reader has yet — there
    /// are worktrees on this machine sitting at maximum reduction, and losing
    /// one to a refactor is not a trade worth making.
    ///
    /// `None` on every row written before packs existed, which resolves by
    /// number exactly as it always did.
    #[serde(default)]
    pub token_reduction_id: Option<String>,
    /// The pack in which [`Worktree::token_reduction_id`] was selected.
    ///
    /// Level ids are stable across versions of one pack, not globally. Keeping
    /// the family beside the id prevents an unrelated pack reusing a familiar
    /// id from silently changing an existing worktree's policy.
    #[serde(default)]
    pub token_reduction_pack_id: Option<String>,
    /// Whether the worktree is pinned to the top of the sidebar.
    ///
    /// `#[serde(default)]` so state files written before this field existed
    /// still load, unpinned.
    #[serde(default)]
    pub pinned: bool,
}

/// [`Worktree::token_reduction`] when nothing has chosen otherwise: the active
/// pack's declared default, or no reduction for the built-in catalogue.
pub fn default_token_reduction() -> u8 {
    ACTIVE_PACK
        .get()
        .map(|pack| pack.default_level)
        .unwrap_or(4)
}

/// The environment variable an agent process is launched with, carrying
/// [`Worktree::token_reduction`] as a decimal digit from `"0"` to `"4"`.
///
/// A ket-defined variable rather than an agent-specific flag: agents differ
/// in whether they have a reasoning-effort or verbosity flag at all, and one
/// that every agent can read regardless is the only thing that works across
/// the whole catalogue.
pub const TOKEN_REDUCTION_ENV: &str = "KET_TOKEN_REDUCTION";

/// The environment variable carrying the *sentence* for that level — what a
/// `SessionStart` hook hands the agent as its `additionalContext`.
///
/// Passed alongside [`TOKEN_REDUCTION_ENV`] rather than derived from it by the
/// hook script, because the script would then hold a second copy of
/// [`TOKEN_REDUCTION_LEVELS`] in another language: the dialog would promise
/// one thing and the shell would say another the first time only one of them
/// was edited. Empty or unset means "say nothing", which is what the default
/// level means too.
pub const TOKEN_REDUCTION_NOTE_ENV: &str = "KET_TOKEN_REDUCTION_NOTE";

/// The prompt-cache lifetime a session should be launched with, if any.
///
/// **The cheapest lever there is, and the whole of it is one flag.** An agent
/// loop resends the entire conversation on every turn; the cache reprices that
/// resend at a tenth rather than preventing it. The catch is the lifetime: on a
/// Claude subscription the main conversation gets an hour, but on an API key,
/// a cloud provider, or a subscription drawing on usage credits it gets **five
/// minutes** — so a pause for a meeting reprocesses the whole history at full
/// price on the next message.
///
/// `Some("1h")` only on [`Billing::Metered`], and only ever that. A plan
/// already has the hour, and asking for it again changes nothing. `None` when
/// billing is unknown, which is the state until some session has reported:
/// guessing wrong costs money on every cache write (an hour's TTL writes at 2×
/// against five minutes' 1.25×), so the absence of evidence buys the default
/// rather than a coin flip.
///
/// Claude Code only. Codex's cache lifetime is the platform's to manage and it
/// exposes no equivalent setting, so a Codex launch gets nothing from this.
/// `configured` is the reader's own override from
/// [`crate::config::AgentConfig::prompt_cache_ttl`]: `None` leaves the decision
/// here, `"off"` refuses it outright, and an explicit `"5m"` or `"1h"` is taken
/// as given. An unrecognised value is ignored rather than passed through —
/// Claude Code accepts any settings JSON and **silently ignores keys and values
/// it does not know** (checked: `--settings '{"totallyNotAKey":"x"}'` exits
/// zero without a word), so a typo forwarded from a config file would become a
/// flag that does nothing, invisibly, for as long as it stayed there.
pub fn cache_ttl_for(
    agent: &str,
    billing: Option<crate::usage::Billing>,
    configured: Option<&str>,
) -> Option<String> {
    if !agent.eq_ignore_ascii_case("claude") {
        return None;
    }
    match configured {
        Some("off") => None,
        Some(ttl @ ("5m" | "1h")) => Some(ttl.to_owned()),
        // Unrecognised, or nothing set: ket's own judgement.
        _ => matches!(billing, Some(crate::usage::Billing::Metered)).then(|| "1h".to_owned()),
    }
}

/// The launch-line words carrying every settings key ket wants to set.
///
/// One `--settings` flag with one object, not a flag per key: two `--settings`
/// arguments would be two competing documents, and which of them wins is a
/// question with no documented answer. Composing here means the answer never
/// has to be asked.
///
/// A `--settings` flag rather than environment variables, because it is the one
/// that overrides a settings file the reader may already have: a variable would
/// be beaten by their own `promptCacheTtl` and silently do nothing. The JSON is
/// one argument and reaches the shell through `crate::shell::quote`, which
/// single-quotes anything containing a brace or a quote.
///
/// `None` when there is nothing to say, so a launch ket has no opinion about is
/// byte-for-byte the launch it was before this existed.
pub fn launch_settings(
    cache_ttl: Option<&str>,
    bash_output_max_chars: Option<u32>,
    cross_session_inbound: Option<&str>,
) -> Option<Vec<String>> {
    let mut keys: Vec<String> = Vec::new();
    if let Some(ttl) = cache_ttl {
        keys.push(format!(r#""promptCacheTtl":"{ttl}""#));
    }
    if let Some(chars) = bash_output_max_chars {
        keys.push(format!(r#""bashOutputMaxChars":{chars}"#));
    }
    // Spend with no prompt to blame it on: a message from another session is
    // delivered into this one's context on somebody else's schedule. `hold`
    // shows a notice without delivering, `refuse` declines outright.
    //
    // Only ever tightened, never loosened. Claude Code ranks this on an
    // `accept` < `hold` < `refuse` ladder and lets a *stricter* project or local
    // value win over `--settings`, so a level asking for `accept` could only
    // ever weaken what somebody already chose — and a cost tool has no business
    // turning somebody's inbox back on. An unrecognised value is dropped rather
    // than passed through, because an unknown word here is a typo, and a typo
    // in a settings document is a launch that fails rather than a key ignored.
    match cross_session_inbound.map(str::trim) {
        Some(policy @ ("hold" | "refuse")) => {
            keys.push(format!(r#""crossSessionInbound":"{policy}""#));
        }
        // Said out loud rather than dropped in silence. A level that asked to
        // hold its inbox and quietly did not would be the exact failure this
        // plan keeps catching in other people's settings. `accept` takes this
        // branch too and is not a mistake, so it does not warn.
        Some(policy) if !policy.is_empty() && policy != "accept" => {
            tracing::warn!(
                policy,
                "a level asked for a cross-session inbound policy that is not `hold` or \
                 `refuse`; ignoring it"
            );
        }
        _ => {}
    }
    if keys.is_empty() {
        return None;
    }
    Some(vec![
        "--settings".to_owned(),
        format!("{{{}}}", keys.join(",")),
    ])
}

/// The launch-line words that set when a session compacts itself.
///
/// A flag rather than a key in [`launch_settings`]'s document: `--autocompact`
/// is what Claude Code documents for this, and it takes `auto` or a token
/// count. The value is passed through as the reader wrote it — an agent that
/// rejects it says so at launch, which is a better failure than ket quietly
/// normalising a value it does not own.
pub fn autocompact_args(window: &str) -> Vec<String> {
    vec!["--autocompact".to_owned(), window.to_owned()]
}

/// Codex's equivalent, which counts tokens and nothing else.
///
/// `None` when the configured value is not a plain number: Claude's `auto` and
/// its `500k` shorthand mean nothing here, and forwarding either would be a
/// config line Codex refuses on every launch.
pub fn codex_autocompact_args(window: &str) -> Option<Vec<String>> {
    let tokens: u64 = window.trim().parse().ok()?;
    Some(vec![
        "-c".to_owned(),
        format!("model_auto_compact_token_limit={tokens}"),
    ])
}

/// The `-c` words that cap a Codex tool's output.
///
/// Codex's equivalent of `bashOutputMaxChars`, and a config key rather than a
/// settings document, so it travels as its own flag.
pub fn codex_output_limit_args(tokens: u32) -> Vec<String> {
    vec!["-c".to_owned(), format!("tool_output_token_limit={tokens}")]
}

/// Lines of `CLAUDE.md` past which Anthropic's own guidance says to move
/// material into a skill.
///
/// Their number, not one invented here: "Aim to keep CLAUDE.md under 200 lines
/// by including only essentials." The file is read into context at session
/// start, so every line is paid on every session in that worktree whether or
/// not the work touches what it says.
pub const CLAUDE_MD_LINE_BUDGET: u32 = 200;

/// What a worktree's own instruction files cost every session opened in it.
///
/// Deliberately only the files, and only this worktree's. The user-level
/// `~/.claude/CLAUDE.md` is paid everywhere equally, so it is a constant rather
/// than something one row can be blamed for, and nested files load on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextWeight {
    /// Lines in the worktree's `CLAUDE.md`, when it has one.
    pub claude_md_lines: Option<u32>,
    /// Bytes in its `AGENTS.md`, which is what Codex measures.
    pub agents_md_bytes: Option<u64>,
    /// Codex's own configured ceiling for that file, when it has one set.
    ///
    /// Read rather than assumed: `project_doc_max_bytes` has a default this
    /// code does not know, and warning against a guessed limit would be worse
    /// than not warning. `None` means Codex's ceiling is unknown here, and
    /// nothing is claimed about the file's size.
    pub agents_md_budget: Option<u64>,
}

impl ContextWeight {
    /// The one sentence worth saying, or nothing.
    ///
    /// Nothing is the usual answer, and the point: a worktree whose files are
    /// within budget has a row that never appears. "CLAUDE.md: 140 lines" is
    /// not news.
    pub fn warning(&self) -> Option<String> {
        if let Some(lines) = self.claude_md_lines
            && lines > CLAUDE_MD_LINE_BUDGET
        {
            return Some(format!(
                "CLAUDE.md is {lines} lines — every session here pays for all of them"
            ));
        }
        if let (Some(bytes), Some(budget)) = (self.agents_md_bytes, self.agents_md_budget)
            && bytes > budget
        {
            return Some(format!(
                "AGENTS.md is {bytes} bytes, over Codex's {budget}-byte limit — it will be \
                 truncated"
            ));
        }
        None
    }
}

/// Reads a worktree's instruction files.
///
/// At launch, once, never on a repaint: this is a filesystem read, and the
/// answer only changes when someone edits the file — at which point it does not
/// reach a running session anyway, since both agents load these at session
/// start.
pub fn context_weight(directory: &Path) -> ContextWeight {
    let lines = std::fs::read_to_string(directory.join("CLAUDE.md"))
        .ok()
        .map(|text| text.lines().count() as u32);
    let bytes = std::fs::metadata(directory.join("AGENTS.md"))
        .ok()
        .map(|meta| meta.len());

    ContextWeight {
        claude_md_lines: lines,
        agents_md_bytes: bytes,
        agents_md_budget: codex_doc_budget(),
    }
}

/// Codex's configured `project_doc_max_bytes`, if the reader has set one.
///
/// Scanned as text rather than parsed: this is another application's config
/// file, ket wants exactly one integer out of it, and a parse that fails on a
/// key some future Codex adds would lose that integer for no reason.
fn codex_doc_budget() -> Option<u64> {
    let path = crate::paths::home()
        .ok()?
        .join(".codex")
        .join("config.toml");
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("project_doc_max_bytes"))
        .and_then(|rest| rest.trim_start().strip_prefix('='))
        .and_then(|value| value.split('#').next())
        .and_then(|value| value.trim().parse().ok())
}

/// One point on the token-reduction dial, and everything anything says about
/// it.
///
/// The single source: the picker's rows, the sidebar row's tag and the
/// sentence the agent is actually given all read this table. They used to be
/// three separate lists — two in `ket-ui` and one inside the hook script's
/// shell — which is three chances for the level a reader chose to mean
/// something other than what the agent was told.
// Not `Copy` any more: the strings are owned, which is the whole point — a
// level can now come from a document fetched at runtime rather than only from
// this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenReduction {
    /// The stored value, `4` (no reduction) down to `0` (maximum).
    pub level: u8,
    /// How the picker names it.
    pub label: String,
    /// The picker's one-line explanation of what it does.
    pub description: String,
    /// What a sidebar row flags it as, or `None` at the default: a worktree
    /// that reasons and explains freely has nothing to flag, and a tag on
    /// every row says as much as a tag on none.
    pub tag: Option<String>,
    /// The sentence the agent is handed at session start, or `None` when
    /// there is nothing to ask for.
    ///
    /// Written as an instruction to the agent rather than a description of
    /// the level, because that is what it becomes: it arrives as context at
    /// the top of the session and is read as a standing request.
    pub instruction: Option<String>,
    /// A name for this level that survives a pack version bump.
    ///
    /// [`TokenReduction::level`] is the number stored on a worktree and is
    /// only meaningful *within one pack*; this is what a record can be compared
    /// across versions by. The built-in levels use their own stable slugs.
    pub id: String,
    /// The reasoning effort this level asks for, when it asks for one.
    ///
    /// The first of the policy fields a pack can set. All four are `None` on
    /// every built-in level, which is what keeps this change invisible until a
    /// pack is actually loaded: a level that names nothing changes nothing
    /// about the launch it is applied to.
    pub effort: Option<String>,
    /// See [`TokenReduction::effort`].
    pub model: Option<String>,
    /// See [`TokenReduction::effort`].
    pub cache_ttl: Option<String>,
    /// See [`TokenReduction::effort`].
    pub autocompact: Option<String>,
    /// The model a subagent runs on — see [`crate::pack::PackLevel::subagent_model`].
    pub subagent_model: Option<String>,
    /// A subagent's cache lifetime.
    pub subagent_cache_ttl: Option<String>,
    /// Whether agent teams are available under this level.
    pub agent_teams: Option<bool>,
    /// Whether messages from the reader's other sessions reach this one.
    ///
    /// `hold` or `refuse`; anything else is ignored. Real spend with no prompt
    /// to blame it on — see [`launch_settings`], which explains why this is only
    /// ever tightened.
    pub cross_session_inbound: Option<String>,
    /// How much the model writes — `low`, `medium` or `high`.
    ///
    /// **Codex only, and the reason a Codex pack is worth authoring rather
    /// than inheriting one written for Claude.** Every level's `instruction`
    /// *asks* for shorter output; this one caps it, enforced by the API rather
    /// than by persuasion. Claude Code has no equivalent, so a Claude launch
    /// ignores it.
    pub verbosity: Option<String>,
    /// Whether reasoning summaries are emitted — `auto`, `concise`, `detailed`
    /// or `none`.
    ///
    /// Codex only. Summaries are output tokens like any other, and `none` stops
    /// paying for them. See [`TokenReduction::verbosity`].
    pub reasoning_summary: Option<String>,
    /// Provider-specific policy for Claude Code.
    pub claude: EconomyPolicy,
    /// Provider-specific policy for Codex.
    pub codex: EconomyPolicy,
    /// Provider-specific policy for OpenCode.
    pub opencode: EconomyPolicy,
    /// Provider-specific policy for Grok.
    pub grok: EconomyPolicy,
}

/// Launch policy attached to one provider in an Economy pack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EconomyPolicy {
    /// Reasoning effort.
    pub effort: Option<String>,
    /// Main model.
    pub model: Option<String>,
    /// Prompt-cache lifetime.
    pub cache_ttl: Option<String>,
    /// Automatic compaction threshold or mode.
    pub autocompact: Option<String>,
    /// Subagent model.
    pub subagent_model: Option<String>,
    /// Subagent cache lifetime.
    pub subagent_cache_ttl: Option<String>,
    /// Whether agent teams are enabled.
    pub agent_teams: Option<bool>,
    /// Cross-session message policy.
    pub cross_session_inbound: Option<String>,
    /// Response verbosity.
    pub verbosity: Option<String>,
    /// Reasoning-summary mode.
    pub reasoning_summary: Option<String>,
}

impl TokenReduction {
    /// The settings relevant to `agent`, with schema-v1 fields used as the
    /// compatibility base and schema-v2 provider fields layered over them.
    ///
    /// An agent other than Claude Code, Codex, OpenCode or Grok gets the default:
    /// no pack writes for it, so there is nothing to apply.
    pub fn policy_for(&self, agent: &str) -> EconomyPolicy {
        let (mut policy, provider) = if agent.eq_ignore_ascii_case("claude") {
            (
                EconomyPolicy {
                    effort: self.effort.clone(),
                    model: self.model.clone(),
                    cache_ttl: self.cache_ttl.clone(),
                    autocompact: self.autocompact.clone(),
                    subagent_model: self.subagent_model.clone(),
                    subagent_cache_ttl: self.subagent_cache_ttl.clone(),
                    agent_teams: self.agent_teams,
                    cross_session_inbound: self.cross_session_inbound.clone(),
                    ..EconomyPolicy::default()
                },
                &self.claude,
            )
        } else if agent.eq_ignore_ascii_case("codex") {
            (
                EconomyPolicy {
                    effort: self.effort.clone(),
                    model: self.model.clone(),
                    autocompact: self.autocompact.clone(),
                    verbosity: self.verbosity.clone(),
                    reasoning_summary: self.reasoning_summary.clone(),
                    ..EconomyPolicy::default()
                },
                &self.codex,
            )
        } else if agent.eq_ignore_ascii_case("opencode") {
            (EconomyPolicy::default(), &self.opencode)
        } else if agent.eq_ignore_ascii_case("grok") {
            // No schema-v1 base: those fields were written for Claude, and a
            // Claude model name is not one Grok can run.
            (EconomyPolicy::default(), &self.grok)
        } else {
            // An agent with no provider block of its own gets nothing. Falling
            // through to OpenCode's would hand it OpenCode's model name.
            return EconomyPolicy::default();
        };
        macro_rules! overlay {
            ($field:ident) => {
                if provider.$field.is_some() {
                    policy.$field = provider.$field.clone();
                }
            };
        }
        overlay!(effort);
        overlay!(model);
        overlay!(cache_ttl);
        overlay!(autocompact);
        overlay!(subagent_model);
        overlay!(subagent_cache_ttl);
        if provider.agent_teams.is_some() {
            policy.agent_teams = provider.agent_teams;
        }
        overlay!(cross_session_inbound);
        overlay!(verbosity);
        overlay!(reasoning_summary);
        policy
    }

    /// The level's name on its own — "Moderate" rather than "2 — Moderate
    /// reduction" — for a control too narrow for the label, and for a setting
    /// called Economy, where "reduction" is no longer the whole story: a
    /// pack's level can change the model, effort and cache as well as ask for
    /// shorter answers.
    ///
    /// Looked up by [`TokenReduction::id`] rather than stored, so a pack
    /// carrying the built-in levels' ids gets their names without its schema
    /// growing a field. Any other level falls back to its label without the
    /// number.
    pub fn name(&self) -> &str {
        match self.id.as_str() {
            "no-reduction" => "Off",
            "light-reduction" => "Light",
            "moderate-reduction" => "Moderate",
            "heavy-reduction" => "Heavy",
            "maximum-reduction" => "Max",
            _ => self
                .label
                .split_once(" — ")
                .map_or(self.label.as_str(), |(_, name)| name),
        }
    }
}

/// Every level ket ships, in the order a picker shows them: default first,
/// deepest last.
///
/// A built value rather than a `const` array, because [`TokenReduction`] now
/// owns its strings — which is what lets a level come from a pack fetched at
/// runtime instead of from this file. This table is that type's *first* value,
/// not a second shape beside it: when a pack is loaded it replaces what this
/// returns, and every reader goes on reading the same struct.
///
/// Built once. The strings are static text, so this allocates five levels at
/// first use and never again.
static BUILTIN_LEVELS: std::sync::LazyLock<Vec<TokenReduction>> = std::sync::LazyLock::new(|| {
    let level = |level: u8,
                 id: &str,
                 label: &str,
                 description: &str,
                 tag: Option<&str>,
                 instruction: Option<&str>| TokenReduction {
        level,
        id: id.to_owned(),
        label: label.to_owned(),
        description: description.to_owned(),
        tag: tag.map(str::to_owned),
        instruction: instruction.map(str::to_owned),
        // Every built-in level names no policy at all, which is what keeps the
        // owned type invisible until a pack fills these in.
        effort: None,
        model: None,
        cache_ttl: None,
        autocompact: None,
        subagent_model: None,
        subagent_cache_ttl: None,
        agent_teams: None,
        cross_session_inbound: None,
        verbosity: None,
        reasoning_summary: None,
        claude: EconomyPolicy::default(),
        codex: EconomyPolicy::default(),
        opencode: EconomyPolicy::default(),
        grok: EconomyPolicy::default(),
    };

    vec![
        level(
            4,
            "no-reduction",
            "4 — No reduction",
            "The agent reasons and explains as much as it wants.",
            None,
            None,
        ),
        level(
            3,
            "light-reduction",
            "3 — Light reduction",
            "Trims filler and restates less; reasoning stays full.",
            Some("light reduction"),
            Some(
                "Reduce token usage: trim filler and avoid restating what was just said. \
                 Keep reasoning and explanations otherwise full.",
            ),
        ),
        level(
            2,
            "moderate-reduction",
            "2 — Moderate reduction",
            "Shorter explanations and terser output throughout.",
            Some("moderate reduction"),
            Some(
                "Reduce token usage: give shorter explanations and terser output \
                 throughout this session.",
            ),
        ),
        level(
            1,
            "heavy-reduction",
            "1 — Heavy reduction",
            "Minimal narration — mostly just the result.",
            Some("heavy reduction"),
            Some(
                "Reduce token usage heavily: keep narration minimal, state results and \
                 decisions directly, skip recaps and closing summaries.",
            ),
        ),
        level(
            0,
            "maximum-reduction",
            "0 — Maximum reduction",
            "As terse as the agent can manage; answers only.",
            Some("max reduction"),
            Some(
                "Reduce token usage to the maximum: answer as tersely as possible, with \
                 no narration and no explanation unless one is asked for.",
            ),
        ),
    ]
});

/// The pack's levels, once one has been activated.
///
/// A `OnceLock` rather than a lock that can be rewritten, and that is a
/// deliberate limit: the levels in force do not change while ket is running.
/// A newly fetched pack is cached and takes effect at the next launch, which is
/// what [`crate::pack`] stores it for — swapping the table under a running
/// session would change what a worktree's ring means while somebody is looking
/// at it, and would make a level recorded at the start of a session mean
/// something else by the end of it.
///
/// It also keeps every reader's signature unchanged. A table behind a lock
/// could not hand out `&'static`, and every call site would have grown a clone
/// to serve a value that changes once per process.
#[derive(Debug)]
struct ActiveEconomyPack {
    id: String,
    version: u32,
    default_level: u8,
    levels: Vec<TokenReduction>,
}

static ACTIVE_PACK: std::sync::OnceLock<ActiveEconomyPack> = std::sync::OnceLock::new();

/// Puts a pack's levels in force for the rest of this run.
///
/// Called once, at startup, before anything renders. Later calls are ignored
/// rather than queued: two packs in one process is not a state this supports,
/// and silently honouring the last one would be worse than honouring the first.
///
/// A pack with no levels is refused here rather than at every reader — the
/// built-in table stays in force, which is the one guarantee this module makes.
pub fn activate_pack(
    id: String,
    version: u32,
    default_level: u8,
    levels: Vec<TokenReduction>,
) -> bool {
    if levels.is_empty() {
        return false;
    }
    if !levels.iter().any(|level| level.level == default_level) {
        return false;
    }
    ACTIVE_PACK
        .set(ActiveEconomyPack {
            id,
            version,
            default_level,
            levels,
        })
        .is_ok()
}

/// The active pack's id, or the built-in table's name.
pub fn active_pack_id() -> String {
    ACTIVE_PACK
        .get()
        .map(|pack| pack.id.clone())
        .unwrap_or_else(crate::usage::builtin_pack_id)
}

/// The active pack's version, or `0` for the built-in table.
pub fn active_pack_version() -> u32 {
    ACTIVE_PACK.get().map(|pack| pack.version).unwrap_or(0)
}

/// Every level in force: a pack's, when one was activated, else the built-ins.
///
/// The seam a pack arrives through. Every caller already asks here rather than
/// naming a table, so this is the only place that had to learn about packs.
pub fn token_reduction_levels() -> &'static [TokenReduction] {
    ACTIVE_PACK
        .get()
        .map(|pack| pack.levels.as_slice())
        .unwrap_or(&BUILTIN_LEVELS)
}

/// The level number a worktree's stored pair resolves to.
///
/// The name wins when the active table has it, because that is the whole reason
/// it is stored; the number is what answers for every row written before names
/// existed, and for a name belonging to some other pack.
pub fn resolve_level(stored: u8, id: Option<&str>, pack_id: Option<&str>) -> u8 {
    let active_id = active_pack_id();
    if let Some(pack_id) = pack_id {
        if pack_id != active_id {
            return default_token_reduction();
        }
        return id
            .and_then(|id| {
                token_reduction_levels()
                    .iter()
                    .find(|level| level.id == id)
                    .map(|level| level.level)
            })
            .unwrap_or_else(default_token_reduction);
    }

    token_reduction_levels()
        .iter()
        .find(|level| level.level == stored)
        .map(|level| level.level)
        .unwrap_or_else(default_token_reduction)
}

/// Where `level` sits in the active table, and how many levels there are.
///
/// For anything drawing the dial: the built-in five are numbered `4` down to
/// `0`, but a pack may ship three or nine, so a position is the only thing a
/// sweep can honestly be computed from.
pub fn level_position(level: u8) -> Option<(usize, usize)> {
    let levels = token_reduction_levels();
    levels
        .iter()
        .position(|candidate| candidate.level == level)
        .map(|index| (index, levels.len()))
}

/// The level `value` names, falling back to the default rather than failing:
/// an unknown number in a state file someone edited by hand should read as
/// "nothing different", never as maximum reduction.
pub fn token_reduction(value: u8) -> &'static TokenReduction {
    let default = default_token_reduction();
    let levels = token_reduction_levels();
    levels
        .iter()
        .find(|l| l.level == value)
        .or_else(|| levels.iter().find(|l| l.level == default))
        .or_else(|| levels.first())
        .expect("there is always at least one level")
}

/// Arguments and environment produced by one resolved Economy level.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EconomyLaunch {
    /// Agent command-line arguments.
    pub args: Vec<String>,
    /// Agent environment variables.
    pub env: std::collections::BTreeMap<String, String>,
}

/// Translates one provider-aware level into launch arguments and environment.
/// Caller-supplied values are explicit ket settings and therefore take
/// precedence over the pack's suggested cache and compaction values.
pub fn economy_launch(
    agent: &str,
    level: &TokenReduction,
    cache_ttl: Option<&str>,
    output_limit: Option<u32>,
    autocompact: Option<&str>,
) -> EconomyLaunch {
    let policy = level.policy_for(agent);
    let mut launch = EconomyLaunch::default();
    let compact = autocompact.or(policy.autocompact.as_deref());

    if let Some(model) = policy.model.as_deref() {
        launch.args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if agent.eq_ignore_ascii_case("codex") {
        if let Some(effort) = policy.effort.as_deref() {
            launch
                .args
                .extend(["-c".to_owned(), format!("model_reasoning_effort={effort}")]);
        }
        if let Some(verbosity) = policy.verbosity.as_deref() {
            launch
                .args
                .extend(["-c".to_owned(), format!("model_verbosity={verbosity}")]);
        }
        if let Some(summary) = policy.reasoning_summary.as_deref() {
            launch.args.extend([
                "-c".to_owned(),
                format!("model_reasoning_summary={summary}"),
            ]);
        }
        if let Some(limit) = output_limit {
            launch.args.extend(codex_output_limit_args(limit));
        }
        if let Some(words) = compact.and_then(codex_autocompact_args) {
            launch.args.extend(words);
        }
    } else if agent.eq_ignore_ascii_case("grok") {
        // `low`, `medium`, `high`, or `xhigh` on grok-4.6, as of 1.0.44.
        if let Some(effort) = policy.effort.as_deref() {
            launch
                .args
                .extend(["--reasoning-effort".to_owned(), effort.to_owned()]);
        }
    } else if agent.eq_ignore_ascii_case("claude") {
        if let Some(effort) = policy.effort.as_deref() {
            launch
                .args
                .extend(["--effort".to_owned(), effort.to_owned()]);
        }
        if let Some(words) = launch_settings(
            cache_ttl.or(policy.cache_ttl.as_deref()),
            output_limit,
            policy.cross_session_inbound.as_deref(),
        ) {
            launch.args.extend(words);
        }
        if let Some(window) = compact {
            launch.args.extend(autocompact_args(window));
        }
        if let Some(model) = policy.subagent_model {
            launch
                .env
                .insert("CLAUDE_CODE_SUBAGENT_MODEL".to_owned(), model);
        }
        if let Some(ttl) = policy.subagent_cache_ttl {
            launch
                .env
                .insert("CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL".to_owned(), ttl);
        }
        if policy.agent_teams == Some(true) {
            launch.env.insert(
                "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_owned(),
                "1".to_owned(),
            );
        }
    }

    launch
        .env
        .insert(TOKEN_REDUCTION_ENV.to_owned(), level.level.to_string());
    launch.env.insert(
        TOKEN_REDUCTION_NOTE_ENV.to_owned(),
        level.instruction.clone().unwrap_or_default(),
    );
    launch
}

/// Applies the selected Economy policy to a non-interactive agent launch.
///
/// Interactive terminals add user-configured overrides in `ket-ui`; direct
/// `ket run` and Quick Prompt do not pass through that code, so the portable
/// pack policy and environment belong here in core.
pub fn apply_economy(spec: &mut crate::config::AgentSpec, worktree: &Worktree) {
    let resolved = resolve_level(
        worktree.token_reduction,
        worktree.token_reduction_id.as_deref(),
        worktree.token_reduction_pack_id.as_deref(),
    );
    let level = token_reduction(resolved);
    let launch = economy_launch(&spec.name, level, None, None, None);
    // Grok's model and effort flags belong to `grok agent`, and `stdio`
    // rejects them, so they go in before the transport.
    if spec.name == "grok" && spec.args.last().is_some_and(|last| last == "stdio") {
        let at = spec.args.len() - 1;
        spec.args.splice(at..at, launch.args);
    } else {
        spec.args.extend(launch.args);
    }
    spec.env.extend(launch.env);
}

/// A worktree git knows about that ket's registry does not.
///
/// The registry only ever holds what ket itself created — see
/// [`crate::workspace::Workspace::worktrees`] — so a worktree made by a plain
/// `git worktree add`, by another tool, or by an agent's own tooling never
/// appears there. This is what [`crate::workspace::Workspace::worktree_report`]
/// reports it as instead of silently dropping it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWorktree {
    /// Owning project.
    pub project_id: ProjectId,
    /// Absolute path to the worktree.
    pub path: PathBuf,
    /// Branch checked out here. `None` for a detached checkout.
    pub branch: Option<String>,
    /// Commit checked out, if git reported one.
    pub head: Option<String>,
}

/// Derives a worktree's identifier, unique within its project.
///
/// The branch name is *not* usable as a directory name: it may contain `/`,
/// which would nest a directory, and `feature/login` would otherwise collide
/// with a branch called `feature`. The readable half of the slug comes from the
/// branch so the directory is recognisable; the hashed half covers the project
/// and the full branch name, so neither two branches in one project nor the same
/// branch in two projects can collide.
pub fn id_for(project_id: &ProjectId, branch: &str) -> WorktreeId {
    let mut key = Vec::with_capacity(project_id.as_str().len() + branch.len() + 1);
    key.extend_from_slice(project_id.as_str().as_bytes());
    key.push(0);
    key.extend_from_slice(branch.as_bytes());

    WorktreeId::new(slug::slug_keyed(branch, &key))
}

impl Worktree {
    /// Whether the worktree directory still exists on disk.
    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }

    /// Whether provisioning has completed.
    ///
    /// An agent must not be started in a worktree where this is false: it would
    /// hit a missing `node_modules` and spend real money diagnosing an
    /// environment problem rather than the task it was given.
    pub fn is_provisioned(&self) -> bool {
        self.provisioned_at_ms.is_some()
    }

    /// The revision to measure this worktree's commits against.
    ///
    /// Prefers the base *ref* so the answer tracks the branch as it moves, and
    /// falls back to the frozen commit for bases that cannot be re-resolved —
    /// see [`Worktree::base_commit`]. `HEAD` is never used as a ref here: inside
    /// a linked worktree it resolves to the worktree's own branch, which would
    /// make every such worktree look exactly zero commits ahead of itself.
    pub fn base_rev(&self) -> Option<&str> {
        if self.base != "HEAD" && !self.base.is_empty() {
            return Some(&self.base);
        }
        self.base_commit.as_deref()
    }
}

/// Where a worktree lives: `<worktrees_root>/<project id>/<worktree id>`.
pub fn path_for(worktrees_root: &Path, project_id: &ProjectId, id: &WorktreeId) -> PathBuf {
    worktrees_root.join(project_id.as_str()).join(id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectId {
        ProjectId::new("api-0123456789ab")
    }

    #[test]
    fn nested_branches_do_not_nest_directories() {
        let id = id_for(&project(), "feature/login");
        assert!(!id.as_str().contains('/'));
    }

    #[test]
    fn a_branch_does_not_collide_with_its_own_prefix() {
        assert_ne!(
            id_for(&project(), "feature/login"),
            id_for(&project(), "feature")
        );
    }

    #[test]
    fn the_same_branch_in_two_projects_is_distinct() {
        let a = id_for(&ProjectId::new("api-aaaa"), "main");
        let b = id_for(&ProjectId::new("web-bbbb"), "main");
        assert_ne!(a, b);
    }

    #[test]
    fn case_only_branch_differences_survive_case_insensitive_filesystems() {
        assert_ne!(id_for(&project(), "Feature"), id_for(&project(), "feature"));
    }

    #[test]
    fn paths_are_two_levels_under_the_root() {
        let root = Path::new("/data/worktrees");
        let id = id_for(&project(), "feature/login");
        let path = path_for(root, &project(), &id);

        assert_eq!(path.parent().unwrap(), root.join(project().as_str()));
        assert_eq!(
            path.strip_prefix(root).unwrap().components().count(),
            2,
            "expected <project>/<worktree>, got {path:?}"
        );
    }

    #[test]
    fn traversal_in_a_branch_name_cannot_escape_the_root() {
        let root = Path::new("/data/worktrees");
        let id = id_for(&project(), "../../../../etc");
        let path = path_for(root, &project(), &id);

        assert!(path.starts_with(root));
        assert!(!path.to_string_lossy().contains(".."));
    }

    // ---- cache_ttl_for ------------------------------------------------------

    #[test]
    fn cache_ttl_is_only_ever_for_claude() {
        assert_eq!(
            cache_ttl_for("codex", Some(crate::usage::Billing::Metered), None),
            None
        );
    }

    #[test]
    fn an_explicit_off_wins_over_metered_billing() {
        assert_eq!(
            cache_ttl_for("claude", Some(crate::usage::Billing::Metered), Some("off")),
            None
        );
    }

    #[test]
    fn an_explicit_recognised_ttl_is_taken_as_given() {
        assert_eq!(
            cache_ttl_for("claude", None, Some("5m")),
            Some("5m".to_owned())
        );
        assert_eq!(
            cache_ttl_for("Claude", None, Some("1h")),
            Some("1h".to_owned())
        );
    }

    #[test]
    fn metered_billing_with_no_configured_value_defaults_to_an_hour() {
        assert_eq!(
            cache_ttl_for("claude", Some(crate::usage::Billing::Metered), None),
            Some("1h".to_owned())
        );
    }

    #[test]
    fn an_unrecognised_configured_value_falls_back_to_billing() {
        assert_eq!(cache_ttl_for("claude", None, Some("nonsense")), None);
    }

    #[test]
    fn plan_billing_with_nothing_configured_asks_for_nothing() {
        assert_eq!(
            cache_ttl_for("claude", Some(crate::usage::Billing::Plan), None),
            None
        );
    }

    // ---- launch_settings ------------------------------------------------------

    #[test]
    fn launch_settings_is_none_when_nothing_is_asked_for() {
        assert_eq!(launch_settings(None, None, None), None);
    }

    #[test]
    fn launch_settings_composes_one_flag_from_every_key_given() {
        let words = launch_settings(Some("1h"), Some(500), Some("hold")).unwrap();
        assert_eq!(words[0], "--settings");
        assert!(words[1].contains(r#""promptCacheTtl":"1h""#));
        assert!(words[1].contains(r#""bashOutputMaxChars":500"#));
        assert!(words[1].contains(r#""crossSessionInbound":"hold""#));
    }

    #[test]
    fn launch_settings_only_accepts_hold_or_refuse_for_cross_session() {
        let words = launch_settings(None, None, Some("accept")).unwrap_or_default();
        assert!(
            words.is_empty() || !words[1].contains("crossSessionInbound"),
            "accept is the default and never needs to be said"
        );

        assert!(launch_settings(None, None, Some("whatever is unrecognised")).is_none());
    }

    #[test]
    fn launch_settings_trims_the_cross_session_policy() {
        let words = launch_settings(None, None, Some("  refuse  ")).unwrap();
        assert!(words[1].contains(r#""crossSessionInbound":"refuse""#));
    }

    // ---- autocompact -----------------------------------------------------------

    #[test]
    fn claude_autocompact_passes_the_window_through_verbatim() {
        assert_eq!(
            autocompact_args("auto"),
            vec!["--autocompact".to_owned(), "auto".to_owned()]
        );
    }

    #[test]
    fn codex_autocompact_only_accepts_a_plain_token_count() {
        assert_eq!(
            codex_autocompact_args("50000"),
            Some(vec![
                "-c".to_owned(),
                "model_auto_compact_token_limit=50000".to_owned()
            ])
        );
        assert_eq!(codex_autocompact_args("auto"), None);
        assert_eq!(codex_autocompact_args("500k"), None);
        assert_eq!(
            codex_autocompact_args("  50000  "),
            codex_autocompact_args("50000")
        );
    }

    #[test]
    fn codex_output_limit_names_the_config_key() {
        assert_eq!(
            codex_output_limit_args(2000),
            vec!["-c".to_owned(), "tool_output_token_limit=2000".to_owned()]
        );
    }

    // ---- ContextWeight / context_weight -----------------------------------------

    #[test]
    fn within_budget_has_nothing_to_say() {
        let weight = ContextWeight {
            claude_md_lines: Some(CLAUDE_MD_LINE_BUDGET),
            agents_md_bytes: Some(100),
            agents_md_budget: Some(200),
        };
        assert_eq!(weight.warning(), None);
    }

    #[test]
    fn a_claude_md_over_budget_warns_by_line_count() {
        let weight = ContextWeight {
            claude_md_lines: Some(CLAUDE_MD_LINE_BUDGET + 1),
            agents_md_bytes: None,
            agents_md_budget: None,
        };
        let warning = weight.warning().unwrap();
        assert!(warning.contains(&(CLAUDE_MD_LINE_BUDGET + 1).to_string()));
    }

    #[test]
    fn an_agents_md_over_codexs_own_budget_warns_by_byte_count() {
        let weight = ContextWeight {
            claude_md_lines: None,
            agents_md_bytes: Some(500),
            agents_md_budget: Some(200),
        };
        let warning = weight.warning().unwrap();
        assert!(warning.contains("500"));
        assert!(warning.contains("200"));
    }

    #[test]
    fn an_unknown_codex_budget_never_warns_about_agents_md() {
        let weight = ContextWeight {
            claude_md_lines: None,
            agents_md_bytes: Some(1_000_000),
            agents_md_budget: None,
        };
        assert_eq!(weight.warning(), None);
    }

    #[test]
    fn context_weight_reads_the_files_that_exist_and_ignores_those_that_do_not() {
        let dir = std::env::temp_dir().join(format!(
            "ket-worktree-context-weight-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "line one\nline two\nline three\n").unwrap();

        let weight = context_weight(&dir);
        assert_eq!(weight.claude_md_lines, Some(3));
        assert_eq!(weight.agents_md_bytes, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- activate_pack (rejection paths only — success poisons the process-
    // global OnceLock for every other test in this binary) ----------------------

    #[test]
    fn activate_pack_refuses_an_empty_level_list() {
        assert!(!activate_pack("pack".to_owned(), 1, 4, Vec::new()));
    }

    #[test]
    fn activate_pack_refuses_a_default_level_not_present_in_the_levels() {
        let levels = vec![TokenReduction {
            level: 4,
            id: "x".to_owned(),
            label: "x".to_owned(),
            description: String::new(),
            tag: None,
            instruction: None,
            effort: None,
            model: None,
            cache_ttl: None,
            autocompact: None,
            subagent_model: None,
            subagent_cache_ttl: None,
            agent_teams: None,
            cross_session_inbound: None,
            verbosity: None,
            reasoning_summary: None,
            claude: EconomyPolicy::default(),
            codex: EconomyPolicy::default(),
            opencode: EconomyPolicy::default(),
            grok: EconomyPolicy::default(),
        }];
        assert!(!activate_pack("pack".to_owned(), 1, 99, levels));
    }

    // ---- token_reduction_levels / resolve_level / level_position / token_reduction

    #[test]
    fn the_builtin_levels_run_from_four_down_to_zero() {
        let levels = token_reduction_levels();
        let numbers: Vec<u8> = levels.iter().map(|l| l.level).collect();
        assert_eq!(numbers, vec![4, 3, 2, 1, 0]);
    }

    #[test]
    fn resolve_level_with_no_id_or_pack_falls_back_to_the_stored_number() {
        assert_eq!(resolve_level(2, None, None), 2);
    }

    #[test]
    fn resolve_level_with_an_unknown_stored_number_falls_back_to_the_default() {
        assert_eq!(resolve_level(200, None, None), default_token_reduction());
    }

    #[test]
    fn resolve_level_with_a_pack_id_that_is_not_the_active_one_uses_the_default() {
        assert_eq!(
            resolve_level(1, Some("heavy-reduction"), Some("some-other-pack")),
            default_token_reduction()
        );
    }

    #[test]
    fn resolve_level_with_the_active_pack_and_an_unknown_id_uses_the_default() {
        assert_eq!(
            resolve_level(1, Some("not-a-real-id"), Some(&active_pack_id())),
            default_token_reduction()
        );
    }

    #[test]
    fn resolve_level_with_the_active_pack_and_a_known_id_uses_its_level() {
        assert_eq!(
            resolve_level(1, Some("heavy-reduction"), Some(&active_pack_id())),
            1
        );
    }

    #[test]
    fn level_position_reports_where_a_level_sits_and_the_table_size() {
        assert_eq!(level_position(4), Some((0, 5)));
        assert_eq!(level_position(0), Some((4, 5)));
        assert_eq!(level_position(200), None);
    }

    #[test]
    fn token_reduction_falls_back_to_the_default_for_an_unknown_value() {
        let looked_up = token_reduction(200);
        assert_eq!(looked_up.level, default_token_reduction());
    }

    #[test]
    fn token_reduction_finds_the_exact_level() {
        assert_eq!(token_reduction(1).id, "heavy-reduction");
    }

    #[test]
    fn active_pack_version_is_zero_for_the_builtin_table() {
        // Only true so long as no earlier test in this binary has activated a
        // pack; activate_pack's success path is deliberately never exercised
        // here for exactly that reason.
        assert_eq!(active_pack_version(), 0);
    }

    // ---- TokenReduction::policy_for / name --------------------------------------

    fn level_with_everything() -> TokenReduction {
        TokenReduction {
            level: 2,
            id: "moderate-reduction".to_owned(),
            label: "2 — Moderate reduction".to_owned(),
            description: String::new(),
            tag: None,
            instruction: None,
            effort: Some("base-effort".to_owned()),
            model: Some("base-model".to_owned()),
            cache_ttl: Some("5m".to_owned()),
            autocompact: Some("auto".to_owned()),
            subagent_model: Some("base-subagent".to_owned()),
            subagent_cache_ttl: Some("5m".to_owned()),
            agent_teams: Some(false),
            cross_session_inbound: Some("hold".to_owned()),
            verbosity: None,
            reasoning_summary: None,
            claude: EconomyPolicy {
                model: Some("claude-model".to_owned()),
                ..EconomyPolicy::default()
            },
            codex: EconomyPolicy {
                effort: Some("codex-effort".to_owned()),
                verbosity: Some("low".to_owned()),
                ..EconomyPolicy::default()
            },
            opencode: EconomyPolicy {
                model: Some("opencode-model".to_owned()),
                ..EconomyPolicy::default()
            },
            grok: EconomyPolicy {
                effort: Some("grok-effort".to_owned()),
                ..EconomyPolicy::default()
            },
        }
    }

    #[test]
    fn claude_policy_layers_its_provider_block_over_the_schema_v1_base() {
        let policy = level_with_everything().policy_for("claude");
        // Overridden by the provider block.
        assert_eq!(policy.model.as_deref(), Some("claude-model"));
        // Left at the schema-v1 base, since claude's block says nothing about it.
        assert_eq!(policy.effort.as_deref(), Some("base-effort"));
        assert_eq!(policy.cache_ttl.as_deref(), Some("5m"));
    }

    #[test]
    fn codex_policy_never_inherits_cache_or_subagent_fields() {
        let policy = level_with_everything().policy_for("codex");
        assert_eq!(policy.effort.as_deref(), Some("codex-effort"));
        assert_eq!(policy.verbosity.as_deref(), Some("low"));
        assert_eq!(policy.cache_ttl, None, "codex has no cache setting at all");
        assert_eq!(policy.subagent_model, None);
    }

    #[test]
    fn opencode_policy_starts_empty_and_takes_only_its_own_block() {
        let policy = level_with_everything().policy_for("opencode");
        assert_eq!(policy.model.as_deref(), Some("opencode-model"));
        assert_eq!(
            policy.effort, None,
            "opencode has no schema-v1 base to inherit from"
        );
    }

    #[test]
    fn grok_policy_never_inherits_the_schema_v1_base_either() {
        let policy = level_with_everything().policy_for("grok");
        assert_eq!(policy.effort.as_deref(), Some("grok-effort"));
        assert_eq!(policy.model, None, "grok's block set no model");
    }

    #[test]
    fn an_agent_with_no_provider_block_gets_the_empty_default() {
        assert_eq!(
            level_with_everything().policy_for("some-future-agent"),
            EconomyPolicy::default()
        );
    }

    #[test]
    fn builtin_level_names_use_the_short_form() {
        assert_eq!(token_reduction(4).name(), "Off");
        assert_eq!(token_reduction(3).name(), "Light");
        assert_eq!(token_reduction(2).name(), "Moderate");
        assert_eq!(token_reduction(1).name(), "Heavy");
        assert_eq!(token_reduction(0).name(), "Max");
    }

    #[test]
    fn an_unrecognised_level_id_falls_back_to_the_label_after_the_dash() {
        let mut level = level_with_everything();
        level.id = "some-pack-level".to_owned();
        level.label = "3 — Custom Name".to_owned();
        assert_eq!(level.name(), "Custom Name");
    }

    #[test]
    fn a_label_with_no_dash_is_used_whole() {
        let mut level = level_with_everything();
        level.id = "some-pack-level".to_owned();
        level.label = "Just A Name".to_owned();
        assert_eq!(level.name(), "Just A Name");
    }

    // ---- economy_launch / apply_economy -----------------------------------------

    #[test]
    fn economy_launch_always_stamps_the_reduction_env_vars() {
        let level = token_reduction(2);
        let launch = economy_launch("claude", level, None, None, None);
        assert_eq!(launch.env.get(TOKEN_REDUCTION_ENV), Some(&"2".to_owned()));
        assert!(launch.env.contains_key(TOKEN_REDUCTION_NOTE_ENV));
    }

    #[test]
    fn economy_launch_for_claude_includes_settings_and_effort() {
        let level = level_with_everything();
        let launch = economy_launch("claude", &level, None, None, None);
        assert!(launch.args.contains(&"--effort".to_owned()));
        assert!(launch.args.contains(&"--settings".to_owned()));
        assert!(launch.args.contains(&"--autocompact".to_owned()));
        assert_eq!(
            launch.env.get("CLAUDE_CODE_SUBAGENT_MODEL"),
            Some(&"base-subagent".to_owned())
        );
        assert!(
            !launch
                .env
                .contains_key("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS")
        );
    }

    #[test]
    fn economy_launch_caller_supplied_cache_ttl_wins_over_the_packs() {
        let level = level_with_everything();
        let launch = economy_launch("claude", &level, Some("1h"), None, None);
        let settings = launch
            .args
            .iter()
            .find(|a| a.contains("promptCacheTtl"))
            .unwrap();
        assert!(settings.contains("1h"));
        assert!(!settings.contains("5m"));
    }

    #[test]
    fn economy_launch_for_codex_uses_config_flags_not_settings_json() {
        let level = level_with_everything();
        let launch = economy_launch("codex", &level, None, Some(1000), None);
        assert!(!launch.args.contains(&"--settings".to_owned()));
        assert!(
            launch
                .args
                .contains(&"model_reasoning_effort=codex-effort".to_owned())
        );
        assert!(launch.args.contains(&"model_verbosity=low".to_owned()));
        assert!(
            launch
                .args
                .contains(&"tool_output_token_limit=1000".to_owned())
        );
    }

    #[test]
    fn economy_launch_for_grok_only_sets_reasoning_effort() {
        let level = level_with_everything();
        let launch = economy_launch("grok", &level, None, None, None);
        assert_eq!(
            launch.args,
            vec!["--reasoning-effort".to_owned(), "grok-effort".to_owned()]
        );
    }

    #[test]
    fn economy_launch_for_an_unrecognised_agent_only_stamps_env() {
        let level = level_with_everything();
        let launch = economy_launch("some-future-agent", &level, None, None, None);
        assert!(launch.args.is_empty());
    }

    fn agent_spec(name: &str, args: Vec<String>) -> crate::config::AgentSpec {
        crate::config::AgentSpec {
            name: name.to_owned(),
            transport: crate::config::Transport::Pty,
            command: name.to_owned(),
            args,
            env: std::collections::BTreeMap::new(),
            env_remove: Vec::new(),
            launch: None,
        }
    }

    #[test]
    fn apply_economy_extends_the_specs_args_and_env() {
        let mut spec = agent_spec("claude", Vec::new());
        let worktree = Worktree {
            token_reduction: 2,
            token_reduction_id: Some("moderate-reduction".to_owned()),
            token_reduction_pack_id: None,
            ..default_worktree()
        };
        apply_economy(&mut spec, &worktree);
        assert!(spec.env.contains_key(TOKEN_REDUCTION_ENV));
    }

    #[test]
    fn apply_economy_keeps_stdio_last_in_groks_argument_list() {
        // The builtin levels carry no per-provider policy (see
        // `level_with_everything` above, which is what a pack fills in), so
        // this exercises the splice position rather than any inserted words.
        let mut spec = agent_spec("grok", vec!["agent".to_owned(), "stdio".to_owned()]);
        let worktree = Worktree {
            token_reduction: 1,
            token_reduction_id: Some("heavy-reduction".to_owned()),
            token_reduction_pack_id: None,
            ..default_worktree()
        };
        apply_economy(&mut spec, &worktree);
        assert_eq!(spec.args.last().map(String::as_str), Some("stdio"));
        assert!(spec.env.contains_key(TOKEN_REDUCTION_ENV));
    }

    fn default_worktree() -> Worktree {
        Worktree {
            id: WorktreeId::new("w1"),
            project_id: ProjectId::new("p1"),
            branch: "work".to_owned(),
            name: None,
            agent_title: None,
            path: PathBuf::from("/tmp/w1"),
            base: "main".to_owned(),
            base_commit: None,
            agent: None,
            created_at_ms: 0,
            provisioned_at_ms: None,
            provisioned_paths: Vec::new(),
            token_reduction: default_token_reduction(),
            token_reduction_id: None,
            token_reduction_pack_id: None,
            pinned: false,
        }
    }

    // ---- Worktree::exists / is_provisioned / base_rev ---------------------------

    #[test]
    fn base_rev_prefers_the_named_base_over_the_frozen_commit() {
        let mut w = default_worktree();
        w.base = "main".to_owned();
        w.base_commit = Some("abc123".to_owned());
        assert_eq!(w.base_rev(), Some("main"));
    }

    #[test]
    fn base_rev_falls_back_to_the_frozen_commit_for_head_or_empty() {
        let mut w = default_worktree();
        w.base = "HEAD".to_owned();
        w.base_commit = Some("abc123".to_owned());
        assert_eq!(w.base_rev(), Some("abc123"));

        w.base = String::new();
        assert_eq!(w.base_rev(), Some("abc123"));
    }

    #[test]
    fn is_provisioned_reflects_the_timestamp() {
        let mut w = default_worktree();
        assert!(!w.is_provisioned());
        w.provisioned_at_ms = Some(1);
        assert!(w.is_provisioned());
    }

    #[test]
    fn exists_checks_the_real_filesystem() {
        let w = default_worktree();
        assert!(!w.exists(), "a made-up path should not exist");
    }
}
