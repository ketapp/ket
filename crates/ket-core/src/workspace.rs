//! The engine facade — what both the CLI and the future web shell drive.
//!
//! Everything here is UI-agnostic (invariant 3). Operations mutate durable state
//! under a lock and announce themselves on the event bus, so any number of
//! clients can watch the same engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::{self, CancelHandle, Session, SessionContext, SessionStatus};
use crate::branch_cleanup;
use crate::config::{Config, HookConfig, ProvisionConfig, Transport};
use crate::diff::{self, WorktreeDiff};
use crate::event::{Event, EventBus, SessionOutcome};
use crate::git::{Fetch, Git, PreparedBase};
use crate::hook::{HookPoint, HookRunner};
use crate::id::{ProjectId, SessionId, WorktreeId};
use crate::journal::Journal;
use crate::project::{Project, ProjectSettings};
use crate::provision::{self, ProvisionReport};
use crate::status::{self, WorktreeStatus};
use crate::store::{MAX_LAYOUT_BYTES, ProjectState, Store};
use crate::worktree::{self, DiscoveredWorktree, Worktree};
use crate::worktree_trash;
use crate::{KetError, Result, paths};

fn clean_worktree_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(120).collect();
    (!name.is_empty()).then_some(name)
}

/// How often the watchdog checks whether a session has stalled.
const WATCHDOG_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// What [`Workspace::collapse`] should do with the attempts it touches.
#[derive(Debug, Clone, Copy, Default)]
pub struct CollapseOptions {
    /// Merge even when the winner or the primary checkout is dirty.
    ///
    /// Only what is committed is merged either way; this waives the refusal,
    /// it does not widen the merge.
    pub force: bool,
    /// Keep the winning worktree instead of removing it once merged.
    pub keep_winner: bool,
    /// Keep the losing worktrees instead of removing them.
    pub keep_losers: bool,
}

/// What [`Workspace::worktree_report`] found, managed and discovered apart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeReport {
    /// Worktrees ket created — the same records [`Workspace::worktrees`] returns.
    pub managed: Vec<Worktree>,
    /// Worktrees git knows about that are neither ket's own nor the primary
    /// checkout.
    pub discovered: Vec<DiscoveredWorktree>,
}

/// A worktree, its project, and the git handle a merge needs.
///
/// The `Git` is bound to the **primary checkout**, which is where a merge
/// happens — not to the worktree, which is where the commit happens.
struct MergePlan {
    worktree: Worktree,
    project: Project,
    git: Git,
    base: String,
}

/// What [`Workspace::merge_worktree`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReport {
    /// The worktree that was merged.
    pub worktree: WorktreeId,
    /// Its branch.
    pub branch: String,
    /// The branch it was merged into.
    pub into: String,
    /// The commit made from uncommitted work, if there was any to make.
    pub committed: Option<crate::git::CommitReport>,
    /// Whether the base already contained the branch, so no merge was needed.
    pub already_merged: bool,
    /// Whether the checkout was removed afterwards.
    pub removed: bool,
    /// Why removal did not happen, when it was asked for and failed.
    ///
    /// Not an `Err`: the merge succeeded, and reporting a failure for the
    /// tidying afterwards would tell the reader their work did not land.
    pub remove_failed: Option<String>,
}

/// What [`Workspace::remove_worktree`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemovalReport {
    /// The branch that outlived its checkout, when one did.
    ///
    /// Set when the branch still holds commits no base has. Deleting a
    /// worktree must never be the thing that throws away work nobody has seen,
    /// so the branch stays and the caller is told which one — a surface that
    /// silently kept it would leave the person believing it was gone.
    pub preserved_branch: Option<String>,
    /// Whether the checkout was renamed aside and left to a worker thread.
    ///
    /// False when it had to be deleted in place, which is the slow path and
    /// means the caller has already waited for it.
    pub deferred: bool,
}

/// Why a merge did not happen.
///
/// Two variants rather than one [`KetError`], because the surface that reports
/// this has exactly one question to ask — whether to offer to do it anyway —
/// and every other way of answering it means matching on message text.
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    /// Refused for a reason `force` waives.
    #[error("{0}")]
    Waivable(String),
    /// Everything else, as it came. Forcing would not help.
    #[error(transparent)]
    Failed(#[from] KetError),
}

/// What [`Workspace::collapse`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollapseReport {
    /// The branch that was merged.
    pub merged: String,
    /// The branch it was merged into.
    pub into: String,
    /// Worktrees removed because they lost.
    pub discarded: Vec<WorktreeId>,
    /// Whether the base already contained the winner, so no merge was needed.
    pub already_merged: bool,
    /// Whether the winning worktree was left in place.
    pub winner_kept: bool,
}

/// Owns durable state and the event bus, and exposes ket's operations.
#[derive(Debug)]
pub struct Workspace {
    store: Store,
    bus: Arc<EventBus>,
    worktrees_root: PathBuf,
    config: Config,
    /// Runs the commands configured at each lifecycle point, gating the
    /// operation they sit around by exit code. See [`crate::hook`].
    hooks: HookRunner,
    /// Sessions this process is driving.
    ///
    /// In memory rather than in the store, and deliberately so: a permission
    /// answer or a cancel can only reach a session through the process that
    /// spawned it, so a handle here would be a lie in any other process. The
    /// *record* of the session is persisted; the control surface is not.
    live: std::sync::Mutex<std::collections::HashMap<SessionId, LiveSession>>,
}

/// The control surface for a session this process is driving.
#[derive(Debug, Clone)]
struct LiveSession {
    ctx: Arc<SessionContext>,
    cancel: CancelHandle,
}

impl Workspace {
    /// Opens the workspace at ket's default locations.
    ///
    /// This bus journals to disk; the explicit-path constructors below do not.
    /// That split is deliberate: a test must never append to the developer's
    /// real event journal, and a workspace pointed at a sandbox has no business
    /// writing outside it.
    pub fn open() -> Result<Self> {
        if let Some(pack) = crate::pack::load() {
            worktree::activate_pack(
                pack.pack_id.clone(),
                pack.pack_version,
                pack.default_level(),
                pack.levels(),
            );
        }
        // Read first: it says where new worktrees go.
        let config = Config::load()?;
        Ok(Self {
            store: Store::open_default()?,
            bus: Arc::new(EventBus::with_journal(
                crate::event::DEFAULT_CAPACITY,
                Journal::at(paths::events_file()?),
            )),
            worktrees_root: config.worktrees_dir()?,
            config,
            hooks: HookRunner::real(),
            live: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Opens a workspace with explicit locations and default config.
    ///
    /// Config is held rather than re-read per operation, so tests never depend
    /// on whatever happens to be in the developer's `~/.config/ket`.
    pub fn with_paths(store: Store, worktrees_root: impl Into<PathBuf>) -> Self {
        Self::with_config(store, worktrees_root, Config::default())
    }

    /// Opens a workspace with explicit locations and an explicit config.
    ///
    /// Hooks run for real (see [`HookRunner::real`]) — with an empty
    /// [`HookConfig`], which is what [`Config::default`] carries, that spawns
    /// nothing at all, so this stays exactly as inert for existing tests as it
    /// was before hooks existed. A test that configures hooks and wants to
    /// avoid real processes should use [`Workspace::with_hooks`] instead.
    pub fn with_config(store: Store, worktrees_root: impl Into<PathBuf>, config: Config) -> Self {
        Self::with_hooks(store, worktrees_root, config, HookRunner::real())
    }

    /// Opens a workspace with an explicit [`HookRunner`], so a test can
    /// substitute a fake [`crate::hook::HookSpawner`] and assert on lifecycle
    /// hooks without depending on real binaries — the same seam
    /// [`crate::surface::ExternalSurface`] uses for its own spawner.
    pub fn with_hooks(
        store: Store,
        worktrees_root: impl Into<PathBuf>,
        config: Config,
        hooks: HookRunner,
    ) -> Self {
        Self {
            store,
            bus: Arc::new(EventBus::default()),
            worktrees_root: worktrees_root.into(),
            config,
            hooks,
            live: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The configuration this workspace was opened with.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The event bus, for clients that want to watch.
    pub fn bus(&self) -> &Arc<EventBus> {
        &self.bus
    }

    /// Runs the hooks configured for `point`, project hooks from
    /// `project_root`'s own `.ket.toml` first, then the global config.
    ///
    /// Reads the project file fresh on every call rather than caching it,
    /// matching [`ProvisionConfig::for_repo`]'s own read-per-call — a hook file
    /// edited between two operations should take effect on the next one
    /// without restarting ket.
    fn run_hook(
        &self,
        point: HookPoint,
        project_automation_trusted: bool,
        project_root: &Path,
        context: serde_json::Value,
    ) -> Result<()> {
        let project_hooks = if project_automation_trusted {
            HookConfig::for_repo(project_root)?
        } else {
            if project_root.join(".ket.toml").is_file() {
                tracing::warn!(
                    project = %project_root.display(),
                    "skipping repository hooks because automation is not trusted"
                );
            }
            HookConfig::default()
        };
        self.hooks
            .run(point, &project_hooks, &self.config.hooks, &context)
    }

    // ---- projects ---------------------------------------------------------

    /// Registers the repository containing `path`.
    ///
    /// Idempotent: registering an already-known project returns the existing
    /// entry rather than failing or duplicating it.
    pub fn add_project(&self, path: &Path) -> Result<Project> {
        let discovered = Project::discover(path)?;
        let id = discovered.id.clone();

        let (project, is_new) =
            self.store
                .update(|state| match state.projects.iter().find(|p| p.id == id) {
                    Some(existing) => Ok((existing.clone(), false)),
                    None => {
                        state.projects.push(discovered.clone());
                        Ok((discovered.clone(), true))
                    }
                })?;

        if is_new {
            self.bus.publish(Event::ProjectRegistered {
                project_id: project.id.clone(),
                name: project.name.clone(),
            });
        }

        Ok(project)
    }

    /// Every registered project, in the order the sidebar shows them.
    ///
    /// Most recently opened first, until someone drags the headings into an
    /// arrangement of their own — after which that arrangement is the order,
    /// and recency only places the projects it does not name. See
    /// [`Workspace::set_project_order`].
    ///
    /// Callers that want *recency* rather than *display order* have to say so:
    /// [`Workspace::resolve_project`] reads the timestamps itself for exactly
    /// that reason.
    pub fn projects(&self) -> Result<Vec<Project>> {
        Ok(sidebar_projects(&self.store.load()?))
    }

    /// Deregisters a project. The repository itself is untouched.
    ///
    /// Refuses while ket-managed worktrees remain, unless `force`. Silently
    /// orphaning worktrees would leave directories nothing knows how to clean up.
    pub fn remove_project(&self, id: &ProjectId, force: bool) -> Result<()> {
        let id = id.clone();

        self.store.update(|state| {
            if !state.projects.iter().any(|p| p.id == id) {
                return Err(KetError::UnknownProject(id.to_string()));
            }

            let live = state
                .worktrees
                .iter()
                .filter(|w| w.project_id == id)
                .count();
            if live > 0 && !force {
                return Err(KetError::Conflict(format!(
                    "project {id} still has {live} worktree(s); remove them first or use --force"
                )));
            }

            state.projects.retain(|p| p.id != id);
            state.worktrees.retain(|w| w.project_id != id);
            state.project_state.remove(&id);
            state.project_settings.remove(&id);
            state.trusted_automation.remove(&id);
            state.worktree_order.remove(&id);
            state.project_order.retain(|ordered| ordered != &id);
            Ok(())
        })?;

        self.bus.publish(Event::ProjectRemoved { project_id: id });
        Ok(())
    }

    /// This user's preferences for a project — defaults when none were set.
    ///
    /// A missing entry is the ordinary state, not an error; only an unknown
    /// project is refused, so a stale id cannot masquerade as a project with
    /// default settings.
    pub fn project_settings(&self, id: &ProjectId) -> Result<ProjectSettings> {
        let state = self.store.load()?;
        if !state.projects.iter().any(|p| &p.id == id) {
            return Err(KetError::UnknownProject(id.to_string()));
        }
        Ok(state.project_settings.get(id).cloned().unwrap_or_default())
    }

    /// Whether the repository's committed `.ket.toml` may execute commands.
    pub fn automation_trusted(&self, id: &ProjectId) -> Result<bool> {
        let state = self.store.load()?;
        if !state.projects.iter().any(|project| &project.id == id) {
            return Err(KetError::UnknownProject(id.to_string()));
        }
        Ok(state.trusted_automation.contains(id))
    }

    /// Grants or withdraws permission to execute repository-owned automation.
    pub fn set_automation_trusted(&self, id: &ProjectId, trusted: bool) -> Result<()> {
        let id = id.clone();
        self.store.update(|state| {
            if !state.projects.iter().any(|project| project.id == id) {
                return Err(KetError::UnknownProject(id.to_string()));
            }
            if trusted {
                state.trusted_automation.insert(id.clone());
            } else {
                state.trusted_automation.remove(&id);
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Which directories hold regenerable build output in this project.
    ///
    /// The project's own list when it has set one, else the global default.
    /// An empty list is a real answer and stays empty — a repository that
    /// commits its `dist/` has nothing here that is safe to clear, and
    /// falling back to the global list would be ket overriding a person who
    /// said so explicitly.
    pub fn build_dirs(&self, id: &ProjectId) -> Vec<String> {
        match self.project_settings(id) {
            Ok(ProjectSettings {
                build_dirs: Some(dirs),
                ..
            }) => dirs,
            _ => self.config.storage.build_dirs.clone(),
        }
    }

    /// Replaces a project's preferences.
    ///
    /// Validated and normalised before anything is written, so the store never
    /// holds a colour that will not parse or an icon over the limit — every
    /// reader would otherwise have to defend against both.
    pub fn set_project_settings(&self, id: &ProjectId, settings: ProjectSettings) -> Result<()> {
        let settings = settings.normalised();
        settings.validate()?;
        let id = id.clone();

        self.store.update(|state| {
            if !state.projects.iter().any(|p| p.id == id) {
                return Err(KetError::UnknownProject(id.to_string()));
            }
            if settings == ProjectSettings::default() {
                // Defaults are represented by absence, so a project put back
                // the way it came leaves no entry behind to grow stale.
                state.project_settings.remove(&id);
            } else {
                state.project_settings.insert(id.clone(), settings.clone());
            }
            Ok(())
        })?;

        self.bus
            .publish(Event::ProjectSettingsChanged { project_id: id });
        Ok(())
    }

    /// Changes the branch new worktrees are based on.
    ///
    /// Checked against the repository when it is reachable: a base that does
    /// not exist would fail at the next `worktree add` with git's message,
    /// far from the dialog where the mistake was made. An unavailable project
    /// cannot be checked and is not refused — the setting may be right for a
    /// repository that is merely unplugged.
    pub fn set_default_base(&self, id: &ProjectId, base: &str) -> Result<()> {
        let base = base.trim();
        if base.is_empty() {
            return Err(KetError::Config("default base cannot be empty".to_owned()));
        }

        let project = self
            .projects()?
            .into_iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| KetError::UnknownProject(id.to_string()))?;

        if project.is_available() && !Git::new(&project.root).branch_exists(base) {
            return Err(KetError::Config(format!(
                "{}: no branch named {base:?}",
                project.name
            )));
        }

        let id = id.clone();
        let base = base.to_owned();
        self.store.update(
            |state| match state.projects.iter_mut().find(|p| p.id == id) {
                Some(project) => {
                    project.default_base = base;
                    Ok(())
                }
                None => Err(KetError::UnknownProject(id.to_string())),
            },
        )?;

        self.bus
            .publish(Event::ProjectSettingsChanged { project_id: id });
        Ok(())
    }

    /// Changes which agent runs for a project when none is named.
    ///
    /// `None` clears the preference. A name is checked against the configured
    /// agents, because a preference for an agent that does not exist would
    /// surface as a failure to *run*, not as the typo it is.
    pub fn set_preferred_agent(&self, id: &ProjectId, agent: Option<&str>) -> Result<()> {
        let agent = agent.map(str::trim).filter(|a| !a.is_empty());
        if let Some(name) = agent
            && self.config.agent(name).is_none()
        {
            return Err(KetError::Config(format!(
                "no agent named {name:?} is configured"
            )));
        }

        let id = id.clone();
        let agent = agent.map(str::to_owned);
        self.store.update(
            |state| match state.projects.iter_mut().find(|p| p.id == id) {
                Some(project) => {
                    project.preferred_agent = agent;
                    Ok(())
                }
                None => Err(KetError::UnknownProject(id.to_string())),
            },
        )?;

        self.bus
            .publish(Event::ProjectSettingsChanged { project_id: id });
        Ok(())
    }

    /// The order the projects are shown in, as they were last arranged.
    ///
    /// Empty for a sidebar nobody has rearranged — which is not the same as an
    /// arrangement that happens to match recency, and is why this answers with
    /// what was stored rather than with the current list.
    pub fn project_order(&self) -> Result<Vec<ProjectId>> {
        Ok(self.store.load()?.project_order)
    }

    /// Records the order the projects are shown in.
    ///
    /// Ids that do not name a registered project are dropped rather than
    /// refused: the caller is a list that was just dragged, and a project
    /// removed in another window between the drag and the drop is an ordinary
    /// race, not a mistake worth failing the reorder over.
    ///
    /// An order that names nothing clears the record, so a sidebar put back
    /// the way it came goes back to recency — the rule
    /// [`Workspace::set_worktree_order`] follows for a project's rows.
    ///
    /// No event: every existing one names the project it is about, and this is
    /// about all of them at once. The sidebar rearranges itself before it
    /// calls, so there is nothing here for it to be told.
    pub fn set_project_order(&self, order: &[ProjectId]) -> Result<()> {
        let order = order.to_vec();

        self.store.update(|state| {
            let mut kept: Vec<ProjectId> = Vec::with_capacity(order.len());
            for wanted in &order {
                let known = state.projects.iter().any(|p| &p.id == wanted);
                if known && !kept.contains(wanted) {
                    kept.push(wanted.clone());
                }
            }
            state.project_order = kept;
            Ok(())
        })
    }

    /// Marks a project as just used, for recency ordering.
    pub fn touch_project(&self, id: &ProjectId) -> Result<()> {
        let id = id.clone();
        self.store.update(
            |state| match state.projects.iter_mut().find(|p| p.id == id) {
                Some(project) => {
                    project.last_opened_ms = crate::now_ms();
                    Ok(())
                }
                None => Err(KetError::UnknownProject(id.to_string())),
            },
        )
    }

    /// Resolves which project a command applies to.
    ///
    /// In order: an explicit id or name, then the project containing `cwd`, then
    /// the most recently opened. With many projects registered, that last
    /// fallback is what makes the tool usable without naming one every time.
    pub fn resolve_project(&self, explicit: Option<&str>, cwd: &Path) -> Result<Project> {
        let projects = self.projects()?;

        if let Some(wanted) = explicit {
            if let Some(found) = projects.iter().find(|p| p.id.as_str() == wanted) {
                return Ok(found.clone());
            }

            let by_name: Vec<_> = projects.iter().filter(|p| p.name == wanted).collect();
            return match by_name.as_slice() {
                [one] => Ok((*one).clone()),
                [] => Err(KetError::UnknownProject(wanted.to_owned())),
                many => Err(KetError::Conflict(format!(
                    "{wanted} is ambiguous across {} projects; use an id: {}",
                    many.len(),
                    many.iter()
                        .map(|p| p.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))),
            };
        }

        if let Some(found) = projects.iter().find(|p| p.contains(cwd)) {
            return Ok(found.clone());
        }

        // Read off the timestamps rather than taken from the head of the list.
        // `projects` is in the sidebar's order now, and a sidebar someone has
        // dragged into shape is a statement about where things sit, not about
        // where they last worked — so a project dragged to the top must not
        // become the one every bare `ket` command lands in.
        projects
            .into_iter()
            .min_by(|a, b| {
                b.last_opened_ms
                    .cmp(&a.last_opened_ms)
                    .then_with(|| a.name.cmp(&b.name))
            })
            .ok_or_else(|| {
                KetError::UnknownProject("no projects registered; run `ket project add`".to_owned())
            })
    }

    // ---- worktrees --------------------------------------------------------

    /// Creates a worktree on a new branch.
    ///
    /// Cleans up after itself: if the registry write fails after git has created
    /// the directory, the worktree is removed rather than orphaned on disk.
    pub fn create_worktree(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        agent: Option<&str>,
    ) -> Result<Worktree> {
        self.create_worktree_prepared(project_id, branch, base, agent)
            .map(|(worktree, _)| worktree)
    }

    /// [`Workspace::create_worktree`], also saying what the base went through
    /// on the way: whether the remote was fetched, and what became of the
    /// local base branch. Nothing in it is an error — the worktree exists —
    /// but "your main was three commits behind and has been brought up" is
    /// worth a line somewhere a person will read it.
    pub fn create_worktree_prepared(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(Worktree, PreparedBase)> {
        self.create_worktree_prepared_with_economy(project_id, branch, base, agent, None)
    }

    /// Creates a worktree with its Economy selection in the initial registry
    /// transaction, so creation can never succeed with a different policy than
    /// the dialog reported.
    pub fn create_worktree_prepared_with_economy(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        agent: Option<&str>,
        economy_level_id: Option<&str>,
    ) -> Result<(Worktree, PreparedBase)> {
        if branch.trim().is_empty() {
            return Err(KetError::Conflict("branch name is empty".to_owned()));
        }

        let state = self.store.load()?;
        let project = state
            .projects
            .iter()
            .find(|p| &p.id == project_id)
            .ok_or_else(|| KetError::UnknownProject(project_id.to_string()))?;

        if !project.is_available() {
            return Err(KetError::Conflict(format!(
                "project {} is not available at {}",
                project.name,
                project.root.display()
            )));
        }

        let git = Git::new(&project.root);
        if git.branch_exists(branch) {
            return Err(KetError::Conflict(format!(
                "branch {branch} already exists in {}",
                project.name
            )));
        }

        let id = worktree::id_for(project_id, branch);
        if state.worktrees.iter().any(|w| w.id == id) {
            return Err(KetError::Conflict(format!("worktree {id} already exists")));
        }

        let path = worktree::path_for(&self.worktrees_root, project_id, &id);
        if path.exists() {
            return Err(KetError::Conflict(format!(
                "{} already exists on disk",
                path.display()
            )));
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
        }

        let base = base.unwrap_or(&project.default_base).to_owned();

        // Fetch first and cut from the remote when it is ahead, so "main"
        // means the main everyone else can see rather than wherever this
        // clone's copy was last left. `base` itself is recorded as asked —
        // `main`, not `refs/remotes/origin/main` — because merges land on the
        // local branch and divergence is measured against it.
        let prepared = git.prepare_base(&base);
        if let Some(notice) = prepared.notice() {
            // Debug, not info: the callers that have somewhere a person will
            // read it — the CLI's stderr, the shell's log — say it there.
            tracing::debug!(project = %project.name, "{notice}");
        }

        // Resolved *before* the worktree exists, in the primary checkout, and
        // that ordering is the point: `HEAD` here means the repository's current
        // commit, whereas the same string read from inside the new worktree
        // would mean the branch we are about to create. Tags and raw object ids
        // need freezing for a different reason — they will never move, so
        // re-resolving them later buys nothing and can only fail.
        let base_commit = git.rev_parse(&prepared.rev).ok();

        git.worktree_add(&path, branch, &prepared.rev)?;
        if !matches!(prepared.fetch, Fetch::NoRemote) {
            git.ensure_push_auto_setup_remote();
        }

        let economy = economy_level_id
            .and_then(|id| {
                worktree::token_reduction_levels()
                    .iter()
                    .find(|level| level.id == id)
            })
            .unwrap_or_else(|| worktree::token_reduction(worktree::default_token_reduction()));
        if economy_level_id.is_some_and(|id| id != economy.id) {
            let _ = git.worktree_remove(&path, true);
            let _ = git.branch_delete(branch, true);
            return Err(KetError::Conflict("unknown Economy level".to_owned()));
        }

        let created = Worktree {
            id: id.clone(),
            project_id: project_id.clone(),
            branch: branch.to_owned(),
            name: None,
            agent_title: None,
            path,
            base,
            base_commit,
            // What the project says it is for. An explicit agent per worktree
            // is the next step; this is the value that makes the label true
            // today rather than empty.
            // What the caller chose, else what the project prefers.
            agent: agent
                .map(str::to_owned)
                .or_else(|| project.preferred_agent.clone()),
            created_at_ms: crate::now_ms(),
            provisioned_at_ms: None,
            provisioned_paths: Vec::new(),
            token_reduction: economy.level,
            token_reduction_id: Some(economy.id.clone()),
            token_reduction_pack_id: Some(worktree::active_pack_id()),
            pinned: false,
        };

        let record = created.clone();
        let focus = created.id.clone();
        let owner = project_id.clone();
        if let Err(e) = self.store.update(move |state| {
            state.worktrees.push(record);

            // A worktree you just made is one you are working in. Recorded in
            // the same transaction so the registry and the project's idea of
            // where you were can never disagree.
            let project_state = state.project_state.entry(owner).or_default();
            project_state.open.push(focus.clone());
            project_state.active = Some(focus);
            Ok(())
        }) {
            // Registry write failed. Undo the git side so we do not leave a
            // directory nothing knows about.
            tracing::error!(%e, "registry write failed; removing the worktree just created");
            let _ = git.worktree_remove(&created.path, true);
            let _ = git.branch_delete(branch, true);
            return Err(e);
        }

        // A blocking `worktree.created` hook must leave no half-made worktree
        // behind, so it is unwound exactly like the registry-write failure
        // above: drop the record just written, then undo the git side.
        if let Err(e) = self.run_hook(
            HookPoint::WorktreeCreated,
            state.trusted_automation.contains(&project.id),
            &project.root,
            serde_json::json!({
                "worktreeId": created.id,
                "projectId": created.project_id,
                "branch": created.branch,
                "base": created.base,
                "path": created.path,
            }),
        ) {
            tracing::error!(%e, "worktree.created hook blocked; removing the worktree just created");
            let forget = created.id.clone();
            let _ = self.store.update(move |state| {
                state.worktrees.retain(|w| w.id != forget);
                for project_state in state.project_state.values_mut() {
                    project_state.reconcile(&state.worktrees);
                }
                Ok(())
            });
            let _ = git.worktree_remove(&created.path, true);
            let _ = git.branch_delete(branch, true);
            return Err(e);
        }

        self.bus.publish(Event::WorktreeCreated {
            worktree_id: created.id.clone(),
            project_id: created.project_id.clone(),
            branch: created.branch.clone(),
        });
        self.announce_project_state(project_id)?;

        Ok((created, prepared))
    }

    /// Creates a worktree and provisions it, so an agent can work in it.
    ///
    /// This is the operation callers almost always want. A provisioning failure
    /// leaves the worktree in place but unprovisioned — the checkout is real and
    /// may hold work, so destroying it over a failed `npm install` would be
    /// worse than leaving it for `ket worktree provision` to retry.
    pub fn create_and_provision(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(Worktree, Option<ProvisionReport>)> {
        let worktree = self.create_worktree(project_id, branch, base, agent)?;
        Ok(self.provision_new(worktree))
    }

    /// The provisioning half of [`Workspace::create_and_provision`], for a
    /// caller that created the worktree itself and wants the same
    /// leave-it-in-place handling of a failure.
    pub fn provision_new(&self, worktree: Worktree) -> (Worktree, Option<ProvisionReport>) {
        match self.provision_worktree(&worktree.id) {
            Ok(report) => {
                let refreshed = self
                    .worktrees(None)
                    .ok()
                    .and_then(|all| all.into_iter().find(|w| w.id == worktree.id))
                    .unwrap_or(worktree);
                (refreshed, Some(report))
            }
            Err(e) => {
                tracing::warn!(%e, worktree = %worktree.id, "worktree created but not provisioned");
                (worktree, None)
            }
        }
    }

    /// Provisions an existing worktree.
    ///
    /// Idempotent, so it is safe to re-run after a failure. The registry is only
    /// marked provisioned once this succeeds.
    pub fn provision_worktree(&self, id: &WorktreeId) -> Result<ProvisionReport> {
        let state = self.store.load()?;
        let worktree = state
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;

        let project = state
            .projects
            .iter()
            .find(|p| p.id == worktree.project_id)
            .ok_or_else(|| KetError::UnknownProject(worktree.project_id.to_string()))?;

        if !project.is_available() {
            return Err(KetError::Conflict(format!(
                "project {} is not available at {}",
                project.name,
                project.root.display()
            )));
        }

        // Repo-local `.ket.toml` wins over the global config, so a project can
        // describe its own needs without every project sharing one shape.
        // Untrusted, the repository's file and directory strategies still
        // apply — they are declarative — but not its command. Nor does the
        // global one stand in for it: the repo's section replaces the global
        // config whole, so the global command never ran here.
        let config = match ProvisionConfig::repo_only(&project.root)? {
            Some(mut repo) if !state.trusted_automation.contains(&project.id) => {
                if !repo.post_command.is_empty() {
                    tracing::warn!(
                        project = %project.root.display(),
                        "ignoring repository post-command because automation is not trusted; \
                         run `ket project set {} --automation trust` to allow it",
                        project.id
                    );
                    repo.post_command.clear();
                }
                repo
            }
            Some(repo) => repo,
            None => self.config.provision.clone(),
        };

        self.bus.publish(Event::WorktreeProvisionStarted {
            worktree_id: id.clone(),
        });

        let bus = Arc::clone(&self.bus);
        let progress_id = id.clone();
        let result = provision::provision(&project.root, &worktree.path, &config, |step| {
            bus.publish(Event::WorktreeProvisionProgress {
                worktree_id: progress_id.clone(),
                step: step.to_owned(),
            });
        });

        let report = match result {
            Ok(report) => report,
            Err(e) => {
                self.bus.publish(Event::WorktreeProvisionFailed {
                    worktree_id: id.clone(),
                    why: e.to_string(),
                });
                return Err(e);
            }
        };

        // Gated the same way a failed provisioning step is: the registry is
        // never marked provisioned, so a retry of `ket worktree provision`
        // picks this back up. What provisioning already materialised on disk
        // is left in place rather than torn down — exactly what an ordinary
        // provisioning failure (a failing post-command, say) already does, and
        // the idempotent directory checks make a re-run safe either way.
        if let Err(e) = self.run_hook(
            HookPoint::WorktreeProvisioned,
            state.trusted_automation.contains(&project.id),
            &project.root,
            serde_json::json!({
                "worktreeId": id,
                "projectId": worktree.project_id,
                "durationMs": report.duration_ms,
                "degraded": report.degraded_to_full_copy(),
            }),
        ) {
            self.bus.publish(Event::WorktreeProvisionFailed {
                worktree_id: id.clone(),
                why: e.to_string(),
            });
            return Err(e);
        }

        let mark = id.clone();
        let created = report.created_paths();
        self.store.update(move |state| {
            if let Some(worktree) = state.worktrees.iter_mut().find(|w| w.id == mark) {
                worktree.provisioned_at_ms = Some(crate::now_ms());

                // Unioned, not replaced. A re-run reports the directories it
                // found already in place as `AlreadyPresent`, so overwriting
                // would forget what the first run made and leave it behind at
                // teardown.
                for path in created {
                    if !worktree.provisioned_paths.contains(&path) {
                        worktree.provisioned_paths.push(path);
                    }
                }
                worktree.provisioned_paths.sort();
            }
            Ok(())
        })?;

        if !report.uninitialised_submodules.is_empty() {
            // Not a failure: plenty of projects never touch their submodules.
            // But an agent that needs one gets an error about the submodule
            // rather than about its task, so this must be visible.
            tracing::warn!(
                worktree = %id,
                submodules = ?report.uninitialised_submodules,
                "worktree has uninitialised submodules; add a post_command of \
                 `git submodule update --init --recursive` if agents need them"
            );
        }

        if report.degraded_to_full_copy() {
            tracing::warn!(
                worktree = %id,
                "provisioning fell back to a full copy; copy-on-write unavailable on this volume"
            );
        }

        self.bus.publish(Event::WorktreeProvisionFinished {
            worktree_id: id.clone(),
            duration_ms: report.duration_ms,
            degraded: report.degraded_to_full_copy(),
        });

        Ok(report)
    }

    /// Worktrees, optionally filtered to one project.
    pub fn worktrees(&self, project_id: Option<&ProjectId>) -> Result<Vec<Worktree>> {
        let state = self.store.load()?;
        let mut worktrees = state.worktrees.clone();
        if let Some(id) = project_id {
            worktrees.retain(|w| &w.project_id == id);
        }
        worktrees.sort_by_key(|w| std::cmp::Reverse(w.created_at_ms));
        apply_worktree_order(&mut worktrees, &state.worktree_order);
        Ok(worktrees)
    }

    /// The order a project's worktrees are shown in, as it was last arranged.
    ///
    /// Empty for a project nobody has rearranged — which is not the same as
    /// an order that happens to match the default, and is why this answers
    /// with what was stored rather than with the current list.
    pub fn worktree_order(&self, id: &ProjectId) -> Result<Vec<WorktreeId>> {
        let state = self.store.load()?;
        if !state.projects.iter().any(|p| &p.id == id) {
            return Err(KetError::UnknownProject(id.to_string()));
        }
        Ok(state.worktree_order.get(id).cloned().unwrap_or_default())
    }

    /// Records the order this project's worktrees are shown in.
    ///
    /// Ids that do not name one of this project's worktrees are dropped
    /// rather than refused: the caller is a list that was just dragged, and a
    /// worktree removed in another window between the drag and the drop is an
    /// ordinary race, not a mistake worth failing the reorder over.
    ///
    /// An order that names nothing removes the entry, so a project put back
    /// the way it came leaves no record behind — the same rule
    /// [`Workspace::set_project_settings`] follows.
    pub fn set_worktree_order(&self, id: &ProjectId, order: &[WorktreeId]) -> Result<()> {
        let id = id.clone();
        let order = order.to_vec();

        self.store.update(|state| {
            if !state.projects.iter().any(|p| p.id == id) {
                return Err(KetError::UnknownProject(id.to_string()));
            }

            let mut kept: Vec<WorktreeId> = Vec::with_capacity(order.len());
            for wanted in &order {
                let mine = state
                    .worktrees
                    .iter()
                    .any(|w| &w.id == wanted && w.project_id == id);
                if mine && !kept.contains(wanted) {
                    kept.push(wanted.clone());
                }
            }

            if kept.is_empty() {
                state.worktree_order.remove(&id);
            } else {
                state.worktree_order.insert(id.clone(), kept);
            }
            Ok(())
        })?;

        self.bus
            .publish(Event::ProjectSettingsChanged { project_id: id });
        Ok(())
    }

    /// Worktrees ket manages, and the ones git knows about that it does not.
    ///
    /// [`Workspace::worktrees`] answers "what did ket create" — the right
    /// question for a registry, which deliberately never scans disk. But a
    /// worktree made by a plain `git worktree add`, by another tool, or by an
    /// agent's own tooling is then invisible to it, and a UI that silently
    /// omits such worktrees looks broken in any repo where something else
    /// touched `git worktree`. This reconciles the two views in one call, so a
    /// client never computes the difference itself — invariant 1 keeps that
    /// computation in core, not in the shell.
    ///
    /// The primary checkout is excluded from `discovered` entirely: it is the
    /// repository's own working tree, not a worktree anyone forgot about, and a
    /// client that wants to show it does so from [`Project::root`] directly. A
    /// project whose repository has moved or gone reports its managed
    /// worktrees as usual and an empty `discovered`, matching how
    /// [`Project::is_available`] is treated everywhere else — an ordinary
    /// state, not an error.
    pub fn worktree_report(&self, project_id: Option<&ProjectId>) -> Result<WorktreeReport> {
        let state = self.store.load()?;

        let mut managed = state.worktrees.clone();
        if let Some(id) = project_id {
            managed.retain(|w| &w.project_id == id);
        }
        sort_worktrees(&mut managed, &state.worktree_order);

        let mut discovered = Vec::new();
        for project in state
            .projects
            .iter()
            .filter(|p| project_id.is_none_or(|id| &p.id == id))
        {
            if !project.is_available() {
                continue;
            }

            let Ok(known) = Git::new(&project.root).worktree_list() else {
                continue;
            };

            let root = canonical(&project.root);
            let ours: Vec<PathBuf> = managed
                .iter()
                .filter(|w| w.project_id == project.id)
                .map(|w| canonical(&w.path))
                .collect();

            for entry in known {
                // A bare entry is the repository's administrative record, not a
                // checkout anyone works in.
                if entry.bare {
                    continue;
                }

                let path = canonical(&entry.path);
                if path == root || ours.contains(&path) {
                    continue;
                }

                discovered.push(DiscoveredWorktree {
                    project_id: project.id.clone(),
                    path,
                    branch: entry.branch_name().map(str::to_owned),
                    head: entry.head.clone(),
                });
            }
        }

        Ok(WorktreeReport {
            managed,
            discovered,
        })
    }

    /// Takes a worktree git already has into ket's registry.
    ///
    /// The counterpart to [`Self::create_worktree`] for a checkout that already
    /// exists — one made by hand, by another tool, or by a ket that has since
    /// forgotten it. Nothing on disk is touched: this writes the record that
    /// turns a row under "discovered" into one the sidebar can open, diff and
    /// collapse like any other.
    ///
    /// Refused for a detached checkout. Every operation past this point — the
    /// diff against a base, the collapse back into it, the resume of a session
    /// keyed on a branch — needs a branch name, and inventing one for a
    /// detached HEAD would produce a worktree that looks adoptable and fails at
    /// the first thing anyone asks of it.
    ///
    /// `base_commit` is the merge base rather than wherever the base points
    /// now: an adopted worktree diverged at some point in the past, and
    /// recording today's tip would report every commit made on the base since
    /// then as this worktree's own work.
    pub fn adopt_worktree(&self, project_id: &ProjectId, path: &Path) -> Result<Worktree> {
        let state = self.store.load()?;
        let project = state
            .projects
            .iter()
            .find(|p| &p.id == project_id)
            .ok_or_else(|| KetError::UnknownProject(project_id.to_string()))?;

        if !project.is_available() {
            return Err(KetError::Conflict(format!(
                "project {} is not available at {}",
                project.name,
                project.root.display()
            )));
        }

        let wanted = canonical(path);
        let git = Git::new(&project.root);

        // Asked of git rather than taken from the caller: the sidebar's row was
        // built from a report that may be seconds old, and adopting a worktree
        // that has been removed since would write a record for a directory that
        // is not there.
        let entry = git
            .worktree_list()?
            .into_iter()
            .find(|entry| canonical(&entry.path) == wanted)
            .ok_or_else(|| {
                KetError::Conflict(format!(
                    "{} is not a worktree of {}",
                    wanted.display(),
                    project.name
                ))
            })?;

        let branch = entry.branch_name().map(str::to_owned).ok_or_else(|| {
            KetError::Conflict(format!(
                "{} is a detached checkout, so there is no branch to adopt",
                wanted.display()
            ))
        })?;

        if let Some(known) = state
            .worktrees
            .iter()
            .find(|w| canonical(&w.path) == wanted)
        {
            return Ok(known.clone());
        }

        let id = worktree::id_for(project_id, &branch);
        if state.worktrees.iter().any(|w| w.id == id) {
            return Err(KetError::Conflict(format!(
                "a worktree for {branch} is already registered"
            )));
        }

        let base = project.default_base.clone();
        let base_commit = git
            .merge_base(&base, &branch)
            .ok()
            .or_else(|| git.rev_parse(&base).ok());

        let adopted = Worktree {
            id: id.clone(),
            project_id: project_id.clone(),
            branch,
            name: None,
            agent_title: None,
            path: wanted,
            base,
            base_commit,
            agent: project.preferred_agent.clone(),
            // When ket learned of it, which is all it can honestly say: the
            // directory's own timestamps are the checkout's, not the branch's.
            created_at_ms: crate::now_ms(),
            // Emphatically not provisioned. A checkout somebody else set up may
            // be perfectly ready, but ket did not do it and must not claim to
            // have: `provision` is idempotent and re-running it is cheap, while
            // starting an agent in an unprepared tree is not.
            provisioned_at_ms: None,
            provisioned_paths: Vec::new(),
            token_reduction: worktree::default_token_reduction(),
            token_reduction_id: worktree::token_reduction(worktree::default_token_reduction())
                .id
                .clone()
                .into(),
            token_reduction_pack_id: Some(worktree::active_pack_id()),
            pinned: false,
        };

        let record = adopted.clone();
        let focus = adopted.id.clone();
        let owner = project_id.clone();
        self.store.update(move |state| {
            state.worktrees.push(record);
            let project_state = state.project_state.entry(owner).or_default();
            project_state.open.push(focus.clone());
            project_state.active = Some(focus);
            Ok(())
        })?;

        self.bus.publish(Event::WorktreeCreated {
            worktree_id: adopted.id.clone(),
            project_id: adopted.project_id.clone(),
            branch: adopted.branch.clone(),
        });
        self.announce_project_state(project_id)?;

        Ok(adopted)
    }

    /// Removes a worktree and, optionally, its branch.
    ///
    /// Without `force`, uncommitted changes are refused. That refusal is
    /// deliberate: discarding an agent's unreviewed work by accident is the
    /// worst thing this tool could do. It is checked here, before anything is
    /// touched, rather than being left entirely to git — see
    /// [`crate::provision::teardown`] for why the order matters.
    pub fn remove_worktree(
        &self,
        id: &WorktreeId,
        force: bool,
        delete_branch: bool,
    ) -> Result<RemovalReport> {
        self.remove_worktree_inner(id, force, delete_branch, false)
    }

    /// [`Self::remove_worktree`], plus whether the branch may be forced.
    ///
    /// The two forces are separate decisions and collapsing them would be a
    /// bug in both directions. `force` waives a *dirty checkout*, which is
    /// what someone answers "delete anyway" to; `force_branch_delete` waives
    /// *unmerged commits*, which they were never asked about. Only
    /// [`Self::collapse`] passes it, because discarding the attempt is the
    /// whole point of what it is doing.
    fn remove_worktree_inner(
        &self,
        id: &WorktreeId,
        force: bool,
        delete_branch: bool,
        force_branch_delete: bool,
    ) -> Result<RemovalReport> {
        let state = self.store.load()?;
        let worktree = state
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;

        let project = state
            .projects
            .iter()
            .find(|p| p.id == worktree.project_id)
            .ok_or_else(|| KetError::UnknownProject(worktree.project_id.to_string()))?;

        if !force && worktree.exists() {
            self.refuse_if_removal_would_lose_work(worktree)?;
        }

        // Fired before anything is touched, so a block leaves the worktree
        // fully intact — nothing has been torn down or removed yet.
        self.run_hook(
            HookPoint::WorktreeRemoved,
            state.trusted_automation.contains(&project.id),
            &project.root,
            serde_json::json!({
                "worktreeId": worktree.id,
                "projectId": worktree.project_id,
                "branch": worktree.branch,
                "path": worktree.path,
            }),
        )?;

        // Undo provisioning first. Everything this removes is a copy of, or a
        // link to, something the primary checkout still has.
        self.teardown_worktree(worktree)?;

        // A project whose repo has gone still needs its registry entry cleared,
        // so an unavailable repo drops the record rather than blocking forever.
        let mut report = RemovalReport::default();

        if project.is_available() {
            let git = Git::new(&project.root);
            report.deferred = self.remove_checkout(&git, &worktree.path, force)?;

            if delete_branch {
                report.preserved_branch =
                    delete_branch_after_removal(&git, &worktree.branch, force_branch_delete);
            }
        } else {
            tracing::warn!(
                project = %project.name,
                "project repository is unavailable; dropping the worktree record only"
            );
        }

        let worktree_path = worktree.path.clone();
        let project_id = worktree.project_id.clone();

        let id = id.clone();
        let forget = id.clone();
        self.store.update(move |state| {
            state.worktrees.retain(|w| w.id != forget);
            for project_state in state.project_state.values_mut() {
                project_state.reconcile(&state.worktrees);
            }
            Ok(())
        })?;

        self.prune_empty_project_dir(&worktree_path);

        self.bus.publish(Event::WorktreeRemoved { worktree_id: id });
        self.announce_project_state(&project_id)?;
        Ok(report)
    }

    /// Removes the checkout, deferring the expensive part when it can.
    ///
    /// Returns whether the delete was deferred. Falling back is not a failure:
    /// a rename that cannot happen — a trash root on another filesystem, a
    /// worktree git has locked — just means the caller waits for the delete
    /// the way it always used to, and git reports its own refusal in its own
    /// words rather than one invented here.
    fn remove_checkout(&self, git: &Git, path: &Path, force: bool) -> Result<bool> {
        // A checkout that is already gone has nothing to rename and nothing for
        // git to remove: `git worktree remove` on a path that is not a working
        // tree is fatal, and git prunes a registration whose directory has
        // vanished the next time anything lists worktrees — so by the time
        // someone clicks delete, there is usually no registration left either.
        //
        // Letting that be an error stranded the row forever. The sidebar shows
        // it as `missing`, deleting it failed every time, and the only way to
        // be rid of what is *ket's own record* was `ket worktree prune`, which
        // is not a thing the row says about itself. The record is the last
        // thing standing here, and dropping it is the whole point of the
        // action.
        if !path.is_dir() {
            if let Err(e) = clear_registration(git, path) {
                tracing::warn!(
                    %e,
                    path = %path.display(),
                    "checkout was already gone; dropping the record anyway"
                );
            }
            return Ok(false);
        }

        let Some(trash) = worktree_trash::move_to_trash(path) else {
            git.worktree_remove(path, force)?;
            return Ok(false);
        };

        if let Err(e) = clear_registration(git, path) {
            // Put the checkout back, so the removal below sees the worktree
            // git still has registered. If even that fails, the two are now
            // inconsistent and saying so is all that is left.
            if !worktree_trash::restore_from_trash(&trash, path) {
                return Err(e);
            }
            tracing::warn!(%e, "deferred worktree removal failed; deleting in place");
            git.worktree_remove(path, force)?;
            return Ok(false);
        }

        worktree_trash::schedule_deletion(trash);
        Ok(true)
    }

    /// Refuses a non-forced removal that would discard work.
    ///
    /// Two conditions, both checked here rather than left to git's own refusal:
    ///
    /// - **Changes that are not provisioning's doing.** Git would refuse anyway,
    ///   but only *after* [`Workspace::teardown_worktree`] had already stripped
    ///   the environment out of a worktree that then survives.
    /// - **Initialised submodules.** Git refuses to remove such a worktree at
    ///   all, with a message that says nothing about what to do next. Saying so
    ///   here, and naming `--force`, is the difference between a dead end and an
    ///   instruction.
    fn refuse_if_removal_would_lose_work(&self, worktree: &Worktree) -> Result<()> {
        if let Ok(submodules) = Git::new(&worktree.path).submodule_status() {
            let live: Vec<&str> = submodules
                .iter()
                .filter(|s| s.initialised)
                .map(|s| s.path.as_str())
                .collect();

            if !live.is_empty() {
                return Err(KetError::Conflict(format!(
                    "{} has checked-out submodule(s) ({}); git refuses to remove \
                     worktrees containing them, so this needs --force",
                    worktree.id,
                    live.join(", ")
                )));
            }
        }

        // A status read can fail for reasons that have nothing to do with the
        // worktree being dirty — a repository format gix cannot read, say. Let
        // git have the final word in that case rather than refusing on a guess.
        let Ok(status) = status::of_worktree(&worktree.path, None) else {
            tracing::debug!(worktree = %worktree.id, "status unavailable; deferring to git");
            return Ok(());
        };

        let blocking = status
            .files
            .iter()
            .filter(|file| !was_provisioned(&file.path, &worktree.provisioned_paths))
            .count();

        if blocking > 0 {
            return Err(KetError::Conflict(format!(
                "{} has {blocking} uncommitted change(s); use --force to discard them",
                worktree.id
            )));
        }

        Ok(())
    }

    /// Removes what provisioning created, and records that it is gone.
    ///
    /// The registry is updated even when removal later fails, so a worktree that
    /// survives a refused `rm` is visibly unprovisioned rather than claiming an
    /// environment it no longer has.
    fn teardown_worktree(&self, worktree: &Worktree) -> Result<()> {
        if worktree.provisioned_paths.is_empty() || !worktree.exists() {
            return Ok(());
        }

        let report = provision::teardown(&worktree.path, &worktree.provisioned_paths, |_| {});

        for failure in &report.failed {
            tracing::warn!(
                worktree = %worktree.id,
                path = %failure.path,
                why = %failure.why,
                "could not remove a provisioned path"
            );
        }

        let id = worktree.id.clone();
        self.store.update(move |state| {
            if let Some(worktree) = state.worktrees.iter_mut().find(|w| w.id == id) {
                worktree.provisioned_at_ms = None;
                worktree.provisioned_paths.clear();
            }
            Ok(())
        })
    }

    /// Removes a worktree's per-project directory once it holds nothing.
    ///
    /// Without this, every project ket has ever created a worktree for leaves an
    /// empty directory under the worktrees root forever. Harmless individually,
    /// but this is a tool built for working across many projects.
    fn prune_empty_project_dir(&self, worktree_path: &Path) {
        let Some(parent) = worktree_path.parent() else {
            return;
        };

        // Only ever inside our own root, and never the root itself.
        if !parent.starts_with(&self.worktrees_root) || parent == self.worktrees_root {
            return;
        }

        let is_empty = std::fs::read_dir(parent)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);

        if is_empty {
            let _ = std::fs::remove_dir(parent);
        }
    }

    /// The checks every merge shares, in the order they matter.
    ///
    /// Deliberately *not* here: whether the worktree is dirty, whether the
    /// primary checkout has the base out, and whether it is clean.
    /// [`Self::collapse`] refuses on the first of those; [`Self::merge_worktree`]
    /// exists to fix it by committing. Keeping them at the call sites is what
    /// makes the two orderings readable side by side.
    fn prepare_merge(&self, state: &crate::store::State, id: &WorktreeId) -> Result<MergePlan> {
        let worktree = state
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?
            .clone();

        let project = state
            .projects
            .iter()
            .find(|p| p.id == worktree.project_id)
            .ok_or_else(|| KetError::UnknownProject(worktree.project_id.to_string()))?
            .clone();

        if !project.is_available() {
            return Err(KetError::Conflict(format!(
                "{}'s repository is not available, so there is nothing to merge into",
                project.name
            )));
        }

        let git = Git::new(&project.root);
        let base = worktree.base.clone();

        // A tag, a raw commit, or a detached HEAD is a legitimate base to branch
        // *from* and an impossible thing to merge *into*.
        if base.is_empty() || base == "HEAD" || !git.branch_exists(&base) {
            return Err(KetError::Conflict(format!(
                "{} was branched from `{base}`, which is not a branch that can be merged into",
                worktree.id
            )));
        }

        Ok(MergePlan {
            worktree,
            project,
            git,
            base,
        })
    }

    /// Commits a worktree's uncommitted work and merges its branch into its base.
    ///
    /// The sibling worktrees are left alone. That is the whole difference from
    /// [`Self::collapse`], and it is the difference between a quick action on
    /// one row and the operation that ends a race between several.
    ///
    /// Everything but `force` comes from `config.merge`, because those settings
    /// *are* this operation's configuration and a second copy of them in an
    /// options struct is a second thing to keep in step.
    ///
    /// The commit happens **before** the primary checkout is inspected: it is
    /// what makes the "uncommitted changes would not be merged" refusal moot,
    /// so asking first would refuse work this method was called to do.
    pub fn merge_worktree(
        &self,
        id: &WorktreeId,
        force: bool,
    ) -> std::result::Result<MergeReport, MergeError> {
        let state = self.store.load()?;
        let MergePlan {
            worktree,
            project,
            git,
            base,
        } = self.prepare_merge(&state, id)?;

        // One read of the worktree, used for three decisions. Skipped entirely
        // when the directory is gone: there is then nothing to commit, and the
        // branch may still be worth merging.
        let status = worktree
            .exists()
            .then(|| crate::status::of_worktree(&worktree.path, None))
            .transpose()?;

        if let Some(status) = &status {
            // Committing on top of a half-applied rebase is how work is lost,
            // and `--force` cannot make that safe. `collapse` lacks this check
            // and should have it.
            if let Some(operation) = status.in_progress {
                return Err(MergeError::Failed(KetError::Conflict(format!(
                    "{} is in the middle of a {}; finish or abort it in that worktree first",
                    worktree.id,
                    operation.label(),
                ))));
            }

            if let Some(conflicted) = status
                .files
                .iter()
                .find(|file| file.kind == crate::status::ChangeKind::Conflicted)
            {
                return Err(MergeError::Failed(KetError::Conflict(format!(
                    "{} has an unresolved conflict in `{}`; resolve it before merging",
                    worktree.id, conflicted.path,
                ))));
            }
        }

        let committed = match status {
            Some(status) if status.total_changes > 0 => {
                let message = self.config.merge.commit_message(
                    &worktree.branch,
                    &base,
                    worktree.id.as_str(),
                    status.total_changes,
                );
                Some(Git::new(&worktree.path).commit_all(&message)?)
            }
            _ => None,
        };

        let current = git.current_branch()?;
        if current.as_deref() != Some(base.as_str()) {
            // Not waivable, and deliberately not fixed by checking `base` out:
            // switching the reader's primary checkout out from under them,
            // possibly with an editor open on it, is not something a one-click
            // action gets to do.
            return Err(MergeError::Failed(KetError::Conflict(format!(
                "{} has `{}` checked out, not `{base}`; check out `{base}` there before merging into it",
                project.name,
                current.as_deref().unwrap_or("a detached HEAD"),
            ))));
        }

        if git.is_dirty()? && !force {
            return Err(MergeError::Waivable(format!(
                "{} has uncommitted changes on `{base}`. The merge lands on top of them.",
                project.name
            )));
        }

        let already_merged = git.is_ancestor(&worktree.branch, &base);
        if !already_merged {
            git.merge(
                &worktree.branch,
                &format!("ket: merge {} into {base}", worktree.branch),
            )
            .map_err(|e| {
                // git reports a conflict on *stdout* and leaves stderr empty, so
                // the raw error here is a command line and an exit status with
                // nothing explaining either. A conflict is also much the most
                // likely reason to be here, everything else having been checked.
                // `Git::merge` has already aborted, so the promise in the last
                // sentence is one this code keeps.
                let detail = match &e {
                    KetError::Git { stderr, .. } if !stderr.is_empty() => format!(": {stderr}"),
                    _ => String::new(),
                };
                MergeError::Failed(KetError::Conflict(format!(
                    "could not merge `{}` into `{base}`{detail}. It most likely conflicts; \
                     resolve it in the worktree with git directly. Nothing was changed.",
                    worktree.branch,
                )))
            })?;
        }

        // Unforced: everything was just committed, so it cannot be dirty. If it
        // somehow is, refusing is right — and the merge has already landed, so
        // this is reported rather than returned as a failure.
        let mut removed = false;
        let mut remove_failed = None;
        if !self.config.merge.keep_after_merge {
            match self.remove_worktree(&worktree.id, false, false) {
                Ok(_) => removed = true,
                Err(e) => remove_failed = Some(e.to_string()),
            }
        }

        Ok(MergeReport {
            worktree: worktree.id,
            branch: worktree.branch,
            into: base,
            committed,
            already_merged,
            removed,
            remove_failed,
        })
    }

    /// Merges one attempt into its base and discards the ones that lost.
    ///
    /// This is the end of the loop the whole tool exists for: several agents
    /// worked the same task in parallel worktrees, one of them won, and the
    /// rest are now waste. Doing it by hand is a merge plus N removals plus N
    /// branch deletions, which is exactly the kind of tedium that stops people
    /// running three agents in the first place.
    ///
    /// Refuses more than it forces. Merging a *branch* takes only what was
    /// committed, so a winner with uncommitted changes would be silently
    /// half-merged — the worst outcome available, and the one this checks for
    /// first.
    pub fn collapse(&self, winner: &WorktreeId, opts: CollapseOptions) -> Result<CollapseReport> {
        let state = self.store.load()?;
        let MergePlan {
            worktree,
            project,
            git,
            base,
        } = self.prepare_merge(&state, winner)?;

        // Uncommitted work in the winner is not part of its branch and would not
        // survive the merge.
        if worktree.exists() && Git::new(&worktree.path).is_dirty()? && !opts.force {
            return Err(KetError::Conflict(format!(
                "{} has uncommitted changes, which merging its branch would not include; \
                 commit them first, or pass --force to merge only what is committed",
                worktree.id
            )));
        }

        let current = git.current_branch()?;
        if current.as_deref() != Some(base.as_str()) {
            return Err(KetError::Conflict(format!(
                "{} has `{}` checked out, not `{base}`; check out `{base}` before collapsing into it",
                project.name,
                current.as_deref().unwrap_or("a detached HEAD")
            )));
        }

        if git.is_dirty()? && !opts.force {
            return Err(KetError::Conflict(format!(
                "{} has uncommitted changes; commit or stash them before merging into `{base}`",
                project.name
            )));
        }

        // Distinguish "already contained" from "merged now", because to someone
        // deciding whether their agent produced anything, those are different
        // answers and git reports both as success.
        let already_merged = git.is_ancestor(&worktree.branch, &base);
        if !already_merged {
            git.merge(
                &worktree.branch,
                &format!("ket: collapse {} into {base}", worktree.branch),
            )?;
        }

        let mut discarded = Vec::new();
        if !opts.keep_losers {
            let losers: Vec<WorktreeId> = state
                .worktrees
                .iter()
                .filter(|w| w.project_id == worktree.project_id && w.id != worktree.id)
                .map(|w| w.id.clone())
                .collect();

            for loser in losers {
                // Forced: a losing attempt is expected to be dirty, and having
                // just merged the winner, refusing to clean up would leave the
                // command half-done.
                self.remove_worktree_inner(&loser, true, true, true)?;
                discarded.push(loser);
            }
        }

        if !opts.keep_winner {
            self.remove_worktree_inner(&worktree.id, true, true, true)?;
        }

        Ok(CollapseReport {
            merged: worktree.branch,
            into: base,
            discarded,
            already_merged,
            winner_kept: opts.keep_winner,
        })
    }

    /// Reconciles the registry against what is actually on disk.
    ///
    /// Returns the worktrees that were dropped. Directories vanish for ordinary
    /// reasons — a manual `rm -rf`, a git operation elsewhere — and the registry
    /// has to be able to catch up without a reset.
    pub fn prune(&self) -> Result<Vec<WorktreeId>> {
        let state = self.store.load()?;

        for project in &state.projects {
            if project.is_available()
                && let Err(e) = Git::new(&project.root).worktree_prune()
            {
                tracing::warn!(%e, project = %project.name, "git worktree prune failed");
            }
        }

        let gone: Vec<WorktreeId> = state
            .worktrees
            .iter()
            .filter(|w| !w.exists())
            .map(|w| w.id.clone())
            .collect();

        if gone.is_empty() {
            return Ok(gone);
        }

        let removing = gone.clone();
        self.store.update(move |state| {
            state.worktrees.retain(|w| !removing.contains(&w.id));
            for project_state in state.project_state.values_mut() {
                project_state.reconcile(&state.worktrees);
            }
            Ok(())
        })?;

        for worktree in state.worktrees.iter().filter(|w| gone.contains(&w.id)) {
            self.prune_empty_project_dir(&worktree.path);
        }

        for id in &gone {
            self.bus.publish(Event::WorktreeRemoved {
                worktree_id: id.clone(),
            });
        }

        Ok(gone)
    }
    // ---- per-project durable state ----------------------------------------

    /// Where you were in a project when you last left it.
    ///
    /// Reconciled against the live registry on every read, so a worktree that
    /// has since been removed — by ket, by hand, or by git — cannot linger here
    /// as a reference to something that is gone.
    pub fn project_state(&self, project_id: &ProjectId) -> Result<ProjectState> {
        let state = self.store.load()?;

        let mut project_state = state
            .project_state
            .get(project_id)
            .cloned()
            .unwrap_or_default();

        let live: Vec<Worktree> = state
            .worktrees
            .into_iter()
            .filter(|w| &w.project_id == project_id)
            .collect();
        project_state.reconcile(&live);

        Ok(project_state)
    }

    /// Marks a worktree as open in its project, and gives it focus.
    ///
    /// Idempotent: opening an already-open worktree just moves focus to it.
    pub fn open_worktree(&self, id: &WorktreeId) -> Result<()> {
        let project_id = self.mutate_project_state(id, |project_state, id| {
            if !project_state.open.contains(id) {
                project_state.open.push(id.clone());
            }
            project_state.active = Some(id.clone());
        })?;

        self.announce_project_state(&project_id)
    }

    /// Marks a worktree as closed. The worktree itself is untouched.
    ///
    /// Focus moves to whatever else is still open — see
    /// [`ProjectState::reconcile`], which runs straight after this and owns that
    /// rule for every path that can close a worktree, deletion included.
    pub fn close_worktree(&self, id: &WorktreeId) -> Result<()> {
        let project_id = self.mutate_project_state(id, |project_state, id| {
            project_state.open.retain(|open| open != id);
        })?;

        self.announce_project_state(&project_id)
    }

    /// A client's view state for the window itself, as it was last stored.
    ///
    /// Not a project's — see [`crate::store::State::view`] for why the two are
    /// separate — and so not reconciled against anything: what is in it is the
    /// client's vocabulary, and only the client can say which parts of it have
    /// gone stale.
    pub fn view_state(&self) -> Result<Option<serde_json::Value>> {
        Ok(self.store.load()?.view)
    }

    /// Stores the client's view state for the window, verbatim.
    ///
    /// Bounded like [`Workspace::set_layout`], and for the same reason: nothing
    /// else would notice a client growing this without bound.
    pub fn set_view_state(&self, view: Option<serde_json::Value>) -> Result<()> {
        if let Some(view) = &view {
            let size = serde_json::to_vec(view)
                .map_err(|e| KetError::Config(format!("serialising view state: {e}")))?
                .len();

            if size > MAX_LAYOUT_BYTES {
                return Err(KetError::Conflict(format!(
                    "view state is {size} bytes; the limit is {MAX_LAYOUT_BYTES}"
                )));
            }
        }

        self.store.update(move |state| {
            state.view = view;
            Ok(())
        })
    }

    /// Stores a client's view state for a project, verbatim.
    ///
    /// Core never looks inside it — a layout schema here would be exactly the
    /// UI-specific knowledge invariant 3 keeps out of `ket-core`. What core does
    /// enforce is a size limit, because nothing else would notice a client
    /// growing one without bound.
    pub fn set_layout(
        &self,
        project_id: &ProjectId,
        layout: Option<serde_json::Value>,
    ) -> Result<()> {
        if let Some(layout) = &layout {
            let size = serde_json::to_vec(layout)
                .map_err(|e| KetError::Config(format!("serialising layout: {e}")))?
                .len();

            if size > MAX_LAYOUT_BYTES {
                return Err(KetError::Conflict(format!(
                    "layout is {size} bytes; the limit is {MAX_LAYOUT_BYTES}"
                )));
            }
        }

        let id = project_id.clone();
        self.store.update(move |state| {
            if !state.projects.iter().any(|p| p.id == id) {
                return Err(KetError::UnknownProject(id.to_string()));
            }
            state.project_state.entry(id).or_default().layout = layout;
            Ok(())
        })
    }

    /// Sets how aggressively agents launched in this worktree should cut
    /// their own token usage — see [`Worktree::token_reduction`].
    ///
    /// A setter rather than a `create_worktree` parameter: every existing
    /// caller of `create_worktree` (the CLI, every integration test) would
    /// otherwise need to learn about a level it has no opinion on the moment
    /// this field was added.
    pub fn set_token_reduction(&self, id: &WorktreeId, level: u8) -> Result<()> {
        let level = worktree::token_reduction_levels()
            .iter()
            .find(|candidate| candidate.level == level)
            .ok_or_else(|| KetError::Conflict(format!("unknown Economy level `{level}`")))?;
        self.set_economy_level(id, &level.id)
    }

    /// Sets a worktree's Economy level by its stable id, recording the active
    /// pack family and numeric compatibility value atomically.
    pub fn set_economy_level(&self, id: &WorktreeId, level_id: &str) -> Result<()> {
        let level = worktree::token_reduction_levels()
            .iter()
            .find(|level| level.id == level_id)
            .ok_or_else(|| KetError::Conflict(format!("unknown Economy level `{level_id}`")))?;
        let level_number = level.level;
        let level_id = level.id.clone();
        let pack_id = worktree::active_pack_id();
        let id = id.clone();
        self.store.update(move |state| {
            let worktree = state
                .worktrees
                .iter_mut()
                .find(|w| w.id == id)
                .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;
            worktree.token_reduction = level_number;
            worktree.token_reduction_id = Some(level_id);
            worktree.token_reduction_pack_id = Some(pack_id);
            Ok(())
        })
    }

    /// Changes only the worktree's display name, leaving its git branch alone.
    /// Blank names restore the branch as the row label.
    pub fn set_worktree_name(&self, id: &WorktreeId, name: &str) -> Result<()> {
        let id = id.clone();
        let name = clean_worktree_name(name);
        self.store.update(move |state| {
            let worktree = state
                .worktrees
                .iter_mut()
                .find(|worktree| worktree.id == id)
                .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;
            worktree.name = name;
            Ok(())
        })
    }

    /// Pins or unpins a worktree — see [`Worktree::pinned`].
    pub fn set_worktree_pinned(&self, id: &WorktreeId, pinned: bool) -> Result<()> {
        let id = id.clone();
        self.store.update(move |state| {
            let worktree = state
                .worktrees
                .iter_mut()
                .find(|w| w.id == id)
                .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;
            worktree.pinned = pinned;
            Ok(())
        })
    }

    /// Imports a title created by an agent's rename command.
    ///
    /// Returns whether the stored name changed. Repeated polls of the same
    /// title do nothing, including after a person has manually renamed the row.
    pub fn sync_worktree_name_from_agent(&self, id: &WorktreeId, title: &str) -> Result<bool> {
        let id = id.clone();
        let Some(title) = clean_worktree_name(title) else {
            return Ok(false);
        };
        let current = self.store.load()?;
        let worktree = current
            .worktrees
            .iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;
        if worktree.agent_title.as_deref() == Some(title.as_str()) {
            return Ok(false);
        }
        self.store.update(move |state| {
            let worktree = state
                .worktrees
                .iter_mut()
                .find(|worktree| worktree.id == id)
                .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;
            if worktree.agent_title.as_deref() == Some(title.as_str()) {
                return Ok(false);
            }
            worktree.name = Some(title.clone());
            worktree.agent_title = Some(title);
            Ok(true)
        })
    }

    /// Applies `change` to the project owning `id`, returning that project.
    fn mutate_project_state(
        &self,
        id: &WorktreeId,
        change: impl FnOnce(&mut ProjectState, &WorktreeId),
    ) -> Result<ProjectId> {
        let id = id.clone();

        self.store.update(move |state| {
            let project_id = state
                .worktrees
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.project_id.clone())
                .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;

            let project_state = state.project_state.entry(project_id.clone()).or_default();
            change(project_state, &id);
            project_state.reconcile(&state.worktrees);

            Ok(project_id)
        })
    }

    /// Publishes a project's current durable state.
    fn announce_project_state(&self, project_id: &ProjectId) -> Result<()> {
        let project_state = self.project_state(project_id)?;

        self.bus.publish(Event::ProjectStateChanged {
            project_id: project_id.clone(),
            open: project_state.open,
            active: project_state.active,
        });

        Ok(())
    }

    // ---- read side --------------------------------------------------------

    /// Reads a worktree's changed files and divergence from its base.
    ///
    /// Computed on demand and not cached: an idle worktree holds metadata only.
    pub fn worktree_status(&self, id: &WorktreeId) -> Result<WorktreeStatus> {
        let state = self.store.load()?;
        let worktree = state
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;

        if !worktree.exists() {
            return Err(KetError::Conflict(format!(
                "{} is registered but its directory is gone; run `ket worktree prune`",
                worktree.id
            )));
        }

        status::of_worktree(&worktree.path, worktree.base_rev())
    }

    /// Diffs a worktree against the revision it branched from.
    ///
    /// Unlike [`Workspace::worktree_status`] this reads file contents, so it is
    /// computed only when someone asks to see it rather than on every refresh.
    /// The comparison runs base to *working tree*, so an agent's work counts
    /// whether or not it committed — see [`crate::diff`].
    pub fn worktree_diff(&self, id: &WorktreeId) -> Result<WorktreeDiff> {
        let state = self.store.load()?;
        let worktree = state
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .ok_or_else(|| KetError::UnknownWorktree(id.to_string()))?;

        if !worktree.exists() {
            return Err(KetError::Conflict(format!(
                "{} is registered but its directory is gone; run `ket worktree prune`",
                worktree.id
            )));
        }

        let base = worktree.base_rev().ok_or_else(|| {
            KetError::Conflict(format!("{} records no base to diff against", worktree.id))
        })?;

        diff::of_worktree(&worktree.path, base)
    }

    // ---- agents ------------------------------------------------------------

    /// Runs an agent against a worktree, returning when the turn ends.
    ///
    /// Refuses an unprovisioned worktree. That refusal is the whole point of
    /// Epic 2b: an agent dropped into a bare checkout fails on its first command
    /// and then spends real money diagnosing an environment problem that does
    /// not exist.
    pub async fn run_agent(
        &self,
        worktree_id: &WorktreeId,
        agent: Option<&str>,
        prompt: &str,
    ) -> Result<SessionOutcome> {
        if prompt.trim().is_empty() {
            return Err(KetError::Conflict("prompt is empty".to_owned()));
        }

        let state = self.store.load()?;
        let worktree = state
            .worktrees
            .iter()
            .find(|w| &w.id == worktree_id)
            .cloned()
            .or_else(|| {
                state.projects.iter().find_map(|project| {
                    let primary = WorktreeId::new(format!("primary-{}", project.id));
                    (&primary == worktree_id).then(|| Worktree {
                        id: primary,
                        project_id: project.id.clone(),
                        branch: project.default_base.clone(),
                        name: None,
                        agent_title: None,
                        path: project.root.clone(),
                        base: project.default_base.clone(),
                        base_commit: None,
                        agent: project.preferred_agent.clone(),
                        created_at_ms: project.last_opened_ms,
                        provisioned_at_ms: Some(crate::now_ms()),
                        provisioned_paths: Vec::new(),
                        token_reduction: worktree::default_token_reduction(),
                        token_reduction_id: None,
                        token_reduction_pack_id: None,
                        pinned: false,
                    })
                })
            })
            .ok_or_else(|| KetError::UnknownWorktree(worktree_id.to_string()))?;

        let project = state
            .projects
            .iter()
            .find(|p| p.id == worktree.project_id)
            .ok_or_else(|| KetError::UnknownProject(worktree.project_id.to_string()))?;

        if !worktree.exists() {
            return Err(KetError::Conflict(format!(
                "{} is registered but its directory is gone; run `ket worktree prune`",
                worktree.id
            )));
        }

        if !worktree.is_provisioned() {
            return Err(KetError::Conflict(format!(
                "{} is not provisioned; run `ket worktree provision {}` first",
                worktree.id, worktree.id
            )));
        }

        // Explicit name, then the project's preference, then the first
        // configured agent. No agent is privileged, so "the first one" is a
        // property of the user's config rather than of ket.
        let wanted = agent
            .map(str::to_owned)
            .or_else(|| project.preferred_agent.clone())
            .or_else(|| self.config.agents.first().map(|a| a.name.clone()))
            .ok_or_else(|| KetError::Config("no agents configured".to_owned()))?;

        let mut spec = self
            .config
            .agent(&wanted)
            .ok_or_else(|| KetError::Agent {
                agent: wanted.clone(),
                why: "not configured; see `ket config show`".to_owned(),
            })?
            .clone();
        worktree::apply_economy(&mut spec, &worktree);

        let id = SessionId::new(crate::slug::slug_keyed(
            &spec.name,
            format!("{}{}", worktree.id, crate::now_ms()).as_bytes(),
        ));

        // Cloned rather than held as borrows of `state`: `run_agent`'s future
        // is spawned by its callers (see `ket-cli`'s `run_agent`), so it must
        // stay `Send` across the transport's `.await` below, and these are
        // needed again afterward for the `agent.finished` hook.
        let project_root = project.root.clone();
        let hook_project_id = project.id.clone();

        // Fired before anything is registered, so a block never leaves a
        // session record, a live entry, or a spawned process behind — the
        // agent never actually starts.
        self.run_hook(
            HookPoint::AgentStarted,
            state.trusted_automation.contains(&project.id),
            &project_root,
            serde_json::json!({
                "sessionId": id,
                "worktreeId": worktree_id,
                "projectId": hook_project_id,
                "agent": spec.name,
            }),
        )?;

        let (cancel, signal) = agent::cancellation();
        let ctx = Arc::new(SessionContext::new(
            id.clone(),
            worktree.path.clone(),
            Arc::clone(&self.bus),
            self.config.agent.timeouts.clone(),
            self.config.agent.permissions.clone(),
            signal,
        ));

        self.register_session(&Session {
            id: id.clone(),
            project_id: project.id.clone(),
            worktree_id: worktree.id.clone(),
            agent: spec.name.clone(),
            status: SessionStatus::Live(ctx.state()),
            driver_pid: std::process::id(),
            started_at_ms: crate::now_ms(),
            heartbeat_ms: crate::now_ms(),
        })?;

        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(
            id.clone(),
            LiveSession {
                ctx: Arc::clone(&ctx),
                cancel: cancel.clone(),
            },
        );

        self.bus.publish(Event::AgentSessionStarted {
            session_id: id.clone(),
            worktree_id: worktree.id.clone(),
            agent: spec.name.clone(),
        });

        // The watchdog is what makes the per-state timeouts real. Without it a
        // bound is a number in a config file that nothing ever consults.
        let watchdog = tokio::spawn(watch(Arc::clone(&ctx), cancel.clone()));

        let first_prompt = agent::first_prompt(prompt);
        let outcome = match spec.transport {
            Transport::Acp => agent::acp::run(&spec, &ctx, &first_prompt).await,
            Transport::Pty => agent::pty::run(&spec, &ctx, &first_prompt).await,
        };

        watchdog.abort();
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);

        let outcome = match outcome {
            Ok(outcome) => outcome,
            // A transport failing is a failed session, not a failed command: the
            // caller wants the outcome recorded and the events flushed, not an
            // error that loses both.
            Err(e) => SessionOutcome::Failed { why: e.to_string() },
        };

        // The session is recorded finished — and the event published — before
        // the hook runs, unconditionally. Unlike `worktree.created`, nothing
        // here is undoable: the agent already did whatever it did to the
        // worktree. A blocking `agent.finished` hook cannot erase that, only
        // report a problem, so it must not be able to leave the session stuck
        // `Live` forever (invariant 4) while it does.
        self.finish_session(&id, &outcome)?;
        self.bus.publish(Event::AgentSessionEnded {
            session_id: id.clone(),
            outcome: outcome.clone(),
        });

        self.run_hook(
            HookPoint::AgentFinished,
            state.trusted_automation.contains(&project.id),
            &project_root,
            serde_json::json!({
                "sessionId": id,
                "worktreeId": worktree_id,
                "projectId": hook_project_id,
                "agent": spec.name,
                "outcome": &outcome,
            }),
        )?;

        Ok(outcome)
    }

    /// Answers a permission request from a session this process is driving.
    ///
    /// Returns whether there was a live request to answer, so a client can tell
    /// a stale answer from one that landed.
    pub fn answer_permission(&self, session: &SessionId, request_id: &str, allowed: bool) -> bool {
        let live = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session)
            .cloned();

        live.is_some_and(|live| live.ctx.answer_permission(request_id, allowed))
    }

    /// Asks a session to stop.
    ///
    /// This is invariant 4's escape hatch for every agent state, including the
    /// two that have no timeout because a person is expected to be deciding.
    pub fn cancel_session(&self, session: &SessionId) -> bool {
        let live = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session)
            .cloned();

        match live {
            Some(live) => {
                live.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Asks every session this process is driving to stop.
    ///
    /// What Ctrl-C is wired to: killing ket must not leave agents running.
    pub fn cancel_all(&self) -> usize {
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        for session in live.values() {
            session.cancel.cancel();
        }
        live.len()
    }

    /// Sessions, optionally filtered to one project.
    ///
    /// Records whose driving process is gone are reported as failed rather than
    /// left claiming to be running — see [`Session::is_abandoned`].
    pub fn sessions(&self, project_id: Option<&ProjectId>) -> Result<Vec<Session>> {
        let now = crate::now_ms();
        let mut sessions = self.store.load()?.sessions;

        for session in &mut sessions {
            if session.is_abandoned(now) {
                session.status = SessionStatus::Ended(SessionOutcome::Failed {
                    why: "the ket process driving this session is gone".to_owned(),
                });
            }
        }

        if let Some(id) = project_id {
            sessions.retain(|s| &s.project_id == id);
        }

        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at_ms));
        Ok(sessions)
    }

    /// Drops finished and abandoned session records.
    ///
    /// Returns how many went. A force-quit leaves records behind, and without
    /// this they accumulate forever as sessions nothing will ever finish.
    pub fn reap_sessions(&self) -> Result<usize> {
        let now = crate::now_ms();

        self.store.update(move |state| {
            let before = state.sessions.len();
            state
                .sessions
                .retain(|s| !s.status.is_ended() && !s.is_abandoned(now));
            Ok(before - state.sessions.len())
        })
    }

    /// Records a new session.
    fn register_session(&self, session: &Session) -> Result<()> {
        let session = session.clone();
        self.store.update(move |state| {
            state.sessions.push(session);
            Ok(())
        })
    }

    /// Marks a session finished.
    fn finish_session(&self, id: &SessionId, outcome: &SessionOutcome) -> Result<()> {
        let id = id.clone();
        let outcome = outcome.clone();

        self.store.update(move |state| {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == id) {
                session.status = SessionStatus::Ended(outcome);
                session.heartbeat_ms = crate::now_ms();
            }
            Ok(())
        })
    }
}

/// Cancels a session that has outstayed its current state's bound.
///
/// Polling rather than a timer armed on each transition: states change often,
/// and a watchdog that has to be rearmed on every change is a watchdog that
/// eventually is not.
async fn watch(ctx: Arc<SessionContext>, cancel: CancelHandle) {
    loop {
        tokio::time::sleep(WATCHDOG_TICK).await;

        if ctx.is_stuck() {
            tracing::warn!(
                session = %ctx.id,
                state = ?ctx.state(),
                elapsed = ?ctx.time_in_state(),
                "session exceeded its timeout; cancelling"
            );
            cancel.cancel();
            return;
        }
    }
}

/// Puts each project's worktrees in the order its rows were dragged into.
///
/// Applied over the slots a project already occupies rather than by sorting
/// the whole list: an unfiltered list holds several projects interleaved by
/// creation time, and one project's arrangement is not a claim about where
/// its worktrees belong among another's.
///
/// Worktrees the stored order does not name — every worktree created since
/// the drag — keep the newest-first position the sort above gave them, at the
/// top where a new worktree is worth seeing.
fn apply_worktree_order(
    worktrees: &mut [Worktree],
    order: &std::collections::BTreeMap<ProjectId, Vec<WorktreeId>>,
) {
    for (project_id, wanted) in order {
        let slots: Vec<usize> = worktrees
            .iter()
            .enumerate()
            .filter(|(_, w)| &w.project_id == project_id)
            .map(|(slot, _)| slot)
            .collect();

        let mut arranged: Vec<Worktree> =
            slots.iter().map(|&slot| worktrees[slot].clone()).collect();
        arranged.sort_by_key(|w| {
            wanted
                .iter()
                .position(|id| id == &w.id)
                .map_or(0, |rank| rank + 1)
        });

        for (slot, worktree) in slots.into_iter().zip(arranged) {
            worktrees[slot] = worktree;
        }
    }
}

/// Puts the projects in the order their headings were dragged into.
///
/// Projects the stored order does not name — every project added since the
/// drag — keep the most-recent-first place the sort gave them, which is the
/// top: a project you just added is the one worth seeing. That is the rule
/// [`apply_worktree_order`] follows for a project's own rows, and it works
/// here because `sort_by_key` is stable.
/// The projects in the order the sidebar lists them: most recently opened
/// first, then by name, then as dragged — see [`apply_project_order`].
///
/// Public so the host's phone snapshot lists them in the same order as the
/// desktop does: two orders for one list is two places for it to drift.
pub fn sidebar_projects(state: &crate::store::State) -> Vec<Project> {
    let mut projects = state.projects.clone();
    projects.sort_by(|a, b| {
        b.last_opened_ms
            .cmp(&a.last_opened_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    apply_project_order(&mut projects, &state.project_order);
    projects
}

/// Every managed worktree in the order the sidebar lists a project's rows:
/// newest first, then as dragged, then pinned ones leading — the sidebar also
/// puts the repository's own checkout above them all. For the same reason as
/// [`sidebar_projects`].
pub fn sidebar_worktrees(state: &crate::store::State) -> Vec<Worktree> {
    let mut worktrees = state.worktrees.clone();
    sort_worktrees(&mut worktrees, &state.worktree_order);
    // Stable, so pinning only lifts the pinned rows.
    worktrees.sort_by_key(|w| !w.pinned);
    worktrees
}

/// Newest first, then as dragged.
fn sort_worktrees(
    worktrees: &mut [Worktree],
    order: &std::collections::BTreeMap<ProjectId, Vec<WorktreeId>>,
) {
    worktrees.sort_by_key(|w| std::cmp::Reverse(w.created_at_ms));
    apply_worktree_order(worktrees, order);
}

fn apply_project_order(projects: &mut [Project], order: &[ProjectId]) {
    if order.is_empty() {
        return;
    }
    projects.sort_by_key(|project| {
        order
            .iter()
            .position(|id| id == &project.id)
            .map_or(0, |rank| rank + 1)
    });
}

/// Resolves symlinks so a path from git and one from the registry, naming the
/// same directory, compare equal.
///
/// Falls back to the path as given when it does not exist — a discovered
/// worktree's directory can vanish between git reporting it and this call
/// reading it, and that race is not this function's to fail on.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// [`canonical`], for a path whose directory has just been renamed away.
///
/// `canonicalize` needs the path to exist and this one deliberately no longer
/// does. Its parent still does, and resolving that is what matters: on macOS a
/// worktree under `$TMPDIR` is `/var/...` to ket and `/private/var/...` to git,
/// and comparing those two raw would report a registration git still holds as
/// already gone.
fn canonical_missing(path: &Path) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path.to_path_buf();
    };
    canonical(parent).join(name)
}

/// Clears git's registration for a checkout that has been renamed away.
///
/// `git worktree remove --force` on an already-missing directory is accepted
/// and touches only this entry, so it is tried first — `prune` would clear
/// every other stale registration in the repository at the same time, and
/// those belong to other worktrees whose owners did not ask for this.
///
/// Both failing is the case worth reporting: a worktree git has *locked* is
/// skipped by `prune` and refused by `remove`, and the caller answers that by
/// putting the checkout back and letting git state its own objection.
fn clear_registration(git: &Git, path: &Path) -> Result<()> {
    if git.worktree_remove(path, true).is_ok() {
        return Ok(());
    }

    git.worktree_prune()?;

    let wanted = canonical_missing(path);
    let still_registered = git
        .worktree_list()?
        .iter()
        .any(|w| canonical_missing(&w.path) == wanted);

    if still_registered {
        return Err(KetError::Git {
            command: "worktree prune".to_owned(),
            status: "0".to_owned(),
            stderr: format!("{} is still registered after pruning", path.display()),
        });
    }

    Ok(())
}

/// Deletes the branch of a worktree that has just gone, keeping unmerged work.
///
/// Returns the branch name when it was kept, so the caller can say so.
///
/// `-d` is always the first thing tried, because a branch it accepts needs no
/// further argument. What it refuses is the interesting case: it decides by
/// ancestry, so a branch a forge squashed or rebased into the base reads as
/// unmerged even though every line of it has landed. Leaving those behind is
/// how a repository ends up with dozens of merged branches and the worktrees
/// to match, so the refusal is checked rather than believed — see
/// [`crate::branch_cleanup`].
fn delete_branch_after_removal(git: &Git, branch: &str, force: bool) -> Option<String> {
    if force {
        if let Err(e) = git.branch_delete(branch, true) {
            tracing::warn!(%e, branch = %branch, "worktree removed but branch delete failed");
        }
        return None;
    }

    if git.branch_delete(branch, false).is_ok() {
        return None;
    }

    if branch_cleanup::branch_is_fully_merged(git, branch) {
        match git.branch_delete(branch, true) {
            Ok(()) => return None,
            Err(e) => {
                tracing::warn!(%e, branch = %branch, "branch is merged but would not delete");
            }
        }
    }

    Some(branch.to_owned())
}

/// Whether a changed path is something provisioning put there.
///
/// Matches the path itself or anything beneath it, and tolerates the trailing
/// slash a collapsed untracked directory carries.
fn was_provisioned(path: &str, provisioned: &[String]) -> bool {
    let path = path.trim_end_matches('/');

    provisioned.iter().any(|created| {
        let created = created.trim_end_matches('/');
        path == created
            || path
                .strip_prefix(created)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
