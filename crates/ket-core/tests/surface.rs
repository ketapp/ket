//! The editing surface against a real filesystem and real processes.
//!
//! Everything here is an adversarial case, because the ordinary case — click a
//! line, the editor opens there — is covered by the argv tests inside the
//! module. What is left is the situations where ket is *not* the only writer:
//! an agent working in the worktree the human has open, an editor saving behind
//! ket's back, a file that disappears mid-review.
//!
//! No real editor is ever launched. The one place a process is genuinely spawned
//! is where the point *is* the process: a binary that does not exist must fail
//! immediately, and one that never exits must not hold anybody up. Both are
//! invariant 4 — every state is escapable.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Sandbox, git, init_repo};
use ket_core::KetError;
use ket_core::surface::{
    EditorCommand, EditorSurface, ExternalSurface, FileChange, FileChangeKind, FileStamp,
    FileWatcher, MAX_PENDING_CHANGES, Position, ProcessSpawner, Spawner, TEMP_PREFIX,
};

/// How long a test will wait for the filesystem to report something.
///
/// Generous: FSEvents and inotify are both asynchronous, and a loaded CI machine
/// is slower than a laptop. A test that fails here is a test that would have
/// failed at ten times the timeout too.
const WATCH_TIMEOUT: Duration = Duration::from_secs(10);

/// A spawner that records the argv instead of running it.
#[derive(Debug, Default)]
struct Recorder(Mutex<Vec<Vec<String>>>);

impl Recorder {
    fn calls(&self) -> Vec<Vec<String>> {
        self.0.lock().unwrap().clone()
    }
}

impl Spawner for Recorder {
    fn spawn(&self, command: &EditorCommand) -> ket_core::Result<()> {
        self.0.lock().unwrap().push(command.argv_lossy());
        Ok(())
    }
}

/// Builds an environment lookup from `(key, value)` pairs.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<std::ffi::OsString> + use<> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |key| {
        owned
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| std::ffi::OsString::from(v))
    }
}

/// Waits for a change to `path`, or fails the test.
fn wait_for(watcher: &FileWatcher, path: &Path) -> FileChange {
    let deadline = Instant::now() + WATCH_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(change) = watcher.next_within(Duration::from_millis(100))
            && change.path == path
        {
            return change;
        }
    }
    panic!(
        "nothing reported for {} within {WATCH_TIMEOUT:?}",
        path.display()
    );
}

/// Collects changes for a while, so a burst can be inspected as a whole.
fn collect_for(watcher: &FileWatcher, window: Duration) -> Vec<FileChange> {
    let deadline = Instant::now() + window;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Some(change) = watcher.next_within(Duration::from_millis(50)) {
            seen.push(change);
        }
    }
    seen.extend(watcher.drain());
    seen
}

// ---------------------------------------------------------------------------
// Someone else is writing
// ---------------------------------------------------------------------------

#[test]
fn a_file_that_changes_on_disk_while_it_is_open_in_the_editor_is_reported() {
    let sandbox = Sandbox::new("surface-open");
    let file = sandbox.path("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    let watcher = FileWatcher::watch(sandbox.root()).unwrap();

    // Hand the file to the editor. From here ket has no idea what happens to it
    // except through the watcher.
    let recorder = Arc::new(Recorder::default());
    let surface =
        ExternalSurface::from_command_line("zed", Arc::clone(&recorder) as Arc<_>).unwrap();
    surface
        .reveal(&Position::file(&file).with_line(1).with_column(4))
        .unwrap();
    assert_eq!(recorder.calls().len(), 1);

    fs::write(&file, "fn main() { println!(\"hi\"); }\n").unwrap();

    let change = wait_for(&watcher, &file);
    assert_ne!(change.kind, FileChangeKind::Removed);
    assert!(change.at_ms > 0);
}

#[test]
fn an_agent_editing_the_file_a_person_is_editing_surfaces_without_a_manual_refresh() {
    // The situation ket is built around: an agent is working in the worktree the
    // reviewer has open. Neither side announces itself; the watcher is the only
    // thing that knows.
    let sandbox = Sandbox::new("surface-agent");
    let file = sandbox.path("shared.rs");
    fs::write(&file, "// v0\n").unwrap();

    let watcher = FileWatcher::watch(sandbox.root()).unwrap();

    let recorder = Arc::new(Recorder::default());
    let surface =
        ExternalSurface::from_command_line("code", Arc::clone(&recorder) as Arc<_>).unwrap();
    let at = Position::file(&file).with_line(1);
    surface.reveal(&at).unwrap();

    // The human's editor saves.
    fs::write(&file, "// v1 by the human\n").unwrap();
    wait_for(&watcher, &file);

    // The agent writes the very same file.
    fs::write(&file, "// v2 by the agent\n").unwrap();
    wait_for(&watcher, &file);

    // Both writers are visible, and the handoff argv is still exactly one call.
    assert_eq!(
        recorder.calls(),
        vec![vec![
            "code".to_owned(),
            "--goto".to_owned(),
            format!("{}:1", file.display()),
        ]]
    );
}

#[test]
fn a_file_deleted_while_it_is_open_is_reported_as_a_removal() {
    let sandbox = Sandbox::new("surface-delete");
    let file = sandbox.path("doomed.rs");
    fs::write(&file, "// here for now\n").unwrap();

    let watcher = FileWatcher::watch(sandbox.root()).unwrap();

    fs::remove_file(&file).unwrap();

    let deadline = Instant::now() + WATCH_TIMEOUT;
    let mut removal = None;
    while Instant::now() < deadline && removal.is_none() {
        if let Some(change) = watcher.next_within(Duration::from_millis(100))
            && change.path == file
            && change.kind == FileChangeKind::Removed
        {
            removal = Some(change);
        }
    }
    assert!(removal.is_some(), "the deletion was never reported");
    assert!(!file.exists());
}

#[test]
fn one_save_coalesces_into_one_change_per_path() {
    // An editor saving a file is several filesystem events — write, rename,
    // touch permissions. A view wants "this file changed", once.
    let sandbox = Sandbox::new("surface-coalesce");
    let file = sandbox.path("busy.rs");
    fs::write(&file, "0\n").unwrap();

    let watcher = FileWatcher::watch(sandbox.root()).unwrap();

    for n in 1..=8 {
        fs::write(&file, format!("{n}\n")).unwrap();
    }

    // Let the burst arrive, then take it in one go.
    let deadline = Instant::now() + WATCH_TIMEOUT;
    let mut drained = Vec::new();
    while Instant::now() < deadline {
        drained = watcher.drain();
        if !drained.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let for_file: Vec<_> = drained.iter().filter(|c| c.path == file).collect();
    assert_eq!(
        for_file.len(),
        1,
        "expected one coalesced change, got {drained:?}"
    );
}

#[test]
fn git_churn_is_not_reported_as_a_change_to_anybodys_file() {
    // A worktree's .git directory rewrites itself constantly. Passing that
    // through would fill the queue with noise exactly when an agent is busy.
    let sandbox = Sandbox::new("surface-git");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let watcher = FileWatcher::watch(&repo).unwrap();

    let file = repo.join("added.rs");
    fs::write(&file, "// new\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "add a file"]);

    wait_for(&watcher, &file);
    let seen = collect_for(&watcher, Duration::from_millis(500));

    for change in &seen {
        assert!(
            !change.path.components().any(|c| c.as_os_str() == ".git"),
            "git internals leaked into the change stream: {change:?}"
        );
    }
}

#[test]
fn a_flood_of_changes_is_bounded_and_says_when_it_dropped_some() {
    // An agent running `npm install` in a watched worktree produces tens of
    // thousands of events. Queueing all of them to redraw a list once is the
    // unbounded buffer the cap exists to prevent — but a silently short list
    // during a review would be worse than a slow one, so dropping must be
    // announced.
    let sandbox = Sandbox::new("surface-flood");
    let dir = sandbox.path("flood");
    fs::create_dir_all(&dir).unwrap();

    let watcher = FileWatcher::watch(sandbox.root()).unwrap();

    let count = MAX_PENDING_CHANGES * 4;
    for n in 0..count {
        fs::write(dir.join(format!("f{n}.txt")), b"x").unwrap();
    }

    // Deliberately not draining while the flood arrives: the case that matters
    // is the one where nobody is looking, which is precisely when a queue grows
    // without bound.
    std::thread::sleep(Duration::from_secs(2));

    assert!(
        watcher.overflowed(),
        "the queue swallowed a flood of {count} writes without saying so"
    );

    let drained = watcher.drain();
    assert!(
        drained.len() <= MAX_PENDING_CHANGES,
        "held {} changes, past the cap of {MAX_PENDING_CHANGES}",
        drained.len()
    );

    // Draining is what acknowledges the drop. Leaving the flag set would make
    // every later refresh a full one, forever.
    assert!(!watcher.overflowed());
}

#[test]
fn waiting_for_a_change_that_never_comes_returns_rather_than_blocking() {
    // Invariant 4: nothing here may hold a thread indefinitely.
    let sandbox = Sandbox::new("surface-quiet");
    let watcher = FileWatcher::watch(sandbox.root()).unwrap();

    let started = Instant::now();
    assert!(watcher.next_within(Duration::from_millis(150)).is_none());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "next_within overshot its timeout by {:?}",
        started.elapsed()
    );
    assert!(watcher.try_next().is_none());
}

#[test]
fn watching_something_that_does_not_exist_is_an_error_not_a_panic() {
    let result = FileWatcher::watch(Path::new("/nonexistent/ket/surface/root"));
    assert!(matches!(result, Err(KetError::Io { .. })), "{result:?}");
}

// ---------------------------------------------------------------------------
// Writing without clobbering
// ---------------------------------------------------------------------------

#[test]
fn writing_a_file_an_external_editor_changed_is_refused_rather_than_clobbering_it() {
    let sandbox = Sandbox::new("surface-conflict");
    let file = sandbox.path("contested.rs");
    fs::write(&file, "// as ket read it\n").unwrap();

    let (_bytes, stamp) = ket_core::surface::read(&file).unwrap();

    // The editor saves while ket was thinking.
    fs::write(&file, "// the human's careful edit\n").unwrap();

    let result = ket_core::surface::write_if_unchanged(&file, &stamp, b"// ket's version\n");
    assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");

    // And, the point of the whole exercise: their work is still there.
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "// the human's careful edit\n"
    );
}

#[test]
fn a_same_length_replacement_is_still_a_conflict() {
    // Length and mtime together are not enough. An editor autosaving twice
    // inside one filesystem timestamp tick, with the same number of bytes, is
    // the case that gets through — hence the content digest.
    let sandbox = Sandbox::new("surface-samelen");
    let file = sandbox.path("same.rs");
    fs::write(&file, "aaaa").unwrap();

    let stamp = FileStamp::of(&file).unwrap();
    fs::write(&file, "bbbb").unwrap();

    let result = ket_core::surface::write_if_unchanged(&file, &stamp, b"cccc");
    assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "bbbb");
}

#[test]
fn writing_a_file_nobody_touched_succeeds_and_returns_a_fresh_stamp() {
    let sandbox = Sandbox::new("surface-write");
    let file = sandbox.path("calm.rs");
    fs::write(&file, "// v0\n").unwrap();

    let (_bytes, stamp) = ket_core::surface::read(&file).unwrap();
    let next = ket_core::surface::write_if_unchanged(&file, &stamp, b"// v1\n").unwrap();

    assert_eq!(fs::read_to_string(&file).unwrap(), "// v1\n");
    assert_ne!(next.digest, stamp.digest);

    // The returned stamp is usable for the next write, with no re-read.
    ket_core::surface::write_if_unchanged(&file, &next, b"// v2\n").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "// v2\n");
}

#[test]
fn writing_a_file_that_was_deleted_reports_a_conflict_rather_than_recreating_it() {
    // A file deleted mid-review is usually an agent that decided to remove it.
    // Recreating it from a stale buffer would silently undo that.
    let sandbox = Sandbox::new("surface-gone");
    let file = sandbox.path("removed.rs");
    fs::write(&file, "// briefly\n").unwrap();

    let stamp = FileStamp::of(&file).unwrap();
    fs::remove_file(&file).unwrap();

    let result = ket_core::surface::write_if_unchanged(&file, &stamp, b"// back?\n");
    match result {
        Err(KetError::Conflict(message)) => assert!(
            message.contains("deleted"),
            "the message should say what happened: {message}"
        ),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(!file.exists(), "the file was recreated behind the deletion");
}

#[test]
fn a_write_leaves_no_temporary_behind_and_keeps_the_original_permissions() {
    let sandbox = Sandbox::new("surface-atomic");
    let file = sandbox.path("script.sh");
    fs::write(&file, "#!/bin/sh\necho v0\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let stamp = FileStamp::of(&file).unwrap();
    ket_core::surface::write_if_unchanged(&file, &stamp, b"#!/bin/sh\necho v1\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "the rename un-executed the script: {mode:o}"
        );
    }

    let leftovers: Vec<_> = fs::read_dir(sandbox.root())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(TEMP_PREFIX))
        .collect();
    assert!(
        leftovers.is_empty(),
        "left temporaries behind: {leftovers:?}"
    );
}

// ---------------------------------------------------------------------------
// Handoff failures
// ---------------------------------------------------------------------------

#[test]
fn an_unset_editor_variable_is_a_clear_error_and_not_a_hang() {
    let lookup = env(&[]);
    let started = Instant::now();
    let result = ExternalSurface::with_lookup(&lookup, Arc::new(ProcessSpawner));

    match result {
        Err(KetError::Config(message)) => {
            assert!(
                message.contains("KET_EDITOR"),
                "the message must name the variable to set: {message}"
            );
            assert!(
                message.contains("code") || message.contains("zed"),
                "the message must show what a value looks like: {message}"
            );
        }
        other => panic!("expected a config error, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn an_empty_editor_variable_falls_through_to_the_next_one() {
    // `KET_EDITOR=` in a shell profile is set-but-useless, and treating it as
    // configured would produce "could not start editor ``".
    let lookup = env(&[("KET_EDITOR", "   "), ("VISUAL", ""), ("EDITOR", "nvim")]);
    let surface = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default())).unwrap();

    let at = Position::file("/w/a.rs").with_line(7);
    assert_eq!(
        surface.command_for(&at).unwrap().argv_lossy(),
        ["nvim", "+7", "/w/a.rs"]
    );
}

#[test]
fn ket_editor_wins_over_visual_and_editor() {
    let lookup = env(&[
        ("KET_EDITOR", "zed"),
        ("VISUAL", "code"),
        ("EDITOR", "nvim"),
    ]);
    let surface = ExternalSurface::with_lookup(&lookup, Arc::new(Recorder::default())).unwrap();
    assert_eq!(surface.describe(), "zed");
}

#[test]
fn an_editor_that_is_not_installed_fails_immediately_with_a_message() {
    // The most common real failure, and the one that must never look like a
    // hang: a name that is not on PATH.
    let sandbox = Sandbox::new("surface-missing");
    let file = sandbox.path("a.rs");
    fs::write(&file, "// x\n").unwrap();

    let lookup = env(&[("KET_EDITOR", "ket-no-such-editor-9f3a")]);
    let surface = ExternalSurface::with_lookup(&lookup, Arc::new(ProcessSpawner)).unwrap();

    let started = Instant::now();
    let result = surface.reveal(&Position::file(&file).with_line(1));

    match result {
        Err(KetError::Config(message)) => {
            assert!(
                message.contains("ket-no-such-editor-9f3a"),
                "the message must name the program: {message}"
            );
            assert!(
                message.contains("KET_EDITOR"),
                "the message must say what to change: {message}"
            );
        }
        other => panic!("expected a config error, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?} to report a missing binary",
        started.elapsed()
    );
}

#[test]
fn revealing_returns_immediately_even_when_the_editor_never_exits() {
    // An editor is expected to outlive the call. `reveal` hands off; it does not
    // wait for the person to finish editing, because that is a wedged UI.
    let sandbox = Sandbox::new("surface-slow");
    let editor = sandbox.path("slow-editor.sh");
    fs::write(&editor, "#!/bin/sh\nsleep 5\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let file = sandbox.path("a.rs");
    fs::write(&file, "// x\n").unwrap();

    let surface =
        ExternalSurface::from_command_line(&editor.to_string_lossy(), Arc::new(ProcessSpawner))
            .unwrap();

    let started = Instant::now();
    surface.reveal(&Position::file(&file)).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "reveal waited for the editor: {:?}",
        started.elapsed()
    );
}

#[test]
fn the_watcher_reaches_the_surface_through_the_trait() {
    // The seam that lets a native editor pane arrive later: callers hold an
    // `EditorSurface`, not an `ExternalSurface`.
    let sandbox = Sandbox::new("surface-trait");
    let file = sandbox.path("a.rs");
    fs::write(&file, "// x\n").unwrap();

    let surface: Box<dyn EditorSurface> =
        Box::new(ExternalSurface::from_command_line("zed", Arc::new(Recorder::default())).unwrap());

    assert!(!surface.caps().embedded);
    assert!(surface.caps().editable);

    let watcher = surface.watch(sandbox.root()).unwrap();
    fs::write(&file, "// y\n").unwrap();
    wait_for(&watcher, &file);
}
