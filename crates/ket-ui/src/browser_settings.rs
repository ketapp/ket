//! Settings → Browser, in a ket built with Chromium: the Chromium its browser
//! tabs run, and the profile each worktree keeps beneath Chromium's root —
//! what each one takes on disk, which are in use, and deleting them.
//!
//! - **Read when the pane shows**, off the window's thread: a profile's size
//!   is a walk of every file in it (see `ket_core::storage`), and the walk
//!   and the deleting are both ket-core's.
//! - **Known by name.** A profile's directory is its worktree's id, made
//!   safe ([`crate::chromium::profile_name`]), so every worktree ket knows
//!   names its own. Chromium keeps its own state in directories beside them,
//!   told apart by shape: a worktree's id is a slug ending in a dash and a
//!   twelve-digit hash ([`ket_core::slug`]), which none of Chromium's names
//!   are. A directory of that shape that no worktree claims is a removed
//!   worktree's.
//! - **Never one in use.** Chromium holds a profile's files while a page uses
//!   them, so a worktree with a browser tab open keeps its profile until its
//!   tabs are closed, and until their pages have finished closing, which can
//!   take Chromium a moment after the tab has gone
//!   ([`ket_chromium::profile_in_use`]). The row looks again until it has.
//! - **Asked once.** Delete asks in its own row, as General's Remove from your
//!   agents does: Settings is already a dialog.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use gpui::{Context, div, prelude::*, px};
use ket_core::storage::{Clearable, format_bytes};
use ket_core::theme::Theme;

use super::{Entry, Section, row, setting};
use crate::Shell;
use crate::chromium::{profile_name, release_profile, reserve_profile, root_cache};
use crate::ui::button::button;
use crate::ui::field::error;

/// A delete row's three buttons — Delete…, Cancel, Delete — each told apart
/// from the same button in another row by its index.
type Keys = [&'static str; 3];

/// A profile's row's.
const PROFILE_KEYS: Keys = [
    "browser-profile-delete",
    "browser-profile-cancel",
    "browser-profile-confirm",
];

/// The removed worktrees' row's.
const REMOVED_KEYS: Keys = [
    "browser-removed-delete",
    "browser-removed-cancel",
    "browser-removed-confirm",
];

/// How often the pane looks whether Chromium has let go of a profile whose
/// tabs have closed.
const HELD_LOOK: Duration = Duration::from_millis(250);

/// Why a profile with no tab open cannot be deleted yet.
const HELD: &str = "Chromium still has this profile open. It lets go once its pages have \
                    finished closing, or when ket quits.";

/// The Browser pane's state, kept on the settings view and dropped with it.
#[derive(Default)]
pub(super) struct BrowserPane {
    /// What the last look beneath Chromium's root found, largest first — or
    /// why there was nowhere to look. `None` until a look has answered.
    found: Option<Result<Vec<Profile>, String>>,
    /// Whether a look is under way, so showing the pane twice does not walk
    /// the profiles twice.
    reading: bool,
    /// Counts the Deletes, so a look that set out before one does not land
    /// after it with the profiles it deleted.
    deletes: u64,
    /// The Delete that was pressed once and is asking to be sure.
    confirming: Option<Target>,
    /// The Delete under way.
    deleting: Option<Target>,
    /// What the last Delete could not do.
    failed: Option<String>,
    /// Whether the pane is looking for Chromium to let go of a profile it
    /// shows as held — see [`Shell::watch_held_profiles`]. Set while drawing.
    watching: Cell<bool>,
}

/// What a Delete is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// One profile, by its directory's name.
    One(String),
    /// Every removed worktree's.
    Removed,
}

/// A profile found beneath Chromium's root.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Profile {
    /// Its directory's name.
    dir: String,
    /// What it holds, in bytes.
    bytes: u64,
}

/// A profile as the pane names it.
struct Shown<'a> {
    profile: &'a Profile,
    /// Its worktree's name and project, or `None` for a removed worktree.
    worktree: Option<(String, String)>,
    /// How many browser tabs use it now.
    tabs: usize,
    /// Chromium still has it open with no tab using it: a page of its is
    /// still closing.
    held: bool,
}

impl Shell {
    fn profiles_pane(&mut self) -> Option<&mut BrowserPane> {
        self.preferences.as_mut().map(|view| &mut view.browser)
    }

    /// Call after the settings view lands on a section. Looks beneath
    /// Chromium's root again if the section is Browser; otherwise does
    /// nothing.
    pub(super) fn browser_shown(&mut self, cx: &mut Context<Self>) {
        if self
            .preferences
            .as_ref()
            .is_some_and(|view| view.section == Section::Browser)
        {
            self.read_browser_profiles(cx);
        }
    }

    /// Backs out of a Delete that is asking to be sure. Returns whether one
    /// was, so Escape can do this before it closes the settings view.
    pub(super) fn cancel_browser_prompt(&mut self) -> bool {
        self.profiles_pane()
            .is_some_and(|pane| pane.confirming.take().is_some())
    }

    /// Every worktree ket knows, by its profile's directory name: the
    /// worktree's name and its project's.
    fn worktree_profiles(&self) -> HashMap<String, (String, String)> {
        self.projects
            .iter()
            .flat_map(|project| {
                project.worktrees.iter().map(|node| {
                    (
                        profile_name(&node.id),
                        (node.label().to_string(), project.name.to_string()),
                    )
                })
            })
            .collect()
    }

    /// How many browser tabs each profile has open, by directory name.
    fn open_profiles(&self) -> HashMap<String, usize> {
        let mut open = HashMap::new();
        for tab in self.chromium_tabs() {
            *open.entry(profile_name(&tab.worktree)).or_default() += 1;
        }
        open
    }

    /// Whether Chromium still has `dir`'s profile open — a page is open in
    /// it, or still closing. On the main thread.
    fn profile_held(dir: &str) -> bool {
        root_cache().is_ok_and(|root| ket_chromium::profile_in_use(&root.join(dir)))
    }

    /// Whether the Browser pane shows a profile Chromium still has open with
    /// no tab using it.
    fn showing_held_profile(&self) -> bool {
        let Some(view) = self
            .preferences
            .as_ref()
            .filter(|view| view.section == Section::Browser)
        else {
            return false;
        };
        let Some(Ok(found)) = &view.browser.found else {
            return false;
        };
        let open = self.open_profiles();
        found
            .iter()
            .any(|profile| !open.contains_key(&profile.dir) && Self::profile_held(&profile.dir))
    }

    /// Looks every [`HELD_LOOK`] whether Chromium has let go of the profiles
    /// the pane shows as held, and draws the pane again once it has: nothing
    /// else says when a closing page has finished. Once at a time.
    fn watch_held_profiles(&self, cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_ref() else {
            return;
        };
        if view.browser.watching.replace(true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(HELD_LOOK).await;
                let held = this.update(cx, |this, cx| {
                    let held = this.showing_held_profile();
                    if !held {
                        if let Some(pane) = this.profiles_pane() {
                            pane.watching.set(false);
                        }
                        cx.notify();
                    }
                    held
                });
                if !matches!(held, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    /// Walks the profiles beneath Chromium's root, off the window's thread.
    fn read_browser_profiles(&mut self, cx: &mut Context<Self>) {
        let known: Vec<String> = self.worktree_profiles().into_keys().collect();
        let Some(pane) = self.profiles_pane() else {
            return;
        };
        if pane.reading {
            return;
        }
        pane.reading = true;
        let deletes = pane.deletes;
        let looking = cx.background_executor().spawn(async move {
            let root = root_cache()?;
            Ok(profiles(&root, &known))
        });
        cx.spawn(async move |this, cx| {
            let found = looking.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(pane) = this.profiles_pane() {
                    pane.reading = false;
                    if pane.deletes == deletes {
                        pane.found = Some(found);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Deletes what `target` names, then looks again — both off the window's
    /// thread, and in one go, so the rows change once, to what is left.
    ///
    /// Which directories that is, is settled here rather than when the row
    /// was drawn: a tab opened since, by an agent, takes its profile out of
    /// the plan, as does a page Chromium is still closing.
    fn delete_browser_profiles(&mut self, target: Target, cx: &mut Context<Self>) {
        let known = self.worktree_profiles();
        let open = self.open_profiles();
        let names: Vec<String> = known.keys().cloned().collect();
        let Some(pane) = self.profiles_pane() else {
            return;
        };
        pane.confirming = None;
        if pane.deleting.is_some() {
            return;
        }
        let Some(Ok(found)) = &pane.found else {
            return;
        };
        let plan: Vec<Clearable> = found
            .iter()
            .filter(|profile| !open.contains_key(&profile.dir) && !Self::profile_held(&profile.dir))
            .filter(|profile| match &target {
                Target::One(dir) => profile.dir == *dir,
                Target::Removed => !known.contains_key(&profile.dir),
            })
            .map(|profile| Clearable {
                path: profile.dir.clone(),
                bytes: profile.bytes,
            })
            .collect();

        if plan.is_empty() {
            pane.failed = Some(match target {
                Target::One(dir) if open.contains_key(&dir) => {
                    "A browser tab is using this profile now. Close its tabs, then delete it."
                        .to_owned()
                }
                Target::One(_) => HELD.to_owned(),
                Target::Removed => "No removed worktree has a profile left.".to_owned(),
            });
            cx.notify();
            return;
        }
        let root = match root_cache() {
            Ok(root) => root,
            Err(why) => {
                pane.failed = Some(why);
                cx.notify();
                return;
            }
        };
        // This window knows only its own pages: another ket's Chromium may
        // have any of these profiles open.
        if ket_core::workspace::chromium_held_elsewhere(&root) {
            pane.failed =
                Some("Another ket is using Chromium's data now. Quit it, then delete.".to_owned());
            cx.notify();
            return;
        }
        // Each held against a page opening on it until the deletion is done:
        // an agent opening a tab in that worktree meanwhile is told to wait.
        let mut plan = plan;
        plan.retain(|clearable| reserve_profile(&root.join(&clearable.path)));
        if plan.is_empty() {
            pane.failed = Some(HELD.to_owned());
            cx.notify();
            return;
        }
        let reserved: Vec<std::path::PathBuf> = plan
            .iter()
            .map(|clearable| root.join(&clearable.path))
            .collect();
        pane.failed = None;
        pane.deleting = Some(target);
        pane.deletes += 1;
        let planned = plan.len();
        let deleting = cx.background_executor().spawn(async move {
            let cleared = ket_core::storage::clear(&root, &plan);
            (cleared, profiles(&root, &names))
        });
        cx.spawn(async move |this, cx| {
            let (cleared, left) = deleting.await;
            // Whether or not the pane is still there to say so.
            for profile in &reserved {
                release_profile(profile);
            }
            let _ = this.update(cx, |this, cx| {
                let Some(pane) = this.profiles_pane() else {
                    return;
                };
                pane.deleting = None;
                pane.found = Some(Ok(left));
                pane.failed = (cleared.failed > 0).then(|| match planned {
                    1 => "Could not delete the profile. ket's log says why.".to_owned(),
                    _ => format!(
                        "Could not delete {} of {planned} profiles. ket's log says why.",
                        cleared.failed
                    ),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The Browser pane: the Chromium in use, then each profile with its
    /// size, whether a tab is using it, and Delete — and one Delete for every
    /// removed worktree's.
    pub(super) fn browser_settings(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let pane = &view.browser;
        let root = root_cache();
        let kept = match &root {
            Ok(root) => format!(" Everything it keeps is in {}.", crate::header::tilde(root)),
            Err(_) => String::new(),
        };
        let mut out = vec![setting(
            &format!("Chromium {}", ket_chromium::chromium_version()),
            &format!("The engine ket's browser tabs run on, built into this ket.{kept}"),
            div().into_any_element(),
            t,
        )];
        out.push(setting(
            "Profiles",
            "Each worktree keeps its own sign-ins and site data. Deleting a profile signs that \
             worktree out everywhere.",
            div().into_any_element(),
            t,
        ));

        let found = match (&root, &pane.found) {
            (Err(why), _) | (_, Some(Err(why))) => {
                out.push(Entry::layout(error(why.clone(), t).pb(px(11.0))));
                return out;
            }
            (Ok(_), None) => {
                out.push(Entry::layout(row(
                    "Looking\u{2026}",
                    "Measuring each worktree\u{2019}s profile.",
                    div().into_any_element(),
                    t,
                )));
                return out;
            }
            (Ok(_), Some(Ok(found))) => found,
        };
        if found.is_empty() {
            out.push(Entry::layout(row(
                "No profiles yet",
                "A worktree gets one the first time it opens a browser tab.",
                div().into_any_element(),
                t,
            )));
        }

        let known = self.worktree_profiles();
        let open = self.open_profiles();
        let shown: Vec<Shown> = found
            .iter()
            .map(|profile| {
                let tabs = open.get(&profile.dir).copied().unwrap_or(0);
                Shown {
                    profile,
                    worktree: known.get(&profile.dir).cloned(),
                    tabs,
                    held: tabs == 0 && Self::profile_held(&profile.dir),
                }
            })
            .collect();
        if shown.iter().any(|shown| shown.held) {
            self.watch_held_profiles(cx);
        }
        for (index, profile) in shown.iter().enumerate() {
            out.push(self.profile_row(index, profile, pane, t, cx));
        }

        let removed: Vec<&Shown> = shown
            .iter()
            .filter(|shown| shown.worktree.is_none() && shown.tabs == 0 && !shown.held)
            .collect();
        if !removed.is_empty() {
            let bytes = removed.iter().map(|shown| shown.profile.bytes).sum();
            let what = match removed.len() {
                1 => format!("1 profile, {}", format_bytes(bytes)),
                n => format!("{n} profiles, {}", format_bytes(bytes)),
            };
            out.push(self.delete_row(
                "Delete profiles of removed worktrees",
                &format!("{what}, left by worktrees ket no longer has."),
                "Deletes their sign-ins and site data for good.",
                Target::Removed,
                (REMOVED_KEYS, 0),
                pane,
                t,
                cx,
            ));
        }
        if let Some(why) = &pane.failed {
            out.push(Entry::layout(error(why.clone(), t).pb(px(11.0))));
        }
        out
    }

    /// One profile's row: whose it is, its size, whether a tab uses it, and
    /// Delete — or why not.
    fn profile_row(
        &self,
        index: usize,
        shown: &Shown,
        pane: &BrowserPane,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Entry {
        let size = format_bytes(shown.profile.bytes);
        let (name, whose) = match &shown.worktree {
            Some((worktree, project)) => (worktree.clone(), format!("{project} \u{b7} {size}")),
            None => (
                "Removed worktree".to_owned(),
                format!("{} \u{b7} {size}", shown.profile.dir),
            ),
        };
        let unable = match shown.tabs {
            0 if shown.held => Some(HELD.to_owned()),
            0 => None,
            1 => Some("Open in a browser tab now. Close it to delete this profile.".to_owned()),
            n => Some(format!(
                "Open in {n} browser tabs now. Close them to delete this profile."
            )),
        };
        if let Some(why) = unable {
            return setting(
                &name,
                &format!("{whose}. {why}"),
                button((PROFILE_KEYS[0], index), "Delete\u{2026}")
                    .danger()
                    .small()
                    .enabled(false)
                    .render(t)
                    .into_any_element(),
                t,
            );
        }
        let asking = format!("Signs {name} out of every site, and deletes its site data.");
        self.delete_row(
            &name,
            &whose,
            &asking,
            Target::One(shown.profile.dir.clone()),
            (PROFILE_KEYS, index),
            pane,
            t,
            cx,
        )
    }

    /// A row whose control deletes: Delete…, then Cancel and Delete in its
    /// place with what deleting costs, then the deleting itself.
    #[allow(clippy::too_many_arguments)]
    fn delete_row(
        &self,
        name: &str,
        description: &str,
        asking: &str,
        target: Target,
        id: (Keys, usize),
        pane: &BrowserPane,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Entry {
        let ([delete, cancel, confirm], index) = id;
        if pane.deleting.as_ref() == Some(&target) {
            return setting(
                name,
                description,
                button((delete, index), "Delete")
                    .danger()
                    .small()
                    .loading("Deleting\u{2026}")
                    .render(t)
                    .into_any_element(),
                t,
            );
        }
        if pane.confirming.as_ref() == Some(&target) {
            let control = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    button((cancel, index), "Cancel")
                        .ghost()
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_browser_prompt();
                            cx.notify();
                        })),
                )
                .child(
                    button((confirm, index), "Delete")
                        .danger()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_browser_profiles(target.clone(), cx);
                            cx.notify();
                        })),
                )
                .into_any_element();
            return setting(name, asking, control, t);
        }
        let idle = pane.deleting.is_none();
        let control = button((delete, index), "Delete\u{2026}")
            .danger()
            .small()
            .enabled(idle)
            .render(t)
            .when(idle, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(pane) = this.profiles_pane() {
                        pane.confirming = Some(target.clone());
                        pane.failed = None;
                    }
                    cx.notify();
                }))
            })
            .into_any_element();
        setting(name, description, control, t)
    }
}

/// The profiles beneath `root`, largest first: the directories named for a
/// worktree in `known`, and those shaped like one, which a removed worktree
/// left. Chromium's own directories beside them are neither.
fn profiles(root: &Path, known: &[String]) -> Vec<Profile> {
    // `*` matches every directory straight beneath the root, so each is
    // measured whole and none is looked inside for another.
    ket_core::storage::plan_clear(root, &["*".to_owned()])
        .into_iter()
        .filter(|dir| known.contains(&dir.path) || worktree_shaped(&dir.path))
        .map(|dir| Profile {
            dir: dir.path,
            bytes: dir.bytes,
        })
        .collect()
}

/// Whether a directory's name is shaped like a worktree's id: lowercase
/// letters and digits in runs joined by single dashes, the last run twelve
/// hex digits — what [`ket_core::slug::slug_keyed`] makes, and what
/// [`profile_name`] leaves as it is.
fn worktree_shaped(name: &str) -> bool {
    const HASH: usize = 12;
    let Some((readable, hash)) = name.rsplit_once('-') else {
        return false;
    };
    hash.len() == HASH
        && hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && !readable.is_empty()
        && readable.split('-').all(|run| {
            !run.is_empty()
                && run
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ket_core::id::WorktreeId;

    #[test]
    fn a_worktrees_id_is_worktree_shaped_and_chromiums_own_are_not() {
        let id = ket_core::slug::slug_keyed("feature/login", b"project\0feature/login");
        assert!(worktree_shaped(&profile_name(&WorktreeId::new(id))));
        assert!(worktree_shaped("primary-ket-0123456789ab"));
        assert!(worktree_shaped("x-ffffffffffff"));
        for chromium in [
            "Default",
            "GPUPersistentCache",
            "component_crx_cache",
            "Crowd Deny",
            "segmentation_platform",
            "WidevineCdm",
            "ket-0123456789",
            "ket-0123456789AB",
            "-0123456789ab",
            "ket--0123456789ab",
            "Ket-0123456789ab",
        ] {
            assert!(!worktree_shaped(chromium), "{chromium}");
        }
    }

    #[test]
    fn the_profiles_are_the_worktrees_and_not_chromiums_own_state() {
        let root =
            std::env::temp_dir().join(format!("ket-browser-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, bytes) in [
            ("ket-main-0123456789ab", 5),
            ("old-branch-ba9876543210", 3),
            ("w1", 2),
            ("Default", 7),
            ("GPUPersistentCache", 1),
        ] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
            std::fs::write(root.join(dir).join("Cookies"), vec![0u8; bytes]).expect("a file");
        }
        std::fs::write(root.join("Local State"), b"{}").expect("a file");

        let found = profiles(&root, &["w1".to_owned()]);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            found,
            [
                Profile {
                    dir: "ket-main-0123456789ab".into(),
                    bytes: 5
                },
                Profile {
                    dir: "old-branch-ba9876543210".into(),
                    bytes: 3
                },
                Profile {
                    dir: "w1".into(),
                    bytes: 2
                },
            ]
        );
    }
}
