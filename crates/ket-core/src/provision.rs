//! Making a fresh worktree usable.
//!
//! `git worktree add` produces a checkout, not a working environment. There is
//! no `node_modules`, no `.env`, no build cache. An agent dropped into one fails
//! on its first command and then spends real money diagnosing an environment
//! problem that does not exist.
//!
//! Provisioning closes that gap: dependency directories are cloned copy-on-write
//! (see [`crate::cow`]), and declared untracked files are copied.
//!
//! **On secrets.** This module copies `.env` files. Their *contents* never reach
//! a log line, an event, or a progress message — only the relative path is ever
//! reported. That is not incidental; treat it as a rule when extending this.
//!
//! **On teardown.** Everything provisioning creates is recorded, and
//! [`teardown`] removes exactly that list — never what config *says* should be
//! there. A repository that tracks its own `vendor/` was left alone on the way
//! in, and must be left alone on the way out.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{DirStrategy, ProvisionConfig};
use crate::cow::{self, CloneMethod};
use crate::git::Git;
use crate::{KetError, Result};

/// How long to wait between polls when timing out the post-provision command.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What happened to one directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DirectoryAction {
    /// Materialised, by the given method.
    Cloned {
        /// Whether copy-on-write was actually used.
        copy_on_write: bool,
    },
    /// Symlinked into the primary checkout.
    Symlinked,
    /// Config said to skip it.
    SkippedByConfig,
    /// The source directory does not exist — a project that has never had
    /// `npm install` run in it, for instance. Not an error.
    SkippedNoSource,
    /// Something is already at that path in the worktree.
    ///
    /// Either the branch tracks it, or a previous run put it there. Either way
    /// provisioning leaves it alone — and so, crucially, does [`teardown`],
    /// which is why this is a distinct outcome rather than a kind of "skipped".
    AlreadyPresent,
}

impl DirectoryAction {
    /// Whether provisioning is what put this directory in the worktree.
    ///
    /// The teardown question, and the only thing that makes removing it safe.
    pub fn was_created(&self) -> bool {
        matches!(self, Self::Cloned { .. } | Self::Symlinked)
    }
}

/// The outcome for one configured directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryOutcome {
    /// Path relative to the repository root.
    pub path: String,
    /// What was done.
    pub action: DirectoryAction,
    /// How long it took.
    pub duration_ms: u64,
}

/// What provisioning did.
///
/// Contains paths only — never file contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionReport {
    /// One entry per configured directory.
    pub directories: Vec<DirectoryOutcome>,
    /// Relative paths of files copied.
    pub files: Vec<String>,
    /// Duration of the post-provision command, if one ran.
    pub post_command_ms: Option<u64>,
    /// Submodules the worktree declares but has not checked out.
    ///
    /// `git worktree add` does not populate submodules, so these directories
    /// exist and are empty. An agent that needs one fails on its first build
    /// with an error about the submodule, not about the task — exactly the
    /// wasted-money case provisioning exists to prevent. Fix it per project with
    /// a `post_command` of `git submodule update --init --recursive`.
    pub uninitialised_submodules: Vec<String>,
    /// Total wall time.
    pub duration_ms: u64,
}

impl ProvisionReport {
    /// Whether anything degraded from copy-on-write to a full copy.
    ///
    /// Worth surfacing: a silent fallback on a non-APFS volume or a network
    /// mount turns instant provisioning into something slow enough to notice,
    /// with no other symptom.
    pub fn degraded_to_full_copy(&self) -> bool {
        self.directories.iter().any(|d| {
            matches!(
                d.action,
                DirectoryAction::Cloned {
                    copy_on_write: false
                }
            )
        })
    }

    /// Repository-relative paths this run created, for [`teardown`] to undo.
    ///
    /// Only what provisioning actually materialised: a directory the checkout
    /// already had is not in here, and must never be.
    pub fn created_paths(&self) -> Vec<String> {
        self.directories
            .iter()
            .filter(|d| d.action.was_created())
            .map(|d| d.path.clone())
            .chain(self.files.iter().cloned())
            .collect()
    }
}

/// Materialises everything a worktree needs.
///
/// `progress` is called with short, human-readable steps. Those strings are
/// published as events, so they must never contain file contents.
///
/// On failure the caller must treat the worktree as unusable: it is only marked
/// provisioned once this returns `Ok`, so an interrupted run leaves a worktree
/// that is visibly not ready rather than one that looks fine and is not.
pub fn provision(
    source: &Path,
    target: &Path,
    config: &ProvisionConfig,
    mut progress: impl FnMut(&str),
) -> Result<ProvisionReport> {
    let started = Instant::now();

    if !target.is_dir() {
        return Err(KetError::Conflict(format!(
            "{} is not a directory",
            target.display()
        )));
    }

    let mut directories = Vec::new();
    for spec in &config.directories {
        let outcome =
            provision_directory(source, target, &spec.path, spec.strategy, &mut progress)?;
        directories.push(outcome);
    }

    let files = copy_declared_files(source, target, &config.files, &mut progress)?;

    let post_command_ms = if config.post_command.is_empty() {
        None
    } else {
        Some(run_post_command(
            target,
            &config.post_command,
            Duration::from_secs(config.post_timeout_secs),
            &mut progress,
        )?)
    };

    // After the post-command, which is where a project would have initialised
    // them. Reporting a submodule the post-command just checked out would be
    // false alarm rather than warning.
    let uninitialised_submodules = uninitialised_submodules(target);

    Ok(ProvisionReport {
        directories,
        files,
        post_command_ms,
        uninitialised_submodules,
        duration_ms: elapsed_ms(started),
    })
}

/// Submodules the worktree declares but has not checked out.
///
/// Best effort: a repository with no submodules, or a git that will not answer,
/// yields an empty list rather than failing a provision that otherwise worked.
fn uninitialised_submodules(worktree: &Path) -> Vec<String> {
    match Git::new(worktree).submodule_status() {
        Ok(entries) => entries
            .into_iter()
            .filter(|s| !s.initialised)
            .map(|s| s.path)
            .collect(),
        Err(e) => {
            tracing::debug!(%e, "could not read submodule status");
            Vec::new()
        }
    }
}

/// One path teardown could not remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeardownFailure {
    /// Path relative to the worktree root.
    pub path: String,
    /// Why it could not be removed.
    pub why: String,
}

/// What [`teardown`] removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeardownReport {
    /// Relative paths removed.
    pub removed: Vec<String>,
    /// Paths that could not be removed.
    ///
    /// Never fatal. A worktree that cannot be fully undressed is still a
    /// worktree that should be removable.
    pub failed: Vec<TeardownFailure>,
}

/// Removes what provisioning put into a worktree.
///
/// `created` is [`ProvisionReport::created_paths`] as recorded on the worktree,
/// never the provisioning *config*: config describes intent, and intent does not
/// distinguish a `vendor/` this tool cloned from a `vendor/` the repository has
/// tracked since 2019.
///
/// Called before `git worktree remove` rather than after, and the ordering is
/// the whole design:
///
/// - After is pointless. Git deletes the worktree directory wholesale, so there
///   is nothing left to tear down.
/// - Before is what makes removal work at all. A symlinked `node_modules` is not
///   a directory, so the near-universal `node_modules/` ignore rule does not
///   match it; git sees an untracked file and refuses to remove the worktree.
///   Every symlink-provisioned worktree would need `--force`, which is exactly
///   the flag that also throws away an agent's real work.
///
/// The cost is that a removal refused *after* this ran leaves the worktree
/// stripped of its environment. That is recoverable — `ket worktree provision`
/// rebuilds it in seconds — and provisioned artefacts are copies of things the
/// primary checkout still has. Uncommitted work is not, which is why git's
/// refusal is still what decides.
///
/// Never follows a symlink out of the worktree: a link is unlinked, not walked.
pub fn teardown(
    worktree: &Path,
    created: &[String],
    mut progress: impl FnMut(&str),
) -> TeardownReport {
    let mut removed = Vec::new();
    let mut failed = Vec::new();

    for rel in created {
        let Some(path) = safe_join(worktree, rel) else {
            // Only reachable from a hand-edited state file, but this function
            // deletes things, so it checks anyway.
            failed.push(TeardownFailure {
                path: (*rel).clone(),
                why: "path escapes the worktree".to_owned(),
            });
            continue;
        };

        let parent = Path::new(rel).parent().and_then(Path::to_str).unwrap_or("");
        if !no_symlink_path(worktree, parent) {
            failed.push(TeardownFailure {
                path: (*rel).clone(),
                why: "parent path traverses a symlink".to_owned(),
            });
            continue;
        }

        // `symlink_metadata` is load-bearing. `metadata` follows the link, and a
        // symlink to the primary checkout's `node_modules` would then look like
        // a directory to delete — destroying the source every worktree shares.
        let Ok(metadata) = path.symlink_metadata() else {
            continue; // Already gone. Teardown is idempotent.
        };

        let outcome = if metadata.file_type().is_symlink() || !metadata.is_dir() {
            std::fs::remove_file(&path)
        } else {
            std::fs::remove_dir_all(&path)
        };

        match outcome {
            Ok(()) => {
                progress(&format!("removed {rel}"));
                removed.push((*rel).clone());
            }
            Err(e) => failed.push(TeardownFailure {
                path: (*rel).clone(),
                why: e.to_string(),
            }),
        }
    }

    TeardownReport { removed, failed }
}

/// Materialises one directory.
fn provision_directory(
    source: &Path,
    target: &Path,
    rel: &str,
    strategy: DirStrategy,
    progress: &mut impl FnMut(&str),
) -> Result<DirectoryOutcome> {
    let started = Instant::now();

    let finish = |action: DirectoryAction| -> Result<DirectoryOutcome> {
        Ok(DirectoryOutcome {
            path: rel.to_owned(),
            action,
            duration_ms: elapsed_ms(started),
        })
    };

    if strategy == DirStrategy::Skip {
        return finish(DirectoryAction::SkippedByConfig);
    }

    // A configured directory is a fixed name from config, but validate anyway:
    // config is user-editable and `../..` in it must not escape the worktree.
    let src = match safe_join(source, rel) {
        Some(path) => path,
        None => {
            tracing::warn!(rel, "provision directory escapes the repository; skipping");
            return finish(DirectoryAction::SkippedByConfig);
        }
    };
    let Some(dst) = safe_join(target, rel) else {
        return finish(DirectoryAction::SkippedByConfig);
    };

    if !no_symlink_path(source, rel) || !no_symlink_path(target, rel) {
        tracing::warn!(rel, "provision path traverses a symlink; skipping");
        return finish(DirectoryAction::SkippedByConfig);
    }

    if !src.is_dir() {
        return finish(DirectoryAction::SkippedNoSource);
    }

    // `symlink_metadata`, not `exists`: a symlink left by an earlier run whose
    // target has since gone still occupies the path, and `exists` follows it and
    // says no. Cloning onto it would then fail with EEXIST from the syscall.
    if dst.symlink_metadata().is_ok() {
        // Already provisioned, or the branch tracks it. Leave it alone; a
        // re-run must be idempotent rather than destructive.
        return finish(DirectoryAction::AlreadyPresent);
    }

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
    }

    match strategy {
        DirStrategy::Symlink => {
            progress(&format!("linking {rel}"));
            std::os::unix::fs::symlink(&src, &dst).map_err(|e| KetError::io(&dst, e))?;
            finish(DirectoryAction::Symlinked)
        }
        DirStrategy::Clone => {
            progress(&format!("cloning {rel}"));
            let method = cow::clone_directory(&src, &dst).map_err(|e| KetError::io(&dst, e))?;
            finish(DirectoryAction::Cloned {
                copy_on_write: method == CloneMethod::CopyOnWrite,
            })
        }
        DirStrategy::Skip => unreachable!("handled above"),
    }
}

/// Copies untracked files matching the configured globs.
fn copy_declared_files(
    source: &Path,
    target: &Path,
    patterns: &[String],
    progress: &mut impl FnMut(&str),
) -> Result<Vec<String>> {
    let mut copied = Vec::new();

    // The repository's own path is data, not a pattern. A checkout living under
    // `~/work/[archived]/api` would otherwise have its brackets read as a
    // character class, and `.env` would silently not be copied.
    let Some(source_str) = source.to_str() else {
        tracing::warn!(path = %source.display(), "source path is not UTF-8; copying nothing");
        return Ok(copied);
    };
    let escaped_source = glob::Pattern::escape(source_str);

    for pattern in patterns {
        // Reject traversal before it reaches the glob engine.
        if pattern.contains("..") {
            tracing::warn!(pattern, "provision file pattern contains '..'; skipping");
            continue;
        }

        // Still validated against the real path, so an absolute or escaping
        // pattern is refused; only the *string* handed to glob is the escaped one.
        if safe_join(source, pattern).is_none() {
            continue;
        }
        let joined = format!("{escaped_source}/{pattern}");
        let as_str = joined.as_str();

        let matches = match glob::glob(as_str) {
            Ok(matches) => matches,
            Err(e) => {
                tracing::warn!(pattern, %e, "invalid provision file pattern; skipping");
                continue;
            }
        };

        for entry in matches.flatten() {
            if !entry
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_file())
            {
                continue;
            }

            let Ok(rel) = entry.strip_prefix(source) else {
                continue;
            };
            let Some(rel_str) = rel.to_str() else {
                continue;
            };
            if !no_symlink_path(source, rel_str) {
                tracing::warn!(path = %rel_str, "provision source traverses a symlink; skipping");
                continue;
            }
            let Some(dst) = safe_join(target, rel_str) else {
                continue;
            };

            let Some(parent) = dst.parent() else {
                continue;
            };
            let Some(parent_rel) = parent.strip_prefix(target).ok().and_then(|p| p.to_str()) else {
                continue;
            };
            if !no_symlink_path(target, parent_rel) || !no_symlink_path(target, rel_str) {
                tracing::warn!(path = %rel_str, "provision destination traverses a symlink; skipping");
                continue;
            }

            if dst.symlink_metadata().is_ok() {
                continue;
            }

            std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;

            // Recheck after creating parents: a dangling link is still an
            // occupied destination, and a concurrent replacement must not be
            // followed by the copy.
            if !no_symlink_path(target, parent_rel) || dst.symlink_metadata().is_ok() {
                tracing::warn!(path = %rel_str, "provision destination changed; skipping");
                continue;
            }

            let source_metadata = entry
                .symlink_metadata()
                .map_err(|e| KetError::io(&entry, e))?;
            let mut input = File::open(&entry).map_err(|e| KetError::io(&entry, e))?;
            let opened_source = input.metadata().map_err(|e| KetError::io(&entry, e))?;
            if !opened_source.is_file()
                || opened_source.dev() != source_metadata.dev()
                || opened_source.ino() != source_metadata.ino()
            {
                tracing::warn!(path = %rel_str, "provision source changed; skipping");
                continue;
            }
            let parent_metadata = parent.metadata().map_err(|e| KetError::io(parent, e))?;
            let mut output = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dst)
            {
                Ok(output) => output,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(KetError::io(&dst, error)),
            };
            let current_parent = parent.metadata().map_err(|e| KetError::io(parent, e))?;
            if current_parent.dev() != parent_metadata.dev()
                || current_parent.ino() != parent_metadata.ino()
            {
                return Err(KetError::Conflict(format!(
                    "provision destination `{rel_str}` changed while it was opened"
                )));
            }
            io::copy(&mut input, &mut output).map_err(|e| KetError::io(&dst, e))?;

            // The path, never the contents. These are secrets by definition.
            progress(&format!("copied {rel_str}"));
            copied.push(rel_str.to_owned());
        }
    }

    copied.sort();
    Ok(copied)
}

/// Runs the post-provision command, killing it if it overruns.
fn run_post_command(
    worktree: &Path,
    command: &[String],
    timeout: Duration,
    progress: &mut impl FnMut(&str),
) -> Result<u64> {
    let Some((program, args)) = command.split_first() else {
        return Ok(0);
    };

    progress(&format!("running {program}"));
    let started = Instant::now();

    let mut child = Command::new(program)
        .args(args)
        .current_dir(worktree)
        .spawn()
        .map_err(|e| KetError::io(program, e))?;

    loop {
        match child.try_wait().map_err(|e| KetError::io(program, e))? {
            Some(status) if status.success() => return Ok(elapsed_ms(started)),
            Some(status) => {
                return Err(KetError::Conflict(format!(
                    "post-provision command `{program}` failed: {status}"
                )));
            }
            None => {
                if started.elapsed() >= timeout {
                    // A hung command must not wedge the worktree forever
                    // (invariant 4). Kill it and report, rather than waiting.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(KetError::Conflict(format!(
                        "post-provision command `{program}` timed out after {}s",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// Joins `rel` onto `base`, refusing anything that escapes `base`.
///
/// Purely lexical, so it works for paths that do not exist yet. `..` is rejected
/// outright rather than resolved, because resolving it against a path containing
/// symlinks gives an answer that does not match what the filesystem will do.
pub(crate) fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    use std::path::Component;

    let candidate = Path::new(rel);
    if candidate.is_absolute() {
        return None;
    }

    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(base.join(candidate))
}

/// Returns false if an existing component below `base` is a symlink.
fn no_symlink_path(base: &Path, rel: &str) -> bool {
    let mut path = base.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            std::path::Component::Normal(name) => path.push(name),
            std::path::Component::CurDir => continue,
            _ => return false,
        }
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return false;
        }
    }
    true
}

/// Milliseconds elapsed, saturating.
fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DirectorySpec;
    use std::fs;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ket-prov-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A source checkout with dependencies and a secret, plus an empty target.
    fn fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = sandbox(tag);
        let source = root.join("source");
        let target = root.join("target");

        fs::create_dir_all(source.join("node_modules/pkg")).unwrap();
        fs::write(source.join("node_modules/pkg/index.js"), b"module").unwrap();
        fs::write(source.join(".env"), b"SECRET=hunter2\n").unwrap();
        fs::write(source.join("README.md"), b"# tracked\n").unwrap();
        fs::create_dir_all(&target).unwrap();

        (root, source, target)
    }

    fn config() -> ProvisionConfig {
        ProvisionConfig {
            directories: vec![DirectorySpec {
                path: "node_modules".to_owned(),
                strategy: DirStrategy::Clone,
            }],
            files: vec![".env".to_owned()],
            post_command: Vec::new(),
            post_timeout_secs: 30,
        }
    }

    #[test]
    fn materialises_dependencies_and_declared_files() {
        let (root, source, target) = fixture("basic");

        let report = provision(&source, &target, &config(), |_| {}).unwrap();

        assert!(target.join("node_modules/pkg/index.js").is_file());
        assert_eq!(fs::read(target.join(".env")).unwrap(), b"SECRET=hunter2\n");
        assert_eq!(report.files, vec![".env".to_owned()]);
        assert_eq!(report.directories.len(), 1);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn secrets_never_appear_in_progress_messages() {
        // The whole point of the progress channel is that it is published as
        // events. If a `.env` value ever reaches it, it reaches the UI and the
        // logs too.
        let (root, source, target) = fixture("secrets");

        let mut steps = Vec::new();
        provision(&source, &target, &config(), |s| steps.push(s.to_owned())).unwrap();

        let joined = steps.join("\n");
        assert!(
            !joined.contains("hunter2"),
            "progress leaked a secret: {joined}"
        );
        assert!(
            !joined.contains("SECRET="),
            "progress leaked a secret: {joined}"
        );
        assert!(
            joined.contains(".env"),
            "should still name the file: {joined}"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn secrets_never_appear_in_the_report() {
        let (root, source, target) = fixture("report");
        let report = provision(&source, &target, &config(), |_| {}).unwrap();

        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("hunter2"), "report leaked a secret: {json}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_source_directory_is_not_an_error() {
        // A project that has never had `npm install` run in it is normal.
        let root = sandbox("no-deps");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();

        let report = provision(&source, &target, &config(), |_| {}).unwrap();
        assert_eq!(
            report.directories[0].action,
            DirectoryAction::SkippedNoSource
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn re_running_is_idempotent() {
        let (root, source, target) = fixture("idempotent");

        provision(&source, &target, &config(), |_| {}).unwrap();
        // Mutate the worktree's copy; a second run must not clobber it.
        fs::write(target.join("node_modules/pkg/index.js"), b"local edit").unwrap();
        provision(&source, &target, &config(), |_| {}).unwrap();

        assert_eq!(
            fs::read(target.join("node_modules/pkg/index.js")).unwrap(),
            b"local edit"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cloned_dependencies_are_independent_between_worktrees() {
        // Two agents running `npm install` concurrently is the case this
        // protects against — which is why `clone` is the default, not `symlink`.
        let (root, source, _) = fixture("independent");
        let a = root.join("wt-a");
        let b = root.join("wt-b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();

        provision(&source, &a, &config(), |_| {}).unwrap();
        provision(&source, &b, &config(), |_| {}).unwrap();

        fs::write(a.join("node_modules/pkg/index.js"), b"agent a").unwrap();
        assert_eq!(
            fs::read(b.join("node_modules/pkg/index.js")).unwrap(),
            b"module",
            "one worktree's install must not be visible in another"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn symlinked_directories_are_shared_which_is_why_they_are_opt_in() {
        // Documents the hazard rather than pretending it does not exist.
        let (root, source, target) = fixture("symlink");
        let mut cfg = config();
        cfg.directories[0].strategy = DirStrategy::Symlink;

        let report = provision(&source, &target, &cfg, |_| {}).unwrap();
        assert_eq!(report.directories[0].action, DirectoryAction::Symlinked);

        fs::write(
            target.join("node_modules/pkg/index.js"),
            b"written via link",
        )
        .unwrap();
        assert_eq!(
            fs::read(source.join("node_modules/pkg/index.js")).unwrap(),
            b"written via link",
            "symlinked directories are shared; that is the documented trade-off"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn traversal_in_config_cannot_escape_the_worktree() {
        let (root, source, target) = fixture("traversal");
        let mut cfg = config();
        cfg.directories[0].path = "../../escape".to_owned();
        cfg.files = vec!["../outside.txt".to_owned()];

        fs::write(root.join("outside.txt"), b"should not be copied").unwrap();

        let report = provision(&source, &target, &cfg, |_| {}).unwrap();
        assert!(report.files.is_empty());
        assert!(!root.join("escape").exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn glob_patterns_match_multiple_files() {
        let root = sandbox("globs");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join(".env"), b"a").unwrap();
        fs::write(source.join(".env.production.local"), b"b").unwrap();
        fs::write(source.join("unrelated.txt"), b"c").unwrap();

        let cfg = ProvisionConfig {
            directories: Vec::new(),
            files: vec![".env".to_owned(), ".env.*.local".to_owned()],
            post_command: Vec::new(),
            post_timeout_secs: 30,
        };

        let report = provision(&source, &target, &cfg, |_| {}).unwrap();
        assert_eq!(
            report.files,
            vec![".env".to_owned(), ".env.production.local".to_owned()]
        );
        assert!(!target.join("unrelated.txt").exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_hanging_post_command_is_killed_rather_than_wedging_the_worktree() {
        let (root, source, target) = fixture("timeout");
        let mut cfg = config();
        cfg.post_command = vec!["sleep".to_owned(), "60".to_owned()];
        cfg.post_timeout_secs = 1;

        let started = Instant::now();
        let result = provision(&source, &target, &cfg, |_| {});

        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout did not fire promptly"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_failing_post_command_fails_the_provision() {
        // Otherwise the worktree is marked ready when its install did not run.
        let (root, source, target) = fixture("post-fail");
        let mut cfg = config();
        cfg.post_command = vec!["false".to_owned()];

        assert!(provision(&source, &target, &cfg, |_| {}).is_err());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_successful_post_command_is_timed() {
        let (root, source, target) = fixture("post-ok");
        let mut cfg = config();
        cfg.post_command = vec!["true".to_owned()];

        let report = provision(&source, &target, &cfg, |_| {}).unwrap();
        assert!(report.post_command_ms.is_some());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_directory_already_in_the_worktree_is_not_reported_as_created() {
        // The teardown question. If this ever reports `Cloned`, removing the
        // worktree would delete a directory the repository tracks itself.
        let (root, source, target) = fixture("already-present");
        fs::create_dir_all(target.join("node_modules")).unwrap();
        fs::write(target.join("node_modules/tracked.js"), b"committed").unwrap();

        let report = provision(&source, &target, &config(), |_| {}).unwrap();

        assert_eq!(
            report.directories[0].action,
            DirectoryAction::AlreadyPresent
        );
        assert!(!report.created_paths().contains(&"node_modules".to_owned()));
        assert_eq!(
            fs::read(target.join("node_modules/tracked.js")).unwrap(),
            b"committed"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_source_path_containing_glob_metacharacters_still_matches() {
        // `[2024]` in a directory name is a character class to the glob engine.
        // Left unescaped, the pattern matches nothing and `.env` is silently not
        // copied — the worst kind of failure, since the worktree looks fine.
        let root = sandbox("glob-meta");
        let source = root.join("archive [2024]/api");
        let target = root.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join(".env"), b"KEY=value\n").unwrap();

        let report = provision(&source, &target, &config(), |_| {}).unwrap();

        assert_eq!(report.files, vec![".env".to_owned()]);
        assert!(target.join(".env").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn teardown_removes_only_what_it_is_told_to() {
        let (root, source, target) = fixture("teardown-scope");
        fs::write(target.join("agent-work.txt"), b"unreviewed").unwrap();

        let report = provision(&source, &target, &config(), |_| {}).unwrap();
        let created = report.created_paths();

        let torn = teardown(&target, &created, |_| {});

        assert_eq!(torn.failed, Vec::new());
        assert!(!target.join("node_modules").exists());
        assert!(!target.join(".env").exists());
        assert!(
            target.join("agent-work.txt").is_file(),
            "teardown removed something it did not create"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn teardown_unlinks_a_symlink_rather_than_following_it() {
        // Following it would delete the primary checkout's node_modules — the
        // source every other worktree was cloned from.
        let (root, source, target) = fixture("teardown-symlink");
        let mut cfg = config();
        cfg.directories[0].strategy = DirStrategy::Symlink;

        let report = provision(&source, &target, &cfg, |_| {}).unwrap();
        teardown(&target, &report.created_paths(), |_| {});

        assert!(!target.join("node_modules").exists());
        assert_eq!(
            fs::read(source.join("node_modules/pkg/index.js")).unwrap(),
            b"module",
            "teardown followed a symlink out of the worktree"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn teardown_is_idempotent() {
        // It runs on a path that may already have been half-cleaned by a failed
        // removal, so a second pass must be quiet rather than an error.
        let (root, source, target) = fixture("teardown-twice");
        let report = provision(&source, &target, &config(), |_| {}).unwrap();
        let created = report.created_paths();

        assert!(!teardown(&target, &created, |_| {}).removed.is_empty());
        let second = teardown(&target, &created, |_| {});
        assert!(second.removed.is_empty());
        assert!(second.failed.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn teardown_refuses_a_recorded_path_that_escapes_the_worktree() {
        // Only reachable from a hand-edited state file, but this function
        // deletes things, so the check is not optional.
        let (root, source, target) = fixture("teardown-escape");
        let _ = source;
        fs::write(root.join("precious.txt"), b"not yours").unwrap();

        let torn = teardown(&target, &["../precious.txt".to_owned()], |_| {});

        assert_eq!(torn.removed, Vec::<String>::new());
        assert_eq!(torn.failed.len(), 1);
        assert!(root.join("precious.txt").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn safe_join_rejects_escapes_and_absolutes() {
        let base = Path::new("/base");
        assert!(safe_join(base, "node_modules").is_some());
        assert!(safe_join(base, "a/b/c").is_some());
        assert!(safe_join(base, "../etc").is_none());
        assert!(safe_join(base, "a/../../etc").is_none());
        assert!(safe_join(base, "/etc/passwd").is_none());
    }
}
