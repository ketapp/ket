# ket

Run coding agents in parallel across isolated Git worktrees, review what they
produce, and keep the best result.

ket is built for moving among many projects and several agents—Claude Code,
Codex, OpenCode, and Grok—without treating any one agent as the center of the
workflow. The desktop shell is native Rust and GPUI, not Electron. Optional
browser tabs use the system WKWebView.

> ket launches tools that can execute commands as your user. Only enable
> repository automation after you trust the repository. Worktrees isolate Git
> changes; they are not security sandboxes.

## Supported platforms

The desktop application and release bundle currently target macOS 11 or newer.
Core and CLI code may compile elsewhere, but non-macOS desktop behavior is not
a supported release target. The companion phone application uses Expo for iOS,
Android, and development web builds; its normal connection is to a relay on the
same local network as the Mac.

## Workspace layout

| Path | Role |
|---|---|
| `crates/ket-core` | Projects, worktrees, agents, persistence, terminal host, and policy |
| `crates/ket-cli` | The `ket` command-line client |
| `crates/ket-ui` | Native GPUI desktop application |
| `crates/ket-remote` | Noise session and phone application protocol |
| `crates/ket-relay-protocol` | Bounded relay framing |
| `crates/ket-relay` | Local/development WebSocket relay |
| `crates/ket-android` | Android-facing Rust integration |
| `mobile/remote` | TypeScript remote-protocol client |
| `mobile/app` | Expo phone application |
| `vendor` | Locally patched GPUI and Alacritty terminal crates |

## Build and run

Install Xcode, Git, Node.js 22 or newer, and Rust. The full Xcode app is
needed, not only its command-line tools: GPUI compiles its Metal shaders with
`xcrun metal` (on Xcode 26, also install the Metal toolchain from Xcode's
Components settings). The pinned Rust toolchain installs automatically from
`rust-toolchain.toml`.

```sh
cargo check --workspace
cargo build -p ket-cli
cargo build -p ket-ui
```

For development, keep ket away from your normal application state:

```sh
export XDG_DATA_HOME="$(mktemp -d)"
cargo run -p ket-cli -- project add .
cargo run -p ket-ui
```

Creating a worktree still changes the source repository's `.git/worktrees`
registry. Remove development worktrees explicitly when finished.

Logging is controlled by `KET_LOG`, using tracing-subscriber's filter syntax:

```sh
KET_LOG=debug cargo run -p ket-cli -- doctor
KET_LOG=ket_core::event=trace cargo run -p ket-cli -- events --follow
```

## Phone development

```sh
npm ci --prefix mobile/remote
npm ci --prefix mobile/app
npm run typecheck --prefix mobile/remote
npm run typecheck --prefix mobile/app
npm run start --prefix mobile/app
```

See `mobile/app/README.md` for device and native-build instructions.

## Protocol generation

`crates/ket-remote/proto/ket/remote/v1/remote.proto` is the canonical phone
application contract. Rust bindings are generated during the crate build.
Regenerate and commit TypeScript bindings after every schema change:

```sh
npm ci --prefix mobile/remote
npm run generate --prefix mobile/remote
git diff --exit-code -- mobile/remote/src/gen
```

The local desktop-host wire protocol is separate and currently version 5. The
phone application protocol is version 1.7. Compatibility-sensitive changes
must preserve live-host attachment or include an explicit migration plan.

## Quality checks

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

CI also checks generated protocol drift. The scheduled dependency workflow runs
Rust advisory/license checks and npm audits.

## Release process

1. Update versions and user-facing release notes.
2. Run all quality, protocol-drift, advisory, and license checks.
3. Build the application with `scripts/bundle-macos.sh`.
4. Test the bundle using disposable ket state and a disposable repository.
5. For distribution to another Mac, replace the script's ad-hoc signature with
   Developer ID signing and Apple notarization.
6. Publish the matching source revision, lockfiles, `LICENSE`, and `NOTICE.md`.

The repository does not currently automate signing, notarization, or publishing.

## Security

Pairing requires desktop confirmation and grants an explicit Viewer,
Controller, Operator, or Administrator role. Phone traffic is encrypted with
Noise and pins the host key. Local IPC uses owner-only paths and validates
privileged peers. Network frames, handshake concurrency, untracked review
reads, and expensive phone operations are bounded.

Agents still run with the current user's authority, and trusted project hooks
and post-provision commands are code execution. See `SECURITY.md` for private
reporting.

### Outside ket's own data directory

To show what each agent is doing, ket installs hooks into the agents' own
configuration when it starts:

- Claude Code: hook commands in `~/.claude/settings.json`, and a status line
  that runs any status line you already had after its own.
- Codex: hook commands in `~/.codex/hooks.json`.
- OpenCode: a plugin in `~/.config/opencode/plugin/`.
- Grok: a hooks file at `~/.grok/hooks/ket.json`.

These hooks run in every session of those agents, including ones ket did not
start. A report that cannot reach a running ket is kept in the owner-only file
`~/.ket/agent-hooks/spool.jsonl` until ket next starts. Settings → General →
**Remove ket from your agents** takes all of them out again.

To show Claude usage limits, ket reads Claude Code's sign-in from the macOS
Keychain (or `.credentials.json` in Claude Code's config directory) and sends
it only to Anthropic's usage endpoint. It does not read any other agent's
credentials.

## Contributing and licensing

Contributions are welcome; start with `CONTRIBUTING.md` and follow
`CODE_OF_CONDUCT.md`. ket is available under the MIT license. Bundled third-
party code, fonts, icons, and generated assets are listed in `NOTICE.md`.
