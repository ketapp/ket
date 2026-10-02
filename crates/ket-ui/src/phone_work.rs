//! New work a phone asked for: a worktree made the way the Create Worktree
//! dialog makes one, landed in, and its agent started on the phone's prompt.
//! A backlog note started from its dialog goes the same way — see
//! [`crate::backlog`].
//!
//! The host hands each request to one window — see
//! `ket_core::host::take_phone_work` — and this is that window doing it, so a
//! worktree started from a phone is the same as one started here: in the
//! store, in the sidebar, with its agent's session in a tab.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{Context, WeakEntity};
use ket_core::host::PhoneWork;
use ket_core::id::ProjectId;
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::ui::toast::Tone;

/// How long a new agent is given to start before its prompt is typed. Its
/// input is read from the terminal once its interface is up; typed sooner,
/// the prompt lands in the shell it is starting from.
const AGENT_START: Duration = Duration::from_secs(4);

/// How long a new worktree's tabs are given to open their terminals before
/// landing gives up on finding its agent.
const TERMINAL_OPEN: Duration = Duration::from_secs(15);

/// How often landing looks again while they open.
const LANDING_POLL: Duration = Duration::from_millis(100);

/// The longest attached images are waited for before the prompt follows
/// them anyway — see [`Shell::images_shown`].
const ATTACH_WAIT: Duration = Duration::from_secs(4);

/// A beat after the last image shows, before the prompt is typed.
const ATTACH_SETTLE: Duration = Duration::from_millis(150);

/// How often the screen is looked at while images are read.
const ATTACH_POLL: Duration = Duration::from_millis(50);

impl Shell {
    /// Starts whatever work phones have asked for since the last look.
    pub(crate) fn take_phone_work(&mut self, cx: &mut Context<Self>) {
        for work in ket_core::host::take_phone_work() {
            self.start_phone_work(work, cx);
        }
        // Operator authority is the approval to merge. Keep later requests
        // queued while one merge is running rather than acknowledging work
        // that only opened a desktop dialog.
        if self.merging.is_none()
            && let Some(worktree) = ket_core::host::take_phone_merge()
        {
            let id = ket_core::id::WorktreeId::from(worktree.as_str());
            let Some(at) = self.position_of(&id) else {
                self.toast(
                    Tone::Error,
                    "A phone asked to merge a worktree that isn't here",
                    cx,
                );
                return;
            };
            let Some(node) = self
                .projects
                .get(at.project)
                .and_then(|project| project.worktrees.get(at.worktree))
            else {
                return;
            };
            if node.primary || node.missing {
                self.toast(Tone::Error, "The phone's worktree cannot be merged", cx);
                return;
            }
            self.toast(Tone::Info, "Merging from your phone…", cx);
            self.merge_worktree_now(id, false, cx);
        }
    }

    fn start_phone_work(&mut self, work: PhoneWork, cx: &mut Context<Self>) {
        let project = ProjectId::from(work.project.as_str());
        let Some(name) = self.project_name(&project) else {
            self.toast(
                Tone::Error,
                "A phone asked for work in a project that isn't here",
                cx,
            );
            return;
        };
        self.toast(
            Tone::Info,
            format!("Starting work in {name} from your phone…"),
            cx,
        );
        // A backlog note: started as the backlog's own Start starts it, files
        // and all, and marked done once its worktree exists.
        if let Some(note) = work.note.as_deref() {
            self.start_phone_note(&project, note, cx);
            return;
        }
        let branch = branch_for(&work.prompt, "phone-task");
        self.start_work(
            NewWork {
                project,
                branch,
                prompt: work.prompt,
                attachments: Vec::new(),
                agent: work.agent,
                base: None,
                economy: None,
                origin: WorkOrigin::Phone,
            },
            cx,
        );
    }

    fn project_name(&self, project: &ProjectId) -> Option<String> {
        self.projects
            .iter()
            .find(|node| &node.id == project)
            .map(|node| node.name.to_string())
    }

    /// Tells the host which agents this window can start, for a phone's New
    /// task to offer. In the background: finding an agent that is not on
    /// `PATH` asks a login shell. Sent at launch and when Settings closes,
    /// which is where agents are turned on and off.
    pub(crate) fn publish_agents(&self, cx: &mut Context<Self>) {
        cx.background_executor()
            .spawn(async {
                let names = crate::terminal::runnable_specs()
                    .into_iter()
                    .map(|spec| spec.name)
                    .collect();
                ket_core::host::publish_agents(names);
            })
            .detach();
    }

    /// Makes a worktree the way the Create Worktree dialog makes one, lands
    /// in it, and starts its agent on `work.prompt`.
    pub(crate) fn start_work(&mut self, work: NewWork, cx: &mut Context<Self>) {
        let name = self.project_name(&work.project).unwrap_or_default();
        // The agent asked for, if this desktop can run it; else the
        // project's own, which creating the worktree picks.
        let agent = work.agent.clone().filter(|agent| {
            self.runnable_agents()
                .iter()
                .any(|spec| &spec.name == agent)
        });

        let asked = agent.clone();
        let project = work.project.clone();
        let branch = work.branch.clone();
        let prompt = work.prompt.clone();
        let base = work.base.clone();
        let picked = work.economy;
        let created = cx.background_executor().spawn(async move {
            // Jev's beta, when it is on and nobody picked a level: the level
            // chosen from the prompt, among the pack's own. Anything else —
            // off, slow, refused — leaves the level to the default, as before.
            let chosen = match picked {
                Some(_) => None,
                None => ket_core::config::Config::load()
                    .ok()
                    .and_then(|config| ket_core::jev::economy_for(&config.jev, &prompt)),
            };
            let level_id = picked
                .or(chosen.map(|(level, _)| level))
                .map(|level| ket_core::worktree::token_reduction(level).id.clone());
            let workspace = Workspace::open()?;
            let (created, _) = workspace.create_worktree_prepared_with_economy(
                &project,
                &branch,
                base.as_deref(),
                asked.as_deref(),
                level_id.as_deref(),
            )?;
            Ok::<_, ket_core::KetError>((created, chosen))
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let created = created.await;
            let _ = shell.update(cx, |shell, cx| match created {
                Ok((created, chosen)) => {
                    if let Some((level, confidence)) = chosen {
                        shell.toast(
                            Tone::Info,
                            format!(
                                "Economy: {} \u{b7} chosen by Jev ({:.0}%)",
                                ket_core::worktree::token_reduction(level).name(),
                                confidence * 100.0
                            ),
                            cx,
                        );
                    }
                    if let WorkOrigin::Backlog { note } = &work.origin {
                        shell.backlog_started(&work.project, note, &created, cx);
                    }
                    shell.land_work(created.id, agent, work.prompt, work.attachments, cx);
                }
                Err(error) => match &work.origin {
                    WorkOrigin::Phone => shell.toast(
                        Tone::Error,
                        format!("Couldn't start the phone's work in {name}: {error}"),
                        cx,
                    ),
                    WorkOrigin::QuickPrompt => shell.toast(
                        Tone::Error,
                        format!("Couldn't start a worktree in {name}: {error}"),
                        cx,
                    ),
                    WorkOrigin::Backlog { .. } => shell.backlog_start_failed(error.to_string(), cx),
                },
            });
        })
        .detach();
    }

    /// Shows the new worktree, makes sure its agent is running, and types the
    /// prompt once the agent has had time to start — the images first, as
    /// pastes the agent attaches, and the other files as paths in the prompt.
    pub(crate) fn land_work(
        &mut self,
        id: ket_core::id::WorktreeId,
        agent: Option<String>,
        prompt: String,
        attachments: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.reload(cx);
        self.remeasure_worktree(id.clone(), cx);
        // Landing in it opens its agent's session on this first visit, as a
        // click would.
        if let Some(selection) = self.position_of(&id) {
            self.select(selection, cx);
        }
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            // A tab's terminal opens in the background, so the one landing
            // just opened is not an agent yet. Wait for this worktree's tabs
            // to finish starting, then look; open an agent only when none of
            // them turned out to be one, so a session the landing started is
            // not doubled.
            let mut asked = false;
            let mut waited = Duration::ZERO;
            let terminal = loop {
                let step = shell
                    .update(cx, |shell, cx| {
                        if shell.terminals_starting(&id) {
                            return Landing::Wait;
                        }
                        if let Some(terminal) = shell.agent_terminal(&id) {
                            return Landing::Ready(terminal);
                        }
                        if asked {
                            return Landing::Missing;
                        }
                        asked = true;
                        let pane = shell.spaces.entry(id.clone()).or_default().focused;
                        let agent = agent.clone().or_else(|| {
                            shell
                                .runnable_agents()
                                .first()
                                .map(|spec| spec.name.clone())
                        });
                        let before = shell.next_terminal;
                        if let Some(agent) = agent {
                            shell.open_agent_tab_in(pane, &agent, cx);
                        }
                        if shell.next_terminal == before {
                            Landing::Missing
                        } else {
                            Landing::Wait
                        }
                    })
                    .unwrap_or(Landing::Missing);
                match step {
                    Landing::Ready(terminal) => break Some(terminal),
                    Landing::Missing => break None,
                    Landing::Wait if waited >= TERMINAL_OPEN => break None,
                    Landing::Wait => {
                        cx.background_executor().timer(LANDING_POLL).await;
                        waited += LANDING_POLL;
                    }
                }
            };
            let Some(terminal) = terminal else {
                let _ = shell.update(cx, |shell, cx| {
                    shell.toast(
                        Tone::Error,
                        "The new worktree has no agent to give the prompt to",
                        cx,
                    );
                });
                return;
            };
            cx.background_executor().timer(AGENT_START).await;
            let (before, left) = shell
                .update(cx, |shell, cx| {
                    let before = shell.images_shown(terminal);
                    (before, shell.attach_to_terminal(terminal, &attachments, cx))
                })
                .unwrap_or((0, attachments.clone()));
            let attached = attachments.len() - left.len();
            wait_for_images(&shell, terminal, before + attached, cx).await;
            let prompt = with_files(prompt, &left);
            let _ = shell.update(cx, |shell, cx| {
                shell.submit_to_terminal(terminal, &prompt, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Shell {
    /// Hands `prompt` to `id`'s agent with `attachments`: images pasted first
    /// as the agent's own attachments, waited for until it shows them, then
    /// the prompt. With no agent open there, one is opened as landing new
    /// work does. Without attachments this is [`Shell::send_prompt_to_worktree`].
    pub(crate) fn send_with_attachments(
        &mut self,
        id: ket_core::id::WorktreeId,
        prompt: String,
        attachments: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if attachments.is_empty() {
            self.send_prompt_to_worktree(id, prompt, cx);
            return;
        }
        let Some(terminal) = self.agent_terminal(&id) else {
            self.land_work(id, None, prompt, attachments, cx);
            return;
        };
        let before = self.images_shown(terminal);
        let left = self.attach_to_terminal(terminal, &attachments, cx);
        let attached = attachments.len() - left.len();
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            wait_for_images(&shell, terminal, before + attached, cx).await;
            let prompt = with_files(prompt, &left);
            let _ = shell.update(cx, |shell, cx| {
                shell.submit_to_terminal(terminal, &prompt, cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// Waits until `terminal` shows `want` image chips — the agent has read what
/// was pasted — or [`ATTACH_WAIT`] has gone by, then a beat more. Returns at
/// once when nothing was attached.
async fn wait_for_images(
    shell: &WeakEntity<Shell>,
    terminal: crate::terminal::TerminalId,
    want: usize,
    cx: &mut gpui::AsyncApp,
) {
    let mut waited = Duration::ZERO;
    loop {
        let shown = shell
            .update(cx, |shell, _| shell.images_shown(terminal))
            .unwrap_or(want);
        if shown >= want || waited >= ATTACH_WAIT {
            break;
        }
        cx.background_executor().timer(ATTACH_POLL).await;
        waited += ATTACH_POLL;
    }
    cx.background_executor().timer(ATTACH_SETTLE).await;
}

/// Where landing in a new worktree has got to in finding its agent.
enum Landing {
    /// A tab is still opening its terminal.
    Wait,
    Ready(crate::terminal::TerminalId),
    /// No agent is running there, and none could be opened.
    Missing,
}

/// Work to start in a new worktree.
pub(crate) struct NewWork {
    pub(crate) project: ProjectId,
    pub(crate) branch: String,
    pub(crate) prompt: String,
    /// Files handed to the agent with the prompt.
    pub(crate) attachments: Vec<PathBuf>,
    /// The agent asked for; the project's own when `None`.
    pub(crate) agent: Option<String>,
    /// The branch to cut it from; the project's default base when `None`.
    pub(crate) base: Option<String>,
    /// The Economy level picked for it. `None` leaves the choice to Jev,
    /// when its beta is on, and to the default level otherwise.
    pub(crate) economy: Option<u8>,
    pub(crate) origin: WorkOrigin,
}

/// Who asked for the work, which is who hears how it went.
pub(crate) enum WorkOrigin {
    Phone,
    /// The quick prompt, sent to a new worktree.
    QuickPrompt,
    /// A backlog note, marked done once its worktree exists.
    Backlog {
        note: String,
    },
}

/// `prompt`, with the files the agent could not be handed as attachments
/// listed after it by path.
fn with_files(prompt: String, files: &[PathBuf]) -> String {
    if files.is_empty() {
        return prompt;
    }
    let list: Vec<String> = files
        .iter()
        .map(|path| format!("- {}", path.display()))
        .collect();
    format!("{prompt}\n\nAttached files:\n{}", list.join("\n"))
}

/// A branch name from a prompt: its first few words, and a few characters
/// that keep two alike from colliding — `fix-the-login-redirect-k3f9`.
pub(crate) fn branch_for(prompt: &str, fallback: &str) -> String {
    branch_seeded(prompt, fallback, ket_core::now_ms())
}

/// [`branch_for`] with the trailing characters taken from `seed` rather than
/// the clock, for a caller that shows the name before it makes it and has to
/// get the same one both times.
pub(crate) fn branch_seeded(prompt: &str, fallback: &str, seed: u64) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(5)
        .map(str::to_ascii_lowercase)
        .collect();
    let mut stem = words.join("-");
    stem.truncate(40);
    let stem = stem.trim_end_matches('-');
    let stem = if stem.is_empty() { fallback } else { stem };
    let tag: String = (0..4)
        .map(|at| {
            let digit = (seed / 36_u64.pow(at)) % 36;
            char::from_digit(u32::try_from(digit).unwrap_or(0), 36).unwrap_or('0')
        })
        .collect();
    format!("{stem}-{tag}")
}
