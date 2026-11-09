//! `ket browser`: a worktree's browser tab, driven from its terminal.

use clap::Subcommand;
use ket_core::KetError;
use ket_core::command::{self, Category, Invocation, Registry};
use ket_core::host::wire::{BrowserScroll, BrowserTarget, BrowserWait, SnapshotNode};
use ket_core::host::{BrowserAction, BrowserAnswer};

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    // The browser's commands are the CLI's alone as well: they are how an
    // agent in a terminal reaches the tab, which in the app is right there.
    let browser = |id: &str, title: &str| {
        command::Command::new(id, title, Category::Agent).arg(command::Arg::required(
            "worktree",
            command::ArgKind::Worktree,
        ))
    };

    registry.register(
        browser(
            "browser.open",
            "Load a page in the worktree's browser tab, opening one if need be",
        )
        .arg(command::Arg::optional("url", command::ArgKind::Text)),
    )?;

    registry.register(browser("browser.tabs", "List the worktree's browser tabs"))?;

    registry.register(browser(
        "browser.back",
        "Go back a page in the worktree's browser tab",
    ))?;

    registry.register(browser(
        "browser.forward",
        "Go forward a page in the worktree's browser tab",
    ))?;

    registry.register(browser(
        "browser.reload",
        "Reload the worktree's browser tab",
    ))?;

    registry.register(
        browser(
            "browser.screenshot",
            "Save a PNG of the worktree's browser tab",
        )
        .arg(command::Arg::flag("full_page")),
    )?;

    registry.register(
        browser(
            "browser.console",
            "Print what the worktree's browser tab logged to its console",
        )
        .arg(command::Arg::flag("errors"))
        .arg(command::Arg::optional("since", command::ArgKind::Text))
        .arg(command::Arg::optional("lines", command::ArgKind::Text)),
    )?;

    registry.register(
        browser(
            "browser.eval",
            "Run JavaScript in the worktree's browser tab and print what it comes to",
        )
        .arg(command::Arg::required("expression", command::ArgKind::Text)),
    )?;

    registry.bind("browser.open", |invocation, _| {
        let url = invocation.text_of("url").map(str::to_owned);
        drive_browser(invocation, BrowserAction::Open { url })
    })?;

    registry.bind("browser.tabs", |invocation, _| {
        drive_browser(invocation, BrowserAction::Tabs)
    })?;

    registry.bind("browser.back", |invocation, _| {
        drive_browser(invocation, BrowserAction::Back)
    })?;

    registry.bind("browser.forward", |invocation, _| {
        drive_browser(invocation, BrowserAction::Forward)
    })?;

    registry.bind("browser.reload", |invocation, _| {
        drive_browser(invocation, BrowserAction::Reload)
    })?;

    registry.bind("browser.screenshot", |invocation, _| {
        let full_page = invocation.is_set("full_page");
        drive_browser(invocation, BrowserAction::Screenshot { full_page })
    })?;

    registry.bind("browser.console", |invocation, _| {
        let since_ms = invocation
            .text_of("since")
            .map(|since| age_ms(since, "--since"))
            .transpose()?;
        let lines = match invocation.text_of("lines") {
            Some(lines) => lines
                .parse()
                .map_err(|_| KetError::Conflict(format!("--lines takes a count, not {lines}")))?,
            None => 200,
        };
        let errors_only = invocation.is_set("errors");
        drive_browser(
            invocation,
            BrowserAction::Console {
                errors_only,
                since_ms,
                lines,
            },
        )
    })?;

    registry.bind("browser.eval", |invocation, _| {
        let expression = match invocation.require_text("expression")? {
            "-" => {
                std::io::read_to_string(std::io::stdin()).map_err(|e| KetError::io("stdin", e))?
            }
            expression => expression.to_owned(),
        };
        if expression.trim().is_empty() {
            return Err(KetError::Conflict("say what to run".to_owned()));
        }
        drive_browser(invocation, BrowserAction::Eval { expression })
    })?;

    registry.register(
        browser(
            "browser.snapshot",
            "Print the worktree's browser tab's accessibility tree, with refs to act on",
        )
        .arg(command::Arg::flag("interactive"))
        .arg(command::Arg::optional("max", command::ArgKind::Text)),
    )?;

    registry.bind("browser.snapshot", |invocation, _| {
        drive_browser(invocation, snapshot_action(invocation)?)
    })?;

    registry.register(
        browser(
            "browser.click",
            "Click an element in the worktree's browser tab",
        )
        .arg(command::Arg::required("target", command::ArgKind::Text))
        .arg(command::Arg::flag("double"))
        .arg(command::Arg::flag("right")),
    )?;

    registry.register(
        browser(
            "browser.fill",
            "Put text in a field in the worktree's browser tab",
        )
        .arg(command::Arg::required("target", command::ArgKind::Text))
        .arg(command::Arg::required("text", command::ArgKind::Text)),
    )?;

    registry.bind("browser.click", |invocation, _| {
        let target = browser_target(invocation.require_text("target")?)?;
        let (double, right) = (invocation.is_set("double"), invocation.is_set("right"));
        drive_browser(
            invocation,
            BrowserAction::Click {
                target,
                double,
                right,
            },
        )
    })?;

    registry.bind("browser.fill", |invocation, _| {
        let target = browser_target(invocation.require_text("target")?)?;
        let text = text_or_stdin(invocation.require_text("text")?)?;
        drive_browser(invocation, BrowserAction::Fill { target, text })
    })?;

    registry.register(
        browser(
            "browser.type",
            "Type text into what has focus in the worktree's browser tab",
        )
        .arg(command::Arg::required("text", command::ArgKind::Text))
        .arg(command::Arg::flag("keys")),
    )?;

    registry.register(
        browser("browser.press", "Press a key in the worktree's browser tab")
            .arg(command::Arg::required("key", command::ArgKind::Text)),
    )?;

    registry.bind("browser.type", |invocation, _| {
        let text = text_or_stdin(invocation.require_text("text")?)?;
        if text.is_empty() {
            return Err(KetError::Conflict("say what to type".to_owned()));
        }
        let keys = invocation.is_set("keys");
        drive_browser(invocation, BrowserAction::Type { text, keys })
    })?;

    registry.bind("browser.press", |invocation, _| {
        let key = invocation.require_text("key")?.trim().to_owned();
        if key.is_empty() {
            return Err(KetError::Conflict("say which key to press".to_owned()));
        }
        drive_browser(invocation, BrowserAction::Press { key })
    })?;

    registry.register(
        browser(
            "browser.scroll",
            "Scroll the worktree's browser tab, or an element in it",
        )
        .arg(command::Arg::optional("target", command::ArgKind::Text))
        .arg(command::Arg::optional("to", command::ArgKind::Text))
        .arg(command::Arg::optional("by", command::ArgKind::Text)),
    )?;

    registry.register(
        browser(
            "browser.wait",
            "Wait for something to be so in the worktree's browser tab",
        )
        .arg(command::Arg::optional("target", command::ArgKind::Text))
        .arg(command::Arg::optional("text", command::ArgKind::Text))
        .arg(command::Arg::optional("url", command::ArgKind::Text))
        .arg(command::Arg::flag("load"))
        .arg(command::Arg::optional("gone", command::ArgKind::Text))
        .arg(command::Arg::optional("timeout", command::ArgKind::Text)),
    )?;

    registry.bind("browser.scroll", |invocation, _| {
        drive_browser(invocation, scroll_action(invocation)?)
    })?;

    registry.bind("browser.wait", |invocation, _| {
        drive_browser(invocation, wait_action(invocation)?)
    })?;

    registry.register(
        browser(
            "browser.record.start",
            "Start recording the worktree's browser tab to a video",
        )
        .arg(command::Arg::optional("max", command::ArgKind::Text)),
    )?;

    registry.register(browser(
        "browser.record.stop",
        "Stop recording the worktree's browser tab, and print the video and its stills",
    ))?;

    registry.bind("browser.record.start", |invocation, _| {
        drive_browser(invocation, record_action(invocation)?)
    })?;

    registry.bind("browser.record.stop", |invocation, _| {
        drive_browser(invocation, BrowserAction::RecordStop)
    })?;

    registry.register(
        browser(
            "browser.dialog.accept",
            "Press OK on the JavaScript dialog in the worktree's browser tab",
        )
        .arg(command::Arg::optional("text", command::ArgKind::Text)),
    )?;

    registry.register(browser(
        "browser.dialog.dismiss",
        "Press Cancel on the JavaScript dialog in the worktree's browser tab",
    ))?;

    registry.bind("browser.dialog.accept", |invocation, _| {
        let text = invocation.text_of("text").map(str::to_owned);
        drive_browser(invocation, BrowserAction::Dialog { accept: true, text })
    })?;

    registry.bind("browser.dialog.dismiss", |invocation, _| {
        drive_browser(
            invocation,
            BrowserAction::Dialog {
                accept: false,
                text: None,
            },
        )
    })?;

    Ok(())
}

impl BrowserCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    /// The worktree is the caller's to add.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            BrowserCmd::Open { url } => Invocation::new("browser.open").maybe_text("url", url),
            BrowserCmd::Tabs => Invocation::new("browser.tabs"),
            BrowserCmd::Back => Invocation::new("browser.back"),
            BrowserCmd::Forward => Invocation::new("browser.forward"),
            BrowserCmd::Reload => Invocation::new("browser.reload"),
            BrowserCmd::Screenshot { full_page } => {
                Invocation::new("browser.screenshot").flag("full_page", full_page)
            }
            BrowserCmd::Console {
                errors,
                since,
                lines,
            } => Invocation::new("browser.console")
                .flag("errors", errors)
                .maybe_text("since", since)
                .text("lines", lines.to_string()),
            BrowserCmd::Eval { expression } => {
                Invocation::new("browser.eval").text("expression", expression)
            }
            BrowserCmd::Snapshot { interactive, max } => Invocation::new("browser.snapshot")
                .flag("interactive", interactive)
                .text("max", max.to_string()),
            BrowserCmd::Click {
                target,
                double,
                right,
            } => Invocation::new("browser.click")
                .text("target", target)
                .flag("double", double)
                .flag("right", right),
            BrowserCmd::Fill { target, text } => Invocation::new("browser.fill")
                .text("target", target)
                .text("text", text),
            BrowserCmd::Type { text, keys } => Invocation::new("browser.type")
                .text("text", text)
                .flag("keys", keys),
            BrowserCmd::Press { key } => Invocation::new("browser.press").text("key", key),
            BrowserCmd::Scroll {
                target,
                down,
                up,
                to_bottom,
                to_top,
            } => {
                let (to, by) = match (down, up) {
                    (_, Some(by)) => ("up", by),
                    _ if to_bottom => ("bottom", None),
                    _ if to_top => ("top", None),
                    (by, None) => ("down", by.flatten()),
                };
                Invocation::new("browser.scroll")
                    .maybe_text("target", target)
                    .text("to", to)
                    .maybe_text("by", by.map(|px| px.to_string()))
            }
            BrowserCmd::Wait {
                target,
                text,
                url,
                load,
                gone,
                timeout,
            } => Invocation::new("browser.wait")
                .maybe_text("target", target)
                .maybe_text("text", text)
                .maybe_text("url", url)
                .flag("load", load)
                .maybe_text("gone", gone)
                .text("timeout", timeout),
            BrowserCmd::Record {
                cmd: RecordCmd::Start { max },
            } => Invocation::new("browser.record.start").text("max", max),
            BrowserCmd::Record {
                cmd: RecordCmd::Stop,
            } => Invocation::new("browser.record.stop"),
            BrowserCmd::Dialog {
                cmd: DialogCmd::Accept { text },
            } => Invocation::new("browser.dialog.accept").maybe_text("text", text),
            BrowserCmd::Dialog {
                cmd: DialogCmd::Dismiss,
            } => Invocation::new("browser.dialog.dismiss"),
        }
    }
}

#[derive(Subcommand, Debug)]
pub(super) enum BrowserCmd {
    /// Load a page and wait for it, opening a tab if the worktree has none.
    ///
    /// Prints the page's address and title once it has loaded, or after ten
    /// seconds if it is still loading. With no URL, only says what the tab
    /// shows.
    Open {
        /// The address. `localhost:3000` means `http://localhost:3000`.
        url: Option<String>,
    },
    /// List the worktree's browser tabs; `*` marks the one ket acts on.
    Tabs,
    /// Go back a page, and wait for it.
    Back,
    /// Go forward a page, and wait for it.
    Forward,
    /// Load the page again, and wait for it.
    Reload,
    /// Save a PNG of the page and print its path.
    Screenshot {
        /// The whole page, not just what fits in the tab: up to 16,384 pixels
        /// tall.
        #[arg(long)]
        full_page: bool,
    },
    /// Print what the page logged to its console, oldest first.
    ///
    /// One message a line, `HH:MM:SS level  text (source:line)`; the same
    /// message several times in a row is printed once, with `×N`.
    Console {
        /// Only errors.
        #[arg(long)]
        errors: bool,
        /// Only what was logged this recently: `30s`, `5m`, `2h`.
        #[arg(long, value_name = "AGE")]
        since: Option<String>,
        /// At most this many of the latest messages.
        #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u32).range(1..))]
        lines: u32,
    },
    /// Run JavaScript in the page and print what it comes to.
    ///
    /// A promise is awaited, and `await` works at the top level. A string
    /// prints as it is, anything else as JSON; a throw is an error. Past 4 KB
    /// the value is cut, with a note on stderr.
    Eval {
        /// The JavaScript. `-` reads it from stdin.
        expression: String,
    },
    /// Print the page's accessibility tree, a node a line, with refs to act
    /// on.
    ///
    /// The page's address and title, then `- role "name" [@eN] states`,
    /// indented by nesting. Pass a ref — `@e6` — to `click`, `fill`,
    /// `scroll` or `wait`. A node keeps its ref from one snapshot to the
    /// next until the page loads another document; then take a new one.
    ///
    /// A frame from the page's own site shows under its `Iframe` line, refs
    /// and all. One from another site runs where ket can't reach yet: its
    /// line says `cross-site=<host>`, with nothing under it. Clicking it
    /// lands in it, and `type` then types into what that focused. A CSS
    /// selector looks in the page itself, not in its frames.
    Snapshot {
        /// Only what can be acted on: buttons, links, fields, checkboxes,
        /// tabs, menu items.
        #[arg(long, short)]
        interactive: bool,
        /// At most this many lines; past it, a note on stderr says how many
        /// were cut.
        #[arg(long, default_value_t = 400, value_parser = clap::value_parser!(u32).range(1..=10_000))]
        max: u32,
    },
    /// Click an element with the mouse, as a person would.
    ///
    /// Scrolls it into view and clicks its centre with real mouse events,
    /// unlike `eval`'s `.click()`. Refuses when something else covers it,
    /// and says what. Prints the page's address and title afterwards,
    /// waiting for a page the click loads.
    Click {
        /// A ref from `snapshot`, `@e6`, or a CSS selector, which looks in
        /// the page, not its frames.
        target: String,
        /// Double-click.
        #[arg(long)]
        double: bool,
        /// Click with the right button.
        #[arg(long, conflicts_with = "double")]
        right: bool,
    },
    /// Put text in a field in place of what it holds, as typing would.
    ///
    /// For a `<select>`, picks the option with that text or value. A
    /// checkbox or radio button is clicked instead.
    Fill {
        /// A ref from `snapshot`, `@e6`, or a CSS selector, which looks in
        /// the page, not its frames.
        target: String,
        /// What the field is to hold. `-` reads it from stdin.
        text: String,
    },
    /// Type text into whatever on the page has focus.
    Type {
        /// What to type. `-` reads it from stdin.
        text: String,
        /// A key down and up for each character, for pages that listen to
        /// keys rather than to input.
        #[arg(long)]
        keys: bool,
    },
    /// Press a key: `Enter`, `Tab`, `Escape`, `Backspace`, `ArrowDown`,
    /// `Meta+A`, `Shift+Tab`.
    ///
    /// Modifiers are `Alt`, `Control`, `Meta` (⌘) and `Shift`, joined with
    /// `+`. Prints the page's address and title afterwards, waiting for a
    /// page the key loads.
    Press {
        /// The key, with any modifiers.
        key: String,
    },
    /// Scroll the page, or an element in it, with the mouse wheel: one
    /// screen down unless told otherwise.
    Scroll {
        /// A ref from `snapshot` or a CSS selector, to scroll inside it
        /// rather than the page. A selector looks in the page, not its
        /// frames.
        target: Option<String>,
        /// Down, this many pixels; one screen with no number.
        #[arg(long, value_name = "PX", num_args = 0..=1, group = "way")]
        down: Option<Option<u32>>,
        /// Up, this many pixels; one screen with no number.
        #[arg(long, value_name = "PX", num_args = 0..=1, group = "way")]
        up: Option<Option<u32>>,
        /// All the way down.
        #[arg(long, group = "way")]
        to_bottom: bool,
        /// All the way up.
        #[arg(long, group = "way")]
        to_top: bool,
    },
    /// Wait until an element shows, text appears, the address changes, the
    /// page loads, or an element goes.
    ///
    /// Looks every tenth of a second. Prints the page's address and title
    /// once it is so, or says what never happened and exits non-zero.
    #[command(group(clap::ArgGroup::new("until").required(true).args(["target", "text", "url", "load", "gone"])))]
    Wait {
        /// A ref from `snapshot` or a CSS selector: wait for it to be on
        /// the page and visible. A selector looks in the page, not its
        /// frames.
        target: Option<String>,
        /// Wait for the page to show this text.
        #[arg(long)]
        text: Option<String>,
        /// Wait for the page's address to contain this.
        #[arg(long)]
        url: Option<String>,
        /// Wait for the page to finish loading.
        #[arg(long)]
        load: bool,
        /// Wait for this element to leave the page, or be hidden.
        #[arg(long, value_name = "TARGET")]
        gone: Option<String>,
        /// How long to wait: `500ms`, `5s`, at most `15s`.
        #[arg(long, value_name = "AGE", default_value = "10s")]
        timeout: String,
    },
    /// Record the page to a video, to show a person what happened.
    ///
    /// `start` begins; `stop` finishes the MP4 and prints its path, then a
    /// few stills from it as PNGs, a path a line, for an agent that reads
    /// images but not video, then how long it runs. A recording that reaches
    /// `--max` stops itself, and `stop` then prints what it made.
    Record {
        #[command(subcommand)]
        cmd: RecordCmd,
    },
    /// Answer the JavaScript dialog the page has open: an alert, a confirm, a
    /// prompt, or a beforeunload prompt asking whether to leave the page.
    ///
    /// The page is stopped until it is answered. A command that opens one
    /// says what it says, and so does every command until it is answered;
    /// `tabs` marks a tab waiting on one. Prints the page's address and title
    /// afterwards, waiting for a page the answer loads. The person sees it in
    /// the tab, and whichever of you answers first closes it for the other.
    Dialog {
        #[command(subcommand)]
        cmd: DialogCmd,
    },
}

#[derive(Subcommand, Debug)]
pub(super) enum DialogCmd {
    /// Press OK: for a beforeunload prompt, leave the page.
    Accept {
        /// What to answer a prompt with, in place of what the page filled
        /// in.
        #[arg(long)]
        text: Option<String>,
    },
    /// Press Cancel: for a beforeunload prompt, stay on the page.
    Dismiss,
}

#[derive(Subcommand, Debug)]
pub(super) enum RecordCmd {
    /// Start recording the page.
    Start {
        /// The longest the video may run: `30s`, `2m`, at most `10m`.
        #[arg(long, value_name = "AGE", default_value = "2m")]
        max: String,
    },
    /// Stop recording, and print the video's path, its stills' and its
    /// length.
    Stop,
}

/// Handles `ket browser ...`: asks the ket window, prints what the page did.
pub(super) fn drive_browser(
    invocation: &Invocation,
    action: BrowserAction,
) -> Result<(), KetError> {
    let worktree = invocation.require_text("worktree")?;
    let answer = ket_core::host::browser(worktree, action.clone())?;
    print!("{}", browser_text(&answer, &action));
    if let BrowserAnswer::Value {
        value,
        cut_from: Some(full),
    } = &answer
    {
        eprintln!(
            "(cut at {} of {}; return less to see the rest)",
            size(value.len() as u64),
            size(*full)
        );
    }
    if let BrowserAnswer::Snapshot { nodes, cut, .. } = &answer
        && *cut > 0
    {
        eprintln!("{}", snapshot_cut(nodes.len(), *cut, &action));
    }
    Ok(())
}

/// What the terminal is told when a snapshot was cut at `shown` lines,
/// `cut` short of all of them.
pub(super) fn snapshot_cut(shown: usize, cut: u32, asked: &BrowserAction) -> String {
    let all = shown + cut as usize;
    let fewer = match asked {
        BrowserAction::Snapshot {
            interactive: false, ..
        } => ", --interactive only what can be acted on",
        _ => "",
    };
    format!("(cut at {shown} of {all} lines; --max {all} shows them all{fewer})")
}

/// What `ket browser snapshot` asks for.
pub(super) fn snapshot_action(invocation: &Invocation) -> Result<BrowserAction, KetError> {
    let max = match invocation.text_of("max") {
        Some(max) => max
            .parse()
            .ok()
            .filter(|max| *max > 0)
            .ok_or_else(|| KetError::Conflict(format!("--max takes a count, not {max}")))?,
        None => 400,
    };
    Ok(BrowserAction::Snapshot {
        interactive: invocation.is_set("interactive"),
        max,
    })
}

/// A `ket browser` command's element: `@e6`, a ref from a snapshot, or a CSS
/// selector.
pub(super) fn browser_target(text: &str) -> Result<BrowserTarget, KetError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(KetError::Conflict(
            "say which element: a ref like @e6 from `ket browser snapshot`, or a CSS selector"
                .to_owned(),
        ));
    }
    let Some(number) = text.strip_prefix('@') else {
        return Ok(BrowserTarget::Selector(text.to_owned()));
    };
    number
        .strip_prefix('e')
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .map(BrowserTarget::Ref)
        .ok_or_else(|| {
            KetError::Conflict(format!(
                "{text} is not a ref; refs look like @e6, from `ket browser snapshot`"
            ))
        })
}

/// `text`, or stdin when it is `-`.
pub(super) fn text_or_stdin(text: &str) -> Result<String, KetError> {
    match text {
        "-" => std::io::read_to_string(std::io::stdin()).map_err(|e| KetError::io("stdin", e)),
        text => Ok(text.to_owned()),
    }
}

/// How long `ket browser wait` waits unless told otherwise.
pub(super) const WAIT_DEFAULT_MS: u64 = 10_000;

/// The longest `ket browser wait` waits: short of how long the window gives
/// any one request, so a wait that times out is told so rather than cut off.
pub(super) const WAIT_MOST_MS: u64 = 15_000;

/// What `ket browser scroll` asks for: one screen down unless told
/// otherwise.
pub(super) fn scroll_action(invocation: &Invocation) -> Result<BrowserAction, KetError> {
    let target = invocation
        .text_of("target")
        .map(browser_target)
        .transpose()?;
    let by = invocation
        .text_of("by")
        .map(|px| {
            px.parse()
                .map_err(|_| KetError::Conflict(format!("scroll by pixels, not {px}")))
        })
        .transpose()?;
    let by = match invocation.text_of("to").unwrap_or("down") {
        "down" => BrowserScroll::Down(by),
        "up" => BrowserScroll::Up(by),
        "bottom" => BrowserScroll::Bottom,
        "top" => BrowserScroll::Top,
        other => {
            return Err(KetError::Conflict(format!(
                "scroll down, up, to the bottom or to the top, not {other}"
            )));
        }
    };
    Ok(BrowserAction::Scroll { target, by })
}

/// What `ket browser wait` asks for, and for how long.
pub(super) fn wait_action(invocation: &Invocation) -> Result<BrowserAction, KetError> {
    let until = wait_until(invocation)?;
    let timeout_ms = match invocation.text_of("timeout") {
        Some(timeout) => age_ms(timeout, "--timeout")?,
        None => WAIT_DEFAULT_MS,
    };
    if timeout_ms > WAIT_MOST_MS {
        return Err(KetError::Conflict(format!(
            "--timeout is at most {}; wait again to wait longer",
            age(WAIT_MOST_MS)
        )));
    }
    Ok(BrowserAction::Wait { until, timeout_ms })
}

/// How long `ket browser record` records unless told otherwise.
pub(super) const RECORD_DEFAULT_MS: u64 = 120_000;

/// The longest `ket browser record` records.
pub(super) const RECORD_MOST_MS: u64 = 600_000;

/// What `ket browser record start` asks for: a recording of at most `--max`.
pub(super) fn record_action(invocation: &Invocation) -> Result<BrowserAction, KetError> {
    let max_ms = match invocation.text_of("max") {
        Some(max) => age_ms(max, "--max")?,
        None => RECORD_DEFAULT_MS,
    };
    if max_ms > RECORD_MOST_MS {
        return Err(KetError::Conflict(format!(
            "--max is at most {}; record again to record longer",
            age(RECORD_MOST_MS)
        )));
    }
    if max_ms < 1_000 {
        return Err(KetError::Conflict("--max is at least 1s".to_owned()));
    }
    let max_seconds = u32::try_from(max_ms.div_ceil(1_000)).unwrap_or(u32::MAX);
    Ok(BrowserAction::RecordStart { max_seconds })
}

/// What `ket browser wait` is to wait for: an element, `--text`, `--url`,
/// `--load` or `--gone`, of which clap lets through exactly one.
pub(super) fn wait_until(invocation: &Invocation) -> Result<BrowserWait, KetError> {
    if let Some(target) = invocation.text_of("target") {
        return Ok(BrowserWait::Shown(browser_target(target)?));
    }
    if let Some(gone) = invocation.text_of("gone") {
        return Ok(BrowserWait::Gone(browser_target(gone)?));
    }
    if let Some(text) = invocation.text_of("text").filter(|text| !text.is_empty()) {
        return Ok(BrowserWait::Text(text.to_owned()));
    }
    if let Some(url) = invocation.text_of("url").filter(|url| !url.is_empty()) {
        return Ok(BrowserWait::Url(url.to_owned()));
    }
    if invocation.is_set("load") {
        return Ok(BrowserWait::Load);
    }
    Err(KetError::Conflict(
        "say what to wait for: an element, --text, --url, --load or --gone".to_owned(),
    ))
}

/// What `ket browser` prints for an answer to `asked`: a line per page, `*`
/// on the tab it acts on in a list; a screenshot's path alone; the console a
/// message a line.
pub(super) fn browser_text(answer: &BrowserAnswer, asked: &BrowserAction) -> String {
    let page = |url: &str, title: &str| match title {
        "" => format!("{url}\n"),
        title => format!("{url}  \"{title}\"\n"),
    };
    match answer {
        BrowserAnswer::Done { url, title } => page(url, title),
        BrowserAnswer::Tabs { tabs } if tabs.is_empty() => {
            "no browser tabs in this worktree; `ket browser open <url>` opens one\n".to_owned()
        }
        BrowserAnswer::Tabs { tabs } => tabs
            .iter()
            .map(|tab| {
                let mark = if tab.active { '*' } else { ' ' };
                let line = page(&tab.url, &tab.title);
                match &tab.dialog {
                    Some(dialog) => format!("{mark} {}  (waiting on {dialog})\n", line.trim_end()),
                    None => format!("{mark} {line}"),
                }
            })
            .collect(),
        BrowserAnswer::Screenshot { path } => format!("{path}\n"),
        BrowserAnswer::Console { lines } => match asked {
            BrowserAction::Console {
                errors_only,
                since_ms,
                ..
            } => console_text(lines, *errors_only, *since_ms),
            _ => console_text(lines, false, None),
        },
        BrowserAnswer::Value { value, .. } => format!("{value}\n"),
        BrowserAnswer::Snapshot {
            url, title, nodes, ..
        } => {
            let mut text = page(url, title);
            if nodes.is_empty() {
                text.push_str(match asked {
                    BrowserAction::Snapshot {
                        interactive: true, ..
                    } => "nothing on the page can be clicked or typed in\n",
                    _ => "the page is empty\n",
                });
            }
            text.extend(nodes.iter().map(snapshot_line));
            text
        }
        BrowserAnswer::RecordingStarted => {
            "recording; ket browser record stop finishes it\n".to_owned()
        }
        BrowserAnswer::Recording {
            video,
            frames,
            seconds,
        } => {
            let mut text = format!("{video}\n");
            for frame in frames {
                text.push_str(frame);
                text.push('\n');
            }
            text.push_str(&format!("({seconds:.1}s)\n"));
            text
        }
    }
}

/// A snapshot's node as a line: `- role "name" [@eN] value="…" states`,
/// indented two spaces a level.
pub(super) fn snapshot_line(node: &SnapshotNode) -> String {
    let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    let mut line = format!("{}- {}", "  ".repeat(usize::from(node.depth)), node.role);
    if !node.name.is_empty() {
        line.push(' ');
        line.push_str(&quoted(&node.name));
    }
    if let Some(n) = node.r#ref {
        line.push_str(&format!(" [@e{n}]"));
    }
    if let Some(value) = &node.value {
        line.push_str(&format!(" value={}", quoted(value)));
    }
    for state in &node.states {
        line.push(' ');
        line.push_str(state);
    }
    line.push('\n');
    line
}

/// A page's console, a message a line — `HH:MM:SS level  text
/// (source:line)` — with the same message several times in a row printed
/// once, `×N`; or, when there is none, saying so.
pub(super) fn console_text(
    lines: &[ket_core::host::ConsoleLine],
    errors_only: bool,
    since_ms: Option<u64>,
) -> String {
    if lines.is_empty() {
        let none = if errors_only {
            "no errors"
        } else {
            "nothing logged"
        };
        return match since_ms {
            Some(ms) => format!("{none} in the last {}\n", age(ms)),
            None => format!("{none}\n"),
        };
    }
    lines
        .chunk_by(|a, b| {
            (&a.level, &a.text, &a.source, a.line) == (&b.level, &b.text, &b.source, b.line)
        })
        .map(|run| {
            let line = &run[0];
            let from = match (line.source.as_str(), line.line) {
                ("", _) => String::new(),
                (source, 0) => format!(" ({source})"),
                (source, at) => format!(" ({source}:{at})"),
            };
            let times = match run.len() {
                1 => String::new(),
                n => format!(" ×{n}"),
            };
            format!(
                "{} {:<7}  {}{from}{times}\n",
                line.time, line.level, line.text
            )
        })
        .collect()
}

/// An age for option `flag` — `--since` or `--timeout` — `30s`, `5m`, `2h`
/// or `1d`, in milliseconds. A bare number is seconds.
pub(super) fn age_ms(text: &str, flag: &str) -> Result<u64, KetError> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (count, unit) = text.split_at(split);
    let unit_ms = match unit.trim() {
        "ms" => 1,
        "" | "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => 0,
    };
    count
        .parse::<u64>()
        .ok()
        .filter(|_| unit_ms > 0)
        .map(|count| count.saturating_mul(unit_ms))
        .ok_or_else(|| {
            KetError::Conflict(format!(
                "{flag} takes an age like 30s, 5m or 2h, not {text}"
            ))
        })
}

/// An age in milliseconds the way `--since` takes one: `5m` for five minutes.
pub(super) fn age(ms: u64) -> String {
    match ms {
        ms if ms >= 86_400_000 && ms % 86_400_000 == 0 => format!("{}d", ms / 86_400_000),
        ms if ms >= 3_600_000 && ms % 3_600_000 == 0 => format!("{}h", ms / 3_600_000),
        ms if ms >= 60_000 && ms % 60_000 == 0 => format!("{}m", ms / 60_000),
        ms if ms % 1_000 == 0 => format!("{}s", ms / 1_000),
        ms => format!("{ms}ms"),
    }
}

/// A size in bytes, for a person: `512 bytes`, `4 KB`, `1.2 MB`.
pub(super) fn size(bytes: u64) -> String {
    match bytes {
        bytes if bytes < 1024 => format!("{bytes} bytes"),
        bytes if bytes < 1024 * 1024 => format!("{} KB", (bytes + 512) / 1024),
        bytes => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
    }
}

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "browser", "open"],
    &[
        "ket",
        "browser",
        "open",
        "localhost:3000",
        "--worktree",
        "wt-2",
    ],
    &["ket", "browser", "tabs"],
    &["ket", "browser", "back"],
    &["ket", "browser", "forward"],
    &["ket", "browser", "-w", "wt-2", "reload"],
    &["ket", "browser", "screenshot", "--full-page"],
    &[
        "ket", "browser", "console", "--errors", "--since", "5m", "--lines", "50",
    ],
    &["ket", "browser", "eval", "document.title"],
    &["ket", "browser", "snapshot"],
    &["ket", "browser", "snapshot", "-i", "--max", "50"],
    &["ket", "browser", "click", "@e6"],
    &["ket", "browser", "click", "#save", "--double"],
    &["ket", "browser", "click", "@e2", "--right"],
    &["ket", "browser", "fill", "@e3", "brad@example.com"],
    &["ket", "browser", "type", "hello", "--keys"],
    &["ket", "browser", "press", "Shift+Tab"],
    &["ket", "browser", "scroll"],
    &["ket", "browser", "scroll", "@e9", "--up", "300"],
    &["ket", "browser", "scroll", "--to-bottom"],
    &["ket", "browser", "wait", "#save"],
    &[
        "ket",
        "browser",
        "wait",
        "--text",
        "Saved",
        "--timeout",
        "5s",
    ],
    &["ket", "browser", "wait", "--url", "/dashboard"],
    &["ket", "browser", "wait", "--load"],
    &["ket", "browser", "wait", "--gone", ".spinner"],
    &["ket", "browser", "record", "start"],
    &["ket", "browser", "record", "start", "--max", "30s"],
    &["ket", "browser", "record", "stop", "-w", "wt-2"],
    &["ket", "browser", "dialog", "accept"],
    &["ket", "browser", "dialog", "accept", "--text", "Sam"],
    &["ket", "browser", "dialog", "dismiss", "-w", "wt-2"],
];

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    use crate::Cli;
    use crate::tests::invocation_for;

    #[test]
    fn browser_pages_print_one_a_line_with_the_acted_on_tab_marked() {
        let tab = |url: &str, title: &str, active| ket_core::host::BrowserTab {
            url: url.to_owned(),
            title: title.to_owned(),
            active,
            dialog: None,
        };
        assert_eq!(
            browser_text(
                &BrowserAnswer::Done {
                    url: "http://localhost:3000/".to_owned(),
                    title: "Sign in".to_owned(),
                },
                &BrowserAction::Tabs
            ),
            "http://localhost:3000/  \"Sign in\"\n"
        );
        assert_eq!(
            browser_text(
                &BrowserAnswer::Tabs {
                    tabs: vec![
                        tab("http://localhost:3000/", "Sign in", false),
                        tab("about:blank", "", true),
                    ],
                },
                &BrowserAction::Tabs
            ),
            "  http://localhost:3000/  \"Sign in\"\n* about:blank\n"
        );
        assert!(
            browser_text(
                &BrowserAnswer::Tabs { tabs: Vec::new() },
                &BrowserAction::Tabs
            )
            .starts_with("no browser tabs")
        );
    }

    fn logged(
        time: &str,
        level: &str,
        text: &str,
        source: &str,
        line: u32,
    ) -> ket_core::host::ConsoleLine {
        ket_core::host::ConsoleLine {
            at_ms: 0,
            time: time.to_owned(),
            level: level.to_owned(),
            text: text.to_owned(),
            source: source.to_owned(),
            line,
        }
    }

    #[test]
    fn the_console_prints_a_line_a_message_and_a_run_of_repeats_once() {
        let app = "http://localhost:3000/app.js";
        let thrown = "Uncaught TypeError: x is undefined";
        let lines = [
            logged("09:41:00", "info", "booted", app, 3),
            logged("09:41:01", "error", thrown, app, 12),
            logged("09:41:01", "error", thrown, app, 12),
            logged("09:41:02", "error", thrown, app, 12),
            logged("09:41:03", "warning", "slow", "", 0),
            logged("09:41:04", "error", thrown, app, 12),
            logged("09:41:05", "debug", "inline", "about:blank", 0),
        ];
        assert_eq!(
            console_text(&lines, false, None),
            "09:41:00 info     booted (http://localhost:3000/app.js:3)\n\
             09:41:01 error    Uncaught TypeError: x is undefined \
             (http://localhost:3000/app.js:12) ×3\n\
             09:41:03 warning  slow\n\
             09:41:04 error    Uncaught TypeError: x is undefined \
             (http://localhost:3000/app.js:12)\n\
             09:41:05 debug    inline (about:blank)\n"
        );
    }

    #[test]
    fn an_empty_console_says_what_was_looked_for() {
        assert_eq!(
            console_text(&[], true, Some(300_000)),
            "no errors in the last 5m\n"
        );
        assert_eq!(
            console_text(&[], false, Some(90_000)),
            "nothing logged in the last 90s\n"
        );
        assert_eq!(console_text(&[], false, None), "nothing logged\n");
    }

    #[test]
    fn since_takes_an_age_in_its_own_units() {
        assert_eq!(age_ms("5m", "--since").unwrap(), 300_000);
        assert_eq!(age_ms("30s", "--since").unwrap(), 30_000);
        assert_eq!(age_ms("45", "--since").unwrap(), 45_000);
        assert_eq!(age_ms("2h", "--since").unwrap(), 7_200_000);
        assert_eq!(age(age_ms("1d", "--since").unwrap()), "1d");
        assert!(age_ms("five minutes", "--since").is_err());
        assert!(age_ms("5y", "--since").is_err());
        assert_eq!(size(4096), "4 KB");
        assert_eq!(size(1_258_291), "1.2 MB");
    }

    fn node(depth: u16, role: &str, name: &str, at: Option<u32>) -> SnapshotNode {
        SnapshotNode {
            depth,
            role: role.to_owned(),
            name: name.to_owned(),
            r#ref: at,
            value: None,
            states: Vec::new(),
        }
    }

    #[test]
    fn a_snapshot_prints_the_page_then_a_node_a_line_indented() {
        let snapshot = |interactive| BrowserAction::Snapshot {
            interactive,
            max: 400,
        };
        let asked = |argv: &[&str]| snapshot_action(&invocation_for(argv)).unwrap();
        assert_eq!(asked(&["ket", "browser", "snapshot"]), snapshot(false));
        assert_eq!(
            asked(&["ket", "browser", "snapshot", "-i", "--max", "20"]),
            BrowserAction::Snapshot {
                interactive: true,
                max: 20
            }
        );
        let mut email = node(1, "textbox", "Email", Some(2));
        email.value = Some("brad@example.com".to_owned());
        email.states = vec!["focused".to_owned(), "required".to_owned()];
        let mut remember = node(1, "checkbox", "Remember me", Some(3));
        remember.states = vec!["checked".to_owned()];
        let answer = BrowserAnswer::Snapshot {
            url: "http://localhost:3000/login".to_owned(),
            title: "Sign in".to_owned(),
            nodes: vec![
                node(0, "form", "", Some(1)),
                email,
                remember,
                node(1, "button", "Say \"hi\"", Some(4)),
                node(2, "text", "after", None),
            ],
            cut: 0,
        };
        assert_eq!(
            browser_text(&answer, &snapshot(false)),
            "http://localhost:3000/login  \"Sign in\"\n\
             - form [@e1]\n  \
               - textbox \"Email\" [@e2] value=\"brad@example.com\" focused required\n  \
               - checkbox \"Remember me\" [@e3] checked\n  \
               - button \"Say \\\"hi\\\"\" [@e4]\n    \
                 - text \"after\"\n"
        );
        let empty = |interactive| {
            browser_text(
                &BrowserAnswer::Snapshot {
                    url: "about:blank".to_owned(),
                    title: String::new(),
                    nodes: Vec::new(),
                    cut: 0,
                },
                &snapshot(interactive),
            )
        };
        assert_eq!(empty(false), "about:blank\nthe page is empty\n");
        assert_eq!(
            empty(true),
            "about:blank\nnothing on the page can be clicked or typed in\n"
        );
        assert_eq!(
            snapshot_cut(400, 813, &snapshot(false)),
            "(cut at 400 of 1213 lines; --max 1213 shows them all, --interactive only what \
             can be acted on)"
        );
        assert_eq!(
            snapshot_cut(10, 2, &snapshot(true)),
            "(cut at 10 of 12 lines; --max 12 shows them all)"
        );
    }

    #[test]
    fn a_target_is_a_ref_or_else_a_selector() {
        assert_eq!(browser_target("@e6").unwrap(), BrowserTarget::Ref(6));
        assert_eq!(
            browser_target(" #save ").unwrap(),
            BrowserTarget::Selector("#save".to_owned())
        );
        assert_eq!(
            browser_target("button[type=submit]").unwrap(),
            BrowserTarget::Selector("button[type=submit]".to_owned())
        );
        for bad in ["@6", "@e", "@e0", "@ex", "@e6x"] {
            let why = browser_target(bad).unwrap_err().to_string();
            assert!(why.contains("is not a ref"), "{bad}: {why}");
        }
        assert!(browser_target("  ").is_err());
        let click = |extra: &[&str]| {
            Cli::try_parse_from([&["ket", "browser", "click", "#a"], extra].concat())
        };
        assert!(click(&["--double"]).is_ok());
        assert!(click(&["--double", "--right"]).is_err(), "one or the other");
    }

    #[test]
    fn scroll_and_wait_options_come_through_as_the_action_they_mean() {
        let scroll = |argv: &[&str]| match scroll_action(&invocation_for(argv)).unwrap() {
            BrowserAction::Scroll { target, by } => (target, by),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            scroll(&["ket", "browser", "scroll"]),
            (None, BrowserScroll::Down(None))
        );
        assert_eq!(
            scroll(&["ket", "browser", "scroll", "--down"]),
            (None, BrowserScroll::Down(None))
        );
        assert_eq!(
            scroll(&["ket", "browser", "scroll", "@e4", "--up", "250"]),
            (Some(BrowserTarget::Ref(4)), BrowserScroll::Up(Some(250)))
        );
        assert_eq!(
            scroll(&["ket", "browser", "scroll", "--to-top"]),
            (None, BrowserScroll::Top)
        );
        assert!(
            Cli::try_parse_from(["ket", "browser", "scroll", "--up", "--to-bottom"]).is_err(),
            "one way at a time"
        );
        let wait = |argv: &[&str]| wait_action(&invocation_for(argv));
        let until = |argv: &[&str]| match wait(argv).unwrap() {
            BrowserAction::Wait { until, timeout_ms } => (until, timeout_ms),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            until(&["ket", "browser", "wait", "@e2"]),
            (BrowserWait::Shown(BrowserTarget::Ref(2)), 10_000)
        );
        let gone = [
            "ket",
            "browser",
            "wait",
            "--gone",
            ".spinner",
            "--timeout",
            "2s",
        ];
        assert_eq!(
            until(&gone),
            (
                BrowserWait::Gone(BrowserTarget::Selector(".spinner".to_owned())),
                2_000
            )
        );
        let url = [
            "ket",
            "browser",
            "wait",
            "--url",
            "/done",
            "--timeout",
            "500ms",
        ];
        assert_eq!(until(&url), (BrowserWait::Url("/done".to_owned()), 500));
        assert_eq!(
            until(&["ket", "browser", "wait", "--load"]),
            (BrowserWait::Load, 10_000)
        );
        let too_long = wait(&["ket", "browser", "wait", "--load", "--timeout", "1m"]);
        assert!(too_long.unwrap_err().to_string().contains("at most 15s"));
        // Exactly one thing to wait for.
        assert!(Cli::try_parse_from(["ket", "browser", "wait"]).is_err());
        assert!(Cli::try_parse_from(["ket", "browser", "wait", "#a", "--load"]).is_err());
    }

    #[test]
    fn a_recording_runs_two_minutes_unless_told_and_prints_its_files_a_line_each() {
        let record = |argv: &[&str]| record_action(&invocation_for(argv));
        assert_eq!(
            record(&["ket", "browser", "record", "start"]).unwrap(),
            BrowserAction::RecordStart { max_seconds: 120 }
        );
        assert_eq!(
            record(&["ket", "browser", "record", "start", "--max", "1500ms"]).unwrap(),
            BrowserAction::RecordStart { max_seconds: 2 }
        );
        let too_long = record(&["ket", "browser", "record", "start", "--max", "11m"]);
        assert!(too_long.unwrap_err().to_string().contains("at most 10m"));
        assert!(record(&["ket", "browser", "record", "start", "--max", "0s"]).is_err());
        assert_eq!(
            invocation_for(&["ket", "browser", "record", "stop"])
                .id
                .as_str(),
            "browser.record.stop"
        );

        let stop = BrowserAction::RecordStop;
        assert_eq!(
            browser_text(&BrowserAnswer::RecordingStarted, &stop),
            "recording; ket browser record stop finishes it\n"
        );
        let made = BrowserAnswer::Recording {
            video: "/c/recordings/recording-1-0.mp4".to_owned(),
            frames: vec![
                "/c/recordings/recording-1-0-1.png".to_owned(),
                "/c/recordings/recording-1-0-2.png".to_owned(),
            ],
            seconds: 12.34,
        };
        assert_eq!(
            browser_text(&made, &stop),
            "/c/recordings/recording-1-0.mp4\n\
             /c/recordings/recording-1-0-1.png\n\
             /c/recordings/recording-1-0-2.png\n\
             (12.3s)\n"
        );
    }

    #[test]
    fn a_dialog_is_answered_ok_or_cancel_and_a_tab_waiting_on_one_says_so() {
        let asked = |argv: &[&str]| {
            let invocation = invocation_for(argv);
            (
                invocation.id.as_str().to_owned(),
                invocation.text_of("text").map(str::to_owned),
            )
        };
        assert_eq!(
            asked(&["ket", "browser", "dialog", "accept", "--text", "Sam"]),
            ("browser.dialog.accept".to_owned(), Some("Sam".to_owned()))
        );
        assert_eq!(
            asked(&["ket", "browser", "dialog", "dismiss"]),
            ("browser.dialog.dismiss".to_owned(), None)
        );
        assert!(
            Cli::try_parse_from(["ket", "browser", "dialog", "dismiss", "--text", "x"]).is_err()
        );
        assert!(Cli::try_parse_from(["ket", "browser", "dialog"]).is_err());
        let tabs = BrowserAnswer::Tabs {
            tabs: vec![
                ket_core::host::BrowserTab {
                    url: "http://localhost:3000/files".to_owned(),
                    title: "Files".to_owned(),
                    active: true,
                    dialog: Some("a confirm: \"Delete 3 files?\"".to_owned()),
                },
                ket_core::host::BrowserTab {
                    url: "about:blank".to_owned(),
                    title: String::new(),
                    active: false,
                    dialog: Some("an alert".to_owned()),
                },
            ],
        };
        assert_eq!(
            browser_text(&tabs, &BrowserAction::Tabs),
            "* http://localhost:3000/files  \"Files\"  (waiting on a confirm: \"Delete 3 \
             files?\")\n  about:blank  (waiting on an alert)\n"
        );
    }
}
