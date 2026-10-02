# Working on ket

## Never run ket against your real data directory

`ket` keeps its projects, worktrees and sessions in `$XDG_DATA_HOME/ket`,
falling back to `~/.local/share/ket`. That file belongs to whoever uses ket on
this machine, and it is what the sidebar shows when they open the app.

Running the app or the CLI without an override writes to it. Adding a project
to try out a UI change, or opening a demo worktree, leaves an entry there that
the person did not create and has to be told about. To them it looks like the
app invented projects on its own.

So: before running `ket`, the bundled `ket.app`, or any test that touches real
state, point the data directory somewhere throwaway.

```sh
export XDG_DATA_HOME="$(mktemp -d)"
cargo run -p ket-cli -- project add .
```

`crates/ket-core/src/paths.rs` reads `XDG_DATA_HOME` first, so this is enough —
projects, worktrees, the journal and the layout blob all follow it.

If you find state you did not create in `~/.local/share/ket/state.json`, say so
rather than quietly deleting it: which entries are real is the user's call,
not yours.

### `XDG_DATA_HOME` does not sandbox git

It redirects ket's own state — `state.json`, the journal, the worktrees
directory. It does **not** redirect git. `git worktree add` records every
checkout in the *real* repository's `.git/worktrees`, wherever the checkout
itself lands, so a sandboxed run still leaves registrations in the repository
that outlive the temp directory they point into.

That is not cosmetic. Those registrations are what ket's discovery reads, so a
later run against the real data directory finds worktrees whose path is a temp
directory that has already been reaped, and the user sees broken rows they
never created.

So a sandboxed run that creates worktrees has to clean up after itself:

```sh
git -C <repo> worktree list          # before, to know what is yours
# ... run whatever you were running ...
git -C <repo> worktree remove --force <each path you created>
```

`git worktree prune` is not enough while the temp directory still exists —
prune only clears registrations whose path is already gone.

And never suggest running ket against the real data directory to "see the real
projects". There is no version of that which is safe; seed a persistent
throwaway `XDG_DATA_HOME` with `ket-cli project add` instead.

## Do not read another application's credential files

Not `~/.claude/.credentials.json`, not OpenCode's `auth.json`, not
`~/.codex/auth.json`, not a Keychain item. This is about you, the agent doing
the work: if a task seems to require opening one, stop and report.

ket itself reads exactly one: Claude Code's login, to show Claude usage
(`crates/ket-core/src/rate_limits/claude.rs`). Every other agent's usage is
read without touching its credentials. Widening that is a maintainer's decision,
not an implementation detail.

## Before committing

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D
warnings` have to be clean. The workspace denies `clippy::all` and
`unsafe_code`, and warns on `missing_docs`.

There are five documented `unsafe` carve-outs, and the reason for each is in
the workspace manifest's lint block. Adding a sixth is a maintainer's call,
not yours.

## Say what changed in `CHANGELOG.md`

A change someone using ket would notice — something new, something that
behaves differently, a bug they could hit that no longer happens — adds one
line under `## Unreleased` in `CHANGELOG.md`, in the same commit, under
`### New`, `### Improved` or `### Fixed` (add the heading if it is not there
yet).

Write it for the person using ket, not for the next agent: what they can now
do, in plain words, with no module names or commit hashes. Refactors, lint
fixes, plans and internal tooling get no line. The release notes are built
from this file and nothing else, so a change without a line ships unannounced.

## Build UI from `ket-ui::ui`, never one-off components

Every control in the shell comes from `crates/ket-ui/src/ui/`; its `mod.rs`
lists every recipe, so read that rather than trusting memory or a list here.
Before drawing anything a person clicks, types into or reads as a control, find
the recipe that already does it and use it, sized or placed by the caller (`.w(..)`, `.leading(..)`,
`.read_only()`). A hand-rolled `div` that looks like a button, field, trigger
or menu is exactly the drift that module was written to end.

If nothing there fits, do not invent a local helper or add a new component to
`ui/` on your own: say what is missing and ask. A new shared component is a
maintainer's call.

What each of those recipes looks like — colours, type, sizes, states — is
[`design/STYLE-GUIDE.md`](design/STYLE-GUIDE.md), pictured in
`design/style-guide.html`. Hold new UI to it.
