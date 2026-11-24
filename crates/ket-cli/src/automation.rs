//! Automations: work that starts itself, once at a set time or on a repeat —
//! see `ket_core::automation`. ket's host keeps the time and starts each one;
//! these commands only keep the list.

use clap::Subcommand;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_core::KetError;
use ket_core::automation::{
    Automation, Automations, Clock, Day, Dow, Outcome, Place, Runs, Task, When,
};
use ket_core::command::{self, Category, Invocation, Registry};
use ket_core::workspace::Workspace;

use crate::cwd;

/// Registers this module's commands, which are the CLI's alone, and attaches
/// their implementations.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    let text = |name: &str| command::Arg::optional(name, command::ArgKind::Text);
    let one = |name: &str| command::Arg::required(name, command::ArgKind::Text);
    registry.register(
        command::Command::new(
            "automation.list",
            "List automations, soonest start first",
            Category::Automation,
        )
        .arg(command::Arg::optional("project", command::ArgKind::Project)),
    )?;
    registry.register(
        command::Command::new(
            "automation.add",
            "Add an automation: a prompt or a command, once or on a repeat",
            Category::Automation,
        )
        .arg(one("name"))
        .arg(text("prompt"))
        .arg(text("command"))
        .arg(text("every"))
        .arg(text("on"))
        .arg(text("at"))
        .arg(text("agent"))
        .arg(text("model"))
        .arg(text("effort"))
        .arg(text("base"))
        .arg(text("worktree"))
        .arg(command::Arg::optional("project", command::ArgKind::Project)),
    )?;
    for (id, title) in [
        ("automation.pause", "Pause an automation"),
        ("automation.resume", "Turn a paused automation back on"),
        (
            "automation.run",
            "Start an automation now, whatever its schedule",
        ),
        ("automation.runs", "Show what an automation's runs came to"),
        ("automation.remove", "Delete an automation and its history"),
    ] {
        registry.register(
            command::Command::new(id, title, Category::Automation).arg(one("automation")),
        )?;
    }

    registry.register(
        command::Command::new(
            "automation.exec",
            "Run an automation's command here and record how it ended",
            Category::Automation,
        )
        .arg(one("automation"))
        .arg(one("due")),
    )?;
    registry.bind("automation.exec", |invocation, _| {
        exec(
            invocation.require_text("automation")?,
            invocation.require_text("due")?,
        )
    })?;
    registry.bind("automation.list", |invocation, _| {
        list(invocation.text_of("project"))
    })?;
    registry.bind("automation.add", |invocation, _| {
        add(&Added {
            name: invocation.require_text("name")?,
            prompt: invocation.text_of("prompt"),
            command: invocation.text_of("command"),
            every: invocation.text_of("every"),
            on: invocation.text_of("on"),
            at: invocation.text_of("at"),
            agent: invocation.text_of("agent"),
            model: invocation.text_of("model"),
            effort: invocation.text_of("effort"),
            base: invocation.text_of("base"),
            worktree: invocation.text_of("worktree"),
            project: invocation.text_of("project"),
        })
    })?;
    registry.bind("automation.pause", |invocation, _| {
        set_on(invocation.require_text("automation")?, false)
    })?;
    registry.bind("automation.resume", |invocation, _| {
        set_on(invocation.require_text("automation")?, true)
    })?;
    registry.bind("automation.run", |invocation, _| {
        run_now(invocation.require_text("automation")?)
    })?;
    registry.bind("automation.runs", |invocation, _| {
        runs(invocation.require_text("automation")?)
    })?;
    registry.bind("automation.remove", |invocation, _| {
        remove(invocation.require_text("automation")?)
    })?;
    Ok(())
}

// Parsed once and dropped: `Add`'s size costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub(super) enum AutomationCmd {
    /// List automations, soonest start first, then the paused ones.
    List {
        /// Only this project's. Every project's by default.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Add an automation: an agent's prompt, or a shell command, once or on
    /// a repeat.
    ///
    /// `--every weekday --at 9am`, `--every mon,wed,fri --at 18:00`,
    /// `--every hour --at :05`, `--on tomorrow --at 2am`. Each run works in a
    /// new worktree named after the automation and the day, unless `--in`
    /// names one to open a tab in.
    Add {
        /// What lists call it, and what its runs' branches are named after.
        name: String,
        /// What the agent is told each run.
        #[arg(long, required_unless_present = "command", conflicts_with = "command")]
        prompt: Option<String>,
        /// A shell command to run each time, in a terminal, instead of an
        /// agent.
        #[arg(long)]
        command: Option<String>,
        /// How often: `day`, `weekday`, `hour`, or days such as `mon,wed,fri`.
        #[arg(long, required_unless_present = "on", conflicts_with = "on")]
        every: Option<String>,
        /// Once, on this day: `today`, `tomorrow` or `2026-11-23`.
        #[arg(long)]
        on: Option<String>,
        /// The time: `9am`, `21:30`. For `--every hour`, the minutes past:
        /// `:05`.
        #[arg(long)]
        at: Option<String>,
        /// Agent to run. Defaults to the project's preferred agent.
        #[arg(long, short)]
        agent: Option<String>,
        /// The agent's model. Defaults to the agent's own.
        #[arg(long)]
        model: Option<String>,
        /// The agent's effort. Defaults to the agent's own.
        #[arg(long)]
        effort: Option<String>,
        /// The branch each run's worktree starts from. Defaults to the
        /// project's.
        #[arg(long, conflicts_with = "worktree")]
        base: Option<String>,
        /// Run in this worktree — its id, name or branch — opening a new tab
        /// each time, rather than in a new worktree.
        #[arg(long = "in", value_name = "WORKTREE")]
        worktree: Option<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Pause an automation. It keeps its place, and starts nothing.
    Pause {
        /// Its id or name, from `ket automation list`.
        automation: String,
    },
    /// Turn a paused automation back on, from its next start.
    Resume {
        /// Its id or name, from `ket automation list`.
        automation: String,
    },
    /// Start an automation now, whatever its schedule. Needs ket's host,
    /// which opens a ket window to start it in.
    Run {
        /// Its id or name, from `ket automation list`.
        automation: String,
    },
    /// Show what an automation's runs came to, newest first.
    Runs {
        /// Its id or name, from `ket automation list`.
        automation: String,
    },
    /// Delete an automation and its history. The worktrees its runs made
    /// are kept.
    Rm {
        /// Its id or name, from `ket automation list`.
        automation: String,
    },
    /// Typed by ket into an automation's terminal: runs its command, as
    /// saved, with this shell, and records how it ended.
    #[command(hide = true)]
    Exec {
        /// The automation's id.
        automation: String,
        /// When its run was due, in milliseconds.
        due: String,
    },
}

impl AutomationCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            AutomationCmd::List { project } => {
                Invocation::new("automation.list").maybe_text("project", project)
            }
            AutomationCmd::Add {
                name,
                prompt,
                command,
                every,
                on,
                at,
                agent,
                model,
                effort,
                base,
                worktree,
                project,
            } => Invocation::new("automation.add")
                .text("name", name)
                .maybe_text("prompt", prompt)
                .maybe_text("command", command)
                .maybe_text("every", every)
                .maybe_text("on", on)
                .maybe_text("at", at)
                .maybe_text("agent", agent)
                .maybe_text("model", model)
                .maybe_text("effort", effort)
                .maybe_text("base", base)
                .maybe_text("worktree", worktree)
                .maybe_text("project", project),
            AutomationCmd::Pause { automation } => {
                Invocation::new("automation.pause").text("automation", automation)
            }
            AutomationCmd::Resume { automation } => {
                Invocation::new("automation.resume").text("automation", automation)
            }
            AutomationCmd::Run { automation } => {
                Invocation::new("automation.run").text("automation", automation)
            }
            AutomationCmd::Runs { automation } => {
                Invocation::new("automation.runs").text("automation", automation)
            }
            AutomationCmd::Rm { automation } => {
                Invocation::new("automation.remove").text("automation", automation)
            }
            AutomationCmd::Exec { automation, due } => Invocation::new("automation.exec")
                .text("automation", automation)
                .text("due", due),
        }
    }
}

/// What `ket automation add` was given.
struct Added<'a> {
    name: &'a str,
    prompt: Option<&'a str>,
    command: Option<&'a str>,
    every: Option<&'a str>,
    on: Option<&'a str>,
    at: Option<&'a str>,
    agent: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    base: Option<&'a str>,
    worktree: Option<&'a str>,
    project: Option<&'a str>,
}

/// Handles `ket automation list`.
fn list(project: Option<&str>) -> Result<(), KetError> {
    let project = match project {
        Some(wanted) => Some(Workspace::open()?.resolve_project(Some(wanted), &cwd())?.id),
        None => None,
    };
    let all = Automations::load()?;
    let runs = Runs::load_in(&ket_core::paths::automations_dir()?)?;
    let tz = TimeZone::system();
    let now = Timestamp::now();
    let mine =
        |automation: &&Automation| project.as_ref().is_none_or(|id| &automation.project == id);
    let upcoming: Vec<(&Automation, Timestamp)> = all
        .upcoming(now, &tz)
        .into_iter()
        .filter(|(automation, _)| mine(automation))
        .collect();
    let resting: Vec<&Automation> = all
        .automations
        .iter()
        .filter(mine)
        .filter(|automation| upcoming.iter().all(|(up, _)| up.id != automation.id))
        .collect();
    if upcoming.is_empty() && resting.is_empty() {
        eprintln!(
            "ket: no automations; run `ket automation add \"<name>\" --prompt \"<what to do>\" --every day --at 9am`"
        );
        return Ok(());
    }
    for (automation, next) in upcoming {
        let next = format!("next {}", when_label(next, &tz));
        print_row(automation, &next, &runs);
    }
    for automation in resting {
        let state = if automation.finished() {
            "done"
        } else {
            "paused"
        };
        print_row(automation, state, &runs);
    }
    Ok(())
}

fn print_row(automation: &Automation, state: &str, runs: &Runs) {
    let last = runs
        .of(&automation.id)
        .next()
        .map(|run| format!("  last: {}", run.outcome.label()))
        .unwrap_or_default();
    println!(
        "{:<10} {}  · {} · {}{last}",
        automation.id,
        automation.name,
        automation.when.describe(),
        state
    );
}

/// Handles `ket automation add`.
fn add(given: &Added<'_>) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let project = workspace.resolve_project(given.project, &cwd())?;
    let tz = TimeZone::system();
    let when = when_from(given.every, given.on, given.at, &tz)?;
    when.check_at(Timestamp::now(), &tz)
        .map_err(KetError::Conflict)?;
    let task = match (given.prompt, given.command) {
        (Some(text), None) => Task::Prompt {
            text: text.to_owned(),
            agent: given.agent.map(str::to_owned),
            model: given.model.map(str::to_owned),
            effort: given.effort.map(str::to_owned),
        },
        (None, Some(line)) => Task::Command {
            line: line.to_owned(),
        },
        _ => {
            return Err(KetError::Conflict(
                "say what it does: --prompt for an agent, or --command".to_owned(),
            ));
        }
    };
    let place = match given.worktree {
        Some(wanted) => {
            let worktrees = workspace.worktrees(Some(&project.id))?;
            let found = worktrees
                .iter()
                .find(|worktree| {
                    worktree.id.as_str() == wanted
                        || worktree.branch == wanted
                        || worktree.name.as_deref() == Some(wanted)
                })
                .ok_or_else(|| {
                    KetError::Conflict(format!("no worktree {wanted:?} in {}", project.name))
                })?;
            Place::Worktree {
                id: found.id.clone(),
            }
        }
        None => Place::NewWorktree {
            base: given.base.map(str::to_owned),
        },
    };
    let mut made = None;
    Automations::update(|all| {
        let mut automation = Automation::new(
            &all.automations,
            project.id.clone(),
            given.name,
            task.clone(),
            when.clone(),
        );
        automation.place = place.clone();
        made = Some(automation.clone());
        all.automations.push(automation);
        Ok(())
    })?;
    let Some(automation) = made else {
        return Ok(());
    };
    let next = automation
        .next_start(Timestamp::now(), &tz)
        .map(|next| format!(" · next {}", when_label(next, &tz)))
        .unwrap_or_default();
    println!(
        "{}  {} · {}{next}",
        automation.id,
        automation.name,
        automation.when.describe()
    );
    if ket_core::host::host_pid().is_none() {
        eprintln!("ket: it starts while ket's host is running; open ket to start it");
    }
    Ok(())
}

/// The schedule `--every`, `--on` and `--at` describe.
fn when_from(
    every: Option<&str>,
    on: Option<&str>,
    at: Option<&str>,
    tz: &TimeZone,
) -> Result<When, KetError> {
    let wrong = |what: String| KetError::Conflict(what);
    let time = || -> Result<Clock, KetError> {
        let typed = at.ok_or_else(|| wrong("say when with --at, like --at 9am".to_owned()))?;
        Clock::parse(typed)
            .ok_or_else(|| wrong(format!("{typed:?} isn't a time; try 9am or 21:30")))
    };
    if let Some(day) = on {
        let today = Timestamp::now().to_zoned(tz.clone()).date();
        let date = match day.trim().to_ascii_lowercase().as_str() {
            "today" => today,
            "tomorrow" => today
                .tomorrow()
                .map_err(|e| wrong(format!("no tomorrow: {e}")))?,
            typed => typed
                .parse()
                .map_err(|_| wrong(format!("{day:?} isn't a day; try tomorrow or 2026-11-23")))?,
        };
        return Ok(When::Once {
            day: Day::new(date),
            time: time()?,
        });
    }
    let every =
        every.ok_or_else(|| wrong("say how often with --every, or once with --on".to_owned()))?;
    let when = match every.trim().to_ascii_lowercase().as_str() {
        "day" | "days" | "daily" => When::Daily { time: time()? },
        "weekday" | "weekdays" => When::Weekdays { time: time()? },
        "hour" | "hours" | "hourly" => {
            let minute = match at {
                None => 0,
                Some(typed) => typed
                    .trim()
                    .trim_start_matches(':')
                    .parse::<u8>()
                    .ok()
                    .filter(|minute| *minute < 60)
                    .ok_or_else(|| {
                        wrong(format!("{typed:?} isn't minutes past the hour; try :05"))
                    })?,
            };
            When::Hourly { minute }
        }
        days => {
            let days = days
                .split([',', ' '])
                .filter(|day| !day.is_empty())
                .map(|day| Dow::parse(day).ok_or_else(|| wrong(format!("{day:?} isn't a day"))))
                .collect::<Result<Vec<Dow>, KetError>>()?;
            When::Days {
                days,
                time: time()?,
            }
        }
    };
    when.check().map_err(KetError::Conflict)?;
    Ok(when)
}

/// The automation `wanted` names — its id, or its name, ignoring case —
/// or a clear error.
fn find<'a>(all: &'a Automations, wanted: &str) -> Result<&'a Automation, KetError> {
    if let Some(found) = all.get(wanted) {
        return Ok(found);
    }
    let named: Vec<&Automation> = all
        .automations
        .iter()
        .filter(|automation| automation.name.eq_ignore_ascii_case(wanted.trim()))
        .collect();
    match named.as_slice() {
        [one] => Ok(one),
        [] => Err(KetError::Conflict(format!("no automation {wanted:?}"))),
        _ => Err(KetError::Conflict(format!(
            "more than one automation is called {wanted:?}; use its id"
        ))),
    }
}

/// Handles `ket automation pause` and `resume`.
fn set_on(wanted: &str, on: bool) -> Result<(), KetError> {
    let id = find(&Automations::load()?, wanted)?.id.clone();
    let saved = Automations::update(|all| {
        if let Some(automation) = all.get_mut(&id) {
            automation.set_enabled(on);
        }
        Ok(())
    })?;
    if let Some(automation) = saved.get(&id) {
        let tz = TimeZone::system();
        let next = automation
            .next_start(Timestamp::now(), &tz)
            .map(|next| format!(" · next {}", when_label(next, &tz)))
            .unwrap_or_default();
        let state = if on { "on" } else { "paused" };
        println!("{}  {} · {state}{next}", automation.id, automation.name);
    }
    Ok(())
}

/// Handles `ket automation run`.
fn run_now(wanted: &str) -> Result<(), KetError> {
    let id = find(&Automations::load()?, wanted)?.id.clone();
    Automations::update(|all| {
        if let Some(automation) = all.get_mut(&id) {
            automation.ask_to_run();
        }
        Ok(())
    })?;
    if ket_core::host::host_pid().is_some() {
        println!("{id}  starting now");
    } else {
        eprintln!("ket: asked; it starts once ket's host is running — open ket");
    }
    Ok(())
}

/// Handles `ket automation runs`.
fn runs(wanted: &str) -> Result<(), KetError> {
    let all = Automations::load()?;
    let automation = find(&all, wanted)?;
    let runs = Runs::load_in(&ket_core::paths::automations_dir()?)?;
    let tz = TimeZone::system();
    let mut any = false;
    for run in runs.of(&automation.id) {
        any = true;
        let at = i64::try_from(run.at_ms)
            .ok()
            .and_then(|ms| Timestamp::from_millisecond(ms).ok())
            .map(|at| when_label(at, &tz))
            .unwrap_or_default();
        let why = match &run.outcome {
            Outcome::Failed { why }
            | Outcome::Skipped { why }
            | Outcome::Missed { why }
            | Outcome::CouldNotStart { why } => format!("  {why}"),
            Outcome::Started | Outcome::Done => String::new(),
        };
        let place = run
            .branch
            .as_deref()
            .map(|branch| format!("  {branch}"))
            .unwrap_or_default();
        println!("{at:<22} {:<14}{place}{why}", run.outcome.label());
    }
    if !any {
        eprintln!("ket: {} hasn't run yet", automation.name);
    }
    Ok(())
}

/// Handles `ket automation exec`: runs the command run `due` of automation
/// `id` was started with — as the host recorded it then, whatever the
/// automation says now — with the person's own shell, the line exactly as
/// saved, in this terminal; records how it ended, and exits as the command
/// did. A run that isn't recorded — its automation deleted since — runs
/// nothing.
///
/// Ctrl-C reaches the command, which is in the terminal's foreground with
/// this; this stays to record that it was stopped.
fn exec(automation: &str, due: &str) -> Result<(), KetError> {
    use std::os::unix::process::ExitStatusExt as _;

    let due: u64 = due
        .parse()
        .map_err(|_| KetError::Conflict(format!("{due:?} isn't a due time")))?;
    let root = ket_core::paths::automations_dir()?;
    let runs = Runs::load_in(&root)?;
    let run = runs
        .runs
        .iter()
        .find(|run| run.automation == automation && run.due_ms == due)
        .ok_or_else(|| {
            KetError::Conflict(
                "that run isn't recorded — its automation was deleted — so nothing ran".to_owned(),
            )
        })?;
    let all = Automations::load()?;
    let saved = all.get(automation);
    let line = match (&run.command, saved.map(|found| &found.task)) {
        (Some(line), _) => line.clone(),
        // A run recorded before commands were kept with it.
        (None, Some(Task::Command { line })) => line.clone(),
        (None, _) => {
            return Err(KetError::Conflict(
                "that run has no command to run".to_owned(),
            ));
        }
    };
    let name = saved.map_or(automation, |found| found.name.as_str());
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    eprintln!("ket: {name} \u{b7} {line}");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| KetError::Conflict(format!("couldn't start: {e}")))?;
    let _interrupts = runtime
        .block_on(async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        })
        .map_err(|e| KetError::Conflict(format!("couldn't hold Ctrl-C: {e}")))?;
    let status = std::process::Command::new(&shell)
        .arg("-c")
        .arg(&line)
        .status()
        .map_err(|e| KetError::Conflict(format!("couldn't run it: {e}")))?;
    let outcome = match (status.code(), status.signal()) {
        (Some(0), _) => Outcome::Done,
        (Some(code), _) => Outcome::Failed {
            why: format!("the command exited {code}"),
        },
        (None, signal) => Outcome::Failed {
            why: format!(
                "the command was stopped (signal {})",
                signal.unwrap_or_default()
            ),
        },
    };
    Runs::update_in(&root, |runs| {
        runs.end(automation, due, outcome, ket_core::now_ms());
    })?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Handles `ket automation rm`.
fn remove(wanted: &str) -> Result<(), KetError> {
    let all = Automations::load()?;
    let automation = find(&all, wanted)?;
    Automations::delete(&automation.id)?;
    println!("{}  deleted  {}", automation.id, automation.name);
    Ok(())
}

/// `Fri 20 Nov, 2:00 AM` on this Mac's clock.
fn when_label(at: Timestamp, tz: &TimeZone) -> String {
    let local = at.to_zoned(tz.clone());
    let day = Day::new(local.date()).label();
    let time = u8::try_from(local.hour())
        .ok()
        .zip(u8::try_from(local.minute()).ok())
        .and_then(|(hour, minute)| Clock::new(hour, minute))
        .map(Clock::label)
        .unwrap_or_default();
    format!("{day}, {time}")
}

/// Every way of reaching this module's commands from argv — see `main`'s
/// tests.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "automation", "list"],
    &["ket", "automation", "list", "--project", "proj"],
    &[
        "ket",
        "automation",
        "add",
        "Audit dependencies",
        "--prompt",
        "Audit the dependencies.",
        "--every",
        "day",
        "--at",
        "2am",
    ],
    &[
        "ket",
        "automation",
        "add",
        "Run the e2e suite",
        "--command",
        "npm run e2e",
        "--on",
        "tomorrow",
        "--at",
        "18:00",
        "--in",
        "main",
    ],
    &["ket", "automation", "pause", "abc"],
    &["ket", "automation", "resume", "abc"],
    &["ket", "automation", "run", "abc"],
    &["ket", "automation", "runs", "abc"],
    &["ket", "automation", "rm", "abc"],
    &["ket", "automation", "exec", "abc", "1795000000000"],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::get("America/Los_Angeles").unwrap()
    }

    #[test]
    fn every_and_at_read_as_a_schedule() {
        let tz = tz();
        let when = |every: &str, at: Option<&str>| when_from(Some(every), None, at, &tz).unwrap();
        assert_eq!(when("day", Some("2am")).describe(), "Every day at 2:00 AM");
        assert_eq!(
            when("weekdays", Some("9:00")).describe(),
            "Weekdays at 9:00 AM"
        );
        assert_eq!(
            when("mon,wed,fri", Some("6pm")).describe(),
            "Mon, Wed and Fri at 6:00 PM"
        );
        assert_eq!(when("hour", Some(":05")).describe(), "Every hour at :05");
        assert_eq!(when("hourly", None).describe(), "Every hour at :00");
    }

    #[test]
    fn a_one_off_takes_a_day_and_a_time() {
        let tz = tz();
        let once = when_from(None, Some("2026-11-23"), Some("4pm"), &tz).unwrap();
        assert_eq!(once.describe(), "Once, Mon 23 Nov at 4:00 PM");
        assert!(when_from(None, Some("tomorrow"), Some("2am"), &tz).is_ok());
        assert!(when_from(None, Some("someday"), Some("2am"), &tz).is_err());
    }

    #[test]
    fn a_schedule_missing_a_part_says_which() {
        let tz = tz();
        let err = |every: Option<&str>, at: Option<&str>| {
            when_from(every, None, at, &tz).unwrap_err().to_string()
        };
        assert!(err(None, Some("9am")).contains("--every"));
        assert!(err(Some("day"), None).contains("--at"));
        assert!(err(Some("day"), Some("noonish")).contains("isn't a time"));
        assert!(err(Some("t,th"), Some("9am")).contains("isn't a day"));
        assert!(err(Some("hour"), Some(":75")).contains("minutes past"));
    }

    #[test]
    fn an_automation_is_found_by_id_or_by_a_name_only_one_has() {
        let made = |name: &str, id: &str| {
            let mut automation = Automation::new(
                &[],
                ket_core::id::ProjectId::new("p"),
                name,
                Task::Command {
                    line: "true".to_owned(),
                },
                When::Hourly { minute: 0 },
            );
            automation.id = id.to_owned();
            automation
        };
        let all = Automations {
            automations: vec![
                made("Audit", "a1"),
                made("Twice", "a2"),
                made("twice", "a3"),
            ],
        };
        assert_eq!(find(&all, "a1").unwrap().id, "a1");
        assert_eq!(find(&all, "audit").unwrap().id, "a1");
        assert!(find(&all, "twice").is_err(), "two have that name");
        assert!(find(&all, "nothing").is_err());
    }
}
