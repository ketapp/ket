//! Where the network inspector keeps what it saw.
//!
//! `<data_dir>/network/<worktree>/`, readable by this user alone:
//!
//! - `index.jsonl`, one [`RequestSummary`] a line, oldest first;
//! - `bodies/<id>.json.gz`, what a request added to its conversation (see
//!   [`super::delta`]), gzipped.
//!
//! **Images and files are not kept.** What an agent sends as base64 — a
//! pasted screenshot, a PDF — is stored as its size and a SHA-256
//! fingerprint, which is enough to see that it went and that the same one
//! went again. **A body is capped** at [`BODY_CAP`]: its head and its tail,
//! and how much was cut between them.
//!
//! **Nothing outlives its worktree, or [`KEPT`].** A worktree's logs go when
//! it is removed, and the host sweeps anything older than five days every
//! hour. That is a rule, not a setting: these logs hold whatever the agent
//! read, secrets included, and are kept only as long as they are useful for
//! looking back at a session.

use std::collections::HashSet;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::flags::Flag;
use super::usage::Usage;

/// How long a request is kept.
pub const KEPT: Duration = Duration::from_secs(5 * 24 * 60 * 60);

/// The most of one body kept: its first [`HEAD`] bytes and last [`TAIL`].
pub const BODY_CAP: usize = HEAD + TAIL;
const HEAD: usize = 1_536 * 1024;
const TAIL: usize = 512 * 1024;

const INDEX: &str = "index.jsonl";
const BODIES: &str = "bodies";

/// Held around every change to the files, so a sweep never rewrites an
/// index while a request is being added to it.
static WRITING: Mutex<()> = Mutex::new(());

/// One request, as the panel lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestSummary {
    /// Unique within its worktree, and in the order requests were seen.
    pub id: String,
    /// When it was sent, in milliseconds since the Unix epoch.
    pub at: u64,
    /// The pane it came from, `worktree|agent|nonce`.
    pub pane: String,
    /// The agent, as the pane's key names it.
    pub agent: String,
    /// The provider's host.
    pub upstream: String,
    /// The provider's path, without the query.
    pub path: String,
    /// The model asked for; empty for a request that names none.
    #[serde(default)]
    pub model: String,
    /// The reply's status; 0 when no reply came.
    pub status: u16,
    /// Whether the reply came back whole.
    pub complete: bool,
    /// From sending to the reply's end.
    pub ms: u64,
    /// The request's size as sent, in bytes.
    pub sent: usize,
    /// The reply's size as received, in bytes.
    pub received: usize,
    /// Of what was sent, the system prompt's share.
    #[serde(default)]
    pub system_bytes: usize,
    /// The tools' share.
    #[serde(default)]
    pub tools_bytes: usize,
    /// The share that repeated messages sent before.
    #[serde(default)]
    pub resent_bytes: usize,
    /// The share this request added; all of it for one that is not a
    /// conversation.
    #[serde(default)]
    pub new_bytes: usize,
    /// How many messages it added.
    #[serde(default)]
    pub new_messages: usize,
    /// Whether it carried on from an earlier request in its session; one
    /// that did not is all new.
    #[serde(default)]
    pub continued: bool,
    /// Whether a body was kept — see [`body`].
    #[serde(default)]
    pub body: bool,
    /// Whether the body was cut to [`BODY_CAP`].
    #[serde(default)]
    pub truncated: bool,
    /// The tokens the reply said it took, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// What in the new part looked like an identifier or a secret, most
    /// serious first — see [`super::flags`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,
}

/// The directory `worktree`'s logs are in. `None` for an id that is not a
/// plain name, which never becomes a path.
pub fn worktree_dir(worktree: &str) -> Option<PathBuf> {
    let plain = !worktree.is_empty()
        && worktree != "."
        && worktree != ".."
        && worktree
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !plain {
        return None;
    }
    Some(crate::paths::network_dir().ok()?.join(worktree))
}

/// Whether `worktree` is one of ket's now: a worktree it made whose checkout
/// is still there, or a project's own checkout. Read from the store each
/// time, which is only on a worktree's first request, or its first after
/// its log was deleted. A store that cannot be read says no: one request
/// unlogged is better than one logged for a worktree that is gone.
fn registered(worktree: &str) -> bool {
    let Ok(state) = crate::store::Store::open_default().and_then(|store| store.load()) else {
        return false;
    };
    state
        .worktrees
        .iter()
        .any(|w| w.id.as_str() == worktree && w.exists())
        || worktree
            .strip_prefix("primary-")
            .is_some_and(|project| state.projects.iter().any(|p| p.id.to_string() == project))
}

fn private_dir(path: &std::path::Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// Adds `summary` to `worktree`'s log, with `body` if one was kept: already
/// readied by [`ready_body`].
pub(crate) fn append(
    worktree: &str,
    summary: &RequestSummary,
    body: Option<&[u8]>,
) -> io::Result<()> {
    let dir = worktree_dir(worktree)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a worktree id"))?;
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    // A log that is not there is started only for a worktree ket still has.
    // One deleted while its agent was mid-reply — or still running, from a
    // shell — would otherwise come back with that one request in it.
    if !dir.is_dir() && !registered(worktree) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the worktree is gone",
        ));
    }
    private_dir(&dir.join(BODIES))?;
    if let Some(body) = body {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join(BODIES).join(format!("{}.json.gz", summary.id)))?;
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        gz.write_all(body)?;
        gz.finish()?;
    }
    let mut line = serde_json::to_vec(summary).map_err(io::Error::other)?;
    line.push(b'\n');
    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(dir.join(INDEX))?
        .write_all(&line)
}

/// A body to keep: images and files reduced to their fingerprint, then
/// serialized and cut to [`BODY_CAP`]. Returns it and whether it was cut.
pub(crate) fn ready_body(mut body: Value) -> (Vec<u8>, bool) {
    strip_media(&mut body);
    let text = serde_json::to_string_pretty(&body).unwrap_or_default();
    cap(text)
}

/// A body that was not JSON, as text, cut to [`BODY_CAP`].
pub(crate) fn ready_raw(body: &[u8]) -> (Vec<u8>, bool) {
    cap(String::from_utf8_lossy(body).into_owned())
}

fn cap(text: String) -> (Vec<u8>, bool) {
    if text.len() <= BODY_CAP {
        return (text.into_bytes(), false);
    }
    let mut head = HEAD;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - TAIL;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    let cut = tail - head;
    let mut kept = String::with_capacity(BODY_CAP + 64);
    kept.push_str(&text[..head]);
    kept.push_str(&format!("\n\n… {cut} bytes not kept …\n\n"));
    kept.push_str(&text[tail..]);
    (kept.into_bytes(), true)
}

/// Replaces every base64 source — `{"type": "base64", "data": …}`, how the
/// Messages API carries an image or a document — with its size and the
/// start of its SHA-256.
fn strip_media(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(strip_media),
        Value::Object(map) => {
            let base64 = map.get("type").and_then(Value::as_str) == Some("base64");
            if base64 && let Some(Value::String(data)) = map.get("data") {
                let digest = ring::digest::digest(&ring::digest::SHA256, data.as_bytes());
                let hex: String = digest.as_ref()[..8]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                let bytes = data.len() / 4 * 3;
                map.insert(
                    "data".into(),
                    serde_json::json!({ "not_kept": true, "bytes": bytes, "sha256": hex }),
                );
                return;
            }
            map.values_mut().for_each(strip_media);
        }
        _ => {}
    }
}

/// `worktree`'s requests, oldest first. Empty when it has none.
pub fn summaries(worktree: &str) -> Vec<RequestSummary> {
    let Some(dir) = worktree_dir(worktree) else {
        return Vec::new();
    };
    read_index(&dir.join(INDEX))
}

fn read_index(path: &std::path::Path) -> Vec<RequestSummary> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

/// What request `id` of `worktree`'s added, as kept.
pub fn body(worktree: &str, id: &str) -> Option<String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let path = worktree_dir(worktree)?
        .join(BODIES)
        .join(format!("{id}.json.gz"));
    let mut text = String::new();
    flate2::read::GzDecoder::new(File::open(path).ok()?)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// Each worktree that has logs, with the bytes they take on disk.
pub fn stored() -> Vec<(String, u64)> {
    let Ok(root) = crate::paths::network_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name, size_of(&entry.path()))
        })
        .collect()
}

fn size_of(path: &std::path::Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size_of(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |meta| meta.len()),
            Err(_) => 0,
        })
        .sum()
}

/// Deletes request `id` from `worktree`'s log: its line, and its body.
pub fn remove_request(worktree: &str, id: &str) -> io::Result<()> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    let dir = worktree_dir(worktree)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a worktree id"))?;
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let index = dir.join(INDEX);
    let before = fs::read(&index)?;
    let mut kept = Vec::with_capacity(before.len());
    for line in before.split_inclusive(|&byte| byte == b'\n') {
        let other =
            serde_json::from_slice::<RequestSummary>(line).map_or(true, |summary| summary.id != id);
        if other {
            kept.extend_from_slice(line);
        }
    }
    // The host writes from another process, past this lock: what it added
    // while this was reading goes on the end rather than being lost.
    let now = fs::read(&index)?;
    if let Some(added) = now.strip_prefix(before.as_slice()) {
        kept.extend_from_slice(added);
    }
    let fresh = dir.join(format!("{INDEX}.new"));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&fresh)?;
    file.write_all(&kept)?;
    file.sync_all()?;
    fs::rename(&fresh, &index)?;
    match fs::remove_file(dir.join(BODIES).join(format!("{id}.json.gz"))) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Deletes `worktree`'s logs, if it has any.
pub fn remove_worktree(worktree: &str) {
    let Some(dir) = worktree_dir(worktree) else {
        return;
    };
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, worktree, "network inspector: could not delete the logs")
        }
    }
}

/// Deletes every worktree's logs.
pub fn remove_all() {
    let Ok(root) = crate::paths::network_dir() else {
        return;
    };
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    match fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, "network inspector: could not delete the logs"),
    }
}

/// Deletes every request older than [`KEPT`], and a worktree's directory
/// once nothing in it is left.
pub fn sweep_expired() {
    let Ok(root) = crate::paths::network_dir() else {
        return;
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let cutoff = now_ms().saturating_sub(KEPT.as_millis() as u64);
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    for entry in entries.filter_map(Result::ok) {
        let dir = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let Err(error) = sweep_dir(&dir, cutoff) {
            tracing::warn!(%error, dir = %dir.display(), "network inspector: could not sweep old logs");
        }
    }
}

fn sweep_dir(dir: &std::path::Path, cutoff: u64) -> io::Result<()> {
    let index = dir.join(INDEX);
    let all = read_index(&index);
    let kept: Vec<&RequestSummary> = all.iter().filter(|summary| summary.at >= cutoff).collect();
    if kept.is_empty() {
        return fs::remove_dir_all(dir);
    }
    if kept.len() < all.len() {
        let fresh = dir.join(format!("{INDEX}.new"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&fresh)?;
        for summary in &kept {
            let mut line = serde_json::to_vec(summary).map_err(io::Error::other)?;
            line.push(b'\n');
            file.write_all(&line)?;
        }
        file.sync_all()?;
        fs::rename(&fresh, &index)?;
    }
    let ids: HashSet<&str> = kept.iter().map(|summary| summary.id.as_str()).collect();
    if let Ok(bodies) = fs::read_dir(dir.join(BODIES)) {
        for body in bodies.filter_map(Result::ok) {
            let name = body.file_name();
            let id = name.to_string_lossy();
            let id = id.strip_suffix(".json.gz").unwrap_or(&id);
            if !ids.contains(id) {
                let _ = fs::remove_file(body.path());
            }
        }
    }
    Ok(())
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}
