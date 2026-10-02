//! Making agents report their own lifecycle back to ket.
//!
//! Every other way of knowing what an agent is doing is inference. Watching a
//! terminal's foreground process tells you *something* is running; reading a
//! transcript tells you what happened, minutes late. Neither can distinguish
//! "waiting on the model" from "running your test suite" from "blocked on a
//! permission prompt", which are the three states worth telling apart.
//!
//! Agents already publish exactly that, to anyone who asks. Claude Code, Codex
//! and Grok each support hooks: shell commands the agent runs at fixed points
//! in its own lifecycle, handed a JSON payload on stdin. Registering ket as
//! that command turns guesswork into a push feed.
//!
//! **This writes into another application's configuration**, which is not
//! something to do quietly. [`HookInstaller::install`] merges into the agent's
//! settings rather than replacing them, keeps a backup, and is reversible with
//! [`HookInstaller::remove`]. The installs are *global* — an agent's user-level
//! config, not a per-project one — because a session can start anywhere and a
//! hook registered per project would miss every session outside it. The cost is
//! that ket's hook then runs for sessions in projects ket knows nothing about;
//! those report an empty worktree and are dropped on arrival.
//!
//! **Correlation is by injected environment, not by guessing.** When ket
//! launches an agent it sets [`WORKTREE_ENV`] and friends in the child's
//! environment; the hook script echoes them back with the payload. That is how
//! a report finds its worktree, and it is why this is reliable where matching
//! on a process name or a directory is not.
//!
//! **A report that cannot be delivered is not lost.** The script posts to ket's
//! listener and, failing that, appends to a spool file. Closing ket during a
//! long agent run should not put a hole in its history.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::event::AgentState;
use crate::{KetError, Result};

/// Environment variable naming the worktree an agent was launched for.
pub const WORKTREE_ENV: &str = "KET_WORKTREE_ID";

/// Environment variable naming the pane it was launched into.
pub const PANE_ENV: &str = "KET_PANE_KEY";

/// Environment variable carrying the port ket is listening on.
pub const PORT_ENV: &str = "KET_HOOK_PORT";

/// Environment variable carrying the token that authenticates one launch.
///
/// The listener binds to loopback, which keeps other machines out but not
/// other processes on this one. A token minted per launch means a report is
/// only believed if it came from something ket started.
pub const TOKEN_ENV: &str = "KET_LAUNCH_TOKEN";

/// Environment variable carrying the hook contract's version.
///
/// Sent so a ket that has been upgraded under a long-running agent can tell
/// that a report came from the older script it installed.
pub const VERSION_ENV: &str = "KET_HOOK_VERSION";

/// A session ket is resuming in this pane.
///
/// Launch metadata for the host, not part of the agent hook contract: the
/// host removes it before spawning the terminal. Codex restores the hook
/// environment saved in a resumed session, so its reports otherwise name the
/// pane the conversation originally ran in rather than the pane it is in now.
pub const RESUME_SESSION_ENV: &str = "KET_RESUME_SESSION_ID";

/// The hook contract's version. Bumped when the payload's shape changes.
pub const HOOK_VERSION: &str = "1";

/// One moment in an agent's lifecycle.
///
/// Named after what happened rather than after any one agent's spelling of it,
/// so a second agent is a new arm of [`HookEvent::from_hook_name`] and not a
/// new enum. Codex turned out to need no new arm at all: it spells its events
/// exactly as Claude Code does, down to `hook_event_name` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookEvent {
    /// A session began.
    SessionStart,
    /// A person sent a prompt.
    UserPrompt,
    /// The model asked to run a tool.
    PreTool,
    /// A tool finished.
    PostTool,
    /// A tool failed.
    PostToolFailure,
    /// The agent is blocked on a permission decision.
    PermissionRequest,
    /// A permission decision came back no.
    PermissionDenied,
    /// The agent wants the person, and cannot go on until it gets them.
    ///
    /// Narrower than Claude Code's hook of the same name, which covers a
    /// dozen unrelated announcements — see [`notification_event`], which is
    /// what decides whether one of them is this.
    Notification,
    /// The agent finished its turn.
    Stop,
    /// The person cancelled the turn.
    ///
    /// Codex emits this instead of `Stop` for an interrupted response. Without
    /// it, the last working report remains authoritative and the rail keeps
    /// moving after the model has stopped.
    Interrupt,
    /// The agent's turn ended badly.
    StopFailure,
    /// A subagent began.
    SubagentStart,
    /// A subagent finished.
    SubagentStop,
    /// The session is over.
    SessionEnd,
    /// The conversation was compacted.
    Compact,
    /// The agent is idle and waiting.
    Idle,
    /// One of the lead's teammates finished a turn. Says nothing about the lead.
    TeammateIdle,
}

impl HookEvent {
    /// The event `name` names, if it is one ket handles.
    ///
    /// Not per-agent: Claude Code and Codex use the same spellings for the
    /// events they share, and both deliver them under `hook_event_name`. A
    /// third agent that did not would get its own arm here rather than its own
    /// enum — see this type's doc comment.
    pub fn from_hook_name(name: &str) -> Option<Self> {
        Some(match name {
            "SessionStart" => Self::SessionStart,
            "UserPromptSubmit" => Self::UserPrompt,
            "PreToolUse" => Self::PreTool,
            "PostToolUse" => Self::PostTool,
            "PostToolUseFailure" => Self::PostToolFailure,
            "PermissionRequest" => Self::PermissionRequest,
            "PermissionDenied" => Self::PermissionDenied,
            "Notification" => Self::Notification,
            "SessionEnd" => Self::SessionEnd,
            "Stop" => Self::Stop,
            "Interrupt" => Self::Interrupt,
            // Grok's name for a turn that ended without finishing: Ctrl+C, a
            // declined permission prompt, a turn limit.
            "StopCancelled" => Self::Interrupt,
            "StopFailure" => Self::StopFailure,
            "SubagentStart" => Self::SubagentStart,
            "SubagentStop" => Self::SubagentStop,
            "PostCompact" => Self::Compact,
            "TeammateIdle" => Self::TeammateIdle,
            _ => return None,
        })
    }

    /// Every event ket asks Claude Code for.
    ///
    /// Each name has to be one Claude Code knows: it validates them, and an
    /// entry under a name it does not recognise is a hook that silently never
    /// runs. This is not every event it offers — a status light has no use for
    /// `FileChanged` or `MessageDisplay` — but it is every one that moves the
    /// state machine, plus the two that say a turn is over.
    pub const CLAUDE_EVENTS: [&'static str; 15] = [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "PermissionRequest",
        "PermissionDenied",
        "Notification",
        "Stop",
        "StopFailure",
        "SubagentStart",
        "SubagentStop",
        "PostCompact",
        "TeammateIdle",
    ];

    /// Every event ket asks Codex for.
    ///
    /// The intersection of what Codex offers and what [`HookEvent`] can do
    /// something with, which is smaller than [`HookEvent::CLAUDE_EVENTS`] in
    /// both directions. Codex has no `PostToolUseFailure`, `PermissionDenied`,
    /// `Notification`, `StopFailure` or `TeammateIdle`; it has `PreCompact`,
    /// which this enum has no arm for. Registering a name ket cannot map would
    /// spawn a subprocess per event to produce a report that is then dropped,
    /// so it is left out rather than parsed and discarded.
    ///
    /// Confirmed against Codex CLI 0.153.4's own `~/.codex/hooks.json` on this
    /// machine, not only against its documentation.
    pub const CODEX_EVENTS: [&'static str; 11] = [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Stop",
        "Interrupt",
        "SubagentStart",
        "SubagentStop",
        "PostCompact",
    ];

    /// Every event ket asks Grok for.
    ///
    /// Grok takes Claude Code's names, plus `StopCancelled` for a turn that
    /// ended without finishing. Its subagent events are left out: a subagent's
    /// tools are not the pane's, and [`grok_payload`] drops the ones that fire
    /// inside a subagent for the same reason. `PermissionRequest` does not
    /// exist — a prompt waiting on a person arrives as a `Notification` of
    /// type `permission_prompt`.
    ///
    /// Confirmed against Grok 1.0.44's hook runner on 2026-09-29.
    pub const GROK_EVENTS: [&'static str; 12] = [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "PermissionDenied",
        "Notification",
        "Stop",
        "StopFailure",
        "StopCancelled",
        "PostCompact",
    ];

    /// Where this event leaves the session's state machine.
    ///
    /// `None` for events that say something happened without moving the
    /// session — a compaction does not change whether the agent is thinking.
    /// The states are [`crate::agent`]'s, unchanged: this is a new *source*
    /// for that machine, not a second one beside it.
    pub fn state(self) -> Option<AgentState> {
        Some(match self {
            Self::SessionStart => AgentState::Starting,
            Self::UserPrompt => AgentState::Thinking,
            Self::PreTool | Self::SubagentStart => AgentState::ExecutingTool,
            Self::PostTool | Self::PostToolFailure | Self::SubagentStop => AgentState::Thinking,
            // Both mean the same thing to a person reading a sidebar: it has
            // stopped, and it is stopped on them. `Notification` reaches here
            // only once [`notification_event`] has established that this one
            // actually is that — most of them are not.
            Self::PermissionRequest | Self::Notification => AgentState::AwaitingPermission,
            // A refusal is an answer, and the turn carries on from it.
            Self::PermissionDenied => AgentState::Thinking,
            Self::Stop | Self::Interrupt | Self::StopFailure | Self::Idle | Self::SessionEnd => {
                AgentState::Idle
            }
            Self::Compact | Self::TeammateIdle => return None,
        })
    }

    /// Whether this event means the turn ended badly.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::StopFailure | Self::PostToolFailure)
    }
}

/// One report from an agent's hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookReport {
    /// Which agent sent it.
    pub agent: String,
    /// What happened.
    pub event: HookEvent,
    /// The worktree it was launched for, when ket launched it.
    ///
    /// Empty for a session started outside ket — the cost of a global install.
    /// Those are dropped rather than guessed at.
    pub worktree: Option<String>,
    /// The directory the agent says this event belongs to.
    ///
    /// Codex can service a new thread from a process that inherited another
    /// thread's launch environment. In that case [`Self::worktree`] is stale,
    /// while the `cwd` carried by every hook payload still names the thread's
    /// actual checkout. The UI resolves this path back to one of its worktree
    /// ids before handing the report to the activity tracker.
    #[serde(default)]
    pub cwd: Option<Box<PathBuf>>,
    /// The pane it was launched into, when there was one.
    pub pane: Option<String>,
    /// The agent's own session identifier, when the payload carries one.
    pub session: Option<String>,
    /// The tool a `PreTool`/`PostTool` event names, verbatim.
    ///
    /// Kept beside [`HookReport::note`] rather than instead of it: the note is
    /// a phrase for a person to read on a row, already shortened and rewritten
    /// for that, and this is the raw name for counting. An MCP tool arrives
    /// here as `mcp__<server>__<tool>`, which is the only evidence ket gets of
    /// which servers a worktree actually uses — see [`mcp_server`].
    pub tool_name: Option<String>,
    /// The agent's own words for whatever it is in the middle of.
    ///
    /// Two things, because the state already says which: on a tool call it is
    /// what the tool is doing, and on a stop it is the last thing the agent
    /// said. Both are phrases rather than raw payload — "running a tool" was
    /// the same six words for a `cargo clippy` that takes two minutes and a
    /// file read that takes none, and a finished row said nothing at all
    /// while the message explaining itself sat in the payload. See
    /// [`tool_phrase`] and [`closing_line`].
    pub note: Option<String>,
    /// Whether the tool call in flight is `git` rewriting the worktree.
    ///
    /// Separate from [`Self::note`] and deliberately so: the note prefers the
    /// agent's own `description` ("read the dialog conflict"), which is the
    /// right thing to *show* and says nothing about what the command is. So a
    /// `git rebase --continue` run by an agent looked exactly like a file
    /// read, and the row stayed on the running colour throughout — the bug
    /// this field exists to fix. Read off the command itself, whether or not
    /// there is a description to show beside it.
    pub mutating_git: bool,
    /// When ket received it.
    pub at_ms: u64,
    /// The subagent that sent it — the payload's `agent_id`, which the lead's
    /// own events never carry.
    #[serde(default)]
    pub subagent: Option<String>,
    /// The subagent's kind ("Explore", "general-purpose"), only when
    /// [`Self::subagent`] is set: the lead sends `agent_type` too.
    #[serde(default)]
    pub subagent_type: Option<String>,
    /// What the subagent was sent to do, from the sidecar Claude writes beside
    /// its transcript — see [`subagent_description`].
    #[serde(default)]
    pub subagent_description: Option<String>,
    /// The subagent's model, where the agent says (Codex does).
    #[serde(default)]
    pub subagent_model: Option<String>,
    /// The tool call this event is about, for matching a permission prompt to
    /// the call that answers it.
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The teammate a `TeammateIdle` names.
    #[serde(default)]
    pub teammate: Option<String>,
    /// A `SessionStart` that begins a new conversation in the pane, as opposed
    /// to one that follows a compaction.
    #[serde(default)]
    pub fresh_session: bool,
    /// The background agents Claude lists on a `Stop`. `None` when the field
    /// was absent, which proves nothing either way.
    #[serde(default)]
    pub background: Option<Vec<BackgroundTask>>,
    /// What a `PermissionRequest` is asking a person to allow — see
    /// [`PermissionQuestion`]. Only on that event.
    #[serde(default)]
    pub question: Option<Box<PermissionQuestion>>,
    /// Which call a tool call's own events are about, in a question's shape:
    /// the status tells the call a prompt holds from others running beside
    /// it by this. Claude's `PermissionRequest` carries no `tool_use_id`.
    #[serde(default)]
    pub call: Option<Box<PermissionQuestion>>,
}

/// A permission prompt, as the agent's hook described it: the tool, and
/// what it wants to do with it.
///
/// Kept whole, where a row's note is a clipped phrase: a phone answering the
/// prompt has to see the command it is allowing, not "running cargo".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionQuestion {
    /// `Bash`, `Edit`, `WebFetch`…
    pub tool: String,
    /// The command, the file, the address; `None` when the input names none.
    pub subject: Option<String>,
    /// Which prompt this is: set when a pane's status takes it in, new for
    /// every `PermissionRequest`, so an answer given to one prompt can be
    /// refused once another has taken its place. `0` until then.
    #[serde(default)]
    pub id: u64,
    /// What the agent is asking in its own words, when the prompt is a
    /// question or a plan rather than a permission — see [`Prompt`].
    #[serde(default)]
    pub prompt: Option<Prompt>,
}

/// A prompt that is not a permission: Claude asking a question with options
/// (`AskUserQuestion`), or a plan it wants approved (`ExitPlanMode`).
///
/// Neither is answered with the permission menu's keys — its `1` means
/// something else, and the menu moves with the session's mode — so these are
/// answered through the hook itself: see [`Waiters`] and [`Decision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Prompt {
    /// One to four questions, each with its options.
    Ask {
        /// In the order Claude asked them, which is the order answers go in.
        questions: Vec<Asked>,
    },
    /// A plan. Its text is in the conversation; the prompt only asks to go.
    Plan,
}

/// One question an `AskUserQuestion` asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    /// The question, which is also the key its answer goes back under.
    pub question: String,
    /// A short tag for it: "Scope", "Colour".
    pub header: String,
    /// What can be picked.
    pub options: Vec<Offered>,
    /// Whether any number can be picked, rather than one.
    pub multi_select: bool,
}

/// One option an [`Asked`] offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offered {
    /// What picking it answers.
    pub label: String,
    /// What it means, or empty.
    pub description: String,
}

/// The tools whose prompts are a [`Prompt`] rather than a permission.
pub const PROMPT_TOOLS: [&str; 2] = ["AskUserQuestion", "ExitPlanMode"];

impl Prompt {
    /// The prompt a tool call raises, from its input — `None` for a tool that
    /// asks for a permission, or input that asks nothing.
    fn of(tool: &str, input: Option<&serde_json::Value>) -> Option<Self> {
        let text = |value: &serde_json::Value, key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(|s| s.trim().to_owned())
                .unwrap_or_default()
        };
        match tool {
            "AskUserQuestion" => {
                let questions: Vec<Asked> = input?
                    .get("questions")?
                    .as_array()?
                    .iter()
                    .filter_map(|question| {
                        let options: Vec<Offered> = question
                            .get("options")?
                            .as_array()?
                            .iter()
                            .map(|option| Offered {
                                label: text(option, "label"),
                                description: text(option, "description"),
                            })
                            .filter(|option| !option.label.is_empty())
                            .collect();
                        Some(Asked {
                            // Kept exactly as sent: the answer goes back
                            // under this text, and Claude matches it whole.
                            question: question.get("question")?.as_str()?.to_owned(),
                            header: text(question, "header"),
                            options,
                            multi_select: question
                                .get("multiSelect")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                        })
                    })
                    .collect();
                (!questions.is_empty()).then_some(Self::Ask { questions })
            }
            "ExitPlanMode" => Some(Self::Plan),
            _ => None,
        }
    }
}

/// One agent in the `background_tasks` inventory Claude attaches to `Stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTask {
    /// Its `agent_id`. Meaningless for a teammate entry.
    pub id: String,
    /// Its kind, when listed.
    pub agent_type: Option<String>,
    /// What it was sent to do, when listed.
    pub description: Option<String>,
    /// Whether its status is anything but finished.
    pub running: bool,
    /// A `teammate` entry: listed as running for as long as the team exists,
    /// under an id that never matches its lifecycle events.
    pub teammate: bool,
}

/// The statuses that mean a background task is over. Anything else, including
/// a status ket has never seen, counts as still running.
const FINISHED_STATUSES: [&str; 20] = [
    "idle",
    "done",
    "success",
    "succeeded",
    "complete",
    "completed",
    "finished",
    "failed",
    "error",
    "terminated",
    "exited",
    "aborted",
    "expired",
    "skipped",
    "crashed",
    "killed",
    "cancelled",
    "canceled",
    "timed_out",
    "stopped",
];

/// The agent entries of a payload's `background_tasks`, or `None` when it has
/// none at all. Shells and monitors are left out: they are not agents.
fn background_tasks(payload: &serde_json::Value) -> Option<Vec<BackgroundTask>> {
    let raw = payload.get("background_tasks")?.as_array()?;
    let text = |item: &serde_json::Value, key: &str| {
        item.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Some(
        raw.iter()
            .filter_map(|item| {
                let kind = text(item, "type")?.to_lowercase();
                if !matches!(
                    kind.as_str(),
                    "subagent" | "local_agent" | "local_subagent" | "teammate"
                ) {
                    return None;
                }
                let status = text(item, "status").map(|s| s.to_lowercase());
                Some(BackgroundTask {
                    id: text(item, "id")?,
                    agent_type: text(item, "agent_type"),
                    description: text(item, "description").map(|d| clip(&readable(&d))),
                    running: status.is_none_or(|s| !FINISHED_STATUSES.contains(&s.as_str())),
                    teammate: kind == "teammate",
                })
            })
            .collect(),
    )
}

/// What a Claude subagent was sent to do, read from the `.meta.json` Claude
/// writes beside the subagent's transcript when it spawns it.
///
/// Not part of any documented contract, so every failure is `None` and the
/// row falls back to the subagent's kind.
fn subagent_description(payload: &serde_json::Value, agent_id: &str) -> Option<String> {
    // It becomes part of a path.
    if !agent_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let text = |key: &str| payload.get(key).and_then(|v| v.as_str());
    let sidecar = match text("agent_transcript_path") {
        Some(path) => PathBuf::from(path.strip_suffix(".jsonl")?.to_owned() + ".meta.json"),
        None => PathBuf::from(text("transcript_path")?.strip_suffix(".jsonl")?)
            .join("subagents")
            .join(format!("agent-{agent_id}.meta.json")),
    };
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(sidecar).ok()?).ok()?;
    let description = meta.get("description")?.as_str()?.trim();
    (!description.is_empty()).then(|| clip(&readable(description)))
}

/// Installs and removes ket's hooks in one agent's own configuration.
pub trait HookInstaller: Send + Sync {
    /// The agent's name, matching [`crate::config::AgentSpec::name`].
    fn agent(&self) -> &str;

    /// Whether ket's hooks are currently registered.
    fn is_installed(&self) -> Result<bool>;

    /// Registers them, merging into whatever is already configured.
    fn install(&self, script: &Path) -> Result<()>;

    /// Removes only ket's own entries, leaving everything else alone.
    fn remove(&self) -> Result<()>;
}

// ---- claude -----------------------------------------------------------------

/// Claude Code: hook commands in `~/.claude/settings.json`.
pub struct ClaudeHooks {
    /// The settings file, overridable so this is exercisable without a home.
    settings: PathBuf,
}

impl Default for ClaudeHooks {
    fn default() -> Self {
        Self {
            settings: crate::sessions::claude_config_dir().join("settings.json"),
        }
    }
}

impl ClaudeHooks {
    /// Hooks written into a specific settings file.
    pub fn at(settings: PathBuf) -> Self {
        Self { settings }
    }

    /// The settings document, or an empty one when there is no file yet.
    fn read(&self) -> Result<serde_json::Value> {
        read_settings(&self.settings)
    }

    /// Writes the document back, keeping a copy of what was there before.
    fn write(&self, document: &serde_json::Value) -> Result<()> {
        write_settings(&self.settings, document)
    }
}

/// One agent's settings document, or an empty one when there is no file yet.
fn read_settings(path: &Path) -> Result<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Value::Object(serde_json::Map::new()))
        }
        Err(e) => Err(KetError::io(path, e)),
    }
}

/// Writes a settings document back, keeping a copy of what was there before.
///
/// The backup is the whole point of the ceremony: this file is the user's, it
/// is large, and ket is a guest in it.
fn write_settings(path: &Path, document: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
    }

    if path.exists() {
        let backup = path.with_extension("json.ket-backup");
        std::fs::copy(path, &backup).map_err(|e| KetError::io(&backup, e))?;
    }

    let text = serde_json::to_string_pretty(document)
        .map_err(|e| KetError::Config(format!("could not serialise settings: {e}")))?;
    std::fs::write(path, text).map_err(|e| KetError::io(path, e))
}

/// How ket recognises the hook entries it owns.
///
/// Matched against the *command*, not against a field of its own: the entry
/// has to be exactly the shape Claude Code expects, and the script's name is
/// already unique enough to identify. Kept as a key check too, so an entry
/// written by an earlier ket that did add a field is still removable.
pub const OWNER_MARK: &str = "ket-agent-hook";

impl HookInstaller for ClaudeHooks {
    fn agent(&self) -> &str {
        "claude"
    }

    /// Whether ket's entries are there *and* are the ones this version asks
    /// for.
    ///
    /// Not "is there a ket entry anywhere": that answer made the event list a
    /// one-time write. Someone who had ever run ket kept whatever set the ket
    /// that ran first installed, so a later version could add an event — a
    /// permission prompt, a session ending — and never hear about it on the
    /// only machines that mattered. The script is rewritten on every install
    /// for exactly this reason; the entries that point at it were the half
    /// that got left behind.
    fn is_installed(&self) -> Result<bool> {
        let document = self.read()?;
        let Some(hooks) = document.get("hooks").and_then(|h| h.as_object()) else {
            return Ok(false);
        };

        let ours: std::collections::BTreeSet<&str> = hooks
            .iter()
            .filter(|(event, entries)| {
                entries.as_array().is_some_and(|list| {
                    list.iter().any(|entry| {
                        // An entry from before prompts were held open has
                        // Claude's own timeout, which would cut the wait short.
                        is_kets(entry)
                            && (event.as_str() != "PermissionRequest"
                                || entry
                                    .pointer("/hooks/0/timeout")
                                    .and_then(serde_json::Value::as_u64)
                                    == Some(PROMPT_WAIT_SECS))
                    })
                })
            })
            .map(|(event, _)| event.as_str())
            .collect();

        Ok(ours == HookEvent::CLAUDE_EVENTS.into_iter().collect())
    }

    fn install(&self, script: &Path) -> Result<()> {
        let mut document = self.read()?;

        // Anything already there stays: a settings file with a user's own
        // hooks in it must come out of this with those hooks still in it.
        let root = document
            .as_object_mut()
            .ok_or_else(|| KetError::Config("settings.json is not an object".to_owned()))?;
        let hooks = root
            .entry("hooks")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| KetError::Config("settings.json: hooks is not an object".to_owned()))?;

        for event in HookEvent::CLAUDE_EVENTS {
            let entries = hooks
                .entry(event)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| {
                    KetError::Config(format!("settings.json: hooks.{event} is not an array"))
                })?;

            // Idempotent: installing twice leaves one entry, not two.
            entries.retain(|entry| !is_kets(entry));
            entries.push(kets_entry(event, script));
        }

        // An event an earlier ket asked for and this one does not. Left alone
        // it would keep firing the script forever, which is a subprocess per
        // event for a report nothing reads.
        for (event, entries) in hooks.iter_mut() {
            if HookEvent::CLAUDE_EVENTS.contains(&event.as_str()) {
                continue;
            }
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        self.write(&document)
    }

    fn remove(&self) -> Result<()> {
        let mut document = self.read()?;
        let Some(hooks) = document
            .as_object_mut()
            .and_then(|root| root.get_mut("hooks"))
            .and_then(|hooks| hooks.as_object_mut())
        else {
            return Ok(());
        };

        for entries in hooks.values_mut() {
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        // An event ket emptied goes with it, so removing leaves no trace.
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        self.write(&document)
    }
}

/// How long Claude lets a `PermissionRequest` hook run, in seconds: the
/// longest a question or a plan can wait on a phone. The script's own wait is
/// a little shorter, so it answers `{}` before Claude gives up on it.
pub const PROMPT_WAIT_SECS: u64 = 3600;

/// The events that take a `matcher`.
///
/// Only the tool-shaped ones. An entry carrying a matcher on any of the others
/// is rejected by Claude Code's settings validation, and a rejected entry is a
/// hook that silently never runs — which is exactly what happened when ket
/// wrote one onto every event it asked for.
const MATCHER_EVENTS: [&str; 5] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
];

/// One hook entry, in the shape Claude Code expects.
///
/// Deliberately no ownership key: an entry is `hooks`, plus `matcher` where
/// the event allows one, and nothing else. ket marks its entries by the script
/// they call rather than by an extra field, because an unknown field is
/// another way to have the entry thrown out.
fn kets_entry(event: &str, script: &Path) -> serde_json::Value {
    let mut hook = serde_json::json!({
        "type": "command",
        "command": format!("/bin/sh {} 2>/dev/null || printf '{{}}\\n'", script.display()),
    });
    // A question or a plan holds its hook open until a phone answers it —
    // see `Waiters` — and Claude's own ten minutes is shorter than a walk.
    if event == "PermissionRequest" {
        hook["timeout"] = serde_json::json!(PROMPT_WAIT_SECS);
    }
    let hooks = serde_json::json!([hook]);

    if MATCHER_EVENTS.contains(&event) {
        serde_json::json!({ "matcher": "*", "hooks": hooks })
    } else {
        serde_json::json!({ "hooks": hooks })
    }
}

/// Whether an entry is one ket added.
fn is_kets(entry: &serde_json::Value) -> bool {
    entry.get(OWNER_MARK).is_some()
        || entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|command| command.contains("ket-agent-hook"))
                })
            })
}

// ---- codex ------------------------------------------------------------------

/// Codex: hook commands in `~/.codex/hooks.json`.
///
/// A separate file from Claude's `settings.json`, holding only hooks, but the
/// same document shape inside it — `hooks` → event name → a list of groups,
/// each with its own `hooks` array of `{type, command}`. So this differs from
/// [`ClaudeHooks`] in three things and no more: which file, which event names
/// (see [`HookEvent::CODEX_EVENTS`]), and no `matcher` key.
///
/// **No matcher on any event.** Claude Code validates matchers and rejects an
/// entry carrying one on an event that does not take it — the reason
/// [`MATCHER_EVENTS`] exists. Codex's documented shape allows a matcher, but
/// the working install already on this machine, written by another tool, omits
/// it on all eight events it registers, `PreToolUse` included. Following what
/// is demonstrably accepted beats following what is documented as allowed, and
/// a matcher ket does not need is a way for an entry to be thrown out silently.
///
/// **This file belongs to whoever else is in it.** On the machine this was
/// written against, another tool already had all eight of its own hooks
/// installed here. Everything in [`ClaudeHooks`]'s merge — keep what is there,
/// replace only ket's own entries, remove ket's entries from events ket no
/// longer asks for, drop an event only once it is empty — matters more here,
/// not less.
pub struct CodexHooks {
    /// The hooks file, overridable so this is exercisable without a home.
    settings: PathBuf,
}

impl Default for CodexHooks {
    fn default() -> Self {
        Self {
            settings: home().join(".codex").join("hooks.json"),
        }
    }
}

impl CodexHooks {
    /// Hooks written into a specific file.
    pub fn at(settings: PathBuf) -> Self {
        Self { settings }
    }
}

impl HookInstaller for CodexHooks {
    fn agent(&self) -> &str {
        "codex"
    }

    fn is_installed(&self) -> Result<bool> {
        let document = read_settings(&self.settings)?;
        let Some(hooks) = document.get("hooks").and_then(|hooks| hooks.as_object()) else {
            return Ok(false);
        };

        let ours: std::collections::BTreeSet<&str> = hooks
            .iter()
            .filter(|(_, entries)| {
                entries
                    .as_array()
                    .is_some_and(|list| list.iter().any(is_kets))
            })
            .map(|(event, _)| event.as_str())
            .collect();

        Ok(ours == HookEvent::CODEX_EVENTS.into_iter().collect())
    }

    fn install(&self, script: &Path) -> Result<()> {
        let mut document = read_settings(&self.settings)?;

        let root = document
            .as_object_mut()
            .ok_or_else(|| KetError::Config("hooks.json is not an object".to_owned()))?;
        let hooks = root
            .entry("hooks")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| KetError::Config("hooks.json: hooks is not an object".to_owned()))?;

        for event in HookEvent::CODEX_EVENTS {
            let entries = hooks
                .entry(event)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| {
                    KetError::Config(format!("hooks.json: hooks.{event} is not an array"))
                })?;

            entries.retain(|entry| !is_kets(entry));
            entries.push(serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": format!("/bin/sh {} 2>/dev/null || printf '{{}}\\n'", script.display()),
                }]
            }));
        }

        for (event, entries) in hooks.iter_mut() {
            if HookEvent::CODEX_EVENTS.contains(&event.as_str()) {
                continue;
            }
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        write_settings(&self.settings, &document)
    }

    fn remove(&self) -> Result<()> {
        let mut document = read_settings(&self.settings)?;
        let Some(hooks) = document
            .as_object_mut()
            .and_then(|root| root.get_mut("hooks"))
            .and_then(|hooks| hooks.as_object_mut())
        else {
            return Ok(());
        };

        for entries in hooks.values_mut() {
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        write_settings(&self.settings, &document)
    }
}

// ---- opencode ---------------------------------------------------------------

/// OpenCode: a plugin of ket's own in `~/.config/opencode/plugin/`.
///
/// OpenCode has no hook commands; it has plugins, loaded from that directory
/// with no entry in its config, so ket's is a file of its own and the
/// person's `opencode.json` is never touched. It posts the same envelope as
/// [`SCRIPT`], with a payload in Claude Code's shape — `hook_event_name`,
/// `session_id`, `tool_name`, `tool_input` — so everything that reads a
/// report reads OpenCode's unchanged. Checked against OpenCode 1.18.33's
/// source (`packages/opencode/src/plugin`, `permission`, `session`):
///
/// - `chat.message` is `UserPromptSubmit`; `tool.execute.before` / `after`
///   are `PreToolUse` / `PostToolUse`, with OpenCode's tool names spelled as
///   Claude's so rows read the same.
/// - The `permission.asked` event is `PermissionRequest`. There is no hook
///   for the answer, so its `permission.replied` event is reported as the
///   tool starting — otherwise a prompt answered on the desktop would stay
///   open on the phone until the tool finished. A rejection ends the turn,
///   and the `session.status` idle that follows is `Stop`.
/// - Its `question` tool asking is a `Notification` of a person being needed,
///   with no question a phone could answer, and the reply is the tool going
///   on. A rejected question ends the turn, as a rejected permission does.
/// - `Stop` carries the text the session last said, as Claude's
///   `last_assistant_message`, so a finished row has a closing line.
/// - A subagent's session (one with a parent) is left out altogether: its
///   prompt's Reject opens a feedback box rather than rejecting, so a phone
///   must not answer it, and its tools are not the pane's.
///
/// Inert outside a terminal ket opened: without ket's launch token in the
/// environment it reports nothing.
pub struct OpenCodeHooks {
    /// The plugin file, overridable so this is exercisable without a home.
    plugin: PathBuf,
}

impl Default for OpenCodeHooks {
    fn default() -> Self {
        // OpenCode reads `$XDG_CONFIG_HOME/opencode`, else `~/.config/opencode`.
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home().join(".config"), PathBuf::from);
        Self {
            plugin: config.join("opencode").join("plugin").join("ket.js"),
        }
    }
}

impl OpenCodeHooks {
    /// The plugin written to a specific file.
    pub fn at(plugin: PathBuf) -> Self {
        Self { plugin }
    }
}

impl HookInstaller for OpenCodeHooks {
    fn agent(&self) -> &str {
        "opencode"
    }

    fn is_installed(&self) -> Result<bool> {
        match std::fs::read_to_string(&self.plugin) {
            Ok(text) => Ok(text == OPENCODE_PLUGIN),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KetError::io(&self.plugin, e)),
        }
    }

    /// Writes the plugin. The hook script is not used: the plugin posts
    /// itself.
    fn install(&self, _script: &Path) -> Result<()> {
        if let Some(dir) = self.plugin.parent() {
            std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
        }
        std::fs::write(&self.plugin, OPENCODE_PLUGIN).map_err(|e| KetError::io(&self.plugin, e))
    }

    /// Deletes the plugin — only if it is ket's.
    fn remove(&self) -> Result<()> {
        match std::fs::read_to_string(&self.plugin) {
            Ok(text) if text.starts_with(OPENCODE_PLUGIN_MARK) => {
                std::fs::remove_file(&self.plugin).map_err(|e| KetError::io(&self.plugin, e))
            }
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KetError::io(&self.plugin, e)),
        }
    }
}

/// The plugin's first line, which says a file is ket's to remove.
const OPENCODE_PLUGIN_MARK: &str = "// ket-opencode-plugin";

/// OpenCode's plugin — see [`OpenCodeHooks`]. The v1 module shape, a default
/// export of `{ id, server }`; OpenCode rejects a module with any other
/// export. Every post is queued, so reports keep their order, and none is
/// awaited by a hook OpenCode waits on.
const OPENCODE_PLUGIN: &str = r#"// ket-opencode-plugin — installed by ket. Reports an OpenCode session's
// lifecycle back to ket, so its sidebar and a paired phone see what it is
// doing. Inert outside a terminal ket opened. Deleting this file uninstalls it.
import { appendFileSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

export default {
  id: "ket",
  server: async ({ directory }) => {
    const env = process.env;
    const token = env.KET_LAUNCH_TOKEN;
    if (!token) return {};
    const home = env.HOME || "";
    const hooks = join(home, ".ket", "agent-hooks");

    // Claude Code's names, so a row reads the same whichever agent it is.
    const TOOLS = {
      bash: "Bash", edit: "Edit", write: "Write", read: "Read", glob: "Glob",
      grep: "Grep", list: "LS", webfetch: "WebFetch", websearch: "WebSearch",
      task: "Task", todowrite: "TodoWrite",
    };
    // Only the fields a report reads: a file's contents would be the body.
    const pick = (args) => {
      const a = args || {};
      const out = {};
      for (const key of ["command", "description", "pattern", "path", "url", "query"]) {
        if (typeof a[key] === "string") out[key] = a[key];
      }
      if (typeof a.filePath === "string") out.file_path = a.filePath;
      return out;
    };

    const envelope = (to, payload) => JSON.stringify({
      agent: env.KET_AGENT_NAME || "opencode",
      worktree: env.KET_WORKTREE_ID || "",
      pane: env.KET_PANE_KEY || "",
      token: to,
      version: env.KET_HOOK_VERSION || "1",
      payload,
    });
    const hostEndpoint = () => {
      try {
        const [port, hostToken] = readFileSync(join(hooks, "host-endpoint"), "utf8").split("\n");
        return port && hostToken ? { port, token: hostToken } : null;
      } catch {
        return null;
      }
    };
    const post = async (target, payload) => {
      if (!target || !target.port || !target.token) return false;
      try {
        const response = await fetch(`http://127.0.0.1:${target.port}/hook`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: envelope(target.token, payload),
          signal: AbortSignal.timeout(1000),
        });
        return response.ok;
      } catch {
        return false;
      }
    };
    // In order, the host first, then the window; kept for later if neither.
    // A pane-bound launch's own token is already correct, so it goes first —
    // the discovery file never carries a pane-specific token and would only
    // cost a round trip that never validates for one.
    let queue = Promise.resolve();
    const report = (event, fields) => {
      const payload = { hook_event_name: event, cwd: directory, ...fields };
      queue = queue.then(async () => {
        const inherited = { port: env.KET_HOOK_PORT, token };
        const host = hostEndpoint();
        const [first, second] = env.KET_PANE_KEY ? [inherited, host] : [host, inherited];
        if (await post(first, payload)) return;
        if (await post(second, payload)) return;
        try {
          mkdirSync(hooks, { recursive: true, mode: 0o700 });
          appendFileSync(join(hooks, "spool.jsonl"), envelope(host ? host.token : token, payload) + "\n", { mode: 0o600 });
        } catch {}
      });
    };

    const children = new Set();
    const own = (session) => session && !children.has(session);
    const asked = new Map();
    // What each session last said, for its Stop to carry: the assistant's
    // messages by id, then their text parts as they are updated.
    const said = new Set();
    const last = new Map();
    const stop = (session) => {
      report("Stop", { session_id: session, last_assistant_message: last.get(session) });
      last.delete(session);
    };
    const question = (p) => {
      const m = p.metadata || {};
      const tool_input = {};
      if (typeof m.command === "string") tool_input.command = m.command;
      if (typeof m.filepath === "string") tool_input.file_path = m.filepath;
      if (typeof m.url === "string") tool_input.url = m.url;
      const pattern = (p.patterns || [])[0];
      if (!Object.keys(tool_input).length && pattern) tool_input.command = pattern;
      return { tool_name: TOOLS[p.permission] || p.permission, tool_input };
    };

    return {
      event: async ({ event }) => {
        const p = event.properties || {};
        switch (event.type) {
          case "session.created":
            if (p.info && p.info.parentID) children.add(p.info.id || p.sessionID);
            else report("SessionStart", { session_id: p.sessionID, source: "startup" });
            break;
          case "session.status":
            if (p.status && p.status.type === "idle" && own(p.sessionID)) stop(p.sessionID);
            break;
          case "session.error":
            if (own(p.sessionID)) stop(p.sessionID);
            break;
          case "message.updated":
            if (p.info && p.info.role === "assistant" && own(p.sessionID)) {
              if (said.size > 1000) said.clear();
              said.add(p.info.id);
            }
            break;
          case "message.part.updated": {
            const part = p.part || {};
            if (part.type === "text" && !part.synthetic && said.has(part.messageID) && part.text) {
              last.set(p.sessionID, part.text);
            }
            break;
          }
          // The question tool waits on a person without a permission prompt:
          // a wait, not something a phone can allow.
          case "question.asked":
            if (own(p.sessionID)) {
              report("Notification", { session_id: p.sessionID, notification_type: "agent_needs_input" });
            }
            break;
          case "question.replied":
            if (own(p.sessionID)) report("PreToolUse", { session_id: p.sessionID, tool_name: "AskUserQuestion" });
            break;
          case "permission.asked":
            if (own(p.sessionID)) {
              const q = question(p);
              asked.set(p.id, q);
              report("PermissionRequest", { session_id: p.sessionID, ...q });
            }
            break;
          case "permission.replied": {
            const q = asked.get(p.requestID);
            asked.delete(p.requestID);
            if (q && p.reply !== "reject") report("PreToolUse", { session_id: p.sessionID, ...q });
            break;
          }
        }
      },
      "chat.message": async (input) => {
        if (own(input.sessionID)) report("UserPromptSubmit", { session_id: input.sessionID });
      },
      "tool.execute.before": async (input, output) => {
        if (!own(input.sessionID)) return;
        report("PreToolUse", {
          session_id: input.sessionID,
          tool_use_id: input.callID,
          tool_name: TOOLS[input.tool] || input.tool,
          tool_input: pick(output && output.args),
        });
      },
      "tool.execute.after": async (input) => {
        if (!own(input.sessionID)) return;
        report("PostToolUse", {
          session_id: input.sessionID,
          tool_use_id: input.callID,
          tool_name: TOOLS[input.tool] || input.tool,
          tool_input: pick(input.args),
        });
      },
    };
  },
};
"#;

// ---- grok -------------------------------------------------------------------

/// Grok: a hooks file of ket's own, `~/.grok/hooks/ket.json`.
///
/// Grok merges every `*.json` in its hooks directory, so, as with
/// [`OpenCodeHooks`], ket owns a whole file and never edits one of the
/// person's. The document is Claude Code's shape. Checked against Grok 1.0.44
/// on 2026-09-29:
///
/// - Grok **expands `$VAR` in a hook's command itself**, and refuses to run a
///   hook whose variables are unset. The command ket writes has no `$` in it.
/// - The payload carries Claude's snake_case keys — `hook_event_name` (with
///   Claude's spelling of the event), `session_id`, `tool_name`,
///   `tool_input`, `cwd` — beside Grok's own camelCase ones, but tool names
///   and a few fields are Grok's. [`grok_payload`] translates them.
/// - Grok **also runs Claude Code's hooks** from `~/.claude/settings.json`
///   unless told otherwise, so ket's Claude entry fires inside Grok too. The
///   script tells the two apart by the `grok` argument this entry passes; see
///   [`SCRIPT`].
/// - A `SessionStart` hook's `additionalContext` does not reach the model, so
///   Grok is not handed the Economy note.
///
/// Inert on a machine without Grok: nothing is written unless its home
/// directory already exists.
pub struct GrokHooks {
    /// Grok's home, overridable so this is exercisable without one.
    home: PathBuf,
}

impl Default for GrokHooks {
    fn default() -> Self {
        let home = std::env::var_os("GROK_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home().join(".grok"), PathBuf::from);
        Self { home }
    }
}

impl GrokHooks {
    /// Hooks for a Grok whose home is `home`.
    pub fn at(home: PathBuf) -> Self {
        Self { home }
    }

    fn file(&self) -> PathBuf {
        self.home.join("hooks").join("ket.json")
    }
}

/// ket's hooks file for Grok, calling `script`.
fn grok_document(script: &Path) -> String {
    let command = format!(
        "/bin/sh {} grok 2>/dev/null || printf '{{}}\\n'",
        script.display()
    );
    let hooks: serde_json::Map<String, serde_json::Value> = HookEvent::GROK_EVENTS
        .into_iter()
        .map(|event| {
            let entry = serde_json::json!([{
                "hooks": [{ "type": "command", "command": command }],
            }]);
            (event.to_owned(), entry)
        })
        .collect();
    let document = serde_json::json!({ "hooks": hooks });
    serde_json::to_string_pretty(&document).unwrap_or_default() + "\n"
}

impl HookInstaller for GrokHooks {
    fn agent(&self) -> &str {
        "grok"
    }

    /// Whether the file is exactly what this version writes — or Grok is not
    /// here at all, in which case there is nothing to install.
    fn is_installed(&self) -> Result<bool> {
        if !self.home.is_dir() {
            return Ok(true);
        }
        let file = self.file();
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(text == grok_document(&script_path())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KetError::io(&file, e)),
        }
    }

    fn install(&self, script: &Path) -> Result<()> {
        if !self.home.is_dir() {
            return Ok(());
        }
        let file = self.file();
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
        }
        crate::config::write_atomically(&file, &grok_document(script), false)
    }

    /// Deletes the file — only if it is ket's.
    fn remove(&self) -> Result<()> {
        let file = self.file();
        match std::fs::read_to_string(&file) {
            Ok(text) if text.contains(OWNER_MARK) => {
                std::fs::remove_file(&file).map_err(|e| KetError::io(&file, e))
            }
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KetError::io(&file, e)),
        }
    }
}

/// Copies `from` to `to` in `object` unless `to` is already there.
fn alias(object: &mut serde_json::Map<String, serde_json::Value>, from: &str, to: &str) {
    if !object.contains_key(to)
        && let Some(value) = object.get(from).cloned()
    {
        object.insert(to.to_owned(), value);
    }
}

/// A Grok payload in the shape everything downstream reads, or `None` when
/// it should be dropped.
///
/// Grok sends Claude's snake_case keys for the common fields but its own tool
/// names and a few camelCase-only fields. Translating once here means
/// [`tool_phrase`], [`permission_question`], [`closing_line`] and the rest
/// read a Grok report exactly as they read a Claude one.
///
/// Dropped: anything that fired inside a subagent (it carries
/// `subagentType`). A subagent's tools are not the pane's.
fn grok_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("subagentType").is_some() {
        return None;
    }
    let mut payload = payload.clone();
    let object = payload.as_object_mut()?;

    alias(object, "lastAssistantMessage", "last_assistant_message");
    alias(object, "notificationType", "notification_type");

    if let Some(tasks) = object.get("backgroundTasks").and_then(|t| t.as_array()) {
        let tasks: Vec<serde_json::Value> = tasks
            .iter()
            .map(|task| {
                let mut task = task.clone();
                if let Some(task) = task.as_object_mut() {
                    alias(task, "agentType", "agent_type");
                }
                task
            })
            .collect();
        object.insert("background_tasks".to_owned(), tasks.into());
    }

    let claude = object
        .get("tool_name")
        .and_then(|t| t.as_str())
        .and_then(grok_tool);
    if let Some(claude) = claude {
        object.insert("tool_name".to_owned(), claude.into());
    }
    if let Some(input) = object.get_mut("tool_input").and_then(|i| i.as_object_mut()) {
        grok_input(input);
    }

    Some(payload)
}

/// A Grok tool by Claude's name, or `None` for one with no Claude twin.
/// Shared with [`crate::conversation`], which reads Grok's transcript.
pub(crate) fn grok_tool(name: &str) -> Option<&'static str> {
    Some(match name {
        "run_terminal_command" => "Bash",
        "read_file" => "Read",
        "write" => "Write",
        "search_replace" => "Edit",
        "grep" => "Grep",
        "list_dir" => "Glob",
        "spawn_subagent" => "Task",
        "web_fetch" => "WebFetch",
        "web_search" => "WebSearch",
        _ => return None,
    })
}

/// A Grok tool's input with Claude's keys added beside Grok's own.
pub(crate) fn grok_input(input: &mut serde_json::Map<String, serde_json::Value>) {
    alias(input, "target_file", "file_path");
    alias(input, "target_directory", "pattern");
}

// ---- the status line --------------------------------------------------------

/// Claude Code's status line, which is where its rate limits are published.
///
/// Quota is the one thing Claude Code will not hand over cheaply. There is no
/// file to read and no command to ask, so [`crate::rate_limits`] gets it by
/// starting a *real, billed* session in a scratch directory and reading one
/// frame — every five minutes, for as long as ket is open.
///
/// The status line carries the same numbers for nothing. Claude renders it by
/// running a command of the user's choosing and handing it a JSON payload on
/// stdin, and that payload has a `rate_limits` block in it. A session that is
/// already on screen pays for those numbers whether or not anybody reads them.
///
/// **This one is not a merge, and that is the whole difficulty.** `hooks` is a
/// map that ket can add its own entry to and leave everyone else's alone.
/// `statusLine` is a *single command*, so installing ket's replaces whatever
/// was there — and on this machine what is there belongs to another
/// application. So ket does not replace it: it captures it into
/// [`statusline_chain_path`] and its own script runs it, feeds it the same
/// payload, and passes its output through as the status line. Removing ket's
/// puts the captured command back exactly.
///
/// Two things this deliberately will not do:
///
/// - **Capture its own script as the chained command.** A ket that installed
///   over a ket would otherwise build a script that runs itself, forever.
/// - **Speak for a session it did not launch.** The payload is only posted
///   when [`PORT_ENV`] and [`TOKEN_ENV`] are in the environment, which is true
///   for panes ket opened and nothing else. Quota is per account, so one
///   reporting session answers for all of them.
pub struct ClaudeStatusLine {
    /// The settings file, overridable so this is exercisable without a home.
    settings: PathBuf,
    /// Where the command ket displaced is kept, so it can be given back.
    chain: PathBuf,
}

impl Default for ClaudeStatusLine {
    fn default() -> Self {
        Self {
            settings: crate::sessions::claude_config_dir().join("settings.json"),
            chain: statusline_chain_path(),
        }
    }
}

impl ClaudeStatusLine {
    /// A status line written into specific files.
    pub fn at(settings: PathBuf, chain: PathBuf) -> Self {
        Self { settings, chain }
    }

    /// The command currently configured, if there is one.
    fn configured(&self) -> Result<Option<String>> {
        let document = read_settings(&self.settings)?;
        Ok(document
            .get("statusLine")
            .and_then(|line| line.get("command"))
            .and_then(|c| c.as_str())
            .map(str::to_owned))
    }

    /// Whether ket's own script is what Claude is running.
    pub fn is_installed(&self) -> Result<bool> {
        Ok(self
            .configured()?
            .is_some_and(|command| command.contains(STATUSLINE_MARK)))
    }

    /// Points Claude's status line at `script`, keeping whatever it displaced.
    pub fn install(&self, script: &Path) -> Result<()> {
        let existing = self.configured()?;

        // Written before the settings change, not after: if this fails, the
        // command it would have recorded is still the one Claude runs.
        match &existing {
            // Already ket's — the chain file behind it is the real previous
            // command and must not be overwritten with a pointer to ket.
            Some(command) if command.contains(STATUSLINE_MARK) => {}
            Some(command) => {
                if let Some(parent) = self.chain.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
                }
                std::fs::write(&self.chain, command).map_err(|e| KetError::io(&self.chain, e))?;
            }
            // Nothing displaced, so nothing to give back. An old chain file
            // from a previous install would be a command Claude no longer has.
            None => {
                let _ = std::fs::remove_file(&self.chain);
            }
        }

        let mut document = read_settings(&self.settings)?;
        let root = document
            .as_object_mut()
            .ok_or_else(|| KetError::Config("settings.json is not an object".to_owned()))?;
        root.insert(
            "statusLine".to_owned(),
            serde_json::json!({
                "type": "command",
                "command": format!("/bin/sh {} 2>/dev/null", script.display()),
            }),
        );

        write_settings(&self.settings, &document)
    }

    /// Gives the displaced command back, or takes the status line away
    /// entirely when ket displaced nothing.
    pub fn remove(&self) -> Result<()> {
        if !self.is_installed()? {
            return Ok(());
        }

        let mut document = read_settings(&self.settings)?;
        let Some(root) = document.as_object_mut() else {
            return Ok(());
        };

        match std::fs::read_to_string(&self.chain) {
            Ok(command) if !command.trim().is_empty() => {
                root.insert(
                    "statusLine".to_owned(),
                    serde_json::json!({ "type": "command", "command": command }),
                );
            }
            _ => {
                root.remove("statusLine");
            }
        }

        write_settings(&self.settings, &document)?;
        let _ = std::fs::remove_file(&self.chain);
        Ok(())
    }
}

/// What taking ket out of one agent did — see [`remove_everywhere`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// Ket's entries were there, and are gone.
    Removed,
    /// There were none to take out.
    NotThere,
    /// The agent's settings could not be read or written, and say why.
    Failed(String),
}

/// One place ket was taken out of, and how that went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// The agent, as [`HookInstaller::agent`] names it, or `claude` for its
    /// status line.
    pub agent: String,
    /// Which of its settings: a Claude config directory other than the usual
    /// one, or the status line. `None` for the agent's only one.
    pub detail: Option<String>,
    /// What happened there.
    pub outcome: Removal,
}

/// Takes ket out of every agent it puts itself into, for uninstalling it:
/// its hooks from each Claude config directory, Codex, OpenCode and Grok,
/// and its script from in front of Claude's status line, which gets back
/// whatever it displaced. Only ket's own entries go — everything else in
/// those files stays as it was.
///
/// A ket still running puts them back the next time it starts; this is for
/// the last thing done before it is deleted.
pub fn remove_everywhere() -> Vec<Removed> {
    let usual = crate::sessions::claude_config_dir();
    let mut installers: Vec<(Box<dyn HookInstaller>, Option<String>)> = Vec::new();
    for dir in crate::sessions::claude_config_dirs() {
        let detail = (dir != usual).then(|| dir.display().to_string());
        installers.push((Box::new(ClaudeHooks::at(dir.join("settings.json"))), detail));
    }
    installers.push((Box::new(CodexHooks::default()), None));
    installers.push((Box::new(OpenCodeHooks::default()), None));
    installers.push((Box::new(GrokHooks::default()), None));

    let mut removed: Vec<Removed> = installers
        .iter()
        .map(|(installer, detail)| Removed {
            agent: installer.agent().to_owned(),
            detail: detail.clone(),
            outcome: match installer.is_installed() {
                Ok(false) => Removal::NotThere,
                Ok(true) => match installer.remove() {
                    Ok(()) => Removal::Removed,
                    Err(why) => Removal::Failed(why.to_string()),
                },
                Err(why) => Removal::Failed(why.to_string()),
            },
        })
        .collect();

    let status_line = ClaudeStatusLine::default();
    removed.push(Removed {
        agent: "claude".to_owned(),
        detail: Some("status line".to_owned()),
        outcome: match status_line.is_installed() {
            Ok(false) => Removal::NotThere,
            Ok(true) => match status_line.remove() {
                Ok(()) => Removal::Removed,
                Err(why) => Removal::Failed(why.to_string()),
            },
            Err(why) => Removal::Failed(why.to_string()),
        },
    });
    removed
}

/// How ket recognises its own status line command.
pub const STATUSLINE_MARK: &str = "ket-statusline";

/// Where ket keeps the status line script.
pub fn statusline_script_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("ket-statusline.sh")
}

/// Where the command ket displaced is kept.
pub fn statusline_chain_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("statusline-chain")
}

/// Writes the status line script, creating its directory.
pub fn install_statusline_script() -> Result<PathBuf> {
    let path = statusline_script_path();
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("status line script has no directory".to_owned()))?;
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    std::fs::write(&path, STATUSLINE_SCRIPT).map_err(|e| KetError::io(&path, e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)
            .map_err(|e| KetError::io(&path, e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).map_err(|e| KetError::io(&path, e))?;
    }

    Ok(path)
}

/// The script Claude runs to draw its status line.
///
/// Its output *is* the status line, which makes the failure mode here worse
/// than the hook script's: a hook that misbehaves is a report ket does not
/// get, and this is a line the person is looking at. So the chained command's
/// stdout is passed through untouched and everything ket does happens off to
/// the side of it, after the payload has been read and before the chain runs.
///
/// There is no spool. A status line is drawn again a second later; a dropped
/// one costs nothing and a queue of stale quota readings is worse than none.
const STATUSLINE_SCRIPT: &str = r#"#!/bin/sh
# ket-statusline — installed by ket. Reads Claude's status line payload for
# the rate limits in it, then runs whatever command ket displaced and prints
# its output, which is what you see.
#
# Removing ket's `statusLine` from the agent's settings uninstalls this; the
# file itself is inert without it.

payload=$(cat 2>/dev/null || printf '')
if [ -z "$payload" ]; then exit 0; fi

escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# Only for a session ket launched: without the port and the token there is
# nowhere to send this and nothing that would believe it.
if [ -n "${KET_HOOK_PORT:-}" ] && [ -n "${KET_LAUNCH_TOKEN:-}" ] && command -v curl >/dev/null 2>&1; then
  envelope=$(printf '{"agent":"%s","worktree":"%s","pane":"%s","token":"%s","version":"%s","payload":%s}' \
    "$(escape "${KET_AGENT_NAME:-claude}")" \
    "$(escape "${KET_WORKTREE_ID:-}")" \
    "$(escape "${KET_PANE_KEY:-}")" \
    "$(escape "${KET_LAUNCH_TOKEN:-}")" \
    "$(escape "${KET_HOOK_VERSION:-1}")" \
    "$payload")
  printf '%s' "$envelope" | curl -sS -o /dev/null \
    --connect-timeout 1 --max-time 1 \
    -H 'Content-Type: application/json' \
    --data-binary @- \
    "http://127.0.0.1:${KET_HOOK_PORT}/statusline" 2>/dev/null || :
fi

# Whatever was drawing this line before ket arrived still draws it.
chain="${HOME:-}/.ket/agent-hooks/statusline-chain"
if [ -n "${HOME:-}" ] && [ -s "$chain" ]; then
  command=$(cat "$chain" 2>/dev/null || printf '')
  if [ -n "$command" ]; then
    printf '%s' "$payload" | /bin/sh -c "$command" 2>/dev/null || :
  fi
fi

exit 0
"#;

/// The user's home directory, or the current directory if there is none.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

// ---- the script -------------------------------------------------------------

/// Where ket keeps the script agents call.
pub fn script_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("ket-agent-hook.sh")
}

/// The spool a report falls back to when ket is not listening.
pub fn spool_path() -> PathBuf {
    home().join(".ket").join("agent-hooks").join("spool.jsonl")
}

/// Where the terminal host publishes its current hook port and token.
pub fn host_endpoint_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("host-endpoint")
}

/// Publishes the terminal host's current hook endpoint for resumed sessions.
///
/// Codex restores environment variables from the original session when it
/// runs hooks after `codex resume`. The script therefore cannot rely on the
/// port and token it inherited, even though the resumed process itself was
/// launched with the new values. This owner-only file is the live authority;
/// the inherited values remain the fallback for local, host-disabled runs.
pub fn publish_host_endpoint(port: u16, token: &str) -> Result<()> {
    let path = host_endpoint_path();
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("hook endpoint has no directory".to_owned()))?;
    private_dir(dir)?;
    let temporary = dir.join(format!(".host-endpoint-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    // Owner-only from the moment it exists — written and then narrowed, it
    // could be read in between.
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|e| KetError::io(&temporary, e))?;
        file.write_all(format!("{port}\n{token}\n").as_bytes())
            .map_err(|e| KetError::io(&temporary, e))?;
    }
    std::fs::rename(&temporary, &path).map_err(|e| KetError::io(&path, e))
}

/// A token for a hook listener: 32 random bytes, in hex. What a report must
/// carry to be believed, so not something that can be worked out from a
/// process id and the time.
pub fn new_token() -> String {
    ket_remote::secret()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Makes `dir` if it is missing, and keeps it the owner's alone: it holds
/// the host's hook token and the spool of reports.
fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| KetError::io(dir, e))?;
    }
    Ok(())
}

/// Writes the hook script, creating its directory.
///
/// Rewritten on every install rather than only when absent: a ket that has been
/// upgraded should not leave last version's script behind, and the file is the
/// contract between the two.
pub fn install_script() -> Result<PathBuf> {
    let path = script_path();
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("hook script has no directory".to_owned()))?;
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    std::fs::write(&path, SCRIPT).map_err(|e| KetError::io(&path, e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)
            .map_err(|e| KetError::io(&path, e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).map_err(|e| KetError::io(&path, e))?;
    }

    Ok(path)
}

/// The script every agent's hook runs.
///
/// Three properties it must have, and all three are why this is not one line
/// of curl:
///
/// 1. **It can never break the agent.** Whatever happens — no ket, no curl, no
///    network, a full disk — it prints `{}` and exits 0. A hook that fails is a
///    hook that stops a person's session.
/// 2. **It never blocks.** Connect and total timeouts are one second; an agent
///    must not wait on ket to think.
/// 3. **It does not lose reports.** A failed post is appended to the spool
///    instead, for ket to pick up when it next starts.
///
/// On `SessionStart` it does one more thing: it hands Claude Code back
/// whatever [`crate::worktree::TOKEN_REDUCTION_NOTE_ENV`] holds as
/// `hookSpecificOutput.additionalContext`, telling it how tersely to work for
/// the rest of the session. ket set that variable on the launched process from
/// the worktree's own level — the script deliberately knows nothing about
/// which level means what, so the table in [`crate::worktree`] stays the only
/// copy. That field is the one place a `SessionStart` hook can talk back;
/// every other event still gets the same inert `{}`.
const SCRIPT: &str = r#"#!/bin/sh
# ket-agent-hook — installed by ket. Reports an agent's lifecycle back to it,
# and on session start, tells Claude how tersely this worktree asked it to work.
#
# Removing ket's entries from the agent's settings is what uninstalls this;
# the file itself is inert without them.

payload=$(cat 2>/dev/null || printf '')

# Nothing to say, and nothing that could go wrong from here on.
if [ -z "$payload" ]; then printf '{}\n'; exit 0; fi

# Grok runs Claude Code's hooks as well as its own, so ket's Claude entry
# fires inside Grok too. Only the entry ket wrote for Grok, which passes
# "grok", reports from there; the other would claim to be Claude.
if [ -n "${GROK_HOOK_EVENT:-}" ] && [ "${1:-}" != "grok" ]; then printf '{}\n'; exit 0; fi
agent="${1:-${KET_AGENT_NAME:-claude}}"

escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

envelope() {
  printf '{"agent":"%s","worktree":"%s","pane":"%s","token":"%s","version":"%s","payload":%s}' \
    "$(escape "$agent")" \
    "$(escape "${KET_WORKTREE_ID:-}")" \
    "$(escape "${KET_PANE_KEY:-}")" \
    "$(escape "$1")" \
    "$(escape "${KET_HOOK_VERSION:-1}")" \
    "$payload"
}

post() {
  [ -n "$1" ] && [ -n "$2" ] && command -v curl >/dev/null 2>&1 || return 1
  envelope "$2" | curl -fsS -o /dev/null \
      --connect-timeout 1 --max-time 1 \
      -H 'Content-Type: application/json' \
      --data-binary @- \
      "http://127.0.0.1:$1/hook" 2>/dev/null
}

delivered=0
endpoint="${HOME:-}/.ket/agent-hooks/host-endpoint"
host_port=
host_token=
if [ -n "${KET_LAUNCH_TOKEN:-}" ] && [ -n "${HOME:-}" ] && [ -r "$endpoint" ]; then
  host_port=$(sed -n '1p' "$endpoint" 2>/dev/null)
  host_token=$(sed -n '2p' "$endpoint" 2>/dev/null)
fi
# A pane-bound launch's own token is already the right one for the life of
# this process; trying the host-endpoint discovery file first would spend a
# round trip on every single report, since that file never carries a
# pane-specific token — only a launch with no pane key depends on it.
#
# `wait_port` and `wait_token` are the host's listener, when the report
# reached it, and what it took: where a question or a plan waits below. The
# host hands each terminal it runs its own port and a token bound to the
# pane, so the launch's port matching the discovery file's is the host —
# and the pane's token is the one it accepts. A window's listener is never
# waited on: phones answer through the host.
wait_port=
wait_token=
if [ -n "${KET_PANE_KEY:-}" ]; then
  if post "${KET_HOOK_PORT:-}" "${KET_LAUNCH_TOKEN:-}"; then
    delivered=1
    if [ -n "$host_port" ] && [ "${KET_HOOK_PORT:-}" = "$host_port" ]; then
      wait_port=$host_port
      wait_token=${KET_LAUNCH_TOKEN:-}
    fi
  elif post "$host_port" "$host_token"; then
    delivered=1
    wait_port=$host_port
    wait_token=$host_token
  fi
elif post "$host_port" "$host_token"; then
  delivered=1
  wait_port=$host_port
  wait_token=$host_token
elif post "${KET_HOOK_PORT:-}" "${KET_LAUNCH_TOKEN:-}"; then
  delivered=1
fi

# A question or a plan, reported to the host: held open there until a phone
# answers it, or until it is answered in the terminal and ket lets go. What
# comes back is Claude's decision, in Claude's own shape. Nothing back — an
# older host, a wait that ran out — leaves the prompt to the terminal, as if
# this had never asked. The wait is a little shorter than the entry's timeout.
if [ -n "$wait_port" ] && [ "$agent" = claude ]; then
  case "$payload" in
    *'"hook_event_name":"PermissionRequest"'*)
      case "$payload" in
        *'"tool_name":"AskUserQuestion"'*|*'"tool_name":"ExitPlanMode"'*)
          decision=$(envelope "$wait_token" | curl -sS \
              --connect-timeout 1 --max-time 3570 \
              -H 'Content-Type: application/json' \
              --data-binary @- \
              "http://127.0.0.1:$wait_port/hook/await" 2>/dev/null)
          if [ -n "$decision" ]; then printf '%s\n' "$decision"; exit 0; fi
          ;;
      esac
      ;;
  esac
fi

# Not delivered: keep it for whenever ket comes back.
if [ "$delivered" -eq 0 ] && [ -n "${HOME:-}" ]; then
  envelope=$(envelope "${host_token:-${KET_LAUNCH_TOKEN:-}}")
  spool="${HOME}/.ket/agent-hooks/spool.jsonl"
  # Owner-only: the spool holds the token each report carries.
  ( umask 077
    mkdir -p "${HOME}/.ket/agent-hooks" 2>/dev/null || :
    printf '%s\n' "$envelope" >> "$spool" 2>/dev/null || : )
fi

# Only a SessionStart hook's output can steer the session, and only a session
# ket handed a note has anything to say — the level-to-sentence table lives in
# ket, not here, so there is one copy of it. Every other event, and an empty
# note, gets the same `{}` as before: this is additive, not a replacement for
# the report above.
case "$payload" in
  *'"hook_event_name":"SessionStart"'*)
    # Flattened before it is escaped: `escape` covers the backslashes and
    # quotes that JSON needs, and a newline in the note would otherwise be a
    # literal one inside a string — invalid JSON, and an error the reader sees
    # instead of a session.
    context=$(printf '%s' "${KET_TOKEN_REDUCTION_NOTE:-}" | tr '\n\r\t' '   ')
    if [ -n "$context" ]; then
      # Each agent's own documented way of taking context from a hook, chosen
      # by the name ket launched it under. Claude Code reads a JSON object with
      # `additionalContext` in it; Codex adds a hook's plain stdout to the
      # session as developer context, and would have no idea what Claude's
      # envelope meant. Sending either one the other's shape is at best ignored
      # and at worst an error the reader sees instead of a session, so anything
      # ket does not recognise is told nothing at all.
      case "$agent" in
        claude)
          printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$(escape "$context")"
          ;;
        codex)
          printf '%s\n' "$context"
          ;;
        *)
          printf '{}\n'
          ;;
      esac
    else
      printf '{}\n'
    fi
    ;;
  *)
    printf '{}\n'
    ;;
esac
exit 0
"#;

// ---- the listener -----------------------------------------------------------

/// Receives hook reports on loopback.
///
/// A hand-rolled listener rather than an HTTP crate: this speaks to a script
/// ket itself wrote, the entire protocol is "one POST with a JSON body", and
/// an HTTP stack is a large dependency and a large attack surface for that.
/// It binds to `127.0.0.1:0` — loopback so nothing off this machine can reach
/// it, port zero so ket never fights another process for a fixed number.
pub struct Listener {
    /// The port to hand agents in [`PORT_ENV`].
    port: u16,
    /// Reports as they arrive.
    reports: std::sync::mpsc::Receiver<HookReport>,
    /// Status line payloads as they arrive, kept apart from the reports
    /// because they answer a different question — see [`ClaudeStatusLine`] —
    /// and are read by a different part of the app.
    statuslines: std::sync::mpsc::Receiver<StatusLine>,
    /// Questions and plans whose hooks are waiting to be answered.
    waiters: std::sync::Arc<Waiters>,
    /// Dropped to stop the accept loop.
    _shutdown: Shutdown,
}

/// Closes the listener when the [`Listener`] goes.
struct Shutdown(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Listener {
    /// Starts listening, and tells you which port to hand out.
    ///
    /// `token` is minted by the caller and must appear in a report for it to be
    /// believed: loopback keeps other machines out, not other processes.
    pub fn start(token: String) -> Result<Self> {
        Self::start_with_auth(HookAuth::Legacy(token))
    }

    /// Starts the host listener with credentials bound to individual panes.
    pub fn start_bound(bindings: HookBindings) -> Result<Self> {
        Self::start_with_auth(HookAuth::Bound(bindings))
    }

    fn start_with_auth(auth: HookAuth) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| KetError::io("127.0.0.1:0", e))?;
        let port = listener
            .local_addr()
            .map_err(|e| KetError::io("127.0.0.1:0", e))?
            .port();
        // So the accept loop can notice it has been told to stop.
        listener
            .set_nonblocking(true)
            .map_err(|e| KetError::io("127.0.0.1:0", e))?;

        let (send, reports) = std::sync::mpsc::channel();
        let (send_status, statuslines) = std::sync::mpsc::channel();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stopped.clone();
        let waiters = std::sync::Arc::new(Waiters::default());
        let parked = waiters.clone();

        std::thread::Builder::new()
            .name("ket-agent-hooks".to_owned())
            .spawn(move || {
                let mut grok_tools = GrokTools::new();
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => match read_post(stream, &auth, &mut grok_tools) {
                            Some(Post::Report(report)) => {
                                if send.send(*report).is_err() {
                                    return;
                                }
                            }
                            Some(Post::StatusLine(payload)) => {
                                if send_status.send(payload).is_err() {
                                    return;
                                }
                            }
                            Some(Post::Await(pane, waiter)) => parked.park(pane, waiter),
                            None => {}
                        },
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(40));
                        }
                        // A failed accept is one lost report, not a reason to
                        // stop reporting for the rest of the session.
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
                    }
                }
            })
            .map_err(|e| KetError::io("ket-agent-hooks", e))?;

        Ok(Self {
            port,
            reports,
            statuslines,
            waiters,
            _shutdown: Shutdown(stopped),
        })
    }

    /// The port agents should post to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The questions and plans held open on this listener, for whatever
    /// answers them.
    pub fn waiters(&self) -> std::sync::Arc<Waiters> {
        self.waiters.clone()
    }

    /// Every report that has arrived since the last call.
    ///
    /// Drained rather than blocking: the shell asks on its own tick, and a
    /// hook must never be able to make the window wait.
    pub fn drain(&self) -> Vec<HookReport> {
        self.reports.try_iter().collect()
    }

    /// Every status line payload that has arrived since the last call.
    ///
    /// Payloads still raw, because this module's job is the wire and not the
    /// meaning of what came over it:
    /// [`crate::rate_limits::snapshot_from_statusline`] is what knows a quota
    /// block when it sees one, and [`crate::usage::from_statusline`] the
    /// session's own numbers.
    pub fn drain_statuslines(&self) -> Vec<StatusLine> {
        self.statuslines.try_iter().collect()
    }
}

/// What arrived on one connection.
enum Post {
    /// A lifecycle report from an agent's hook.
    Report(Box<HookReport>),
    /// A status line payload, whole and uninterpreted.
    StatusLine(StatusLine),
    /// A question or a plan's hook, to hold open until it is answered — see
    /// [`Waiters`]. The pane it came from, and the connection.
    Await(String, Waiter),
}

/// Where a script posts a question or a plan it waits on an answer for.
const AWAIT_PATH: &str = "/hook/await";

/// How long a hook just parked is safe from [`Waiters::release_unless`].
///
/// Its report reaches the status store on the host's next tick, a moment
/// after it parks; until then its pane still looks like it is not waiting.
const PARK_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The `PermissionRequest` hooks held open for a question or a plan, by pane.
///
/// Claude Code runs that hook alongside its own dialog and takes whatever
/// decision the hook prints, so a hook that waits is a way to answer the
/// prompt without a single keystroke: the answers themselves, in Claude's
/// words, rather than keys into a menu that changes with the session's mode.
/// A hook that prints nothing leaves the dialog to the terminal.
///
/// Claude does not stop the hook when the prompt is answered at the desktop
/// — checked on 2.1.281 — so letting go is ket's to do, once the pane's
/// status has moved on: see [`Waiters::release_unless`].
#[derive(Default)]
pub struct Waiters {
    parked: std::sync::Mutex<std::collections::HashMap<String, Waiter>>,
}

/// One held hook.
pub struct Waiter {
    stream: std::net::TcpStream,
    /// The prompt, as its report told the status store: matched against the
    /// store's before answering, so an answer cannot reach a later prompt.
    asked: PermissionQuestion,
    /// The tool's input, which the decision hands back with the answers in.
    input: serde_json::Value,
    parked_at: std::time::Instant,
}

impl Waiter {
    /// Hands the hook `body` and closes it. An empty body says nothing, which
    /// the script answers with `{}`.
    fn send(mut self, body: &str) -> bool {
        use std::io::Write;
        let reply = match body.is_empty() {
            true => "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_owned(),
            false => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        };
        let sent = self.stream.write_all(reply.as_bytes()).is_ok();
        let _ = self.stream.flush();
        sent
    }

    /// Whether the script is still there to hear an answer: Claude kills it
    /// when its timeout runs out, and when the session ends.
    fn alive(&self) -> bool {
        if self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut byte = [0_u8; 1];
        let open = match self.stream.peek(&mut byte) {
            Ok(0) => false,
            Ok(_) => true,
            Err(e) => e.kind() == std::io::ErrorKind::WouldBlock,
        };
        let _ = self.stream.set_nonblocking(false);
        open
    }

    /// Whether this is the hook for `question`.
    fn holds(&self, question: &PermissionQuestion) -> bool {
        self.asked.tool == question.tool && self.asked.subject == question.subject
    }
}

impl Waiters {
    /// Holds `waiter` for `pane`, letting go of any older one there: a pane
    /// shows one prompt at a time.
    fn park(&self, pane: String, waiter: Waiter) {
        let old = lock_waiters(&self.parked).insert(pane, waiter);
        if let Some(old) = old {
            old.send("");
        }
    }

    /// Whether no hook is held at all.
    pub fn is_empty(&self) -> bool {
        lock_waiters(&self.parked).is_empty()
    }

    /// Whether `pane`'s hook for `question` is held and still listening —
    /// whether an answer from a phone can reach it.
    pub fn waiting(&self, pane: &str, question: &PermissionQuestion) -> bool {
        lock_waiters(&self.parked)
            .get(pane)
            .is_some_and(|waiter| waiter.holds(question) && waiter.alive())
    }

    /// Answers `pane`'s prompt with `decision`, when its hook for `question`
    /// is held and the decision fits what it asked.
    pub fn resolve(
        &self,
        pane: &str,
        question: &PermissionQuestion,
        decision: &Decision,
    ) -> std::result::Result<(), Unresolved> {
        let mut parked = lock_waiters(&self.parked);
        let waiter = parked
            .get(pane)
            .filter(|waiter| waiter.holds(question) && waiter.alive())
            .ok_or(Unresolved::NotHeld)?;
        let body = decision
            .claude_output(&waiter.input)
            .ok_or(Unresolved::Unfit)?;
        let waiter = parked.remove(pane).ok_or(Unresolved::NotHeld)?;
        match waiter.send(&body) {
            true => Ok(()),
            false => Err(Unresolved::NotHeld),
        }
    }

    /// Lets go of every held hook `keep` does not want, once it has had
    /// [`PARK_GRACE`] to be reported. `keep` is asked with the pane and the
    /// prompt the hook holds.
    pub fn release_unless(&self, keep: impl Fn(&str, &PermissionQuestion) -> bool) {
        let mut parked = lock_waiters(&self.parked);
        if parked.is_empty() {
            return;
        }
        let gone: Vec<String> = parked
            .iter()
            .filter(|(pane, waiter)| {
                waiter.parked_at.elapsed() >= PARK_GRACE
                    && (!waiter.alive() || !keep(pane, &waiter.asked))
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        for pane in gone {
            if let Some(waiter) = parked.remove(&pane) {
                waiter.send("");
            }
        }
    }
}

fn lock_waiters<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why [`Waiters::resolve`] could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    /// No hook is held for that prompt, or it has stopped listening.
    NotHeld,
    /// The decision does not fit what the prompt asked.
    Unfit,
}

/// What a person decided about a [`Prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// An `AskUserQuestion`'s answers, one per question in order: the labels
    /// picked, joined as Claude joins them, or what was typed instead.
    Answers(Vec<String>),
    /// Go ahead with the plan, in the mode the session had before planning.
    Approve,
    /// Not yet: keep planning, with this to go on.
    KeepPlanning(String),
}

impl Decision {
    /// Claude Code's `PermissionRequest` output for this decision about a
    /// prompt raised with `input`, or `None` when it does not fit.
    ///
    /// Allowing either tool takes `updatedInput`: Claude refuses a bare allow
    /// for a tool that needs a person. The input goes back as it came, with
    /// the answers added under each question's own text.
    pub fn claude_output(&self, input: &serde_json::Value) -> Option<String> {
        let decision = match self {
            Self::Answers(answers) => {
                let questions = input.get("questions")?.as_array()?;
                if questions.len() != answers.len() {
                    return None;
                }
                let mut named = serde_json::Map::new();
                for (question, answer) in questions.iter().zip(answers) {
                    let text = question.get("question")?.as_str()?;
                    if answer.trim().is_empty() {
                        return None;
                    }
                    named.insert(text.to_owned(), serde_json::Value::from(answer.as_str()));
                }
                let mut updated = input.clone();
                updated
                    .as_object_mut()?
                    .insert("answers".to_owned(), serde_json::Value::Object(named));
                serde_json::json!({ "behavior": "allow", "updatedInput": updated })
            }
            Self::Approve => {
                input.as_object()?;
                serde_json::json!({ "behavior": "allow", "updatedInput": input })
            }
            Self::KeepPlanning(feedback) => {
                let feedback = feedback.trim();
                if feedback.is_empty() {
                    return None;
                }
                serde_json::json!({ "behavior": "deny", "message": feedback, "interrupt": false })
            }
        };
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": decision,
            }
        }))
        .ok()
    }
}

/// One status line payload, and which worktree drew it.
///
/// The worktree is carried because two different things are read out of the
/// same payload and they are not both global: the quota in it belongs to the
/// account, but the context and cost in it belong to *that session* — see
/// [`crate::usage::from_statusline`] — and a reading with no worktree on it
/// could only be shown against all of them or none.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusLine {
    /// The worktree the session is in, from `KET_WORKTREE_ID`. `None` for a
    /// session ket launched before it set that, or one launched by hand.
    pub worktree: Option<String>,
    /// Which terminal it was launched into, from [`PANE_ENV`].
    ///
    /// The worktree alone cannot answer "which of these is spending this":
    /// every pane in a worktree is launched with the same worktree id, so
    /// without this two agents side by side are one reading that flickers
    /// between them. `None` from a session started before ket set the
    /// variable, or by hand in a shell ket did not open.
    pub pane: Option<String>,
    /// Which agent drew it, from `KET_AGENT_NAME`.
    pub agent: Option<String>,
    /// The payload itself, exactly as Claude wrote it.
    pub payload: serde_json::Value,
}

/// Most bytes read from one hook post.
///
/// A tool payload can carry a whole file's contents. ket reads the envelope,
/// not the transcript, and a hook that could make the window allocate without
/// bound is a hook that can take the window down.
const MAX_BODY: usize = 256 * 1024;

/// The tool each Grok session last announced, by session id.
///
/// Grok's permission prompt arrives as a `Notification` that names no tool.
/// The tool is the one its `PreToolUse` announced just before — Grok waits
/// for that hook to finish before it decides to ask — so the listener keeps
/// the last one per session and hands it to the prompt.
type GrokTools = std::collections::HashMap<String, serde_json::Value>;

/// Most Grok sessions [`GrokTools`] remembers before it starts over.
const GROK_TOOLS_MAX: usize = 64;

/// Per-terminal hook credentials. The token, rather than the hook's claimed
/// pane field, determines which terminal a report can affect.
#[derive(Clone, Default)]
pub struct HookBindings {
    state: Arc<Mutex<HookBindingState>>,
}

#[derive(Default)]
struct HookBindingState {
    panes: HashMap<String, String>,
    sessions: HashMap<String, String>,
}

fn lock_bindings(mutex: &Mutex<HookBindingState>) -> MutexGuard<'_, HookBindingState> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HookBindings {
    /// Issues or resumes a token bound to `pane`.
    pub fn bind(&self, pane: &str, resume_session: Option<&str>) -> String {
        let mut state = lock_bindings(&self.state);
        let token = resume_session
            .and_then(|session| state.sessions.get(session))
            .filter(|token| state.panes.get(*token).is_none_or(|bound| bound == pane))
            .cloned()
            .unwrap_or_else(new_token);
        state.panes.insert(token.clone(), pane.to_owned());
        token
    }

    /// Stops accepting credentials assigned to the closed panes. Session
    /// associations remain for a later explicit resume in this host run.
    pub fn unbind(&self, panes: &[String]) {
        let mut state = lock_bindings(&self.state);
        state
            .panes
            .retain(|_, pane| !panes.iter().any(|closed| closed == pane));
    }

    fn pane(&self, token: &str) -> Option<String> {
        lock_bindings(&self.state).panes.get(token).cloned()
    }

    fn remember_session(&self, token: &str, report: &HookReport) {
        if report.event != HookEvent::SessionStart {
            return;
        }
        let Some(session) = report
            .session
            .as_deref()
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            return;
        };
        let mut state = lock_bindings(&self.state);
        if state.sessions.len() >= 1024 && !state.sessions.contains_key(session) {
            return;
        }
        // First association wins, so later reports cannot move a known
        // session's credential to another pane.
        state
            .sessions
            .entry(session.to_owned())
            .or_insert_with(|| token.to_owned());
    }
}

#[derive(Clone)]
enum HookAuth {
    Legacy(String),
    Bound(HookBindings),
}

/// Checks `token` against `auth`. `Some((pane, token))`, where `pane` is the
/// pane a bound token is verified to belong to, or `None` for a legacy token
/// — which carries no pane concept at all, bound or otherwise. `None`
/// overall when `token` matches neither.
fn authenticate(auth: &HookAuth, token: &str) -> Option<(Option<String>, String)> {
    match auth {
        HookAuth::Legacy(expected) if token == expected => Some((None, token.to_owned())),
        HookAuth::Bound(bindings) => bindings
            .pane(token)
            .map(|pane| (Some(pane), token.to_owned())),
        _ => None,
    }
}

/// Reads one request and turns it into whatever it carried.
fn read_post(
    mut stream: std::net::TcpStream,
    auth: &HookAuth,
    grok_tools: &mut GrokTools,
) -> Option<Post> {
    use std::io::{Read, Write};

    // Accepted from a non-blocking listener, which on macOS makes it
    // non-blocking too: the first read that found nothing waiting would end
    // the request, and a body that came in more than one piece was dropped.
    stream.set_nonblocking(false).ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .ok()?;

    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.len() > MAX_BODY {
                    break;
                }
                // The body starts after the blank line; once we have all of
                // what the headers promised, there is nothing more to wait for.
                if let Some((head, body)) = split_request(&buffer)
                    && body.len() >= content_length(head)
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // A hook that waits for its answer is answered later, by whoever has it.
    if split_request(&buffer).is_some_and(|(head, _)| target(head) == AWAIT_PATH) {
        return awaited(stream, &buffer, auth);
    }

    let parsed = split_request(&buffer).and_then(|(head, body)| {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .map(|envelope| (head, envelope))
    });
    let accepted = parsed.as_ref().and_then(|(_, envelope)| {
        envelope
            .get("token")?
            .as_str()
            .and_then(|t| authenticate(auth, t))
    });
    let refused = parsed
        .as_ref()
        .is_some_and(|(_, envelope)| envelope.is_object() && accepted.is_none());
    let status: &[u8] = if refused {
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"
    } else {
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
    };
    let _ = stream.write_all(status);
    let _ = stream.flush();
    if refused {
        return None;
    }

    let (head, mut envelope) = parsed?;
    let (trusted_pane, token) = accepted?;
    if let Some(pane) = trusted_pane {
        envelope["pane"] = serde_json::Value::String(pane);
    }
    let post = decode_envelope(&envelope, target(head) == "/statusline", grok_tools)?;
    if let (HookAuth::Bound(bindings), Post::Report(report)) = (auth, &post) {
        bindings.remember_session(&token, report);
    }
    Some(post)
}

/// What one hook envelope carried, once its token has been checked.
fn decode_envelope(
    envelope: &serde_json::Value,
    statusline: bool,
    grok_tools: &mut GrokTools,
) -> Option<Post> {
    let payload = envelope.get("payload")?;

    let text = |value: Option<&serde_json::Value>| {
        value
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };

    if statusline {
        return Some(Post::StatusLine(StatusLine {
            worktree: text(envelope.get("worktree")),
            pane: text(envelope.get("pane")),
            agent: text(envelope.get("agent")),
            payload: payload.clone(),
        }));
    }

    let agent = text(envelope.get("agent")).unwrap_or_else(|| "claude".to_owned());
    let grok;
    let payload = if agent == "grok" {
        grok = grok_payload(payload)?;
        &grok
    } else {
        payload
    };

    let event = HookEvent::from_hook_name(payload.get("hook_event_name")?.as_str()?)?;
    // `Notification` is a dozen different announcements under one name, and
    // only some of them are about the agent's state at all. Refining it needs
    // the payload, so it happens here rather than in `HookEvent::state` —
    // which is also why this is allowed to drop the post entirely.
    let event = match event {
        HookEvent::Notification => notification_event(payload)?,
        // Grok auto-allows its question tool, so the only sign it is waiting
        // on a person is the tool starting. Its `PostToolUse` is the answer.
        HookEvent::PreTool
            if agent == "grok"
                && payload.get("tool_name").and_then(|t| t.as_str())
                    == Some("ask_user_question") =>
        {
            HookEvent::Notification
        }
        other => other,
    };

    let session = text(payload.get("session_id"));
    let mut asked = None;
    if agent == "grok"
        && let Some(session) = session.as_deref()
    {
        match event {
            HookEvent::PreTool => {
                if grok_tools.len() >= GROK_TOOLS_MAX {
                    grok_tools.clear();
                }
                grok_tools.insert(session.to_owned(), payload.clone());
            }
            HookEvent::Notification
                if payload.get("notification_type").and_then(|t| t.as_str())
                    == Some("permission_prompt") =>
            {
                asked = grok_tools.get(session).and_then(permission_question);
            }
            HookEvent::SessionEnd => {
                grok_tools.remove(session);
            }
            _ => {}
        }
    }
    let event = if asked.is_some() {
        HookEvent::PermissionRequest
    } else {
        event
    };

    let subagent = text(payload.get("agent_id"));
    let subagent_description = subagent
        .as_deref()
        .and_then(|id| subagent_description(payload, id));

    Some(Post::Report(Box::new(HookReport {
        subagent_type: subagent
            .is_some()
            .then(|| text(payload.get("agent_type")))
            .flatten(),
        subagent_model: subagent
            .is_some()
            .then(|| text(payload.get("model")))
            .flatten(),
        subagent_description,
        subagent,
        tool_use_id: text(payload.get("tool_use_id")),
        teammate: text(payload.get("teammate_name")),
        // A compaction restarts the session without starting a new
        // conversation, and the subagents it was running carry on.
        fresh_session: event == HookEvent::SessionStart
            && payload.get("source").and_then(|s| s.as_str()) != Some("compact"),
        background: (event == HookEvent::Stop)
            .then(|| background_tasks(payload))
            .flatten(),
        call: matches!(
            event,
            HookEvent::PreTool | HookEvent::PostTool | HookEvent::PostToolFailure
        )
        .then(|| permission_question(payload).map(Box::new))
        .flatten(),
        question: match asked {
            Some(asked) => Some(Box::new(asked)),
            None => (event == HookEvent::PermissionRequest)
                .then(|| permission_question(payload).map(Box::new))
                .flatten(),
        },
        agent,
        event,
        worktree: text(envelope.get("worktree")),
        cwd: text(payload.get("cwd")).map(|path| Box::new(PathBuf::from(path))),
        pane: text(envelope.get("pane")),
        session,
        tool_name: text(payload.get("tool_name")),
        note: match event {
            // Only while a tool is actually in flight. A `PostToolUse` carries
            // the same `tool_name` the `PreToolUse` did, and keeping it there
            // would leave the row naming a command that has already finished.
            HookEvent::PreTool => tool_phrase(payload),
            // A turn that has ended, explaining itself. The subagent's stop is
            // deliberately not included: its last word is about its own errand
            // and the row belongs to the session that sent it.
            HookEvent::Stop => closing_line(payload),
            _ => None,
        },
        // Same window as the note: a `PostToolUse` names the tool that has
        // just finished, and a row still claiming to rebase after the rebase
        // returned is the stale-state bug in the other direction.
        mutating_git: matches!(event, HookEvent::PreTool) && tool_is_mutating_git(payload),
        at_ms: crate::now_ms(),
    })))
}

/// A question or a plan's hook, to hold open — or, when it is not one ket can
/// answer, answered at once with nothing, which leaves the prompt to the
/// terminal.
///
/// Not reported: the script has already reported the same payload to
/// `/hook`, and this is only the wait.
fn awaited(mut stream: std::net::TcpStream, buffer: &[u8], auth: &HookAuth) -> Option<Post> {
    use std::io::Write;
    let parked = (|| {
        let (_, body) = split_request(buffer)?;
        let envelope: serde_json::Value = serde_json::from_slice(body).ok()?;
        if envelope.get("agent").and_then(|a| a.as_str()) != Some("claude") {
            return None;
        }
        let token = envelope.get("token")?.as_str()?;
        // Any token `authenticate` recognises may park a wait. A bound one
        // parks it under the pane it belongs to, as `/hook` files its report
        // there: the envelope's own claim would let one pane's hook stand in
        // for another's prompt. A legacy token carries no pane, so it still
        // goes by the claim.
        let (trusted, _) = authenticate(auth, token)?;
        let pane = match trusted {
            Some(pane) => pane,
            None => envelope
                .get("pane")
                .and_then(|p| p.as_str())
                .filter(|p| !p.is_empty())?
                .to_owned(),
        };
        let payload = envelope.get("payload")?;
        if payload.get("hook_event_name").and_then(|e| e.as_str()) != Some("PermissionRequest") {
            return None;
        }
        let asked = permission_question(payload)?;
        asked.prompt.as_ref()?;
        Some((pane, asked, payload.get("tool_input")?.clone()))
    })();
    let Some((pane, asked, input)) = parked else {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
        return None;
    };
    // Held until the answer, with no read timeout left to cut it short.
    let _ = stream.set_read_timeout(None);
    Some(Post::Await(
        pane,
        Waiter {
            stream,
            asked,
            input,
            parked_at: std::time::Instant::now(),
        },
    ))
}

/// The reports hook scripts spooled while no listener could take them.
///
/// The tokens they carry are not checked: they belong to listeners that have
/// gone, so none would pass. The spool is owner-only, which is the boundary —
/// whoever can write it could read any token ket hands out anyway.
pub fn reports_from_spool(text: &str) -> Vec<HookReport> {
    let mut grok_tools = GrokTools::new();
    text.lines()
        .filter_map(|line| {
            let envelope: serde_json::Value = serde_json::from_str(line).ok()?;
            match decode_envelope(&envelope, false, &mut grok_tools)? {
                Post::Report(mut report)
                    if !matches!(
                        report.event,
                        HookEvent::PermissionRequest | HookEvent::Notification
                    ) =>
                {
                    // The spool is shared by all same-user processes and has
                    // no live listener to authenticate its pane token. Keep
                    // delayed status at worktree scope; never replay a prompt
                    // that a phone could answer after its originating context
                    // disappeared.
                    report.pane = None;
                    Some(*report)
                }
                Post::Report(_) => None,
                // `decode_envelope` never produces one: a wait is parked
                // live, over the stream, never written to the spool.
                Post::StatusLine(_) | Post::Await(..) => None,
            }
        })
        .collect()
}

/// What a `Notification` is actually about, or `None` when it is not ket's
/// business.
///
/// The hook is not one event. Claude Code raises it for a permission prompt,
/// for a session that has gone quiet, for a successful login, for entering and
/// leaving computer use, for an elicitation being answered — eleven kinds at
/// the last count — and says which in `notification_type` beside the message.
///
/// ket believed every one of them was a permission prompt. The common one is
/// `idle_prompt`, raised once a session has sat at its prompt a while, which
/// is the *opposite* of blocked: the turn is over and nothing is waiting on
/// anybody. So a worktree that had just finished went quiet when `Stop`
/// landed and then lit up "needs you" a minute later, holding it for the ten
/// minutes of [`crate::activity::REPORT_LINGERS_MS`] — on a row where nothing
/// at all was happening. A mark that cries wolf every time a task ends is one
/// that gets read past, which costs the real permission prompts too.
///
/// Nothing is lost by being strict here: a genuine permission prompt raises
/// Claude Code's own `PermissionRequest`, which ket registers and which maps
/// to the same state on its own.
fn notification_event(payload: &serde_json::Value) -> Option<HookEvent> {
    if let Some(kind) = payload.get("notification_type").and_then(|v| v.as_str()) {
        return match kind {
            // Stopped, and stopped on a person.
            "permission_prompt" | "worker_permission_prompt" | "agent_needs_input" => {
                Some(HookEvent::Notification)
            }
            // Finished, or quiet long enough to mention it. Both are `Stop`
            // under another name.
            "idle_prompt" | "agent_completed" => Some(HookEvent::Idle),
            // Logging in, computer use starting or stopping, an elicitation
            // answered, a push delivered. None of these says anything about
            // whether the agent is working, so none of them moves the row —
            // and a report posted here would renew the linger on a session
            // that may have been gone for hours.
            _ => None,
        };
    }

    // No `notification_type`: an older Claude Code, or another agent
    // borrowing the name. The message is all there is to go on.
    let message = payload
        .get("message")
        .and_then(|v| v.as_str())?
        .to_lowercase();
    if message.contains("waiting for your input") {
        Some(HookEvent::Idle)
    } else if message.contains("permission") || message.contains("needs your input") {
        Some(HookEvent::Notification)
    } else {
        // Unrecognised. "Needs you" is the loudest claim the sidebar makes,
        // and the one it should never make on a guess.
        None
    }
}

/// The MCP server a tool name belongs to, if it belongs to one.
///
/// Both agents name an MCP tool `mcp__<server>__<tool>`, so the server is the
/// middle field. A built-in tool — `Bash`, `Read` — has no prefix and returns
/// `None`, which is the common case and not a failure.
///
/// Why it matters at all: a connected server's tool schemas sit in the prefix
/// of *every* request, cached or not, whether or not the model ever calls one.
/// Counting the calls is how a reader finds the servers that are costing them
/// context and earning nothing back.
pub fn mcp_server(tool_name: &str) -> Option<&str> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, _tool) = rest.split_once("__")?;
    (!server.is_empty()).then_some(server)
}

/// The path a request was posted to, or `/hook` when the line cannot be read.
///
/// The default matters: a script from an older ket posts to `/hook` without
/// ket ever having to ask what it is.
fn target(head: &[u8]) -> String {
    String::from_utf8_lossy(head)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/hook")
        .to_owned()
}

/// Whether a tool call is `git` changing the worktree's history.
///
/// Only the shell tools can be: everything else an agent runs is reading and
/// writing files, which is what a diff is for. The read is
/// [`crate::activity::git_progress`]'s, so an agent's `git rebase` and one
/// typed by hand into the worktree's terminal are classified by the same
/// table rather than by two that drift.
fn tool_is_mutating_git(payload: &serde_json::Value) -> bool {
    let Some("Bash" | "BashOutput") = payload.get("tool_name").and_then(|t| t.as_str()) else {
        return false;
    };
    payload
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(serde_json::Value::as_str)
        .and_then(crate::activity::git_progress)
        .is_some_and(|(_, mutating)| mutating)
}

/// What a tool call should say on a row, from the payload the agent sent.
///
/// Claude writes a `description` for the two tools whose arguments are least
/// readable — a shell command and a subagent's brief — and it is a phrase
/// meant for a person, so it is used as written apart from its capital.
/// Everything else is composed here, because "reading tree.rs" is the useful
/// half of a `Read` call and the absolute path it actually carries is not.
///
/// `None` for a tool with nothing worth saying, which leaves the row on
/// [`crate::activity::Activity::detail`]'s own phrase rather than inventing a
/// worse one.
fn tool_phrase(payload: &serde_json::Value) -> Option<String> {
    let tool = payload.get("tool_name")?.as_str()?;
    let input = payload.get("tool_input");
    let field = |name: &str| {
        input
            .and_then(|i| i.get(name))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };

    // The file's own name. A row is 200 pixels wide and the leading
    // `/Users/me/…` is the same on every one of them.
    let leaf = |name: &str| {
        field(name).map(|path| {
            path.rsplit('/')
                .next()
                .filter(|leaf| !leaf.is_empty())
                .unwrap_or(path)
                .to_owned()
        })
    };

    let phrase = match tool {
        "Bash" | "BashOutput" => field("description")
            .map(readable)
            // No description — a subagent's shell calls often have none. A
            // `git` command still says what it is doing rather than just
            // "running git" — see `activity::git_progress`, which is the
            // same read a hand-typed `git commit` in an idle terminal gets.
            // Anything else falls back to the program's own name, which is
            // as much as a one-liner reliably gives up, framed rather than
            // bare because "grep" alone reads as a fragment beside
            // "thinking" and "waiting on you".
            .or_else(|| {
                field("command").map(|c| {
                    crate::activity::git_progress(c)
                        .map(|(phrase, _)| phrase)
                        .unwrap_or_else(|| format!("running {}", program(c)))
                })
            }),
        "Agent" | "Task" => field("description")
            .map(readable)
            .or_else(|| field("subagent_type").map(|t| format!("asking {t}"))),
        "Read" | "NotebookRead" => leaf("file_path").map(|f| format!("reading {f}")),
        "Write" => leaf("file_path").map(|f| format!("writing {f}")),
        "Edit" | "MultiEdit" | "NotebookEdit" => leaf("file_path").map(|f| format!("editing {f}")),
        "Grep" | "Glob" => Some("searching".to_owned()),
        "WebFetch" | "WebSearch" => Some("reading the web".to_owned()),
        _ => None,
    }?;

    Some(clip(&phrase))
}

/// The longest subject a [`PermissionQuestion`] keeps. A heredoc or a
/// generated script can be pages long; the phone shows the start of it.
const QUESTION_SUBJECT_MAX: usize = 2_000;

/// What a permission prompt is about, from its tool's input.
fn permission_question(payload: &serde_json::Value) -> Option<PermissionQuestion> {
    let tool = payload.get("tool_name")?.as_str()?.trim();
    if tool.is_empty() {
        return None;
    }
    let input = payload.get("tool_input");
    let field = |name: &str| {
        input
            .and_then(|i| i.get(name))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let subject = match tool {
        "Bash" => field("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" => field("file_path"),
        "NotebookEdit" | "NotebookRead" => field("notebook_path"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Grep" | "Glob" => field("pattern"),
        // What a row can say about a question or a plan in one line: the
        // first question, or the plan's title.
        "AskUserQuestion" => input
            .and_then(|i| i.get("questions"))
            .and_then(|q| q.get(0))
            .and_then(|q| q.get("question"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        "ExitPlanMode" => field("plan").and_then(|plan| {
            plan.lines()
                .map(|line| line.trim().trim_start_matches('#').trim())
                .find(|line| !line.is_empty())
        }),
        _ => field("command").or_else(|| field("file_path")),
    }
    .map(
        |subject| match subject.char_indices().nth(QUESTION_SUBJECT_MAX) {
            Some((at, _)) => format!("{}\u{2026}", &subject[..at]),
            None => subject.to_owned(),
        },
    );
    Some(PermissionQuestion {
        tool: tool.to_owned(),
        subject,
        id: 0,
        prompt: Prompt::of(tool, input),
    })
}

/// The first sentence of whatever the agent signed off with.
///
/// A stopped session used to say "ready", which is true and tells you
/// nothing — the row goes quiet at exactly the moment you most want to know
/// how it went, and the payload has been carrying the answer since the
/// beginning. Only the opening sentence: what arrives is a whole reply, often
/// several paragraphs of markdown, and a row is one line.
fn closing_line(payload: &serde_json::Value) -> Option<String> {
    let message = payload
        .get("last_assistant_message")?
        .as_str()?
        .trim()
        .trim_start_matches(['#', '*', '-', '>', ' ']);

    // Whichever comes first: the end of the sentence or the end of the line.
    let end = message
        .find(". ")
        .map(|at| at + 1)
        .into_iter()
        .chain(message.find('\n'))
        .min()
        .unwrap_or(message.len());

    let sentence = message[..end].trim().trim_end_matches('.');
    (!sentence.is_empty()).then(|| clip(sentence))
}

/// A description as the agent wrote it, minus its opening capital.
///
/// The rest of the row's vocabulary — "thinking", "waiting on you" — is
/// lowercase, and a sentence starting mid-row in title case reads as a
/// different kind of thing. Only an ordinary capitalised word is lowered:
/// `GitHub` and `CI` have their capitals for a reason and keep them.
fn readable(description: &str) -> String {
    let first = description.split_whitespace().next().unwrap_or("");
    let ordinary = first.chars().next().is_some_and(char::is_uppercase)
        && !first.chars().skip(1).any(char::is_uppercase);

    if !ordinary {
        return description.to_owned();
    }

    let mut chars = description.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => description.to_owned(),
    }
}

/// The program a shell command runs, skipping the variable assignments a
/// one-liner often opens with.
fn program(command: &str) -> &str {
    command
        .split_whitespace()
        .find(|token| !token.contains('=') && !token.starts_with('('))
        .map(|token| token.rsplit('/').next().unwrap_or(token))
        .unwrap_or(command)
}

/// Most characters a phrase may take on a row.
///
/// The row truncates at the sidebar's edge anyway; this is so the *report*
/// stays small, since a description can be a whole sentence and every one of
/// them is held per worktree for as long as the agent runs.
const MAX_PHRASE: usize = 72;

/// A phrase cut to [`MAX_PHRASE`], at a word boundary where there is one.
fn clip(phrase: &str) -> String {
    if phrase.chars().count() <= MAX_PHRASE {
        return phrase.to_owned();
    }

    let cut: String = phrase.chars().take(MAX_PHRASE).collect();
    let end = cut.rfind(' ').unwrap_or(cut.len());
    let kept = cut[..end].trim_end_matches([',', ';', ':', '.']);
    format!("{kept}…")
}

/// Splits a request into its headers and whatever body has arrived.
fn split_request(buffer: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = buffer.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some((&buffer[..at], &buffer[at + 4..]))
}

/// The body length the headers promise, or zero when they do not say.
fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    //! The scripts agents run, run the way they run them: `/bin/sh`, a
    //! payload on stdin, ket's variables in an otherwise empty environment,
    //! and a real listener at the other end. The listener and installers are covered in `tests/agent_hooks.rs`.

    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// A home directory of the test's own, removed when it finishes.
    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("ket-hook-script-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }

        fn hooks(&self) -> PathBuf {
            self.0.join(".ket").join("agent-hooks")
        }

        fn spool(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(self.hooks().join("spool.jsonl"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Runs `script` with `payload` on stdin and nothing in its environment
    /// but a home, the system's own tools, and `env`. Returns what it
    /// printed, having checked it exited 0 — it must, whatever happens.
    fn run(
        script: &str,
        args: &[&str],
        home: &Home,
        env: &[(&str, &str)],
        payload: &str,
    ) -> String {
        let path = home.0.join("script.sh");
        std::fs::write(&path, script).unwrap();
        let mut child = Command::new("/bin/sh")
            .arg(&path)
            .args(args)
            .env_clear()
            .env("HOME", &home.0)
            .env("PATH", "/usr/bin:/bin")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    /// The reports `listener` has, waiting until there are `count`.
    fn reports(listener: &Listener, count: usize) -> Vec<HookReport> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reports = Vec::new();
        while reports.len() < count {
            assert!(Instant::now() < deadline, "only {reports:?}");
            reports.extend(listener.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        reports
    }

    /// A port nothing is listening on.
    fn closed_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port().to_string()
    }

    const PROMPT: &str = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s-1"}"#;

    #[test]
    fn the_script_reports_to_the_port_it_was_launched_with() {
        let home = Home::new("port");
        let listener = Listener::start("window".to_owned()).unwrap();
        let port = listener.port().to_string();
        let worktree = r#"wt "quoted" \ slashed"#;
        let printed = run(
            SCRIPT,
            &[],
            &home,
            &[
                (PORT_ENV, &port),
                (TOKEN_ENV, "window"),
                (WORKTREE_ENV, worktree),
                (PANE_ENV, "pane-1"),
                ("KET_AGENT_NAME", "codex"),
            ],
            PROMPT,
        );
        assert_eq!(printed, "{}\n");

        let report = reports(&listener, 1).remove(0);
        assert_eq!(report.agent, "codex");
        assert_eq!(report.event, HookEvent::UserPrompt);
        assert_eq!(report.worktree.as_deref(), Some(worktree));
        assert_eq!(report.pane.as_deref(), Some("pane-1"));
        assert_eq!(report.session.as_deref(), Some("s-1"));
        assert!(home.spool().is_empty());
    }

    #[test]
    fn the_hosts_endpoint_comes_before_the_inherited_port() {
        let home = Home::new("endpoint");
        let host = Listener::start("host".to_owned()).unwrap();
        let window = Listener::start("window".to_owned()).unwrap();
        let window_port = window.port().to_string();
        let env = [(PORT_ENV, window_port.as_str()), (TOKEN_ENV, "window")];
        std::fs::create_dir_all(home.hooks()).unwrap();
        let endpoint = home.hooks().join("host-endpoint");

        std::fs::write(&endpoint, format!("{}\nhost\n", host.port())).unwrap();
        run(SCRIPT, &[], &home, &env, PROMPT);
        assert_eq!(reports(&host, 1).len(), 1);

        // A host that has gone leaves the window to hear it.
        std::fs::write(&endpoint, format!("{}\nhost\n", closed_port())).unwrap();
        run(SCRIPT, &[], &home, &env, PROMPT);
        assert_eq!(reports(&window, 1).len(), 1);
        assert!(host.drain().is_empty());
        assert!(home.spool().is_empty());
    }

    #[test]
    fn a_pane_bound_launch_tries_its_own_token_before_the_hosts_endpoint() {
        // The reverse of the test above: a launch that has a pane key
        // already has the one token that will actually validate, so trying
        // discovery first would spend a round trip that can never succeed —
        // the file never carries a pane-specific token. Proven here by
        // pointing the file at a dead port and a token that goes with
        // nothing: the report must still land on the first try.
        let home = Home::new("pane-first");
        let window = Listener::start("window".to_owned()).unwrap();
        let window_port = window.port().to_string();
        let env = [
            (PORT_ENV, window_port.as_str()),
            (TOKEN_ENV, "window"),
            (PANE_ENV, "pane-1"),
        ];
        std::fs::create_dir_all(home.hooks()).unwrap();
        let endpoint = home.hooks().join("host-endpoint");
        std::fs::write(&endpoint, format!("{}\nnot-this-one\n", closed_port())).unwrap();

        run(SCRIPT, &[], &home, &env, PROMPT);

        assert_eq!(reports(&window, 1).len(), 1);
        assert!(
            home.spool().is_empty(),
            "the first attempt should have landed"
        );
    }

    #[test]
    fn a_report_nobody_hears_is_spooled_for_the_owner_alone() {
        let home = Home::new("spool");
        let port = closed_port();
        let env = [(PORT_ENV, port.as_str()), (TOKEN_ENV, "window")];
        assert_eq!(run(SCRIPT, &[], &home, &env, PROMPT), "{}\n");
        run(SCRIPT, &[], &home, &env, PROMPT);

        let spooled = home.spool();
        assert_eq!(spooled.len(), 2);
        assert_eq!(spooled[0]["token"], "window");
        assert_eq!(spooled[0]["agent"], "claude");
        assert_eq!(
            spooled[0]["payload"],
            serde_json::from_str::<serde_json::Value>(PROMPT).unwrap()
        );

        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&home.hooks().join("spool.jsonl")), 0o600);
        assert_eq!(mode(&home.hooks()), 0o700);
    }

    #[test]
    fn nothing_to_say_is_answered_and_nothing_more() {
        let home = Home::new("empty");
        assert_eq!(run(SCRIPT, &[], &home, &[], ""), "{}\n");
        assert!(!home.hooks().exists());
    }

    #[test]
    fn the_script_never_breaks_the_agent() {
        // No tools at all: no curl, no sed, no mkdir.
        let home = Home::new("bare");
        let printed = run(SCRIPT, &[], &home, &[("PATH", "/nonexistent")], PROMPT);
        assert_eq!(printed, "{}\n");

        // No home to spool into.
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(SCRIPT)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
    }

    #[test]
    fn session_start_hands_each_agent_its_note_in_its_own_shape() {
        let home = Home::new("note");
        let start = r#"{"hook_event_name":"SessionStart","source":"startup"}"#;
        let note = "Be \"terse\".\nNo\tfluff \\ at all";
        let flat = "Be \"terse\". No fluff \\ at all";
        let with = |agent: &str, payload: &str, note: &str| {
            run(
                SCRIPT,
                &[],
                &home,
                &[
                    ("KET_AGENT_NAME", agent),
                    (crate::worktree::TOKEN_REDUCTION_NOTE_ENV, note),
                ],
                payload,
            )
        };

        let claude: serde_json::Value = serde_json::from_str(&with("claude", start, note)).unwrap();
        assert_eq!(
            claude,
            serde_json::json!({ "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": flat,
            } })
        );
        assert_eq!(with("codex", start, note), format!("{flat}\n"));
        assert_eq!(with("opencode", start, note), "{}\n");
        assert_eq!(with("claude", start, ""), "{}\n");
        assert_eq!(with("claude", PROMPT, note), "{}\n");
    }

    #[test]
    fn inside_grok_only_the_grok_entry_reports() {
        let home = Home::new("grok");
        let listener = Listener::start("window".to_owned()).unwrap();
        let port = listener.port().to_string();
        let env = [
            (PORT_ENV, port.as_str()),
            (TOKEN_ENV, "window"),
            ("GROK_HOOK_EVENT", "UserPromptSubmit"),
        ];

        // Claude's entry, which Grok runs too, keeps quiet.
        assert_eq!(run(SCRIPT, &[], &home, &env, PROMPT), "{}\n");
        assert_eq!(run(SCRIPT, &["grok"], &home, &env, PROMPT), "{}\n");
        let reports = reports(&listener, 1);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].agent, "grok");
        assert!(home.spool().is_empty());
    }

    #[test]
    fn the_status_line_reports_and_still_draws_what_it_displaced() {
        let home = Home::new("statusline");
        let listener = Listener::start("window".to_owned()).unwrap();
        let port = listener.port().to_string();
        std::fs::create_dir_all(home.hooks()).unwrap();
        std::fs::write(
            home.hooks().join("statusline-chain"),
            r#"cat > "$HOME/seen"; printf 'drawn by the chain'"#,
        )
        .unwrap();
        let payload = r#"{"rate_limits":{"five_hour":{"used_percentage":12}}}"#;

        let printed = run(
            STATUSLINE_SCRIPT,
            &[],
            &home,
            &[
                (PORT_ENV, &port),
                (TOKEN_ENV, "window"),
                (WORKTREE_ENV, "wt-1"),
            ],
            payload,
        );
        assert_eq!(printed, "drawn by the chain");
        assert_eq!(
            std::fs::read_to_string(home.0.join("seen")).unwrap(),
            payload
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        let lines = loop {
            let lines = listener.drain_statuslines();
            if !lines.is_empty() {
                break lines;
            }
            assert!(Instant::now() < deadline, "no status line");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(lines[0].worktree.as_deref(), Some("wt-1"));
        assert_eq!(
            lines[0].payload["rate_limits"]["five_hour"]["used_percentage"],
            12
        );

        // Outside ket it posts nothing, and the line is drawn all the same.
        assert_eq!(
            run(STATUSLINE_SCRIPT, &[], &home, &[], payload),
            "drawn by the chain"
        );
        // And with nothing on stdin, there is nothing to draw.
        assert_eq!(run(STATUSLINE_SCRIPT, &[], &home, &[], ""), "");
    }

    #[test]
    fn a_stale_token_is_refused_rather_than_believed() {
        // The regression this guards against: a 204 for a bad token let a
        // report with a stale token count as delivered, so it was never
        // retried or spooled. `curl -f` here turns the listener's 401 into a
        // failure the script notices.
        let home = Home::new("stale-token");
        let listener = Listener::start("real-token".to_owned()).unwrap();
        let port = listener.port().to_string();
        let env = [(PORT_ENV, port.as_str()), (TOKEN_ENV, "stale-token")];

        run(SCRIPT, &[], &home, &env, PROMPT);

        assert!(listener.drain().is_empty(), "a wrong token must not land");
        let spooled = home.spool();
        assert_eq!(spooled.len(), 1, "a refused report must still be kept");
        assert_eq!(spooled[0]["token"], "stale-token");
    }

    #[test]
    fn reports_from_spool_decodes_every_line_but_never_trusts_its_claimed_pane() {
        // Spooled reports come from listeners that have gone; none of their
        // tokens would ever match a live one, and nothing here can verify
        // which pane one actually belongs to — the spool directory being
        // owner-only bounds who wrote it, not which pane they can claim to
        // be. So the event itself still replays, filed at worktree scope
        // only: never under a pane a phone's prompt could land on.
        let envelope = |token: &str| {
            serde_json::json!({
                "agent": "claude",
                "worktree": "wt-1",
                "pane": "pane-1",
                "token": token,
                "version": "2",
                "payload": { "hook_event_name": "UserPromptSubmit", "session_id": "s-1" },
            })
            .to_string()
        };
        let text = format!(
            "{}\n{}\nnot json\n",
            envelope("from-a-listener-that-is-gone"),
            envelope("a-different-stale-token"),
        );

        let reports = reports_from_spool(&text);

        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|r| r.event == HookEvent::UserPrompt));
        assert!(
            reports.iter().all(|r| r.pane.is_none()),
            "a spooled report's claimed pane must never be trusted: {reports:?}"
        );
    }

    #[test]
    fn reports_from_spool_never_replays_a_prompt_a_phone_could_answer() {
        // Unlike an ordinary status report, a permission request or waiting
        // notification is something a phone can act on. Replaying one from
        // the spool, where the claimed pane cannot be verified, would be
        // exactly the forged-prompt injection finding 9 describes — so these
        // two event shapes are dropped outright rather than merely stripped
        // of their pane.
        let envelope = |payload: serde_json::Value| {
            serde_json::json!({
                "agent": "claude",
                "worktree": "wt-1",
                "pane": "pane-1",
                "token": "whatever-was-live-then",
                "version": "2",
                "payload": payload,
            })
            .to_string()
        };
        let text = format!(
            "{}\n{}\n",
            envelope(serde_json::json!({
                "hook_event_name": "PermissionRequest",
                "session_id": "s-1",
                "tool_name": "Bash",
            })),
            envelope(serde_json::json!({
                "hook_event_name": "Notification",
                "notification_type": "permission_prompt",
                "session_id": "s-1",
            })),
        );

        assert!(reports_from_spool(&text).is_empty());
    }

    #[test]
    fn reports_from_spool_skips_a_line_with_no_payload() {
        let text = "{\"agent\":\"claude\",\"token\":\"t\"}\n";
        assert!(reports_from_spool(text).is_empty());
    }
}
