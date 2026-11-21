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

- The phone tells you when an agent is waiting on you, finishes or fails,
  or a pull request's checks fail, even with ket closed: your Mac sends
  the notifications you chose in the phone's Settings › Notifications
  through ket's push service, sealed on the Mac so only your phone can
  read them. A Mac sends one only while the phone isn't connected to it,
  and only if it is still so 20 seconds later, so a prompt answered at the
  Mac doesn't buzz the phone too. While you're using the Mac it holds them,
  and once it has gone two minutes untouched it sends what is still so. Tap one to open its chat; one that's no
  longer so is taken back when the phone next hears from that Mac.
  Settings › Devices on the Mac marks the phones it notifies, and its Send
  test button pushes one a test notification at once and says what Apple
  made of it. `push = false` under `[phones]` in ket's config turns it off.
  The phone app now needs iOS 17 or later.
- A worktree whose agent has answered wears a dot until you look: showing
  its tab on the Mac, or opening its chat or terminal on the phone, clears
  it everywhere. Sending it a message does not.
- Mark a worktree as unread to come back to it: Mark as Unread is in its
  right-click menu on the Mac and in the menu you get by holding its row on
  the phone. Its row gets the dot again and shows under the phone's Unread
  filter until you next go to it, on either.
- When the phone app hits an error, the screen says so and offers Try
  again instead of closing, and the app keeps a report you can share from
  Settings › About › Error reports. Nothing is sent anywhere unless you
  share it.
- Mute a kind of flag in the Network view, such as emails or IP addresses:
  press Mute on its alert, use the bell on a flagged request, or turn it off
  under Flags in Settings › Network. A muted kind no longer raises alerts or
  tints requests. Requests are still logged in full, and turning the kind
  back on brings its flags back.
- A Claude session you give a colour with `/color` wears it in ket: its
  worktree's row in the sidebar shows Claude's mark on a tile of that colour,
  with the name in it, however the sidebar is sorted, and so do its rows on
  the phone.
- Give a project its own favicon or logo: choose an image in its settings,
  and its badge shows the picture in the sidebar and everywhere else ket
  draws it on the Mac. Remove it to go back to the icon. The phone still
  shows the icon or letter.
- Rename a worktree from the phone: Rename… is in the menu you get by
  holding its row, and in the menu on its screen. As on the Mac, it changes
  the name ket shows, not the branch or the folder.

### Improved

- The phone app now runs on iOS and Android only. Open Pair a desktop
  to scan the Mac's QR code or paste the pairing code.
- The phone's chat header is just the worktree's name: the running, waiting
  or starting word above it repeated what the chat already shows. An agent
  that failed says so in the note under the header instead.
- Coming back to the phone app is quicker: it keeps its connection to your
  Mac when that still works, instead of reconnecting every time, so a chat
  shows what is new straight away. A connection that died while the app was
  away is replaced sooner too.
- The phone's Worktrees filters are now All, Active, Unread and Ready.
  Unread lists the worktrees whose agent has answered since you last
  looked; Ready lists quiet ones with commits, changes or an open pull
  request to land. What waits on you stays on the Needs you tab.
- On the phone, a worktree's status draws the agent's backticks as code and
  its asterisks as emphasis, instead of showing the marks.
- On the phone, a chat's notes, such as "Switched to Extra high" or
  "Copied", show in a capsule under the header, over the conversation,
  instead of in a line above the composer that ran into the chat. Tap one
  to put it away.
- On the phone, a back gesture from Usage returns to Worktrees.
- Chats on the phone open faster, onto their newest messages, and a long
  conversation no longer slows the chat down: only what is on screen is
  drawn.
- On the phone, a project's heading stays at the top of the list while you
  scroll through its worktrees.
- ket asks about a cold session by what re-caching it costs, not by its
  size: from $2 at API prices unless you pick another amount, $1 to $10,
  in Settings › Economy.
- Sorted by status, worktrees with the most recent prompts, finished turns
  or requests for input appear first within each status, in both the expanded
  and collapsed sidebar. Individual tool calls leave that order steady.
- Project settings is laid out in two columns, how the project looks beside
  how it behaves: on/off settings are switches, the icon has its own menu
  you can filter by name, and an ⓘ beside each setting says what it does.
  Remove project is in the project's menu.
- Add project is laid out the same way, under the folder you choose, and
  can give the new project its own image too.
- Every dialog has a ✕ in its top-right corner that closes it, the same
  as Cancel or Escape.
- Project badge pictures are redrawn from an icon set made for small sizes,
  so they read more clearly in the sidebar and on the phone. The telescope
  is now a magnifying glass and orbit a ringed planet.
- ket's download is smaller: it carries only the two fonts it uses, Fira
  Code and Geist. If you picked Monaspace, Mona Sans, Geist Mono or Atkinson
  Hyperlegible Mono in Settings, install it on your Mac to keep using it.
- Branch names, numbers and labels are set in Geist on the Mac and the
  phone, with numbers that keep their width as they change. Fira Code stays
  for the terminal, the editor and code.
- The sidebar gives its projects and worktrees more room.
- Sorted by status, the sidebar lists each worktree with just its name and
  its project, with more space between rows.
- Start fresh on a session that went cold clears it in the same tab, as
  `/clear` does, rather than opening a new tab beside it, and on the phone
  the chat moves to the new conversation. The old one stays in `/resume`,
  and anything left typed in the box is cleared first, for Up to bring back.
- Settings opens taller and a little wider, so a long section shows more at
  once.

### Fixed

- Discarding a file's changes and sending feedback close as you click, and
  a notification says how it went; feedback that doesn't go keeps its
  draft. The Merge and Open Pull Request dialogs, and Add Project's folder
  choice, no longer hold the window while git is read: the menu goes at
  once and the dialog fills in as the answers arrive.
- Renaming, pinning or changing the economy of a worktree, and pinning,
  hiding or showing a project, take effect the moment you choose them —
  the rename dialog closes as you press Enter — instead of after a pause
  while the sidebar was rebuilt. If one can't be saved, a notification says
  why and nothing changes. Deleting a worktree closes its dialog at once
  too: the row says "deleting" until it's gone, and the dialog comes back
  only if there's work it would lose.
- ket no longer stops taking keys after a browser tab or the iPhone
  simulator lets go of the keyboard. Every key in a terminal beeped and
  went nowhere until ket was restarted; ket now takes the keyboard back.
- A Codex task no longer stalls when Codex asks to review new or changed
  hooks before it starts: the phone asks instead, with Trust all and
  continue or Continue without trusting, and the task goes in once it is
  answered. Before, the question sat in Codex's terminal and the task was
  never sent.
- The phone chat's + menu shows on the first tap. Sometimes the + turned to
  × with no menu drawn, and only tapping it again and again brought it up.
- Tapping a worktree twice on the phone opens its chat once: Back no longer
  has to step through a second copy of the same chat, terminal or worktree.
- The phone keeps the pairing code field and Pair button above the keyboard.
- Changing Claude's model from the phone no longer says Claude is busy when
  it is waiting at its prompt in a new session, or in one that has just been
  compacted or started fresh.
- A Grok chat on the phone that stopped on an error, such as running out of
  usage, now ends with Grok's message saying so instead of just stopping.
- On the phone, with the keyboard up, a chat scrolled behind the message
  field no longer shows through it under a dark band.
- An OpenCode chat on the phone now says when OpenCode is trying its model
  again, as a local model reading a long conversation often makes it, and
  when a step ended on an error, such as Ollama not running, instead of
  showing nothing.
- When a Codex subagent asks permission, the phone shows the prompt to
  answer, with a notification, instead of only marking the subagent blocked.
- On the phone, a subagent's name starts on the same edge as its worktree's,
  and its waiting word sits with the worktree's.
- Codex's folder trust question can be answered from the phone, and the
  first message stays queued until you answer it, even after two minutes.
- A Codex subagent no longer reads “waiting” after its prompt was declined
  or its turn was interrupted, when there was nothing left to answer.
- A new worktree’s task waits through Codex’s update picker and restart
  instead of accepting Update now and losing the prompt. If Codex changes
  screens during submission, ket stops and keeps the prompt in the error details.
- A new worktree’s task is sent to Codex again. ket typed it in, then refused
  to press Enter and said “Codex changed screens. Your prompt was not
  submitted.”
- After Start fresh on the phone, attaching the tab to a background session
  shows that conversation instead of leaving the chat empty.
- Start fresh from the phone clears the chat even before the new session
  has any messages, and wakes an inactive Mac window to handle the request.
- On the phone, answering a prompt that was already answered clears its card,
  waiting status and notification instead of offering the same prompt again.
- Agent Permissions in Settings now applies to new and resumed Codex terminals:
  Yolo lets Codex act without approval prompts, and Manual restores its normal
  workspace sandbox and approval prompts.
- The sidebar's subagent tree keeps a continuous line from its parent through
  each branch, without gaps between the connectors.
- OpenCode `/clear` and `/new` clear the phone chat too: the old conversation
  no longer stays on screen after the terminal goes back to home.
- A session Claude Code compacted while it sat idle is no longer said to
  have gone cold at its size from before: ket counts what the compaction
  left, which is seldom enough to ask about.
- Claude, Codex and OpenCode wear their own logos in ket, on the Mac and
  the phone: Claude's spark, OpenAI's logo for Codex, and OpenCode's mark in
  one colour.
- Removing ket's hooks from your agents says when an agent's settings file
  could not be read, instead of saying ket is still in it.
- The file where ket's hooks keep agent updates for later no longer grows
  without limit: it skips updates from agents ket didn't start, which ket
  never used, and stops at 8 MB, even if ket is deleted with its hooks
  still in place.

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
