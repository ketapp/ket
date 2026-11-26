//! Permission rules: what an agent may run, decided before it runs it.
//!
//! A rule names a tool and, optionally, one thing about the call — the
//! command, the file, the address, or that the file lies outside the
//! worktree — and says allow, ask or deny, with a reason. Rules come from
//! four places, each its own scope:
//!
//! - **Built-in**: ket's own guardrails, asking before an agent edits the
//!   files that decide what it may do. On unless `builtins = false`.
//! - **Repository**: `[[permissions.rules]]` in the project's `.ket.toml`,
//!   read from the main checkout like its hooks. Deny and ask only — a
//!   cloned repository can tighten what its agents may do, never loosen it,
//!   so its allows are dropped and counted.
//! - **Project**: a person's own rules for one project, kept in
//!   [`paths::permissions_file`] and written by "Always allow in this
//!   project" and "Never".
//! - **Global**: a person's rules for every project, in the same file.
//!
//! Within a scope the first matching rule wins, so a list can be ordered.
//! Across scopes the strictest verdict wins — deny over ask over allow — so
//! one "ask before `git push`" cannot be undone for a project by accident.
//! Nothing matching anywhere is no ruling: the agent's own settings decide.
//!
//! A prompt the rules leave to a person — no ruling, or an ask — goes past
//! the `agent.permission_requested` hooks first (see [`crate::hook`]). One
//! that exits non-zero refuses the call, with what it printed as the reason,
//! and the ruling names it as [`Scope::Hook`]; one that exits `0` lets the
//! prompt through. A hook never allows a call. Claude Code only.
//!
//! Enforced from the agent's hook, which runs this binary with
//! [`HOOK_ARG`] for every call — see [`serve_if_asked`]. Claude Code takes a
//! deny or an ask before the tool runs (`PreToolUse`) and an allow only when
//! it would have asked a person (`PermissionRequest`), so ket never answers a
//! call the person's own Claude settings deny. A failure anywhere — an
//! unreadable file, a payload too large to read — prints nothing, which
//! leaves the call to the agent, as if ket were not there.
//!
//! These are guardrails, not containment. A script, an alias or a command
//! assembled at run time can do what no pattern sees; allows are therefore
//! conservative (a command ket cannot see all of is never allowed) and
//! denies are best effort.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::write_atomically;
use crate::{KetError, Result, paths};

/// The argument the hook runs this binary with.
pub const HOOK_ARG: &str = "--permission-hook";

/// The environment variable naming the binary to run with [`HOOK_ARG`]: set
/// on every launch, and its absence keeps the hook from asking at all.
pub const ENV: &str = "KET_PERMISSION_HOOK";

/// The project a launch belongs to, for its personal rules.
pub const PROJECT_ID_ENV: &str = "KET_PROJECT_ID";

/// The project's main checkout, where its `.ket.toml` is read.
pub const PROJECT_ROOT_ENV: &str = "KET_PROJECT_ROOT";

/// The worktree a launch runs in, which relative patterns are anchored to.
pub const WORKTREE_ROOT_ENV: &str = "KET_WORKTREE_ROOT";

/// The most of a payload the hook reads. A `Write` carries the whole file, so
/// this is far above the filter's bound; past it the call goes unruled.
const MAX_STDIN: usize = 16 * 1024 * 1024;

/// The most of a refusing hook's output the agent is told, in characters.
const HOOK_REASON_MAX: usize = 200;

/// What a rule says about a call.
///
/// Ordered by strictness, which is what [`RuleSet::evaluate`] takes the
/// maximum of across scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Let it run without asking.
    Allow,
    /// Ask a person, whatever the agent's own settings say.
    Ask,
    /// Refuse it, with the reason.
    Deny,
}

impl Verdict {
    /// The word for it, as written in a rule.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Ask => "ask",
            Verdict::Deny => "deny",
        }
    }
}

/// Where a rule came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// ket's own guardrails.
    Builtin,
    /// The project's `.ket.toml`.
    Repo,
    /// The person's rules for this project.
    Project,
    /// The person's rules for every project.
    Global,
    /// A lifecycle hook — see [`crate::hook`].
    Hook,
}

impl Scope {
    /// How a person reads it.
    pub fn label(self) -> &'static str {
        match self {
            Scope::Builtin => "built-in",
            Scope::Repo => "repository",
            Scope::Project => "project",
            Scope::Global => "global",
            Scope::Hook => "hook",
        }
    }
}

/// One or more patterns, written as a string or a list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patterns(pub Vec<String>);

impl Patterns {
    /// One pattern.
    pub fn one(pattern: impl Into<String>) -> Self {
        Self(vec![pattern.into()])
    }

    /// Whether there are none — a field the rule does not use.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Patterns {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self.0.as_slice() {
            [one] => serializer.serialize_str(one),
            many => many.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Patterns {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Either::deserialize(deserializer)? {
            Either::One(one) => Self(vec![one]),
            Either::Many(many) => Self(many),
        })
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// One rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Which tools: a name in Claude Code's spelling (`Bash`, `Edit`), a glob
    /// (`mcp__github__*`), or a group — `shell`, `read`, `edit`, `web`,
    /// `mcp`, `*`.
    pub tool: Patterns,
    /// The shell commands it covers, as globs over each simple command.
    #[serde(default, skip_serializing_if = "Patterns::is_empty")]
    pub command: Patterns,
    /// The files it covers. Relative patterns start at the worktree.
    #[serde(default, skip_serializing_if = "Patterns::is_empty")]
    pub path: Patterns,
    /// The hosts it covers, for a tool that fetches an address.
    #[serde(default, skip_serializing_if = "Patterns::is_empty")]
    pub domain: Patterns,
    /// Covers a file anywhere outside the worktree.
    #[serde(default, skip_serializing_if = "is_false")]
    pub outside_worktree: bool,
    /// What it says.
    pub decision: Verdict,
    /// Why, for the agent and the person reading the refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Who wrote it: `mac`, `phone:<device>`, `settings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
    /// When it was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at_ms: Option<u64>,
}

impl Rule {
    /// A rule for one tool, matching every use of it.
    pub fn for_tool(tool: impl Into<String>, decision: Verdict) -> Self {
        Self {
            tool: Patterns::one(tool),
            command: Patterns::default(),
            path: Patterns::default(),
            domain: Patterns::default(),
            outside_worktree: false,
            decision,
            reason: None,
            added_by: None,
            added_at_ms: None,
        }
    }

    /// Refuses a rule that says two things about a call, or a pattern that
    /// will not compile: either would match something other than what its
    /// writer meant.
    pub fn validate(&self) -> Result<()> {
        let matchers = [
            !self.command.is_empty(),
            !self.path.is_empty(),
            !self.domain.is_empty(),
            self.outside_worktree,
        ]
        .into_iter()
        .filter(|used| *used)
        .count();
        if matchers > 1 {
            return Err(KetError::Config(
                "a permission rule may use one of command, path, domain or outside_worktree"
                    .to_owned(),
            ));
        }
        if self.tool.is_empty() {
            return Err(KetError::Config(
                "a permission rule needs a tool".to_owned(),
            ));
        }
        for pattern in self.tool.0.iter().chain(&self.domain.0) {
            glob(pattern, false)?;
        }
        for pattern in &self.command.0 {
            glob(pattern, false)?;
        }
        for pattern in &self.path.0 {
            glob(pattern, true)?;
        }
        Ok(())
    }

    /// What the rule is about, in a few words: `cargo test *`, `src/**`,
    /// `docs.rs`, or the tool when it covers every use.
    pub fn subject(&self) -> String {
        let join = |patterns: &Patterns| patterns.0.join(", ");
        if !self.command.is_empty() {
            join(&self.command)
        } else if !self.path.is_empty() {
            join(&self.path)
        } else if !self.domain.is_empty() {
            join(&self.domain)
        } else if self.outside_worktree {
            format!("{} outside the worktree", join(&self.tool))
        } else {
            join(&self.tool)
        }
    }
}

/// The rules a person keeps for one project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRules {
    /// First match wins.
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// `permissions.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// The file's format.
    #[serde(default = "version")]
    pub version: u32,
    /// Whether ket's built-in guardrails apply.
    #[serde(default = "yes")]
    pub builtins: bool,
    /// Global rules. First match wins.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Per-project rules, by project id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ProjectRules>,
}

fn version() -> u32 {
    1
}

fn yes() -> bool {
    true
}

impl Default for RuleFile {
    fn default() -> Self {
        Self {
            version: version(),
            builtins: true,
            rules: Vec::new(),
            projects: BTreeMap::new(),
        }
    }
}

impl RuleFile {
    /// Reads the person's file; a missing one is an empty one.
    pub fn load() -> Result<Self> {
        Self::load_from(&paths::permissions_file()?)
    }

    /// Reads `path`; a missing file is an empty one.
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(KetError::io(path, e)),
        };
        let file: Self = toml::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?;
        for rule in file
            .rules
            .iter()
            .chain(file.projects.values().flat_map(|p| &p.rules))
        {
            rule.validate()
                .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?;
        }
        Ok(file)
    }

    /// Writes it to `path`.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| KetError::Config(format!("permissions: {e}")))?;
        write_atomically(path, &text, false)
    }
}

/// The `[permissions]` table a repository's `.ket.toml` may carry.
#[derive(Debug, Clone, Default, Deserialize)]
struct RepoPermissions {
    #[serde(default)]
    rules: Vec<Rule>,
}

/// A repository's rules, as read: the ones that apply, and what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoRules {
    /// Its deny and ask rules, in order.
    pub rules: Vec<Rule>,
    /// Its allow rules, which a repository may not make.
    pub ignored: Vec<Rule>,
    /// Why its `[permissions]` table could not be read, when it could not.
    pub error: Option<String>,
}

impl RepoRules {
    /// Reads `<root>/.ket.toml`. A missing file, or one without a
    /// `[permissions]` table, has no rules.
    pub fn load(root: &Path) -> Self {
        let path = root.join(".ket.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let parsed = toml::from_str::<toml::Table>(&text)
            .map_err(|e| e.to_string())
            .and_then(|table| match table.get("permissions") {
                None => Ok(RepoPermissions::default()),
                Some(value) => value
                    .clone()
                    .try_into::<RepoPermissions>()
                    .map_err(|e| e.to_string()),
            })
            .and_then(|repo| {
                repo.rules
                    .iter()
                    .try_for_each(Rule::validate)
                    .map(|()| repo)
                    .map_err(|e| e.to_string())
            });
        match parsed {
            Ok(repo) => {
                let (ignored, rules) = repo
                    .rules
                    .into_iter()
                    .partition(|rule| rule.decision == Verdict::Allow);
                Self {
                    rules,
                    ignored,
                    error: None,
                }
            }
            Err(error) => Self {
                error: Some(format!("{}: {error}", path.display())),
                ..Self::default()
            },
        }
    }
}

/// ket's guardrails: ask before an agent edits what decides what it may do,
/// or reaches for the rules from the shell.
pub fn builtin_rules(home: Option<&Path>) -> Vec<Rule> {
    let reason = Some("ket's guardrail: this file decides what agents may do".to_owned());
    let mut files = vec![".ket.toml".to_owned()];
    if let Ok(dir) = paths::config_dir() {
        files.push(format!("{}/**", dir.display()));
    }
    if let Some(home) = home {
        let home = home.display();
        files.extend([
            format!("{home}/.ket/**"),
            format!("{home}/.claude/settings*.json"),
            format!("{home}/.codex/hooks.json"),
            format!("{home}/.codex/config.toml"),
            format!("{home}/.grok/hooks/**"),
            format!("{home}/.config/opencode/plugin/**"),
        ]);
    }
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        files.push(format!("{}/settings*.json", PathBuf::from(dir).display()));
    }
    vec![
        Rule {
            path: Patterns(files),
            reason: reason.clone(),
            ..Rule::for_tool("edit", Verdict::Ask)
        },
        Rule {
            command: Patterns(
                // Only commands that write: a `grep` or a `cat` naming the
                // file is reading the rules, which is fine, and an ask on
                // every mention stopped sessions working on ket itself.
                GUARDED_FILES
                    .iter()
                    .flat_map(|file| {
                        WRITERS
                            .iter()
                            .map(move |writer| format!("{writer} *{file}*"))
                            .chain([format!("*> *{file}*"), format!("*>>*{file}*")])
                    })
                    .collect(),
            ),
            reason,
            ..Rule::for_tool("shell", Verdict::Ask)
        },
    ]
}

/// The files the shell guardrail watches for writes.
const GUARDED_FILES: [&str; 2] = ["permissions.toml", "ket-agent-hook"];

/// The programs that change a file named after them.
const WRITERS: [&str; 7] = ["rm", "mv", "cp", "tee", "sed -i", "chmod", "truncate"];

/// The ruling a rule set gives one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ruling {
    /// What to do.
    pub verdict: Verdict,
    /// Which scope's rule decided.
    pub scope: Scope,
    /// The rule's place in that scope's list.
    pub index: usize,
    /// The rule's reason, when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Ruling {
    /// What the agent and the person are told: the reason, and which rule.
    pub fn explanation(&self) -> String {
        let which = match self.scope {
            Scope::Hook => format!("hook {}", self.index + 1),
            scope => format!("{} rule {}", scope.label(), self.index + 1),
        };
        match self.reason.as_deref() {
            Some(reason) => format!("ket: {reason} ({which})"),
            None => format!("ket: {which}"),
        }
    }
}

/// One call, as the rules see it.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The tool, in Claude Code's spelling.
    pub tool: &'a str,
    /// Its input, as the agent sent it.
    pub input: &'a serde_json::Value,
    /// The worktree the call runs in, which relative paths are joined to.
    pub worktree: Option<&'a Path>,
    /// The home directory, for `~`.
    pub home: Option<&'a Path>,
}

/// Every rule that applies to one project, by scope.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    scopes: Vec<(Scope, Vec<Rule>)>,
}

impl RuleSet {
    /// The rules for `project`, whose main checkout is `root`: the person's
    /// file, the repository's deny and ask rules, and the guardrails.
    ///
    /// Fails only when the person's own file cannot be read — the caller
    /// then rules nothing, rather than guessing at what the file meant. A
    /// repository whose table cannot be read contributes nothing.
    pub fn load(project: Option<&str>, root: Option<&Path>, home: Option<&Path>) -> Result<Self> {
        let file = RuleFile::load()?;
        let repo = root.map(RepoRules::load).unwrap_or_default();
        Ok(Self::from_parts(&file, project, repo.rules, home))
    }

    /// The rules from parts already read.
    pub fn from_parts(
        file: &RuleFile,
        project: Option<&str>,
        repo: Vec<Rule>,
        home: Option<&Path>,
    ) -> Self {
        let mut scopes = Vec::new();
        if file.builtins {
            scopes.push((Scope::Builtin, builtin_rules(home)));
        }
        scopes.push((Scope::Repo, repo));
        let personal = project
            .and_then(|id| file.projects.get(id))
            .map(|p| p.rules.clone())
            .unwrap_or_default();
        scopes.push((Scope::Project, personal));
        scopes.push((Scope::Global, file.rules.clone()));
        Self { scopes }
    }

    /// The strictest ruling any scope gives `request`, or `None` when no
    /// scope's rules match it.
    pub fn evaluate(&self, request: &Request<'_>) -> Option<Ruling> {
        let call = Call::of(request);
        let mut best: Option<Ruling> = None;
        for (scope, rules) in &self.scopes {
            let Some(ruling) = rule_scope(*scope, rules, &call, request) else {
                continue;
            };
            // Neither ket's guardrails nor a repository may let anything through.
            if ruling.verdict == Verdict::Allow && matches!(scope, Scope::Builtin | Scope::Repo) {
                continue;
            }
            if best.as_ref().is_none_or(|b| ruling.verdict > b.verdict) {
                best = Some(ruling);
            }
        }
        best
    }

    /// These rules in OpenCode's own spelling, for its `permissions` config:
    /// ket has no hook into OpenCode, so OpenCode is told the rules and
    /// enforces them itself.
    ///
    /// Only ask and deny: an allow would override a deny in the person's own
    /// OpenCode config, and ket only ever tightens what an agent may do.
    /// OpenCode lets the last matching rule win, so asks come first and
    /// denies after them, and each scope's list is reversed — the first rule
    /// in it is the one that ends up last. Where a list puts an allow ahead
    /// of a deny that also matches, OpenCode denies.
    ///
    /// One place it cannot match ket: OpenCode rules `grep` and `glob` by the
    /// search, not the files searched, so a path rule cannot reach them. A
    /// path ask therefore asks before every affected search; a path deny
    /// denies every affected search, since asking would let it be overridden.
    pub fn opencode_permissions(
        &self,
        worktree: Option<&Path>,
        home: Option<&Path>,
    ) -> Vec<serde_json::Value> {
        let mut asks = Vec::new();
        let mut denies = Vec::new();
        let mut search_guards = BTreeMap::<String, Verdict>::new();
        for (_, rules) in &self.scopes {
            for rule in rules.iter().rev() {
                let bucket = match rule.decision {
                    Verdict::Allow => continue,
                    Verdict::Ask => &mut asks,
                    Verdict::Deny => &mut denies,
                };
                if !rule.path.is_empty() {
                    for action in rule.tool.0.iter().flat_map(|tool| opencode_actions(tool)) {
                        if matches!(action.as_str(), "grep" | "glob") {
                            search_guards
                                .entry(action)
                                .and_modify(|verdict| *verdict = (*verdict).max(rule.decision))
                                .or_insert(rule.decision);
                        }
                    }
                }
                for (action, resource) in opencode_pairs(rule, worktree, home) {
                    bucket.push(serde_json::json!({
                        "action": action,
                        "resource": resource,
                        "effect": rule.decision.as_str(),
                    }));
                }
            }
        }
        for (action, verdict) in search_guards {
            let bucket = if verdict == Verdict::Deny {
                &mut denies
            } else {
                &mut asks
            };
            bucket.push(serde_json::json!({
                "action": action,
                "resource": "*",
                "effect": verdict.as_str(),
            }));
        }
        asks.extend(denies);
        asks
    }
}

/// `config`, an OpenCode inline config (`OPENCODE_CONFIG_CONTENT`) or none,
/// with `rules` added after any permissions it already has — last, so they
/// win — and after each of `agents`' own. OpenCode appends an agent's rules
/// after the config's, so a rule only at the top would lose to an agent's
/// allow. Denies also become `tool.use` policies: OpenCode applies those
/// after agent permissions and saved approvals, so they cannot be lifted.
pub fn with_opencode_permissions(
    config: Option<&str>,
    rules: Vec<serde_json::Value>,
    agents: &[String],
) -> String {
    fn append(table: &mut serde_json::Value, rules: &[serde_json::Value]) {
        let Some(table) = table.as_object_mut() else {
            return;
        };
        let permissions = table
            .entry("permissions")
            .or_insert_with(|| serde_json::json!([]));
        match permissions.as_array_mut() {
            Some(list) => list.extend(rules.iter().cloned()),
            None => *permissions = serde_json::Value::Array(rules.to_vec()),
        }
    }
    let mut config = config
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let hard_denies: Vec<_> = rules
        .iter()
        .filter(|rule| rule["effect"] == "deny")
        .filter_map(|rule| {
            Some(serde_json::json!({
                "action": "tool.use",
                "resource": format!("{}:{}", rule["action"].as_str()?, rule["resource"].as_str()?),
                "effect": "deny",
            }))
        })
        .collect();
    append(&mut config, &rules);
    if !hard_denies.is_empty() {
        let policies = config
            .as_object_mut()
            .expect("an object")
            .entry("policies")
            .or_insert_with(|| serde_json::json!([]));
        match policies.as_array_mut() {
            Some(list) => list.extend(hard_denies),
            None => *policies = serde_json::Value::Array(hard_denies),
        }
    }
    if !agents.is_empty() {
        let table = config
            .as_object_mut()
            .expect("an object")
            .entry("agents")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(table) = table.as_object_mut() {
            for agent in agents {
                let entry = table
                    .entry(agent.clone())
                    .or_insert_with(|| serde_json::json!({}));
                append(entry, &rules);
            }
        }
    }
    config.to_string()
}

/// The agents OpenCode at `opencode` has in `directory`, a project's own
/// included. Uses the same inline config as the launch. An incomplete list
/// must be an error: a custom agent's own allow can override a global rule.
/// An answer that lists no agent at all is asked for once more — an OpenCode
/// service answers for a directory it is opening for the first time before
/// it has listed any agent — and an id set still empty after that, the
/// inline config's own agents included, is an error.
pub fn opencode_agents(
    opencode: &Path,
    launch_command: &str,
    directory: &Path,
    inline: Option<&str>,
) -> Result<Vec<String>> {
    // An override may be an alias or a wrapper command. In that case ask the
    // same interactive shell that launches it, rather than trying to exec the
    // text `command -v` printed for an alias or dropping wrapper arguments.
    let direct = opencode.is_file() && launch_command.split_whitespace().count() == 1;
    let marker = "__KET_OPENCODE_AGENTS_JSON__";
    // One `debug agents` run, the ids it listed.
    let ask = || -> Result<std::collections::BTreeSet<String>> {
        let mut command = if direct {
            let mut command = std::process::Command::new(opencode);
            command.args(["debug", "agents"]);
            command
        } else {
            let mut command = std::process::Command::new(crate::shell::login_shell());
            command.args([
                "-i",
                "-c",
                &format!("printf '{marker}\\n'; {launch_command} debug agents"),
            ]);
            command
        };
        command
            .current_dir(directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Some(inline) = inline {
            command.env("OPENCODE_CONFIG_CONTENT", inline);
        }
        let mut child = command
            .spawn()
            .map_err(|e| KetError::Config(format!("could not inspect OpenCode agents: {e}")))?;
        // Drain concurrently: `debug agents` writes whole agent definitions, and
        // a full pipe would otherwise keep the child from exiting until timeout.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| KetError::Config("OpenCode did not provide agent output".to_owned()))?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut limited = std::io::Read::take(stdout, 2 * 1024 * 1024);
            let result = std::io::Read::read_to_end(&mut limited, &mut out).map(|_| out);
            let _ = sender.send(result);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(KetError::Config(
                        "OpenCode did not list its agents in time".to_owned(),
                    ));
                }
            }
        };
        if !status.success() {
            return Err(KetError::Config(format!(
                "OpenCode could not list its agents ({status})"
            )));
        }
        let out = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .map_err(|_| KetError::Config("OpenCode agent output did not finish".to_owned()))?
            .map_err(|e| KetError::Config(format!("could not read OpenCode agents: {e}")))?;
        let out = if direct {
            out.as_slice()
        } else {
            let Some(at) = out
                .windows(marker.len())
                .position(|window| window == marker.as_bytes())
            else {
                return Err(KetError::Config(
                    "OpenCode's shell did not start agent discovery".to_owned(),
                ));
            };
            &out[at + marker.len()..]
        };
        let listed: Vec<serde_json::Value> = serde_json::from_slice(out)
            .map_err(|e| KetError::Config(format!("could not parse OpenCode agents: {e}")))?;
        let ids: std::collections::BTreeSet<String> = listed
            .iter()
            .filter_map(|agent| agent.get("id")?.as_str().map(str::to_owned))
            .collect();
        Ok(ids)
    };
    let mut ids = ask()?;
    if ids.is_empty() {
        // A background OpenCode service answers `debug agents` for a
        // directory it is opening for the first time before that opening has
        // finished, with no agent listed yet. The ask itself is what finishes
        // opening it, so ask again rather than take the empty list as the
        // answer: a working OpenCode always lists its built-in agents, so one
        // that comes back empty twice is a real error, not a late boot.
        std::thread::sleep(std::time::Duration::from_millis(250));
        ids = ask()?;
    }
    // A running OpenCode service can answer `debug agents` using an older
    // config. Inline agent definitions still need ket's rules on this launch.
    if let Some(inline) = inline
        && let Ok(config) = serde_json::from_str::<serde_json::Value>(inline)
        && let Some(agents) = config.get("agents").and_then(serde_json::Value::as_object)
    {
        ids.extend(agents.keys().cloned());
    }
    if ids.is_empty() {
        return Err(KetError::Config("OpenCode listed no agents".to_owned()));
    }
    Ok(ids.into_iter().collect())
}

/// The OpenCode actions a tool pattern covers. A Claude Code tool with no
/// OpenCode counterpart covers none.
fn opencode_actions(pattern: &str) -> Vec<String> {
    let names: &[&str] = match pattern {
        "*" => &[
            "shell",
            "read",
            "glob",
            "grep",
            "edit",
            "webfetch",
            "websearch",
        ],
        "shell" | "Bash" => &["shell"],
        "read" => &["read", "glob", "grep"],
        "Read" | "LS" | "NotebookRead" => &["read"],
        "Glob" => &["glob"],
        "Grep" => &["grep"],
        "edit" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => &["edit"],
        "web" => &["webfetch", "websearch"],
        "WebFetch" => &["webfetch"],
        "WebSearch" => &["websearch"],
        other => {
            // An MCP tool: Claude's `mcp__<server>__<tool>` is OpenCode's
            // `<server>_<tool>`.
            return other
                .strip_prefix("mcp__")
                .map(|rest| vec![rest.replacen("__", "_", 1)])
                .unwrap_or_default();
        }
    };
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A path pattern as OpenCode reads it: anchored the way ket anchors it, and
/// with `**` written `*`, since OpenCode's `*` already crosses `/`. That
/// makes a single `*` cross it too — wider, which an ask or a deny can
/// afford.
fn opencode_path(pattern: &str, worktree: Option<&Path>, home: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(anchored) = anchor(pattern, worktree, home) {
        out.push(anchored.replace("**", "*"));
    }
    // OpenCode may name a file relative to where it runs.
    if !pattern.starts_with('/') && !pattern.starts_with("~/") {
        out.push(pattern.replace("**", "*"));
    }
    out
}

/// Every (action, resource) pair one rule becomes.
fn opencode_pairs(
    rule: &Rule,
    worktree: Option<&Path>,
    home: Option<&Path>,
) -> Vec<(String, String)> {
    let actions: Vec<String> = rule
        .tool
        .0
        .iter()
        .flat_map(|t| opencode_actions(t))
        .collect();
    let mut pairs = Vec::new();
    if rule.outside_worktree {
        if !actions.is_empty() {
            pairs.push(("external_directory".to_owned(), "*".to_owned()));
        }
        return pairs;
    }
    for action in &actions {
        let shell = action == "shell";
        // `grep` and `glob` are ruled by the search, not the files — see
        // `RuleSet::opencode_permissions`.
        let files = matches!(action.as_str(), "read" | "edit");
        if !rule.command.is_empty() {
            if shell {
                pairs.extend(rule.command.0.iter().map(|c| (action.clone(), c.clone())));
            }
        } else if !rule.path.is_empty() {
            if files {
                for pattern in &rule.path.0 {
                    for resource in opencode_path(pattern, worktree, home) {
                        pairs.push((action.clone(), resource));
                    }
                }
            }
        } else if !rule.domain.is_empty() {
            if action == "webfetch" {
                for domain in &rule.domain.0 {
                    pairs.push((action.clone(), format!("*://{domain}")));
                    pairs.push((action.clone(), format!("*://{domain}/*")));
                }
            }
        } else {
            pairs.push((action.clone(), "*".to_owned()));
        }
    }
    pairs
}

/// What one call offers a rule to match on.
struct Call {
    /// The simple commands of a shell call, each normalised.
    commands: Vec<String>,
    /// Whether the command holds anything ket cannot see all of — a
    /// substitution, an `eval`, a string handed to a shell — which no allow
    /// may cover.
    opaque: bool,
    /// The file a call names: as written, joined and normalised, and with
    /// symlinks resolved as far as the path exists.
    path: Option<(PathBuf, PathBuf)>,
    /// The host a call fetches.
    domain: Option<String>,
}

impl Call {
    fn of(request: &Request<'_>) -> Self {
        let text = |key: &str| {
            request
                .input
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let (commands, opaque) = match text("command") {
            Some(command) if in_group("shell", request.tool) => simple_commands(command),
            _ => (Vec::new(), false),
        };
        let path = ["file_path", "notebook_path", "path"]
            .into_iter()
            .find_map(text)
            .map(|raw| resolve(raw, request.worktree, request.home));
        let domain = text("url").and_then(host);
        Self {
            commands,
            opaque,
            path,
            domain,
        }
    }
}

/// The first rule in `rules` that matches each part of `call`, combined.
///
/// A shell call is ruled piece by piece: any piece denied or asked about is
/// enough to deny or ask, but an allow needs every piece allowed and nothing
/// hidden.
fn rule_scope(scope: Scope, rules: &[Rule], call: &Call, request: &Request<'_>) -> Option<Ruling> {
    let first = |piece: Option<&str>| {
        rules.iter().enumerate().find_map(|(index, rule)| {
            matches(rule, call, piece, request).then(|| Ruling {
                verdict: rule.decision,
                scope,
                index,
                reason: rule.reason.clone(),
            })
        })
    };

    if call.commands.is_empty() {
        return first(None);
    }
    let rulings: Vec<Option<Ruling>> = call
        .commands
        .iter()
        .map(|piece| first(Some(piece)))
        .collect();
    let strictest = rulings
        .iter()
        .flatten()
        .max_by_key(|ruling| ruling.verdict)
        .cloned()?;
    if strictest.verdict == Verdict::Allow && (call.opaque || rulings.iter().any(Option::is_none)) {
        return None;
    }
    Some(strictest)
}

/// Whether `rule` covers this call — for a shell call, this one piece of it.
fn matches(rule: &Rule, call: &Call, piece: Option<&str>, request: &Request<'_>) -> bool {
    if !rule
        .tool
        .0
        .iter()
        .any(|tool| tool_matches(tool, request.tool))
    {
        return false;
    }
    if !rule.command.is_empty() {
        let Some(piece) = piece else {
            return false;
        };
        return rule
            .command
            .0
            .iter()
            .any(|pattern| command_matches(pattern, piece));
    }
    if !rule.path.is_empty() {
        let Some((logical, real)) = &call.path else {
            return false;
        };
        let hits = |path: &Path| {
            rule.path.0.iter().any(|pattern| {
                anchor(pattern, request.worktree, request.home)
                    .and_then(|pattern| glob(&pattern, true).ok())
                    .is_some_and(|glob| glob.is_match(path))
            })
        };
        // A refusal on either spelling of the path; a pass only on both, so a
        // symlink can neither dodge a deny nor borrow an allow.
        return match rule.decision {
            Verdict::Allow => hits(logical) && hits(real),
            Verdict::Ask | Verdict::Deny => hits(logical) || hits(real),
        };
    }
    if !rule.domain.is_empty() {
        let Some(domain) = &call.domain else {
            return false;
        };
        return rule.domain.0.iter().any(|pattern| {
            glob(&pattern.to_ascii_lowercase(), false).is_ok_and(|g| g.is_match(domain))
        });
    }
    if rule.outside_worktree {
        let (Some((_, real)), Some(worktree)) = (&call.path, request.worktree) else {
            return false;
        };
        let worktree = real_path(worktree);
        return !real.starts_with(&worktree);
    }
    true
}

/// The tools each group names.
const GROUPS: [(&str, &[&str]); 4] = [
    ("shell", &["Bash"]),
    ("read", &["Read", "Glob", "Grep", "LS", "NotebookRead"]),
    ("edit", &["Edit", "Write", "MultiEdit", "NotebookEdit"]),
    ("web", &["WebFetch", "WebSearch"]),
];

fn in_group(group: &str, tool: &str) -> bool {
    GROUPS
        .iter()
        .any(|(name, tools)| *name == group && tools.contains(&tool))
}

fn tool_matches(pattern: &str, tool: &str) -> bool {
    match pattern {
        "*" => true,
        "mcp" => tool.starts_with("mcp__"),
        group if GROUPS.iter().any(|(name, _)| *name == group) => in_group(group, tool),
        pattern => glob(pattern, false).is_ok_and(|g| g.is_match(tool)),
    }
}

/// Whether one simple command matches a command pattern. A trailing ` *`
/// also covers the command with nothing after it: `cargo test *` is
/// `cargo test` too.
fn command_matches(pattern: &str, piece: &str) -> bool {
    if let Some(bare) = pattern.strip_suffix(" *")
        && bare == piece
    {
        return true;
    }
    glob(pattern, false).is_ok_and(|g| g.is_match(piece))
}

/// A compiled glob. Over paths `*` stops at `/` and `**` crosses it; over
/// commands, tools and hosts `*` matches anything.
fn glob(pattern: &str, path: bool) -> Result<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(path)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| KetError::Config(format!("permission pattern {pattern:?}: {e}")))
}

/// `path` as a pattern that matches it alone: the characters globs read
/// as wildcards, escaped.
pub fn literal(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A path pattern made absolute: `/…` as is, `~/…` under home, anything else
/// under the worktree. `None` when a relative pattern has no worktree to be
/// relative to.
fn anchor(pattern: &str, worktree: Option<&Path>, home: Option<&Path>) -> Option<String> {
    if pattern.starts_with('/') {
        return Some(pattern.to_owned());
    }
    if let Some(rest) = pattern.strip_prefix("~/") {
        return home.map(|home| format!("{}/{rest}", home.display()));
    }
    worktree.map(|root| format!("{}/{pattern}", root.display()))
}

/// A path as written, made absolute and normalised; and the same with
/// symlinks resolved for as much of it as exists.
fn resolve(raw: &str, worktree: Option<&Path>, home: Option<&Path>) -> (PathBuf, PathBuf) {
    let expanded = match (raw.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(raw),
    };
    let joined = match (expanded.is_absolute(), worktree) {
        (false, Some(root)) => root.join(expanded),
        _ => expanded,
    };
    let logical = normalise(&joined);
    let real = real_path(&logical);
    (logical, real)
}

/// `..` and `.` taken out without touching the disk.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `path` with symlinks resolved through its longest existing ancestor.
fn real_path(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

/// The host of an address, lowercased.
fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The words that only run the command after them.
const WRAPPERS: [&str; 6] = ["env", "command", "exec", "nohup", "time", "sudo"];

/// The shells whose `-c` string is a command line of its own.
const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "fish"];

/// A shell command line split into its simple commands, each as its words
/// joined by single spaces, with leading assignments and wrappers taken off.
/// The flag says whether anything was hidden from view: a substitution, a
/// heredoc, an `eval`, a string handed to a shell. Their contents are still
/// split and returned, so a deny can find them.
pub fn simple_commands(line: &str) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut opaque = false;
    split_into(line, &mut out, &mut opaque, 0);
    (out, opaque)
}

fn split_into(line: &str, out: &mut Vec<String>, opaque: &mut bool, depth: usize) {
    if depth > 8 {
        *opaque = true;
        return;
    }
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    let finish_word = |words: &mut Vec<String>, word: &mut String, in_word: &mut bool| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    let finish_piece = |words: &mut Vec<String>, out: &mut Vec<String>, opaque: &mut bool| {
        if let Some(piece) = piece(std::mem::take(words), out, opaque, depth) {
            out.push(piece);
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    in_word = true;
                }
            }
            '\'' => {
                in_word = true;
                for next in chars.by_ref() {
                    if next == '\'' {
                        break;
                    }
                    word.push(next);
                }
            }
            '"' => {
                in_word = true;
                while let Some(next) = chars.next() {
                    match next {
                        '"' => break,
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                word.push(escaped);
                            }
                        }
                        '$' if chars.peek() == Some(&'(') => {
                            chars.next();
                            let inner = take_balanced(&mut chars);
                            *opaque = true;
                            split_into(&inner, out, opaque, depth + 1);
                            word.push_str("$(…)");
                        }
                        '`' => {
                            let inner: String = chars.by_ref().take_while(|&c| c != '`').collect();
                            *opaque = true;
                            split_into(&inner, out, opaque, depth + 1);
                            word.push_str("`…`");
                        }
                        other => word.push(other),
                    }
                }
            }
            '$' | '<' | '>' if chars.peek() == Some(&'(') => {
                chars.next();
                let inner = take_balanced(&mut chars);
                *opaque = true;
                split_into(&inner, out, opaque, depth + 1);
                word.push_str("$(…)");
                in_word = true;
            }
            '`' => {
                let inner: String = chars.by_ref().take_while(|&c| c != '`').collect();
                *opaque = true;
                split_into(&inner, out, opaque, depth + 1);
                word.push_str("`…`");
                in_word = true;
            }
            '<' if chars.peek() == Some(&'<') => {
                // A heredoc's body is input nobody can see from here.
                *opaque = true;
                finish_word(&mut words, &mut word, &mut in_word);
                word.push_str("<<");
                chars.next();
                in_word = true;
            }
            ';' | '\n' | '&' | '|' | '(' | ')' | '{' | '}' => {
                // `>&2` and `2>&1` are redirections, not a separator.
                if c == '&' && (word.ends_with('>') || word.ends_with('<')) {
                    word.push(c);
                    continue;
                }
                finish_word(&mut words, &mut word, &mut in_word);
                finish_piece(&mut words, out, opaque);
                if (c == '&' || c == '|') && chars.peek() == Some(&c) {
                    chars.next();
                }
            }
            c if c.is_whitespace() => finish_word(&mut words, &mut word, &mut in_word),
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    finish_word(&mut words, &mut word, &mut in_word);
    finish_piece(&mut words, out, opaque);
}

/// What follows `$(` up to its matching `)`.
fn take_balanced(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut depth = 1;
    let mut inner = String::new();
    for c in chars.by_ref() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        inner.push(c);
    }
    inner
}

/// One simple command's words as the rules match them, or `None` when there
/// are none. A string handed to a shell or `eval` is split as a command line
/// of its own.
fn piece(
    words: Vec<String>,
    out: &mut Vec<String>,
    opaque: &mut bool,
    depth: usize,
) -> Option<String> {
    let mut words = words.into_iter().peekable();
    // Leading assignments and wrappers, and a wrapper's own flags and
    // assignments: `sudo -E env FOO=1 cargo test` is a `cargo test`.
    loop {
        match words.peek() {
            Some(w) if is_assignment(w) => {
                words.next();
            }
            Some(w) if WRAPPERS.contains(&w.as_str()) => {
                words.next();
                while words
                    .peek()
                    .is_some_and(|w| w.starts_with('-') || is_assignment(w))
                {
                    words.next();
                }
            }
            _ => break,
        }
    }
    let words: Vec<String> = words.collect();
    let first = words.first()?;
    let program = first.rsplit('/').next().unwrap_or(first);
    if program == "eval" {
        *opaque = true;
        split_into(&words[1..].join(" "), out, opaque, depth + 1);
    } else if SHELLS.contains(&program)
        && let Some(at) = words.iter().position(|w| w == "-c")
        && let Some(script) = words.get(at + 1)
    {
        *opaque = true;
        split_into(script, out, opaque, depth + 1);
    }
    Some(words.join(" "))
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

// ---- remembering -------------------------------------------------------------

/// A rule that covers a call like this one, for "Always allow" and "Never":
/// the program and subcommand for a command (`cargo test *`), the folder for
/// a file in the worktree, the host for an address, the tool for anything
/// else. `None` for a call no single rule should cover — a compound command,
/// or one with parts ket cannot see.
pub fn suggest(
    question: &crate::agent_hooks::PermissionQuestion,
    worktree: Option<&Path>,
    decision: Verdict,
) -> Option<Rule> {
    if question.prompt.is_some()
        || crate::agent_hooks::PROMPT_TOOLS.contains(&question.tool.as_str())
    {
        return None;
    }
    let base = Rule::for_tool(question.tool.clone(), decision);
    let subject = question.subject.as_deref();
    let rule = match question.tool.as_str() {
        "Bash" => {
            let (pieces, opaque) = simple_commands(subject?);
            let [piece] = pieces.as_slice() else {
                return None;
            };
            if opaque {
                return None;
            }
            let program = crate::agent_hooks::command_program(piece);
            if program.is_empty() {
                return None;
            }
            Rule {
                command: Patterns::one(format!("{program} *")),
                ..base
            }
        }
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "NotebookRead" => {
            let (logical, _) = resolve(subject?, worktree, None);
            let path = match worktree.and_then(|root| logical.strip_prefix(root).ok()) {
                Some(inside) => match inside.parent().filter(|p| !p.as_os_str().is_empty()) {
                    Some(folder) => format!("{}/**", folder.display()),
                    None => inside.display().to_string(),
                },
                None => logical.display().to_string(),
            };
            Rule {
                path: Patterns::one(path),
                ..base
            }
        }
        "WebFetch" => Rule {
            domain: Patterns::one(host(subject?)?),
            ..base
        },
        _ => base,
    };
    Some(rule)
}

/// The lock beside the person's file, held while it is rewritten: the window
/// and the host can both write it.
fn locked<T>(change: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let path = paths::permissions_file()?;
    let _lock = crate::store::LockGuard::acquire(path.with_extension("toml.lock"))?;
    change(&path)
}

/// Puts `rule` first in `project`'s personal rules, so the latest answer
/// wins over an older one it overlaps.
pub fn remember(project: &str, mut rule: Rule) -> Result<()> {
    rule.validate()?;
    rule.added_at_ms.get_or_insert_with(crate::now_ms);
    locked(|path| {
        let mut file = RuleFile::load_from(path)?;
        file.projects
            .entry(project.to_owned())
            .or_default()
            .rules
            .insert(0, rule);
        file.save_to(path)
    })
}

/// Takes out the rule at `index` of `project`'s rules, or of the global
/// rules when `project` is `None`.
pub fn forget(project: Option<&str>, index: usize) -> Result<()> {
    locked(|path| {
        let mut file = RuleFile::load_from(path)?;
        let rules = match project {
            Some(id) => &mut file.projects.entry(id.to_owned()).or_default().rules,
            None => &mut file.rules,
        };
        if index < rules.len() {
            rules.remove(index);
        }
        file.projects.retain(|_, p| !p.rules.is_empty());
        file.save_to(path)
    })
}

/// Turns ket's guardrails on or off.
pub fn set_builtins(on: bool) -> Result<()> {
    locked(|path| {
        let mut file = RuleFile::load_from(path)?;
        file.builtins = on;
        file.save_to(path)
    })
}

/// Drops a removed project's personal rules. A project added again starts
/// with none.
pub fn prune_project(project: &str) -> Result<()> {
    locked(|path| {
        let mut file = RuleFile::load_from(path)?;
        if file.projects.remove(project).is_some() {
            file.save_to(path)?;
        }
        Ok(())
    })
}

// ---- the hook -------------------------------------------------------------------

/// What the hook prints for a ruling on `event`: the ruling for ket's own
/// report, then Claude Code's answer. `None` when Claude should be told
/// nothing — an allow before the tool runs, an ask once it is already asking.
pub fn claude_output(event: &str, ruling: &Ruling) -> Option<String> {
    let reason = ruling.explanation();
    let answer = match (event, ruling.verdict) {
        ("PreToolUse", Verdict::Deny | Verdict::Ask) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": ruling.verdict.as_str(),
                "permissionDecisionReason": reason,
            }
        }),
        ("PermissionRequest", Verdict::Allow) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": { "behavior": "allow" },
            }
        }),
        ("PermissionRequest", Verdict::Deny) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": { "behavior": "deny", "message": reason },
            }
        }),
        _ => return None,
    };
    let ruling = serde_json::to_string(ruling).ok()?;
    Some(format!("{ruling}\n{answer}"))
}

/// The hook's answer for one payload, from the environment the launch set.
pub fn hook_answer(payload: &serde_json::Value, agent: &str) -> Option<String> {
    if agent != "claude" {
        return None;
    }
    let event = payload.get("hook_event_name")?.as_str()?;
    if !matches!(event, "PreToolUse" | "PermissionRequest") {
        return None;
    }
    let tool = payload.get("tool_name")?.as_str()?;
    if crate::agent_hooks::PROMPT_TOOLS.contains(&tool) {
        return None;
    }
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let project = env(PROJECT_ID_ENV).and_then(|v| v.into_string().ok());
    let root = env(PROJECT_ROOT_ENV).map(PathBuf::from);
    let home = env("HOME").map(PathBuf::from);
    let worktree = env(WORKTREE_ROOT_ENV).map(PathBuf::from).or_else(|| {
        payload
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
    });
    let rules = RuleSet::load(project.as_deref(), root.as_deref(), home.as_deref()).ok()?;
    let empty = serde_json::Value::Null;
    let mut ruling = rules.evaluate(&Request {
        tool,
        input: payload.get("tool_input").unwrap_or(&empty),
        worktree: worktree.as_deref(),
        home: home.as_deref(),
    });
    if event == "PermissionRequest"
        && ruling.as_ref().is_none_or(|r| r.verdict == Verdict::Ask)
        && let Some(denied) = hook_ruling(payload, tool, project.as_deref(), root.as_deref())
    {
        ruling = Some(denied);
    }
    claude_output(event, &ruling?)
}

/// What the `agent.permission_requested` hooks make of a prompt the rules
/// left to a person: a deny when one refused, `None` when every one let it
/// through, none is configured, or one could not be run.
///
/// The repository's hooks run only when its automation is trusted, as every
/// other hook point's do; the store is read only when there is a `.ket.toml`
/// to trust.
fn hook_ruling(
    payload: &serde_json::Value,
    tool: &str,
    project: Option<&str>,
    root: Option<&Path>,
) -> Option<Ruling> {
    use crate::config::{Config, HookConfig};
    use crate::hook::{self, HookPoint, ProcessHookSpawner};

    let config = Config::load().ok()?;
    let repo = match (project, root) {
        (Some(project), Some(root)) if root.join(".ket.toml").is_file() => {
            let trusted = crate::store::Store::open_default()
                .ok()?
                .load()
                .ok()?
                .trusted_automation
                .contains(&crate::id::ProjectId::new(project));
            if trusted {
                HookConfig::for_repo(root).unwrap_or_default()
            } else {
                HookConfig::default()
            }
        }
        _ => HookConfig::default(),
    };
    if repo.agent_permission_requested.is_empty()
        && config.hooks.agent_permission_requested.is_empty()
    {
        return None;
    }

    let null = serde_json::Value::Null;
    let field = |name: &str| payload.get(name).unwrap_or(&null);
    let context = serde_json::json!({
        "sessionId": field("session_id"),
        "worktreeId": std::env::var(crate::agent_hooks::WORKTREE_ENV).ok().filter(|id| !id.is_empty()),
        "projectId": project,
        "agent": "claude",
        "tool": tool,
        "input": field("tool_input"),
        "cwd": field("cwd"),
    });
    let blocked = hook::check(
        HookPoint::AgentPermissionRequested,
        &repo,
        &config.hooks,
        &context,
        &ProcessHookSpawner,
    )
    .ok()??;
    Some(hook_refusal(&blocked))
}

/// The deny a refusing hook gives: the first line it printed as the reason,
/// cut to [`HOOK_REASON_MAX`] characters, or its command when it printed
/// nothing.
fn hook_refusal(blocked: &crate::hook::Blocked) -> Ruling {
    let said = blocked
        .exit
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(HOOK_REASON_MAX).collect());
    Ruling {
        verdict: Verdict::Deny,
        scope: Scope::Hook,
        index: blocked.index,
        reason: Some(said.unwrap_or_else(|| format!("blocked by hook {}", blocked.command))),
    }
}

/// Runs as the permission hook and exits, if this process was started as one.
///
/// Call it first thing in `main`, beside [`crate::filter::serve_if_asked`].
/// Returns only when this process is not the hook. Prints nothing on any
/// failure, which the script takes as "leave it to the agent".
pub fn serve_if_asked() {
    if std::env::args().nth(1).as_deref() != Some(HOOK_ARG) {
        return;
    }
    use std::io::Read;
    let mut payload = String::new();
    let read = std::io::stdin()
        .take(MAX_STDIN as u64)
        .read_to_string(&mut payload)
        .is_ok();
    let agent = std::env::var("KET_AGENT_NAME").unwrap_or_else(|_| "claude".to_owned());
    if read
        && payload.len() < MAX_STDIN
        && let Some(answer) = serde_json::from_str(&payload)
            .ok()
            .as_ref()
            .and_then(|payload| hook_answer(payload, &agent))
    {
        println!("{answer}");
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests;
