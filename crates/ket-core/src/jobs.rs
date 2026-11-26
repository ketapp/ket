//! What an agent left running: the commands it started that are still going,
//! and the ports they serve on.
//!
//! **Read from the process tree, so it works for every agent.** An agent runs
//! each command in a shell of its own — Claude as `zsh -c 'source <its
//! snapshot> … eval '<command>''`, Codex as `zsh -lc <command>` — and makes
//! that shell the leader of a process group of its own, off the agent's
//! terminal. Those shells, and only those, are what this module finds: a
//! direct child of the agent that leads its own group and is not on the
//! agent's terminal. A `caffeinate` the agent keeps beside it stays in the
//! agent's group and is passed over; so is anything that lives where the
//! agent's own executable does (Codex's `codex-code-mode-host`), and anything
//! started with the agent itself (its MCP servers), which are the agent's
//! machinery rather than work it was asked to do.
//!
//! **Which of them are in the background is the caller's call.** A command the
//! agent is waiting on looks the same in the tree as one it left running; only
//! the agent's turn tells them apart. What this module says is what is there.
//!
//! **Cheap when nothing is running.** [`candidates`] is a few syscalls per
//! agent and starts nothing. The command line and the ports need `ps` and
//! `lsof`, because reading either through `libproc` needs `unsafe` (a union
//! for a socket's address, `sysctl` for arguments) and ket allows none here.
//! Both take every process asked about in one run, so a caller that has no
//! candidates never forks, and one that remembers a job's command asks `ps`
//! about each job once.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod claude;

/// Anything that started within this long of its agent is the agent's own
/// machinery — its MCP servers, its helpers — not a command it ran.
pub const STARTUP_MS: u64 = 10_000;

/// Most candidates one agent is read for; past it a list is one nobody reads.
pub const MOST: usize = 32;

/// How long [`stop`] gives a job to end on `SIGTERM` before `SIGKILL`.
pub const GRACE: Duration = Duration::from_secs(3);

/// A command an agent left running in the background, or one that ended a
/// moment ago — as a pane's status carries it to windows and phones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// The process that runs it, which leads its group — see [`Candidate`].
    /// What [`stop`] is asked to end, with `started_at_ms`.
    pub pid: i32,
    /// When it started, in milliseconds since the epoch.
    pub started_at_ms: u64,
    /// What it runs, as [`asked`] gives it.
    pub command: String,
    /// What the agent said it was for, where it says: Claude does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The TCP ports it serves on, lowest first: a dev server's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    /// How it ended, for the few minutes an ended job is still shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// The end of a [`Job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ended {
    /// When it was seen to have ended, in milliseconds since the epoch.
    pub at_ms: u64,
    /// How.
    pub outcome: Outcome,
    /// Its exit code, where the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
}

/// How a [`Job`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// It ran to the end and exited 0.
    Completed,
    /// It exited with anything else.
    Failed,
    /// Something stopped it.
    Stopped,
    /// It is no longer running, and nothing said how it ended.
    Gone,
}

impl From<claude::Status> for Outcome {
    fn from(status: claude::Status) -> Self {
        match status {
            claude::Status::Completed => Self::Completed,
            claude::Status::Failed => Self::Failed,
            claude::Status::Killed => Self::Stopped,
        }
    }
}

/// Where process `pid` is working: the directory an agent was started in.
/// `None` when it has gone, and off macOS.
pub fn cwd_of(pid: i32) -> Option<PathBuf> {
    sample::cwd(pid)
}

/// Ends the job `pid` leads — it and everything it started — if `pid` is
/// still the process that started at `started_at_ms` and still leads its own
/// group: never anything else, should the number have gone to another
/// process. `SIGTERM` first, then, after [`GRACE`], `SIGKILL` to whatever of
/// the group is left. Returns whether it was signalled.
pub fn stop(pid: i32, started_at_ms: u64) -> bool {
    sample::stop(pid, started_at_ms)
}

/// A process the agent started to run a command, still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Candidate {
    /// Its pid, which is also its process group's id: stopping it signals the
    /// group, and so everything the command started.
    pub pid: i32,
    /// When it started, in milliseconds since the epoch. With the pid, what
    /// tells this process from a later one given the same number.
    pub started_at_ms: u64,
}

/// What walks remember from one tick to the next: each process's executable
/// name, by pid and start, so that a process is asked for its path once
/// rather than on every walk. A process no walk met since the last
/// [`Names::settle`] is forgotten then.
#[derive(Debug, Default)]
pub struct Names {
    known: HashMap<(i32, u64), Option<String>>,
    met: HashSet<(i32, u64)>,
}

impl Names {
    /// Forgets every process the walks since the last call did not meet.
    pub fn settle(&mut self) {
        let met = std::mem::take(&mut self.met);
        self.known.retain(|process, _| met.contains(process));
    }
}

/// The processes `agent` started to run commands that are still running,
/// oldest first. Empty when `agent` has gone, and off macOS.
pub fn candidates(agent: i32, names: &mut Names) -> Vec<Candidate> {
    let mut found = sample::candidates(agent, names);
    found.sort_by_key(|c| (c.started_at_ms, c.pid));
    found.truncate(MOST);
    found
}

/// What each of `pids` was asked to run, as the person or agent wrote it:
/// see [`asked`]. A pid that has gone is left out.
pub fn commands(pids: &[i32]) -> HashMap<i32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    sample::command_lines(pids)
        .into_iter()
        .map(|(pid, line)| (pid, asked(&line)))
        .collect()
}

/// The TCP ports each of `groups` listens on, lowest first, keyed by group.
/// A group listening on none is left out.
pub fn listening(groups: &[i32]) -> HashMap<i32, Vec<u16>> {
    if groups.is_empty() {
        return HashMap::new();
    }
    sample::lsof(groups)
        .map(|out| ports(&out))
        .unwrap_or_default()
}

/// The command a process runs, out of the shell the agent wrapped it in.
///
/// `line` is a command line as `ps` prints it: the arguments joined by spaces.
/// A shell started with `-c` runs what follows the flag; when that is
/// Claude's wrapper, the command is the single-quoted word it hands to `eval`.
/// Anything else — a shell the command replaced with `exec`, a server started
/// directly — is its own command line, with the program's directory dropped.
///
/// Where its output went is dropped from the end — `> run.log 2>&1` says
/// nothing about what the command is.
pub fn asked(line: &str) -> String {
    let line = unvis(line.trim());
    let script = shell_script(&line);
    let command = match script {
        Some(script) => evaluated(script).unwrap_or_else(|| script.to_owned()),
        None => {
            let (program, rest) = line.split_once(' ').unwrap_or((&line, ""));
            let program = program.rsplit('/').next().unwrap_or(program);
            format!("{program} {rest}")
        }
    };
    unredirected(command.trim()).to_owned()
}

/// A command line as `ps` writes it back as it was: `ps` escapes what it
/// cannot print the way `vis(3)` does, a newline as `\012` and a backslash as
/// `\\`. Escaped bytes are put back as bytes, so a name in UTF-8 comes back
/// whole.
fn unvis(line: &str) -> String {
    let mut out = Vec::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        if chars.peek() == Some(&'\\') {
            chars.next();
            out.push(b'\\');
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        match u8::from_str_radix(&digits, 8) {
            Ok(byte) if digits.len() == 3 && digits.chars().all(|d| d.is_digit(8)) => {
                out.push(byte);
                chars.nth(2);
            }
            _ => out.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `command` without the redirections at its end: `> out.log`, `2>&1`,
/// `>| file`, `< /dev/null`. One line only: a script's last line says nothing
/// about the first.
fn unredirected(command: &str) -> &str {
    if command.contains('\n') {
        return command;
    }
    let redirect = |token: &str| {
        let token = token.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        token.starts_with('>') || token.starts_with('<')
    };
    let mut end = command.len();
    loop {
        let head = command[..end].trim_end();
        let Some((before, last)) = head.rsplit_once(' ') else {
            return head;
        };
        // `2>&1`, `>out.log`: the whole redirection in one word.
        if redirect(last) && last.len() > 2 && !last.ends_with('>') && !last.ends_with('<') {
            end = before.len();
            continue;
        }
        // `> out.log`: an operator, then its file.
        match before.trim_end().rsplit_once(' ') {
            Some((rest, operator)) if redirect(operator) && operator.len() <= 3 => {
                end = rest.len();
            }
            _ => return head,
        }
    }
}

/// What a `sh`/`bash`/`zsh` started with `-c` (or `-lc`, `-ic`, …) runs.
fn shell_script(line: &str) -> Option<&str> {
    let (program, rest) = line.split_once(' ')?;
    let program = program.rsplit('/').next()?;
    if !matches!(program, "sh" | "bash" | "zsh" | "-sh" | "-bash" | "-zsh") {
        return None;
    }
    let (flags, script) = rest.split_once(' ')?;
    let runs_script =
        flags.starts_with('-') && !flags.starts_with("--") && flags[1..].contains('c');
    runs_script.then_some(script)
}

/// The word Claude's wrapper hands to `eval`: everything after its shell
/// snapshot is sourced and its options set.
///
/// The word is single-quoted, and a single quote inside it is written the
/// shell's way, `'"'"'` — close the quote, a double-quoted `'`, reopen — so
/// the word is read as runs of quoted text until the first bare space.
fn evaluated(script: &str) -> Option<String> {
    if !script.contains("shell-snapshots/") {
        return None;
    }
    let start = script.find(" eval '")? + " eval ".len();
    let mut word = String::new();
    let mut chars = script[start..].chars();
    loop {
        match chars.next() {
            Some('\'') => {
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            Some('"') => {
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    word.push(c);
                }
            }
            _ => break,
        }
    }
    (!word.trim().is_empty()).then_some(word)
}

/// Ports per process group, out of `lsof -F gn`: a `g` line names the group of
/// the lines after it, an `n` line the address listened on — `*:8801`,
/// `127.0.0.1:4201`, `[::1]:3000`. The same port on IPv4 and IPv6 is one.
fn ports(out: &str) -> HashMap<i32, Vec<u16>> {
    let mut found: HashMap<i32, Vec<u16>> = HashMap::new();
    let mut group = None;
    for line in out.lines() {
        match line.split_at_checked(1) {
            Some(("p", _)) => group = None,
            Some(("g", id)) => group = id.parse::<i32>().ok(),
            Some(("n", address)) => {
                let port = address
                    .rsplit(':')
                    .next()
                    .and_then(|p| p.parse::<u16>().ok());
                if let (Some(group), Some(port)) = (group, port) {
                    let ports = found.entry(group).or_default();
                    if !ports.contains(&port) {
                        ports.push(port);
                    }
                }
            }
            _ => {}
        }
    }
    for ports in found.values_mut() {
        ports.sort_unstable();
    }
    found
}

#[cfg(target_os = "macos")]
mod sample {
    use std::collections::HashMap;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use libproc::bsd_info::BSDInfo;
    use libproc::proc_pid::{pidinfo, pidpath};
    use libproc::processes::{ProcFilter, pids_by_type};
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    use super::{Candidate, GRACE, Names, STARTUP_MS};
    use crate::overhead::sample::children;
    pub(super) use crate::overhead::sample::cwd;

    /// How many of the agent's own processes are walked looking for commands.
    /// An agent installed through npm is `node` with the real agent under it;
    /// nothing is deeper than a few.
    const WALK: usize = 16;

    fn info(pid: i32) -> Option<BSDInfo> {
        pidinfo::<BSDInfo>(pid, 0).ok()
    }

    fn started_at_ms(info: &BSDInfo) -> u64 {
        info.pbi_start_tvsec * 1000 + info.pbi_start_tvusec / 1000
    }

    /// An executable's file name, asked of the system once per process.
    fn file_name(names: &mut Names, pid: i32, started_at_ms: u64) -> Option<String> {
        let process = (pid, started_at_ms);
        names.met.insert(process);
        names
            .known
            .entry(process)
            .or_insert_with(|| {
                let path = pidpath(pid).ok()?;
                Some(Path::new(&path).file_name()?.to_string_lossy().into_owned())
            })
            .clone()
    }

    /// Walks the agent's own process group — the agent, and whatever it runs
    /// in its group, such as the native Codex an npm install's `node` starts —
    /// and keeps each child that left it for a group of its own off the
    /// agent's terminal.
    pub(super) fn candidates(agent: i32, names: &mut Names) -> Vec<Candidate> {
        let Some(own) = info(agent) else {
            return Vec::new();
        };
        let born = started_at_ms(&own);
        let mut found = Vec::new();
        let mut pending = vec![(agent, born)];
        let mut walked = 0;
        while let Some((parent, parent_started)) = pending.pop() {
            walked += 1;
            if walked > WALK {
                break;
            }
            // `codex` starts `codex-code-mode-host`: named for the agent that
            // started it, and part of it.
            let helper = file_name(names, parent, parent_started).map(|name| format!("{name}-"));
            for pid in children(parent) {
                let Some(info) = info(pid) else {
                    continue;
                };
                let started_at_ms = started_at_ms(&info);
                if info.pbi_pgid == own.pbi_pgid {
                    pending.push((pid, started_at_ms));
                    continue;
                }
                let leads = u32::try_from(pid).is_ok_and(|pid| pid == info.pbi_pgid);
                if !leads || info.e_tdev == own.e_tdev || started_at_ms < born + STARTUP_MS {
                    continue;
                }
                let helps = helper.as_ref().is_some_and(|helper| {
                    file_name(names, pid, started_at_ms)
                        .is_some_and(|name| name.starts_with(helper))
                });
                if !helps {
                    found.push(Candidate { pid, started_at_ms });
                }
            }
        }
        found
    }

    fn list(ids: &[i32]) -> String {
        ids.iter().map(i32::to_string).collect::<Vec<_>>().join(",")
    }

    pub(super) fn command_lines(pids: &[i32]) -> HashMap<i32, String> {
        let Ok(out) = Command::new("/bin/ps")
            .args(["-ww", "-o", "pid=,command=", "-p", &list(pids)])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return HashMap::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (pid, command) = line.trim_start().split_once(' ')?;
                Some((pid.parse().ok()?, command.to_owned()))
            })
            .collect()
    }

    /// `lsof`'s listening TCP sockets for `groups`. It exits 1 when it finds
    /// none, which is an answer rather than a failure, so only a missing
    /// `lsof` is `None`.
    pub(super) fn lsof(groups: &[i32]) -> Option<String> {
        let out = Command::new("/usr/sbin/lsof")
            .args(["-nP", "-w", "-a", "-iTCP", "-sTCP:LISTEN", "-F", "gn", "-g"])
            .arg(list(groups))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Whether `pid` is the process that started at `started_at_ms`.
    fn same(pid: i32, started_at_ms: u64) -> bool {
        info(pid).is_some_and(|info| self::started_at_ms(&info) == started_at_ms)
    }

    pub(super) fn stop(pid: i32, started_at_ms: u64) -> bool {
        let leads = info(pid).is_some_and(|info| {
            self::started_at_ms(&info) == started_at_ms
                && u32::try_from(pid).is_ok_and(|pid| pid == info.pbi_pgid)
        });
        if !leads || killpg(Pid::from_raw(pid), Signal::SIGTERM).is_err() {
            return false;
        }
        let _ = std::thread::Builder::new()
            .name("ket-job-stop".into())
            .spawn(move || {
                std::thread::sleep(GRACE);
                // A group's id is not given to a new process while anything
                // is left in the group, so what is left is still the job's —
                // unless the leader's number now names a different process.
                let Ok(group) = u32::try_from(pid) else {
                    return;
                };
                let left = pids_by_type(ProcFilter::ByProgramGroup { pgrpid: group })
                    .is_ok_and(|members| !members.is_empty());
                let reused = info(pid).is_some() && !same(pid, started_at_ms);
                if left && !reused {
                    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                }
            });
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod sample {
    use std::collections::HashMap;

    use super::{Candidate, Names};

    pub(super) fn candidates(_agent: i32, _names: &mut Names) -> Vec<Candidate> {
        Vec::new()
    }

    pub(super) fn cwd(_pid: i32) -> Option<std::path::PathBuf> {
        None
    }

    pub(super) fn command_lines(_pids: &[i32]) -> HashMap<i32, String> {
        HashMap::new()
    }

    pub(super) fn lsof(_groups: &[i32]) -> Option<String> {
        None
    }

    pub(super) fn stop(_pid: i32, _started_at_ms: u64) -> bool {
        false
    }
}
