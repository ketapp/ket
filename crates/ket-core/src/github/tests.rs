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
       "headRefName": "deps-expo", "baseRefName": "main", "isDraft": false,
       "isCrossRepository": false, "state": "OPEN", "mergeStateStatus": "BEHIND",
       "reviewDecision": "",
       "reviews": [{"body": "please rename this"}],
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
       "headRefName": "fix-typo", "baseRefName": "main", "isDraft": true,
       "isCrossRepository": true, "state": "OPEN", "mergeStateStatus": "CLEAN",
       "reviewDecision": "APPROVED", "statusCheckRollup": []},
      {"number": 19, "url": "https://github.com/ketapp/ket/pull/19",
       "headRefName": "quiet", "baseRefName": "main", "isDraft": true,
       "isCrossRepository": false, "state": "CLOSED", "mergeStateStatus": "UNKNOWN",
       "reviewDecision": "CHANGES_REQUESTED", "statusCheckRollup": null}
    ]"#;

#[test]
fn open_pull_requests_are_read_with_their_checks_and_forks_left_out() {
    let read = read_pull_request_statuses(LISTED).unwrap();
    assert_eq!(read.len(), 2, "the fork's #18 is not this repository's");

    let deps = &read[0];
    assert_eq!(deps.number, 17);
    assert_eq!(deps.branch, "deps-expo");
    assert_eq!(deps.base, "main");
    assert!(!deps.draft);
    assert!(deps.behind);
    assert_eq!(deps.state, PullRequestState::Open);
    assert_eq!(deps.unresolved_comments, 1);
    assert_eq!(deps.review, Review::None);
    assert!(deps.can_address_comments());
    let inline_only = read_pull_request_statuses(
        r#"[{"number": 4, "url": "https://github.com/o/r/pull/4",
                "headRefName": "fix", "baseRefName": "main", "isDraft": false,
                "isCrossRepository": false, "state": "OPEN",
                "reviewDecision": "", "reviews": [{"body": "", "state": "COMMENTED"}],
                "statusCheckRollup": []}]"#,
    )
    .unwrap();
    assert_eq!(inline_only[0].unresolved_comments, 1);
    assert_eq!(inline_only[0].review, Review::None);
    assert!(inline_only[0].can_address_comments());
    let approved_silent = read_pull_request_statuses(
        r#"[{"number": 5, "url": "https://github.com/o/r/pull/5",
                "headRefName": "fix", "baseRefName": "main", "isDraft": false,
                "isCrossRepository": false, "state": "OPEN",
                "reviewDecision": "APPROVED", "reviews": [{"body": "", "state": "APPROVED"}],
                "statusCheckRollup": []}]"#,
    )
    .unwrap();
    assert_eq!(approved_silent[0].unresolved_comments, 0);
    assert!(!approved_silent[0].can_address_comments());
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
    assert_eq!(quiet.state, PullRequestState::Closed);
    assert!(quiet.can_reopen());
    assert!(!quiet.can_merge());
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
        state: PullRequestState::Open,
        base: "main".to_owned(),
        behind: false,
        unresolved_comments: 0,
        merge_methods: MergeMethods::default(),
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

#[test]
fn merge_methods_are_read_and_the_default_is_the_first_allowed() {
    let methods = read_merge_methods(
        r#"{"mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":true}"#,
    )
    .unwrap();
    assert_eq!(
        methods.allowed(),
        [MergeMethod::Squash, MergeMethod::Rebase]
    );
    assert_eq!(methods.default_method(), Some(MergeMethod::Squash));
    assert!(methods.allows(MergeMethod::Squash));
    assert!(!methods.allows(MergeMethod::Merge));
    assert_eq!(MergeMethod::Squash.flag(), "--squash");
    assert!(
        read_merge_methods(
            r#"{"mergeCommitAllowed":false,"squashMergeAllowed":false,"rebaseMergeAllowed":false}"#
        )
        .is_err()
    );
    assert!(read_merge_methods("not json").is_err());
}

#[test]
fn the_open_pull_request_for_a_branch_beats_a_closed_one() {
    let closed = PullRequestStatus {
        number: 20,
        url: String::new(),
        branch: "fix".to_owned(),
        draft: false,
        review: Review::None,
        checks: Vec::new(),
        state: PullRequestState::Closed,
        base: "main".to_owned(),
        behind: false,
        unresolved_comments: 0,
        merge_methods: MergeMethods::default(),
    };
    let older_open = PullRequestStatus {
        number: 9,
        state: PullRequestState::Open,
        ..closed.clone()
    };
    let open = PullRequestStatus {
        number: 12,
        state: PullRequestState::Open,
        ..closed.clone()
    };
    let other = PullRequestStatus {
        number: 99,
        branch: "other".to_owned(),
        state: PullRequestState::Open,
        ..closed.clone()
    };
    let listed = [closed.clone(), older_open, other, open.clone()];
    let found = best_for_branch(&listed, "fix").expect("the open one");
    assert_eq!(found.number, 12);
    assert_eq!(
        best_for_branch(std::slice::from_ref(&closed), "fix")
            .expect("the closed one")
            .number,
        20
    );
    assert!(best_for_branch(&listed, "missing").is_none());
}

#[test]
fn unresolved_review_threads_are_read_with_author_file_and_line() {
    let out = r#"{
          "data": {
            "repository": {
              "pullRequest": {
                "reviewThreads": {
                  "nodes": [
                    {"isResolved": true, "comments": {"nodes": [
                      {"author": {"login": "done"}, "body": "fixed", "path": "a.rs", "line": 1}
                    ]}},
                    {"isResolved": false, "comments": {"nodes": [
                      {"author": {"login": "alice"}, "body": "please rename\n", "path": "src/lib.rs", "line": 42, "originalLine": 40},
                      {"author": {"login": "bob"}, "body": "agreed", "path": "src/lib.rs", "line": 42}
                    ]}},
                    {"isResolved": false, "comments": {"nodes": [
                      {"author": {"login": "carol"}, "body": "  ", "path": "skip.rs", "line": 3}
                    ]}}
                  ]
                }
              }
            }
          }
        }"#;
    let comments = read_review_threads(out).unwrap();
    assert_eq!(
        comments,
        [
            ReviewComment {
                author: "alice".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: Some(42),
                body: "please rename".to_owned(),
            },
            ReviewComment {
                author: "bob".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: Some(42),
                body: "agreed".to_owned(),
            },
        ]
    );
}

#[test]
fn github_s_own_words_are_kept_when_it_refuses_the_threads() {
    let err = read_review_threads(
        r#"{"data":null,"errors":[{"message":"Resource not accessible by integration"}]}"#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("Resource not accessible by integration"),
        "{err}"
    );
}

#[test]
fn unresolved_thread_counts_are_read_per_pull_request() {
    let counts = read_unresolved_comment_counts(
        r#"{
              "data": {
                "repository": {
                  "n17": {"reviewThreads": {"nodes": [
                    {"isResolved": true, "comments": {"totalCount": 2}},
                    {"isResolved": false, "comments": {"totalCount": 3}},
                    {"isResolved": false, "comments": {"totalCount": 1}}
                  ]}},
                  "n18": {"reviewThreads": {"nodes": []}},
                  "n19": null
                }
              }
            }"#,
    )
    .unwrap();
    assert_eq!(counts.get(&17), Some(&4));
    assert_eq!(counts.get(&18), Some(&0));
    assert_eq!(counts.get(&19), None);
    assert_eq!(
        owner_and_name_from_pull_url("https://github.com/ketapp/ket/pull/17"),
        Some(("ketapp".to_owned(), "ket".to_owned()))
    );
}
