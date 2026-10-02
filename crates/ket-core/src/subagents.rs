//! The subagents a worktree's agent has running, for the rows drawn under it.
//!
//! Keyed by pane, and by the `agent_id` every hook fired inside a subagent
//! carries, which the lead's own events never do. A subagent appears on
//! `SubagentStart` or on the first event it sends, leaves on its
//! `SubagentStop`, and is reconciled against the `background_tasks` inventory
//! Claude attaches to the lead's `Stop` — the only way to learn about a stop
//! that was never delivered.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::agent_hooks::{BackgroundTask, HookEvent, HookReport};

/// Most subagents one pane tracks; past it a row is one nobody reads.
pub const MAX_SUBAGENTS: usize = 32;

/// Longest `agent_id` believed. Anything longer is not an id either agent mints.
const MAX_ID_LEN: usize = 64;

/// What a subagent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentState {
    /// Running.
    Working,
    /// Stopped on a permission prompt, which the person answers in the lead's
    /// pane.
    Blocked,
    /// A teammate between turns: alive and resumable, doing nothing.
    Idle,
}

/// One subagent, as a row needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subagent {
    /// The agent's own id for it.
    pub id: String,
    /// What it was sent to do, else its kind.
    pub label: String,
    /// Its model, where the agent says.
    pub model: Option<String>,
    /// What it is doing.
    pub state: SubagentState,
    /// When ket first heard of it.
    pub started_at_ms: u64,
    /// The pane its lead runs in.
    pub pane: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tracked {
    agent_type: Option<String>,
    description: Option<String>,
    model: Option<String>,
    state: SubagentState,
    /// The call a prompt is holding, so an unrelated call finishing does not
    /// read as the prompt being answered.
    blocked_tool: Option<String>,
    started_at_ms: u64,
    last_at_ms: u64,
    /// Listed by id in a `background_tasks` inventory, which makes a later
    /// inventory that omits it proof it has finished.
    listed: bool,
    /// Named by a `TeammateIdle`: a persistent teammate, not a one-shot lane
    /// that happens to share the id shape.
    teammate: bool,
}

/// The fields a report can add to what is known about a subagent.
struct Fields<'a> {
    agent_type: Option<&'a str>,
    description: Option<&'a str>,
    model: Option<&'a str>,
}

/// One pane's subagents.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Roster {
    /// The lead's session. Set and compared only from the lead's own reports:
    /// a Codex subagent runs as a thread with a session id of its own.
    session: Option<String>,
    children: HashMap<String, Tracked>,
}

impl Roster {
    /// Takes a report one of the subagents sent.
    pub(crate) fn child(&mut self, id: &str, report: &HookReport) {
        if report.event == HookEvent::SubagentStop {
            self.stop(id);
            return;
        }

        let fields = Fields {
            agent_type: report.subagent_type.as_deref(),
            description: report.subagent_description.as_deref(),
            model: report.subagent_model.as_deref(),
        };
        let Some(tracked) = self.upsert(id, fields, report.at_ms) else {
            return;
        };

        match report.event {
            HookEvent::PermissionRequest | HookEvent::Notification => {
                if tracked.state != SubagentState::Blocked || report.tool_use_id.is_some() {
                    tracked.blocked_tool = report.tool_use_id.clone();
                }
                tracked.state = SubagentState::Blocked;
            }
            // The prompt was answered once the call it held comes back. Any
            // other call finishing says nothing: a subagent's read-only calls
            // run beside the one waiting on a person.
            HookEvent::PostTool | HookEvent::PostToolFailure | HookEvent::PermissionDenied => {
                let answered = tracked.state != SubagentState::Blocked
                    || report.event == HookEvent::PermissionDenied
                    || tracked.blocked_tool.is_none()
                    || report.tool_use_id.is_none()
                    || tracked.blocked_tool == report.tool_use_id;
                if answered {
                    tracked.state = SubagentState::Working;
                    tracked.blocked_tool = None;
                }
            }
            _ => {
                if tracked.state != SubagentState::Blocked {
                    tracked.state = SubagentState::Working;
                }
            }
        }
    }

    /// Takes a report the lead sent about itself.
    pub(crate) fn lead(&mut self, report: &HookReport) {
        let replaced =
            report.session.is_some() && self.session.is_some() && report.session != self.session;
        if report.fresh_session || report.event == HookEvent::SessionEnd || replaced {
            self.children.clear();
        }
        if report.session.is_some() {
            self.session.clone_from(&report.session);
        }

        match report.event {
            HookEvent::TeammateIdle => {
                if let Some(name) = report.teammate.as_deref() {
                    self.idle_teammate(name);
                }
            }
            HookEvent::Stop => match &report.background {
                Some(tasks) => self.fold(tasks, report.at_ms),
                // No inventory — Codex never sends one — so nothing vouches
                // for what is left, and a missed stop would otherwise keep a
                // row forever. A background subagent that is still going comes
                // back with its next event.
                None => self.children.clear(),
            },
            // Codex's cancelled turn carries no background inventory. Nothing
            // vouches for children left over from it; a survivor will report
            // again and recreate its row.
            HookEvent::Interrupt => self.children.clear(),
            _ => {}
        }
    }

    /// Forgets every subagent, as when the turn that started them is
    /// cancelled.
    pub(crate) fn clear(&mut self) {
        self.children.clear();
    }

    /// Whether one of them is waiting on a person.
    pub(crate) fn blocked(&self) -> bool {
        self.children
            .values()
            .any(|t| t.state == SubagentState::Blocked)
    }

    /// When any of them was last heard from.
    pub(crate) fn heard_at_ms(&self) -> Option<u64> {
        self.children.values().map(|t| t.last_at_ms).max()
    }

    /// Each one, as a row needs it.
    pub(crate) fn rows<'a>(&'a self, pane: &'a str) -> impl Iterator<Item = Subagent> + 'a {
        self.children.iter().map(move |(id, t)| Subagent {
            id: id.clone(),
            label: t
                .description
                .clone()
                .or_else(|| t.agent_type.clone())
                .unwrap_or_else(|| "subagent".to_owned()),
            model: t.model.clone(),
            state: t.state,
            started_at_ms: t.started_at_ms,
            pane: (!pane.is_empty()).then(|| pane.to_owned()),
        })
    }

    /// Adds a subagent or refreshes one, leaving its state to the caller.
    fn upsert(&mut self, id: &str, fields: Fields<'_>, at_ms: u64) -> Option<&mut Tracked> {
        if id.is_empty() || id.len() > MAX_ID_LEN {
            return None;
        }
        // Never displace one that is working: only a parked teammate gives up
        // its place.
        if !self.children.contains_key(id)
            && self.children.len() >= MAX_SUBAGENTS
            && !self.evict_oldest_idle()
        {
            return None;
        }

        let tracked = self.children.entry(id.to_owned()).or_insert(Tracked {
            agent_type: None,
            description: None,
            model: None,
            state: SubagentState::Working,
            blocked_tool: None,
            started_at_ms: at_ms,
            last_at_ms: at_ms,
            listed: false,
            teammate: false,
        });
        let fill = |slot: &mut Option<String>, value: Option<&str>| {
            if let Some(value) = value {
                *slot = Some(value.to_owned());
            }
        };
        fill(&mut tracked.agent_type, fields.agent_type);
        fill(&mut tracked.description, fields.description);
        fill(&mut tracked.model, fields.model);
        tracked.last_at_ms = tracked.last_at_ms.max(at_ms);
        Some(tracked)
    }

    fn evict_oldest_idle(&mut self) -> bool {
        let oldest = self
            .children
            .iter()
            .filter(|(_, t)| t.state == SubagentState::Idle)
            .min_by_key(|(_, t)| t.started_at_ms)
            .map(|(id, _)| id.clone());
        oldest.is_some_and(|id| self.children.remove(&id).is_some())
    }

    /// A one-shot subagent has finished and its row goes. A teammate has only
    /// finished a turn, and parks.
    fn stop(&mut self, id: &str) {
        let Some(tracked) = self.children.get_mut(id) else {
            return;
        };
        if !is_teammate_id(id) || tracked.listed {
            self.children.remove(id);
            return;
        }
        tracked.state = SubagentState::Idle;
        tracked.blocked_tool = None;
    }

    fn idle_teammate(&mut self, name: &str) {
        let prefix = format!("a{name}-");
        for (id, tracked) in &mut self.children {
            // A hyphen-free rest, so teammate "rev" does not match "rev-two".
            if id
                .strip_prefix(&prefix)
                .is_some_and(|rest| !rest.contains('-'))
            {
                tracked.state = SubagentState::Idle;
                tracked.blocked_tool = None;
                tracked.teammate = true;
            }
        }
    }

    /// Reconciles against the inventory on the lead's `Stop`.
    ///
    /// Authoritative for subagent entries: a foreground subagent cannot outlive
    /// the turn, a running background one is listed under its lifecycle id, and
    /// a finished one drops out. Teammate entries prove nothing per agent, but
    /// their presence means a teammate-shaped id missing from the list may
    /// still be alive.
    fn fold(&mut self, tasks: &[BackgroundTask], at_ms: u64) {
        if tasks.is_empty() {
            self.children.clear();
            return;
        }

        let has_teammates = tasks.iter().any(|t| t.teammate);
        let mut listed = HashSet::new();
        let mut arrived = Vec::new();
        for task in tasks.iter().filter(|t| !t.teammate) {
            listed.insert(task.id.as_str());
            if !task.running {
                self.children.remove(&task.id);
                continue;
            }
            match self.children.get_mut(&task.id) {
                Some(tracked) => {
                    if tracked.state == SubagentState::Idle {
                        tracked.state = SubagentState::Working;
                    }
                    if task.agent_type.is_some() {
                        tracked.agent_type.clone_from(&task.agent_type);
                    }
                    if task.description.is_some() {
                        tracked.description.clone_from(&task.description);
                    }
                    tracked.listed = true;
                }
                None => arrived.push(task),
            }
        }

        self.children.retain(|id, t| {
            listed.contains(id.as_str())
                || (has_teammates
                    && !t.listed
                    && is_teammate_id(id)
                    && (t.state != SubagentState::Idle || t.teammate))
        });

        // One this listener never saw start — ket was restarted mid-run.
        for task in arrived {
            let fields = Fields {
                agent_type: task.agent_type.as_deref(),
                description: task.description.as_deref(),
                model: None,
            };
            if let Some(tracked) = self.upsert(&task.id, fields, at_ms) {
                tracked.listed = true;
            }
        }
    }
}

/// Whether an id is a Claude teammate's `a<name>-<hex>` rather than a one-shot
/// subagent's hyphen-free `a<hex>`, or a Codex thread's UUID — which can start
/// with an `a` and end in hex just the same.
fn is_teammate_id(id: &str) -> bool {
    let uuid = id.len() == 36
        && id
            .char_indices()
            .all(|(at, c)| matches!(at, 8 | 13 | 18 | 23) == (c == '-'));
    !uuid
        && id.rfind('-').is_some_and(|at| {
            at > 1
                && id.starts_with('a')
                && id.len() > at + 1
                && id[at + 1..].chars().all(|c| c.is_ascii_hexdigit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(event: HookEvent, subagent: Option<&str>, at_ms: u64) -> HookReport {
        HookReport {
            agent: "claude".to_owned(),
            event,
            worktree: None,
            cwd: None,
            pane: None,
            session: None,
            tool_name: None,
            note: None,
            mutating_git: false,
            at_ms,
            subagent: subagent.map(str::to_owned),
            subagent_type: None,
            subagent_description: None,
            subagent_model: None,
            tool_use_id: None,
            call: None,
            teammate: None,
            fresh_session: false,
            background: None,
            question: None,
        }
    }

    fn task(id: &str, running: bool, teammate: bool) -> BackgroundTask {
        BackgroundTask {
            id: id.to_owned(),
            agent_type: None,
            description: None,
            running,
            teammate,
        }
    }

    fn rows(roster: &Roster) -> Vec<Subagent> {
        roster.rows("pane-1").collect()
    }

    #[test]
    fn a_first_report_adds_a_working_subagent() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));

        let rows = rows(&roster);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a1a2b3");
        assert_eq!(rows[0].state, SubagentState::Working);
        assert_eq!(rows[0].label, "subagent", "no description or type given");
    }

    #[test]
    fn description_wins_over_agent_type_for_the_label() {
        let mut roster = Roster::default();
        let mut with_type = report(HookEvent::PreTool, Some("a1a2b3"), 100);
        with_type.subagent_type = Some("Explore".to_owned());
        roster.child("a1a2b3", &with_type);
        assert_eq!(rows(&roster)[0].label, "Explore");

        let mut with_description = report(HookEvent::PostTool, Some("a1a2b3"), 200);
        with_description.subagent_description = Some("hunt the bug".to_owned());
        roster.child("a1a2b3", &with_description);
        assert_eq!(rows(&roster)[0].label, "hunt the bug");
    }

    #[test]
    fn a_permission_request_blocks_the_subagent() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));

        let mut ask = report(HookEvent::PermissionRequest, Some("a1a2b3"), 200);
        ask.tool_use_id = Some("call-1".to_owned());
        roster.child("a1a2b3", &ask);

        assert_eq!(rows(&roster)[0].state, SubagentState::Blocked);
        assert!(roster.blocked());
    }

    #[test]
    fn the_matching_tool_call_finishing_unblocks_it() {
        let mut roster = Roster::default();
        let mut ask = report(HookEvent::PermissionRequest, Some("a1a2b3"), 100);
        ask.tool_use_id = Some("call-1".to_owned());
        roster.child("a1a2b3", &ask);

        let mut done = report(HookEvent::PostTool, Some("a1a2b3"), 200);
        done.tool_use_id = Some("call-1".to_owned());
        roster.child("a1a2b3", &done);

        assert_eq!(rows(&roster)[0].state, SubagentState::Working);
        assert!(!roster.blocked());
    }

    #[test]
    fn an_unrelated_tool_call_finishing_does_not_unblock_it() {
        let mut roster = Roster::default();
        let mut ask = report(HookEvent::PermissionRequest, Some("a1a2b3"), 100);
        ask.tool_use_id = Some("call-1".to_owned());
        roster.child("a1a2b3", &ask);

        // A read-only call that ran alongside the blocked one finishes first.
        let mut done = report(HookEvent::PostTool, Some("a1a2b3"), 200);
        done.tool_use_id = Some("call-2".to_owned());
        roster.child("a1a2b3", &done);

        assert_eq!(rows(&roster)[0].state, SubagentState::Blocked);
    }

    #[test]
    fn a_denied_permission_unblocks_regardless_of_which_call_it_names() {
        let mut roster = Roster::default();
        let mut ask = report(HookEvent::PermissionRequest, Some("a1a2b3"), 100);
        ask.tool_use_id = Some("call-1".to_owned());
        roster.child("a1a2b3", &ask);

        roster.child(
            "a1a2b3",
            &report(HookEvent::PermissionDenied, Some("a1a2b3"), 200),
        );

        assert_eq!(rows(&roster)[0].state, SubagentState::Working);
    }

    #[test]
    fn a_one_shot_subagent_is_dropped_on_stop() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));
        roster.child(
            "a1a2b3",
            &report(HookEvent::SubagentStop, Some("a1a2b3"), 200),
        );
        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn a_uuid_shaped_codex_thread_is_dropped_on_stop_not_idled() {
        let mut roster = Roster::default();
        let id = "550e8400-e29b-41d4-a716-446655440000";
        roster.child(id, &report(HookEvent::PreTool, Some(id), 100));
        roster.child(id, &report(HookEvent::SubagentStop, Some(id), 200));
        assert!(
            rows(&roster).is_empty(),
            "a UUID id must not be mistaken for a teammate"
        );
    }

    #[test]
    fn a_teammate_shaped_id_parks_idle_on_stop_instead_of_vanishing() {
        let mut roster = Roster::default();
        roster.child(
            "arev-1a2b3c",
            &report(HookEvent::PreTool, Some("arev-1a2b3c"), 100),
        );
        roster.child(
            "arev-1a2b3c",
            &report(HookEvent::SubagentStop, Some("arev-1a2b3c"), 200),
        );
        let rows = rows(&roster);
        assert_eq!(rows.len(), 1, "a teammate parks rather than disappearing");
        assert_eq!(rows[0].state, SubagentState::Idle);
    }

    #[test]
    fn teammate_idle_only_idles_the_named_teammates_exact_ids() {
        let mut roster = Roster::default();
        roster.child(
            "arev-1a2b3c",
            &report(HookEvent::PreTool, Some("arev-1a2b3c"), 100),
        );
        // A different teammate whose name happens to share the prefix.
        roster.child(
            "arev-two-1a2b3c",
            &report(HookEvent::PreTool, Some("arev-two-1a2b3c"), 100),
        );

        let mut idle = report(HookEvent::TeammateIdle, None, 200);
        idle.teammate = Some("rev".to_owned());
        roster.lead(&idle);

        let states: std::collections::HashMap<String, SubagentState> =
            rows(&roster).into_iter().map(|s| (s.id, s.state)).collect();
        assert_eq!(states["arev-1a2b3c"], SubagentState::Idle);
        assert_eq!(
            states["arev-two-1a2b3c"],
            SubagentState::Working,
            "a hyphenated rest must not match a shorter teammate name"
        );
    }

    #[test]
    fn a_fresh_session_clears_every_child() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));

        let mut fresh = report(HookEvent::SessionStart, None, 200);
        fresh.fresh_session = true;
        roster.lead(&fresh);

        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn session_end_clears_every_child() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));
        roster.lead(&report(HookEvent::SessionEnd, None, 200));
        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn a_session_id_change_clears_every_child() {
        let mut roster = Roster::default();
        let mut first = report(HookEvent::SessionStart, None, 100);
        first.session = Some("s1".to_owned());
        roster.lead(&first);
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));

        let mut second = report(HookEvent::UserPrompt, None, 200);
        second.session = Some("s2".to_owned());
        roster.lead(&second);

        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn a_stop_with_no_inventory_clears_every_child() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));
        roster.lead(&report(HookEvent::Stop, None, 200));
        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn an_interrupt_clears_every_child() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));
        roster.lead(&report(HookEvent::Interrupt, None, 200));
        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn an_empty_inventory_on_stop_clears_every_child() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));

        let mut stop = report(HookEvent::Stop, None, 200);
        stop.background = Some(Vec::new());
        roster.lead(&stop);

        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn a_finished_task_in_the_inventory_is_dropped() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));

        let mut stop = report(HookEvent::Stop, None, 200);
        stop.background = Some(vec![task("a1a2b3", false, false)]);
        roster.lead(&stop);

        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn a_running_task_ket_never_saw_start_arrives_from_the_inventory() {
        let mut roster = Roster::default();
        let mut stop = report(HookEvent::Stop, None, 200);
        stop.background = Some(vec![task("a-arrived", true, false)]);
        roster.lead(&stop);

        let rows = rows(&roster);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a-arrived");
    }

    #[test]
    fn an_idle_teammate_flagged_by_the_inventory_survives_while_the_team_exists() {
        let mut roster = Roster::default();
        roster.child(
            "arev-1a2b3c",
            &report(HookEvent::PreTool, Some("arev-1a2b3c"), 100),
        );
        let mut idle = report(HookEvent::TeammateIdle, None, 150);
        idle.teammate = Some("rev".to_owned());
        roster.lead(&idle);
        assert_eq!(rows(&roster)[0].state, SubagentState::Idle);

        // The inventory names a teammate (of any id) but not this one by id —
        // the parked teammate should still survive because teams exist.
        let mut stop = report(HookEvent::Stop, None, 200);
        stop.background = Some(vec![task("a-someone-else", true, true)]);
        roster.lead(&stop);

        assert_eq!(rows(&roster).len(), 1, "the idled teammate is not dropped");
    }

    #[test]
    fn an_idle_teammate_is_dropped_once_no_team_is_reported() {
        let mut roster = Roster::default();
        roster.child(
            "arev-1a2b3c",
            &report(HookEvent::PreTool, Some("arev-1a2b3c"), 100),
        );
        let mut idle = report(HookEvent::TeammateIdle, None, 150);
        idle.teammate = Some("rev".to_owned());
        roster.lead(&idle);

        // An inventory with no teammate entries at all: the team is gone.
        let mut stop = report(HookEvent::Stop, None, 200);
        stop.background = Some(vec![task("a1a2b3d4", true, false)]);
        roster.lead(&stop);

        assert!(rows(&roster).iter().all(|s| s.id != "arev-1a2b3c"));
    }

    #[test]
    fn heard_at_ms_is_the_latest_report_of_any_child() {
        let mut roster = Roster::default();
        assert_eq!(roster.heard_at_ms(), None);

        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));
        roster.child("a4d5e6", &report(HookEvent::PreTool, Some("a4d5e6"), 300));
        roster.child("a1a2b3", &report(HookEvent::PostTool, Some("a1a2b3"), 150));

        assert_eq!(roster.heard_at_ms(), Some(300));
    }

    #[test]
    fn clear_forgets_everything() {
        let mut roster = Roster::default();
        roster.child("a1a2b3", &report(HookEvent::PreTool, Some("a1a2b3"), 100));
        roster.clear();
        assert!(rows(&roster).is_empty());
        assert_eq!(roster.heard_at_ms(), None);
    }

    #[test]
    fn an_id_that_is_empty_or_too_long_is_rejected() {
        let mut roster = Roster::default();
        roster.child("", &report(HookEvent::PreTool, Some(""), 100));
        assert!(rows(&roster).is_empty());

        let too_long = "a".repeat(MAX_ID_LEN + 1);
        roster.child(&too_long, &report(HookEvent::PreTool, Some(&too_long), 100));
        assert!(rows(&roster).is_empty());
    }

    #[test]
    fn a_full_roster_evicts_the_oldest_idle_child_to_make_room() {
        let mut roster = Roster::default();

        // A teammate, parked idle first so it is the only eviction candidate.
        let victim = "arev-1a2b3c";
        roster.child(victim, &report(HookEvent::PreTool, Some(victim), 0));
        roster.child(victim, &report(HookEvent::SubagentStop, Some(victim), 1));
        assert_eq!(rows(&roster)[0].state, SubagentState::Idle);

        // Fill the rest of the roster with working children.
        for i in 0..MAX_SUBAGENTS - 1 {
            let id = format!("a{i:03}");
            roster.child(&id, &report(HookEvent::PreTool, Some(&id), i as u64 + 2));
        }
        assert_eq!(rows(&roster).len(), MAX_SUBAGENTS);

        roster.child(
            "a-newcomer",
            &report(HookEvent::PreTool, Some("a-newcomer"), 999),
        );

        let ids: Vec<String> = rows(&roster).into_iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), MAX_SUBAGENTS, "the roster stays at its cap");
        assert!(
            !ids.contains(&victim.to_owned()),
            "the idle teammate made room"
        );
        assert!(ids.contains(&"a-newcomer".to_owned()));
    }

    #[test]
    fn a_full_roster_with_nothing_idle_refuses_a_new_child() {
        let mut roster = Roster::default();
        for i in 0..MAX_SUBAGENTS {
            let id = format!("a{i:03}");
            roster.child(&id, &report(HookEvent::PreTool, Some(&id), i as u64));
        }
        assert_eq!(rows(&roster).len(), MAX_SUBAGENTS);

        roster.child(
            "a-overflow",
            &report(HookEvent::PreTool, Some("a-overflow"), 999),
        );
        assert_eq!(rows(&roster).len(), MAX_SUBAGENTS, "no idle child to evict");
        assert!(rows(&roster).iter().all(|s| s.id != "a-overflow"));
    }
}
