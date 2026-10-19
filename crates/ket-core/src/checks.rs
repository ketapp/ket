//! A pull request's failed checks, handed to the agent in its worktree.
//!
//! "Fix checks with agent" — on the desktop's worktree menu, and on a phone —
//! reads what each failed GitHub Actions job printed, as the project's GitHub
//! account, and saves it where the agent can read it: ket's cache, as a
//! screenshot sent to an agent is (see [`crate::capture`]). The agent is told
//! which checks failed and where their output is, rather than handed the
//! output itself: a job's log runs to a thousand lines, and typed into its
//! terminal it would be the whole of the message a phone shows.
//!
//! It is told to push with `ket pr`, which pushes as the account the project
//! picked; a plain `git push` goes as whatever the agent's shell has.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::github::{self, Checkout, PullRequestStatus};
use crate::id::{ProjectId, WorktreeId};
use crate::workspace::Workspace;
use crate::{KetError, Result};

/// How many saved logs are kept; the oldest go as new ones are saved.
const KEPT: usize = 50;

/// What every saved log's file name starts with, so pruning touches only
/// these.
const PREFIX: &str = "checks-";

/// Where failed checks' logs are saved, in ket's cache.
pub fn logs_dir() -> Result<PathBuf> {
    Ok(crate::paths::cache_dir()?.join("checks"))
}

/// Where `gh` runs for `project`, and as whom: its checkout, as the account
/// it picked.
pub fn checkout(project: &ProjectId) -> Result<Checkout> {
    let workspace = Workspace::open()?;
    let root = workspace
        .projects()?
        .into_iter()
        .find(|known| &known.id == project)
        .map(|known| known.root)
        .ok_or_else(|| KetError::UnknownProject(project.as_str().to_owned()))?;
    let account = workspace.project_settings(project)?.github_account;
    Ok(Checkout::new(root, account))
}

/// What the agent in `worktree` is told about `status`'s failed checks: the
/// prompt to type, after saving what the failed jobs printed into `logs` —
/// [`logs_dir`], outside a test.
///
/// A job whose log cannot be read is still named, with why, and the agent
/// is pointed at its page; so is a check another service posted, whose log
/// lives on that service. The logs are read side by side, so a phone
/// waiting on this waits for the slowest, not for all of them in turn.
/// Fails only when nothing failed, or the log file cannot be written.
pub fn hand_over(
    at: &Checkout,
    worktree: &WorktreeId,
    status: &PullRequestStatus,
    logs: &Path,
) -> Result<String> {
    let failed: Vec<&github::Check> = status.failed().collect();
    if failed.is_empty() {
        return Err(KetError::Conflict(format!(
            "#{} has no failed checks to fix",
            status.number
        )));
    }

    let read: Vec<Result<Option<String>>> = std::thread::scope(|scope| {
        let reading: Vec<_> = failed
            .iter()
            .map(|&check| scope.spawn(move || github::failed_log(at, check)))
            .collect();
        reading
            .into_iter()
            .map(|reading| {
                reading
                    .join()
                    .unwrap_or_else(|_| Err(KetError::Conflict("reading the log panicked".into())))
            })
            .collect()
    });
    let mut saved = String::new();
    let mut elsewhere: Vec<&github::Check> = Vec::new();
    for (&check, log) in failed.iter().zip(read) {
        match log {
            Ok(Some(log)) => {
                let _ = write!(
                    saved,
                    "# {}\n{}\n\n{}\n",
                    check.name,
                    check.url,
                    log.trim_end()
                );
            }
            Ok(None) => elsewhere.push(check),
            Err(why) => {
                tracing::warn!(check = check.name, %why, "could not read a failed check's log");
                elsewhere.push(check);
            }
        }
    }

    let log = if saved.is_empty() {
        None
    } else {
        std::fs::create_dir_all(logs).map_err(|e| KetError::io(logs, e))?;
        prune(logs);
        let path = logs.join(format!(
            "{PREFIX}pr-{}-{}.log",
            status.number,
            crate::now_ms()
        ));
        std::fs::write(&path, saved).map_err(|e| KetError::io(&path, e))?;
        Some(path)
    };

    Ok(prompt(
        status.number,
        worktree,
        &failed,
        log.as_deref(),
        &elsewhere,
    ))
}

/// The words the agent is given — see [`hand_over`].
fn prompt(
    number: u64,
    worktree: &WorktreeId,
    failed: &[&github::Check],
    log: Option<&Path>,
    elsewhere: &[&github::Check],
) -> String {
    let names: Vec<&str> = failed.iter().map(|check| check.name.as_str()).collect();
    let mut prompt = format!(
        "The checks on pull request #{number} failed: {}.",
        names.join(", ")
    );
    if let Some(log) = log {
        let _ = write!(prompt, " What they printed is in {}.", log.display());
    }
    for check in elsewhere {
        if check.url.is_empty() {
            let _ = write!(prompt, " ket could not read {}'s log.", check.name);
        } else {
            let _ = write!(prompt, " {}'s log is at {}.", check.name, check.url);
        }
    }
    let _ = write!(
        prompt,
        " Find out why they failed and fix it. Commit the fix, then run \
         `ket pr {worktree}` to push the branch so the checks run again."
    );
    prompt
}

/// Removes the oldest saved logs past [`KEPT`], making room for one more.
/// Best effort: a log that cannot be removed only takes up space.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(PREFIX))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    if logs.len() < KEPT {
        return;
    }
    logs.sort();
    for (_, path) in logs.iter().take(logs.len() + 1 - KEPT) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Check, CheckState};

    fn check(name: &str, url: &str) -> Check {
        Check {
            name: name.to_owned(),
            state: CheckState::Failed,
            url: url.to_owned(),
        }
    }

    #[test]
    fn the_prompt_names_the_checks_the_log_and_how_to_push() {
        let clippy = check("clippy", "https://github.com/o/r/actions/runs/1/job/2");
        let prompt = prompt(
            17,
            &WorktreeId::new("wt-1"),
            &[&clippy],
            Some(Path::new("/cache/ket/checks/checks-pr-17-1.log")),
            &[],
        );
        assert_eq!(
            prompt,
            "The checks on pull request #17 failed: clippy. What they printed is in \
             /cache/ket/checks/checks-pr-17-1.log. Find out why they failed and fix it. \
             Commit the fix, then run `ket pr wt-1` to push the branch so the checks run again."
        );
    }

    #[test]
    fn a_check_with_its_log_elsewhere_is_pointed_at() {
        let circle = check("ci/circleci", "https://circleci.com/gh/o/r/1");
        let silent = check("deploy", "");
        let prompt = prompt(
            9,
            &WorktreeId::new("wt"),
            &[&circle, &silent],
            None,
            &[&circle, &silent],
        );
        assert!(
            prompt.starts_with("The checks on pull request #9 failed: ci/circleci, deploy. "),
            "{prompt}"
        );
        assert!(!prompt.contains("What they printed"), "{prompt}");
        assert!(
            prompt.contains("ci/circleci's log is at https://circleci.com/gh/o/r/1."),
            "{prompt}"
        );
        assert!(
            prompt.contains("ket could not read deploy's log."),
            "{prompt}"
        );
    }
}
