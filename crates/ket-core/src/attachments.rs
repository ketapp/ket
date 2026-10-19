//! Files handed to an agent with a prompt: an image goes into the agent's
//! input as an attachment of its own, anything else is named by its path
//! under the words.
//!
//! A window does this for a drop, the quick prompt and new work; the host
//! does it for a message a phone sends with photos — see `host::phones`.
//! Both go by what is here, so a photo from a phone reaches an agent the
//! way a screenshot dropped on the desktop's pane does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;

use crate::error::{KetError, Result};

/// What the terminal agents read as an image, which is the same list their
/// own paste handling uses.
pub const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "ico"];

/// The longest attached images are waited for before the prompt follows
/// them anyway — see [`images_shown`].
pub const ATTACH_WAIT: Duration = Duration::from_secs(4);

/// A beat after the last image shows, before the prompt is typed.
pub const ATTACH_SETTLE: Duration = Duration::from_millis(150);

/// How often the screen is looked at while images are read.
pub const ATTACH_POLL: Duration = Duration::from_millis(50);

/// The pause between a prompt's paste and its Enter. Sent in the same write,
/// a TUI still taking the paste in can read the carriage return as one more
/// line of it rather than as the submit.
pub const SUBMIT_DELAY: Duration = Duration::from_millis(80);

/// How long the files a phone sent are kept: long enough for the agent to
/// read them when it gets to them, and cleared by a later message after.
const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The longest name a kept file is given, extension and all.
const NAME_MAX: usize = 80;

/// Whether `path` reaches an agent as an image of its own when it is handed
/// one, rather than as a path in the prompt.
pub fn pastes_as_image(path: &Path) -> bool {
    image_paste(path).is_some()
}

/// The path as it should be pasted to attach it, or `None` if it is not an
/// image or cannot go in unquoted.
///
/// The characters refused are the ones a shell would act on if the paste
/// landed at a prompt rather than in an agent, plus the control bytes, which
/// a pty would read as keys rather than as text. A space is not among them:
/// it is only a separator once something splits the line, and the agents do
/// not split a pasted path.
pub fn image_paste(path: &Path) -> Option<&str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    let raw = path.to_str()?;
    let unsafe_here = |c: char| c.is_control() || "\"'`$;&|<>(){}[]*?!#\\".contains(c);
    (!raw.contains(unsafe_here)).then_some(raw)
}

/// `prompt`, with the files the agent could not be handed as attachments
/// listed after it by path.
pub fn with_files(prompt: String, files: &[PathBuf]) -> String {
    if files.is_empty() {
        return prompt;
    }
    let list: Vec<String> = files
        .iter()
        .map(|path| format!("- {}", path.display()))
        .collect();
    if prompt.is_empty() {
        return format!("Attached files:\n{}", list.join("\n"));
    }
    format!("{prompt}\n\nAttached files:\n{}", list.join("\n"))
}

/// `text` as the bytes a paste of it is: its new lines as the carriage
/// returns a terminal reads, inside bracketed-paste marks when the program
/// asked for them, so that it takes the whole as one paste rather than as
/// keys.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut bytes = Vec::with_capacity(text.len() + 12);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(text.as_bytes());
    }
    bytes
}

/// The images among `paths`, each as a bracketed paste of its path — what a
/// drop on the pane does — with how many went that way and the paths that
/// could not, for the prompt to name instead. A paste is self-delimiting, so
/// two images need nothing between them.
pub fn image_pastes(paths: &[PathBuf], bracketed: bool) -> (Vec<u8>, usize, Vec<PathBuf>) {
    let mut bytes = Vec::new();
    let mut pasted = 0;
    let mut left = Vec::new();
    for path in paths {
        match bracketed.then(|| image_paste(path)).flatten() {
            Some(raw) => {
                bytes.extend_from_slice(b"\x1b[200~");
                bytes.extend_from_slice(raw.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                pasted += 1;
            }
            None => left.push(path.clone()),
        }
    }
    (bytes, pasted, left)
}

/// How many image chips — Claude Code's and Codex's "[Image #1]",
/// OpenCode's "[Image 1]" — the bottom of `term`'s live screen shows: where
/// an agent's input box is, and where an image pasted into it appears once
/// the agent has read it. Pasting the prompt before then can lose its
/// Enter, so a sender waits for the count to rise.
pub fn images_shown<T>(term: &Term<T>) -> usize {
    /// The rows at the bottom an input box fits in.
    const INPUT_ROWS: usize = 12;
    let grid = term.grid();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    (rows.saturating_sub(INPUT_ROWS)..rows)
        .map(|row| {
            let line = Line(i32::try_from(row).unwrap_or(i32::MAX));
            let text: String = (0..cols).map(|col| grid[line][Column(col)].c).collect();
            text.matches("[Image").count()
        })
        .sum()
}

/// Keeps `files` — each a name and its bytes, from a phone — in a new
/// directory of their own under `root`, and returns where each one is, in
/// the same order.
///
/// A name is made safe for a path first, and for [`image_paste`]: letters,
/// digits, `.`, `_` and `-`, anything else a `-`. Directories there older
/// than a week are cleared on the way, so what phones sent does not pile up.
pub fn keep(root: &Path, files: &[(&str, &[u8])]) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(root).map_err(|e| KetError::io(root, e))?;
    clear_old(root);
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = root.join(format!(
        "{}-{}-{}",
        crate::now_ms(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).map_err(|e| KetError::io(&dir, e))?;
    let mut kept: Vec<PathBuf> = Vec::with_capacity(files.len());
    for (name, bytes) in files {
        let name = safe_name(name);
        let mut path = dir.join(&name);
        let mut copy = 2;
        while kept.contains(&path) {
            path = dir.join(numbered(&name, copy));
            copy += 1;
        }
        std::fs::write(&path, bytes).map_err(|e| KetError::io(&path, e))?;
        kept.push(path);
    }
    Ok(kept)
}

/// Removes the directories under `root` older than [`KEPT_FOR`]. Best
/// effort: one that cannot be read or removed is left for the next time.
fn clear_old(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > KEPT_FOR);
        if old && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// `name` — from another device, so anything at all — as a file name: its
/// last component, with only letters, digits, `.`, `_` and `-` left in it, no
/// leading dot, and no longer than [`NAME_MAX`], keeping its extension.
fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut safe = String::with_capacity(last.len());
    for c in last.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '-'
        };
        if !(c == '-' && safe.ends_with('-')) {
            safe.push(c);
        }
    }
    let safe = safe.trim_start_matches(['.', '-']).to_owned();
    let safe = if safe.trim_matches(['.', '-', '_']).is_empty() {
        "file".to_owned()
    } else {
        safe
    };
    if safe.len() <= NAME_MAX {
        return safe;
    }
    match safe.rsplit_once('.') {
        Some((stem, extension)) if extension.len() < 12 => {
            format!("{}.{extension}", &stem[..NAME_MAX - extension.len() - 1])
        }
        _ => safe[..NAME_MAX].to_owned(),
    }
}

/// `name` with `-2`, `-3` before its extension, for a second file of the
/// same name in one message.
fn numbered(name: &str, copy: usize) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem}-{copy}.{extension}"),
        _ => format!("{name}-{copy}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_pastes_by_its_path_and_anything_else_is_listed() {
        let paths = [
            PathBuf::from("/tmp/up/IMG_1.jpg"),
            PathBuf::from("/tmp/up/console.txt"),
            PathBuf::from("/tmp/up/shot.PNG"),
        ];
        let (bytes, pasted, left) = image_pastes(&paths, true);
        assert_eq!(
            bytes,
            b"\x1b[200~/tmp/up/IMG_1.jpg\x1b[201~\x1b[200~/tmp/up/shot.PNG\x1b[201~".to_vec()
        );
        assert_eq!(pasted, 2);
        assert_eq!(left, [PathBuf::from("/tmp/up/console.txt")]);
    }

    #[test]
    fn without_bracketed_paste_every_file_is_listed() {
        let paths = [PathBuf::from("/tmp/up/IMG_1.jpg")];
        let (bytes, pasted, left) = image_pastes(&paths, false);
        assert!(bytes.is_empty());
        assert_eq!(pasted, 0);
        assert_eq!(left, paths);
    }

    #[test]
    fn a_path_a_shell_would_act_on_is_not_pasted_as_an_image() {
        assert!(pastes_as_image(Path::new("/tmp/a b.png")));
        assert!(!pastes_as_image(Path::new("/tmp/$(x).png")));
        assert!(!pastes_as_image(Path::new("/tmp/a.pdf")));
    }

    #[test]
    fn files_the_agent_could_not_be_handed_are_listed_after_the_prompt() {
        assert_eq!(with_files("Look".to_owned(), &[]), "Look");
        assert_eq!(
            with_files(
                "Look".to_owned(),
                &[PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.pdf")]
            ),
            "Look\n\nAttached files:\n- /tmp/a.png\n- /tmp/b.pdf"
        );
        assert_eq!(
            with_files(String::new(), &[PathBuf::from("/tmp/b.pdf")]),
            "Attached files:\n- /tmp/b.pdf"
        );
    }

    #[test]
    fn a_paste_is_bracketed_only_when_asked_for_with_new_lines_as_returns() {
        assert_eq!(paste("a\nb", true), b"\x1b[200~a\rb\x1b[201~".to_vec());
        assert_eq!(paste("a\r\nb", false), b"a\rb".to_vec());
    }

    #[test]
    fn a_name_from_a_phone_is_made_safe_for_a_path() {
        assert_eq!(safe_name("IMG_2041.jpg"), "IMG_2041.jpg");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("my screen (1).png"), "my-screen-1-.png");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("$();"), "file");
        assert_eq!(safe_name(""), "file");
        let long = format!("{}.png", "a".repeat(200));
        let made = safe_name(&long);
        assert_eq!(made.len(), NAME_MAX);
        assert!(made.ends_with(".png"));
        assert!(pastes_as_image(
            &Path::new("/tmp").join(safe_name("a;b`c$.jpg"))
        ));
    }

    #[test]
    fn kept_files_get_a_directory_of_their_own_and_a_repeat_name_is_numbered() {
        let root = std::env::temp_dir().join(format!("ket-attachments-{}", crate::now_ms()));
        let kept = keep(
            &root,
            &[
                ("shot.png", b"one".as_slice()),
                ("shot.png", b"two".as_slice()),
            ],
        )
        .unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].parent(), kept[1].parent());
        assert!(kept[0].starts_with(&root));
        assert_eq!(kept[0].file_name().unwrap(), "shot.png");
        assert_eq!(kept[1].file_name().unwrap(), "shot-2.png");
        assert_eq!(std::fs::read(&kept[1]).unwrap(), b"two");
        let again = keep(&root, &[("shot.png", b"three".as_slice())]).unwrap();
        assert_ne!(again[0].parent(), kept[0].parent());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
