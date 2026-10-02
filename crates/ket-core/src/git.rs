//! A thin wrapper around the `git` binary.
//!
//! Worktree mutations go through the real `git` executable rather than a
//! library. `gix` and `libgit2` both have weak worktree support, and corrupting
//! worktrees inside a tool built entirely on worktrees is not a recoverable
//! class of bug. The cost is process spawns; the benefit is that git's own
//! semantics are the semantics, including every edge case nobody has documented.
//!
//! Read-heavy paths (status, diff) will move to `gix` in Epic 7, where the
//! per-call process overhead actually shows up.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{KetError, Result};

/// One entry from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktree {
    /// Absolute path to the worktree.
    pub path: PathBuf,
    /// Commit currently checked out, if any.
    pub head: Option<String>,
    /// Branch ref, e.g. `refs/heads/main`. `None` when detached or bare.
    pub branch: Option<String>,
    /// Whether this is a bare repository entry.
    pub bare: bool,
    /// Whether git reports the worktree as detached.
    pub detached: bool,
    /// Whether git reports the worktree as prunable (its directory is gone).
    pub prunable: bool,
    /// Whether the worktree is locked.
    pub locked: bool,
}

impl GitWorktree {
    /// Short branch name, with `refs/heads/` stripped.
    pub fn branch_name(&self) -> Option<&str> {
        self.branch
            .as_deref()
            .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b))
    }
}

/// How long a fetch may run before ket gives up on the remote and cuts the
/// worktree from what the repository already has.
///
/// A remote that is down, or an SSH host that wants a passphrase nobody is
/// there to type, would otherwise hold "Create worktree" open indefinitely.
/// Thirty seconds is long enough for a prune of a large remote and short
/// enough to be an inconvenience rather than a hang.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// What happened to the remote before a worktree was cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// The repository has no remote; there was nothing to fetch.
    NoRemote,
    /// `git fetch --prune` succeeded, so the remote-tracking refs are current.
    Fetched {
        /// The remote fetched, `origin` almost always.
        remote: String,
    },
    /// The fetch failed or timed out. Creation went on with the refs the
    /// repository already had, which may be stale.
    Failed {
        /// The remote that could not be reached.
        remote: String,
        /// Git's own account of it, or the timeout.
        why: String,
    },
}

/// What happened to the local base branch once the remote was consulted.
///
/// A worktree is cut from the remote-tracking ref when the remote is ahead,
/// and the local branch is brought along when doing so cannot lose
/// anything — never when it has commits of its own, never when the checkout
/// holding it has uncommitted changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalBaseRefresh {
    /// The base is not a local branch with a remote-tracking twin — a tag, a
    /// commit, `HEAD`, an explicit `origin/…` — so there was nothing to bring
    /// up to date.
    NotApplicable,
    /// Local and remote already pointed at the same commit.
    UpToDate,
    /// The local branch was fast-forwarded `behind` commits: in the checkout
    /// that has it out, when one does, else as a bare ref update.
    Updated {
        /// How many commits it moved.
        behind: u64,
        /// The checkout it moved in, when it was checked out somewhere.
        checkout: Option<PathBuf>,
    },
    /// The local branch has commits the remote does not, so it was left alone
    /// and the worktree was cut from it rather than from the remote. ket's
    /// own merges land on the local base without pushing, so this is the
    /// routine state of a repository whose worktrees have been collapsing
    /// into it; a worktree cut from the remote would silently miss them.
    LocalAhead {
        /// Commits the local branch has that the remote does not.
        ahead: u64,
        /// Commits the remote has that the local branch does not — zero
        /// unless the two have diverged.
        behind: u64,
    },
    /// The local branch is checked out somewhere with uncommitted changes, so
    /// it was left alone. The worktree was still cut from the remote.
    DirtyCheckout {
        /// How far behind the remote it was left.
        behind: u64,
        /// The checkout with the changes.
        checkout: PathBuf,
    },
    /// Bringing the local branch up to date failed. The worktree was still cut
    /// from the remote.
    Failed {
        /// How far behind the remote it was left.
        behind: u64,
        /// What went wrong.
        why: String,
    },
}

/// The base a worktree was actually cut from, after the remote was consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBase {
    /// What the caller asked for: `main`, `origin/main`, a tag, a commit.
    pub asked: String,
    /// The revision handed to `git worktree add`: a fully qualified ref where
    /// one was found, else `asked` verbatim.
    pub rev: String,
    /// Whether the remote was reached first.
    pub fetch: Fetch,
    /// What became of the local base branch.
    pub refresh: LocalBaseRefresh,
}

impl PreparedBase {
    /// One line for a log or a terminal, or `None` when nothing worth a
    /// person's attention happened: no remote, or a local branch that was
    /// already current.
    pub fn notice(&self) -> Option<String> {
        let mut lines = Vec::new();
        if let Fetch::Failed { remote, why } = &self.fetch {
            lines.push(format!(
                "could not fetch {remote} ({why}); cut from what was already here"
            ));
        }
        match &self.refresh {
            LocalBaseRefresh::NotApplicable | LocalBaseRefresh::UpToDate => {}
            LocalBaseRefresh::Updated { behind, checkout } => lines.push(match checkout {
                Some(path) => format!(
                    "{} was {behind} commit{} behind the remote; fast-forwarded it in {}",
                    self.asked,
                    plural(*behind),
                    path.display()
                ),
                None => format!(
                    "{} was {behind} commit{} behind the remote; fast-forwarded it",
                    self.asked,
                    plural(*behind)
                ),
            }),
            LocalBaseRefresh::LocalAhead { ahead, behind } => {
                if *behind > 0 {
                    lines.push(format!(
                        "{} has {ahead} commit{} the remote does not and is {behind} behind it; \
                         cut from the local branch, which needs reconciling",
                        self.asked,
                        plural(*ahead)
                    ));
                }
            }
            LocalBaseRefresh::DirtyCheckout { behind, checkout } => lines.push(format!(
                "{} is {behind} commit{} behind the remote but {} has uncommitted changes; \
                 cut from the remote and left the local branch alone",
                self.asked,
                plural(*behind),
                checkout.display()
            )),
            LocalBaseRefresh::Failed { behind, why } => lines.push(format!(
                "{} is {behind} commit{} behind the remote and could not be fast-forwarded ({why}); \
                 cut from the remote",
                self.asked,
                plural(*behind)
            )),
        }
        (!lines.is_empty()).then(|| lines.join("; "))
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Runs `git` in `dir`, returning stdout on success.
fn run_in(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| KetError::io(dir, e))?;

    if !output.status.success() {
        return Err(KetError::Git {
            command: args.join(" "),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Finds the repository root containing `path`.
///
/// Returns [`KetError::Git`] when `path` is not inside a repository, which is
/// how `ket project add` on a plain directory reports itself.
pub fn discover_root(path: &Path) -> Result<PathBuf> {
    let out = run_in(path, &["rev-parse", "--show-toplevel"])?;
    let trimmed = out.trim();

    if trimmed.is_empty() {
        return Err(KetError::Git {
            command: "rev-parse --show-toplevel".to_owned(),
            status: "0".to_owned(),
            stderr: "empty toplevel".to_owned(),
        });
    }

    Ok(PathBuf::from(trimmed))
}

/// Operations against one repository.
#[derive(Debug, Clone)]
pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Binds to a repository root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The repository root this instance operates on.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a revision to a full object id.
    pub fn rev_parse(&self, rev: &str) -> Result<String> {
        Ok(run_in(&self.root, &["rev-parse", rev])?.trim().to_owned())
    }

    /// Whether a local branch exists.
    pub fn branch_exists(&self, branch: &str) -> bool {
        run_in(
            &self.root,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }

    /// Best guess at the repository's default branch.
    ///
    /// Prefers what `origin/HEAD` points at, since that is what the remote
    /// considers default. Falls back to whichever of `main` or `master` exists,
    /// locally or on `origin`, then to the currently checked-out branch — a repository whose default is
    /// neither is common enough to be worth not guessing wrong about.
    pub fn default_branch(&self) -> Result<String> {
        if let Ok(out) = run_in(
            &self.root,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        ) && let Some(name) = out.trim().strip_prefix("origin/")
            && !name.is_empty()
        {
            return Ok(name.to_owned());
        }

        // Local or remote-tracking: a clone parked on a feature branch still
        // has an `origin/main`, and that is the branch its worktrees should
        // be cut from.
        for candidate in ["main", "master"] {
            if self.branch_exists(candidate)
                || self.ref_exists(&format!("refs/remotes/origin/{candidate}"))
            {
                return Ok(candidate.to_owned());
            }
        }

        let current = run_in(&self.root, &["branch", "--show-current"])?;
        let current = current.trim();
        if !current.is_empty() {
            return Ok(current.to_owned());
        }

        // No `origin/HEAD`, no `main`, no `master`, and a detached checkout. Rare
        // but real — a repository parked on a tag, or one whose only branch is
        // named something else entirely. `HEAD` is a legitimate base for
        // `git worktree add`, so registering the project still works; refusing
        // here would make such a repository unusable in ket for no good reason.
        //
        // Callers resolve `HEAD` to a commit when they record it, since inside a
        // linked worktree `HEAD` means that worktree's own branch instead.
        if self.rev_parse("HEAD").is_ok() {
            tracing::debug!(
                root = %self.root.display(),
                "no default branch found; falling back to HEAD"
            );
            return Ok("HEAD".to_owned());
        }

        Err(KetError::Git {
            command: "branch --show-current".to_owned(),
            status: "0".to_owned(),
            stderr: "repository has no branches and no commits (unborn HEAD)".to_owned(),
        })
    }

    /// Whether the working tree has uncommitted changes, tracked or untracked.
    pub fn is_dirty(&self) -> Result<bool> {
        let out = run_in(&self.root, &["status", "--porcelain"])?;
        Ok(!out.trim().is_empty())
    }

    /// The repository's local branches, most recently committed to first —
    /// what a picker of bases offers, the live ones on top.
    pub fn local_branches(&self) -> Result<Vec<String>> {
        let out = run_in(
            &self.root,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname:short)",
                "refs/heads",
            ],
        )?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// The branch checked out here, or `None` on a detached `HEAD`.
    pub fn current_branch(&self) -> Result<Option<String>> {
        let out = run_in(&self.root, &["branch", "--show-current"])?;
        let name = out.trim();
        Ok((!name.is_empty()).then(|| name.to_owned()))
    }

    /// Whether `ancestor` is already contained in `descendant`.
    ///
    /// Used to tell "nothing to merge" from "a merge that does nothing",
    /// which read the same in git's output and mean different things to
    /// someone deciding whether their agent produced anything.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        run_in(
            &self.root,
            &["merge-base", "--is-ancestor", ancestor, descendant],
        )
        .is_ok()
    }

    /// `git diff <args>` over the working tree against `since`: text only, no
    /// colour, no rename detection — each path as itself.
    pub fn diff_since(&self, since: &str, args: &[&str]) -> Result<String> {
        let mut all = vec!["diff", "--no-color", "--no-renames", since];
        all.extend_from_slice(args);
        run_in(&self.root, &all)
    }

    /// Files git is not tracking and not ignoring: new work not yet added.
    pub fn untracked(&self) -> Result<Vec<String>> {
        Ok(run_in(
            &self.root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
    }

    /// A new file's whole contents as a diff from nothing. `git diff
    /// --no-index` exits 1 when the files differ, which here is always.
    pub fn diff_new_file(&self, path: &str) -> Result<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["diff", "--no-color", "--no-index", "--", "/dev/null", path])
            .output()
            .map_err(|e| KetError::io(&self.root, e))?;
        match output.status.code() {
            Some(0 | 1) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            _ => Err(KetError::Git {
                command: format!("diff --no-index /dev/null {path}"),
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }),
        }
    }

    /// Where two refs last had a commit in common.
    ///
    /// What an *adopted* worktree's base commit has to be: a checkout ket did
    /// not create diverged from its base at some point in the past, and
    /// recording the base's tip instead would count every commit made on the
    /// base since as this worktree's own work.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        Ok(run_in(&self.root, &["merge-base", a, b])?.trim().to_owned())
    }

    /// How many lines the working tree adds and removes against `HEAD`.
    ///
    /// `(added, removed)`. This counts exactly what "dirty" is true about —
    /// uncommitted work — and deliberately not what a merge would carry: a
    /// worktree whose agent committed as it went has nothing uncommitted and
    /// still has commits its base has never seen. Both the sidebar's merge
    /// affordance and this number are gated on the same `dirty`, so they agree
    /// with each other; see `ket_ui::tree`'s note on `can_merge` for what that
    /// gate costs.
    ///
    /// `--numstat` rather than reading content: git reports the two numbers per
    /// file without either side having to hold a diff, which is what makes this
    /// cheap enough to run for every dirty worktree on the status tick. Binary
    /// files report `-` for both columns and are skipped rather than counted as
    /// zero, because a changed binary is not a change of no lines.
    ///
    /// A repository with no commits yet has no `HEAD` to diff against and
    /// reports `(0, 0)` rather than failing: nothing has been committed, so
    /// nothing has changed relative to what was.
    pub fn line_changes(&self) -> Result<(u32, u32)> {
        if self.rev_parse("HEAD").is_err() {
            return Ok((0, 0));
        }
        let out = run_in(&self.root, &["diff", "--numstat", "HEAD"])?;
        Ok(out.lines().fold((0, 0), |(added, removed), line| {
            let mut columns = line.split('\t');
            match (
                columns.next().and_then(|n| n.parse::<u32>().ok()),
                columns.next().and_then(|n| n.parse::<u32>().ok()),
            ) {
                (Some(a), Some(r)) => (added + a, removed + r),
                _ => (added, removed),
            }
        }))
    }

    /// Merges `branch` into whatever is checked out here.
    ///
    /// A merge that conflicts is **aborted**, not left half-applied: invariant 4
    /// says every state is escapable, and dropping someone into a conflicted
    /// index from a command that promised to collapse worktrees is the opposite
    /// of that. Resolving a real conflict is a job for git directly, with the
    /// worktrees still present to resolve it from.
    pub fn merge(&self, branch: &str, message: &str) -> Result<()> {
        match run_in(&self.root, &["merge", "--no-edit", "-m", message, branch]) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Best-effort: a merge that failed before starting has nothing
                // to abort, and that abort failing is not the error worth
                // reporting.
                let _ = run_in(&self.root, &["merge", "--abort"]);
                Err(e)
            }
        }
    }

    /// Stages every change in the working tree, and says how many paths that was.
    ///
    /// `add -A` rather than `commit -a`: an agent's output is mostly *new*
    /// files, and `-a` stages only what git already tracks, so a commit made
    /// that way would leave behind exactly the work that was most likely the
    /// point. Ignored files stay out either way, which is what keeps a
    /// provisioned `node_modules` from being committed — and is also where the
    /// risk in this operation lives, since a `.env` that provisioning copied
    /// and the repository does not ignore is an ordinary file to `add -A`.
    ///
    /// The count is what a commit message is allowed to quote, so it is taken
    /// from the index after staging rather than guessed from a status read
    /// before it.
    pub fn stage_all(&self) -> Result<usize> {
        run_in(&self.root, &["add", "-A"])?;
        self.staged_count()
    }

    /// How many paths are staged.
    ///
    /// Two spellings because there is no `HEAD` to diff against in a repository
    /// whose first commit has not been made, and older git reports that as an
    /// error rather than as "everything is new".
    ///
    /// `-z` and a count of NUL bytes rather than lines: a path may contain a
    /// newline, and this number ends up in a commit message.
    fn staged_count(&self) -> Result<usize> {
        let args: &[&str] = if self.rev_parse("HEAD").is_ok() {
            &["diff", "--cached", "--name-only", "-z"]
        } else {
            &["ls-files", "--cached", "-z"]
        };
        Ok(run_in(&self.root, args)?
            .bytes()
            .filter(|b| *b == 0)
            .count())
    }

    /// Commits what is staged, or reports that there was nothing to commit.
    ///
    /// `Ok(None)` for an index that matches `HEAD`. Nothing to commit is a fact
    /// about the worktree, not a failure of the caller — a worktree whose agent
    /// committed as it went is the normal case, not the broken one.
    ///
    /// Hooks are deliberately **not** bypassed: no `--no-verify`. A repository's
    /// own guard exists because somebody wanted it to run, and quietly stepping
    /// around it is worse than being slow. Being slow is survivable because
    /// nothing here runs on a UI thread.
    pub fn commit_staged(&self, message: &str) -> Result<Option<String>> {
        if self.staged_count()? == 0 {
            return Ok(None);
        }

        match run_in(&self.root, &["commit", "-m", message]) {
            Ok(_) => self.rev_parse("HEAD").map(Some),
            // git's own words here are a multi-line lecture about `git config`
            // wrapped in asterisks, which is unreadable in a dialog. The advice
            // in it is right, so it is kept and the presentation is not.
            Err(KetError::Git { stderr, .. })
                if stderr.contains("Please tell me who you are")
                    || stderr.contains("unable to auto-detect email address") =>
            {
                Err(KetError::Conflict(
                    "git has no author identity in this repository; set one with \
                     `git config --global user.email you@example.com` and \
                     `git config --global user.name \"Your Name\"`"
                        .to_owned(),
                ))
            }
            Err(e) => Err(e),
        }
    }

    /// Stages everything and commits it.
    pub fn commit_all(&self, message: &str) -> Result<CommitReport> {
        let files = self.stage_all()?;
        let commit = self.commit_staged(message)?;
        Ok(CommitReport {
            // Zero exactly when nothing was committed: `stage_all` counted an
            // index that `commit_staged` then found empty.
            files: if commit.is_some() { files } else { 0 },
            commit,
        })
    }

    /// Creates a worktree at `path` on a new branch `branch`, based on `base`.
    ///
    /// `--no-track` because `base` is often a remote-tracking ref now, and git
    /// would otherwise make `origin/main` the new branch's upstream — which
    /// turns a plain `git push` from inside the worktree into a push to main.
    /// The branch gets its upstream on its first push instead, see
    /// [`Git::ensure_push_auto_setup_remote`].
    pub fn worktree_add(&self, path: &Path, branch: &str, base: &str) -> Result<()> {
        let path_str = path.to_string_lossy();
        run_in(
            &self.root,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                branch,
                &path_str,
                base,
            ],
        )?;
        Ok(())
    }

    /// Creates a worktree at `path` checking out an existing `branch`.
    pub fn worktree_add_existing(&self, path: &Path, branch: &str) -> Result<()> {
        let path_str = path.to_string_lossy();
        run_in(&self.root, &["worktree", "add", &path_str, branch])?;
        Ok(())
    }

    /// Lists worktrees as git sees them.
    pub fn worktree_list(&self) -> Result<Vec<GitWorktree>> {
        let out = run_in(&self.root, &["worktree", "list", "--porcelain"])?;
        Ok(parse_worktree_porcelain(&out))
    }

    /// Removes a worktree.
    ///
    /// Without `force`, git refuses when the worktree has uncommitted changes.
    /// That refusal is a feature — losing an agent's work to a stray `rm` is
    /// exactly the failure this tool must never have.
    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let path_str = path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path_str);

        run_in(&self.root, &args)?;
        Ok(())
    }

    /// Prunes worktree metadata whose directories no longer exist.
    pub fn worktree_prune(&self) -> Result<()> {
        run_in(&self.root, &["worktree", "prune"])?;
        Ok(())
    }

    /// One line of `git submodule status`.
    pub fn submodule_status(&self) -> Result<Vec<Submodule>> {
        let out = run_in(&self.root, &["submodule", "status"])?;
        Ok(parse_submodule_status(&out))
    }

    /// Deletes a local branch.
    pub fn branch_delete(&self, branch: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };
        run_in(&self.root, &["branch", flag, branch])?;
        Ok(())
    }

    // ---- merge probes --------------------------------------------------
    //
    // All `Option`, like [`Git::base_remote`], and for the same reason: they
    // answer "can you prove this?", and an unreadable repository, an ancient
    // git or a ref that does not resolve are all the same answer — no. A
    // caller that turned those into errors would have to decide, at every one
    // of them, that the failure means "not merged" anyway.

    /// One config value, or `None` when it is unset.
    pub fn config_get(&self, key: &str) -> Option<String> {
        let out = run_in(&self.root, &["config", "--get", key]).ok()?;
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    /// What a symbolic ref points at, e.g. `refs/remotes/origin/HEAD`.
    pub fn symbolic_ref(&self, name: &str) -> Option<String> {
        let out = run_in(&self.root, &["symbolic-ref", "--quiet", name]).ok()?;
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    /// Resolves `rev` to a commit id.
    pub fn commit_of(&self, rev: &str) -> Option<String> {
        let spec = format!("{rev}^{{commit}}");
        let out = run_in(&self.root, &["rev-parse", "--verify", "--quiet", &spec]).ok()?;
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    /// `HEAD`'s short id and when it was committed, in Unix milliseconds.
    /// `None` on an unborn branch.
    pub fn head_stamp(&self) -> Option<(String, u64)> {
        let out = run_in(&self.root, &["log", "-1", "--format=%h %ct", "HEAD"]).ok()?;
        let (id, seconds) = out.trim().split_once(' ')?;
        Some((id.to_owned(), seconds.parse::<u64>().ok()? * 1_000))
    }

    /// Resolves `rev` to the tree it names.
    pub fn tree_of(&self, rev: &str) -> Option<String> {
        let spec = format!("{rev}^{{tree}}");
        let out = run_in(&self.root, &["rev-parse", "--verify", "--quiet", &spec]).ok()?;
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    /// The tree a merge of `branch` into `target` would produce.
    ///
    /// `None` when the merge conflicts — git exits non-zero — or when git is
    /// older than 2.38 and has no `--write-tree`. Both mean the same thing to
    /// the only caller: this has not been proved to be a no-op.
    pub fn merge_tree(&self, target: &str, branch: &str) -> Option<String> {
        let out = run_in(&self.root, &["merge-tree", "--write-tree", target, branch]).ok()?;
        let first = out.lines().next()?.trim();
        (!first.is_empty()).then(|| first.to_owned())
    }

    /// `git cherry`: which of `branch`'s commits are not yet in `target`.
    ///
    /// Lines starting `-` are already upstream as an equivalent patch, `+`
    /// are not. That distinction is the whole point — a rebase-merge leaves a
    /// branch whose commits have different ids and identical patches.
    pub fn cherry(&self, target: &str, branch: &str) -> Option<Vec<String>> {
        let out = run_in(&self.root, &["cherry", "-v", target, branch]).ok()?;
        Some(
            out.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }

    // ---- remotes and the base ------------------------------------------

    /// The remotes this repository knows, as `git remote` lists them.
    pub fn remotes(&self) -> Result<Vec<String>> {
        let out = run_in(&self.root, &["remote"])?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// The remote worth consulting for a base: `origin` when there is one,
    /// else the first listed, else none.
    pub fn base_remote(&self) -> Option<String> {
        let remotes = self.remotes().ok()?;
        remotes
            .iter()
            .find(|r| *r == "origin")
            .or_else(|| remotes.first())
            .cloned()
    }

    /// `git fetch --prune <remote>`, bounded by [`FETCH_TIMEOUT`].
    ///
    /// Never prompts: a remote that wants credentials nobody is there to type
    /// fails instead of hanging, and a hang past the timeout is killed.
    pub fn fetch(&self, remote: &str) -> Result<()> {
        let command = format!("fetch --prune {remote}");
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["fetch", "--prune", "--quiet", remote])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| KetError::io(&self.root, e))?;

        // Drained on its own thread so a chatty remote cannot fill the pipe
        // and deadlock against the wait below.
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let drain = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });

        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= FETCH_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(KetError::Git {
                        command,
                        status: format!("timed out after {}s", FETCH_TIMEOUT.as_secs()),
                        stderr: String::new(),
                    });
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(KetError::io(&self.root, e)),
            }
        };
        let stderr = drain.join().unwrap_or_default();
        if !status.success() {
            return Err(KetError::Git {
                command,
                status: status.to_string(),
                stderr: stderr.trim().to_owned(),
            });
        }
        Ok(())
    }

    /// Whether a fully qualified ref (`refs/heads/main`,
    /// `refs/remotes/origin/main`, `refs/tags/v1`) exists.
    pub fn ref_exists(&self, full_ref: &str) -> bool {
        run_in(&self.root, &["show-ref", "--verify", "--quiet", full_ref]).is_ok()
    }

    /// How many commits `left` has that `right` does not, and vice versa.
    pub fn ahead_behind(&self, left: &str, right: &str) -> Result<(u64, u64)> {
        let range = format!("{left}...{right}");
        let out = run_in(&self.root, &["rev-list", "--left-right", "--count", &range])?;
        let mut parts = out.split_whitespace();
        let parse = |part: Option<&str>| {
            part.and_then(|n| n.parse::<u64>().ok())
                .ok_or_else(|| KetError::Git {
                    command: format!("rev-list --left-right --count {range}"),
                    status: "unparseable".to_owned(),
                    stderr: out.trim().to_owned(),
                })
        };
        Ok((parse(parts.next())?, parse(parts.next())?))
    }

    /// The worktree that has `branch` checked out, if any — the primary
    /// checkout, usually, for a base branch.
    pub fn checkout_of(&self, branch: &str) -> Result<Option<PathBuf>> {
        let full = format!("refs/heads/{branch}");
        Ok(self
            .worktree_list()?
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(full.as_str()))
            .map(|w| w.path))
    }

    /// Moves `full_ref` from `old` to `new`, and only if it still is at
    /// `old`: git refuses when the ref moved under us, which is the
    /// compare-and-swap that makes updating a branch nobody has checked out
    /// safe without a lock.
    pub fn update_ref(&self, full_ref: &str, new: &str, old: &str) -> Result<()> {
        run_in(&self.root, &["update-ref", full_ref, new, old])?;
        Ok(())
    }

    /// `git reset --hard <rev>` in this checkout.
    pub fn reset_hard(&self, rev: &str) -> Result<()> {
        run_in(&self.root, &["reset", "--hard", "--quiet", rev])?;
        Ok(())
    }

    /// Throws away every uncommitted change to `paths`, staged or not, so
    /// each is as `HEAD` has it. Cannot be undone.
    ///
    /// Decided per path, because one rename can be both kinds: its old half is
    /// tracked and its new half may never have been added. A tracked path is
    /// restored from `HEAD` in the index and on disk, which also removes one
    /// that `HEAD` does not have — a staged addition. An untracked one has no
    /// `HEAD` copy to go back to, so it is deleted.
    ///
    /// Literal pathspecs, so a file named `*.rs` discards itself and not
    /// every Rust file in the checkout.
    pub fn discard(&self, paths: &[&str]) -> Result<()> {
        for path in paths {
            if self.tracked(path) {
                run_in(
                    &self.root,
                    &[
                        "--literal-pathspecs",
                        "restore",
                        "--source=HEAD",
                        "--staged",
                        "--worktree",
                        "--",
                        path,
                    ],
                )?;
            } else {
                // A file, never a directory: a row is one file, and a
                // recursive delete behind a one-file confirmation is not
                // something to reach by accident.
                let full = self.root.join(path);
                std::fs::remove_file(&full).map_err(|e| KetError::io(&full, e))?;
            }
        }
        Ok(())
    }

    /// Whether git knows `path` — in the index, or in `HEAD` for a deletion
    /// that has already been staged.
    fn tracked(&self, path: &str) -> bool {
        run_in(
            &self.root,
            &[
                "--literal-pathspecs",
                "ls-files",
                "--error-unmatch",
                "--",
                path,
            ],
        )
        .is_ok()
            || run_in(&self.root, &["cat-file", "-e", &format!("HEAD:{path}")]).is_ok()
    }

    /// Whether tracked files here have changes. Untracked files do not count:
    /// a fast-forward leaves them where they are, and a scratch file in the
    /// primary checkout should not stop the base from moving.
    fn has_tracked_changes(&self) -> Result<bool> {
        let out = run_in(
            &self.root,
            &["status", "--porcelain", "--untracked-files=no"],
        )?;
        Ok(!out.trim().is_empty())
    }

    /// Lets a plain `git push` from a fresh worktree create its own upstream,
    /// since [`Git::worktree_add`] no longer leaves one. Best effort and
    /// idempotent: the setting is shared repository config, written once and
    /// never overwritten if someone already chose a value.
    pub fn ensure_push_auto_setup_remote(&self) {
        if run_in(&self.root, &["config", "--get", "push.autoSetupRemote"])
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            return;
        }
        if let Err(e) = run_in(
            &self.root,
            &["config", "--local", "push.autoSetupRemote", "true"],
        ) {
            tracing::warn!(%e, "could not set push.autoSetupRemote");
        }
    }

    /// Decides what a new worktree is cut from.
    ///
    /// Fetches the remote first, so `origin/main` means the remote's main as
    /// of now rather than as of the last time someone pulled. Then, for a
    /// plain branch name:
    ///
    /// - remote strictly ahead: cut from the remote-tracking ref, and
    ///   fast-forward the local branch when nothing can be lost by it;
    /// - local ahead, or diverged: cut from the local branch, which is where
    ///   ket's own merges land, and say so;
    /// - no remote-tracking twin: cut from the local branch as before.
    ///
    /// `origin/main`, `refs/…`, tags, commits and `HEAD` are taken as given.
    /// Nothing here fails: a remote that cannot be reached is reported, not
    /// fatal, and the caller creates the worktree from whatever is here.
    pub fn prepare_base(&self, asked: &str) -> PreparedBase {
        let remote = self.base_remote();
        let fetch = match &remote {
            None => Fetch::NoRemote,
            Some(remote) => match self.fetch(remote) {
                Ok(()) => Fetch::Fetched {
                    remote: remote.clone(),
                },
                Err(e) => Fetch::Failed {
                    remote: remote.clone(),
                    why: e.to_string(),
                },
            },
        };
        let taken_as_given = |rev: String| PreparedBase {
            asked: asked.to_owned(),
            rev,
            fetch: fetch.clone(),
            refresh: LocalBaseRefresh::NotApplicable,
        };

        if asked.starts_with("refs/") {
            return taken_as_given(asked.to_owned());
        }
        // `origin/main` names a remote-tracking ref first; a local branch
        // with a slash in its name only when no remote has one by that name.
        if asked.contains('/') {
            let tracking = format!("refs/remotes/{asked}");
            if self.ref_exists(&tracking) {
                return taken_as_given(tracking);
            }
        }

        let local = format!("refs/heads/{asked}");
        let tracking = remote
            .as_ref()
            .map(|r| format!("refs/remotes/{r}/{asked}"))
            .filter(|r| self.ref_exists(r));
        match (self.ref_exists(&local), tracking) {
            (true, Some(tracking)) => {
                let (rev, refresh) = self.refresh_local_base(asked, &local, &tracking);
                PreparedBase {
                    asked: asked.to_owned(),
                    rev,
                    fetch: fetch.clone(),
                    refresh,
                }
            }
            (true, None) => taken_as_given(local),
            (false, Some(tracking)) => taken_as_given(tracking),
            // A tag, a commit id, `HEAD`: `git worktree add` resolves it.
            (false, None) => taken_as_given(asked.to_owned()),
        }
    }

    /// The remote-is-ahead half of [`Git::prepare_base`]: which ref to cut
    /// from, and what became of the local branch.
    fn refresh_local_base(
        &self,
        branch: &str,
        local: &str,
        tracking: &str,
    ) -> (String, LocalBaseRefresh) {
        let (ahead, behind) = match self.ahead_behind(local, tracking) {
            Ok(counts) => counts,
            Err(e) => {
                return (
                    local.to_owned(),
                    LocalBaseRefresh::Failed {
                        behind: 0,
                        why: e.to_string(),
                    },
                );
            }
        };
        if ahead > 0 {
            return (
                local.to_owned(),
                LocalBaseRefresh::LocalAhead { ahead, behind },
            );
        }
        if behind == 0 {
            return (local.to_owned(), LocalBaseRefresh::UpToDate);
        }

        // The remote is strictly ahead. The worktree is cut from it whatever
        // happens next; the rest is bringing the local branch along.
        let failed = |why: String| LocalBaseRefresh::Failed { behind, why };
        let refresh = match (self.rev_parse(tracking), self.rev_parse(local)) {
            (Ok(remote_oid), Ok(local_oid)) => match self.checkout_of(branch) {
                Ok(Some(checkout)) => {
                    let there = Git::new(&checkout);
                    match there.has_tracked_changes() {
                        Ok(true) => LocalBaseRefresh::DirtyCheckout { behind, checkout },
                        Ok(false) => match there.reset_hard(&remote_oid) {
                            Ok(()) => LocalBaseRefresh::Updated {
                                behind,
                                checkout: Some(checkout),
                            },
                            Err(e) => failed(e.to_string()),
                        },
                        Err(e) => failed(e.to_string()),
                    }
                }
                Ok(None) => match self.update_ref(local, &remote_oid, &local_oid) {
                    Ok(()) => LocalBaseRefresh::Updated {
                        behind,
                        checkout: None,
                    },
                    Err(e) => failed(e.to_string()),
                },
                Err(e) => failed(e.to_string()),
            },
            (Err(e), _) | (_, Err(e)) => failed(e.to_string()),
        };
        (tracking.to_owned(), refresh)
    }
}

/// What a commit did, or did not have to do.
///
/// Both fields rather than an `Option` alone, because "committed four files"
/// and "there was nothing to commit" are different answers to somebody deciding
/// whether their agent produced anything, and the second is not a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReport {
    /// The new commit's object id, or `None` when there was nothing to commit.
    pub commit: Option<String>,
    /// How many paths it touched. Zero exactly when `commit` is `None`.
    pub files: usize,
}

/// A submodule as `git submodule status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    /// Path relative to the repository root.
    pub path: String,
    /// Whether the submodule is checked out.
    ///
    /// False in every freshly created worktree: `git worktree add` leaves
    /// submodule directories empty, and nothing tells you until a build fails.
    pub initialised: bool,
}

/// Parses `git submodule status`.
///
/// Each line is a status character, an object id, the path, and optionally a
/// description in parentheses. The status character is the whole point: `-`
/// means "not initialised", and everything else means the submodule is present.
/// Note that the path may contain spaces, so it is taken as the *second*
/// whitespace-separated field and the rest of the line is ignored only from the
/// trailing `(...)` — which is why the description is stripped rather than the
/// line being split into three.
fn parse_submodule_status(text: &str) -> Vec<Submodule> {
    let mut out = Vec::new();

    for line in text.lines() {
        let Some(marker) = line.chars().next() else {
            continue;
        };

        // `<marker><oid> <path>[ (describe)]`
        let rest = &line[marker.len_utf8()..];
        let Some((_oid, after_oid)) = rest.split_once(' ') else {
            continue;
        };

        // Strip a trailing ` (...)` describe suffix without disturbing a path
        // that legitimately contains spaces or brackets.
        let path = match after_oid.rfind(" (") {
            Some(at) if after_oid.ends_with(')') => &after_oid[..at],
            _ => after_oid,
        };

        if path.is_empty() {
            continue;
        }

        out.push(Submodule {
            path: path.to_owned(),
            initialised: marker != '-',
        });
    }

    out
}

/// Parses `git worktree list --porcelain`.
///
/// Records are separated by blank lines. Each begins with a `worktree <path>`
/// line, followed by zero or more attributes. Some attributes are bare words
/// (`bare`, `detached`), others carry a value (`HEAD <oid>`, `branch <ref>`),
/// and `locked`/`prunable` may appear either way depending on whether git has a
/// reason string. Parsing the porcelain format rather than the human one is
/// deliberate: the human format is explicitly not a stable interface.
fn parse_worktree_porcelain(text: &str) -> Vec<GitWorktree> {
    let mut out = Vec::new();
    let mut current: Option<GitWorktree> = None;

    for line in text.lines() {
        let line = line.trim_end();

        if line.is_empty() {
            if let Some(worktree) = current.take() {
                out.push(worktree);
            }
            continue;
        }

        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, Some(v)),
            None => (line, None),
        };

        match key {
            "worktree" => {
                if let Some(worktree) = current.take() {
                    out.push(worktree);
                }
                current = Some(GitWorktree {
                    path: PathBuf::from(value.unwrap_or_default()),
                    head: None,
                    branch: None,
                    bare: false,
                    detached: false,
                    prunable: false,
                    locked: false,
                });
            }
            "HEAD" => {
                if let Some(w) = current.as_mut() {
                    w.head = value.map(str::to_owned);
                }
            }
            "branch" => {
                if let Some(w) = current.as_mut() {
                    w.branch = value.map(str::to_owned);
                }
            }
            "bare" => {
                if let Some(w) = current.as_mut() {
                    w.bare = true;
                }
            }
            "detached" => {
                if let Some(w) = current.as_mut() {
                    w.detached = true;
                }
            }
            "prunable" => {
                if let Some(w) = current.as_mut() {
                    w.prunable = true;
                }
            }
            "locked" => {
                if let Some(w) = current.as_mut() {
                    w.locked = true;
                }
            }
            _ => {}
        }
    }

    // A final record with no trailing blank line still counts.
    if let Some(worktree) = current.take() {
        out.push(worktree);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
worktree /Users/me/dev/ket
HEAD 0123456789abcdef0123456789abcdef01234567
branch refs/heads/main

worktree /Users/me/.local/share/ket/worktrees/ket-abc/feature-login-0123456789ab
HEAD fedcba9876543210fedcba9876543210fedcba98
branch refs/heads/feature/login

worktree /Users/me/detached-one
HEAD aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
detached

worktree /Users/me/gone
HEAD bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
branch refs/heads/gone
prunable gitdir file points to non-existent location
";

    #[test]
    fn parses_every_record() {
        let parsed = parse_worktree_porcelain(SAMPLE);
        assert_eq!(parsed.len(), 4);
    }

    #[test]
    fn parses_paths_and_branches() {
        let parsed = parse_worktree_porcelain(SAMPLE);
        assert_eq!(parsed[0].path, PathBuf::from("/Users/me/dev/ket"));
        assert_eq!(parsed[0].branch_name(), Some("main"));
    }

    #[test]
    fn keeps_slashes_inside_branch_names() {
        // `feature/login` must survive intact; it is only the *path* that gets
        // slugged, never the branch name itself.
        let parsed = parse_worktree_porcelain(SAMPLE);
        assert_eq!(parsed[1].branch_name(), Some("feature/login"));
    }

    #[test]
    fn recognises_detached_heads() {
        let parsed = parse_worktree_porcelain(SAMPLE);
        assert!(parsed[2].detached);
        assert_eq!(parsed[2].branch, None);
    }

    #[test]
    fn recognises_prunable_with_a_reason_string() {
        // `prunable` carries a trailing reason, so it must not be treated as a
        // key/value pair whose value is meaningful.
        let parsed = parse_worktree_porcelain(SAMPLE);
        assert!(parsed[3].prunable);
        assert_eq!(parsed[3].branch_name(), Some("gone"));
    }

    #[test]
    fn handles_a_missing_trailing_blank_line() {
        let text = "worktree /a\nHEAD abc\nbranch refs/heads/x";
        let parsed = parse_worktree_porcelain(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].branch_name(), Some("x"));
    }

    #[test]
    fn handles_bare_repositories() {
        let parsed = parse_worktree_porcelain("worktree /a\nbare\n");
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].bare);
        assert_eq!(parsed[0].head, None);
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse_worktree_porcelain("").is_empty());
        assert!(parse_worktree_porcelain("\n\n").is_empty());
    }

    const SUBMODULES: &str = "\
-0123456789abcdef0123456789abcdef01234567 libs/not-checked-out
 fedcba9876543210fedcba9876543210fedcba98 libs/ready (heads/main)
+aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa libs/different (v1.0-2-gabcdef)
Ubbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb libs/conflicted
";

    #[test]
    fn recognises_which_submodules_are_checked_out() {
        // Every fresh worktree starts with all of them uninitialised: git
        // creates the directories and leaves them empty. Getting this backwards
        // would mean never warning about the case that actually happens.
        let parsed = parse_submodule_status(SUBMODULES);

        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].path, "libs/not-checked-out");
        assert!(!parsed[0].initialised);

        for entry in &parsed[1..] {
            assert!(entry.initialised, "{entry:?} should count as present");
        }
    }

    #[test]
    fn strips_the_describe_suffix_without_eating_the_path() {
        let parsed = parse_submodule_status(SUBMODULES);
        assert_eq!(parsed[1].path, "libs/ready");
        assert_eq!(parsed[2].path, "libs/different");
    }

    #[test]
    fn keeps_submodule_paths_that_contain_spaces_and_brackets() {
        let text = " 0123456789abcdef0123456789abcdef01234567 vendor/my lib [v2] (heads/main)\n";
        let parsed = parse_submodule_status(text);
        assert_eq!(parsed[0].path, "vendor/my lib [v2]");
    }

    #[test]
    fn a_repository_without_submodules_yields_nothing() {
        assert!(parse_submodule_status("").is_empty());
        assert!(parse_submodule_status("\n").is_empty());
    }

    #[test]
    fn tolerates_unknown_attributes() {
        // git gains attributes over time; an unrecognised one must not drop the
        // record or shift subsequent ones.
        let text = "worktree /a\nHEAD abc\nsomethingnew value\nbranch refs/heads/x\n";
        let parsed = parse_worktree_porcelain(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].branch_name(), Some("x"));
    }
}
