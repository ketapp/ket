//! Refs: `@eN`, the handle `ket browser snapshot` gives each node it shows,
//! which `click`, `fill`, `scroll` and `wait` take.
//!
//! A ref names a node by its backend DOM id, which outlives snapshots, and
//! the frame whose document it is in: the page's own, or a frame's in it.
//! Each tab keeps the refs it has handed out — a tab in its window, as two
//! windows number their tabs alike — for the document they were handed out
//! in: a node keeps its number from one snapshot to the next, and
//! when the main frame commits another document — its loader changes, a
//! reload included — they start again from `@e1`. A ref from before that
//! names nothing, and says so; so does one whose frame has loaded another
//! document since, whose node is gone with it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use gpui::EntityId;

use crate::browser::BrowserId;

/// The most refs one document is handed. Past it a node has none, rather
/// than a number some other node had — a ref never changes what it names.
const REFS_MAX: usize = 10_000;

/// How many tabs' refs are kept; the one used longest ago goes first.
const TABS_KEPT: usize = 32;

/// Whose refs they are: a browser tab, in its window. A tab's id is only
/// unique within its window — every window's first tab is 0 — so its
/// window, by the entity of the shell it shows, is part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Tab {
    window: u64,
    browser: u64,
}

impl Tab {
    /// Tab `browser` in the window whose shell is `window`.
    pub(super) fn new(window: EntityId, browser: BrowserId) -> Self {
        Self {
            window: window.as_u64(),
            browser: browser.0,
        }
    }
}

/// Every window's tabs' refs.
static REFS: Mutex<Store> = Mutex::new(Store::new());

fn store() -> MutexGuard<'static, Store> {
    REFS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every tab's refs, with when each was last used.
struct Store {
    /// Counts uses, for which tab's refs were used longest ago.
    clock: u64,
    /// By tab: when its refs were last used, and the refs.
    tabs: BTreeMap<Tab, (u64, Refs)>,
}

impl Store {
    const fn new() -> Self {
        Self {
            clock: 0,
            tabs: BTreeMap::new(),
        }
    }

    /// Does `act` to `tab`'s refs, making it some if it has none.
    fn with_refs<T>(&mut self, tab: Tab, act: impl FnOnce(&mut Refs) -> T) -> T {
        self.clock += 1;
        let now = self.clock;
        if !self.tabs.contains_key(&tab) && self.tabs.len() >= TABS_KEPT {
            let oldest = self
                .tabs
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(tab, _)| *tab);
            if let Some(oldest) = oldest {
                self.tabs.remove(&oldest);
            }
        }
        let (used, refs) = self.tabs.entry(tab).or_default();
        *used = now;
        act(refs)
    }

    /// The node ref `n` names in `tab`, if it names one in document
    /// `loader`.
    fn node(&self, tab: Tab, n: u32, loader: &str) -> Option<Named> {
        self.tabs.get(&tab)?.1.node(n, loader)
    }
}

/// The refs one tab has handed out, for one document.
#[derive(Debug, Default)]
pub(super) struct Refs {
    /// The document's loader.
    loader: String,
    /// Each ref's node: `@e1`'s first.
    nodes: Vec<Named>,
    /// Each node's ref.
    of: HashMap<Named, u32>,
}

/// What a ref names: a node, by its backend DOM id, in the document of a
/// frame, by its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Named {
    /// The frame.
    pub frame: String,
    /// The node.
    pub node: i64,
}

impl Refs {
    /// Makes these the refs of document `loader`: as they are if they were,
    /// none if they were another's.
    pub(super) fn on(&mut self, loader: &str) {
        if self.loader != loader {
            *self = Self {
                loader: loader.to_owned(),
                ..Self::default()
            };
        }
    }

    /// The ref of `node`, in `frame`'s document, handing it the next if it
    /// has none; `None` once [`REFS_MAX`] have been handed out.
    pub(super) fn of(&mut self, frame: &str, node: i64) -> Option<u32> {
        let named = Named {
            frame: frame.to_owned(),
            node,
        };
        if let Some(n) = self.of.get(&named) {
            return Some(*n);
        }
        if self.nodes.len() >= REFS_MAX {
            return None;
        }
        self.nodes.push(named.clone());
        let n = u32::try_from(self.nodes.len()).ok()?;
        self.of.insert(named, n);
        Some(n)
    }

    /// The node ref `n` names, if it names one in document `loader`.
    pub(super) fn node(&self, n: u32, loader: &str) -> Option<Named> {
        if self.loader != loader {
            return None;
        }
        let at = usize::try_from(n).ok()?.checked_sub(1)?;
        self.nodes.get(at).cloned()
    }
}

/// Does `act` to `tab`'s refs, making it some if it has none.
pub(super) fn with_refs<T>(tab: Tab, act: impl FnOnce(&mut Refs) -> T) -> T {
    store().with_refs(tab, act)
}

/// The node ref `n` names in `tab`, if it names one in document `loader`.
pub(super) fn node(tab: Tab, n: u32, loader: &str) -> Option<Named> {
    store().node(tab, n, loader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_hold_for_a_document_and_start_again_with_the_next() {
        let mut refs = Refs::default();
        refs.on("loader-1");
        assert_eq!(
            (refs.of("MAIN", 105), refs.of("MAIN", 111)),
            (Some(1), Some(2))
        );
        // The same nodes in a later snapshot keep their numbers; a new one
        // gets the next, and so does a node in a frame.
        refs.on("loader-1");
        assert_eq!(
            (
                refs.of("MAIN", 111),
                refs.of("MAIN", 200),
                refs.of("MAIN", 105),
                refs.of("FRAME", 105)
            ),
            (Some(2), Some(3), Some(1), Some(4))
        );
        let named = |frame: &str, node| Named {
            frame: frame.to_owned(),
            node,
        };
        assert_eq!(refs.node(2, "loader-1"), Some(named("MAIN", 111)));
        assert_eq!(refs.node(4, "loader-1"), Some(named("FRAME", 105)));
        // Never handed out, or from another document: nothing.
        assert_eq!(refs.node(9, "loader-1"), None);
        assert_eq!(refs.node(0, "loader-1"), None);
        assert_eq!(refs.node(2, "loader-2"), None);
        // Another document starts again from @e1.
        refs.on("loader-2");
        assert_eq!(refs.node(2, "loader-1"), None);
        assert_eq!(refs.of("MAIN", 111), Some(1));
        // Full, a new node gets no ref rather than another's number.
        for node in 0..(REFS_MAX as i64) {
            refs.of("MAIN", 1_000 + node);
        }
        assert_eq!(refs.of("MAIN", 5), None);
        assert_eq!(refs.of("MAIN", 111), Some(1));
    }

    fn tab(window: u64, browser: u64) -> Tab {
        Tab::new(EntityId::from(window), BrowserId(browser))
    }

    #[test]
    fn the_tabs_used_longest_ago_lose_their_refs_first() {
        let mut store = Store::new();
        for n in 0..=(TABS_KEPT as u64) {
            store.with_refs(tab(1, n), |refs| {
                refs.on("doc");
                refs.of("MAIN", 42);
            });
        }
        let at = |n| store.node(tab(1, n), 1, "doc").map(|named| named.node);
        assert_eq!(at(0), None, "the first tab's went");
        assert_eq!(at(1), Some(42));
        assert_eq!(at(TABS_KEPT as u64), Some(42));
        assert_eq!(store.node(tab(1, 1), 1, "another doc"), None);
    }

    #[test]
    fn two_windows_first_tabs_keep_refs_of_their_own() {
        let mut store = Store::new();
        let (left, right) = (tab(1, 0), tab(2, 0));
        assert_ne!(left, right);
        store.with_refs(left, |refs| {
            refs.on("left-doc");
            refs.of("MAIN", 42);
        });
        // The other window's tab 0, on another page: its snapshot starts
        // its own refs, and leaves the first window's be.
        store.with_refs(right, |refs| {
            refs.on("right-doc");
            assert_eq!(refs.of("MAIN", 77), Some(1));
        });
        let at = |store: &Store, tab, loader| store.node(tab, 1, loader).map(|named| named.node);
        assert_eq!(at(&store, left, "left-doc"), Some(42));
        assert_eq!(at(&store, right, "right-doc"), Some(77));
        assert_eq!(at(&store, right, "left-doc"), None);
        assert_eq!(at(&store, left, "right-doc"), None);
        // On the same page in both, still each its own.
        store.with_refs(right, |refs| {
            refs.on("left-doc");
            refs.of("MAIN", 88);
        });
        assert_eq!(at(&store, left, "left-doc"), Some(42));
        assert_eq!(at(&store, right, "left-doc"), Some(88));
    }
}
