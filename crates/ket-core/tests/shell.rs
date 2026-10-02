//! Launching a command the way the person at the keyboard would launch it:
//! `ShellKind::of`, `quote`, `line`, `launch_in`, `ensure_wrappers_in`.
//!
//! `probe`/`find_on_login_path` are covered only on the fast path that never
//! spawns a shell (a name already on `PATH`, or an absolute path). The
//! shell-spawning fallback (`ask_shell`/`login_path`/`variable_seen_by`) is
//! left alone — not hermetic (a real interactive login shell, up to
//! `PROBE_TIMEOUT` each), and every one of those private helpers already has
//! its own inline unit test for the string-building/parsing logic around it.

use std::fs;

use ket_core::shell::{
    STARTUP_COMMAND_ENV, ShellKind, clear_probe_cache, ensure_wrappers_in, find_on_login_path,
    launch_in, line, probe, quote, variable_seen_by,
};

mod common;
use common::Sandbox;

#[test]
fn shell_kind_recognises_zsh_by_its_executable_name() {
    assert_eq!(ShellKind::of("/bin/zsh"), ShellKind::Zsh);
    assert_eq!(ShellKind::of("zsh"), ShellKind::Zsh);
    assert_eq!(ShellKind::of("/usr/local/bin/ZSH"), ShellKind::Zsh);
}

#[test]
fn shell_kind_treats_everything_else_as_other() {
    assert_eq!(ShellKind::of("/bin/bash"), ShellKind::Other);
    assert_eq!(ShellKind::of("fish"), ShellKind::Other);
    assert_eq!(ShellKind::of(""), ShellKind::Other);
    assert_eq!(ShellKind::of("/bin/sh"), ShellKind::Other);
}

#[test]
fn quote_leaves_plain_words_unquoted() {
    assert_eq!(quote("hello"), "hello");
    assert_eq!(quote("path/to/file.txt"), "path/to/file.txt");
    assert_eq!(quote("a-b_c.d:e=f+g,h@i"), "a-b_c.d:e=f+g,h@i");
}

#[test]
fn quote_wraps_anything_with_shell_metacharacters() {
    assert_eq!(quote("hello world"), "'hello world'");
    assert_eq!(quote("$HOME"), "'$HOME'");
    assert_eq!(quote("a;b"), "'a;b'");
}

#[test]
fn quote_escapes_an_embedded_single_quote() {
    assert_eq!(quote("it's"), "'it'\\''s'");
}

#[test]
fn quote_wraps_an_empty_string() {
    assert_eq!(quote(""), "''");
}

#[test]
fn line_joins_the_command_and_quotes_each_argument() {
    assert_eq!(
        line("claude", &["hello world".to_owned(), "plain".to_owned()]),
        "claude 'hello world' plain"
    );
}

#[test]
fn line_trims_the_command_and_drops_empty_parts() {
    assert_eq!(line("  claude  ", &[]), "claude");
}

#[test]
fn line_with_no_arguments_is_just_the_command() {
    assert_eq!(line("claude", &[]), "claude");
}

#[test]
fn launch_in_with_no_startup_is_a_plain_shell() {
    let launch = launch_in(None, "/bin/zsh", None);
    assert_eq!(launch.command, "/bin/zsh");
    assert!(launch.args.is_empty());
    assert!(launch.env.is_empty());
}

#[test]
fn launch_in_for_a_non_zsh_shell_falls_back_to_dash_i_dash_c() {
    let launch = launch_in(
        Some(std::path::Path::new("/nonexistent")),
        "/bin/bash",
        Some("claude"),
    );
    assert_eq!(launch.command, "/bin/bash");
    assert_eq!(
        launch.args,
        vec!["-i".to_owned(), "-c".to_owned(), "claude".to_owned()]
    );
    assert!(launch.env.is_empty());
}

#[test]
fn launch_in_for_zsh_with_no_root_falls_back_to_dash_i_dash_c() {
    let launch = launch_in(None, "/bin/zsh", Some("claude"));
    assert_eq!(
        launch.args,
        vec!["-i".to_owned(), "-c".to_owned(), "claude".to_owned()]
    );
    assert!(launch.env.is_empty());
}

#[test]
fn launch_in_for_zsh_with_a_writable_root_uses_the_wrapper() {
    let sandbox = Sandbox::new("shell-launch-zsh");
    let root = sandbox.path("wrappers");

    let launch = launch_in(Some(&root), "/bin/zsh", Some("claude hello"));

    assert_eq!(launch.command, "/bin/zsh");
    assert_eq!(launch.args, vec!["-l".to_owned()]);
    assert_eq!(
        launch.env.get(STARTUP_COMMAND_ENV),
        Some(&"claude hello".to_owned())
    );
    assert!(launch.env.contains_key("ZDOTDIR"));
    assert!(launch.env.get("ZDOTDIR").unwrap().contains("wrappers"));
}

#[test]
fn launch_in_for_zsh_writes_the_wrapper_file() {
    let sandbox = Sandbox::new("shell-launch-writes");
    let root = sandbox.path("wrappers");

    launch_in(Some(&root), "/bin/zsh", Some("claude"));

    assert!(root.join("zsh").join(".zshenv").is_file());
}

#[test]
fn ensure_wrappers_in_is_idempotent() {
    let sandbox = Sandbox::new("shell-ensure-idempotent");
    let root = sandbox.path("wrappers");

    let first = ensure_wrappers_in(&root).unwrap();
    let contents_first = fs::read_to_string(root.join("zsh").join(".zshenv")).unwrap();
    let second = ensure_wrappers_in(&root).unwrap();
    let contents_second = fs::read_to_string(root.join("zsh").join(".zshenv")).unwrap();

    assert_eq!(first, second);
    assert_eq!(contents_first, contents_second);
}

#[test]
fn ensure_wrappers_in_rewrites_a_stale_wrapper() {
    let sandbox = Sandbox::new("shell-ensure-stale");
    let root = sandbox.path("wrappers");
    let zshenv = root.join("zsh").join(".zshenv");
    fs::create_dir_all(zshenv.parent().unwrap()).unwrap();
    fs::write(&zshenv, "# stale, from an older ket build\n").unwrap();

    ensure_wrappers_in(&root).unwrap();

    let contents = fs::read_to_string(&zshenv).unwrap();
    assert!(contents.contains("ket zsh startup wrapper"));
    assert!(contents.contains("__ket_line_init"));
}

#[test]
fn ensure_wrappers_in_creates_missing_parent_directories() {
    let sandbox = Sandbox::new("shell-ensure-missing-parents");
    let root = sandbox.path("a/b/c/wrappers");

    let returned = ensure_wrappers_in(&root).unwrap();
    assert_eq!(returned, root);
    assert!(root.join("zsh").join(".zshenv").is_file());
}

#[test]
fn probe_of_an_empty_or_blank_name_is_none_without_spawning_anything() {
    assert_eq!(probe(""), None);
    assert_eq!(probe("   "), None);
}

#[test]
fn probe_finds_a_name_already_on_path_without_spawning_a_shell() {
    // `ls` resolving via `PATH` alone is the fast path in `probe`, which
    // returns before it would ever ask a shell — so this stays hermetic.
    let found = probe("ls").expect("ls should be on PATH in any test environment");
    assert_eq!(found.file_name().unwrap(), "ls");
}

#[test]
fn find_on_login_path_accepts_an_absolute_executable_path() {
    let sandbox = Sandbox::new("shell-find-absolute");
    let script = sandbox.path("my-script.sh");
    fs::write(&script, "#!/bin/sh\n").unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(find_on_login_path(script.to_str().unwrap()), Some(script));
}

#[test]
fn find_on_login_path_refuses_a_non_executable_absolute_path() {
    let sandbox = Sandbox::new("shell-find-absolute-not-exec");
    let file = sandbox.path("not-executable.txt");
    fs::write(&file, "just text\n").unwrap();

    assert_eq!(find_on_login_path(file.to_str().unwrap()), None);
}

#[test]
fn clear_probe_cache_does_not_panic_with_nothing_cached() {
    clear_probe_cache();
}

#[test]
fn variable_seen_by_refuses_a_malformed_binary_or_variable_without_spawning_a_shell() {
    assert_eq!(
        variable_seen_by("claude hi", "claude;rm", "CLAUDE_CONFIG_DIR", &[]),
        None
    );
    assert_eq!(
        variable_seen_by("claude hi", "claude", "HAS SPACE", &[]),
        None
    );
    assert_eq!(
        variable_seen_by("", "claude", "CLAUDE_CONFIG_DIR", &[]),
        None
    );
}
