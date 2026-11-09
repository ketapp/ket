//! How a session writes, when an Economy level asks for less prose.
//!
//! A level's `instruction` is one sentence handed over at session start, and
//! a sentence early in a long conversation is one the model drifts away from.
//! A style is the same request made where it cannot drift: appended to Claude
//! Code's system prompt with `--append-system-prompt-file`, so it is part of
//! every request, survives compaction, and sits in the cached prefix rather
//! than costing anything per turn.
//!
//! Prose only. A style changes how replies read, never what the agent does:
//! code, commands, commit messages, pull request text and files keep their own
//! conventions, and anything where order or risk matters is still said in
//! full sentences. Habits that change behaviour — editing rather than
//! rewriting a file, reading line ranges — are deliberately not here.
//!
//! The text is ket's, compiled in, and written under `~/.ket/styles` rather
//! than into any Claude config directory: a launch names the file by path, so
//! there is nothing to install into the agent and nothing to take out of it.

use std::path::{Path, PathBuf};

use crate::error::{KetError, Result};

/// Replies as terse engineering notes.
pub const TERSE: &str = "terse";

/// Every style ket ships, by the name a pack uses for it.
const STYLES: &[(&str, &str)] = &[(TERSE, TERSE_TEXT)];

const TERSE_TEXT: &str = "\
# Reply style: terse

Write every reply as terse engineering notes. All technical substance stays; \
only the padding goes.

- No filler, pleasantries or hedging, and no restating the question.
- Fragments are fine. Prefer the short word to the long one.
- Lead with the result or the decision. No narration of what you are about \
to do or have just done, no recaps, no closing summaries.
- Explain only when asked, or when a step would be unsafe to follow without \
the reason.
- Several findings go in a list, not a paragraph.

Keep these exact and written normally, whatever the above says:

- Code, commands, paths, identifiers, error messages and quoted output.
- Commit messages, pull request and issue text, documentation, and any file \
you write: follow the repository's own conventions.
- Security warnings, destructive or irreversible actions, and steps whose \
order matters: say them in full sentences.
";

/// Whether ket has a style called `name`.
pub fn exists(name: &str) -> bool {
    text(name).is_some()
}

/// The names of every style ket ships, for an error that lists them.
pub fn names() -> impl Iterator<Item = &'static str> {
    STYLES.iter().map(|(name, _)| *name)
}

fn text(name: &str) -> Option<&'static str> {
    STYLES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, text)| *text)
}

/// Where the style called `name` is kept for a launch to name.
///
/// A path and nothing else: computing it touches no disk, so a launch line
/// can be built — and tested — without one. [`ensure`] is what writes it.
pub fn path(name: &str) -> PathBuf {
    dir().join(format!("{name}.md"))
}

fn dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ket")
        .join("styles")
}

/// Writes the style called `name` where [`path`] says, unless it is already
/// there word for word.
///
/// Rewritten when it differs, like the hook script: an upgraded ket should
/// not leave last version's wording behind. Called just before a launch that
/// names the file, because a session told to read a file that is not there
/// does not start.
pub fn ensure(name: &str) -> Result<PathBuf> {
    let text =
        text(name).ok_or_else(|| KetError::Config(format!("ket has no style called `{name}`")))?;
    let path = path(name);
    write_if_changed(&path, text)?;
    Ok(path)
}

fn write_if_changed(path: &Path, text: &str) -> Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == text) {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("style file has no directory".to_owned()))?;
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    std::fs::write(path, text).map_err(|e| KetError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse_is_a_style_and_nothing_else_is() {
        assert!(exists(TERSE));
        assert!(!exists("caveman"));
        assert_eq!(names().collect::<Vec<_>>(), [TERSE]);
    }

    #[test]
    fn a_styles_path_is_a_markdown_file_under_ket_s_own_directory() {
        let path = path(TERSE);
        assert!(path.ends_with(".ket/styles/terse.md"), "{}", path.display());
    }

    #[test]
    fn the_terse_style_keeps_what_must_stay_exact() {
        // The exemptions are the part a reader is trusting; losing one in an
        // edit would put commit messages and warnings in fragments.
        for kept in ["Code, commands", "Commit messages", "Security warnings"] {
            assert!(TERSE_TEXT.contains(kept), "{kept}");
        }
    }

    #[test]
    fn writing_is_skipped_when_the_file_already_says_it() {
        let dir = std::env::temp_dir().join(format!("ket-style-{}", std::process::id()));
        let path = dir.join("nested").join("terse.md");
        write_if_changed(&path, TERSE_TEXT).expect("first write");
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();
        write_if_changed(&path, TERSE_TEXT).expect("same text");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), first);
        write_if_changed(&path, "older wording").expect("changed text");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "older wording");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
