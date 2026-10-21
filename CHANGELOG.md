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

- The phone's Worktrees screen looks like your Mac's sidebar: a compact
  filter with New beside it, and the list running the full height under a
  floating tab bar. Worktrees at work open up to show what their agent is
  doing. A quick prompt at the foot starts an agent in the project and with
  the agent you last used.
- Hold a worktree on your phone for its menu: pin it to the top of its
  project, act on its pull request, or delete it. Delete asks first, and
  offers Delete anyway when it would lose uncommitted work. Your
  repository's own checkout can't be pinned or deleted.
- Put idle agents to sleep from a worktree's menu while their tabs stay open, then wake into the same conversation. You can turn on automatic sleep and choose its delay in Settings → General.
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
