//! Parts of the shell drawn as cached views of their own.
//!
//! gpui rebuilds a view — its elements, their layout, their paint — whenever
//! that view or anything beneath it changes, and the shell is one view with
//! everything in it. A terminal pane is a cached child view, but when its
//! program writes, gpui marks the pane *and every view above it* as changed,
//! so the shell rebuilt the whole window for each echo: the sidebar's rows
//! laid out again, identical to the last frame, for a character typed in a
//! terminal beside them.
//!
//! A [`Region`] is a cached view whose content is still built by a `Shell`
//! method, from the shell's own state. Being a sibling of the pane rather
//! than its parent, it is not dirtied by the pane, and gpui replays its last
//! frame instead of building it. It rebuilds when the shell itself is
//! notified — which is what every change to the state it draws already does
//! — and when gpui refreshes the whole window: focus, activation, a drag
//! starting. A change inside it, a hover say, notifies the region directly.

use gpui::{
    AnyElement, AnyView, Context, Entity, IntoElement, Render, StyleRefinement, Subscription,
    WeakEntity, Window, div,
};

use crate::Shell;

/// Builds a region's content from the shell.
pub(crate) type Build = fn(&mut Shell, &mut Window, &mut Context<Shell>) -> AnyElement;

/// A part of the shell gpui can reuse between frames. See the module docs.
pub(crate) struct Region {
    /// Whose state this draws.
    shell: WeakEntity<Shell>,
    /// What it draws.
    build: Build,
    /// Rebuilds this region whenever the shell is notified.
    _follow: Subscription,
}

impl Region {
    /// A region drawing what `build` makes of `shell`.
    pub(crate) fn new(shell: &Entity<Shell>, build: Build, cx: &mut Context<Self>) -> Self {
        Self {
            shell: shell.downgrade(),
            build,
            _follow: cx.observe(shell, |_, _, cx| cx.notify()),
        }
    }

    /// The region as an element of the shell's tree, cached.
    ///
    /// `outer` is the box the region occupies in its parent's layout: a
    /// cached view is laid out as a single node until it is rebuilt, so the
    /// size its content would have asked for has to be stated here.
    pub(crate) fn element(this: &Entity<Self>, outer: StyleRefinement) -> AnyElement {
        AnyView::from(this.clone()).cached(outer).into_any_element()
    }
}

impl Render for Region {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let build = self.build;
        self.shell
            .update(cx, |shell, cx| build(shell, window, cx))
            // The shell outlives every window it draws, so this is a frame
            // drawn during teardown at most.
            .unwrap_or_else(|_| div().into_any_element())
    }
}
