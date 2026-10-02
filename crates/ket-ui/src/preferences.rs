//! The global settings view — ket's own preferences, not a project's.
//!
//! Project settings live in [`crate::projects`] and are about *one repository*:
//! its name, its colour, which agent it prefers. This is the other thing people
//! mean by settings — how the application itself behaves — and it is reached
//! from anywhere with the platform's own chord rather than from a row in the
//! sidebar.
//!
//! Laid out the way editors have settled on: a rail of sections down the left,
//! a filter across the top, and rows of one setting each on the right. The
//! shape matters more than the novelty here. Someone opening this has opened
//! VS Code's before, and a settings screen is not where a tool should be
//! teaching a new idiom.
//!
//! The Agents section is backed by the durable core configuration. The other
//! sections remain layout placeholders and say so in their footer rather than
//! pretending their controls have saved anything.

use gpui::{
    AnyElement, App, Context, Entity, KeyDownEvent, MouseButton, PathPromptOptions, Window, div,
    prelude::*, px, relative,
};
use ket_core::agents::{Catalogue, DefaultAgent, PermissionPreset, ShellProber};
use ket_core::config::{
    AgentLaunch, Config, DEFAULT_LINE_HEIGHT, FONT_SIZE_RANGE, LINE_HEIGHT_RANGE, OpenCodeGoConfig,
    UpdateCheck,
};
use ket_core::rate_limits::Provider;
use ket_core::theme::{Appearance, AppearancePreference, Theme};

use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::{alpha, paint};
use crate::ui::agent::{agent_label, mark, provider};
use crate::ui::button::button;
use crate::ui::chip::{badge, caption, kbd, tag, tinted_tag};
use crate::ui::dialog::{card, centered, header};
use crate::ui::group::{button_group, segment};
use crate::ui::icon::{Icon, icon, icon_well, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, OpenMenu, dropdown};
use crate::ui::row::heading_row;
use crate::ui::toggle::{checkbox, switch};
use crate::ui::{CAPTION, LABEL, RADIUS_LG, RADIUS_MD, RADIUS_SM, WELL_SM};
use crate::{EditorType, Shell};

/// How wide the whole thing is.
///
/// Wide enough that a setting's description does not wrap after four words,
/// which is what makes a settings screen feel like a form to fill in rather
/// than something to read.
const WIDTH: gpui::Pixels = px(1120.0);

/// How tall.
const HEIGHT: gpui::Pixels = px(760.0);

/// The most of the window the card may take.
///
/// [`WIDTH`] and [`HEIGHT`] are what it wants; this is what it settles for.
/// Both are larger than the default window's own 1200×780, so without a clamp
/// the card would run under the window's edges the moment anyone resized —
/// and a dialog whose close button is off-screen is a trap, not a dialog.
const MOST_OF_THE_WINDOW: f32 = 0.9;

/// The section rail's width: the search field above it and the rows.
const RAIL: gpui::Pixels = px(212.0);

/// A pane's title, in the sheet's header.
const PANE_TITLE: gpui::Pixels = px(18.0);

/// The well beside a pane's title.
const PANE_WELL: gpui::Pixels = px(36.0);

/// The section's glyph inside [`PANE_WELL`].
const PANE_GLYPH: gpui::Pixels = px(20.0);

/// The sheet's inset, either side of a pane.
const SHEET_PAD: gpui::Pixels = px(24.0);

/// One group of settings.
///
/// The rail's order is [`Section::GROUPS`], not this declaration — `Agents` is
/// drawn first there because it is the section with settings that actually do
/// something, and a rail whose first row is a placeholder teaches people that
/// the top of the list is not worth reading.
///
/// What the view *opens* on is fixed ([`Section::Agents`]) rather than
/// remembered: a settings screen that opens on whatever you last looked at
/// makes the same click do different things on different days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    /// Worktrees, updates, and the things that fit nowhere else.
    General,
    /// Which agents exist and how they are launched.
    Agents,
    /// Active and staged Economy policy packs.
    Economy,
    /// Saved prompt snippets.
    Snippets,
    /// Theme, fonts, density.
    Appearance,
    /// The tweak editor.
    Editor,
    /// The terminal panes.
    Terminal,
    /// What the sidebar's merge button does.
    Merge,
    /// Pairing phones with this Mac, and revoking them.
    Devices,
    /// Chords.
    Keybindings,
}

impl Section {
    /// Every section, in the order the rail shows them: [`Section::GROUPS`]
    /// run together, which is the order the arrow keys walk.
    pub(crate) const ALL: [Section; 10] = [
        Section::Agents,
        Section::Economy,
        Section::Snippets,
        Section::Merge,
        Section::General,
        Section::Appearance,
        Section::Editor,
        Section::Terminal,
        Section::Keybindings,
        Section::Devices,
    ];

    /// The rail's two groups, each under a caption: what ket does with
    /// agents, and how ket itself behaves on this Mac — which is where a
    /// paired device reaches it from.
    const GROUPS: [(&'static str, &'static [Section]); 2] = [
        (
            "Agents",
            &[
                Section::Agents,
                Section::Economy,
                Section::Snippets,
                Section::Merge,
            ],
        ),
        (
            "This Mac",
            &[
                Section::General,
                Section::Appearance,
                Section::Editor,
                Section::Terminal,
                Section::Keybindings,
                Section::Devices,
            ],
        ),
    ];

    /// What the rail calls it.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Editor => "Editor",
            Section::Terminal => "Terminal",
            Section::Merge => "Merge",
            Section::Devices => "Devices",
            Section::Agents => "Agents",
            Section::Economy => "Economy",
            Section::Snippets => "Snippets",
            Section::Keybindings => "Keybindings",
        }
    }

    /// The mark beside it.
    pub(crate) fn icon(self) -> Icon {
        match self {
            Section::General => Icon::AppMark,
            Section::Appearance => Icon::Swatch,
            Section::Editor => Icon::TextCursor,
            Section::Terminal => Icon::Prompt,
            Section::Merge => Icon::MergeInto,
            Section::Devices => Icon::LaptopPhone,
            Section::Agents => Icon::Orbit,
            Section::Economy => Icon::Gauge,
            Section::Snippets => Icon::MessageBookmark,
            Section::Keybindings => Icon::Command,
        }
    }

    /// The one line under the pane's title: what the section is for.
    fn description(self) -> &'static str {
        match self {
            Section::Agents => "Which agents ket can start, and how each one launches.",
            Section::Economy => {
                "Agents narrate less \u{2014} fewer recaps, less restating. Each session is \
                 priced against your own with Economy off."
            }
            Section::Snippets => "Prompts you keep, ready to send to any worktree\u{2019}s agent.",
            Section::Merge => {
                "What the sidebar\u{2019}s merge button commits, and what it leaves behind."
            }
            Section::General => "Worktrees, updates, and the things that fit nowhere else.",
            Section::Appearance => "Theme, fonts and density.",
            Section::Editor => "How ket\u{2019}s own editor draws and plays files.",
            Section::Terminal => "What a terminal pane runs, and how much it keeps.",
            Section::Keybindings => "The chords for ket\u{2019}s commands.",
            Section::Devices => {
                "Pair an iPhone or iPad to answer agents and watch terminals from it. Devices \
                 reach this desktop over Wi-Fi."
            }
        }
    }

    /// How the pane sits on the sheet. See [`Frame`].
    fn frame(self) -> Frame {
        match self {
            Section::Economy | Section::Devices => Frame::Bare,
            Section::Agents | Section::Snippets => Frame::Padded,
            Section::General
            | Section::Appearance
            | Section::Editor
            | Section::Terminal
            | Section::Merge
            | Section::Keybindings => Frame::Rows,
        }
    }

    /// The hue of the well beside the pane's title: the theme's own signal
    /// colours, so a theme that redraws them redraws these. The sections
    /// that are only about ket itself stay grey.
    fn hue(self, t: &Theme) -> gpui::Rgba {
        paint(match self {
            Section::Agents => t.quota.hot,
            Section::Economy => t.diff.added,
            Section::Snippets => t.diff.modified,
            Section::Merge => t.status.merging,
            Section::Appearance => t.terminal.ansi.magenta,
            Section::Editor => t.terminal.ansi.cyan,
            Section::Devices => t.status.attention,
            Section::General | Section::Terminal | Section::Keybindings => t.text.dim,
        })
    }
}

/// How a pane's entries sit on the sheet.
///
/// The sheet is `sunken`, the fill fields, selects, button-group tracks and
/// an off switch already use — so anything of those drawn straight on it
/// would lose its edge. Panes built from them go in a card at the step the
/// whole view used to be, where they read as they always have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// Setting rows, each ruled off below, in a card inset at the sides.
    Rows,
    /// Anything else built of controls, in a card inset all round.
    Padded,
    /// A pane that draws its own cards.
    Bare,
}

impl Preferences {
    /// What an agent launches with: its spec in the config, or — for one the
    /// config never named, like an agent shipped after it was written — the
    /// one ket ships.
    fn agent_spec(&self, name: &str) -> Option<&ket_core::config::AgentSpec> {
        self.config
            .agents
            .iter()
            .find(|spec| spec.name == name)
            .or_else(|| self.agents.get(name).map(|entry| &entry.spec))
    }

    /// The agent's spec in the config, copied in from the catalogue first
    /// when the config does not name it yet.
    fn agent_spec_mut(&mut self, name: &str) -> Option<&mut ket_core::config::AgentSpec> {
        if !self.config.agents.iter().any(|spec| spec.name == name) {
            let spec = self.agents.get(name)?.spec.clone();
            self.config.agents.push(spec);
        }
        self.config.agents.iter_mut().find(|spec| spec.name == name)
    }
}

/// The settings view while it is open.
pub(crate) struct Preferences {
    /// Which section the right-hand side is showing.
    pub(crate) section: Section,
    /// The filter across the top. An entity because a text field is one —
    /// see [`crate::input`] for why the input method needs it to be.
    pub(crate) search: Entity<TextInput>,
    /// The durable settings being shown and changed.
    pub(crate) config: Config,
    /// Installed state merged with that configuration.
    pub(crate) agents: Catalogue,
    /// The provider whose launch details are expanded.
    pub(crate) expanded_agent: Option<String>,
    /// Whether the OpenCode Go account card is expanded.
    usage_expanded: bool,
    /// The OpenCode Go credentials, as they are being edited.
    ///
    /// Not a draft on an agent card: the quota they unlock belongs to a
    /// subscription rather than to the binary the card configures, and they
    /// are edited whether or not any card is open.
    usage: UsageDraft,
    /// The TypeSafe API key for Jev's beta, as it is being typed. Drawn as
    /// bullets, as the OpenCode Go key is.
    jev_key: Entity<TextInput>,
    /// A persistence or detection failure shown in the pane that caused it.
    pub(crate) error: Option<String>,
    /// False when the existing config could not be loaded.
    ///
    /// Showing defaults is still useful, but writing them over a malformed
    /// file would destroy the exact text the user needs to repair.
    pub(crate) writable: bool,
    /// Unsaved launch values for the expanded provider.
    draft: Option<AgentDraft>,
    /// The merge pane's commit-message template, as it is being typed.
    ///
    /// Built with the view rather than when the pane is first drawn, for the
    /// same reason [`UsageDraft`] is: a field is an entity, and one created
    /// mid-render would miss the observer that repaints as it is typed into.
    merge_template: Entity<TextInput>,
    /// The Editor pane's point size, as it is being typed.
    ///
    /// Empty is a real value here and means "follow the code font size", which
    /// is what the placeholder says it will do.
    editor_font_size: Entity<TextInput>,
    /// The Editor pane's line height, as it is being typed.
    editor_line_height: Entity<TextInput>,
    /// The Theme picker's dropdown, open with the current choice ticked, or
    /// `None` while it is closed.
    theme_menu: Option<OpenMenu<Option<String>>>,
    /// The General pane's Worktree directory dropdown, when open.
    worktree_dir_menu: Option<OpenMenu<DirPick>>,
    /// The General pane's Check for updates dropdown, when open.
    update_menu: Option<OpenMenu<UpdateCheck>>,
    /// The Phones pane: the pairing code, the paired phones, and the clock
    /// that keeps both current. Dropped with the view, which stops it.
    pub(crate) phones: crate::phones::PhonesPane,
    /// The Snippets pane: what is saved, and the card being edited.
    pub(crate) snippets: crate::snippets::SnippetsPane,
    /// General's Remove from your agents, as far as it has got.
    unhook: Unhook,
}

/// Taking ket out of the agents it puts its hooks into, for uninstalling it
/// — see [`ket_core::agent_hooks::remove_everywhere`]. Asked in the row
/// itself rather than in a dialog: Settings is already one.
enum Unhook {
    Idle,
    /// The row is asking to be sure.
    Asking,
    Working,
    /// What happened in each agent.
    Done(Vec<ket_core::agent_hooks::Removed>),
}

/// One of the view's text fields.
///
/// There is no variant for "none": which field has the keyboard is a fact
/// gpui already holds, and a copy of it here is a copy that goes stale the
/// moment a press somewhere else blurs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The filter across the top.
    Search,
    /// The expanded provider's command line.
    Command,
    /// Its default arguments.
    Arguments,
    /// Its environment.
    Environment,
    /// The OpenCode Go API key, where one is pasted.
    ApiKey,
    /// The `opencode.ai` session cookie, where the provider has one.
    Cookie,
    /// The workspace to read usage from, where the provider has one.
    Workspace,
    /// The merge button's commit-message template.
    MergeTemplate,
    /// The size ket's own editor draws at.
    EditorFontSize,
    /// The multiple of that size one editor line occupies.
    EditorLineHeight,
    /// The open snippet card's name.
    SnippetName,
    /// The TypeSafe API key for Jev's beta.
    JevKey,
}

impl Field {
    /// Every field, in the order they are drawn.
    const ALL: [Field; 12] = [
        Field::Search,
        Field::Command,
        Field::Arguments,
        Field::Environment,
        Field::ApiKey,
        Field::Cookie,
        Field::Workspace,
        Field::MergeTemplate,
        Field::EditorFontSize,
        Field::EditorLineHeight,
        Field::SnippetName,
        Field::JevKey,
    ];

    /// Whether this field is one of the pane's editable values rather than
    /// the filter across the top.
    ///
    /// The filter is the only field that is not, and the difference decides
    /// what Escape, Enter and Tab mean.
    fn editable(self) -> bool {
        !matches!(self, Field::Search)
    }

    /// The next field Tab moves to, wrapping within its own group.
    ///
    /// Tab does not leave the group it is in: the filter is reached by
    /// clicking it or by pressing Escape, and a Tab that walked out of the
    /// expanded provider would abandon a half-typed command without saying
    /// so. The three usage fields are a group of their own for the same
    /// reason — they are a different setting that happens to share the
    /// pane.
    fn next(self) -> Self {
        match self {
            Field::Command => Field::Arguments,
            Field::Arguments => Field::Environment,
            Field::Environment => Field::Command,
            Field::ApiKey => Field::Cookie,
            Field::Cookie => Field::Workspace,
            Field::Workspace => Field::ApiKey,
            // A group of one: it is the only field in its pane, so Tab has
            // nowhere else to go and staying is better than jumping panes.
            Field::MergeTemplate => Field::MergeTemplate,
            // The two type settings are one group: they are saved together, so
            // Tab between them is a move within a single control, and there is
            // nowhere else in the pane a keyboard would want to go.
            Field::EditorFontSize => Field::EditorLineHeight,
            Field::EditorLineHeight => Field::EditorFontSize,
            // Tab from the name goes to the body box, which is not a text
            // field — see `Shell::snippets_key`.
            Field::SnippetName => Field::SnippetName,
            // The only field in its group.
            Field::JevKey => Field::JevKey,
            Field::Search => Field::Search,
        }
    }
}

impl Preferences {
    /// Which field has the keyboard, if any, and the field itself.
    ///
    /// Read back off the handles rather than remembered, so a field blurred
    /// by a press elsewhere is not still believed to hold the caret.
    fn focused(&self, window: &Window, cx: &App) -> Option<(Field, Entity<TextInput>)> {
        Field::ALL.into_iter().find_map(|field| {
            let input = self.input(field)?;
            input.read(cx).is_focused(window).then_some((field, input))
        })
    }

    /// The field, when the view has one — the launch fields exist only while
    /// a provider is expanded.
    fn input(&self, field: Field) -> Option<Entity<TextInput>> {
        match field {
            Field::Search => Some(self.search.clone()),
            Field::Command => self.draft.as_ref().map(|draft| draft.command.clone()),
            Field::Arguments => self.draft.as_ref().map(|draft| draft.arguments.clone()),
            Field::Environment => self.draft.as_ref().map(|draft| draft.environment.clone()),
            Field::ApiKey => Some(self.usage.api_key.clone()),
            Field::Cookie => Some(self.usage.cookie.clone()),
            Field::Workspace => Some(self.usage.workspace.clone()),
            Field::MergeTemplate => Some(self.merge_template.clone()),
            Field::EditorFontSize => Some(self.editor_font_size.clone()),
            Field::EditorLineHeight => Some(self.editor_line_height.clone()),
            Field::SnippetName => self.snippets.name.clone(),
            Field::JevKey => Some(self.jev_key.clone()),
        }
    }

    /// The next field Tab should land in, skipping any that is not on screen.
    ///
    /// Bounded by the number of fields so a group with only one of them
    /// cannot spin here looking for a second.
    fn next_field(&self, from: Field) -> Field {
        let mut field = from;
        for _ in 0..Field::ALL.len() {
            field = field.next();
            if self.input(field).is_some() {
                return field;
            }
        }
        from
    }
}

/// Unsaved launch values for the expanded provider.
struct AgentDraft {
    /// Which provider they belong to.
    name: String,
    /// Its command line.
    command: Entity<TextInput>,
    /// Its default arguments, space separated and quoted.
    arguments: Entity<TextInput>,
    /// `KEY=value` pairs, space separated and quoted like arguments are.
    environment: Entity<TextInput>,
}

/// Which of the three OpenCode Go credentials a Clear button clears.
///
/// One button per field rather than one for the set: each is the answer to a
/// different problem — a stale key, an expired cookie, a wrong workspace —
/// and clearing one should not also sign the account out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageField {
    /// The API key.
    ApiKey,
    /// The session cookie.
    Cookie,
    /// The workspace override.
    Workspace,
}

/// One of the decisions Jev's beta may make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JevFlag {
    /// A new task's Economy level, from its prompt.
    AutoEconomy,
    /// A permission prompt's risk, for the phone.
    ApprovalRisk,
}

/// A pick from the Worktree directory dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirPick {
    /// ket's own, under its data directory.
    Default,
    /// The directory already chosen: picking it changes nothing.
    Current,
    /// Ask macOS for a folder.
    Choose,
}

/// The OpenCode Go credentials, as they are being edited.
///
/// The key and cookie fields hold real credentials and draw them as bullets
/// — see [`TextInput::masked`]. Showing a saved credential back, masked, is
/// what every settings screen does, and it is the only way the field can also
/// say *whether* one is saved without a second control to explain itself.
struct UsageDraft {
    /// The OpenCode Go API key, drawn as bullets.
    api_key: Entity<TextInput>,
    /// The `opencode.ai` session cookie, drawn as bullets.
    cookie: Entity<TextInput>,
    /// The `wrk_…` override, usually left empty.
    workspace: Entity<TextInput>,
}

impl UsageDraft {
    /// Fields seeded from what is saved.
    fn seeded(saved: &OpenCodeGoConfig, cx: &mut App) -> Self {
        Self {
            api_key: TextInput::masked("Leave blank to use OPENCODE_API_KEY", &saved.api_key, cx),
            cookie: TextInput::masked(
                "Paste the opencode.ai session cookie",
                &saved.session_cookie,
                cx,
            ),
            workspace: TextInput::with_text("Detected automatically", &saved.workspace_id, cx),
        }
    }

    /// All three fields, for the caller that has to treat them alike.
    fn fields(&self) -> [&Entity<TextInput>; 3] {
        [&self.api_key, &self.cookie, &self.workspace]
    }

    /// What is in them.
    fn values(&self, cx: &App) -> (String, String, String) {
        (
            self.api_key.read(cx).text(),
            self.cookie.read(cx).text(),
            self.workspace.read(cx).text(),
        )
    }
}

impl AgentDraft {
    /// A draft seeded from what is saved, ready to be typed into.
    fn seeded(
        name: &str,
        command: &str,
        arguments: String,
        environment: String,
        cx: &mut App,
    ) -> Self {
        Self {
            name: name.to_owned(),
            command: TextInput::with_text("Command line, alias or function", command, cx),
            arguments: TextInput::with_text("No default arguments", &arguments, cx),
            environment: TextInput::with_text("KEY=value, space separated", &environment, cx),
        }
    }

    /// The three, for the caller that has to treat them alike.
    fn fields(&self) -> [&Entity<TextInput>; 3] {
        [&self.command, &self.arguments, &self.environment]
    }

    /// What is in the three fields.
    fn values(&self, cx: &App) -> (String, String, String) {
        (
            self.command.read(cx).text(),
            self.arguments.read(cx).text(),
            self.environment.read(cx).text(),
        )
    }
}

impl Shell {
    /// Opens the settings view on the first rail item, [`Section::Agents`].
    pub(crate) fn open_preferences(&mut self, cx: &mut Context<Self>) {
        self.open_preferences_at(Section::Agents, cx);
    }

    /// The same, landing on one section rather than the first.
    ///
    /// For the sidebar's search, whose settings rows *are* the sections: a row
    /// that says "Keybindings" and then opens on General has not done what it
    /// said, and leaves the reader to find the rail entry themselves.
    pub(crate) fn open_preferences_at(&mut self, section: Section, cx: &mut Context<Self>) {
        // One overlay at a time, as everywhere else in the shell.
        self.menu = None;
        self.project_menu = None;
        self.palette.open = false;

        let (config, error, writable) = match Config::load() {
            Ok(config) => (config, None, true),
            Err(error) => (
                Config::default(),
                Some(format!("Could not load settings: {error}")),
                false,
            ),
        };
        let agents = Catalogue::detect(&config, &ShellProber);
        // The view exists to be typed into, so the filter asks for the
        // keyboard as it is built. Granted on the first frame — see
        // `TextInput::request_focus`. Without it the window's handle stays
        // focused, and a terminal pane behind the modal has an input handler
        // registered against exactly that handle: every character typed here
        // was landing in the shell underneath.
        let search = TextInput::new("Search settings", cx);
        search.update(cx, |input, _| input.request_focus());
        self.watch_field(&search, cx);
        // Built with the view rather than when the Agents pane is first drawn:
        // a field is an entity, and one created mid-render would miss the
        // observer that repaints the shell as it is typed into.
        let usage = UsageDraft::seeded(&config.opencode_go, cx);
        for input in usage.fields() {
            self.watch_field(input, cx);
        }
        let jev_key = TextInput::masked("Paste a TypeSafe API key", &config.jev.api_key, cx);
        self.watch_field(&jev_key, cx);
        let merge_template = TextInput::with_text(
            ket_core::config::DEFAULT_COMMIT_TEMPLATE,
            &config.merge.commit_template,
            cx,
        );
        self.watch_field(&merge_template, cx);
        // The size field is empty when nothing overrides the code size, and
        // says what it is inheriting in its placeholder rather than showing
        // that number as if it had been typed. A field pre-filled with an
        // inherited value cannot be cleared back to inheriting it.
        let editor_font_size = TextInput::with_text(
            format!(
                "{} \u{2014} the code font size",
                config.theme.code_font_size
            ),
            &config
                .editor
                .font_size
                .map(|size| size.to_string())
                .unwrap_or_default(),
            cx,
        );
        self.watch_field(&editor_font_size, cx);
        let editor_line_height = TextInput::with_text(
            DEFAULT_LINE_HEIGHT.to_string(),
            &config.editor.line_height.to_string(),
            cx,
        );
        self.watch_field(&editor_line_height, cx);
        self.popup = None;
        self.preferences = Some(Preferences {
            section,
            search,
            config,
            agents,
            expanded_agent: None,
            usage_expanded: false,
            usage,
            jev_key,
            error,
            writable,
            draft: None,
            merge_template,
            editor_font_size,
            editor_line_height,
            theme_menu: None,
            worktree_dir_menu: None,
            update_menu: None,
            phones: crate::phones::PhonesPane::default(),
            snippets: crate::snippets::SnippetsPane::load(),
            unhook: Unhook::Idle,
        });
        self.phones_shown(cx);
    }

    /// Saves an Agents setting, keeping the modal open with an actionable error.
    fn save_agent_preferences(&mut self) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.error = view.config.save().err().map(|error| error.to_string());
        view.agents = Catalogue::detect(&view.config, &ShellProber);
    }

    /// Changes whether recognized audio files can play inside ket.
    fn set_audio_enabled(&mut self, enabled: bool) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.viewer.audio = enabled;
        view.error = view.config.save().err().map(|error| error.to_string());
        self.audio_enabled = enabled;
        if !enabled {
            self.audio.disable();
        }
    }

    /// Changes whether a merged worktree's checkout survives the merge.
    fn set_merge_keep(&mut self, keep: bool) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.merge.keep_after_merge = keep;
        view.error = view.config.save().err().map(|error| error.to_string());
    }

    /// Changes whether the merge button asks before it acts.
    fn set_merge_confirm(&mut self, confirm: bool) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.merge.confirm = confirm;
        view.error = view.config.save().err().map(|error| error.to_string());
    }

    /// Makes one change to the settings and saves it, for a control that is a
    /// whole setting on its own — a switch, a pick from a dropdown.
    ///
    /// A change the file refuses is taken back, so what the pane shows and
    /// what the rest of ket reads cannot drift apart.
    fn save_setting(&mut self, change: impl FnOnce(&mut Config)) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        let before = view.config.clone();
        change(&mut view.config);
        view.error = view.config.save().err().map(|error| error.to_string());
        if view.error.is_some() {
            view.config = before;
        }
    }

    /// Closes whichever pane dropdown is open. Returns whether one was, so
    /// Escape can do this before it closes the view.
    fn close_preference_menus(&mut self) -> bool {
        let Some(view) = self.preferences.as_mut() else {
            return false;
        };
        let theme = view.theme_menu.take().is_some();
        let dir = view.worktree_dir_menu.take().is_some();
        let update = view.update_menu.take().is_some();
        theme || dir || update
    }

    /// Opens or closes the Worktree directory dropdown.
    fn toggle_worktree_dir_menu(&mut self) {
        let Some(view) = self.preferences.as_ref() else {
            return;
        };
        let open = view.worktree_dir_menu.is_some();
        let chosen = view.config.general.worktree_dir.clone();
        self.close_preference_menus();
        if open {
            return;
        }
        let default = ket_core::paths::worktrees_dir()
            .map(|dir| crate::header::tilde(&dir))
            .unwrap_or_default();
        let mut items = vec![MenuEntry::Item({
            let item = MenuItem::new(DirPick::Default, "Default").subtitle(default);
            if chosen.is_none() {
                item.checked()
            } else {
                item
            }
        })];
        if let Some(chosen) = chosen {
            items.push(MenuEntry::Item(
                MenuItem::new(DirPick::Current, "Chosen")
                    .subtitle(crate::header::tilde(std::path::Path::new(&chosen)))
                    .checked(),
            ));
        }
        let selected = items.len() - 1;
        items.push(MenuEntry::Separator);
        items.push(MenuEntry::Item(
            MenuItem::new(DirPick::Choose, "Choose a folder\u{2026}").icon(Icon::Folder),
        ));
        if let Some(view) = self.preferences.as_mut() {
            view.worktree_dir_menu = Some(
                OpenMenu::new(
                    "preferences-worktree-dir".into(),
                    None,
                    DIR_SELECT_WIDTH,
                    items,
                )
                .selected(selected)
                .offset(px(0.0), px(6.0)),
            );
        }
    }

    /// Acts on a pick from the Worktree directory dropdown.
    fn pick_worktree_dir(&mut self, pick: DirPick, cx: &mut Context<Self>) {
        self.close_preference_menus();
        match pick {
            DirPick::Default => self.save_setting(|config| config.general.worktree_dir = None),
            DirPick::Current => {}
            DirPick::Choose => self.choose_worktree_dir(cx),
        }
    }

    /// Asks macOS for the folder new worktrees go in, and saves it.
    fn choose_worktree_dir(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use for Worktrees".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            // The default picked by hand is still the default, not a choice
            // that happens to match it until the data directory moves.
            let dir = (ket_core::paths::worktrees_dir().ok().as_ref() != Some(&path))
                .then(|| path.display().to_string());
            let _ = this.update(cx, |this, cx| {
                this.save_setting(|config| config.general.worktree_dir = dir);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens or closes the Check for updates dropdown.
    fn toggle_update_menu(&mut self) {
        let Some(view) = self.preferences.as_ref() else {
            return;
        };
        let open = view.update_menu.is_some();
        let current = view.config.updates.check;
        self.close_preference_menus();
        if open {
            return;
        }
        let items = UpdateCheck::ALL
            .into_iter()
            .map(|check| {
                let item = MenuItem::new(check, update_check_label(check));
                MenuEntry::Item(if check == current {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        let selected = UpdateCheck::ALL
            .iter()
            .position(|check| *check == current)
            .unwrap_or(0);
        if let Some(view) = self.preferences.as_mut() {
            view.update_menu = Some(
                OpenMenu::new("preferences-updates".into(), None, SELECT_WIDTH, items)
                    .selected(selected)
                    .offset(px(0.0), px(6.0)),
            );
        }
    }

    /// Opens or closes the theme dropdown.
    fn toggle_theme_menu(&mut self) {
        let appearance = self.theme.appearance;
        let open = self
            .preferences
            .as_ref()
            .is_some_and(|view| view.theme_menu.is_some());
        self.close_preference_menus();
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if open {
            return;
        }
        let current = view.config.theme.name.clone();
        view.theme_menu = Some(
            OpenMenu::new(
                "preferences-theme".into(),
                None,
                SELECT_WIDTH,
                theme_menu_items(&current, appearance),
            )
            .selected(theme_menu_index(&current, appearance))
            .offset(px(0.0), px(6.0)),
        );
    }

    /// Changes which of ket's own themes the shell draws with, and applies it
    /// immediately rather than waiting for the next launch.
    fn set_theme_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.theme.name = name;
        view.error = view.config.save().err().map(|error| error.to_string());
        view.theme_menu = None;
        let theme_config = view.config.theme.clone();
        self.theme_config = theme_config;
        self.theme = crate::appearance::current(&self.theme_config, cx);
    }

    /// Changes whether the shell is dark, light or follows macOS, and
    /// applies it at once. The theme picker's choices follow along: it only
    /// offers themes drawn for the appearance now in effect.
    fn set_appearance(&mut self, appearance: AppearancePreference, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.theme.appearance = appearance;
        view.error = view.config.save().err().map(|error| error.to_string());
        view.theme_menu = None;
        self.theme_config = view.config.theme.clone();
        self.theme = crate::appearance::current(&self.theme_config, cx);
    }

    /// Saves the commit-message template.
    ///
    /// [`Config::save`] validates before it writes, so a template naming a
    /// placeholder nothing fills leaves the file alone and puts the reason
    /// under the field. The typed text is kept either way — taking it away is
    /// how somebody loses the sentence they were halfway through fixing.
    fn save_merge_template(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_ref() else {
            return;
        };
        if !view.writable {
            return;
        }
        let typed = view.merge_template.read(cx).text();
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        let previous = std::mem::replace(&mut view.config.merge.commit_template, typed);
        match view.config.save() {
            Ok(()) => view.error = None,
            Err(error) => {
                // Put the saved value back, so what the pane reports and what
                // the merge button would use cannot drift apart.
                view.config.merge.commit_template = previous;
                view.error = Some(error.to_string());
            }
        }
    }

    /// Saves the Editor pane's type settings, and applies them to every open
    /// editor at once. Returns whether the pane is settled — false when a
    /// value would not parse, which is what holds the view open with its
    /// error rather than letting the typing vanish with the modal.
    ///
    /// Both fields are read and written together: they are one control in two
    /// boxes, and saving a size while a half-typed line height sat beside it
    /// would put the file on screen into a shape nobody asked for. A value
    /// that will not parse leaves the file alone and says why — the typed text
    /// stays, because taking it away is how somebody loses the number they
    /// were halfway through correcting.
    fn save_editor_type(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(view) = self.preferences.as_ref() else {
            return true;
        };
        if !view.writable {
            return true;
        }
        let typed_size = view.editor_font_size.read(cx).text();
        let typed_height = view.editor_line_height.read(cx).text();

        // Empty is not a failure to parse a number, it is the absence of an
        // override: the editor goes back to drawing at the code font size.
        let size = match typed_size.trim() {
            "" => Ok(None),
            text => text.parse::<u16>().ok().map(Some).ok_or(format!(
                "Font size must be a whole number between {} and {}",
                FONT_SIZE_RANGE.start(),
                FONT_SIZE_RANGE.end(),
            )),
        };
        // Emptied rather than mistyped, like the size field above it: a
        // cleared box means "whatever ket shipped with", not a parse failure.
        let height = match typed_height.trim() {
            "" => Ok(DEFAULT_LINE_HEIGHT),
            text => text
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or(format!(
                    "Line height must be a number between {} and {}",
                    LINE_HEIGHT_RANGE.start(),
                    LINE_HEIGHT_RANGE.end(),
                )),
        };

        let Some(view) = self.preferences.as_mut() else {
            return true;
        };
        let (size, height) = match (size, height) {
            (Ok(size), Ok(height)) => (size, height),
            // The size's complaint first, because it is the field above.
            (Err(why), _) | (_, Err(why)) => {
                view.error = Some(why);
                return false;
            }
        };

        // Nothing typed that is not already saved. Said early so that closing
        // the view does not rewrite the file every time it is opened.
        if (view.config.editor.font_size, view.config.editor.line_height) == (size, height) {
            return true;
        }

        let previous = (view.config.editor.font_size, view.config.editor.line_height);
        view.config.editor.font_size = size;
        view.config.editor.line_height = height;
        match view.config.save() {
            Ok(()) => {
                view.error = None;
                // Live, not on the next launch. The whole reason to put a
                // type setting in front of somebody is so they can see what
                // 1.6 looks like and decide, and a restart between the two
                // makes that a guess instead of a choice.
                self.editor_type = EditorType {
                    font_size: size.unwrap_or(self.theme_config.code_font_size),
                    line_height: height,
                };
                cx.notify();
                true
            }
            Err(error) => {
                // Put the saved values back, so what the pane reports and what
                // the editor draws with cannot drift apart. `Config::save`
                // validates before it writes, so this is where a size of 300
                // or a line height of 0.2 is refused.
                (view.config.editor.font_size, view.config.editor.line_height) = previous;
                view.error = Some(error.to_string());
                false
            }
        }
    }

    /// Changes whether one provider is offered by selectors and launch resolution.
    fn set_agent_enabled(&mut self, name: &str, enabled: bool) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        if enabled {
            view.config.agent.disabled.remove(name);
        } else {
            view.config.agent.disabled.insert(name.to_owned());
            if matches!(&view.config.agent.default, DefaultAgent::Named(current) if current == name)
            {
                view.config.agent.default = DefaultAgent::Auto;
            }
        }
        self.save_agent_preferences();
        // Grok's usage is only read while it is switched on — see
        // `GrokRateLimitSource` — so the panel should follow the switch now
        // rather than on the cache's next pass. Off the window's thread: on,
        // it starts Grok's agent to ask.
        if name == "grok" {
            let cache = std::sync::Arc::clone(&self.rate_limits);
            std::thread::spawn(move || cache.refresh(Provider::Grok));
        }
    }

    /// Changes the fallback used when a project has no preferred agent.
    fn set_default_agent(&mut self, default: DefaultAgent) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.agent.default = default;
        self.save_agent_preferences();
    }

    /// Applies one named posture over the existing per-tool permission policy.
    fn set_permission_preset(&mut self, preset: PermissionPreset) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.agent.permissions.apply_preset(preset);
        self.save_agent_preferences();
    }

    /// Re-runs detection without changing any saved choice.
    ///
    /// Drops the cached shell answers first. Probing an alias costs a whole
    /// shell startup, so it is cached for the life of the process — and this
    /// button exists precisely for the person who has just edited their
    /// `.zshrc` and wants ket to look again.
    fn refresh_agents(&mut self) {
        ket_core::shell::clear_probe_cache();
        ket_core::ollama::clear_cache();
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        view.agents = Catalogue::detect(&view.config, &ShellProber);
        view.error = None;
    }

    /// Expands one provider's launch details, closing any other provider.
    ///
    /// Collapsing a card commits what is in it. Someone who types a command and
    /// clicks the chevron has finished editing, not abandoned it, and a panel
    /// that throws the typing away on the way out is one you cannot trust.
    fn toggle_agent_details(&mut self, name: &str, cx: &mut Context<Self>) {
        self.commit_agent_draft(cx);
        let Some(view) = self.preferences.as_ref() else {
            return;
        };
        let opening = view.expanded_agent.as_deref() != Some(name);
        // Seeded before the borrow is taken again: building the fields needs
        // the app context, which the view cannot be held across.
        let seed = opening
            .then(|| view.agent_spec(name))
            .flatten()
            .map(|spec| {
                (
                    spec.launch_command().to_owned(),
                    format_arguments(spec.launch_args()),
                    format_environment(&spec.env),
                )
            });
        let draft = seed.map(|(command, arguments, environment)| {
            AgentDraft::seeded(name, &command, arguments, environment, cx)
        });
        if let Some(draft) = draft.as_ref() {
            for input in draft.fields() {
                self.watch_field(input, cx);
            }
        }
        if let Some(view) = self.preferences.as_mut() {
            view.expanded_agent = opening.then(|| name.to_owned());
            view.draft = draft;
            if opening {
                view.usage_expanded = false;
            }
        }
    }

    /// Expands the OpenCode Go account card, closing any open agent card.
    fn toggle_opencode_go_details(&mut self, cx: &mut Context<Self>) {
        self.commit_agent_draft(cx);
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        let opening = !view.usage_expanded;
        view.usage_expanded = opening;
        if opening {
            view.expanded_agent = None;
            view.draft = None;
        }
    }

    /// Whether the expanded provider's draft differs from what is saved.
    fn agent_draft_is_dirty(&self, cx: &App) -> bool {
        let Some(view) = self.preferences.as_ref() else {
            return false;
        };
        let Some(draft) = view.draft.as_ref() else {
            return false;
        };
        view.agent_spec(&draft.name)
            .is_some_and(|spec| draft_differs(draft, spec, cx))
    }

    /// Saves the expanded provider's draft if it has changed, then forgets it.
    ///
    /// Returns whether the panel is safe to leave: a draft that cannot be
    /// parsed is kept, with its error showing, rather than being written as
    /// something the user did not type or dropped without a word.
    fn commit_agent_draft(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.agent_draft_is_dirty(cx) {
            if let Some(view) = self.preferences.as_mut() {
                view.draft = None;
            }
            return true;
        }
        self.save_agent_launch(cx);
        match self.preferences.as_mut() {
            Some(view) if view.error.is_none() => {
                view.draft = None;
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    /// Commits the expanded provider's command and arguments together.
    fn save_agent_launch(&mut self, cx: &mut Context<Self>) {
        let typed = self
            .preferences
            .as_ref()
            .and_then(|view| view.draft.as_ref())
            .map(|draft| draft.values(cx));
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        let (Some(draft), Some((command, arguments, environment))) = (view.draft.as_ref(), typed)
        else {
            return;
        };
        let arguments = match parse_arguments(&arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                view.error = Some(error);
                return;
            }
        };
        let environment = match parse_environment(&environment) {
            Ok(environment) => environment,
            Err(error) => {
                view.error = Some(error);
                return;
            }
        };
        let command = command.trim();
        if command.is_empty() {
            view.error = Some("An agent command cannot be empty.".to_owned());
            return;
        }
        let name = draft.name.clone();
        if let Some(spec) = view.agent_spec_mut(&name) {
            spec.launch = Some(AgentLaunch {
                command: command.to_owned(),
                args: arguments,
            });
            // `env_remove` is deliberately not editable here. It exists to
            // strip the variables a parent agent session leaks into a child,
            // which is a hazard of ket's own making rather than a preference,
            // and an agent launched without it loses its worktree's session.
            spec.env = environment;
        }

        self.save_agent_preferences();
    }

    /// Whether the usage fields say anything different from what is saved.
    fn usage_draft_is_dirty(&self, cx: &App) -> bool {
        let Some(view) = self.preferences.as_ref() else {
            return false;
        };
        let (api_key, cookie, workspace) = view.usage.values(cx);
        api_key.trim() != view.config.opencode_go.api_key
            || cookie.trim() != view.config.opencode_go.session_cookie
            || workspace.trim() != view.config.opencode_go.workspace_id
    }

    /// Commits the OpenCode Go credentials.
    ///
    /// Trimmed on the way in: a credential copied out of DevTools usually
    /// arrives with a newline on the end, and a header with a stray space
    /// fails against the server in a way that reads as a rejected one.
    fn save_opencode_credentials(&mut self, cx: &mut Context<Self>) {
        if !self.usage_draft_is_dirty(cx) {
            return;
        }
        let typed = self.preferences.as_ref().map(|view| view.usage.values(cx));
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        let Some((api_key, cookie, workspace)) = typed else {
            return;
        };

        view.config.opencode_go.api_key = api_key.trim().to_owned();
        view.config.opencode_go.session_cookie = cookie.trim().to_owned();
        view.config.opencode_go.workspace_id = workspace.trim().to_owned();
        self.save_agent_preferences();
        self.refetch_opencode_usage();
    }

    /// Forgets one of the saved OpenCode Go credentials.
    ///
    /// Clearing one drops the stale snapshot rather than leaving the last
    /// figures on screen under a credential that no longer exists, and the
    /// key pasted into the card — or `OPENCODE_API_KEY` — picks the account
    /// straight back up on the next refresh.
    /// Saves the Jev key as typed.
    fn save_jev_key(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.jev.api_key = view.jev_key.read(cx).text().trim().to_owned();
        self.save_agent_preferences();
    }

    /// Forgets the Jev key, which turns its beta off.
    fn clear_jev_key(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if !view.writable {
            return;
        }
        view.config.jev.api_key.clear();
        let key = view.jev_key.clone();
        key.update(cx, |input, _| input.clear());
        self.save_agent_preferences();
    }

    /// Turns one of Jev's decisions on or off.
    fn set_jev_flag(&mut self, which: JevFlag, on: bool) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        match which {
            JevFlag::AutoEconomy => view.config.jev.auto_economy = on,
            JevFlag::ApprovalRisk => view.config.jev.approval_risk = on,
        }
        self.save_agent_preferences();
    }

    fn clear_opencode_credentials(&mut self, which: UsageField, cx: &mut Context<Self>) {
        if self.preferences.as_ref().is_none_or(|view| !view.writable) {
            return;
        }
        if let Some(view) = self.preferences.as_mut() {
            match which {
                UsageField::ApiKey => view.config.opencode_go.api_key.clear(),
                UsageField::Cookie => view.config.opencode_go.session_cookie.clear(),
                UsageField::Workspace => view.config.opencode_go.workspace_id.clear(),
            }
        }
        self.save_agent_preferences();

        // Rebuilt rather than emptied in place: the field holding the caret
        // has to be replaced wholesale, the same reason Reset rebuilds a
        // launch draft. Seeded from what is now saved, so anything half-typed
        // into the cleared field goes with it.
        let seeded = self
            .preferences
            .as_ref()
            .map(|view| UsageDraft::seeded(&view.config.opencode_go, cx));
        if let Some(usage) = seeded {
            for input in usage.fields() {
                self.watch_field(input, cx);
            }
            if let Some(view) = self.preferences.as_mut() {
                view.usage = usage;
            }
        }

        self.refetch_opencode_usage();
    }

    /// Re-reads OpenCode's quota with whatever is now configured.
    ///
    /// A key or a cookie is pasted in order to see a number, so seeing it
    /// should not wait on the cache's own timer. Off the window's thread: a
    /// fetch is up to three round trips to opencode.ai, and the panel behind
    /// this one still has to draw.
    fn refetch_opencode_usage(&self) {
        if self
            .preferences
            .as_ref()
            .is_some_and(|view| view.error.is_some())
        {
            return;
        }
        let cache = std::sync::Arc::clone(&self.rate_limits);
        std::thread::spawn(move || cache.refresh(Provider::OpenCode));
    }

    /// Restores a provider's shipped launch command and arguments.
    fn reset_agent(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.preferences.as_ref().is_none_or(|view| !view.writable) {
            return;
        }
        // The shipped environment comes from the catalogue, which is the only
        // place ket keeps what an agent looked like before it was overridden.
        let shipped_env = self.preferences.as_ref().and_then(|view| {
            view.agents
                .get(name)
                .and_then(|entry| entry.shipped.as_ref())
                .map(|shipped| shipped.env.clone())
        });
        let mut shipped = None;
        if let Some(view) = self.preferences.as_mut()
            && let Some(spec) = view.agent_spec_mut(name)
        {
            spec.launch = None;
            if let Some(env) = shipped_env {
                spec.env = env;
            }
            shipped = Some((
                spec.launch_command().to_owned(),
                format_arguments(spec.launch_args()),
                format_environment(&spec.env),
            ));
        }
        // Rebuilt rather than assigned into: the fields are entities, and the
        // one holding the keyboard has to be replaced wholesale for the caret
        // to land at the end of the restored value.
        let draft = shipped.map(|(command, arguments, environment)| {
            AgentDraft::seeded(name, &command, arguments, environment, cx)
        });
        if let Some(draft) = draft.as_ref() {
            for input in draft.fields() {
                self.watch_field(input, cx);
            }
        }
        if let Some(view) = self.preferences.as_mut()
            && draft.is_some()
        {
            view.draft = draft;
        }
        self.save_agent_preferences();
    }

    /// Closes it, committing any pending launch draft first.
    ///
    /// A draft that will not parse holds the panel open with its error rather
    /// than vanishing, so the only way to lose typing is to fix it or clear it.
    pub(crate) fn close_preferences(&mut self, cx: &mut Context<Self>) {
        // Committed before the launch draft decides whether the view may
        // close: a pasted cookie is as much unsaved work as a typed command,
        // and it has no card to hold open with an error of its own.
        self.save_opencode_credentials(cx);
        // Same reason: a number typed into the Editor pane and then dismissed
        // with Escape is a setting somebody believes they changed. A value
        // that will not parse holds the view open, the way an unparseable
        // launch draft does.
        let settled = self.save_editor_type(cx);
        if settled && self.commit_agent_draft(cx) && self.commit_snippet_draft(cx) {
            self.preferences = None;
            self.publish_agents(cx);
        }
    }

    /// Handles a key while it is open. Returns whether it consumed one.
    ///
    /// The focused field gets first refusal: motion, deletion, selection and
    /// the clipboard are its, and what it declines — Enter, Escape, Tab and
    /// the vertical arrows — is what the view itself uses. Printable keys are
    /// declined too, and deliberately: consuming one would tell gpui the
    /// event is finished, and it is the fall-through to macOS's input context
    /// that produces dead keys and every input method. Reporting them as
    /// consumed *here* is still right, because it stops the shell behind the
    /// modal from also acting on them.
    pub(crate) fn preferences_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.preferences.is_none() {
            return false;
        }
        if self.snippets_key(event, window, cx) {
            return true;
        }

        let focused = self
            .preferences
            .as_ref()
            .and_then(|view| view.focused(window, cx));

        if let Some((_, input)) = focused.clone() {
            if is_text(&event.keystroke) {
                return true;
            }
            if input.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
                return true;
            }
        }

        let on_field = focused.as_ref().is_some_and(|(field, _)| field.editable());
        let searching = self
            .preferences
            .as_ref()
            .is_some_and(|view| !view.search.read(cx).text().trim().is_empty());
        let on_usage_field = matches!(
            focused,
            Some((Field::ApiKey | Field::Cookie | Field::Workspace, _))
        );
        let on_template_field = matches!(focused, Some((Field::MergeTemplate, _)));
        let on_jev_field = matches!(focused, Some((Field::JevKey, _)));
        let on_type_field = matches!(
            focused,
            Some((Field::EditorFontSize | Field::EditorLineHeight, _))
        );

        match event.keystroke.key.as_str() {
            // A Revoke asking to be sure backs out first; the view closes on
            // the next Escape.
            "escape" if self.cancel_phone_prompt() => {}
            "escape" if self.close_preference_menus() => {}
            "escape" if self.clear_preferences_search(cx) => {}
            "escape" => self.close_preferences(cx),
            "enter" if on_template_field => self.save_merge_template(cx),
            "enter" if on_jev_field => self.save_jev_key(cx),
            "enter" if on_type_field => {
                self.save_editor_type(cx);
            }
            "enter" if on_usage_field => self.save_opencode_credentials(cx),
            "enter" if on_field => self.save_agent_launch(cx),
            "tab" if on_field => {
                if let Some((field, _)) = focused
                    && let Some(next) = self.preferences.as_ref().map(|view| view.next_field(field))
                {
                    self.focus_preferences_field(next, window, cx);
                }
            }
            // Results span sections, so there is no current one to step from.
            "down" | "up" if searching => {}
            "down" | "up" => {
                if let Some(view) = self.preferences.as_mut() {
                    let at = Section::ALL
                        .iter()
                        .position(|s| *s == view.section)
                        .unwrap_or(0);
                    let last = Section::ALL.len() - 1;
                    view.section = if event.keystroke.key == "down" {
                        Section::ALL[(at + 1).min(last)]
                    } else {
                        Section::ALL[at.saturating_sub(1)]
                    };
                }
                self.phones_shown(cx);
            }
            // Everything else is swallowed. A modal takes every key it is
            // given, so a stray chord cannot drive the shell behind it. The
            // app's own chords are unaffected: gpui matches key *bindings* to
            // actions before it runs any `on_key_down`.
            _ => {}
        }

        true
    }

    /// Puts the keyboard in one of the view's fields, selecting what is there.
    ///
    /// Selected because arriving in a field by Tab is arriving to *replace* a
    /// value, which is what tabbing does in every other form.
    fn focus_preferences_field(&mut self, field: Field, window: &mut Window, cx: &mut App) {
        let Some(input) = self.preferences.as_ref().and_then(|view| view.input(field)) else {
            return;
        };
        input.update(cx, |input, _| input.select_all());
        window.focus(input.read(cx).focus_handle());
    }

    /// The settings view, or nothing when it is closed.
    pub(crate) fn preferences_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Before the view is borrowed: the snippet body is an editor pane,
        // which needs the shell mutably. Handed to whichever pass of
        // `section_entries` draws the Snippets section, once.
        let mut snippet_body = self.snippet_body_pane(window, cx);
        let view = self.preferences.as_ref()?;
        let t = &self.theme;

        // Built before the rail, which dims the sections a search found
        // nothing in.
        let query = view.search.read(cx).text();
        let query = query.trim();
        let searching = !query.is_empty();
        let mut found = Vec::new();
        let body = if searching {
            let mut body = Vec::new();
            for section in Section::ALL {
                let hits = self
                    .section_entries(section, t, &mut snippet_body, window, cx)
                    .into_iter()
                    .filter(|entry| {
                        entry
                            .words
                            .as_deref()
                            .is_some_and(|words| found_by(query, section, words))
                    })
                    .map(|entry| entry.element)
                    .collect::<Vec<_>>();
                if hits.is_empty() {
                    continue;
                }
                body.push(result_heading(section, found.is_empty(), t, cx));
                body.push(framed(section, hits, t));
                found.push(section);
            }
            if body.is_empty() {
                body.push(
                    caption(format!("No settings match \u{201c}{query}\u{201d}."), t)
                        .py(px(11.0))
                        .into_any_element(),
                );
            }
            body
        } else {
            let entries = self
                .section_entries(view.section, t, &mut snippet_body, window, cx)
                .into_iter()
                .map(|entry| entry.element)
                .collect();
            vec![framed(view.section, entries, t)]
        };

        let close = crate::ui::button::icon_button("preferences-close", Icon::Close)
            .circle()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_preferences(cx);
                cx.notify();
            }));

        let filter = text_field(
            &view.search,
            "preferences-filter",
            false,
            Style::new(t, self.caret.visible).leading(Icon::Search),
            window,
            cx,
        );

        // The rail: the filter, then each group under its caption.
        let mut rail_rows: Vec<AnyElement> = Vec::new();
        for (index, (group, sections)) in Section::GROUPS.iter().enumerate() {
            rail_rows.push(
                caption(*group, t)
                    .px(px(10.0))
                    .pb(px(6.0))
                    .when(index > 0, |el| el.pt(px(14.0)))
                    .into_any_element(),
            );
            for &section in *sections {
                // While searching no section is the one showing, and those
                // with nothing matching fade back so the ones with results
                // are the ones read.
                let on = !searching && section == view.section;
                let missed = searching && !found.contains(&section);
                rail_rows.push(
                    div()
                        .id(section.title())
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .h(px(32.0))
                        .px(px(10.0))
                        .rounded(RADIUS_SM)
                        .text_size(LABEL)
                        .cursor_pointer()
                        .text_color(paint(if on { t.text.primary } else { t.text.dim }))
                        .when(on, |el| el.bg(paint(t.selection)))
                        .when(!on, |el| el.hover(|s| s.bg(paint(t.hover))))
                        .when(missed, |el| el.opacity(0.45))
                        .child(icon(
                            section.icon(),
                            paint(if on { t.accent } else { t.text.dim }),
                        ))
                        .child(section.title())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_preferences_section(section, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let rail = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(RAIL)
            .gap(px(2.0))
            .child(div().pb(px(12.0)).child(filter))
            .children(rail_rows);

        // The pane's own header: the section in its hue, what it is for, and
        // anything that belongs to the whole pane. Results come from several
        // sections, each under its own heading, so a search has none.
        let section = view.section;
        let head = (!searching).then(|| {
            let hue = section.hue(t);
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(14.0))
                .px(SHEET_PAD)
                .pt(px(20.0))
                .child(
                    icon_well(Some(hue), false, t)
                        .size(PANE_WELL)
                        .rounded(RADIUS_MD)
                        .child(sized_icon(section.icon(), PANE_GLYPH, hue)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(PANE_TITLE)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(paint(t.text.primary))
                                .child(section.title()),
                        )
                        .child(caption(section.description(), t)),
                )
                .children((section == Section::Economy).then(|| {
                    button("economy-docs", "How Economy works")
                        .link()
                        .trailing(Icon::ExternalLink)
                        .render(t)
                        .on_click(|_, _, cx| cx.open_url(ECONOMY_DOCS))
                }))
        });

        // The sheet the pane sits on: a step down from the card, so the rail
        // reads as the card's and the pane as a page laid on it.
        let sheet = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .rounded(RADIUS_LG)
            .bg(paint(t.sunken))
            .overflow_hidden()
            .children(head)
            .child(
                div()
                    .id("preferences-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .gap(px(2.0))
                    .px(SHEET_PAD)
                    .pt(px(18.0))
                    .pb(SHEET_PAD)
                    .overflow_y_scroll()
                    .children(body),
            );

        Some(
            centered(
                card("preferences", WIDTH, t)
                    // Any press lets go of the snippet body box; one inside
                    // it takes it back, being deeper and so captured later.
                    .capture_any_mouse_down(cx.listener(|this, _, _, _| {
                        this.release_snippet_body();
                    }))
                    .max_w(relative(MOST_OF_THE_WINDOW))
                    .h(HEIGHT)
                    .max_h(relative(MOST_OF_THE_WINDOW))
                    .child(header("Settings", t).child(close))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.))
                            .gap(px(18.0))
                            .child(rail)
                            .child(sheet),
                    )
                    .children(view.error.as_ref().map(|error| {
                        div()
                            .text_size(CAPTION)
                            .text_color(paint(t.status.failed))
                            .child(error.clone())
                    }))
                    .when_some(
                        match view.section {
                            // Results come from several sections, so no one
                            // section's note speaks for them.
                            _ if searching => None,
                            Section::Agents
                            | Section::Economy
                            | Section::Snippets
                            | Section::General
                            | Section::Editor
                            | Section::Merge
                            | Section::Devices => None,
                            Section::Appearance => Some(
                                "Only Theme saves here — the rest of this section is still a layout.",
                            ),
                            Section::Keybindings => Some("Read-only for now — chords cannot be changed yet."),
                            _ => Some("A layout, not a settings store — nothing here saves yet."),
                        },
                        |el, text| el.child(placeholder_note(text, t, cx)),
                    ),
            )
            .into_any_element(),
        )
    }

    /// What one section's pane draws.
    fn section_entries(
        &self,
        section: Section,
        t: &Theme,
        snippet_body: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        match section {
            Section::Agents => self.agent_settings(t, window, cx),
            Section::Economy => self.economy_settings(t, window, cx),
            Section::Snippets => self.snippet_settings(t, snippet_body, window, cx),
            Section::Editor => self.viewer_settings(t, window, cx),
            Section::Merge => self.merge_settings(t, window, cx),
            Section::Devices => self.phone_settings(t, window, cx),
            Section::Appearance => self.appearance_settings(t, window, cx),
            Section::General => self.general_settings(t, cx),
            section => rows(section, t),
        }
    }

    /// The Economy pane: what Economy has saved beside where the worktrees
    /// sit on its levels, then the one group of things to set — the pack in
    /// force and Jev's beta. What Economy is, is the pane header's line and
    /// the website's page.
    ///
    /// Drawn as its own cards on the sheet ([`Frame::Bare`]), each lead in
    /// its own hue: green for what was saved, as in the header's popover.
    fn economy_settings(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        let overview = div()
            .flex()
            .gap(px(14.0))
            .child(self.economy_figures(cx))
            .child(self.economy_levels());

        let group = div()
            .flex()
            .flex_col()
            .mt(px(12.0))
            .rounded(RADIUS_LG)
            .border_1()
            .border_color(paint(t.border))
            .bg(paint(t.elevated))
            .child(
                self.pack_row(t, cx)
                    .border_b_1()
                    .border_color(paint(t.border)),
            )
            .children(self.jev_rows(t, window, cx));

        vec![
            Entry::setting(
                "saved savings spent cost estimate week month period worktrees levels",
                overview,
            ),
            Entry::setting(
                "pack source adoption managed company policy version Jev decisions beta \
                 TypeSafe API key auto economy approval risk",
                group,
            ),
        ]
    }

    /// The pack in force, and the one thing about it that may need doing:
    /// adopting a newer one that is waiting.
    ///
    /// Adopting is the Economy popover's "Use it" too — the same
    /// [`ket_core::pack::promote`], which renames the staged file into the
    /// cache and leaves this run's levels alone until a restart.
    fn pack_row(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let named = |pack: &ket_core::pack::Pack| {
            format!(
                "{} v{}",
                pack.name.clone().unwrap_or_else(|| pack.pack_id.clone()),
                pack.pack_version
            )
        };
        let (title, version) = match &self.active_pack {
            Some(pack) => (
                pack.name.clone().unwrap_or_else(|| pack.pack_id.clone()),
                Some(format!("v{}", pack.pack_version)),
            ),
            None => ("Built-in levels".to_owned(), None),
        };
        let staged = ket_core::pack::staged(self.active_pack.as_ref());
        // Once adopted nothing is staged any more, but the levels in force
        // are still this run's: the cache holding a different pack from the
        // one loaded is what says a restart is owed.
        let adopted = staged
            .is_none()
            .then(ket_core::pack::load)
            .flatten()
            .filter(|cached| {
                self.active_pack.as_ref().is_none_or(|active| {
                    active.pack_id != cached.pack_id || active.pack_version != cached.pack_version
                })
            });
        let status = if let Some(pack) = &staged {
            format!(
                "{} is verified and ready. It applies from the next restart.",
                named(pack)
            )
        } else if let Some(pack) = &adopted {
            format!("{} is adopted and applies when ket restarts.", named(pack))
        } else {
            match &self.pack_refresh_status {
                crate::PackRefreshStatus::Checking => {
                    "Checking for a newer pack\u{2026}".to_owned()
                }
                crate::PackRefreshStatus::Failed(error) => {
                    format!("The last check failed, so this verified pack stays: {error}")
                }
                _ => "Up to date.".to_owned(),
            }
        };
        let managed = self
            .preferences
            .as_ref()
            .is_some_and(|view| view.config.pack.url.is_some());
        let description = if managed || staged.is_some() || adopted.is_some() {
            status
        } else if self.active_pack.is_none() {
            "ket\u{2019}s own levels. A company can publish its own with [pack] in config.toml."
                .to_owned()
        } else {
            "No source is configured, so this pack stays as it is.".to_owned()
        };

        let title = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(LABEL)
                    .text_color(paint(t.text.primary))
                    .child(title),
            )
            .children(version.map(|version| {
                div()
                    .font_family(crate::fonts::chrome())
                    .text_size(CAPTION)
                    .text_color(paint(t.text.dim))
                    .child(version)
            }))
            .when(managed, |el| {
                el.child(tinted_tag("Managed", paint(t.terminal.ansi.cyan)))
            });
        let adopt = staged.map(|pack| {
            button("economy-pack-adopt", format!("Use v{}", pack.pack_version))
                .primary()
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Err(error) = ket_core::pack::promote()
                        && let Some(view) = this.preferences.as_mut()
                    {
                        view.error = Some(format!("Could not adopt the pack: {error}"));
                    }
                    cx.notify();
                }))
                .into_any_element()
        });
        economy_row(
            (Icon::ShieldCheck, paint(t.terminal.ansi.cyan)),
            title,
            description,
            adopt,
            t,
        )
    }

    /// Jev's beta: what it does, the key, and the two decisions it may make,
    /// as rows of the Economy group.
    fn jev_rows(&self, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::Div> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let jev = &view.config.jev;
        let writable = view.writable;
        let saved = jev.enabled();
        // Built before the listeners below: `text_field` wants the context
        // mutably, a listener holds it immutably.
        let field = text_field(
            &view.jev_key,
            "jev-api-key",
            false,
            Style::new(t, self.caret.visible),
            window,
            cx,
        );
        let key = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .child(field.w(JEV_KEY_W))
            .child(
                button("jev-api-key-save", "Save")
                    .enabled(writable)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_jev_key(cx);
                        cx.notify();
                    })),
            )
            .children(saved.then(|| {
                button("jev-api-key-clear", "Clear")
                    .ghost()
                    .enabled(writable)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_jev_key(cx);
                        cx.notify();
                    }))
            }));
        let toggle = |id: &'static str, on: bool, which: JevFlag| {
            switch(id, on, t)
                .when(writable && saved, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_jev_flag(which, !on);
                            cx.notify();
                        }))
                })
                .into_any_element()
        };
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(LABEL)
                    .text_color(paint(t.text.primary))
                    .child("Jev decisions"),
            )
            .child(tinted_tag("Beta", paint(t.status.merging)));
        let what = match saved {
            true => "Picks levels and scores permission prompts in about a tenth of a second.",
            false => {
                "Picks levels and scores permission prompts in about a tenth of a second. Off \
                 until a key is saved."
            }
        };
        vec![
            economy_row(
                (Icon::Asterisk, paint(t.status.merging)),
                title,
                what,
                Some(key.into_any_element()),
                t,
            )
            .border_b_1()
            .border_color(paint(t.border)),
            economy_row(
                (Icon::ListTodo, paint(t.diff.added)),
                economy_title("Auto Economy", t),
                "Phone and backlog tasks take the level Jev picks from their prompt.",
                Some(toggle(
                    "jev-auto-economy",
                    jev.auto_economy && saved,
                    JevFlag::AutoEconomy,
                )),
                t,
            )
            .border_b_1()
            .border_color(paint(t.border)),
            economy_row(
                (Icon::Smartphone, paint(t.status.attention)),
                economy_title("Approval risk", t),
                "The phone shows low, medium or high beside Allow and Deny. It never answers for \
                 you.",
                Some(toggle(
                    "jev-approval-risk",
                    jev.approval_risk && saved,
                    JevFlag::ApprovalRisk,
                )),
                t,
            ),
        ]
    }

    /// Shows one section, clearing any search: picking a section while
    /// results are up is asking to see that section, which the results would
    /// otherwise stand in front of.
    fn show_preferences_section(&mut self, section: Section, cx: &mut Context<Self>) {
        // Leaving the Agents section is leaving the card that was being
        // edited, so it commits like closing does. The same for Snippets.
        self.commit_agent_draft(cx);
        self.commit_snippet_draft(cx);
        if let Some(view) = self.preferences.as_mut() {
            view.section = section;
            view.search.update(cx, |input, _| input.clear());
        }
        // A dropdown belongs to the pane it was opened in.
        self.close_preference_menus();
        self.phones_shown(cx);
    }

    /// Empties the filter. Returns whether there was anything in it, so
    /// Escape can do this before it closes the view.
    fn clear_preferences_search(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(view) = self.preferences.as_ref() else {
            return false;
        };
        if view.search.read(cx).is_empty() {
            return false;
        }
        view.search.update(cx, |input, _| input.clear());
        true
    }

    /// The Appearance pane. Only the Theme row is real — the rest are still
    /// the layout placeholders the other rows in this file are; see
    /// `Section::Appearance` in `preferences_view`'s match.
    fn appearance_settings(
        &self,
        t: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let current = view.config.theme.name.clone();
        let theme_open = view.theme_menu.is_some();
        let chosen = view.config.theme.appearance;
        let appearance_group = button_group("preferences-appearance")
            .children(
                [
                    (AppearancePreference::Dark, "Dark"),
                    (AppearancePreference::Light, "Light"),
                    (AppearancePreference::System, "System"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (appearance, label))| {
                    segment(("preferences-appearance", index), label)
                        .selected(chosen == appearance)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_appearance(appearance, cx);
                            cx.notify();
                        }))
                }),
            )
            .render(t);

        let theme_menu = view.theme_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |this, name, cx| {
                    this.set_theme_name(name.clone(), cx);
                    cx.notify();
                },
                |this, cx| {
                    if let Some(view) = this.preferences.as_mut() {
                        view.theme_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let theme_select = dropdown(
            "preferences-theme-select",
            crate::ui::select::select(
                "preferences-theme",
                theme_label(&current, self.theme.appearance),
            )
            .open(theme_open)
            .render(t)
            .w(SELECT_WIDTH),
            theme_menu,
        )
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _window: &mut Window, cx| {
                this.toggle_theme_menu();
                cx.notify();
            }),
        )
        .into_any_element();

        vec![
            setting(
                "Appearance",
                "Dark, light, or whatever macOS is currently set to.",
                appearance_group,
                t,
            ),
            Entry::setting(
                "Theme Which palette to draw with. Desk Light Acqua Coral Punk Acid colours",
                row("Theme", "Which palette to draw with.", theme_select, t),
            ),
            setting(
                "Interface font size",
                "The sidebar, tabs and status bar are sized from this.",
                crate::ui::field::field(("placeholder-number", 1usize), 15.to_string(), "")
                    .read_only()
                    .render(t)
                    .w(px(84.0))
                    .into_any_element(),
                t,
            ),
            setting(
                "Code font size",
                "The size the terminal and diffs are drawn at, and the editor unless Editor sets its own.",
                crate::ui::field::field(("placeholder-number", 2usize), 12.to_string(), "")
                    .read_only()
                    .render(t)
                    .w(px(84.0))
                    .into_any_element(),
                t,
            ),
            setting(
                "Show the status bar",
                "Agent quota and the selected worktree's state, along the bottom.",
                switch(("placeholder-switch", 1usize), true, t).into_any_element(),
                t,
            ),
        ]
    }

    /// The functional Agents pane: defaults, permissions, detection and launch details.
    fn agent_settings(&self, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };

        let default = &view.config.agent.default;
        let preset = view.config.agent.permissions.preset();
        let entries = view.agents.entries();
        let installed = entries.iter().filter(|entry| entry.installed).count();
        let default_group = button_group("agent-default")
                .child(
                    segment("agent-default-auto", "Auto")
                        .selected(matches!(default, DefaultAgent::Auto))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_default_agent(DefaultAgent::Auto);
                            cx.notify();
                        })),
                )
                .child(
                    segment("agent-default-none", "No agent (blank terminal)")
                        .selected(matches!(default, DefaultAgent::None))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_default_agent(DefaultAgent::None);
                            cx.notify();
                        })),
                )
                // Naming a specific agent is done on that agent's own card,
                // beside the command it would run. Listing every agent twice
                // in one pane gave two controls for one setting, and the pair
                // had no way to say which of them you had last used. So this
                // segment appears only once that has happened, carries no
                // handler, and exists to show the group's third answer rather
                // than to offer it.
                .children(
                    entries
                        .iter()
                        .find(|entry| {
                            matches!(default, DefaultAgent::Named(current) if current == &entry.name)
                        })
                        .map(|entry| {
                            let (which, colour) = provider(&entry.name, t);
                            segment("agent-default-named", agent_label(&entry.name))
                                .selected(true)
                                .leading(which, colour)
                        }),
                )
                .render(t);
        let permissions_group = div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .child(
                button_group("agent-permissions")
                    .child(
                        segment("agent-permissions-yolo", "Yolo")
                            .selected(preset == PermissionPreset::Yolo)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_permission_preset(PermissionPreset::Yolo);
                                cx.notify();
                            })),
                    )
                    .child(
                        segment("agent-permissions-manual", "Manual")
                            .selected(preset == PermissionPreset::Manual)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_permission_preset(PermissionPreset::Manual);
                                cx.notify();
                            })),
                    )
                    .render(t),
            )
            // Hand-edited permissions are neither answer, and the group
            // shows neither as selected while this is beside it.
            .children((preset == PermissionPreset::Custom).then(|| tag("Custom", t)));
        let mut content = vec![
            together(
                "Default Agent Used when neither the worktree nor its project names an agent. Auto No agent blank terminal",
                [
                    subsection(
                        "Default Agent",
                        "Used when neither the worktree nor its project names an agent.",
                        t,
                    ),
                    default_group.into_any_element(),
                ],
            ),
            Entry::layout(div().h(px(14.0))),
            together(
                "Agent Permissions Choose whether ket allows tools automatically or asks before they run. Yolo Manual Custom",
                [
                    subsection(
                        "Agent Permissions",
                        "Choose whether ket allows tools automatically or asks before they run.",
                        t,
                    ),
                    permissions_group.into_any_element(),
                ],
            ),
            Entry::layout(div().h(px(18.0))),
            Entry::setting(
                "Agents detected installed refresh",
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(LABEL)
                            .text_color(paint(t.text.primary))
                            .child("Agents"),
                    )
                    .child(
                        div()
                            .ml(px(7.0))
                            .child(tag(format!("{installed} detected"), t)),
                    )
                    .child(div().flex_grow())
                    .child(
                        button("refresh-agents", "Refresh")
                            .ghost()
                            .leading(Icon::Refresh)
                            .render(t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_agents();
                                cx.notify();
                            })),
                    ),
            ),
        ];

        let mut card = |index: usize, entry: &ket_core::agents::AgentEntry| {
            let name = entry.name.clone();
            let expanded = view.expanded_agent.as_deref() == Some(&name);
            let is_default = matches!(default, DefaultAgent::Named(current) if current == &name);
            let draft = view.draft.as_ref().filter(|draft| draft.name == name);
            let words = format!(
                "{} {name} {} {} agent enabled disabled default launch command arguments environment{}",
                agent_label(&name),
                entry.spec.launch_command(),
                entry.spec.launch_args().join(" "),
                if entry.installed {
                    ""
                } else {
                    " not found available to install"
                },
            );
            let card = agent_card(
                AgentCardState {
                    index,
                    entry,
                    expanded,
                    is_default,
                    draft,
                    writable: view.writable,
                    blink: self.caret.visible,
                    mono: self.font_family.clone(),
                    launches: self
                        .projects
                        .iter()
                        .filter_map(|project| {
                            let launch = project.agent_overrides.get(&name)?;
                            Some(ProjectLaunch {
                                name: project.name.clone(),
                                color: project.color,
                                mark: project.icon.clone().unwrap_or_else(|| {
                                    project
                                        .name
                                        .chars()
                                        .next()
                                        .map(|c| c.to_uppercase().to_string())
                                        .unwrap_or_default()
                                        .into()
                                }),
                                line: crate::agent_override::line(launch),
                            })
                        })
                        .collect(),
                },
                t,
                window,
                cx,
            );
            Entry::setting(words, card)
        };
        let (ollama_entries, other_entries): (Vec<_>, Vec<_>) = entries
            .iter()
            .enumerate()
            .partition(|(_, entry)| entry.name.starts_with("ollama:"));
        content.extend(
            other_entries
                .iter()
                // An expanded card stays in place even when its command has
                // stopped resolving. Editing a command into one that is not on
                // PATH yet is normal; having the card jump to another list
                // mid-edit is not.
                .filter(|(_, entry)| entry.installed || expanded_is(view, &entry.name))
                .map(|(index, entry)| card(*index, entry)),
        );

        // Local models are shown as their own group — contiguous already,
        // since `entries` sorts by name and every one is spelled
        // `ollama:<tag>` — with messaging for the two states a missing group
        // can't tell apart on its own: not installed at all (handled by the
        // "Available to install" footer below, same as any other agent) vs
        // installed but with nothing to show right now.
        match ket_core::ollama::status(&ShellProber) {
            ket_core::ollama::Status::NotInstalled => {}
            ket_core::ollama::Status::NotRunning => {
                content.push(Entry::layout(div().h(px(12.0))));
                content.push(Entry::layout(
                    div()
                        .text_size(LABEL)
                        .text_color(paint(t.text.dim))
                        .child("Local models"),
                ));
                content.push(Entry::layout(caption(
                    "Ollama is installed but not running. Start it, then Refresh.",
                    t,
                )));
            }
            ket_core::ollama::Status::Models(models) => {
                content.push(Entry::layout(div().h(px(12.0))));
                content.push(Entry::layout(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(LABEL)
                                .text_color(paint(t.text.dim))
                                .child("Local models"),
                        )
                        .child(
                            div()
                                .ml(px(7.0))
                                .child(tag(format!("{} pulled", models.len()), t)),
                        ),
                ));
                if models.is_empty() {
                    content.push(Entry::layout(caption(
                        "No models pulled yet — run `ollama pull qwen2.5-coder` to add one.",
                        t,
                    )));
                } else {
                    content.extend(
                        ollama_entries
                            .iter()
                            .filter(|(_, entry)| entry.installed || expanded_is(view, &entry.name))
                            .map(|(index, entry)| card(*index, entry)),
                    );
                }
            }
        }

        let missing = entries
            .iter()
            .filter(|entry| !entry.installed && !expanded_is(view, &entry.name))
            .count();
        if missing > 0 {
            content.push(Entry::layout(div().h(px(12.0))));
            content.push(Entry::layout(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(LABEL)
                            .text_color(paint(t.text.dim))
                            .child("Available to install"),
                    )
                    .child(div().ml(px(7.0)).child(tag(format!("{missing} agents"), t))),
            ));
            content.extend(
                entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| !entry.installed && !expanded_is(view, &entry.name))
                    .map(|(index, entry)| card(index, entry)),
            );
        }
        content.extend(self.opencode_go_settings(t, window, cx));
        content.push(Entry::layout(
            caption(
                "Agent authentication stays with each CLI; ket never reads their credential files. OpenCode Go uses only the credentials entered in its card, or OPENCODE_API_KEY. Local models need no credentials at all.",
                t,
            )
            .mt(px(10.0)),
        ));
        content
    }

    /// The OpenCode Go credentials, in their own card.
    ///
    /// Below the agent cards rather than inside OpenCode's, because they are
    /// not about the binary those cards configure. OpenCode the CLI is
    /// bring-your-own-key and has no plan; the quota these unlock belongs to a
    /// subscription, and would go on reporting even if the binary were never
    /// installed. Putting them in the card would have said the opposite.
    ///
    /// It shares the agent cards' complete collapsed/expanded structure so the
    /// list has one interaction instead of a special permanently-open block.
    /// Its "Usage account" summary keeps the distinction visible: this card
    /// reports quota and does not launch another agent.
    fn opencode_go_settings(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let writable = view.writable;
        let saved = &view.config.opencode_go;
        let configured = saved.is_configured();
        let has_key = !saved.api_key.is_empty();
        let has_cookie = !saved.session_cookie.is_empty();
        let has_workspace = !saved.workspace_id.is_empty();
        let expanded = view.usage_expanded;
        let style = || Style::new(t, self.caret.visible);
        let mono = self.font_family.clone();

        let labelled = |label: &str, field: gpui::Stateful<gpui::Div>, clear, help: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(CAPTION)
                        .text_color(paint(t.text.primary))
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(field.flex_1().font_family(mono.clone()))
                        .children(clear),
                )
                .child(help)
                .into_any_element()
        };

        // As with an agent card, hidden details are not rendered controls. The
        // field entities remain alive in the view, so collapsing loses neither
        // text nor observers.
        let details = expanded.then(|| {
            let key_field = text_field(
                &view.usage.api_key,
                "opencode-go-api-key",
                false,
                style(),
                window,
                cx,
            );
            let cookie_field = text_field(
                &view.usage.cookie,
                "opencode-go-cookie",
                false,
                style(),
                window,
                cx,
            );
            let workspace_field = text_field(
                &view.usage.workspace,
                "opencode-go-workspace",
                false,
                style(),
                window,
                cx,
            );

            let clear = |id: &'static str, which: UsageField, shown: bool| {
                shown.then(|| {
                    button(id, "Clear")
                        .ghost()
                        .enabled(writable)
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.clear_opencode_credentials(which, cx);
                            cx.notify();
                        }))
                })
            };
            let key_clear = clear("opencode-go-api-key-clear", UsageField::ApiKey, has_key);
            let cookie_clear = clear("opencode-go-cookie-clear", UsageField::Cookie, has_cookie);
            let workspace_clear = clear(
                "opencode-go-workspace-clear",
                UsageField::Workspace,
                has_workspace,
            );

            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .pt(px(9.0))
                .border_t_1()
                .border_color(paint(t.border))
                .child(caption(
                    "Reads an OpenCode Go subscription's quota into the Usage panel. It does not change how the OpenCode CLI launches.",
                    t,
                ))
                .child(labelled(
                    "API key",
                    key_field,
                    key_clear,
                    caption(
                        "The OpenCode Go API key, as copied from the console. When this is empty, OPENCODE_API_KEY is read instead.",
                        t,
                    )
                    .into_any_element(),
                ))
                .child(labelled(
                    "Session cookie",
                    cookie_field,
                    cookie_clear,
                    caption(
                        "Only needed for a legacy console (OpenCode Black) account. Paste the whole Cookie header, or just the auth value. Find it signed in at opencode.ai: DevTools → Network → any request → Cookie.",
                        t,
                    )
                    .into_any_element(),
                ))
                .child(labelled(
                    "Workspace ID",
                    workspace_field,
                    workspace_clear,
                    caption(
                        "Only needed when the account has several and the wrong one is read. It is in the URL: opencode.ai/workspace/wrk_…/go.",
                        t,
                    )
                    .into_any_element(),
                ))
        });

        let card = div()
            .flex()
            .flex_col()
            .mt(px(8.0))
            .p(px(11.0))
            .gap(px(10.0))
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(paint(t.border))
            .bg(paint(t.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(mark("opencode", t))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child("OpenCode Go")
                                    // The tag describes this card, not the
                                    // account: a working key may come from
                                    // OPENCODE_API_KEY with nothing pasted
                                    // here, so the card says what it holds
                                    // rather than claiming the account is
                                    // not connected.
                                    .children((!configured).then(|| tag("Nothing saved", t))),
                            )
                            .child(
                                div()
                                    .font_family(mono)
                                    .text_size(px(11.0))
                                    .text_color(paint(t.text.dim))
                                    .child("Usage account"),
                            ),
                    )
                    .child(div().flex_grow())
                    .child(
                        crate::ui::button::icon_button(
                            "opencode-go-details",
                            if expanded {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                        )
                        .bare()
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_opencode_go_details(cx);
                            cx.notify();
                        })),
                    ),
            )
            .children(details);

        vec![Entry::setting(
            "OpenCode Go usage account quota subscription API key session cookie workspace ID",
            card,
        )]
    }

    /// The small, functional part of the Editor section: media is a viewer
    /// concern, but this is where readers already look for file behaviour.
    fn viewer_settings(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        let enabled = self
            .preferences
            .as_ref()
            .is_some_and(|view| view.config.viewer.audio);
        let writable = self.preferences.as_ref().is_some_and(|view| view.writable);
        let control = switch("audio-previews", enabled, t).when(writable, |el| {
            el.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_audio_enabled(!enabled);
                    cx.notify();
                }))
        });
        // Built before the switch: `text_field` wants the context mutably and
        // a listener holds it immutably, so the two cannot be interleaved in
        // one expression — the same order the Merge and Agents panes take.
        let (font_size, line_height) = match self.preferences.as_ref() {
            Some(view) => (
                Some(text_field(
                    &view.editor_font_size,
                    "editor-font-size",
                    false,
                    Style::new(t, self.caret.visible),
                    window,
                    cx,
                )),
                Some(text_field(
                    &view.editor_line_height,
                    "editor-line-height",
                    false,
                    Style::new(t, self.caret.visible),
                    window,
                    cx,
                )),
            ),
            None => (None, None),
        };

        let mut settings = vec![
            setting(
                "Font size",
                "The size ket's own editor draws at. Leave it empty to follow the code font size, which the terminal also uses.",
                number_well(font_size),
                t,
            ),
            setting(
                "Line height",
                "How tall a line is, as a multiple of the font size. Press Enter to apply.",
                number_well(line_height),
                t,
            ),
            setting(
                "Audio previews",
                "Play supported audio files in a viewer without opening an output device until Play is pressed.",
                control.into_any_element(),
                t,
            ),
            setting(
                "Word wrap",
                "Wrap long lines to the pane's width.",
                switch(("placeholder-switch", 2usize), true, t).into_any_element(),
                t,
            ),
            setting(
                "Line numbers",
                "Show a gutter with line numbers.",
                switch(("placeholder-switch", 3usize), true, t).into_any_element(),
                t,
            ),
            setting(
                "Tab width",
                "How many columns a tab character occupies.",
                crate::ui::field::field(("placeholder-number", 3usize), 4.to_string(), "")
                    .read_only()
                    .render(t)
                    .w(px(84.0))
                    .into_any_element(),
                t,
            ),
            setting(
                "Format on save",
                "Run the project's formatter when a buffer is written.",
                switch(("placeholder-switch", 4usize), false, t).into_any_element(),
                t,
            ),
        ];
        settings.push(Entry::layout(div().pt(px(12.0)).pb(px(14.0)).child(caption(
            "Font size, line height and audio previews are saved; the remaining Editor settings are layout placeholders.",
            t,
        ))));
        settings
    }

    /// The General pane: what selecting and deleting a worktree do, where new
    /// ones go, and how often to look for a newer ket.
    fn general_settings(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let writable = view.writable;
        let open_terminal = view.config.general.open_terminal;
        let confirm_delete = view.config.general.confirm_delete;
        let check = view.config.updates.check;
        let dir = view
            .config
            .worktrees_dir()
            .map(|dir| crate::header::tilde(&dir))
            .unwrap_or_default();

        let dir_menu = view.worktree_dir_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |this, pick, cx| {
                    this.pick_worktree_dir(*pick, cx);
                    cx.notify();
                },
                |this, cx| {
                    this.close_preference_menus();
                    cx.notify();
                },
            )
        });
        let dir_select = dropdown(
            "preferences-worktree-dir-select",
            crate::ui::select::select("preferences-worktree-dir", dir)
                .open(view.worktree_dir_menu.is_some())
                .render(t)
                .w(DIR_SELECT_WIDTH),
            dir_menu,
        )
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_worktree_dir_menu();
                cx.notify();
            }),
        )
        .into_any_element();

        let update_menu = view.update_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |this, check, cx| {
                    let check = *check;
                    this.close_preference_menus();
                    this.save_setting(|config| config.updates.check = check);
                    cx.notify();
                },
                |this, cx| {
                    this.close_preference_menus();
                    cx.notify();
                },
            )
        });
        let update_select = dropdown(
            "preferences-updates-select",
            crate::ui::select::select("preferences-updates", update_check_label(check))
                .open(view.update_menu.is_some())
                .render(t)
                .w(SELECT_WIDTH),
            update_menu,
        )
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_update_menu();
                cx.notify();
            }),
        )
        .into_any_element();

        // There is no updater yet; the choice is kept for the one that comes.
        // Every build is a dev build until then, and a dev build never looks.
        let updates = match ket_core::build_info::CHANNEL {
            ket_core::build_info::Channel::Dev => {
                "How often to look for a newer ket. Dev builds never look."
            }
            _ => "How often to look for a newer ket.",
        };

        vec![
            setting(
                "Open a terminal with a worktree",
                "Selecting a worktree that has no terminal opens one in its directory.",
                switch("general-open-terminal", open_terminal, t)
                    .when(writable, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_setting(|config| {
                                    config.general.open_terminal = !open_terminal;
                                });
                                cx.notify();
                            }))
                    })
                    .into_any_element(),
                t,
            ),
            setting(
                "Confirm before deleting a worktree",
                "Ask first, and say what would be lost. Off, work that would be lost still stops the delete and asks.",
                switch("general-confirm-delete", confirm_delete, t)
                    .when(writable, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_setting(|config| {
                                    config.general.confirm_delete = !confirm_delete;
                                });
                                cx.notify();
                            }))
                    })
                    .into_any_element(),
                t,
            ),
            setting(
                "Worktree directory",
                "Where new worktrees are created. Existing ones stay where they are.",
                dir_select,
                t,
            ),
            setting("Check for updates", updates, update_select, t),
            self.unhook_setting(t, cx),
        ]
    }

    /// General's last row: ket taken out of every agent, for uninstalling it.
    fn unhook_setting(&self, t: &Theme, cx: &mut Context<Self>) -> Entry {
        const NAME: &str = "Remove ket from your agents";
        let Some(view) = self.preferences.as_ref() else {
            return Entry::layout(div());
        };
        let set = |unhook: Unhook| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                if let Some(view) = this.preferences.as_mut() {
                    view.unhook = match unhook {
                        Unhook::Idle => Unhook::Idle,
                        Unhook::Asking => Unhook::Asking,
                        _ => return,
                    };
                }
                cx.notify();
            })
        };
        let (description, control) = match &view.unhook {
            Unhook::Idle => (
                "Takes ket's hooks out of Claude Code, Codex, OpenCode and Grok, and gives Claude's \
                 status line back what it had. For uninstalling ket: it puts them back the next \
                 time it starts."
                    .to_owned(),
                button("general-unhook", "Remove\u{2026}")
                    .danger()
                    .small()
                    .render(t)
                    .on_click(set(Unhook::Asking))
                    .into_any_element(),
            ),
            Unhook::Asking => (
                "Until ket starts again, its rows stop showing what the agents are doing.".to_owned(),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        button("general-unhook-cancel", "Cancel")
                            .ghost()
                            .small()
                            .render(t)
                            .on_click(set(Unhook::Idle)),
                    )
                    .child(
                        button("general-unhook-confirm", "Remove")
                            .danger()
                            .small()
                            .render(t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.remove_from_agents(cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            ),
            Unhook::Working => (
                "Taking ket out of each agent's settings\u{2026}".to_owned(),
                button("general-unhook-working", "Remove")
                    .danger()
                    .small()
                    .loading("Removing\u{2026}")
                    .render(t)
                    .into_any_element(),
            ),
            Unhook::Done(removed) => (
                unhook_summary(removed),
                button("general-unhook-quit", "Quit ket")
                    .small()
                    .render(t)
                    .on_click(|_, _, cx| cx.quit())
                    .into_any_element(),
            ),
        };
        setting(NAME, &description, control, t)
    }

    /// Takes ket out of every agent, off the window's thread: finding each
    /// project's Claude directory can ask a login shell.
    fn remove_from_agents(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        view.unhook = Unhook::Working;
        let removing = cx
            .background_executor()
            .spawn(async { ket_core::agent_hooks::remove_everywhere() });
        cx.spawn(async move |this, cx| {
            let removed = removing.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this.preferences.as_mut() {
                    view.unhook = Unhook::Done(removed);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Merge pane: what the sidebar's merge button writes and leaves behind.
    fn merge_settings(&self, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let writable = view.writable;
        let keep = view.config.merge.keep_after_merge;
        let confirm = view.config.merge.confirm;
        let mono = self.font_family.clone();

        // Built before the switches: `text_field` needs the context mutably and
        // a listener holds it immutably, so the two cannot be interleaved in
        // one expression — the same order the Agents pane takes.
        let template = text_field(
            &view.merge_template,
            "merge-commit-template",
            false,
            Style::new(t, self.caret.visible),
            window,
            cx,
        );

        let placeholders = ket_core::config::COMMIT_PLACEHOLDERS
            .iter()
            .map(|name| format!("{{{name}}}"))
            .collect::<Vec<_>>()
            .join(" · ");

        vec![
            // Stacked rather than a `row`'s right-hand control: a field wants
            // the width of the pane, and the label-left/control-right shape
            // squeezes it to its content. Same layout as the OpenCode Go
            // fields, which are the only other real ones here.
            Entry::setting(
                format!(
                    "Commit message template What to write when a worktree has uncommitted work. {placeholders}"
                ),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .pb(px(14.0))
                    .border_b_1()
                    .border_color(paint(t.border))
                    .child(
                        div()
                            .text_size(LABEL)
                            .text_color(paint(t.text.primary))
                            .child("Commit message"),
                    )
                    .child(caption(
                        "What to write when a worktree has uncommitted work. Enter saves.",
                        t,
                    ))
                    .child(template.font_family(mono.clone()))
                    .child(caption(placeholders.clone(), t)),
            ),
            setting(
                "Keep the worktree after merging",
                "Merging leaves the checkout in place. Turn this off to remove it once its work has landed.",
                switch("merge-keep", keep, t)
                    .when(writable, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_merge_keep(!keep);
                                cx.notify();
                            }))
                    })
                    .into_any_element(),
                t,
            ),
            setting(
                "Confirm before merging",
                "Ask first, and list what would be committed. Worth leaving on: the commit stages everything git does not ignore.",
                switch("merge-confirm", confirm, t)
                    .when(writable, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_merge_confirm(!confirm);
                                cx.notify();
                            }))
                    })
                    .into_any_element(),
                t,
            ),
            setting(
                "Shortcut",
                "Commits and merges the selected worktree.",
                kbd(crate::ui::menu::glyphs(&merge_chord()), mono, t).into_any_element(),
                t,
            ),
        ]
    }
}

/// The chord that runs the merge button, for the label the pane shows.
///
/// Parsed from the same text `crate::shortcuts` matches on, so the pane cannot
/// promise a chord the shell does not answer to.
fn merge_chord() -> ket_core::keybinding::Chord {
    ket_core::keybinding::Chord::parse(crate::shortcuts::MERGE_WORKTREE_CHORD)
        .expect("the shipped chord parses")
}

/// Whether this agent's card is the expanded one.
fn expanded_is(view: &Preferences, name: &str) -> bool {
    view.expanded_agent.as_deref() == Some(name)
}

pub(crate) fn subsection(title: &str, description: &str, t: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .text_size(LABEL)
                .text_color(paint(t.text.primary))
                .child(title.to_owned()),
        )
        .child(caption(description.to_owned(), t))
        .into_any_element()
}

/// A project that launches an agent with a command of its own, as the
/// agent's card in Settings names it.
struct ProjectLaunch {
    /// What the sidebar calls the project.
    name: gpui::SharedString,
    /// Its badge colour.
    color: ket_core::theme::Color,
    /// What its badge carries: its icon, else its initial.
    mark: gpui::SharedString,
    /// The whole command line it launches the agent with.
    line: gpui::SharedString,
}

struct AgentCardState<'a> {
    index: usize,
    entry: &'a ket_core::agents::AgentEntry,
    expanded: bool,
    is_default: bool,
    draft: Option<&'a AgentDraft>,
    writable: bool,
    /// The shell's caret phase, so every field on screen blinks together.
    blink: bool,
    mono: gpui::SharedString,
    /// Projects that launch this agent with their own command.
    launches: Vec<ProjectLaunch>,
}

fn agent_card(
    state: AgentCardState<'_>,
    t: &Theme,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let AgentCardState {
        index,
        entry,
        expanded,
        is_default,
        draft,
        writable,
        blink,
        mono,
        launches,
    } = state;
    let tint = crate::agent_override::hue(t);
    let overridden = !launches.is_empty();
    let name = entry.name.clone();
    let enable_name = name.clone();
    let disable_name = name.clone();
    let reset_name = name.clone();
    let default_name = name.clone();
    let label = agent_label(&name);
    let summary = std::iter::once(entry.spec.launch_command())
        .chain(entry.spec.launch_args().iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    // What the override replaced, shown struck through beside it. An override
    // you cannot see the shipped value of is one you cannot judge.
    let replaced = entry
        .shipped
        .as_ref()
        .map(|shipped| shipped.launch_command().to_owned())
        .filter(|shipped| shipped != entry.spec.launch_command());
    let installed = entry.installed;
    let enabled = entry.enabled;
    let dirty = draft.is_some_and(|draft| draft_differs(draft, &entry.spec, cx));

    // Built only when there is a draft, which is the same thing as the card
    // being expanded: the fields *are* the draft, so there is nothing to draw
    // a text field over until one exists.
    let fields = draft.map(|draft| {
        let style = || Style::new(t, blink);
        (
            text_field(
                &draft.command,
                ("agent-command", index),
                false,
                style(),
                window,
                cx,
            )
            .flex_1()
            .font_family(mono.clone()),
            text_field(
                &draft.arguments,
                ("agent-arguments", index),
                false,
                style(),
                window,
                cx,
            )
            .flex_1()
            .font_family(mono.clone()),
            text_field(
                &draft.environment,
                ("agent-environment", index),
                false,
                style(),
                window,
                cx,
            )
            .flex_1()
            .font_family(mono.clone()),
        )
    });

    div()
        .flex()
        .flex_col()
        .mt(px(8.0))
        .p(px(11.0))
        .gap(px(10.0))
        .rounded(RADIUS_MD)
        .border_1()
        // Edged in the override's tint while any project runs this agent its
        // own way: the command on the card is not the whole story then.
        .border_color(if overridden {
            alpha(tint, 0.55)
        } else {
            paint(t.border)
        })
        .bg(paint(t.panel))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(if overridden {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(22.0))
                        .rounded(crate::ui::RADIUS_SM)
                        .bg(alpha(tint, 0.16))
                        .border_1()
                        .border_color(alpha(tint, 0.4))
                        .child(mark(&name, t))
                        .into_any_element()
                } else {
                    mark(&name, t).into_any_element()
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(2.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(label)
                                .children((!installed).then(|| tag("Not found", t))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .min_w_0()
                                .font_family(mono.clone())
                                .text_size(px(11.0))
                                .text_color(paint(t.text.dim))
                                .children(replaced.map(|replaced| {
                                    div().flex_none().line_through().child(replaced)
                                }))
                                .child(div().truncate().child(summary)),
                        )
                        // Every project that runs it differently, by name,
                        // with the line it runs.
                        .children(launches.into_iter().map(|launch| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .min_w_0()
                                .text_size(px(11.0))
                                .child(
                                    badge(launch.color, launch.mark, t)
                                        .size(px(14.0))
                                        .text_size(px(8.5)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(paint(t.text.primary))
                                        .child(format!("{} runs", launch.name)),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(mono.clone())
                                        .text_color(tint)
                                        .child(launch.line),
                                )
                        })),
                )
                .child(div().flex_grow())
                // Both answers, with the one in force raised — rather than
                // the single button this was, whose changing label could be
                // read either as the current state or as what a click would
                // do to it.
                .child(
                    button_group(("agent-enabled", index))
                        .child(
                            segment(("agent-enabled-on", index), "Enabled")
                                .selected(enabled)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_agent_enabled(&enable_name, true);
                                    cx.notify();
                                })),
                        )
                        .child(
                            segment(("agent-enabled-off", index), "Disabled")
                                .selected(!enabled)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_agent_enabled(&disable_name, false);
                                    cx.notify();
                                })),
                        )
                        .render(t),
                )
                // Only offered for an agent that could actually be chosen: a
                // switched-off default is a setting that quietly does nothing,
                // and `set_agent_enabled` has to undo it the moment it is set.
                .children(enabled.then(|| {
                    checkbox(("agent-card-default", index), is_default, "Default", t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let default = if is_default {
                                DefaultAgent::Auto
                            } else {
                                DefaultAgent::Named(default_name.clone())
                            };
                            this.set_default_agent(default);
                            cx.notify();
                        }),
                    )
                }))
                .child(
                    crate::ui::button::icon_button(
                        ("agent-details", index),
                        if expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                    )
                    .bare()
                    .small()
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_agent_details(&name, cx);
                        cx.notify();
                    })),
                ),
        )
        .when_some(fields, |card, (command_field, arguments_field, environment_field)| {
            card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pt(px(9.0))
                    .border_t_1()
                    .border_color(paint(t.border))
                    .child(detail_control(
                        "Command",
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .gap(px(8.0))
                            .child(command_field)
                            .children(entry.is_overridden().then(|| {
                                button(("agent-reset", index), "Reset")
                                    .ghost()
                                    .enabled(writable)
                                    .render(t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.reset_agent(&reset_name, cx);
                                        cx.notify();
                                    }))
                            })),
                        t,
                    ))
                    .child(detail_control("Arguments", arguments_field, t))
                    .child(detail_control("Environment", environment_field, t))
                    // "Resolves to" rather than "Detected at": the answer comes
                    // from `command -v` in a real shell, so it is a path for an
                    // executable and the definition itself for an alias.
                    .child(match entry.found_at.as_ref() {
                        Some(path) => detail(
                            "Resolves to",
                            &path.display().to_string(),
                            "monospace".into(),
                            t,
                        ),
                        None => detail_control(
                            "Resolves to",
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(11.0))
                                .text_color(paint(t.status.attention))
                                .child(format!(
                                    "Your shell does not know `{}`. Refresh after editing your shell config.",
                                    entry
                                        .spec
                                        .launch_command()
                                        .split_whitespace()
                                        .next()
                                        .unwrap_or(entry.spec.launch_command())
                                )),
                            t,
                        ),
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(caption(
                                "Typed at your login shell's first prompt, so aliases, functions and shims all work.",
                                t,
                            ))
                            .child(div().flex_grow())
                            .child(
                                button(("agent-save", index), "Save")
                                    .primary()
                                    .enabled(dirty && writable)
                                    .render(t)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_agent_launch(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
        })
        .into_any_element()
}

pub(crate) fn detail_control(
    label: &str,
    control: impl gpui::IntoElement,
    t: &Theme,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .w(px(76.0))
                .flex_none()
                .text_size(CAPTION)
                .text_color(paint(t.text.dim))
                .child(label.to_owned()),
        )
        .child(control)
        .into_any_element()
}

/// Whether a draft says anything different from the spec it was seeded from.
fn draft_differs(draft: &AgentDraft, spec: &ket_core::config::AgentSpec, cx: &App) -> bool {
    let (command, arguments, environment) = draft.values(cx);
    command.trim() != spec.launch_command()
        || arguments != format_arguments(spec.launch_args())
        || environment != format_environment(&spec.env)
}

/// Expands a leading `~/` the way a shell would.
///
/// Environment values are handed to `exec` verbatim, so a `~` that a person
/// typed because their shell has always taken it would otherwise reach the
/// agent as a literal directory named `~`. Expanded on save rather than on
/// launch, so the field shows back the path that will actually be used.
fn expand_home(value: &str) -> String {
    let Some(rest) = value.strip_prefix("~/") else {
        return value.to_owned();
    };
    match ket_core::paths::home() {
        Ok(home) => home.join(rest).display().to_string(),
        Err(_) => value.to_owned(),
    }
}

/// Renders an environment as the `KEY=value` line the field edits.
fn format_environment(environment: &std::collections::BTreeMap<String, String>) -> String {
    format_arguments(
        &environment
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>(),
    )
}

/// Reads that line back, quoting and all.
///
/// A value may hold anything, `=` included, so only the first `=` separates.
/// A token without one is a mistake worth naming: silently dropping it would
/// leave a variable the user believes they set.
fn parse_environment(input: &str) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut environment = std::collections::BTreeMap::new();
    for token in parse_arguments(input)? {
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!(
                "`{token}` is not a KEY=value pair. Environment entries need an `=`."
            ));
        };
        if key.trim().is_empty() {
            return Err("An environment variable needs a name before the `=`.".to_owned());
        }
        environment.insert(key.trim().to_owned(), expand_home(value));
    }
    Ok(environment)
}

fn format_arguments(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            if !argument.is_empty()
                && argument
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._/@:=+".contains(c))
            {
                argument.clone()
            } else {
                format!("'{}'", argument.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_arguments(input: &str) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;

    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), other) => current.push(other),
            (Some('"'), '\\') => escaped = true,
            (Some('"'), other) => current.push(other),
            (None, '\'') | (None, '"') => {
                quote = Some(character);
                started = true;
            }
            (None, '\\') => {
                escaped = true;
                started = true;
            }
            (None, whitespace) if whitespace.is_whitespace() => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, other) => {
                current.push(other);
                started = true;
            }
            _ => {}
        }
    }

    if escaped || quote.is_some() {
        return Err("Arguments contain an unfinished quote or escape.".to_owned());
    }
    if started {
        arguments.push(current);
    }
    Ok(arguments)
}

fn detail(label: &str, value: &str, mono: gpui::SharedString, t: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .w(px(76.0))
                .text_size(CAPTION)
                .text_color(paint(t.text.dim))
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(mono)
                .text_size(px(11.0))
                .text_color(paint(t.text.primary))
                .child(value.to_owned()),
        )
        .into_any_element()
}

/// One setting: what it is called, what it does, and the thing you change.
///
/// A row rather than a form field, because most settings are a sentence and a
/// switch — and a form that shows only the switch makes people guess.
pub(crate) fn row(name: &str, description: &str, control: AnyElement, t: &Theme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(px(16.0))
        .py(px(11.0))
        .border_b_1()
        .border_color(paint(t.border))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(LABEL)
                        .text_color(paint(t.text.primary))
                        .child(name.to_owned()),
                )
                .child(caption(description.to_owned(), t)),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// Where the website explains Economy as a whole.
const ECONOMY_DOCS: &str = "https://ketapp.dev/docs/concepts/economy/";

/// Padding either side of a row in the Economy group.
const ECONOMY_ROW_PAD: gpui::Pixels = px(16.0);

/// The gap between an Economy row's well and its words.
const ECONOMY_ROW_GAP: gpui::Pixels = px(14.0);

/// The Jev key field's width, beside its Save button.
const JEV_KEY_W: gpui::Pixels = px(240.0);

/// A row of the Economy group: its glyph in a well of its own hue, a title
/// over a description, and a control at the far end.
fn economy_row(
    lead: (Icon, gpui::Rgba),
    title: impl IntoElement,
    description: impl Into<gpui::SharedString>,
    control: Option<AnyElement>,
    t: &Theme,
) -> gpui::Div {
    let (which, hue) = lead;
    div()
        .flex()
        .items_center()
        .gap(ECONOMY_ROW_GAP)
        .px(ECONOMY_ROW_PAD)
        .py(px(13.0))
        .child(
            icon_well(Some(hue), false, t)
                .size(WELL_SM)
                .rounded(RADIUS_MD)
                .child(icon(which, hue)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(3.0))
                .child(title)
                .child(caption(description, t)),
        )
        .children(control)
}

/// An Economy row's title, when it is only words.
fn economy_title(text: &'static str, t: &Theme) -> gpui::Div {
    div()
        .text_size(LABEL)
        .text_color(paint(t.text.primary))
        .child(text)
}

/// One thing a pane draws, and the words the filter finds it by.
///
/// A search draws every pane and keeps what matches, so the controls in its
/// results are the real ones: a switch thrown there is thrown, and a field
/// typed into there is saved the way it is in its own pane. What is only
/// layout — a spacer, a pane's closing caption, a live block like the pairing
/// code — has no words, and is left out of results rather than stranded
/// between them.
pub(crate) struct Entry {
    /// What a search matches against, or `None` for layout.
    words: Option<String>,
    /// What is drawn.
    element: AnyElement,
}

impl Entry {
    /// A setting, found by `words`.
    pub(crate) fn setting(words: impl Into<String>, element: impl IntoElement) -> Self {
        Self {
            words: Some(words.into()),
            element: element.into_any_element(),
        }
    }

    /// Layout, which a search leaves out.
    pub(crate) fn layout(element: impl IntoElement) -> Self {
        Self {
            words: None,
            element: element.into_any_element(),
        }
    }
}

/// A [`row`], found by its name and description.
/// What taking ket out of the agents did, in a sentence or two: where it
/// came out, where there was none, and what could not be changed.
fn unhook_summary(removed: &[ket_core::agent_hooks::Removed]) -> String {
    use ket_core::agent_hooks::Removal;
    let name = |entry: &ket_core::agent_hooks::Removed| {
        let agent = agent_label(&entry.agent);
        match entry.detail.as_deref() {
            Some("status line") => format!("{agent}'s status line"),
            Some(dir) => format!("{agent} ({dir})"),
            None => agent,
        }
    };
    let list = |names: Vec<String>| match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let taken: Vec<String> = removed
        .iter()
        .filter(|entry| entry.outcome == Removal::Removed)
        .map(name)
        .collect();
    let failed: Vec<String> = removed
        .iter()
        .filter_map(|entry| match &entry.outcome {
            Removal::Failed(why) => Some(format!("{}: {why}", name(entry))),
            _ => None,
        })
        .collect();
    let mut said = if taken.is_empty() {
        "No agent had ket's hooks in it.".to_owned()
    } else {
        format!("Removed from {}.", list(taken))
    };
    if !failed.is_empty() {
        said.push_str(&format!(" Could not change {}.", failed.join("; ")));
    }
    said.push_str(" Quit ket now, before it starts them up again, then delete it.");
    said
}

fn setting(name: &str, description: &str, control: AnyElement, t: &Theme) -> Entry {
    Entry::setting(
        format!("{name} {description}"),
        row(name, description, control, t),
    )
}

/// Several elements that are one setting — a subsection's heading and the
/// control under it — so a search keeps or drops them together.
///
/// Spaced as the pane body spaces its children, so grouping them changes
/// nothing on screen.
fn together(words: impl Into<String>, parts: impl IntoIterator<Item = AnyElement>) -> Entry {
    Entry::setting(words, div().flex().flex_col().gap(px(2.0)).children(parts))
}

/// Whether every word of `query` appears in `words` or in the section's
/// title, ignoring case.
///
/// Words rather than [`crate::palette::score`]'s subsequence: across a
/// sentence-long description almost any short query is a subsequence of
/// something, and a filter that keeps everything has not filtered. The
/// section's title counts so that "terminal" finds every Terminal setting, not
/// only the ones that happen to say the word.
fn found_by(query: &str, section: Section, words: &str) -> bool {
    let haystack = format!("{} {words}", section.title()).to_lowercase();
    query
        .split_whitespace()
        .all(|term| haystack.contains(&term.to_lowercase()))
}

/// The heading over one section's results, which opens that section.
/// A pane's entries, set on the sheet the way its [`Frame`] says.
fn framed(section: Section, entries: Vec<AnyElement>, t: &Theme) -> AnyElement {
    let column = div().flex().flex_col().gap(px(2.0)).children(entries);
    // Never shrunk: clipping its own corners leaves the card no content
    // minimum, so the scrolling body would squeeze it to the sheet's height
    // and cut rows off rather than scroll to them.
    let card = || {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .rounded(RADIUS_LG)
            .border_1()
            .border_color(paint(t.border))
            .bg(paint(t.elevated))
            .overflow_hidden()
    };
    match section.frame() {
        // The last row's rule would sit on the card's own edge; pulled under
        // it, the edge is the only line there.
        Frame::Rows => card()
            .px(px(16.0))
            .child(column.mb(px(-1.0)))
            .into_any_element(),
        Frame::Padded => card().p(px(16.0)).child(column).into_any_element(),
        Frame::Bare => column.into_any_element(),
    }
}

fn result_heading(section: Section, first: bool, t: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    heading_row(
        gpui::ElementId::Name(format!("preferences-result-{}", section.title()).into()),
        t,
    )
    .px_0()
    .when(!first, |el| el.mt(px(14.0)))
    .child(icon(section.icon(), paint(t.accent)))
    .child(section.title())
    .child(div().flex_grow())
    .child(icon(Icon::ChevronRight, paint(t.text.dim)))
    .on_click(cx.listener(move |this, _, _, cx| {
        this.show_preferences_section(section, cx);
        cx.notify();
    }))
    .into_any_element()
}

/// How wide the Theme and Check for updates pickers' triggers and dropdowns are.
const SELECT_WIDTH: gpui::Pixels = px(150.0);

/// The Worktree directory select and its menu: wide enough for ket's own
/// `~/.local/share/ket/worktrees` without truncating it.
const DIR_SELECT_WIDTH: gpui::Pixels = px(260.0);

/// What the Check for updates select calls each choice.
fn update_check_label(check: UpdateCheck) -> &'static str {
    match check {
        UpdateCheck::OnLaunch => "On launch",
        UpdateCheck::Daily => "Daily",
        UpdateCheck::Weekly => "Weekly",
        UpdateCheck::Never => "Never",
    }
}

/// The themes the picker offers for `appearance`, in the order it lists them.
///
/// The first row is always the shipped theme — "Desk" in the dark, "Light" in
/// the light — stored as no name at all, so it follows the appearance. The
/// rest are ket's named themes drawn for that appearance, then the user's own
/// files: a dark theme is not offered while the shell is light, because
/// choosing it would change nothing until the appearance flipped.
fn theme_choices(appearance: Appearance) -> Vec<(Option<String>, gpui::SharedString)> {
    let shipped = match appearance {
        Appearance::Dark => "Desk",
        Appearance::Light => "Light",
    };
    std::iter::once((None, gpui::SharedString::from(shipped)))
        .chain(
            Theme::available()
                .into_iter()
                .filter(|entry| !matches!(entry.name.as_str(), "dark" | "light"))
                .filter(|entry| entry.theme.appearance == appearance)
                .map(|entry| (Some(entry.name), gpui::SharedString::from(entry.label))),
        )
        .collect()
}

/// The theme rows the picker offers, with the one in effect ticked.
fn theme_menu_items(
    current: &Option<String>,
    appearance: Appearance,
) -> Vec<MenuEntry<Option<String>>> {
    let index = theme_menu_index(current, appearance);
    theme_choices(appearance)
        .into_iter()
        .enumerate()
        .map(|(position, (value, label))| {
            let item = MenuItem::new(value, label);
            MenuEntry::Item(if position == index {
                item.checked()
            } else {
                item
            })
        })
        .collect()
}

/// Where in those rows the theme in effect sits, so the menu opens with it
/// under the keyboard. A name the rows do not hold — a dark theme while the
/// shell is light — is not in effect, and the shipped row is.
fn theme_menu_index(current: &Option<String>, appearance: Appearance) -> usize {
    theme_choices(appearance)
        .iter()
        .position(|(value, _)| value == current)
        .unwrap_or(0)
}

/// What the picker calls the theme in effect.
fn theme_label(current: &Option<String>, appearance: Appearance) -> gpui::SharedString {
    let choices = theme_choices(appearance);
    choices[theme_menu_index(current, appearance)].1.clone()
}

/// A typed number's field, sized to a number rather than to the pane.
///
/// A settings row puts its control on the right at whatever width the control
/// asks for, and [`text_field`] asks for all of it — which is right for a
/// commit template and absurd for "13".
fn number_well(field: Option<gpui::Stateful<gpui::Div>>) -> AnyElement {
    match field {
        Some(field) => div().w(px(96.0)).child(field).into_any_element(),
        None => div().into_any_element(),
    }
}

/// The rows one section shows.
///
/// Placeholders, deliberately: the point of this view is the shape, and every
/// value below is either read from the running config or made up to show what
/// the row will look like. Nothing here writes anything.
fn rows(section: Section, t: &Theme) -> Vec<Entry> {
    match section {
        // Drawn by the shell's own pane methods, which need the window and
        // the shell; they never reach this table.
        Section::Agents
        | Section::Economy
        | Section::Snippets
        | Section::General
        | Section::Appearance
        | Section::Editor
        | Section::Merge
        | Section::Devices => Vec::new(),
        Section::Terminal => vec![
            setting(
                "Shell",
                "The program a new terminal runs.",
                crate::ui::select::select(("placeholder-select", 4usize), "$SHELL")
                    .render(t)
                    .w(px(150.0))
                    .into_any_element(),
                t,
            ),
            setting(
                "Scrollback",
                "Lines of history a pane keeps, memory permitting.",
                crate::ui::field::field(("placeholder-number", 4usize), 10_000.to_string(), "")
                    .read_only()
                    .render(t)
                    .w(px(84.0))
                    .into_any_element(),
                t,
            ),
            setting(
                "Close the tab when the shell exits",
                "What every other terminal on the platform does.",
                switch(("placeholder-switch", 8usize), true, t).into_any_element(),
                t,
            ),
            setting(
                "Cursor blink",
                "Blink the block cursor while the pane has focus.",
                switch(("placeholder-switch", 9usize), true, t).into_any_element(),
                t,
            ),
        ],
        // Read-only and real, unlike the rest of this table: every chord the
        // shell answers to, parsed from the text it matches on where that
        // text is named, so the list cannot promise a chord nobody handles.
        Section::Keybindings => keybindings()
            .into_iter()
            .map(|(name, description, chords)| {
                setting(name, description, chord_chips(&chords, t), t)
            })
            .collect(),
    }
}

/// The modifier ket's own chords hold: Cmd on macOS, Ctrl elsewhere.
#[cfg(target_os = "macos")]
const PRIMARY: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const PRIMARY: &str = "ctrl";

/// What the Keybindings pane lists: a name, what it does, and the chords —
/// more than one where a pair or a set of keys share the job.
fn keybindings() -> Vec<(&'static str, &'static str, Vec<String>)> {
    // `main.rs` writes its menu chords in gpui's `cmd-,` form.
    let menu = |chord: &str| chord.replace('-', "+");
    let primary = |key: &str| format!("{PRIMARY}+{key}");
    let agent = crate::shortcuts::agent_session_chord(0).expect("the first agent has a chord");
    vec![
        (
            "Command palette",
            "Every command ket can run. In a plain shell, clears it instead.",
            vec![primary("k")],
        ),
        (
            "Find file",
            "Open a file in the worktree by name.",
            vec![primary("p")],
        ),
        (
            "Search",
            "Jump to a worktree, a tab or a setting.",
            vec![primary("o")],
        ),
        (
            "Quick prompt",
            "Write a prompt and pick which agent gets it.",
            vec![primary("shift+p")],
        ),
        (
            "Send snippet",
            "Pick a saved snippet and send it to the selected worktree\u{2019}s agent.",
            vec![crate::shortcuts::SEND_SNIPPET_CHORD.to_owned()],
        ),
        ("Settings", "This view.", vec![menu(crate::SETTINGS_CHORD)]),
        (
            "New worktree",
            "Create a worktree in the current project.",
            vec![crate::shortcuts::NEW_WORKTREE_CHORD.to_owned()],
        ),
        (
            "Add to backlog",
            "Write a note for the current project\u{2019}s backlog, quoting what is selected.",
            vec![crate::shortcuts::CAPTURE_CHORD.to_owned()],
        ),
        (
            "Merge worktree",
            "Commit the selected worktree\u{2019}s work and merge it.",
            vec![crate::shortcuts::MERGE_WORKTREE_CHORD.to_owned()],
        ),
        (
            "New terminal tab",
            "A fresh shell in the focused pane.",
            vec![menu(crate::NEW_TERMINAL_TAB_CHORD)],
        ),
        (
            "Terminal",
            "Open or focus the selected worktree\u{2019}s terminal.",
            vec!["ctrl+`".to_owned()],
        ),
        (
            "New agent session",
            "A session with the first to ninth agent in the + menu.",
            vec![format!("{agent}\u{2013}9")],
        ),
        (
            "New browser tab",
            "A browser in the focused pane.",
            vec![crate::shortcuts::NEW_BROWSER_CHORD.to_owned()],
        ),
        (
            "New blank document",
            "An untitled editor in the focused pane.",
            vec![menu(crate::NEW_BLANK_DOCUMENT_CHORD)],
        ),
        (
            "Close tab",
            "The active tab.",
            vec![menu(crate::CLOSE_TAB_CHORD)],
        ),
        (
            "Previous and next tab",
            "Step through the focused pane\u{2019}s tabs.",
            vec![primary("shift+["), primary("shift+]")],
        ),
        (
            "Toggle sidebar",
            "Show or hide the project tree.",
            vec![primary("\\")],
        ),
        (
            "Toggle right panel",
            "Show or hide Files, Search and Git.",
            vec![primary("j")],
        ),
        (
            "Git changes",
            "The right panel\u{2019}s Git view.",
            vec![primary("shift+g")],
        ),
        (
            "Search in files",
            "Search text across the worktree.",
            vec![primary("shift+f")],
        ),
        (
            "Move between panes",
            "Focus the neighbouring pane.",
            ["left", "right", "up", "down"]
                .map(|arrow| format!("ctrl+alt+{arrow}"))
                .into(),
        ),
        (
            "Save",
            "Editor: write the file to disk.",
            vec![primary("s")],
        ),
        (
            "Find in file",
            "Editor: search the open file.",
            vec![primary("f")],
        ),
        (
            "Toggle line wrap",
            "Editor: wrap long lines.",
            vec![primary("alt+w")],
        ),
        (
            "Delete line",
            "Editor: the whole line.",
            vec![primary("backspace")],
        ),
        (
            "Open in VS Code",
            "Editor: the open file, in VS Code.",
            vec![primary("alt+v")],
        ),
        (
            "Open in Zed",
            "Editor: the open file, in Zed.",
            vec![primary("alt+e")],
        ),
        (
            "Find in terminal",
            "Terminal: search the scrollback.",
            vec!["cmd+f".to_owned()],
        ),
        (
            "Next and previous match",
            "Terminal: step through what Find found.",
            vec!["cmd+g".to_owned(), "cmd+shift+g".to_owned()],
        ),
        ("Quit", "Close ket.", vec![menu(crate::QUIT_CHORD)]),
    ]
}

/// A row's chords as key chips, drawn the way a menu or the palette draws
/// them.
///
/// A range — the agent sessions' `⌥⌘1–9` — is its first chord and the
/// last key after a dash.
fn chord_chips(chords: &[String], t: &Theme) -> AnyElement {
    div()
        .flex()
        .gap(px(4.0))
        .children(chords.iter().map(|chord| {
            let (first, last) = match chord.split_once('\u{2013}') {
                Some((first, last)) => (first, format!("\u{2013}{last}")),
                None => (chord.as_str(), String::new()),
            };
            let first =
                ket_core::keybinding::Chord::parse(first).expect("the shipped chord parses");
            let keys = format!("{}{last}", crate::ui::menu::glyphs(&first));
            kbd(keys, crate::fonts::chrome(), t)
        }))
        .into_any_element()
}

/// The footer's note, kept out of [`rows`] so every section carries it.
///
/// `text` says exactly how much of the section above it is real: a section
/// that is entirely [`rows`] placeholders gets the blanket warning, and one
/// with a real row or two among them — Appearance's Theme picker, currently —
/// gets told apart, because "nothing here saves yet" next to a control that
/// just saved something is the sort of lie a settings screen cannot afford.
pub(crate) fn placeholder_note(text: &str, t: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .pt(px(12.0))
        .child(caption(text.to_owned(), t))
        .child(div().flex_grow())
        .child(
            button("preferences-done", "Done")
                .primary()
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_preferences(cx);
                    cx.notify();
                })),
        )
        .into_any_element()
}
