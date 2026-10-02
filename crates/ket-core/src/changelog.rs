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
