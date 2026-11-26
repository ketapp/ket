use super::*;

/// A spawner that records instead of running anything.
#[derive(Debug, Default)]
struct Recorder(std::sync::Mutex<Vec<Vec<String>>>);

impl Spawner for Recorder {
    fn spawn(&self, command: &EditorCommand) -> Result<()> {
        self.0.lock().unwrap().push(command.argv_lossy());
        Ok(())
    }
}

fn surface(command_line: &str) -> ExternalSurface {
    ExternalSurface::from_command_line(command_line, Arc::new(Recorder::default())).unwrap()
}

fn argv(command_line: &str, at: &Position) -> Vec<String> {
    surface(command_line).command_for(at).unwrap().argv_lossy()
}

#[test]
fn vs_code_takes_the_position_as_a_suffix_after_goto() {
    let at = Position::file("/w/src/main.rs")
        .with_line(12)
        .with_column(4);
    assert_eq!(argv("code", &at), ["code", "--goto", "/w/src/main.rs:12:4"]);
}

#[test]
fn zed_takes_the_position_as_a_bare_suffix() {
    let at = Position::file("/w/src/main.rs")
        .with_line(12)
        .with_column(4);
    assert_eq!(argv("zed", &at), ["zed", "/w/src/main.rs:12:4"]);
}

#[test]
fn neovim_takes_the_line_as_an_ex_command_before_the_path() {
    let at = Position::file("/w/src/main.rs")
        .with_line(12)
        .with_column(4);
    assert_eq!(argv("nvim", &at), ["nvim", "+12", "/w/src/main.rs"]);
}

#[test]
fn a_missing_column_trims_the_separator_that_would_have_led_to_it() {
    let at = Position::file("/w/a.rs").with_line(9);
    assert_eq!(argv("code", &at), ["code", "--goto", "/w/a.rs:9"]);
    assert_eq!(argv("zed", &at), ["zed", "/w/a.rs:9"]);
    assert_eq!(argv("emacsclient", &at), ["emacsclient", "+9", "/w/a.rs"]);
    assert_eq!(argv("nano", &at), ["nano", "+9", "/w/a.rs"]);
}

#[test]
fn a_missing_line_leaves_only_the_path() {
    let at = Position::file("/w/a.rs");
    assert_eq!(argv("code", &at), ["code", "--goto", "/w/a.rs"]);
    assert_eq!(argv("zed", &at), ["zed", "/w/a.rs"]);
    assert_eq!(argv("nvim", &at), ["nvim", "/w/a.rs"]);
    assert_eq!(argv("emacsclient", &at), ["emacsclient", "/w/a.rs"]);
    assert_eq!(argv("mate", &at), ["mate", "/w/a.rs"]);
}

#[test]
fn a_flag_is_dropped_along_with_the_value_it_introduces() {
    // The shape rule 3 exists for: `--column` without a column is worse than
    // no flag, and JetBrains editors reject it outright.
    let full = Position::file("/w/a.rs").with_line(3).with_column(7);
    assert_eq!(
        argv("idea", &full),
        ["idea", "--line", "3", "--column", "7", "/w/a.rs"]
    );

    let line_only = Position::file("/w/a.rs").with_line(3);
    assert_eq!(argv("idea", &line_only), ["idea", "--line", "3", "/w/a.rs"]);

    let bare = Position::file("/w/a.rs");
    assert_eq!(argv("idea", &bare), ["idea", "/w/a.rs"]);
}

#[test]
fn an_unknown_editor_opens_the_file_rather_than_failing() {
    let at = Position::file("/w/a.rs").with_line(3).with_column(7);
    let s = surface("acme");
    assert!(!s.knows_positions());
    assert_eq!(
        s.command_for(&at).unwrap().argv_lossy(),
        ["acme", "/w/a.rs"]
    );
    assert_eq!(s.describe(), "acme (file only)");
}

#[test]
fn the_editor_is_matched_by_the_binarys_name_not_its_path() {
    let at = Position::file("/w/a.rs").with_line(3);
    assert_eq!(
        argv("/opt/homebrew/bin/zed", &at),
        ["/opt/homebrew/bin/zed", "/w/a.rs:3"]
    );
}

#[test]
fn a_users_own_arguments_survive_in_front_of_the_template() {
    let at = Position::file("/w/a.rs").with_line(3);
    assert_eq!(
        argv("code --wait --new-window", &at),
        ["code", "--wait", "--new-window", "--goto", "/w/a.rs:3"]
    );
}

#[test]
fn placeholders_in_the_editor_variable_replace_the_profile_entirely() {
    let at = Position::file("/w/a.rs").with_line(3).with_column(7);
    assert_eq!(
        argv("acme --file {path} --line {line} --col {col}", &at),
        ["acme", "--file", "/w/a.rs", "--line", "3", "--col", "7"]
    );

    let bare = Position::file("/w/a.rs");
    assert_eq!(
        argv("acme --file {path} --line {line} --col {col}", &bare),
        ["acme", "--file", "/w/a.rs"]
    );
}

#[test]
fn a_quoted_path_with_a_space_stays_one_argument() {
    let editor = "\"/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl\" --wait";
    let at = Position::file("/w/a.rs").with_line(3);
    assert_eq!(
        argv(editor, &at),
        [
            "/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl",
            "--wait",
            "/w/a.rs:3",
        ]
    );
}

#[test]
fn splitting_handles_quotes_escapes_and_runs_of_whitespace() {
    assert_eq!(split_command_line("code  --wait"), ["code", "--wait"]);
    assert_eq!(split_command_line("'a b' c"), ["a b", "c"]);
    assert_eq!(split_command_line("a\\ b"), ["a b"]);
    assert_eq!(split_command_line("\"a\\\"b\""), ["a\"b"]);
    assert_eq!(split_command_line("   "), Vec::<String>::new());
    // Unterminated quote closes at the end rather than erroring.
    assert_eq!(split_command_line("code \"a b"), ["code", "a b"]);
}

#[test]
fn a_column_without_a_line_is_dropped_rather_than_rendered_as_a_dangling_separator() {
    let at = Position::file("/w/a.rs").with_column(7);
    assert_eq!(at.col, None);
    assert_eq!(argv("zed", &at), ["zed", "/w/a.rs"]);
}

#[test]
fn a_zero_line_is_clamped_because_every_editor_here_is_one_based() {
    let at = Position::file("/w/a.rs").with_line(0).with_column(0);
    assert_eq!((at.line, at.col), (Some(1), Some(1)));
}

#[test]
fn a_relative_path_is_refused_rather_than_resolved_against_an_unknown_directory() {
    let at = Position::file("src/main.rs").with_line(3);
    assert!(matches!(
        surface("code").command_for(&at),
        Err(KetError::Path { .. })
    ));
}

#[test]
fn revealing_spawns_exactly_the_command_that_was_computed() {
    // The seam: what `reveal` runs is what `command_for` says it runs.
    let recorder = Arc::new(Recorder::default());
    let surface =
        ExternalSurface::from_command_line("code --wait", Arc::clone(&recorder) as Arc<_>).unwrap();
    let at = Position::file("/w/a.rs").with_line(3).with_column(7);

    surface.reveal(&at).unwrap();

    assert_eq!(
        recorder.0.lock().unwrap().as_slice(),
        [surface.command_for(&at).unwrap().argv_lossy()]
    );
}

#[test]
fn the_external_surface_is_not_embedded_and_does_not_render_diffs() {
    // Both are why `crate::diff` exists and why the shell draws the review.
    let caps = surface("code").caps();
    assert_eq!(
        caps,
        SurfaceCaps {
            embedded: false,
            editable: true,
            diffs: false,
        }
    );
}

#[test]
fn a_worktree_relative_path_is_joined_onto_the_root() {
    let at = Position::in_worktree("/w", "src/main.rs");
    assert_eq!(at.path, Path::new("/w/src/main.rs"));
}

#[test]
fn every_profile_renders_a_command_for_every_amount_of_position() {
    // The table is data, and the rules must hold for all of it: whatever a
    // row says, the path always survives.
    for profile in PROFILES {
        let s = surface(profile.program);
        for at in [
            Position::file("/w/a.rs"),
            Position::file("/w/a.rs").with_line(4),
            Position::file("/w/a.rs").with_line(4).with_column(2),
        ] {
            let argv = s.command_for(&at).unwrap().argv_lossy();
            assert!(
                argv.iter().any(|a| a.contains("/w/a.rs")),
                "{}: lost the path: {argv:?}",
                profile.program
            );
            assert!(
                !argv.iter().any(|a| a.contains('{')),
                "{}: unsubstituted placeholder: {argv:?}",
                profile.program
            );
        }
    }
}

#[test]
fn no_two_profiles_claim_the_same_program() {
    let mut seen = std::collections::BTreeSet::new();
    for profile in PROFILES {
        assert!(
            seen.insert(profile.program),
            "duplicate: {}",
            profile.program
        );
    }
}

#[test]
fn braces_that_are_not_placeholders_are_left_alone() {
    let at = Position::file("/w/a.rs").with_line(3);
    assert_eq!(
        argv("acme {path} {nope}", &at),
        ["acme", "/w/a.rs", "{nope}"]
    );
}

#[test]
fn git_churn_and_our_own_temporaries_are_not_reported_as_changes() {
    assert!(is_noise(Path::new("/w/.git/index.lock")));
    assert!(is_noise(Path::new("/w/.git")));
    assert!(is_noise(Path::new("/w/.ket-tmp.900.main.rs")));
    assert!(!is_noise(Path::new("/w/src/main.rs")));
    assert!(!is_noise(Path::new("/w/gitignore")));
}

#[test]
fn reading_a_file_is_not_a_change() {
    use notify::event::AccessKind;
    assert_eq!(classify(notify::EventKind::Access(AccessKind::Any)), None);
}

/// A throwaway directory, removed when the guard drops.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ket-surface-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // macOS puts the temp dir behind /private; canonicalise so paths
        // compared against what the watcher reports are the same ones.
        let dir = dir.canonicalize().unwrap_or(dir);
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn file_stamp_of_bytes_matches_a_stamp_taken_from_disk() {
    let dir = TempDir::new("stamp");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"hello").unwrap();

    let from_disk = FileStamp::of(&path).unwrap();
    let from_bytes = FileStamp::of_bytes(b"hello", from_disk.modified_ms);
    assert_eq!(from_disk, from_bytes);
}

#[test]
fn file_stamp_of_missing_file_is_an_error() {
    let dir = TempDir::new("stamp-missing");
    assert!(FileStamp::of(&dir.0.join("nope.txt")).is_err());
}

#[test]
fn different_contents_never_share_a_digest() {
    let a = FileStamp::of_bytes(b"one", None);
    let b = FileStamp::of_bytes(b"two", None);
    assert_ne!(a.digest, b.digest);
}

#[test]
fn metadata_matches_is_false_once_the_file_is_gone() {
    let dir = TempDir::new("stamp-gone");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"hello").unwrap();
    let stamp = FileStamp::of(&path).unwrap();

    std::fs::remove_file(&path).unwrap();
    assert!(!stamp.metadata_matches(&path));
}

#[test]
fn metadata_matches_is_false_without_a_modification_time() {
    let stamp = FileStamp {
        len: 5,
        modified_ms: None,
        digest: 0,
    };
    let dir = TempDir::new("stamp-no-mtime");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"hello").unwrap();
    assert!(!stamp.metadata_matches(&path));
}

#[test]
fn write_if_unchanged_writes_when_the_stamp_still_matches() {
    let dir = TempDir::new("write-ok");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"before").unwrap();
    let stamp = FileStamp::of(&path).unwrap();

    let new_stamp = write_if_unchanged(&path, &stamp, b"after").unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"after");
    assert_eq!(new_stamp, FileStamp::of(&path).unwrap());
}

#[test]
fn write_if_unchanged_refuses_when_the_file_changed_since_it_was_read() {
    let dir = TempDir::new("write-changed");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"before").unwrap();
    let stamp = FileStamp::of(&path).unwrap();

    // Someone else writes in between.
    std::fs::write(&path, b"someone else's edit").unwrap();

    let err = write_if_unchanged(&path, &stamp, b"after").expect_err("should refuse");
    assert!(matches!(err, KetError::Conflict(_)));
    assert_eq!(std::fs::read(&path).unwrap(), b"someone else's edit");
}

#[test]
fn write_if_unchanged_refuses_when_the_file_was_deleted() {
    let dir = TempDir::new("write-deleted");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"before").unwrap();
    let stamp = FileStamp::of(&path).unwrap();

    std::fs::remove_file(&path).unwrap();

    let err = write_if_unchanged(&path, &stamp, b"after").expect_err("should refuse");
    assert!(matches!(err, KetError::Conflict(_)));
    assert!(err.to_string().contains("deleted"));
}

#[test]
fn write_if_unchanged_leaves_no_temp_file_behind_on_success() {
    let dir = TempDir::new("write-no-temp");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"before").unwrap();
    let stamp = FileStamp::of(&path).unwrap();

    write_if_unchanged(&path, &stamp, b"after").unwrap();

    let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(TEMP_PREFIX))
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "left a temp file behind: {leftovers:?}"
    );
}

#[test]
fn read_returns_bytes_and_a_matching_stamp() {
    let dir = TempDir::new("read");
    let path = dir.0.join("file.txt");
    std::fs::write(&path, b"contents").unwrap();

    let (bytes, stamp) = read(&path).unwrap();
    assert_eq!(bytes, b"contents");
    assert_eq!(stamp, FileStamp::of(&path).unwrap());
}

#[test]
fn read_of_a_missing_file_is_an_error() {
    let dir = TempDir::new("read-missing");
    assert!(read(&dir.0.join("nope.txt")).is_err());
}

#[test]
fn a_watcher_reports_a_new_file_as_a_change() {
    let dir = TempDir::new("watch-create");
    let watcher = FileWatcher::watch(&dir.0).unwrap();
    assert_eq!(watcher.root(), dir.0.as_path());
    assert!(!watcher.overflowed());

    std::fs::write(dir.0.join("new.txt"), b"hi").unwrap();

    let change = watcher.next_within(Duration::from_secs(5));
    assert!(change.is_some(), "expected a change to be reported");
}

#[test]
fn a_watcher_ignores_git_internals() {
    let dir = TempDir::new("watch-git");
    std::fs::create_dir(dir.0.join(".git")).unwrap();
    let watcher = FileWatcher::watch(&dir.0).unwrap();

    std::fs::write(dir.0.join(".git").join("index.lock"), b"x").unwrap();
    std::fs::write(dir.0.join("real.txt"), b"hi").unwrap();

    // Whatever arrives, it must be the real file, never the git-internal one.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_real = false;
    while std::time::Instant::now() < deadline {
        match watcher.try_next() {
            Some(change) => {
                assert!(
                    !change.path.starts_with(dir.0.join(".git")),
                    "reported a git-internal path: {change:?}"
                );
                if change.path == dir.0.join("real.txt") {
                    saw_real = true;
                    break;
                }
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    assert!(saw_real, "never saw the real file reported");
}

#[test]
fn drain_collects_every_pending_change_at_once() {
    let dir = TempDir::new("watch-drain");
    let watcher = FileWatcher::watch(&dir.0).unwrap();

    std::fs::write(dir.0.join("a.txt"), b"a").unwrap();
    std::fs::write(dir.0.join("b.txt"), b"b").unwrap();

    // Give the watcher time to observe both before draining.
    let _ = watcher.next_within(Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(200));

    let drained = watcher.drain();
    assert!(!drained.is_empty());
    assert!(watcher.try_next().is_none(), "drain must take everything");
}

#[test]
fn a_file_change_debug_names_its_path_and_kind() {
    let change = FileChange {
        path: PathBuf::from("/w/file.txt"),
        kind: FileChangeKind::Modified,
        at_ms: 0,
    };
    assert!(format!("{change:?}").contains("file.txt"));
}

#[test]
fn profile_for_matches_the_file_stem_case_insensitively() {
    assert_eq!(
        profile_for(Path::new("/usr/local/bin/CODE")).unwrap().label,
        "VS Code"
    );
    assert_eq!(profile_for(Path::new("zed")).unwrap().label, "Zed");
    assert!(profile_for(Path::new("not-a-real-editor")).is_none());
}

#[test]
fn editor_command_argv_lossy_puts_the_program_first() {
    let command = EditorCommand {
        program: OsString::from("code"),
        args: vec![OsString::from("--goto"), OsString::from("/w/a.rs:1:1")],
    };
    assert_eq!(command.argv_lossy(), ["code", "--goto", "/w/a.rs:1:1"]);
}

#[test]
fn knows_positions_is_true_for_a_matched_profile_or_a_custom_template() {
    assert!(surface("code").knows_positions());
    assert!(surface("some-unknown-thing {path}:{line}").knows_positions());
    assert!(!surface("some-unknown-thing").knows_positions());
}

#[test]
fn describe_notes_file_only_for_an_editor_with_no_position_template() {
    assert_eq!(surface("code").describe(), "code");
    assert_eq!(
        surface("some-unknown-thing").describe(),
        "some-unknown-thing (file only)"
    );
}

#[test]
fn for_program_matches_the_profile_from_an_unquoted_bundle_path() {
    let s = ExternalSurface::for_program(
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        Arc::new(Recorder::default()),
    );
    assert_eq!(s.profile().unwrap().label, "VS Code");
}

#[test]
fn from_command_line_refuses_an_empty_string() {
    let err = ExternalSurface::from_command_line("   ", Arc::new(Recorder::default()))
        .expect_err("should refuse");
    assert!(err.to_string().contains("empty"));
}

#[test]
fn with_lookup_skips_a_blank_variable_and_falls_through_to_the_next() {
    let lookup = |key: &str| -> Option<OsString> {
        match key {
            "KET_EDITOR" => Some(OsString::from("  ")),
            "EDITOR" => Some(OsString::from("zed")),
            _ => None,
        }
    };
    let s = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default())).unwrap();
    assert_eq!(s.profile().unwrap().label, "Zed");
}

#[test]
fn with_lookup_refuses_non_utf8_and_names_the_variable() {
    use std::os::unix::ffi::OsStringExt;
    let lookup = |key: &str| -> Option<OsString> {
        (key == "KET_EDITOR").then(|| OsString::from_vec(vec![0xff, 0xfe]))
    };
    let err = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default()))
        .expect_err("should refuse");
    assert!(err.to_string().contains("KET_EDITOR"));
}

#[test]
fn with_lookup_names_every_variable_it_tried_when_none_is_set() {
    let lookup = |_: &str| -> Option<OsString> { None };
    let err = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default()))
        .expect_err("should refuse");
    for var in EDITOR_VARS {
        assert!(err.to_string().contains(var), "{err} missing {var}");
    }
}

#[test]
fn split_command_line_closes_an_unterminated_quote_at_the_end() {
    assert_eq!(split_command_line("code \"a b"), ["code", "a b"]);
}

#[test]
fn split_command_line_of_only_whitespace_is_empty() {
    assert!(split_command_line("   ").is_empty());
}

#[test]
fn classify_maps_every_notify_event_kind() {
    use notify::EventKind as K;
    use notify::event::{ModifyKind, RenameMode};

    assert_eq!(
        classify(K::Create(notify::event::CreateKind::Any)),
        Some(FileChangeKind::Created)
    );
    assert_eq!(
        classify(K::Remove(notify::event::RemoveKind::Any)),
        Some(FileChangeKind::Removed)
    );
    assert_eq!(
        classify(K::Modify(ModifyKind::Name(RenameMode::From))),
        Some(FileChangeKind::Removed)
    );
    assert_eq!(
        classify(K::Modify(ModifyKind::Name(RenameMode::To))),
        Some(FileChangeKind::Created)
    );
    assert_eq!(
        classify(K::Modify(ModifyKind::Data(notify::event::DataChange::Any))),
        Some(FileChangeKind::Modified)
    );
    assert_eq!(classify(K::Any), Some(FileChangeKind::Modified));
    assert_eq!(classify(K::Access(notify::event::AccessKind::Any)), None);
    assert_eq!(classify(K::Other), None);
}

#[test]
fn is_noise_flags_a_temp_file_and_a_deeply_nested_git_path() {
    assert!(is_noise(Path::new("/w/.ket-tmp.123.foo")));
    assert!(is_noise(Path::new("/w/.git/refs/heads/main")));
    assert!(!is_noise(Path::new("/w/src/main.rs")));
}

#[test]
fn file_manager_command_reveals_with_open_dash_r() {
    let command = file_manager_command(Path::new("/w/a.rs")).unwrap();
    assert_eq!(command.argv_lossy(), ["/usr/bin/open", "-R", "/w/a.rs"]);
}

#[test]
fn a_fresh_watcher_reports_its_canonicalised_root_and_starts_clean() {
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = Temp(std::env::temp_dir().join(format!("surface-watch-root-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();

    let watcher = FileWatcher::watch(&dir.0).unwrap();
    assert_eq!(watcher.root(), dir.0.canonicalize().unwrap());
    // Not "nothing pending": on Linux, notify can report a stray change on
    // a directory nobody touched, which failed CI. Starting clean means no
    // dropped events to account for.
    assert!(!watcher.overflowed());
}
