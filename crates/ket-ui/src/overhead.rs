//! What ket and its agents cost the machine: two read-outs at the status
//! bar's right end, and the popover they open.
//!
//! `ket 358 MB · 0.3% CPU` and `agents 1.9 GB · 1.3% CPU`. Two because they
//! answer different questions: the first is what ket itself costs — the window
//! and the host behind its terminals — and the second is what the work running
//! in them costs. One total would let a build in a terminal read as ket being
//! heavy.
//!
//! **CPU is a share of this Mac**, every core together, though
//! `ket_core::overhead` counts per core as Activity Monitor does. The figures
//! take the same 70/80/90 ramp as the agents' quotas — see
//! [`crate::status_bar::pressure`] — and a ramp that ends at 100% only means
//! something against a figure that does too: per core, a build's ordinary 400%
//! would sit uncoloured beside a quota's red 90%. Memory takes the ramp by its
//! share of this Mac's memory.
//!
//! One exception, in the popover's rows: a session keeping a whole core busy
//! is warm whatever the Mac's share. An agent stuck in a loop costs one core,
//! which on a Mac with many is a few percent and would never colour otherwise.
//!
//! **The popover** charts the last [`HISTORY`] of the total, splits ket into
//! window and host, lists each session by what runs in it and where, and lists
//! the models a local Ollama holds: outside ket's totals, since Ollama.app
//! starts it, but where a local agent's weight really is. While it is open it
//! reads every [`MIN_SPAN`] rather than on the shell's ten-second tick.
//!
//! **It does things, too**, the few that save a trip to Activity Monitor: a
//! session's line goes to its tab, its Stop sends the `Ctrl-C` a person
//! would type there, and a model Ollama holds can be unloaded. Moving the
//! pointer along a chart reads that moment off it in place of the latest.
//!
//! What is counted, and how cheaply, is `ket_core::overhead`'s. Nothing is
//! read while the bar is hidden: a hidden bar is neither built nor fed.
//!
//! **Never a number it does not have.** CPU is a rate, and the first reading
//! has nothing to measure from; until the second, a read-out gives memory
//! alone rather than a `0%` that would say "idle" about nothing.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, Div, FontWeight, MouseMoveEvent, PathBuilder, Pixels, Rgba,
    SharedString, Stateful, Task, Window, canvas, div, fill, point, prelude::*, px, relative, size,
};
use ket_core::activity::Foreground;
use ket_core::id::WorktreeId;
use ket_core::machine;
use ket_core::ollama::LoadedModel;
use ket_core::overhead::{
    HISTORY, MIN_SPAN, Meter, Overhead, Point, Reading, Session, SessionKind,
};
use ket_core::storage::format_bytes;
use ket_core::theme::Theme;

use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::status_bar::{WARM, pressure, tabular};
use crate::tabs::TabKind;
use crate::terminal::TerminalId;
use crate::ui::button::{button, icon_button, pressable};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::popup::{self, Placement};
use crate::ui::row::sized_row;
use crate::ui::toast::Tone;
use crate::ui::tooltip::{Side, tooltip_in_place};
use crate::ui::{CAPTION, LABEL, RADIUS_MD};
use crate::{PopupKind, Shell};

/// The popover's width: a session's line carries what runs in it, where, and
/// two figures.
const POPUP_W: Pixels = px(400.0);

/// Inner padding, matching the Economy popover's.
const PAD: Pixels = px(12.0);

/// Small print: the header's caption, a chart's time axis, notes.
const META: Pixels = px(10.5);

/// A chart's figure: what it reads now, or where the pointer is.
const HERO: Pixels = px(19.0);

/// A group's name: ket, the agents, what is outside.
const GROUP: Pixels = px(12.0);

/// A line's name and figures.
const ITEM: Pixels = px(11.5);

/// A line's height.
const LINE_H: Pixels = px(24.0);

/// The figure columns, right-aligned, so figures line up down the popover.
/// CPU's holds a bar of the line's share of the busiest beside its figure.
const MEMORY_W: Pixels = px(60.0);
const CPU_W: Pixels = px(94.0);
const CPU_FIGURE_W: Pixels = px(42.0);

/// The bar in the CPU column.
const BAR_W: Pixels = px(44.0);
const BAR_H: Pixels = px(4.0);

/// A chart's height.
const CHART_H: Pixels = px(60.0);

/// Where a scale's peak sits on a chart: [`plot`] leaves a quarter of
/// headroom above it.
const PEAK_AT: f32 = 0.8;

/// What a shell at its prompt is called: nothing running, so nothing to stop.
const SHELLS: [&str; 7] = ["zsh", "bash", "fish", "sh", "nu", "dash", "login"];

/// The most sessions listed one by one; the rest are summed on one line.
const MOST_LISTED: usize = 8;

/// How often the popover reads while open: [`MIN_SPAN`], and a little more,
/// so a timer that fires a hair early is not inside the span and turned away.
const WATCH_EVERY: Duration = MIN_SPAN.saturating_add(Duration::from_millis(250));

/// Where Activity Monitor is, for the header's button.
const ACTIVITY_MONITOR: &str = "file:///System/Applications/Utilities/Activity%20Monitor.app";

/// Programs that start agents rather than being one. An agent run through
/// `npx` is `node` to the system; the worktree knows which agent it is for.
const LAUNCHERS: [&str; 11] = [
    "node", "npx", "npm", "bun", "bunx", "deno", "uv", "uvx", "python", "python3", "env",
];

/// The meter, and what the popover asked a local Ollama.
#[derive(Debug, Default)]
pub(crate) struct OverheadItems {
    meter: Meter,
    /// The models a local Ollama holds, as of the popover's last asking.
    ollama: Vec<LoadedModel>,
    /// Whether an asking is under way, so a slow daemon is not asked twice.
    asking: bool,
    /// The popover's quicker reading, while it is open.
    watching: Option<Task<()>>,
    /// Where each chart — memory, then CPU — was last drawn, for the pointer
    /// over it.
    charts: Rc<[Cell<Option<Bounds<Pixels>>>; 2]>,
    /// Which chart the pointer is over, and how far along it, 0 to 1.
    hover: Option<(usize, f32)>,
    /// Models an Unload has been asked for and Ollama has not let go of yet.
    unloading: Vec<String>,
    /// The tooltip the pointer has up: a line's, by its place in the list,
    /// on its name or on its Stop.
    tip: Option<(usize, Tip)>,
}

/// Which of a line's tooltips — see [`OverheadItems::tip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tip {
    /// The name, which goes to the tab.
    Go,
    /// The Stop button.
    Stop,
}

impl Shell {
    /// Takes a reading. Cheap enough for the window's thread — a few
    /// syscalls per process — and asked for only while the bar is open.
    pub(crate) fn refresh_overhead(&mut self) {
        self.overhead.meter.read();
    }

    /// Drops the readings, their baselines and the chart, for a bar that is
    /// closing or about to start reading again.
    pub(crate) fn forget_overhead(&mut self) {
        self.overhead.meter.forget();
    }

    /// The read-outs, which open the popover — or nothing before the first
    /// reading.
    pub(crate) fn overhead_trigger(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overhead = self.overhead.meter.latest()?;
        let mac = Mac::this();
        let open = self.popup == Some(PopupKind::Overhead);
        // Drawn straight on the desk, like the hints beside them.
        let bar = self.theme.on_backdrop();

        let ket = overhead.ket();
        let agents = overhead.agents.reading;
        let trigger = pressable("overhead", &bar)
            .gap(px(14.0))
            .px(px(6.0))
            .mx(px(-6.0))
            .py(px(2.0))
            .when(open, |el| el.bg(paint(bar.hover)))
            .child(readout("ket", ket, mac, &bar))
            // Nothing running is not worth a read-out saying so.
            .when(overhead.agents.processes > 0, |el| {
                el.child(readout("agents", agents, mac, &bar))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.popup == Some(PopupKind::Overhead) {
                    this.popup = None;
                } else {
                    // A popup is the sole non-modal overlay.
                    this.close_palette(cx);
                    this.close_finder(cx);
                    this.menu = None;
                    this.project_menu = None;
                    this.worktree_menu = None;
                    this.dismiss_feedback();
                    this.popup = Some(PopupKind::Overhead);
                    this.watch_overhead(cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element();

        let content = open.then(|| self.overhead_popup(overhead, mac, cx));
        Some(popup::anchored(
            "overhead-popup",
            trigger,
            content,
            Placement::AboveEnd,
            POPUP_W,
            window,
            &self.theme,
        ))
    }

    /// Reads now and every [`WATCH_EVERY`] while the popover is open, asking
    /// Ollama each time too; stops by itself once it closes.
    fn watch_overhead(&mut self, cx: &mut Context<Self>) {
        self.refresh_overhead();
        self.ask_ollama(cx);
        self.overhead.watching = Some(cx.spawn(async move |shell, cx| {
            loop {
                cx.background_executor().timer(WATCH_EVERY).await;
                let open = shell.update(cx, |shell, cx| {
                    let open = shell.status_bar_open && shell.popup == Some(PopupKind::Overhead);
                    if open {
                        shell.refresh_overhead();
                        shell.ask_ollama(cx);
                        cx.notify();
                    }
                    open
                });
                if !matches!(open, Ok(true)) {
                    return;
                }
            }
        }));
    }

    /// Asks a local Ollama what it holds, off the window's thread.
    fn ask_ollama(&mut self, cx: &mut Context<Self>) {
        if self.overhead.asking {
            return;
        }
        self.overhead.asking = true;
        let asked = cx
            .background_executor()
            .spawn(async { ket_core::ollama::loaded_now() });
        cx.spawn(async move |shell, cx| {
            let loaded = asked.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.overhead.asking = false;
                if shell.overhead.ollama != loaded {
                    shell.overhead.ollama = loaded;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn overhead_popup(&self, overhead: &Overhead, mac: Mac, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let places = self.places();

        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(PAD)
            .py(px(9.0))
            .border_b_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(LABEL)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Memory and CPU"),
                    )
                    .child(aside("ket and what it runs · last 10 minutes", t)),
            )
            .child(
                button("overhead-activity-monitor", "Activity Monitor")
                    .link()
                    .small()
                    .trailing(Icon::ExternalLink)
                    .render(t)
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.open_url(ACTIVITY_MONITOR);
                        cx.stop_propagation();
                    })),
            );

        let total = overhead.total();
        let history = self.overhead.meter.history();
        let memory_tone = mac.memory_share(total.memory).and_then(|s| pressure(s, t));
        let cpu_tone = total.cpu.and_then(|cpu| pressure(mac.cpu_share(cpu), t));
        let (memory_now, memory_of) = mac.memory_parts(total.memory);
        let (cpu_now, cpu_of) = total
            .cpu
            .map_or_else(|| ("–".to_owned(), String::new()), |cpu| mac.cpu_parts(cpu));
        let charts = div()
            .flex()
            .gap(px(8.0))
            .px(PAD)
            .pt(px(10.0))
            .pb(px(4.0))
            .child(self.chart_tile(
                0,
                "Memory",
                (memory_now, memory_of),
                Series::of(history, |reading| Some(reading.memory as f32)),
                memory_tone.unwrap_or_else(|| memory_ink(t)),
                |value, _| format_bytes(value as u64),
                cx,
            ))
            .child(self.chart_tile(
                1,
                "CPU",
                (cpu_now, cpu_of),
                Series::of(history, |reading| reading.cpu.map(|cpu| mac.cpu_share(cpu))),
                cpu_tone.unwrap_or_else(|| cpu_ink(t)),
                |value, _| percent(value),
                cx,
            ));

        let ket = overhead.ket();
        let sessions = listed(&overhead.agents.sessions);
        // A full bar is a whole core busy, or the busiest line once one
        // passes that — see `Scale`.
        let busiest = sessions
            .iter()
            .take(MOST_LISTED)
            .filter_map(|session| session.reading.cpu)
            .chain(overhead.window.cpu)
            .chain(overhead.host.and_then(|host| host.cpu))
            .map(|cpu| mac.cpu_share(cpu))
            .fold(0.0_f32, f32::max);
        let scale = Scale { mac, busiest };

        let mut rows = div()
            .flex()
            .flex_col()
            .px(PAD)
            .pb(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(20.0))
                    .gap(px(8.0))
                    .text_size(META)
                    .text_color(paint(t.text.dim))
                    .child(div().flex_1())
                    .child(div().w(MEMORY_W).text_right().child("Memory"))
                    .child(div().w(CPU_W).text_right().child("CPU")),
            )
            .child(group("ket", ket, scale, t))
            .child(line(
                sized_icon(Icon::TerminalWindow, px(13.0), paint(t.text.dim)),
                "Window",
                None,
                overhead.window,
                scale,
                None,
                None,
                t,
            ))
            .children(overhead.host.map(|host| {
                line(
                    sized_icon(Icon::Terminal, px(13.0), paint(t.text.dim)),
                    "Terminal host",
                    None,
                    host,
                    scale,
                    None,
                    None,
                    t,
                )
            }))
            .child(div().mt(px(6.0)).child(group(
                "Agents and terminals",
                overhead.agents.reading,
                scale,
                t,
            )));

        if sessions.is_empty() {
            rows = rows.child(
                div()
                    .h(LINE_H)
                    .flex()
                    .items_center()
                    .child(aside("Nothing running", t)),
            );
        }
        for (index, session) in sessions.iter().take(MOST_LISTED).enumerate() {
            let (name, place) = label(session, &places);
            let tab = self.session_tab(session, &places);
            let stoppable = tab.is_some() && !SHELLS.contains(&session.program.as_str());
            let tip = self
                .overhead
                .tip
                .filter(|(line, _)| *line == index)
                .map(|(_, tip)| tip);
            let stop = tab.clone().filter(|_| stoppable).map(|(_, terminal)| {
                let button = icon_button(("overhead-stop", index), Icon::Stop)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.interrupt_terminal(terminal, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .into_any_element();
                // In place, not deferred: the popover is already a deferred
                // surface, and `gpui` aborts the frame on a second level.
                tooltip_in_place(
                    ("overhead-stop-tip", index),
                    button,
                    format!("Stop {name} (Ctrl-C)"),
                    Side::Top,
                    tip == Some(Tip::Stop),
                    t,
                )
                .opacity(0.0)
                .group_hover(
                    SharedString::from(format!("overhead-line-{index}")),
                    |style| style.opacity(1.0),
                )
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    // Back to the name's label on the way out: the pointer
                    // is still on the line.
                    let next = if *hovered { Tip::Stop } else { Tip::Go };
                    if this.overhead.tip != Some((index, next)) {
                        this.overhead.tip = Some((index, next));
                        cx.notify();
                    }
                }))
                .into_any_element()
            });
            let go_label = format!("Go to {name}'s tab");
            let wrap_name: Option<Box<dyn FnOnce(AnyElement) -> Stateful<Div> + '_>> =
                tab.as_ref().map(|_| {
                    let shown = tip == Some(Tip::Go);
                    Box::new(move |words: AnyElement| {
                        tooltip_in_place(
                            ("overhead-go-tip", index),
                            words,
                            go_label,
                            Side::Top,
                            shown,
                            t,
                        )
                    }) as Box<dyn FnOnce(AnyElement) -> Stateful<Div> + '_>
                });
            let body = line(
                crate::ui::agent::mark(&name, t),
                name,
                place,
                session.reading,
                scale,
                stop,
                wrap_name,
                t,
            );
            rows = rows.child(match tab {
                Some((worktree, terminal)) => sized_row(("overhead-line", index), false, LINE_H, t)
                    .group(SharedString::from(format!("overhead-line-{index}")))
                    .mx(px(-6.0))
                    .px(px(6.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.go_to_session(&worktree, terminal, cx);
                        cx.stop_propagation();
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let mine = this.overhead.tip.is_some_and(|(line, _)| line == index);
                        if *hovered && !mine {
                            this.overhead.tip = Some((index, Tip::Go));
                            cx.notify();
                        } else if !*hovered && mine {
                            this.overhead.tip = None;
                            cx.notify();
                        }
                    }))
                    .child(body.flex_1().min_w_0())
                    .into_any_element(),
                None => body.into_any_element(),
            });
        }
        if let Some(rest) = rest(&sessions) {
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .h(LINE_H)
                    .gap(px(8.0))
                    .text_size(ITEM)
                    .text_color(paint(t.text.dim))
                    .child(div().flex_1().child(format!("{} more", rest.count)))
                    .child(figure(format_bytes(rest.memory), MEMORY_W, t.text.dim))
                    .child(div().w(CPU_W)),
            );
        }

        if !self.asleep_terminals.is_empty() {
            let count = self.asleep_terminals.len();
            let freed: u64 = self
                .asleep_terminals
                .values()
                .map(|tab| tab.freed_bytes)
                .sum();
            let summary = if freed > 0 {
                format!("Asleep: {count} agents · {} freed", format_bytes(freed))
            } else {
                format!("Asleep: {count} agents")
            };
            rows = rows.child(
                div()
                    .mt(px(7.0))
                    .h(LINE_H)
                    .flex()
                    .items_center()
                    .text_size(ITEM)
                    .text_color(paint(t.text.dim))
                    .child(summary),
            );
            let asleep: Vec<_> = self
                .asleep_terminals
                .iter()
                .map(|(id, tab)| (*id, tab.agent.clone()))
                .collect();
            for (id, agent) in asleep {
                rows = rows.child(
                    sized_row(("overhead-asleep", id.0), false, LINE_H, t)
                        .child(sized_icon(Icon::Moon, px(13.0), paint(t.text.dim)))
                        .child(div().flex_1().min_w_0().child(agent))
                        .child(
                            button(("overhead-wake", id.0), "Wake")
                                .small()
                                .render(t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.wake_terminal(id, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                );
            }
        }

        // An `ollama serve` in one of ket's own terminals is in that
        // session's tree already, its models with it: listing them again
        // would count them twice.
        let served_inside = overhead
            .agents
            .sessions
            .iter()
            .any(|session| session.program == "ollama");
        if !self.overhead.ollama.is_empty() && !served_inside {
            rows = rows
                .child(
                    div()
                        .mt(px(6.0))
                        .flex()
                        .items_center()
                        .h(px(26.0))
                        .text_size(GROUP)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(paint(t.text.dim))
                        .child("Outside ket"),
                )
                .children(
                    self.overhead
                        .ollama
                        .iter()
                        .enumerate()
                        .map(|(index, model)| {
                            let tag = model.tag.clone();
                            let unloading = self.overhead.unloading.contains(&tag);
                            let unload = button(
                                ("overhead-unload", index),
                                if unloading { "Unloading…" } else { "Unload" },
                            )
                            .small()
                            .enabled(!unloading)
                            .render(t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.unload_model(tag.clone(), cx);
                                cx.stop_propagation();
                            }))
                            .into_any_element();
                            line(
                                crate::ui::agent::mark("ollama", t),
                                "Ollama",
                                Some(model.tag.clone()),
                                Reading {
                                    memory: model.size_bytes,
                                    cpu: None,
                                },
                                scale,
                                Some(unload),
                                None,
                                t,
                            )
                        }),
                )
                .child(div().pl(px(22.0)).child(aside(
                    "Holds your local agents' models. Not in the totals above.",
                    t,
                )));
        }

        div()
            .flex()
            .flex_col()
            // Words and figures to read off, as the other popovers are.
            .prose()
            .child(header)
            .child(charts)
            .child(rows)
            .into_any_element()
    }

    /// One chart: its name, the figure it reads — now, or where the pointer
    /// is along it — and the line of the last [`HISTORY`].
    #[allow(clippy::too_many_arguments)]
    fn chart_tile(
        &self,
        which: usize,
        name: &'static str,
        (now, of): (String, String),
        series: Series,
        ink: ket_core::theme::Color,
        say: fn(f32, Mac) -> String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let mac = Mac::this();
        let hovered = self
            .overhead
            .hover
            .filter(|(chart, _)| *chart == which)
            .and_then(|(_, along)| series.nearest(along));
        let (figure_text, caption) = match hovered {
            Some(index) => (say(series.values[index], mac), ago(series.ages[index])),
            None => (now, of),
        };
        let peak = (series.values.len() > 1).then(|| format!("peak {}", say(series.peak, mac)));
        let bounds = self.overhead.charts.clone();
        let ground = paint(t.sunken);
        let line_ink = paint(ink);
        let points = series.points.clone();
        let rule = paint(t.selection);
        let cursor = paint(t.text.dim);

        let plot = canvas(
            move |bounds_now, _, _| bounds[which].set(Some(bounds_now)),
            move |bounds, _, window, _| {
                paint_chart(
                    window,
                    bounds,
                    &points,
                    hovered,
                    ChartInk {
                        line: line_ink,
                        ground,
                        rule,
                        cursor,
                    },
                );
            },
        )
        .w_full()
        .h(CHART_H);

        div()
            .id(("overhead-chart", which))
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(10.0))
            .pt(px(9.0))
            .pb(px(7.0))
            .rounded(RADIUS_MD)
            .bg(paint(t.sunken))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let Some(bounds) = this.overhead.charts[which].get() else {
                    return;
                };
                let width = f32::from(bounds.size.width);
                if width <= 0.0 {
                    return;
                }
                let along = (f32::from(event.position.x - bounds.origin.x) / width).clamp(0.0, 1.0);
                if this.overhead.hover != Some((which, along)) {
                    this.overhead.hover = Some((which, along));
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered && this.overhead.hover.is_some_and(|(chart, _)| chart == which) {
                    this.overhead.hover = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(CAPTION)
                    .text_color(paint(t.text.dim))
                    .child(div().size(px(7.0)).rounded(px(3.5)).bg(line_ink))
                    .child(div().flex_1().child(name))
                    .children(peak.map(|peak| tabular(div()).text_size(META).child(peak))),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .min_w_0()
                    .child(
                        tabular(div())
                            .flex_none()
                            .text_size(HERO)
                            .line_height(px(22.0))
                            .text_color(paint(t.text.primary))
                            .child(figure_text),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(CAPTION)
                            .text_color(paint(t.text.dim))
                            .child(caption),
                    ),
            )
            .child(plot)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(META)
                    .text_color(paint(t.text.dim))
                    .child("10 min ago")
                    .child("now"),
            )
            .into_any_element()
    }

    /// The terminal a session runs in, and its worktree: the tab in that
    /// worktree whose job is the session's program, or that was started with
    /// its agent, or — for an agent — the worktree's agent tab. `None` for a
    /// session outside every worktree, or one with no tab.
    fn session_tab(&self, session: &Session, places: &[Place]) -> Option<(WorktreeId, TerminalId)> {
        let cwd = session.cwd.as_deref()?;
        let place = place(cwd, places)?;
        let space = self.spaces.get(&place.id)?;
        let terminals: Vec<TerminalId> = space
            .all_tabs()
            .into_iter()
            .filter_map(|tab| match tab.kind {
                TabKind::Terminal(id) => Some(id),
                _ => None,
            })
            .collect();
        let (name, _) = label(session, places);
        let runs = |id: &TerminalId| {
            self.terminals.get(id).is_some_and(|handle| {
                matches!(handle.term.foreground(), Some(Foreground::Running(ref program)) if *program == session.program)
                    || handle.agent.as_deref() == Some(name.as_str())
            })
        };
        terminals
            .iter()
            .copied()
            .find(runs)
            .or_else(|| {
                self.agent_terminal(&place.id)
                    .filter(|_| session.kind == SessionKind::Agent)
            })
            .or_else(|| terminals.first().copied())
            .map(|terminal| (place.id.clone(), terminal))
    }

    /// Goes to a session's tab, and puts the popover away.
    fn go_to_session(
        &mut self,
        worktree: &WorktreeId,
        terminal: TerminalId,
        cx: &mut Context<Self>,
    ) {
        self.popup = None;
        self.switch_worktree_id(worktree, cx);
        if let Some(space) = self.spaces.get_mut(worktree) {
            space.focus_terminal(terminal);
        }
        cx.notify();
    }

    /// Asks Ollama to let go of `tag`, off the window's thread, and reads
    /// what it holds again once it has.
    fn unload_model(&mut self, tag: String, cx: &mut Context<Self>) {
        if self.overhead.unloading.contains(&tag) {
            return;
        }
        self.overhead.unloading.push(tag.clone());
        cx.notify();
        let asked = tag.clone();
        let unloaded = cx
            .background_executor()
            .spawn(async move { ket_core::ollama::unload(&asked) });
        cx.spawn(async move |shell, cx| {
            let result = unloaded.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.overhead.unloading.retain(|held| *held != tag);
                if let Err(error) = result {
                    shell.toast(Tone::Error, format!("Could not unload {tag}: {error}"), cx);
                }
                shell.ask_ollama(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Every worktree's directory, name and agent, to say where a session
    /// runs.
    fn places(&self) -> Vec<Place> {
        self.projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .map(|node| Place {
                id: node.id.clone(),
                path: node.path.clone(),
                name: node.name.clone().unwrap_or_else(|| node.branch.clone()),
                agent: node.agent.clone(),
            })
            .collect()
    }
}

impl OverheadItems {
    /// The latest footprint attributed to a terminal's process tree.
    pub(crate) fn terminal_memory(&self, pid: u32) -> Option<u64> {
        self.meter
            .latest()?
            .agents
            .sessions
            .iter()
            .find(|session| session.root_pid == pid as i32)
            .map(|session| session.reading.memory)
    }
}

/// What the figures are measured against: this Mac's cores and memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mac {
    cores: usize,
    /// `None` where ket cannot tell, which leaves memory uncoloured.
    memory: Option<u64>,
}

impl Mac {
    fn this() -> Self {
        Self {
            cores: machine::cores(),
            memory: machine::total_memory_bytes(),
        }
    }

    /// A per-core CPU figure as a share of every core.
    fn cpu_share(self, cpu: f32) -> f32 {
        cpu / self.cores.max(1) as f32
    }

    /// Bytes as a share of this Mac's memory.
    fn memory_share(self, bytes: u64) -> Option<f32> {
        self.memory
            .filter(|total| *total > 0)
            .map(|total| (bytes as f64 / total as f64 * 100.0) as f32)
    }

    /// A chart's figure for memory, and what it is out of: `1.9 GB`,
    /// `of 64 GB`.
    fn memory_parts(self, bytes: u64) -> (String, String) {
        (
            format_bytes(bytes),
            self.memory
                .map_or_else(String::new, |total| format!("of {}", format_bytes(total))),
        )
    }

    /// A chart's figure for CPU, and how many cores' worth: `5.0%`,
    /// `0.9 of 18 cores`.
    fn cpu_parts(self, cpu: f32) -> (String, String) {
        (
            percent(self.cpu_share(cpu)),
            format!("{:.1} of {} cores", cpu / 100.0, self.cores),
        )
    }
}

/// The memory chart's colour below the ramp: the theme's cyan, taken a
/// quarter of the way to the chart's ground so it sits as low as the CPU
/// chart's violet — the pair checked for colour-blind separation on the
/// shipped dark theme.
fn memory_ink(t: &Theme) -> ket_core::theme::Color {
    t.terminal.ansi.cyan.mix(t.sunken, 0.28)
}

/// The CPU chart's colour below the ramp — see [`memory_ink`].
fn cpu_ink(t: &Theme) -> ket_core::theme::Color {
    t.terminal.ansi.magenta.mix(t.sunken, 0.2)
}

/// `4 min ago`, `just now`: how long ago a chart's point was read.
fn ago(seconds: f32) -> String {
    let minutes = (seconds / 60.0).round();
    if seconds < 30.0 {
        "just now".to_owned()
    } else if minutes < 1.5 {
        "1 min ago".to_owned()
    } else {
        format!("{minutes:.0} min ago")
    }
}

/// What a chart draws, and what the pointer reads off it.
#[derive(Debug, Clone, Default)]
struct Series {
    /// See [`plot`].
    points: Vec<(f32, f32)>,
    /// Each point's figure, in the same order.
    values: Vec<f32>,
    /// How long before the newest each point was read, in seconds.
    ages: Vec<f32>,
    /// The highest figure.
    peak: f32,
}

impl Series {
    fn of<'a>(
        history: impl IntoIterator<Item = &'a Point> + Clone,
        value: impl Fn(Reading) -> Option<f32>,
    ) -> Self {
        let read: Vec<(Instant, f32)> = history
            .clone()
            .into_iter()
            .filter_map(|point| Some((point.at, value(point.reading)?)))
            .collect();
        let newest = read.last().map(|(at, _)| *at);
        Self {
            points: plot(history, &value),
            values: read.iter().map(|(_, v)| *v).collect(),
            ages: read
                .iter()
                .map(|(at, _)| {
                    newest.map_or(0.0, |newest| {
                        newest.saturating_duration_since(*at).as_secs_f32()
                    })
                })
                .collect(),
            peak: read.iter().map(|(_, v)| *v).fold(0.0_f32, f32::max),
        }
    }

    /// The point nearest `along`, 0 to 1 across the chart.
    fn nearest(&self, along: f32) -> Option<usize> {
        self.points
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (a.0 - along).abs().total_cmp(&(b.0 - along).abs()))
            .map(|(index, _)| index)
    }
}

/// The colours a chart is drawn in.
#[derive(Debug, Clone, Copy)]
struct ChartInk {
    line: Rgba,
    /// The tile's own, for the ring around a dot.
    ground: Rgba,
    /// The peak and the floor.
    rule: Rgba,
    /// The pointer's line.
    cursor: Rgba,
}

/// Draws a chart in `bounds`: the floor, a hairline at the peak, a wash and a
/// smooth line through `points`, a dot on the newest, and the pointer's line
/// and dot when it is on the chart.
fn paint_chart(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    points: &[(f32, f32)],
    hovered: Option<usize>,
    ink: ChartInk,
) {
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let bottom = top + height;
    // Room for a dot's ring at either edge.
    let inset = 5.0;
    let at = |(x, y): (f32, f32)| {
        (
            left + width * x,
            top + inset + (height - inset * 2.0) * (1.0 - y),
        )
    };

    let mut floor = PathBuilder::stroke(px(1.0));
    floor.move_to(point(px(left), px(bottom - 0.5)));
    floor.line_to(point(px(left + width), px(bottom - 0.5)));
    if let Ok(path) = floor.build() {
        window.paint_path(path, ink.rule);
    }
    if points.len() < 2 {
        return;
    }
    let (_, peak_y) = at((0.0, PEAK_AT));
    let mut peak = PathBuilder::stroke(px(1.0)).dash_array(&[px(2.0), px(3.0)]);
    peak.move_to(point(px(left), px(peak_y)));
    peak.line_to(point(px(left + width), px(peak_y)));
    if let Ok(path) = peak.build() {
        window.paint_path(path, ink.rule);
    }

    // A Catmull-Rom curve through every point, its handles kept inside the
    // plot so the curve never overshoots below the floor.
    let spots: Vec<(f32, f32)> = points.iter().map(|&p| at(p)).collect();
    let clamp = |y: f32| y.clamp(top + inset, bottom - inset);
    let curve = |builder: &mut PathBuilder| {
        for i in 0..spots.len() - 1 {
            let p0 = spots[i.saturating_sub(1)];
            let p1 = spots[i];
            let p2 = spots[i + 1];
            let p3 = spots[(i + 2).min(spots.len() - 1)];
            let c1 = (
                p1.0 + (p2.0 - p0.0) / 6.0,
                clamp(p1.1 + (p2.1 - p0.1) / 6.0),
            );
            let c2 = (
                p2.0 - (p3.0 - p1.0) / 6.0,
                clamp(p2.1 - (p3.1 - p1.1) / 6.0),
            );
            builder.cubic_bezier_to(
                point(px(p2.0), px(p2.1)),
                point(px(c1.0), px(c1.1)),
                point(px(c2.0), px(c2.1)),
            );
        }
    };
    let first = spots[0];
    let last = spots[spots.len() - 1];

    let mut wash = PathBuilder::fill();
    wash.move_to(point(px(first.0), px(bottom)));
    wash.line_to(point(px(first.0), px(first.1)));
    curve(&mut wash);
    wash.line_to(point(px(last.0), px(bottom)));
    wash.close();
    if let Ok(path) = wash.build() {
        window.paint_path(path, alpha(ink.line, 0.16));
    }

    let mut stroke = PathBuilder::stroke(px(2.0));
    stroke.move_to(point(px(first.0), px(first.1)));
    curve(&mut stroke);
    if let Ok(path) = stroke.build() {
        window.paint_path(path, ink.line);
    }

    let dot = |window: &mut Window, (x, y): (f32, f32)| {
        window.paint_quad(
            fill(
                Bounds::new(point(px(x - 5.0), px(y - 5.0)), size(px(10.0), px(10.0))),
                ink.ground,
            )
            .corner_radii(px(5.0)),
        );
        window.paint_quad(
            fill(
                Bounds::new(point(px(x - 3.5), px(y - 3.5)), size(px(7.0), px(7.0))),
                ink.line,
            )
            .corner_radii(px(3.5)),
        );
    };
    if let Some(index) = hovered.filter(|&index| index < spots.len()) {
        let (x, y) = spots[index];
        let mut cursor = PathBuilder::stroke(px(1.0));
        cursor.move_to(point(px(x), px(top)));
        cursor.line_to(point(px(x), px(bottom)));
        if let Ok(path) = cursor.build() {
            window.paint_path(path, ink.cursor);
        }
        dot(window, (x, y));
    }
    dot(window, last);
}

/// What the CPU column's bars are measured against.
#[derive(Debug, Clone, Copy)]
///
/// **A full bar is one core kept busy**, or the busiest line once something
/// passes that. Measured against the busiest line alone, a window idling at
/// 1% filled its bar and read as maxed out; against the whole Mac, a runaway
/// agent pinning a core would be a sliver. One core is what a loop costs, so
/// that is where the bar fills — and the quota ramp colours anything heavier.
struct Scale {
    mac: Mac,
    /// The busiest line's share of the Mac.
    busiest: f32,
}

impl Scale {
    /// How full a line's bar is, 0 to 1, or `None` before it has a rate.
    fn of(self, reading: Reading) -> Option<f32> {
        let share = self.mac.cpu_share(reading.cpu?);
        let core = self.mac.cpu_share(100.0);
        let full = self.busiest.max(core);
        Some(if full > 0.0 {
            (share / full).clamp(0.0, 1.0)
        } else {
            0.0
        })
    }
}

/// `0.4%`, `23%`: one decimal while it is small enough for a tenth to matter,
/// none once it is not, as sizes are — see [`format_bytes`].
fn percent(share: f32) -> String {
    if share < 10.0 {
        format!("{share:.1}%")
    } else {
        format!("{share:.0}%")
    }
}

/// `558 MB · 0.3% CPU`, or `558 MB` before there is a rate.
fn figures(reading: Reading, mac: Mac) -> String {
    match reading.cpu {
        Some(cpu) => format!(
            "{} · {} CPU",
            format_bytes(reading.memory),
            percent(mac.cpu_share(cpu))
        ),
        None => format_bytes(reading.memory),
    }
}

/// How far up the quota ramp a reading is: the worse of its CPU and memory
/// shares of this Mac.
fn level(reading: Reading, mac: Mac) -> f32 {
    let cpu = reading.cpu.map_or(0.0, |cpu| mac.cpu_share(cpu));
    let memory = mac.memory_share(reading.memory).unwrap_or(0.0);
    cpu.max(memory)
}

/// [`level`] for one session's CPU: its share of the Mac, or warm at least
/// once it keeps a whole core busy — see the module doc.
fn session_cpu_level(cpu: f32, mac: Mac) -> f32 {
    let share = mac.cpu_share(cpu);
    if cpu >= 100.0 { share.max(WARM) } else { share }
}

/// A word and its figures on the status bar, the figures coloured once they
/// climb the ramp.
fn readout(word: &'static str, reading: Reading, mac: Mac, t: &Theme) -> Div {
    let ink = pressure(level(reading, mac), t).unwrap_or(t.text.primary);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .child(div().prose().child(word))
        .child(tabular(div().text_color(paint(ink))).child(figures(reading, mac)))
}

/// Where each reading in `history` falls on a chart [`HISTORY`] wide, the
/// newest at the right edge: `x` from 0, that long ago, to 1, now; `y` from 0
/// to 1 of a scale with room above the highest point. Readings `value` has no
/// figure for are left out, and the line joins across them.
fn plot<'a>(
    history: impl IntoIterator<Item = &'a Point>,
    value: &impl Fn(Reading) -> Option<f32>,
) -> Vec<(f32, f32)> {
    let points: Vec<(Instant, f32)> = history
        .into_iter()
        .filter_map(|point| Some((point.at, value(point.reading)?)))
        .collect();
    let Some(&(newest, _)) = points.last() else {
        return Vec::new();
    };
    let peak = points.iter().map(|(_, v)| *v).fold(0.0_f32, f32::max);
    // A quarter of headroom, so a line at its peak is not drawn along the
    // top edge; never a zero scale, so a flat zero is a line along the floor.
    let scale = (peak * 1.25).max(f32::EPSILON);
    let span = HISTORY.as_secs_f32();
    points
        .into_iter()
        .map(|(at, v)| {
            let age = newest.saturating_duration_since(at).as_secs_f32();
            (1.0 - (age / span).min(1.0), (v / scale).clamp(0.0, 1.0))
        })
        .collect()
}

/// A group's line: its name and what it adds up to.
fn group(name: &'static str, reading: Reading, scale: Scale, t: &Theme) -> Div {
    let ink = pressure(level(reading, scale.mac), t).unwrap_or(t.text.primary);
    div()
        .flex()
        .items_center()
        .h(px(26.0))
        .gap(px(8.0))
        .text_size(GROUP)
        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(name))
        .child(figure(format_bytes(reading.memory), MEMORY_W, ink))
        .child(cpu_cell(reading, scale, ink, cpu_ink(t), t))
}

/// One line under a group: a mark, a name, where it runs, what can be done
/// about it, and its figures, each coloured on its own once it climbs the
/// ramp.
#[allow(clippy::too_many_arguments)]
fn line(
    mark: gpui::Svg,
    name: impl Into<SharedString>,
    place: Option<String>,
    reading: Reading,
    scale: Scale,
    action: Option<AnyElement>,
    wrap_name: Option<Box<dyn FnOnce(AnyElement) -> Stateful<Div> + '_>>,
    t: &Theme,
) -> Div {
    let mac = scale.mac;
    let memory_ink = mac
        .memory_share(reading.memory)
        .and_then(|share| pressure(share, t))
        .unwrap_or(t.text.dim);
    let hot = reading
        .cpu
        .and_then(|cpu| pressure(session_cpu_level(cpu, mac), t));
    let busy = reading.cpu.is_some_and(|cpu| mac.cpu_share(cpu) >= 0.05);
    let cpu_ink_text = hot.unwrap_or(if busy { t.text.primary } else { t.text.dim });
    div()
        .flex()
        .items_center()
        .h(LINE_H)
        .gap(px(8.0))
        .text_size(ITEM)
        .child(
            div()
                .flex_none()
                .size(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .child(mark),
        )
        .child({
            let words =
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(paint(t.text.dim))
                    .child(
                        // The name stays whole; where it runs gives way, with an
                        // ellipsis, before the figures do.
                        div()
                            .flex()
                            .min_w_0()
                            .overflow_hidden()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(paint(t.text.primary))
                                    .child(name.into()),
                            )
                            .children(place.map(|place| {
                                div().min_w_0().truncate().child(format!("· {place}"))
                            })),
                    );
            // Content-sized when it carries a tooltip, so the label centres
            // over the words rather than over the row's empty middle.
            match wrap_name {
                Some(wrap) => div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .child(
                        wrap(words.into_any_element())
                            .flex_shrink()
                            .min_w_0()
                            .max_w_full(),
                    )
                    .into_any_element(),
                None => words.flex_1().into_any_element(),
            }
        })
        .children(action)
        .child(figure(format_bytes(reading.memory), MEMORY_W, memory_ink))
        .child(cpu_cell(
            reading,
            scale,
            cpu_ink_text,
            hot.unwrap_or_else(|| cpu_ink(t)),
            t,
        ))
}

/// The CPU column: a bar of the line's share of the busiest, and its figure.
fn cpu_cell(
    reading: Reading,
    scale: Scale,
    ink: ket_core::theme::Color,
    bar: ket_core::theme::Color,
    t: &Theme,
) -> Div {
    let full = scale.of(reading);
    // Anything its figure shows as running shows here too, however little:
    // a sliver, not an empty track. A figure that reads 0.0% gets none.
    let shown = reading
        .cpu
        .is_some_and(|cpu| scale.mac.cpu_share(cpu) >= 0.05);
    let width = full.map_or(0.0, |full| {
        if shown {
            f32::from(BAR_W) * full.max(0.06)
        } else {
            0.0
        }
    });
    div()
        .flex_none()
        .w(CPU_W)
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .child(
            div()
                .w(BAR_W)
                .h(BAR_H)
                .rounded(BAR_H / 2.0)
                .bg(paint(t.selection))
                .child(
                    div()
                        .w(px(width))
                        .h(BAR_H)
                        .rounded(BAR_H / 2.0)
                        .bg(paint(bar)),
                ),
        )
        .child(figure(cpu_text(reading, scale.mac), CPU_FIGURE_W, ink))
}

/// A figure in a column.
fn figure(text: impl Into<SharedString>, width: Pixels, ink: ket_core::theme::Color) -> Div {
    tabular(div())
        .flex_none()
        .w(width)
        .text_right()
        .text_color(paint(ink))
        .child(text.into())
}

/// A reading's CPU as a share of the Mac, or a dash before it has one.
fn cpu_text(reading: Reading, mac: Mac) -> String {
    reading
        .cpu
        .map_or_else(|| "–".to_owned(), |cpu| percent(mac.cpu_share(cpu)))
}

/// Small print.
fn aside(text: impl Into<SharedString>, t: &Theme) -> AnyElement {
    div()
        .text_size(META)
        .line_height(relative(1.45))
        .text_color(paint(t.text.dim))
        .child(text.into())
        .into_any_element()
}

/// A worktree, for placing a session.
#[derive(Debug, Clone)]
struct Place {
    id: WorktreeId,
    path: PathBuf,
    name: SharedString,
    agent: Option<SharedString>,
}

/// The sessions busiest first: by CPU, those with none yet last, then by
/// memory.
fn listed(sessions: &[Session]) -> Vec<&Session> {
    let mut listed: Vec<&Session> = sessions.iter().collect();
    listed.sort_by(|a, b| {
        let cpu = |s: &Session| s.reading.cpu.unwrap_or(-1.0);
        cpu(b)
            .total_cmp(&cpu(a))
            .then(b.reading.memory.cmp(&a.reading.memory))
    });
    listed
}

/// The sessions past [`MOST_LISTED`], summed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rest {
    count: usize,
    memory: u64,
}

fn rest(listed: &[&Session]) -> Option<Rest> {
    let rest = listed.get(MOST_LISTED..).filter(|rest| !rest.is_empty())?;
    Some(Rest {
        count: rest.len(),
        memory: rest.iter().map(|session| session.reading.memory).sum(),
    })
}

/// What a session's line says: what runs in it, and where.
///
/// Where is the worktree its directory is in, by that worktree's name, or
/// the directory's own name outside every worktree. What runs is the program,
/// except an agent started through a launcher, which is the worktree's agent.
fn label(session: &Session, places: &[Place]) -> (String, Option<String>) {
    let place = session.cwd.as_deref().and_then(|cwd| place(cwd, places));
    let program = session.program.as_str();
    let name = match (session.kind, place) {
        (SessionKind::Agent, Some(place)) if LAUNCHERS.contains(&program) => place
            .agent
            .as_ref()
            .map_or_else(|| program.to_owned(), ToString::to_string),
        _ if program.is_empty() => "process".to_owned(),
        _ => program.to_owned(),
    };
    let where_ = match place {
        Some(place) => Some(place.name.to_string()),
        None => session
            .cwd
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned()),
    };
    (name, where_)
}

/// The worktree `cwd` is in: the deepest whose directory holds it. The system
/// reports a directory under `/tmp` or `/var` by its real `/private` path,
/// which a worktree made there was not.
fn place<'a>(cwd: &Path, places: &'a [Place]) -> Option<&'a Place> {
    let bare = cwd
        .strip_prefix("/private")
        .ok()
        .map(|rest| Path::new("/").join(rest));
    places
        .iter()
        .filter(|place| {
            cwd.starts_with(&place.path)
                || bare
                    .as_deref()
                    .is_some_and(|bare| bare.starts_with(&place.path))
        })
        .max_by_key(|place| place.path.components().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;

    const MAC: Mac = Mac {
        cores: 18,
        memory: Some(64 * GB),
    };

    fn reading(memory: u64, cpu: Option<f32>) -> Reading {
        Reading { memory, cpu }
    }

    fn session(kind: SessionKind, program: &str, cwd: &str, cpu: Option<f32>) -> Session {
        Session {
            root_pid: 0,
            kind,
            program: program.to_owned(),
            cwd: Some(PathBuf::from(cwd)),
            reading: reading(100 * MB, cpu),
            processes: 1,
        }
    }

    fn places() -> Vec<Place> {
        vec![
            Place {
                id: WorktreeId::new("main"),
                path: PathBuf::from("/Users/me/dev/ket"),
                name: "main".into(),
                agent: None,
            },
            Place {
                id: WorktreeId::new("fix-tabs"),
                path: PathBuf::from("/Users/me/.local/share/ket/worktrees/ket/fix-tabs"),
                name: "Fix tabs".into(),
                agent: Some("codex".into()),
            },
        ]
    }

    #[test]
    fn a_small_share_keeps_its_tenth_and_a_large_one_does_not() {
        assert_eq!(percent(0.42), "0.4%");
        assert_eq!(percent(9.94), "9.9%");
        assert_eq!(percent(23.4), "23%");
    }

    #[test]
    fn cpu_reads_as_a_share_of_every_core() {
        assert_eq!(
            figures(reading(558 * MB, Some(18.0)), MAC),
            "558 MB · 1.0% CPU"
        );
        assert_eq!(
            figures(reading(558 * MB, None), MAC),
            "558 MB",
            "no rate yet"
        );
        assert_eq!(MAC.cpu_parts(29.0).1, "0.3 of 18 cores");
    }

    #[test]
    fn memory_reads_against_the_whole_mac() {
        assert_eq!(
            MAC.memory_parts(2 * GB),
            ("2.0 GB".to_owned(), "of 64 GB".to_owned())
        );
        let unknown = Mac {
            memory: None,
            ..MAC
        };
        assert_eq!(unknown.memory_parts(2 * GB).1, "");
        assert_eq!(unknown.memory_share(2 * GB), None);
    }

    #[test]
    fn the_ramp_takes_the_worse_of_cpu_and_memory() {
        // 90% of the cores, 3% of the memory.
        assert!((level(reading(2 * GB, Some(1_620.0)), MAC) - 90.0).abs() < 0.01);
        // 75% of the memory, idle.
        assert!((level(reading(48 * GB, Some(0.0)), MAC) - 75.0).abs() < 0.01);
    }

    #[test]
    fn a_session_keeping_a_core_busy_is_warm_on_any_mac() {
        // One core of eighteen is under 6% of the Mac: warm all the same.
        assert!(session_cpu_level(100.0, MAC) >= WARM);
        assert!(session_cpu_level(99.0, MAC) < WARM);
        // A build across most of the Mac climbs past warm on its own.
        assert!(session_cpu_level(1_700.0, MAC) > 90.0);
    }

    #[test]
    fn the_chart_ends_at_the_newest_reading_and_spans_ten_minutes() {
        let start = Instant::now();
        let history = [
            Point {
                at: start,
                reading: reading(100, None),
            },
            Point {
                at: start + HISTORY / 2,
                reading: reading(100, Some(50.0)),
            },
            Point {
                at: start + HISTORY,
                reading: reading(200, Some(100.0)),
            },
        ];
        let memory = plot(&history, &|r| Some(r.memory as f32));
        assert_eq!(memory.len(), 3);
        assert!((memory[0].0 - 0.0).abs() < 1e-6);
        assert!((memory[1].0 - 0.5).abs() < 1e-6);
        assert!((memory[2].0 - 1.0).abs() < 1e-6);
        // The peak sits under the top, with a quarter's headroom.
        assert!((memory[2].1 - 0.8).abs() < 1e-6);
        assert!((memory[0].1 - 0.4).abs() < 1e-6);

        let cpu = plot(&history, &|r| r.cpu);
        assert_eq!(cpu.len(), 2, "a reading with no rate is left out");
    }

    #[test]
    fn a_short_history_sits_at_the_right_edge() {
        let start = Instant::now();
        let history = [
            Point {
                at: start,
                reading: reading(100, Some(1.0)),
            },
            Point {
                at: start + Duration::from_secs(60),
                reading: reading(100, Some(1.0)),
            },
        ];
        let points = plot(&history, &|r| r.cpu);
        assert!((points[0].0 - 0.9).abs() < 1e-6);
        assert!((points[1].0 - 1.0).abs() < 1e-6);
        assert!(plot(&[], &|r: Reading| r.cpu).is_empty());
    }

    #[test]
    fn a_session_is_placed_in_the_deepest_worktree_holding_it() {
        let places = places();
        let shell = session(
            SessionKind::Terminal,
            "cargo",
            "/Users/me/.local/share/ket/worktrees/ket/fix-tabs/crates",
            Some(1.0),
        );
        assert_eq!(
            label(&shell, &places),
            ("cargo".to_owned(), Some("Fix tabs".to_owned()))
        );
        let outside = session(SessionKind::Terminal, "zsh", "/Users/me/notes", None);
        assert_eq!(
            label(&outside, &places),
            ("zsh".to_owned(), Some("notes".to_owned()))
        );
    }

    #[test]
    fn a_private_path_matches_the_worktree_made_without_it() {
        let places = vec![Place {
            id: WorktreeId::new("scratch"),
            path: PathBuf::from("/tmp/scratch"),
            name: "scratch".into(),
            agent: None,
        }];
        let shell = session(SessionKind::Terminal, "zsh", "/private/tmp/scratch", None);
        assert_eq!(label(&shell, &places).1.as_deref(), Some("scratch"));
    }

    #[test]
    fn an_agent_behind_a_launcher_is_named_for_its_worktree() {
        let places = places();
        let npx = session(
            SessionKind::Agent,
            "node",
            "/Users/me/.local/share/ket/worktrees/ket/fix-tabs",
            None,
        );
        assert_eq!(label(&npx, &places).0, "codex");
        // A native agent says what it is itself.
        let native = session(
            SessionKind::Agent,
            "opencode",
            "/Users/me/.local/share/ket/worktrees/ket/fix-tabs",
            None,
        );
        assert_eq!(label(&native, &places).0, "opencode");
        // A terminal running `node` is running node.
        let script = session(
            SessionKind::Terminal,
            "node",
            "/Users/me/.local/share/ket/worktrees/ket/fix-tabs",
            None,
        );
        assert_eq!(label(&script, &places).0, "node");
    }

    #[test]
    fn the_busiest_come_first_and_the_new_last() {
        let sessions = vec![
            session(SessionKind::Terminal, "zsh", "/a", Some(0.0)),
            session(SessionKind::Terminal, "new", "/a", None),
            session(SessionKind::Terminal, "cargo", "/a", Some(800.0)),
            session(SessionKind::Agent, "claude", "/a", Some(12.0)),
        ];
        let order: Vec<&str> = listed(&sessions)
            .iter()
            .map(|s| s.program.as_str())
            .collect();
        assert_eq!(order, ["cargo", "claude", "zsh", "new"]);
    }

    #[test]
    fn past_the_most_listed_the_rest_are_summed() {
        let sessions: Vec<Session> = (0..MOST_LISTED + 2)
            .map(|_| session(SessionKind::Terminal, "zsh", "/a", Some(0.0)))
            .collect();
        let listed = listed(&sessions);
        assert_eq!(
            rest(&listed),
            Some(Rest {
                count: 2,
                memory: 200 * MB
            })
        );
        assert_eq!(rest(&listed[..MOST_LISTED]), None);
    }
}
