use super::*;

/// A workspace whose state, worktrees and Chromium roots live in a
/// throwaway directory.
fn sandboxed(tag: &str) -> (PathBuf, Workspace) {
    let root = std::env::temp_dir().join(format!("ket-ws-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let workspace = Workspace::with_paths(Store::at(root.join("state.json")), root.join("wt"))
        .with_chromium_roots(vec![root.join("chromium"), root.join("chromium-dev")]);
    (root, workspace)
}

// macOS: a live lock is told by asking whether its process is alive.
#[cfg(target_os = "macos")]
#[test]
fn a_removed_worktrees_browser_profiles_go_unless_chromium_holds_them() {
    let (root, workspace) = sandboxed("profiles");
    let worktree = WorktreeId::new("demo-site/1a2b");
    let name = paths::chromium_profile_name(&worktree);
    let [release, dev] = [root.join("chromium"), root.join("chromium-dev")];
    for profile in [release.join(&name), dev.join(&name), release.join("other")] {
        fs::create_dir_all(profile.join("Local Storage")).unwrap();
    }
    // A release ket is running, this process standing in for it.
    std::os::unix::fs::symlink(
        format!("host-{}", std::process::id()),
        release.join("SingletonLock"),
    )
    .unwrap();

    workspace.remove_browser_profiles(&worktree, None);
    assert!(
        release.join(&name).is_dir(),
        "a running Chromium's profile stays"
    );
    assert!(!dev.join(&name).exists());
    assert!(release.join("other").is_dir(), "another worktree's stays");

    // That ket has closed the worktree's pages, and deletes its own.
    workspace.remove_browser_profiles(&worktree, Some(&release));
    assert!(!release.join(&name).exists());
    assert!(release.join("other").is_dir());
    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_lock_whose_chromium_has_gone_holds_nothing() {
    let (root, _) = sandboxed("lock");
    assert!(!chromium_holds(&root));
    // No process has pid i32::MAX.
    std::os::unix::fs::symlink(format!("host-{}", i32::MAX), root.join("SingletonLock")).unwrap();
    assert!(!chromium_holds(&root));
    let _ = fs::remove_dir_all(&root);
}
