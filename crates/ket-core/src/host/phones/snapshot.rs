//! The host as a phone sees it: projects, worktrees and their terminals,
//! what each agent is doing, the theme, and plan usage.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ket_remote::proto;

use super::keys::answer_keys;
use super::{Phones, lock};
use crate::activity::Signal;
use crate::agent_hooks::Prompt;
use crate::event::AgentState;
use crate::host::server::Host;
use crate::id::WorktreeId;
use crate::rate_limits::{ProviderSnapshot, SnapshotStatus};
use crate::subagents::SubagentState;

/// A pull request as a phone is sent it.
fn pull_request_summary(open: &crate::github::PullRequestStatus) -> proto::PullRequestSummary {
    use crate::github::{CheckState, PullRequestState, Review};
    use proto::check_summary::State;
    use proto::pull_request_summary::{Review as Reviewed, State as Shown};
    proto::PullRequestSummary {
        number: open.number,
        url: open.url.clone(),
        draft: open.draft,
        review: match open.review {
            Review::None => Reviewed::Unspecified,
            Review::Approved => Reviewed::Approved,
            Review::ChangesRequested => Reviewed::ChangesRequested,
            Review::Required => Reviewed::Required,
        } as i32,
        checks: open
            .checks
            .iter()
            .map(|check| proto::CheckSummary {
                name: check.name.clone(),
                state: match check.state {
                    CheckState::Passed => State::Passed,
                    CheckState::Failed => State::Failed,
                    CheckState::Pending => State::Pending,
                    CheckState::Skipped => State::Skipped,
                } as i32,
                url: check.url.clone(),
            })
            .collect(),
        state: match open.state {
            PullRequestState::Open => Shown::Open,
            PullRequestState::Closed => Shown::Closed,
            PullRequestState::Merged => Shown::Merged,
        } as i32,
        base: open.base.clone(),
        behind: open.behind,
        unresolved_comments: open.unresolved_comments,
        merge_commit_allowed: open.merge_methods.merge,
        squash_merge_allowed: open.merge_methods.squash,
        rebase_merge_allowed: open.merge_methods.rebase,
    }
}

/// The background sessions a Claude terminal lists while it shows Claude's
/// agent view — see [`crate::agent_view`] — or `None` for any other.
fn agent_view(
    id: u64,
    agent: &str,
    terminal: &crate::terminal::LocalTerminal,
    place: Option<&super::places::Place>,
) -> Option<proto::AgentView> {
    use crate::agent_view::SessionState;
    use proto::background_session::State;

    if terminal.is_closed() {
        crate::agent_view::forget(id);
        return None;
    }
    // A Claude tab, or a shell someone typed `claude agents` into — which
    // is agent view by its command line alone.
    let foreground = match terminal.foreground() {
        Some(crate::activity::Foreground::Running(command)) => Some(command),
        _ => None,
    };
    let started_as = foreground
        .as_deref()
        .is_some_and(crate::agent_view::runs_agent_view);
    let claude = crate::agents::harness(agent).eq_ignore_ascii_case("claude")
        || foreground
            .as_deref()
            .is_some_and(crate::agent_view::runs_claude);
    if !claude {
        return None;
    }
    let rows = terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
    let title = terminal.title().unwrap_or_default();
    if !crate::agent_view::in_agent_view(id, &rows, &title, terminal.foreground_group(), started_as)
    {
        return None;
    }
    let (sessions, listed) = place
        .map(|place| crate::agent_view::listing(&place.path))
        // No checkout to ask about: nothing more is coming.
        .unwrap_or((Vec::new(), true));
    Some(proto::AgentView {
        listed,
        sessions: sessions
            .into_iter()
            .map(|session| proto::BackgroundSession {
                session: session.session,
                name: session.name,
                state: match session.state {
                    SessionState::NeedsYou => State::NeedsYou,
                    SessionState::Working => State::Working,
                    SessionState::Done => State::Done,
                } as i32,
                started_at_ms: session.started_at_ms,
                active_at_ms: session.active_at_ms,
                doing: session.doing,
            })
            .collect(),
    })
}

/// The host's terminals, grouped by worktree and project, with what each
/// worktree's agent is doing.
pub(super) fn snapshot(host: &Host, phones: &Phones) -> proto::HostSnapshot {
    let listing = host.listing();
    // Tabs the windows have put to sleep: no terminal, still the worktree's.
    let asleep = host.asleep_tabs();
    // Every worktree a terminal is filed under, so one made a moment ago is
    // placed in its project now rather than a few seconds from now.
    let named: Vec<&str> = listing
        .iter()
        .filter_map(|(_, key, _)| key.as_deref())
        .map(|key| key.split('|').next().unwrap_or_default())
        .chain(asleep.iter().map(|(_, tab)| tab.worktree.as_str()))
        .filter(|worktree| !worktree.is_empty())
        .collect();
    let places = phones.places_knowing(&named);
    let figures = phones.figures(&places);
    let backlogs = phones.backlog_counts(&places);
    let now = crate::now_ms();
    // Which session each pane's agent is in, for its mode.
    let panes = host.agent_status().panes;
    let mut worktrees: Vec<proto::WorktreeSummary> = Vec::new();
    // Which terminal each pane is, for naming the one a prompt is in.
    let mut terminal_of: HashMap<String, u64> = HashMap::new();
    for (id, key, terminal) in listing.iter().cloned() {
        if let Some(key) = &key {
            terminal_of.insert(key.clone(), id);
        }
        // Keys are `worktree|agent|nonce`; a terminal without one is still
        // listed, under no worktree.
        let mut parts = key.as_deref().unwrap_or_default().splitn(3, '|');
        let worktree = parts.next().unwrap_or_default().to_owned();
        let agent = parts
            .next()
            .filter(|a| *a != "shell")
            .unwrap_or_default()
            .to_owned();
        let agent_view = agent_view(id, &agent, &terminal, places.get(&worktree));
        let session = key
            .as_deref()
            .filter(|_| !agent.is_empty())
            .and_then(|key| {
                panes
                    .iter()
                    .find(|pane| pane.pane == key)
                    .and_then(|pane| pane.session.clone())
                    .or_else(|| host.bound_session(key))
            });
        let settings = key
            .as_deref()
            .filter(|_| !agent.is_empty())
            .and_then(|_| crate::agent_model::settings(&agent, session.as_deref()))
            .map(|settings| {
                let choices = |choices: Vec<crate::agent_model::Choice>| {
                    choices
                        .into_iter()
                        .map(|choice| proto::AgentChoice {
                            id: choice.id,
                            label: choice.label,
                        })
                        .collect()
                };
                proto::AgentSettings {
                    model: settings.model,
                    effort: settings.effort,
                    models: choices(settings.models),
                    efforts: choices(settings.efforts),
                    saves_default: settings.saves_default,
                }
            });
        let modes = key.as_deref().filter(|_| !agent.is_empty()).and_then(|_| {
            crate::agent_mode::modes(&agent, session.as_deref()).map(|modes| proto::AgentModes {
                current: modes.current,
                choices: modes
                    .choices
                    .into_iter()
                    .map(|mode| proto::AgentMode {
                        id: mode.id,
                        label: mode.label,
                        detail: mode.detail,
                    })
                    .collect(),
            })
        });
        let summary = proto::TerminalSummary {
            id,
            title: terminal.title().unwrap_or_else(|| {
                if agent.is_empty() {
                    "Terminal".into()
                } else {
                    agent.clone()
                }
            }),
            agent,
            closed: terminal.is_closed(),
            agent_view,
            modes,
            settings,
        };
        match worktrees.iter_mut().find(|w| w.id == worktree) {
            Some(existing) => existing.terminals.push(summary),
            None => {
                let place = places.get(&worktree);
                let figure = figures.get(&worktree).copied().unwrap_or_default();
                worktrees.push(proto::WorktreeSummary {
                    name: place.map_or_else(|| worktree.clone(), |p| p.name.clone()),
                    branch: place.map(|p| p.branch.clone()).unwrap_or_default(),
                    base: place.and_then(|p| p.base.clone()).unwrap_or_default(),
                    pinned: place.is_some_and(|p| p.pinned),
                    files_changed: figure.files_changed,
                    lines_added: figure.lines_added,
                    lines_removed: figure.lines_removed,
                    commits_ahead: figure.commits_ahead,
                    pull_request: host
                        .pull_request(&worktree)
                        .map(|open| pull_request_summary(&open)),
                    id: worktree,
                    terminals: vec![summary],
                    ..Default::default()
                });
            }
        }
    }

    // An asleep tab is listed with its worktree, for the phone to wake.
    for (_, tab) in &asleep {
        let summary = proto::AsleepTerminal {
            key: tab.key.clone(),
            title: tab.title.clone(),
            agent: tab.agent.clone(),
            since_ms: tab.since_ms,
            freed_bytes: tab.freed_bytes,
        };
        match worktrees.iter_mut().find(|w| w.id == tab.worktree) {
            Some(existing) => existing.asleep.push(summary),
            None => {
                let place = places.get(&tab.worktree);
                let figure = figures.get(&tab.worktree).copied().unwrap_or_default();
                worktrees.push(proto::WorktreeSummary {
                    name: place.map_or_else(|| tab.worktree.clone(), |p| p.name.clone()),
                    branch: place.map(|p| p.branch.clone()).unwrap_or_default(),
                    base: place.and_then(|p| p.base.clone()).unwrap_or_default(),
                    pinned: place.is_some_and(|p| p.pinned),
                    files_changed: figure.files_changed,
                    lines_added: figure.lines_added,
                    lines_removed: figure.lines_removed,
                    commits_ahead: figure.commits_ahead,
                    pull_request: host
                        .pull_request(&tab.worktree)
                        .map(|open| pull_request_summary(&open)),
                    id: tab.worktree.clone(),
                    asleep: vec![summary],
                    ..Default::default()
                });
            }
        }
    }

    // An idle worktree is still a place the phone can start a new session.
    for place in &places.worktrees {
        if worktrees.iter().any(|worktree| worktree.id == place.id) {
            continue;
        }
        let figure = figures.get(&place.id).copied().unwrap_or_default();
        worktrees.push(proto::WorktreeSummary {
            id: place.id.clone(),
            name: place.name.clone(),
            branch: place.branch.clone(),
            base: place.base.clone().unwrap_or_default(),
            pinned: place.pinned,
            files_changed: figure.files_changed,
            lines_added: figure.lines_added,
            lines_removed: figure.lines_removed,
            commits_ahead: figure.commits_ahead,
            pull_request: host
                .pull_request(&place.id)
                .map(|open| pull_request_summary(&open)),
            ..Default::default()
        });
    }

    let status = host.agent_status();
    {
        let mut activity = lock(&phones.activity);
        activity.accept_snapshot(status.clone());
        for worktree in &mut worktrees {
            if worktree.id.is_empty() {
                continue;
            }
            let id = WorktreeId::new(worktree.id.clone());
            let now_doing = activity.activity(&id, now);
            worktree.agent_state = now_doing.detail();
            worktree.agent = now_doing.agent().unwrap_or_default().to_owned();
            worktree.subagents = activity
                .subagents(&id, now)
                .into_iter()
                .map(|subagent| proto::SubagentSummary {
                    id: subagent.id,
                    label: subagent.label,
                    model: subagent.model.unwrap_or_default(),
                    state: match subagent.state {
                        SubagentState::Working => proto::subagent_summary::State::Working,
                        SubagentState::Blocked => proto::subagent_summary::State::Blocked,
                        SubagentState::Idle => proto::subagent_summary::State::Idle,
                    } as i32,
                    started_at_ms: subagent.started_at_ms,
                })
                .collect();
            worktree.activity = match activity.signal(&id, now) {
                Signal::Quiet => proto::Activity::Quiet,
                Signal::Running => proto::Activity::Running,
                Signal::Working => proto::Activity::Working,
                Signal::Merging => proto::Activity::Merging,
                Signal::Blocked => proto::Activity::Blocked,
                Signal::Failed => proto::Activity::Failed,
            } as i32;
            // Every prompt the worktree's panes are waiting on, newest first;
            // the newest alone for a phone from before 1.13.
            let mut waiting: Vec<_> = status
                .panes
                .iter()
                .filter(|pane| {
                    pane.worktree == worktree.id && pane.state == AgentState::AwaitingPermission
                })
                .collect();
            waiting.sort_by_key(|pane| std::cmp::Reverse(pane.at_ms));
            worktree.questions = waiting
                .iter()
                .filter_map(|pane| question_of(pane, &terminal_of, host))
                .collect();
            // A dialog read off the screen goes only in the list: a phone
            // too old to read it would offer Allow on the single field, and
            // no key answers a dialog that way.
            worktree.question = worktree
                .questions
                .iter()
                .find(|question| question.tool != crate::screen::DIALOG_TOOL)
                .cloned();
        }
    }

    // Projects in the order a sidebar has them, then anything ket's store
    // does not know, under the Mac itself.
    let mut projects: Vec<proto::ProjectSummary> = places
        .projects
        .iter()
        .map(|project| proto::ProjectSummary {
            id: project.id.clone(),
            name: project.name.clone(),
            worktrees: Vec::new(),
            color: project.color.clone(),
            backlog: backlogs.get(&project.id).copied().unwrap_or(0),
        })
        .collect();
    let mut elsewhere = Vec::new();
    for worktree in worktrees {
        let project = places
            .get(&worktree.id)
            .and_then(|place| projects.iter_mut().find(|p| p.id == place.project));
        match project {
            Some(project) => project.worktrees.push(worktree),
            None => elsewhere.push(worktree),
        }
    }
    // Every project the sidebar has stays, running or not: the phone lists
    // them as the desktop does, starts new work in any of them, and reaches
    // each one's backlog.
    for project in &mut projects {
        project.worktrees.sort_by_key(|w| {
            places
                .worktrees
                .iter()
                .position(|place| place.id == w.id)
                .unwrap_or(usize::MAX)
        });
    }
    if !elsewhere.is_empty() {
        projects.push(proto::ProjectSummary {
            id: String::new(),
            name: "This desktop".into(),
            worktrees: elsewhere,
            color: String::new(),
            backlog: 0,
        });
    }
    proto::HostSnapshot {
        projects,
        palette: host.theme().as_ref().map(palette),
        usage: host.usage().iter().map(provider_usage).collect(),
        agents: host.agents(),
        addresses: phones.relay.addresses(),
    }
}

/// The desktop's theme, as the colours a phone draws with.
fn palette(theme: &crate::theme::Theme) -> proto::Palette {
    let hex = |color: crate::theme::Color| color.to_hex();
    proto::Palette {
        light: theme.appearance == crate::theme::Appearance::Light,
        backdrop: hex(theme.backdrop),
        surface: hex(theme.surface),
        sunken: hex(theme.sunken),
        elevated: hex(theme.elevated),
        hover: hex(theme.hover),
        selection: hex(theme.selection),
        border: hex(theme.border),
        rule: hex(theme.rule),
        accent: hex(theme.accent),
        on_accent: hex(theme.on_accent),
        text: hex(theme.text.primary),
        dim: hex(theme.text.dim),
        marker: hex(theme.marker),
        running: hex(theme.status.running),
        attention: hex(theme.status.attention),
        failed: hex(theme.status.failed),
        merging: hex(theme.status.merging),
        added: hex(theme.diff.added),
        removed: hex(theme.diff.removed),
        modified: hex(theme.diff.modified),
        quota_warm: hex(theme.quota.warm),
        quota_hot: hex(theme.quota.hot),
        quota_critical: hex(theme.quota.critical),
    }
}

/// One agent's plan usage, for a phone.
/// A risk read and its confidence.
type RiskRead = (crate::jev::Risk, f32);

/// Jev's risk reads, by prompt id: `None` while one is being asked for.
static RISKS: OnceLock<Mutex<HashMap<u64, Option<RiskRead>>>> = OnceLock::new();

/// Most prompts remembered before the oldest reads are dropped.
const RISKS_KEPT: usize = 256;

/// A prompt's risk read, when Jev's beta is on and has answered.
///
/// The first snapshot to see a prompt asks for its read on a thread of its
/// own and goes out without it; the snapshot tick after the answer carries
/// it. Each prompt is asked about once, whatever the answer.
/// The prompt `pane` is waiting on, as a phone reads it, or `None` when the
/// pane's status names no question yet.
fn question_of(
    pane: &crate::agent_status::PaneAgentStatus,
    terminal_of: &HashMap<String, u64>,
    host: &Host,
) -> Option<proto::Question> {
    let question = pane.question.as_ref()?;
    // The terminal the prompt is in, which the answer must
    // name. Without one — or before the prompt has an id —
    // it can be read on the phone but not answered.
    let terminal = terminal_of.get(&pane.pane).copied();
    let bound = terminal.is_some() && question.id != 0;
    // A question or a plan is answered through its held
    // hook, so it can be while one is held.
    let answerable = match question.prompt {
        // Read off the screen and answered with its number: no hook to hold.
        Some(Prompt::Dialog { .. }) => bound,
        // Codex's form, answered with its numbers: no hook to hold either.
        Some(Prompt::Ask { ref questions })
            if question.tool == crate::agent_hooks::CODEX_ASK_TOOL =>
        {
            bound && crate::agent_hooks::codex_answerable(questions)
        }
        Some(_) => bound && host.prompt_held(&pane.pane, question),
        None => {
            bound
                && answer_keys(
                    &pane.agent,
                    &question.tool,
                    proto::answer::Choice::AllowOnce,
                )
                .is_some()
        }
    };
    Some(proto::Question {
        tool: question.tool.clone(),
        subject: question.subject.clone().unwrap_or_default(),
        answerable,
        always: bound
            && answer_keys(
                &pane.agent,
                &question.tool,
                proto::answer::Choice::AllowAlways,
            )
            .is_some(),
        id: question.id,
        terminal_id: terminal.unwrap_or_default(),
        // Nothing to weigh: it asks for no access at all. And
        // never for a local agent, whose commands stay on
        // the Mac: Jev's read would send them to TypeSafe.
        risk: (question.prompt.is_none() && crate::local_models::model_tag(&pane.agent).is_none())
            .then(|| risk_read(question))
            .flatten(),
        asks: match &question.prompt {
            Some(Prompt::Ask { questions }) => questions
                .iter()
                .map(|asked| proto::Asked {
                    question: asked.question.clone(),
                    header: asked.header.clone(),
                    choices: asked
                        .options
                        .iter()
                        .map(|option| proto::asked::Choice {
                            label: option.label.clone(),
                            description: option.description.clone(),
                        })
                        .collect(),
                    multi_select: asked.multi_select,
                })
                .collect(),
            // One question, the dialog's, with its options as choices. A
            // dialog with no question line of its own is asked by its title.
            Some(Prompt::Dialog { title, options }) => {
                let subject = question.subject.clone().unwrap_or_default();
                let (asked, header) = match subject.is_empty() {
                    true => (title.clone(), String::new()),
                    false => (subject, title.clone()),
                };
                vec![proto::Asked {
                    question: asked,
                    header,
                    choices: options
                        .iter()
                        .map(|option| proto::asked::Choice {
                            label: option.clone(),
                            description: String::new(),
                        })
                        .collect(),
                    multi_select: false,
                }]
            }
            _ => Vec::new(),
        },
        plan: matches!(question.prompt, Some(Prompt::Plan)),
    })
}

fn risk_read(question: &crate::agent_hooks::PermissionQuestion) -> Option<proto::Risk> {
    if question.id == 0 {
        return None;
    }
    let risks = RISKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut known = lock(risks);
    if let Some(read) = known.get(&question.id) {
        return read.map(|(risk, confidence)| proto::Risk {
            level: risk.word().to_owned(),
            confidence,
        });
    }
    if known.len() >= RISKS_KEPT {
        known.clear();
    }
    known.insert(question.id, None);
    drop(known);
    let (id, tool, subject) = (
        question.id,
        question.tool.clone(),
        question.subject.clone().unwrap_or_default(),
    );
    let asked = std::thread::Builder::new()
        .name("ket-host-jev".into())
        .spawn(move || {
            let Ok(config) = crate::config::Config::load() else {
                return;
            };
            let read = crate::jev::risk_of(&config.jev, &tool, &subject);
            if read.is_some() {
                lock(risks).insert(id, read);
            }
        });
    if let Err(e) = asked {
        tracing::info!("jev: could not ask for a risk read: {e}");
    }
    None
}

fn provider_usage(snapshot: &ProviderSnapshot) -> proto::ProviderUsage {
    let (status, reason) = match &snapshot.status {
        SnapshotStatus::Fresh => (proto::UsageStatus::Fresh, String::new()),
        SnapshotStatus::Stale => (proto::UsageStatus::Stale, String::new()),
        SnapshotStatus::Unavailable { reason } => (proto::UsageStatus::Unavailable, reason.clone()),
        SnapshotStatus::Unsupported => (proto::UsageStatus::Unsupported, String::new()),
    };
    // Being turned away outranks whatever else there is to say about a
    // provider, and is the one thing about it a phone must not miss.
    let reason = match snapshot.limited_at_ms {
        Some(_) => "Rate limited".to_owned(),
        None => reason,
    };
    proto::ProviderUsage {
        provider: snapshot.provider.label().to_owned(),
        status: status as i32,
        windows: snapshot
            .windows
            .iter()
            .map(|window| proto::UsageWindow {
                name: window.name.clone(),
                used_percent: window.used_percent,
                resets_at_ms: window.resets_at_ms.unwrap_or_default(),
            })
            .collect(),
        plan: snapshot.plan.clone().unwrap_or_default(),
        fetched_at_ms: snapshot.fetched_at_ms.unwrap_or_default(),
        reason,
    }
}
