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

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::event::AgentState;

mod claude;
mod codex;
mod decode;
mod failure;
mod grok;
mod listener;
mod opencode;
mod places;
mod script;
mod settings;
mod statusline;

pub use claude::{ClaudeHooks, PROMPT_WAIT_SECS};
pub use codex::CodexHooks;
pub(crate) use decode::command_program;
pub use decode::{mcp_server, reports_from_spool};
pub use grok::GrokHooks;
pub(crate) use grok::{grok_input, grok_tool};
pub use listener::{Decision, HookBindings, Listener, StatusLine, Unresolved, Waiter, Waiters};
pub use opencode::OpenCodeHooks;
pub use places::{
    Place, Removal, Removed, STATUSLINE_PLACE, installed_record_path, record_installed,
    remove_everywhere,
};
pub use script::{
    host_endpoint_path, install_script, new_token, publish_host_endpoint, script_path, spool_path,
};
pub use settings::OWNER_MARK;
pub use statusline::{
    ClaudeStatusLine, STATUSLINE_MARK, install_statusline_script, statusline_chain_path,
    statusline_script_path,
};

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

/// Environment variable carrying [`CLI_NOTE`]. Set where ket puts its own
/// CLI on a terminal's `PATH`, so a session is told about `ket` only where
/// it can run it.
pub const CLI_NOTE_ENV: &str = "KET_CLI_NOTE";

/// What an agent in a ket pane is told about the `ket` command as its session
/// starts, after the Economy note. Kept short: it stays in the context for
/// the whole session.
pub const CLI_NOTE: &str = "You are running inside ket, which manages this project's worktrees \
     and agents. The `ket` command on your PATH drives it: `ket backlog list` reads the \
     project's backlog, `ket backlog start <note>` or `ket start '<prompt>'` starts work in a \
     new worktree as a tab in the ket window, and `ket --help` lists the rest. `ket browser` \
     drives this worktree's browser tab in ket: `open <url>`, then `snapshot -i` lists what \
     can be acted on with refs like @e3 for `click @e3`, `fill @e3 '<text>'` and `press \
     Enter`, `screenshot` or `console` shows how it went, `dialog accept` or `dismiss` \
     answers an alert or confirm the page opens, and `record start` then `record stop` saves \
     a video and stills for showing a person what happened. `close` the tab when you're done \
     with it: it holds its page's memory until closed. `ket browser --help` lists the rest.";

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
    /// dozen unrelated announcements — see `notification_event`, which is
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
    /// The conversation is being compacted, until the agent's next report —
    /// see [`crate::agent_status::PaneAgentStatus::compacting`].
    PreCompact,
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
            "PreCompact" => Self::PreCompact,
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
    /// state machine, plus the two that say a turn is over and the two either
    /// side of a compaction, which a phone's chat says is under way.
    pub const CLAUDE_EVENTS: [&'static str; 16] = [
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
        "PreCompact",
        "PostCompact",
        "TeammateIdle",
    ];

    /// Every event ket asks Codex for.
    ///
    /// The intersection of what Codex offers and what [`HookEvent`] can do
    /// something with, which is smaller than [`HookEvent::CLAUDE_EVENTS`] in
    /// both directions. Codex has no `PostToolUseFailure`, `PermissionDenied`,
    /// `Notification`, `StopFailure` or `TeammateIdle`. Registering a name ket
    /// cannot map would spawn a subprocess per event to produce a report that
    /// is then dropped, so it is left out rather than parsed and discarded.
    ///
    /// Confirmed against Codex CLI 0.153.4's own `~/.codex/hooks.json` on this
    /// machine, not only against its documentation.
    pub const CODEX_EVENTS: [&'static str; 12] = [
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
        "PreCompact",
        "PostCompact",
    ];

    /// Every event ket asks Grok for.
    ///
    /// Grok takes Claude Code's names, plus `StopCancelled` for a turn that
    /// ended without finishing. Its subagent events are left out: a subagent's
    /// tools are not the pane's, and [`grok_payload`](grok::grok_payload) drops the ones that fire
    /// inside a subagent for the same reason. `PermissionRequest` does not
    /// exist — a prompt waiting on a person arrives as a `Notification` of
    /// type `permission_prompt`.
    ///
    /// Confirmed against Grok 1.0.44's hook runner on 2026-09-29; `PreCompact`
    /// against 1.0.77's on 2026-11-09.
    pub const GROK_EVENTS: [&'static str; 13] = [
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
        "PreCompact",
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
            Self::PreCompact | Self::Compact | Self::TeammateIdle => return None,
        })
    }

    /// Whether this event means the turn ended badly.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::StopFailure | Self::PostToolFailure)
    }
}

/// What one shell command's output weighed, as the agent's hook reported it.
///
/// The `PostToolUse` payload carries the tool's whole response, so the cost
/// of a command's output in the context is a thing ket can count rather than
/// assume: every byte here entered the conversation as fresh input once, and
/// is re-read as a cache hit on every turn after. Bytes rather than tokens,
/// because no tokenizer is shipped; `bytes / 4` is the usual estimate and a
/// floor for code and logs, which tokenize closer to three characters each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    /// The program the command ran, with its subcommand where one names the
    /// job — `cargo test`, `git status` — and nothing more: arguments can hold
    /// paths and secrets, and a count keyed by them would be a count of one.
    pub program: String,
    /// What the tool returned, in bytes.
    pub raw_bytes: u64,
    /// What reached the model, in bytes: the same as [`Self::raw_bytes`]
    /// unless the session's level filtered the output before the model read
    /// it, in which case the hook says how much it let through.
    pub shown_bytes: u64,
    /// Whether the command failed — a `PostToolUseFailure` rather than a
    /// `PostToolUse`.
    pub failed: bool,
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
    /// `tool_phrase` and `closing_line`.
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
    /// What a finished shell command handed back, measured — see
    /// [`CommandOutput`]. Only on a `PostTool` or `PostToolFailure` for the
    /// shell tool; `None` on every other event and tool.
    #[serde(default)]
    pub command: Option<Box<CommandOutput>>,
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
    /// its transcript — see `subagent_description`.
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
    /// What a permission rule ruled on this call, when one did — see
    /// [`crate::permissions`]. A prompt a rule answered arrives already as
    /// the event it became: allowed, a tool call; denied, a refusal.
    #[serde(default)]
    pub ruling: Option<Box<crate::permissions::Ruling>>,
    /// What a `StopFailure` says went wrong — see [`failure::failure`]. Only
    /// on that event, and only when its payload gave an account.
    #[serde(default)]
    pub failure: Option<Box<crate::limits::Failure>>,
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
/// Codex's questions ([`CODEX_ASK_TOOL`]) are the exception: its hook cannot
/// answer, so its form is answered with its own keys — see
/// [`Decision::Picks`].
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
    /// A dialog the agent's TUI drew over its prompt, read off the screen —
    /// see [`crate::screen`]. Answered with a number and Enter, not through
    /// a hook: the TUI said nothing to one.
    Dialog {
        /// Its heading.
        title: String,
        /// Its numbered options in order, without the numbers.
        options: Vec<String>,
    },
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
pub const PROMPT_TOOLS: [&str; 3] = ["AskUserQuestion", "ExitPlanMode", CODEX_ASK_TOOL];

/// Codex's question tool, as its hooks name it: its `PreToolUse` is the form
/// going up, its `PostToolUse` the answer.
pub const CODEX_ASK_TOOL: &str = "request_user_input";

/// Whether Codex's form asking `questions` can be answered by number: every
/// question has options, and no more than one key's worth. A question
/// answered in words is left to the terminal.
pub fn codex_answerable(questions: &[Asked]) -> bool {
    questions
        .iter()
        .all(|question| (1..=9).contains(&question.options.len()))
}

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
            // Codex's shape: an id, a header, the question, and options that
            // can be missing for a question answered in words. One pick
            // each; "None of the above" is an option of its own when
            // `isOther` asks for it, as the form draws it. Every question is
            // kept, options or not, so the answers line up with the form's.
            CODEX_ASK_TOOL => {
                let questions: Vec<Asked> = input?
                    .get("questions")?
                    .as_array()?
                    .iter()
                    .filter_map(|question| {
                        let mut options: Vec<Offered> = question
                            .get("options")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|option| Offered {
                                label: text(option, "label"),
                                description: text(option, "description"),
                            })
                            .collect();
                        let other = question
                            .get("isOther")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        if other && !options.is_empty() {
                            options.push(Offered {
                                label: "None of the above".to_owned(),
                                description: String::new(),
                            });
                        }
                        Some(Asked {
                            question: question.get("question")?.as_str()?.trim().to_owned(),
                            header: text(question, "header"),
                            options,
                            multi_select: false,
                        })
                    })
                    .collect();
                (!questions.is_empty()).then_some(Self::Ask { questions })
            }
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

/// Installs and removes ket's hooks in one agent's own configuration.
pub trait HookInstaller: Send + Sync {
    /// The agent's name, matching [`crate::config::AgentSpec::name`].
    fn agent(&self) -> &str;

    /// Whether ket's hooks are currently registered.
    fn is_installed(&self) -> Result<bool>;

    /// Whether any of ket's entries is there, from this version or an older
    /// one. Not [`is_installed`](Self::is_installed): that asks whether this
    /// version's full set is, and a set an older ket wrote is still ket's to
    /// take out.
    fn has_any(&self) -> Result<bool>;

    /// Where these hooks go, as [`record_installed`] notes it.
    fn place(&self) -> Place;

    /// Registers them, merging into whatever is already configured.
    fn install(&self, script: &Path) -> Result<()>;

    /// Removes only ket's own entries, leaving everything else alone.
    fn remove(&self) -> Result<()>;
}

/// The user's home directory, or the current directory if there is none.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests;
