use super::*;
use serde_json::json;

fn rule(tool: &str, decision: Verdict) -> Rule {
    Rule::for_tool(tool, decision)
}

fn command(pattern: &str, decision: Verdict) -> Rule {
    Rule {
        command: Patterns::one(pattern),
        ..rule("Bash", decision)
    }
}

fn path(tool: &str, pattern: &str, decision: Verdict) -> Rule {
    Rule {
        path: Patterns::one(pattern),
        ..rule(tool, decision)
    }
}

/// A rule set with no guardrails: `global` and `project` as the person's,
/// `repo` as the repository's.
fn set(global: Vec<Rule>, project: Vec<Rule>, repo: Vec<Rule>) -> RuleSet {
    let mut file = RuleFile {
        builtins: false,
        rules: global,
        ..RuleFile::default()
    };
    file.projects
        .insert("p".to_owned(), ProjectRules { rules: project });
    RuleSet::from_parts(&file, Some("p"), repo, None)
}

fn shell(rules: &RuleSet, line: &str) -> Option<Verdict> {
    let input = json!({ "command": line });
    rules
        .evaluate(&Request {
            tool: "Bash",
            input: &input,
            worktree: Some(Path::new("/work/tree")),
            home: Some(Path::new("/home/me")),
        })
        .map(|ruling| ruling.verdict)
}

fn file(rules: &RuleSet, tool: &str, at: &str, worktree: &Path) -> Option<Verdict> {
    let input = json!({ "file_path": at });
    rules
        .evaluate(&Request {
            tool,
            input: &input,
            worktree: Some(worktree),
            home: Some(Path::new("/home/me")),
        })
        .map(|ruling| ruling.verdict)
}

/// What OpenCode is told, as (action, resource, effect) triples.
fn opencode(rules: &RuleSet) -> Vec<(String, String, String)> {
    rules
        .opencode_permissions(Some(Path::new("/work/tree")), Some(Path::new("/home/me")))
        .iter()
        .map(|rule| {
            let field = |key: &str| rule[key].as_str().unwrap().to_owned();
            (field("action"), field("resource"), field("effect"))
        })
        .collect()
}

fn told(action: &str, resource: &str, effect: &str) -> (String, String, String) {
    (action.to_owned(), resource.to_owned(), effect.to_owned())
}

#[test]
fn opencode_is_told_asks_then_denies_with_each_list_reversed_and_no_allows() {
    let rules = set(
        vec![
            command("git push *", Verdict::Ask),
            command("cargo test *", Verdict::Allow),
            command("rm -rf *", Verdict::Deny),
            command("curl *", Verdict::Ask),
        ],
        vec![command("npm publish *", Verdict::Deny)],
        vec![],
    );
    // The last match wins in OpenCode: a list's first rule comes last
    // within its effect, and any deny comes after every ask.
    assert_eq!(
        opencode(&rules),
        vec![
            told("shell", "curl *", "ask"),
            told("shell", "git push *", "ask"),
            told("shell", "npm publish *", "deny"),
            told("shell", "rm -rf *", "deny"),
        ]
    );
}

#[test]
fn each_kind_of_rule_reaches_opencode_in_its_spelling() {
    let rules = set(
        vec![
            path("edit", "secrets/**", Verdict::Deny),
            Rule {
                domain: Patterns::one("example.com"),
                ..rule("WebFetch", Verdict::Ask)
            },
            Rule {
                outside_worktree: true,
                ..rule("edit", Verdict::Ask)
            },
            rule("mcp__github__create_issue", Verdict::Deny),
            rule("TodoWrite", Verdict::Deny),
        ],
        vec![],
        vec![],
    );
    let told_opencode = opencode(&rules);
    for expected in [
        told("edit", "/work/tree/secrets/*", "deny"),
        told("edit", "secrets/*", "deny"),
        told("webfetch", "*://example.com", "ask"),
        told("webfetch", "*://example.com/*", "ask"),
        told("external_directory", "*", "ask"),
        told("github_create_issue", "*", "deny"),
    ] {
        assert!(
            told_opencode.contains(&expected),
            "{expected:?} in {told_opencode:?}"
        );
    }
    // A tool OpenCode has no counterpart for says nothing to it.
    assert_eq!(told_opencode.len(), 6);
}

#[test]
fn a_path_deny_on_reading_denies_every_search_it_could_reach() {
    let rules = set(
        vec![path("read", "secrets/**", Verdict::Deny)],
        vec![],
        vec![],
    );
    let told_opencode = opencode(&rules);
    assert!(told_opencode.contains(&told("read", "/work/tree/secrets/*", "deny")));
    // Searches are ruled by what is searched for, not where.
    assert!(!told_opencode.iter().any(|(action, resource, _)| {
        matches!(action.as_str(), "grep" | "glob") && resource != "*"
    }));
    assert!(told_opencode.contains(&told("grep", "*", "deny")));
    assert!(told_opencode.contains(&told("glob", "*", "deny")));
}

#[test]
fn a_path_ask_affects_only_its_search_tool_and_a_deny_wins() {
    let rules = set(
        vec![
            path("Grep", "reports/**", Verdict::Ask),
            path("Grep", "secrets/**", Verdict::Deny),
        ],
        vec![],
        vec![],
    );
    let told_opencode = opencode(&rules);
    assert!(told_opencode.contains(&told("grep", "*", "deny")));
    assert!(!told_opencode.iter().any(|(action, _, _)| action == "glob"));
}

#[test]
fn opencode_permissions_go_after_a_configs_own() {
    let rule = json!({ "action": "shell", "resource": "rm *", "effect": "deny" });
    let merged = with_opencode_permissions(
        Some(
            r#"{"model":"ollama/m","permissions":[{"action":"read","resource":"*","effect":"allow"}],"policies":[{"action":"provider.use","resource":"*","effect":"deny"}]}"#,
        ),
        vec![rule.clone()],
        &["build".to_owned()],
    );
    let merged: serde_json::Value = serde_json::from_str(&merged).unwrap();
    assert_eq!(merged["model"], "ollama/m");
    assert_eq!(merged["permissions"].as_array().unwrap().len(), 2);
    assert_eq!(merged["permissions"][1], rule);
    // An agent's own rules come after the config's, so ket's go there too.
    assert_eq!(merged["agents"]["build"]["permissions"], json!([rule]));
    assert_eq!(
        merged["policies"],
        json!([
            { "action": "provider.use", "resource": "*", "effect": "deny" },
            { "action": "tool.use", "resource": "shell:rm *", "effect": "deny" },
        ])
    );
    let fresh: serde_json::Value =
        serde_json::from_str(&with_opencode_permissions(None, vec![rule.clone()], &[])).unwrap();
    assert_eq!(
        fresh,
        json!({
            "permissions": [rule],
            "policies": [{ "action": "tool.use", "resource": "shell:rm *", "effect": "deny" }],
        })
    );
}

/// A throwaway directory and an executable `opencode` script in it that
/// runs `body`, for `opencode_agents` against a scripted OpenCode.
fn scripted(name: &str, body: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "ket-opencode-{name}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("opencode");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    (dir, script)
}

#[test]
fn agent_discovery_uses_inline_config_and_drains_large_output() {
    use std::os::unix::fs::PermissionsExt as _;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "ket-opencode-agents-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("opencode");
    let inline = r#"{"agents":{"custom":{"permissions":[]}}}"#;
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n[ \"$OPENCODE_CONFIG_CONTENT\" = '{}' ] || exit 1\ncat agents.json\n",
            inline
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let many: Vec<_> = (0..5_000)
        .map(|n| json!({ "id": format!("agent-{n}") }))
        .collect();
    std::fs::write(dir.join("agents.json"), serde_json::to_vec(&many).unwrap()).unwrap();

    let found = opencode_agents(&script, script.to_str().unwrap(), &dir, Some(inline)).unwrap();
    assert_eq!(found.len(), many.len() + 1);
    assert!(found.contains(&"custom".to_owned()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_discovery_asks_again_when_the_first_answer_lists_none() {
    // A first answer of none, as an OpenCode service still opening the
    // directory gives; the second has the agents.
    let (dir, script) = scripted(
        "asks-again",
        r#"if [ -e asked ]; then printf '[{"id":"build"}]'; else touch asked; printf '[]'; fi"#,
    );

    let found = opencode_agents(&script, script.to_str().unwrap(), &dir, None).unwrap();
    assert_eq!(found, vec!["build".to_owned()]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_discovery_errors_when_no_agent_is_ever_listed() {
    // An OpenCode that never lists an agent is a real error — but only
    // after asking once more, the way a late answer is asked for.
    let (dir, script) = scripted(
        "never-listed",
        r#"echo x >> runs
printf '[]'"#,
    );

    let error = opencode_agents(&script, script.to_str().unwrap(), &dir, None).unwrap_err();
    assert_eq!(error.to_string(), "config: OpenCode listed no agents");
    let asked = std::fs::read_to_string(dir.join("runs"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(asked, 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_discovery_keeps_the_inline_agents_when_no_agent_is_listed() {
    // The inline config's own agents keep the launch going when none are
    // listed — and the empty answer is still asked about once more
    // rather than settled for.
    let (dir, script) = scripted(
        "inline-agents",
        r#"echo x >> runs
printf '[]'"#,
    );
    let inline = r#"{"agents":{"custom":{"permissions":[]}}}"#;

    let found = opencode_agents(&script, script.to_str().unwrap(), &dir, Some(inline)).unwrap();
    assert_eq!(found, vec!["custom".to_owned()]);
    let asked = std::fs::read_to_string(dir.join("runs"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(asked, 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_discovery_asks_once_when_the_first_answer_lists_agents() {
    // A complete first answer is not asked about again.
    let (dir, script) = scripted(
        "asks-once",
        r#"echo x >> runs
printf '[{"id":"build"}]'"#,
    );

    let found = opencode_agents(&script, script.to_str().unwrap(), &dir, None).unwrap();
    assert_eq!(found, vec!["build".to_owned()]);
    let asked = std::fs::read_to_string(dir.join("runs"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(asked, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_first_matching_rule_in_a_list_wins() {
    let rules = set(
        vec![
            command("cargo test --release*", Verdict::Deny),
            command("cargo test *", Verdict::Allow),
        ],
        vec![],
        vec![],
    );
    assert_eq!(shell(&rules, "cargo test --release"), Some(Verdict::Deny));
    assert_eq!(shell(&rules, "cargo test --all"), Some(Verdict::Allow));
    assert_eq!(shell(&rules, "cargo build"), None);
}

#[test]
fn the_strictest_scope_wins() {
    // A global ask is not undone by the project's allow.
    let rules = set(
        vec![command("git push *", Verdict::Ask)],
        vec![command("git push *", Verdict::Allow)],
        vec![],
    );
    assert_eq!(shell(&rules, "git push origin main"), Some(Verdict::Ask));
    // A repository's deny beats both.
    let rules = set(
        vec![command("git push *", Verdict::Allow)],
        vec![],
        vec![command("git push *", Verdict::Deny)],
    );
    assert_eq!(shell(&rules, "git push"), Some(Verdict::Deny));
}

#[test]
fn neither_a_repository_nor_a_guardrail_may_allow() {
    let rules = set(
        vec![],
        vec![],
        vec![command("npm publish*", Verdict::Allow)],
    );
    assert_eq!(shell(&rules, "npm publish"), None);

    let dir = std::env::temp_dir().join(format!("ket-perm-repo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
            dir.join(".ket.toml"),
            "[[permissions.rules]]\ntool = \"Bash\"\ncommand = \"npm publish*\"\ndecision = \"allow\"\n\n\
             [[permissions.rules]]\ntool = \"Bash\"\ncommand = \"terraform apply*\"\ndecision = \"deny\"\n",
        )
        .unwrap();
    let repo = RepoRules::load(&dir);
    assert_eq!(repo.rules.len(), 1);
    assert_eq!(repo.ignored.len(), 1);
    assert!(repo.error.is_none());
    std::fs::write(dir.join(".ket.toml"), "[permissions]\nrules = 3\n").unwrap();
    assert!(RepoRules::load(&dir).error.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_trailing_star_covers_the_bare_command() {
    let rules = set(
        vec![command("cargo test *", Verdict::Allow)],
        vec![],
        vec![],
    );
    assert_eq!(shell(&rules, "cargo test"), Some(Verdict::Allow));
    assert_eq!(shell(&rules, "cargo testing"), None);
}

#[test]
fn a_compound_command_is_allowed_only_when_every_piece_is() {
    let rules = set(
        vec![
            command("cargo test *", Verdict::Allow),
            command("cargo fmt *", Verdict::Allow),
            command("rm -rf *", Verdict::Deny),
        ],
        vec![],
        vec![],
    );
    assert_eq!(
        shell(&rules, "cargo fmt && cargo test"),
        Some(Verdict::Allow)
    );
    assert_eq!(
        shell(&rules, "cargo fmt; cargo build"),
        None,
        "build is not allowed"
    );
    assert_eq!(shell(&rules, "cargo test || rm -rf /"), Some(Verdict::Deny));
    assert_eq!(shell(&rules, "cargo test | rm -rf x"), Some(Verdict::Deny));
}

#[test]
fn hidden_commands_never_allow_but_are_still_denied() {
    let rules = set(
        vec![
            command("echo *", Verdict::Allow),
            command("rm -rf *", Verdict::Deny),
        ],
        vec![],
        vec![],
    );
    assert_eq!(shell(&rules, "echo $(whoami)"), None);
    assert_eq!(shell(&rules, "echo `rm -rf /`"), Some(Verdict::Deny));
    assert_eq!(shell(&rules, "echo \"$(rm -rf /)\""), Some(Verdict::Deny));
    assert_eq!(shell(&rules, "sh -c 'rm -rf /'"), Some(Verdict::Deny));
    assert_eq!(shell(&rules, "eval rm -rf /"), Some(Verdict::Deny));
    assert_eq!(
        shell(&rules, "bash -c 'echo hi'"),
        None,
        "a shell string never allows"
    );
}

#[test]
fn wrappers_and_assignments_come_off_and_quotes_go() {
    let (pieces, opaque) = simple_commands("sudo -E env FOO=1 cargo test 'a b' 2>&1");
    assert_eq!(pieces, vec!["cargo test a b 2>&1"]);
    assert!(!opaque);
    let (pieces, _) = simple_commands("RUST_LOG=debug  nohup   cargo run");
    assert_eq!(pieces, vec!["cargo run"]);
    let (_, opaque) = simple_commands("cat <<EOF\nhi\nEOF");
    assert!(opaque);
}

#[test]
fn paths_are_anchored_at_the_worktree_and_normalised() {
    let root = Path::new("/work/tree");
    let rules = set(
        vec![
            path("edit", "**/.env", Verdict::Deny),
            path("edit", "src/**", Verdict::Allow),
            path("Read", "~/.ssh/**", Verdict::Deny),
            path("Read", "/etc/*", Verdict::Ask),
        ],
        vec![],
        vec![],
    );
    assert_eq!(
        file(&rules, "Edit", "src/main.rs", root),
        Some(Verdict::Allow)
    );
    assert_eq!(
        file(&rules, "Write", "/work/tree/src/a/b.rs", root),
        Some(Verdict::Allow)
    );
    assert_eq!(
        file(&rules, "Edit", "src/../.env", root),
        Some(Verdict::Deny)
    );
    assert_eq!(file(&rules, "Edit", "src/../../other/x", root), None);
    assert_eq!(
        file(&rules, "Read", "~/.ssh/id_ed25519", root),
        Some(Verdict::Deny)
    );
    assert_eq!(file(&rules, "Read", "/etc/hosts", root), Some(Verdict::Ask));
    assert_eq!(
        file(&rules, "Read", "/etc/ssh/sshd_config", root),
        None,
        "* stops at /"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_neither_dodges_a_deny_nor_borrows_an_allow() {
    let base = std::env::temp_dir().join(format!("ket-perm-link-{}", std::process::id()));
    let tree = base.join("tree");
    let secrets = base.join("secrets");
    std::fs::create_dir_all(tree.join("src")).unwrap();
    std::fs::create_dir_all(&secrets).unwrap();
    std::os::unix::fs::symlink(&secrets, tree.join("src/linked")).unwrap();
    let tree = tree.canonicalize().unwrap();
    let secrets = secrets.canonicalize().unwrap();
    let rules = set(
        vec![
            path("edit", &format!("{}/**", secrets.display()), Verdict::Deny),
            path("edit", "src/**", Verdict::Allow),
        ],
        vec![],
        vec![],
    );
    assert_eq!(
        file(&rules, "Edit", "src/linked/key", &tree),
        Some(Verdict::Deny)
    );
    let only_allow = set(vec![path("edit", "src/**", Verdict::Allow)], vec![], vec![]);
    assert_eq!(file(&only_allow, "Edit", "src/linked/key", &tree), None);

    let outside = set(
        vec![Rule {
            outside_worktree: true,
            ..rule("edit", Verdict::Ask)
        }],
        vec![],
        vec![],
    );
    assert_eq!(
        file(&outside, "Edit", "src/linked/key", &tree),
        Some(Verdict::Ask)
    );
    assert_eq!(file(&outside, "Edit", "src/real.rs", &tree), None);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn domains_and_tool_groups_match() {
    let rules = set(
        vec![
            Rule {
                domain: Patterns::one("*.corp.example.com"),
                ..rule("WebFetch", Verdict::Ask)
            },
            rule("mcp", Verdict::Deny),
        ],
        vec![],
        vec![],
    );
    let fetch = |url: &str| {
        let input = json!({ "url": url });
        rules
            .evaluate(&Request {
                tool: "WebFetch",
                input: &input,
                worktree: None,
                home: None,
            })
            .map(|r| r.verdict)
    };
    assert_eq!(
        fetch("https://wiki.corp.example.com/a?b"),
        Some(Verdict::Ask)
    );
    assert_eq!(
        fetch("https://User@WIKI.CORP.EXAMPLE.COM:8443/"),
        Some(Verdict::Ask)
    );
    assert_eq!(fetch("https://example.com/"), None);
    let input = json!({});
    let mcp = rules.evaluate(&Request {
        tool: "mcp__github__create_issue",
        input: &input,
        worktree: None,
        home: None,
    });
    assert_eq!(mcp.map(|r| r.verdict), Some(Verdict::Deny));
}

#[test]
fn the_guardrails_ask_about_the_rule_files_and_can_be_turned_off() {
    let file_on = RuleFile::default();
    let rules = RuleSet::from_parts(&file_on, None, vec![], Some(Path::new("/home/me")));
    let root = Path::new("/work/tree");
    assert_eq!(file(&rules, "Edit", ".ket.toml", root), Some(Verdict::Ask));
    assert_eq!(
        file(&rules, "Write", "/home/me/.claude/settings.json", root),
        Some(Verdict::Ask)
    );
    // Reading the rules is fine; changing them is asked about.
    assert_eq!(shell(&rules, "cat ~/.config/ket/permissions.toml"), None);
    assert_eq!(shell(&rules, "grep -rn ket-agent-hook crates/"), None);
    assert_eq!(
        shell(&rules, "rm ~/.config/ket/permissions.toml"),
        Some(Verdict::Ask)
    );
    assert_eq!(
        shell(&rules, "echo x > ~/.config/ket/permissions.toml"),
        Some(Verdict::Ask)
    );
    assert_eq!(
        shell(
            &rules,
            "sed -i '' 's/a/b/' ~/.ket/agent-hooks/ket-agent-hook.sh"
        ),
        Some(Verdict::Ask)
    );
    assert_eq!(file(&rules, "Edit", "src/main.rs", root), None);
    let off = RuleFile {
        builtins: false,
        ..RuleFile::default()
    };
    let rules = RuleSet::from_parts(&off, None, vec![], Some(Path::new("/home/me")));
    assert_eq!(file(&rules, "Edit", ".ket.toml", root), None);
}

#[test]
fn a_rule_says_one_thing_about_a_call() {
    let two = Rule {
        command: Patterns::one("x"),
        path: Patterns::one("y"),
        ..rule("Bash", Verdict::Deny)
    };
    assert!(two.validate().is_err());
    assert!(rule("Bash", Verdict::Deny).validate().is_ok());
    let parsed: RuleFile = toml::from_str(
        "[[rules]]\ntool = [\"Edit\", \"Write\"]\npath = \"src/**\"\ndecision = \"allow\"\n",
    )
    .unwrap();
    assert_eq!(parsed.rules[0].tool.0, vec!["Edit", "Write"]);
    assert!(
        toml::from_str::<RuleFile>("[[rules]]\ntool = \"Bash\"\ndecision = \"maybe\"\n").is_err()
    );
}

#[test]
fn claude_is_told_a_deny_or_ask_before_and_an_allow_once_it_asks() {
    let ruling = |verdict| Ruling {
        verdict,
        scope: Scope::Repo,
        index: 2,
        reason: Some("Infra goes through CI".to_owned()),
    };
    let pre = claude_output("PreToolUse", &ruling(Verdict::Deny)).unwrap();
    let (line, answer) = pre.split_once('\n').unwrap();
    let line: Ruling = serde_json::from_str(line).unwrap();
    assert_eq!(line.verdict, Verdict::Deny);
    let answer: serde_json::Value = serde_json::from_str(answer).unwrap();
    assert_eq!(answer["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecisionReason"],
        "ket: Infra goes through CI (repository rule 3)"
    );
    assert!(claude_output("PreToolUse", &ruling(Verdict::Ask)).is_some());
    assert!(claude_output("PreToolUse", &ruling(Verdict::Allow)).is_none());

    let allow = claude_output("PermissionRequest", &ruling(Verdict::Allow)).unwrap();
    let answer: serde_json::Value =
        serde_json::from_str(allow.split_once('\n').unwrap().1).unwrap();
    assert_eq!(
        answer["hookSpecificOutput"]["decision"]["behavior"],
        "allow"
    );
    assert!(claude_output("PermissionRequest", &ruling(Verdict::Ask)).is_none());
    assert!(claude_output("PostToolUse", &ruling(Verdict::Deny)).is_none());
}

fn refused(stdout: &str) -> crate::hook::Blocked {
    crate::hook::Blocked {
        index: 1,
        command: "sh gate.sh".to_owned(),
        timeout_secs: 30,
        exit: crate::hook::HookExit {
            exit_code: Some(1),
            timed_out: false,
            stdout: stdout.to_owned(),
        },
    }
}

#[test]
fn a_refusing_hook_denies_with_its_first_line() {
    let ruling = hook_refusal(&refused("\n  no pushes from agents  \nsecond line\n"));
    assert_eq!(ruling.verdict, Verdict::Deny);
    assert_eq!(ruling.scope, Scope::Hook);
    assert_eq!(ruling.index, 1);
    assert_eq!(ruling.reason.as_deref(), Some("no pushes from agents"));
    assert_eq!(ruling.explanation(), "ket: no pushes from agents (hook 2)");

    let long = hook_refusal(&refused(&"é".repeat(HOOK_REASON_MAX + 50)));
    assert_eq!(
        long.reason.unwrap().chars().count(),
        HOOK_REASON_MAX,
        "cut on a character, not a byte"
    );

    let silent = hook_refusal(&refused("  \n"));
    assert_eq!(silent.reason.as_deref(), Some("blocked by hook sh gate.sh"));
}

#[test]
fn claude_is_told_why_a_hook_refused_its_prompt() {
    let ruling = hook_refusal(&refused("refused by my hook\n"));
    let printed = claude_output("PermissionRequest", &ruling).unwrap();
    let (line, answer) = printed.split_once('\n').unwrap();
    let line: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(line["scope"], "hook");
    assert_eq!(line["verdict"], "deny");
    let answer: serde_json::Value = serde_json::from_str(answer).unwrap();
    assert_eq!(answer["hookSpecificOutput"]["decision"]["behavior"], "deny");
    assert_eq!(
        answer["hookSpecificOutput"]["decision"]["message"],
        "ket: refused by my hook (hook 2)"
    );
}

#[test]
fn a_hook_is_named_by_its_place_not_as_a_rule() {
    let ruling = Ruling {
        verdict: Verdict::Deny,
        scope: Scope::Hook,
        index: 0,
        reason: None,
    };
    assert_eq!(ruling.explanation(), "ket: hook 1");
    assert_eq!(Scope::Hook.label(), "hook");
}

#[test]
fn only_claude_and_only_a_call_are_ruled() {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "AskUserQuestion",
        "tool_input": {},
    });
    assert!(hook_answer(&payload, "claude").is_none());
    let bash = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "ls" },
    });
    assert!(hook_answer(&bash, "codex").is_none());
    let done = json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash" });
    assert!(hook_answer(&done, "claude").is_none());
}

fn asked(tool: &str, subject: &str) -> crate::agent_hooks::PermissionQuestion {
    crate::agent_hooks::PermissionQuestion {
        tool: tool.to_owned(),
        subject: Some(subject.to_owned()),
        id: 0,
        prompt: None,
    }
}

#[test]
fn an_answer_suggests_the_rule_a_person_would_write() {
    let root = Path::new("/work/tree");
    let bash = suggest(
        &asked("Bash", "cargo test --all"),
        Some(root),
        Verdict::Allow,
    )
    .unwrap();
    assert_eq!(bash.command.0, vec!["cargo test *"]);
    assert_eq!(bash.decision, Verdict::Allow);
    let ls = suggest(&asked("Bash", "ls -la"), Some(root), Verdict::Deny).unwrap();
    assert_eq!(ls.command.0, vec!["ls *"]);
    assert!(
        suggest(
            &asked("Bash", "cargo fmt && cargo test"),
            Some(root),
            Verdict::Allow
        )
        .is_none()
    );
    assert!(suggest(&asked("Bash", "echo $(id)"), Some(root), Verdict::Allow).is_none());

    let edit = suggest(
        &asked("Edit", "/work/tree/src/ui/a.rs"),
        Some(root),
        Verdict::Allow,
    )
    .unwrap();
    assert_eq!(edit.path.0, vec!["src/ui/**"]);
    let top = suggest(&asked("Write", "README.md"), Some(root), Verdict::Allow).unwrap();
    assert_eq!(top.path.0, vec!["README.md"]);
    let outside = suggest(&asked("Read", "/etc/hosts"), Some(root), Verdict::Allow).unwrap();
    assert_eq!(outside.path.0, vec!["/etc/hosts"]);

    let fetch = suggest(
        &asked("WebFetch", "https://docs.rs/serde"),
        Some(root),
        Verdict::Allow,
    )
    .unwrap();
    assert_eq!(fetch.domain.0, vec!["docs.rs"]);
    let mcp = suggest(&asked("mcp__github__list", "x"), Some(root), Verdict::Allow).unwrap();
    assert_eq!(mcp.tool.0, vec!["mcp__github__list"]);
    assert!(mcp.command.is_empty() && mcp.path.is_empty());
    assert!(
        suggest(
            &asked("AskUserQuestion", "Which?"),
            Some(root),
            Verdict::Allow
        )
        .is_none()
    );
}
