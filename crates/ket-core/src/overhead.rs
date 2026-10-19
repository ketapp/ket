//! What ket costs the machine, and what the agents it runs cost beside it.
//!
//! Two figures, kept apart on purpose. **ket's own** is the window and the
//! host that serves its terminals: the part ket answers for, and the part a
//! person comparing it against an Electron app is asking about. **The agents'**
//! is every terminal session and agent process ket started, with everything
//! they started in turn: the part the person's own work answers for. Added
//! together, a build running in a terminal would read as ket being heavy.
//!
//! **Memory is the physical footprint**, `ri_phys_footprint` — the figure
//! Activity Monitor's Memory column shows, so what ket says can be checked
//! against it. Resident size is the other candidate and reads less than half
//! as much for a window drawn on the GPU, whose buffers it does not count.
//!
//! **CPU is a share of one core**, averaged since the previous reading — the
//! convention Activity Monitor uses, so four busy cores read 400%. What a
//! person is shown is the caller's to decide; the status bar divides by
//! [`crate::machine::cores`] and shows a share of the whole Mac.
//!
//! **Each session is read on its own** as well as in the total: what runs in
//! it, where, and what it costs — and the readings of the last
//! [`HISTORY`] are kept, for a chart.
//!
//! **Cheap enough to leave on.** A reading is a handful of syscalls per
//! process: list a process's children, read its usage. No `ps`, no fork, no
//! walk of every process on the machine — a `ps` sweep measured 17 ms of CPU
//! against 0.07 ms for walking one agent's tree. And the caller decides when
//! to read at all; the status bar reads only while it is open.
//!
//! **No `unsafe` and no private API.** The one awkward unit is CPU time read
//! for *another* process: the kernel reports it in Mach ticks, 125/3 ns each on
//! Apple silicon and 1 ns on Intel, and the call that says which is not
//! wrapped safely by anything ket depends on. So the rate is measured instead:
//! ket reads its own CPU time both ways — `getrusage`, in microseconds, and the
//! per-process call, in ticks — and the ratio of the two totals is the rate.
//! The longer ket runs, the more exact it gets.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How far back [`Meter::history`] goes.
pub const HISTORY: Duration = Duration::from_secs(10 * 60);

/// The shortest span a CPU figure is averaged over.
///
/// A window regaining focus reads at once, however recently it last read, and
/// a span of a second or two is the redraw that focus caused rather than what
/// ket costs sitting still. Inside this span a read returns the last reading
/// unchanged.
pub const MIN_SPAN: Duration = Duration::from_secs(5);

/// The most processes one reading will visit across every agent's tree.
///
/// A `make -j` fanned out under an agent is a few hundred; this is a bound on
/// a reading's cost, not a figure anyone should reach. Past it, the trees not
/// yet walked are left out of the reading rather than slowing the window.
const MOST: usize = 4096;

/// Memory and CPU, for one process or a group of them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Reading {
    /// Physical footprint, in bytes.
    pub memory: u64,
    /// Percent of one core since the previous reading. `None` until there has
    /// been one to measure from — never `0.0`, which would say "idle" about a
    /// figure nobody has.
    pub cpu: Option<f32>,
}

impl Reading {
    /// Two readings together. CPU only when both have a figure: a sum with a
    /// part missing would understate what it says it adds up.
    fn plus(self, other: Self) -> Self {
        Self {
            memory: self.memory + other.memory,
            cpu: self.cpu.zip(other.cpu).map(|(a, b)| a + b),
        }
    }
}

/// What kind of session a tree was walked from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A terminal ket opened: a shell, and whatever runs in it.
    Terminal,
    /// An agent the window started over its protocol, with no terminal.
    Agent,
}

/// One terminal session or agent, with everything it started.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// What it was started as.
    pub kind: SessionKind,
    /// What runs in it, as the system names the program: a terminal's
    /// foreground job — `cargo`, `claude` — or its shell at a prompt; an
    /// agent's own executable.
    pub program: String,
    /// Where it runs: its first process's working directory, when that could
    /// be read.
    pub cwd: Option<PathBuf>,
    /// What it costs. CPU is `None` for a session new since the previous
    /// reading.
    pub reading: Reading,
    /// How many processes its tree holds.
    pub processes: usize,
}

/// Every agent and terminal session ket started, with what they started.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Agents {
    /// What they add up to.
    ///
    /// CPU counts only sessions with a baseline, so a session that just
    /// started adds nothing rather than blanking the figure: agents come and
    /// go all day, and a total that went blank every time one started would
    /// rarely show anything.
    pub reading: Reading,
    /// Each one, in no particular order.
    pub sessions: Vec<Session>,
    /// Every process in their trees.
    pub processes: usize,
}

/// One reading of everything.
#[derive(Debug, Clone, PartialEq)]
pub struct Overhead {
    /// The process reading: ket's window.
    pub window: Reading,
    /// The host serving its terminals, when one is connected.
    pub host: Option<Reading>,
    /// What runs in them.
    pub agents: Agents,
}

impl Overhead {
    /// ket's own: the window and its host together.
    ///
    /// CPU only when every part has a figure. Window alone would understate a
    /// total that says it includes the host.
    pub fn ket(&self) -> Reading {
        match self.host {
            Some(host) => self.window.plus(host),
            None => self.window,
        }
    }

    /// ket and everything it runs.
    pub fn total(&self) -> Reading {
        self.ket().plus(self.agents.reading)
    }
}

/// One reading's [`Overhead::total`], when it was taken: a point on a chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// When.
    pub at: Instant,
    /// What it read.
    pub reading: Reading,
}

/// The window's own counters, read both ways — see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnRaw {
    memory: u64,
    /// From `getrusage`, in nanoseconds.
    cpu_ns: u64,
    /// The same time, from the per-process call, in ticks.
    ticks: u64,
}

/// A process ket did not read for itself, counted in ticks.
///
/// Known by its pid *and* when it started, so that a pid the system has
/// handed to someone else since the last reading starts a new baseline
/// rather than reading as a jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OtherRaw {
    id: (i32, u64),
    memory: u64,
    ticks: u64,
}

/// One session's tree, summed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeRaw {
    /// The root's pid and start time.
    id: (i32, u64),
    kind: SessionKind,
    /// See [`Session::program`].
    program: String,
    cwd: Option<PathBuf>,
    memory: u64,
    /// Every live process's own time, plus the time of children it has
    /// already reaped — so a compiler that ran and exited between two
    /// readings is in the second one through whoever waited for it.
    ticks: u64,
    processes: usize,
}

/// Reads [`Overhead`] and remembers enough to turn the next reading's CPU
/// time into a rate.
#[derive(Debug, Default)]
pub struct Meter {
    /// When the baselines below were taken.
    at: Option<Instant>,
    /// The window's CPU time then.
    window: Option<u64>,
    /// The host's id and ticks then.
    host: Option<((i32, u64), u64)>,
    /// Each session's ticks then, by its root's id.
    trees: HashMap<(i32, u64), u64>,
    /// Nanoseconds per tick, once ket has used enough CPU to measure it.
    ns_per_tick: Option<f64>,
    latest: Option<Overhead>,
    /// Each reading's total, oldest first, back as far as [`HISTORY`].
    history: VecDeque<Point>,
}

impl Meter {
    /// Reads everything now. `None` where ket cannot read its own usage at
    /// all, which today is anywhere but macOS.
    pub fn read(&mut self) -> Option<&Overhead> {
        let (own, host, trees) = sample::all()?;
        Some(self.advance(Instant::now(), own, host, &trees))
    }

    /// The last reading, without reading again.
    pub fn latest(&self) -> Option<&Overhead> {
        self.latest.as_ref()
    }

    /// The totals of the readings over the last [`HISTORY`], oldest first.
    ///
    /// Only readings that were taken: a caller that reads only now and then
    /// has a sparse history, and one that stopped and called [`Meter::forget`]
    /// has none from before.
    pub fn history(&self) -> &VecDeque<Point> {
        &self.history
    }

    /// Drops every baseline and the history, so the next reading starts over.
    ///
    /// For when readings stop for a while: the first figure after a pause
    /// should be what ket costs now, not an average over the pause — and a
    /// chart should not draw a line across a span nobody measured.
    pub fn forget(&mut self) {
        *self = Self {
            ns_per_tick: self.ns_per_tick,
            ..Self::default()
        };
    }

    /// Turns raw counters into a reading against the previous baselines, and
    /// keeps these as the next ones — or, inside [`MIN_SPAN`] of the last
    /// reading, gives that one again.
    fn advance(
        &mut self,
        now: Instant,
        own: OwnRaw,
        host_raw: Option<OtherRaw>,
        trees: &[TreeRaw],
    ) -> &Overhead {
        let too_soon = self
            .at
            .is_some_and(|at| now.saturating_duration_since(at) < MIN_SPAN);
        let overhead = match self.latest.take() {
            Some(latest) if too_soon => latest,
            _ => self.measure(now, own, host_raw, trees),
        };
        self.latest.insert(overhead)
    }

    fn measure(
        &mut self,
        now: Instant,
        own: OwnRaw,
        host_raw: Option<OtherRaw>,
        trees: &[TreeRaw],
    ) -> Overhead {
        let span = self.at.map(|at| now.saturating_duration_since(at));

        if let Some(rate) = ns_per_tick(own.cpu_ns, own.ticks) {
            self.ns_per_tick = Some(rate);
        }
        let rate = self.ns_per_tick;
        let ns = |ticks: u64| rate.map(|rate| (ticks as f64 * rate) as u64);

        let window = Reading {
            memory: own.memory,
            cpu: self
                .window
                .zip(span)
                .and_then(|(before, span)| percent(before, own.cpu_ns, span)),
        };

        let host = host_raw.map(|raw| {
            // Only against the same host: a reconnect to a new one, or a pid
            // reused, starts over.
            let cpu = match (self.host, span) {
                (Some((id, before)), Some(span)) if id == raw.id => ns(before)
                    .zip(ns(raw.ticks))
                    .and_then(|(before, after)| percent(before, after, span)),
                _ => None,
            };
            Reading {
                memory: raw.memory,
                cpu,
            }
        });

        // A session new since the last reading has no baseline, so no CPU of
        // its own and nothing in the total's; one that ended is simply gone.
        // A process reparented away from a tree takes its time with it: that
        // is a smaller total, not negative work.
        let busy = |tree: &TreeRaw| {
            let before = self.trees.get(&tree.id)?;
            Some(tree.ticks.saturating_sub(*before))
        };
        let cpu = |busy: Option<u64>| {
            span.zip(busy.and_then(ns))
                .and_then(|(span, busy)| percent(0, busy, span))
        };
        let sessions = trees
            .iter()
            .map(|tree| Session {
                kind: tree.kind,
                program: tree.program.clone(),
                cwd: tree.cwd.clone(),
                reading: Reading {
                    memory: tree.memory,
                    cpu: cpu(busy(tree)),
                },
                processes: tree.processes,
            })
            .collect();
        let agents = Agents {
            reading: Reading {
                memory: trees.iter().map(|tree| tree.memory).sum(),
                cpu: cpu(Some(trees.iter().filter_map(busy).sum())),
            },
            sessions,
            processes: trees.iter().map(|tree| tree.processes).sum(),
        };

        self.at = Some(now);
        self.window = Some(own.cpu_ns);
        self.host = host_raw.map(|raw| (raw.id, raw.ticks));
        self.trees = trees.iter().map(|tree| (tree.id, tree.ticks)).collect();

        let overhead = Overhead {
            window,
            host,
            agents,
        };
        self.history.push_back(Point {
            at: now,
            reading: overhead.total(),
        });
        while self
            .history
            .front()
            .is_some_and(|point| now.saturating_duration_since(point.at) > HISTORY)
        {
            self.history.pop_front();
        }
        overhead
    }
}

/// Nanoseconds per tick, from the same CPU time counted both ways. `None`
/// until there is any to divide.
fn ns_per_tick(ns: u64, ticks: u64) -> Option<f64> {
    (ns > 0 && ticks > 0).then(|| ns as f64 / ticks as f64)
}

/// CPU time spent over `span`, as a percent of one core.
///
/// `None` for a counter that went backwards — a different process, or a
/// baseline that no longer applies — and for an empty span.
fn percent(before_ns: u64, after_ns: u64, span: Duration) -> Option<f32> {
    let spent = after_ns.checked_sub(before_ns)?;
    let span = span.as_nanos();
    (span > 0).then(|| (spent as f64 / span as f64 * 100.0) as f32)
}

/// The kernel's counters, read without `unsafe` through `libproc` and `nix`.
/// Whether a process name is a version number — `2.1.288` — rather than a
/// program's name.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn versioned(name: &str) -> bool {
    name.contains('.') && name.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The program an executable at `path` was installed as, when it lives under
/// a `versions` directory: `claude` for `…/claude/versions/2.1.288`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn installed_as(path: &std::path::Path) -> Option<String> {
    let versions = path.parent()?;
    if versions.file_name()? != "versions" {
        return None;
    }
    let program = versions.parent()?.file_name()?.to_str()?;
    (!program.is_empty()).then(|| program.to_owned())
}

#[cfg(target_os = "macos")]
mod sample {
    use libproc::bsd_info::BSDInfo;
    use libproc::pid_rusage::{RUsageInfoV2, pidrusage};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use libproc::proc_pid::{PIDInfo, PidInfoFlavor, pidinfo, pidpath};
    use libproc::processes::{ProcFilter, pids_by_type};
    use nix::sys::resource::{UsageWho, getrusage};
    use nix::sys::time::TimeValLike;

    use super::{MOST, OtherRaw, OwnRaw, SessionKind, TreeRaw};

    /// Everything one reading needs: the window, its host, and the session
    /// trees under both.
    pub(super) fn all() -> Option<(OwnRaw, Option<OtherRaw>, Vec<TreeRaw>)> {
        let pid = i32::try_from(std::process::id()).ok()?;
        let own = own(pid)?;
        let host_pid = crate::host::host_pid().filter(|host| *host != pid);
        let host = host_pid.and_then(other);

        let mut budget = MOST;
        let mut trees = sessions(pid, true, &mut budget);
        if let Some(host_pid) = host_pid
            && host.is_some()
        {
            trees.extend(sessions(host_pid, false, &mut budget));
        }
        Some((own, host, trees))
    }

    /// This process, its CPU time read both ways.
    fn own(pid: i32) -> Option<OwnRaw> {
        let usage = getrusage(UsageWho::RUSAGE_SELF).ok()?;
        let cpu_ns = usage.user_time().num_nanoseconds() + usage.system_time().num_nanoseconds();
        let info = pidrusage::<RUsageInfoV2>(pid).ok()?;
        Some(OwnRaw {
            memory: info.ri_phys_footprint,
            cpu_ns: u64::try_from(cpu_ns).ok()?,
            ticks: info.ri_user_time + info.ri_system_time,
        })
    }

    /// Another process, on its own: the host. Not its children's time, which
    /// is the terminals' and counted with the agents.
    fn other(pid: i32) -> Option<OtherRaw> {
        let info = pidrusage::<RUsageInfoV2>(pid).ok()?;
        Some(OtherRaw {
            id: (pid, info.ri_proc_start_abstime),
            memory: info.ri_phys_footprint,
            ticks: info.ri_user_time + info.ri_system_time,
        })
    }

    /// What a process's controlling terminal and process group say about it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Lineage {
        /// `e_tdev`: the controlling terminal's device, or [`NO_TTY`].
        pub(super) tty: u32,
        pub(super) group: u32,
    }

    /// `e_tdev` for a process with no controlling terminal: `NODEV`.
    pub(super) const NO_TTY: u32 = u32::MAX;

    /// How a direct child stands to the parent that started it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Standing {
        /// On a terminal of its own: the PTY ket opened for it. A session,
        /// whichever process started it.
        Terminal,
        /// Leading a process group of its own, with no terminal of its own.
        /// The window starts three things this way: the host, the Android
        /// emulator, and every ACP agent — the protocol crate makes each one
        /// its own group so that ending it ends what its `npx` started too.
        Apart,
        /// In its parent's group: a `git`, a `ps`, a shell probe, running for
        /// a moment on its parent's behalf. Not a session.
        Helper,
    }

    pub(super) fn standing(pid: i32, child: Lineage, parent: Lineage) -> Standing {
        if child.tty != NO_TTY && child.tty != parent.tty {
            Standing::Terminal
        } else if u32::try_from(pid).is_ok_and(|pid| pid == child.group)
            && child.group != parent.group
        {
            Standing::Apart
        } else {
            Standing::Helper
        }
    }

    fn info(pid: i32) -> Option<BSDInfo> {
        pidinfo::<BSDInfo>(pid, 0).ok()
    }

    fn lineage_of(info: &BSDInfo) -> Lineage {
        Lineage {
            tty: info.e_tdev,
            group: info.pbi_pgid,
        }
    }

    #[cfg(test)]
    fn lineage(pid: i32) -> Option<Lineage> {
        info(pid).as_ref().map(lineage_of)
    }

    /// The program's name, as the system has it: the longer of its two
    /// spellings, which only the shorter, sixteen-character one cuts off.
    fn name_of(info: &BSDInfo) -> String {
        let long = text(&info.pbi_name);
        let name = if long.is_empty() {
            text(&info.pbi_comm)
        } else {
            long
        };
        // Claude Code's native install runs an executable named for its
        // version — `~/.local/share/claude/versions/2.1.288` — so the system
        // calls it `2.1.288`. The program is the directory above `versions`.
        if super::versioned(&name)
            && let Ok(path) = pidpath(i32::try_from(info.pbi_pid).unwrap_or(0))
            && let Some(program) = super::installed_as(Path::new(&path))
        {
            return program;
        }
        name
    }

    /// A C string from a fixed buffer, up to its terminator.
    fn text(raw: &[std::ffi::c_char]) -> String {
        let bytes: Vec<u8> = raw
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The job in front on a session's terminal, when that is not the
    /// session's own shell: the `cargo` a person is waiting on, rather than
    /// the `zsh` it was typed into.
    ///
    /// `e_tpgid` is the terminal's foreground process group, whose id is its
    /// leader's pid. A shell at its prompt is that leader itself.
    pub(super) fn foreground(root: i32, tpgid: u32) -> Option<i32> {
        i32::try_from(tpgid)
            .ok()
            .filter(|&leader| leader > 0 && leader != root)
    }

    /// What a session runs — see [`super::Session::program`].
    fn program(root: i32, info: &BSDInfo, kind: SessionKind) -> String {
        let front = match kind {
            SessionKind::Terminal => foreground(root, info.e_tpgid)
                .and_then(self::info)
                .map(|front| name_of(&front))
                .filter(|name| !name.is_empty()),
            SessionKind::Agent => None,
        };
        front.unwrap_or_else(|| name_of(info))
    }

    /// `struct proc_vnodepathinfo`: a process's working directory and its
    /// root, each a `vnode_info` followed by a path. Only the first path is
    /// read, so the rest is kept as bytes of the right length rather than
    /// spelled out field by field: 152 for the `vnode_info`, `MAXPATHLEN`
    /// for a path.
    #[repr(C)]
    struct Directories {
        _cwd_vnode: [u8; VNODE_INFO],
        cwd: [u8; MAXPATHLEN],
        _root: [u8; VNODE_INFO + MAXPATHLEN],
    }

    const VNODE_INFO: usize = 152;
    const MAXPATHLEN: usize = 1024;

    impl PIDInfo for Directories {
        fn flavor() -> PidInfoFlavor {
            PidInfoFlavor::VNodePathInfo
        }
    }

    /// Where `pid` is working: the directory a shell is in, or an agent was
    /// started in.
    pub(super) fn cwd(pid: i32) -> Option<PathBuf> {
        let dirs = pidinfo::<Directories>(pid, 0).ok()?;
        let end = dirs
            .cwd
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(dirs.cwd.len());
        (end > 0).then(|| PathBuf::from(OsStr::from_bytes(&dirs.cwd[..end])))
    }

    /// Whether `pid` runs ket's own executable: the host, which the window
    /// starts apart and which is ket rather than an agent. Known by its binary
    /// rather than by the connection's pid, because the window starts the host
    /// before it has connected to it.
    fn ket_itself(pid: i32) -> bool {
        static MINE: OnceLock<Option<PathBuf>> = OnceLock::new();
        let mine = MINE.get_or_init(|| std::env::current_exe().ok()?.canonicalize().ok());
        mine.as_ref().is_some_and(|mine| {
            pidpath(pid)
                .is_ok_and(|path| Path::new(&path).canonicalize().ok().as_ref() == Some(mine))
        })
    }

    /// `parent`'s children that are sessions, each with its tree summed.
    ///
    /// Under the host that is its terminals. Under the window it is also what
    /// it starts [`Standing::Apart`], less the two such things that are not
    /// agents: ket itself and an emulator, which says so with a file named
    /// after its pid.
    fn sessions(parent: i32, window: bool, budget: &mut usize) -> Vec<TreeRaw> {
        let Some(parent_lineage) = info(parent).as_ref().map(lineage_of) else {
            return Vec::new();
        };
        children(parent)
            .into_iter()
            .filter_map(|child| {
                let info = info(child)?;
                let kind = match standing(child, lineage_of(&info), parent_lineage) {
                    Standing::Terminal => SessionKind::Terminal,
                    Standing::Apart
                        if window
                            && !ket_itself(child)
                            && !u32::try_from(child).is_ok_and(crate::android::is_emulator) =>
                    {
                        SessionKind::Agent
                    }
                    Standing::Apart | Standing::Helper => return None,
                };
                let program = program(child, &info, kind);
                tree(child, budget).map(|tree| TreeRaw {
                    kind,
                    program,
                    cwd: cwd(child),
                    ..tree
                })
            })
            .collect()
    }

    fn children(pid: i32) -> Vec<i32> {
        let Ok(ppid) = u32::try_from(pid) else {
            return Vec::new();
        };
        pids_by_type(ProcFilter::ByParentProcess { ppid })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|child| i32::try_from(child).ok())
            .filter(|child| *child > 0)
            .collect()
    }

    /// `root` and everything under it, summed. `None` when the root has gone.
    ///
    /// Named for what it is counted as rather than what it runs; the caller,
    /// which knows, says.
    fn tree(root: i32, budget: &mut usize) -> Option<TreeRaw> {
        if *budget == 0 {
            return None;
        }
        let first = pidrusage::<RUsageInfoV2>(root).ok()?;
        *budget -= 1;
        let mut tree = TreeRaw {
            id: (root, first.ri_proc_start_abstime),
            kind: SessionKind::Terminal,
            program: String::new(),
            cwd: None,
            memory: 0,
            ticks: 0,
            processes: 0,
        };
        add(&mut tree, &first);

        let mut pending = children(root);
        while let Some(pid) = pending.pop() {
            if *budget == 0 {
                break;
            }
            // Unreadable is not the end of the branch: a `sudo` in a
            // terminal belongs to root, and what it runs may not.
            if let Ok(info) = pidrusage::<RUsageInfoV2>(pid) {
                *budget -= 1;
                add(&mut tree, &info);
            }
            pending.extend(children(pid));
        }
        Some(tree)
    }

    fn add(tree: &mut TreeRaw, info: &RUsageInfoV2) {
        tree.memory += info.ri_phys_footprint;
        tree.ticks += info.ri_user_time
            + info.ri_system_time
            + info.ri_child_user_time
            + info.ri_child_system_time;
        tree.processes += 1;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const SHELL_TTY: u32 = 0x1000_0005;

        fn line(tty: u32, group: u32) -> Lineage {
            Lineage { tty, group }
        }

        #[test]
        fn a_terminal_ket_opened_stands_on_its_own_terminal() {
            let host = line(NO_TTY, 100);
            assert_eq!(
                standing(200, line(SHELL_TTY, 200), host),
                Standing::Terminal
            );
        }

        #[test]
        fn a_helper_in_its_parents_group_is_a_helper() {
            // The host's `ps`, the window's `git`.
            let host = line(NO_TTY, 100);
            assert_eq!(standing(300, line(NO_TTY, 100), host), Standing::Helper);
            let window = line(0x1000_004e, 50);
            assert_eq!(
                standing(301, line(0x1000_004e, 50), window),
                Standing::Helper
            );
        }

        #[test]
        fn a_child_leading_its_own_group_stands_apart() {
            // An ACP agent, the host, the emulator: on the window's terminal
            // when it was started from one, on none when from the Finder.
            let window = line(0x1000_004e, 50);
            assert_eq!(standing(60, line(0x1000_004e, 60), window), Standing::Apart);
            let from_finder = line(NO_TTY, 50);
            assert_eq!(standing(60, line(NO_TTY, 60), from_finder), Standing::Apart);
        }

        #[test]
        fn a_child_in_someone_elses_group_is_not_apart() {
            // In a group, but not the one it leads: a job of a job.
            let window = line(NO_TTY, 50);
            assert_eq!(standing(61, line(NO_TTY, 60), window), Standing::Helper);
        }

        #[test]
        fn ket_knows_its_own_binary_and_not_another() {
            use std::os::unix::process::CommandExt;

            let pid = i32::try_from(std::process::id()).expect("a pid");
            assert!(ket_itself(pid));

            let mut apart = std::process::Command::new("/bin/sleep")
                .arg("5")
                .process_group(0)
                .spawn()
                .expect("sleep");
            let child = i32::try_from(apart.id()).expect("a pid");
            let found = lineage(child)
                .zip(lineage(pid))
                .map(|(child_line, parent)| standing(child, child_line, parent));
            let theirs = ket_itself(child);
            let _ = apart.kill();
            let _ = apart.wait();
            assert_eq!(found, Some(Standing::Apart));
            assert!(!theirs);
        }

        #[test]
        fn a_shell_at_its_prompt_is_its_own_foreground() {
            assert_eq!(foreground(200, 200), None);
            assert_eq!(foreground(200, 0), None, "no terminal");
            assert_eq!(foreground(200, u32::MAX), None, "not a pid");
            assert_eq!(foreground(200, 431), Some(431), "a job in front");
        }

        #[test]
        fn a_process_is_named_and_placed_as_the_system_has_it() {
            let dir = std::env::temp_dir()
                .canonicalize()
                .expect("a temp dir to start in");
            let mut child = std::process::Command::new("/bin/sleep")
                .arg("5")
                .current_dir(&dir)
                .spawn()
                .expect("sleep");
            let pid = i32::try_from(child.id()).expect("a pid");
            let named = info(pid).map(|info| name_of(&info));
            let placed = cwd(pid);
            let _ = child.kill();
            let _ = child.wait();
            assert_eq!(named.as_deref(), Some("sleep"));
            assert_eq!(placed, Some(dir));
        }

        #[test]
        fn this_process_reads_both_ways() {
            let pid = i32::try_from(std::process::id()).expect("a pid");
            // Some CPU to measure.
            let mut x = 0u64;
            for i in 0..2_000_000u64 {
                x = x.wrapping_add(i * i);
            }
            assert!(x > 0);
            let own = own(pid).expect("its own usage");
            assert!(own.memory > 0);
            assert!(own.ticks > 0);
            let rate = super::super::ns_per_tick(own.cpu_ns, own.ticks).expect("a rate");
            // 1 on Intel, 125/3 on Apple silicon; anything wildly off is the
            // two counters disagreeing about what they count.
            assert!((0.5..1000.0).contains(&rate), "{rate}");
        }

        #[test]
        fn a_child_is_found_in_the_tree_it_belongs_to() {
            let pid = i32::try_from(std::process::id()).expect("a pid");
            let mut child = std::process::Command::new("/bin/sleep")
                .arg("5")
                .spawn()
                .expect("sleep");
            let mut budget = MOST;
            let tree = tree(pid, &mut budget).expect("this process");
            let _ = child.kill();
            let _ = child.wait();
            assert!(tree.processes >= 2, "{tree:?}");
            assert!(tree.memory > 0);
        }
    }
}

/// Nowhere else reads its own usage yet.
#[cfg(not(target_os = "macos"))]
mod sample {
    use super::{OtherRaw, OwnRaw, TreeRaw};

    pub(super) fn all() -> Option<(OwnRaw, Option<OtherRaw>, Vec<TreeRaw>)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = 1_000_000_000;

    fn own(cpu_ns: u64) -> OwnRaw {
        // Apple silicon's rate: 3 ticks to every 125 ns.
        OwnRaw {
            memory: 100,
            cpu_ns,
            ticks: cpu_ns * 3 / 125,
        }
    }

    fn other(pid: i32, started: u64, ns: u64) -> OtherRaw {
        OtherRaw {
            id: (pid, started),
            memory: 50,
            ticks: ns * 3 / 125,
        }
    }

    fn tree(pid: i32, ns: u64) -> TreeRaw {
        TreeRaw {
            id: (pid, 1),
            kind: SessionKind::Terminal,
            program: format!("job-{pid}"),
            cwd: Some(PathBuf::from(format!("/work/{pid}"))),
            memory: 1_000,
            ticks: ns * 3 / 125,
            processes: 3,
        }
    }

    fn close(a: Option<f32>, b: f32) -> bool {
        a.is_some_and(|a| (a - b).abs() < 0.01)
    }

    fn session<'a>(overhead: &'a Overhead, program: &str) -> &'a Session {
        overhead
            .agents
            .sessions
            .iter()
            .find(|session| session.program == program)
            .expect("that session")
    }

    #[test]
    fn the_tick_rate_is_the_ratio_of_the_two_totals() {
        let rate = ns_per_tick(125_000, 3_000).expect("a rate");
        assert!((rate - 41.667).abs() < 0.001);
        assert_eq!(ns_per_tick(7, 7), Some(1.0));
        assert_eq!(ns_per_tick(0, 0), None);
        assert_eq!(ns_per_tick(5, 0), None);
    }

    #[test]
    fn percent_is_of_one_core() {
        let ten_s = Duration::from_secs(10);
        assert!(close(percent(0, SECOND, ten_s), 10.0));
        assert!(close(percent(SECOND, 21 * SECOND, ten_s), 200.0));
        assert_eq!(percent(2, 1, ten_s), None);
        assert_eq!(percent(0, 1, Duration::ZERO), None);
    }

    #[test]
    fn kets_own_is_the_window_and_the_host() {
        let both = Overhead {
            window: Reading {
                memory: 400,
                cpu: Some(0.3),
            },
            host: Some(Reading {
                memory: 60,
                cpu: Some(0.1),
            }),
            agents: Agents::default(),
        };
        assert_eq!(both.ket().memory, 460);
        assert!(close(both.ket().cpu, 0.4));

        let host_unread = Overhead {
            host: Some(Reading {
                memory: 60,
                cpu: None,
            }),
            ..both.clone()
        };
        assert_eq!(host_unread.ket().cpu, None, "a partial sum understates");

        let no_host = Overhead {
            host: None,
            ..both.clone()
        };
        assert_eq!(no_host.ket(), both.window);
    }

    #[test]
    fn the_total_is_ket_and_everything_it_runs() {
        let overhead = Overhead {
            window: Reading {
                memory: 400,
                cpu: Some(0.3),
            },
            host: None,
            agents: Agents {
                reading: Reading {
                    memory: 1_000,
                    cpu: Some(50.0),
                },
                sessions: Vec::new(),
                processes: 0,
            },
        };
        assert_eq!(overhead.total().memory, 1_400);
        assert!(close(overhead.total().cpu, 50.3));
    }

    #[test]
    fn the_first_reading_has_memory_and_no_cpu() {
        let mut meter = Meter::default();
        let now = Instant::now();
        let first = meter.advance(now, own(SECOND), Some(other(9, 1, 0)), &[tree(20, 0)]);
        assert_eq!(first.window.memory, 100);
        assert_eq!(first.window.cpu, None);
        assert_eq!(first.host.map(|h| h.cpu), Some(None));
        assert_eq!(first.agents.reading.memory, 1_000);
        assert_eq!(first.agents.reading.cpu, None);
        assert_eq!(first.agents.sessions.len(), 1);
        assert_eq!(first.agents.processes, 3);
        let only = &first.agents.sessions[0];
        assert_eq!(only.program, "job-20");
        assert_eq!(only.cwd.as_deref(), Some(std::path::Path::new("/work/20")));
        assert_eq!(only.reading.cpu, None);
    }

    #[test]
    fn the_second_reading_has_cpu_for_everything() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), Some(other(9, 1, 0)), &[tree(20, 0)]);
        let later = now + Duration::from_secs(10);
        let second = meter.advance(
            later,
            own(2 * SECOND),
            Some(other(9, 1, SECOND / 2)),
            &[tree(20, 5 * SECOND)],
        );
        assert!(close(second.window.cpu, 10.0));
        assert!(close(second.host.and_then(|h| h.cpu), 5.0));
        assert!(close(second.agents.reading.cpu, 50.0));
        assert!(close(second.agents.sessions[0].reading.cpu, 50.0));
    }

    #[test]
    fn each_session_is_read_on_its_own() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), None, &[tree(20, 0), tree(21, 0)]);
        let later = meter.advance(
            now + Duration::from_secs(10),
            own(SECOND),
            None,
            &[tree(20, 10 * SECOND), tree(21, SECOND)],
        );
        assert!(close(session(later, "job-20").reading.cpu, 100.0));
        assert!(close(session(later, "job-21").reading.cpu, 10.0));
        assert!(close(later.agents.reading.cpu, 110.0));
    }

    #[test]
    fn a_reading_inside_the_minimum_span_is_the_last_one() {
        let mut meter = Meter::default();
        let now = Instant::now();
        let first = meter.advance(now, own(SECOND), None, &[]).clone();
        let soon = meter.advance(now + Duration::from_secs(1), own(9 * SECOND), None, &[]);
        assert_eq!(*soon, first);
        assert_eq!(meter.history().len(), 1, "and adds nothing to the chart");
    }

    #[test]
    fn a_new_host_or_a_reused_pid_starts_over() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), Some(other(9, 1, 0)), &[]);
        let later = now + Duration::from_secs(10);
        // Same pid, started at a different time: a different process.
        let reused = meter.advance(later, own(SECOND), Some(other(9, 2, SECOND)), &[]);
        assert_eq!(reused.host.and_then(|h| h.cpu), None);
        // And the baseline is now that one's.
        let after = meter.advance(
            later + Duration::from_secs(10),
            own(SECOND),
            Some(other(9, 2, 2 * SECOND)),
            &[],
        );
        assert!(close(after.host.and_then(|h| h.cpu), 10.0));
    }

    #[test]
    fn a_host_gone_and_back_starts_over() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), Some(other(9, 1, 0)), &[]);
        let gone = meter.advance(now + Duration::from_secs(10), own(SECOND), None, &[]);
        assert_eq!(gone.host, None);
        let back = meter.advance(
            now + Duration::from_secs(20),
            own(SECOND),
            Some(other(9, 1, 5 * SECOND)),
            &[],
        );
        assert_eq!(back.host.and_then(|h| h.cpu), None);
    }

    #[test]
    fn a_new_session_adds_memory_and_no_cpu_and_an_ended_one_is_gone() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), None, &[tree(20, 0), tree(21, 0)]);
        let later = meter.advance(
            now + Duration::from_secs(10),
            own(SECOND),
            None,
            // 21 ended; 22 is new, with a lifetime of CPU behind it that is
            // not this span's.
            &[tree(20, SECOND), tree(22, 100 * SECOND)],
        );
        assert_eq!(later.agents.sessions.len(), 2);
        assert_eq!(later.agents.reading.memory, 2_000);
        assert!(close(later.agents.reading.cpu, 10.0));
        assert_eq!(session(later, "job-22").reading.cpu, None);
        assert_eq!(session(later, "job-22").reading.memory, 1_000);
    }

    #[test]
    fn a_tree_that_shrank_reads_as_idle_not_negative() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), None, &[tree(20, 10 * SECOND)]);
        let later = meter.advance(
            now + Duration::from_secs(10),
            own(SECOND),
            None,
            &[tree(20, 4 * SECOND)],
        );
        assert!(close(later.agents.reading.cpu, 0.0));
        assert!(close(later.agents.sessions[0].reading.cpu, 0.0));
    }

    #[test]
    fn the_history_keeps_each_readings_total_for_ten_minutes() {
        let mut meter = Meter::default();
        let start = Instant::now();
        let every = Duration::from_secs(10);
        // Eleven minutes of readings, ten seconds apart.
        for step in 0..=66u32 {
            meter.advance(
                start + every * step,
                own(u64::from(step) * SECOND),
                None,
                &[tree(20, 0)],
            );
        }
        let history = meter.history();
        let first = history.front().expect("a point");
        let last = history.back().expect("a point");
        assert_eq!(last.at - first.at, HISTORY);
        assert_eq!(history.len(), 61);
        // The window's 100 bytes and the session's 1,000.
        assert_eq!(last.reading.memory, 1_100);
        // A second of window CPU every ten seconds, and an idle session.
        assert!(close(last.reading.cpu, 10.0));
    }

    #[test]
    fn the_first_point_has_no_cpu_rather_than_none_at_all() {
        let mut meter = Meter::default();
        meter.advance(Instant::now(), own(SECOND), None, &[]);
        let first = meter.history().front().expect("a point");
        assert_eq!(first.reading.memory, 100);
        assert_eq!(first.reading.cpu, None);
    }

    #[test]
    fn forgetting_keeps_the_rate_and_drops_the_baselines_and_the_history() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.advance(now, own(SECOND), None, &[tree(20, 0)]);
        meter.forget();
        assert_eq!(meter.latest(), None);
        assert!(meter.history().is_empty());
        assert!(meter.ns_per_tick.is_some());
        let fresh = meter.advance(now + Duration::from_secs(1), own(2 * SECOND), None, &[]);
        assert_eq!(fresh.window.cpu, None, "no baseline to measure from");
    }
}
