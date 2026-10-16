//! ket's release notes, read from the `CHANGELOG.md` compiled into it.
//!
//! The file at the repository root is the only source of release notes — see
//! its header for the shape and for where else the same text goes. Compiled in rather than fetched, so a build can always say what
//! it contains, offline, and a dev build can show what has landed since the
//! last release.
//!
//! The parse is deliberately small: `## ` starts a release, `### ` a group
//! within it, `- ` an item, and an indented line continues the item above.
//! Anything else is prose for people reading the file, and skipped.

/// The changelog as this build was compiled with it.
const TEXT: &str = include_str!("../../../CHANGELOG.md");

/// The section of changes not yet in a release.
pub const UNRELEASED: &str = "Unreleased";

/// One release's notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its version, or [`UNRELEASED`].
    pub name: String,
    /// The day it was released, as written: `YYYY-MM-DD`.
    pub date: Option<String>,
    /// Its notes, in the order the file has them.
    pub groups: Vec<Group>,
}

impl Release {
    /// Whether it has no notes at all.
    pub fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.items.is_empty())
    }
}

/// A run of notes under one heading: New, Improved or Fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The heading, as written.
    pub kind: String,
    /// One line each, continuation lines joined on.
    pub items: Vec<String>,
}

/// The notes for `name` — a version, or [`UNRELEASED`] — if the changelog
/// has a section for it.
pub fn release(name: &str) -> Option<Release> {
    parse(TEXT).into_iter().find(|release| release.name == name)
}

fn parse(text: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            // `0.4.2 — 2026-10-14`: an em dash, or a plain hyphen typed by
            // someone without one to hand.
            let (name, date) = match heading.split_once(" — ").or(heading.split_once(" - ")) {
                Some((name, date)) => (name.trim(), Some(date.trim().to_owned())),
                None => (heading.trim(), None),
            };
            releases.push(Release {
                name: name.to_owned(),
                date,
                groups: Vec::new(),
            });
            continue;
        }
        let Some(release) = releases.last_mut() else {
            continue;
        };
        if let Some(kind) = line.strip_prefix("### ") {
            release.groups.push(Group {
                kind: kind.trim().to_owned(),
                items: Vec::new(),
            });
        } else if let Some(item) = line.strip_prefix("- ").or(line.strip_prefix("* ")) {
            if release.groups.is_empty() {
                release.groups.push(Group {
                    kind: String::new(),
                    items: Vec::new(),
                });
            }
            if let Some(group) = release.groups.last_mut() {
                group.items.push(item.trim().to_owned());
            }
        } else if line.starts_with("  ")
            && !line.trim().is_empty()
            && let Some(last) = release
                .groups
                .last_mut()
                .and_then(|group| group.items.last_mut())
        {
            last.push(' ');
            last.push_str(line.trim());
        }
    }
    releases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_groups_items_and_continuations_are_read() {
        let text = "# Changelog\n\nProse for people.\n\n## Unreleased\n\n### New\n\n- A thing\n  that wraps.\n- Another\n\n### Fixed\n\n* A star item\n\n## 0.4.2 — 2026-10-14\n\n- Ungrouped item\n\n## 0.4.1 - 2026-10-01\n";
        let releases = parse(text);
        assert_eq!(releases.len(), 3);

        let unreleased = &releases[0];
        assert_eq!(unreleased.name, UNRELEASED);
        assert_eq!(unreleased.date, None);
        assert_eq!(unreleased.groups.len(), 2);
        assert_eq!(unreleased.groups[0].kind, "New");
        assert_eq!(
            unreleased.groups[0].items,
            ["A thing that wraps.", "Another"]
        );
        assert_eq!(unreleased.groups[1].items, ["A star item"]);

        let dated = &releases[1];
        assert_eq!(dated.name, "0.4.2");
        assert_eq!(dated.date.as_deref(), Some("2026-10-14"));
        // An item before any heading gets a group of its own, unnamed.
        assert_eq!(dated.groups[0].kind, "");
        assert_eq!(dated.groups[0].items, ["Ungrouped item"]);

        let hyphen = &releases[2];
        assert_eq!(hyphen.date.as_deref(), Some("2026-10-01"));
        assert!(hyphen.is_empty());
    }

    #[test]
    fn prose_before_the_first_release_and_stray_indented_lines_are_skipped() {
        let releases = parse("- not in a release\n  nor this\n## 1.0\n  orphan continuation\n");
        assert_eq!(releases.len(), 1);
        assert!(releases[0].is_empty());
    }

    #[test]
    fn the_shipped_changelog_reads_cleanly() {
        // What the app's "What's new" and the release notes are made from.
        let releases = parse(TEXT);
        assert_eq!(
            releases.first().map(|r| r.name.as_str()),
            Some(UNRELEASED),
            "Unreleased comes first"
        );
        for release in &releases {
            for group in &release.groups {
                assert!(
                    ["New", "Improved", "Fixed"].contains(&group.kind.as_str()),
                    "{} has a group called {:?}",
                    release.name,
                    group.kind
                );
                for item in &group.items {
                    assert!(!item.is_empty(), "{}: an empty item", release.name);
                }
            }
            if release.name != UNRELEASED {
                let date = release.date.as_deref().unwrap_or_default();
                assert!(
                    date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-',
                    "{} is dated {date:?}",
                    release.name
                );
            }
        }
        assert!(release(UNRELEASED).is_some());
        assert!(release("no-such-version").is_none());
    }
}
