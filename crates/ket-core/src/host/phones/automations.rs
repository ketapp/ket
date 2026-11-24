//! What a phone asks of the automations: the list, and pausing, resuming,
//! starting now or deleting one. The host writes the file itself — no ket
//! window is needed — and its clock acts on the change within a second.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_remote::proto::{self, envelope::Payload};

use super::requests::{envelope, error};
use super::session::Phone;
use crate::automation::{Automation, Automations, Runs, Task};

pub(super) fn automations_from_phone(phone: &Phone<'_>, id: u64) -> Option<proto::Envelope> {
    Some(match listed(phone) {
        Ok(reply) => envelope(id, Payload::Automations(reply)),
        Err(why) => error(id, proto::ErrorCode::Unspecified, &why),
    })
}

pub(super) fn edit_automation_from_phone(
    phone: &Phone<'_>,
    id: u64,
    edit: &proto::EditAutomation,
) -> Option<proto::Envelope> {
    let visible = visible_projects(phone);
    let all = match Automations::load() {
        Ok(all) => all,
        Err(why) => {
            let why = format!("could not read the automations: {why}");
            return Some(error(id, proto::ErrorCode::Unspecified, &why));
        }
    };
    let Some(automation) = all
        .get(&edit.automation_id)
        .filter(|automation| visible.contains(&automation.project.as_str().to_owned()))
    else {
        return Some(error(
            id,
            proto::ErrorCode::NotFound,
            "no such automation on this Mac",
        ));
    };
    let target = automation.id.clone();
    let done = match proto::AutomationEdit::try_from(edit.edit) {
        Ok(proto::AutomationEdit::Pause) => change(&target, |automation| {
            automation.set_enabled(false);
        }),
        Ok(proto::AutomationEdit::Resume) => change(&target, |automation| {
            automation.set_enabled(true);
        }),
        Ok(proto::AutomationEdit::RunNow) => change(&target, Automation::ask_to_run),
        Ok(proto::AutomationEdit::Delete) => {
            Automations::delete(&target).map_err(|why| why.to_string())
        }
        Ok(proto::AutomationEdit::Unspecified) | Err(_) => {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                "that isn't a change an automation takes",
            ));
        }
    };
    Some(match done.and_then(|()| listed(phone)) {
        Ok(reply) => envelope(id, Payload::Automations(reply)),
        Err(why) => error(id, proto::ErrorCode::Unspecified, &why),
    })
}

fn change(id: &str, edit: impl FnOnce(&mut Automation)) -> Result<(), String> {
    Automations::update(|all| {
        if let Some(automation) = all.get_mut(id) {
            edit(automation);
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(|why| format!("could not save the automation: {why}"))
}

/// The projects this phone is shown.
fn visible_projects(phone: &Phone<'_>) -> Vec<String> {
    phone
        .phones
        .places()
        .projects
        .iter()
        .map(|project| project.id.clone())
        .collect()
}

fn listed(phone: &Phone<'_>) -> Result<proto::Automations, String> {
    let all =
        Automations::load().map_err(|why| format!("could not read the automations: {why}"))?;
    let runs = crate::paths::automations_dir()
        .ok()
        .and_then(|root| Runs::load_in(&root).ok())
        .unwrap_or_default();
    Ok(summaries(
        &all,
        &runs,
        &visible_projects(phone),
        Timestamp::now(),
        &TimeZone::system(),
    ))
}

/// The list a phone is sent: the automations in `visible` projects, those
/// to come soonest first, then the paused, then the finished.
fn summaries(
    all: &Automations,
    runs: &Runs,
    visible: &[String],
    now: Timestamp,
    tz: &TimeZone,
) -> proto::Automations {
    let mut rows: Vec<(u8, u64, &Automation)> = all
        .automations
        .iter()
        .filter(|automation| visible.iter().any(|id| id == automation.project.as_str()))
        .map(|automation| {
            let next = automation
                .next_start(now, tz)
                .and_then(|at| u64::try_from(at.as_millisecond()).ok());
            let rank = match (next, automation.finished()) {
                (Some(_), _) => 0,
                (None, false) => 1,
                (None, true) => 2,
            };
            (rank, next.unwrap_or(0), automation)
        })
        .collect();
    rows.sort_by_key(|(rank, next, automation)| (*rank, *next, automation.created_ms));
    proto::Automations {
        automations: rows
            .into_iter()
            .map(|(_, next, automation)| {
                let last = runs.of(&automation.id).next();
                proto::AutomationSummary {
                    id: automation.id.clone(),
                    name: automation.name.clone(),
                    project_id: automation.project.as_str().to_owned(),
                    schedule: automation.when.describe(),
                    enabled: automation.enabled,
                    finished: automation.finished(),
                    next_start_ms: next,
                    last_result: last
                        .map_or_else(String::new, |run| run.outcome.label().to_owned()),
                    last_run_ms: last.map_or(0, |run| run.at_ms),
                    agent: match &automation.task {
                        Task::Prompt { agent, .. } => agent.clone().unwrap_or_default(),
                        Task::Command { .. } => String::new(),
                    },
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{Clock, Outcome, Run, When};
    use crate::id::ProjectId;

    fn made(name: &str, project: &str, when: When) -> Automation {
        let mut automation = Automation::new(
            &[],
            ProjectId::new(project),
            name,
            Task::Command {
                line: "true".to_owned(),
            },
            when,
        );
        automation.id = name.to_owned();
        automation
    }

    #[test]
    fn a_phone_is_sent_its_projects_automations_soonest_first() {
        let tz = TimeZone::get("America/Los_Angeles").unwrap();
        let now = Timestamp::now();
        let daily = |hour| When::Daily {
            time: Clock::new(hour, 0).unwrap(),
        };
        let mut paused = made("paused", "mine", daily(9));
        paused.enabled = false;
        let all = Automations {
            automations: vec![
                paused,
                made("hourly", "mine", When::Hourly { minute: 59 }),
                made("hidden", "theirs", When::Hourly { minute: 0 }),
            ],
        };
        let mut runs = Runs::default();
        runs.record(Run {
            automation: "hourly".to_owned(),
            due_ms: 1,
            at_ms: 2,
            outcome: Outcome::Done,
            worktree: None,
            branch: None,
            ended_ms: None,
            tokens: None,
            command: None,
        });
        let sent = summaries(&all, &runs, &["mine".to_owned()], now, &tz);
        let names: Vec<&str> = sent.automations.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["hourly", "paused"]);
        let hourly = &sent.automations[0];
        assert!(hourly.enabled && hourly.next_start_ms > 0);
        assert_eq!(hourly.schedule, "Every hour at :59");
        assert_eq!(hourly.last_result, "Done");
        assert_eq!(sent.automations[1].next_start_ms, 0);
    }
}
