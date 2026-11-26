//! Reading what a hook posts into a [`HookReport`]: which event it is, what
//! tool, and what the agent is waiting on.

use std::path::PathBuf;

use super::grok::grok_payload;
use super::listener::{Post, StatusLine};
use super::{
    BackgroundTask, CODEX_ASK_TOOL, CommandOutput, HookEvent, HookReport, PermissionQuestion,
    Prompt, failure,
};

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

/// The tool each Grok session last announced, by session id.
///
/// Grok's permission prompt arrives as a `Notification` that names no tool.
/// The tool is the one its `PreToolUse` announced just before — Grok waits
/// for that hook to finish before it decides to ask — so the listener keeps
/// the last one per session and hands it to the prompt.
pub(super) type GrokTools = std::collections::HashMap<String, serde_json::Value>;

/// Most Grok sessions [`GrokTools`] remembers before it starts over.
const GROK_TOOLS_MAX: usize = 64;

/// What one hook envelope carried, once its token has been checked.
pub(super) fn decode_envelope(
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

    // A call a permission rule already decided. Claude never shows a prompt
    // ket allowed, and never runs a call ket refused, so neither may wait on
    // a person here — not on a row, not on a phone.
    let ruling: Option<crate::permissions::Ruling> = envelope
        .get("ruling")
        .filter(|ruling| !ruling.is_null())
        .and_then(|ruling| serde_json::from_value(ruling.clone()).ok());
    let event = match (event, ruling.as_ref().map(|ruling| ruling.verdict)) {
        (HookEvent::PermissionRequest, Some(crate::permissions::Verdict::Allow)) => {
            HookEvent::PreTool
        }
        (
            HookEvent::PreTool | HookEvent::PermissionRequest,
            Some(crate::permissions::Verdict::Deny),
        ) => HookEvent::PermissionDenied,
        (event, _) => event,
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
            // Codex's question asks no permission: its `PreToolUse` is the
            // form on screen — see `StatusStore::report`.
            None => (event == HookEvent::PermissionRequest
                || (agent.eq_ignore_ascii_case("codex")
                    && event == HookEvent::PreTool
                    && text(payload.get("tool_name")).as_deref() == Some(CODEX_ASK_TOOL)))
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
        command: command_output(event, payload, envelope).map(Box::new),
        ruling: ruling.map(Box::new),
        failure: (event == HookEvent::StopFailure)
            .then(|| failure::failure(payload).map(Box::new))
            .flatten(),
        at_ms: crate::now_ms(),
    })))
}

/// The measured output of a finished shell command, when the payload is one.
///
/// Only the foreground shell tool: a backgrounded command's result arrives
/// through `BashOutput` in pieces, and counting the pieces and the whole
/// would count it twice. `BashOutput` itself is left out for the same
/// reason, so background runs are simply not measured.
pub(super) fn command_output(
    event: HookEvent,
    payload: &serde_json::Value,
    envelope: &serde_json::Value,
) -> Option<CommandOutput> {
    if !matches!(event, HookEvent::PostTool | HookEvent::PostToolFailure) {
        return None;
    }
    if payload.get("tool_name").and_then(|t| t.as_str()) != Some("Bash") {
        return None;
    }
    let input = payload.get("tool_input");
    if input
        .and_then(|i| i.get("run_in_background"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return None;
    }
    let command = input
        .and_then(|i| i.get("command"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    // Claude sends the response itself; an agent whose plugin sends only its
    // size (OpenCode, which keeps bodies out of its posts) says so with a
    // number instead.
    let raw_bytes = match payload.get("tool_response") {
        Some(response) => response_bytes(response),
        None => payload
            .get("tool_response_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    };
    let shown_bytes = envelope
        .get("shown")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(raw_bytes)
        .min(raw_bytes);
    Some(CommandOutput {
        program: command_program(command),
        raw_bytes,
        shown_bytes,
        failed: event == HookEvent::PostToolFailure,
    })
}

/// How many bytes of text a tool response holds, whatever shape it came in.
///
/// Claude's shell tool answers with `{stdout, stderr, …}`; older payloads and
/// other agents answer with a string. Summing every string leaf reads both,
/// and reads a shape ket has not seen yet as the text it carries rather than
/// as nothing.
pub(super) fn response_bytes(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::String(text) => text.len() as u64,
        serde_json::Value::Array(items) => items.iter().map(response_bytes).sum(),
        serde_json::Value::Object(fields) => fields.values().map(response_bytes).sum(),
        _ => 0,
    }
}

/// The program a command runs, with its subcommand for the tools whose
/// subcommand is the job: `cargo test` and `cargo build` are different
/// things to be noisy about, `cargo` is not.
pub(crate) fn command_program(command: &str) -> String {
    const WITH_SUBCOMMAND: [&str; 7] = ["cargo", "git", "npm", "pnpm", "yarn", "go", "gh"];
    let program = program(command).to_ascii_lowercase();
    if !WITH_SUBCOMMAND.contains(&program.as_str()) {
        return program;
    }
    // The first plain word after the program: not a flag, and not a flag's
    // value either — `git -C /x status` is a `git status`, and a path or a
    // number is never a subcommand.
    let subcommand = command
        .split_whitespace()
        .skip_while(|token| token.contains('=') || token.starts_with('('))
        .skip(1)
        .find(|token| {
            !token.starts_with('-')
                && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && token.chars().any(|c| c.is_ascii_alphabetic())
        })
        .map(str::to_ascii_lowercase);
    match subcommand {
        Some(sub) => format!("{program} {sub}"),
        None => program,
    }
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
pub(super) fn permission_question(payload: &serde_json::Value) -> Option<PermissionQuestion> {
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
        "AskUserQuestion" | CODEX_ASK_TOOL => input
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
