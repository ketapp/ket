//! The mode an agent is in, read and switched the way a phone's mode pill
//! asks: Claude's from the `permission-mode` records in its transcript,
//! OpenCode's from its session database, Codex's never.
//!
//! Reading a mode looks where the agent keeps it — `~/.claude*/projects`,
//! `~/.local/share/opencode/opencode.db` — so each test that reads one runs
//! again in a child process whose home and data directories are throwaway
//! ones (see [`sandboxed`]): a test cannot point its own process elsewhere,
//! and the person running them has real transcripts there.
//!
//! Switching is driven through a real pty at a stand-in agent: a `sh` script
//! that waits for the agent's key and records the next mode where the real
//! agent would, so the press-and-read loop is checked against the same files
//! it reads in use.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::Sandbox;
use ket_core::agent_mode::{Mode, Modes, claude_mode, modes, opencode_agent, set};
use ket_core::terminal::{LocalTerminal, TerminalSize, TerminalSpec};

/// Set in the child a sandboxed test runs in, to its throwaway home.
const SANDBOXED: &str = "KET_AGENT_MODE_TEST_HOME";

/// Longest any one wait goes before the test calls it stuck.
const WAIT: Duration = Duration::from_secs(10);

/// In the parent: runs test `name` again in a child whose home, data,
/// config and cache directories are throwaway ones, checks it passed, and
/// returns `None`. In that child: the throwaway home, and the test goes on.
fn sandboxed(name: &str) -> Option<PathBuf> {
    if let Some(home) = std::env::var_os(SANDBOXED) {
        return Some(PathBuf::from(home));
    }
    let sandbox = Sandbox::new(&format!("agent-mode-{name}"));
    let home = sandbox.path("home");
    fs::create_dir_all(&home).expect("a home directory");
    let mut child = Command::new(std::env::current_exe().expect("this test binary"));
    child
        .arg(name)
        .args(["--exact", "--test-threads=1", "--nocapture"])
        .env(SANDBOXED, &home)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", sandbox.path("data"))
        .env("XDG_CONFIG_HOME", sandbox.path("config"))
        .env("XDG_CACHE_HOME", sandbox.path("cache"))
        // Claude's own pointer at the person's account; the child must find
        // only the transcripts the test writes.
        .env_remove("CLAUDE_CONFIG_DIR");
    // A test run from inside a ket pane must not look like that pane.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("KET_") {
            child.env_remove(key);
        }
    }
    let out = child.output().expect("the test runs again");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    None
}

/// A Claude transcript for `session` under `home`'s `.claude`, holding
/// `lines`.
fn transcript(home: &Path, session: &str, lines: &[String]) -> PathBuf {
    let dir = home.join(".claude").join("projects").join("-tmp-project");
    fs::create_dir_all(&dir).expect("a projects directory");
    let path = dir.join(format!("{session}.jsonl"));
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(&path, body).expect("write the transcript");
    path
}

/// The record Claude writes each time its permission mode changes.
fn mode_record(session: &str, mode: &str) -> String {
    format!(r#"{{"type":"permission-mode","permissionMode":"{mode}","sessionId":"{session}"}}"#)
}

/// A turn the person typed, as Claude records it.
fn user_turn(session: &str, text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "sessionId": session,
        "message": { "role": "user", "content": text },
    })
    .to_string()
}

/// OpenCode's database under `home`, with `sessions` as `(id, agent)`.
fn opencode_database(home: &Path, sessions: &[(&str, &str)]) -> PathBuf {
    let dir = home.join(".local").join("share").join("opencode");
    fs::create_dir_all(&dir).expect("an opencode directory");
    let path = dir.join("opencode.db");
    let mut sql = "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, agent TEXT);".to_owned();
    for (id, agent) in sessions {
        sql.push_str(&format!(
            "INSERT INTO session_v2 VALUES ('{id}', '{agent}');"
        ));
    }
    sqlite(&path, &sql);
    path
}

fn sqlite(database: &Path, sql: &str) {
    let out = Command::new("sqlite3")
        .arg(database)
        .arg(sql)
        .output()
        .expect("sqlite3 runs");
    assert!(
        out.status.success(),
        "sqlite3: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A stand-in agent in a real pty: `script` under `sh`, with `env`, once it
/// has touched `$READY` — the sign its terminal is raw and listening.
fn agent(dir: &Path, script: &str, env: &[(&str, &str)]) -> LocalTerminal {
    let ready = dir.join("ready");
    let _ = fs::remove_file(&ready);
    let mut vars: BTreeMap<String, String> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    vars.insert("READY".to_owned(), ready.display().to_string());
    let terminal = LocalTerminal::open(&TerminalSpec {
        command: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            format!("stty raw -echo; : > \"$READY\"; {script}"),
        ],
        cwd: dir.to_path_buf(),
        env: vars,
        env_remove: Vec::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
        adopt_only: false,
    })
    .expect("the stand-in agent starts");
    assert!(
        wait_until(|| ready.exists()),
        "the stand-in agent never got ready"
    );
    terminal
}

/// A stand-in Claude: each Shift+Tab records the next of `$MODES` to
/// `$TRANSCRIPT`, as Claude does when it cycles. Any other key does nothing.
const CLAUDE: &str = r#"set -- $MODES
shift_tab=$(printf '\033[Z')
while [ $# -gt 0 ]; do
  key=$(head -c 3)
  [ -n "$key" ] || exit
  [ "$key" = "$shift_tab" ] || continue
  printf '{"type":"permission-mode","permissionMode":"%s","sessionId":"%s"}\n' "$1" "$SESSION" >> "$TRANSCRIPT"
  shift
done
cat > /dev/null"#;

/// A stand-in OpenCode: each Tab sets the session's agent in `$DB` to the
/// next of `$AGENTS`. Any other key does nothing.
const OPENCODE: &str = r#"set -- $AGENTS
tab=$(printf '\t')
while [ $# -gt 0 ]; do
  key=$(head -c 1)
  [ -n "$key" ] || exit
  [ "$key" = "$tab" ] || continue
  sqlite3 "$DB" "UPDATE session_v2 SET agent = '$1' WHERE id = '$SESSION';"
  shift
done
cat > /dev/null"#;

/// A stand-in agent that hears every key and does nothing with it.
const DEAF: &str = "cat > /dev/null";

fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    ready()
}

fn ids(modes: &Modes) -> Vec<&str> {
    modes
        .choices
        .iter()
        .map(|choice| choice.id.as_str())
        .collect()
}

/// The `permission-mode` records in the transcript at `path`, in order.
fn recorded(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("read the transcript")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| record["type"] == "permission-mode")
        .filter_map(|record| record["permissionMode"].as_str().map(str::to_owned))
        .collect()
}

// ---- modes, without a session -------------------------------------------------

#[test]
fn claude_with_no_session_offers_its_cycle_and_knows_no_mode() {
    let offered = modes("claude", None).expect("Claude has modes");
    assert_eq!(offered.current, "");
    assert_eq!(
        offered.choices,
        vec![
            Mode {
                id: "default".to_owned(),
                label: "Default".to_owned(),
                detail: "Asks before edits and commands".to_owned(),
            },
            Mode {
                id: "acceptEdits".to_owned(),
                label: "Accept edits".to_owned(),
                detail: "Edits go in; commands still ask".to_owned(),
            },
            Mode {
                id: "plan".to_owned(),
                label: "Plan".to_owned(),
                detail: "Reads and plans; changes nothing".to_owned(),
            },
        ]
    );
}

#[test]
fn codex_offers_default_and_plan_but_never_knows_which() {
    let offered = modes("codex", Some("019a-anything")).expect("Codex has modes");
    assert_eq!(offered.current, "");
    assert_eq!(ids(&offered), ["default", "plan"]);
}

#[test]
fn opencode_with_no_session_offers_build_and_plan() {
    let offered = modes("opencode", None).expect("OpenCode has modes");
    assert_eq!(offered.current, "");
    assert_eq!(ids(&offered), ["build", "plan"]);
}

#[test]
fn modes_go_by_the_harness_whatever_the_agent_is_named() {
    assert_eq!(modes("claude:work", None), modes("claude", None));
    assert_eq!(modes("Claude", None), modes("claude", None));
    assert_eq!(modes("CODEX:gpt-5", None), modes("codex", None));
    assert_eq!(modes("opencode:qwen3", None), modes("opencode", None));
}

#[test]
fn an_agent_without_modes_or_a_shell_has_none() {
    for agent in ["grok", "zsh", "shell", ""] {
        assert_eq!(modes(agent, None), None, "{agent:?}");
    }
}

#[test]
fn claude_mode_is_never_read_for_a_session_id_that_escapes() {
    assert_eq!(claude_mode(""), None);
    assert_eq!(claude_mode("../../etc/passwd"), None);
    assert_eq!(claude_mode("a/b"), None);
}

#[test]
fn opencode_agent_is_never_asked_for_an_id_that_is_not_opencodes() {
    // Each would otherwise be spliced into a SQL statement.
    for session in [
        "",
        "ses_",
        "abc123",
        "ses_x'; DROP TABLE session_v2; --",
        "ses_a b",
    ] {
        assert_eq!(opencode_agent(session, false), None, "{session:?}");
    }
}

// ---- Claude's mode, from its transcript ---------------------------------------

#[test]
fn claude_mode_is_the_newest_permission_mode_record() {
    let Some(home) = sandboxed("claude_mode_is_the_newest_permission_mode_record") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000001";
    transcript(
        &home,
        session,
        &[
            user_turn(session, "plan the refactor first"),
            mode_record(session, "plan"),
            user_turn(session, "go ahead"),
            mode_record(session, "acceptEdits"),
            user_turn(session, "and the tests"),
        ],
    );
    assert_eq!(claude_mode(session).as_deref(), Some("acceptEdits"));
}

#[test]
fn claude_mode_is_none_until_the_mode_first_changes() {
    let Some(home) = sandboxed("claude_mode_is_none_until_the_mode_first_changes") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000002";
    transcript(&home, session, &[user_turn(session, "hello")]);
    assert_eq!(claude_mode(session), None);
    assert_eq!(claude_mode("6f1c2a7e-0000-4000-8000-00000000ffff"), None);

    let offered = modes("claude", Some(session)).expect("Claude has modes");
    assert_eq!(offered.current, "");
    assert_eq!(ids(&offered), ["default", "acceptEdits", "plan"]);
}

#[test]
fn claude_mode_ignores_a_turn_that_only_mentions_a_mode_record() {
    let Some(home) = sandboxed("claude_mode_ignores_a_turn_that_only_mentions_a_mode_record")
    else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000003";
    transcript(
        &home,
        session,
        &[
            mode_record(session, "plan"),
            // A person pasting a record into the chat is not Claude changing
            // mode.
            user_turn(
                session,
                r#"why does it say {"type":"permission-mode","permissionMode":"bypassPermissions"}"#,
            ),
            // Nor is a tool's result that names the record type, on a user
            // record carrying a mode of its own.
            serde_json::json!({
                "type": "user",
                "sessionId": session,
                "permissionMode": "bypassPermissions",
                "toolUseResult": { "mode": "content", "pattern": "permission-mode" },
            })
            .to_string(),
            // And a line cut short is not a record at all.
            r#"{"type":"permission-mode","permissionMode":"auto""#.to_owned(),
        ],
    );
    assert_eq!(claude_mode(session).as_deref(), Some("plan"));
}

#[test]
fn claude_mode_finds_the_newest_record_at_the_end_of_a_long_transcript() {
    let Some(home) =
        sandboxed("claude_mode_finds_the_newest_record_at_the_end_of_a_long_transcript")
    else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000004";
    let mut lines = vec![mode_record(session, "plan")];
    // Well past the tail that is read, so the read starts mid-line.
    let filler = "x".repeat(997);
    lines.extend((0..600).map(|_| user_turn(session, &filler)));
    lines.push(mode_record(session, "acceptEdits"));
    let path = transcript(&home, session, &lines);
    assert!(fs::metadata(&path).expect("the transcript").len() > 512 * 1024);

    assert_eq!(claude_mode(session).as_deref(), Some("acceptEdits"));
}

#[test]
fn claude_offers_a_mode_outside_its_cycle_once_it_is_seen_in_it() {
    let Some(home) = sandboxed("claude_offers_a_mode_outside_its_cycle_once_it_is_seen_in_it")
    else {
        return;
    };
    for (n, (mode, label, detail)) in [
        ("auto", "Auto", "Claude decides what to allow"),
        ("bypassPermissions", "Bypass", "Nothing asks"),
        ("dontAsk", "Don't ask", "Refuses what it would have asked"),
        // One a newer Claude adds: offered under its own name.
        ("yolo", "yolo", ""),
    ]
    .into_iter()
    .enumerate()
    {
        let session = format!("6f1c2a7e-0000-4000-8000-00000000010{n}");
        transcript(&home, &session, &[mode_record(&session, mode)]);
        let offered = modes("claude", Some(&session)).expect("Claude has modes");
        assert_eq!(offered.current, mode);
        assert_eq!(ids(&offered), ["default", "acceptEdits", "plan", mode]);
        assert_eq!(offered.choices[3].label, label);
        assert_eq!(offered.choices[3].detail, detail);
    }
}

#[test]
fn claude_in_a_mode_of_its_cycle_is_offered_the_cycle_alone() {
    let Some(home) = sandboxed("claude_in_a_mode_of_its_cycle_is_offered_the_cycle_alone") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000005";
    transcript(&home, session, &[mode_record(session, "plan")]);
    let offered = modes("claude:work", Some(session)).expect("Claude has modes");
    assert_eq!(offered.current, "plan");
    assert_eq!(ids(&offered), ["default", "acceptEdits", "plan"]);
}

#[test]
fn claude_transcripts_are_found_beside_the_home_account() {
    let Some(home) = sandboxed("claude_transcripts_are_found_beside_the_home_account") else {
        return;
    };
    // `CLAUDE_CONFIG_DIR=~/.claude-alt claude`, behind an alias the
    // host's shell may not expand.
    let session = "6f1c2a7e-0000-4000-8000-000000000006";
    let dir = home
        .join(".claude-alt")
        .join("projects")
        .join("-tmp-project");
    fs::create_dir_all(&dir).expect("a projects directory");
    fs::write(
        dir.join(format!("{session}.jsonl")),
        mode_record(session, "plan") + "\n",
    )
    .expect("write the transcript");
    assert_eq!(claude_mode(session).as_deref(), Some("plan"));
}

// ---- OpenCode's agent, from its database --------------------------------------

#[test]
fn opencode_agent_is_the_sessions_agent_in_its_database() {
    let Some(home) = sandboxed("opencode_agent_is_the_sessions_agent_in_its_database") else {
        return;
    };
    opencode_database(&home, &[("ses_plan1", "plan"), ("ses_blank", "")]);
    assert_eq!(opencode_agent("ses_plan1", false).as_deref(), Some("plan"));
    assert_eq!(opencode_agent("ses_blank", false), None);
    assert_eq!(opencode_agent("ses_unknown", false), None);
}

#[test]
fn opencode_agent_is_none_without_a_database() {
    if sandboxed("opencode_agent_is_none_without_a_database").is_none() {
        return;
    }
    assert_eq!(opencode_agent("ses_anything", false), None);
    let offered = modes("opencode", Some("ses_anything")).expect("OpenCode has modes");
    assert_eq!(offered.current, "");
    assert_eq!(ids(&offered), ["build", "plan"]);
}

#[test]
fn opencode_agent_lets_a_recent_read_stand_only_when_asked() {
    let Some(home) = sandboxed("opencode_agent_lets_a_recent_read_stand_only_when_asked") else {
        return;
    };
    let database = opencode_database(&home, &[("ses_cache1", "build")]);
    assert_eq!(opencode_agent("ses_cache1", true).as_deref(), Some("build"));

    sqlite(
        &database,
        "UPDATE session_v2 SET agent = 'plan' WHERE id = 'ses_cache1';",
    );
    assert_eq!(
        opencode_agent("ses_cache1", true).as_deref(),
        Some("build"),
        "a snapshot a moment later reuses the read"
    );
    assert_eq!(
        opencode_agent("ses_cache1", false).as_deref(),
        Some("plan"),
        "switching reads afresh"
    );
    assert_eq!(
        opencode_agent("ses_cache1", true).as_deref(),
        Some("plan"),
        "and that fresh read is the one kept"
    );
}

#[test]
fn opencode_offers_its_own_agent_capitalised() {
    let Some(home) = sandboxed("opencode_offers_its_own_agent_capitalised") else {
        return;
    };
    opencode_database(&home, &[("ses_review1", "review"), ("ses_plan2", "plan")]);

    let offered = modes("opencode", Some("ses_review1")).expect("OpenCode has modes");
    assert_eq!(offered.current, "review");
    assert_eq!(ids(&offered), ["build", "plan", "review"]);
    assert_eq!(offered.choices[2].label, "Review");
    assert_eq!(offered.choices[2].detail, "");

    let offered = modes("opencode", Some("ses_plan2")).expect("OpenCode has modes");
    assert_eq!(offered.current, "plan");
    assert_eq!(ids(&offered), ["build", "plan"]);
}

// ---- switching ----------------------------------------------------------------

#[test]
fn set_presses_shift_tab_until_claude_reports_the_mode() {
    let Some(home) = sandboxed("set_presses_shift_tab_until_claude_reports_the_mode") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000201";
    let path = transcript(&home, session, &[user_turn(session, "hello")]);
    let claude = agent(
        &home,
        CLAUDE,
        &[
            ("MODES", "acceptEdits plan default"),
            ("SESSION", session),
            ("TRANSCRIPT", &path.display().to_string()),
        ],
    );

    assert_eq!(set(&claude, "claude", Some(session), "plan"), Ok(()));
    assert_eq!(recorded(&path), ["acceptEdits", "plan"]);
    claude.kill();
}

#[test]
fn set_leaves_claude_alone_when_it_is_already_in_the_mode() {
    let Some(home) = sandboxed("set_leaves_claude_alone_when_it_is_already_in_the_mode") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000202";
    let path = transcript(&home, session, &[mode_record(session, "plan")]);
    let claude = agent(
        &home,
        CLAUDE,
        &[
            ("MODES", "default acceptEdits plan"),
            ("SESSION", session),
            ("TRANSCRIPT", &path.display().to_string()),
        ],
    );

    assert_eq!(set(&claude, "claude", Some(session), "plan"), Ok(()));
    assert_eq!(recorded(&path), ["plan"], "no key was pressed");
    claude.kill();
}

#[test]
fn set_stops_once_claude_comes_round_without_the_mode() {
    let Some(home) = sandboxed("set_stops_once_claude_comes_round_without_the_mode") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000203";
    let path = transcript(&home, session, &[mode_record(session, "default")]);
    let claude = agent(
        &home,
        CLAUDE,
        &[
            ("MODES", "acceptEdits plan default acceptEdits plan default"),
            ("SESSION", session),
            ("TRANSCRIPT", &path.display().to_string()),
        ],
    );

    assert_eq!(
        set(&claude, "claude", Some(session), "auto"),
        Err("Claude doesn't offer that mode here".to_owned())
    );
    // Round once, and the press that showed a mode again.
    assert_eq!(
        recorded(&path),
        ["default", "acceptEdits", "plan", "default", "acceptEdits"]
    );
    claude.kill();
}

#[test]
fn set_says_when_claude_does_not_take_the_key() {
    let Some(home) = sandboxed("set_says_when_claude_does_not_take_the_key") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000204";
    let path = transcript(&home, session, &[mode_record(session, "acceptEdits")]);
    let busy = agent(&home, DEAF, &[]);

    assert_eq!(
        set(&busy, "claude", Some(session), "plan"),
        Err("Claude didn't take the key — switch it in the terminal".to_owned())
    );
    assert_eq!(recorded(&path), ["acceptEdits"]);
    busy.kill();
}

#[test]
fn set_says_when_claude_has_not_yet_said_its_mode() {
    let Some(home) = sandboxed("set_says_when_claude_has_not_yet_said_its_mode") else {
        return;
    };
    let session = "6f1c2a7e-0000-4000-8000-000000000205";
    transcript(&home, session, &[user_turn(session, "hello")]);
    let busy = agent(&home, DEAF, &[]);

    assert_eq!(
        set(&busy, "claude", Some(session), "plan"),
        Err("Claude hasn't said which mode it is in yet".to_owned())
    );
    busy.kill();
}

#[test]
fn set_presses_tab_until_opencode_runs_the_agent() {
    let Some(home) = sandboxed("set_presses_tab_until_opencode_runs_the_agent") else {
        return;
    };
    let database = opencode_database(&home, &[("ses_switch1", "build")]);
    let opencode = agent(
        &home,
        OPENCODE,
        &[
            ("AGENTS", "plan build plan"),
            ("SESSION", "ses_switch1"),
            ("DB", &database.display().to_string()),
        ],
    );

    assert_eq!(
        set(&opencode, "opencode:qwen3", Some("ses_switch1"), "plan"),
        Ok(())
    );
    assert_eq!(
        opencode_agent("ses_switch1", false).as_deref(),
        Some("plan")
    );
    opencode.kill();
}

#[test]
fn set_gives_codex_one_blind_shift_tab() {
    let sandbox = Sandbox::new("agent-mode-codex");
    let keys = sandbox.path("keys");
    let codex = agent(
        sandbox.root(),
        "head -c 3 > \"$KEYS\"; cat > /dev/null",
        &[("KEYS", &keys.display().to_string())],
    );

    assert_eq!(set(&codex, "codex", None, "plan"), Ok(()));
    assert!(
        wait_until(|| fs::read(&keys).is_ok_and(|bytes| bytes.len() == 3)),
        "Codex never got its key"
    );
    assert_eq!(fs::read(&keys).expect("the keys"), b"\x1b[Z");
    codex.kill();
}

#[test]
fn set_refuses_an_agent_with_no_modes() {
    let sandbox = Sandbox::new("agent-mode-none");
    let shell = agent(sandbox.root(), DEAF, &[]);
    assert_eq!(
        set(&shell, "grok", None, "plan"),
        Err("this agent has no modes to switch".to_owned())
    );
    shell.kill();
}
