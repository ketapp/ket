//! Agent hooks, over the wire and on disk.
//!
//! Two halves, both against the real thing. The listener is posted to over a
//! real loopback socket, exactly as the hook script's `curl` does, so what is
//! checked is the report ket makes of each agent's payload — the note on a
//! row, the permission prompt a phone answers, the reports it must drop. The
//! installers write into settings files in a sandbox, where the claim worth
//! checking is that the person's own configuration comes out whole.
//!
//! The scripts the agents run are covered beside them, in `agent_hooks.rs`'s
//! own tests.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::Sandbox;
use ket_core::agent_hooks::{
    self, Asked, BackgroundTask, ClaudeHooks, ClaudeStatusLine, CodexHooks, Decision, GrokHooks,
    HookEvent, HookInstaller, HookReport, Listener, Offered, OpenCodeHooks, PermissionQuestion,
    Prompt, StatusLine, Unresolved, mcp_server, script_path,
};
use ket_core::event::AgentState;
use serde_json::{Value, json};

const TOKEN: &str = "launch-token-for-tests";

/// The session every marker report is sent under — see [`Hooks::settle`].
const MARKER: &str = "ket-test-marker";

/// Longest the listener may take to hand over what it was sent.
const WAIT: Duration = Duration::from_secs(5);

// ---- the wire ---------------------------------------------------------------

/// A listener, and ways to post to it as an agent's hook would.
struct Hooks {
    listener: Listener,
}

impl Hooks {
    fn new() -> Self {
        Self {
            listener: Listener::start(TOKEN.to_owned()).unwrap(),
        }
    }

    fn port(&self) -> u16 {
        self.listener.port()
    }

    /// Sends one request and waits for ket's answer, which comes before it
    /// has made anything of the request.
    fn send(&self, request: &[u8]) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        // 401 is a real answer too: the token the envelope carried was wrong,
        // and the listener says so out loud rather than swallowing the
        // report — see `a_report_without_the_launch_token_is_dropped`. What
        // `send` guards against is the request hanging the agent, not the
        // particular status.
        assert!(
            answer.starts_with("HTTP/1.1 204") || answer.starts_with("HTTP/1.1 401"),
            "{answer:?}"
        );
    }

    /// Every report made since the last call, once the listener has caught
    /// up. It reads one connection at a time, so a marker posted after the
    /// rest arrives after whatever they became.
    fn settle(&self) -> Vec<HookReport> {
        let marker = envelope(
            "claude",
            json!({ "hook_event_name": "PostCompact", "session_id": MARKER }),
        );
        self.send(&request("/hook", &marker.to_string()));
        let deadline = Instant::now() + WAIT;
        let mut reports = Vec::new();
        loop {
            for report in self.listener.drain() {
                if report.session.as_deref() == Some(MARKER) {
                    return reports;
                }
                reports.push(report);
            }
            assert!(Instant::now() < deadline, "the listener went quiet");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// What the listener made of one envelope, or `None` when it dropped it.
    fn post(&self, envelope: &Value) -> Option<HookReport> {
        self.post_body(&envelope.to_string())
    }

    fn post_body(&self, body: &str) -> Option<HookReport> {
        self.send(&request("/hook", body));
        let mut reports = self.settle();
        assert!(reports.len() <= 1, "{reports:?}");
        reports.pop()
    }

    fn claude(&self, payload: Value) -> Option<HookReport> {
        self.post(&envelope("claude", payload))
    }

    fn grok(&self, payload: Value) -> Option<HookReport> {
        self.post(&envelope("grok", payload))
    }
}

/// One POST, as `curl --data-binary` sends it.
fn request(path: &str, body: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The envelope the hook script wraps a payload in.
fn envelope(agent: &str, payload: Value) -> Value {
    json!({
        "agent": agent,
        "worktree": "wt-1",
        "pane": "pane-1",
        "token": TOKEN,
        "version": "1",
        "payload": payload,
    })
}

/// A payload for `event`, with `extra`'s fields on top of the ones every
/// hook sends.
fn event(name: &str, extra: Value) -> Value {
    let mut payload = json!({
        "hook_event_name": name,
        "session_id": "s-1",
        "cwd": "/tmp/project",
    });
    let object = payload.as_object_mut().unwrap();
    for (key, value) in extra.as_object().unwrap() {
        object.insert(key.clone(), value.clone());
    }
    payload
}

/// A `PreToolUse` for `tool` with `input`.
fn pre_tool(tool: &str, input: Value) -> Value {
    event(
        "PreToolUse",
        json!({ "tool_name": tool, "tool_input": input }),
    )
}

#[test]
fn a_report_says_where_it_came_from() {
    let hooks = Hooks::new();
    let report = hooks
        .claude(event("UserPromptSubmit", json!({})))
        .expect("reported");

    assert_eq!(report.agent, "claude");
    assert_eq!(report.event, HookEvent::UserPrompt);
    assert_eq!(report.worktree.as_deref(), Some("wt-1"));
    assert_eq!(report.pane.as_deref(), Some("pane-1"));
    assert_eq!(report.session.as_deref(), Some("s-1"));
    assert_eq!(report.cwd.as_deref(), Some(&PathBuf::from("/tmp/project")));
    assert_eq!(report.tool_name, None);
    assert_eq!(report.note, None);
    assert!(!report.mutating_git);
    assert_eq!(report.subagent, None);
    assert_eq!(report.question, None);
    assert_eq!(report.background, None);
    assert!(!report.fresh_session);
    assert!(report.at_ms > 0);
}

#[test]
fn a_report_from_outside_ket_names_no_worktree() {
    let hooks = Hooks::new();
    let mut outside = envelope("", event("Stop", json!({ "session_id": "" })));
    outside["worktree"] = json!("");
    outside["pane"] = json!("");
    let report = hooks.post(&outside).expect("reported");
    assert_eq!(report.worktree, None);
    assert_eq!(report.pane, None);
    assert_eq!(report.session, None);
    // An agent that does not say is taken to be Claude.
    assert_eq!(report.agent, "claude");

    let mut unnamed = envelope("claude", event("Stop", json!({})));
    unnamed.as_object_mut().unwrap().remove("agent");
    assert_eq!(hooks.post(&unnamed).expect("reported").agent, "claude");
}

#[test]
fn a_report_without_the_launch_token_is_dropped() {
    let hooks = Hooks::new();
    for token in [json!("someone-else"), json!(""), json!(null), json!(7)] {
        let mut forged = envelope("claude", event("UserPromptSubmit", json!({})));
        forged["token"] = token.clone();
        assert_eq!(hooks.post(&forged), None, "token {token}");
    }
    let mut missing = envelope("claude", event("UserPromptSubmit", json!({})));
    missing.as_object_mut().unwrap().remove("token");
    assert_eq!(hooks.post(&missing), None);

    // The status line is held to the same.
    let mut forged = envelope("claude", json!({ "rate_limits": {} }));
    forged["token"] = json!("someone-else");
    hooks.send(&request("/statusline", &forged.to_string()));
    hooks.settle();
    assert!(hooks.listener.drain_statuslines().is_empty());
}

#[test]
fn a_post_that_is_not_a_report_is_dropped_and_the_listener_carries_on() {
    let hooks = Hooks::new();
    let bodies = [
        String::new(),
        "not json".to_owned(),
        "[1, 2, 3]".to_owned(),
        json!({ "token": TOKEN }).to_string(),
        envelope("claude", json!({})).to_string(),
        envelope("claude", json!({ "hook_event_name": 5 })).to_string(),
        envelope("claude", json!({ "hook_event_name": "FileChanged" })).to_string(),
        envelope("claude", json!({ "hook_event_name": "stop" })).to_string(),
        envelope("claude", json!("a string payload")).to_string(),
    ];
    for body in bodies {
        assert_eq!(hooks.post_body(&body), None, "{body}");
    }
    assert!(hooks.claude(event("Stop", json!({}))).is_some());
}

#[test]
fn a_request_line_that_cannot_be_read_is_taken_as_a_hook() {
    let hooks = Hooks::new();
    let body = envelope("claude", event("Stop", json!({}))).to_string();
    let request = format!("POST\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    hooks.send(request.as_bytes());
    let reports = hooks.settle();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].event, HookEvent::Stop);
}

#[test]
fn a_body_that_arrives_after_its_headers_is_waited_for() {
    let hooks = Hooks::new();
    let body = envelope("claude", event("Stop", json!({}))).to_string();
    let request = request("/hook", &body);
    let split = request.len() - body.len();

    let mut stream = TcpStream::connect(("127.0.0.1", hooks.port())).unwrap();
    stream.write_all(&request[..split]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(&request[split..split + 10]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(&request[split + 10..]).unwrap();
    let mut answer = String::new();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let _ = stream.read_to_string(&mut answer);
    assert!(answer.starts_with("HTTP/1.1 204"), "{answer:?}");

    let reports = hooks.settle();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].event, HookEvent::Stop);
}

#[test]
fn an_oversized_post_is_dropped_without_taking_the_listener_down() {
    let hooks = Hooks::new();
    let mut payload = event("PreToolUse", json!({ "tool_name": "Write" }));
    payload["tool_input"] = json!({ "content": "x".repeat(300 * 1024) });
    let request = request("/hook", &envelope("claude", payload).to_string());

    let mut stream = TcpStream::connect(("127.0.0.1", hooks.port())).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    // ket stops reading past its limit, so the tail of this may be refused.
    let _ = stream.write_all(&request);
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);

    assert!(hooks.settle().is_empty());
    assert!(hooks.claude(event("Stop", json!({}))).is_some());
}

#[test]
fn a_silent_connection_holds_the_listener_for_half_a_second_at_most() {
    let hooks = Hooks::new();
    let _silent = TcpStream::connect(("127.0.0.1", hooks.port())).unwrap();
    let started = Instant::now();
    assert!(hooks.claude(event("Stop", json!({}))).is_some());
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn dropping_the_listener_closes_its_port() {
    let hooks = Hooks::new();
    let port = hooks.port();
    drop(hooks);
    let deadline = Instant::now() + WAIT;
    while TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(Instant::now() < deadline, "still listening on {port}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---- events -----------------------------------------------------------------

#[test]
fn every_hook_name_ket_asks_an_agent_for_is_one_it_can_read() {
    // An event registered with an agent and then dropped on arrival is a
    // subprocess per event for nothing.
    for name in HookEvent::CLAUDE_EVENTS
        .iter()
        .chain(&HookEvent::CODEX_EVENTS)
        .chain(&HookEvent::GROK_EVENTS)
    {
        assert!(HookEvent::from_hook_name(name).is_some(), "{name}");
    }
}

#[test]
fn hook_names_map_to_events() {
    let table = [
        ("SessionStart", HookEvent::SessionStart),
        ("UserPromptSubmit", HookEvent::UserPrompt),
        ("PreToolUse", HookEvent::PreTool),
        ("PostToolUse", HookEvent::PostTool),
        ("PostToolUseFailure", HookEvent::PostToolFailure),
        ("PermissionRequest", HookEvent::PermissionRequest),
        ("PermissionDenied", HookEvent::PermissionDenied),
        ("Notification", HookEvent::Notification),
        ("SessionEnd", HookEvent::SessionEnd),
        ("Stop", HookEvent::Stop),
        ("Interrupt", HookEvent::Interrupt),
        ("StopCancelled", HookEvent::Interrupt),
        ("StopFailure", HookEvent::StopFailure),
        ("SubagentStart", HookEvent::SubagentStart),
        ("SubagentStop", HookEvent::SubagentStop),
        ("PostCompact", HookEvent::Compact),
        ("TeammateIdle", HookEvent::TeammateIdle),
    ];
    for (name, event) in table {
        assert_eq!(HookEvent::from_hook_name(name), Some(event), "{name}");
    }
    for name in ["", "stop", "PreCompact", "FileChanged", "Idle"] {
        assert_eq!(HookEvent::from_hook_name(name), None, "{name}");
    }
}

#[test]
fn events_move_the_state_machine() {
    use AgentState::*;
    let table = [
        (HookEvent::SessionStart, Some(Starting)),
        (HookEvent::UserPrompt, Some(Thinking)),
        (HookEvent::PreTool, Some(ExecutingTool)),
        (HookEvent::SubagentStart, Some(ExecutingTool)),
        (HookEvent::PostTool, Some(Thinking)),
        (HookEvent::PostToolFailure, Some(Thinking)),
        (HookEvent::SubagentStop, Some(Thinking)),
        (HookEvent::PermissionRequest, Some(AwaitingPermission)),
        (HookEvent::Notification, Some(AwaitingPermission)),
        (HookEvent::PermissionDenied, Some(Thinking)),
        (HookEvent::Stop, Some(Idle)),
        (HookEvent::Interrupt, Some(Idle)),
        (HookEvent::StopFailure, Some(Idle)),
        (HookEvent::Idle, Some(Idle)),
        (HookEvent::SessionEnd, Some(Idle)),
        (HookEvent::Compact, None),
        (HookEvent::TeammateIdle, None),
    ];
    for (event, state) in table {
        assert_eq!(event.state(), state, "{event:?}");
        assert_eq!(
            event.is_failure(),
            matches!(event, HookEvent::StopFailure | HookEvent::PostToolFailure),
            "{event:?}"
        );
    }
}

#[test]
fn a_report_survives_the_trip_from_host_to_window() {
    let hooks = Hooks::new();
    let report = hooks
        .claude(pre_tool("Bash", json!({ "command": "git commit -m wip" })))
        .expect("reported");
    let text = serde_json::to_string(&report).unwrap();
    assert_eq!(serde_json::from_str::<HookReport>(&text).unwrap(), report);

    // One written before the optional fields existed still reads.
    let old: HookReport = serde_json::from_value(json!({
        "agent": "claude",
        "event": "stop",
        "worktree": "wt-1",
        "pane": null,
        "session": null,
        "toolName": null,
        "note": null,
        "mutatingGit": false,
        "atMs": 1,
    }))
    .unwrap();
    assert_eq!(old.event, HookEvent::Stop);
    assert_eq!(old.cwd, None);
    assert_eq!(old.background, None);
    assert!(!old.fresh_session);
}

// ---- what a row says ---------------------------------------------------------

#[test]
fn a_tool_call_says_what_it_is_doing() {
    let hooks = Hooks::new();
    let table: [(&str, Value, Option<&str>); 19] = [
        (
            "Bash",
            json!({ "description": "Run the test suite", "command": "cargo test" }),
            Some("run the test suite"),
        ),
        // Capitals that mean something are kept.
        (
            "Bash",
            json!({ "description": "GitHub: open the PR" }),
            Some("GitHub: open the PR"),
        ),
        (
            "Bash",
            json!({ "description": "CI checks" }),
            Some("CI checks"),
        ),
        (
            "Bash",
            json!({ "command": "RUST_LOG=debug /usr/local/bin/cargo build" }),
            Some("running cargo"),
        ),
        (
            "Bash",
            json!({ "command": "git commit -m wip" }),
            Some("committing"),
        ),
        (
            "BashOutput",
            json!({ "command": "git fetch" }),
            Some("fetching"),
        ),
        ("Bash", json!({ "description": "   " }), None),
        (
            "Agent",
            json!({ "description": "Find the parser" }),
            Some("find the parser"),
        ),
        (
            "Task",
            json!({ "subagent_type": "Explore" }),
            Some("asking Explore"),
        ),
        (
            "Read",
            json!({ "file_path": "/Users/me/dev/ket/src/tree.rs" }),
            Some("reading tree.rs"),
        ),
        // A path with no leaf is shown whole rather than as nothing.
        (
            "Read",
            json!({ "file_path": "/Users/me/dir/" }),
            Some("reading /Users/me/dir/"),
        ),
        ("Read", json!({}), None),
        (
            "Write",
            json!({ "file_path": "/a/b.txt" }),
            Some("writing b.txt"),
        ),
        (
            "MultiEdit",
            json!({ "file_path": "c.rs" }),
            Some("editing c.rs"),
        ),
        ("Grep", json!({ "pattern": "fn main" }), Some("searching")),
        ("Glob", json!({}), Some("searching")),
        (
            "WebSearch",
            json!({ "query": "gpui" }),
            Some("reading the web"),
        ),
        ("mcp__github__create_issue", json!({ "title": "x" }), None),
        ("SomethingNew", json!({ "command": "ls" }), None),
    ];
    for (tool, input, note) in table {
        let report = hooks
            .claude(pre_tool(tool, input.clone()))
            .expect("reported");
        assert_eq!(report.event, HookEvent::PreTool);
        assert_eq!(report.tool_name.as_deref(), Some(tool));
        assert_eq!(report.note.as_deref(), note, "{tool} {input}");
    }
}

#[test]
fn a_long_phrase_is_cut_at_a_word() {
    let hooks = Hooks::new();
    let description = "Rebuild the whole workspace, then run every integration test, and report back on any failures";
    let note = hooks
        .claude(pre_tool("Bash", json!({ "description": description })))
        .and_then(|r| r.note)
        .unwrap();
    assert!(note.chars().count() <= 73, "{note}");
    let kept = note.strip_suffix('…').expect("marked as cut");
    assert!(description.to_lowercase().starts_with(&kept.to_lowercase()));
    assert!(!kept.ends_with([' ', ',', ';', ':', '.']), "{note}");

    // One word longer than a row, and letters wider than a byte.
    for word in ["x".repeat(100), "é".repeat(100)] {
        let note = hooks
            .claude(pre_tool("Agent", json!({ "description": word })))
            .and_then(|r| r.note)
            .unwrap();
        assert_eq!(note.chars().count(), 73, "{note}");
        assert!(note.ends_with('…'));
    }
}

#[test]
fn git_rewriting_the_worktree_is_flagged_only_while_it_runs() {
    let hooks = Hooks::new();
    let table = [
        ("Bash", "git rebase --continue", true),
        ("Bash", "git commit -m wip", true),
        ("BashOutput", "git push origin main", true),
        ("Bash", "/usr/bin/git merge main", true),
        ("Bash", "git fetch", false),
        ("Bash", "git status", false),
        ("Bash", "cargo test", false),
        // Only the shell tools run commands.
        ("Read", "git commit", false),
    ];
    for (tool, command, mutating) in table {
        let report = hooks
            .claude(pre_tool(
                tool,
                json!({ "command": command, "description": "Carry on" }),
            ))
            .expect("reported");
        assert_eq!(report.mutating_git, mutating, "{tool} {command}");
    }

    // Once it has returned, the row stops claiming it.
    let finished = hooks
        .claude(event(
            "PostToolUse",
            json!({ "tool_name": "Bash", "tool_input": { "command": "git commit -m wip" } }),
        ))
        .expect("reported");
    assert!(!finished.mutating_git);
    assert_eq!(finished.note, None);
    assert_eq!(finished.tool_name.as_deref(), Some("Bash"));
}

#[test]
fn a_finished_turn_says_how_it_went() {
    let hooks = Hooks::new();
    let table = [
        (
            json!("Fixed the parser. Also tidied the tests."),
            Some("Fixed the parser"),
        ),
        (json!("## Summary\nAll green"), Some("Summary")),
        (json!("- first item\n- second item"), Some("first item")),
        (json!("> quoted line"), Some("quoted line")),
        (json!("Done."), Some("Done")),
        (json!("v1.2.3 is out"), Some("v1.2.3 is out")),
        (json!("   "), None),
        (json!(""), None),
        (json!(42), None),
    ];
    for (message, note) in table {
        let report = hooks
            .claude(event(
                "Stop",
                json!({ "last_assistant_message": message.clone() }),
            ))
            .expect("reported");
        assert_eq!(report.note.as_deref(), note, "{message}");
    }
    assert_eq!(hooks.claude(event("Stop", json!({}))).unwrap().note, None);

    let long = "word ".repeat(40);
    let note = hooks
        .claude(event("Stop", json!({ "last_assistant_message": long })))
        .and_then(|r| r.note)
        .unwrap();
    assert!(note.chars().count() <= 73 && note.ends_with('…'), "{note}");

    // A subagent's last word is about its own errand, and a failed turn's
    // is not a summary.
    for name in ["SubagentStop", "StopFailure"] {
        let report = hooks
            .claude(event(
                name,
                json!({ "last_assistant_message": "All done." }),
            ))
            .expect("reported");
        assert_eq!(report.note, None, "{name}");
    }
}

// ---- needs you ---------------------------------------------------------------

#[test]
fn only_a_notification_that_waits_on_a_person_says_so() {
    let hooks = Hooks::new();
    let by_type = [
        ("permission_prompt", Some(HookEvent::Notification)),
        ("worker_permission_prompt", Some(HookEvent::Notification)),
        ("agent_needs_input", Some(HookEvent::Notification)),
        ("idle_prompt", Some(HookEvent::Idle)),
        ("agent_completed", Some(HookEvent::Idle)),
        ("auth_success", None),
        ("elicitation_complete", None),
        ("", None),
    ];
    for (kind, expected) in by_type {
        // The type decides, whatever the message says.
        let report = hooks.claude(event(
            "Notification",
            json!({ "notification_type": kind, "message": "Claude needs your permission" }),
        ));
        assert_eq!(report.map(|r| r.event), expected, "{kind}");
    }

    let by_message = [
        (
            Some("Claude needs your permission to use Bash"),
            Some(HookEvent::Notification),
        ),
        (
            Some("Claude Code needs your input"),
            Some(HookEvent::Notification),
        ),
        (
            Some("Claude is waiting for your input"),
            Some(HookEvent::Idle),
        ),
        (Some("Something else entirely"), None),
        (None, None),
    ];
    for (message, expected) in by_message {
        let report = hooks.claude(event("Notification", json!({ "message": message })));
        assert_eq!(report.map(|r| r.event), expected, "{message:?}");
    }
}

#[test]
fn a_permission_prompt_carries_what_it_asks() {
    let hooks = Hooks::new();
    let ask = |tool: &str, input: Value| {
        hooks
            .claude(event(
                "PermissionRequest",
                json!({ "tool_name": tool, "tool_input": input, "tool_use_id": "call-1" }),
            ))
            .expect("reported")
    };
    let table = [
        (
            "Bash",
            json!({ "command": " rm -rf target " }),
            Some("rm -rf target"),
        ),
        ("Edit", json!({ "file_path": "/a/b.rs" }), Some("/a/b.rs")),
        ("Write", json!({ "file_path": "/a/c.rs" }), Some("/a/c.rs")),
        (
            "NotebookEdit",
            json!({ "notebook_path": "/n.ipynb" }),
            Some("/n.ipynb"),
        ),
        (
            "WebFetch",
            json!({ "url": "https://example.com" }),
            Some("https://example.com"),
        ),
        (
            "WebSearch",
            json!({ "query": "gpui drag" }),
            Some("gpui drag"),
        ),
        ("Grep", json!({ "pattern": "TODO" }), Some("TODO")),
        (
            "mcp__db__query",
            json!({ "command": "select 1" }),
            Some("select 1"),
        ),
        ("mcp__fs__open", json!({ "file_path": "/x" }), Some("/x")),
        ("mcp__fs__list", json!({ "depth": 2 }), None),
        ("Bash", json!({}), None),
    ];
    for (tool, input, subject) in table {
        let report = ask(tool, input.clone());
        assert_eq!(report.event, HookEvent::PermissionRequest);
        assert_eq!(report.tool_use_id.as_deref(), Some("call-1"));
        assert_eq!(
            report.question.as_deref(),
            Some(&PermissionQuestion {
                tool: tool.to_owned(),
                subject: subject.map(str::to_owned),
                id: 0,
                prompt: None,
            }),
            "{tool} {input}"
        );
    }

    // A heredoc is kept to its start, cut on a character, not a byte.
    for letter in ["x", "é"] {
        let long = letter.repeat(2_500);
        let subject = ask("Bash", json!({ "command": long }))
            .question
            .and_then(|q| q.subject)
            .unwrap();
        assert_eq!(subject.chars().count(), 2_001);
        assert!(subject.ends_with('…'));
    }

    // No tool, no question — but still a prompt.
    for tool in [json!("  "), json!(null)] {
        let report = hooks
            .claude(event("PermissionRequest", json!({ "tool_name": tool })))
            .expect("reported");
        assert_eq!(report.question, None);
        assert_eq!(report.event.state(), Some(AgentState::AwaitingPermission));
    }

    // Only a prompt carries one.
    let report = hooks
        .claude(pre_tool("Bash", json!({ "command": "ls" })))
        .unwrap();
    assert_eq!(report.question, None);
}

// ---- questions and plans -----------------------------------------------------

/// An `AskUserQuestion` with two questions, one of them multi-select.
fn two_questions() -> Value {
    json!({ "questions": [
        { "question": "Pick a colour", "header": "Colour", "multiSelect": false, "options": [
            { "label": "Red", "description": "warm" },
            { "label": "Blue", "description": "" },
        ]},
        { "question": "Pick toppings", "header": "Toppings", "multiSelect": true, "options": [
            { "label": "Cheese", "description": "" },
            { "label": "Ham", "description": "" },
        ]},
    ]})
}

/// A `PermissionRequest` for `tool` with `input`.
fn prompt_for(tool: &str, input: Value) -> Value {
    event(
        "PermissionRequest",
        json!({ "tool_name": tool, "tool_input": input }),
    )
}

#[test]
fn a_question_or_a_plan_carries_what_it_asks() {
    let hooks = Hooks::new();

    let asked = hooks
        .claude(prompt_for("AskUserQuestion", two_questions()))
        .and_then(|r| r.question)
        .expect("a question");
    assert_eq!(asked.subject.as_deref(), Some("Pick a colour"));
    assert_eq!(
        asked.prompt,
        Some(Prompt::Ask {
            questions: vec![
                Asked {
                    question: "Pick a colour".to_owned(),
                    header: "Colour".to_owned(),
                    options: vec![
                        Offered {
                            label: "Red".to_owned(),
                            description: "warm".to_owned()
                        },
                        Offered {
                            label: "Blue".to_owned(),
                            description: String::new()
                        },
                    ],
                    multi_select: false,
                },
                Asked {
                    question: "Pick toppings".to_owned(),
                    header: "Toppings".to_owned(),
                    options: vec![
                        Offered {
                            label: "Cheese".to_owned(),
                            description: String::new()
                        },
                        Offered {
                            label: "Ham".to_owned(),
                            description: String::new()
                        },
                    ],
                    multi_select: true,
                },
            ],
        })
    );

    let plan = hooks
        .claude(prompt_for(
            "ExitPlanMode",
            json!({ "plan": "\n# Ship it\n\n1. Build\n", "planFilePath": "/p.md" }),
        ))
        .and_then(|r| r.question)
        .expect("a plan");
    assert_eq!(plan.subject.as_deref(), Some("Ship it"));
    assert_eq!(plan.prompt, Some(Prompt::Plan));

    // A permission is not one.
    let bash = hooks
        .claude(prompt_for("Bash", json!({ "command": "ls" })))
        .and_then(|r| r.question)
        .expect("a permission");
    assert_eq!(bash.prompt, None);
}

/// Opens a `/hook/await` for `payload` and leaves it waiting, once the
/// listener holds it.
fn park(hooks: &Hooks, payload: Value) -> TcpStream {
    let question = hooks
        .claude(payload.clone())
        .and_then(|r| r.question)
        .expect("a question");
    let mut stream = TcpStream::connect(("127.0.0.1", hooks.port())).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream
        .write_all(&request(
            "/hook/await",
            &envelope("claude", payload).to_string(),
        ))
        .unwrap();
    let waiters = hooks.listener.waiters();
    let deadline = Instant::now() + WAIT;
    while !waiters.waiting("pane-1", &question) {
        assert!(Instant::now() < deadline, "never held");
        std::thread::sleep(Duration::from_millis(5));
    }
    stream
}

/// What a held hook was finally told: the status line, and the body.
fn told(mut stream: TcpStream) -> (String, String) {
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    (
        head.lines().next().unwrap_or_default().to_owned(),
        body.to_owned(),
    )
}

#[test]
fn a_held_question_is_answered_through_its_hook() {
    let hooks = Hooks::new();
    let payload = prompt_for("AskUserQuestion", two_questions());
    let question = hooks
        .claude(payload.clone())
        .and_then(|r| r.question)
        .unwrap();
    let stream = park(&hooks, payload);

    let waiters = hooks.listener.waiters();
    // Answers that do not fit are refused, and the hook is still held.
    assert_eq!(
        waiters.resolve(
            "pane-1",
            &question,
            &Decision::Answers(vec!["Blue".to_owned()])
        ),
        Err(Unresolved::Unfit)
    );
    // Another pane holds nothing.
    assert_eq!(
        waiters.resolve("pane-2", &question, &Decision::Approve),
        Err(Unresolved::NotHeld)
    );
    waiters
        .resolve(
            "pane-1",
            &question,
            &Decision::Answers(vec!["Blue".to_owned(), "Cheese, Ham".to_owned()]),
        )
        .unwrap();

    let (status, body) = told(stream);
    assert_eq!(status, "HTTP/1.1 200 OK");
    let body: Value = serde_json::from_str(&body).unwrap();
    let decision = &body["hookSpecificOutput"]["decision"];
    assert_eq!(
        body["hookSpecificOutput"]["hookEventName"],
        "PermissionRequest"
    );
    assert_eq!(decision["behavior"], "allow");
    assert_eq!(
        decision["updatedInput"]["questions"],
        two_questions()["questions"]
    );
    assert_eq!(
        decision["updatedInput"]["answers"],
        json!({ "Pick a colour": "Blue", "Pick toppings": "Cheese, Ham" })
    );
    // Answered once: it is no longer held.
    assert!(!waiters.waiting("pane-1", &question));
}

#[test]
fn a_plan_is_approved_or_sent_back_through_its_hook() {
    let hooks = Hooks::new();
    let input = json!({ "plan": "# Ship it", "planFilePath": "/p.md" });
    let payload = prompt_for("ExitPlanMode", input.clone());
    let question = hooks
        .claude(payload.clone())
        .and_then(|r| r.question)
        .unwrap();
    let waiters = hooks.listener.waiters();

    let stream = park(&hooks, payload.clone());
    waiters
        .resolve("pane-1", &question, &Decision::Approve)
        .unwrap();
    let body: Value = serde_json::from_str(&told(stream).1).unwrap();
    assert_eq!(
        body["hookSpecificOutput"]["decision"],
        json!({ "behavior": "allow", "updatedInput": input })
    );

    let stream = park(&hooks, payload);
    assert_eq!(
        waiters.resolve(
            "pane-1",
            &question,
            &Decision::KeepPlanning("  ".to_owned())
        ),
        Err(Unresolved::Unfit)
    );
    waiters
        .resolve(
            "pane-1",
            &question,
            &Decision::KeepPlanning("Add tests".to_owned()),
        )
        .unwrap();
    let body: Value = serde_json::from_str(&told(stream).1).unwrap();
    assert_eq!(
        body["hookSpecificOutput"]["decision"],
        json!({ "behavior": "deny", "message": "Add tests", "interrupt": false })
    );
}

#[test]
fn a_held_prompt_let_go_is_told_nothing() {
    let hooks = Hooks::new();
    let payload = prompt_for("ExitPlanMode", json!({ "plan": "# Ship it" }));
    let question = hooks
        .claude(payload.clone())
        .and_then(|r| r.question)
        .unwrap();
    let stream = park(&hooks, payload);
    let waiters = hooks.listener.waiters();

    // Just parked: its report may not have reached the status yet.
    waiters.release_unless(|_, _| false);
    assert!(waiters.waiting("pane-1", &question));

    std::thread::sleep(Duration::from_millis(2_100));
    waiters.release_unless(|pane, asked| pane == "pane-1" && asked.tool == "Bash");
    let (status, body) = told(stream);
    assert_eq!(status, "HTTP/1.1 204 No Content");
    assert_eq!(body, "");
    assert!(!waiters.waiting("pane-1", &question));
}

#[test]
fn an_await_that_is_not_a_question_is_answered_at_once() {
    let hooks = Hooks::new();
    // A permission, a stranger, and another agent: none of them waits.
    let bash = envelope("claude", prompt_for("Bash", json!({ "command": "ls" })));
    let mut stranger = envelope("claude", prompt_for("ExitPlanMode", json!({ "plan": "x" })));
    stranger["token"] = json!("not-the-token");
    let codex = envelope("codex", prompt_for("ExitPlanMode", json!({ "plan": "x" })));
    for body in [bash, stranger, codex] {
        hooks.send(&request("/hook/await", &body.to_string()));
    }
    // And none of them is reported: the script reports to `/hook` first.
    assert!(hooks.settle().is_empty());
}

// ---- sessions and subagents -------------------------------------------------

#[test]
fn only_a_session_start_that_is_not_a_compaction_is_fresh() {
    let hooks = Hooks::new();
    for (source, fresh) in [
        (json!("startup"), true),
        (json!("resume"), true),
        (json!("clear"), true),
        (json!(null), true),
        (json!("compact"), false),
    ] {
        let report = hooks
            .claude(event("SessionStart", json!({ "source": source })))
            .unwrap();
        assert_eq!(report.fresh_session, fresh, "{source}");
    }
    let prompt = hooks.claude(event("UserPromptSubmit", json!({}))).unwrap();
    assert!(!prompt.fresh_session);
}

#[test]
fn a_stop_lists_the_background_agents_still_running() {
    let hooks = Hooks::new();
    let stop = hooks
        .claude(event(
            "Stop",
            json!({ "background_tasks": [
                { "type": "subagent", "id": "a1", "agent_type": "Explore",
                  "description": "Find the parser", "status": "running" },
                { "type": "local_agent", "id": "a2", "status": "Completed" },
                { "type": "LOCAL_SUBAGENT", "id": "a3", "status": "something-new" },
                { "type": "teammate", "id": "t1", "status": "idle" },
                { "type": "subagent", "id": "a4" },
                { "type": "shell", "id": "sh1", "status": "running" },
                { "type": "monitor", "id": "m1" },
                { "type": "subagent", "status": "running" },
                { "type": "subagent", "id": "  ", "status": "running" },
                { "id": "a5", "status": "running" },
                "not an object",
            ] }),
        ))
        .unwrap();
    let task =
        |id: &str, agent_type: Option<&str>, description: Option<&str>, running, teammate| {
            BackgroundTask {
                id: id.to_owned(),
                agent_type: agent_type.map(str::to_owned),
                description: description.map(str::to_owned),
                running,
                teammate,
            }
        };
    assert_eq!(
        stop.background,
        Some(vec![
            task("a1", Some("Explore"), Some("find the parser"), true, false),
            task("a2", None, None, false, false),
            // A status never seen before is taken as still running.
            task("a3", None, None, true, false),
            task("t1", None, None, false, true),
            task("a4", None, None, true, false),
        ])
    );

    for (tasks, expected) in [
        (json!([]), Some(vec![])),
        (json!("none"), None),
        (json!(null), None),
    ] {
        let stop = hooks
            .claude(event("Stop", json!({ "background_tasks": tasks })))
            .unwrap();
        assert_eq!(stop.background, expected, "{tasks}");
    }
    assert_eq!(
        hooks.claude(event("Stop", json!({}))).unwrap().background,
        None
    );

    // Only a stop is read for them.
    let tool = hooks
        .claude(event(
            "PostToolUse",
            json!({ "background_tasks": [{ "type": "subagent", "id": "a1" }] }),
        ))
        .unwrap();
    assert_eq!(tool.background, None);
}

#[test]
fn a_subagents_report_says_which_one_it_is() {
    let hooks = Hooks::new();
    let report = hooks
        .claude(event(
            "PreToolUse",
            json!({ "agent_id": "abc", "agent_type": "Explore", "model": "gpt-5",
                    "tool_name": "Read", "tool_use_id": "call-9" }),
        ))
        .unwrap();
    assert_eq!(report.subagent.as_deref(), Some("abc"));
    assert_eq!(report.subagent_type.as_deref(), Some("Explore"));
    assert_eq!(report.subagent_model.as_deref(), Some("gpt-5"));
    assert_eq!(report.tool_use_id.as_deref(), Some("call-9"));

    // The lead sends its own type and model, and they are not a subagent's.
    let lead = hooks
        .claude(event(
            "PreToolUse",
            json!({ "agent_type": "main", "model": "opus", "tool_name": "Read" }),
        ))
        .unwrap();
    assert_eq!(lead.subagent, None);
    assert_eq!(lead.subagent_type, None);
    assert_eq!(lead.subagent_model, None);

    let teammate = hooks
        .claude(event(
            "TeammateIdle",
            json!({ "teammate_name": "reviewer" }),
        ))
        .unwrap();
    assert_eq!(teammate.event, HookEvent::TeammateIdle);
    assert_eq!(teammate.teammate.as_deref(), Some("reviewer"));
}

fn write_meta(path: &Path, meta: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, meta).unwrap();
}

#[test]
fn a_subagents_errand_is_read_from_beside_its_transcript() {
    let sandbox = Sandbox::new("agent-hooks-sidecar");
    let hooks = Hooks::new();
    let describe = |fields: Value| {
        hooks
            .claude(event("SubagentStart", fields))
            .unwrap()
            .subagent_description
    };

    // Beside the subagent's own transcript, when the payload names it.
    write_meta(
        &sandbox.path("sub/agent-x.meta.json"),
        r#"{ "description": "Survey the tests" }"#,
    );
    let own = sandbox.path("sub/agent-x.jsonl");
    assert_eq!(
        describe(json!({ "agent_id": "x", "agent_transcript_path": own })).as_deref(),
        Some("survey the tests")
    );

    // Otherwise under the session's.
    write_meta(
        &sandbox.path("session/subagents/agent-abc_1.meta.json"),
        r#"{ "description": "Read the relay" }"#,
    );
    let session = sandbox.path("session.jsonl");
    assert_eq!(
        describe(json!({ "agent_id": "abc_1", "transcript_path": session })).as_deref(),
        Some("read the relay")
    );

    // An id that would walk the path somewhere else is not followed.
    write_meta(
        &sandbox.path("session/subagents/agent-../evil.meta.json"),
        r#"{ "description": "Should not be read" }"#,
    );
    assert_eq!(
        describe(json!({ "agent_id": "../evil", "transcript_path": session })),
        None
    );

    // Anything missing or malformed is no description, never an error.
    write_meta(&sandbox.path("bad/agent-y.meta.json"), "not json");
    write_meta(
        &sandbox.path("blank/agent-z.meta.json"),
        r#"{ "description": "   " }"#,
    );
    for fields in [
        json!({ "agent_id": "nobody", "transcript_path": session }),
        json!({ "agent_id": "y", "agent_transcript_path": sandbox.path("bad/agent-y.jsonl") }),
        json!({ "agent_id": "z", "agent_transcript_path": sandbox.path("blank/agent-z.jsonl") }),
        json!({ "agent_id": "x", "agent_transcript_path": sandbox.path("sub/agent-x.txt") }),
        json!({ "agent_id": "x" }),
    ] {
        assert_eq!(describe(fields.clone()), None, "{fields}");
    }
}

// ---- grok -------------------------------------------------------------------

#[test]
fn grok_reads_as_claude_does() {
    let hooks = Hooks::new();
    let table = [
        (
            "run_terminal_command",
            json!({ "command": "cargo build" }),
            "Bash",
            Some("running cargo"),
        ),
        (
            "read_file",
            json!({ "target_file": "/a/main.rs" }),
            "Read",
            Some("reading main.rs"),
        ),
        (
            "write",
            json!({ "target_file": "/a/new.rs" }),
            "Write",
            Some("writing new.rs"),
        ),
        (
            "search_replace",
            json!({ "target_file": "/a/lib.rs" }),
            "Edit",
            Some("editing lib.rs"),
        ),
        ("grep", json!({ "pattern": "x" }), "Grep", Some("searching")),
        (
            "list_dir",
            json!({ "target_directory": "/a" }),
            "Glob",
            Some("searching"),
        ),
        (
            "spawn_subagent",
            json!({ "description": "Look around" }),
            "Task",
            Some("look around"),
        ),
        ("web_fetch", json!({}), "WebFetch", Some("reading the web")),
        (
            "web_search",
            json!({}),
            "WebSearch",
            Some("reading the web"),
        ),
        ("something_else", json!({}), "something_else", None),
    ];
    for (tool, input, claude, note) in table {
        let report = hooks.grok(pre_tool(tool, input)).expect("reported");
        assert_eq!(report.agent, "grok");
        assert_eq!(report.tool_name.as_deref(), Some(claude), "{tool}");
        assert_eq!(report.note.as_deref(), note, "{tool}");
    }

    // A field Grok also spells Claude's way keeps Claude's.
    let both = hooks
        .grok(pre_tool(
            "read_file",
            json!({ "target_file": "/a/grok.rs", "file_path": "/a/claude.rs" }),
        ))
        .unwrap();
    assert_eq!(both.note.as_deref(), Some("reading claude.rs"));

    let stop = hooks
        .grok(event(
            "Stop",
            json!({ "lastAssistantMessage": "Built it. Twice.",
                    "backgroundTasks": [{ "type": "subagent", "id": "g1",
                                          "agentType": "explorer", "status": "running" }] }),
        ))
        .unwrap();
    assert_eq!(stop.note.as_deref(), Some("Built it"));
    assert_eq!(
        stop.background.unwrap()[0].agent_type.as_deref(),
        Some("explorer")
    );

    let cancelled = hooks.grok(event("StopCancelled", json!({}))).unwrap();
    assert_eq!(cancelled.event, HookEvent::Interrupt);
}

#[test]
fn what_a_grok_subagent_does_is_not_the_panes() {
    let hooks = Hooks::new();
    let mut inside = pre_tool("run_terminal_command", json!({ "command": "ls" }));
    inside["subagentType"] = json!("explorer");
    assert_eq!(hooks.grok(inside), None);
}

#[test]
fn a_grok_permission_prompt_names_the_tool_it_announced() {
    let hooks = Hooks::new();
    let prompt = |session: &str| {
        hooks
            .grok(event(
                "Notification",
                json!({ "session_id": session, "notificationType": "permission_prompt" }),
            ))
            .expect("reported")
    };

    hooks
        .grok(pre_tool(
            "run_terminal_command",
            json!({ "command": "rm -rf build" }),
        ))
        .unwrap();
    let asked = prompt("s-1");
    assert_eq!(asked.event, HookEvent::PermissionRequest);
    assert_eq!(
        asked.question.as_deref(),
        Some(&PermissionQuestion {
            tool: "Bash".to_owned(),
            subject: Some("rm -rf build".to_owned()),
            id: 0,
            prompt: None,
        })
    );

    // Another session announced nothing: still a prompt, with no question.
    let unknown = prompt("s-2");
    assert_eq!(unknown.event, HookEvent::Notification);
    assert_eq!(unknown.question, None);

    // A session that has ended is forgotten.
    hooks.grok(event("SessionEnd", json!({}))).unwrap();
    assert_eq!(prompt("s-1").question, None);

    // Claude's prompts are never answered from Grok's memory.
    hooks
        .grok(pre_tool("run_terminal_command", json!({ "command": "ls" })))
        .unwrap();
    let claude = hooks
        .claude(event(
            "Notification",
            json!({ "notification_type": "permission_prompt" }),
        ))
        .unwrap();
    assert_eq!(claude.event, HookEvent::Notification);
    assert_eq!(claude.question, None);
}

#[test]
fn grok_remembers_a_bounded_number_of_sessions() {
    let hooks = Hooks::new();
    for n in 0..=64 {
        let announced = envelope(
            "grok",
            event(
                "PreToolUse",
                json!({ "session_id": format!("g-{n}"), "tool_name": "run_terminal_command",
                        "tool_input": { "command": format!("echo {n}") } }),
            ),
        );
        hooks.send(&request("/hook", &announced.to_string()));
    }
    assert_eq!(hooks.settle().len(), 65);
    let prompt = |session: &str| {
        hooks
            .grok(event(
                "Notification",
                json!({ "session_id": session, "notification_type": "permission_prompt" }),
            ))
            .unwrap()
            .question
    };
    assert_eq!(prompt("g-0"), None);
    assert_eq!(
        prompt("g-64").and_then(|q| q.subject).as_deref(),
        Some("echo 64")
    );
}

#[test]
fn grok_asking_a_question_is_waiting_on_a_person() {
    let hooks = Hooks::new();
    let asking = hooks
        .grok(pre_tool(
            "ask_user_question",
            json!({ "question": "Which?" }),
        ))
        .unwrap();
    assert_eq!(asking.event, HookEvent::Notification);
    assert_eq!(asking.event.state(), Some(AgentState::AwaitingPermission));

    // The same tool name from anything else is only a tool.
    let claude = hooks
        .claude(pre_tool("ask_user_question", json!({})))
        .unwrap();
    assert_eq!(claude.event, HookEvent::PreTool);
}

// ---- the status line --------------------------------------------------------

#[test]
fn a_status_line_is_kept_whole_and_apart_from_reports() {
    let hooks = Hooks::new();
    let payload = json!({
        "rate_limits": { "five_hour": { "used_percentage": 42.5 } },
        "cost": { "total_cost_usd": 1.25 },
    });
    hooks.send(&request(
        "/statusline",
        &envelope("claude", payload.clone()).to_string(),
    ));
    let mut bare = envelope("", payload.clone());
    bare["worktree"] = json!("");
    hooks.send(&request("/statusline", &bare.to_string()));

    assert!(hooks.settle().is_empty(), "not a report");
    let lines: Vec<StatusLine> = hooks.listener.drain_statuslines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].worktree.as_deref(), Some("wt-1"));
    assert_eq!(lines[0].pane.as_deref(), Some("pane-1"));
    assert_eq!(lines[0].agent.as_deref(), Some("claude"));
    assert_eq!(lines[0].payload, payload);
    assert_eq!(lines[1].worktree, None);
    assert_eq!(lines[1].agent, None);
    assert!(
        hooks.listener.drain_statuslines().is_empty(),
        "drained once"
    );
}

#[test]
fn mcp_tools_name_their_server() {
    let table = [
        ("mcp__github__create_issue", Some("github")),
        (
            "mcp__claude_ai_Claude_Docs__batch",
            Some("claude_ai_Claude_Docs"),
        ),
        ("mcp__server__", Some("server")),
        ("mcp____tool", None),
        ("mcp__noseparator", None),
        ("Bash", None),
        ("", None),
    ];
    for (tool, server) in table {
        assert_eq!(mcp_server(tool), server, "{tool}");
    }
}

// ---- installing into an agent -----------------------------------------------

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn write_json(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

/// The commands under one event, in order.
fn commands(document: &Value, event: &str) -> Vec<String> {
    document["hooks"][event]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .flat_map(|entry| entry["hooks"].as_array().cloned().unwrap_or_default())
                .filter_map(|hook| hook["command"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn script() -> PathBuf {
    PathBuf::from("/Users/someone/.ket/agent-hooks/ket-agent-hook.sh")
}

/// A settings file holding a person's own hooks and settings, plus an event
/// an older ket asked for and this one does not.
fn lived_in() -> Value {
    json!({
        "model": "opus",
        "permissions": { "allow": ["Bash(ls:*)"] },
        "hooks": {
            "PreToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "~/bin/guard.sh" }] },
            ],
            "Stop": [
                { "hooks": [{ "type": "command", "command": "say done" }] },
                { "hooks": [{ "type": "command",
                              "command": "/bin/sh /old/ket-agent-hook.sh 2>/dev/null || printf '{}\\n'" }] },
            ],
            "FileChanged": [
                { "hooks": [{ "type": "command", "command": "/bin/sh /old/ket-agent-hook.sh" }] },
            ],
            "MessageDisplay": [
                { "hooks": [{ "type": "command", "command": "/bin/sh /old/ket-agent-hook.sh" }] },
                { "hooks": [{ "type": "command", "command": "~/bin/log.sh" }] },
            ],
        },
    })
}

#[test]
fn claude_hooks_install_every_event_into_an_empty_home() {
    let sandbox = Sandbox::new("agent-hooks-claude-empty");
    let settings = sandbox.path("claude/settings.json");
    let hooks = ClaudeHooks::at(settings.clone());
    assert_eq!(hooks.agent(), "claude");
    assert!(!hooks.is_installed().unwrap());

    hooks.install(&script()).unwrap();
    assert!(hooks.is_installed().unwrap());
    // No backup of a file that was not there.
    assert!(!sandbox.path("claude/settings.json.ket-backup").exists());

    let document = read_json(&settings);
    let events = document["hooks"].as_object().unwrap();
    assert_eq!(events.len(), HookEvent::CLAUDE_EVENTS.len());
    for event in HookEvent::CLAUDE_EVENTS {
        let entries = events[event].as_array().unwrap();
        assert_eq!(entries.len(), 1, "{event}");
        let entry = entries[0].as_object().unwrap();
        // Claude Code throws out an entry with a matcher it does not take,
        // or with a key it does not know.
        let takes_matcher = matches!(
            event,
            "PreToolUse"
                | "PostToolUse"
                | "PostToolUseFailure"
                | "PermissionRequest"
                | "PermissionDenied"
        );
        assert_eq!(
            entry.get("matcher").cloned(),
            takes_matcher.then(|| json!("*")),
            "{event}"
        );
        assert_eq!(entry.len(), if takes_matcher { 2 } else { 1 }, "{event}");
        let mut hook = json!({
            "type": "command",
            "command": format!("/bin/sh {} 2>/dev/null || printf '{{}}\\n'", script().display()),
        });
        // A question or a plan waits on its hook for a phone's answer.
        if event == "PermissionRequest" {
            hook["timeout"] = json!(agent_hooks::PROMPT_WAIT_SECS);
        }
        assert_eq!(entry["hooks"], json!([hook]), "{event}");
    }
}

#[test]
fn claude_hooks_leave_the_persons_settings_as_they_were() {
    let sandbox = Sandbox::new("agent-hooks-claude-merge");
    let settings = sandbox.path("settings.json");
    write_json(&settings, &lived_in());
    let hooks = ClaudeHooks::at(settings.clone());
    assert!(
        !hooks.is_installed().unwrap(),
        "an older ket's set is not this one's"
    );

    hooks.install(&script()).unwrap();
    hooks.install(&script()).unwrap();
    assert!(hooks.is_installed().unwrap());

    let document = read_json(&settings);
    assert_eq!(document["model"], "opus");
    assert_eq!(document["permissions"], lived_in()["permissions"]);
    // Theirs first and untouched; one of ket's, however often it installs.
    assert_eq!(
        document["hooks"]["PreToolUse"][0],
        lived_in()["hooks"]["PreToolUse"][0]
    );
    assert_eq!(commands(&document, "PreToolUse").len(), 2);
    let stop = commands(&document, "Stop");
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0], "say done");
    assert!(
        stop[1].contains(&script().display().to_string()),
        "{stop:?}"
    );
    // An event ket no longer asks for loses ket's entry, and goes if that
    // leaves it empty.
    assert!(document["hooks"].get("FileChanged").is_none());
    assert_eq!(commands(&document, "MessageDisplay"), vec!["~/bin/log.sh"]);

    // What was there before the last install is kept beside it.
    let backup = read_json(&sandbox.path("settings.json.ket-backup"));
    assert_eq!(backup["model"], "opus");

    hooks.remove().unwrap();
    assert!(!hooks.is_installed().unwrap());
    let removed = read_json(&settings);
    let mut expected = lived_in();
    let events = expected["hooks"].as_object_mut().unwrap();
    events.remove("FileChanged");
    events["Stop"].as_array_mut().unwrap().pop();
    events["MessageDisplay"].as_array_mut().unwrap().remove(0);
    assert_eq!(removed, expected);
}

#[test]
fn claude_hooks_find_an_older_kets_marked_entries() {
    let sandbox = Sandbox::new("agent-hooks-claude-mark");
    let settings = sandbox.path("settings.json");
    write_json(
        &settings,
        &json!({ "hooks": { "Stop": [
            { "ket-agent-hook": true, "hooks": [{ "type": "command", "command": "/somewhere/else.sh" }] },
            { "hooks": [{ "type": "command", "command": "say done" }] },
        ] } }),
    );
    let hooks = ClaudeHooks::at(settings.clone());
    hooks.remove().unwrap();
    assert_eq!(commands(&read_json(&settings), "Stop"), vec!["say done"]);
}

#[test]
fn claude_hooks_notice_an_event_missing_from_the_set() {
    let sandbox = Sandbox::new("agent-hooks-claude-partial");
    let settings = sandbox.path("settings.json");
    let hooks = ClaudeHooks::at(settings.clone());
    hooks.install(&script()).unwrap();

    let mut document = read_json(&settings);
    document["hooks"]
        .as_object_mut()
        .unwrap()
        .remove("PermissionRequest");
    write_json(&settings, &document);
    assert!(!hooks.is_installed().unwrap());

    hooks.install(&script()).unwrap();
    assert!(hooks.is_installed().unwrap());
}

#[test]
fn claude_hooks_refuse_a_settings_file_they_cannot_merge_into() {
    let sandbox = Sandbox::new("agent-hooks-claude-bad");
    let settings = sandbox.path("settings.json");
    let hooks = ClaudeHooks::at(settings.clone());
    for text in [
        "{ not json",
        "[1, 2]",
        r#"{ "hooks": [] }"#,
        r#"{ "hooks": { "Stop": { "command": "x" } } }"#,
    ] {
        std::fs::write(&settings, text).unwrap();
        assert!(hooks.install(&script()).is_err(), "{text}");
        // Refused, not rewritten.
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), text);
    }
    std::fs::write(&settings, "{ not json").unwrap();
    assert!(hooks.is_installed().is_err());
    assert!(hooks.remove().is_err());
}

#[test]
fn removing_hooks_that_were_never_installed_writes_nothing() {
    let sandbox = Sandbox::new("agent-hooks-remove-none");
    let claude = sandbox.path("claude.json");
    let codex = sandbox.path("codex.json");
    ClaudeHooks::at(claude.clone()).remove().unwrap();
    CodexHooks::at(codex.clone()).remove().unwrap();
    assert!(!claude.exists());
    assert!(!codex.exists());
}

#[test]
fn codex_hooks_install_without_matchers_and_leave_others_alone() {
    let sandbox = Sandbox::new("agent-hooks-codex");
    let file = sandbox.path("codex/hooks.json");
    write_json(&file, &lived_in());
    let hooks = CodexHooks::at(file.clone());
    assert_eq!(hooks.agent(), "codex");
    assert!(!hooks.is_installed().unwrap());

    hooks.install(&script()).unwrap();
    hooks.install(&script()).unwrap();
    assert!(hooks.is_installed().unwrap());

    let document = read_json(&file);
    for event in HookEvent::CODEX_EVENTS {
        let ours: Vec<&Value> = document["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry.to_string().contains("ket-agent-hook"))
            .collect();
        assert_eq!(ours.len(), 1, "{event}");
        assert_eq!(
            ours[0].as_object().unwrap().len(),
            1,
            "no matcher on {event}"
        );
    }
    // Claude's events that Codex does not have are not registered.
    assert!(document["hooks"].get("Notification").is_none());
    assert!(document["hooks"].get("TeammateIdle").is_none());
    assert_eq!(
        document["hooks"]["PreToolUse"][0],
        lived_in()["hooks"]["PreToolUse"][0]
    );
    assert_eq!(commands(&document, "Stop")[0], "say done");
    assert!(document["hooks"].get("FileChanged").is_none());

    hooks.remove().unwrap();
    let removed = read_json(&file);
    assert!(!removed.to_string().contains("ket-agent-hook"));
    assert_eq!(commands(&removed, "Stop"), vec!["say done"]);
    assert_eq!(removed["model"], "opus");
}

#[test]
fn the_opencode_plugin_is_a_file_of_kets_own() {
    let sandbox = Sandbox::new("agent-hooks-opencode");
    let plugin = sandbox.path("opencode/plugin/ket.js");
    let hooks = OpenCodeHooks::at(plugin.clone());
    assert_eq!(hooks.agent(), "opencode");
    assert!(!hooks.is_installed().unwrap());

    hooks.install(&script()).unwrap();
    assert!(hooks.is_installed().unwrap());
    let text = std::fs::read_to_string(&plugin).unwrap();
    assert!(text.starts_with("// ket-opencode-plugin"));
    assert!(
        text.contains("KET_LAUNCH_TOKEN"),
        "inert without ket's token"
    );

    // An older plugin is not this one.
    std::fs::write(&plugin, text.replace("chat.message", "chat.old")).unwrap();
    assert!(!hooks.is_installed().unwrap());

    hooks.remove().unwrap();
    assert!(!plugin.exists());
    hooks.remove().unwrap();

    // A file of the person's at that path is theirs.
    std::fs::write(&plugin, "export default {};\n").unwrap();
    hooks.remove().unwrap();
    assert_eq!(
        std::fs::read_to_string(&plugin).unwrap(),
        "export default {};\n"
    );
}

#[test]
fn grok_hooks_are_written_only_where_grok_is() {
    let sandbox = Sandbox::new("agent-hooks-grok");
    let absent = GrokHooks::at(sandbox.path("no-grok"));
    assert_eq!(absent.agent(), "grok");
    // Nothing to install for a Grok that is not here.
    assert!(absent.is_installed().unwrap());
    absent.install(&script_path()).unwrap();
    assert!(!sandbox.path("no-grok").exists());

    let home = sandbox.path("grok");
    std::fs::create_dir_all(&home).unwrap();
    let hooks = GrokHooks::at(home.clone());
    assert!(!hooks.is_installed().unwrap());
    hooks.install(&script_path()).unwrap();
    assert!(hooks.is_installed().unwrap());

    let file = home.join("hooks/ket.json");
    let text = std::fs::read_to_string(&file).unwrap();
    // Grok expands `$` in a command itself, and will not run one whose
    // variables are unset.
    let document: Value = serde_json::from_str(&text).unwrap();
    let events = document["hooks"].as_object().unwrap();
    assert_eq!(events.len(), HookEvent::GROK_EVENTS.len());
    for event in HookEvent::GROK_EVENTS {
        let command = &commands(&document, event)[0];
        assert!(!command.contains('$'), "{command}");
        assert!(
            command.contains(&format!("{} grok", script_path().display())),
            "{command}"
        );
    }

    // Written for another script, it is out of date.
    hooks.install(&script()).unwrap();
    assert!(!hooks.is_installed().unwrap());

    hooks.remove().unwrap();
    assert!(!file.exists());

    // A hooks file of the person's own is left where it is.
    std::fs::write(&file, r#"{ "hooks": {} }"#).unwrap();
    hooks.remove().unwrap();
    assert!(file.exists());
}

#[test]
fn the_status_line_keeps_the_command_it_displaced_and_gives_it_back() {
    let sandbox = Sandbox::new("agent-hooks-statusline");
    let settings = sandbox.path("settings.json");
    let chain = sandbox.path("hooks/statusline-chain");
    let theirs = "npx ccstatusline@latest";
    write_json(
        &settings,
        &json!({ "model": "opus", "statusLine": { "type": "command", "command": theirs, "padding": 0 } }),
    );
    let line = ClaudeStatusLine::at(settings.clone(), chain.clone());
    assert!(!line.is_installed().unwrap());

    let ours = sandbox.path("ket-statusline.sh");
    line.install(&ours).unwrap();
    assert!(line.is_installed().unwrap());
    assert_eq!(std::fs::read_to_string(&chain).unwrap(), theirs);
    let document = read_json(&settings);
    assert_eq!(document["model"], "opus");
    assert_eq!(
        document["statusLine"]["command"],
        format!("/bin/sh {} 2>/dev/null", ours.display())
    );

    // Installing over itself must not chain to itself.
    line.install(&ours).unwrap();
    assert_eq!(std::fs::read_to_string(&chain).unwrap(), theirs);

    line.remove().unwrap();
    assert!(!line.is_installed().unwrap());
    assert!(!chain.exists());
    let restored = read_json(&settings);
    assert_eq!(restored["statusLine"]["command"], theirs);
    assert_eq!(restored["model"], "opus");

    // Removing again, with it no longer ket's, touches nothing.
    line.remove().unwrap();
    assert_eq!(read_json(&settings), restored);
}

#[test]
fn a_status_line_that_displaced_nothing_leaves_none_behind() {
    let sandbox = Sandbox::new("agent-hooks-statusline-none");
    let settings = sandbox.path("settings.json");
    let chain = sandbox.path("statusline-chain");
    // A chain left over from an install whose command has since gone.
    std::fs::write(&chain, "old-command").unwrap();
    write_json(&settings, &json!({ "model": "opus" }));

    let line = ClaudeStatusLine::at(settings.clone(), chain.clone());
    line.install(&sandbox.path("ket-statusline.sh")).unwrap();
    assert!(!chain.exists());

    line.remove().unwrap();
    assert_eq!(read_json(&settings), json!({ "model": "opus" }));
}
