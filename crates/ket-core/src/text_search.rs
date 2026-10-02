//! Workspace-wide plain-text search and VS Code Search-view file patterns.
//!
//! The UI deliberately has no filesystem dependency, so walking, filtering
//! and reading live here. File discovery reuses [`crate::files::index`], which
//! means global search follows the same repository ignore rules as the file
//! finder instead of inventing a second idea of which files belong to a
//! worktree.
//!
//! Include and exclude fields follow VS Code's Search view rather than its
//! settings-file glob rules: comma separates patterns outside braces and
//! character classes, `./` anchors a pattern at the worktree root, and every
//! other pattern gets an implicit `**/` prefix. The syntax itself is `*`, `?`,
//! `**`, `{a,b}`, `[a-z]`, and `[!a-z]`, with `/` as the separator.

use std::ops::Range;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::{KetError, Result};

/// Most matching occurrences returned from one search.
pub const MAX_MATCHES: usize = 500;

/// Files larger than this are treated as generated/binary material.
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// A workspace text-search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Literal text to find, case-insensitively.
    pub text: String,
    /// VS Code Search-view include patterns, comma separated.
    pub include: String,
    /// VS Code Search-view exclude patterns, comma separated.
    pub exclude: String,
}

/// One line containing one or more occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// One-based line number.
    pub line: u32,
    /// One-based character column of the first occurrence.
    pub column: u32,
    /// The line as stored in the file, without its line ending.
    pub preview: String,
    /// Byte ranges of every occurrence in [`Self::preview`].
    pub ranges: Vec<Range<usize>>,
}

/// Matches grouped under one file, as in VS Code's Search view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    /// Absolute path, for opening the result.
    pub path: PathBuf,
    /// Slash-separated path relative to the worktree.
    pub relative_path: String,
    /// Matching lines in source order.
    pub lines: Vec<LineMatch>,
}

/// The bounded result of a workspace search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matches {
    /// Matching files in path order.
    pub files: Vec<FileMatches>,
    /// Total occurrences represented by [`Self::files`].
    pub total: usize,
    /// Whether [`MAX_MATCHES`] stopped the search early.
    pub truncated: bool,
    /// Whether file discovery itself hit its safety limit.
    pub index_truncated: bool,
}

/// Searches every indexed text file below `root`.
pub fn search(root: &Path, query: &Query) -> Result<Matches> {
    let include = PatternSet::include(&query.include)?;
    let exclude = PatternSet::exclude(&query.exclude)?;
    let index = crate::files::search_index(root)?;
    let mut answer = Matches {
        index_truncated: index.truncated(),
        ..Matches::default()
    };
    if query.text.is_empty() {
        return Ok(answer);
    }

    for relative_path in index.paths() {
        if !include.matches(relative_path) || exclude.matches(relative_path) {
            continue;
        }
        let path = root.join(relative_path);
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // NUL is the same cheap binary signal ripgrep uses before its richer
        // binary handling. Searching lossy decoded media produces nonsense.
        if bytes.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            let mut ranges = insensitive_ranges(line, &query.text);
            if ranges.is_empty() {
                continue;
            }
            let remaining = MAX_MATCHES.saturating_sub(answer.total);
            if ranges.len() > remaining {
                ranges.truncate(remaining);
                answer.truncated = true;
            }
            let first = ranges[0].start;
            let column = line[..first].chars().count().saturating_add(1);
            answer.total = answer.total.saturating_add(ranges.len());
            lines.push(LineMatch {
                line: u32::try_from(line_index.saturating_add(1)).unwrap_or(u32::MAX),
                column: u32::try_from(column).unwrap_or(u32::MAX),
                preview: line.to_owned(),
                ranges,
            });
            if answer.total >= MAX_MATCHES {
                answer.truncated = true;
                break;
            }
        }
        if !lines.is_empty() {
            answer.files.push(FileMatches {
                path,
                relative_path: relative_path.clone(),
                lines,
            });
        }
        if answer.truncated {
            break;
        }
    }
    Ok(answer)
}

/// Finds ASCII-case-insensitively while leaving byte offsets valid for the
/// original UTF-8 line. Non-ASCII text remains exactly matchable; changing
/// only ASCII bytes cannot change the string's byte layout.
fn insensitive_ranges(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    let folded_haystack = haystack.to_ascii_lowercase();
    let folded_needle = needle.to_ascii_lowercase();
    folded_haystack
        .match_indices(&folded_needle)
        .map(|(start, found)| start..start + found.len())
        .collect()
}

/// A compiled comma-separated Search-view field.
struct PatternSet {
    glob: GlobSet,
    empty_matches: bool,
}

impl PatternSet {
    fn include(field: &str) -> Result<Self> {
        Self::compile(field, true)
    }

    fn exclude(field: &str) -> Result<Self> {
        Self::compile(field, false)
    }

    fn compile(field: &str, empty_matches: bool) -> Result<Self> {
        let patterns = split_patterns(field);
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            for expanded in search_view_patterns(&pattern) {
                let glob = GlobBuilder::new(&expanded)
                    .literal_separator(true)
                    // VS Code requires `/` in Search-view patterns; a
                    // backslash is not its escape syntax.
                    .backslash_escape(false)
                    .case_insensitive(cfg!(any(target_os = "macos", target_os = "windows")))
                    .build()
                    .map_err(|error| {
                        KetError::Config(format!("invalid file pattern {pattern:?}: {error}"))
                    })?;
                builder.add(glob);
            }
        }
        let glob = builder
            .build()
            .map_err(|error| KetError::Config(format!("invalid file patterns: {error}")))?;
        Ok(Self {
            glob,
            empty_matches,
        })
    }

    fn matches(&self, path: &str) -> bool {
        if self.glob.is_empty() {
            self.empty_matches
        } else {
            self.glob.is_match(path)
        }
    }
}

/// Splits commas only where VS Code does: not inside braces or brackets.
fn split_patterns(field: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut start = 0;
    let mut braces = 0usize;
    let mut brackets = 0usize;
    for (index, character) in field.char_indices() {
        match character {
            '{' => braces = braces.saturating_add(1),
            '}' => braces = braces.saturating_sub(1),
            '[' => brackets = brackets.saturating_add(1),
            ']' => brackets = brackets.saturating_sub(1),
            ',' if braces == 0 && brackets == 0 => {
                let pattern = field[start..index].trim();
                if !pattern.is_empty() {
                    patterns.push(pattern.to_owned());
                }
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    let tail = field[start..].trim();
    if !tail.is_empty() {
        patterns.push(tail.to_owned());
    }
    patterns
}

/// Applies Search-view anchoring and lets a directory-like match include its
/// descendants. `./src` is root-relative; `src` can name any `src` directory.
fn search_view_patterns(pattern: &str) -> Vec<String> {
    let (pattern, anchored) = match pattern.strip_prefix("./") {
        Some(pattern) => (pattern, true),
        None => (pattern, false),
    };
    let base = if anchored || pattern.starts_with("**/") {
        pattern.to_owned()
    } else {
        format!("**/{pattern}")
    };
    if pattern.ends_with("/**") {
        vec![base]
    } else {
        vec![base.clone(), format!("{base}/**")]
    }
}
