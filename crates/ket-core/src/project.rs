//! Projects — the top of the data model.
//!
//! Worktrees hang off projects, not the other way round. ket is built for
//! working across many codebases at once, so nothing below this level may
//! assume there is only one repository.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AgentLaunch, AgentSpec};
use crate::id::ProjectId;
use crate::theme::Color;
use crate::{KetError, Result, git, slug};

/// Largest icon accepted, serialised.
///
/// The icon is opaque to core — see [`ProjectSettings::icon`] — and an
/// unbounded opaque blob is how a store quietly becomes a dumping ground, the
/// same reason `MAX_LAYOUT_BYTES` exists. Four kilobytes holds any emoji, glyph
/// name or path a shell could want, and nothing it should be storing here.
pub const MAX_ICON_BYTES: usize = 4 * 1024;

/// A git repository ket knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Stable identifier, derived from the repository's location.
    pub id: ProjectId,
    /// Display name — the repository directory's own name.
    pub name: String,
    /// Absolute path to the repository root.
    pub root: PathBuf,
    /// Branch new worktrees are based on unless told otherwise.
    pub default_base: String,
    /// Agent used for this project when none is named.
    pub preferred_agent: Option<String>,
    /// When this project was last opened, for recency ordering.
    pub last_opened_ms: u64,
}

/// Derives a project's identifier from its location.
///
/// The readable half comes from the directory name so a person can recognise it;
/// the hashed half comes from the full path, so two checkouts of the same
/// repository — or two unrelated projects that happen to both be called `api` —
/// never collide.
pub fn id_for(root: &Path) -> ProjectId {
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());

    ProjectId::new(slug::slug_keyed(&name, root.to_string_lossy().as_bytes()))
}

impl Project {
    /// Registers the repository containing `path`.
    ///
    /// Accepts any path inside a repository, not just its root — running
    /// `ket project add .` from a subdirectory is the common case.
    pub fn discover(path: &Path) -> Result<Self> {
        let root = git::discover_root(path)?;

        // Canonicalise so that `/tmp/x` and `/private/tmp/x` on macOS, or a path
        // reached through a symlink, resolve to one identity rather than two.
        let root = root.canonicalize().unwrap_or(root);

        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_owned());

        let default_base = git::Git::new(&root).default_branch()?;

        Ok(Self {
            id: id_for(&root),
            name,
            root,
            default_base,
            preferred_agent: None,
            last_opened_ms: crate::now_ms(),
        })
    }

    /// Whether the repository is still present and still a repository.
    ///
    /// Projects outlive the directories they point at. A moved or deleted repo
    /// must degrade to "unavailable" rather than making the whole tool fail.
    pub fn is_available(&self) -> bool {
        self.root.is_dir() && git::discover_root(&self.root).is_ok()
    }

    /// Whether `path` lies inside this project.
    pub fn contains(&self, path: &Path) -> bool {
        path.canonicalize()
            .map(|p| p.starts_with(&self.root))
            .unwrap_or(false)
    }
}

/// This user's preferences about a project, on this machine.
///
/// Distinct from two neighbours it is easy to confuse with. `.ket.toml`
/// describes the *repository* and travels with it, so an icon there would be
/// pushed to everyone who clones the repo. [`crate::store::ProjectState`] is
/// where you *were* — open worktrees, focus — and is rewritten on every
/// switch. This is neither: it is how a person wants a project to look and
/// behave in their own sidebar, and it changes when they say so.
///
/// `default_base` and `preferred_agent` deliberately stay on [`Project`]. They
/// are operational inputs — worktree creation reads one, agent launch reads
/// the other — and every path that needs them already holds a `Project`.
/// Moving them here would make each of those a second store lookup for the
/// sake of tidiness, so instead [`crate::workspace::Workspace`] grows setters
/// for them. What lives here is presentation and visibility, plus the
/// per-project overrides of a global setting — build directories, agent
/// commands — that only matter once something reads them by project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSettings {
    /// What the sidebar calls this project. `None` means the directory name.
    ///
    /// Distinct from [`Project::name`], which is derived from the path and
    /// feeds the project id: renaming a project must not re-key it.
    pub display_name: Option<String>,
    /// Badge colour, as `#rrggbb`. `None` means the shell's neutral default.
    ///
    /// Stored as text rather than a [`Color`] so the store stays hand-editable,
    /// but validated on the way in: a colour that will not parse is a mistake
    /// to report at the point of change, not something to make every renderer
    /// guess around.
    pub color: Option<String>,
    /// Stored verbatim and never interpreted.
    ///
    /// Whether an icon is an emoji, a glyph name or a file path is the shell's
    /// taxonomy; an `IconKind` enum here would be exactly the UI knowledge core
    /// is meant to keep out. Bounded by [`MAX_ICON_BYTES`].
    pub icon: Option<String>,
    /// Hide worktrees git knows about that ket did not create.
    ///
    /// **On by default.** It was off, on the reasoning that a project where
    /// something else made a worktree should look like it has one. That
    /// reasoning ignored what the shell does with the list: a discovered
    /// worktree an agent has worked in is adopted into the registry on load,
    /// so the default was not "show more rows", it was "write to state.json
    /// about checkouts the owner never asked ket to manage". Any tool that
    /// runs `git worktree add` in a repo — a sandboxed test run, another
    /// agent, a person — then turns into permanent registry entries, including
    /// ones whose path is a temp directory that is already gone.
    ///
    /// Off for a repo where git's other checkouts really are yours and you
    /// want ket to pick them up.
    pub hide_discovered_worktrees: bool,
    /// Which directories in this project's worktrees hold regenerable build
    /// output. `None` means the global [`crate::config::StorageConfig`] list.
    ///
    /// Per project because the answer is per *stack*: a Rust checkout's cost
    /// is `target/` and a Node one's is `node_modules`, and a monorepo's is
    /// several of each at paths only that repository knows. An empty list is
    /// meaningful and distinct from `None` — it says this project has nothing
    /// safe to clear, which is the right answer for a repository that commits
    /// its `dist/`.
    #[serde(default)]
    pub build_dirs: Option<Vec<String>>,
    /// The interactive command an agent launches with in this project, by
    /// agent name, in place of the one in the config.
    ///
    /// What lets one folder run `claude-work` while every other runs
    /// `claude-personal`. Only the terminal command is replaced — the same
    /// field [`AgentSpec::launch`] holds — so the agent keeps its name, its
    /// transport and its ACP adapter, and the sessions it leaves behind still
    /// file under that name. An agent with no entry launches as configured.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, AgentLaunch>,
    /// Keep this project's backlog in the repository, a file per note under
    /// `.ket/backlog/`, rather than privately in the data directory.
    ///
    /// For a team: committed, the notes are pushed and pulled with the code,
    /// and everyone works from one list. Off by default, because a backlog
    /// starts as one person's notes, and ket writing files into a checkout
    /// is something to be asked for. Changed through
    /// [`crate::backlog::Backlog::set_location`], which moves the notes
    /// across; flipped anywhere else, they stay where they were and the list
    /// looks empty.
    #[serde(default)]
    pub backlog_in_repo: bool,
}

impl Default for ProjectSettings {
    /// Hand-written for `hide_discovered_worktrees`, which is the one field
    /// whose zero value is not its default. `#[serde(default)]` on the struct
    /// routes missing fields through here too, so a settings blob written
    /// before this field existed reads as hidden rather than as `false`.
    fn default() -> Self {
        Self {
            display_name: None,
            color: None,
            icon: None,
            hide_discovered_worktrees: true,
            build_dirs: None,
            agents: BTreeMap::new(),
            backlog_in_repo: false,
        }
    }
}

impl ProjectSettings {
    /// Refuses settings the store must not hold.
    ///
    /// Called on the way in rather than on the way out, so a bad value is an
    /// error for whoever set it and never a surprise for whoever reads it.
    pub fn validate(&self) -> Result<()> {
        if let Some(color) = &self.color {
            Color::from_hex(color)?;
        }

        if let Some(icon) = &self.icon
            && icon.len() > MAX_ICON_BYTES
        {
            return Err(KetError::Config(format!(
                "project icon is {} bytes; the limit is {MAX_ICON_BYTES}",
                icon.len()
            )));
        }

        Ok(())
    }

    /// Trims the free-text fields, turning blanks into "unset".
    ///
    /// A display name of three spaces is not a name; storing it would render a
    /// project with no label and no way to see why.
    pub fn normalised(mut self) -> Self {
        let blank_to_none =
            |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        self.display_name = blank_to_none(self.display_name);
        self.color = blank_to_none(self.color).map(|c| c.to_ascii_lowercase());
        self.icon = blank_to_none(self.icon);
        // A blank command is no override: it would type nothing at the
        // prompt, which is a pane that sits there looking broken.
        self.agents = std::mem::take(&mut self.agents)
            .into_iter()
            .filter_map(|(name, mut launch)| {
                launch.command = launch.command.trim().to_owned();
                (!name.trim().is_empty() && !launch.command.is_empty())
                    .then(|| (name.trim().to_owned(), launch))
            })
            .collect();
        self
    }

    /// `spec` as this project launches it: its own command in place of the
    /// configured one, when it has one.
    pub fn agent_spec(&self, spec: &AgentSpec) -> AgentSpec {
        let mut spec = spec.clone();
        if let Some(launch) = self.agents.get(&spec.name) {
            spec.launch = Some(launch.clone());
        }
        spec
    }

    /// Whether this project launches `agent` differently from the config.
    pub fn overrides(&self, agent: &str) -> bool {
        self.agents.contains_key(agent)
    }

    /// The name to show for `project`: the chosen one, else the directory's.
    pub fn name_for<'a>(&'a self, project: &'a Project) -> &'a str {
        self.display_name.as_deref().unwrap_or(&project.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_directory_name_in_different_places_gets_different_ids() {
        let a = id_for(Path::new("/Users/me/work/api"));
        let b = id_for(Path::new("/Users/me/oss/api"));

        assert_ne!(a, b);
        assert!(a.as_str().starts_with("api-"));
        assert!(b.as_str().starts_with("api-"));
    }

    #[test]
    fn ids_are_stable_for_a_given_path() {
        // These are persisted; instability would orphan every worktree.
        assert_eq!(
            id_for(Path::new("/Users/me/dev/ket")),
            id_for(Path::new("/Users/me/dev/ket"))
        );
    }

    #[test]
    fn ids_are_single_path_components() {
        // The id becomes a directory name under the worktrees root.
        let id = id_for(Path::new("/deeply/nested/path/to/my.project"));
        assert!(!id.as_str().contains('/'));
        assert!(!id.as_str().contains(".."));
    }

    #[test]
    fn a_root_path_still_produces_an_id() {
        let id = id_for(Path::new("/"));
        assert!(!id.as_str().is_empty());
    }
}
