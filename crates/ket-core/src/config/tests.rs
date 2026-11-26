use super::*;

mod phones;

#[test]
fn defaults_are_valid() {
    Config::default().validate().unwrap();
}

#[test]
fn existing_configs_ask_about_cold_sessions_above_the_default_threshold() {
    for config in [Config::default(), toml::from_str("[agent]\n").unwrap()] {
        assert!(config.agent.asks_on_cold_resume());
        assert_eq!(config.agent.cold_resume_min_tokens(), 100_000);
    }
}

#[test]
fn muted_flags_survive_a_round_trip_and_are_left_out_until_there_are_some() {
    use crate::network::flags::Kind;

    let config = Config::default();
    assert!(config.agent.network.muted.is_empty());
    assert!(!toml::to_string(&config).unwrap().contains("muted"));

    let config: Config =
        toml::from_str("[agent.network]\nlog = true\nmuted = [\"ip\", \"email\"]\n").unwrap();
    let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert!(restored.agent.network.log);
    assert_eq!(
        restored.agent.network.muted.into_iter().collect::<Vec<_>>(),
        [Kind::Email, Kind::Ip]
    );
    assert!(toml::from_str::<Config>("[agent.network]\nmuted = [\"names\"]\n").is_err());
}

#[test]
fn cold_resume_choices_survive_a_config_round_trip() {
    for (enabled, threshold) in [(false, 250_000), (true, 0)] {
        let config: Config = toml::from_str(&format!(
            "[agent]\ncold_resume = {enabled}\ncold_resume_min_tokens = {threshold}\n"
        ))
        .unwrap();
        config.validate().unwrap();
        let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.agent.asks_on_cold_resume(), enabled);
        assert_eq!(restored.agent.cold_resume_min_tokens(), threshold);
        assert_eq!(restored.agent.cold_resume, Some(enabled));
        assert_eq!(restored.agent.cold_resume_min_tokens, Some(threshold));
    }
}

#[test]
fn sleep_choice_survives_round_trip_and_interval_is_bounded() {
    assert!(!SleepConfig::default().enabled());
    assert!(
        SleepConfig {
            enabled: Some(true),
            ..SleepConfig::default()
        }
        .enabled()
    );
    let mut config: Config =
        toml::from_str("[sleep]\nenabled = false\nafter_minutes = 120\n").unwrap();
    assert!(!config.sleep.enabled());
    assert_eq!(config.sleep.after_minutes, 120);
    config.validate().unwrap();
    config.sleep.after_minutes = 4;
    assert!(config.validate().is_err());
    config.sleep.after_minutes = 1441;
    assert!(config.validate().is_err());
}

#[test]
fn the_shipped_code_size_is_eleven() {
    // The size the window opens at for anyone who never edits a config
    // file, which is nearly everyone.
    let config = Config::default();
    assert_eq!(config.theme.code_font_size, 11);
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
fn a_font_family_is_read_back_as_it_was_written() {
    let config: Config =
        toml::from_str("[theme]\ncode_font = \"Menlo\"\nui_font = \"Avenir\"\n").unwrap();
    config.validate().unwrap();
    assert_eq!(config.theme.code_font.as_deref(), Some("Menlo"));
    assert_eq!(config.theme.ui_font.as_deref(), Some("Avenir"));
    // Neither is written when neither was chosen, so a file that never
    // mentioned a font does not grow two keys on its next save.
    let text = toml::to_string(&Config::default()).unwrap();
    assert!(!text.contains("code_font ="), "{text}");
    assert!(!text.contains("ui_font ="), "{text}");
}

#[test]
fn a_build_dirs_list_ket_wrote_under_the_old_default_moves_to_the_new_one() {
    let root = temp_root("stale-build-dirs");
    let path = root.join("config.toml");

    // Exactly what an earlier ket saved: its own default, spelled out.
    let mut stale = Config::default();
    stale.storage.build_dirs = PREVIOUS_BUILD_DIRS
        .iter()
        .map(|d| (*d).to_owned())
        .collect();
    stale.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();
    assert_eq!(loaded.storage, StorageConfig::default());

    // One the user edited is theirs, however close to the old default.
    let mut chosen = Config::default();
    chosen.storage.build_dirs = vec!["target".to_owned(), "node_modules".to_owned()];
    chosen.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();
    assert_eq!(loaded.storage, chosen.storage);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_blank_font_family_is_refused() {
    let config: Config = toml::from_str("[theme]\ncode_font = \" \"\n").unwrap();
    let err = config.validate().unwrap_err().to_string();
    assert!(err.contains("code_font"), "unhelpful message: {err}");
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
fn the_opencode_pane_runs_its_own_server() {
    // OpenCode 2 runs plugins in its shared background service, where the
    // pane's environment never arrives; `--standalone` puts the pane back
    // on a server of its own. The ACP command above is untouched — ACP
    // already runs one. See `agent_hooks::OpenCodeHooks` for the whole
    // story, and `default_agents` for why this lives on the launch line.
    let config = Config::default();
    let opencode = config.agent("opencode").expect("opencode");
    assert_eq!(opencode.launch_line(&[]), "opencode --standalone");
}

#[test]
fn a_saved_spec_with_no_launch_line_of_its_own_opens_the_shipped_way() {
    // A config an older ket wrote carries the agents it shipped with,
    // launch line and all, and a configured spec beats the shipped one.
    // "No line of my own" has to mean the shipped line, or a shipped
    // launch that changes would never reach anyone with a saved config.
    // The variable they set is still theirs.
    let config: Config = toml::from_str(
            "[[agents]]\nname = \"opencode\"\ntransport = \"acp\"\ncommand = \"opencode\"\nargs = [\"acp\"]\n\
             [agents.env]\nOPENCODE_LOG_LEVEL = \"debug\"\n",
        )
        .unwrap();

    let opencode = config.agent("opencode").expect("opencode");
    assert_eq!(opencode.launch_line(&[]), "opencode --standalone");
    assert_eq!(
        opencode.env.get("OPENCODE_LOG_LEVEL").map(String::as_str),
        Some("debug")
    );
}

#[test]
fn a_launch_line_of_the_persons_own_is_left_alone() {
    let config: Config = toml::from_str(
            "[[agents]]\nname = \"opencode\"\ntransport = \"acp\"\ncommand = \"opencode\"\nargs = [\"acp\"]\n\
             [agents.launch]\ncommand = \"opencode\"\nargs = [\"--print-logs\"]\n",
        )
        .unwrap();

    let opencode = config.agent("opencode").expect("opencode");
    assert_eq!(opencode.launch_line(&[]), "opencode --print-logs");
}

#[test]
fn a_pty_spec_under_a_shipped_name_opens_with_its_own_command() {
    // A pty spec names its interactive program outright; the shipped
    // line belongs to the agent ket drives over ACP, not to this one.
    let config: Config = toml::from_str(
        "[[agents]]\nname = \"opencode\"\ntransport = \"pty\"\ncommand = \"my-opencode\"\n",
    )
    .unwrap();

    let opencode = config.agent("opencode").expect("opencode");
    assert_eq!(opencode.launch_line(&[]), "my-opencode");
}

#[test]
fn a_spec_with_no_launch_line_launches_like_the_shipped_one() {
    let config = Config::default();
    let shipped = config.agent("opencode").expect("opencode");
    let mut saved = shipped.clone();
    saved.launch = None;
    assert!(saved.launches_like(shipped));

    saved.launch = Some(AgentLaunch {
        command: "opencode".to_owned(),
        args: Vec::new(),
    });
    assert!(!saved.launches_like(shipped));
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
    let parsed: Config = toml::from_str("[server]\nbind = \"127.0.0.1\"\nport = 9999\n").unwrap();
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

/// A merge config whose commit template is `template`.
fn template(template: &str) -> MergeConfig {
    MergeConfig {
        commit_template: template.to_owned(),
        ..MergeConfig::default()
    }
}

#[test]
fn issue_is_the_number_a_branch_is_named_for() {
    let merge = template("{issue}");
    assert_eq!(
        merge.commit_message("142-fix-the-login", "main", "w", 1),
        "#142"
    );
    // As a backlog note that is an issue names its branch.
    assert_eq!(
        merge.commit_message("142-fix-the-login-k3f9", "main", "w", 1),
        "#142"
    );
    // A prefix before the last `/` is someone's naming scheme, not the issue.
    assert_eq!(merge.commit_message("sam/142-fix", "main", "w", 1), "#142");
    assert_eq!(merge.commit_message("sam/142", "main", "w", 1), "#142");
}

#[test]
fn issue_is_empty_for_a_branch_not_named_for_one() {
    let merge = template("[{issue}]");
    for branch in ["fix-the-login", "v2", "142fix", "fix-142", "142/fix", ""] {
        assert_eq!(
            merge.commit_message(branch, "main", "w", 1),
            "[]",
            "{branch}"
        );
    }
}

#[test]
fn an_empty_issue_leaves_no_trailing_space() {
    let merge = template("fix: the thing {issue}");
    assert_eq!(
        merge.commit_message("fix-thing", "main", "w", 1),
        "fix: the thing"
    );
    assert_eq!(
        merge.commit_message("9-fix-thing", "main", "w", 1),
        "fix: the thing #9"
    );
}

#[test]
fn issue_sits_beside_the_other_placeholders() {
    let merge = template("{branch} into {base} ({files} files) {issue}");
    assert_eq!(
        merge.commit_message("18-sync", "main", "w", 3),
        "18-sync into main (3 files) #18"
    );
}

#[test]
fn a_template_using_issue_is_valid() {
    let mut config = Config::default();
    config.merge.commit_template = "{branch} {issue}".to_owned();
    config.validate().unwrap();
    assert!(template("{issue}").unknown_placeholders().is_empty());
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
