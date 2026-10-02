//! User configuration, read from `~/.config/ket/config.toml`.
//!
//! ket ships working defaults for all four agents in rotation — Claude Code,
//! Codex, OpenCode and Grok — because a tool that requires a config file before it
//! will start is a bad first run. The file only needs to exist to *override*
//! something.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{KetError, Result, paths};

/// How ket talks to an agent process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Agent Client Protocol — JSON-RPC 2.0 over the process's stdio.
    ///
    /// The good path: tool calls and diffs arrive as structured data rather than
    /// as ANSI escapes to be scraped.
    Acp,
    /// A pseudo-terminal. The fallback for agents that do not speak ACP.
    Pty,
}

/// A single agent ket can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    /// Name used to select this agent, e.g. `ket run claude`.
    pub name: String,
    /// How ket communicates with it.
    pub transport: Transport,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the agent process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Variables to strip from the inherited environment before spawning.
    ///
    /// An agent inherits ket's environment, and ket is regularly *launched from
    /// inside an agent session* — that is the tool's whole purpose. Agents guard
    /// against being nested inside themselves by checking a marker variable, so
    /// without stripping it, running ket from inside Claude Code makes every
    /// `claude` session fail with "Claude Code cannot be launched inside another
    /// Claude Code session". Verified: that is exactly what happens.
    ///
    /// A neutral mechanism with per-agent data, rather than a special case for
    /// one agent — every agent in the rotation has the same hazard.
    #[serde(default)]
    pub env_remove: Vec<String>,
    /// Optional override for the interactive command ket opens in a terminal.
    ///
    /// ACP agents use `command` and `args` for their protocol adapter, which
    /// is not necessarily the CLI a person wants to interact with. Keeping
    /// this separate lets a profile such as `claude-personal` launch in the
    /// terminal without replacing the Claude ACP adapter.
    ///
    /// The command is *shell text*, not a path: it is typed at a real prompt
    /// (see [`crate::shell`]), so an alias or a function is as valid here as
    /// an executable, and it may carry its own arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<AgentLaunch>,
}

/// An agent's interactive terminal command and arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLaunch {
    /// Executable or wrapper path to launch.
    pub command: String,
    /// Arguments passed to the interactive command.
    #[serde(default)]
    pub args: Vec<String>,
}

impl AgentSpec {
    /// The command ket opens in an interactive terminal.
    pub fn launch_command(&self) -> &str {
        self.launch
            .as_ref()
            .map(|launch| launch.command.as_str())
            .unwrap_or_else(|| match self.transport {
                Transport::Acp => self.name.as_str(),
                Transport::Pty => self.command.as_str(),
            })
    }

    /// The whole command line ket types at the shell's first prompt.
    ///
    /// The command is shell text and goes through untouched: that is what lets
    /// it be an alias, a function, or a shim that only exists once a shell has
    /// read its own configuration. The arguments are separate words nobody
    /// wrote shell syntax in, so they are quoted.
    pub fn launch_line(&self, extra: &[String]) -> String {
        let mut arguments = self.launch_args().to_vec();
        arguments.extend_from_slice(extra);
        crate::shell::line(self.launch_command(), &arguments)
    }

    /// The arguments ket passes to the interactive command.
    pub fn launch_args(&self) -> &[String] {
        self.launch
            .as_ref()
            .map(|launch| launch.args.as_slice())
            .unwrap_or_else(|| match self.transport {
                Transport::Acp => &[],
                Transport::Pty => self.args.as_slice(),
            })
    }
}

/// How long an agent session may sit in one state before it is considered stuck.
///
/// One bound per state rather than one for "busy", because the sensible number
/// differs by more than an order of magnitude between them — see
/// [`crate::agent::timeout_for`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentTimeouts {
    /// Spawning the process and completing the ACP handshake.
    ///
    /// Generous, because an ACP adapter launched through `npx` may be
    /// downloading itself on first run. Measured cold: well over a minute.
    pub starting_secs: u64,
    /// Completing an authentication flow.
    ///
    /// This one is waiting on a person finishing a login, so it is long.
    pub authenticating_secs: u64,
    /// Waiting on the model.
    pub thinking_secs: u64,
    /// Running a tool the model asked for.
    ///
    /// A build or a test suite legitimately takes half an hour. This is the
    /// number that makes splitting `ExecutingTool` from `Thinking` worth having.
    pub executing_tool_secs: u64,
}

impl Default for AgentTimeouts {
    fn default() -> Self {
        Self {
            starting_secs: 120,
            authenticating_secs: 300,
            thinking_secs: 300,
            executing_tool_secs: 1800,
        }
    }
}

/// What to do when an agent asks to use a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionPolicy {
    /// Grant it without asking.
    Allow,
    /// Ask, and block the session until someone answers.
    Prompt,
    /// Refuse it without asking.
    Deny,
}

/// Which tools an agent may use, and which need a decision first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionConfig {
    /// Applied to any tool without an entry in `tools`.
    ///
    /// `Prompt` by default. An agent running unattended in a worktree is still
    /// an agent running commands on your machine, and defaulting to `allow`
    /// would make that decision silently on the user's behalf.
    pub default: PermissionPolicy,
    /// Per-tool overrides, keyed by the tool name the agent reports.
    pub tools: BTreeMap<String, PermissionPolicy>,
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            default: PermissionPolicy::Prompt,
            tools: BTreeMap::new(),
        }
    }
}

impl PermissionConfig {
    /// The named posture this policy currently matches, if any.
    ///
    /// Reported rather than stored. Storing a preset alongside the policy would
    /// let the two disagree — a config hand-edited to allow one tool would still
    /// claim to be `Manual` — and a settings screen that lies about which preset
    /// is active is worse than one that admits the policy is custom.
    pub fn preset(&self) -> crate::agents::PermissionPreset {
        use crate::agents::PermissionPreset;

        if !self.tools.is_empty() {
            return PermissionPreset::Custom;
        }

        match self.default {
            PermissionPolicy::Allow => PermissionPreset::Yolo,
            PermissionPolicy::Prompt => PermissionPreset::Manual,
            PermissionPolicy::Deny => PermissionPreset::Custom,
        }
    }

    /// Applies a named posture, leaving per-tool entries alone.
    ///
    /// Per-tool decisions are the more specific statement and keep winning: someone
    /// who denied one tool by name meant it, and a preset is a statement about
    /// everything they did *not* name. Selecting [`PermissionPreset::Custom`] is
    /// meaningless and does nothing.
    pub fn apply_preset(&mut self, preset: crate::agents::PermissionPreset) {
        use crate::agents::PermissionPreset;

        self.default = match preset {
            PermissionPreset::Yolo => PermissionPolicy::Allow,
            PermissionPreset::Manual => PermissionPolicy::Prompt,
            PermissionPreset::Custom => return,
        };
    }

    /// The policy for one tool.
    pub fn for_tool(&self, tool: &str) -> PermissionPolicy {
        self.tools.get(tool).copied().unwrap_or(self.default)
    }
}

/// Settings governing agent sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Per-state timeouts.
    pub timeouts: AgentTimeouts,
    /// Tool permission policy.
    pub permissions: PermissionConfig,
    /// Agents the user has switched off.
    ///
    /// Names rather than a flag on [`AgentSpec`], so switching an agent off does
    /// not require the user to have a spec for it — the shipped agents have none
    /// until someone overrides them.
    #[serde(default)]
    pub disabled: std::collections::BTreeSet<String>,
    /// Which agent a bare `ket run` picks — see [`crate::agents::Catalogue::resolve`].
    #[serde(default)]
    pub default: crate::agents::DefaultAgent,
    /// What to ask for as the prompt cache's lifetime, overriding what ket
    /// would choose on its own.
    ///
    /// Unset — the default — means ket decides, which today is "ask for an hour
    /// on a metered account and say nothing otherwise"; see
    /// [`crate::worktree::cache_ttl_for`]. `"off"` stops ket asking at all,
    /// which is the setting for a workload that never idles past five minutes
    /// and would rather not pay the longer lifetime's higher write rate.
    /// `"5m"` or `"1h"` force one, for a reader who knows their own pattern
    /// better than an inference does.
    #[serde(default)]
    pub prompt_cache_ttl: Option<String>,
    /// How much *successful* command output an agent takes inline, overriding
    /// its own default.
    ///
    /// Claude Code's `bashOutputMaxChars` (characters, default 30000) and
    /// Codex's `tool_output_token_limit` (tokens) are the same idea, and both
    /// are the safe way to spend less on a noisy build: they cap what reaches
    /// the model without touching the command, the exit status, or what a
    /// *failed* run reports. Unset means each agent's own default, untouched.
    ///
    /// Lower is not automatically cheaper. Truncating past what a task needed
    /// buys a re-run, and a re-run costs more than the lines it saved — which
    /// is why ket has no opinion here and only carries the reader's.
    #[serde(default)]
    pub output_limit: Option<u32>,
    /// When a session should compact its own history, overriding the agent's
    /// own threshold.
    ///
    /// Claude Code takes `auto` or a token count like `500k`; Codex takes a
    /// number of tokens. Unset means each agent's own default, untouched.
    ///
    /// A tradeoff in both directions, which is why ket carries the value rather
    /// than choosing it. Compacting earlier keeps every later turn smaller and
    /// cheaper, but each compaction is itself a request that reads the whole
    /// conversation, and what it summarises away is gone. Compacting later
    /// pays a bigger prefix on every turn instead. Whether the first is worth
    /// it depends on how long the reader's tasks run, which ket cannot know.
    #[serde(default)]
    pub autocompact: Option<String>,
}

/// Settings for the web shell's local server (Epic 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Address to bind.
    ///
    /// Loopback by default, deliberately: this server spawns processes and writes
    /// files. Binding it wider must be a conscious act, and must come with token
    /// auth.
    pub bind: IpAddr,
    /// Port to listen on.
    pub port: u16,
}

/// How ket hands a file off to an external editor, and how its own editor
/// draws text.
///
/// `PartialEq` without `Eq`, because a line height is a ratio and a ratio is
/// fractional. Nothing compares configs for anything but equality of content —
/// the same reason [`Config`] itself gave up `Eq` for [`crate::rates`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// Command and arguments. `{path}`, `{line}` and `{col}` are substituted.
    ///
    /// The fallback that must stay reachable whatever editor ket grows.
    pub open_command: Vec<String>,
    /// Point size for text in ket's own editor, overriding
    /// [`ThemeConfig::code_font_size`].
    ///
    /// `None` — the default — means the editor draws at the shared code size,
    /// which is what it has always done and what keeps a file and a terminal
    /// looking like the same machine. Setting it is the deliberate act of
    /// saying they are *not* the same: prose-heavy files read larger, a
    /// terminal grid does not, and somebody who wants that should not have to
    /// resize their shell to get it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u16>,
    /// How tall an editor line is, as a multiple of the font size.
    ///
    /// A ratio rather than pixels, so it survives a change of font size — the
    /// number people mean by "line height" in every editor that offers one.
    #[serde(deserialize_with = "line_height")]
    pub line_height: f32,
}

/// Settings for non-text file viewers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewerConfig {
    /// Whether recognized audio files may be played inside ket.
    pub audio: bool,
}

/// The commit message the merge button writes when nothing else supplies one.
///
/// Deliberately not a summary of the diff. Summarising a diff is either a model
/// call or a lie, and the whole promise of the default path is that it costs
/// nothing and happens now — see [`MessageSource`].
pub const DEFAULT_COMMIT_TEMPLATE: &str = "{branch}: uncommitted work ({files} files)";

/// The placeholders [`MergeConfig::commit_template`] may use.
///
/// Named here rather than inline in the renderer because validation and
/// rendering have to agree about the set, and a settings pane that accepts
/// `{summry}` and then silently writes it into git history is worse than one
/// that refuses at the point of typing.
pub const COMMIT_PLACEHOLDERS: [&str; 4] = ["branch", "base", "worktree", "files"];

/// Who writes the commit message the merge button uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageSource {
    /// Fill [`MergeConfig::commit_template`] in locally. No tokens, no waiting,
    /// and it works with no agent configured, none running, and no network.
    #[default]
    Template,
    /// Ask an agent to read the diff and write one line.
    ///
    /// Costs tokens and a few seconds. The agent returns *only a string* — ket
    /// still runs the staging, the commit and the merge itself — and any
    /// failure falls back to the template rather than abandoning the merge.
    Agent,
}

/// What the sidebar's merge button does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    /// Template for the commit made from uncommitted work before merging.
    ///
    /// Substitutes the names in [`COMMIT_PLACEHOLDERS`], written in braces.
    pub commit_template: String,
    /// Who writes the message.
    pub message_source: MessageSource,
    /// Whether the checkout survives the merge.
    ///
    /// True by default: a quick action that silently removes the thing you were
    /// looking at is not quick, it is a surprise.
    pub keep_after_merge: bool,
    /// Whether the button asks before it acts.
    ///
    /// True by default, and worth leaving that way. The commit stages
    /// everything git does not ignore, so the confirmation is the only place a
    /// person sees that a provisioned `.env` is about to be committed and then
    /// merged into the base branch.
    pub confirm: bool,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            commit_template: DEFAULT_COMMIT_TEMPLATE.to_owned(),
            message_source: MessageSource::Template,
            keep_after_merge: true,
            confirm: true,
        }
    }
}

/// What ket itself does around worktrees: whether arriving at one opens a
/// terminal, whether deleting one asks first, and where new ones are made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    /// Whether selecting a worktree that has no terminal opens one: its
    /// agent's session, or a shell when it has no agent.
    ///
    /// Creating a worktree opens its session either way — that is asking for
    /// the agent, not arriving somewhere.
    pub open_terminal: bool,
    /// Whether deleting a worktree asks first.
    ///
    /// Off, a clean worktree goes at once. One with work that would be lost
    /// still stops and says so: an unforced delete refuses it, and that
    /// refusal is a question no setting answers in advance.
    pub confirm_delete: bool,
    /// Where new worktrees are made, or `None` for [`paths::worktrees_dir`].
    ///
    /// A leading `~/` is the home directory. Worktrees already made stay
    /// where they are: git records each checkout's path, so moving one is a
    /// `git worktree move`, not a setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_dir: Option<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            open_terminal: true,
            confirm_delete: true,
            worktree_dir: None,
        }
    }
}

/// How often a release build looks for a newer ket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateCheck {
    /// On every launch.
    OnLaunch,
    /// At most once a day.
    Daily,
    /// At most once a week.
    #[default]
    Weekly,
    /// Not unless asked.
    Never,
}

impl UpdateCheck {
    /// Every choice, in the order a picker lists them.
    pub const ALL: [UpdateCheck; 4] = [
        UpdateCheck::OnLaunch,
        UpdateCheck::Daily,
        UpdateCheck::Weekly,
        UpdateCheck::Never,
    ];
}

/// How ket updates itself. Saved ahead of the updater that reads it, and
/// never acted on by a dev build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    /// How often to look.
    pub check: UpdateCheck,
}

impl MergeConfig {
    /// Fills the template's placeholders.
    ///
    /// One left-to-right pass rather than four [`str::replace`] calls: git
    /// permits `{` in a branch name, so substituting `{branch}` first could
    /// plant a `{files}` that the next pass would then substitute in turn —
    /// a branch called `{files}-fix` should name itself, not report a count.
    ///
    /// An unknown placeholder is copied through verbatim rather than dropped.
    /// [`Config::validate`] refuses to save one, so reaching here means a
    /// hand-edited file, and showing `{summry}` in the commit is what makes
    /// that visible.
    pub fn commit_message(&self, branch: &str, base: &str, worktree: &str, files: usize) -> String {
        let files = files.to_string();
        let mut out = String::with_capacity(self.commit_template.len());
        let mut rest = self.commit_template.as_str();

        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            rest = &rest[open..];

            let Some(close) = rest.find('}') else {
                // An unclosed brace is the rest of the template, verbatim.
                break;
            };
            let name = &rest[1..close];
            match name {
                "branch" => out.push_str(branch),
                "base" => out.push_str(base),
                "worktree" => out.push_str(worktree),
                "files" => out.push_str(&files),
                _ => out.push_str(&rest[..=close]),
            }
            rest = &rest[close + 1..];
        }

        out.push_str(rest);
        out
    }

    /// The placeholder names this template uses that nothing can fill.
    ///
    /// Used by [`Config::validate`] so a typo is refused where it was typed.
    fn unknown_placeholders(&self) -> Vec<String> {
        let mut unknown = Vec::new();
        let mut rest = self.commit_template.as_str();

        while let Some(open) = rest.find('{') {
            rest = &rest[open..];
            let Some(close) = rest.find('}') else { break };
            let name = &rest[1..close];
            // Only word-shaped tokens are treated as placeholders. Prose in
            // braces is a person writing prose, not a misspelling.
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphabetic())
                && !COMMIT_PLACEHOLDERS.contains(&name)
            {
                unknown.push(name.to_owned());
            }
            rest = &rest[close + 1..];
        }

        unknown
    }
}

/// How a dependency directory is materialised into a new worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DirStrategy {
    /// Copy-on-write clone. Each worktree gets an independent copy.
    ///
    /// The default, and almost always the right answer: near-instant on APFS,
    /// and writes in one worktree cannot affect another.
    Clone,
    /// Symlink into the primary checkout.
    ///
    /// Free, but **shared**. Two agents running `npm install` against the same
    /// symlinked `node_modules` will corrupt it for both. Only safe for
    /// directories nothing writes to — a read-only cache, say. Chosen per
    /// directory rather than globally, precisely because that distinction
    /// cannot be made once for everything.
    Symlink,
    /// Leave it out of new worktrees entirely.
    Skip,
}

/// One directory to materialise when provisioning a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorySpec {
    /// Path relative to the repository root, e.g. `node_modules`.
    pub path: String,
    /// How to materialise it.
    pub strategy: DirStrategy,
}

/// What a fresh worktree needs before an agent can work in it.
///
/// A bare `git worktree add` produces a checkout, not a working environment:
/// no `node_modules`, no `.env`, no build cache. An agent dropped into one
/// fails on its first command and then burns tokens diagnosing an environment
/// problem that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvisionConfig {
    /// Dependency directories to materialise.
    pub directories: Vec<DirectorySpec>,
    /// Globs, relative to the repository root, of untracked files to copy.
    ///
    /// Declared explicitly and never guessed: these are the files git
    /// deliberately does not track, and copying the wrong one moves a secret
    /// somewhere it was not meant to go.
    pub files: Vec<String>,
    /// Command run inside the new worktree once everything is in place.
    pub post_command: Vec<String>,
    /// How long the post-provision command may run before it is killed.
    pub post_timeout_secs: u64,
}

/// Default [`StorageConfig::build_dirs`].
///
/// Deliberately short, and every entry a directory whose contents a build
/// command puts back. A list that tried to be exhaustive would eventually name
/// something regenerable only in theory, and the one thing this list must not
/// do is be wrong about that.
const DEFAULT_BUILD_DIRS: &[&str] = &["target", "node_modules", "dist", "build", ".next"];

/// Which of a checkout's directories hold output a build can put back.
///
/// The list exists because the alternative is guessing, and this one is read
/// by something that deletes directories. Ket cannot know that *your* `build/`
/// is generated rather than checked in, so it is told — the same reasoning as
/// [`ProvisionConfig::files`], and for a sharper reason: provisioning a wrong
/// file copies something; clearing a wrong directory destroys it.
///
/// Patterns are globs against a path relative to the worktree root, so
/// `crates/*/target` reaches a Rust workspace's per-crate output. A matched
/// directory is never descended into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// Directories, relative to a worktree's root, holding regenerable output.
    pub build_dirs: Vec<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            build_dirs: DEFAULT_BUILD_DIRS
                .iter()
                .map(|dir| (*dir).to_owned())
                .collect(),
        }
    }
}

/// One hook: a command run at a lifecycle point, gating it by its exit code.
///
/// Modeled on [`ProvisionConfig::post_command`], but with its own timeout per
/// hook rather than one shared by a whole config section: a fast
/// permission-check script and a slow "worktree provisioned" notifier have
/// nothing in common timing-wise, and a config author should not have to
/// choose one number for both.
///
/// See [`crate::hook`] for how hooks are run, what their exit code decides,
/// and why the context they receive is never in `command`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HookSpec {
    /// Program and arguments. Never a shell string, and never a place to put a
    /// secret — see [`crate::hook`] on why context arrives on stdin instead.
    pub command: Vec<String>,
    /// How long the hook may run before it is killed and treated as blocking.
    pub timeout_secs: u64,
}

impl Default for HookSpec {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            timeout_secs: 30,
        }
    }
}

/// Hooks to run at each lifecycle point, split by where they were configured.
///
/// Empty by default: running an arbitrary command on every worktree or agent
/// transition is a side effect a person did not necessarily ask for, so a tool
/// that shipped with hooks pre-populated would be running commands nobody
/// wrote. See [`crate::hook`] for the six points, how a project's own
/// `.ket.toml` and the global config combine, and the timeout and failure
/// policy that keeps a hook from wedging a worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HookConfig {
    /// Run when a worktree is created.
    pub worktree_created: Vec<HookSpec>,
    /// Run once a worktree has finished provisioning.
    pub worktree_provisioned: Vec<HookSpec>,
    /// Run when a worktree is removed.
    pub worktree_removed: Vec<HookSpec>,
    /// Run when an agent session starts.
    pub agent_started: Vec<HookSpec>,
    /// Run when an agent session finishes.
    pub agent_finished: Vec<HookSpec>,
    /// Run when an agent asks to use a tool and policy did not settle it.
    pub agent_permission_requested: Vec<HookSpec>,
}

impl HookConfig {
    /// Every configured list, paired with the lifecycle-point name it belongs
    /// to, so validation (and anything else that wants to walk all six) does
    /// not have to name the fields a second time.
    pub fn by_point(&self) -> [(&'static str, &[HookSpec]); 6] {
        [
            ("worktree.created", &self.worktree_created),
            ("worktree.provisioned", &self.worktree_provisioned),
            ("worktree.removed", &self.worktree_removed),
            ("agent.started", &self.agent_started),
            ("agent.finished", &self.agent_finished),
            (
                "agent.permission_requested",
                &self.agent_permission_requested,
            ),
        ]
    }
}

/// Which appearance the shell renders, and any theme overriding the shipped
/// default for it.
///
/// Kept as its own struct, rather than two bare fields on [`Config`], because
/// the two only mean something together: `name` is which file to load,
/// `appearance` is which shipped default to fall back to while loading it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Dark, light, or follow the OS. See [`crate::theme::AppearancePreference`].
    pub appearance: crate::theme::AppearancePreference,
    /// One of `ket_core::theme::Theme::builtins`, or the name of a file
    /// under `~/.config/ket/themes/` for a theme of the user's own. `None`
    /// uses the shipped theme unmodified for the resolved appearance.
    pub name: Option<String>,
    /// Point size for the code face — the terminal grid, the diff, the editor.
    ///
    /// A whole number: `Config` derives `Eq`, which `f32` cannot, and no font
    /// dialog anyone has used offers 12.5.
    ///
    /// Separate from the chrome's size because they answer to different things.
    /// This one is read for hours and is the number a person means by "the font
    /// size"; the chrome follows the platform's own conventions and is tuned
    /// against them.
    pub code_font_size: u16,
    /// Base size for the window's chrome, in pixels.
    ///
    /// Everything in the sidebar, tabs and status bar is expressed relative to
    /// this, so raising it scales the chrome as a whole rather than one label.
    pub ui_font_size: u16,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            appearance: crate::theme::AppearancePreference::default(),
            name: None,
            code_font_size: DEFAULT_CODE_FONT_SIZE,
            ui_font_size: DEFAULT_UI_FONT_SIZE,
        }
    }
}

/// Sizes a font can sensibly be set to.
///
/// Rejected at load rather than clamped: a `0` in the file means the person
/// meant something, and a window that silently ignores it teaches nothing.
/// The bottom is where text stops being legible, the top where a single line
/// no longer fits a pane.
pub const FONT_SIZE_RANGE: std::ops::RangeInclusive<u16> = 6..=72;

/// Default point size for the code face.
pub const DEFAULT_CODE_FONT_SIZE: u16 = 12;

/// Reads a line height written either way round.
///
/// TOML tells `2` and `2.0` apart and serde would refuse the first of them for
/// an `f32`. That refusal is not a warning: [`Config::load`] cannot repair a
/// file it cannot parse, so the whole configuration — agents, credentials, the
/// lot — falls back to defaults because somebody hand-edited one number
/// without a decimal point in it.
fn line_height<'de, D>(deserializer: D) -> std::result::Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Written {
        Fractional(f64),
        Whole(i64),
    }

    Ok(match Written::deserialize(deserializer)? {
        Written::Fractional(value) => value as f32,
        Written::Whole(value) => value as f32,
    })
}

/// Line heights an editor can sensibly be set to.
///
/// Bounded at both ends for the same reason [`FONT_SIZE_RANGE`] is: below
/// about 1.0 the lines overlap, and past 3.0 a screenful holds so few of them
/// that the file stops being readable as a shape.
pub const LINE_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 1.0..=3.0;

/// Default multiple of the font size an editor line occupies.
///
/// Tighter than `gpui`'s default, which leaves code looking double-spaced, and
/// looser than the terminal's 1.3, which is packed as tight as a terminal
/// wants to be. The gutter and the text share it, so the numbers stay on the
/// lines they number, and the current-line wash needs the air to read as a
/// band rather than as two lines that have closed up.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.5;

/// Default base size for the window's chrome.
///
/// Matches what macOS uses for its own chrome, which is what the shell's
/// typography was picked against.
pub const DEFAULT_UI_FONT_SIZE: u16 = 15;

/// Everything ket reads from the user's config file.
///
/// `PartialEq` but not `Eq`: a rate is a price, prices are fractional, and
/// [`crate::rates::ModelRate`] therefore holds `f64`. Nothing compares configs
/// for anything but equality of content, so the bound was never load-bearing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Agents available to run.
    pub agents: Vec<AgentSpec>,
    /// How agent sessions behave once running.
    pub agent: AgentConfig,
    /// Local server settings.
    pub server: ServerConfig,
    /// External editor handoff.
    pub editor: EditorConfig,
    /// Non-text file viewer behaviour.
    pub viewer: ViewerConfig,
    /// What new worktrees need before an agent can use them.
    pub provision: ProvisionConfig,
    /// Commands run at lifecycle points, gating them by exit code.
    pub hooks: HookConfig,
    /// Appearance and theme selection.
    pub theme: ThemeConfig,
    /// Where a level pack is fetched from, if anywhere.
    pub pack: PackConfig,
    /// Where per-request usage telemetry is sent, if anywhere.
    pub telemetry: TelemetryConfig,
    /// What the organisation actually pays per token, if that is known.
    pub rates: crate::rates::RateConfig,
    /// How ket reaches an organisation's own usage and cost reports.
    pub admin: crate::admin::AdminConfig,
    /// Credentials for the OpenCode Go usage dashboard.
    pub opencode_go: OpenCodeGoConfig,
    /// What the sidebar's merge button does.
    pub merge: MergeConfig,
    /// Which directories hold output a build can put back.
    pub storage: StorageConfig,
    /// Whether phones can reach this Mac, and on which port.
    pub phones: PhonesConfig,
    /// TypeSafe's Jev, for the decisions it makes in beta — see [`crate::jev`].
    pub jev: JevConfig,
    /// What selecting and deleting a worktree do, and where new ones go.
    pub general: GeneralConfig,
    /// How ket updates itself.
    pub updates: UpdatesConfig,
}

/// Jev, TypeSafe's decision model, in beta: a new task's Economy level
/// chosen from its prompt, and a risk read on a permission prompt for the
/// phone to show. Nothing is sent anywhere until a key is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JevConfig {
    /// A TypeSafe API key, as pasted from its console. Empty turns every Jev
    /// feature off.
    pub api_key: String,
    /// Which model answers: TypeSafe's alias for its newest.
    pub model: String,
    /// Whether a new worktree's Economy level may be chosen from its prompt.
    pub auto_economy: bool,
    /// Whether a permission prompt is scored for risk, for the phone.
    pub approval_risk: bool,
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "jev-latest".to_owned(),
            auto_economy: true,
            approval_risk: true,
        }
    }
}

impl JevConfig {
    /// Whether Jev is set up at all.
    pub fn enabled(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// Phones, on the local network.
///
/// Off until turned on in Settings → Devices. On, `ket-host` runs the relay
/// itself on [`Self::port`] and dials it, so a phone on the same Wi-Fi can
/// reach the Mac with nothing else started. The pairing keys are the whole of
/// the authentication; there is no account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhonesConfig {
    /// Whether the host serves phones.
    pub enabled: bool,
    /// The port the relay listens on, on every interface.
    pub port: u16,
}

/// The relay's port unless the config names another.
pub const DEFAULT_PHONES_PORT: u16 = 7979;

impl Default for PhonesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PHONES_PORT,
        }
    }
}

/// Where ket's token-reduction levels come from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PackConfig {
    /// An `https://` URL naming one pack document.
    ///
    /// Unset — the default, and what every install starts as — means ket's own
    /// built-in levels and no network access at all.
    ///
    /// **Pin it in the URL.** A raw-content URL carries the commit it was
    /// fetched at (`.../<sha>/packs/default.json`), which makes the address and
    /// the version one fact instead of two that can disagree. That is also why
    /// there is no separate `ref` field here: a second place to write the
    /// version is a second place for it to be wrong.
    ///
    /// This is set once, by whoever installs ket, and points at a repository
    /// they control. It is not a field for the person whose agents run against
    /// the pack to fill in — the instruction inside a pack reaches every
    /// session that worktree opens.
    pub url: Option<String>,
}

/// The OTLP protocol used when a pack does not say otherwise.
///
/// `http/protobuf` rather than `grpc` because it is the one a collector behind
/// an ordinary HTTPS load balancer accepts without special configuration, and a
/// team's collector is far more often behind one of those than reachable on a
/// bare gRPC port.
pub const DEFAULT_OTLP_PROTOCOL: &str = "http/protobuf";

/// The OTLP protocols Claude Code accepts.
const OTLP_PROTOCOLS: [&str; 3] = ["grpc", "http/json", "http/protobuf"];

/// Where per-request usage telemetry goes.
///
/// Claude Code exports `claude_code.token.usage` and `claude_code.cost.usage`
/// itself, split by model, by `type` (`input`, `output`, `cacheRead`,
/// `cacheCreation`) and by `query_source` (`main`, `subagent`, `auxiliary`) —
/// which is per-request attribution ket would otherwise have to reconstruct
/// from a transcript. Turning it on is four environment variables on the launch
/// line and no parsing at all, because the collector on the other end does the
/// work.
///
/// Unset — the default, and what every install starts as — sets no variables,
/// opens no connection and exports nothing.
///
/// ## What this deliberately cannot turn on
///
/// Claude Code also has `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_ASSISTANT_RESPONSES`,
/// `OTEL_LOG_TOOL_CONTENT` and `OTEL_LOG_RAW_API_BODIES`, which put prompt text,
/// replies, tool input and whole API bodies into the exported stream. There is
/// no field here for any of them, and ket never sets them.
///
/// The reason is not that they are useless — they would make attribution
/// exact — but that this is a *cost* tool: nobody agreeing to have their token
/// counts measured has agreed to have their prompts shipped to a collector, and
/// a checkbox in a config file is not that consent. Anyone who genuinely wants
/// it can set the variable in their own environment, where the decision is
/// visibly theirs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    /// The collector's OTLP endpoint — `http://` or `https://`.
    ///
    /// Plaintext is allowed here, unlike [`PackConfig::url`], and the asymmetry
    /// is deliberate: a pack is an instruction ket hands its own agents and has
    /// to be authenticated, while a collector is very often reached on
    /// `localhost:4318` or across a private network where TLS terminates
    /// somewhere else. Refusing `http` would refuse the ordinary case.
    ///
    /// Unset turns the whole feature off.
    pub endpoint: Option<String>,
    /// One of `grpc`, `http/json` or `http/protobuf`.
    ///
    /// See [`DEFAULT_OTLP_PROTOCOL`]. An unrecognised value is refused at load
    /// rather than passed through, because Claude Code's own failure for one is
    /// to export nothing — silently, into a collector nobody is watching yet.
    pub protocol: String,
    /// Headers for the collector, in OTLP's own `key=value,key=value` form.
    ///
    /// Usually an `Authorization` bearer. Kept as one opaque string rather than
    /// a map because that is the shape the variable takes and re-encoding a map
    /// into it would only add a way to get the escaping wrong.
    pub headers: Option<String>,
    /// Whether to export events as well as metrics.
    ///
    /// Off by default. The events carry per-request cache-read and
    /// cache-creation counts, which is more detail than the metrics — but they
    /// are also the stream the content variables above would pour text into, so
    /// the safe default is the one that cannot carry any.
    pub logs: bool,
    /// How often metrics are flushed, in milliseconds.
    ///
    /// `None` leaves Claude Code's own default of 60,000. Worth lowering while
    /// measuring: a session shorter than one interval can end having
    /// exported nothing at all, and short sessions are common.
    pub export_interval_ms: Option<u64>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            protocol: DEFAULT_OTLP_PROTOCOL.to_owned(),
            headers: None,
            logs: false,
            export_interval_ms: None,
        }
    }
}

impl TelemetryConfig {
    /// The environment an agent is launched with, or nothing when unconfigured.
    ///
    /// Empty rather than an error for every unusable configuration, and the
    /// reasons are logged at the point they are found: telemetry is
    /// instrumentation, and a mistyped endpoint must never be the thing that
    /// stops a worktree opening. Same bargain [`crate::pack`] makes.
    pub fn env(&self) -> std::collections::BTreeMap<String, String> {
        let mut env = std::collections::BTreeMap::new();

        let Some(endpoint) = self
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        else {
            return env;
        };

        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            tracing::warn!(
                endpoint,
                "telemetry endpoint is not an http(s) URL; not exporting"
            );
            return env;
        }

        if !OTLP_PROTOCOLS.contains(&self.protocol.as_str()) {
            tracing::warn!(
                protocol = %self.protocol,
                "telemetry protocol is not one Claude Code accepts; not exporting"
            );
            return env;
        }

        env.insert("CLAUDE_CODE_ENABLE_TELEMETRY".to_owned(), "1".to_owned());
        env.insert("OTEL_METRICS_EXPORTER".to_owned(), "otlp".to_owned());
        env.insert(
            "OTEL_EXPORTER_OTLP_ENDPOINT".to_owned(),
            endpoint.to_owned(),
        );
        env.insert(
            "OTEL_EXPORTER_OTLP_PROTOCOL".to_owned(),
            self.protocol.clone(),
        );

        if let Some(headers) = self.headers.as_deref().filter(|h| !h.trim().is_empty()) {
            env.insert(
                "OTEL_EXPORTER_OTLP_HEADERS".to_owned(),
                headers.trim().to_owned(),
            );
        }

        // Set either way rather than only when on: the variable's absence and
        // `none` mean the same thing to Claude Code, but a person reading the
        // launch environment of a pane should be able to see that the decision
        // was made rather than wonder whether it was forgotten.
        env.insert(
            "OTEL_LOGS_EXPORTER".to_owned(),
            if self.logs { "otlp" } else { "none" }.to_owned(),
        );

        if let Some(interval) = self.export_interval_ms {
            env.insert(
                "OTEL_METRIC_EXPORT_INTERVAL".to_owned(),
                interval.to_string(),
            );
        }

        env
    }
}

impl Default for ProvisionConfig {
    fn default() -> Self {
        Self {
            directories: vec![
                DirectorySpec {
                    path: "node_modules".to_owned(),
                    strategy: DirStrategy::Clone,
                },
                DirectorySpec {
                    path: ".venv".to_owned(),
                    strategy: DirStrategy::Clone,
                },
                DirectorySpec {
                    path: "vendor".to_owned(),
                    strategy: DirStrategy::Clone,
                },
                // Rust build output is excluded by default: it is often tens of
                // gigabytes, and cargo already supports sharing it through
                // CARGO_TARGET_DIR. Switch to `clone` per project if wanted.
                DirectorySpec {
                    path: "target".to_owned(),
                    strategy: DirStrategy::Skip,
                },
            ],
            files: vec![
                ".env".to_owned(),
                ".env.local".to_owned(),
                ".env.*.local".to_owned(),
            ],
            post_command: Vec::new(),
            post_timeout_secs: 300,
        }
    }
}

impl ProvisionConfig {
    /// Loads a repository-local override from `<root>/.ket.toml`, if present.
    ///
    /// A repo-local file **replaces** the global provision config rather than
    /// merging into it. Merging two partial specifications produces a result
    /// neither file describes, which is exactly the wrong property for
    /// something that decides where secrets get copied.
    pub fn for_repo(global: &Self, root: &Path) -> Result<Self> {
        Ok(Self::repo_only(root)?.unwrap_or_else(|| global.clone()))
    }

    /// The `[provision]` section of `<root>/.ket.toml`, or `None` when the
    /// repository does not describe its own.
    pub fn repo_only(root: &Path) -> Result<Option<Self>> {
        let path = root.join(".ket.toml");

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(KetError::io(&path, e)),
        };

        #[derive(Deserialize)]
        struct RepoFile {
            provision: Option<ProvisionConfig>,
        }

        let parsed: RepoFile = toml::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?;

        Ok(parsed.provision)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 7717,
        }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            open_command: vec![
                "code".to_owned(),
                "--goto".to_owned(),
                "{path}:{line}:{col}".to_owned(),
            ],
            font_size: None,
            line_height: DEFAULT_LINE_HEIGHT,
        }
    }
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self { audio: true }
    }
}

/// What ket needs in order to read an OpenCode Go plan's usage.
///
/// **Why the key is a setting rather than something ket discovers.** OpenCode
/// itself has no plan quota — it is bring-your-own-key, and whatever limit
/// applies belongs to the backend provider you pointed it at. The quota that
/// *does* exist belongs to OpenCode Go, OpenCode's own hosted subscription,
/// and reading it needs a Go key.
///
/// ket will not go looking for that key. It sits in OpenCode's `auth.json`
/// and its `credential` table, and reading another application's credential
/// files is this repository's policy to refuse — see `AGENTS.md`. So the key
/// is pasted here by the person whose account it is, or set on
/// `OPENCODE_API_KEY`, and an empty field with neither means
/// [`crate::rate_limits::Provider::OpenCode`] keeps reporting
/// [`crate::rate_limits::SnapshotStatus::Unsupported`] rather than guessing.
/// The cookie exists for a legacy console account whose usage the keyed
/// endpoint cannot see.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OpenCodeGoConfig {
    /// An OpenCode Go API key, as pasted from the console.
    ///
    /// Left empty when `OPENCODE_API_KEY` is set instead.
    pub api_key: String,
    /// The `opencode.ai` session cookie, as pasted from a browser.
    ///
    /// Only needed for a legacy console (OpenCode Black) account, whose
    /// usage exists only behind the console session. Either a full `Cookie`
    /// header or the bare seal value; see
    /// [`crate::rate_limits::normalize_cookie`] for which forms are accepted
    /// and why a bare token is wrapped rather than rejected.
    pub session_cookie: String,
    /// A `wrk_…` workspace to read, when discovery picks the wrong one.
    ///
    /// Normally left empty: the cookie is enough to list the workspaces the
    /// account can see. It exists for an account with several, where the
    /// first one listed is not the one being billed.
    pub workspace_id: String,
}

impl OpenCodeGoConfig {
    /// Whether enough has been configured to attempt a fetch at all.
    ///
    /// The workspace id is deliberately not part of this: it is an override
    /// for a case discovery gets wrong, not a second required field. A key
    /// or a cookie pasted here is also what makes ket's own config file a
    /// credential worth writing privately — see `write_atomically`.
    pub fn is_configured(&self) -> bool {
        !self.session_cookie.trim().is_empty() || !self.api_key.trim().is_empty()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agents: default_agents(),
            agent: AgentConfig::default(),
            pack: PackConfig::default(),
            telemetry: TelemetryConfig::default(),
            rates: crate::rates::RateConfig::default(),
            admin: crate::admin::AdminConfig::default(),
            server: ServerConfig::default(),
            editor: EditorConfig::default(),
            viewer: ViewerConfig::default(),
            provision: ProvisionConfig::default(),
            hooks: HookConfig::default(),
            theme: ThemeConfig::default(),
            opencode_go: OpenCodeGoConfig::default(),
            merge: MergeConfig::default(),
            storage: StorageConfig::default(),
            phones: PhonesConfig::default(),
            jev: JevConfig::default(),
            general: GeneralConfig::default(),
            updates: UpdatesConfig::default(),
        }
    }
}

/// The four agents in rotation, with transports verified rather than assumed.
///
/// Each was checked by spawning it and completing an ACP `initialize` handshake,
/// not by reading its documentation. As of 2026-09-05 the first three speak
/// ACP protocol v1 and report `loadSession: true`, which is what makes Epic 9's
/// session persistence possible across the whole rotation; Grok was checked the
/// same way on 2026-09-29:
///
/// | Agent | How | Version handshaken |
/// |---|---|---|
/// | Claude Code | `npx @zed-industries/claude-code-acp@0.16.2` | 0.16.2 |
/// | Codex | `npx @zed-industries/codex-acp@0.16.0` | 0.16.0 |
/// | OpenCode | `opencode acp`, **native** — no adapter | 1.18.29 |
/// | Grok | `grok agent stdio`, **native** — no adapter | 1.0.44 |
///
/// All four also advertise `authMethods`, which is the concrete reason
/// `authenticate` is not optional: an implementation that assumes an agent is
/// ready to prompt the moment it spawns will hang against every one of them.
///
/// Re-verify before changing any of this. A guessed ACP transport fails at
/// runtime in a confusing way, which is exactly what the check exists to avoid.
pub(crate) fn default_agents() -> Vec<AgentSpec> {
    vec![
        AgentSpec {
            name: "claude".to_owned(),
            transport: Transport::Acp,
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@zed-industries/claude-code-acp@0.16.2".to_owned(),
            ],
            env: BTreeMap::new(),
            env_remove: vec![
                "CLAUDECODE".to_owned(),
                "CLAUDE_CODE_ENTRYPOINT".to_owned(),
                "CLAUDE_CODE_SSE_PORT".to_owned(),
                // Not a nesting guard like the others — this one turns off
                // transcript saving. ket is regularly launched from inside a
                // Claude session, so without stripping it every agent ket
                // starts records nothing, and a session with no transcript is
                // a session that cannot be resumed. That is what made a
                // worktree's work vanish across a restart.
                "CLAUDE_CODE_CHILD_SESSION".to_owned(),
            ],
            launch: None,
        },
        AgentSpec {
            name: "codex".to_owned(),
            transport: Transport::Acp,
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@zed-industries/codex-acp@0.16.0".to_owned(),
            ],
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            launch: None,
        },
        AgentSpec {
            name: "opencode".to_owned(),
            // The only one that needs no Node adapter: ACP is a subcommand of the
            // agent itself, so there is no extra runtime dependency to install.
            transport: Transport::Acp,
            command: "opencode".to_owned(),
            args: vec!["acp".to_owned()],
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            launch: None,
        },
        AgentSpec {
            name: "grok".to_owned(),
            // Native too. `-m` and `--reasoning-effort` belong to `agent`, not
            // to `stdio`, which rejects them — anything added to this spec's
            // args has to go before the last one.
            transport: Transport::Acp,
            command: "grok".to_owned(),
            args: vec!["agent".to_owned(), "stdio".to_owned()],
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            launch: None,
        },
    ]
}

/// Writes `text` to `path` through a sibling temporary file renamed into
/// place.
///
/// A crash may leave an old temporary file, but it cannot leave the real file
/// truncated halfway through a document. `private` writes it `0600`;
/// otherwise it keeps whatever permissions the file it replaces had.
pub(crate) fn write_atomically(path: &Path, text: &str, private: bool) -> Result<()> {
    let parent = path.parent().ok_or_else(|| KetError::Path {
        what: "directory of the config file",
        why: format!("{} has no parent", path.display()),
    })?;
    std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let temporary = parent.join(format!(".ket-{}.{}.tmp", std::process::id(), name));

    let write = || -> Result<()> {
        std::fs::write(&temporary, text).map_err(|e| KetError::io(&temporary, e))?;
        if private {
            // Set on the temporary file, before the rename that publishes
            // it: tightening afterwards would leave a window in which the
            // credential is readable at the real path.
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| KetError::io(&temporary, e))?;
        } else if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())
                .map_err(|e| KetError::io(&temporary, e))?;
        }
        std::fs::rename(&temporary, path).map_err(|e| KetError::io(path, e))
    };

    match write() {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

impl Config {
    /// Loads configuration from the default location.
    pub fn load() -> Result<Self> {
        let path = paths::config_file()?;
        Self::load_from(&path)
    }

    /// Persists configuration at the path [`Config::load`] reads.
    ///
    /// Settings are written through a sibling temporary file and renamed into
    /// place. A crash may leave an old temporary file, but it cannot leave the
    /// real config truncated halfway through a TOML document.
    pub fn save(&self) -> Result<()> {
        let path = paths::config_file()?;
        self.save_to(&path)
    }

    /// Persists configuration at an explicit path.
    ///
    /// Kept separate from [`Config::save`] so callers and tests can target an
    /// isolated config directory without changing process-global environment.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|e| KetError::Config(e.to_string()))?;
        // This file can hold an OpenCode Go session cookie, which is a live
        // credential — see [`OpenCodeGoConfig`]. Nothing but this user needs
        // to read their own config, and the alternative is a secret sitting at
        // whatever the umask happened to be.
        // This file may contain provider cookies or API keys. Keep the whole
        // document private regardless of which credential is configured.
        write_atomically(path, &text, true)
    }

    /// Loads configuration from a specific path.
    ///
    /// A missing file yields defaults rather than an error. A *malformed* file is
    /// an error — silently falling back to defaults because of a typo would be
    /// far more confusing than refusing to start.
    pub fn load_from(path: &Path) -> Result<Self> {
        let config = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(KetError::io(path, e)),
        };

        config.validate()?;
        Ok(config)
    }

    /// Checks internal consistency.
    pub fn validate(&self) -> Result<()> {
        if self.agents.is_empty() {
            return Err(KetError::Config("no agents configured".to_owned()));
        }

        let mut seen = BTreeMap::new();
        for agent in &self.agents {
            if agent.name.trim().is_empty() {
                return Err(KetError::Config("an agent has an empty name".to_owned()));
            }
            if seen.insert(&agent.name, ()).is_some() {
                return Err(KetError::Config(format!(
                    "duplicate agent name: {}",
                    agent.name
                )));
            }
            if agent
                .launch
                .as_ref()
                .is_some_and(|launch| launch.command.trim().is_empty())
            {
                return Err(KetError::Config(format!(
                    "agent {} has an empty launch command",
                    agent.name
                )));
            }
        }

        if self.editor.open_command.is_empty() {
            return Err(KetError::Config(
                "editor.open_command must not be empty".to_owned(),
            ));
        }

        if self.merge.commit_template.trim().is_empty() {
            return Err(KetError::Config(
                "merge.commit_template must not be empty".to_owned(),
            ));
        }

        // Refused here rather than substituted as-is at merge time, because
        // here is where somebody typed it and can still fix it. A `{summry}`
        // that survives to a commit is in the base branch's history for good.
        let unknown = self.merge.unknown_placeholders();
        if !unknown.is_empty() {
            return Err(KetError::Config(format!(
                "merge.commit_template uses {{{}}}, which nothing fills; the placeholders are {}",
                unknown.join("}, {"),
                COMMIT_PLACEHOLDERS
                    .iter()
                    .map(|name| format!("{{{name}}}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            )));
        }

        for (field, size) in [
            ("theme.code_font_size", Some(self.theme.code_font_size)),
            ("theme.ui_font_size", Some(self.theme.ui_font_size)),
            ("editor.font_size", self.editor.font_size),
        ] {
            let Some(size) = size else {
                // Unset, which is the editor following the code size rather
                // than a size of nought.
                continue;
            };
            if !FONT_SIZE_RANGE.contains(&size) {
                return Err(KetError::Config(format!(
                    "{field} must be between {} and {}, got {size}",
                    FONT_SIZE_RANGE.start(),
                    FONT_SIZE_RANGE.end(),
                )));
            }
        }

        let line_height = self.editor.line_height;
        if !line_height.is_finite() || !LINE_HEIGHT_RANGE.contains(&line_height) {
            return Err(KetError::Config(format!(
                "editor.line_height must be between {} and {}, got {line_height}",
                LINE_HEIGHT_RANGE.start(),
                LINE_HEIGHT_RANGE.end(),
            )));
        }

        // A relative directory would be relative to wherever ket happened to
        // be started from, which for the app is `/`.
        if let Some(dir) = self.general.worktree_dir.as_deref()
            && !(dir == "~" || dir.starts_with("~/") || Path::new(dir).is_absolute())
        {
            return Err(KetError::Config(format!(
                "general.worktree_dir must be an absolute path or start with ~/, got {dir:?}"
            )));
        }

        for (name, hooks) in self.hooks.by_point() {
            for hook in hooks {
                if hook.command.is_empty() {
                    return Err(KetError::Config(format!(
                        "a {name} hook has an empty command"
                    )));
                }
            }
        }

        Ok(())
    }

    /// The point size ket's own editor draws at.
    ///
    /// The override if there is one, and the shared code size otherwise. One
    /// function rather than the `unwrap_or` written out at each call site,
    /// because the editor asks in two places — the text and the cell width the
    /// wrap column is measured from — and the two disagreeing would wrap lines
    /// at the wrong column.
    pub fn editor_font_size(&self) -> u16 {
        self.editor.font_size.unwrap_or(self.theme.code_font_size)
    }

    /// Where new worktrees are made: [`GeneralConfig::worktree_dir`] with its
    /// `~/` expanded, or [`paths::worktrees_dir`] when none is set.
    pub fn worktrees_dir(&self) -> Result<PathBuf> {
        let Some(dir) = self.general.worktree_dir.as_deref() else {
            return paths::worktrees_dir();
        };
        Ok(match dir.strip_prefix("~/") {
            Some(rest) => paths::home()?.join(rest),
            None if dir == "~" => paths::home()?,
            None => PathBuf::from(dir),
        })
    }

    /// Looks up a configured agent by name.
    pub fn agent(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.iter().find(|a| a.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn the_shipped_code_size_is_twelve() {
        // The size the window opens at for anyone who never edits a config
        // file, which is nearly everyone.
        let config = Config::default();
        assert_eq!(config.theme.code_font_size, 12);
        assert_eq!(config.theme.ui_font_size, DEFAULT_UI_FONT_SIZE);
    }

    #[test]
    fn a_font_size_is_read_back_as_it_was_written() {
        let config: Config = toml::from_str("[theme]\ncode_font_size = 16\n").unwrap();
        config.validate().unwrap();
        assert_eq!(config.theme.code_font_size, 16);
        // A key the file did not mention keeps its default.
        assert_eq!(config.theme.ui_font_size, DEFAULT_UI_FONT_SIZE);
    }

    #[test]
    fn an_unusable_font_size_is_refused_rather_than_drawn() {
        // Zero would render an invisible grid and no explanation.
        let config: Config = toml::from_str("[theme]\ncode_font_size = 0\n").unwrap();
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("code_font_size"), "unhelpful message: {err}");

        let config: Config = toml::from_str("[theme]\nui_font_size = 400\n").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn defaults_ship_all_three_agents() {
        // No agent is privileged.
        let config = Config::default();
        for name in ["claude", "codex", "opencode"] {
            assert!(
                config.agent(name).is_some(),
                "missing default agent: {name}"
            );
        }
    }

    #[test]
    fn every_default_agent_speaks_acp() {
        // Verified by handshake, not by documentation — see `default_agents`.
        // If one of these is ever demoted to PTY, it should be because a probe
        // said so, and this test is where that gets recorded.
        for agent in &Config::default().agents {
            assert_eq!(
                agent.transport,
                Transport::Acp,
                "{} was demoted to PTY without a note",
                agent.name
            );
        }
    }

    #[test]
    fn the_claude_adapter_sheds_the_nesting_guard() {
        // ket exists to run agents, and is itself run from inside one. Without
        // this the `claude` agent fails outright in that situation, which is the
        // situation its author is in every day.
        let config = Config::default();
        let claude = config.agent("claude").expect("claude");
        assert!(claude.env_remove.iter().any(|v| v == "CLAUDECODE"));
    }

    #[test]
    fn opencode_needs_no_node_adapter() {
        // The others are Node subprocesses and a real runtime dependency;
        // OpenCode speaks ACP itself. Worth keeping true, and worth noticing if
        // it stops being.
        let config = Config::default();
        let opencode = config.agent("opencode").expect("opencode");
        assert_eq!(opencode.command, "opencode");
        assert_eq!(opencode.args, ["acp"]);
    }

    #[test]
    fn tool_permissions_fall_back_to_the_default() {
        let mut permissions = PermissionConfig::default();
        permissions
            .tools
            .insert("read".to_owned(), PermissionPolicy::Allow);

        assert_eq!(permissions.for_tool("read"), PermissionPolicy::Allow);
        assert_eq!(permissions.for_tool("bash"), PermissionPolicy::Prompt);
    }

    #[test]
    fn permissions_default_to_asking() {
        // An agent running unattended is still running commands on your machine.
        // Defaulting to `allow` would make that call on the user's behalf.
        assert_eq!(
            PermissionConfig::default().default,
            PermissionPolicy::Prompt
        );
    }

    #[test]
    fn default_bind_is_loopback() {
        // This server spawns processes. If this ever defaults wider, it is a
        // security regression, not a convenience.
        assert!(Config::default().server.bind.is_loopback());
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let original = Config::default();
        let text = toml::to_string_pretty(&original).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn partial_config_inherits_defaults() {
        let parsed: Config =
            toml::from_str("[server]\nbind = \"127.0.0.1\"\nport = 9999\n").unwrap();
        assert_eq!(parsed.server.port, 9999);
        assert_eq!(parsed.agents, default_agents());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // A typo in a config file should say so, not be silently ignored.
        let result = toml::from_str::<Config>("[server]\nbnid = \"127.0.0.1\"\n");
        assert!(result.is_err());
    }

    #[test]
    fn duplicate_agent_names_are_rejected() {
        let mut config = Config::default();
        let first = config.agents[0].clone();
        config.agents.push(first);
        assert!(matches!(config.validate(), Err(KetError::Config(_))));
    }

    #[test]
    fn empty_agent_list_is_rejected() {
        let config = Config {
            agents: Vec::new(),
            ..Config::default()
        };
        assert!(matches!(config.validate(), Err(KetError::Config(_))));
    }

    #[test]
    fn hooks_are_empty_by_default() {
        // Running a command on every lifecycle transition is a side effect a
        // person did not necessarily ask for; a fresh install must run none.
        let hooks = HookConfig::default();
        for (name, list) in hooks.by_point() {
            assert!(list.is_empty(), "{name} hooks should start empty");
        }
    }

    #[test]
    fn a_hook_with_an_empty_command_is_rejected() {
        // Caught at load time rather than left to fail confusingly the first
        // time the hook point fires.
        let mut config = Config::default();
        config.hooks.worktree_created.push(HookSpec {
            command: Vec::new(),
            timeout_secs: 30,
        });
        assert!(matches!(config.validate(), Err(KetError::Config(_))));
    }

    #[test]
    fn a_hook_missing_its_timeout_inherits_the_default() {
        let parsed: HookConfig =
            toml::from_str("worktree_created = [{ command = [\"./notify.sh\"] }]\n").unwrap();
        assert_eq!(parsed.worktree_created[0].timeout_secs, 30);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let parsed = Config::load_from(Path::new("/nonexistent/ket/config.toml")).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn theme_defaults_to_dark_with_no_custom_theme() {
        // Dark rather than following the OS. Deferring to the host is the more
        // polite default and remains one setting away, but ket is read for
        // hours and dark is the palette it is designed and contrast-checked
        // against — `System` handed a light-mode Mac the light palette on first
        // run without anyone having chosen it.
        let config = Config::default();
        assert_eq!(
            config.theme.appearance,
            crate::theme::AppearancePreference::Dark
        );
        assert_eq!(config.theme.name, None);
    }

    #[test]
    fn partial_config_can_pin_an_appearance_without_naming_a_theme() {
        let parsed: Config = toml::from_str("[theme]\nappearance = \"dark\"\n").unwrap();
        assert_eq!(
            parsed.theme.appearance,
            crate::theme::AppearancePreference::Dark
        );
        assert_eq!(parsed.theme.name, None);
    }

    #[test]
    fn malformed_file_is_an_error_not_a_silent_default() {
        let dir = std::env::temp_dir().join("ket-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("malformed.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();

        let result = Config::load_from(&path);
        std::fs::remove_file(&path).ok();

        assert!(matches!(result, Err(KetError::Config(_))));
    }

    #[test]
    fn the_default_permission_policy_reads_as_the_manual_preset() {
        use crate::agents::PermissionPreset;
        assert_eq!(
            PermissionConfig::default().preset(),
            PermissionPreset::Manual
        );
    }

    #[test]
    fn a_preset_round_trips() {
        use crate::agents::PermissionPreset;

        let mut permissions = PermissionConfig::default();
        permissions.apply_preset(PermissionPreset::Yolo);

        assert_eq!(permissions.default, PermissionPolicy::Allow);
        assert_eq!(permissions.preset(), PermissionPreset::Yolo);
    }

    #[test]
    fn a_hand_tuned_policy_reports_custom_rather_than_claiming_a_preset() {
        use crate::agents::PermissionPreset;

        let mut permissions = PermissionConfig::default();
        permissions
            .tools
            .insert("shell".to_owned(), PermissionPolicy::Deny);

        // The default is still Prompt, so a naive reading would call this Manual.
        // Saying so would be a lie: this policy denies a tool that Manual prompts
        // for, and a settings screen must not claim otherwise.
        assert_eq!(permissions.preset(), PermissionPreset::Custom);
    }

    #[test]
    fn applying_a_preset_leaves_per_tool_decisions_alone() {
        use crate::agents::PermissionPreset;

        let mut permissions = PermissionConfig::default();
        permissions
            .tools
            .insert("shell".to_owned(), PermissionPolicy::Deny);
        permissions.apply_preset(PermissionPreset::Yolo);

        // Someone who denied a tool by name meant it; a preset speaks about
        // everything they did not name.
        assert_eq!(
            permissions.for_tool("shell"),
            PermissionPolicy::Deny,
            "a named tool keeps its decision"
        );
        assert_eq!(
            permissions.for_tool("anything-else"),
            PermissionPolicy::Allow
        );
    }

    /// A unique scratch directory for one test, cleaned up by the caller.
    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ket-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn repo_only_is_none_without_a_ket_toml() {
        let root = temp_root("repo-only-absent");
        assert!(ProvisionConfig::repo_only(&root).unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repo_only_reads_the_repos_own_provision_section() {
        let root = temp_root("repo-only-present");
        std::fs::write(
            root.join(".ket.toml"),
            "[provision]\npost_command = [\"npm\", \"install\"]\n",
        )
        .unwrap();

        let repo = ProvisionConfig::repo_only(&root).unwrap().unwrap();
        assert_eq!(repo.post_command, vec!["npm", "install"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repo_only_is_none_with_a_ket_toml_that_has_no_provision_table() {
        // `for_repo`'s integration tests (`tests/config.rs`) cover falling
        // back to the global config; this is `repo_only`'s own half of that:
        // the untrusted-provisioning path in `Workspace::provision_worktree`
        // needs to tell "no opinion" apart from "an empty one".
        let root = temp_root("repo-only-no-table");
        std::fs::write(root.join(".ket.toml"), "# nothing about provisioning\n").unwrap();

        assert!(ProvisionConfig::repo_only(&root).unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
