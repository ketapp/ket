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
    /// Open, closed unmerged, or merged.
    #[serde(default)]
    pub state: PullRequestState,
    /// The branch it asks to merge into.
    #[serde(default)]
    pub base: String,
    /// Whether its branch is behind that base, as GitHub last said.
    #[serde(default)]
    pub behind: bool,
    /// Unresolved review-thread comments, as last counted. Line comments
    /// included; a review body with no threads is not. Zero when GitHub did
    /// not say.
    #[serde(default)]
    pub unresolved_comments: u32,
    /// Merge methods the repository allows. All of them until ket has asked.
    #[serde(default)]
    pub merge_methods: MergeMethods,
}

/// Whether a pull request is still open, was closed, or has landed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    /// Open, draft or ready.
    #[default]
    Open,
    /// Closed without merging.
    Closed,
    /// Merged into its base.
    Merged,
}

/// How GitHub will land a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    /// A merge commit.
    Merge,
    /// Squash into one commit.
    Squash,
    /// Rebase onto the base.
    Rebase,
}

impl MergeMethod {
    /// The `gh pr merge` flag for this method.
    pub fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }

    /// The word the dialog and the phone use: "merge", "squash", "rebase".
    pub fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }

    /// A short label for a picker: "Merge commit", "Squash", "Rebase".
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "Merge commit",
            Self::Squash => "Squash",
            Self::Rebase => "Rebase",
        }
    }
}

/// Which merge methods a repository allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeMethods {
    /// A merge commit.
    pub merge: bool,
    /// Squash into one commit.
    pub squash: bool,
    /// Rebase onto the base.
    pub rebase: bool,
}

impl Default for MergeMethods {
    fn default() -> Self {
        Self {
            merge: true,
            squash: true,
            rebase: true,
        }
    }
}

impl MergeMethods {
    /// The methods GitHub will accept, in the order a picker shows them.
    pub fn allowed(self) -> Vec<MergeMethod> {
        let mut methods = Vec::new();
        if self.merge {
            methods.push(MergeMethod::Merge);
        }
        if self.squash {
            methods.push(MergeMethod::Squash);
        }
        if self.rebase {
            methods.push(MergeMethod::Rebase);
        }
        methods
    }

    /// The repository's default: a merge commit when that is allowed, else
    /// squash, else rebase.
    pub fn default_method(self) -> Option<MergeMethod> {
        self.allowed().into_iter().next()
    }

    /// Whether `method` is allowed.
    pub fn allows(self, method: MergeMethod) -> bool {
        match method {
            MergeMethod::Merge => self.merge,
            MergeMethod::Squash => self.squash,
            MergeMethod::Rebase => self.rebase,
        }
    }
}

/// One unresolved review comment, as Address comments hands it to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComment {
    /// Who wrote it, a GitHub login. Empty when GitHub did not say.
    pub author: String,
    /// The file it is on. Empty for a review body with no line.
    pub path: String,
    /// The line in the new file, when GitHub named one.
    pub line: Option<u32>,
    /// What they wrote.
    pub body: String,
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

    /// Whether it can be merged from ket: open and not a draft.
    pub fn can_merge(&self) -> bool {
        self.state == PullRequestState::Open && !self.draft
    }

    /// Whether it can be marked ready: an open draft.
    pub fn can_ready(&self) -> bool {
        self.state == PullRequestState::Open && self.draft
    }

    /// Whether it can be reopened: closed without merging.
    pub fn can_reopen(&self) -> bool {
        self.state == PullRequestState::Closed
    }

    /// Whether its branch can be updated from the base: open and behind.
    pub fn can_update(&self) -> bool {
        self.state == PullRequestState::Open && self.behind
    }

    /// Whether Address comments applies: open, and there are review comments
    /// or a reviewer asked for changes.
    pub fn can_address_comments(&self) -> bool {
        self.state == PullRequestState::Open
            && (self.unresolved_comments > 0 || self.review == Review::ChangesRequested)
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

/// The pull request for `branch` to show: an open one over a closed or
/// merged one, then the highest number.
pub fn best_for_branch<'a>(
    prs: &'a [PullRequestStatus],
    branch: &str,
) -> Option<&'a PullRequestStatus> {
    prs.iter()
        .filter(|pr| pr.branch == branch)
        .max_by_key(|pr| (matches!(pr.state, PullRequestState::Open) as u8, pr.number))
}

/// JSON fields every pull-request list and view asks for.
const STATUS_JSON: &str = "number,url,headRefName,baseRefName,isDraft,isCrossRepository,reviewDecision,statusCheckRollup,state,mergeStateStatus,reviews";

/// The pull requests of the repository `at` is a checkout of, with their
/// checks — open ones, and recently closed or merged ones, so a worktree
/// whose pull request was closed still shows it. One call for every
/// worktree of a project, matched to them by branch. Opened by ket or by
/// an agent that ran `gh pr create` itself.
///
/// Pull requests from forks are left out: a fork's branch names are
/// someone else's, and a fork's `fix-typo` is not this repository's.
pub fn open_pull_requests(at: &Checkout) -> Result<Vec<PullRequestStatus>> {
    let methods = merge_methods(at).ok();
    let mut prs = list_pull_requests(at, "open")?;
    let closed = list_pull_requests(at, "closed")?;
    prs.extend(closed);
    if let Some(methods) = methods {
        for pr in &mut prs {
            pr.merge_methods = methods;
        }
    }
    fill_unresolved_comments(at, &mut prs);
    Ok(prs)
}

/// The pull request whose head is `head`, in any state — so a closed one
/// is still found. `None` when there is none.
pub fn pull_request_for_branch(at: &Checkout, head: &str) -> Result<Option<PullRequestStatus>> {
    let out = run(
        at,
        &[
            "pr",
            "list",
            "--head",
            head,
            "--state",
            "all",
            "--limit",
            "1",
            "--json",
            STATUS_JSON,
        ],
        None,
    )?;
    let mut prs = read_pull_request_statuses(&out)?;
    fill_unresolved_comments(at, &mut prs);
    Ok(prs.into_iter().next())
}

/// Pull request `number`, as GitHub has it now.
pub fn pull_request(at: &Checkout, number: u64) -> Result<PullRequestStatus> {
    let number = number.to_string();
    let out = run(at, &["pr", "view", &number, "--json", STATUS_JSON], None)?;
    let listed = format!("[{}]", out.trim());
    let mut prs = read_pull_request_statuses(&listed)?;
    fill_unresolved_comments(at, &mut prs);
    prs.into_iter()
        .next()
        .ok_or_else(|| failed(format!("gh did not describe pull request {number}")))
}

fn list_pull_requests(at: &Checkout, state: &str) -> Result<Vec<PullRequestStatus>> {
    let limit = PULL_REQUEST_LIMIT.to_string();
    let out = run(
        at,
        &[
            "pr",
            "list",
            "--state",
            state,
            "--limit",
            &limit,
            "--json",
            STATUS_JSON,
        ],
        None,
    )?;
    read_pull_request_statuses(&out)
}

/// Overwrites [`PullRequestStatus::unresolved_comments`] with unresolved
/// review-thread comments, line comments included. `gh pr list`'s `reviews`
/// field is bodies on submitted reviews, so a reviewer who only commented
/// on lines would otherwise look like they said nothing. A GraphQL miss
/// leaves the list's fallback count.
fn fill_unresolved_comments(at: &Checkout, prs: &mut [PullRequestStatus]) {
    if prs.is_empty() {
        return;
    }
    let Some((owner, name)) = prs
        .iter()
        .find_map(|pr| owner_and_name_from_pull_url(&pr.url))
    else {
        return;
    };
    let numbers: Vec<u64> = prs.iter().map(|pr| pr.number).collect();
    let Ok(counts) = fetch_unresolved_comment_counts(at, &owner, &name, &numbers) else {
        return;
    };
    for pr in prs {
        if let Some(&count) = counts.get(&pr.number) {
            pr.unresolved_comments = count;
        }
    }
}

/// `owner` and `name` from a pull request page, `https://host/owner/name/pull/17`.
fn owner_and_name_from_pull_url(url: &str) -> Option<(String, String)> {
    let path = url.split_once("://")?.1;
    let mut parts = path.split('/');
    let _host = parts.next()?;
    let owner = parts.next()?.to_owned();
    let name = parts.next()?.to_owned();
    if parts.next() != Some("pull") || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

fn fetch_unresolved_comment_counts(
    at: &Checkout,
    owner: &str,
    name: &str,
    numbers: &[u64],
) -> Result<std::collections::HashMap<u64, u32>> {
    if numbers.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut query = String::from(
        "query=query($owner:String!,$name:String!){repository(owner:$owner,name:$name){",
    );
    for number in numbers {
        query.push('n');
        query.push_str(&number.to_string());
        query.push_str(":pullRequest(number:");
        query.push_str(&number.to_string());
        query.push_str(
            "){reviewThreads(first:100){nodes{isResolved comments(first:1){totalCount}}}}",
        );
    }
    query.push_str("}}");
    let owner_arg = format!("owner={owner}");
    let name_arg = format!("name={name}");
    let out = run(
        at,
        &[
            "api", "graphql", "-f", &query, "-F", &owner_arg, "-F", &name_arg,
        ],
        None,
    )?;
    read_unresolved_comment_counts(&out)
}

fn read_unresolved_comment_counts(out: &str) -> Result<std::collections::HashMap<u64, u32>> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        data: Option<Data>,
        #[serde(default)]
        errors: Option<Vec<GraphErr>>,
    }
    #[derive(Deserialize)]
    struct GraphErr {
        #[serde(default)]
        message: String,
    }
    #[derive(Deserialize)]
    struct Data {
        #[serde(default)]
        repository: std::collections::HashMap<String, Option<Pr>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pr {
        review_threads: Option<Threads>,
    }
    #[derive(Deserialize)]
    struct Threads {
        #[serde(default)]
        nodes: Vec<Thread>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Thread {
        #[serde(default)]
        is_resolved: bool,
        comments: Option<Count>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Count {
        #[serde(default)]
        total_count: u32,
    }

    let parsed: Envelope = serde_json::from_str(out).map_err(|e| {
        failed(format!(
            "could not read the pull request's review comments: {e}"
        ))
    })?;
    let Some(data) = parsed.data else {
        if let Some(errors) = parsed.errors.filter(|errors| !errors.is_empty()) {
            let why = errors
                .iter()
                .map(|error| error.message.as_str())
                .filter(|message| !message.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(failed(if why.is_empty() {
                "GitHub refused to read the pull request's review comments".to_owned()
            } else {
                why
            }));
        }
        return Ok(std::collections::HashMap::new());
    };
    let mut counts = std::collections::HashMap::new();
    for (alias, pr) in data.repository {
        let Some(number) = alias
            .strip_prefix('n')
            .and_then(|rest| rest.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(pr) = pr else {
            continue;
        };
        let threads = pr
            .review_threads
            .map(|threads| threads.nodes)
            .unwrap_or_default();
        let count = threads
            .iter()
            .filter(|thread| !thread.is_resolved)
            .map(
                |thread| match thread.comments.as_ref().map(|c| c.total_count) {
                    Some(0) | None => 1,
                    Some(n) => n,
                },
            )
            .sum();
        counts.insert(number, count);
    }
    Ok(counts)
}

/// Merge methods the repository `at` is a checkout of allows.
pub fn merge_methods(at: &Checkout) -> Result<MergeMethods> {
    let out = run(
        at,
        &[
            "repo",
            "view",
            "--json",
            "mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed",
        ],
        None,
    )?;
    read_merge_methods(&out)
}

fn read_merge_methods(out: &str) -> Result<MergeMethods> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Allowed {
        #[serde(default)]
        merge_commit_allowed: bool,
        #[serde(default)]
        squash_merge_allowed: bool,
        #[serde(default)]
        rebase_merge_allowed: bool,
    }
    let allowed: Allowed = serde_json::from_str(out).map_err(|e| {
        failed(format!(
            "could not read the repository's merge methods: {e}"
        ))
    })?;
    let methods = MergeMethods {
        merge: allowed.merge_commit_allowed,
        squash: allowed.squash_merge_allowed,
        rebase: allowed.rebase_merge_allowed,
    };
    if methods.allowed().is_empty() {
        return Err(failed("this repository allows no merge methods".to_owned()));
    }
    Ok(methods)
}

/// Merges pull request `number` with `method`. Does not delete the branch.
pub fn merge_pull_request(at: &Checkout, number: u64, method: MergeMethod) -> Result<()> {
    run(
        at,
        &["pr", "merge", &number.to_string(), method.flag()],
        None,
    )
    .map(|_| ())
}

/// Updates pull request `number`'s branch from its base.
pub fn update_branch(at: &Checkout, number: u64) -> Result<()> {
    run(at, &["pr", "update-branch", &number.to_string()], None).map(|_| ())
}

/// Marks pull request `number` ready for review.
pub fn mark_ready(at: &Checkout, number: u64) -> Result<()> {
    run(at, &["pr", "ready", &number.to_string()], None).map(|_| ())
}

/// Reopens pull request `number`.
pub fn reopen_pull_request(at: &Checkout, number: u64) -> Result<()> {
    run(at, &["pr", "reopen", &number.to_string()], None).map(|_| ())
}

/// Unresolved review comments on pull request `number`, line comments
/// included, with who wrote each.
pub fn unresolved_review_comments(at: &Checkout, number: u64) -> Result<Vec<ReviewComment>> {
    let repo = repository(at)?;
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(failed(format!("repository {repo} has no owner/name")));
    };
    let number = number.to_string();
    let out = run(
        at,
        &[
            "api",
            "graphql",
            "-f",
            REVIEW_THREADS_QUERY,
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("name={name}"),
            "-F",
            &format!("number:int={number}"),
        ],
        None,
    )?;
    read_review_threads(&out)
}

/// `gh api graphql -f` value: the query document, named `query`.
const REVIEW_THREADS_QUERY: &str = "query=query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{isResolved comments(first:30){nodes{author{login} body path line originalLine}}}}}}}";

fn read_review_threads(out: &str) -> Result<Vec<ReviewComment>> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        data: Option<Data>,
        #[serde(default)]
        errors: Option<Vec<GraphErr>>,
    }
    #[derive(Deserialize)]
    struct GraphErr {
        #[serde(default)]
        message: String,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repo>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Option<Pr>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pr {
        review_threads: Option<Threads>,
    }
    #[derive(Deserialize)]
    struct Threads {
        #[serde(default)]
        nodes: Vec<Thread>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Thread {
        #[serde(default)]
        is_resolved: bool,
        comments: Option<Comments>,
    }
    #[derive(Deserialize)]
    struct Comments {
        #[serde(default)]
        nodes: Vec<Node>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        author: Option<Author>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        path: String,
        line: Option<u32>,
        original_line: Option<u32>,
    }
    #[derive(Deserialize)]
    struct Author {
        #[serde(default)]
        login: String,
    }

    let parsed: Envelope = serde_json::from_str(out).map_err(|e| {
        failed(format!(
            "could not read the pull request's review comments: {e}"
        ))
    })?;
    if let Some(errors) = parsed.errors.filter(|errors| !errors.is_empty()) {
        let why = errors
            .iter()
            .map(|error| error.message.as_str())
            .filter(|message| !message.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(failed(if why.is_empty() {
            "GitHub refused to read the pull request's review comments".to_owned()
        } else {
            why
        }));
    }
    let threads = parsed
        .data
        .and_then(|data| data.repository)
        .and_then(|repo| repo.pull_request)
        .and_then(|pr| pr.review_threads)
        .map(|threads| threads.nodes)
        .unwrap_or_default();
    Ok(threads
        .into_iter()
        .filter(|thread| !thread.is_resolved)
        .flat_map(|thread| {
            thread
                .comments
                .unwrap_or(Comments { nodes: Vec::new() })
                .nodes
        })
        .filter(|node| !node.body.trim().is_empty())
        .map(|node| ReviewComment {
            author: node.author.map(|author| author.login).unwrap_or_default(),
            path: node.path,
            line: node.line.or(node.original_line),
            body: node.body.trim().to_owned(),
        })
        .collect())
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
        base_ref_name: String,
        #[serde(default)]
        is_draft: bool,
        #[serde(default)]
        is_cross_repository: bool,
        #[serde(default)]
        review_decision: Option<String>,
        #[serde(default)]
        status_check_rollup: Option<Vec<Rolled>>,
        #[serde(default)]
        state: String,
        #[serde(default)]
        merge_state_status: String,
        #[serde(default)]
        reviews: Vec<ListedReview>,
    }

    #[derive(Deserialize)]
    struct ListedReview {
        #[serde(default)]
        body: String,
        #[serde(default)]
        state: String,
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
            state: match pr.state.as_str() {
                "MERGED" => PullRequestState::Merged,
                "CLOSED" => PullRequestState::Closed,
                _ => PullRequestState::Open,
            },
            base: pr.base_ref_name,
            behind: pr.merge_state_status.eq_ignore_ascii_case("BEHIND"),
            unresolved_comments: pr
                .reviews
                .iter()
                .filter(|review| {
                    !review.body.trim().is_empty() || review.state.eq_ignore_ascii_case("COMMENTED")
                })
                .count() as u32,
            merge_methods: MergeMethods::default(),
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
    )
    .map_err(|e| match e {
        KetError::Conflict(why) => match unseen_repository(&why) {
            Some(name) => failed(format!(
                "{} can't find `{name}` on GitHub. If it's private, pick an account that \
                 can see it.",
                at.account.as_deref().map_or_else(
                    || "The default account".to_owned(),
                    |login| format!("@{login}")
                )
            )),
            None => failed(why),
        },
        e => e,
    })?;
    Ok(name.trim().to_owned())
}

/// The `owner/name` in `gh`'s answer when the account cannot see that
/// repository — private to people it is not one of, or not there at all,
/// which GitHub does not tell apart.
fn unseen_repository(why: &str) -> Option<&str> {
    let rest = why.strip_prefix("GraphQL: Could not resolve to a Repository with the name '")?;
    rest.split('\'').next().filter(|name| !name.is_empty())
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
        // Ahead of the sign-in arm: with remotes on another host, `gh` says
        // to run `gh auth login` too, meaning to add that host.
        KetError::Conflict(why)
            if why.contains("no git remotes found") || why.contains("none of the git remotes") =>
        {
            failed(
                "This project has no remote on GitHub, so there is no repository to use."
                    .to_owned(),
            )
        }
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
mod tests;
