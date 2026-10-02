//! The pieces of `Config` not already covered by the inline `#[cfg(test)]`
//! module in `src/config.rs`: `AgentSpec`'s launch-line resolution,
//! `HookConfig::by_point`, `TelemetryConfig::env`, `OpenCodeGoConfig`, and
//! `ProvisionConfig::for_repo`.

use std::collections::BTreeMap;
use std::fs;

use ket_core::config::{
    AgentLaunch, AgentSpec, HookConfig, OpenCodeGoConfig, ProvisionConfig, TelemetryConfig,
    Transport,
};

mod common;
use common::Sandbox;

fn acp_agent(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_owned(),
        transport: Transport::Acp,
        command: format!("{name}-adapter"),
        args: vec!["acp".to_owned()],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        launch: None,
    }
}

fn pty_agent(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_owned(),
        transport: Transport::Pty,
        command: name.to_owned(),
        args: vec!["--flag".to_owned()],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        launch: None,
    }
}

// ---- AgentSpec::launch_command / launch_args / launch_line ------------------

#[test]
fn an_acp_agent_with_no_launch_override_opens_by_its_own_name() {
    let agent = acp_agent("claude");
    assert_eq!(agent.launch_command(), "claude");
    assert!(agent.launch_args().is_empty());
}

#[test]
fn a_pty_agent_with_no_launch_override_opens_with_its_own_command_and_args() {
    let agent = pty_agent("aider");
    assert_eq!(agent.launch_command(), "aider");
    assert_eq!(agent.launch_args(), ["--flag".to_owned()]);
}

#[test]
fn a_launch_override_wins_over_either_transports_default() {
    let mut agent = acp_agent("claude");
    agent.launch = Some(AgentLaunch {
        command: "claude-personal".to_owned(),
        args: vec!["--resume".to_owned()],
    });
    assert_eq!(agent.launch_command(), "claude-personal");
    assert_eq!(agent.launch_args(), ["--resume".to_owned()]);
}

#[test]
fn launch_line_quotes_arguments_and_appends_extra_ones() {
    let mut agent = pty_agent("aider");
    agent.args = vec!["say hello".to_owned()];
    let line = agent.launch_line(&["extra arg".to_owned()]);
    assert_eq!(line, "aider 'say hello' 'extra arg'");
}

// ---- HookConfig::by_point ----------------------------------------------------

#[test]
fn by_point_names_all_six_lifecycle_points_in_order() {
    let names: Vec<&str> = HookConfig::default()
        .by_point()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        names,
        vec![
            "worktree.created",
            "worktree.provisioned",
            "worktree.removed",
            "agent.started",
            "agent.finished",
            "agent.permission_requested",
        ]
    );
}

#[test]
fn by_point_pairs_each_name_with_its_own_list() {
    let mut hooks = HookConfig::default();
    hooks.agent_started.push(ket_core::config::HookSpec {
        command: vec!["echo".to_owned(), "started".to_owned()],
        timeout_secs: 30,
    });

    let started = hooks
        .by_point()
        .into_iter()
        .find(|(name, _)| *name == "agent.started")
        .unwrap()
        .1;
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0].command,
        vec!["echo".to_owned(), "started".to_owned()]
    );
}

// ---- TelemetryConfig::env -----------------------------------------------------

#[test]
fn telemetry_env_is_empty_without_an_endpoint() {
    assert!(TelemetryConfig::default().env().is_empty());
}

#[test]
fn telemetry_env_is_empty_for_a_blank_endpoint() {
    let telemetry = TelemetryConfig {
        endpoint: Some("   ".to_owned()),
        ..TelemetryConfig::default()
    };
    assert!(telemetry.env().is_empty());
}

#[test]
fn telemetry_env_refuses_a_non_http_endpoint() {
    let telemetry = TelemetryConfig {
        endpoint: Some("ftp://collector:4318".to_owned()),
        ..TelemetryConfig::default()
    };
    assert!(telemetry.env().is_empty());
}

#[test]
fn telemetry_env_refuses_an_unrecognised_protocol() {
    let telemetry = TelemetryConfig {
        endpoint: Some("http://localhost:4318".to_owned()),
        protocol: "carrier-pigeon".to_owned(),
        ..TelemetryConfig::default()
    };
    assert!(telemetry.env().is_empty());
}

#[test]
fn telemetry_env_carries_the_endpoint_and_protocol_when_valid() {
    let telemetry = TelemetryConfig {
        endpoint: Some("http://localhost:4318".to_owned()),
        protocol: "http/protobuf".to_owned(),
        ..TelemetryConfig::default()
    };
    let env = telemetry.env();
    assert_eq!(
        env.get("CLAUDE_CODE_ENABLE_TELEMETRY").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        env.get("OTEL_EXPORTER_OTLP_ENDPOINT").map(String::as_str),
        Some("http://localhost:4318")
    );
    assert_eq!(
        env.get("OTEL_EXPORTER_OTLP_PROTOCOL").map(String::as_str),
        Some("http/protobuf")
    );
    assert_eq!(
        env.get("OTEL_LOGS_EXPORTER").map(String::as_str),
        Some("none")
    );
    assert!(!env.contains_key("OTEL_EXPORTER_OTLP_HEADERS"));
    assert!(!env.contains_key("OTEL_METRIC_EXPORT_INTERVAL"));
}

#[test]
fn telemetry_env_sets_logs_to_otlp_when_enabled() {
    let telemetry = TelemetryConfig {
        endpoint: Some("https://localhost:4318".to_owned()),
        logs: true,
        ..TelemetryConfig::default()
    };
    assert_eq!(
        telemetry
            .env()
            .get("OTEL_LOGS_EXPORTER")
            .map(String::as_str),
        Some("otlp")
    );
}

#[test]
fn telemetry_env_carries_headers_and_export_interval_when_set() {
    let telemetry = TelemetryConfig {
        endpoint: Some("https://localhost:4318".to_owned()),
        headers: Some(" Authorization=Bearer x  ".to_owned()),
        export_interval_ms: Some(5_000),
        ..TelemetryConfig::default()
    };
    let env = telemetry.env();
    assert_eq!(
        env.get("OTEL_EXPORTER_OTLP_HEADERS").map(String::as_str),
        Some("Authorization=Bearer x")
    );
    assert_eq!(
        env.get("OTEL_METRIC_EXPORT_INTERVAL").map(String::as_str),
        Some("5000")
    );
}

#[test]
fn telemetry_env_drops_blank_headers() {
    let telemetry = TelemetryConfig {
        endpoint: Some("https://localhost:4318".to_owned()),
        headers: Some("   ".to_owned()),
        ..TelemetryConfig::default()
    };
    assert!(!telemetry.env().contains_key("OTEL_EXPORTER_OTLP_HEADERS"));
}

// ---- OpenCodeGoConfig::is_configured ------------------------------------------

#[test]
fn opencode_go_is_not_configured_with_nothing_set() {
    assert!(!OpenCodeGoConfig::default().is_configured());
}

#[test]
fn opencode_go_is_configured_by_an_api_key_alone() {
    let config = OpenCodeGoConfig {
        api_key: "sk-test".to_owned(),
        ..OpenCodeGoConfig::default()
    };
    assert!(config.is_configured());
}

#[test]
fn opencode_go_is_configured_by_a_session_cookie_alone() {
    let config = OpenCodeGoConfig {
        session_cookie: "seal-value".to_owned(),
        ..OpenCodeGoConfig::default()
    };
    assert!(config.is_configured());
}

#[test]
fn opencode_go_blank_fields_do_not_count_as_configured() {
    let config = OpenCodeGoConfig {
        api_key: "   ".to_owned(),
        session_cookie: "\n".to_owned(),
        ..OpenCodeGoConfig::default()
    };
    assert!(!config.is_configured());
}

// ---- ProvisionConfig::for_repo ------------------------------------------------

#[test]
fn for_repo_without_a_ket_toml_returns_the_global_config_unchanged() {
    let sandbox = Sandbox::new("config-for-repo-missing");
    let root = sandbox.path("repo");
    fs::create_dir_all(&root).unwrap();

    let global = ProvisionConfig::default();
    let resolved = ProvisionConfig::for_repo(&global, &root).unwrap();
    assert_eq!(resolved, global);
}

#[test]
fn for_repo_with_no_provision_table_returns_the_global_config() {
    let sandbox = Sandbox::new("config-for-repo-no-table");
    let root = sandbox.path("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(".ket.toml"),
        "# nothing about provisioning here\n",
    )
    .unwrap();

    let global = ProvisionConfig::default();
    let resolved = ProvisionConfig::for_repo(&global, &root).unwrap();
    assert_eq!(resolved, global);
}

#[test]
fn for_repo_with_a_provision_table_replaces_the_global_config_entirely() {
    let sandbox = Sandbox::new("config-for-repo-replace");
    let root = sandbox.path("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(".ket.toml"),
        "[provision]\npost_command = [\"npm\", \"install\"]\n",
    )
    .unwrap();

    let global = ProvisionConfig {
        post_command: vec!["should not survive".to_owned()],
        ..ProvisionConfig::default()
    };

    let resolved = ProvisionConfig::for_repo(&global, &root).unwrap();
    assert_eq!(
        resolved.post_command,
        vec!["npm".to_owned(), "install".to_owned()]
    );
}

#[test]
fn for_repo_with_a_malformed_ket_toml_is_an_error() {
    let sandbox = Sandbox::new("config-for-repo-malformed");
    let root = sandbox.path("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".ket.toml"), "this is not valid toml [[[").unwrap();

    let global = ProvisionConfig::default();
    assert!(ProvisionConfig::for_repo(&global, &root).is_err());
}
