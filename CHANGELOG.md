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

### Improved

- A new terminal opened beside one already in the pane starts at the pane's
  size, so its prompt appears sooner and does not redraw once the shell is
  up.
- The note composer now offers a description as you type, and open notes show
  their full title beside a separate panel for tags and worktree options.

### Fixed

- The drawing in the Delete worktree dialog stays still while the delete
  runs, instead of shrinking and flickering.
- A worktree now shows blue “waiting” while Codex asks you a question in its terminal.
- Running OpenCode 2 in a ket pane no longer reports a failed plugin: the
  plugin ket installs now speaks OpenCode 2's plugin language as well as 1's.
- An OpenCode 2 pane now runs its own server, so the plugin reports the pane
  it is in rather than nothing; an OpenCode 1 pane is unchanged, and a
  settings file saved by an earlier ket picks this up without being edited.
