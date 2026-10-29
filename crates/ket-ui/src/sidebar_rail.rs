//! The sidebar folded to a rail, and the strip's ⋯ menu.
//!
//! The sidebar folds the way the header does: one button at the end of its
//! strip, lit while folded, kept in the window's view state. Folded, it is a
//! 52px column of tiles (C Rail on the compact sidebar canvas, 2026-10-12).
//! Grouped by project, a tile is a project's badge; sorted by status, it is
//! one worktree, drawn as its project's badge, in a band per state (D1
//! Bands). Names and branches do not fit in 52px and are not squeezed in:
//! resting the pointer on a tile peeks the rows it stands for beside it.
//!
//! The rail ignores the strip's filter. The filter's chips are not on the
//! rail, and a column that quietly shows less than everything with no
//! control on screen saying why reads as worktrees having gone missing.

use std::time::Duration;

use gpui::{
    AnyElement, Context, Div, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Pixels,
    SharedString, Stateful, Window, div, prelude::*, px,
};
use ket_core::activity::Signal;
use ket_core::keybinding::Chord;
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::tree::{ProjectNode, Selection, SidebarFilter, row_mark, state_word};
use crate::ui::button::icon_button;
use crate::ui::chip::{BADGE, badge};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::popup::{self, Placement};
use crate::ui::tooltip::{Side, tooltip};
use crate::ui::{ICON_GAP, RADIUS_MD, RADIUS_SM, WELL_SM, WELL_XS};

/// The folded sidebar's width.
pub(crate) const RAIL_W: Pixels = px(52.0);

/// A tile's side: the badge with room around it for the hover and
/// selection fill.
const TILE: Pixels = px(36.0);

/// A worktree tile's height in the status sort: a step under [`TILE`], since
/// a band of a dozen worktrees is a long column.
const TILE_SHORT: Pixels = px(30.0);

/// How far the selection bar sits outside its tile, in the rail's gutter.
const BAR_OUTSET: Pixels = px(8.0);

/// The peek's width.
const PEEK_W: Pixels = px(264.0);

/// The ⋯ menu's width.
const MENU_W: Pixels = px(200.0);

/// How long the pointer rests on a tile before its peek opens.
const OPEN_DELAY: Duration = Duration::from_millis(250);

/// How long a peek waits after the pointer leaves it and its tile, so the
/// gap between the two can be crossed.
const CLOSE_DELAY: Duration = Duration::from_millis(200);

/// The menu's origin, for [`OpenMenu::opened_by`].
const MENU_ORIGIN: &str = "sidebar-more";

/// What the strip's ⋯ menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarAction {
    QuickPrompt,
    AddProject,
    /// Folds every project when any is open, else opens them all.
    FoldProjects,
}

/// Which tile a peek belongs to.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RailTarget {
    /// A project's tile, in the grouped rail.
    Project(usize),
    /// A worktree's tile, in the status sort.
    Worktree(Selection),
}

/// Where the pointer is on the rail, and which peek is open.
///
/// The same shape as the agent override card's: the tile and the peek each
/// report the pointer, and a timer settles what is open once it has stayed
/// put — see [`Shell::settle_rail_hover`].
#[derive(Default)]
pub(crate) struct RailHover {
    over_tile: Option<RailTarget>,
    over_peek: bool,
    pub(crate) open: Option<RailTarget>,
    epoch: u64,
}

/// A project's badge text: its icon, or its initial.
fn project_mark(project: &ProjectNode) -> SharedString {
    project.icon.clone().unwrap_or_else(|| {
        project
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default()
            .into()
    })
}

/// A status band's colour.
fn band_colour(band: SidebarFilter, t: &Theme) -> gpui::Rgba {
    paint(match band {
        SidebarFilter::Waiting => t.status.attention,
        SidebarFilter::Running => t.status.running,
        _ => t.text.dim,
    })
}

/// A tile's frame: hover fill, and the selected tile's fill and bar.
fn tile(
    id: impl Into<gpui::ElementId>,
    height: Pixels,
    selected: bool,
    t: &Theme,
) -> Stateful<Div> {
    let hover = paint(t.hover);
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(TILE)
        .h(height)
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .when(selected, |el| {
            el.bg(paint(t.hover)).child(
                div()
                    .absolute()
                    .left(-BAR_OUTSET)
                    .top(height / 4.0)
                    .bottom(height / 4.0)
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(paint(t.marker)),
            )
        })
        .when(!selected, |el| el.hover(move |style| style.bg(hover)))
}

impl Shell {
    /// Folds the sidebar to its rail, or lets it back out.
    pub(crate) fn toggle_sidebar_compact(&mut self, cx: &mut Context<Self>) {
        self.sidebar_compact = !self.sidebar_compact;
        // The button under the pointer is gone on the next frame and hears no
        // hover-out, so its tooltip would wait for the next one.
        self.hovered_strip_action = None;
        self.rail_hover = RailHover::default();
        self.sidebar_menu = None;
        self.persist_view();
        cx.notify();
    }

    /// A strip or rail button's tooltip, sharing the strip's hover field: one
    /// button in the sidebar's chrome is under the pointer at a time.
    fn rail_tip(
        &self,
        id: &'static str,
        button: Stateful<Div>,
        label: SharedString,
        side: Side,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        tooltip(
            id,
            button.into_any_element(),
            label,
            side,
            self.hovered_strip_action == Some(id) && self.sidebar_menu.is_none(),
            &self.theme,
        )
        .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
            if *is_hovered {
                this.hovered_strip_action = Some(id);
            } else if this.hovered_strip_action == Some(id) {
                this.hovered_strip_action = None;
            }
            cx.notify();
        }))
        .into_any_element()
    }

    /// The fold button: at the end of the strip, at the top of the rail.
    pub(crate) fn sidebar_fold_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let folded = self.sidebar_compact;
        let button = icon_button(
            "sidebar-fold",
            if folded {
                Icon::UnfoldHorizontal
            } else {
                Icon::FoldHorizontal
            },
        )
        .bare();
        let button = if folded {
            button.small().pressed(true).indicator(true)
        } else {
            button.dense()
        };
        self.rail_tip(
            "sidebar-fold",
            button
                .render(&self.theme)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar_compact(cx))),
            if folded {
                "Expand sidebar".into()
            } else {
                "Compact sidebar".into()
            },
            if folded { Side::Right } else { Side::BottomEnd },
            cx,
        )
    }

    /// The ⋯ button and, while open, its menu: Quick prompt and Add project,
    /// which used to be two glyphs of their own on the strip, and Collapse
    /// all or Expand all.
    pub(crate) fn sidebar_more_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let rail = self.sidebar_compact;
        let open = self.sidebar_menu.is_some();
        let button = icon_button("sidebar-more", Icon::Ellipsis).bare();
        let button = if rail { button.small() } else { button.dense() };
        let trigger = self.rail_tip(
            "sidebar-more",
            button
                .pressed(open)
                .render(&self.theme)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_sidebar_menu();
                    cx.notify();
                })),
            "More".into(),
            if rail { Side::Right } else { Side::Bottom },
            cx,
        );
        let panel = self.sidebar_menu.as_ref().map(|menu| {
            menu.view(
                &self.theme,
                cx,
                |shell, action, cx| shell.run_sidebar_action(*action, cx),
                |shell, cx| {
                    shell.sidebar_menu = None;
                    cx.notify();
                },
            )
        });
        dropdown("sidebar-more-menu", trigger, panel)
            .flex_none()
            // The shell root closes every menu on a press; without this the
            // press that should close this one would close it on the way
            // down, and the click would open a fresh one.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }

    /// Opens the ⋯ menu, or closes it when it is already open.
    fn toggle_sidebar_menu(&mut self) {
        if self.sidebar_menu.take().is_some() {
            return;
        }
        // One menu at a time: two open is two things claiming the keyboard.
        self.menu = None;
        self.worktree_menu = None;
        self.path_menu = None;
        self.project_menu = None;
        self.popup = None;
        self.rail_hover = RailHover::default();

        let chord = Chord::parse("cmd+shift+p").expect("the shipped chord parses");
        let mut entries = vec![
            MenuEntry::Item(
                MenuItem::new(SidebarAction::QuickPrompt, "Quick prompt")
                    .icon(Icon::Pencil)
                    .chord(chord),
            ),
            MenuEntry::Item(
                MenuItem::new(SidebarAction::AddProject, "Add project…").icon(Icon::FolderPlus),
            ),
        ];
        // Only where projects fold: the status sort and the rail list no
        // project with a caret, so there the pick would change nothing on
        // screen.
        if !self.activity_view && !self.sidebar_compact && !self.projects.is_empty() {
            let item = if self.projects.iter().any(|project| project.expanded) {
                MenuItem::new(SidebarAction::FoldProjects, "Collapse all")
                    .icon(Icon::ChevronsDownUp)
            } else {
                MenuItem::new(SidebarAction::FoldProjects, "Expand all")
                    .icon(Icon::ChevronsUpDown)
            };
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::Item(item));
        }
        let menu = OpenMenu::new(MENU_ORIGIN.into(), None, MENU_W, entries).uncapped();
        // Hung under the button with its right edge on the button's on the
        // strip; beside it on the rail, where the window clamps it up off
        // the bottom edge.
        self.sidebar_menu = Some(if self.sidebar_compact {
            menu.offset(WELL_SM + px(10.0), -WELL_SM)
        } else {
            menu.offset(WELL_XS - MENU_W, px(6.0))
        });
    }

    /// Runs one pick from the ⋯ menu.
    fn run_sidebar_action(&mut self, action: SidebarAction, cx: &mut Context<Self>) {
        self.sidebar_menu = None;
        match action {
            SidebarAction::QuickPrompt => self.open_quick_prompt(cx),
            SidebarAction::AddProject => self.add_project(cx),
            SidebarAction::FoldProjects => {
                let open = !self.projects.iter().any(|project| project.expanded);
                for project in &mut self.projects {
                    project.expanded = open;
                }
                self.persist_view();
            }
        }
        cx.notify();
    }

    /// Handles a key while the ⋯ menu is open.
    pub(crate) fn sidebar_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.sidebar_menu.as_mut() else {
            return false;
        };
        match menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => {}
            MenuKey::Close => self.sidebar_menu = None,
            MenuKey::Run(action) => self.run_sidebar_action(action, cx),
        }
        true
    }

    /// Notes the pointer on or off a tile or its peek.
    fn note_rail_hover(
        &mut self,
        target: Option<RailTarget>,
        peek: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        if let Some(over) = peek {
            self.rail_hover.over_peek = over;
        } else {
            self.rail_hover.over_tile = target;
        }
        self.settle_rail_hover(cx);
    }

    /// Opens or closes a peek once the pointer has stayed put.
    fn settle_rail_hover(&mut self, cx: &mut Context<Self>) {
        let hover = &mut self.rail_hover;
        hover.epoch += 1;
        let wanted = hover.over_tile.or(hover.open.filter(|_| hover.over_peek));
        if wanted == hover.open {
            return;
        }
        let epoch = hover.epoch;
        // Moving from one open peek to the next tile swaps at once: the
        // pointer is already reading the rail, and a second wait per tile
        // makes running down it feel sticky.
        let delay = match (wanted, hover.open) {
            (Some(_), Some(_)) => Duration::ZERO,
            (Some(_), None) => OPEN_DELAY,
            (None, _) => CLOSE_DELAY,
        };
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let _ = shell.update(cx, |shell, cx| {
                let hover = &mut shell.rail_hover;
                if hover.epoch == epoch {
                    hover.open = hover.over_tile.or(hover.open.filter(|_| hover.over_peek));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Selects a worktree from the rail, the way clicking its row does.
    fn rail_select(&mut self, target: Selection, cx: &mut Context<Self>) {
        let reselected = self.selection == Some(target);
        self.select(target, cx);
        if reselected {
            self.focus_main_tab();
        }
        self.rail_hover = RailHover::default();
        cx.notify();
    }

    /// The sidebar, folded.
    pub(crate) fn sidebar_rail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let now = ket_core::now_ms();
        let status_view = self.activity_view;

        // The sort, stacked: the strip's two answers, one over the other.
        let mut sort = |id: &'static str, which: Icon, status: bool, label: &'static str| {
            self.rail_tip(
                id,
                icon_button(id, which)
                    .bare()
                    .dense()
                    .pressed(status_view == status)
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.activity_view != status {
                            this.activity_view = status;
                            this.rail_hover = RailHover::default();
                            this.persist_view();
                            cx.notify();
                        }
                    })),
                label.into(),
                Side::Right,
                cx,
            )
        };
        let sort = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(1.0))
            .p(px(2.0))
            .rounded(RADIUS_MD)
            .bg(paint(t.sunken))
            .child(sort(
                "rail-view-status",
                Icon::ViewStatus,
                true,
                "Sort worktrees by status",
            ))
            .child(sort(
                "rail-view-project",
                Icon::ViewProject,
                false,
                "Group by project",
            ));

        let rule = || div().flex_none().w(px(28.0)).h(px(1.0)).bg(paint(t.rule));

        let head = div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .pt(px(10.0))
            .pb(px(8.0))
            .child(self.sidebar_fold_button(cx))
            .child(sort)
            .child(div().mt(px(2.0)).child(rule()));

        let tiles = if status_view {
            self.rail_bands(now, window, cx)
        } else {
            self.rail_projects(now, window, cx)
        };
        let list = div()
            .id("sidebar-rail")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .gap(px(4.0))
            .pt(px(2.0))
            .pb(px(8.0))
            .overflow_y_scroll()
            .children(tiles);

        // New worktree in the selected project, or the first; with no project
        // at all, Add project — the same as the expanded sidebar's foot.
        let project = self
            .selection
            .map(|selection| selection.project)
            .filter(|&index| index < self.projects.len())
            .or((!self.projects.is_empty()).then_some(0));
        let new = self.rail_tip(
            "rail-new-worktree",
            icon_button("rail-new-worktree", Icon::Plus)
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    match project {
                        Some(index) => this.open_new_worktree(index, true, cx),
                        None => this.add_project(cx),
                    }
                    cx.notify();
                })),
            match project {
                Some(_) => "New worktree".into(),
                None => "Add project".into(),
            },
            Side::Right,
            cx,
        );
        // The search field lives in the strip, which the rail does not have:
        // searching unfolds the sidebar first.
        let search = self.rail_tip(
            "rail-search",
            icon_button("rail-search", Icon::Search)
                .bare()
                .small()
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_sidebar_compact(cx);
                    this.open_search(cx);
                    cx.notify();
                })),
            crate::shortcuts::SEARCH_HINT.into(),
            Side::Right,
            cx,
        );
        let foot = div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .pt(px(6.0))
            .pb(px(10.0))
            .child(search)
            .child(self.sidebar_more_button(cx))
            .child(new);

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(head)
            .child(list)
            .child(foot)
            .into_any_element()
    }

    /// Grouped by project: a tile per project.
    fn rail_projects(&self, now: u64, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = &self.theme;
        let selected = self.selection.map(|selection| selection.project);
        self.projects
            .iter()
            .enumerate()
            .map(|(pi, project)| {
                let target = RailTarget::Project(pi);
                // Whatever is most urgent in the project, as a dot on its
                // corner: waiting on a person over working over nothing.
                let urgent = project
                    .worktrees
                    .iter()
                    .map(|node| SidebarFilter::of(self.row_signal(node, now)))
                    .min_by_key(|band| match band {
                        SidebarFilter::Waiting => 0,
                        SidebarFilter::Running => 1,
                        _ => 2,
                    })
                    .filter(|band| *band != SidebarFilter::Idle);
                let count = project.worktrees.len();
                let launches = project.override_summary().is_some();

                let face = tile(("rail-project", pi), TILE, selected == Some(pi), t)
                    .child(badge(project.color, project_mark(project), t))
                    .when_some(urgent, |el, band| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(3.0))
                                .right(px(3.0))
                                .size(px(9.0))
                                .rounded_full()
                                .border_2()
                                .border_color(paint(t.panel))
                                .bg(band_colour(band, t)),
                        )
                    })
                    .when(count > 1, |el| {
                        el.child(
                            div()
                                .absolute()
                                .right(px(1.0))
                                .bottom(px(1.0))
                                .min_w(px(14.0))
                                .h(px(14.0))
                                .px(px(3.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_1()
                                .border_color(paint(t.panel))
                                .bg(paint(t.elevated))
                                .text_size(px(9.0))
                                .text_color(paint(t.text.primary))
                                .child(count.to_string()),
                        )
                    })
                    // The agent override's glyph, in its tint, where the
                    // heading would have drawn it beside the name.
                    .when(launches, |el| {
                        el.child(
                            div()
                                .absolute()
                                .left(px(1.0))
                                .bottom(px(1.0))
                                .p(px(1.0))
                                .rounded(px(3.0))
                                .bg(paint(t.panel))
                                .child(sized_icon(
                                    Icon::Launch,
                                    px(10.0),
                                    crate::agent_override::hue(t),
                                )),
                        )
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let over = if *hovered {
                            Some(target)
                        } else if this.rail_hover.over_tile == Some(target) {
                            None
                        } else {
                            return;
                        };
                        this.note_rail_hover(over, None, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.rail_hover = RailHover::default();
                            this.open_project_menu(pi, event.position);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    // To the worktree last visited in the project, the way
                    // the expanded sidebar's heading remembers it.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(project) = this.projects.get(pi) else {
                            return;
                        };
                        if project.worktrees.is_empty() {
                            return;
                        }
                        let target = this
                            .project_focus
                            .get(&project.id)
                            .and_then(|id| this.position_of(id))
                            .filter(|selection| selection.project == pi)
                            .unwrap_or(Selection {
                                project: pi,
                                worktree: 0,
                            });
                        this.rail_select(target, cx);
                    }));

                let peek = (self.rail_hover.open == Some(target))
                    .then(|| self.project_peek(pi, project, now, cx));
                popup::hover_anchored(
                    ("rail-project-peek", pi),
                    face.into_any_element(),
                    peek,
                    Placement::RightStart,
                    PEEK_W,
                    window,
                    t,
                )
            })
            .collect()
    }

    /// Sorted by status: a tile per worktree, in a band per state.
    fn rail_bands(&self, now: u64, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = &self.theme;
        let mut entries: Vec<(Selection, Signal, bool)> = self
            .projects
            .iter()
            .enumerate()
            .flat_map(|(pi, project)| {
                project.worktrees.iter().enumerate().map(move |(wi, node)| {
                    (
                        Selection {
                            project: pi,
                            worktree: wi,
                        },
                        self.row_signal(node, now),
                        node.pinned,
                    )
                })
            })
            .collect();
        entries.sort_by_key(|(at, signal, pinned)| {
            (
                std::cmp::Reverse(*pinned),
                crate::projects_view::status_rank(*signal),
                at.project,
                at.worktree,
            )
        });

        let mut out = Vec::new();
        for band in [
            SidebarFilter::Waiting,
            SidebarFilter::Running,
            SidebarFilter::Idle,
        ] {
            let members: Vec<_> = entries
                .iter()
                .filter(|(_, signal, _)| SidebarFilter::of(*signal) == band)
                .collect();
            if members.is_empty() {
                continue;
            }
            let quiet = band == SidebarFilter::Idle;
            let mut column = div()
                .flex()
                .flex_col()
                .flex_none()
                .items_center()
                .gap(px(1.0))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .h(px(12.0))
                        .child(div().size(px(6.0)).rounded_full().bg(band_colour(band, t))),
                );
            for &&(at, signal, _) in &members {
                let Some(project) = self.projects.get(at.project) else {
                    continue;
                };
                let target = RailTarget::Worktree(at);
                let id = at.project * 1000 + at.worktree;
                let face = tile(
                    ("rail-worktree", id),
                    TILE_SHORT,
                    self.selection == Some(at),
                    t,
                )
                .child(
                    badge(project.color, project_mark(project), t)
                        .when(quiet, |el| el.opacity(0.5)),
                )
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let over = if *hovered {
                        Some(target)
                    } else if this.rail_hover.over_tile == Some(target) {
                        None
                    } else {
                        return;
                    };
                    this.note_rail_hover(over, None, cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.rail_hover = RailHover::default();
                        this.open_worktree_menu(at.project, at.worktree, event.position);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.rail_select(at, cx)));
                let peek = (self.rail_hover.open == Some(target)).then(|| {
                    let body = div()
                        .flex()
                        .flex_col()
                        .p(px(6.0))
                        .child(self.peek_row(at, signal, true, now, cx));
                    self.peek_frame(body, cx)
                });
                column = column.child(popup::hover_anchored(
                    ("rail-worktree-peek", id),
                    face.into_any_element(),
                    peek,
                    Placement::RightStart,
                    PEEK_W,
                    window,
                    t,
                ));
            }
            out.push(column.into_any_element());
        }
        out
    }

    /// The peek's body, reporting the pointer so the gap from the tile can
    /// be crossed.
    fn peek_frame(&self, body: Div, cx: &mut Context<Self>) -> AnyElement {
        body.id("rail-peek")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.note_rail_hover(None, Some(*hovered), cx);
            }))
            .into_any_element()
    }

    /// A project's peek: its name, then its worktrees as rows.
    fn project_peek(
        &self,
        pi: usize,
        project: &ProjectNode,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let head = div()
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .h(px(30.0))
            .px(px(8.0))
            .child(badge(project.color, project_mark(project), t))
            .child(
                div()
                    .prose()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(project.name.clone()),
            );
        let rows: Vec<_> = (0..project.worktrees.len())
            .map(|wi| {
                let at = Selection {
                    project: pi,
                    worktree: wi,
                };
                let signal = self.row_signal(&project.worktrees[wi], now);
                self.peek_row(at, signal, false, now, cx).into_any_element()
            })
            .collect();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(6.0))
            .child(head)
            .when(rows.is_empty(), |el| {
                el.child(
                    div()
                        .prose()
                        .px(px(8.0))
                        .pb(px(6.0))
                        .text_size(px(12.0))
                        .text_color(paint(t.text.dim))
                        .child("No worktrees yet"),
                )
            })
            .children(rows);
        self.peek_frame(body, cx)
    }

    /// One worktree, as the list draws it: its mark, its branch, and under
    /// it what it is doing — with its project first in the status sort, where
    /// the tile is all that said whose it is.
    fn peek_row(
        &self,
        at: Selection,
        signal: Signal,
        with_project: bool,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = &self.theme;
        let id = ("rail-peek-row", at.project * 1000 + at.worktree);
        let Some(node) = self
            .projects
            .get(at.project)
            .and_then(|project| project.worktrees.get(at.worktree))
        else {
            return div().id(id);
        };
        let project = &self.projects[at.project];
        let activity = self.activity.activity(&node.id, now);
        let detail = activity.detail();
        let selected = self.selection == Some(at);
        let hover = paint(t.hover);
        let dim = paint(t.text.dim);

        let second = div()
            .prose()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .mt(px(2.0))
            .text_size(px(12.0))
            .text_color(dim)
            .when(with_project, |el| {
                el.child(
                    div()
                        .flex_none()
                        .size(px(6.0))
                        .rounded(px(2.0))
                        .bg(paint(project.color)),
                )
                .child(div().flex_none().child(project.name.clone()))
                .when(!detail.is_empty(), |el| {
                    el.child(div().flex_none().child("·"))
                })
            })
            .when(!detail.is_empty(), |el| {
                el.child(div().min_w_0().truncate().child(detail.clone()))
            });
        let lines = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(div().truncate().child(node.label()))
            .when(with_project || !detail.is_empty(), |el| el.child(second));

        div()
            .id(id)
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .px(px(8.0))
            .py(px(6.0))
            .rounded(RADIUS_SM)
            .text_size(px(13.0))
            .cursor_pointer()
            .when(selected, |el| el.bg(alpha(paint(t.selection), 0.9)))
            .when(!selected, |el| el.hover(move |style| style.bg(hover)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(BADGE)
                    .child(row_mark(&activity, node.dirty, node.missing, t)),
            )
            .child(lines)
            .children(state_word(node, signal, t))
            .on_click(cx.listener(move |this, _, _, cx| this.rail_select(at, cx)))
    }
}
