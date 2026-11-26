//! The scripts agents run, run the way they run them: `/bin/sh`, a
//! payload on stdin, ket's variables in an otherwise empty environment,
//! and a real listener at the other end. The listener and installers are covered in `tests/agent_hooks.rs`.

use super::decode::{GrokTools, command_output, decode_envelope, response_bytes};
use super::listener::Post;
use super::script::SCRIPT;
use super::statusline::STATUSLINE_SCRIPT;
use super::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---- measuring a command's output ----------------------------------------

#[test]
fn a_program_is_keyed_with_its_subcommand_only_where_that_names_the_job() {
    assert_eq!(command_program("cargo test --lib foo"), "cargo test");
    assert_eq!(command_program("git -C /x status --short"), "git status");
    assert_eq!(
        command_program("FOO=1 /usr/local/bin/pnpm install"),
        "pnpm install"
    );
    assert_eq!(command_program("ls -la /some/secret/path"), "ls");
    assert_eq!(command_program("git"), "git");
}

#[test]
fn response_bytes_counts_every_string_whatever_the_shape() {
    assert_eq!(response_bytes(&serde_json::json!("abcd")), 4);
    assert_eq!(
        response_bytes(&serde_json::json!({"stdout": "abc", "stderr": "de", "exit_code": 0})),
        5
    );
    assert_eq!(response_bytes(&serde_json::json!([{"text": "ab"}, "c"])), 3);
}

#[test]
fn a_finished_shell_command_is_measured_and_a_background_one_is_not() {
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "cargo check"},
        "tool_response": {"stdout": "x".repeat(400), "stderr": "", "exit_code": 0},
    });
    let envelope = serde_json::json!({"shown": 100});
    let output = command_output(HookEvent::PostTool, &payload, &envelope).expect("measured");
    assert_eq!(output.program, "cargo check");
    assert_eq!(output.raw_bytes, 400);
    assert_eq!(output.shown_bytes, 100);
    assert!(!output.failed);

    let failed = command_output(HookEvent::PostToolFailure, &payload, &serde_json::json!({}))
        .expect("measured");
    assert!(failed.failed);
    assert_eq!(failed.shown_bytes, 400);

    let mut background = payload.clone();
    background["tool_input"]["run_in_background"] = serde_json::json!(true);
    assert!(command_output(HookEvent::PostTool, &background, &envelope).is_none());
    assert!(command_output(HookEvent::PreTool, &payload, &envelope).is_none());
}

#[test]
fn a_plugin_that_sends_only_a_size_is_measured_by_it() {
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
        "tool_response_bytes": 1234,
    });
    let output =
        command_output(HookEvent::PostTool, &payload, &serde_json::json!({})).expect("measured");
    assert_eq!(output.raw_bytes, 1234);
    assert_eq!(output.shown_bytes, 1234);
}

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

    fn spool_text(&self) -> String {
        std::fs::read_to_string(self.hooks().join("spool.jsonl")).unwrap_or_default()
    }

    fn spool(&self) -> Vec<serde_json::Value> {
        self.spool_text()
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
fn run(script: &str, args: &[&str], home: &Home, env: &[(&str, &str)], payload: &str) -> String {
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

/// A finished shell command, compact the way Claude Code sends it.
const COMMAND_DONE: &str = r#"{"hook_event_name":"PostToolUse","session_id":"s-1","tool_use_id":"toolu_01","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_response":{"stdout":"test a ... ok\n","stderr":"","interrupted":false}}"#;

/// A stand-in for `ket --filter-hook`: prints `answer` as the byte count
/// and the decision, or nothing. Executable, as the script requires.
fn fake_filter(home: &Home, answer: &str) -> String {
    let path = home.0.join("fake-ket");
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{answer}'\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.display().to_string()
}

const DECISION: &str = r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":{"stdout":"1 passed"}}}"#;

#[test]
fn the_script_answers_with_the_filters_decision_and_reports_how_much_was_shown() {
    let home = Home::new("filter-on");
    let port = closed_port();
    let filter = fake_filter(&home, &format!("8\n{DECISION}\n"));
    let env = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::filter::ENV, filter.as_str()),
    ];
    let printed = run(SCRIPT, &[], &home, &env, COMMAND_DONE);
    assert_eq!(printed, format!("{DECISION}\n"));
    let spooled = home.spool();
    assert_eq!(spooled.len(), 1);
    assert_eq!(spooled[0]["shown"], 8);
}

#[test]
fn without_a_filter_the_script_says_nothing_and_shown_is_null() {
    let home = Home::new("filter-off");
    let port = closed_port();
    let env = [(PORT_ENV, port.as_str()), (TOKEN_ENV, "window")];
    assert_eq!(run(SCRIPT, &[], &home, &env, COMMAND_DONE), "{}\n");
    assert_eq!(home.spool()[0]["shown"], serde_json::Value::Null);
}

#[test]
fn a_filter_with_nothing_to_say_leaves_the_output_alone() {
    let home = Home::new("filter-silent");
    let port = closed_port();
    let filter = fake_filter(&home, "");
    let env = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::filter::ENV, filter.as_str()),
    ];
    assert_eq!(run(SCRIPT, &[], &home, &env, COMMAND_DONE), "{}\n");
    assert_eq!(home.spool()[0]["shown"], serde_json::Value::Null);
}

#[test]
fn the_filter_is_asked_only_for_claude_and_only_for_a_finished_shell_command() {
    let home = Home::new("filter-scope");
    let port = closed_port();
    let filter = fake_filter(&home, &format!("8\n{DECISION}\n"));
    let claude = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::filter::ENV, filter.as_str()),
    ];
    // Another agent: the branch is Claude's hook contract.
    let codex = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::filter::ENV, filter.as_str()),
        ("KET_AGENT_NAME", "codex"),
    ];
    assert_eq!(run(SCRIPT, &[], &home, &codex, COMMAND_DONE), "{}\n");
    // Another tool, and another event, with the filter set.
    let read = COMMAND_DONE.replace(r#""tool_name":"Bash""#, r#""tool_name":"Read""#);
    assert_eq!(run(SCRIPT, &[], &home, &claude, &read), "{}\n");
    let pre = COMMAND_DONE.replace("PostToolUse", "PreToolUse");
    assert_eq!(run(SCRIPT, &[], &home, &claude, &pre), "{}\n");
    // A binary that is not there.
    let missing = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::filter::ENV, "/nonexistent/ket"),
    ];
    assert_eq!(run(SCRIPT, &[], &home, &missing, COMMAND_DONE), "{}\n");
}

#[test]
fn an_envelope_carries_what_the_model_was_shown_into_the_report() {
    let payload: serde_json::Value = serde_json::from_str(COMMAND_DONE).unwrap();
    let envelope = serde_json::json!({
        "agent": "claude", "worktree": "wt-1", "pane": "pane-1", "token": "t",
        "shown": 8, "payload": payload,
    });
    let Some(Post::Report(report)) = decode_envelope(&envelope, false, &mut GrokTools::new())
    else {
        panic!("a report");
    };
    let command = report.command.expect("measured");
    assert_eq!(command.program, "cargo test");
    assert_eq!(command.raw_bytes, 14);
    assert_eq!(command.shown_bytes, 8);
    assert!(!command.failed);
    // `null`, the script's word for "not filtered", means the whole
    // thing was shown.
    let mut plain = envelope.clone();
    plain["shown"] = serde_json::Value::Null;
    let Some(Post::Report(report)) = decode_envelope(&plain, false, &mut GrokTools::new()) else {
        panic!("a report");
    };
    assert_eq!(report.command.unwrap().shown_bytes, 14);
}

/// A shell call about to run, compact the way Claude Code sends it.
const COMMAND_ASKED: &str = r#"{"hook_event_name":"PreToolUse","session_id":"s-1","tool_use_id":"toolu_02","tool_name":"Bash","tool_input":{"command":"git push --force"}}"#;

const RULED_DENY: &str = r#"{"verdict":"deny","scope":"global","index":0}"#;
const DENIED: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"ket: global rule 1"}}"#;

#[test]
fn the_script_answers_with_a_rules_verdict_and_reports_the_ruling() {
    let home = Home::new("rules-on");
    let port = closed_port();
    let rules = fake_filter(&home, &format!("{RULED_DENY}\n{DENIED}\n"));
    let env = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::permissions::ENV, rules.as_str()),
    ];
    assert_eq!(
        run(SCRIPT, &[], &home, &env, COMMAND_ASKED),
        format!("{DENIED}\n")
    );
    assert_eq!(home.spool()[0]["ruling"]["verdict"], "deny");
    // A `PermissionRequest` gets the same treatment.
    let asked = COMMAND_ASKED.replace("PreToolUse", "PermissionRequest");
    assert_eq!(run(SCRIPT, &[], &home, &env, &asked), format!("{DENIED}\n"));
}

#[test]
fn without_a_ruling_the_script_says_nothing_and_ruling_is_null() {
    let home = Home::new("rules-silent");
    let port = closed_port();
    let silent = fake_filter(&home, "");
    let env = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::permissions::ENV, silent.as_str()),
    ];
    assert_eq!(run(SCRIPT, &[], &home, &env, COMMAND_ASKED), "{}\n");
    assert_eq!(home.spool()[0]["ruling"], serde_json::Value::Null);
    // No binary at all, and one that is not there, are the same.
    let unset = [(PORT_ENV, port.as_str()), (TOKEN_ENV, "window")];
    assert_eq!(run(SCRIPT, &[], &home, &unset, COMMAND_ASKED), "{}\n");
    let missing = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::permissions::ENV, "/nonexistent/ket"),
    ];
    assert_eq!(run(SCRIPT, &[], &home, &missing, COMMAND_ASKED), "{}\n");
}

#[test]
fn rules_are_asked_only_for_claude_and_never_for_a_question_or_a_plan() {
    let home = Home::new("rules-scope");
    let port = closed_port();
    let rules = fake_filter(&home, &format!("{RULED_DENY}\n{DENIED}\n"));
    let codex = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::permissions::ENV, rules.as_str()),
        ("KET_AGENT_NAME", "codex"),
    ];
    assert_eq!(run(SCRIPT, &[], &home, &codex, COMMAND_ASKED), "{}\n");
    let claude = [
        (PORT_ENV, port.as_str()),
        (TOKEN_ENV, "window"),
        (crate::permissions::ENV, rules.as_str()),
    ];
    // Claude's own; Codex's question tool never reaches a rule.
    for tool in ["AskUserQuestion", "ExitPlanMode"] {
        let prompt =
            COMMAND_ASKED.replace(r#""tool_name":"Bash""#, &format!(r#""tool_name":"{tool}""#));
        assert_eq!(run(SCRIPT, &[], &home, &claude, &prompt), "{}\n", "{tool}");
    }
    // Another event: a finished call is not ruled.
    assert_eq!(run(SCRIPT, &[], &home, &claude, COMMAND_DONE), "{}\n");
}

/// `envelope` with `ruling` set to `verdict`, around `payload`.
fn ruled(payload: &str, verdict: &str) -> serde_json::Value {
    let payload: serde_json::Value = serde_json::from_str(payload).unwrap();
    serde_json::json!({
        "agent": "claude", "worktree": "wt-1", "pane": "pane-1", "token": "t",
        "ruling": { "verdict": verdict, "scope": "global", "index": 0 },
        "payload": payload,
    })
}

fn decoded(envelope: &serde_json::Value) -> HookReport {
    match decode_envelope(envelope, false, &mut GrokTools::new()) {
        Some(Post::Report(report)) => *report,
        _ => panic!("a report"),
    }
}

#[test]
fn a_prompt_a_rule_allowed_arrives_as_the_call_going_ahead() {
    let asked = COMMAND_ASKED.replace("PreToolUse", "PermissionRequest");
    let report = decoded(&ruled(&asked, "allow"));
    assert_eq!(report.event, HookEvent::PreTool);
    assert!(report.question.is_none(), "nothing left to ask");
    assert!(report.call.is_some());
    assert_eq!(
        report.ruling.map(|ruling| ruling.verdict),
        Some(crate::permissions::Verdict::Allow)
    );
}

#[test]
fn a_call_a_rule_refused_arrives_as_a_refusal() {
    assert_eq!(
        decoded(&ruled(COMMAND_ASKED, "deny")).event,
        HookEvent::PermissionDenied
    );
    let asked = COMMAND_ASKED.replace("PreToolUse", "PermissionRequest");
    assert_eq!(
        decoded(&ruled(&asked, "deny")).event,
        HookEvent::PermissionDenied
    );
}

#[test]
fn an_ask_or_no_ruling_leaves_the_event_as_it_was() {
    let asked = COMMAND_ASKED.replace("PreToolUse", "PermissionRequest");
    let report = decoded(&ruled(&asked, "ask"));
    assert_eq!(report.event, HookEvent::PermissionRequest);
    assert!(report.question.is_some());
    assert_eq!(
        decoded(&ruled(COMMAND_ASKED, "ask")).event,
        HookEvent::PreTool
    );
    let mut plain = ruled(&asked, "allow");
    plain["ruling"] = serde_json::Value::Null;
    assert_eq!(decoded(&plain).event, HookEvent::PermissionRequest);
}

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
fn only_codex_spools_a_report_from_an_agent_ket_did_not_launch() {
    // Nothing of ket's in the environment: Claude in a plain terminal.
    // Reading the spool drops a report with no worktree unless it is
    // Codex's, which is placed by its directory.
    let home = Home::new("spool-unlaunched");
    run(SCRIPT, &[], &home, &[], PROMPT);
    run(
        SCRIPT,
        &["grok"],
        &home,
        &[("GROK_HOOK_EVENT", "x")],
        PROMPT,
    );
    assert!(home.spool().is_empty(), "{:?}", home.spool());

    run(SCRIPT, &[], &home, &[("KET_AGENT_NAME", "codex")], PROMPT);
    let spooled = home.spool();
    assert_eq!(spooled.len(), 1);
    assert_eq!(spooled[0]["agent"], "codex");
    assert_eq!(reports_from_spool(&home.spool_text()).len(), 1);
}

#[test]
fn the_spool_stops_growing_at_its_cap() {
    let home = Home::new("spool-cap");
    let port = closed_port();
    let env = [(PORT_ENV, port.as_str()), (TOKEN_ENV, "window")];
    std::fs::create_dir_all(home.hooks()).unwrap();
    let spool = home.hooks().join("spool.jsonl");

    let file = std::fs::File::create(&spool).unwrap();
    file.set_len(8 * 1024 * 1024 - 1).unwrap();
    assert_eq!(run(SCRIPT, &[], &home, &env, PROMPT), "{}\n");
    let under = std::fs::metadata(&spool).unwrap().len();
    assert!(under > 8 * 1024 * 1024, "under the cap it is written");

    // At or past the cap, nothing more, and the hook still answers.
    assert_eq!(run(SCRIPT, &[], &home, &env, PROMPT), "{}\n");
    assert_eq!(std::fs::metadata(&spool).unwrap().len(), under);
}

#[test]
fn codex_hook_identifies_itself_in_a_plain_terminal() {
    let home = Home::new("codex-plain-terminal");
    let listener = Listener::start("window".to_owned()).unwrap();
    let port = listener.port().to_string();
    run(
        SCRIPT,
        &["codex"],
        &home,
        &[
            (PORT_ENV, &port),
            (TOKEN_ENV, "window"),
            (WORKTREE_ENV, "wt-1"),
            (PANE_ENV, "pane-1"),
            ("KET_AGENT_NAME", "claude"),
        ],
        PROMPT,
    );
    assert_eq!(reports(&listener, 1)[0].agent, "codex");
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
