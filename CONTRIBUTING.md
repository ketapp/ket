# Contributing to ket

Thanks for helping improve ket. Please open an issue before a large change so
the design and scope can be agreed before substantial work begins.

## A change, start to finish

1. **Start from an issue.** Find or open one at
   https://github.com/ketapp/ket/issues. A `feat` or `fix` names the issue
   it closes, so it needs one before the commit, not after.
2. **Fork and clone.** Fork the repository on GitHub, clone your fork, and
   turn the commit check on once per clone:

   ```sh
   git clone https://github.com/<you>/ket.git && cd ket
   git config core.hooksPath .githooks
   ```

3. **Branch from the issue.** `gh issue develop 142 --checkout` names the
   branch `142-…`, which is what ket's `{issue}` commit placeholder reads.
   Any branch name works; the number in the commit message is what matters.
4. **Set up and build.** See *Development setup* below, then:

   ```sh
   cargo build -p ket-cli
   cargo build -p ket-ui
   ```

5. **Run it against disposable state**, never your own:

   ```sh
   export XDG_DATA_HOME="$(mktemp -d)"
   cargo run -p ket-cli -- project add .
   cargo run -p ket-ui
   ```

6. **Make the change**, keeping the *Architectural invariants* below. Add or
   update tests when behaviour changes.
7. **Run the checks** in *Before submitting*. CI runs the same ones on the
   pull request.
8. **Commit** in the format under *Commit messages*. The hook refuses a
   subject that does not fit, and nothing else: it asks nothing of the
   content.

   ```sh
   git commit -m "fix(editor): triple-click selects the line, not the line break (#142)"
   ```

9. **Push and open a pull request** against `main` with the same one-line
   title, `Closes #142` in the description, and what you did to verify it.
   Pull requests are squash-merged, so the title is the commit that lands.

`main` is updated by the maintainers in batches rather than one commit at a
time, so rebase onto it before opening the pull request and again if it
moves while yours is open.

## Development setup

The desktop application is developed and released on macOS. Install Xcode (the
full app, not only its command-line tools — GPUI compiles its Metal shaders
with `xcrun metal`), Git, Node.js 22 or newer, and the Rust toolchain declared
in `rust-toolchain.toml`.

```sh
cargo check --workspace
cargo run -p ket-cli -- --help

npm ci --prefix mobile/remote
npm ci --prefix mobile/app
npm run typecheck --prefix mobile/remote
npm run typecheck --prefix mobile/app
```

Never run ket against another user's state while developing. Use a disposable
data directory:

```sh
export XDG_DATA_HOME="$(mktemp -d)"
cargo run -p ket-cli -- project add .
```

This does not sandbox Git. A command that creates worktrees also registers them
in the source repository; remove those worktrees explicitly when finished.

## Architectural invariants

Code comments refer to these by number. A change that needs to break one is
the wrong change.

1. **All state and computation live in `ket-core`.** The shell is a renderer:
   it receives view-models and emits intents.
2. **The shell never touches the filesystem or git.** Enforced by crate
   boundary: `ket-ui` depends on `ket-core` and never lists a git or
   filesystem crate in its manifest.
3. **Core is UI-agnostic.** `ket-core` compiles and is fully exercisable
   without a window. The CLI is a first-class client, not a debug tool.
4. **Every state is escapable.** No loading that can't unstick, no optimistic
   state that never reconciles, no agent session that can't be killed.
5. **Contracts are verified against the authoritative source.** Worktree
   behaviour is checked against real `git`, not against assumptions about it.

Every structure that accumulates — scrollback, event channels, diffs, status
lists, layout blobs — has an explicit bound and a policy for when it is full.

## Before submitting

Run the checks relevant to your change:

```sh
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

npm run typecheck --prefix mobile/remote
npm test --prefix mobile/remote
npm run typecheck --prefix mobile/app
npm run lint --prefix mobile/app
npm test --prefix mobile/app -- --runInBand
```

When the phone protocol changes, regenerate and commit its TypeScript output:

```sh
npm ci --prefix mobile/remote
npm run generate --prefix mobile/remote
git diff --exit-code -- mobile/remote/src/gen
```

Keep changes focused, update user-facing documentation, and include tests when
behavior changes. Do not commit credentials, transcripts, local ket state, or
generated native Expo projects.

## Commit messages

Every commit and pull request title is one line:

```text
type(scope): what changed (#issue)
```

```text
feat(backlog): ⇧⌘A quotes the selection on a web page (#138)
fix(editor): triple-click selects the line, not the line break (#142)
chore(deps): bump tokio to 1.48
```

- `type` is one of `feat`, `fix`, `perf`, `refactor`, `docs`, `chore`, `ci`,
  `build` or `revert`. `feat` and `fix` end with the issue they close; the
  others may.
- `scope` is optional: the area it touches, such as `editor`, `backlog` or
  `phone`.
- Start from the issue: in ket, make the backlog note a GitHub issue (or
  switch the project to follow its issues) and Start it; outside ket,
  `gh issue develop 142 --checkout`. Either way the branch begins `142-`,
  and ket's `{issue}` commit template placeholder reads the number back
  from it.
- Pull requests are squash-merged, so the title is what lands on `main`. Put
  `Closes #142` in the description so merging closes the issue.

Turn the local check on once per clone:

```sh
git config core.hooksPath .githooks
```

CI runs the same check, `scripts/check-commit-msg.sh`, on every pull request
title.

## Dependency policy

- Commit every Rust and npm lockfile.
- Pin executable agent adapters to an exact version; do not use an unversioned
  `npx` package.
- Explain new runtime dependencies and minimize enabled features.
- Preserve license notices for vendored code and assets; update `NOTICE.md`.
- Run `cargo audit`, `cargo deny check`, and npm audit before a release.
- Never read or copy credentials from another application during development.

## Pull requests

Describe the user-visible result, security implications, verification, and any
known limitations. By contributing, you agree that your contribution is
licensed under the repository's MIT license.
