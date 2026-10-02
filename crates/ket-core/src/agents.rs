//! Which agents are installed, and which one to run.
//!
//! Configuration says how to *launch* an agent — see [`crate::config::AgentSpec`].
//! This module answers the questions that come before that: which of them exist on
//! this machine, which the user has switched off, and which one a bare `ket run`
//! should pick.
//!
//! **Why detection matters.** Before this, ket carried a fixed list of three agents
//! whether or not any was installed, so a missing one failed at spawn time with
//! whatever the OS said about a missing binary. Detecting up front means a settings
//! screen shows your machine rather than a hardcoded list, an agent you have not
//! installed is visibly absent rather than mysteriously broken, and a fourth agent
//! is a row in [`CANDIDATES`] rather than a code change.
//!
//! **No agent is privileged.** Per-agent differences live in the candidate table as
//! *data*, never as branching on a name — the same rule that put the marker
//! variables to strip into [`crate::config::AgentSpec::env_remove`] rather than into
//! an `if agent == "claude"`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AgentSpec, Config, default_agents};

/// How long a single probe may take before it is treated as absent.
///
/// Detection runs at startup and on demand. A probe that hangs — a stale network
/// mount on `PATH` is the usual cause — must not take the window with it, which is
/// invariant 4 applied to something that looks too small to need it.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// One agent ket knows how to look for.
///
/// A table rather than a function per agent: adding an agent should be a row.
struct Candidate {
    /// The name used to select it, e.g. `ket run claude`.
    name: &'static str,
    /// The binary whose presence on `PATH` means this agent is installed.
    ///
    /// Deliberately the *agent's own* binary rather than whatever ket spawns.
    /// Claude and Codex are driven through `npx` adapters, so probing for `npx`
    /// would report every agent installed on any machine with Node, which is
    /// useless. What a person means by "is Claude installed" is `claude`.
    probe: &'static str,
}

/// The name of every agent ket would recognise, without probing for any of
/// them.
///
/// [`Catalogue::detect`] answers this too, but it runs a shell probe per
/// candidate — a whole login-shell startup each where the command is an alias
/// (see [`crate::shell`]). A caller that only wants to know whether a word in
/// a terminal's foreground names an agent, such as
/// [`crate::activity::Tracker`], is asking a question no probe helps with, and
/// should not pay for one.
pub fn known_agent_names(config: &Config) -> Vec<String> {
    let mut names: Vec<String> = CANDIDATES
        .iter()
        .map(|candidate| candidate.name.to_owned())
        .collect();

    // An agent someone wrote their own spec for is one ket should recognise in
    // a terminal, the same way `detect` keeps it in the catalogue.
    for spec in &config.agents {
        if !names.contains(&spec.name) {
            names.push(spec.name.clone());
        }
    }

    names
}

/// Every agent ket can detect.
const CANDIDATES: &[Candidate] = &[
    Candidate {
        name: "claude",
        probe: "claude",
    },
    Candidate {
        name: "codex",
        probe: "codex",
    },
    Candidate {
        name: "grok",
        probe: "grok",
    },
    Candidate {
        name: "opencode",
        probe: "opencode",
    },
];

/// Finds executables, so detection can be tested without installing agents.
///
/// The same seam [`crate::surface`] uses for editors, and for the same reason: a
/// test that depends on which binaries happen to be on the developer's machine is a
/// test that fails for reasons unrelated to the change.
pub trait Prober: Send + Sync {
    /// Where `binary` lives, or `None` if it is not on `PATH`.
    fn find(&self, binary: &str) -> Option<PathBuf>;
}

/// Looks along the real `PATH`.
#[derive(Debug, Default, Clone, Copy)]
pub struct PathProber;

impl Prober for PathProber {
    fn find(&self, binary: &str) -> Option<PathBuf> {
        // The walk runs on a worker so a wedged `PATH` entry cannot take the
        // caller with it. `stat` on a stale network mount blocks in the kernel
        // and is not interruptible, so the thread may outlive this call — it is
        // detached deliberately, and it holds nothing but a `String`.
        let wanted = binary.to_owned();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let _ = tx.send(walk_path(&wanted));
        });

        match rx.recv_timeout(PROBE_TIMEOUT) {
            Ok(found) => found,
            // A probe that did not answer in time is treated as absent. Saying
            // "not installed" is wrong but recoverable; hanging the window on a
            // dead mount is neither.
            Err(_) => {
                tracing::warn!(%binary, "probe timed out; treating as not installed");
                None
            }
        }
    }
}

/// Asks the user's shell, falling back to a `PATH` walk.
///
/// The prober a settings screen wants. Agents are launched by typing at a real
/// prompt (see [`crate::shell`]), so an alias or a shell function is every bit
/// as runnable as a file on `PATH` — and a `PATH` walk reports it missing.
/// Answers are cached; [`crate::shell::clear_probe_cache`] is what Refresh
/// calls.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShellProber;

impl Prober for ShellProber {
    fn find(&self, binary: &str) -> Option<PathBuf> {
        crate::shell::probe(binary)
    }
}

/// Looks for `binary` in each `PATH` entry, in order.
fn walk_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;

    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

/// Whether `path` is a file this user could run.
///
/// Checked by mode rather than by attempting a spawn: probing must not have side
/// effects, and starting an agent to find out whether it exists would start an
/// agent.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// What a bare `ket run` should pick when nothing else says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "name")]
pub enum DefaultAgent {
    /// Decide at run time — see [`Catalogue::resolve`] for the exact rule.
    #[default]
    Auto,
    /// Open a plain terminal instead of starting an agent.
    None,
    /// Always this agent.
    Named(String),
}

/// One row of the agents list, as a settings screen needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    /// The name used to select it.
    pub name: String,
    /// Whether its binary was found on this machine.
    pub installed: bool,
    /// Where its binary was found, when it was.
    pub found_at: Option<PathBuf>,
    /// Whether the user has switched it off.
    ///
    /// Distinct from `!installed`: one is a choice and the other is a fact, and a
    /// settings screen shows them differently.
    pub enabled: bool,
    /// How ket would launch it, user overrides applied.
    pub spec: AgentSpec,
    /// How ket would launch it with no overrides.
    ///
    /// Kept so an override can be shown against the default it replaced, and reset
    /// back to it. `None` for an agent that exists only because the user configured
    /// it, which has no shipped default to return to.
    pub shipped: Option<AgentSpec>,
}

impl AgentEntry {
    /// Whether the user has changed how this agent is launched.
    pub fn is_overridden(&self) -> bool {
        self.shipped
            .as_ref()
            .is_some_and(|shipped| shipped != &self.spec)
    }

    /// Whether this agent can actually be run right now.
    pub fn is_runnable(&self) -> bool {
        self.installed && self.enabled
    }
}

/// What resolution decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Run this agent.
    Agent(Box<AgentSpec>),
    /// Deliberately no agent — open a plain terminal.
    NoAgent,
    /// Nothing could be chosen, and why.
    ///
    /// Carries a reason rather than being an `Option`, because "you have no agents
    /// installed" and "the agent you named is switched off" need different answers
    /// from the person reading them.
    Unavailable(String),
}

/// Every agent ket knows about on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalogue {
    entries: Vec<AgentEntry>,
}

impl Catalogue {
    /// Detects what is installed and merges the user's configuration over it.
    ///
    /// Configured agents that are not in [`CANDIDATES`] are kept: someone who has
    /// written their own `AgentSpec` for an agent ket has never heard of should not
    /// have it deleted by a detection pass. Such an entry is reported installed,
    /// since ket has no way to probe for something it does not know.
    pub fn detect(config: &Config, prober: &dyn Prober) -> Self {
        let shipped = default_agents();
        let mut entries = Vec::new();

        for candidate in CANDIDATES {
            // The user's spec wins over the shipped one; that is what an override
            // *is*. An agent with no configured spec falls back to the shipped one.
            let shipped_spec = shipped.iter().find(|spec| spec.name == candidate.name);
            let configured = config
                .agents
                .iter()
                .find(|spec| spec.name == candidate.name);

            let Some(spec) = configured.or(shipped_spec) else {
                continue;
            };
            // The first word of the override, not all of it: a command line
            // may carry its own arguments, and `command -v 'claude --resume'`
            // answers for nothing.
            let probe = spec
                .launch
                .as_ref()
                .and_then(|launch| launch.command.split_whitespace().next())
                .unwrap_or(candidate.probe);
            let found = prober.find(probe);

            entries.push(AgentEntry {
                name: candidate.name.to_owned(),
                installed: found.is_some(),
                found_at: found,
                enabled: !config.agent.disabled.contains(candidate.name),
                spec: spec.clone(),
                shipped: shipped_spec.cloned(),
            });
        }

        // Models a local Ollama has pulled — zero to many, named by whatever
        // the user ran `ollama pull` on, so this is an append pass rather
        // than a fifth row in `CANDIDATES`. Checked before the loop below so
        // a model the user has *also* hand-written a `config.agents` entry
        // for is deduplicated by that loop's own guard, exactly as a shipped
        // candidate would be.
        for model in crate::ollama::installed_models(prober) {
            let name = format!("ollama:{}", model.tag);
            if entries.iter().any(|entry| entry.name == name) {
                continue;
            }

            let configured = config.agents.iter().find(|spec| spec.name == name);
            let spec = configured.cloned().unwrap_or_else(|| AgentSpec {
                name: name.clone(),
                transport: crate::config::Transport::Pty,
                command: "ollama".to_owned(),
                args: vec!["run".to_owned(), model.tag.clone()],
                env: Default::default(),
                env_remove: Vec::new(),
                launch: None,
            });

            entries.push(AgentEntry {
                name: name.clone(),
                installed: true,
                found_at: prober.find("ollama"),
                enabled: !config.agent.disabled.contains(&name),
                spec,
                // Nothing shipped to reset to — same as an agent the user
                // configured by hand (see the loop below).
                shipped: None,
            });
        }

        // Anything the user configured that ket cannot probe for.
        for spec in &config.agents {
            if entries.iter().any(|entry| entry.name == spec.name) {
                continue;
            }

            entries.push(AgentEntry {
                name: spec.name.clone(),
                installed: true,
                found_at: None,
                enabled: !config.agent.disabled.contains(&spec.name),
                spec: spec.clone(),
                shipped: None,
            });
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Self { entries }
    }

    /// Every agent, in name order.
    pub fn entries(&self) -> &[AgentEntry] {
        &self.entries
    }

    /// One agent by name.
    pub fn get(&self, name: &str) -> Option<&AgentEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// Agents that could be run right now, in name order.
    pub fn runnable(&self) -> impl Iterator<Item = &AgentEntry> {
        self.entries.iter().filter(|entry| entry.is_runnable())
    }

    /// Chooses an agent.
    ///
    /// Order: an explicitly named agent, then the project's preference, then the
    /// configured default. `Auto` resolves to **the first runnable agent in name
    /// order** — stable across runs, which matters more than being clever. An
    /// `Auto` that picked differently on two runs of the same command would be
    /// worse than having no `Auto` at all.
    ///
    /// An explicit name that is switched off or missing is an error rather than a
    /// silent fallback: someone who typed `ket run codex` wants Codex, and quietly
    /// running Claude instead is the kind of help nobody asked for.
    pub fn resolve(
        &self,
        explicit: Option<&str>,
        project_preference: Option<&str>,
        default: &DefaultAgent,
    ) -> Resolution {
        if let Some(name) = explicit {
            return self.named(name);
        }

        if let Some(name) = project_preference {
            // A project preference for an agent that is gone falls through rather
            // than failing: the preference was set once, possibly on another
            // machine, and is a hint rather than an instruction.
            if let Some(entry) = self.get(name).filter(|entry| entry.is_runnable()) {
                return Resolution::Agent(Box::new(entry.spec.clone()));
            }
        }

        match default {
            DefaultAgent::None => Resolution::NoAgent,
            DefaultAgent::Named(name) => self.named(name),
            DefaultAgent::Auto => match self.runnable().next() {
                Some(entry) => Resolution::Agent(Box::new(entry.spec.clone())),
                None => Resolution::Unavailable(self.why_nothing_runnable()),
            },
        }
    }

    /// Resolves one agent by name, explaining any refusal.
    fn named(&self, name: &str) -> Resolution {
        match self.get(name) {
            None => Resolution::Unavailable(format!("no agent named `{name}`")),
            Some(entry) if !entry.installed => {
                Resolution::Unavailable(format!("`{name}` is not installed"))
            }
            Some(entry) if !entry.enabled => {
                Resolution::Unavailable(format!("`{name}` is switched off in settings"))
            }
            Some(entry) => Resolution::Agent(Box::new(entry.spec.clone())),
        }
    }

    /// Why `Auto` found nothing, in terms a person can act on.
    fn why_nothing_runnable(&self) -> String {
        let installed = self.entries.iter().filter(|entry| entry.installed).count();

        if installed == 0 {
            let names: Vec<&str> = CANDIDATES.iter().map(|c| c.name).collect();
            return format!("no agents installed; ket looks for {}", names.join(", "));
        }

        "every installed agent is switched off in settings".to_owned()
    }
}

/// A named shorthand for a whole permission posture.
///
/// The per-tool policy in [`crate::config::PermissionConfig`] is more expressive
/// than this, and stays the underlying model — these are two points in it that are
/// worth naming, not a second system beside it. A second permission system that
/// could disagree with the first is how a bug nobody can reproduce gets built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionPreset {
    /// Let the agent act without asking.
    Yolo,
    /// Ask before every tool the policy does not already settle.
    Manual,
    /// Neither — the policy has been tuned by hand.
    ///
    /// Reported, never selected. A settings screen must be able to say "Custom"
    /// rather than claiming a preset that is not actually in effect.
    Custom,
}

/// A set of disabled agent names.
pub type DisabledAgents = BTreeSet<String>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A prober with a fixed answer, so tests do not depend on this machine.
    struct Fake(Vec<&'static str>);

    impl Prober for Fake {
        fn find(&self, binary: &str) -> Option<PathBuf> {
            self.0
                .contains(&binary)
                .then(|| PathBuf::from("/fake/bin").join(binary))
        }
    }

    fn config() -> Config {
        Config::default()
    }

    #[test]
    fn the_real_prober_finds_a_binary_that_certainly_exists() {
        // Every other test here uses a fake, which proves the merging logic and
        // nothing about the `PATH` walk itself. `sh` is on every machine this
        // will ever run on, so this checks the real prober without asserting
        // which agents a given developer happens to have installed.
        assert!(PathProber.find("sh").is_some(), "sh should be on PATH");
        assert!(PathProber.find("ket-no-such-binary-exists").is_none());
    }

    #[test]
    fn the_real_prober_ignores_a_path_entry_that_is_not_executable() {
        let dir = std::env::temp_dir().join(format!("ket-prober-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let plain = dir.join("ket-not-executable");
        std::fs::write(&plain, b"not a program").expect("write");

        // A file of the right name that cannot be run is not an installed agent,
        // and reporting it as one would fail later at spawn time instead.
        assert!(!is_executable(&plain));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detection_reports_only_what_is_installed() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude", "opencode"]));

        assert!(catalogue.get("claude").unwrap().installed);
        assert!(catalogue.get("opencode").unwrap().installed);
        assert!(!catalogue.get("codex").unwrap().installed);
    }

    #[test]
    fn an_agent_that_is_not_installed_is_still_listed() {
        // A settings screen shows what ket looked for and did not find, rather
        // than a shorter list that silently omits it.
        let catalogue = Catalogue::detect(&config(), &Fake(vec![]));
        assert_eq!(catalogue.entries().len(), CANDIDATES.len());
    }

    #[test]
    fn an_agent_that_vanished_keeps_its_overrides() {
        // The case most likely to be got wrong: customise an agent, uninstall it,
        // reinstall it, and find your configuration gone.
        let mut config = config();
        let spec = config
            .agents
            .iter_mut()
            .find(|spec| spec.name == "claude")
            .expect("claude is a shipped agent");
        spec.command = "claude-personal".to_owned();

        let catalogue = Catalogue::detect(&config, &Fake(vec![]));
        let claude = catalogue.get("claude").expect("still listed");

        assert!(!claude.installed);
        assert_eq!(claude.spec.command, "claude-personal");
        assert!(claude.is_overridden());
    }

    #[test]
    fn an_override_is_visible_against_the_shipped_default() {
        let mut config = config();
        config
            .agents
            .iter_mut()
            .find(|spec| spec.name == "codex")
            .expect("codex is shipped")
            .args = vec!["--custom".to_owned()];

        let catalogue = Catalogue::detect(&config, &Fake(vec!["codex"]));
        let codex = catalogue.get("codex").unwrap();

        assert!(codex.is_overridden());
        assert_ne!(codex.shipped.as_ref().unwrap().args, codex.spec.args);
    }

    #[test]
    fn an_unmodified_agent_is_not_reported_as_overridden() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude"]));
        assert!(!catalogue.get("claude").unwrap().is_overridden());
    }

    #[test]
    fn a_disabled_agent_is_not_runnable() {
        let mut config = config();
        config.agent.disabled.insert("claude".to_owned());

        let catalogue = Catalogue::detect(&config, &Fake(vec!["claude", "codex"]));

        assert!(!catalogue.get("claude").unwrap().is_runnable());
        assert!(
            catalogue.get("claude").unwrap().installed,
            "still installed"
        );
    }

    #[test]
    fn an_agent_configured_by_hand_survives_detection() {
        let mut config = config();
        config.agents.push(AgentSpec {
            name: "homegrown".to_owned(),
            transport: crate::config::Transport::Pty,
            command: "my-agent".to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            launch: None,
        });

        let catalogue = Catalogue::detect(&config, &Fake(vec![]));
        let mine = catalogue.get("homegrown").expect("kept");

        assert!(mine.installed, "ket cannot probe for what it does not know");
        assert!(mine.shipped.is_none(), "nothing to reset to");
        assert!(!mine.is_overridden());
    }

    #[test]
    fn auto_resolves_to_the_first_runnable_agent_in_name_order() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["codex", "opencode"]));

        let Resolution::Agent(spec) = catalogue.resolve(None, None, &DefaultAgent::Auto) else {
            panic!("expected an agent");
        };
        assert_eq!(spec.name, "codex");
    }

    #[test]
    fn auto_is_stable_across_repeated_resolution() {
        // An Auto that picked differently on two runs of the same command would be
        // worse than no Auto at all.
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude", "codex", "opencode"]));

        let first = catalogue.resolve(None, None, &DefaultAgent::Auto);
        let second = catalogue.resolve(None, None, &DefaultAgent::Auto);
        assert_eq!(first, second);
    }

    #[test]
    fn an_explicit_agent_that_is_switched_off_is_refused_rather_than_substituted() {
        let mut config = config();
        config.agent.disabled.insert("codex".to_owned());
        let catalogue = Catalogue::detect(&config, &Fake(vec!["claude", "codex"]));

        let Resolution::Unavailable(why) =
            catalogue.resolve(Some("codex"), None, &DefaultAgent::Auto)
        else {
            panic!("expected a refusal, not a substitution");
        };
        assert!(why.contains("switched off"), "unhelpful: {why}");
    }

    #[test]
    fn an_explicit_agent_that_is_not_installed_says_so() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude"]));

        let Resolution::Unavailable(why) =
            catalogue.resolve(Some("codex"), None, &DefaultAgent::Auto)
        else {
            panic!("expected a refusal");
        };
        assert!(why.contains("not installed"), "unhelpful: {why}");
    }

    #[test]
    fn a_project_preference_for_a_missing_agent_falls_through_to_the_default() {
        // A preference set once, possibly on another machine, is a hint rather
        // than an instruction.
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude"]));

        let Resolution::Agent(spec) = catalogue.resolve(None, Some("codex"), &DefaultAgent::Auto)
        else {
            panic!("expected a fallback");
        };
        assert_eq!(spec.name, "claude");
    }

    #[test]
    fn a_project_preference_wins_over_the_configured_default() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude", "codex"]));

        let Resolution::Agent(spec) = catalogue.resolve(
            None,
            Some("codex"),
            &DefaultAgent::Named("claude".to_owned()),
        ) else {
            panic!("expected codex");
        };
        assert_eq!(spec.name, "codex");
    }

    #[test]
    fn the_none_default_opens_a_terminal_rather_than_failing() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec!["claude"]));
        assert_eq!(
            catalogue.resolve(None, None, &DefaultAgent::None),
            Resolution::NoAgent
        );
    }

    #[test]
    fn nothing_installed_explains_what_ket_looked_for() {
        let catalogue = Catalogue::detect(&config(), &Fake(vec![]));

        let Resolution::Unavailable(why) = catalogue.resolve(None, None, &DefaultAgent::Auto)
        else {
            panic!("expected a refusal");
        };
        assert!(
            why.contains("claude"),
            "should name what it looked for: {why}"
        );
    }

    #[test]
    fn everything_switched_off_is_a_different_message_from_nothing_installed() {
        let mut config = config();
        for candidate in CANDIDATES {
            config.agent.disabled.insert(candidate.name.to_owned());
        }
        let catalogue = Catalogue::detect(&config, &Fake(vec!["claude"]));

        let Resolution::Unavailable(why) = catalogue.resolve(None, None, &DefaultAgent::Auto)
        else {
            panic!("expected a refusal");
        };
        assert!(why.contains("switched off"), "unhelpful: {why}");
    }
}
