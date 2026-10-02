//! Every action ket can perform, as data.
//!
//! One idea underpins Epic 10, and it is the only part of it that cannot be
//! deferred: every action is a *named command in a registry here in core*.
//! Keybindings map keys to command ids, the palette lists commands, the CLI
//! dispatches commands, and plugins register new ones. Built once, all four fall
//! out of it. Built four times — once per surface — the first divergence between
//! them is a rewrite rather than a refactor, which is why this module lands
//! before the shell's palette rather than alongside it.
//!
//! Two things are deliberately separate here:
//!
//! - **A [`Command`] is data**: id, title, category, arguments. It is
//!   serialisable so a palette can render it, and it carries no code at all.
//! - **A [`Handler`] belongs to the client.** The CLI prints a diff to a
//!   terminal; the shell will open a pane showing one. `worktree.diff` means
//!   something different to each, and neither meaning belongs in `ket-core`
//!   (invariant 3). The [`Registry`] is where a client hangs its
//!   implementations, and [`Registry::unbound`] is how it proves it supplied one
//!   for everything.
//!
//! *Applicable* and *invocable* are also separate, and that distinction is the
//! whole of the context model. A palette can only **offer** `worktree.diff` when
//! it already knows which worktree; the CLI names one on the command line and
//! needs no context whatsoever. So [`Command::applies_in`] answers "can this be
//! offered given nothing but the current selection", while [`Registry::resolve`]
//! answers "are this command's arguments satisfied by what was passed *plus*
//! what is selected". Filtering a palette with the first is what stops a command
//! from being offered and then failing when someone picks it — the failure this
//! module exists to prevent.
//!
//! One rule decides when a CLI flag earns its own command: a flag that changes
//! *what happens* becomes a separate command (`ket ps --reap` is `session.reap`,
//! not `session.list` with a switch), and a flag that changes how the same thing
//! is rendered stays an argument (`ket diff --stat`). A palette offers actions,
//! and "list sessions" and "delete finished session records" are not one action
//! with a checkbox on it.
//!
//! Command-layer failures surface as [`KetError::Conflict`]: an unknown id, a
//! command that does not apply here, a missing argument. Nothing has gone wrong
//! in the sense the other variants describe — the request is simply not one ket
//! will carry out.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{ProjectId, SessionId, WorktreeId};
use crate::{KetError, Result};

/// Most commands one registry will hold.
///
/// The built-in set is a few dozen and will stay that way; the cap is for the
/// plugin path, where a registry grows from code ket did not write. Registration
/// past it fails loudly rather than dropping the command, because a palette
/// quietly missing an entry with no error anywhere is the worst version of this.
pub const MAX_COMMANDS: usize = 1024;

/// Identifies a command: namespaced, lowercase, and stable forever.
///
/// Stable is the load-bearing word. These ids are what a user's
/// `keybindings.toml` refers to and what a plugin invokes, so renaming one
/// silently breaks a file ket does not own. An id is public interface in a way a
/// [`Command::title`] is not — titles are free to be reworded at any time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandId(String);

impl CommandId {
    /// Wraps an already-formed id. Validated by [`Registry::register`].
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part before the first `.`, e.g. `worktree` for `worktree.create`.
    ///
    /// Worth naming rather than leaving every caller to split on a dot: the
    /// namespace is what a plugin claims and what a keybinding file groups by.
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    /// Whether this id is shaped like one ket will accept.
    ///
    /// At least two dot-separated segments, each starting with a lowercase
    /// letter and holding only `[a-z0-9_]`. Enforced rather than merely
    /// suggested, because an unnamespaced `run` registered by a plugin collides
    /// with core's own vocabulary the first time core grows a command by that
    /// name — and the collision would land on a user's keybinding file.
    pub fn is_well_formed(&self) -> bool {
        let mut segments = 0usize;

        for segment in self.0.split('.') {
            segments += 1;

            let mut chars = segment.chars();
            if !chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
                return false;
            }
            if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return false;
            }
        }

        segments >= 2
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: the latter silently ignores width, which turns
        // every `{:<20}` in a command listing into no padding at all.
        f.pad(&self.0)
    }
}

impl From<&str> for CommandId {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

impl From<String> for CommandId {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

impl Borrow<str> for CommandId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// How a command is grouped when it is shown to a person.
///
/// Deliberately *not* derived from the id's namespace. `worktree.diff` and
/// `worktree.collapse` both act on a worktree, but they belong next to each
/// other under review, which is where someone comparing three attempts goes
/// looking for them; grouping by namespace would scatter the two of them through
/// a dozen lifecycle commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// ket itself: diagnostics, this command list, the event stream.
    Application,
    /// Reading and locating configuration.
    Config,
    /// Registering and choosing projects.
    Project,
    /// Notes on work not started yet, kept until one becomes a worktree.
    Backlog,
    /// Worktree lifecycle: create, list, provision, remove.
    Worktree,
    /// Reading what agents produced, and keeping the best of it.
    Review,
    /// Running agents, and the sessions that result.
    Agent,
}

impl Category {
    /// Every category, in the order a listing should present them.
    pub const ALL: [Category; 7] = [
        Category::Application,
        Category::Config,
        Category::Project,
        Category::Backlog,
        Category::Worktree,
        Category::Review,
        Category::Agent,
    ];

    /// The lowercase name, as it appears on the wire and in `ket commands`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Application => "application",
            Category::Config => "config",
            Category::Project => "project",
            Category::Backlog => "backlog",
            Category::Worktree => "worktree",
            Category::Review => "review",
            Category::Agent => "agent",
        }
    }

    /// Parses a category name, ignoring case and surrounding space.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        Category::ALL.into_iter().find(|c| c.as_str() == name)
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// What kind of value an argument carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArgKind {
    /// Free text: a branch name, a prompt, an agent name.
    Text,
    /// A filesystem path.
    Path,
    /// A switch. Absent means false, which is why a switch is never required.
    Flag,
    /// A project id or name.
    Project,
    /// A worktree id.
    Worktree,
    /// An agent session id.
    Session,
}

impl ArgKind {
    /// Whether a [`Context`] can supply a value of this kind.
    ///
    /// True only for the three kinds that name something ket has selected. A
    /// prompt cannot be inferred from where the cursor is, which is exactly why
    /// a required *text* argument never makes a command inapplicable: the client
    /// asks for it, and asking is not failing.
    pub fn is_selectable(&self) -> bool {
        matches!(
            self,
            ArgKind::Project | ArgKind::Worktree | ArgKind::Session
        )
    }

    /// The lowercase name, as it appears on the wire.
    pub fn as_str(&self) -> &'static str {
        match self {
            ArgKind::Text => "text",
            ArgKind::Path => "path",
            ArgKind::Flag => "flag",
            ArgKind::Project => "project",
            ArgKind::Worktree => "worktree",
            ArgKind::Session => "session",
        }
    }
}

impl fmt::Display for ArgKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// One argument a command accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arg {
    /// Name, in `snake_case`. This is the key used in an [`Invocation`].
    pub name: String,
    /// What it carries.
    pub kind: ArgKind,
    /// Whether the command cannot run without it.
    pub required: bool,
}

impl Arg {
    /// An argument the command cannot run without.
    pub fn required(name: impl Into<String>, kind: ArgKind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: true,
        }
    }

    /// An argument the command has a sensible answer for when it is absent.
    pub fn optional(name: impl Into<String>, kind: ArgKind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: false,
        }
    }

    /// A switch. Never required: absence is the `false` case.
    pub fn flag(name: impl Into<String>) -> Self {
        Self::optional(name, ArgKind::Flag)
    }
}

/// A named action, and everything a client needs in order to offer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    /// Stable, namespaced identifier.
    pub id: CommandId,
    /// One imperative line, as it appears in a palette.
    pub title: String,
    /// How it is grouped for a person.
    pub category: Category,
    /// Arguments, in the order a client should ask for them.
    pub args: Vec<Arg>,
    /// Panes this command is offered in. Empty means every pane.
    ///
    /// Opaque to core, which never interprets a pane name beyond comparing it —
    /// a pane taxonomy in `ket-core` would be exactly the UI knowledge invariant
    /// 3 keeps out, the same reason `ProjectState::layout` is stored verbatim.
    /// Empty for every built-in, since all of them predate the shell; the M1
    /// shell's own commands ("scroll the terminal", "focus the diff") are what
    /// this exists for.
    pub panes: Vec<String>,
}

impl Command {
    /// A command with no arguments and no pane restriction.
    pub fn new(id: impl Into<CommandId>, title: impl Into<String>, category: Category) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            category,
            args: Vec::new(),
            panes: Vec::new(),
        }
    }

    /// Adds an argument.
    pub fn arg(mut self, arg: Arg) -> Self {
        self.args.push(arg);
        self
    }

    /// Restricts the command to the named panes.
    pub fn in_panes(mut self, panes: &[&str]) -> Self {
        self.panes = panes.iter().map(|pane| (*pane).to_owned()).collect();
        self
    }

    /// Looks up a declared argument by name.
    pub fn arg_named(&self, name: &str) -> Option<&Arg> {
        self.args.iter().find(|arg| arg.name == name)
    }

    /// Whether a client can *offer* this command given only what is selected.
    ///
    /// Intentionally stricter than what [`Registry::resolve`] will accept: a
    /// command whose required worktree is named explicitly is perfectly
    /// invocable with an empty context, but it is not something to put in a menu
    /// when there is nothing for it to act on.
    pub fn applies_in(&self, context: &Context) -> bool {
        self.pane_allows(context)
            && self
                .args
                .iter()
                .all(|arg| !arg.required || !arg.kind.is_selectable() || context.supplies(arg.kind))
    }

    /// Whether `query` appears in the id or the title, ignoring case.
    ///
    /// Substring, not fuzzy, and unranked: ordering a palette's results depends
    /// on recency and on what was typed last, which are things the client knows
    /// and core does not.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        self.id.as_str().contains(&query) || self.title.to_ascii_lowercase().contains(&query)
    }

    /// The arguments rendered as a usage line, e.g. `<branch> [base] [--stat]`.
    ///
    /// Empty when the command takes none, so a caller can skip the line rather
    /// than print a stray pair of empty brackets.
    pub fn signature(&self) -> String {
        self.args
            .iter()
            .map(|arg| match (arg.kind, arg.required) {
                (ArgKind::Flag, _) => format!("[--{}]", arg.name),
                (_, true) => format!("<{}>", arg.name),
                (_, false) => format!("[{}]", arg.name),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether the focused pane permits this command.
    ///
    /// A context with no pane at all — every client until the shell exists —
    /// satisfies only commands that declare no restriction. That is the right
    /// answer for the CLI: "scroll the terminal pane up" has no meaning there.
    fn pane_allows(&self, context: &Context) -> bool {
        if self.panes.is_empty() {
            return true;
        }

        context
            .pane
            .as_deref()
            .is_some_and(|pane| self.panes.iter().any(|allowed| allowed == pane))
    }
}

/// What is selected at the moment a command is offered or invoked.
///
/// The three ids are what a [`Command`] can have filled in for it; the pane is
/// what decides whether it is offered at all. Every field is optional, and an
/// empty context is an ordinary thing — it is what a CLI process started outside
/// any project has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// The project in view.
    pub project: Option<ProjectId>,
    /// The worktree in view.
    ///
    /// A worktree implies a project, but core does not enforce that here: a
    /// client that knows the worktree and not the project should get its
    /// worktree commands rather than an argument about consistency.
    pub worktree: Option<WorktreeId>,
    /// The agent session in view.
    pub session: Option<SessionId>,
    /// The focused pane, named by the client and never interpreted by core.
    pub pane: Option<String>,
}

impl Context {
    /// The value this context can supply for `kind`, if any.
    pub fn value_for(&self, kind: ArgKind) -> Option<String> {
        match kind {
            ArgKind::Project => self.project.as_ref().map(ProjectId::to_string),
            ArgKind::Worktree => self.worktree.as_ref().map(WorktreeId::to_string),
            ArgKind::Session => self.session.as_ref().map(SessionId::to_string),
            ArgKind::Text | ArgKind::Path | ArgKind::Flag => None,
        }
    }

    /// Whether this context can fill an argument of `kind`.
    pub fn supplies(&self, kind: ArgKind) -> bool {
        match kind {
            ArgKind::Project => self.project.is_some(),
            ArgKind::Worktree => self.worktree.is_some(),
            ArgKind::Session => self.session.is_some(),
            ArgKind::Text | ArgKind::Path | ArgKind::Flag => false,
        }
    }
}

/// A value passed for one argument.
///
/// Untagged on the wire: a text argument is a JSON string and a switch is a JSON
/// boolean, which is what anyone hand-writing an invocation would type anyway.
/// The two shapes cannot be mistaken for each other, so a tag would be noise in
/// every message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgValue {
    /// A switch that was set.
    Flag(bool),
    /// Text, including ids and paths.
    Text(String),
}

impl ArgValue {
    /// Whether this value is the shape `kind` declares.
    pub fn fits(&self, kind: ArgKind) -> bool {
        match (self, kind) {
            (ArgValue::Flag(_), ArgKind::Flag) => true,
            (ArgValue::Flag(_), _) => false,
            (ArgValue::Text(_), ArgKind::Flag) => false,
            (ArgValue::Text(_), _) => true,
        }
    }
}

/// A command, plus the arguments it was invoked with.
///
/// The map is bounded by the command it names: [`Registry::resolve`] rejects any
/// name the command does not declare, so an invocation cannot grow past that
/// command's own argument list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invocation {
    /// Which command.
    pub id: CommandId,
    /// Arguments by name.
    pub args: BTreeMap<String, ArgValue>,
}

impl Invocation {
    /// An invocation of `id` with no arguments yet.
    pub fn new(id: impl Into<CommandId>) -> Self {
        Self {
            id: id.into(),
            args: BTreeMap::new(),
        }
    }

    /// Sets a text argument.
    pub fn text(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.args.insert(name.into(), ArgValue::Text(value.into()));
        self
    }

    /// Sets a text argument only when there is one.
    ///
    /// The absent case is what lets the context fill a required argument: an
    /// option nobody passed has to leave a hole rather than an empty string, or
    /// [`Registry::resolve`] has nothing to fill.
    pub fn maybe_text(self, name: impl Into<String>, value: Option<impl Into<String>>) -> Self {
        match value {
            Some(value) => self.text(name, value),
            None => self,
        }
    }

    /// Sets a switch. A `false` switch is left out, since absence is false.
    pub fn flag(mut self, name: impl Into<String>, value: bool) -> Self {
        if value {
            self.args.insert(name.into(), ArgValue::Flag(true));
        }
        self
    }

    /// The text of `name`, or `None` if it was not passed or is a switch.
    pub fn text_of(&self, name: &str) -> Option<&str> {
        match self.args.get(name) {
            Some(ArgValue::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// The text of `name`, which [`Registry::resolve`] guarantees is present for
    /// a required argument.
    pub fn require_text(&self, name: &str) -> Result<&str> {
        self.text_of(name)
            .ok_or_else(|| KetError::Conflict(format!("{} needs {name}", self.id)))
    }

    /// Whether the switch `name` was set.
    pub fn is_set(&self, name: &str) -> bool {
        matches!(self.args.get(name), Some(ArgValue::Flag(true)))
    }
}

/// What a client does when a command is invoked.
///
/// `Send + Sync` because the shell holds one registry and dispatches from
/// whichever thread the key press or the IPC message arrived on.
pub type Handler = Box<dyn Fn(&Invocation, &Context) -> Result<()> + Send + Sync>;

/// One command, and whatever a client bound to it.
struct Entry {
    command: Command,
    handler: Option<Handler>,
}

/// Every command ket can perform, and the client's implementations of them.
///
/// Core owns the catalogue ([`Registry::builtin`]); a client owns the handlers.
/// That split is why one registry serves both a CLI that prints text and a shell
/// that opens panes, without core knowing that either exists.
pub struct Registry {
    commands: BTreeMap<CommandId, Entry>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            commands: BTreeMap::new(),
        }
    }

    /// Every command ket ships, with nothing bound to them yet.
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        for command in builtin_commands() {
            // A malformed or duplicated built-in is a mistake in this file
            // rather than a runtime condition, and `every_builtin_is_registered`
            // is what turns it into a test failure instead of this panic.
            registry
                .register(command)
                .expect("built-in commands are well formed");
        }
        registry
    }

    /// Adds a command.
    ///
    /// Rejects an id that is not namespaced, an id already taken, an unnamed or
    /// duplicated argument, and a required switch. Each is a way a plugin could
    /// otherwise make the palette quietly wrong, and every one of them is far
    /// cheaper to catch here than to explain later.
    pub fn register(&mut self, command: Command) -> Result<()> {
        if !command.id.is_well_formed() {
            return Err(KetError::Conflict(format!(
                "{} is not a valid command id; ids are lowercase and namespaced, like worktree.create",
                command.id
            )));
        }

        if self.commands.contains_key(command.id.as_str()) {
            return Err(KetError::Conflict(format!(
                "{} is already registered",
                command.id
            )));
        }

        if self.commands.len() >= MAX_COMMANDS {
            return Err(KetError::Conflict(format!(
                "the command registry holds {MAX_COMMANDS} commands; {} does not fit",
                command.id
            )));
        }

        for (index, arg) in command.args.iter().enumerate() {
            if arg.name.is_empty() {
                return Err(KetError::Conflict(format!(
                    "{} has an unnamed argument",
                    command.id
                )));
            }
            if command.args[..index].iter().any(|a| a.name == arg.name) {
                return Err(KetError::Conflict(format!(
                    "{} declares {} twice",
                    command.id, arg.name
                )));
            }
            if arg.kind == ArgKind::Flag && arg.required {
                return Err(KetError::Conflict(format!(
                    "{}: {} is a required switch, which nothing could ever satisfy",
                    command.id, arg.name
                )));
            }
        }

        self.commands.insert(
            command.id.clone(),
            Entry {
                command,
                handler: None,
            },
        );
        Ok(())
    }

    /// Gives a registered command an implementation.
    ///
    /// Binding an unknown id fails, which is what turns a typo in a client's
    /// bind list into a test failure rather than a command that silently never
    /// runs. Binding twice fails too: a second implementation is either a bug or
    /// two plugins fighting over one id, and quietly taking the last one is how
    /// that becomes impossible to explain.
    pub fn bind(
        &mut self,
        id: &str,
        handler: impl Fn(&Invocation, &Context) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        let entry = self
            .commands
            .get_mut(id)
            .ok_or_else(|| KetError::Conflict(format!("unknown command: {id}")))?;

        if entry.handler.is_some() {
            return Err(KetError::Conflict(format!(
                "{id} already has an implementation"
            )));
        }

        entry.handler = Some(Box::new(handler));
        Ok(())
    }

    /// Looks up a command.
    pub fn get(&self, id: &str) -> Option<&Command> {
        self.commands.get(id).map(|entry| &entry.command)
    }

    /// Every command, in id order.
    pub fn list(&self) -> impl Iterator<Item = &Command> {
        self.commands.values().map(|entry| &entry.command)
    }

    /// How many commands are registered.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// The commands a client can offer given `context`.
    pub fn applicable(&self, context: &Context) -> Vec<&Command> {
        self.list()
            .filter(|command| command.applies_in(context))
            .collect()
    }

    /// The commands in one category, in id order.
    pub fn in_category(&self, category: Category) -> Vec<&Command> {
        self.list()
            .filter(|command| command.category == category)
            .collect()
    }

    /// Commands with no implementation bound.
    ///
    /// This is the single-source-of-truth check, and it is meant to be asserted
    /// empty by a client's own tests: it is what proves the client dispatches
    /// *through* the registry rather than keeping a second, divergent list of
    /// the things it can do.
    pub fn unbound(&self) -> Vec<&CommandId> {
        self.commands
            .values()
            .filter(|entry| entry.handler.is_none())
            .map(|entry| &entry.command.id)
            .collect()
    }

    /// Checks an invocation and fills in whatever the context can supply.
    ///
    /// Only *required* arguments are filled from the context. Filling an
    /// optional one would silently change what the command does: `worktree.list`
    /// with no project means every project, and quietly narrowing it to the
    /// selected one is a different command wearing the same name.
    pub fn resolve(&self, invocation: &Invocation, context: &Context) -> Result<Invocation> {
        let command = self
            .get(invocation.id.as_str())
            .ok_or_else(|| KetError::Conflict(format!("unknown command: {}", invocation.id)))?;

        if !command.pane_allows(context) {
            return Err(KetError::Conflict(format!(
                "{} is only available in: {}",
                command.id,
                command.panes.join(", ")
            )));
        }

        for (name, value) in &invocation.args {
            let Some(arg) = command.arg_named(name) else {
                return Err(KetError::Conflict(format!(
                    "{} has no argument {name}",
                    command.id
                )));
            };

            if !value.fits(arg.kind) {
                return Err(KetError::Conflict(format!(
                    "{}: {name} is a {}",
                    command.id, arg.kind
                )));
            }
        }

        let mut resolved = invocation.clone();

        for arg in &command.args {
            if !arg.required || resolved.args.contains_key(&arg.name) {
                continue;
            }

            // Two messages, because the two failures have different answers. A
            // selectable argument can be named *or* selected, and saying only
            // "needs worktree" leaves out half of how to satisfy it.
            let value = context.value_for(arg.kind).ok_or_else(|| {
                if arg.kind.is_selectable() {
                    KetError::Conflict(format!(
                        "{} needs a {}: none was named and none is selected",
                        command.id, arg.name
                    ))
                } else {
                    KetError::Conflict(format!("{} needs {}", command.id, arg.name))
                }
            })?;

            resolved
                .args
                .insert(arg.name.clone(), ArgValue::Text(value));
        }

        Ok(resolved)
    }

    /// Resolves an invocation and runs whatever the client bound to it.
    ///
    /// The handler sees the *resolved* invocation, so it never has to think
    /// about where a required argument came from.
    pub fn dispatch(&self, invocation: &Invocation, context: &Context) -> Result<()> {
        let resolved = self.resolve(invocation, context)?;

        let handler = self
            .commands
            .get(resolved.id.as_str())
            .and_then(|entry| entry.handler.as_ref())
            .ok_or_else(|| {
                KetError::Conflict(format!("{} has no implementation here", resolved.id))
            })?;

        handler(&resolved, context)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Handlers are closures and cannot be shown; the counts are what anyone
        // debugging a registry is actually asking about.
        f.debug_struct("Registry")
            .field("commands", &self.commands.len())
            .field("unbound", &self.unbound().len())
            .finish()
    }
}

/// Every command ket ships.
///
/// This list *is* ket's feature set. A capability that is not here cannot be
/// bound to a key, cannot appear in the palette, and — because the CLI
/// dispatches through the registry — cannot be reached from a terminal either.
fn builtin_commands() -> Vec<Command> {
    vec![
        Command::new(
            "app.commands",
            "List every command ket can perform",
            Category::Application,
        )
        .arg(Arg::flag("applicable"))
        .arg(Arg::optional("category", ArgKind::Text)),
        Command::new(
            "app.doctor",
            "Check that everything ket depends on is present",
            Category::Application,
        ),
        Command::new(
            "config.path",
            "Show where ket reads configuration from",
            Category::Config,
        ),
        Command::new(
            "config.show",
            "Show the resolved configuration",
            Category::Config,
        ),
        // Two commands rather than one with a switch: `--follow` never returns,
        // and "print the journal" and "watch what happens next" are different
        // things to offer someone.
        Command::new(
            "event.follow",
            "Follow ket's event stream until interrupted",
            Category::Application,
        ),
        Command::new(
            "event.list",
            "Print the event journal as JSON lines",
            Category::Application,
        ),
        Command::new(
            "agent.run",
            "Run an agent against a worktree",
            Category::Agent,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree))
        .arg(Arg::required("prompt", ArgKind::Text))
        .arg(Arg::optional("agent", ArgKind::Text)),
        Command::new(
            "project.add",
            "Register the repository containing a path",
            Category::Project,
        )
        .arg(Arg::optional("path", ArgKind::Path)),
        Command::new(
            "project.list",
            "List registered projects",
            Category::Project,
        ),
        Command::new(
            "project.remove",
            "Deregister a project, leaving the repository alone",
            Category::Project,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::flag("force")),
        // One command with optional fields rather than one per field. A
        // settings dialog saves several at once, and five commands that each
        // reopen the store to change one string is five chances to leave the
        // project half-updated.
        Command::new(
            "project.settings.set",
            "Change a project's display, defaults or automation trust",
            Category::Project,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::optional("name", ArgKind::Text))
        .arg(Arg::optional("color", ArgKind::Text))
        .arg(Arg::optional("icon", ArgKind::Text))
        .arg(Arg::optional("base", ArgKind::Text))
        .arg(Arg::optional("agent", ArgKind::Text))
        .arg(Arg::optional("discovered", ArgKind::Text))
        .arg(Arg::optional("automation", ArgKind::Text)),
        Command::new(
            "backlog.add",
            "Add a note to a project's backlog",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("title", ArgKind::Text))
        .arg(Arg::optional("body", ArgKind::Text))
        // Several tags travel as one text argument, a line each: an argument
        // is text or a switch, and a tag never holds a line break.
        .arg(Arg::optional("tags", ArgKind::Text)),
        Command::new(
            "backlog.list",
            "List a project's backlog notes",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::flag("all"))
        .arg(Arg::optional("tag", ArgKind::Text))
        .arg(Arg::optional("search", ArgKind::Text)),
        Command::new(
            "backlog.done",
            "Mark a backlog note done by hand",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("note", ArgKind::Text)),
        Command::new(
            "backlog.reopen",
            "Put a done backlog note back among the open ones",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("note", ArgKind::Text)),
        Command::new(
            "backlog.remove",
            "Delete a backlog note and its files",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("note", ArgKind::Text)),
        Command::new(
            "backlog.tag",
            "Add tags to a backlog note, or take them off",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("note", ArgKind::Text))
        .arg(Arg::required("tags", ArgKind::Text))
        .arg(Arg::flag("remove")),
        Command::new(
            "backlog.move",
            "Move a backlog note before another, or to the end of its priority",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("note", ArgKind::Text))
        .arg(Arg::optional("before", ArgKind::Text)),
        Command::new(
            "backlog.store",
            "Keep a project's backlog privately or in its repository",
            Category::Backlog,
        )
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::required("location", ArgKind::Text)),
        Command::new("session.list", "List agent sessions", Category::Agent),
        Command::new(
            "session.reap",
            "Drop finished and abandoned session records",
            Category::Agent,
        ),
        Command::new(
            "worktree.close",
            "Close a worktree, leaving it on disk",
            Category::Worktree,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree)),
        Command::new(
            "worktree.collapse",
            "Merge one attempt into its base and discard the rest",
            Category::Review,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree))
        .arg(Arg::flag("force"))
        .arg(Arg::flag("keep"))
        .arg(Arg::flag("keep_losers")),
        Command::new(
            "worktree.merge",
            "Commit a worktree's work and merge it into its base",
            Category::Review,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree))
        .arg(Arg::flag("force")),
        Command::new(
            "worktree.create",
            "Create a worktree on a new branch",
            Category::Worktree,
        )
        .arg(Arg::required("branch", ArgKind::Text))
        .arg(Arg::required("project", ArgKind::Project))
        .arg(Arg::optional("base", ArgKind::Text))
        .arg(Arg::flag("no_provision")),
        Command::new(
            "worktree.diff",
            "Show what a worktree changed against its base",
            Category::Review,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree))
        .arg(Arg::flag("stat")),
        Command::new("worktree.list", "List worktrees", Category::Worktree)
            .arg(Arg::optional("project", ArgKind::Project)),
        Command::new(
            "worktree.open",
            "Open a worktree and give it focus",
            Category::Worktree,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree)),
        Command::new(
            "worktree.provision",
            "Re-run provisioning for a worktree",
            Category::Worktree,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree)),
        Command::new(
            "worktree.prune",
            "Drop registry entries whose directories are gone",
            Category::Worktree,
        ),
        Command::new("worktree.remove", "Remove a worktree", Category::Worktree)
            .arg(Arg::required("worktree", ArgKind::Worktree))
            .arg(Arg::flag("force"))
            .arg(Arg::flag("delete_branch")),
        Command::new(
            "worktree.status",
            "Show a worktree's changed files and how far it has diverged",
            Category::Worktree,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree_selected() -> Context {
        Context {
            worktree: Some(WorktreeId::new("wt-1")),
            ..Context::default()
        }
    }

    #[test]
    fn every_builtin_is_registered() {
        // `builtin` expects registration to succeed; this is what turns a
        // malformed id or a duplicated argument into a failure here rather than
        // a panic in whichever binary happened to start first.
        let registry = Registry::builtin();
        assert_eq!(registry.len(), builtin_commands().len());
        assert!(registry.list().all(|command| command.id.is_well_formed()));
    }

    #[test]
    fn the_wire_format_is_flat_camel_case() {
        // Pinned: a palette renders exactly this shape, and it crosses the IPC
        // boundary in M1.
        let command = Command::new(
            "worktree.diff",
            "Show what a worktree changed",
            Category::Review,
        )
        .arg(Arg::required("worktree", ArgKind::Worktree))
        .arg(Arg::flag("stat"));

        assert_eq!(
            serde_json::to_value(&command).unwrap(),
            serde_json::json!({
                "id": "worktree.diff",
                "title": "Show what a worktree changed",
                "category": "review",
                "args": [
                    { "name": "worktree", "kind": "worktree", "required": true },
                    { "name": "stat", "kind": "flag", "required": false },
                ],
                "panes": [],
            })
        );
    }

    #[test]
    fn an_invocations_arguments_are_bare_json_values() {
        let invocation = Invocation::new("worktree.diff")
            .text("worktree", "wt-1")
            .flag("stat", true);

        assert_eq!(
            serde_json::to_value(&invocation).unwrap(),
            serde_json::json!({
                "id": "worktree.diff",
                "args": { "worktree": "wt-1", "stat": true },
            })
        );
    }

    #[test]
    fn an_id_without_a_namespace_is_rejected() {
        let mut registry = Registry::new();
        let result = registry.register(Command::new("run", "Run", Category::Agent));
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn an_id_with_capitals_is_rejected() {
        let mut registry = Registry::new();
        let result = registry.register(Command::new("Worktree.Create", "x", Category::Worktree));
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn a_plugin_may_namespace_more_deeply() {
        let mut registry = Registry::new();
        registry
            .register(Command::new(
                "acme.deploy.staging",
                "Deploy to staging",
                Category::Agent,
            ))
            .unwrap();
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registering_the_same_id_twice_fails() {
        let mut registry = Registry::builtin();
        let result = registry.register(Command::new(
            "worktree.create",
            "Something else entirely",
            Category::Worktree,
        ));
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn a_required_switch_is_rejected() {
        let mut registry = Registry::new();
        let result = registry.register(
            Command::new("test.thing", "Thing", Category::Application)
                .arg(Arg::required("force", ArgKind::Flag)),
        );
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn the_registry_refuses_to_grow_past_its_cap() {
        let mut registry = Registry::new();
        for n in 0..MAX_COMMANDS {
            registry
                .register(Command::new(
                    format!("plugin.c{n}"),
                    "Generated",
                    Category::Application,
                ))
                .unwrap();
        }

        let result = registry.register(Command::new(
            "plugin.last",
            "One too many",
            Category::Application,
        ));
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn a_command_needing_a_worktree_is_not_offered_without_one() {
        let registry = Registry::builtin();
        let diff = registry.get("worktree.diff").unwrap();

        assert!(!diff.applies_in(&Context::default()));
        assert!(diff.applies_in(&worktree_selected()));
    }

    #[test]
    fn a_required_text_argument_does_not_make_a_command_inapplicable() {
        // `agent.run` needs a prompt, and no selection could ever supply one. A
        // client asks for it; that is not a reason to hide the command once a
        // worktree is in view.
        let registry = Registry::builtin();
        let run = registry.get("agent.run").unwrap();

        assert!(!run.applies_in(&Context::default()));
        assert!(run.applies_in(&worktree_selected()));
    }

    #[test]
    fn a_command_that_cannot_be_offered_is_still_invocable_when_named() {
        // The distinction the whole context model exists for: `ket diff wt-1`
        // works from anywhere, even though a palette would not offer it.
        let registry = Registry::builtin();
        let invocation = Invocation::new("worktree.diff").text("worktree", "wt-1");

        let resolved = registry.resolve(&invocation, &Context::default()).unwrap();
        assert_eq!(resolved.text_of("worktree"), Some("wt-1"));
    }

    #[test]
    fn a_required_argument_is_filled_from_the_context() {
        let registry = Registry::builtin();
        let resolved = registry
            .resolve(&Invocation::new("worktree.status"), &worktree_selected())
            .unwrap();

        assert_eq!(resolved.text_of("worktree"), Some("wt-1"));
    }

    #[test]
    fn an_optional_argument_is_not_filled_from_the_context() {
        // `worktree.list` with no project means every project. Filling it from
        // the selection would silently turn it into a different command.
        let registry = Registry::builtin();
        let context = Context {
            project: Some(ProjectId::new("proj-a")),
            ..Context::default()
        };

        let resolved = registry
            .resolve(&Invocation::new("worktree.list"), &context)
            .unwrap();
        assert_eq!(resolved.text_of("project"), None);
    }

    #[test]
    fn a_required_argument_nothing_can_supply_is_an_error_not_a_panic() {
        let registry = Registry::builtin();
        let result = registry.resolve(&Invocation::new("worktree.status"), &Context::default());
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn an_argument_the_command_does_not_declare_is_rejected() {
        let registry = Registry::builtin();
        let invocation = Invocation::new("worktree.status")
            .text("worktree", "wt-1")
            .text("colour", "blue");

        let result = registry.resolve(&invocation, &Context::default());
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn a_switch_passed_where_text_is_declared_is_rejected() {
        let registry = Registry::builtin();
        let invocation = Invocation::new("worktree.status").flag("worktree", true);

        let result = registry.resolve(&invocation, &Context::default());
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn an_unset_switch_is_absent_rather_than_false() {
        let invocation = Invocation::new("worktree.diff").flag("stat", false);
        assert!(invocation.args.is_empty());
        assert!(!invocation.is_set("stat"));
    }

    #[test]
    fn a_pane_restricted_command_is_hidden_elsewhere() {
        let mut registry = Registry::new();
        registry
            .register(
                Command::new("terminal.clear", "Clear the terminal", Category::Agent)
                    .in_panes(&["terminal"]),
            )
            .unwrap();

        let command = registry.get("terminal.clear").unwrap();
        assert!(!command.applies_in(&Context::default()));
        assert!(!command.applies_in(&Context {
            pane: Some("diff".to_owned()),
            ..Context::default()
        }));
        assert!(command.applies_in(&Context {
            pane: Some("terminal".to_owned()),
            ..Context::default()
        }));
    }

    #[test]
    fn a_pane_restricted_command_cannot_be_invoked_from_elsewhere() {
        let mut registry = Registry::new();
        registry
            .register(
                Command::new("terminal.clear", "Clear the terminal", Category::Agent)
                    .in_panes(&["terminal"]),
            )
            .unwrap();

        let result = registry.resolve(&Invocation::new("terminal.clear"), &Context::default());
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn dispatch_runs_what_the_client_bound() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);

        let mut registry = Registry::builtin();
        registry
            .bind("worktree.status", move |invocation, _| {
                assert_eq!(invocation.text_of("worktree"), Some("wt-1"));
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();

        registry
            .dispatch(&Invocation::new("worktree.status"), &worktree_selected())
            .unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn binding_an_unknown_command_fails() {
        // This is what makes a typo in a client's bind list a test failure
        // instead of a command that silently never runs.
        let mut registry = Registry::builtin();
        let result = registry.bind("worktree.creat", |_, _| Ok(()));
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn binding_a_command_twice_fails() {
        let mut registry = Registry::builtin();
        registry.bind("worktree.prune", |_, _| Ok(())).unwrap();
        let result = registry.bind("worktree.prune", |_, _| Ok(()));
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn dispatching_something_nothing_implements_is_an_error_not_a_panic() {
        let registry = Registry::builtin();
        let result = registry.dispatch(&Invocation::new("worktree.prune"), &Context::default());
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn dispatching_an_unknown_command_is_an_error_not_a_panic() {
        let registry = Registry::builtin();
        let result = registry.dispatch(&Invocation::new("nope.nope"), &Context::default());
        assert!(matches!(result, Err(KetError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn unbound_shrinks_as_a_client_binds() {
        let mut registry = Registry::builtin();
        let before = registry.unbound().len();
        registry.bind("worktree.prune", |_, _| Ok(())).unwrap();
        assert_eq!(registry.unbound().len(), before - 1);
    }

    #[test]
    fn a_category_parses_case_insensitively_and_round_trips() {
        for category in Category::ALL {
            assert_eq!(Category::parse(category.as_str()), Some(category));
        }
        assert_eq!(Category::parse("  Worktree "), Some(Category::Worktree));
        assert_eq!(Category::parse("panes"), None);
    }

    #[test]
    fn review_commands_are_grouped_away_from_worktree_lifecycle() {
        // The reason `category` is a field rather than the id's namespace.
        let registry = Registry::builtin();
        let review: Vec<_> = registry
            .in_category(Category::Review)
            .iter()
            .map(|command| command.id.as_str().to_owned())
            .collect();

        assert_eq!(
            review,
            ["worktree.collapse", "worktree.diff", "worktree.merge"]
        );
    }

    #[test]
    fn a_signature_shows_which_arguments_are_needed() {
        let registry = Registry::builtin();
        assert_eq!(
            registry.get("worktree.create").unwrap().signature(),
            "<branch> <project> [base] [--no_provision]"
        );
        assert!(
            registry
                .get("worktree.prune")
                .unwrap()
                .signature()
                .is_empty()
        );
    }

    #[test]
    fn a_search_matches_the_id_and_the_title() {
        let registry = Registry::builtin();
        let diff = registry.get("worktree.diff").unwrap();

        assert!(diff.matches("diff"));
        assert!(diff.matches("CHANGED"));
        assert!(diff.matches(""));
        assert!(!diff.matches("keybinding"));
    }

    #[test]
    fn a_namespace_is_the_part_before_the_first_dot() {
        assert_eq!(CommandId::new("worktree.create").namespace(), "worktree");
        assert_eq!(CommandId::new("acme.deploy.staging").namespace(), "acme");
    }

    #[test]
    fn ids_honour_formatting_width() {
        // `ket commands` lays out columns with `{:<20}`; a `Display` that writes
        // the string directly ignores that, and the listing comes out ragged.
        assert_eq!(
            format!("[{:<12}]", CommandId::new("app.doctor")),
            "[app.doctor  ]"
        );
    }
}
