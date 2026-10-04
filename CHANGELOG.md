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
