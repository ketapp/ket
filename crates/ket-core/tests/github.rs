//! `ket_core::github`'s issue and check calls, through a `gh` of the test's own.
//!
//! The unit tests in `github.rs` read captured `gh` output; these run the
//! calls themselves against a shell script handed in as
//! [`Checkout::program`], which records its arguments, its stdin and the
//! `GH_TOKEN` it saw, and answers the way `gh` does. `pull_request.rs` covers
//! the pull request calls the same way.
//!
//! The calls that take no checkout — `accounts`, `sign_out`, `install`,
//! `start_login` — always use the real `gh` and are not run here.

use std::fs;
use std::path::{Path, PathBuf};

use ket_core::checks;
use ket_core::github::{
    self, Check, CheckState, Checkout, CloseReason, IssueLink, MergeMethod, PullRequestLink,
    PullRequestStatus, Review, close_issue, create_issue, open_issues, reopen_issue, repository,
};
use ket_core::id::WorktreeId;

mod common;
use common::Sandbox;

/// A scripted `gh`: `answers` is the body of a shell `case "$1 $2"`.
struct FakeGh {
    script: PathBuf,
    log: PathBuf,
    stdin: PathBuf,
}

impl FakeGh {
    fn new(sandbox: &Sandbox, answers: &str) -> Self {
        let gh = Self {
            script: sandbox.path("gh"),
            log: sandbox.path("gh.log"),
            stdin: sandbox.path("gh.stdin"),
        };
        fs::write(
            &gh.script,
            format!(
                "#!/bin/sh\n\
                 printf '%s|%s\\n' \"${{GH_TOKEN:-none}}\" \"$*\" >> \"{log}\"\n\
                 case \"$1 $2\" in\n\
                 \"auth token\") echo \"tok-$6\" ;;\n\
                 {answers}\n\
                 *) echo \"unexpected: $*\" >&2; exit 2 ;;\n\
                 esac\n",
                log = gh.log.display(),
            ),
        )
        .unwrap();
        make_executable(&gh.script);
        gh
    }

    /// Lines of `token|args`, one per call.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn checkout(&self, root: &Path, account: Option<&str>) -> Checkout {
        Checkout {
            program: Some(self.script.clone()),
            ..Checkout::new(root, account.map(str::to_owned))
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[test]
fn an_issue_is_created_with_its_body_on_stdin_and_read_off_the_address() {
    let sandbox = Sandbox::new("gh-issue-create");
    let stdin = sandbox.path("gh.stdin");
    let gh = FakeGh::new(
        &sandbox,
        &format!(
            "\"issue create\") cat > \"{}\"; echo 'Creating issue in o/r'; echo; \
             echo 'https://github.com/o/r/issues/142' ;;",
            stdin.display()
        ),
    );
    let link = create_issue(
        &gh.checkout(sandbox.root(), None),
        "Fix the login",
        "It loops.\n\nEvery time.",
    )
    .unwrap();

    assert_eq!(
        link,
        IssueLink {
            number: 142,
            url: "https://github.com/o/r/issues/142".to_owned(),
        }
    );
    assert_eq!(
        gh.calls(),
        ["none|issue create --title Fix the login --body-file -"]
    );
    assert_eq!(
        fs::read_to_string(&gh.stdin).unwrap(),
        "It loops.\n\nEvery time."
    );
}

#[test]
fn open_issues_are_listed_tidied_and_marked_complete() {
    let sandbox = Sandbox::new("gh-issue-list");
    let gh = FakeGh::new(
        &sandbox,
        // `printf`, not `echo`: sh's echo would expand the `\n` in the JSON.
        r#""issue list") printf '%s\n' '[{"number": 3, "title": " Spaced \n", "body": "a\r\nb", "url": "u3"}, {"number": 1, "title": "No body", "url": "u1"}]' ;;"#,
    );
    let open = open_issues(&gh.checkout(sandbox.root(), None)).unwrap();
    assert!(open.complete);
    let titles: Vec<&str> = open.issues.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Spaced", "No body"]);
    assert_eq!(open.issues[0].body, "a\nb");
    assert_eq!(
        gh.calls(),
        ["none|issue list --state open --limit 500 --json number,title,body,url"]
    );
}

#[test]
fn an_issue_is_closed_with_its_reason_and_reopened() {
    let sandbox = Sandbox::new("gh-issue-close");
    let gh = FakeGh::new(&sandbox, "\"issue close\") ;;\n\"issue reopen\") ;;");
    let at = gh.checkout(sandbox.root(), None);
    close_issue(&at, 7, CloseReason::Completed).unwrap();
    close_issue(&at, 8, CloseReason::NotPlanned).unwrap();
    reopen_issue(&at, 7).unwrap();
    assert_eq!(
        gh.calls(),
        [
            "none|issue close 7 --reason completed",
            "none|issue close 8 --reason not planned",
            "none|issue reopen 7",
        ]
    );
}

#[test]
fn the_repository_is_named_as_gh_resolves_it() {
    let sandbox = Sandbox::new("gh-repo");
    let gh = FakeGh::new(&sandbox, "\"repo view\") echo 'ketapp/ket' ;;");
    assert_eq!(
        repository(&gh.checkout(sandbox.root(), None)).unwrap(),
        "ketapp/ket"
    );
}

#[test]
fn a_picked_account_hands_its_token_to_each_call_and_nothing_else() {
    let sandbox = Sandbox::new("gh-account");
    let gh = FakeGh::new(&sandbox, "\"repo view\") echo 'o/r' ;;");
    repository(&gh.checkout(sandbox.root(), Some("sam"))).unwrap();
    assert_eq!(
        gh.calls(),
        [
            // The lookup itself carries no token.
            "none|auth token --hostname github.com --user sam",
            "tok-sam|repo view --json nameWithOwner --jq .nameWithOwner",
        ]
    );
}

#[test]
fn an_account_that_is_no_longer_signed_in_says_which_one() {
    let sandbox = Sandbox::new("gh-account-gone");
    let script = sandbox.path("gh");
    fs::write(
        &script,
        "#!/bin/sh\necho 'no oauth token found for github.com account sam' >&2\nexit 1\n",
    )
    .unwrap();
    make_executable(&script);
    let at = Checkout {
        program: Some(script),
        ..Checkout::new(sandbox.root(), Some("sam".to_owned()))
    };
    let err = repository(&at).unwrap_err().to_string();
    assert!(err.contains("@sam is no longer signed in"), "{err}");
}

#[test]
fn gh_asking_for_a_login_reads_as_ket_not_being_signed_in() {
    let sandbox = Sandbox::new("gh-signed-out");
    let gh = FakeGh::new(
        &sandbox,
        "\"issue list\") echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4 ;;",
    );
    let err = open_issues(&gh.checkout(sandbox.root(), None))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ket is not signed in to GitHub"), "{err}");
}

#[test]
fn any_other_failure_is_reported_in_gh_s_own_words() {
    let sandbox = Sandbox::new("gh-other-failure");
    let gh = FakeGh::new(
        &sandbox,
        "\"issue close\") echo 'GraphQL: Could not resolve to an issue (closeIssue)' >&2; exit 1 ;;",
    );
    let err = close_issue(
        &gh.checkout(sandbox.root(), None),
        9,
        CloseReason::Completed,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("Could not resolve to an issue"), "{err}");
}

#[test]
fn output_with_no_address_is_an_error_not_a_made_up_issue() {
    let sandbox = Sandbox::new("gh-no-address");
    let gh = FakeGh::new(&sandbox, "\"issue create\") cat >/dev/null; echo 'done' ;;");
    let err = create_issue(&gh.checkout(sandbox.root(), None), "t", "b")
        .unwrap_err()
        .to_string();
    assert!(err.contains("did not say which issue"), "{err}");
}

#[test]
fn a_pull_request_list_with_none_open_is_none() {
    let sandbox = Sandbox::new("gh-pr-none");
    let gh = FakeGh::new(&sandbox, "\"pr list\") echo '[]' ;;");
    assert_eq!(
        github::open_pull_request_for(&gh.checkout(sandbox.root(), None), "b").unwrap(),
        None
    );
}

#[test]
fn a_pull_request_is_opened_as_a_draft_when_asked() {
    let sandbox = Sandbox::new("gh-pr-draft");
    let gh = FakeGh::new(
        &sandbox,
        "\"pr create\") cat >/dev/null; echo 'https://github.com/o/r/pull/12' ;;",
    );
    let link = github::create_pull_request(
        &gh.checkout(sandbox.root(), None),
        "main",
        "fix",
        "Fix",
        "",
        true,
    )
    .unwrap();
    assert_eq!(
        link,
        PullRequestLink {
            number: 12,
            url: "https://github.com/o/r/pull/12".to_owned(),
        }
    );
    assert!(
        gh.calls()[0].ends_with("--body-file - --draft"),
        "{:?}",
        gh.calls()
    );
}

#[test]
fn the_credential_helper_names_this_gh_and_the_picked_accounts_token() {
    let sandbox = Sandbox::new("gh-credential-helper");
    let gh = FakeGh::new(&sandbox, "");
    let picked = github::git_credential_helper(&gh.checkout(sandbox.root(), Some("sam")))
        .unwrap()
        .expect("gh is there");
    assert_eq!(
        picked.helper,
        format!("!{} auth git-credential", gh.script.display())
    );
    assert_eq!(picked.token.as_deref(), Some("tok-sam"));

    let active = github::git_credential_helper(&gh.checkout(sandbox.root(), None))
        .unwrap()
        .unwrap();
    assert!(active.token.is_none());
}

#[test]
fn open_pull_requests_are_listed_with_their_checks_in_one_call() {
    let sandbox = Sandbox::new("gh-pr-checks");
    let json = "number,url,headRefName,baseRefName,isDraft,isCrossRepository,reviewDecision,statusCheckRollup,state,mergeStateStatus,reviews";
    let gh = FakeGh::new(
        &sandbox,
        "\"repo view\") echo '{\"mergeCommitAllowed\":true,\"squashMergeAllowed\":false,\"rebaseMergeAllowed\":true}' ;;\n\
         \"pr list\") if [ \"$4\" = closed ]; then echo '[]'; else echo '[{\"number\": 5, \"url\": \"https://github.com/o/r/pull/5\", \
         \"headRefName\": \"fix\", \"baseRefName\": \"main\", \"isDraft\": false, \
         \"isCrossRepository\": false, \"state\": \"OPEN\", \"mergeStateStatus\": \"BEHIND\", \
         \"reviewDecision\": \"APPROVED\", \"statusCheckRollup\": [{\"__typename\": \"CheckRun\", \
         \"name\": \"rust\", \"status\": \"COMPLETED\", \"conclusion\": \"SUCCESS\", \
         \"detailsUrl\": \"\"}]}]'; fi ;;\n\
         \"api graphql\") printf '%s\\n' '{\"data\":{\"repository\":{\"n5\":{\"reviewThreads\":{\"nodes\":[{\"isResolved\":false,\"comments\":{\"totalCount\":2}}]}}}}}' ;;",
    );
    let open = github::open_pull_requests(&gh.checkout(sandbox.root(), None)).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].branch, "fix");
    assert_eq!(open[0].base, "main");
    assert!(open[0].behind);
    assert_eq!(open[0].review, Review::Approved);
    assert_eq!(open[0].unresolved_comments, 2);
    assert!(open[0].can_address_comments());
    assert_eq!(open[0].checks_state(), Some(CheckState::Passed));
    assert!(open[0].merge_methods.merge);
    assert!(!open[0].merge_methods.squash);
    let calls = gh.calls();
    assert_eq!(
        calls[0],
        "none|repo view --json mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed"
    );
    assert_eq!(
        calls[1],
        format!("none|pr list --state open --limit 100 --json {json}")
    );
    assert_eq!(
        calls[2],
        format!("none|pr list --state closed --limit 100 --json {json}")
    );
    assert!(
        calls[3].contains("none|api graphql -f query=")
            && calls[3].contains("n5:pullRequest(number:5)")
            && calls[3].contains("comments(first:1){totalCount}")
            && calls[3].contains("-F owner=o")
            && calls[3].contains("-F name=r"),
        "{calls:?}"
    );
}

#[test]
fn failed_checks_are_handed_over_with_what_they_printed_saved_beside() {
    let sandbox = Sandbox::new("gh-checks-hand-over");
    let gh = FakeGh::new(
        &sandbox,
        "\"run view\") printf 'lint\\tRun clippy\\t2026-10-15T20:31:19.0000000Z error: unused\\n' ;;",
    );
    let failed = |name: &str, url: &str| Check {
        name: name.to_owned(),
        state: CheckState::Failed,
        url: url.to_owned(),
    };
    let status = PullRequestStatus {
        number: 17,
        url: "https://github.com/o/r/pull/17".to_owned(),
        branch: "fix".to_owned(),
        draft: false,
        review: Review::None,
        state: github::PullRequestState::Open,
        base: "main".to_owned(),
        behind: false,
        unresolved_comments: 0,
        merge_methods: github::MergeMethods::default(),
        checks: vec![
            failed("lint", "https://github.com/o/r/actions/runs/1/job/2"),
            Check {
                state: CheckState::Passed,
                ..failed("rust", "https://github.com/o/r/actions/runs/1/job/3")
            },
            failed("ci/circleci", "https://circleci.com/gh/o/r/1"),
        ],
    };
    let logs = sandbox.path("checks");

    let prompt = checks::hand_over(
        &gh.checkout(sandbox.root(), Some("work")),
        &WorktreeId::new("wt-1"),
        &status,
        &logs,
    )
    .unwrap();

    // Only the failed Actions job's log is asked for, as the project's account.
    let calls = gh.calls();
    assert_eq!(
        calls.last().unwrap(),
        "tok-work|run view --job 2 --log-failed"
    );
    assert!(
        !calls.iter().any(|call| call.contains("--job 3")),
        "{calls:?}"
    );

    let saved: Vec<PathBuf> = fs::read_dir(&logs)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(saved.len(), 1);
    let log = fs::read_to_string(&saved[0]).unwrap();
    assert_eq!(
        log,
        "# lint\nhttps://github.com/o/r/actions/runs/1/job/2\n\n== Run clippy ==\nerror: unused\n"
    );
    assert!(
        prompt.starts_with("The checks on pull request #17 failed: lint, ci/circleci. "),
        "{prompt}"
    );
    assert!(prompt.contains(&saved[0].display().to_string()), "{prompt}");
    assert!(
        prompt.contains("ci/circleci's log is at https://circleci.com/gh/o/r/1."),
        "{prompt}"
    );
    assert!(prompt.ends_with("run `ket pr wt-1` to push the branch so the checks run again."));
}

#[test]
fn a_pull_request_is_merged_updated_readied_and_reopened_with_gh_s_flags() {
    let sandbox = Sandbox::new("gh-pr-actions");
    let gh = FakeGh::new(
        &sandbox,
        "\"pr merge\") ;;\n\
         \"pr update-branch\") ;;\n\
         \"pr ready\") ;;\n\
         \"pr reopen\") ;;",
    );
    let at = gh.checkout(sandbox.root(), None);
    github::merge_pull_request(&at, 17, MergeMethod::Squash).unwrap();
    github::merge_pull_request(&at, 18, MergeMethod::Merge).unwrap();
    github::merge_pull_request(&at, 19, MergeMethod::Rebase).unwrap();
    github::update_branch(&at, 17).unwrap();
    github::mark_ready(&at, 17).unwrap();
    github::reopen_pull_request(&at, 17).unwrap();
    assert_eq!(
        gh.calls(),
        [
            "none|pr merge 17 --squash",
            "none|pr merge 18 --merge",
            "none|pr merge 19 --rebase",
            "none|pr update-branch 17",
            "none|pr ready 17",
            "none|pr reopen 17",
        ]
    );
}

#[test]
fn github_s_refusal_is_kept_when_a_merge_is_blocked() {
    let sandbox = Sandbox::new("gh-pr-merge-refused");
    let gh = FakeGh::new(
        &sandbox,
        "\"pr merge\") echo 'X Pull request #17 is not mergeable: the merge commit cannot be created' >&2; exit 1 ;;",
    );
    let err =
        github::merge_pull_request(&gh.checkout(sandbox.root(), None), 17, MergeMethod::Merge)
            .unwrap_err()
            .to_string();
    assert!(err.contains("the merge commit cannot be created"), "{err}");
}

#[test]
fn review_comments_are_asked_of_graphql_as_the_picked_account() {
    let sandbox = Sandbox::new("gh-pr-comments");
    let gh = FakeGh::new(
        &sandbox,
        "\"repo view\") echo 'ketapp/ket' ;;\n\
         \"api graphql\") printf '%s\\n' '{\"data\":{\"repository\":{\"pullRequest\":{\"reviewThreads\":{\"nodes\":[{\"isResolved\":false,\"comments\":{\"nodes\":[{\"author\":{\"login\":\"sam\"},\"body\":\"rename me\",\"path\":\"a.rs\",\"line\":3}]}}]}}}}}' ;;",
    );
    let comments =
        github::unresolved_review_comments(&gh.checkout(sandbox.root(), Some("work")), 17).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].author, "sam");
    assert_eq!(comments[0].path, "a.rs");
    assert_eq!(comments[0].line, Some(3));
    let calls = gh.calls();
    assert_eq!(
        calls[0],
        "none|auth token --hostname github.com --user work"
    );
    assert!(
        calls[1].starts_with("tok-work|repo view --json nameWithOwner"),
        "{calls:?}"
    );
    let graphql = calls
        .iter()
        .find(|line| line.contains("api graphql"))
        .expect("graphql call");
    assert!(
        graphql.contains("tok-work|api graphql -f query=")
            && graphql.contains("-F owner=ketapp")
            && graphql.contains("-F name=ket")
            && graphql.contains("-F number:int=17"),
        "{calls:?}"
    );
}

#[test]
fn a_closed_pull_request_is_found_by_its_branch() {
    let sandbox = Sandbox::new("gh-pr-for-branch");
    let json = "number,url,headRefName,baseRefName,isDraft,isCrossRepository,reviewDecision,statusCheckRollup,state,mergeStateStatus,reviews";
    let gh = FakeGh::new(
        &sandbox,
        "\"pr list\") echo '[{\"number\": 9, \"url\": \"https://github.com/o/r/pull/9\", \
         \"headRefName\": \"fix\", \"baseRefName\": \"main\", \"isDraft\": false, \
         \"isCrossRepository\": false, \"state\": \"CLOSED\", \"mergeStateStatus\": \"DIRTY\", \
         \"reviewDecision\": \"\", \"statusCheckRollup\": []}]' ;;\n\
         \"api graphql\") printf '%s\\n' '{\"data\":{\"repository\":{\"n9\":{\"reviewThreads\":{\"nodes\":[]}}}}}' ;;",
    );
    let found = github::pull_request_for_branch(&gh.checkout(sandbox.root(), None), "fix").unwrap();
    let found = found.expect("the closed pull request");
    assert_eq!(found.number, 9);
    assert_eq!(found.state, github::PullRequestState::Closed);
    assert!(found.can_reopen());
    let calls = gh.calls();
    assert_eq!(
        calls[0],
        format!("none|pr list --head fix --state all --limit 1 --json {json}")
    );
    assert!(
        calls[1].contains("none|api graphql") && calls[1].contains("n9:pullRequest(number:9)"),
        "{calls:?}"
    );
}

#[test]
fn a_pull_request_with_nothing_failed_has_nothing_to_hand_over() {
    let sandbox = Sandbox::new("gh-checks-none-failed");
    let gh = FakeGh::new(&sandbox, "");
    let status = PullRequestStatus {
        number: 3,
        url: String::new(),
        branch: "b".to_owned(),
        draft: false,
        review: Review::None,
        state: github::PullRequestState::Open,
        base: String::new(),
        behind: false,
        unresolved_comments: 0,
        merge_methods: github::MergeMethods::default(),
        checks: Vec::new(),
    };
    let err = checks::hand_over(
        &gh.checkout(sandbox.root(), None),
        &WorktreeId::new("wt"),
        &status,
        &sandbox.path("checks"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("#3 has no failed checks"), "{err}");
    assert!(gh.calls().is_empty());
}
