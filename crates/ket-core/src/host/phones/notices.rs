//! What's news for a paired phone that isn't connected: the rules of the
//! phone's own notices (`compare` in `mobile/app/src/lib/notices.ts`),
//! applied here to the same snapshot, so a push says what the phone would
//! have said had it been awake. Change one and change the other.
//!
//! - an agent starting to wait on you;
//! - one that was at work going quiet: finished; or one failing;
//! - a pull request's checks starting to fail.
//!
//! The first look only learns what is already so. Nothing is sent at once:
//! each notice waits `[phones] push_after_seconds` and goes only if it is
//! still so then — still waiting, still unread, still failing — so one dealt
//! with at the Mac doesn't buzz the phone as well. While someone is using
//! the Mac, what comes due is held, and goes once they've stepped away if
//! it is still so then (see `Phones::tick`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ket_remote::proto::{self, Activity, check_summary};

/// The most of a notice's title and body sent: the whole has to fit Apple's
/// 4 KB once encrypted and encoded.
const MOST_TITLE: usize = 120;
const MOST_BODY: usize = 300;

/// What a notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::host) enum Kind {
    Waiting,
    Finished,
    /// Sent to a phone that asked for finished tasks: "A task finishes"
    /// covers both.
    Failed,
    Checks,
}

/// Where a tap on the notification goes on the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::host) enum Opens {
    /// The chat of this terminal.
    Chat(u64),
    Worktree,
    PullRequest,
}

/// One notice, in the words it is shown with.
#[derive(Debug, Clone)]
pub(in crate::host) struct Notice {
    pub(in crate::host) kind: Kind,
    /// Names it: a newer notice with the same id takes its place on the
    /// phone. The phone's own notices use the same ids.
    pub(in crate::host) id: String,
    /// Groups a project's notifications on the phone.
    pub(in crate::host) project: String,
    pub(in crate::host) worktree: String,
    pub(in crate::host) title: String,
    pub(in crate::host) body: String,
    pub(in crate::host) opens: Opens,
    /// The phones connected at some moment while it waited: each heard it
    /// from its own snapshot, posting it itself or showing it on screen,
    /// and is not pushed it again.
    pub(in crate::host) heard: Vec<Vec<u8>>,
    /// For a wait, the prompt its words name, and who is asking: answered
    /// while another keeps the worktree waiting, it is re-worded for that
    /// one before it goes.
    prompt: Option<u64>,
    who: String,
}

/// A worktree at the last look.
struct Seen {
    activity: Activity,
    agent: String,
    checks_failed: bool,
}

/// What the last look saw, and what is waiting out its delay.
#[derive(Default)]
pub(in crate::host) struct Notices {
    /// `None` until the first look, which only learns what is already so.
    seen: Option<HashMap<String, Seen>>,
    pending: Vec<(Instant, Notice)>,
}

impl Notices {
    /// Forgets everything: nothing is being watched.
    pub(in crate::host) fn reset(&mut self) {
        if self.seen.is_some() || !self.pending.is_empty() {
            *self = Self::default();
        }
    }

    /// Looks at `snapshot`, with the phones in `connected` connected. What
    /// is news since the last look waits `after`; what has waited that long
    /// and is still so comes back, to be sent.
    pub(in crate::host) fn look(
        &mut self,
        snapshot: &proto::HostSnapshot,
        now: Instant,
        after: Duration,
        connected: &[Vec<u8>],
    ) -> Vec<Notice> {
        let before = self.seen.take();
        let mut seen = HashMap::new();
        for project in &snapshot.projects {
            for worktree in &project.worktrees {
                let activity = activity_of(worktree);
                let was = before.as_ref().and_then(|before| before.get(&worktree.id));
                let agent = agent_of(worktree)
                    .or_else(|| was.map(|was| was.agent.clone()))
                    .unwrap_or_default();
                let checks_failed = checks_failed(worktree);
                if before.is_some() {
                    for notice in news(project, worktree, activity, was, &agent, checks_failed) {
                        self.pending.retain(|(_, waiting)| waiting.id != notice.id);
                        self.pending.push((now + after, notice));
                    }
                }
                seen.insert(
                    worktree.id.clone(),
                    Seen {
                        activity,
                        agent,
                        checks_failed,
                    },
                );
            }
        }
        self.seen = Some(seen);
        for (_, notice) in &mut self.pending {
            for device in connected {
                if !notice.heard.contains(device) {
                    notice.heard.push(device.clone());
                }
            }
        }

        let mut due = Vec::new();
        self.pending.retain_mut(|(at, notice)| {
            if *at > now {
                return true;
            }
            if still(notice, snapshot) {
                due.push(notice.clone());
            }
            false
        });
        due
    }

    /// Puts `due` back to wait until `until`, when each goes again only if
    /// it is still so: for while someone is at the Mac, which shows it.
    pub(in crate::host) fn hold(&mut self, due: Vec<Notice>, until: Instant) {
        for notice in due {
            self.pending.retain(|(_, waiting)| waiting.id != notice.id);
            self.pending.push((until, notice));
        }
    }
}

/// What has happened to `worktree` since it was `was`.
fn news(
    project: &proto::ProjectSummary,
    worktree: &proto::WorktreeSummary,
    activity: Activity,
    was: Option<&Seen>,
    agent: &str,
    checks_failed: bool,
) -> Vec<Notice> {
    let mut news = Vec::new();
    let name = if worktree.name.is_empty() {
        &worktree.branch
    } else {
        &worktree.name
    };
    let title = cut(&format!("{} \u{b7} {name}", project.name), MOST_TITLE);
    let who = if agent.is_empty() {
        "An agent".to_owned()
    } else {
        agent_label(agent)
    };
    let notice = |kind, suffix: &str, body: String, opens| Notice {
        kind,
        id: format!("{}{suffix}", worktree.id),
        project: project.id.clone(),
        worktree: worktree.id.clone(),
        title: title.clone(),
        body: cut(&body, MOST_BODY),
        opens,
        heard: Vec::new(),
        prompt: None,
        who: who.clone(),
    };
    let chat = chat_of(worktree).map_or(Opens::Worktree, Opens::Chat);

    let was_activity = was.map(|was| was.activity);
    if activity == Activity::Blocked && was_activity != Some(Activity::Blocked) {
        let (body, prompt) = waiting(&who, worktree);
        news.push(Notice {
            prompt,
            ..notice(Kind::Waiting, "", body, chat)
        });
    }
    if let Some(was) = was {
        if moving(was.activity) && !was.agent.is_empty() && activity == Activity::Quiet {
            let said = if worktree.agent_state.is_empty() {
                ".".to_owned()
            } else {
                format!(": {}", worktree.agent_state)
            };
            news.push(notice(
                Kind::Finished,
                "/finished",
                format!("{who} finished{said}"),
                chat,
            ));
        }
        if was.activity != Activity::Failed && activity == Activity::Failed {
            news.push(notice(
                Kind::Failed,
                "/finished",
                format!("{who} failed."),
                chat,
            ));
        }
        if !was.checks_failed && checks_failed {
            let pr = worktree.pull_request.as_ref();
            let names: Vec<&str> = pr
                .map(|pr| {
                    pr.checks
                        .iter()
                        .filter(|check| check.state == check_summary::State::Failed as i32)
                        .map(|check| check.name.as_str())
                        .collect()
                })
                .unwrap_or_default();
            let number = pr.map_or_else(|| "?".to_owned(), |pr| pr.number.to_string());
            news.push(notice(
                Kind::Checks,
                "/checks",
                format!("Checks failed on #{number}: {}", names.join(", ")),
                Opens::PullRequest,
            ));
        }
    }
    news
}

/// What a wait says, and the prompt it names: the newest, when ket knows it.
fn waiting(who: &str, worktree: &proto::WorktreeSummary) -> (String, Option<u64>) {
    let question = worktree.question.as_ref();
    let subject = question
        .map(|question| question.subject.as_str())
        .filter(|subject| !subject.is_empty())
        .map_or_else(|| ".".to_owned(), |subject| format!(": {subject}"));
    let body = cut(&format!("{who} is waiting on you{subject}"), MOST_BODY);
    (
        body,
        question.map(|question| question.id).filter(|id| *id != 0),
    )
}

/// Whether `notice` is still so in `snapshot`, after its delay: the prompt
/// unanswered, the finished task unread, the checks still failing. A wait
/// whose prompt was answered while another keeps the worktree waiting is
/// re-worded for the one waiting now, rather than naming one already dealt
/// with, or saying nothing of the new one.
fn still(notice: &mut Notice, snapshot: &proto::HostSnapshot) -> bool {
    let Some(worktree) = snapshot
        .projects
        .iter()
        .flat_map(|project| &project.worktrees)
        .find(|worktree| worktree.id == notice.worktree)
    else {
        return false;
    };
    let activity = activity_of(worktree);
    match notice.kind {
        Kind::Waiting => {
            if activity != Activity::Blocked {
                return false;
            }
            if let Some(id) = notice.prompt {
                let asked = worktree
                    .questions
                    .iter()
                    .chain(worktree.question.as_ref())
                    .any(|question| question.id == id);
                if !asked {
                    let (body, prompt) = waiting(&notice.who, worktree);
                    notice.body = body;
                    notice.prompt = prompt;
                    notice.opens = chat_of(worktree).map_or(Opens::Worktree, Opens::Chat);
                }
            }
            true
        }
        // Read at the Mac meanwhile — its tab shown — or at work again.
        Kind::Finished => {
            !moving(activity) && worktree.terminals.iter().any(|terminal| terminal.unread)
        }
        Kind::Failed => activity == Activity::Failed,
        Kind::Checks => checks_failed(worktree),
    }
}

fn activity_of(worktree: &proto::WorktreeSummary) -> Activity {
    Activity::try_from(worktree.activity).unwrap_or(Activity::Quiet)
}

/// The phone's `MOVING`: at work, or waiting partway through.
fn moving(activity: Activity) -> bool {
    matches!(
        activity,
        Activity::Working | Activity::Running | Activity::Merging | Activity::Blocked
    )
}

fn checks_failed(worktree: &proto::WorktreeSummary) -> bool {
    worktree.pull_request.as_ref().is_some_and(|pr| {
        pr.checks
            .iter()
            .any(|check| check.state == check_summary::State::Failed as i32)
    })
}

/// The agent a worktree's notice names — the phone's `agentOf`: the one
/// whose state it reports, else the one in its agent terminal, else in its
/// first asleep tab. None while a program, not an agent, is busy in it.
fn agent_of(worktree: &proto::WorktreeSummary) -> Option<String> {
    // The phone's `programBusy`: another tab's agent beside "npm" would
    // say that agent was the one busy.
    if activity_of(worktree) == Activity::Running && worktree.agent.is_empty() {
        return None;
    }
    [
        worktree.agent.as_str(),
        agent_terminal(worktree).map_or("", |terminal| terminal.agent.as_str()),
        worktree.asleep.first().map_or("", |tab| tab.agent.as_str()),
    ]
    .into_iter()
    .find(|agent| !agent.is_empty())
    .map(str::to_owned)
}

/// The terminal running a worktree's reporting agent — the phone's
/// `agentTerminal`. A tab in Claude's agent view runs `claude` too, but what
/// the worktree reports comes from another: it is the last choice.
fn agent_terminal(worktree: &proto::WorktreeSummary) -> Option<&proto::TerminalSummary> {
    let open = || {
        worktree
            .terminals
            .iter()
            .filter(|terminal| !terminal.closed && !terminal.agent.is_empty())
    };
    let ranked: Vec<&proto::TerminalSummary> = open()
        .filter(|terminal| terminal.agent_view.is_none())
        .chain(open().filter(|terminal| terminal.agent_view.is_some()))
        .collect();
    ranked
        .iter()
        .find(|terminal| {
            !worktree.agent.is_empty() && terminal.agent.eq_ignore_ascii_case(&worktree.agent)
        })
        .or_else(|| ranked.first())
        .copied()
}

/// The chat a tap opens: the newest prompt's terminal, else the agent's.
fn chat_of(worktree: &proto::WorktreeSummary) -> Option<u64> {
    let asking = worktree
        .questions
        .first()
        .or(worktree.question.as_ref())
        .map(|question| question.terminal_id)
        .filter(|id| *id != 0);
    asking.or_else(|| agent_terminal(worktree).map(|terminal| terminal.id))
}

/// An agent's name as the phone shows it — its `agentLabel`.
fn agent_label(agent: &str) -> String {
    let name = agent.trim();
    if let Some(model) = name
        .to_ascii_lowercase()
        .strip_prefix("opencode:ollama/")
        .map(|_| &name["opencode:ollama/".len()..])
    {
        return format!("{model} \u{b7} OpenCode");
    }
    match name.to_ascii_lowercase().as_str() {
        "claude" => "Claude".to_owned(),
        "codex" => "Codex".to_owned(),
        "opencode" => "OpenCode".to_owned(),
        "gemini" => "Gemini".to_owned(),
        "grok" => "Grok".to_owned(),
        _ => {
            let mut chars = name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// `text` cut to `most` characters, with an ellipsis when it was longer.
fn cut(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(most.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}
