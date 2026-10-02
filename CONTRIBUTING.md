# Contributing to ket

Thanks for helping improve ket. Please open an issue before a large change so
the design and scope can be agreed before substantial work begins.

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
