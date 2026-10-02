//! Fixtures shared by the integration tests.
//!
//! Every test here works against a real repository built by the real `git`
//! binary: worktree behaviour is checked
//! against git, not against our own beliefs about git.

// Each test binary is a separate crate and uses a different subset of this
// module, so anything one of them does not call looks dead to that build.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway directory tree, removed when the test finishes.
pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// Creates an empty sandbox, unique to this process and thread.
    pub fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "ket-it-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create sandbox");

        // macOS puts the temp dir behind /private; canonicalise so the paths we
        // compare against git's output are the same ones.
        let root = root.canonicalize().unwrap_or(root);
        Self { root }
    }

    /// A path inside the sandbox.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// The sandbox root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Runs a git command, panicking with git's own stderr on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");

    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Runs a git command, returning `Ok(stdout)` or `Err(stderr)`.
pub fn try_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// Creates a repository with one commit on `main`.
pub fn init_repo(path: &Path) {
    init_repo_on(path, "main");
}

/// Creates a repository with one commit on a named initial branch.
pub fn init_repo_on(path: &Path, branch: &str) {
    fs::create_dir_all(path).expect("create repo dir");
    git(
        path,
        &["init", &format!("--initial-branch={branch}"), "--quiet"],
    );
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "ket test"]);
    fs::write(path.join("README.md"), b"# fixture\n").expect("write README");
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "initial"]);
}

/// Writes a file and commits it.
pub fn commit_file(repo: &Path, rel: &str, contents: &str, message: &str) {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(&path, contents).expect("write file");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", message]);
}

/// The commit `rev` resolves to.
pub fn rev(repo: &Path, rev: &str) -> String {
    git(repo, &["rev-parse", rev]).trim().to_owned()
}
