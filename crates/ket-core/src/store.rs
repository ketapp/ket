//! Durable state, persisted as JSON.
//!
//! JSON rather than SQLite for now: the data is small, human-inspectable state
//! is valuable while the shape is still moving, and it costs no dependency.
//! SQLite is the migration path once concurrent access between the CLI and
//! the server becomes routine.
//!
//! Two properties matter more than the format:
//!
//! - **Writes are atomic.** State is written to a temporary file and renamed
//!   over the target, so an interrupted write cannot truncate the registry and
//!   lose every project.
//! - **Mutations are serialised.** [`Store::update`] takes an advisory lock, so
//!   two `ket` processes cannot read-modify-write over each other. The lock goes
//!   stale rather than wedging: a crashed process must not require manual
//!   cleanup before the tool works again (invariant 4).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::{ProjectId, WorktreeId};
use crate::project::{Project, ProjectSettings};
use crate::worktree::Worktree;
use crate::{KetError, Result, paths};

/// How long to wait for a lock held by another process.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// After this, a lock is assumed to belong to a process that died.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(60);

/// Largest layout blob accepted for one project, serialised.
///
/// ket never reads inside a layout, so nothing here would notice a client that
/// grew one without bound — and this file is loaded in full on every command.
/// 64 KiB is far more than a pane arrangement needs and far less than enough to
/// matter.
pub const MAX_LAYOUT_BYTES: usize = 64 * 1024;

/// Explicit repository-automation grants, plus whether their field existed on
/// disk. The presence bit drives the one-time migration but is not
/// serialized, and not part of equality either: it is load provenance, not
/// content, and two `State`s that trust the same projects must compare equal
/// whether one of them was just loaded and the other just constructed.
#[derive(Debug, Clone, Default)]
pub struct AutomationTrust {
    projects: std::collections::BTreeSet<ProjectId>,
    initialized: bool,
}

impl PartialEq for AutomationTrust {
    fn eq(&self, other: &Self) -> bool {
        self.projects == other.projects
    }
}

impl Eq for AutomationTrust {}

impl std::ops::Deref for AutomationTrust {
    type Target = std::collections::BTreeSet<ProjectId>;

    fn deref(&self) -> &Self::Target {
        &self.projects
    }
}

impl std::ops::DerefMut for AutomationTrust {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.projects
    }
}

impl Serialize for AutomationTrust {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.projects.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AutomationTrust {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Ok(Self {
            projects: std::collections::BTreeSet::deserialize(deserializer)?,
            initialized: true,
        })
    }
}

/// Where you were in a project when you last left it.
///
/// This is what makes switching back to a project you left last week restore
/// something rather than starting over — the point of projects being the top of
/// the data model at all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectState {
    /// Worktrees open in this project, in the order they were opened.
    pub open: Vec<WorktreeId>,
    /// The worktree that had focus.
    pub active: Option<WorktreeId>,
    /// Client-owned view state — pane sizes, tab order, whatever the shell wants.
    ///
    /// Deliberately opaque. Core stores and returns it verbatim and never looks
    /// inside, because a layout schema in `ket-core` would be exactly the
    /// UI-specific knowledge invariant 3 exists to keep out. Bounded by
    /// [`MAX_LAYOUT_BYTES`].
    pub layout: Option<serde_json::Value>,
}

impl ProjectState {
    /// Drops references to worktrees that no longer exist, and re-picks focus.
    ///
    /// Worktrees are removed by ket, by hand, and by git itself, so this state
    /// is always potentially one step behind reality. Reconciling on read means
    /// a stale entry can never resurrect a worktree that is gone.
    ///
    /// One rule governs focus: the active worktree is always one of the open
    /// ones, and when it stops being so, focus moves to the most recently opened
    /// rather than to nothing. Removing the worktree you were looking at should
    /// leave you looking at the next one, not at an empty project with two
    /// worktrees still open in it.
    pub fn reconcile(&mut self, live: &[Worktree]) {
        self.open.retain(|id| live.iter().any(|w| &w.id == id));

        if !self
            .active
            .as_ref()
            .is_some_and(|id| self.open.contains(id))
        {
            self.active = self.open.last().cloned();
        }
    }
}

/// Everything ket persists between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Registered projects.
    pub projects: Vec<Project>,
    /// Worktrees ket created, across every project.
    pub worktrees: Vec<Worktree>,
    /// Agent sessions, live and recently finished.
    ///
    /// Persisted so `ket ps` can show what is running across every project even
    /// though each session is driven by whichever process started it. Reaped
    /// rather than kept — see [`crate::workspace::Workspace::reap_sessions`].
    pub sessions: Vec<crate::agent::Session>,
    /// Per-project durable state, keyed by project id.
    ///
    /// Kept beside projects rather than inside them so that a `Project` stays a
    /// small identity record: listing twenty projects should not deserialise
    /// twenty layout blobs.
    pub project_state: BTreeMap<ProjectId, ProjectState>,
    /// Per-project preferences, keyed by project id.
    ///
    /// A separate map from `project_state` because the two change on different
    /// clocks: state is rewritten on every worktree switch, settings when a
    /// person opens a dialog. A project absent here has defaults. Unknown
    /// fields inside an entry are ignored on load, the same policy as
    /// [`ProjectState`]: a newer ket writing a field an older one does not know
    /// must not make the older one refuse to start.
    pub project_settings: BTreeMap<ProjectId, ProjectSettings>,
    /// Projects whose committed `.ket.toml` is allowed to execute commands.
    /// Absent by default: registering a repository is not consent to run it.
    pub trusted_automation: AutomationTrust,
    /// The order a project's worktrees are shown in, once its rows have been
    /// dragged into an arrangement. A project absent here is shown newest
    /// first, which is what every project does until someone says otherwise.
    ///
    /// Kept out of `project_settings` because the dialogs that write those
    /// rebuild the whole struct from their own fields: an order stored there
    /// would be thrown away by the next person to press Save on a project's
    /// settings, from a dialog that never mentions the sidebar's order.
    ///
    /// Ids that no longer name a worktree are ignored on read rather than
    /// migrated away, so removing a worktree needs no bookkeeping here.
    pub worktree_order: BTreeMap<ProjectId, Vec<WorktreeId>>,
    /// The order the projects themselves are shown in, once their headings
    /// have been dragged into an arrangement. Empty until someone rearranges
    /// them, which is when projects are shown most recently opened first.
    ///
    /// A list rather than a map, unlike [`Self::worktree_order`]: there is one
    /// sidebar, and the projects in it are one sequence.
    ///
    /// Ids that no longer name a project are ignored on read, so this needs no
    /// migrating — though [`crate::workspace::Workspace::remove_project`]
    /// clears the entry anyway, alongside everything else it forgets.
    pub project_order: Vec<ProjectId>,
    /// Client-owned view state for the window itself, rather than for one
    /// project: which panels are open and how wide, what is expanded, where
    /// the window was, where you were.
    ///
    /// Deliberately opaque, for the reason [`ProjectState::layout`] is — this
    /// is the shell's vocabulary, and core storing it is not core knowing it.
    /// Kept apart from that blob because it is not a project's: it is written
    /// whichever project is selected, and a window that has never opened one
    /// still has a shape worth coming back to. Bounded by
    /// [`MAX_LAYOUT_BYTES`], the same as a layout.
    pub view: Option<serde_json::Value>,
}

/// Reads and writes [`State`].
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// Opens the store at its default location, creating the directory if needed.
    pub fn open_default() -> Result<Self> {
        let dir = paths::data_dir()?;
        fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
        Ok(Self::at(dir.join("state.json")))
    }

    /// Opens a store at a specific path. Used by tests.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads state, treating a missing file as empty.
    ///
    /// A *corrupt* file is an error rather than a silent reset: quietly
    /// forgetting every registered project would be far worse than refusing to
    /// start and saying why.
    pub fn load(&self) -> Result<State> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                let mut state: State = serde_json::from_str(&text).map_err(|e| {
                    KetError::Config(format!("{}: corrupt state file: {e}", self.path.display()))
                })?;
                // Before repository automation required an explicit setting,
                // every registered project ran it. Preserve that behaviour on
                // the first upgrade only. A sidecar marker survives an older
                // binary rewriting state.json without the new field.
                if !state.trusted_automation.initialized {
                    let marker = self.automation_marker();
                    match fs::read_to_string(&marker) {
                        Ok(saved) => {
                            state.trusted_automation.projects = serde_json::from_str(&saved)
                                .map_err(|error| {
                                    KetError::Config(format!(
                                        "{}: corrupt automation migration marker: {error}",
                                        marker.display()
                                    ))
                                })?;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            state.trusted_automation.projects = state
                                .projects
                                .iter()
                                .map(|project| project.id.clone())
                                .collect();
                            self.save_automation_marker(&state.trusted_automation.projects)?;
                        }
                        Err(error) => return Err(KetError::io(&marker, error)),
                    }
                    state.trusted_automation.initialized = true;
                }
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(KetError::io(&self.path, e)),
        }
    }

    /// Writes state atomically.
    pub fn save(&self, state: &State) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
        }

        let text = serde_json::to_string_pretty(state)
            .map_err(|e| KetError::Config(format!("serialising state: {e}")))?;

        // Same directory, so the rename stays on one filesystem and is atomic.
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        fs::write(&tmp, text.as_bytes()).map_err(|e| KetError::io(&tmp, e))?;
        fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            KetError::io(&self.path, e)
        })?;
        // Kept alongside state.json itself, not only inside `update`: `save`
        // is a public write path of its own, and a marker that only tracks
        // `update`'s callers would go stale the moment anything else wrote
        // state directly, then hand a stale trust list back to the very
        // migration it exists for.
        self.save_automation_marker(&state.trusted_automation.projects)?;

        Ok(())
    }

    /// Runs `f` against the current state under a lock, saving if it succeeds.
    ///
    /// The state is not written when `f` returns an error, so a failed operation
    /// cannot leave a half-applied registry behind.
    pub fn update<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let _guard = LockGuard::acquire(self.lock_path())?;

        let mut state = self.load()?;
        let value = f(&mut state)?;
        self.save(&state)?;

        Ok(value)
    }

    fn lock_path(&self) -> PathBuf {
        self.path.with_extension("lock")
    }

    fn automation_marker(&self) -> PathBuf {
        self.path.with_extension("automation-trust-v1")
    }

    fn save_automation_marker(
        &self,
        trusted: &std::collections::BTreeSet<ProjectId>,
    ) -> Result<()> {
        // Unique per write, and not `with_extension`: that would make it
        // `state.tmp.<pid>`, the temporary file `save` renames over state.json.
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let marker = self.automation_marker();
        let temporary = marker.with_file_name(format!(
            ".{}.{}.{}.tmp",
            marker
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id(),
            WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let text = serde_json::to_vec(trusted)
            .map_err(|error| KetError::Config(format!("serialising automation trust: {error}")))?;
        fs::write(&temporary, text).map_err(|error| KetError::io(&temporary, error))?;
        fs::rename(&temporary, &marker).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            KetError::io(&marker, error)
        })
    }
}

/// An advisory lock held for the lifetime of the guard.
#[derive(Debug)]
struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    /// Acquires the lock, waiting for a live holder and stealing a stale one.
    fn acquire(path: PathBuf) -> Result<Self> {
        let started = Instant::now();

        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(&path) {
                        // The holder died. Reclaim rather than making the user
                        // delete a lock file by hand.
                        tracing::warn!(path = %path.display(), "removing stale lock");
                        let _ = fs::remove_file(&path);
                        continue;
                    }

                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(KetError::Config(format!(
                            "timed out waiting for {} — another ket process is busy",
                            path.display()
                        )));
                    }

                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(KetError::io(&path, e)),
            }
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a lock file is old enough to assume its holder is gone.
fn is_stale(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        // It vanished between our failed create and this check; treat as free.
        return true;
    };

    let Ok(modified) = metadata.modified() else {
        return false;
    };

    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > LOCK_STALE_AFTER)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ProjectId, WorktreeId};

    /// A store in a unique temporary directory, cleaned up by the caller.
    fn temp_store(tag: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("ket-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (Store::at(dir.join("state.json")), dir)
    }

    fn project(name: &str) -> Project {
        Project {
            id: ProjectId::new(name),
            name: name.to_owned(),
            root: PathBuf::from(format!("/tmp/{name}")),
            default_base: "main".to_owned(),
            preferred_agent: None,
            last_opened_ms: 0,
        }
    }

    fn worktree(id: &str) -> Worktree {
        Worktree {
            agent: None,
            id: WorktreeId::new(id),
            project_id: ProjectId::new("proj"),
            branch: id.to_owned(),
            name: None,
            agent_title: None,
            path: PathBuf::from(format!("/tmp/{id}")),
            base: "main".to_owned(),
            base_commit: None,
            created_at_ms: 0,
            provisioned_at_ms: None,
            provisioned_paths: Vec::new(),
            token_reduction: crate::worktree::default_token_reduction(),
            token_reduction_id: None,
            token_reduction_pack_id: None,
            pinned: false,
        }
    }

    #[test]
    fn reconcile_drops_worktrees_that_are_gone() {
        let mut state = ProjectState {
            open: vec![WorktreeId::new("a"), WorktreeId::new("b")],
            active: Some(WorktreeId::new("a")),
            layout: None,
        };

        state.reconcile(&[worktree("a")]);

        assert_eq!(state.open, vec![WorktreeId::new("a")]);
        assert_eq!(state.active, Some(WorktreeId::new("a")));
    }

    #[test]
    fn reconcile_moves_focus_rather_than_clearing_it() {
        // Removing the worktree you were looking at should leave you looking at
        // the next one, not at an empty project with one still open in it.
        let mut state = ProjectState {
            open: vec![WorktreeId::new("a"), WorktreeId::new("b")],
            active: Some(WorktreeId::new("b")),
            layout: None,
        };

        state.reconcile(&[worktree("a")]);

        assert_eq!(state.active, Some(WorktreeId::new("a")));
    }

    #[test]
    fn reconcile_clears_focus_only_when_nothing_is_open() {
        let mut state = ProjectState {
            open: vec![WorktreeId::new("a")],
            active: Some(WorktreeId::new("a")),
            layout: None,
        };

        state.reconcile(&[]);

        assert!(state.open.is_empty());
        assert_eq!(state.active, None);
    }

    #[test]
    fn per_project_state_round_trips_through_the_file() {
        let (store, dir) = temp_store("project-state");

        let mut state = State::default();
        state.project_state.insert(
            ProjectId::new("proj"),
            ProjectState {
                open: vec![WorktreeId::new("a")],
                active: Some(WorktreeId::new("a")),
                layout: Some(serde_json::json!({ "split": [0.3, 0.7] })),
            },
        );
        store.save(&state).unwrap();

        assert_eq!(store.load().unwrap(), state);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_state_file_written_before_project_state_existed_still_loads() {
        // These files are on real disks already. A schema addition that makes
        // ket refuse to start would lose every registered project.
        let (store, dir) = temp_store("older-schema");
        fs::write(store.path(), br#"{"projects":[],"worktrees":[]}"#).unwrap();

        assert_eq!(store.load().unwrap(), State::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let (store, dir) = temp_store("missing");
        assert_eq!(store.load().unwrap(), State::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_round_trips() {
        let (store, dir) = temp_store("roundtrip");

        let mut state = State::default();
        state.projects.push(project("alpha"));
        store.save(&state).unwrap();

        assert_eq!(store.load().unwrap(), state);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_state_is_an_error_not_a_silent_reset() {
        // Losing the registry silently would be much worse than refusing to run.
        let (store, dir) = temp_store("corrupt");
        fs::write(store.path(), b"{ not json").unwrap();

        assert!(matches!(store.load(), Err(KetError::Config(_))));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_persists_on_success() {
        let (store, dir) = temp_store("update-ok");

        store
            .update(|state| {
                state.projects.push(project("beta"));
                Ok(())
            })
            .unwrap();

        assert_eq!(store.load().unwrap().projects.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_writes_nothing_when_the_closure_fails() {
        // A failed operation must not leave a partially applied registry.
        let (store, dir) = temp_store("update-fail");
        store.save(&State::default()).unwrap();

        let result: Result<()> = store.update(|state| {
            state.projects.push(project("gamma"));
            Err(KetError::Config("deliberate".to_owned()))
        });

        assert!(result.is_err());
        assert!(store.load().unwrap().projects.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_releases_its_lock_even_when_the_closure_fails() {
        // Otherwise one failed command wedges every later one.
        let (store, dir) = temp_store("lock-release");

        let _: Result<()> = store.update(|_| Err(KetError::Config("boom".to_owned())));
        assert!(store.update(|_| Ok(())).is_ok());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_lock_is_reclaimed_rather_than_wedging_the_tool() {
        let (store, dir) = temp_store("stale-lock");
        let lock = store.lock_path();

        fs::write(&lock, b"").unwrap();
        // Backdate past the staleness threshold.
        let old = SystemTime::now() - (LOCK_STALE_AFTER + Duration::from_secs(30));
        fs::File::open(&lock)
            .unwrap()
            .set_modified(old)
            .expect("set_modified");

        assert!(store.update(|_| Ok(())).is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_leaves_no_temporary_files_behind() {
        let (store, dir) = temp_store("no-temps");
        store.save(&State::default()).unwrap();

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();

        assert!(leftovers.is_empty(), "left {leftovers:?}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_state_file_from_before_automation_trust_trusts_every_registered_project() {
        // Before the field existed, a repo's `.ket.toml` always ran. The first
        // load after upgrading must not change that for projects already there.
        // `trusted_automation` has to be genuinely absent from the JSON, the
        // way a file written before this field existed actually looks — not
        // merely empty, which `State::default()` would serialize as `[]` and
        // not trigger the migration at all.
        let (store, dir) = temp_store("automation-migrate");
        let mut pre = State::default();
        pre.projects.push(project("alpha"));
        pre.projects.push(project("beta"));
        let mut value = serde_json::to_value(&pre).unwrap();
        value.as_object_mut().unwrap().remove("trusted_automation");
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = store.load().unwrap();

        assert!(loaded.trusted_automation.contains(&ProjectId::new("alpha")));
        assert!(loaded.trusted_automation.contains(&ProjectId::new("beta")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn automation_migration_survives_an_older_binary_rewriting_the_file() {
        // An older ket does not know the field and drops it when it rewrites
        // state.json. The marker left by the first load must carry the grant
        // through a second migration rather than re-trusting everything.
        let (store, dir) = temp_store("automation-migrate-twice");
        let mut pre = State::default();
        pre.projects.push(project("alpha"));
        pre.projects.push(project("beta"));
        let without_field = |state: &State| {
            let mut value = serde_json::to_value(state).unwrap();
            value.as_object_mut().unwrap().remove("trusted_automation");
            serde_json::to_vec(&value).unwrap()
        };
        fs::write(store.path(), without_field(&pre)).unwrap();

        let mut first = store.load().unwrap();
        // The person explicitly withdraws trust from "beta" after upgrading.
        first.trusted_automation.remove(&ProjectId::new("beta"));
        store.save(&first).unwrap();

        // An older binary resaves the file, dropping the field it does not
        // know about entirely — not writing it back as an empty list.
        fs::write(store.path(), without_field(&pre)).unwrap();

        let second = store.load().unwrap();
        assert!(second.trusted_automation.contains(&ProjectId::new("alpha")));
        assert!(!second.trusted_automation.contains(&ProjectId::new("beta")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_freshly_registered_project_is_not_trusted_by_default() {
        // Registering a repository is not consent to run its committed hooks.
        let (store, dir) = temp_store("automation-fresh");

        store
            .update(|state| {
                state.projects.push(project("fresh"));
                Ok(())
            })
            .unwrap();

        assert!(
            !store
                .load()
                .unwrap()
                .trusted_automation
                .contains(&ProjectId::new("fresh"))
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn trusted_automation_round_trips_through_update() {
        let (store, dir) = temp_store("automation-roundtrip");

        store
            .update(|state| {
                state.projects.push(project("alpha"));
                state.trusted_automation.insert(ProjectId::new("alpha"));
                Ok(())
            })
            .unwrap();

        assert!(
            store
                .load()
                .unwrap()
                .trusted_automation
                .contains(&ProjectId::new("alpha"))
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_automation_marker_write_never_collides_with_states_own_temp_file() {
        // Both `save` and `save_automation_marker` once built their temporary
        // file as `<name>.tmp.<pid>` — identical for state.json and its
        // marker. Changing trust alongside an ordinary state write exercises
        // both temp files in the same process without one clobbering the
        // other's rename.
        let (store, dir) = temp_store("automation-no-collision");

        for i in 0..5 {
            store
                .update(|state| {
                    let id = ProjectId::new(format!("proj-{i}"));
                    state.projects.push(project(&format!("proj-{i}")));
                    state.trusted_automation.insert(id);
                    Ok(())
                })
                .unwrap();
        }

        let state = store.load().unwrap();
        assert_eq!(state.projects.len(), 5);
        for i in 0..5 {
            assert!(
                state
                    .trusted_automation
                    .contains(&ProjectId::new(format!("proj-{i}")))
            );
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn untrusting_a_project_does_not_revert_on_the_next_load() {
        let (store, dir) = temp_store("automation-untrust");
        store
            .update(|state| {
                state.projects.push(project("alpha"));
                state.trusted_automation.insert(ProjectId::new("alpha"));
                Ok(())
            })
            .unwrap();

        store
            .update(|state| {
                state.trusted_automation.remove(&ProjectId::new("alpha"));
                Ok(())
            })
            .unwrap();

        assert!(
            !store
                .load()
                .unwrap()
                .trusted_automation
                .contains(&ProjectId::new("alpha"))
        );
        fs::remove_dir_all(&dir).ok();
    }
}
