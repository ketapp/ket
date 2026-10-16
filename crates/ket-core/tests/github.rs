//! `ket_core::github`'s issue calls, through a `gh` of the test's own.
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

use ket_core::github::{
    self, Checkout, CloseReason, IssueLink, PullRequestLink, close_issue, create_issue,
    open_issues, reopen_issue, repository,
};

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
