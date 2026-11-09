//! Registered projects and their settings.

use clap::Subcommand;
use ket_core::KetError;
use ket_core::command::{Invocation, Registry};
use ket_core::workspace::Workspace;
use std::path::PathBuf;

use crate::cwd;

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    registry.bind("project.add", |invocation, _| {
        add_project(invocation.text_of("path").unwrap_or("."))
    })?;

    registry.bind("project.list", |_, _| list_projects())?;

    registry.bind("project.remove", |invocation, _| {
        remove_project(
            invocation.require_text("project")?,
            invocation.is_set("force"),
        )
    })?;

    registry.bind("project.settings.set", |invocation, _| {
        set_project_settings(invocation)
    })?;

    Ok(())
}

impl ProjectCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            ProjectCmd::Add { path } => {
                Invocation::new("project.add").text("path", path.to_string_lossy())
            }
            ProjectCmd::List => Invocation::new("project.list"),
            ProjectCmd::Rm { project, force } => Invocation::new("project.remove")
                .text("project", project)
                .flag("force", force),
            ProjectCmd::Set {
                project,
                name,
                color,
                icon,
                base,
                agent,
                discovered,
                automation,
            } => Invocation::new("project.settings.set")
                .text("project", project)
                .maybe_text("name", name)
                .maybe_text("color", color)
                .maybe_text("icon", icon)
                .maybe_text("base", base)
                .maybe_text("agent", agent)
                .maybe_text("discovered", discovered)
                .maybe_text("automation", automation),
        }
    }
}

#[derive(Subcommand, Debug)]
pub(super) enum ProjectCmd {
    /// Register the repository containing a path.
    Add {
        /// Any path inside the repository. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// List registered projects, most recently used first.
    List,
    /// Deregister a project. The repository itself is untouched.
    Rm {
        /// Project id or name.
        project: String,
        /// Remove even while ket-managed worktrees remain.
        #[arg(long)]
        force: bool,
    },
    /// Change how a project is shown and what it defaults to.
    Set {
        /// Project id or name.
        project: String,
        /// Name shown in the sidebar. An empty string restores the directory name.
        #[arg(long)]
        name: Option<String>,
        /// Badge colour as `#rrggbb`. An empty string restores the default.
        #[arg(long)]
        color: Option<String>,
        /// Icon, stored verbatim. An empty string clears it.
        #[arg(long)]
        icon: Option<String>,
        /// Branch new worktrees are based on.
        #[arg(long)]
        base: Option<String>,
        /// Agent used when none is named. An empty string clears it.
        #[arg(long)]
        agent: Option<String>,
        /// Whether worktrees ket did not create are shown: `hide` or `show`.
        #[arg(long, value_parser = ["hide", "show"])]
        discovered: Option<String>,
        /// Whether committed hooks and post-command may run: `trust` or `deny`.
        #[arg(long, value_parser = ["trust", "deny"])]
        automation: Option<String>,
    },
}

/// Handles `ket project ...`.
pub(super) fn add_project(path: &str) -> Result<(), KetError> {
    let project = Workspace::open()?.add_project(std::path::Path::new(path))?;
    println!("{}  {}", project.id, project.root.display());
    Ok(())
}

/// Handles `ket project list`.
pub(super) fn list_projects() -> Result<(), KetError> {
    let workspace = Workspace::open()?;

    let projects = workspace.projects()?;
    if projects.is_empty() {
        eprintln!("ket: no projects registered; run `ket project add`");
        return Ok(());
    }

    let worktrees = workspace.worktrees(None)?;
    for project in projects {
        let count = worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .count();
        // Availability is worth showing: projects outlive their directories, and
        // a moved repo should be visible, not silent.
        let mark = if project.is_available() { ' ' } else { '!' };
        println!(
            "{mark} {:<28} {:<3} {}",
            project.id,
            count,
            project.root.display()
        );
    }
    Ok(())
}

/// Handles `ket project rm`.
pub(super) fn remove_project(project: &str, force: bool) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let resolved = workspace.resolve_project(Some(project), &cwd())?;
    workspace.remove_project(&resolved.id, force)?;
    println!("removed {}", resolved.id);
    Ok(())
}

/// Handles `ket project set`.
///
/// Presentation fields go through one `ProjectSettings` write; `base` and
/// `agent` have their own setters because they are checked against the
/// repository and the config respectively, and a failed check must not
/// half-apply the rest. Presentation is written first for the same reason —
/// it cannot fail on anything but the store itself.
pub(super) fn set_project_settings(invocation: &Invocation) -> Result<(), KetError> {
    let workspace = Workspace::open()?;
    let resolved = workspace.resolve_project(Some(invocation.require_text("project")?), &cwd())?;

    let mut settings = workspace.project_settings(&resolved.id)?;
    let mut touched = false;
    // An explicit empty string clears a field; an absent flag leaves it alone.
    let mut apply = |slot: &mut Option<String>, value: Option<&str>| {
        if let Some(value) = value {
            *slot = Some(value.to_owned());
            touched = true;
        }
    };
    apply(&mut settings.display_name, invocation.text_of("name"));
    apply(&mut settings.color, invocation.text_of("color"));
    apply(&mut settings.icon, invocation.text_of("icon"));
    match invocation.text_of("discovered") {
        Some("hide") => {
            settings.hide_discovered_worktrees = true;
            touched = true;
        }
        Some("show") => {
            settings.hide_discovered_worktrees = false;
            touched = true;
        }
        Some(other) => {
            return Err(KetError::Config(format!(
                "discovered must be `hide` or `show`, not {other:?}"
            )));
        }
        None => {}
    }
    if touched {
        workspace.set_project_settings(&resolved.id, settings)?;
    }

    if let Some(base) = invocation.text_of("base") {
        workspace.set_default_base(&resolved.id, base)?;
    }
    if let Some(agent) = invocation.text_of("agent") {
        let agent = (!agent.trim().is_empty()).then_some(agent);
        workspace.set_preferred_agent(&resolved.id, agent)?;
    }
    match invocation.text_of("automation") {
        Some("trust") => workspace.set_automation_trusted(&resolved.id, true)?,
        Some("deny") => workspace.set_automation_trusted(&resolved.id, false)?,
        Some(other) => {
            return Err(KetError::Config(format!(
                "automation must be `trust` or `deny`, not {other:?}"
            )));
        }
        None => {}
    }

    let project = workspace.resolve_project(Some(resolved.id.as_str()), &cwd())?;
    let settings = workspace.project_settings(&project.id)?;
    println!(
        "{}  name={}  color={}  icon={}  base={}  agent={}  discovered={}  automation={}",
        project.id,
        settings.name_for(&project),
        settings.color.as_deref().unwrap_or("-"),
        settings.icon.as_deref().unwrap_or("-"),
        project.default_base,
        project.preferred_agent.as_deref().unwrap_or("-"),
        if settings.hide_discovered_worktrees {
            "hide"
        } else {
            "show"
        },
        if workspace.automation_trusted(&project.id)? {
            "trust"
        } else {
            "deny"
        },
    );
    Ok(())
}

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "project", "add"],
    &["ket", "project", "list"],
    &["ket", "project", "rm", "proj-a", "--force"],
    &[
        "ket",
        "project",
        "set",
        "proj-a",
        "--name",
        "Ready Set Reading",
        "--color",
        "#eab308",
        "--discovered",
        "hide",
    ],
];
