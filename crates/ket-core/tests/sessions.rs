//! Reading each agent's own record of its sessions, and the `Sessions`
//! facade that aggregates them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ket_core::sessions::{
    ClaudeSessions, CodexRollout, CodexSessions, DiscoveredSession, GeminiSessions, GrokSessions,
    LiveKind, LiveSession, MAX_SESSIONS_PER_SOURCE, OpencodeSessions, SessionSource, Sessions,
};

mod common;
use common::Sandbox;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

// ---- claude -----------------------------------------------------------------

/// The directory Claude would file this worktree's sessions under.
fn claude_slug_dir(root: &Path, worktree: &Path) -> PathBuf {
    let canon = worktree.canonicalize().unwrap();
    let slug = canon.to_string_lossy().replace('/', "-");
    root.join(slug)
}

#[test]
fn claude_sessions_for_an_unknown_directory_is_empty() {
    let sandbox = Sandbox::new("sessions-claude-unknown");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();

    let sessions = ClaudeSessions::at(sandbox.path("projects"));
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert!(found.is_empty());
}

#[test]
fn claude_sessions_title_falls_back_to_the_first_user_line() {
    let sandbox = Sandbox::new("sessions-claude-first-line");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    write(
        &dir.join("abc.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"fix the login bug\"}}\n",
    );

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, "claude");
    assert_eq!(found[0].id, "abc");
    assert_eq!(found[0].title.as_deref(), Some("fix the login bug"));
    assert_eq!(found[0].explicit_title, None);
    assert_eq!(found[0].directory, worktree);
}

#[test]
fn claude_sessions_skips_meta_lines_and_tag_prefixed_content() {
    let sandbox = Sandbox::new("sessions-claude-skip");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    write(
        &dir.join("abc.jsonl"),
        concat!(
            "{\"type\":\"user\",\"isMeta\":true,\"message\":{\"content\":\"scaffolding\"}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>foo</command-name>\"}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"the real prompt\"}}\n",
        ),
    );

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found[0].title.as_deref(), Some("the real prompt"));
}

#[test]
fn claude_sessions_ai_title_wins_over_the_first_user_line() {
    let sandbox = Sandbox::new("sessions-claude-ai-title");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    write(
        &dir.join("abc.jsonl"),
        concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"fix the login bug\"}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Login bug fix\"}\n",
        ),
    );

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found[0].title.as_deref(), Some("Login bug fix"));
    assert_eq!(found[0].explicit_title, None, "an ai title is not explicit");
}

#[test]
fn claude_sessions_custom_title_wins_over_everything() {
    let sandbox = Sandbox::new("sessions-claude-custom-title");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    write(
        &dir.join("abc.jsonl"),
        concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"fix the login bug\"}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Login bug fix\"}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"My Own Name\"}\n",
        ),
    );

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found[0].title.as_deref(), Some("My Own Name"));
    assert_eq!(found[0].explicit_title.as_deref(), Some("My Own Name"));
}

#[test]
fn claude_sessions_ignores_files_that_are_not_jsonl() {
    let sandbox = Sandbox::new("sessions-claude-non-jsonl");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    write(
        &dir.join("abc.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
    );
    write(&dir.join("notes.txt"), "not a session");
    write(&dir.join("secrets.json"), "{}");

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "abc");
}

#[test]
fn claude_sessions_are_sorted_newest_first() {
    let sandbox = Sandbox::new("sessions-claude-sort");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    write(
        &dir.join("older.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"first\"}}\n",
    );
    std::thread::sleep(Duration::from_millis(30));
    write(
        &dir.join("newer.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"second\"}}\n",
    );

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "newer");
    assert_eq!(found[1].id, "older");
}

#[test]
fn claude_sessions_are_capped_per_source() {
    let sandbox = Sandbox::new("sessions-claude-cap");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("projects");
    let dir = claude_slug_dir(&root, &worktree);

    for i in 0..MAX_SESSIONS_PER_SOURCE + 5 {
        write(
            &dir.join(format!("s{i:03}.jsonl")),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
        );
    }

    let sessions = ClaudeSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), MAX_SESSIONS_PER_SOURCE);
}

#[test]
fn claude_resume_args_names_the_session() {
    let sessions = ClaudeSessions::at(PathBuf::from("/nonexistent"));
    assert_eq!(
        sessions.resume_args("abc-123"),
        vec!["--resume".to_owned(), "abc-123".to_owned()]
    );
}

// ---- codex ------------------------------------------------------------------

#[test]
fn codex_sessions_for_a_missing_store_is_empty() {
    let sandbox = Sandbox::new("sessions-codex-missing-store");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();

    let sessions = CodexSessions::at(sandbox.path("does-not-exist"));
    assert!(sessions.sessions_for(&[&worktree]).unwrap().is_empty());
}

#[test]
fn codex_sessions_matches_by_the_cwd_named_in_the_first_record() {
    let sandbox = Sandbox::new("sessions-codex-match");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("sessions");

    let rollout = root
        .join("2026")
        .join("01")
        .join("15")
        .join("rollout-a.jsonl");
    write(
        &rollout,
        &format!(
            "{{\"payload\":{{\"cwd\":{:?},\"id\":\"sess-1\"}}}}\n",
            worktree.to_string_lossy()
        ),
    );

    let sessions = CodexSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, "codex");
    assert_eq!(found[0].id, "sess-1");
    assert_eq!(found[0].directory, worktree);
}

#[test]
fn codex_sessions_do_not_cross_over_to_a_different_cwd() {
    let sandbox = Sandbox::new("sessions-codex-cross");
    let worktree = sandbox.path("worktree");
    let other = sandbox.path("other");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let root = sandbox.path("sessions");

    let rollout = root
        .join("2026")
        .join("01")
        .join("15")
        .join("rollout-a.jsonl");
    write(
        &rollout,
        &format!(
            "{{\"payload\":{{\"cwd\":{:?},\"id\":\"sess-1\"}}}}\n",
            other.to_string_lossy()
        ),
    );

    let sessions = CodexSessions::at(root);
    assert!(sessions.sessions_for(&[&worktree]).unwrap().is_empty());
}

#[test]
fn codex_sessions_ignores_files_not_shaped_like_a_rollout() {
    let sandbox = Sandbox::new("sessions-codex-shape");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("sessions");

    write(
        &root.join("2026").join("01").join("15").join("notes.jsonl"),
        &format!(
            "{{\"payload\":{{\"cwd\":{:?},\"id\":\"sess-1\"}}}}\n",
            worktree.to_string_lossy()
        ),
    );

    let sessions = CodexSessions::at(root);
    assert!(sessions.sessions_for(&[&worktree]).unwrap().is_empty());
}

#[test]
fn codex_resume_args_names_the_session() {
    let sessions = CodexSessions::at(PathBuf::from("/nonexistent"));
    assert_eq!(
        sessions.resume_args("sess-1"),
        vec!["resume".to_owned(), "sess-1".to_owned()]
    );
}

// ---- grok -------------------------------------------------------------------

#[test]
fn grok_sessions_for_a_missing_store_is_empty() {
    let sandbox = Sandbox::new("sessions-grok-missing");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();

    let sessions = GrokSessions::at(sandbox.path("does-not-exist"));
    assert!(sessions.sessions_for(&[&worktree]).unwrap().is_empty());
}

#[test]
fn grok_sessions_prefers_the_generated_title() {
    let sandbox = Sandbox::new("sessions-grok-title");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("sessions");

    let canon = worktree.canonicalize().unwrap();
    let encoded: String = canon
        .to_string_lossy()
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();

    write(
        &root.join(&encoded).join("abc").join("summary.json"),
        "{\"generated_title\": \"Fix the thing\", \"session_summary\": \"fallback\"}",
    );

    let sessions = GrokSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, "grok");
    assert_eq!(found[0].id, "abc");
    assert_eq!(found[0].title.as_deref(), Some("Fix the thing"));
}

#[test]
fn grok_sessions_falls_back_to_the_session_summary() {
    let sandbox = Sandbox::new("sessions-grok-fallback");
    let worktree = sandbox.path("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("sessions");

    let canon = worktree.canonicalize().unwrap();
    let encoded: String = canon
        .to_string_lossy()
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();

    write(
        &root.join(&encoded).join("abc").join("summary.json"),
        "{\"session_summary\": \"a plain summary\"}",
    );

    let sessions = GrokSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found[0].title.as_deref(), Some("a plain summary"));
}

#[test]
fn grok_resume_args_names_the_session() {
    let sessions = GrokSessions::at(PathBuf::from("/nonexistent"));
    assert_eq!(
        sessions.resume_args("abc"),
        vec!["--resume".to_owned(), "abc".to_owned()]
    );
}

// ---- gemini -----------------------------------------------------------------

#[test]
fn gemini_is_an_honest_placeholder() {
    let sessions = GeminiSessions;
    let dir = PathBuf::from("/wherever");
    assert!(sessions.sessions_for(&[&dir]).unwrap().is_empty());
    assert!(sessions.resume_args("anything").is_empty());
}

// ---- the `Sessions` facade ---------------------------------------------------

/// A source under full test control, for exercising `Sessions` without disk.
struct FakeSource {
    agent: &'static str,
    sessions: Vec<DiscoveredSession>,
    fails: bool,
    live: Vec<LiveSession>,
    attach: Option<Vec<String>>,
}

impl SessionSource for FakeSource {
    fn agent(&self) -> &str {
        self.agent
    }

    fn sessions_for(&self, _directories: &[&Path]) -> ket_core::Result<Vec<DiscoveredSession>> {
        if self.fails {
            return Err(ket_core::KetError::Config("boom".to_owned()));
        }
        Ok(self.sessions.clone())
    }

    fn resume_args(&self, session: &str) -> Vec<String> {
        vec!["resume".to_owned(), session.to_owned()]
    }

    fn live_in(&self, directory: &Path) -> Vec<LiveSession> {
        self.live
            .iter()
            .filter(|l| l.directory == directory)
            .cloned()
            .collect()
    }

    fn live(&self) -> Vec<LiveSession> {
        self.live.clone()
    }

    fn attach_args(&self, _live: &LiveSession) -> Option<Vec<String>> {
        self.attach.clone()
    }
}

fn discovered(agent: &str, id: &str, directory: &Path, updated_at_ms: u64) -> DiscoveredSession {
    DiscoveredSession {
        agent: agent.to_owned(),
        id: id.to_owned(),
        title: None,
        explicit_title: None,
        directory: directory.to_path_buf(),
        updated_at_ms,
    }
}

#[test]
fn scan_buckets_by_directory_and_sorts_newest_first() {
    let a = PathBuf::from("/a");
    let b = PathBuf::from("/b");

    let source1 = FakeSource {
        agent: "one",
        sessions: vec![
            discovered("one", "old", &a, 10),
            discovered("one", "new", &a, 20),
        ],
        fails: false,
        live: Vec::new(),
        attach: None,
    };
    let source2 = FakeSource {
        agent: "two",
        sessions: vec![discovered("two", "other-dir", &b, 5)],
        fails: false,
        live: Vec::new(),
        attach: None,
    };

    let sessions = Sessions::with_sources(vec![Box::new(source1), Box::new(source2)]);
    let buckets = sessions.scan(&[&a, &b]);

    let for_a = &buckets[&a];
    assert_eq!(for_a.len(), 2);
    assert_eq!(for_a[0].id, "new", "newest first within a bucket");
    assert_eq!(for_a[1].id, "old");

    assert_eq!(buckets[&b][0].id, "other-dir");
}

#[test]
fn scan_skips_a_source_that_errors_and_keeps_the_others() {
    let a = PathBuf::from("/a");
    let broken = FakeSource {
        agent: "broken",
        sessions: Vec::new(),
        fails: true,
        live: Vec::new(),
        attach: None,
    };
    let working = FakeSource {
        agent: "working",
        sessions: vec![discovered("working", "s1", &a, 1)],
        fails: false,
        live: Vec::new(),
        attach: None,
    };

    let sessions = Sessions::with_sources(vec![Box::new(broken), Box::new(working)]);
    let buckets = sessions.scan(&[&a]);
    assert_eq!(buckets[&a].len(), 1);
    assert_eq!(buckets[&a][0].id, "s1");
}

#[test]
fn resume_args_delegates_to_the_named_agent_and_is_none_for_an_unknown_one() {
    let source = FakeSource {
        agent: "claude",
        sessions: Vec::new(),
        fails: false,
        live: Vec::new(),
        attach: None,
    };
    let sessions = Sessions::with_sources(vec![Box::new(source)]);

    assert_eq!(
        sessions.resume_args("claude", "abc"),
        Some(vec!["resume".to_owned(), "abc".to_owned()])
    );
    assert_eq!(sessions.resume_args("nonexistent", "abc"), None);
}

#[test]
fn live_flattens_across_every_source() {
    let a = PathBuf::from("/a");
    let source1 = FakeSource {
        agent: "one",
        sessions: Vec::new(),
        fails: false,
        live: vec![LiveSession {
            id: "l1".to_owned(),
            handle: None,
            kind: LiveKind::Interactive,
            directory: a.clone(),
            busy: Some(true),
        }],
        attach: None,
    };
    let source2 = FakeSource {
        agent: "two",
        sessions: Vec::new(),
        fails: false,
        live: vec![LiveSession {
            id: "l2".to_owned(),
            handle: None,
            kind: LiveKind::Background,
            directory: a,
            busy: None,
        }],
        attach: None,
    };

    let sessions = Sessions::with_sources(vec![Box::new(source1), Box::new(source2)]);
    let live = sessions.live();
    assert_eq!(live.len(), 2);
    assert!(live.iter().any(|l| l.id == "l1"));
    assert!(live.iter().any(|l| l.id == "l2"));
}

#[test]
fn open_args_resumes_the_record_when_nothing_is_holding_it() {
    let dir = PathBuf::from("/a");
    let source = FakeSource {
        agent: "claude",
        sessions: Vec::new(),
        fails: false,
        live: Vec::new(),
        attach: None,
    };
    let sessions = Sessions::with_sources(vec![Box::new(source)]);

    assert_eq!(
        sessions.open_args("claude", &dir, "s1"),
        Some(vec!["resume".to_owned(), "s1".to_owned()])
    );
}

#[test]
fn open_args_attaches_to_a_backgrounded_session() {
    let dir = PathBuf::from("/a");
    let source = FakeSource {
        agent: "claude",
        sessions: Vec::new(),
        fails: false,
        live: vec![LiveSession {
            id: "s1".to_owned(),
            handle: Some("h1".to_owned()),
            kind: LiveKind::Background,
            directory: dir.clone(),
            busy: None,
        }],
        attach: Some(vec!["attach".to_owned(), "h1".to_owned()]),
    };
    let sessions = Sessions::with_sources(vec![Box::new(source)]);

    assert_eq!(
        sessions.open_args("claude", &dir, "s1"),
        Some(vec!["attach".to_owned(), "h1".to_owned()])
    );
}

#[test]
fn open_args_refuses_a_session_someone_already_has_open() {
    let dir = PathBuf::from("/a");
    let source = FakeSource {
        agent: "claude",
        sessions: Vec::new(),
        fails: false,
        live: vec![LiveSession {
            id: "s1".to_owned(),
            handle: None,
            kind: LiveKind::Interactive,
            directory: dir.clone(),
            busy: None,
        }],
        attach: None,
    };
    let sessions = Sessions::with_sources(vec![Box::new(source)]);

    assert_eq!(sessions.open_args("claude", &dir, "s1"), None);
}

#[test]
fn open_args_is_none_for_an_agent_with_no_matching_source() {
    let sessions = Sessions::with_sources(Vec::new());
    assert_eq!(
        sessions.open_args("nonexistent", &PathBuf::from("/a"), "s1"),
        None
    );
}

// ---- opencode (real sqlite3) -------------------------------------------------

fn sqlite_db(sandbox: &Sandbox, rows: &[(&str, Option<&str>, &str, u64)]) -> PathBuf {
    let db = sandbox.path("opencode.db");
    let status = std::process::Command::new("sqlite3")
        .arg(&db)
        .arg(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_updated INTEGER); \
             CREATE TABLE credential (secret TEXT);",
        )
        .status()
        .expect("spawn sqlite3");
    assert!(status.success());

    for (id, title, directory, time_updated) in rows {
        let title_sql = match title {
            Some(t) => format!("'{}'", t.replace('\'', "''")),
            None => "NULL".to_owned(),
        };
        let stmt = format!(
            "INSERT INTO session (id, title, directory, time_updated) VALUES ('{}', {}, '{}', {});",
            id.replace('\'', "''"),
            title_sql,
            directory.replace('\'', "''"),
            time_updated
        );
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(stmt)
            .status()
            .expect("spawn sqlite3");
        assert!(status.success());
    }

    db
}

#[test]
fn opencode_sessions_reads_matching_rows_from_a_real_database() {
    let sandbox = Sandbox::new("sessions-opencode-real");
    let worktree = "/some/worktree";
    let db = sqlite_db(
        &sandbox,
        &[
            ("s1", Some("Fix the bug"), worktree, 100),
            ("s2", None, worktree, 200),
            ("s3", Some("elsewhere"), "/other", 50),
        ],
    );

    let sessions = OpencodeSessions::at(db);
    let dir = PathBuf::from(worktree);
    let found = sessions.sessions_for(&[&dir]).unwrap();

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "s2", "newest first");
    assert_eq!(found[0].agent, "opencode");
    assert_eq!(found[0].title, None);
    assert_eq!(found[1].id, "s1");
    assert_eq!(found[1].title.as_deref(), Some("Fix the bug"));
}

#[test]
fn opencode_sessions_for_a_missing_database_is_empty() {
    let sandbox = Sandbox::new("sessions-opencode-missing-db");
    let sessions = OpencodeSessions::at(sandbox.path("does-not-exist.db"));
    let dir = PathBuf::from("/whatever");
    assert!(sessions.sessions_for(&[&dir]).unwrap().is_empty());
}

#[test]
fn opencode_sessions_for_no_directories_is_empty() {
    let sandbox = Sandbox::new("sessions-opencode-no-dirs");
    let db = sqlite_db(&sandbox, &[("s1", None, "/a", 1)]);
    let sessions = OpencodeSessions::at(db);
    assert!(sessions.sessions_for(&[]).unwrap().is_empty());
}

#[test]
fn opencode_resume_args_names_the_session() {
    let sessions = OpencodeSessions::at(PathBuf::from("/nonexistent"));
    assert_eq!(
        sessions.resume_args("s1"),
        vec!["--session".to_owned(), "s1".to_owned()]
    );
}

// ---- codex rollout (tailing) --------------------------------------------------

fn rollout_line(kind: &str, payload: serde_json::Value) -> String {
    serde_json::json!({ "type": kind, "payload": payload }).to_string()
}

#[test]
fn codex_rollout_for_directory_is_none_without_a_store() {
    let sandbox = Sandbox::new("rollout-no-store");
    let dir = sandbox.path("worktree");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(CodexRollout::under(&sandbox.path("does-not-exist"), &dir).is_none());
}

#[test]
fn codex_rollout_finds_the_session_named_for_its_directory() {
    let sandbox = Sandbox::new("rollout-find");
    let dir = sandbox.path("worktree");
    std::fs::create_dir_all(&dir).unwrap();
    let root = sandbox.path("sessions");

    write(
        &root
            .join("2026")
            .join("01")
            .join("15")
            .join("rollout-a.jsonl"),
        &format!(
            "{}\n",
            rollout_line(
                "session_meta",
                serde_json::json!({"id": "sess-1", "cwd": dir.to_string_lossy()})
            )
        ),
    );

    let rollout = CodexRollout::under(&root, &dir).expect("found");
    assert_eq!(rollout.session(), Some("sess-1"));
    assert_eq!(rollout.cwd(), Some(dir.to_string_lossy().as_ref()));
    assert_eq!(rollout.model(), None);
    assert_eq!(rollout.effort(), None);
    assert_eq!(
        rollout.path(),
        root.join("2026")
            .join("01")
            .join("15")
            .join("rollout-a.jsonl")
    );
}

#[test]
fn codex_rollout_poll_reports_the_newest_token_count_and_turn_settings() {
    let sandbox = Sandbox::new("rollout-poll");
    let dir = sandbox.path("worktree");
    std::fs::create_dir_all(&dir).unwrap();
    let root = sandbox.path("sessions");
    let path = root
        .join("2026")
        .join("01")
        .join("15")
        .join("rollout-a.jsonl");

    write(
        &path,
        &format!(
            "{}\n",
            rollout_line(
                "session_meta",
                serde_json::json!({"id": "sess-1", "cwd": dir.to_string_lossy()})
            )
        ),
    );

    let mut rollout = CodexRollout::under(&root, &dir).expect("found");
    assert!(rollout.poll().is_none(), "nothing appended yet");

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    use std::io::Write;
    writeln!(
        file,
        "{}",
        rollout_line(
            "turn_context",
            serde_json::json!({"model": "opus", "effort": "high"})
        )
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        rollout_line(
            "token_count",
            serde_json::json!({"type": "token_count", "total": 100})
        )
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        rollout_line(
            "token_count",
            serde_json::json!({"type": "token_count", "total": 200})
        )
    )
    .unwrap();

    let tokens = rollout.poll().expect("a token_count record was appended");
    assert_eq!(tokens["total"], 200, "newest token_count wins");
    assert_eq!(rollout.model(), Some("opus"));
    assert_eq!(rollout.effort(), Some("high"));

    assert!(rollout.poll().is_none(), "nothing new since the last poll");
}

#[test]
fn codex_rollout_poll_keeps_the_last_settings_when_a_later_turn_omits_them() {
    let sandbox = Sandbox::new("rollout-poll-keep");
    let dir = sandbox.path("worktree");
    std::fs::create_dir_all(&dir).unwrap();
    let root = sandbox.path("sessions");
    let path = root
        .join("2026")
        .join("01")
        .join("15")
        .join("rollout-a.jsonl");

    write(
        &path,
        &format!(
            "{}\n",
            rollout_line(
                "session_meta",
                serde_json::json!({"id": "sess-1", "cwd": dir.to_string_lossy()})
            )
        ),
    );
    let mut rollout = CodexRollout::under(&root, &dir).expect("found");

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    use std::io::Write;
    writeln!(
        file,
        "{}",
        rollout_line(
            "turn_context",
            serde_json::json!({"model": "opus", "effort": "high"})
        )
    )
    .unwrap();
    rollout.poll();
    assert_eq!(rollout.model(), Some("opus"));

    // A later turn names no model at all — the prior one must survive.
    writeln!(
        file,
        "{}",
        rollout_line("turn_context", serde_json::json!({"effort": "low"}))
    )
    .unwrap();
    rollout.poll();
    assert_eq!(
        rollout.model(),
        Some("opus"),
        "unset fields are not cleared"
    );
    assert_eq!(rollout.effort(), Some("low"));
}

// ---- grok directory encoding ---------------------------------------------------

#[test]
fn grok_sessions_encodes_special_characters_in_the_directory() {
    let sandbox = Sandbox::new("sessions-grok-encoding");
    let worktree = sandbox.path("a worktree (with) special chars!");
    std::fs::create_dir_all(&worktree).unwrap();
    let root = sandbox.path("sessions");

    let canon = worktree.canonicalize().unwrap();
    let encoded: String = canon
        .to_string_lossy()
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();

    write(
        &root.join(&encoded).join("abc").join("summary.json"),
        "{\"generated_title\": \"special chars work\"}",
    );

    let sessions = GrokSessions::at(root);
    let found = sessions.sessions_for(&[&worktree]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title.as_deref(), Some("special chars work"));
}
