# Changelog

What changed in each release of ket, for the people using it. This file is the
only source: the release script files `Unreleased` under the new version, and
the same text becomes the update's notes, the app's What's new and the site's
changelog page.

Each release is `## <version> — <YYYY-MM-DD>`, with `- ` bullets under
`### New`, `### Improved` and `### Fixed`; a bullet runs on over indented
lines. Write for users: what they can now do or no longer hit, in plain
words; no internal names, no commit hashes. ket reads this file
(`ket_core::changelog`), so keep to that shape.

## Unreleased

### New

- When the phone app hits an error, the screen says so and offers Try
  again instead of closing, and the app keeps a report you can share from
  Settings › About › Error reports. Nothing is sent anywhere unless you
  share it.

### Improved

- ket's download is smaller: it carries only the two fonts it uses, Fira
  Code and Geist. If you picked Monaspace, Mona Sans, Geist Mono or Atkinson
  Hyperlegible Mono in Settings, install it on your Mac to keep using it.

## 0.2.0 — 2026-11-12

### New

- Run coding agents in parallel across your projects, each in its own Git
  worktree: a separate checkout on its own branch, where the agent can change
  files without touching your main checkout. The sidebar lists each
  project's worktrees and what their agents are doing. ket runs on Macs with
  Apple silicon and macOS 13 or later.
- On first launch, ket lists the coding agents it finds on your Mac and
  helps you install or sign in to one, then add your first project.
- Start a task by writing a prompt in the New worktree dialog, and ket makes
  a worktree with a branch named from the prompt and starts the agent on it
  with the model and effort you pick. Leave the prompt empty for a worktree
  with a shell.
- ket starts the coding agent you already use, such as Claude Code, in a
  terminal tab of the worktree, and you can add any other agent by its
  command in ket's config file. For Claude Code, Codex, Grok and OpenCode,
  the sidebar shows which agents are working and which are waiting on you,
  and ket posts a macOS notification when one stops on a permission prompt
  while you're in another app.
- Run a model on your Mac as a coding agent. In Settings → Agents → On this
  Mac, ket lists the Ollama models that can work as agents and offers
  downloads that fit your Mac's memory, then runs the model through
  OpenCode, which ket limits to your Mac's Ollama.
- Send a prompt to any project from wherever you are with the quick prompt
  (⌘⇧P), into one of its worktrees or a new one. Dictate it if you'd rather
  speak; for common languages, your Mac transcribes it without sending the
  audio anywhere.
- Keep a backlog of notes for each project, and press Start to give a note
  to an agent in a new worktree. Turn on GitHub sync in a project's settings
  and ket keeps its notes in step with its open issues.
- Put an idle agent to sleep from a worktree's menu to free its memory while
  its tab stays open, and wake it into the same conversation. Turn on
  automatic sleep in Settings → General and choose how long ket waits.
- Open the Git panel to see a worktree's uncommitted changes and what its
  branch would bring into its base, and click a file for its diff. Pick
  lines in a diff to comment on them, then send your comments to the agent
  as one message.
- Merge a worktree from its row: ket commits its changes after showing you
  the files, and merges the branch into its base in your main checkout. If
  git finds a conflict, ket aborts the merge and leaves your checkout as it
  was, and you can choose Merge with agent to have the worktree's agent
  finish the job.
- Push a worktree and open a pull request as the project's GitHub account,
  from the worktree's menu or the merge dialog, once you sign in under
  Settings → GitHub. The worktree's row then shows the pull request's
  checks, red when one fails. Choose Fix checks with agent or Address
  comments with agent to send the agent what to fix.
- Each worktree has its own tabs, which you can split into panes. Terminals
  keep running after you quit ket, and ket reattaches them when you open it
  again.
- Browse and search a worktree's files from the right panel, then edit a
  file or view an image in a tab. Press ⌘P to find a file, or type :122
  there to jump to line 122 of the file you have open.
- Open a browser tab beside your agents to watch the app they build. Tabs
  run Chromium, and each worktree keeps its own sign-ins and site data. The
  code button opens DevTools in a separate window, and the camera button
  sends a screenshot of the page to the worktree's agent.
- Agents can drive their worktree's browser tab with the ket browser
  command, clicking and typing in a page with real mouse and keyboard events
  and reading what it logged. They can save a screenshot, or record a video
  of the tab to show you what happened.
- Open the iOS Simulator or an Android emulator as a tab beside the agent
  building your app, and press ⌘S to send the agent a screenshot. You need
  Xcode or the Android SDK installed.
- See in the header how much of your plan each agent has used and when it
  resets, for Claude Code, Codex and Grok, and for OpenCode Go once you add
  its key. The status bar shows the memory and CPU that ket and your agents
  use; click it for the last ten minutes, by agent and terminal.
- Set a worktree's Economy level, from Off to Max, to ask Claude Code or
  Codex to write less, and ket estimates what each session cost and what
  Economy saved. Reopen a Claude Code session whose prompt cache has expired
  and ket tells you what resuming it costs, and offers a fresh session
  beside it.
- Turn on Log what agents send in Settings → Network, and ket keeps what
  Claude Code and Codex send to their model providers on this Mac for five
  days. The Network view lists the requests from agents you start after that
  and flags any that carried something shaped like a secret or a personal
  identifier, such as an API key or a card number. From a flagged request
  you can stop agents reading the file it came from.
- Pick a theme and choose the interface and code fonts in Settings →
  Appearance. Give a project a colour and a glyph for its badge, and pin it
  to the top of the sidebar or hide it from its right-click menu. A hidden
  project comes back while one of its agents needs you. Save prompts you
  reuse as snippets and send them to an agent with ⌘/.
- ket tells you when a newer version is out, with a Download button that
  opens the download page. It looks on the schedule you pick in Settings →
  General, and Check now there looks straight away.
- Use the ket command to script what the app does, from adding projects to
  opening pull requests, and agents in ket can run it too, so you can ask
  one to read the backlog or start a task. For a bug report, choose Help →
  Export Diagnostics, or run ket diagnostics if the app won't open, and ket
  saves a zip with passwords and API keys taken out.
- Settings → Devices pairs the ket app for iPhone and iPad, which isn't on
  the App Store yet.
