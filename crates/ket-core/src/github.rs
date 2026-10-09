//! GitHub issues, through the `gh` command line.
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
}

/// An issue a note is linked to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLink {
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
    issue_link(&out)
}

/// The issue `gh issue create` says it made, from what it printed.
fn issue_link(out: &str) -> Result<IssueLink> {
    // `gh issue create` prints the new issue's address and nothing else.
    let url = out
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("https://"))
        .ok_or_else(|| {
            failed(format!(
                "gh did not say which issue it made: {}",
                out.trim()
            ))
        })?;
    let number = url
        .rsplit('/')
        .next()
        .and_then(|tail| tail.parse().ok())
        .ok_or_else(|| failed(format!("gh made an issue at {url}, with no number in it")))?;
    Ok(IssueLink {
        number,
        url: url.to_owned(),
    })
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
    let gh = gh()?;
    let token = match &at.account {
        Some(login) => Some(
            run_program(
                &gh,
                &["auth", "token", "--hostname", "github.com", "--user", login],
                &at.root,
                None,
                TIMEOUT,
                &[],
            )
            .map_err(|_| {
                failed(format!(
                    "@{login} is no longer signed in to GitHub on this Mac. Sign in again in \
                     Settings \u{2192} GitHub, or pick another account in the project's settings."
                ))
            })?
            .trim()
            .to_owned(),
        ),
        None => None,
    };
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
        let link = issue_link("https://github.com/ketapp/ket/issues/142\n").unwrap();
        assert_eq!(
            link,
            IssueLink {
                number: 142,
                url: "https://github.com/ketapp/ket/issues/142".to_owned(),
            }
        );
    }

    #[test]
    fn the_address_is_the_last_one_gh_prints_after_any_chatter() {
        // `gh` may say where it is creating the issue before the address.
        let out = "\nCreating issue in ketapp/ket\n\n  https://github.com/ketapp/ket/issues/7  \n";
        assert_eq!(issue_link(out).unwrap().number, 7);
    }

    #[test]
    fn output_without_an_address_or_number_is_an_error() {
        let err = issue_link("something went sideways")
            .unwrap_err()
            .to_string();
        assert!(err.contains("something went sideways"), "{err}");
        assert!(issue_link("https://github.com/ketapp/ket/issues/new").is_err());
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
}
