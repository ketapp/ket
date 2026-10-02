# alacritty_terminal 0.26.0, patched for ket

Vendored from crates.io and wired in through `[patch.crates-io]` in the
workspace manifest. Not upstreamed, by decision.

The only change is three read-only accessors on `Term`, in `src/term/mod.rs`,
each marked `ket patch:`:

- `inactive_grid()` — the screen that is not being shown
- `scroll_region()` — the DECSTBM region
- `is_tab_stop(column)` — the tab stops

Terminal checkpoints (`ket-core::terminal::checkpoint`) need all three to
rebuild a screen on another client. Without them a checkpoint taken while a
full-screen program runs loses the screen underneath it.

Removed from the published crate: `tests/` (46 MB of recordings) and its
`[[test]]` target, plus cargo's registry bookkeeping files.

To move to a new upstream version: vendor it again, re-apply the three
accessors, and diff against this copy.
