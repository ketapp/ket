//! The colour a Claude session was given with `/color`.
//!
//! Claude records it in the session's transcript, as
//! `{"type":"agent-color","agentColor":"red",…}`, when it is set and again
//! from time to time after; `/color default` records `"default"`, which takes
//! it off. The copies can be most of a megabyte apart in a long session, so
//! the tail that a session's title is read from would miss it: a transcript is
//! searched once, from its end back, and after that only what has been added
//! since.
//!
//! Asked every snapshot and every tick, so an answer stands for [`RECHECK`]
//! before the file is looked at again, and a transcript's path is looked for
//! once.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// A colour Claude's `/color` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionColor {
    /// `/color red`.
    Red,
    /// `/color blue`.
    Blue,
    /// `/color green`.
    Green,
    /// `/color yellow`.
    Yellow,
    /// `/color purple`.
    Purple,
    /// `/color orange`.
    Orange,
    /// `/color pink`.
    Pink,
    /// `/color cyan`.
    Cyan,
}

impl SessionColor {
    /// The colour Claude calls `name`; `None` for `default`, or one ket
    /// doesn't know.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "red" => Self::Red,
            "blue" => Self::Blue,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "purple" => Self::Purple,
            "orange" => Self::Orange,
            "pink" => Self::Pink,
            "cyan" => Self::Cyan,
            _ => return None,
        })
    }

    /// Claude's name for it, which is how phones are told it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Purple => "purple",
            Self::Orange => "orange",
            Self::Pink => "pink",
            Self::Cyan => "cyan",
        }
    }

    /// How ket paints it: Tailwind's 500, the shades ket's project colours
    /// come from. The phone keeps the same table.
    pub fn hex(self) -> &'static str {
        match self {
            Self::Red => "#ef4444",
            Self::Blue => "#3b82f6",
            Self::Green => "#22c55e",
            Self::Yellow => "#eab308",
            Self::Purple => "#a855f7",
            Self::Orange => "#f97316",
            Self::Pink => "#ec4899",
            Self::Cyan => "#06b6d4",
        }
    }
}

/// How long an answer stands before its transcript is looked at again.
const RECHECK: Duration = Duration::from_secs(1);

/// How much of a transcript is read at a time, searching back from its end.
const CHUNK: u64 = 256 * 1024;

/// How far before the last read's end the next one starts, for a record
/// that was still being written then. A record is one short line.
const OVERLAP: u64 = 4 * 1024;

const RECORD: &[u8] = b"\"type\":\"agent-color\"";

#[derive(Clone)]
struct Seen {
    /// The transcript, once there is one: a session's first moments have
    /// none yet.
    path: Option<PathBuf>,
    /// How much of it has been searched.
    len: u64,
    color: Option<SessionColor>,
    at: Instant,
}

/// The colour `session` of `agent` was given, or `None` for none, or an
/// agent with no `/color`. Reads the transcript, so it belongs off a
/// window's thread.
pub fn of(agent: &str, session: &str) -> Option<SessionColor> {
    static SEEN: OnceLock<Mutex<HashMap<String, Seen>>> = OnceLock::new();
    if crate::agents::harness(agent) != "claude" || session.is_empty() {
        return None;
    }
    let seen = SEEN.get_or_init(Mutex::default);
    let known = seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(session)
        .cloned();
    if let Some(known) = &known
        && known.at.elapsed() < RECHECK
    {
        return known.color;
    }
    let (path, from, before) = match known {
        Some(Seen {
            path: Some(path),
            len,
            color,
            ..
        }) => (Some(path), len, color),
        _ => (crate::conversation::claude_transcript(session), 0, None),
    };
    let (len, color) = match &path {
        Some(path) => advance(path, from, before),
        None => (0, None),
    };
    seen.lock().unwrap_or_else(PoisonError::into_inner).insert(
        session.to_owned(),
        Seen {
            path,
            len,
            color,
            at: Instant::now(),
        },
    );
    color
}

/// The colour in `path` once what was added after `from` has been searched,
/// `before` being what the first `from` bytes said, and how long the file is
/// now. A file grown shorter has been rewritten, and is searched afresh.
fn advance(path: &Path, from: u64, before: Option<SessionColor>) -> (u64, Option<SessionColor>) {
    let len = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let (from, before) = if len < from {
        (0, None)
    } else {
        (from, before)
    };
    if len == from {
        return (len, before);
    }
    let color = newest(path, from.saturating_sub(OVERLAP), len).unwrap_or(before);
    (len, color)
}

/// The newest colour record in `path` between `from` and `to`, searched
/// from `to` back: `Some(None)` for one that took the colour off, `None` for
/// no record there.
fn newest(path: &Path, from: u64, to: u64) -> Option<Option<SessionColor>> {
    let mut file = File::open(path).ok()?;
    let mut end = to;
    let mut buffer = Vec::new();
    while end > from {
        // Each chunk reaches a line past the next one's start, so a record
        // cut by the boundary is whole in one of them.
        let start = end.saturating_sub(CHUNK).max(from);
        let read_to = (end + OVERLAP).min(to);
        buffer.resize(usize::try_from(read_to - start).ok()?, 0);
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut buffer).ok()?;
        let mut search = buffer.as_slice();
        while let Some(at) = rfind(search, RECORD) {
            let line_start = search[..at]
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(0, |newline| newline + 1);
            let line_end = buffer[at..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(buffer.len(), |newline| at + newline);
            if let Some(found) = parse(&buffer[line_start..line_end]) {
                return Some(found);
            }
            // A record still being written, or a mention of one in a
            // message: keep looking before it.
            search = &search[..at];
        }
        end = start;
    }
    None
}

/// A colour record's colour: `Some(None)` for `default`; `None` for a line
/// that isn't one.
fn parse(line: &[u8]) -> Option<Option<SessionColor>> {
    let record: serde_json::Value = serde_json::from_slice(line).ok()?;
    if record.get("type")?.as_str()? != "agent-color" {
        return None;
    }
    Some(SessionColor::from_name(record.get("agentColor")?.as_str()?))
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(color: &str) -> String {
        format!(r#"{{"type":"agent-color","agentColor":"{color}","sessionId":"s1"}}"#)
    }

    fn prompt(text: &str) -> String {
        format!(r#"{{"type":"user","message":{{"role":"user","content":"{text}"}}}}"#)
    }

    /// Lines of other records, `bytes` long in all, none of them a colour.
    fn filler(bytes: usize) -> String {
        let line = prompt(&"x".repeat(90)) + "\n";
        let mut out = line.repeat(bytes / line.len());
        let rest = bytes - out.len();
        if rest > 0 {
            out.push_str(&" ".repeat(rest - 1));
            out.push('\n');
        }
        out
    }

    fn transcript(name: &str, text: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ket-agent-color-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn whole(path: &Path) -> Option<Option<SessionColor>> {
        newest(path, 0, std::fs::metadata(path).unwrap().len())
    }

    #[test]
    fn every_colour_is_read_by_its_name_and_painted_from_a_hex_that_parses() {
        for color in [
            SessionColor::Red,
            SessionColor::Blue,
            SessionColor::Green,
            SessionColor::Yellow,
            SessionColor::Purple,
            SessionColor::Orange,
            SessionColor::Pink,
            SessionColor::Cyan,
        ] {
            assert_eq!(SessionColor::from_name(color.name()), Some(color));
            assert!(
                crate::theme::Color::from_hex(color.hex()).is_ok(),
                "{color:?}"
            );
        }
        assert_eq!(SessionColor::from_name("default"), None);
        assert_eq!(SessionColor::from_name("magenta"), None);
    }

    #[test]
    fn the_newest_record_is_the_colour_and_default_takes_it_off() {
        let red = transcript("red", &(prompt("hi") + "\n" + &record("red") + "\n"));
        assert_eq!(whole(&red), Some(Some(SessionColor::Red)));

        let changed = transcript(
            "changed",
            &[record("red"), prompt("hi"), record("blue")].join("\n"),
        );
        assert_eq!(whole(&changed), Some(Some(SessionColor::Blue)));

        let reset = transcript("reset", &[record("red"), record("default")].join("\n"));
        assert_eq!(whole(&reset), Some(None));

        let none = transcript("none", &(prompt("hi") + "\n"));
        assert_eq!(whole(&none), None);
    }

    #[test]
    fn a_colour_named_in_a_message_is_not_a_colour_record() {
        // How a message mentioning the record is written: escaped, inside a
        // string.
        let text = prompt(r#"set it with \"type\":\"agent-color\",\"agentColor\":\"red\""#);
        let path = transcript("mention", &(text + "\n"));
        assert_eq!(whole(&path), None);
    }

    #[test]
    fn a_record_far_from_the_end_is_found() {
        let text = record("pink") + "\n" + &filler(3 * CHUNK as usize);
        let path = transcript("far", &text);
        assert_eq!(whole(&path), Some(Some(SessionColor::Pink)));
    }

    #[test]
    fn a_record_cut_by_where_one_read_stops_is_read_whole() {
        // The record straddles the start of the last chunk read from the end.
        let line = record("cyan") + "\n";
        let after = CHUNK as usize - line.len() / 2;
        let text = filler(4096) + &line + &filler(after);
        let path = transcript("straddle", &text);
        let len = text.len() as u64;
        let start = len - CHUNK;
        let at = 4096_u64;
        assert!(
            at < start && start < at + line.len() as u64,
            "the test cuts the record"
        );
        assert_eq!(whole(&path), Some(Some(SessionColor::Cyan)));
    }

    #[test]
    fn what_is_added_is_read_on_top_of_what_was_read_before() {
        let path = transcript("grows", &(record("red") + "\n" + &filler(2000)));
        let (len, color) = advance(&path, 0, None);
        assert_eq!(color, Some(SessionColor::Red));

        // Nothing new: the answer stands.
        assert_eq!(advance(&path, len, color), (len, color));

        // Lines without a colour keep it.
        let mut text = std::fs::read_to_string(&path).unwrap() + &filler(500);
        std::fs::write(&path, &text).unwrap();
        let (len, color) = advance(&path, len, color);
        assert_eq!(color, Some(SessionColor::Red));

        // A new colour, then a reset.
        text.push_str(&(record("green") + "\n"));
        std::fs::write(&path, &text).unwrap();
        let (len, color) = advance(&path, len, color);
        assert_eq!(color, Some(SessionColor::Green));
        text.push_str(&(record("default") + "\n"));
        std::fs::write(&path, &text).unwrap();
        let (_, color) = advance(&path, len, color);
        assert_eq!(color, None);
    }

    #[test]
    fn a_record_still_being_written_is_read_once_it_is_whole() {
        let line = record("orange");
        let path = transcript("partial", &(filler(1000) + &line[..line.len() - 10]));
        let (len, color) = advance(&path, 0, None);
        assert_eq!(color, None, "half a record is no record");

        std::fs::write(&path, filler(1000) + &line + "\n").unwrap();
        let (_, color) = advance(&path, len, color);
        assert_eq!(color, Some(SessionColor::Orange));
    }

    #[test]
    fn a_transcript_rewritten_shorter_is_read_afresh() {
        let path = transcript("rewritten", &(record("red") + "\n" + &filler(5000)));
        let (len, color) = advance(&path, 0, None);
        assert_eq!(color, Some(SessionColor::Red));

        std::fs::write(&path, record("yellow") + "\n").unwrap();
        let (_, color) = advance(&path, len, color);
        assert_eq!(color, Some(SessionColor::Yellow));
    }
}
