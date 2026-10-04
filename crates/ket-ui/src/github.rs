//! The GitHub accounts ket can use, shared by Settings → GitHub, where they
//! are added and removed, and a project's settings, where one is picked.
//!
//! The accounts are `gh`'s (see `ket_core::github`), and they belong to the
//! Mac rather than to any project: an account is signed in to once, and every
//! project can then use it. Nobody types a `gh` command. Install fetches `gh`
//! with Homebrew, and adding an account runs its browser sign-in, showing the
//! one-time code here and opening the page it is entered on.

use gpui::{AnyElement, ClipboardItem, Context, SharedString, WeakEntity, div, prelude::*, px};
use ket_core::github::{self, Account};

use crate::Shell;
use crate::ui::button::button;
use crate::ui::chip::{caption, readout};
use crate::ui::icon::Icon;

/// What ket knows of the Mac's GitHub accounts, and the step under way.
#[derive(Default)]
pub(crate) struct GitHubState {
    pub(crate) accounts: Accounts,
    pub(crate) step: Step,
}

/// The accounts `gh` is signed in to.
#[derive(Default)]
pub(crate) enum Accounts {
    /// Not asked yet.
    #[default]
    Unread,
    /// Being asked, with nothing known before.
    Checking,
    /// `gh` is not installed.
    Missing,
    /// What `gh` said, the active account first. Possibly none.
    Known(Vec<Account>),
    /// `gh` could not be asked, and why.
    Failed(String),
}

/// Something ket is doing with GitHub for the person.
#[derive(Default)]
pub(crate) enum Step {
    #[default]
    Idle,
    /// Homebrew is installing `gh`.
    Installing,
    /// `gh` is being asked for a code to sign in with.
    Starting,
    /// Waiting for `code` to be entered at `url`.
    Waiting { code: SharedString, url: String },
    /// `login` is being signed out.
    SigningOut(String),
    /// The last step failed, and why.
    Failed(String),
}

impl GitHubState {
    /// The accounts a project can pick: working, and held by `gh` itself.
    pub(crate) fn pickable(&self) -> Vec<&Account> {
        match &self.accounts {
            Accounts::Known(accounts) => accounts
                .iter()
                .filter(|account| account.works() && account.pickable())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The account `gh` uses when a project picks none.
    pub(crate) fn active(&self) -> Option<&Account> {
        match &self.accounts {
            Accounts::Known(accounts) => accounts.iter().find(|account| account.active),
            _ => None,
        }
    }

    /// Whether some account is signed in and working.
    pub(crate) fn connected(&self) -> bool {
        !self.pickable().is_empty()
    }
}

impl Shell {
    /// Asks `gh` which accounts it is signed in to, then has an open
    /// project's settings check its repository again.
    pub(crate) fn read_github_accounts(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.github.accounts, Accounts::Known(_)) {
            self.github.accounts = Accounts::Checking;
        }
        let read = cx.background_executor().spawn(async { github::accounts() });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let read = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.github.accounts = match read {
                    Ok(Some(accounts)) => Accounts::Known(accounts),
                    Ok(None) => Accounts::Missing,
                    Err(e) => Accounts::Failed(e.to_string()),
                };
                shell.read_project_repository(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Installs `gh` with Homebrew, then asks again.
    pub(crate) fn install_gh(&mut self, cx: &mut Context<Self>) {
        self.github.step = Step::Installing;
        let install = cx.background_executor().spawn(async { github::install() });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let installed = install.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.github.step = match installed {
                    Ok(()) => Step::Idle,
                    Err(e) => Step::Failed(e.to_string()),
                };
                shell.read_github_accounts(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Signs another account in, in the browser: shows the code `gh` gives,
    /// copies it, opens the page it is entered on, and asks again once it
    /// has been. The account that was active stays active.
    pub(crate) fn add_github_account(&mut self, cx: &mut Context<Self>) {
        self.github.step = Step::Starting;
        let start = cx
            .background_executor()
            .spawn(async { github::start_login() });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let login = match start.await {
                Ok(login) => login,
                Err(e) => {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.github.step = Step::Failed(e.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let (code, url) = (login.code.clone(), login.url.clone());
            let _ = shell.update(cx, |shell, cx| {
                shell.github.step = Step::Waiting {
                    code: code.clone().into(),
                    url: url.clone(),
                };
                cx.write_to_clipboard(ClipboardItem::new_string(code));
                cx.open_url(&url);
                cx.notify();
            });
            let signed_in = cx
                .background_executor()
                .spawn(async move { login.wait() })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.github.step = match signed_in {
                    Ok(()) => Step::Idle,
                    Err(e) => Step::Failed(e.to_string()),
                };
                shell.read_github_accounts(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Signs `gh` out of `login`.
    pub(crate) fn sign_out_github(&mut self, login: String, cx: &mut Context<Self>) {
        self.github.step = Step::SigningOut(login.clone());
        let out = cx
            .background_executor()
            .spawn(async move { github::sign_out(&login) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let out = out.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.github.step = match out {
                    Ok(()) => Step::Idle,
                    Err(e) => Step::Failed(e.to_string()),
                };
                shell.read_github_accounts(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The line that gets ket to GitHub from wherever it stands: install
    /// `gh`, sign in, or the code to enter. With an account already signed
    /// in and nothing under way, the button that adds another.
    pub(crate) fn github_setup(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let line = div().flex().flex_wrap().items_center().gap(px(8.0));
        let sign_in = |label: &'static str, primary: bool| {
            let button = button((id, 1usize), label).small().loading_if(
                matches!(self.github.step, Step::Starting),
                "Asking GitHub for a code\u{2026}",
            );
            let button = if primary { button.primary() } else { button };
            button.render(t).on_click(cx.listener(|this, _, _, cx| {
                this.add_github_account(cx);
                cx.notify();
            }))
        };
        let failed = match &self.github.step {
            Step::Failed(why) => Some(caption(why.clone(), t)),
            _ => None,
        };
        match (&self.github.step, &self.github.accounts) {
            (Step::Waiting { code, url }, _) => {
                let url = url.clone();
                line.child(readout(t).text_size(px(14.0)).child(code.clone()))
                    .child(caption(
                        "Paste this code on the GitHub page that opened. It is on your clipboard.",
                        t,
                    ))
                    .child(
                        button((id, 2usize), "Open GitHub")
                            .link()
                            .small()
                            .trailing(Icon::ExternalLink)
                            .render(t)
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .into_any_element()
            }
            (_, Accounts::Unread | Accounts::Checking) => line
                .child(caption("Checking GitHub\u{2026}", t))
                .into_any_element(),
            (step, Accounts::Missing) => line
                .child(caption(
                    "ket reaches GitHub through its command line, gh.",
                    t,
                ))
                .child(
                    button((id, 3usize), "Install gh")
                        .small()
                        .primary()
                        .loading_if(matches!(step, Step::Installing), "Installing gh\u{2026}")
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.install_gh(cx);
                            cx.notify();
                        })),
                )
                .children(failed)
                .into_any_element(),
            (_, Accounts::Failed(why)) => line
                .child(caption(why.clone(), t))
                .child(
                    button((id, 4usize), "Check again")
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.read_github_accounts(cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
            (_, Accounts::Known(_)) if !self.github.connected() => line
                .child(caption("Not signed in to GitHub.", t))
                .child(sign_in("Sign in to GitHub", true))
                .children(failed)
                .into_any_element(),
            (_, Accounts::Known(_)) => line
                .child(sign_in("Add another account", false))
                .children(failed)
                .into_any_element(),
        }
    }
}
