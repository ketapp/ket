//! Settings → Agents, the "On this Mac" tab: what this Mac's Ollama has,
//! which of its models are coding agents, and the ones worth downloading.
//! Chosen as A2 on the local models settings canvas, 2026-10-16.
//!
//! The pane asks in the background — the daemon, the memory, the free disk
//! and OpenCode — and draws what came back, the way the Devices pane does.
//! A model becomes an agent once its profile is pinned to the context it
//! fits at (see `ket_core::local_models`); the pane prepares a profile
//! itself, for every local model that calls tools, as soon as OpenCode is
//! there to drive it. Downloads run on their own threads and outlive the pane;
//! a one-second clock follows them while it shows.

use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, FontWeight, ImageSource, Resource, SharedString, Task, WeakEntity, div,
    prelude::*, px,
};
use ket_core::agents::{Catalogue, DefaultAgent, Prober, ShellProber};
use ket_core::local_models::{self, Family, RECOMMENDED, Standing};
use ket_core::ollama::{self, Probe, Snapshot};
use ket_core::storage::format_bytes;
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::preferences::{Entry, Section, subsection};
use crate::terminal::TerminalId;
use crate::ui::banner::banner;
use crate::ui::button::{Variant, button, icon_button};
use crate::ui::chip::{caption, tag, tinted_tag};
use crate::ui::icon::{Icon, icon_well, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, OpenMenu, dropdown};
use crate::ui::progress::progress;
use crate::ui::toast::Tone;
use crate::ui::toggle::{checkbox, switch};
use crate::ui::{RADIUS_LG, RADIUS_MD, WELL_SM};

/// How often the pane looks at running pulls.
const TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a prompt just handed to a local agent counts as running before
/// its agent has reported it: the report takes a moment, and a second queued
/// prompt let go in that moment would load the model twice.
const SETTLE: Duration = Duration::from_secs(15);

/// A prompt waiting for the local model.
#[derive(Debug)]
struct QueuedPrompt {
    terminal: TerminalId,
    prompt: String,
    /// The review comments the prompt was written from, put back in their
    /// worktree's review if the prompt never goes — see `crate::review`.
    review: Option<crate::review::QueuedReview>,
}

/// Local tasks run one at a time: a local model is the Mac's memory and its
/// GPU, and two at once share both badly. Prompts for a local agent wait
/// here while another local agent is working, and go in order.
#[derive(Debug, Default)]
pub(crate) struct LocalQueue {
    waiting: VecDeque<QueuedPrompt>,
    /// When the last prompt went to a local agent — see [`SETTLE`].
    released_at: Option<Instant>,
}

/// What the pane last heard.
#[derive(Default)]
pub(crate) struct LocalModelsPane {
    found: Option<Found>,
    fetching: bool,
    /// Models whose profile is being made now.
    preparing: BTreeSet<String>,
    /// Models whose profile failed, so the pane does not retry them on every
    /// refresh. Cleared by Refresh.
    failed: BTreeSet<String>,
    /// The model whose Delete has been asked for, waiting on the row's
    /// confirmation.
    confirming_remove: Option<String>,
    /// The ⋯ menu open, and the model it is for.
    pub(crate) menu: Option<(String, OpenMenu<LocalAction>)>,
    /// Whether the line of models that are not agents is opened out.
    others_open: bool,
    ticker: Option<Task<()>>,
}

/// One background look.
#[derive(Debug, Clone)]
struct Found {
    probe: Probe,
    memory: Option<u64>,
    /// Free space where Ollama keeps its models, which a download needs.
    free_disk: Option<u64>,
    /// `None` when OpenCode is not installed; otherwise whether it can run on
    /// its own server, which local agents need.
    opencode: Option<bool>,
}

/// Asks everything the pane shows. Blocks; run it off the window's thread.
fn fetch() -> Found {
    let prober = ShellProber;
    let models_dir = ket_core::machine::ollama_models_dir();
    Found {
        probe: ollama::probe(&prober),
        memory: ket_core::machine::total_memory_bytes(),
        free_disk: models_dir
            .as_deref()
            .and_then(ket_core::machine::free_disk_bytes),
        opencode: prober
            .find("opencode")
            .map(|found| prober.accepts(&found, "--standalone")),
    }
}

impl Shell {
    fn local_models_pane(&mut self) -> Option<&mut LocalModelsPane> {
        self.preferences.as_mut().map(|view| &mut view.local_models)
    }

    /// Call after the settings view lands on a section. On Agents, starts the
    /// pane's clock and looks again; otherwise does nothing.
    pub(crate) fn local_models_shown(&mut self, cx: &mut Context<Self>) {
        if self
            .preferences
            .as_ref()
            .is_none_or(|view| view.section != Section::Agents)
        {
            return;
        }
        if let Some(pane) = self.local_models_pane()
            && pane.ticker.is_none()
        {
            pane.ticker = Some(cx.spawn(tick));
        }
        self.refresh_local_models(false, cx);
    }

    /// Looks again in the background. `fresh` drops every cached answer
    /// first — Refresh, and the end of a pull or a removal.
    pub(crate) fn refresh_local_models(&mut self, fresh: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.local_models_pane() else {
            return;
        };
        if pane.fetching {
            return;
        }
        pane.fetching = true;
        if fresh {
            pane.failed.clear();
            ollama::clear_cache();
        }
        let work = cx.background_executor().spawn(async move { fetch() });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let found = work.await;
            let _ = shell.update(cx, |shell, cx| shell.apply_local_models(found, cx));
        })
        .detach();
    }

    /// Puts a look's answer on the pane, re-reads the catalogue so a newly
    /// ready agent is offered everywhere at once, and prepares the profiles
    /// still missing.
    fn apply_local_models(&mut self, found: Found, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        view.agents = Catalogue::detect(&view.config, &ShellProber);
        let pane = &mut view.local_models;
        pane.fetching = false;
        let ready_to_drive = found.opencode == Some(true);
        let wanted: Vec<(String, u32)> = match (&found.probe, found.memory) {
            (Probe::Running(snapshot), Some(total)) if ready_to_drive => snapshot
                .listed()
                .filter_map(
                    |model| match local_models::standing(snapshot, model, total) {
                        Standing::Unprepared(context) => Some((model.tag.clone(), context)),
                        _ => None,
                    },
                )
                .filter(|(tag, _)| !pane.preparing.contains(tag) && !pane.failed.contains(tag))
                .collect(),
            _ => Vec::new(),
        };
        pane.found = Some(found);
        for (tag, context) in wanted {
            self.prepare_local_model(tag, context, cx);
        }
        cx.notify();
    }

    /// Makes `tag`'s agent profile at `context`, in the background.
    fn prepare_local_model(&mut self, tag: String, context: u32, cx: &mut Context<Self>) {
        let Some(pane) = self.local_models_pane() else {
            return;
        };
        if !pane.preparing.insert(tag.clone()) {
            return;
        }
        let model = tag.clone();
        let work = cx
            .background_executor()
            .spawn(async move { ollama::prepare(&model, context) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Some(pane) = shell.local_models_pane() {
                    pane.preparing.remove(&tag);
                    if outcome.is_err() {
                        pane.failed.insert(tag.clone());
                    }
                }
                if let Err(e) = outcome {
                    shell.toast(Tone::Error, format!("Could not prepare {tag}: {e}"), cx);
                }
                shell.refresh_local_models(false, cx);
            });
        })
        .detach();
    }

    /// Starts pulling `tag`, after checking it fits on disk.
    fn pull_local_model(&mut self, tag: &str, size: u64, cx: &mut Context<Self>) {
        let free = self
            .preferences
            .as_ref()
            .and_then(|view| view.local_models.found.as_ref())
            .and_then(|found| found.free_disk);
        if free.is_some_and(|free| free < size.saturating_add(size / 10)) {
            self.toast(
                Tone::Error,
                format!(
                    "Not enough free disk to download {tag} ({}).",
                    format_bytes(size)
                ),
                cx,
            );
            return;
        }
        ollama::pull(tag);
        cx.notify();
    }

    /// Takes `tag` out of memory, in the background.
    fn unload_local_model(&mut self, tag: String, cx: &mut Context<Self>) {
        let model = tag.clone();
        let work = cx
            .background_executor()
            .spawn(async move { ollama::unload(&model) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Err(e) = outcome {
                    shell.toast(
                        Tone::Error,
                        format!("Could not free {tag}'s memory: {e}"),
                        cx,
                    );
                }
                shell.refresh_local_models(true, cx);
            });
        })
        .detach();
    }

    /// Deletes `tag` and its agent profile, on the second press.
    fn remove_local_model(&mut self, tag: String, cx: &mut Context<Self>) {
        let Some(pane) = self.local_models_pane() else {
            return;
        };
        if pane.confirming_remove.as_deref() != Some(tag.as_str()) {
            pane.confirming_remove = Some(tag);
            cx.notify();
            return;
        }
        pane.confirming_remove = None;
        let model = tag.clone();
        let work = cx.background_executor().spawn(async move {
            ollama::delete_profile(&model)?;
            ollama::delete(&model)
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                match outcome {
                    Ok(()) => shell.toast(Tone::Success, format!("Deleted {tag}."), cx),
                    Err(e) => shell.toast(Tone::Error, format!("Could not delete {tag}: {e}"), cx),
                }
                shell.refresh_local_models(true, cx);
            });
        })
        .detach();
    }

    /// The "On this Mac" tab of the Agents pane: anything missing, the local
    /// agents with their memory, and models worth downloading. Built from
    /// the shared `ui` components only.
    pub(crate) fn local_models_entries(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let pane = &view.local_models;
        let mut out = Vec::new();
        let Some(found) = pane.found.as_ref() else {
            out.push(Entry::layout(caption("Looking for Ollama\u{2026}", t)));
            return out;
        };
        let snapshot = match &found.probe {
            Probe::NotInstalled => {
                out.push(self.problem(
                    "local-ollama-missing",
                    "Local models run on Ollama, which isn't installed. Run `brew install ollama`, \
                     or download it from ollama.com/download, then Check again.",
                    cx,
                ));
                return out;
            }
            Probe::NotRunning => {
                out.push(self.problem(
                    "local-ollama-stopped",
                    "Ollama is installed but not running. Open Ollama.app, run `ollama serve`, or \
                     `brew services start ollama`, then Check again.",
                    cx,
                ));
                return out;
            }
            Probe::Running(snapshot) => snapshot,
        };
        if !snapshot.endpoint.is_loopback() {
            out.push(Entry::layout(banner(
                "local-remote",
                Tone::Info,
                format!(
                    "Ollama at {} is another machine, so its models are not offered as local agents.",
                    snapshot.endpoint.base.trim_start_matches("http://")
                ),
                t,
            )));
        }
        match found.opencode {
            None => out.push(self.problem(
                "local-opencode-missing",
                "OpenCode drives local models as agents, and it isn't installed. Install it from \
                 opencode.ai, then Check again.",
                cx,
            )),
            Some(false) => out.push(self.problem(
                "local-opencode-old",
                "This OpenCode can't run on its own server, which local agents need. Update to \
                 OpenCode 2, then Check again.",
                cx,
            )),
            Some(true) => {}
        }

        let total = found.memory.unwrap_or(0);
        let pulls = ollama::pulls();
        let downloading: Vec<_> = pulls
            .iter()
            .filter(|pull| !pull.progress().done)
            .cloned()
            .collect();
        let (agents, others): (Vec<_>, Vec<_>) = snapshot.listed().partition(|model| {
            matches!(
                local_models::standing(snapshot, model, total),
                Standing::Ready(_) | Standing::Unprepared(_)
            )
        });

        // Your agents: the card, its memory in the header.
        let loaded: u64 = snapshot.loaded.iter().map(|m| m.size_bytes).sum();
        let fraction = if total > 0 {
            (loaded as f64 / total as f64) as f32
        } else {
            0.0
        };
        let mut card = local_card(t).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(16.0))
                .h(px(44.0))
                .child(
                    div()
                        .prose()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(paint(t.text.primary))
                        .child("Your agents"),
                )
                .child(div().flex_1())
                .child(caption("Memory", t))
                .child(progress(fraction).width(px(160.0)).render(t))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(paint(t.text.dim))
                        .child(memory_figure(loaded, total)),
                ),
        );
        let mut rows = 0;
        for model in &agents {
            card = card.child(self.agent_row(snapshot, found, model, t, cx));
            rows += 1;
        }
        for pull in &downloading {
            card = card.child(download_row(pull, t, cx));
            rows += 1;
        }
        if rows == 0 {
            card = card.child(
                div()
                    .px(px(16.0))
                    .py(px(14.0))
                    .border_t_1()
                    .border_color(paint(t.border))
                    .child(caption("No local agents yet. Download one below.", t)),
            );
        }
        if !others.is_empty() {
            let after_agents = rows > 0;
            card = card.child(self.others_line(&others, after_agents, snapshot, total, t, cx));
        }
        out.push(Entry::setting(
            "local agents memory free models on this mac",
            card,
        ));

        // Add a model: what is worth downloading on this Mac.
        if total > 0 {
            let tier = local_models::tier(total);
            // Ollama lists a model pulled without a tag as `name:latest`.
            let have = |tag: &str| {
                snapshot
                    .models
                    .iter()
                    .any(|m| m.tag == tag || m.tag.strip_suffix(":latest") == Some(tag))
                    || downloading.iter().any(|pull| pull.tag() == tag)
            };
            // In the list's order, best first: the first card is the best fit.
            let mut offered: Vec<_> = RECOMMENDED
                .iter()
                .filter(|model| model.min_tier <= tier && !have(model.tag))
                .collect();
            offered.truncate(3);
            if !offered.is_empty() {
                out.push(Entry::layout(div().h(px(18.0))));
                out.push(Entry::setting(
                    "add a model download recommended",
                    subsection(
                        "Add a model",
                        &format!(
                            "Picked for this Mac's {}. Each one is an agent as soon as it's \
                             downloaded.",
                            tier.label()
                        ),
                        t,
                    ),
                ));
                out.push(Entry::layout(div().h(px(10.0))));
                let free = found.free_disk;
                let best = offered[0].tag;
                let cards = div()
                    .flex()
                    .gap(px(12.0))
                    .children(offered.into_iter().map(|model| {
                        let short = free
                            .is_some_and(|free| free < model.size_bytes + model.size_bytes / 10);
                        offer_card(model, model.tag == best, short, t, cx)
                    }));
                out.push(Entry::setting(
                    "download qwen gemma glm gpt-oss model",
                    cards,
                ));
            }
        }

        out.push(Entry::layout(div().h(px(14.0))));
        out.push(Entry::setting(
            "local private jev web tools internet",
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(sized_icon(Icon::Lock, px(13.0), paint(t.text.dim)))
                .child(caption(
                    "Models and prompts stay on this Mac, unless you pick another model inside \
                     OpenCode. Its web tools still reach the internet.",
                    t,
                )),
        ));
        out
    }

    /// One local agent: its logo, its name, and what can be done with it.
    fn agent_row(
        &self,
        snapshot: &Snapshot,
        found: &Found,
        model: &ollama::ModelInfo,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.preferences.as_ref() else {
            return div().into_any_element();
        };
        let pane = &view.local_models;
        let tag = model.tag.clone();
        let agent = local_models::agent_name(&tag);
        let pretty = local_models::pretty(&tag);
        let standing = local_models::standing(snapshot, model, found.memory.unwrap_or(0));
        let loaded = snapshot
            .loaded
            .iter()
            .find(|loaded| loaded.tag == tag || loaded.tag == local_models::profile_tag(&tag))
            .cloned();
        let context = match standing {
            Standing::Ready(context) | Standing::Unprepared(context) => Some(context),
            _ => None,
        };
        let mut facts = vec![tag.clone(), format_bytes(model.size_bytes)];
        facts.extend(context.map(|context| format!("{}k context", context / 1024)));

        let mut title = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .prose()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(paint(t.text.primary))
                    .child(pretty.name),
            )
            .children(pretty.size.map(|size| {
                div()
                    .prose()
                    .text_size(px(12.0))
                    .text_color(paint(t.text.dim))
                    .child(size)
            }));
        if let Some(loaded) = &loaded {
            title = title.child(tinted_tag(
                format!("loaded \u{b7} {}", format_bytes(loaded.size_bytes)),
                paint(t.status.running),
            ));
        }
        match standing {
            Standing::Unprepared(_) if pane.preparing.contains(&tag) => {
                title = title.child(tag_chip("preparing\u{2026}", t));
            }
            Standing::Unprepared(_) if pane.failed.contains(&tag) => {
                title = title.child(tag_chip("couldn't be prepared", t));
            }
            Standing::Unprepared(_) if found.opencode != Some(true) => {
                title = title.child(tag_chip("needs OpenCode", t));
            }
            _ => {}
        }

        let confirming = pane.confirming_remove.as_deref() == Some(tag.as_str());
        let mut controls = div().flex().flex_none().items_center().gap(px(12.0));
        if confirming {
            let deleted = tag.clone();
            let kept = tag.clone();
            controls = controls
                .child(
                    button(SharedString::from(format!("local-keep-{tag}")), "Keep")
                        .ghost()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(pane) = this.local_models_pane()
                                && pane.confirming_remove.as_deref() == Some(kept.as_str())
                            {
                                pane.confirming_remove = None;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    button(
                        SharedString::from(format!("local-delete-{tag}")),
                        format!("Delete {}", format_bytes(model.size_bytes)),
                    )
                    .danger()
                    .small()
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_local_model(deleted.clone(), cx);
                    })),
                );
        } else {
            if let Some(loaded) = &loaded {
                let unloaded = loaded.tag.clone();
                controls = controls.child(
                    button(
                        SharedString::from(format!("local-free-{tag}")),
                        "Free memory",
                    )
                    .small()
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.unload_local_model(unloaded.clone(), cx);
                    })),
                );
            }
            if matches!(standing, Standing::Ready(_)) {
                let enabled = !view.config.agent.disabled.contains(&agent);
                let is_default = matches!(
                    &view.config.agent.default,
                    DefaultAgent::Named(current) if current == &agent
                );
                if enabled {
                    let default_name = agent.clone();
                    controls = controls.child(
                        checkbox(
                            SharedString::from(format!("local-default-{tag}")),
                            is_default,
                            "Default",
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let default = if is_default {
                                DefaultAgent::Auto
                            } else {
                                DefaultAgent::Named(default_name.clone())
                            };
                            this.set_default_agent(default);
                            cx.notify();
                        })),
                    );
                }
                let switch_name = agent.clone();
                controls = controls.child(
                    switch(
                        SharedString::from(format!("local-enabled-{tag}")),
                        enabled,
                        t,
                    )
                    .when(view.writable, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_agent_enabled(&switch_name, !enabled);
                                cx.notify();
                            }))
                    }),
                );
            }
            controls = controls.child(self.model_menu(&tag, model.size_bytes, t, cx));
        }

        div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .px(px(16.0))
            .h(px(62.0))
            .border_t_1()
            .border_color(paint(t.border))
            .child(logo(&tag, px(32.0), t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .flex_1()
                    .min_w_0()
                    .child(title)
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(paint(t.text.dim))
                            .truncate()
                            .child(facts.join(" \u{b7} ")),
                    ),
            )
            .child(controls)
            .into_any_element()
    }

    /// The ⋯ button and the menu behind it: set as default, the model's page,
    /// and Delete — the one destructive action, kept off the row.
    fn model_menu(&self, tag: &str, size: u64, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self
            .preferences
            .as_ref()
            .and_then(|view| view.local_models.menu.as_ref())
            .filter(|(owner, _)| owner == tag);
        let panel = open.map(|(_, menu)| {
            menu.view(
                t,
                cx,
                |this, action, cx| {
                    if let Some(pane) = this.local_models_pane() {
                        pane.menu = None;
                    }
                    this.run_local_action(action.clone(), cx);
                    cx.notify();
                },
                |this, cx| {
                    if let Some(pane) = this.local_models_pane() {
                        pane.menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let owner = tag.to_owned();
        dropdown(
            SharedString::from(format!("local-menu-{tag}")),
            icon_button(
                SharedString::from(format!("local-more-{tag}")),
                Icon::Ellipsis,
            )
            .bare()
            .small()
            .render(t)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_local_menu(&owner, size);
                cx.notify();
            })),
            panel,
        )
        .into_any_element()
    }

    /// Opens a model's ⋯ menu, or closes it when it is the one open.
    fn toggle_local_menu(&mut self, tag: &str, size: u64) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        let agent = local_models::agent_name(tag);
        let is_default =
            matches!(&view.config.agent.default, DefaultAgent::Named(current) if current == &agent);
        let pane = &mut view.local_models;
        if pane.menu.as_ref().is_some_and(|(owner, _)| owner == tag) {
            pane.menu = None;
            return;
        }
        let mut items = Vec::new();
        if !is_default {
            items.push(MenuEntry::Item(
                MenuItem::new(LocalAction::Default(tag.to_owned()), "Set as default")
                    .icon(Icon::Check),
            ));
        }
        items.push(MenuEntry::Item(
            MenuItem::new(LocalAction::Page(tag.to_owned()), "View on ollama.com")
                .icon(Icon::ExternalLink),
        ));
        items.push(MenuEntry::Separator);
        items.push(MenuEntry::Item(
            MenuItem::new(LocalAction::Delete(tag.to_owned()), "Delete model")
                .icon(Icon::Trash)
                .detail(format_bytes(size), None)
                .danger(),
        ));
        pane.menu = Some((
            tag.to_owned(),
            OpenMenu::new("local-model-menu".into(), None, MENU_W, items)
                .offset(WELL_SM - MENU_W, px(6.0)),
        ));
    }

    fn run_local_action(&mut self, action: LocalAction, cx: &mut Context<Self>) {
        match action {
            LocalAction::Default(tag) => {
                self.set_default_agent(DefaultAgent::Named(local_models::agent_name(&tag)));
            }
            LocalAction::Page(tag) => cx.open_url(&model_page(&tag)),
            LocalAction::Delete(tag) => {
                if let Some(pane) = self.local_models_pane() {
                    pane.confirming_remove = Some(tag);
                }
            }
        }
    }

    /// The models that are not agents, folded into one line that opens into
    /// a row each.
    fn others_line(
        &self,
        others: &[&ollama::ModelInfo],
        after_agents: bool,
        snapshot: &Snapshot,
        total: u64,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self
            .preferences
            .as_ref()
            .is_some_and(|view| view.local_models.others_open);
        let why = |model: &ollama::ModelInfo| match local_models::standing(snapshot, model, total) {
            Standing::Cloud => "runs in Ollama's cloud",
            Standing::Remote => "is on another machine",
            Standing::NoTools => "can't call tools",
            Standing::TooBig => "is too big for this Mac",
            Standing::Taken => "has its agent name taken by another model",
            Standing::Ready(_) | Standing::Unprepared(_) => "",
        };
        let summary = others
            .iter()
            .map(|model| format!("{} {}", local_models::pretty(&model.tag).name, why(model)))
            .collect::<Vec<_>>()
            .join(", ");
        let count = others.len();
        let mut block = div().flex().flex_col().child(
            div()
                .id("local-others")
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(16.0))
                .h(px(38.0))
                .border_t_1()
                .border_color(paint(t.border))
                .cursor_pointer()
                .child(sized_icon(
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    px(12.0),
                    paint(t.text.dim),
                ))
                .child(
                    div()
                        .prose()
                        .text_size(px(12.0))
                        .text_color(paint(t.text.dim))
                        .truncate()
                        .child(format!(
                            "{count}{} {}: {summary}",
                            if after_agents { " more" } else { "" },
                            if count == 1 {
                                "model isn't an agent"
                            } else {
                                "models aren't agents"
                            },
                        )),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(pane) = this.local_models_pane() {
                        pane.others_open = !pane.others_open;
                    }
                    cx.notify();
                })),
        );
        if open {
            for model in others {
                let tag = model.tag.clone();
                let confirming = self.preferences.as_ref().is_some_and(|view| {
                    view.local_models.confirming_remove.as_deref() == Some(tag.as_str())
                });
                let removed = tag.clone();
                block = block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(16.0))
                        .h(px(44.0))
                        .border_t_1()
                        .border_color(paint(t.border))
                        .child(logo(&tag, px(26.0), t))
                        .child(
                            div()
                                .prose()
                                .text_size(px(13.0))
                                .text_color(paint(t.text.primary))
                                .child(local_models::pretty(&tag).name),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(caption(format!("{tag} {}", why(model)), t)),
                        )
                        .child(
                            button(
                                SharedString::from(format!("local-other-delete-{tag}")),
                                if confirming {
                                    format!("Delete {}?", format_bytes(model.size_bytes))
                                } else {
                                    "Delete".to_owned()
                                },
                            )
                            .small()
                            .variant(if confirming {
                                Variant::Danger
                            } else {
                                Variant::Ghost
                            })
                            .render(t)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.remove_local_model(removed.clone(), cx);
                                },
                            )),
                        ),
                );
            }
        }
        block.into_any_element()
    }

    /// A banner saying what is missing, with a way to look again.
    fn problem(&self, id: &'static str, message: &str, cx: &mut Context<Self>) -> Entry {
        let t = &self.theme;
        Entry::layout(
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.0))
                .pb(px(12.0))
                .child(banner(id, Tone::Info, message.to_owned(), t))
                .child(
                    button((id, 1usize), "Check again")
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_local_models(true, cx);
                        })),
                ),
        )
    }
}

impl Shell {
    /// Whether `terminal` runs a local agent.
    pub(crate) fn runs_local_agent(&self, terminal: TerminalId) -> bool {
        self.terminals
            .get(&terminal)
            .and_then(|handle| handle.agent.as_deref())
            .is_some_and(|agent| local_models::model_tag(agent).is_some())
    }

    /// Why a prompt for `terminal` would wait in the local queue rather than
    /// go now: what the local model is busy with, or the tasks waiting for
    /// it first. `None` when it would go now.
    pub(crate) fn local_queue_wait(&self, terminal: TerminalId) -> Option<String> {
        if !self.runs_local_agent(terminal) {
            return None;
        }
        match self.local_model_busy() {
            Some(label) => Some(format!("The local model is busy with {label}")),
            None if !self.local_queue.waiting.is_empty() => {
                Some("Other tasks are waiting for the local model".to_owned())
            }
            None => None,
        }
    }

    /// Submits `prompt` to `terminal` at once, as
    /// [`Shell::submit_or_queue_review`] does when nothing is ahead of it,
    /// then `done` — see [`Shell::submit_to_terminal_then`]. For a sender
    /// that would rather refuse than queue: see [`Shell::local_queue_wait`].
    pub(crate) fn submit_now_then(
        &mut self,
        terminal: TerminalId,
        prompt: &str,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Shell, bool, &mut Context<Shell>) + 'static,
    ) {
        if self.runs_local_agent(terminal) {
            self.local_queue.released_at = Some(Instant::now());
        }
        self.submit_to_terminal_then(terminal, prompt, cx, done);
    }

    /// The worktree a local agent is working in now, by its row's label, or
    /// a prompt let go to one moments ago; `None` when the local model is
    /// free.
    fn local_model_busy(&self) -> Option<SharedString> {
        if self
            .local_queue
            .released_at
            .is_some_and(|at| at.elapsed() < SETTLE)
        {
            return Some("a task that just started".into());
        }
        let now = ket_core::now_ms();
        self.terminals
            .keys()
            .filter(|terminal| self.runs_local_agent(**terminal))
            .filter_map(|terminal| self.terminal_keys.get(terminal))
            .filter(|key| {
                self.activity
                    .pane(key, now)
                    .is_some_and(|(_, signal)| signal == ket_core::activity::Signal::Working)
            })
            .find_map(|key| {
                let worktree = self.activity.pane_status(key)?.worktree.clone();
                self.projects
                    .iter()
                    .flat_map(|project| &project.worktrees)
                    .find(|node| node.id.as_str() == worktree)
                    .map(|node| node.label())
            })
    }

    /// Hands `prompt` to the agent in `terminal` — or, when that is a local
    /// agent and the local model is busy, queues it and says so.
    pub(crate) fn submit_or_queue(
        &mut self,
        terminal: TerminalId,
        prompt: &str,
        cx: &mut Context<Self>,
    ) {
        self.submit_or_queue_review(terminal, prompt, None, cx);
    }

    /// [`Shell::submit_or_queue`], carrying the review `prompt` was written
    /// from so a queued one that never goes can be given back. Returns
    /// whether it was queued, which it has already said.
    pub(crate) fn submit_or_queue_review(
        &mut self,
        terminal: TerminalId,
        prompt: &str,
        review: Option<crate::review::QueuedReview>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.runs_local_agent(terminal) {
            self.submit_to_terminal(terminal, prompt, cx);
            return false;
        }
        let busy = self.local_model_busy();
        if busy.is_some() || !self.local_queue.waiting.is_empty() {
            self.local_queue.waiting.push_back(QueuedPrompt {
                terminal,
                prompt: prompt.to_owned(),
                review,
            });
            let ahead = self.local_queue.waiting.len();
            let detail = match busy {
                Some(label) => {
                    format!("The local model is busy with {label}. This starts when it's done.")
                }
                None => "It starts after the local tasks ahead of it.".to_owned(),
            };
            self.toast_detail(Tone::Info, format!("Queued, {ahead} waiting"), detail, cx);
            return true;
        }
        self.local_queue.released_at = Some(Instant::now());
        self.submit_to_terminal(terminal, prompt, cx);
        false
    }

    /// Lets the next queued prompt go once the local model is free. On the
    /// poll tick and whenever agents report.
    pub(crate) fn drain_local_queue(&mut self, cx: &mut Context<Self>) {
        if self.local_queue.waiting.is_empty() || self.local_model_busy().is_some() {
            return;
        }
        while let Some(next) = self.local_queue.waiting.pop_front() {
            // A tab closed while it waited takes its prompt with it — but
            // not the review comments it was written from, which go back.
            if !self.terminals.contains_key(&next.terminal) {
                if let Some(review) = next.review {
                    self.restore_review(review, cx);
                }
                continue;
            }
            self.local_queue.released_at = Some(Instant::now());
            self.submit_to_terminal(next.terminal, &next.prompt, cx);
            self.toast(Tone::Info, "Started the next queued local task", cx);
            break;
        }
    }
}

/// Width of a model's ⋯ menu.
const MENU_W: gpui::Pixels = px(232.0);

/// What a model's ⋯ menu can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalAction {
    /// Make its agent the default.
    Default(String),
    /// Open its page in Ollama's library.
    Page(String),
    /// Ask to delete it — confirmed on the row.
    Delete(String),
}

/// What is loaded out of all the memory there is: `2.6 / 64 GB`, the unit
/// said once when both share it.
fn memory_figure(loaded: u64, total: u64) -> String {
    let total = format_bytes(total);
    if loaded == 0 {
        return format!("0 / {total}");
    }
    let loaded = format_bytes(loaded);
    match (loaded.rsplit_once(' '), total.rsplit_once(' ')) {
        (Some((amount, unit)), Some((_, total_unit))) if unit == total_unit => {
            format!("{amount} / {total}")
        }
        _ => format!("{loaded} / {total}"),
    }
}

/// A model's page: Ollama's library for its own models, the publisher's
/// page for a namespaced one, Hugging Face for one pulled from there.
fn model_page(tag: &str) -> String {
    let base = tag.split(':').next().unwrap_or(tag);
    if let Some(repo) = base
        .strip_prefix("hf.co/")
        .or_else(|| base.strip_prefix("huggingface.co/"))
    {
        format!("https://huggingface.co/{repo}")
    } else if base.contains('/') {
        format!("https://ollama.com/{base}")
    } else {
        format!("https://ollama.com/library/{base}")
    }
}

/// The card the local sections sit in: the agent cards' own surface.
fn local_card(t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(RADIUS_LG)
        .border_1()
        .border_color(paint(t.border))
        .bg(paint(t.panel))
}

/// A quiet state chip beside a model's name.
fn tag_chip(text: &str, t: &Theme) -> gpui::Div {
    tag(text.to_owned(), t)
}

/// A model family's logo on a light tile, which every family's colours read
/// on; a terminal glyph for a family ket has no logo for.
fn logo(tag: &str, size: gpui::Pixels, t: &Theme) -> AnyElement {
    let file = match local_models::family(tag) {
        Family::Qwen => Some("logos/qwen.svg"),
        Family::Gemma => Some("logos/gemma.svg"),
        Family::Glm => Some("logos/glm.svg"),
        Family::OpenAi => Some("logos/openai.svg"),
        Family::Meta => Some("logos/meta.svg"),
        Family::Mistral => Some("logos/mistral.svg"),
        Family::DeepSeek => Some("logos/deepseek.svg"),
        Family::Microsoft => Some("logos/microsoft.svg"),
        Family::Other => None,
    };
    let tile = icon_well(None, false, t).size(size).rounded(RADIUS_MD);
    match file {
        Some(file) => tile
            .bg(gpui::rgb(0xf4f5f7))
            .child(
                gpui::img(ImageSource::Resource(Resource::Embedded(file.into()))).size(size * 0.6),
            )
            .into_any_element(),
        None => tile
            .child(sized_icon(Icon::Terminal, size * 0.5, paint(t.text.dim)))
            .into_any_element(),
    }
}

/// A recommended model: what it is, what it is good at, its page, and the
/// button that downloads it. The best fit for this Mac stands out.
fn offer_card(
    model: &'static local_models::Recommended,
    best: bool,
    short_of_disk: bool,
    t: &Theme,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let pretty = local_models::pretty(model.tag);
    let size = model.size_bytes;
    let tag = model.tag;
    let mut meta = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .children(pretty.size.map(|size| caption(size, t)));
    if best {
        meta = meta.child(tinted_tag("Best fit", paint(t.status.running)));
    }
    let download = button(
        SharedString::from(format!("local-download-{tag}")),
        if short_of_disk {
            "Not enough disk".to_owned()
        } else {
            format!("Download {}", format_bytes(size))
        },
    )
    .enabled(!short_of_disk);
    let download = if best { download.primary() } else { download };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(12.0))
        .p(px(16.0))
        .rounded(RADIUS_LG)
        .border_1()
        .border_color(if best {
            alpha(paint(t.status.running), 0.45)
        } else {
            paint(t.border)
        })
        .bg(paint(t.panel))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(logo(tag, px(36.0), t))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .min_w_0()
                        .child(
                            div()
                                .prose()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(paint(t.text.primary))
                                .truncate()
                                .child(pretty.name),
                        )
                        .child(meta),
                ),
        )
        .child(
            div()
                .flex_1()
                .prose()
                .text_size(px(12.5))
                .text_color(paint(t.text.primary))
                .child(model.note),
        )
        .child(
            div().child(
                button(
                    SharedString::from(format!("local-page-{tag}")),
                    "View on ollama.com",
                )
                .link()
                .small()
                .trailing(Icon::ExternalLink)
                .render(t)
                .on_click(move |_, _, cx| cx.open_url(&model_page(tag))),
            ),
        )
        .child(
            download
                .render(t)
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pull_local_model(tag, size, cx);
                })),
        )
        .into_any_element()
}

/// A download in progress: its logo, the bytes so far, the bar, and Cancel.
fn download_row(
    pull: &std::sync::Arc<ollama::Pull>,
    t: &Theme,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let progress_now = pull.progress();
    let fraction = if progress_now.total > 0 {
        progress_now.completed as f32 / progress_now.total as f32
    } else {
        0.0
    };
    let words = if progress_now.total > 0 {
        format!(
            "downloading {} of {}",
            format_bytes(progress_now.completed),
            format_bytes(progress_now.total)
        )
    } else {
        format!("{}\u{2026}", progress_now.status)
    };
    let pretty = local_models::pretty(pull.tag());
    let cancelled = pull.clone();
    div()
        .flex()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .h(px(62.0))
        .border_t_1()
        .border_color(paint(t.border))
        .child(logo(pull.tag(), px(32.0), t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.0))
                        .child(
                            div()
                                .prose()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(paint(t.text.primary))
                                .child(pretty.name),
                        )
                        .children(pretty.size.map(|size| caption(size, t)))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(paint(t.text.dim))
                                .child(words),
                        ),
                )
                .child(progress(fraction).grow().render(t)),
        )
        .child(
            button(
                SharedString::from(format!("local-cancel-{}", pull.tag())),
                "Cancel",
            )
            .small()
            .ghost()
            .render(t)
            .on_click(cx.listener(move |_, _, _, cx| {
                cancelled.cancel();
                cx.notify();
            })),
        )
        .into_any_element()
}

/// The pane's clock: follows running pulls while Settings is open, and when
/// one finishes, says so and looks again. Stops when Settings closes.
async fn tick(shell: WeakEntity<Shell>, cx: &mut gpui::AsyncApp) {
    loop {
        cx.background_executor().timer(TICK).await;
        let open = shell.update(cx, |shell, cx| {
            let Some(view) = shell.preferences.as_ref() else {
                return false;
            };
            if view.section != Section::Agents {
                return true;
            }
            let pulls = ollama::pulls();
            let mut finished = false;
            for pull in pulls.iter().filter(|pull| pull.progress().done) {
                let progress = pull.progress();
                let tag = pull.tag().to_owned();
                let name = local_models::pretty(&tag).name;
                if let Some(error) = progress.error {
                    shell.toast(
                        Tone::Error,
                        format!("Could not download {name}: {error}"),
                        cx,
                    );
                } else if !progress.cancelled {
                    shell.toast(Tone::Success, format!("Downloaded {name}."), cx);
                }
                ollama::forget_pull(&tag);
                finished = true;
            }
            if finished {
                shell.refresh_local_models(true, cx);
            }
            if pulls.iter().any(|pull| !pull.progress().done) {
                cx.notify();
            }
            true
        });
        if !matches!(open, Ok(true)) {
            return;
        }
    }
}
