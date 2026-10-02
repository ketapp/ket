//! The Epic 5b.0 spike harness: can a fresh terminal rebuild a live one from a
//! checkpoint and the output after it?
//!
//! Three steps, each runnable on its own:
//!
//! - `record <scenario> <dir>` runs a real program in a real pty through
//!   [`LocalTerminal`], checkpoints it at the start, and saves the checkpoint plus
//!   everything after it as one byte stream. It then rebuilds the live screen
//!   from that stream and compares, which checks the path a real client takes.
//! - `verify <dir>/<scenario>` replays a recording into a host terminal in
//!   randomly sized chunks, checkpoints it at hundreds of points, rebuilds a
//!   client from each, and compares the two cell by cell: straight away, a
//!   little later, and at the end. It also exports cases for xterm.js
//!   (`scripts/checkpoint-xterm`), which reads the same checkpoints.
//! - `all <dir>` does both for every scenario.
//!
//! Scenarios: `crafted`, `shell`, `vim`, `less`, `top`, `wide`, `flood`,
//! `claude`. Run it in release; the replay is quadratic-ish and debug is slow:
//!
//! ```sh
//! cargo run --release -p ket-core --example checkpoint -- all /tmp/ckpt
//! ```

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ket_core::terminal::alacritty_terminal::event::{Event, EventListener};
use ket_core::terminal::alacritty_terminal::grid::{Dimensions, Grid};
use ket_core::terminal::alacritty_terminal::index::{Column, Line};
use ket_core::terminal::alacritty_terminal::term::cell::{Cell, Flags};
use ket_core::terminal::alacritty_terminal::term::color::COUNT;
use ket_core::terminal::alacritty_terminal::term::{Config, Term, TermMode};
use ket_core::terminal::alacritty_terminal::vte::ansi::{Processor, Timeout};
use ket_core::terminal::checkpoint::{self, Stream};
use ket_core::terminal::{LocalTerminal, Since, TerminalSize, TerminalSpec};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["record", name, dir] => scenario(name).and_then(|s| record(&s, Path::new(dir))),
        ["verify", path] => verify(Path::new(path)).map(|_| ()),
        ["all", dir] => all(Path::new(dir)),
        _ => Err(format!(
            "usage: checkpoint record <scenario> <dir> | verify <dir>/<scenario> | all <dir>\n\
             scenarios: {}",
            SCENARIOS.join(", ")
        )),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

const SCENARIOS: &[&str] = &[
    "crafted",
    "shell",
    "vim",
    "less",
    "top",
    "wide",
    "flood",
    "dense",
    "resize",
    "rep",
    "stuck-sync",
    "claude",
    "claude-ui",
];

/// Records and verifies every scenario, and prints one line each at the end.
fn all(dir: &Path) -> Result<(), String> {
    let mut summary = Vec::new();
    for &name in SCENARIOS {
        println!("\n=== {name}");
        let outcome = scenario(name)
            .and_then(|s| record(&s, dir))
            .and_then(|()| verify(&dir.join(name)));
        summary.push((name, outcome));
    }
    println!("\n=== summary");
    for (name, outcome) in summary {
        match outcome {
            Ok(report) => println!("{name:8} {report}"),
            Err(error) => println!("{name:8} FAILED: {error}"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenarios

/// One program, and what to type into it.
struct Scenario {
    name: &'static str,
    command: Vec<String>,
    env: BTreeMap<String, String>,
    size: TerminalSize,
    steps: Vec<Step>,
}

enum Step {
    /// Sleep.
    Wait(u64),
    /// Type bytes.
    Send(Vec<u8>),
    /// Wait until the program has been silent this long, or five seconds.
    Quiet(u64),
    /// Resize the terminal, the way dragging a split does.
    Resize(u16, u16),
}

fn wait(ms: u64) -> Step {
    Step::Wait(ms)
}

fn send(text: &str) -> Step {
    Step::Send(text.as_bytes().to_vec())
}

fn quiet(ms: u64) -> Step {
    Step::Quiet(ms)
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join("ket-checkpoint-spike");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn sh(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

fn scenario(name: &str) -> Result<Scenario, String> {
    let dir = scratch();
    let sample = dir.join("sample.rs");
    std::fs::write(&sample, include_str!("../src/terminal.rs")).map_err(|e| e.to_string())?;
    let crafted = dir.join("crafted.bin");
    std::fs::write(&crafted, crafted_bytes()).map_err(|e| e.to_string())?;
    let dense = dir.join("dense.bin");
    std::fs::write(&dense, dense_bytes()).map_err(|e| e.to_string())?;
    let wide = dir.join("wide.txt");
    std::fs::write(&wide, wide_text()).map_err(|e| e.to_string())?;

    let size = TerminalSize::new(100, 30);
    let mut env = BTreeMap::new();
    let (command, steps) = match name {
        "crafted" => (
            sh(&format!("cat '{}'; sleep 0.3", crafted.display())),
            vec![quiet(400)],
        ),
        "shell" => {
            env.insert("CLICOLOR_FORCE".into(), "1".into());
            env.insert("PS1".into(), "\\[\\e[1;32m\\]spike\\[\\e[0m\\]$ ".into());
            (
                vec![
                    "/bin/bash".into(),
                    "--norc".into(),
                    "--noprofile".into(),
                    "-i".into(),
                ],
                vec![
                    quiet(300),
                    send("ls -laG /usr/bin | head -150\r"),
                    quiet(300),
                    send(
                        "printf '\\033[1;31mred\\033[0m \\033[4munder\\033[0m\\ttab\\tstops\\n'\r",
                    ),
                    quiet(200),
                    send(
                        "echo 'a line long enough to wrap across the edge of a hundred column terminal, twice over, to see wrap flags survive'\r",
                    ),
                    quiet(200),
                    send("exit\r"),
                ],
            )
        }
        "vim" => (
            vec![
                "/usr/bin/vim".into(),
                "-u".into(),
                "NONE".into(),
                "-N".into(),
                "-c".into(),
                "syntax on".into(),
                sample.display().to_string(),
            ],
            vec![
                quiet(500),
                send("120G"),
                quiet(200),
                send("\x06"),
                quiet(200),
                send("\x06"),
                quiet(200),
                send("\x02"),
                quiet(200),
                send(":split\r"),
                quiet(200),
                send("ohello from the spike\x1b"),
                quiet(200),
                send("\x05\x05\x05"),
                quiet(200),
                send(":qa!\r"),
            ],
        ),
        "less" => (
            vec![
                "/usr/bin/less".into(),
                "-R".into(),
                crafted.display().to_string(),
            ],
            vec![
                quiet(400),
                send(" "),
                quiet(200),
                send(" "),
                quiet(200),
                send("/tab\r"),
                quiet(200),
                send("G"),
                quiet(200),
                send("q"),
            ],
        ),
        "top" => (
            vec![
                "/usr/bin/top".into(),
                "-o".into(),
                "cpu".into(),
                "-s".into(),
                "1".into(),
            ],
            vec![wait(3500), send("q")],
        ),
        "wide" => (
            sh(&format!("cat '{}'; sleep 0.3", wide.display())),
            vec![quiet(400)],
        ),
        "flood" => (sh("seq 1 40000; sleep 0.3"), vec![quiet(500)]),
        "resize" => {
            env.insert("PS1".into(), "spike$ ".into());
            (
                vec![
                    "/bin/bash".into(),
                    "--norc".into(),
                    "--noprofile".into(),
                    "-i".into(),
                ],
                vec![
                    quiet(300),
                    send(
                        "seq 1 300; echo 'a long line that will reflow when the width changes under it, and again'\r",
                    ),
                    quiet(300),
                    Step::Resize(60, 20),
                    quiet(300),
                    send("echo narrower now\r"),
                    quiet(300),
                    Step::Resize(130, 35),
                    quiet(300),
                    send("exit\r"),
                ],
            )
        }
        "rep" => (
            // REP repeats the last character *printed*, which the parser
            // remembers privately. Hundreds of them, so random checkpoints
            // land between a character and its repeat.
            // Adversarial: the status line on the bottom row is what a
            // checkpoint prints last, so it is not what REP should repeat.
            sh(
                "for i in $(seq 1 150); do printf '\\033[30;1Hstatus %s\\033[5;%sHZ\\033[2b' $i $((i % 90 + 1)); done; sleep 0.3",
            ),
            vec![quiet(400)],
        ),
        "stuck-sync" => (
            // Opens a synchronized update and never closes it, the way a
            // program killed mid-frame does, then keeps writing.
            sh(
                "printf 'before\\n\\033[?2026hinside a frame that never ends\\n'; sleep 0.5; printf 'after\\n'; sleep 0.3",
            ),
            vec![quiet(600)],
        ),
        "dense" => (
            sh(&format!("cat '{}'; sleep 0.3", dense.display())),
            vec![quiet(500)],
        ),
        "claude" => (
            // Opened in an empty scratch directory, where it asks whether to
            // trust the folder. Nothing is sent to a model: Escape declines
            // and it exits.
            vec!["claude".into()],
            vec![
                wait(5000),
                send("\x1b"),
                wait(1500),
                send("\x03"),
                wait(500),
                send("\x03"),
            ],
        ),
        "claude-ui" => (
            // Types a draft and opens the slash menu, and never presses
            // Enter: nothing reaches a model. Two ^C exit.
            vec!["claude".into()],
            vec![
                wait(6000),
                send("this is a draft the spike types and never sends"),
                wait(1500),
                send("\x15"),
                wait(500),
                send("/"),
                wait(1500),
                send("\x1b[B\x1b[B"),
                wait(800),
                send("\x1b"),
                wait(800),
                send("\x03"),
                wait(400),
                send("\x03"),
                wait(1000),
            ],
        ),
        other => return Err(format!("no scenario called {other}")),
    };

    let name = SCENARIOS
        .iter()
        .find(|&&n| n == name)
        .copied()
        .unwrap_or("unknown");
    Ok(Scenario {
        name,
        command,
        env,
        size,
        steps,
    })
}

/// A stream written to exercise what real programs rarely do all at once.
fn crafted_bytes() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("\x1b]2;crafted title\x07");
    s.push_str("\x1b]4;1;rgb:12/34/56\x1b\\\x1b]10;rgb:ee/dd/cc\x1b\\");
    for i in 0..8 {
        s.push_str(&format!("\x1b[3{i}mfg{i}\x1b[4{i}mbg{i}\x1b[0m "));
    }
    s.push_str(
        "\r\n\x1b[1;2;3;4;7;9mall\x1b[0m \x1b[4:3;58;5;196mcurl\x1b[0m \x1b[21mdouble\x1b[0m\r\n",
    );
    s.push_str("\x1b[38;5;208m256\x1b[38;2;10;200;30mrgb\x1b[48;5;17mbg\x1b[0m\r\n");
    s.push_str("\x1b]8;id=a;https://example.com\x1b\\a link\x1b]8;;\x1b\\ and \x1b]8;;https://no-id.example\x1b\\anon\x1b]8;;\x1b\\\r\n");
    s.push_str("tabs:\tone\ttwo\tthree\r\n");
    // Custom tab stops.
    s.push_str("\x1b[3g");
    for col in [5, 13, 21, 50] {
        s.push_str(&format!("\x1b[{col}G\x1bH"));
    }
    s.push_str("\rcustom\ta\tb\tc\td\r\n");
    // Line drawing.
    s.push_str("\x1b(0lqqqqk\r\nx    x\r\nmqqqqj\x1b(B\r\n");
    // Pending wrap at the last column, then a wrapped line, then a wide char
    // that does not fit.
    s.push_str(&"=".repeat(100));
    s.push_str("\r\n");
    s.push_str(&"w".repeat(150));
    s.push_str("\r\n");
    s.push_str(&"-".repeat(99));
    s.push_str("世界\r\n");
    // Combining characters and REP.
    s.push_str("e\u{301} a\u{308} x\x1b[5b\r\n");
    // Saved cursor, insert mode, scroll region with origin mode.
    s.push_str("\x1b[5;10H\x1b[33msaved\x1b7\x1b[0m\x1b[20;1H");
    s.push_str("insert: 12345\x1b[5D\x1b[4hXY\x1b[4l\r\n");
    s.push_str("\x1b[10;20r\x1b[?6h\x1b[5;1Hin region\x1b[?6l\x1b[r");
    s.push_str("\x1b[25;1H");
    for i in 0..40 {
        s.push_str(&format!("scroll {i}\r\n"));
    }
    // A synchronized update split around content.
    s.push_str("\x1b[?2026hsynced \x1b[1mframe\x1b[0m\x1b[?2026l\r\n");
    // Modes left set at the end.
    s.push_str("\x1b[?1h\x1b=\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?25l\x1b[5 q");
    // The alternate screen, with its own content and cursor.
    // Line drawing left designated on the primary screen, which the
    // alternate screen's cursor inherits and the program then resets.
    s.push_str("\x1b(0\x1b[?1049h\x1b(B\x1b[2J\x1b[H\x1b[44malt screen\x1b[0m\r\n\x1b[3;3Hthere\x1b7\x1b[10;40H\x1b[32m");
    s.into_bytes()
}

/// Wide and combining characters at every awkward column.
/// The worst case for a checkpoint's size: a full scrollback of full lines,
/// every cell in a colour of its own.
fn dense_bytes() -> Vec<u8> {
    let mut s = String::new();
    for line in 0..4000u32 {
        for col in 0..100u32 {
            let fg = (line * 7 + col) % 256;
            let bg = (line + col * 3) % 256;
            let c = char::from(b'!' + ((line + col) % 90) as u8);
            s.push_str(&format!("\x1b[38;5;{fg};48;5;{bg}m{c}"));
        }
        s.push_str("\x1b[0m\r\n");
    }
    s.into_bytes()
}

fn wide_text() -> String {
    let mut s = String::new();
    for pad in 95..100 {
        s.push_str(&" ".repeat(pad));
        s.push_str("中文字符\r\n");
    }
    s.push_str("🙂🙃🙂🙃 emoji\r\n");
    s.push_str("ﾊﾝｶｸ half width\r\n");
    s.push_str("क्षि devanagari, ก่ thai\r\n");
    for _ in 0..30 {
        s.push_str("日本語のテキストが折り返す。");
    }
    s.push_str("\r\n");
    s
}

// ---------------------------------------------------------------------------
// Recording

/// Metadata saved next to a recording.
#[derive(serde::Serialize, serde::Deserialize)]
struct Meta {
    cols: u16,
    rows: u16,
    scrollback: usize,
}

fn record(s: &Scenario, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let cwd = if s.name == "claude-ui" {
        // A folder Claude Code already trusts, so it goes straight to its UI.
        std::env::current_dir().map_err(|e| e.to_string())?
    } else if s.name == "claude" {
        let empty = scratch().join("empty-project");
        let _ = std::fs::create_dir_all(&empty);
        empty
    } else {
        scratch()
    };
    let spec = TerminalSpec {
        command: s.command[0].clone(),
        args: s.command[1..].to_vec(),
        cwd,
        env: s.env.clone(),
        env_remove: Vec::new(),
        size: s.size,
        key: None,
        adopt: false,
    };
    let terminal = LocalTerminal::open(&spec).map_err(|e| e.to_string())?;
    let mut rec = Collector::new(&terminal);

    for step in &s.steps {
        match step {
            Step::Wait(ms) => rec.sleep(&terminal, Duration::from_millis(*ms)),
            Step::Send(bytes) => {
                let _ = terminal.send_input(bytes.clone());
            }
            Step::Quiet(ms) => quiet_for(&terminal, &mut rec, *ms),
            Step::Resize(cols, rows) => {
                let _ = terminal.resize(TerminalSize::new(*cols, *rows));
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !terminal.is_closed() && Instant::now() < deadline {
        rec.sleep(&terminal, Duration::from_millis(50));
    }
    if !terminal.is_closed() {
        println!("  {} did not exit; killing it", s.name);
        terminal.kill();
        rec.sleep(&terminal, Duration::from_millis(200));
    }

    // The last of the output and the live grid, under the same lock, so the
    // rebuild is compared against exactly the screen it should produce.
    // Only `output_since` in here: a checkpoint takes the grid lock this
    // already holds.
    rec.pump(&terminal);
    let first = rec.first.clone();
    let (stream, live) = terminal.with_term(|live| {
        let mut stream = rec.stream.clone();
        match terminal.output_since(rec.cursor) {
            Since::Output { bytes, .. } => stream.extend_from_slice(&bytes),
            Since::Reset => return Err("fell behind at the very end".to_owned()),
        }
        let mut client = Client::new(first.size, first.scrollback);
        client.feed(&stream);
        let diffs = differences(live, &client.term);
        Ok((stream, diffs))
    })?;
    if rec.resets > 0 {
        println!(
            "  reset {} times (a resize, or falling behind the replay buffer) and re-checkpointed",
            rec.resets
        );
    }

    let meta = Meta {
        cols: first.size.cols,
        rows: first.size.rows,
        scrollback: first.scrollback,
    };
    std::fs::write(dir.join(format!("{}.bin", s.name)), &stream).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join(format!("{}.json", s.name)),
        serde_json::to_string(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    println!(
        "  recorded {} bytes; live rebuild {}",
        stream.len(),
        if live.is_empty() {
            "matches".to_owned()
        } else {
            format!("DIFFERS:\n    {}", live.join("\n    "))
        }
    );
    Ok(())
}

/// A client of a live terminal: a checkpoint, then whatever comes after it,
/// read every few milliseconds the way a remote client would.
///
/// A client that falls further behind than the replay buffer takes a new
/// checkpoint and carries on. Its bytes are still a valid stream, because a
/// checkpoint starts with a full reset — unless the size changed, which no
/// escape sequence can tell a terminal. Then the stream starts again from the
/// new checkpoint, as a real client would rebuild its terminal.
struct Collector {
    first: ket_core::terminal::Checkpoint,
    cursor: ket_core::terminal::OutputCursor,
    stream: Vec<u8>,
    resets: usize,
}

impl Collector {
    fn new(terminal: &LocalTerminal) -> Self {
        let first = terminal.checkpoint();
        Self {
            cursor: first.cursor,
            stream: first.ansi.clone(),
            first,
            resets: 0,
        }
    }

    fn pump(&mut self, terminal: &LocalTerminal) {
        match terminal.output_since(self.cursor) {
            Since::Output { bytes, cursor } => {
                self.stream.extend_from_slice(&bytes);
                self.cursor = cursor;
            }
            Since::Reset => {
                let fresh = terminal.checkpoint();
                if fresh.size == self.first.size && fresh.scrollback == self.first.scrollback {
                    self.stream.extend_from_slice(&fresh.ansi);
                } else {
                    self.stream = fresh.ansi.clone();
                    self.first = fresh.clone();
                }
                self.cursor = fresh.cursor;
                self.resets += 1;
            }
        }
    }

    fn sleep(&mut self, terminal: &LocalTerminal, how_long: Duration) {
        let until = Instant::now() + how_long;
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            self.pump(terminal);
        }
    }
}

/// Waits until the terminal has produced no frame for `ms`, or five seconds.
fn quiet_for(terminal: &LocalTerminal, rec: &mut Collector, ms: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = terminal.frame();
    let mut since = Instant::now();
    while Instant::now() < deadline {
        rec.sleep(terminal, Duration::from_millis(20));
        let now = terminal.frame();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= Duration::from_millis(ms) {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Verifying

/// Records what a terminal announces that a checkpoint has to carry.
#[derive(Clone, Default)]
struct Listener {
    title: Rc<RefCell<Option<String>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => *self.title.borrow_mut() = Some(title),
            Event::ResetTitle => *self.title.borrow_mut() = None,
            _ => {}
        }
    }
}

/// A terminal fed bytes the way `ket-core` feeds its own.
struct Client {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
}

impl Client {
    fn new(size: TerminalSize, scrollback: usize) -> Self {
        let listener = Listener::default();
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            listener,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn title(&self) -> Option<String> {
        self.listener.title.borrow().clone()
    }
}

/// A client built from one checkpoint, waiting to be compared.
struct Pending {
    client: Client,
    /// Where in the host's output the checkpoint was taken.
    offset: usize,
    cursor: ket_core::terminal::OutputCursor,
    /// Compare once the host reaches this offset.
    check_at: usize,
    /// Kept to the end of the recording after that.
    to_end: bool,
}

/// How far after a checkpoint the "a little later" comparison happens.
const LATER: usize = 8 * 1024;

#[derive(Default)]
struct Tally {
    checkpoints: usize,
    mid_sequence: usize,
    in_sync: usize,
    immediate_bad: usize,
    later: usize,
    later_bad: usize,
    end: usize,
    end_bad: usize,
    sizes: Vec<usize>,
    times: Vec<Duration>,
    /// Time spent copying the screen: what the grid lock covers.
    locked: Vec<Duration>,
    examples: Vec<String>,
}

impl Tally {
    fn note(&mut self, what: &str, offset: usize, diffs: &[String]) {
        if self.examples.len() < 12 {
            self.examples.push(format!(
                "{what} @ {offset}:\n      {}",
                diffs.join("\n      ")
            ));
        }
    }
}

fn verify(base: &Path) -> Result<String, String> {
    let stream = std::fs::read(base.with_extension("bin")).map_err(|e| e.to_string())?;
    let meta: Meta = serde_json::from_str(
        &std::fs::read_to_string(base.with_extension("json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let size = TerminalSize::new(meta.cols, meta.rows);
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut host = Client::new(size, meta.scrollback);
    let mut recorder = Stream::new();
    recorder.arm();
    let mut rng = Lcg(0x5eed ^ stream.len() as u64);
    let every = (stream.len() / 300).max(16);
    // Small enough that even a short recording is split into many reads.
    let max_chunk = (stream.len() / 100).clamp(16, 4096);
    let mut next_checkpoint = 0;
    let mut pending: Vec<Pending> = Vec::new();
    let mut tally = Tally::default();
    let mut cases = Vec::new();
    let mut offset = 0;

    while offset < stream.len() {
        let len = (rng.next() as usize % max_chunk + 1).min(stream.len() - offset);
        let chunk = &stream[offset..offset + len];
        host.parser.advance(&mut host.term, chunk);
        let held = host
            .parser
            .sync_timeout()
            .pending_timeout()
            .then(|| host.parser.sync_bytes_count());
        recorder.record(chunk, held);
        offset += len;

        // Clients due for comparison catch up through the replay buffer, the
        // way a real client would.
        for p in pending.iter_mut().filter(|p| offset >= p.check_at) {
            if let Since::Output { bytes, cursor } = recorder.since(p.cursor) {
                p.client.feed(&bytes);
                p.cursor = cursor;
            } else {
                return Err(format!("replay lost output at {offset}"));
            }
        }
        let mut kept = Vec::new();
        for mut p in pending.drain(..) {
            if offset < p.check_at {
                kept.push(p);
                continue;
            }
            tally.later += 1;
            let diffs = compare(&host, &p.client);
            if !diffs.is_empty() {
                tally.later_bad += 1;
                tally.note("later", p.offset, &diffs);
            }
            if p.to_end {
                p.check_at = usize::MAX;
                kept.push(p);
            }
        }
        pending = kept;
        // Clients kept to the end are fed as the host goes, so the replay
        // buffer never has to hold the whole recording.
        for p in pending
            .iter_mut()
            .filter(|p| p.check_at == usize::MAX && p.cursor != recorder.cursor())
        {
            p.client.feed(chunk);
            p.cursor = recorder.cursor();
        }

        if offset >= next_checkpoint && offset < stream.len() {
            next_checkpoint = offset + every;
            // `KET_SPIKE_NO_TAIL=1` is the negative control: without the
            // tail, checkpoints taken mid-sequence have to fail.
            let tail = if std::env::var_os("KET_SPIKE_NO_TAIL").is_some() {
                checkpoint::Tail::default()
            } else {
                recorder.tail()
            };
            // The copy is what the grid lock covers; the writing is not.
            let started = Instant::now();
            let snapshot = checkpoint::Snapshot::of(&host.term);
            tally.locked.push(started.elapsed());
            let ansi = checkpoint::serialise(&snapshot, host.title().as_deref(), &tail);
            tally.times.push(started.elapsed());
            tally.sizes.push(ansi.len());
            tally.checkpoints += 1;
            if !tail.bytes.is_empty() {
                if tail.bytes.starts_with(b"\x1b[?2026h") {
                    tally.in_sync += 1;
                } else {
                    tally.mid_sequence += 1;
                }
            }

            let mut client = Client::new(size, meta.scrollback);
            client.feed(&ansi);
            let diffs = compare(&host, &client);
            if !diffs.is_empty() {
                tally.immediate_bad += 1;
                tally.note("immediate", offset, &diffs);
            }
            if cases.len() < 24 && (tally.checkpoints % 12 == 1 || !tail.bytes.is_empty()) {
                cases.push((offset, ansi));
            }
            pending.push(Pending {
                client,
                offset,
                cursor: recorder.cursor(),
                check_at: offset + LATER,
                to_end: tally.checkpoints % 10 == 0,
            });
        }
    }

    for mut p in pending {
        if p.check_at != usize::MAX
            && let Since::Output { bytes, .. } = recorder.since(p.cursor)
        {
            p.client.feed(&bytes);
        }
        tally.end += 1;
        let diffs = compare(&host, &p.client);
        if !diffs.is_empty() {
            tally.end_bad += 1;
            tally.note("end", p.offset, &diffs);
        }
    }

    export_xterm(base, &name, &meta, &stream, &cases, &host)?;
    Ok(report(&name, stream.len(), &mut tally))
}

fn report(name: &str, bytes: usize, t: &mut Tally) -> String {
    t.sizes.sort_unstable();
    t.times.sort_unstable();
    t.locked.sort_unstable();
    let locked_max = t.locked.last().copied().unwrap_or_default();
    let median = |v: &[usize]| v.get(v.len() / 2).copied().unwrap_or(0);
    let size_median = median(&t.sizes);
    let size_max = t.sizes.last().copied().unwrap_or(0);
    let time_median = t.times.get(t.times.len() / 2).copied().unwrap_or_default();
    let time_max = t.times.last().copied().unwrap_or_default();

    println!(
        "  {name}: {bytes} bytes, {} checkpoints ({} mid-sequence, {} in a synchronized update)",
        t.checkpoints, t.mid_sequence, t.in_sync
    );
    println!(
        "    immediately: {} of {} differ",
        t.immediate_bad, t.checkpoints
    );
    println!(
        "    +{} KiB:     {} of {} differ",
        LATER / 1024,
        t.later_bad,
        t.later
    );
    println!("    at the end:  {} of {} differ", t.end_bad, t.end);
    println!(
        "    checkpoint size: median {} KiB, max {} KiB; total: median {:?}, max {:?}; under the lock: max {:?}",
        size_median / 1024,
        size_max / 1024,
        time_median,
        time_max,
        locked_max
    );
    for example in &t.examples {
        println!("    {example}");
    }
    format!(
        "{} checkpoints ({} mid-seq, {} sync); differ: {} now, {} later, {} at end; max {} KiB, {:?} ({:?} locked)",
        t.checkpoints,
        t.mid_sequence,
        t.in_sync,
        t.immediate_bad,
        t.later_bad,
        t.end_bad,
        size_max / 1024,
        time_max,
        locked_max
    )
}

/// A deterministic source of chunk sizes, so a failure reproduces.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

// ---------------------------------------------------------------------------
// Comparing

fn compare(host: &Client, client: &Client) -> Vec<String> {
    let mut diffs = differences(&host.term, &client.term);
    if host.title() != client.title() {
        diffs.push(format!("title: {:?} vs {:?}", host.title(), client.title()));
    }
    diffs.truncate(6);
    diffs
}

/// Everything a person or a program could observe that differs, up to a few.
fn differences<A, B>(host: &Term<A>, client: &Term<B>) -> Vec<String> {
    let mut out = Vec::new();
    let mask = !TermMode::VI;
    if (*host.mode() & mask) != (*client.mode() & mask) {
        out.push(format!(
            "mode: {:?} vs {:?}",
            *host.mode() & mask,
            *client.mode() & mask
        ));
    }
    if host.scroll_region() != client.scroll_region() {
        out.push(format!(
            "scroll region: {:?} vs {:?}",
            host.scroll_region(),
            client.scroll_region()
        ));
    }
    if let Some(col) =
        (0..host.columns()).find(|&c| host.is_tab_stop(Column(c)) != client.is_tab_stop(Column(c)))
    {
        out.push(format!("tab stop at column {col}"));
    }
    if let Some(index) = (0..COUNT).find(|&i| host.colors()[i] != client.colors()[i]) {
        out.push(format!(
            "colour {index}: {:?} vs {:?}",
            host.colors()[index],
            client.colors()[index]
        ));
    }
    if host.cursor_style() != client.cursor_style() {
        out.push(format!(
            "cursor style: {:?} vs {:?}",
            host.cursor_style(),
            client.cursor_style()
        ));
    }
    grids("screen", host.grid(), client.grid(), &mut out);
    if host.mode().contains(TermMode::ALT_SCREEN) {
        grids(
            "primary under alt",
            host.inactive_grid(),
            client.inactive_grid(),
            &mut out,
        );
    }
    out
}

fn grids(label: &str, a: &Grid<Cell>, b: &Grid<Cell>, out: &mut Vec<String>) {
    if a.columns() != b.columns()
        || a.screen_lines() != b.screen_lines()
        || a.history_size() != b.history_size()
    {
        out.push(format!(
            "{label}: {}x{}+{} vs {}x{}+{}",
            a.columns(),
            a.screen_lines(),
            a.history_size(),
            b.columns(),
            b.screen_lines(),
            b.history_size()
        ));
        return;
    }
    let mut rows = 0;
    for line in a.topmost_line().0..=a.bottommost_line().0 {
        let (ra, rb) = (&a[Line(line)], &b[Line(line)]);
        if let Some(col) = (0..a.columns()).find(|&c| !same(&ra[Column(c)], &rb[Column(c)])) {
            rows += 1;
            if rows <= 2 {
                out.push(format!(
                    "{label} line {line} col {col}: {} vs {}",
                    show(&ra[Column(col)]),
                    show(&rb[Column(col)])
                ));
                out.push(format!(
                    "    {:?}\n       vs {:?}",
                    text(a, line),
                    text(b, line)
                ));
            }
        }
    }
    if rows > 2 {
        out.push(format!("{label}: {rows} lines differ in all"));
    }
    if a.cursor != b.cursor {
        out.push(format!(
            "{label} cursor: {:?} wrap={} vs {:?} wrap={} (template {} vs {})",
            a.cursor.point,
            a.cursor.input_needs_wrap,
            b.cursor.point,
            b.cursor.input_needs_wrap,
            show(&a.cursor.template),
            show(&b.cursor.template)
        ));
        if a.cursor.charsets != b.cursor.charsets {
            out.push(format!("{label} charsets differ"));
        }
    }
    if a.saved_cursor != b.saved_cursor {
        out.push(format!(
            "{label} saved cursor: {:?} vs {:?} (template {} vs {})",
            a.saved_cursor.point,
            b.saved_cursor.point,
            show(&a.saved_cursor.template),
            show(&b.saved_cursor.template)
        ));
    }
}

/// Whether two cells are the same, allowing for hyperlink ids alacritty made up.
///
/// A link sent without an id gets one from a process-wide counter, so two
/// terminals fed the very same bytes disagree about it. Only the URI is the
/// program's.
fn same(a: &Cell, b: &Cell) -> bool {
    if a == b {
        return true;
    }
    let generated = |cell: &Cell| {
        cell.hyperlink()
            .is_some_and(|l| l.id().ends_with("_alacritty"))
    };
    if !(generated(a) && generated(b)) {
        return false;
    }
    let uri = |cell: &Cell| cell.hyperlink().map(|l| l.uri().to_owned());
    let strip = |cell: &Cell| {
        let mut cell = cell.clone();
        cell.set_hyperlink(None);
        cell
    };
    uri(a) == uri(b) && strip(a) == strip(b)
}

fn show(cell: &Cell) -> String {
    let mut s = format!("{:?} fg={:?} bg={:?}", cell.c, cell.fg, cell.bg);
    if !cell.flags.is_empty() {
        s.push_str(&format!(" {:?}", cell.flags));
    }
    if let Some(zw) = cell.zerowidth() {
        s.push_str(&format!(" zw={zw:?}"));
    }
    if let Some(ul) = cell.underline_color() {
        s.push_str(&format!(" ul={ul:?}"));
    }
    if let Some(link) = cell.hyperlink() {
        s.push_str(&format!(" link={}:{}", link.id(), link.uri()));
    }
    s
}

fn text(grid: &Grid<Cell>, line: i32) -> String {
    let row = &grid[Line(line)];
    let mut s = String::new();
    for col in 0..grid.columns() {
        let cell = &row[Column(col)];
        if !cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            // A tab's cell is blank on screen; alacritty only records the tab
            // for copying.
            s.push(if cell.c == '\t' { ' ' } else { cell.c });
            s.extend(cell.zerowidth().unwrap_or_default());
        }
    }
    s.trim_end().to_owned()
}

// ---------------------------------------------------------------------------
// xterm.js export

#[derive(serde::Serialize)]
struct XtermCase {
    /// Where in `stream` the checkpoint was taken; the rest follows from there.
    offset: usize,
    checkpoint: String,
}

#[derive(serde::Serialize)]
struct XtermExport {
    name: String,
    cols: u16,
    rows: u16,
    scrollback: usize,
    stream: String,
    cases: Vec<XtermCase>,
    alacritty: Vec<String>,
    alacritty_cursor: (i32, usize),
}

/// Writes what `scripts/checkpoint-xterm` needs: the whole recording, the
/// sampled checkpoints with the output after each, and what alacritty showed
/// at the end, as text.
fn export_xterm(
    base: &Path,
    name: &str,
    meta: &Meta,
    stream: &[u8],
    cases: &[(usize, Vec<u8>)],
    host: &Client,
) -> Result<(), String> {
    let grid = host.term.grid();
    let export = XtermExport {
        name: name.to_owned(),
        cols: meta.cols,
        rows: meta.rows,
        scrollback: meta.scrollback,
        stream: hex(stream),
        cases: cases
            .iter()
            .map(|(offset, ansi)| XtermCase {
                offset: *offset,
                checkpoint: hex(ansi),
            })
            .collect(),
        alacritty: (grid.topmost_line().0..=grid.bottommost_line().0)
            .map(|line| text(grid, line))
            .collect(),
        alacritty_cursor: (grid.cursor.point.line.0, grid.cursor.point.column.0),
    };
    std::fs::write(
        base.with_extension("xterm.json"),
        serde_json::to_string(&export).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}
