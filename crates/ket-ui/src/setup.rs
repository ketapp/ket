//! First-run coding-agent detection and setup actions.
//!
//! Only each agent's own CLI answers whether it is signed in. A CLI without
//! a documented status subcommand is treated as installed and ready to open;
//! ket does not inspect another application's private state.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::fonts::Prose;
use gpui::{AnyElement, AsyncApp, Context, WeakEntity, Window, div, prelude::*, px};
use ket_core::agents::{Catalogue, Prober, ShellProber};

use crate::Shell;
use crate::paint::paint;
use crate::ui::button::button;
use crate::ui::icon::{Icon, sized_icon};

/// The state the quick-start UI can establish without opening private files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AgentState {
    #[default]
    Checking,
    Missing,
    SignedOut,
    Ready,
}

impl AgentState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking…",
            Self::Missing => "Not installed",
            Self::SignedOut => "Signed out",
            Self::Ready => "Ready",
        }
    }
}

/// The two optional quick starts, in the order the setup view displays them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuickStart {
    Claude,
    Codex,
}

impl QuickStart {
    pub(crate) const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    fn command(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub(crate) fn install(self) -> &'static str {
        match self {
            Self::Claude => "curl -fsSL https://claude.ai/install.sh | bash",
            Self::Codex => "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
        }
    }

    pub(crate) fn login(self) -> &'static str {
        match self {
            Self::Claude => "claude auth login",
            Self::Codex => "codex login",
        }
    }

    fn status_args(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["auth", "status"],
            Self::Codex => &["login", "status"],
        }
    }

    pub(crate) fn check(self, path: &Path) -> AgentState {
        // A shell alias or function may be runnable but has no executable
        // file to ask here. In that case the installed state is all we claim.
        if !path.is_file() || !has_status_command(path, self.status_args()[0]) {
            return AgentState::Ready;
        }
        match bounded_exit(path, self.status_args()) {
            Some(true) => AgentState::Ready,
            Some(false) => AgentState::SignedOut,
            None => AgentState::Ready,
        }
    }
}

fn has_status_command(path: &Path, group: &str) -> bool {
    let Some(help) = bounded_output(path, &["--help"]) else {
        return false;
    };
    if !help.status.success()
        || !(String::from_utf8_lossy(&help.stdout).contains(group)
            || String::from_utf8_lossy(&help.stderr).contains(group))
    {
        return false;
    }
    let Some(output) = bounded_output(path, &[group, "--help"]) else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).contains("status")
        || String::from_utf8_lossy(&output.stderr).contains("status")
}

fn bounded_exit(path: &Path, args: &[&str]) -> Option<bool> {
    bounded_output(path, args).map(|output| output.status.success())
}

fn bounded_output(path: &Path, args: &[&str]) -> Option<std::process::Output> {
    let path = path.to_path_buf();
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = Command::new(path).args(args).stdin(Stdio::null()).output();
        let _ = tx.send(output);
    });
    rx.recv_timeout(Duration::from_secs(4)).ok()?.ok()
}

/// A detected agent and the state reported by its CLI.
pub(crate) struct SetupAgent {
    pub(crate) name: String,
    pub(crate) state: AgentState,
}

pub(crate) fn checking_agents() -> Vec<SetupAgent> {
    QuickStart::ALL
        .into_iter()
        .map(|agent| SetupAgent {
            name: agent.command().to_owned(),
            state: AgentState::Checking,
        })
        .collect()
}

impl QuickStart {
    fn for_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|agent| agent.command() == name)
    }
}

/// Includes native installations made after ket inherited its PATH.
struct SetupProber;

impl Prober for SetupProber {
    fn find(&self, command: &str) -> Option<PathBuf> {
        ShellProber.find(command).or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/bin").join(command))
                .filter(|path| path.is_file())
        })
    }
}

impl Shell {
    pub(crate) fn refresh_setup(&mut self, cx: &mut Context<Self>) {
        ket_core::shell::clear_probe_cache();
        let check = cx.background_executor().spawn(async move {
            let config = ket_core::config::Config::load().unwrap_or_default();
            let catalogue = Catalogue::detect(&config, &SetupProber);
            let mut agents: Vec<_> = catalogue
                .runnable()
                .map(|entry| SetupAgent {
                    name: entry.name.clone(),
                    state: QuickStart::for_name(&entry.name)
                        .zip(entry.found_at.as_deref())
                        .map_or(AgentState::Ready, |(agent, path)| agent.check(path)),
                })
                .collect();
            if agents.is_empty() {
                agents = QuickStart::ALL
                    .into_iter()
                    .map(|agent| SetupAgent {
                        name: agent.command().to_owned(),
                        state: AgentState::Missing,
                    })
                    .collect();
            }
            agents
        });
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let mut agents = check.await;
            let _ = this.update(cx, |shell, cx| {
                // A refresh while the installer runs may find another agent.
                // Keep the active command reachable until it exits or is stopped.
                if let Some((active, _)) = shell.setup_running
                    && !agents.iter().any(|agent| agent.name == active.command())
                {
                    agents.push(SetupAgent {
                        name: active.command().to_owned(),
                        state: shell.setup_state(active.command()),
                    });
                }
                shell.setup_agents = agents;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_setup(&mut self, cx: &mut Context<Self>) {
        self.setup_open = true;
        self.preferences = None;
        self.popup = None;
        self.refresh_setup(cx);
        cx.notify();
    }

    pub(crate) fn setup_state(&self, name: &str) -> AgentState {
        self.setup_agents
            .iter()
            .find(|agent| agent.name == name)
            .map_or(AgentState::Missing, |agent| agent.state)
    }

    pub(crate) fn setup_visible(&self) -> bool {
        self.setup_open
            || (self.space().is_none() && (!self.setup_ready() || self.projects.is_empty()))
    }

    pub(crate) fn setup_ready(&self) -> bool {
        self.setup_agents
            .iter()
            .any(|agent| agent.state == AgentState::Ready)
    }

    pub(crate) fn setup_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let agent_done = self.setup_ready();
        let project_done = !self.projects.is_empty();
        let found = self
            .setup_agents
            .iter()
            .any(|agent| agent.state != AgentState::Missing);
        let mut agents = div().flex().flex_col().ml(px(32.0)).mt(px(10.0));
        for (index, agent) in self.setup_agents.iter().enumerate() {
            let quick = QuickStart::for_name(&agent.name);
            let running = self
                .setup_running
                .filter(|(active, _)| Some(*active) == quick);
            let busy = running.is_some();
            let label = running.map_or(agent.state.label(), |(_, install)| {
                if install { "Installing" } else { "Signing in" }
            });
            let dot = if busy {
                Some(t.status.attention)
            } else {
                match agent.state {
                    AgentState::Ready => Some(t.status.running),
                    AgentState::SignedOut => Some(t.diff.modified),
                    AgentState::Missing => Some(t.text.dim),
                    AgentState::Checking => None,
                }
            };
            let (mark, tint) = crate::ui::agent::provider(&agent.name, t);
            let row = crate::ui::row::heading_row(("setup-agent", index), t)
                .h(px(44.0))
                .px(px(0.0))
                .gap(px(12.0))
                .cursor_default()
                .prose()
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(28.0))
                        .rounded(px(7.0))
                        .bg(paint(t.panel))
                        .child(sized_icon(mark, px(16.0), tint)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(crate::ui::agent::agent_label(&agent.name)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.0))
                        .text_color(paint(if busy { t.status.attention } else { t.text.dim }))
                        .children(dot.map(|colour| {
                            crate::ui::chip::status_dot(
                                paint(colour),
                                crate::ui::chip::Motion::Still,
                                px(6.0),
                            )
                        }))
                        .child(label),
                )
                .children(quick.and_then(|quick| {
                    if busy {
                        Some(
                            button(("stop-setup", index), "Stop")
                                .ghost()
                                .small()
                                .render(t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(id) = this.setup_terminal {
                                        this.close_terminal(id);
                                    }
                                    this.refresh_setup(cx);
                                    cx.notify();
                                })),
                        )
                    } else if matches!(agent.state, AgentState::Missing | AgentState::SignedOut) {
                        let install = agent.state == AgentState::Missing;
                        Some(
                            button(
                                ("run-setup", index),
                                if install { "Install" } else { "Sign in" },
                            )
                            .small()
                            .enabled(self.setup_terminal.is_none())
                            .render(t)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.start_setup_command(quick, install, cx);
                                },
                            )),
                        )
                    } else {
                        None
                    }
                }));
            let mut block = div()
                .flex()
                .flex_col()
                .when(index > 0, |el| el.border_t_1().border_color(paint(t.rule)))
                .child(row);
            if busy && let Some(id) = self.setup_terminal {
                block = block.child(
                    div()
                        .ml(px(40.0))
                        .mb(px(8.0))
                        .h(px(112.0))
                        .flex_none()
                        .rounded(px(8.0))
                        .bg(paint(t.panel))
                        .border_1()
                        .border_color(paint(t.rule))
                        .overflow_hidden()
                        .child(self.terminal_pane(id, window, cx)),
                );
            }
            agents = agents.child(block);
        }
        let mut add = button("setup-add-project", "Add Project").leading(Icon::Plus);
        if agent_done {
            add = add.primary();
        }
        div()
            .flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .px(px(24.0))
            .prose()
            .text_color(paint(t.text.primary))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(px(520.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .mb(px(26.0))
                            .child(sized_icon(Icon::KetMark, px(26.0), paint(t.status.running)))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(24.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Welcome to ket"),
                            )
                            .children(self.setup_open.then(|| {
                                button("close-setup", "Done")
                                    .ghost()
                                    .small()
                                    .render(t)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(id) = this.setup_terminal {
                                            this.close_terminal(id);
                                            this.refresh_setup(cx);
                                        }
                                        this.setup_open = false;
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(px(10.0))
                            .bg(paint(t.sunken))
                            .p(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(step_marker(1, agent_done, t))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(if found {
                                                "Coding agents"
                                            } else {
                                                "Install a coding agent"
                                            }),
                                    ),
                            )
                            .child(agents),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .mt(px(10.0))
                            .rounded(px(10.0))
                            .bg(paint(t.sunken))
                            .p(px(16.0))
                            .child(step_marker(2, project_done, t))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(14.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Add a project"),
                            )
                            .children((!project_done).then(|| {
                                add.render(t)
                                    .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn step_marker(number: u8, done: bool, t: &ket_core::theme::Theme) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.0))
        .rounded_full()
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .when(done, |el| {
            el.bg(paint(t.status.running)).child(sized_icon(
                Icon::Check,
                px(14.0),
                paint(t.surface),
            ))
        })
        .when(!done, |el| {
            el.border(px(1.5))
                .border_color(paint(t.text.dim))
                .child(number.to_string())
        })
}
