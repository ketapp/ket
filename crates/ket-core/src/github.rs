//! GitHub issues and pull requests, through the `gh` command line.
//!
//! `gh` rather than the REST API and a token of ket's own: it already holds
//! the person's logins, in their keychain. A project that picks one of
//! several accounts has that account's token handed to each of its calls —
//! see [`Checkout`] — and ket keeps none of them. Nobody has
//! to type a `gh` command, though: [`install`] fetches it with Homebrew, and
//! [`start_login`] runs its browser sign-in and hands back the one-time code
//! for ket to show. Nothing here asks for a credential or reads one.
//!
//! Every call runs in a project's checkout and lets `gh` work out the
//! repository from its remotes, the way it does on the command line. Each is
//! bounded by [`TIMEOUT`] and never prompts.
//!
//! A push is git's, not `gh`'s, but it needs the same account: see
//! [`git_credential_helper`], which lends `gh` to git for one push.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{KetError, Result};

/// How long one `gh` call may take before it is killed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Most open issues one [`open_issues`] reads. A repository with more is
/// read short, and [`OpenIssues::complete`] says so.
const LIST_LIMIT: usize = 500;

/// Where a `gh` call runs, and as whom: a project's checkout, whose remotes
/// name the repository, and the account the project picked — `None` for
/// whichever `gh` has active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// The checkout's root.
    pub root: PathBuf,
    /// The GitHub login to act as.
    pub account: Option<String>,
    /// The `gh` to run, when it is not the one on the login shell's PATH.
    ///
    /// A test's seam, and nothing else's: `gh` is found on the login shell's
    /// PATH, which a test cannot reach into, so a test that wants to see
    /// what ket would run hands in a script of its own here. `None`, which
    /// every caller in the app passes, is the real `gh`.
    pub program: Option<PathBuf>,
}

impl Checkout {
    /// A checkout acting as `account`, with the real `gh`.
    pub fn new(root: impl Into<PathBuf>, account: Option<String>) -> Self {
        Self {
            root: root.into(),
            account,
            program: None,
        }
    }

    /// Where its `gh` is.
    fn gh(&self) -> Result<PathBuf> {
        match &self.program {
            Some(program) => Ok(program.clone()),
            None => gh(),
        }
    }
}

/// An issue a note is linked to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLink {
    /// Its number in the repository, as `#142` shows it.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
}

/// A pull request a worktree's branch is open as.
///
/// The same two facts as an [`IssueLink`], and its own type anyway: a
/// worktree's pull request and a note's issue are never the same thing, and
/// one type for both would let a number cross from one to the other
/// without anything noticing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestLink {
    /// Its number in the repository, as `#142` shows it.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
}

/// An open issue, as [`open_issues`] reads it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Issue {
    /// Its number in the repository.
    pub number: u64,
    /// Its title.
    pub title: String,
    /// Its description, in markdown. Empty when it has none.
    #[serde(default)]
    pub body: String,
    /// Its page on GitHub.
    pub url: String,
}

impl Issue {
    /// The link a note keeps to it.
    pub fn link(&self) -> IssueLink {
        IssueLink {
            number: self.number,
            url: self.url.clone(),
        }
    }
}

/// Every open issue in a repository — or as many as were read.
#[derive(Debug, Clone, Default)]
pub struct OpenIssues {
    /// The issues, newest first.
    pub issues: Vec<Issue>,
    /// Whether that is all of them. Only a complete list can say an issue
    /// missing from it was closed.
    pub complete: bool,
}

/// Why an issue is being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The work was done.
    Completed,
    /// It will not be done.
    NotPlanned,
}

/// Opens an issue in the repository `at` is a checkout of.
pub fn create_issue(at: &Checkout, title: &str, body: &str) -> Result<IssueLink> {
    let out = run(
        at,
        &["issue", "create", "--title", title, "--body-file", "-"],
        Some(body),
    )?;
    let (number, url) = link_in_output(&out, "issue")?;
    Ok(IssueLink { number, url })
}

/// Opens a pull request for `head` into `base` in the repository `at` is a
/// checkout of. `head` has to be on the remote already: this names it, it
/// does not push it.
pub fn create_pull_request(
    at: &Checkout,
    base: &str,
    head: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<PullRequestLink> {
    let mut args = vec![
        "pr",
        "create",
        "--base",
        base,
        "--head",
        head,
        "--title",
        title,
        "--body-file",
        "-",
    ];
    if draft {
        args.push("--draft");
    }
    let out = run(at, &args, Some(body))?;
    let (number, url) = link_in_output(&out, "pull request")?;
    Ok(PullRequestLink { number, url })
}

/// The open pull request whose head is `head`, if there is one — opened by
/// ket, or by an agent that ran `gh pr create` itself.
pub fn open_pull_request_for(at: &Checkout, head: &str) -> Result<Option<PullRequestLink>> {
    let out = run(
        at,
        &[
            "pr",
            "list",
            "--head",
            head,
            "--state",
            "open",
            "--json",
            "number,url",
        ],
        None,
    )?;
    read_pull_requests(&out).map(|mut open| open.drain(..).next())
}

/// The pull requests `gh pr list --json number,url` printed.
fn read_pull_requests(out: &str) -> Result<Vec<PullRequestLink>> {
    serde_json::from_str(out)
        .map_err(|e| failed(format!("could not read gh's list of pull requests: {e}")))
}

/// Most open pull requests one [`open_pull_requests`] reads.
const PULL_REQUEST_LIMIT: usize = 100;

/// A pull request open from one of the repository's own branches, with what
/// GitHub says of it now: whether it is a draft, where its review stands,
/// and its checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestStatus {
    /// Its number in the repository.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
    /// The branch it is open from.
    pub branch: String,
    /// Whether it is still a draft.
    pub draft: bool,
    /// What its reviewers have decided, as far as they have.
    pub review: Review,
    /// Its checks, in GitHub's order.
    pub checks: Vec<Check>,
}

/// Where a pull request's review stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    /// None is needed, or none has been given.
    #[default]
    None,
    /// Approved.
    Approved,
    /// A reviewer asked for changes.
    ChangesRequested,
    /// The repository's rules want a review it does not have yet.
    Required,
}

/// One check on a pull request: a GitHub Actions job, or a status another
/// service posted on its commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// What GitHub calls it: the job's name, or the status's context.
    pub name: String,
    /// How it stands.
    pub state: CheckState,
    /// Its page — the job's log, or wherever the service points. Empty when
    /// it gave none.
    pub url: String,
}

/// How a check stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// It passed, or finished with nothing to say against the change.
    Passed,
    /// It failed, errored or timed out.
    Failed,
    /// Queued or running.
    Pending,
    /// Skipped, cancelled or superseded: it says nothing either way.
    Skipped,
}

impl PullRequestStatus {
    /// The link a worktree keeps to it.
    pub fn link(&self) -> PullRequestLink {
        PullRequestLink {
            number: self.number,
            url: self.url.clone(),
        }
    }

    /// Its checks taken together: failed if any failed, else pending if any
    /// is still going, else passed if any passed. `None` when it has no
    /// check that says anything.
    pub fn checks_state(&self) -> Option<CheckState> {
        let any = |state| self.checks.iter().any(|check| check.state == state);
        [CheckState::Failed, CheckState::Pending, CheckState::Passed]
            .into_iter()
            .find(|&state| any(state))
    }

    /// The checks that failed.
    pub fn failed(&self) -> impl Iterator<Item = &Check> {
        self.checks
            .iter()
            .filter(|check| check.state == CheckState::Failed)
    }

    /// Its checks in a few words, as a tooltip or a detail line says them:
    /// "2 of 9 checks failing", "3 checks running", "all 9 checks passed".
    /// Empty when it has none that say anything.
    pub fn checks_summary(&self) -> String {
        let counted: Vec<&Check> = self
            .checks
            .iter()
            .filter(|check| check.state != CheckState::Skipped)
            .collect();
        let total = counted.len();
        let count = |state| counted.iter().filter(|check| check.state == state).count();
        let checks = |n: usize| if n == 1 { "check" } else { "checks" };
        match self.checks_state() {
            None => String::new(),
            Some(CheckState::Failed) => {
                let failed = count(CheckState::Failed);
                if failed == total {
                    format!("{failed} {} failing", checks(failed))
                } else {
                    format!("{failed} of {total} {} failing", checks(total))
                }
            }
            Some(CheckState::Pending) => {
                let pending = count(CheckState::Pending);
                format!("{pending} {} running", checks(pending))
            }
            Some(CheckState::Passed | CheckState::Skipped) if total == 1 => {
                "1 check passed".to_owned()
            }
            Some(CheckState::Passed | CheckState::Skipped) => {
                format!("all {total} checks passed")
            }
        }
    }
}

/// The open pull requests of the repository `at` is a checkout of, with
/// their checks — one call for every worktree of a project, matched to them
/// by branch. Opened by ket or by an agent that ran `gh pr create` itself.
///
/// Pull requests from forks are left out: a fork's branch names are
/// someone else's, and a fork's `fix-typo` is not this repository's.
pub fn open_pull_requests(at: &Checkout) -> Result<Vec<PullRequestStatus>> {
    let limit = PULL_REQUEST_LIMIT.to_string();
    let out = run(
        at,
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            &limit,
            "--json",
            "number,url,headRefName,isDraft,isCrossRepository,reviewDecision,statusCheckRollup",
        ],
        None,
    )?;
    read_pull_request_statuses(&out)
}

/// The pull requests [`open_pull_requests`] had `gh` print, forks left out.
fn read_pull_request_statuses(out: &str) -> Result<Vec<PullRequestStatus>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Listed {
        number: u64,
        url: String,
        head_ref_name: String,
        #[serde(default)]
        is_draft: bool,
        #[serde(default)]
        is_cross_repository: bool,
        #[serde(default)]
        review_decision: Option<String>,
        #[serde(default)]
        status_check_rollup: Option<Vec<Rolled>>,
    }

    /// One entry of `statusCheckRollup`: a check run, or a commit status.
    #[derive(Deserialize)]
    #[serde(tag = "__typename")]
    enum Rolled {
        CheckRun(CheckRun),
        StatusContext(StatusContext),
        #[serde(other)]
        Other,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CheckRun {
        #[serde(default)]
        name: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        conclusion: String,
        #[serde(default)]
        details_url: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct StatusContext {
        #[serde(default)]
        context: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        target_url: String,
    }

    let listed: Vec<Listed> = serde_json::from_str(out)
        .map_err(|e| failed(format!("could not read gh's list of pull requests: {e}")))?;
    Ok(listed
        .into_iter()
        .filter(|pr| !pr.is_cross_repository)
        .map(|pr| PullRequestStatus {
            number: pr.number,
            url: pr.url,
            branch: pr.head_ref_name,
            draft: pr.is_draft,
            review: match pr.review_decision.as_deref() {
                Some("APPROVED") => Review::Approved,
                Some("CHANGES_REQUESTED") => Review::ChangesRequested,
                Some("REVIEW_REQUIRED") => Review::Required,
                _ => Review::None,
            },
            checks: pr
                .status_check_rollup
                .unwrap_or_default()
                .into_iter()
                .filter_map(|rolled| match rolled {
                    Rolled::CheckRun(run) => Some(Check {
                        state: match (run.status.as_str(), run.conclusion.as_str()) {
                            ("COMPLETED", "SUCCESS" | "NEUTRAL") => CheckState::Passed,
                            (
                                "COMPLETED",
                                "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED",
                            ) => CheckState::Failed,
                            ("COMPLETED", _) => CheckState::Skipped,
                            _ => CheckState::Pending,
                        },
                        name: run.name,
                        url: run.details_url,
                    }),
                    Rolled::StatusContext(status) => Some(Check {
                        state: match status.state.as_str() {
                            "SUCCESS" => CheckState::Passed,
                            "FAILURE" | "ERROR" => CheckState::Failed,
                            _ => CheckState::Pending,
                        },
                        name: status.context,
                        url: status.target_url,
                    }),
                    Rolled::Other => None,
                })
                .collect(),
        })
        .collect())
}

/// What `check`'s failed steps printed, when it is a GitHub Actions job —
/// its page names the job — else `None`: another service's status keeps its
/// log on that service. Tidied for reading: the job's name and each line's
/// timestamp taken off, and a heading wherever the step changes.
pub fn failed_log(at: &Checkout, check: &Check) -> Result<Option<String>> {
    let Some(job) = job_of(&check.url) else {
        return Ok(None);
    };
    let job = job.to_string();
    let out = run(at, &["run", "view", "--job", &job, "--log-failed"], None)?;
    Ok(Some(readable_log(&out)))
}

/// The Actions job a check's page is, from an address like
/// `https://github.com/o/r/actions/runs/123/job/456`.
fn job_of(url: &str) -> Option<u64> {
    let (_, rest) = url.split_once("/actions/runs/")?;
    let (_, job) = rest.split_once("/job/")?;
    job.split(['/', '?', '#']).next()?.parse().ok()
}

/// `gh run view --log-failed`'s output for a person or an agent to read:
/// each line is `job<TAB>step<TAB>timestamp text`, and only the text and a
/// heading where the step changes are kept. `UNKNOWN STEP` is what `gh`
/// says when it could not tell the steps apart, which is no heading.
fn readable_log(out: &str) -> String {
    let mut readable = String::with_capacity(out.len());
    let mut step: Option<&str> = None;
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(_job), Some(this_step), Some(rest)) = (parts.next(), parts.next(), parts.next())
        else {
            readable.push_str(line);
            readable.push('\n');
            continue;
        };
        if step != Some(this_step) {
            if this_step != "UNKNOWN STEP" {
                if !readable.is_empty() {
                    readable.push('\n');
                }
                readable.push_str("== ");
                readable.push_str(this_step);
                readable.push_str(" ==\n");
            }
            step = Some(this_step);
        }
        let rest = rest.trim_start_matches('\u{feff}');
        let text = match rest.split_once(' ') {
            Some((stamp, text)) if is_timestamp(stamp) => text,
            _ if is_timestamp(rest) => "",
            _ => rest,
        };
        readable.push_str(text);
        readable.push('\n');
    }
    readable
}

/// Whether `word` is an Actions log line's timestamp,
/// `2026-10-15T20:31:18.1053761Z`.
fn is_timestamp(word: &str) -> bool {
    word.len() >= 20
        && word.ends_with('Z')
        && word.as_bytes().get(10) == Some(&b'T')
        && word.as_bytes()[..4].iter().all(u8::is_ascii_digit)
}

/// The thing `gh … create` says it made, from what it printed: its number
/// and its address. `what` names it in the error when it printed neither.
fn link_in_output(out: &str, what: &str) -> Result<(u64, String)> {
    // `gh issue create` and `gh pr create` print the new thing's address
    // last, after any chatter about where it is being made.
    let url = out
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("https://"))
        .ok_or_else(|| {
            failed(format!(
                "gh did not say which {what} it made: {}",
                out.trim()
            ))
        })?;
    let number = url
        .rsplit('/')
        .next()
        .and_then(|tail| tail.parse().ok())
        .ok_or_else(|| failed(format!("gh made a {what} at {url}, with no number in it")))?;
    Ok((number, url.to_owned()))
}

/// What a `git push` needs to go as `at`'s account: the credential helper
/// that lends it `gh`, and the token that tells `gh` which account.
///
/// `gh auth git-credential` is what `gh auth setup-git` installs in a
/// person's git config; handed to one push on the command line instead, it
/// answers that push with `GH_TOKEN`'s account when one is set, else with
/// `gh`'s active account, and nothing is written anywhere. An SSH remote
/// never asks, so the helper is harmless there.
///
/// `None` when `gh` is not installed: the push then goes as git's own
/// configuration decides, which is what happened before ket could push.
pub fn git_credential_helper(at: &Checkout) -> Result<Option<PushCredentials>> {
    let Ok(gh) = at.gh() else {
        return Ok(None);
    };
    let token = token_for(at)?;
    Ok(Some(PushCredentials {
        helper: format!("!{} auth git-credential", gh.display()),
        token,
    }))
}

/// How a push identifies itself — see [`git_credential_helper`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushCredentials {
    /// The `credential.helper` value to run the push with.
    pub helper: String,
    /// The account's token, for `GH_TOKEN`; `None` for `gh`'s active account.
    pub token: Option<String>,
}

impl PushCredentials {
    /// The environment the push runs with.
    pub fn env(&self) -> Vec<(&str, &str)> {
        self.token
            .as_deref()
            .map(|token| vec![("GH_TOKEN", token)])
            .unwrap_or_default()
    }
}

/// The open issues of the repository `at` is a checkout of.
pub fn open_issues(at: &Checkout) -> Result<OpenIssues> {
    let limit = LIST_LIMIT.to_string();
    let out = run(
        at,
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            &limit,
            "--json",
            "number,title,body,url",
        ],
        None,
    )?;
    read_issues(&out)
}

/// The issues `gh issue list --json` printed, tidied, and whether that is
/// all of them.
fn read_issues(out: &str) -> Result<OpenIssues> {
    let mut issues: Vec<Issue> = serde_json::from_str(out)
        .map_err(|e| failed(format!("could not read gh's list of issues: {e}")))?;
    for issue in &mut issues {
        // Bodies written in a browser arrive with Windows line ends.
        issue.body = issue.body.replace("\r\n", "\n").trim().to_owned();
        issue.title = issue.title.trim().to_owned();
    }
    Ok(OpenIssues {
        complete: issues.len() < LIST_LIMIT,
        issues,
    })
}

/// Closes issue `number`.
pub fn close_issue(at: &Checkout, number: u64, reason: CloseReason) -> Result<()> {
    let reason = match reason {
        CloseReason::Completed => "completed",
        CloseReason::NotPlanned => "not planned",
    };
    run(
        at,
        &["issue", "close", &number.to_string(), "--reason", reason],
        None,
    )
    .map(|_| ())
}

/// Reopens issue `number`.
pub fn reopen_issue(at: &Checkout, number: u64) -> Result<()> {
    run(at, &["issue", "reopen", &number.to_string()], None).map(|_| ())
}

/// An account `gh` is signed in to on github.com.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Account {
    /// The GitHub username.
    #[serde(default)]
    pub login: String,
    /// Whether it is the one `gh` uses when not told otherwise.
    #[serde(default)]
    pub active: bool,
    /// `error` once GitHub no longer takes its token; `timeout` when it
    /// could not be asked.
    #[serde(default)]
    state: String,
    /// Where its token comes from: the keychain, or an environment variable.
    #[serde(default, rename = "tokenSource")]
    token_source: String,
}

impl Account {
    /// Whether GitHub still takes it, as far as `gh` could tell.
    pub fn works(&self) -> bool {
        self.state != "error"
    }

    /// Whether a project can pick it: not a token from an environment
    /// variable, which `gh` cannot hand out by name.
    pub fn pickable(&self) -> bool {
        !self.token_source.contains("TOKEN")
    }
}

/// The accounts `gh` is signed in to on github.com, the active one first.
/// `None` when `gh` is not installed.
pub fn accounts() -> Result<Option<Vec<Account>>> {
    let Ok(gh) = gh() else {
        return Ok(None);
    };
    // With `--json`, `gh` succeeds whatever state the accounts are in.
    let out = run_program(
        &gh,
        &[
            "auth",
            "status",
            "--hostname",
            "github.com",
            "--json",
            "hosts",
        ],
        &crate::paths::home()?,
        None,
        TIMEOUT,
        &[],
    )
    .map_err(|e| match e {
        KetError::Conflict(why) if why.contains("unknown flag") => {
            failed("This gh is too old for ket. Update it with `brew upgrade gh`.".to_owned())
        }
        e => e,
    })?;
    read_accounts(&out).map(Some)
}

/// The github.com accounts `gh auth status --json hosts` printed, the
/// active one first.
fn read_accounts(out: &str) -> Result<Vec<Account>> {
    #[derive(Deserialize)]
    struct Status {
        #[serde(default)]
        hosts: std::collections::BTreeMap<String, Vec<Account>>,
    }
    let status: Status = serde_json::from_str(out)
        .map_err(|e| failed(format!("could not read gh's accounts: {e}")))?;
    let mut accounts: Vec<Account> = status
        .hosts
        .into_iter()
        .filter(|(host, _)| host == "github.com")
        .flat_map(|(_, accounts)| accounts)
        .filter(|account| !account.login.is_empty())
        .collect();
    accounts.sort_by_key(|account| !account.active);
    Ok(accounts)
}

/// Signs `gh` out of `login` on github.com.
pub fn sign_out(login: &str) -> Result<()> {
    run_program(
        &gh()?,
        &[
            "auth",
            "logout",
            "--hostname",
            "github.com",
            "--user",
            login,
        ],
        &crate::paths::home()?,
        None,
        TIMEOUT,
        &[],
    )
    .map(|_| ())
}

/// The `owner/name` of the GitHub repository `at` is a checkout of, as
/// `gh` resolves it from the remotes — read as `at`'s account, so one that
/// account cannot see is an error here rather than at the first issue.
pub fn repository(at: &Checkout) -> Result<String> {
    let name = run(
        at,
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "--jq",
            ".nameWithOwner",
        ],
        None,
    )?;
    Ok(name.trim().to_owned())
}

/// Installs `gh` with Homebrew. Fails, saying where to get it, on a Mac
/// without Homebrew.
pub fn install() -> Result<()> {
    let brew = crate::shell::find_on_login_path("brew").ok_or_else(|| {
        failed(
            "Homebrew is not installed, so ket cannot install gh. Get it from cli.github.com."
                .to_owned(),
        )
    })?;
    let home = crate::paths::home()?;
    run_program(
        &brew,
        &["install", "gh"],
        &home,
        None,
        INSTALL_TIMEOUT,
        &[("HOMEBREW_NO_ENV_HINTS", "1")],
    )
    .map(|_| ())
}

/// How long [`install`] may take: Homebrew may update itself first.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long a [`Login`] waits for the code to be entered: as long as GitHub
/// keeps a device code.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A sign-in under way: `gh auth login --web`, waiting for the person to
/// enter [`Login::code`] at [`Login::url`].
pub struct Login {
    /// The one-time code to enter.
    pub code: String,
    /// Where to enter it.
    pub url: String,
    child: Child,
    /// The rest of what `gh` says, for when it fails.
    said: std::sync::mpsc::Receiver<String>,
    /// The account that was active before, made active again afterwards:
    /// signing in to another account makes it `gh`'s active one, and which
    /// one that is stays the person's choice, not ket's.
    restore: Option<String>,
}

/// Starts signing in to github.com in the browser. Returns once `gh` has
/// a code to show; [`Login::wait`] waits for it to be entered.
///
/// No `--git-protocol`: without it `gh` signs in and leaves the person's
/// git configuration alone.
pub fn start_login() -> Result<Login> {
    let gh = gh()?;
    let restore = accounts()
        .ok()
        .flatten()
        .and_then(|accounts| accounts.into_iter().find(|account| account.active))
        .map(|account| account.login);
    let mut child = command(&gh, &crate::paths::home()?, &[])
        .args(["auth", "login", "--web", "--hostname", "github.com"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| KetError::io(&gh, e))?;
    let stderr = child.stderr.take().expect("stderr was piped");
    let (tell, said) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if tell.send(line).is_err() {
                break;
            }
        }
    });

    // `! One-time code (ABCD-1234) copied to clipboard`, then the address.
    let mut code = None;
    let mut heard = Vec::new();
    let started = Instant::now();
    while started.elapsed() < TIMEOUT {
        let Ok(line) = said.recv_timeout(Duration::from_millis(200)) else {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            continue;
        };
        if let Some(found) = login_code(&line) {
            code = Some(found.to_owned());
        }
        if let Some(url) = line
            .split_whitespace()
            .find(|word| word.starts_with("https://"))
            && let Some(code) = code.take()
        {
            return Ok(Login {
                code,
                url: url.to_owned(),
                child,
                said,
                restore,
            });
        }
        heard.push(line);
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(failed(match heard.join("\n").trim() {
        "" => "gh did not give a code to sign in with".to_owned(),
        said => said.to_owned(),
    }))
}

/// The one-time code a line of `gh auth login --web` shows, if it shows
/// one: the first word after "code" shaped like `ABCD-1234`. gh words the
/// line differently with and without `--clipboard` — "code (ABCD-1234)
/// copied" against "code: ABCD-1234" — so the word right after is not
/// always it.
fn login_code(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once("code")?;
    rest.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|word| {
            word.split_once('-').is_some_and(|(left, right)| {
                [left, right]
                    .iter()
                    .all(|part| part.len() >= 4 && part.chars().all(|c| c.is_ascii_alphanumeric()))
            })
        })
}

impl Login {
    /// Waits for the code to be entered and `gh` to save the sign-in.
    pub fn wait(mut self) -> Result<()> {
        let started = Instant::now();
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= LOGIN_TIMEOUT => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(failed("The code expired before it was entered.".to_owned()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => return Err(failed(e.to_string())),
            }
        };
        if status.success() {
            if let Some(login) = &self.restore {
                // Best effort: the sign-in itself worked either way.
                let _ = run_program(
                    &gh()?,
                    &[
                        "auth",
                        "switch",
                        "--hostname",
                        "github.com",
                        "--user",
                        login,
                    ],
                    &crate::paths::home()?,
                    None,
                    TIMEOUT,
                    &[],
                );
            }
            return Ok(());
        }
        let said: Vec<String> = self.said.try_iter().collect();
        Err(failed(match said.join("\n").trim() {
            "" => format!("gh could not sign in ({status})"),
            said => said.to_owned(),
        }))
    }
}

/// Runs `gh args` in `at`, as its account, feeding it `input`, and returns
/// what it printed.
///
/// An account is used by handing this one call its token in `GH_TOKEN`,
/// asked of `gh` itself just before. It goes nowhere else — not stored, not
/// logged — and `gh`'s active account is left as it was.
fn run(at: &Checkout, args: &[&str], input: Option<&str>) -> Result<String> {
    let gh = at.gh()?;
    let token = token_for(at)?;
    let env: Vec<(&str, &str)> = token
        .as_deref()
        .map(|token| vec![("GH_TOKEN", token)])
        .unwrap_or_default();
    run_program(&gh, args, &at.root, input, TIMEOUT, &env).map_err(|e| match e {
        KetError::Conflict(why) if why.contains("gh auth login") => failed(
            "ket is not signed in to GitHub. Sign in in Settings \u{2192} GitHub.".to_owned(),
        ),
        e => e,
    })
}

/// The token of the account `at` picked, asked of `gh` just now; `None`
/// when it picked none and `gh`'s active account will do.
///
/// The token goes nowhere but the one call it is fetched for — not stored,
/// not logged — and `gh`'s active account is left as it was.
fn token_for(at: &Checkout) -> Result<Option<String>> {
    let Some(login) = &at.account else {
        return Ok(None);
    };
    run_program(
        &at.gh()?,
        &["auth", "token", "--hostname", "github.com", "--user", login],
        &at.root,
        None,
        TIMEOUT,
        &[],
    )
    .map(|token| Some(token.trim().to_owned()))
    .map_err(|_| {
        failed(format!(
            "@{login} is no longer signed in to GitHub on this Mac. Sign in again in \
             Settings \u{2192} GitHub, or pick another account in the project's settings."
        ))
    })
}

/// A `program` in `dir` that never prompts, with the login shell's PATH and
/// `env` set.
fn command(program: &Path, dir: &Path, env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(dir)
        // Never ask: there is nobody at a terminal to answer.
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        // Only ever the token `env` gives: one inherited from the shell
        // would quietly outrank the account the project picked.
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .env_remove("GH_HOST")
        .env_remove("GH_REPO")
        .envs(env.iter().copied());
    // A GUI app inherits launchd's short PATH, and `gh` runs `git`.
    if let Ok(path) = std::env::join_paths(crate::shell::login_path()) {
        command.env("PATH", path);
    }
    command
}

/// Runs `program args` in `dir`, feeding it `input`, for up to `timeout`,
/// and returns what it printed.
fn run_program(
    program: &Path,
    args: &[&str],
    dir: &Path,
    input: Option<&str>,
    timeout: Duration,
    env: &[(&str, &str)],
) -> Result<String> {
    let mut child = command(program, dir, env)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| KetError::io(program, e))?;

    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| KetError::io(program, e))?;
    }

    // Both drained on their own threads so a long list cannot fill a pipe
    // and deadlock against the wait below.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));

    let name = program
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(format!(
                    "{name} {} took longer than {}s",
                    args.first().copied().unwrap_or_default(),
                    timeout.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(KetError::io(program, e)),
        }
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !status.success() {
        let err = err.trim();
        return Err(failed(if err.is_empty() {
            format!("{name} {} failed ({status})", args.join(" "))
        } else {
            err.to_owned()
        }));
    }
    Ok(out)
}

/// Where `gh` is installed.
fn gh() -> Result<PathBuf> {
    crate::shell::find_on_login_path("gh").ok_or_else(|| {
        failed(
            "GitHub needs the gh command line. Install it in Settings \u{2192} GitHub.".to_owned(),
        )
    })
}

fn failed(why: String) -> KetError {
    KetError::Conflict(why)
}

#[cfg(test)]
mod tests {
    //! What ket reads off `gh`'s output. None of these run `gh`: each feeds
    //! a helper the text `gh` prints.

    use super::*;

    #[test]
    fn a_new_issue_is_read_off_the_address_gh_prints() {
        let (number, url) =
            link_in_output("https://github.com/ketapp/ket/issues/142\n", "issue").unwrap();
        assert_eq!(number, 142);
        assert_eq!(url, "https://github.com/ketapp/ket/issues/142");
    }

    #[test]
    fn a_new_pull_request_is_read_the_same_way() {
        let (number, url) =
            link_in_output("https://github.com/ketapp/ket/pull/143\n", "pull request").unwrap();
        assert_eq!(number, 143);
        assert_eq!(url, "https://github.com/ketapp/ket/pull/143");
    }

    #[test]
    fn the_address_is_the_last_one_gh_prints_after_any_chatter() {
        // `gh` may say where it is creating the issue before the address.
        let out = "\nCreating issue in ketapp/ket\n\n  https://github.com/ketapp/ket/issues/7  \n";
        assert_eq!(link_in_output(out, "issue").unwrap().0, 7);
    }

    #[test]
    fn output_without_an_address_or_number_is_an_error() {
        let err = link_in_output("something went sideways", "issue")
            .unwrap_err()
            .to_string();
        assert!(err.contains("something went sideways"), "{err}");
        assert!(err.contains("which issue"), "{err}");
        assert!(link_in_output("https://github.com/ketapp/ket/issues/new", "issue").is_err());
    }

    #[test]
    fn listed_pull_requests_are_read_and_an_empty_list_is_none() {
        let out = r#"[{"number": 9, "url": "https://github.com/o/r/pull/9"}]"#;
        assert_eq!(
            read_pull_requests(out).unwrap(),
            [PullRequestLink {
                number: 9,
                url: "https://github.com/o/r/pull/9".to_owned(),
            }]
        );
        assert!(read_pull_requests("[]").unwrap().is_empty());
        assert!(read_pull_requests("not json").is_err());
    }

    #[test]
    fn a_push_carries_the_token_only_when_an_account_was_picked() {
        let picked = PushCredentials {
            helper: "!/opt/gh auth git-credential".to_owned(),
            token: Some("ghp_x".to_owned()),
        };
        assert_eq!(picked.env(), [("GH_TOKEN", "ghp_x")]);
        let active = PushCredentials {
            helper: picked.helper.clone(),
            token: None,
        };
        assert!(active.env().is_empty());
    }

    #[test]
    fn a_checkout_runs_the_gh_it_was_handed() {
        let own = Checkout {
            program: Some(PathBuf::from("/tmp/fake-gh")),
            ..Checkout::new("/tmp/repo", None)
        };
        assert_eq!(own.gh().unwrap(), PathBuf::from("/tmp/fake-gh"));
    }

    #[test]
    fn listed_issues_are_tidied_and_a_missing_body_is_empty() {
        let out = r#"[
            {"number": 3, "title": "  Fix the login \n", "body": "Steps:\r\n1. open\r\n\r\n", "url": "https://github.com/o/r/issues/3"},
            {"number": 1, "title": "No body", "url": "https://github.com/o/r/issues/1"}
        ]"#;
        let open = read_issues(out).unwrap();
        assert!(open.complete);
        assert_eq!(open.issues.len(), 2);
        assert_eq!(open.issues[0].title, "Fix the login");
        assert_eq!(open.issues[0].body, "Steps:\n1. open");
        assert_eq!(open.issues[1].body, "");
        assert_eq!(
            open.issues[1].link(),
            IssueLink {
                number: 1,
                url: "https://github.com/o/r/issues/1".to_owned(),
            }
        );
    }

    #[test]
    fn a_list_as_long_as_the_limit_is_not_complete() {
        // The limit may have cut it short, so a missing issue there says
        // nothing about whether it was closed.
        let issues: Vec<String> = (1..=LIST_LIMIT)
            .map(|n| format!(r#"{{"number":{n},"title":"t","body":"","url":"u"}}"#))
            .collect();
        let full = read_issues(&format!("[{}]", issues.join(","))).unwrap();
        assert_eq!(full.issues.len(), LIST_LIMIT);
        assert!(!full.complete);

        let short = read_issues(&format!("[{}]", issues[1..].join(","))).unwrap();
        assert!(short.complete);
    }

    #[test]
    fn an_empty_list_is_complete_and_junk_is_an_error() {
        let open = read_issues("[]").unwrap();
        assert!(open.issues.is_empty() && open.complete);
        assert!(read_issues("gh: not a git repository").is_err());
    }

    #[test]
    fn accounts_are_github_com_ones_only_with_the_active_one_first() {
        let out = r#"{"hosts": {
            "github.com": [
                {"login": "work", "active": false, "state": "success", "tokenSource": "keyring"},
                {"login": "", "active": false},
                {"login": "sam", "active": true, "state": "success", "tokenSource": "keyring"}
            ],
            "ghe.example.com": [
                {"login": "corp", "active": true, "state": "success", "tokenSource": "keyring"}
            ]
        }}"#;
        let logins: Vec<String> = read_accounts(out)
            .unwrap()
            .into_iter()
            .map(|account| account.login)
            .collect();
        assert_eq!(logins, ["sam", "work"]);
    }

    #[test]
    fn no_hosts_is_no_accounts_and_junk_is_an_error() {
        assert!(read_accounts("{}").unwrap().is_empty());
        assert!(read_accounts("not json").is_err());
    }

    #[test]
    fn an_account_whose_token_github_refuses_does_not_work() {
        let out = r#"{"hosts": {"github.com": [
            {"login": "stale", "active": true, "state": "error", "tokenSource": "keyring"},
            {"login": "slow", "active": false, "state": "timeout", "tokenSource": "keyring"}
        ]}}"#;
        let accounts = read_accounts(out).unwrap();
        assert!(!accounts[0].works());
        // One GitHub could not be asked about may still be fine.
        assert!(accounts[1].works());
    }

    #[test]
    fn an_account_from_an_environment_token_cannot_be_picked() {
        let out = r#"{"hosts": {"github.com": [
            {"login": "env", "active": true, "state": "success", "tokenSource": "GH_TOKEN"},
            {"login": "kept", "active": false, "state": "success", "tokenSource": "keyring"}
        ]}}"#;
        let accounts = read_accounts(out).unwrap();
        assert!(!accounts[0].pickable());
        assert!(accounts[1].pickable());
    }

    #[test]
    fn the_sign_in_code_is_read_off_the_clipboard_line() {
        assert_eq!(
            login_code("! One-time code (ABCD-1234) copied to clipboard"),
            Some("ABCD-1234")
        );
    }

    #[test]
    fn the_sign_in_code_is_read_off_the_line_gh_prints_without_a_clipboard() {
        // `gh auth login --web` without `--clipboard`, as ket runs it, says
        // this instead.
        assert_eq!(
            login_code("! First copy your one-time code: ABCD-1234"),
            Some("ABCD-1234")
        );
    }

    #[test]
    fn a_line_with_no_code_in_it_gives_none() {
        assert_eq!(
            login_code(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            ),
            None
        );
        assert_eq!(
            login_code("! Failed to copy one-time code to clipboard"),
            None
        );
    }

    /// `gh pr list --json …,statusCheckRollup` for one pull request, as
    /// ketapp/ket's #17 printed it, trimmed to the fields ket reads.
    const LISTED: &str = r#"[
      {"number": 17, "url": "https://github.com/ketapp/ket/pull/17",
       "headRefName": "deps-expo", "isDraft": false, "isCrossRepository": false,
       "reviewDecision": "",
       "statusCheckRollup": [
         {"__typename": "CheckRun", "name": "rust-advisories-and-licenses",
          "status": "COMPLETED", "conclusion": "FAILURE", "workflowName": "Dependency security",
          "detailsUrl": "https://github.com/ketapp/ket/actions/runs/38802060258/job/116653348974"},
         {"__typename": "CheckRun", "name": "rust", "status": "COMPLETED", "conclusion": "SUCCESS",
          "detailsUrl": "https://github.com/ketapp/ket/actions/runs/38802060369/job/116653349453"},
         {"__typename": "CheckRun", "name": "mobile", "status": "IN_PROGRESS", "conclusion": "",
          "detailsUrl": "https://github.com/ketapp/ket/actions/runs/38802060369/job/116653349181"},
         {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED",
          "detailsUrl": ""},
         {"__typename": "StatusContext", "context": "ci/circleci", "state": "ERROR",
          "targetUrl": "https://circleci.com/gh/o/r/1"},
         {"__typename": "SomethingNew", "name": "ignored"}
       ]},
      {"number": 18, "url": "https://github.com/ketapp/ket/pull/18",
       "headRefName": "fix-typo", "isDraft": true, "isCrossRepository": true,
       "reviewDecision": "APPROVED", "statusCheckRollup": []},
      {"number": 19, "url": "https://github.com/ketapp/ket/pull/19",
       "headRefName": "quiet", "isDraft": true, "isCrossRepository": false,
       "reviewDecision": "CHANGES_REQUESTED", "statusCheckRollup": null}
    ]"#;

    #[test]
    fn open_pull_requests_are_read_with_their_checks_and_forks_left_out() {
        let read = read_pull_request_statuses(LISTED).unwrap();
        assert_eq!(read.len(), 2, "the fork's #18 is not this repository's");

        let deps = &read[0];
        assert_eq!(deps.number, 17);
        assert_eq!(deps.branch, "deps-expo");
        assert!(!deps.draft);
        assert_eq!(deps.review, Review::None);
        let states: Vec<(&str, CheckState)> = deps
            .checks
            .iter()
            .map(|check| (check.name.as_str(), check.state))
            .collect();
        assert_eq!(
            states,
            [
                ("rust-advisories-and-licenses", CheckState::Failed),
                ("rust", CheckState::Passed),
                ("mobile", CheckState::Pending),
                ("docs", CheckState::Skipped),
                ("ci/circleci", CheckState::Failed),
            ]
        );
        assert_eq!(deps.checks[4].url, "https://circleci.com/gh/o/r/1");

        let quiet = &read[1];
        assert!(quiet.draft);
        assert_eq!(quiet.review, Review::ChangesRequested);
        assert!(quiet.checks.is_empty(), "a null rollup is no checks");
        assert_eq!(quiet.checks_state(), None);
        assert_eq!(quiet.checks_summary(), "");
    }

    fn with_checks(states: &[CheckState]) -> PullRequestStatus {
        PullRequestStatus {
            number: 1,
            url: "https://github.com/o/r/pull/1".to_owned(),
            branch: "b".to_owned(),
            draft: false,
            review: Review::None,
            checks: states
                .iter()
                .enumerate()
                .map(|(at, &state)| Check {
                    name: format!("check {at}"),
                    state,
                    url: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn checks_are_failed_before_pending_before_passed_and_skipped_says_nothing() {
        use CheckState::{Failed, Passed, Pending, Skipped};
        assert_eq!(
            with_checks(&[Passed, Pending, Failed]).checks_state(),
            Some(Failed)
        );
        assert_eq!(
            with_checks(&[Passed, Pending]).checks_state(),
            Some(Pending)
        );
        assert_eq!(with_checks(&[Passed, Skipped]).checks_state(), Some(Passed));
        assert_eq!(with_checks(&[Skipped]).checks_state(), None);
    }

    #[test]
    fn checks_are_summed_up_in_a_few_words() {
        use CheckState::{Failed, Passed, Pending, Skipped};
        assert_eq!(
            with_checks(&[Failed, Passed, Pending, Skipped]).checks_summary(),
            "1 of 3 checks failing"
        );
        assert_eq!(
            with_checks(&[Failed, Failed]).checks_summary(),
            "2 checks failing"
        );
        assert_eq!(with_checks(&[Failed]).checks_summary(), "1 check failing");
        assert_eq!(
            with_checks(&[Pending, Passed]).checks_summary(),
            "1 check running"
        );
        assert_eq!(
            with_checks(&[Passed, Passed, Skipped]).checks_summary(),
            "all 2 checks passed"
        );
        assert_eq!(with_checks(&[Passed]).checks_summary(), "1 check passed");
    }

    #[test]
    fn an_actions_job_is_read_off_its_page() {
        assert_eq!(
            job_of("https://github.com/ketapp/ket/actions/runs/38802060258/job/116653348974"),
            Some(116_653_348_974)
        );
        assert_eq!(
            job_of("https://github.com/o/r/actions/runs/1/job/2?pr=17"),
            Some(2)
        );
        assert_eq!(job_of("https://circleci.com/gh/o/r/1"), None);
        assert_eq!(job_of("https://github.com/o/r/actions/runs/1"), None);
        assert_eq!(job_of(""), None);
    }

    #[test]
    fn a_failed_log_loses_its_job_names_and_timestamps_and_gains_step_headings() {
        let out = "lint\tSet up job\t\u{feff}2026-10-15T20:31:18.1053761Z Current runner version: '2.337.0'\n\
                   lint\tRun clippy\t2026-10-15T20:31:19.0000000Z error: unused variable `x`\n\
                   lint\tRun clippy\t2026-10-15T20:31:19.1000000Z\n\
                   lint\tRun clippy\t2026-10-15T20:31:20.0000000Z ##[error]Process completed with exit code 1.\n\
                   a line gh printed without the columns\n";
        assert_eq!(
            readable_log(out),
            "== Set up job ==\n\
             Current runner version: '2.337.0'\n\
             \n\
             == Run clippy ==\n\
             error: unused variable `x`\n\
             \n\
             ##[error]Process completed with exit code 1.\n\
             a line gh printed without the columns\n"
        );
    }

    #[test]
    fn a_log_whose_steps_gh_could_not_tell_apart_has_no_headings() {
        let out = "job\tUNKNOWN STEP\t2026-10-15T20:31:18.1053761Z one\n\
                   job\tUNKNOWN STEP\t2026-10-15T20:31:18.1053762Z two\n";
        assert_eq!(readable_log(out), "one\ntwo\n");
    }
}
