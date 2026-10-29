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

- The phone's terminal can send keys one at a time: switch the line at the
  bottom from Message to Live keys and each key you press goes straight to the
  terminal, with no Enter after it. Use it to answer a prompt that takes a
  single key, such as a digit for Claude Code's suggestions, or y and n.
- The New worktree dialog opens on a prompt, like the phone's New task: say
  what the agent should do and ket makes the worktree, names the branch from
  the prompt's first words, and starts the agent on it with the model and
  effort you pick beside the agent. The branch can be typed over, and a
  reset puts the prompt's name back. Leave the prompt empty for a worktree
  with just a shell, as before. Economy is a small menu in the dialog's
  footer now, showing what the chosen level changes.
- When nothing needs you, the phone's Needs you tab shows the planet from
  ket's welcome window rising under the tab bar, drawn in glyphs and turning
  slowly, instead of a lone card on black. It holds still under Reduce Motion
  and while the tab is out of sight.
- Shift-click Delete in a worktree's right-click menu to delete it without the
  confirmation. The row reads "Delete, don't ask" while shift is held, and
  work that would be lost still stops the delete and asks.
- Jump to a line in the open editor from the file finder: press ⌘P, type `:122`,
  and press Return. In a diff tab the same lands on that line of the file as
  it is now, or on the nearest change after it when the line is not part of
  one.
- Your phone can tell you what needs you while ket is not in front: a banner
  when an agent stops on a prompt, when a task finishes or fails, and when a
  check fails on a pull request, with the app's badge counting the prompts
  waiting. Turn it on in Settings → Notifications, with a switch for each
  kind. The phone posts these itself from what your Mac tells it, so they
  arrive while the connection is still up — about half a minute after you
  leave the app — and the rest wait for the next open.
- Arrange your snippets: drag one in the list in Settings → Snippets to a new
  place, or hold ⌥ and press ↑ or ↓ to move the one showing in the editor.
  The order you leave is the order menus offer them in.

### Improved

- The top of the phone's Worktrees screen is as tall as every other screen's
  header, and the desktop's name sits in a rounded bar across its width with
  the lock set into its end. The ket mark beside it shows how the line to the
  desktop is: green when connected, grey while connecting, red when it is
  down.
- The buttons at the top of a chat on your phone, the mode pill and the
  worktree and terminal buttons beside it, are a little taller with larger
  glyphs, so they are easier to hit.
- A terminal’s agent view list on your phone looks like the rest of the app:
  the same margin as every other screen, a terminal button framed like the
  chat’s, and each group as one card with its name and count at the top, as
  on the Needs you tab.
- A worktree’s right-click menu is shorter, with Git actions grouped in a
  submenu and wider menus to give labels more room. Its header says Worktree,
  and it opens directly on its actions, without a search row.

- On the phone, the Mac switcher comes up as a sheet from the foot of the
  screen instead of a menu under the pill, and every sheet — New task, a
  session's settings, the Mac switcher — now rises the same way.
- On your phone's Worktrees list a project's badge and the agent marks under
  it now sit in one column, so every name starts at the same place; rows and
  project headings have a little more room; and the list's card ends above
  the tab bar instead of running off the foot of the screen behind it.
- A worktree's screen on your phone is its changes: the files it changed
  since its base with the lines and commits ahead over them, and Merge or
  Pull request docked at the foot. The figures that repeated the chat and
  the list, and the Terminal and New session buttons, are gone; the
  terminal, a new session, Pin, Delete and the rest of a pull request's
  actions are in the ⋯ menu in the bar, and a pull request's first thing
  to do takes the foot while it is open.
- Tap your Mac's name at the top of Worktrees on your phone or iPad to see
  the Macs paired with it and how each is connected, and switch between them.
  It opens with one Mac too.
- Start task on your phone opens a chat straight away: your task as the
  message on its way, and under it what your Mac is doing — making the
  worktree, then starting the agent — with the seconds counted. It becomes
  the agent's own chat the moment the agent is up, and anything you typed
  meanwhile is kept as the draft. Two minutes without a worktree and it says
  so, with the choice to keep waiting, ask again or let it go. The row on
  Worktrees shows the same stage and opens the chat.
- Your Mac picks up a task from your phone within a moment. It used to wait
  for the window's next refresh, up to half a minute with ket in the
  background.
- Add project no longer stops at a folder that isn't ready. A folder that
  isn't a Git repository offers Create repository; one with no commits yet
  offers Make the first commit; a base branch that doesn't exist offers to
  create it. Each is one button in the dialog, instead of git's error.
- Acid is easier on the eyes: a near-black green desk instead of neon lime,
  with lime and yellow kept for what's lit — running, waiting, the accent —
  in softer shades, and sage for secondary text. On your phone it's no
  longer a lime sheet with pale words on it.
- On the phone, Worktrees no longer carries a prompt box above the tab bar.
  New work starts from the green Start button at the top of the list, which
  opens the full sheet: project, agent, base, model and effort.
- The phone's composer moves: Send lights up green the moment there is
  something to send, launches its arrow on the tap and turns into Stop
  while the agent works; + turns into × as its menu opens; and the + and /
  menus float over the chat, so nothing under them shifts and the keyboard
  stays up. With Reduce Motion on, it all lands at once.
- The green Secure pill at the top of your phone's Worktrees screen is now a
  padlock on the end of your Mac's name. It hangs open and grey while the
  phone connects, and snaps shut and turns green the moment the line to your
  Mac is sealed, on every connect and reconnect. Tap it, as before, for what
  that means. With Reduce Motion on it is simply open or shut.
- In Settings, MCP has a section of its own beside Agents and Permissions: the
  servers the selected worktree's agent CLIs are configured with, the calls ket
  has seen them make, and the commands that set them up — no longer at the
  bottom of the Agents section.

- On your phone, a tab running Claude Code's agent view lists its background
  sessions — waiting on you, working and done — and each opens as a chat you
  can read and reply to. Replying opens that session in agent view on your
  Mac first, so the message never lands in a different one.
- The phone's Worktrees screen looks like your Mac's sidebar: a compact
  filter with New beside it, and the list running the full height under a
  floating tab bar. Worktrees at work open up to show what their agent is
  doing. A quick prompt at the foot starts an agent in the project and with
  the agent you last used.
- Hold a worktree on your phone for its menu: pin it to the top of its
  project, act on its pull request, or delete it. Delete asks first, and
  offers Delete anyway when it would lose uncommitted work. Your
  repository's own checkout can't be pinned or deleted.
- Put idle agents to sleep from a worktree's menu while their tabs stay open, then wake into the same conversation. You can turn on automatic sleep and choose its delay in Settings → General. Asleep agents show on your phone too, with when they slept and what that freed, and a tap wakes one on your Mac and opens its chat — rather than the worktree saying nothing is running until you click the tab on the Mac.
- Merge a pull request, update its branch, mark a draft ready, reopen a closed one, or give its review comments to the agent — from the worktree's menu on your Mac, or from the pull request card on your phone.
- Reach your Mac from your phone over Tailscale away from home, or add an address for another network without pairing again.
- Open a pull request from your phone: Pull request… beside Merge on a
  worktree's screen offers the title and description your Mac's dialog
  would, to edit or dictate, then commits what is uncommitted, pushes the
  branch and opens the pull request — or pushes to the one already open.
  It works with ket's window closed.
- From your phone, choose a base branch, model and effort when starting work, or open a new agent session or shell in an existing worktree.
- When Codex stops on its “Approaching rate limits” notice over a permission
  prompt, the phone shows the notice as a question with Codex's own options and
  answers it for you, instead of an Allow that pressed a key the notice ignored.
- Settings → Agents shows MCP servers configured for Claude Code and Codex in the selected worktree, plus MCP calls ket has recorded there. Check servers runs on demand, with setup commands and troubleshooting links nearby.
- Pick lines in a diff and comment on them. Comments collect in the Git
  panel's Review section until you send them to the worktree's agent as one
  message.
- Comment on a diff from your phone too: tap a line, or a second one in the
  same hunk to take in the lines between, and write what should change. Send
  to agent, under the diff or on the worktree's screen, gives the agent every
  comment as one message, the way the Review section on your Mac does.
- A worktree's pull request shows how its checks are doing: the GitHub mark
  on its row turns red when one fails and green once they all pass, and its
  tooltip says how many. Fix checks with agent, in the worktree's menu or on
  your phone, gives the agent there what the failed checks printed, to fix
  and push. The phone's worktree screen lists the pull request's checks and
  review, and its row wears the number in the checks' colour.
- A hook can refuse an agent's permission prompt: add a command under
  `[hooks] agent_permission_requested` in `config.toml`, or in a trusted
  repository's `.ket.toml`. Exit 0 lets the prompt reach you; any other exit
  refuses the call, with the hook's first line of output as the reason.
- Claude Code and Codex sessions opened in ket are told at the start that
  the `ket` command is there, so you can ask yours to read the backlog or
  start work without explaining ket first.
- `ket start "<prompt>"` and `ket backlog start <note>` start work from a
  shell the way your phone does: the open ket window makes the worktree and
  opens its agent in a tab, and a started note is marked done.
- The status bar shows how much memory and CPU ket itself uses, and beside
  it what your agents and terminals use. Click either for the last ten
  minutes charted, each agent and terminal by name and worktree, and the
  models a local Ollama is holding. Figures turn yellow, orange and red as
  they climb, like your agents' usage limits do.
- The macOS menu bar now offers File, Edit, View, Project, Worktree, Window and Help commands, with shortcuts, availability and checkmarks that follow the current selection.
- The phone app runs full-screen on an iPad. On a wide screen your worktrees
  stay in a sidebar and what you open shows beside them, conversations keep
  to a comfortable width, and with a keyboard attached ⌘Return sends a
  message or starts a task.
- A worktree's work can leave ket as a pull request. Right-click the
  worktree and choose Push and open pull request, or press Pull request… in
  the merge dialog: ket commits what is uncommitted, pushes the branch as
  the project's GitHub account, and opens the pull request with the title
  and description you checked, as a draft if you like. The row then shows
  a mark that opens it on GitHub, and `ket pr` does the same from a shell.
- Right-click → Inspect Element in a browser tab opens Web Inspector, docked
  inside that tab.
- Links clicked in the terminal can now be copied from the link menu.
- The commit message template for merging a worktree can include `{issue}`,
  which becomes `#142` on a branch named `142-…`, the way `gh issue develop`
  names them.
- A backlog note can be made into a GitHub issue from its ⋯ menu, and a note
  that is one starts on a branch named for it, like `142-fix-the-login`.
- A project can keep its backlog in step with its GitHub issues, switched on
  in its settings: open issues arrive as notes, a note is done when its issue
  closes, and marking a note done or deleting it closes the issue.
- Settings has a GitHub section: sign in to one or more GitHub accounts in
  the browser, with no terminal, and ket installs GitHub's command line
  first if it needs it.
- Each project picks which GitHub account its issues use, and its settings
  show the repository they go to.
- Image files (PNG, JPEG, GIF, WebP, BMP, TIFF) open in their own tab, scaled
  to fit, with their size shown underneath.
- Settings → Appearance now sets the interface and code font sizes and
  shows or hides the status bar, and each change is drawn as soon as it
  is made.
- ⌘= and ⌘− step the code font size and ⌘0 puts it back, from the View
  menu too; in Settings, the up and down arrows step a font size box.
- Settings → Appearance picks the interface font and the code font from
  what is installed, with the code list kept to fixed-width faces.
- The Theme picker shows each theme's accent colour beside its name.
- On your phone, a message to an agent can carry photos and files. Tap +
  beside the field for Photos, Camera, Files or a copied image; they wait in
  the field until you send, and the agent gets each photo as an image of its
  own, as if you had dropped it on its terminal at the Mac. Other files reach
  it by path. Needs the Mac's ket updated too.
- The phone's message field has a mic: tap it and what you say is written
  into the message as you speak, with the keyboard down if you like, then
  tap again or just Send. It transcribes on the iPhone, and for a local agent
  never anywhere else.

### Improved

- Deleting a worktree from your phone takes it off the list the moment you
  confirm, instead of some twenty seconds later when your Mac has finished
  with git. If the delete is refused, the row comes back with the reason.
- A terminal on your phone always takes the phone's size now, so its text
  wraps to the screen at a size you can read; the switch for it in Settings
  is gone. The desktop gets its own size back when you leave the terminal.
- On your phone, nothing casts a shadow any more: the tab bar, the quick
  prompt, menus, the New task sheet and the buttons sit flat on the screen.
- On your phone, picking another of a terminal's tabs swaps the terminal in
  place instead of sliding a new screen in.
- ⌘/ in the editor comments out the selected lines, or the line you're on,
  and uncomments them when you press it again, as in VS Code. Everywhere
  else ⌘/ still opens your snippets.
- Worktree names stay as you named them when an agent changes its session title, and their sidebar labels are slightly smaller.
- In Send Snippet (⌘/), the up and down arrows go round the list: down
  from the last snippet goes to the first, and up from the first goes to the
  last.
- On your phone, a terminal's tabs sit at the top in place of its title and
  scroll sideways, so every tab keeps a readable name however many are open,
  and the terminal gets more room. A dot on the agent's mark shows whether it
  is running or waiting, and titles no longer start with the agent's spinner
  or ✳. The "you have control" badge is gone; leaving the terminal still
  hands control back to your Mac.
- A backlog note opened on your phone is easier to work with: its priority is
  a row of four you tap once, what it says grows with the text instead of
  scrolling in a small box, and Start sits at the far end with Mark done
  beside it. Delete is a bin at the other end, away from Start, and still
  asks first. While you have unsaved changes, Save takes Start's place.
- The Feedback, Bug and Idea switch at the top of Send feedback is easier to
  read: each answer has a mark beside it at full size, and the one you chose
  fills its track instead of floating in the middle of it.
- The window is more compact: narrower gaps between the sidebar, the tabs and
  the header, tighter rows and smaller type in the sidebar, a shorter tab bar
  and status bar, and an 11-point terminal, so more of your projects and more
  of each terminal fit on screen. A font size you set yourself is kept.
- When the window is too narrow for the full header, it switches to the
  compact strip on its own and comes back when there is room again; until
  then its expand button is greyed out and says there is not enough room. The
  sidebar, panel and header buttons at its right end are never cut off.
- The Memory and CPU popover reads better and does more: larger figures over
  smoother charts that you can run the pointer along to read any moment, a bar
  beside each line's CPU, Claude listed by name instead of its version number,
  and no footnote. Click a line to go to its tab, use its Stop button to stop
  what runs there, or unload a model Ollama is holding.
- Files clicked in the Git list now share a preview tab. Double-click to keep a diff open, or use Open file to keep the editable file.
- A new terminal opened beside one already in the pane starts at the pane's
  size, so its prompt appears sooner and does not redraw once the shell is
  up.
- The note composer now offers a description as you type, and open notes show
  their full title beside a separate panel for tags and worktree options.
- On the phone, a run of tool calls in a conversation folds into one line
  that counts them and names the tools; tap it to see each one. The agent's
  words read straight through, and the terminal still has all of it.

### Fixed

- Returning to Worktrees on your phone keeps the list in place, including
  its scroll position, filter and folded projects.
- In New task on your phone, a field's menu opens just above that field
  and stays on it while the keyboard goes down. The Base branch, Model and
  Effort menus used to float above the whole form, rows away from what you
  tapped.
- Starting a task from your phone takes you to its chat once: when the
  agent opens, that chat carries on where you are instead of a second
  screen sliding in. Until the agent has your message, the chat shows just
  the message on its way, not "No agent has reported from this terminal
  yet" above it.
- Changing Claude's effort from your phone no longer says it wasn't
  switched when it was. The session sheet now moves its mark to your pick
  as you tap, says at its foot while the change goes through, and then
  says whether it took, or why not.
- The Screenshot to agent dialog no longer offers the snippets menu or its
  shortcut, which did not work correctly with mobile screenshots.
- Clear build output now finds `target`, `node_modules` and `.next`
  directories anywhere in a worktree, not only at its root, so a checkout
  with a package under a subfolder shows them as build output and the menu
  item is there to clear them. A config ket wrote under the old list moves
  to the new one; a list you edited yourself is kept.
- Holding a worktree on your phone no longer nudges its row a pixel as its
  menu opens.
- Moving between screens on your phone or iPad no longer shows white or
  grey behind them: everything behind a screen is drawn in your theme.
- A Claude tab that Claude Code reattached to one of its background sessions
  now opens on your phone as that session's conversation, instead of saying
  the chat couldn't be read until you went back to agent view.
- A worktree name with spaces in it, like `new worktree bug`, now creates the
  branch `new-worktree-bug` instead of the dialog showing git's refusal.
  Anything else git will not have in a branch name is dropped the same way.
  The status line that flashed under the fields while the worktree was
  created is gone.
- When Claude Code backgrounds a session, the phone's chat now says so where
  it stops, instead of leaving a message you sent marked unseen.
- Answer Codex's questions from your phone's chat. When Codex asks you to
  pick between options, the chat shows them as buttons instead of only
  saying it is waiting; a question that needs typed words still opens the
  terminal. When Codex finishes a plan and asks "Implement this plan?", the
  phone shows that too, with Codex's own three choices.
- Switch Codex out of planning mode from your phone in the same session and
  worktree, using the Switch mode popup without leaving chat, or Shift+Tab
  in the terminal's keys.
- Answering a permission from your phone closes the prompt the moment you
  tap, and a second tap no longer types the agent's key again. The Mac's
  sidebar shows the agent running, or stopped if you denied it, as soon as
  you answer rather than when the agent next reports.
- OpenCode chats load on your phone again. OpenCode 2 keeps its sessions
  somewhere new, and the phone said it couldn't read them. A session ket
  reopened for you reads straight away too, rather than saying no agent has
  reported until you type to it.
- Hold any line in a phone chat to copy it, including the ones about the
  chat itself — why it's empty, what went wrong.
- OpenCode's questions reach your phone as a card with their choices, as
  Claude's do, and your pick answers the form on your Mac. A question with a
  free-text field still waits for the terminal.
- A chat's bar shows the mode its agent is in — Claude's permission mode,
  OpenCode's agent, Codex's planning — as a pill beside the name. Tap it to
  pick another: your Mac presses the agent's own shortcut until the agent
  reports the mode you chose. Codex, which never says, gets the toggle
  without a tick.
- Approving a Grok prompt from your phone no longer rejects it. Grok's
  choices move about from one prompt to the next, so ket now reads the menu
  on screen and picks the choice you tapped by name. When Grok offers no
  Always allow for a command, the phone says so instead of guessing.
- Address comments with agent is offered when a reviewer left comments on lines, even if they did not request changes or write a review body.
- An agent sitting at its own prompt no longer shows as busy when ket has not
  heard from it yet, such as a session resumed after ket restarts.
- Starting an OpenCode session no longer fails with “OpenCode listed no
  agents” the first time a worktree is opened after OpenCode's background
  service started: the service answers before it has finished opening the
  worktree, so ket asks it once more rather than take that first answer.
- When several agents run in one worktree, the phone shows each one's prompt in
  its own chat and lists each under Needs you, instead of only the newest.
- Opening a worktree's conversation on the phone no longer holds the tap while
  a long conversation is drawn: the screen slides in first, then fills.
- A worktree no longer stays marked as waiting after you dismiss an agent's
  question without answering it.
- OpenCode now keeps ket's “Never” rules when you switch agents or search files.
- The phone's worktree list marks each row with its agent's logo, even before
  the agent has reported anything, instead of a branch icon.
- Codex started by ket reports to ket again when Codex's own background server
  is running, as it is while the Codex app is open: its questions show the
  worktree as waiting and reach your phone, and the phone can read its
  conversation.
- On your phone, typing in a chat no longer makes the conversation jump or
  drops the field behind the keyboard, and Send sends with one press after
  you have used the terminal at the Mac, instead of asking you to press it
  again.
- A browser tab given an address that does not exist, typed or opened from a
  terminal link, shows a "This site can’t be reached" page with a Reload
  button, instead of a blank white page.
- macOS menu commands and checkmarks now catch up when you click to another tab, and window controls remain available while a dialog is open.
- Agents opened in ket can use `ket` commands, including `ket backlog add`,
  without installing the command separately.
- Scrolling through a long backlog note description no longer moves the
  backlog list behind it.
- The drawing in the Delete worktree dialog stays still while the delete
  runs, instead of shrinking and flickering.
- A worktree now shows blue “waiting” while Codex asks you a question in its terminal.
- Running OpenCode 2 in a ket pane no longer reports a failed plugin: the
  plugin ket installs now speaks OpenCode 2's plugin language as well as 1's.
- An OpenCode 2 pane now runs its own server, so the plugin reports the pane
  it is in rather than nothing; an OpenCode 1 pane is unchanged, and a
  settings file saved by an earlier ket picks this up without being edited.
