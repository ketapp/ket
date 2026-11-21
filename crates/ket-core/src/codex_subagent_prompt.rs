//! A Codex subagent's permission prompt, put up as its tab's own.
//!
//! Codex draws a subagent's approval over whichever thread the tab is
//! showing, as the same menu its lead's prompts use: `y` allows, Escape
//! cancels the request and ends the subagent's turn, and `o` opens the
//! subagent's thread. ket filed the prompt under the subagent's row alone,
//! so it was on screen and on no phone: no card, no notice. A message typed
//! from the phone meanwhile landed on the menu, and its `o` left the tab in
//! the subagent's thread, which takes no input.
//!
//! So while one is up it stands as the tab's prompt, the lead's own reports
//! wait, and once it is answered the lead is back where it was. A refusal
//! ends the subagent's turn, never the lead's. One prompt to a tab, the one
//! Codex has on screen: a subagent's asked while the lead's is up waits in
//! the terminal.

use crate::agent_hooks::{HookEvent, HookReport, PermissionQuestion};
use crate::agent_status::{
    Answered, PROMPT_SIBLINGS_MS, PaneAgentStatus, answers, next_question_id,
};
use crate::event::AgentState;

/// The subagent whose prompt is up as its tab's own, and what the lead was
/// doing when it went up.
#[derive(Debug, Clone)]
pub(crate) struct Asker {
    child: String,
    /// The prompt's id: one that has since replaced it is not the subagent's.
    question: u64,
    state: AgentState,
    note: Option<String>,
    mutating_git: bool,
    /// A prompt of the lead's own, which Codex queues behind the one on
    /// screen and shows once it is answered.
    queued: Option<PermissionQuestion>,
    /// Missing from the last screen read — see [`screen`].
    unseen: bool,
}

/// Takes a report subagent `child` sent: its prompt going up, or word that
/// it has gone.
pub(crate) fn child(entry: &mut PaneAgentStatus, child: &str, report: &HookReport) {
    if !crate::agents::harness(&report.agent).eq_ignore_ascii_case("codex") {
        return;
    }
    if asker(entry).is_some_and(|asker| asker.child == child)
        && let Some(asked) = entry.question.clone()
    {
        let held = report
            .call
            .as_deref()
            .is_some_and(|call| answers(&asked, call));
        // Gone, and how it was answered where that is said.
        let allowed = match report.event {
            HookEvent::PermissionDenied => Some(false),
            HookEvent::PreTool | HookEvent::PostTool | HookEvent::PostToolFailure if held => {
                Some(true)
            }
            // Its next call, which it makes only once the prompt is gone.
            HookEvent::PreTool
                if report.at_ms.saturating_sub(entry.at_ms) >= PROMPT_SIBLINGS_MS =>
            {
                None
            }
            HookEvent::PermissionRequest | HookEvent::SubagentStop => None,
            _ => return,
        };
        tracing::info!(
            pane = %entry.pane,
            agent = %entry.agent,
            event = ?report.event,
            tool = %asked.tool,
            id = asked.id,
            subagent = child,
            "prompt cleared by a report"
        );
        if let Some(allowed) = allowed {
            entry.answered = Some(Answered {
                question: asked,
                allowed,
                at_ms: report.at_ms,
            });
        }
        resume(entry, report.at_ms);
    }

    if report.event == HookEvent::PermissionRequest
        && entry.question.is_none()
        && entry.state != AgentState::AwaitingPermission
        && let Some(question) = report.question.as_deref()
    {
        let mut question = question.clone();
        question.id = next_question_id();
        tracing::info!(
            pane = %entry.pane,
            agent = %entry.agent,
            tool = %question.tool,
            id = question.id,
            subagent = child,
            "prompt up"
        );
        entry.codex_asker = Some(Asker {
            child: child.to_owned(),
            question: question.id,
            state: entry.state,
            note: entry.note.take(),
            mutating_git: std::mem::take(&mut entry.mutating_git),
            queued: None,
            unseen: false,
        });
        entry.question = Some(question);
        entry.state = AgentState::AwaitingPermission;
        entry.at_ms = report.at_ms;
    }
}

/// Holds the lead's `report` while a subagent's prompt is up, and says
/// whether it did. The lead polls its subagent meanwhile, and a call of its
/// own starting or finishing says nothing about a prompt that is not its
/// own. A prompt of its own is kept for when the subagent's is answered:
/// put up now, a phone's answer would land on the subagent's.
pub(crate) fn lead(entry: &mut PaneAgentStatus, report: &HookReport) -> bool {
    if asker(entry).is_none() {
        return false;
    }
    match report.event {
        HookEvent::PreTool | HookEvent::PostTool | HookEvent::PostToolFailure => {}
        HookEvent::PermissionRequest => {
            if let (Some(asker), Some(question)) =
                (entry.codex_asker.as_mut(), report.question.as_deref())
            {
                asker.queued.get_or_insert_with(|| question.clone());
            }
        }
        _ => return false,
    }
    tracing::debug!(
        pane = %entry.pane,
        event = ?report.event,
        "the lead's report while a subagent's prompt is up"
    );
    true
}

/// A phone answered the tab's prompt. When it was a subagent's, puts the
/// lead back where it was and returns true.
pub(crate) fn answered(entry: &mut PaneAgentStatus, allowed: bool, now_ms: u64) -> bool {
    let Some(child) = asker(entry).map(|asker| asker.child.clone()) else {
        return false;
    };
    let Some(question) = entry.question.clone() else {
        return false;
    };
    tracing::info!(
        pane = %entry.pane,
        agent = %entry.agent,
        tool = %question.tool,
        allowed,
        subagent = child,
        "prompt answered from a phone"
    );
    entry.answered = Some(Answered {
        question,
        allowed,
        at_ms: now_ms,
    });
    resume(entry, now_ms);
    true
}

/// Takes down a subagent's prompt the screen no longer shows, and says
/// whether it did.
///
/// Codex says nothing when one is declined at the desk, or the subagent's
/// turn is interrupted under it by the person or its lead. With the lead's
/// reports held meanwhile, the tab read "waiting" and a phone offered a
/// prompt that was gone: its `y` typed into the lead's input, its Escape
/// interrupted the lead. Codex heads a subagent's approval `Thread: <name>`.
/// Missing from two reads in a row, once the prompt has had time to be
/// drawn, it is gone.
pub(crate) fn screen(entry: &mut PaneAgentStatus, rows: &[String], now_ms: u64) -> bool {
    if asker(entry).is_none() || now_ms.saturating_sub(entry.at_ms) < PROMPT_SIBLINGS_MS {
        return false;
    }
    let shown = rows
        .iter()
        .any(|row| row.trim_start().starts_with("Thread: "));
    let Some(asker) = entry.codex_asker.as_mut() else {
        return false;
    };
    if shown {
        asker.unseen = false;
        return false;
    }
    if !std::mem::replace(&mut asker.unseen, true) {
        return false;
    }
    tracing::info!(
        pane = %entry.pane,
        agent = %entry.agent,
        subagent = %asker.child,
        why = "the screen no longer shows it",
        "prompt cleared"
    );
    resume(entry, now_ms);
    true
}

/// The asker, while its prompt is still the tab's.
fn asker(entry: &PaneAgentStatus) -> Option<&Asker> {
    let asker = entry.codex_asker.as_ref()?;
    entry
        .question
        .as_ref()
        .is_some_and(|question| question.id == asker.question)
        .then_some(asker)
}

/// The lead where it was before the subagent asked, or on its own prompt
/// if it asked meanwhile.
fn resume(entry: &mut PaneAgentStatus, at_ms: u64) {
    let Some(asker) = entry.codex_asker.take() else {
        return;
    };
    entry.question = None;
    entry.state = asker.state;
    entry.note = asker.note;
    entry.mutating_git = asker.mutating_git;
    entry.at_ms = at_ms;
    if let Some(mut question) = asker.queued {
        question.id = next_question_id();
        tracing::info!(
            pane = %entry.pane,
            agent = %entry.agent,
            tool = %question.tool,
            id = question.id,
            "prompt up"
        );
        entry.question = Some(question);
        entry.state = AgentState::AwaitingPermission;
    }
}

#[cfg(test)]
mod tests {
    use crate::agent_hooks::{HookEvent, HookReport, PermissionQuestion};
    use crate::agent_status::StatusStore;
    use crate::event::AgentState;

    /// `event` from Codex in pane `p1`: the lead's, or subagent `kid`'s.
    fn codex(event: HookEvent, kid: bool, at_ms: u64) -> HookReport {
        let mut report: HookReport = serde_json::from_value(serde_json::json!({
            "agent": "codex",
            "event": "stop",
            "worktree": "w1",
            "pane": "p1",
            "session": if kid { "s2" } else { "s1" },
            "mutatingGit": false,
            "atMs": at_ms,
        }))
        .unwrap();
        report.event = event;
        report.subagent = kid.then(|| "kid".to_owned());
        report
    }

    fn bash(subject: &str) -> Option<Box<PermissionQuestion>> {
        Some(Box::new(PermissionQuestion {
            tool: "Bash".to_owned(),
            subject: Some(subject.to_owned()),
            id: 0,
            prompt: None,
        }))
    }

    fn asks(kid: bool, subject: &str, at_ms: u64) -> HookReport {
        let mut report = codex(HookEvent::PermissionRequest, kid, at_ms);
        report.question = bash(subject);
        report
    }

    /// `tool` called on `subject`, starting or finishing.
    fn calls(event: HookEvent, kid: bool, tool: &str, subject: &str, at_ms: u64) -> HookReport {
        let mut report = codex(event, kid, at_ms);
        report.tool_name = Some(tool.to_owned());
        report.call = bash(subject).map(|mut call| {
            call.tool = tool.to_owned();
            call
        });
        report
    }

    /// A lead waiting on subagent `kid`, which has asked to run `claude`.
    fn asked() -> StatusStore {
        let mut store = StatusStore::new(1);
        let mut wait = calls(HookEvent::PreTool, false, "wait_agent", "", 0);
        wait.note = Some("waiting on its subagent".to_owned());
        store.report(&wait, |_| true);
        store.report(&asks(true, "claude --print", 100), |_| true);
        store
    }

    #[test]
    fn a_subagents_prompt_is_the_tabs_until_its_call_comes_back() {
        let mut store = asked();
        let pane = store.get("p1").unwrap();
        assert_eq!(pane.state, AgentState::AwaitingPermission);
        let id = pane.question.as_ref().unwrap().id;

        // The lead polling meanwhile answers nothing, however long after.
        store.report(
            &calls(HookEvent::PreTool, false, "wait_agent", "", 60_000),
            |_| true,
        );
        assert_eq!(store.get("p1").unwrap().question.as_ref().unwrap().id, id);

        store.report(
            &calls(HookEvent::PostTool, true, "Bash", "claude --print", 70_000),
            |_| true,
        );
        let pane = store.get("p1").unwrap();
        assert_eq!(pane.state, AgentState::ExecutingTool);
        assert_eq!(pane.note.as_deref(), Some("waiting on its subagent"));
        assert!(pane.question.is_none());
        assert!(pane.answered.as_ref().unwrap().allowed);
    }

    #[test]
    fn its_next_call_means_it_was_answered_at_the_desk() {
        let mut store = asked();
        // Dispatched beside the call that asked: still up.
        store.report(&calls(HookEvent::PreTool, true, "Bash", "ls", 200), |_| {
            true
        });
        assert_eq!(
            store.get("p1").unwrap().state,
            AgentState::AwaitingPermission
        );

        store.report(
            &calls(HookEvent::PreTool, true, "Bash", "ls", 9_000),
            |_| true,
        );
        let pane = store.get("p1").unwrap();
        assert_eq!(pane.state, AgentState::ExecutingTool);
        assert!(pane.question.is_none());
        assert!(pane.answered.is_none(), "nobody said how");
    }

    #[test]
    fn declined_at_the_desk_it_comes_down_once_the_screen_stops_showing_it() {
        let mut store = asked();
        let menu = [
            "  Thread: Schrodinger".to_owned(),
            "› 1. Yes, proceed (y)".to_owned(),
        ];
        let gone = ["› ".to_owned()];

        // Not drawn yet, then on screen: up.
        assert!(!store.codex_subagent_screen("p1", &gone, 500));
        assert!(!store.codex_subagent_screen("p1", &menu, 3_000));
        // Escape at the desk: Codex says nothing. One read could catch a
        // frame half drawn; two in a row cannot.
        assert!(!store.codex_subagent_screen("p1", &gone, 4_000));
        assert!(store.codex_subagent_screen("p1", &gone, 5_000));
        let pane = store.get("p1").unwrap();
        assert_eq!(pane.state, AgentState::ExecutingTool);
        assert!(pane.question.is_none());
    }

    #[test]
    fn a_phones_answer_leaves_the_lead_working_either_way() {
        for allowed in [true, false] {
            let mut store = asked();
            let id = store.get("p1").unwrap().question.as_ref().unwrap().id;
            assert!(store.answered("p1", id, allowed, 200));
            let pane = store.get("p1").unwrap();
            assert_eq!(pane.state, AgentState::ExecutingTool, "allowed={allowed}");
            assert!(pane.question.is_none());
            assert_eq!(pane.answered.as_ref().unwrap().allowed, allowed);
        }
    }

    #[test]
    fn the_leads_own_prompt_waits_behind_it_and_comes_up_after() {
        let mut store = asked();
        store.report(&asks(false, "git push", 300), |_| true);
        let first = store.get("p1").unwrap().question.clone().unwrap();
        assert_eq!(first.subject.as_deref(), Some("claude --print"));

        assert!(store.answered("p1", first.id, true, 400));
        let pane = store.get("p1").unwrap();
        assert_eq!(pane.state, AgentState::AwaitingPermission);
        assert_eq!(
            pane.question.as_ref().unwrap().subject.as_deref(),
            Some("git push")
        );
    }

    #[test]
    fn claudes_subagents_and_a_second_prompt_are_left_alone() {
        // The lead's own prompt is up: a subagent's waits in the terminal.
        let mut store = StatusStore::new(1);
        store.report(&asks(false, "git push", 0), |_| true);
        store.report(&asks(true, "claude --print", 100), |_| true);
        assert_eq!(
            store
                .get("p1")
                .unwrap()
                .question
                .as_ref()
                .unwrap()
                .subject
                .as_deref(),
            Some("git push")
        );

        // Claude's subagent stays a row that reads blocked.
        let mut store = StatusStore::new(1);
        let mut kid = asks(true, "cargo test", 100);
        kid.agent = "claude".to_owned();
        store.report(&kid, |_| true);
        assert!(store.get("p1").unwrap().question.is_none());
    }
}
