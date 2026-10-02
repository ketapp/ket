//! What a worktree would bring to its base if it were merged now, for
//! reviewing on a phone: the files, their line counts, and each file's diff.
//!
//! Measured against where the branch last shared a commit with its base —
//! committed work and uncommitted work alike, since a merge commits what is
//! uncommitted first — plus new files git is not yet tracking. The
//! repository's own checkout, which has no base, is measured against `HEAD`.

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use crate::git::Git;
use crate::{KetError, Result};

/// Most bytes of one file's diff sent; the rest is cut.
const DIFF_MAX: usize = 256 * 1024;

/// Reads an untracked regular file without following symlinks or allocating
/// beyond the largest diff the phone can receive.
fn read_untracked(root: &Path, rel: &str) -> Result<(Vec<u8>, bool)> {
    let path = Path::new(rel);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(KetError::Config(format!("refusing unsafe path `{rel}`")));
    }

    let mut joined = root.to_path_buf();
    for part in path.components() {
        let Component::Normal(name) = part else {
            continue;
        };
        joined.push(name);
        let metadata = joined
            .symlink_metadata()
            .map_err(|error| KetError::io(&joined, error))?;
        if metadata.file_type().is_symlink() {
            return Err(KetError::Config(format!(
                "refusing symlinked untracked file `{rel}`"
            )));
        }
    }
    let metadata = joined
        .symlink_metadata()
        .map_err(|error| KetError::io(&joined, error))?;
    if !metadata.is_file() {
        return Err(KetError::Config(format!(
            "refusing non-regular untracked file `{rel}`"
        )));
    }

    let mut bytes = Vec::with_capacity(DIFF_MAX.min(8 * 1024));
    let file = File::open(&joined).map_err(|error| KetError::io(&joined, error))?;
    let opened = file
        .metadata()
        .map_err(|error| KetError::io(&joined, error))?;
    if !opened.is_file() || opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err(KetError::Config(format!(
            "untracked file `{rel}` changed while it was opened"
        )));
    }
    file.take((DIFF_MAX + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| KetError::io(&joined, error))?;
    let truncated = bytes.len() > DIFF_MAX;
    bytes.truncate(DIFF_MAX);
    Ok((bytes, truncated))
}

/// How a file differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Changed.
    Modified,
    /// New.
    Added,
    /// Gone.
    Deleted,
}

/// One file a merge would bring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the worktree.
    pub path: String,
    /// Lines added and removed; `None` for a binary file.
    pub lines: Option<(u32, u32)>,
    /// How it differs.
    pub status: Status,
}

/// The commit a worktree's changes are measured from.
fn since(git: &Git, base: Option<&str>) -> String {
    base.and_then(|base| git.merge_base("HEAD", base).ok())
        .unwrap_or_else(|| "HEAD".to_owned())
}

/// The files a merge of the worktree at `root` into `base` would bring.
pub fn changes(root: &Path, base: Option<&str>) -> Result<Vec<FileChange>> {
    let git = Git::new(root);
    if git.rev_parse("HEAD").is_err() {
        return Ok(Vec::new());
    }
    let since = since(&git, base);
    let status: std::collections::HashMap<String, Status> = git
        .diff_since(&since, &["--name-status"])?
        .lines()
        .filter_map(|line| {
            let (kind, path) = line.split_once('\t')?;
            let status = match kind.chars().next()? {
                'A' => Status::Added,
                'D' => Status::Deleted,
                _ => Status::Modified,
            };
            Some((path.to_owned(), status))
        })
        .collect();
    let mut files: Vec<FileChange> = git
        .diff_since(&since, &["--numstat"])?
        .lines()
        .filter_map(|line| {
            let mut columns = line.splitn(3, '\t');
            let added = columns.next()?;
            let removed = columns.next()?;
            let path = columns.next()?.to_owned();
            let lines = added.parse().ok().zip(removed.parse().ok());
            let status = status.get(&path).copied().unwrap_or(Status::Modified);
            Some(FileChange {
                path,
                lines,
                status,
            })
        })
        .collect();
    for path in git.untracked()? {
        let lines = read_untracked(root, &path)
            .ok()
            .filter(|(bytes, truncated)| !truncated && !bytes.contains(&0))
            .map(|(bytes, _)| {
                let count = bytes.iter().filter(|&&b| b == b'\n').count();
                (u32::try_from(count).unwrap_or(u32::MAX), 0)
            });
        files.push(FileChange {
            path,
            lines,
            status: Status::Added,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// One file's diff, as `changes` counted it — cut at [`DIFF_MAX`], and
/// whether it was. Only a path `changes` lists is shown: anything else is
/// refused, so this is no way to read an arbitrary file.
pub fn diff(root: &Path, base: Option<&str>, path: &str) -> Result<(String, bool)> {
    let file = changes(root, base)?
        .into_iter()
        .find(|file| file.path == path)
        .ok_or_else(|| KetError::Config(format!("`{path}` has no changes to show")))?;
    let git = Git::new(root);
    let untracked = file.status == Status::Added && git.untracked()?.iter().any(|p| p == path);
    let (text, already_truncated) = if untracked {
        let (bytes, truncated) = read_untracked(root, path)?;
        let body = match String::from_utf8(bytes) {
            Ok(body) => body,
            Err(error) if truncated && error.utf8_error().error_len().is_none() => {
                let valid = error.utf8_error().valid_up_to();
                String::from_utf8(error.into_bytes()[..valid].to_vec())
                    .expect("prefix reported as valid UTF-8")
            }
            Err(_) => return Err(KetError::Config(format!("`{path}` is a binary file"))),
        };
        let lines = body.lines().count();
        let mut text = format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{lines} @@\n");
        for line in body.lines() {
            text.push('+');
            text.push_str(line);
            text.push('\n');
        }
        (text, truncated)
    } else {
        (git.diff_since(&since(&git, base), &["--", path])?, false)
    };
    Ok(match text.char_indices().nth(DIFF_MAX) {
        Some((at, _)) => (text[..at].to_owned(), true),
        None => (text, already_truncated),
    })
}
