//! Terminals, driven through real ptys by real programs.
//!
//! Every test here opens an actual pseudo-terminal and runs an actual process in
//! it. Nothing is faked: the pacing, the backpressure and the scrollback cap are
//! only interesting under a program that is genuinely faster than the screen,
//! and a mock is exactly the thing that would not be.
//!
//! The programs are `sh` and `awk`, which POSIX requires, rather than the tools
//! whose behaviour is being modelled. In particular the full-screen case does
//! **not** run `vim` or `htop`: neither is guaranteed present, both need a
//! terminfo database to agree with, and a test that skips itself on half the
//! machines it runs on is not a test. What it drives instead is the sequences
//! such a program actually emits — enter the alternate screen, address the
//! cursor, draw, leave — through a real pty, which is the part ket has to get
//! right. The escape sequences are written out rather than produced by an editor
//! nobody has installed.
//!
//! In-process terminals, [`LocalTerminal`], never `Terminal::open`: that one
//! goes to the ket host for the default data directory — the owner's own,
//! running their real terminals — and these tests would open theirs there.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::Sandbox;
use ket_core::terminal::alacritty_terminal::grid::Dimensions;
use ket_core::terminal::alacritty_terminal::index::{Column, Line};
use ket_core::terminal::alacritty_terminal::term::TermMode;
use ket_core::terminal::{FRAME_INTERVAL, LocalTerminal, TerminalSize, TerminalSpec};

/// A terminal running `script` under `sh`, sized for a small pane.
fn shell(cwd: &Path, script: &str) -> TerminalSpec {
    TerminalSpec {
        env_remove: Vec::new(),
        command: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
    }
}

/// Polls `ready` until it holds or `limit` elapses.
///
/// A pty is asynchronous by nature — the kernel schedules the program, not the
/// test — so every assertion about output is an assertion about output *within
/// a bound*. The bounds here are generous by a large factor, which is what keeps
/// this from becoming a flake generator on a loaded machine.
fn wait_until(limit: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    ready()
}

/// The visible screen as text, trailing blanks trimmed.
fn screen(terminal: &LocalTerminal) -> String {
    terminal.with_term(|term| {
        let grid = term.grid();
        (0..grid.screen_lines())
            .map(|row| {
                let line = Line(i32::try_from(row).expect("screen fits in i32"));
                (0..grid.columns())
                    .map(|col| grid[line][Column(col)].c)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Waits for `needle` to appear on the visible screen.
fn wait_for_text(terminal: &LocalTerminal, needle: &str) -> bool {
    wait_until(Duration::from_secs(10), || {
        screen(terminal).contains(needle)
    })
}

#[test]
fn what_the_program_printed_ends_up_in_the_grid() {
    let sandbox = Sandbox::new("terminal-basic");
    let terminal = LocalTerminal::open(&shell(sandbox.root(), "printf 'hello from the pty\\n'"))
        .expect("open terminal");

    assert!(
        wait_for_text(&terminal, "hello from the pty"),
        "{}",
        screen(&terminal)
    );
}

#[test]
fn sixteen_terminals_opening_at_the_same_instant_all_get_a_pty() {
    // Found by these tests running in parallel, and it is not a test artefact:
    // `openpty(3)` is not thread safe on macOS, and restoring a project with
    // several worktrees opens several ptys at once. The failure is intermittent
    // and reports an errno that decodes to nothing, so without opening them
    // genuinely simultaneously it looks like bad luck rather than a bug.
    let sandbox = Sandbox::new("terminal-parallel");
    let cwd = sandbox.root();
    std::thread::scope(|scope| {
        for pane in 0..16 {
            scope.spawn(move || {
                let terminal =
                    LocalTerminal::open(&shell(cwd, &format!("printf 'pane {pane}\\n'")))
                        .expect("open terminal");
                assert!(
                    wait_for_text(&terminal, &format!("pane {pane}")),
                    "{}",
                    screen(&terminal)
                );
            });
        }
    });
}

#[test]
fn typing_reaches_the_program_and_its_answer_reaches_the_grid() {
    let sandbox = Sandbox::new("terminal-input");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "read line; printf 'answering: %s\\n' \"$line\"",
    ))
    .expect("open terminal");

    terminal.send_input("ping\n").expect("send input");

    assert!(
        wait_for_text(&terminal, "answering: ping"),
        "{}",
        screen(&terminal)
    );
}

#[test]
fn a_single_line_of_several_megabytes_stays_inside_the_scrollback_cap() {
    // The shape that defeats every "cap the number of lines" scheme written
    // against newlines: four megabytes and not one line break in it. It becomes
    // fifty thousand wrapped rows, and the only thing standing between that and
    // the memory budget is the cap the grid was built with.
    let sandbox = Sandbox::new("terminal-longline");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "awk 'BEGIN { s = \"\"; for (i = 0; i < 64; i++) s = s \"x\"; \
         for (i = 0; i < 65536; i++) printf \"%s\", s }'",
    ))
    .expect("open terminal");

    assert!(
        wait_until(Duration::from_secs(60), || terminal.is_closed()),
        "the program never finished"
    );

    let limit = terminal.scrollback_limit();
    let rows = usize::from(terminal.size().rows);
    let (history, total) =
        terminal.with_term(|term| (term.grid().history_size(), term.grid().total_lines()));

    assert!(limit > 0, "a pane this size should keep some history");
    assert!(
        history <= limit,
        "{history} lines of history, cap is {limit}"
    );
    assert!(
        total <= limit + rows,
        "{total} rows held for a {rows}-row screen with a {limit}-line cap"
    );
    // And the tail is what survived, not the head: a scrollback that evicts the
    // newest lines is a scrollback nobody wants.
    assert!(screen(&terminal).contains("xxxx"), "{}", screen(&terminal));
}

#[test]
fn a_full_screen_program_draws_without_ever_touching_the_scrollback() {
    // The memory failure a full-screen program would otherwise cause: `htop`
    // redrawing twice a second for an hour is seven thousand screens, and if any
    // of it reached the scrollback the pane would be measured in gigabytes. The
    // alternate screen exists so none of it does, and this pins that it works
    // through ket's pty rather than only in principle.
    let sandbox = Sandbox::new("terminal-altscreen");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "i=0; while [ $i -lt 200 ]; do echo \"history $i\"; i=$((i+1)); done; \
         printf '\\033[?1049h\\033[2J'; \
         i=0; while [ $i -lt 24 ]; do printf '\\033[%d;1Hpanel row %d' $((i+1)) $i; i=$((i+1)); done; \
         read go; \
         printf '\\033[?1049l'; printf 'back on the primary screen\\n'",
    ))
    .expect("open terminal");

    assert!(
        wait_for_text(&terminal, "panel row 23"),
        "{}",
        screen(&terminal)
    );

    let (alt, history) = terminal.with_term(|term| {
        (
            term.mode().contains(TermMode::ALT_SCREEN),
            term.grid().history_size(),
        )
    });
    assert!(alt, "the program should be on the alternate screen");
    assert_eq!(history, 0, "the alternate screen grew a scrollback");

    // Leaving it puts the shell's own output back, untouched.
    terminal.send_input("\n").expect("send input");
    assert!(
        wait_for_text(&terminal, "back on the primary screen"),
        "{}",
        screen(&terminal)
    );

    let (alt, history) = terminal.with_term(|term| {
        (
            term.mode().contains(TermMode::ALT_SCREEN),
            term.grid().history_size(),
        )
    });
    assert!(!alt, "the alternate screen was never left");
    assert!(history > 100, "the primary scrollback lost {history} lines");
}

#[test]
fn rapid_repeated_resize_leaves_the_kernel_and_the_grid_agreeing() {
    // A window drag reports a new size several times per frame, and every one of
    // them reflows a grid and re-derives a scrollback budget. What must hold at
    // the end is that the kernel's winsize and the grid describe the same
    // screen: a program reads its width from the former and paints into the
    // latter, and a disagreement is a permanently wrapped display.
    let sandbox = Sandbox::new("terminal-resize");
    let terminal = LocalTerminal::open(&shell(sandbox.root(), "cat")).expect("open terminal");

    for step in 0..300u16 {
        let cols = 40 + (step % 120);
        let rows = 10 + (step % 40);
        terminal
            .resize(TerminalSize::new(cols, rows))
            .expect("resize");
    }

    let settled = TerminalSize::new(97, 31);
    terminal.resize(settled).expect("resize");

    assert_eq!(terminal.size(), settled);
    assert_eq!(terminal.pty_size().expect("read pty size"), settled);
    terminal.with_term(|term| {
        assert_eq!(term.columns(), usize::from(settled.cols));
        assert_eq!(term.screen_lines(), usize::from(settled.rows));
    });
    assert_eq!(terminal.scrollback_limit(), {
        use ket_core::terminal::scrollback_limit;
        scrollback_limit(settled)
    });

    // And the terminal still works, which a wedged resize path would not.
    terminal.send_input("still here\n").expect("send input");
    assert!(
        wait_for_text(&terminal, "still here"),
        "{}",
        screen(&terminal)
    );
}

#[test]
fn resizing_to_nothing_and_back_does_not_take_the_grid_with_it() {
    // A pane collapsed to zero is an ordinary drag, and `columns() - 1` on an
    // empty grid panics inside the emulator.
    let sandbox = Sandbox::new("terminal-collapse");
    let terminal = LocalTerminal::open(&shell(sandbox.root(), "cat")).expect("open terminal");

    for _ in 0..20 {
        terminal.resize(TerminalSize::new(0, 0)).expect("collapse");
        terminal.resize(TerminalSize::new(80, 24)).expect("restore");
    }

    terminal.send_input("survived\n").expect("send input");
    assert!(
        wait_for_text(&terminal, "survived"),
        "{}",
        screen(&terminal)
    );
}

#[test]
fn a_program_that_exits_leaves_the_grid_readable_and_the_handle_usable() {
    // The pane outlives the process. Somebody wants to read what the build said
    // after it finished, and every operation on a closed terminal has to answer
    // rather than hang — invariant 4 does not stop applying at EOF.
    let sandbox = Sandbox::new("terminal-eof");
    let terminal = LocalTerminal::open(&shell(sandbox.root(), "printf 'and then it exited\\n'"))
        .expect("open");

    assert!(
        wait_until(Duration::from_secs(10), || terminal.is_closed()),
        "the terminal never noticed EOF"
    );
    assert!(
        wait_for_text(&terminal, "and then it exited"),
        "{}",
        screen(&terminal)
    );
    assert!(
        wait_until(Duration::from_secs(10), || terminal
            .exit_status()
            .is_some_and(|s| s.success())),
        "no exit status was reaped"
    );

    // Everything below has to return. None of it has to succeed.
    let started = Instant::now();
    terminal.resize(TerminalSize::new(100, 30)).expect("resize");
    let _ = terminal.send_input("nobody is reading this");
    terminal.kill();
    terminal.kill();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a closed terminal blocked for {:?}",
        started.elapsed()
    );

    assert!(screen(&terminal).contains("and then it exited"));
}

#[test]
fn a_stalled_renderer_gets_backpressure_and_the_terminal_stays_killable() {
    // The consumer falling behind is the normal case, not the exotic one: a
    // renderer misses a frame every time the window is doing something else. The
    // queue between the pty and the grid is four reads deep, so what happens is
    // that the reader blocks, then the pty buffer fills, then the program
    // blocks. Nothing grows. And killing must not want the lock the stalled
    // renderer is holding, or closing a busy pane would wait for the frame that
    // is late.
    let sandbox = Sandbox::new("terminal-backpressure");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "awk 'BEGIN { for (i = 0; i < 400000; i++) print i, \"filler filler filler\" }'",
    ))
    .expect("open terminal");
    let terminal = &terminal;

    std::thread::scope(|scope| {
        scope.spawn(|| {
            terminal.with_term(|_| std::thread::sleep(Duration::from_millis(400)));
        });

        // Let the queue fill behind the held lock.
        std::thread::sleep(Duration::from_millis(100));

        let started = Instant::now();
        terminal.kill();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "kill waited {:?} on a stalled renderer",
            started.elapsed()
        );
    });

    assert!(
        wait_until(Duration::from_secs(10), || terminal.is_closed()),
        "the terminal never closed"
    );
}

#[test]
fn a_program_that_traps_hangup_is_still_killable() {
    // `ChildKiller` sends SIGHUP, and SIGHUP is trappable — a shell that ignores
    // it, or an agent with a cleanup handler that never returns, would leave a
    // pane that cannot be closed. The `sleep` in here is a second process in the
    // same group, which is the other half: SIGHUP to the direct child alone
    // leaves it holding the pty open and the pane never reaches EOF.
    //
    // The proof is the exit status, not the pane state. ket's own side stops the
    // moment it is asked to; only a reaped child says the process really died.
    let sandbox = Sandbox::new("terminal-trap");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "trap '' HUP; printf 'trapped and ready\\n'; while :; do sleep 1; done",
    ))
    .expect("open terminal");

    assert!(
        wait_for_text(&terminal, "trapped and ready"),
        "{}",
        screen(&terminal)
    );

    terminal.kill();

    assert!(
        wait_until(Duration::from_secs(15), || terminal.exit_status().is_some()),
        "a program that trapped SIGHUP outlived its terminal"
    );
    assert!(terminal.is_closed());
}

#[test]
fn a_firehose_repaints_once_a_frame_rather_than_once_a_chunk() {
    // The shell writes each line separately, so the pty hands over thousands of
    // small reads rather than a few large ones — which is what a chatty build
    // actually looks like, and the case where "repaint per chunk" would cost
    // thousands of repaints.
    //
    // The bound is the frame budget itself: however long this took, the pane may
    // not have painted more often than once per `FRAME_INTERVAL`. A few frames
    // of slack cover the first immediate paint, the final flush, and the
    // truncation in the division.
    let sandbox = Sandbox::new("terminal-firehose");
    let started = Instant::now();
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "i=0; while [ $i -lt 3000 ]; do echo \"line $i\"; i=$((i+1)); done",
    ))
    .expect("open terminal");

    assert!(
        wait_until(Duration::from_secs(60), || terminal.is_closed()),
        "the program never finished"
    );
    let elapsed = started.elapsed();
    assert!(
        wait_for_text(&terminal, "line 2999"),
        "{}",
        screen(&terminal)
    );

    let frames = terminal.frame();
    let budget = elapsed.as_millis() / FRAME_INTERVAL.as_millis() + 3;
    assert!(frames >= 1, "nothing was ever repainted");
    assert!(
        u128::from(frames) <= budget,
        "{frames} frames in {elapsed:?}, which allows {budget}"
    );
}

#[test]
fn dropping_a_terminal_returns_even_while_the_program_is_flooding_it() {
    // The last state that has to be escapable. Dropping a pane happens on the
    // thread that owns the window, and a drop that waits for a program to notice
    // it has been closed is a frozen window.
    let sandbox = Sandbox::new("terminal-drop");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "trap '' HUP; awk 'BEGIN { for (i = 0; i < 5000000; i++) print i }'",
    ))
    .expect("open terminal");

    assert!(wait_until(Duration::from_secs(10), || terminal.frame() > 0));

    let started = Instant::now();
    drop(terminal);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "drop took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_scrollback_that_has_wrapped_still_reads_as_one_run_of_lines() {
    // Six thousand numbered lines into a cap of about two thousand: the ring has
    // rotated four times over, and the slack alacritty allocated while it filled
    // has been handed back underneath it. Both are silent when they go wrong — a
    // scrollback that looks perfectly plausible and has a row missing or repeated
    // somewhere in the middle — so the whole retained history is read back and
    // checked for being one contiguous run ending at the newest line.
    let sandbox = Sandbox::new("terminal-ring");
    let terminal = LocalTerminal::open(&shell(
        sandbox.root(),
        "awk 'BEGIN { for (i = 0; i < 6000; i++) print \"line\", i }'",
    ))
    .expect("open terminal");

    assert!(
        wait_until(Duration::from_secs(60), || terminal.is_closed()),
        "the program never finished"
    );
    assert!(
        wait_for_text(&terminal, "line 5999"),
        "{}",
        screen(&terminal)
    );

    let numbers: Vec<u32> = terminal.with_term(|term| {
        let grid = term.grid();
        (grid.topmost_line().0..=grid.bottommost_line().0)
            .filter_map(|row| {
                let line = Line(row);
                let text: String = (0..grid.columns())
                    .map(|c| grid[line][Column(c)].c)
                    .collect();
                text.trim().strip_prefix("line ")?.trim().parse().ok()
            })
            .collect()
    });

    assert!(
        numbers.len() > 2000,
        "only {} lines survived a {}-line cap",
        numbers.len(),
        terminal.scrollback_limit()
    );
    assert_eq!(
        numbers.last().copied(),
        Some(5999),
        "the newest line was lost"
    );
    for pair in numbers.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "history jumps from {} to {}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn a_spec_can_strip_an_inherited_variable_before_the_program_sees_it() {
    // The mechanism `AgentSpec::env_remove` needs: agents guard against being
    // nested inside themselves by checking a marker variable, and ket is
    // regularly launched from inside an agent session. Without stripping,
    // every pane running that agent dies on startup.
    //
    // `HOME` stands in for the marker because it is always inherited and
    // setting one would need `unsafe`, which this workspace denies.
    let sandbox = Sandbox::new("terminal-env-remove");
    let mut spec = shell(sandbox.root(), "printf 'home=[%s]\\n' \"$HOME\"");
    spec.env_remove = vec!["HOME".to_owned()];

    let terminal = LocalTerminal::open(&spec).expect("open terminal");
    assert!(
        wait_for_text(&terminal, "home=[]"),
        "HOME reached the program: {}",
        screen(&terminal)
    );
}

#[test]
fn an_agent_spec_becomes_a_terminal_that_runs_it() {
    let spec = ket_core::config::AgentSpec {
        name: "example".to_owned(),
        transport: ket_core::config::Transport::Pty,
        command: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), "printf 'ran\\n'".to_owned()],
        env: Default::default(),
        env_remove: vec!["HOME".to_owned()],
        launch: None,
    };
    let sandbox = Sandbox::new("terminal-agent-spec");

    let built = TerminalSpec::agent(sandbox.root(), &spec);

    // The pane runs the user's login shell and types the agent's line at its
    // prompt, so that an alias or a function is as launchable as a file on
    // PATH — see `ket_core::shell`. Which arguments that takes depends on
    // which shell is running the tests, so what is pinned here is what holds
    // for every one of them: the shell itself, the directory, and the
    // stripping, since dropping that is what makes a session fail to start
    // rather than fail visibly.
    assert_eq!(built.command, ket_core::shell::login_shell());
    assert_eq!(built.env_remove, vec!["HOME".to_owned()]);
    assert_eq!(built.cwd, sandbox.root());

    // The end of it: whatever shape the launch took, the agent's own command
    // has to actually run.
    let terminal = LocalTerminal::open(&built).expect("open terminal");
    assert!(wait_for_text(&terminal, "ran"), "{}", screen(&terminal));
}

#[test]
fn agent_with_extra_words_types_a_longer_line_than_agent_alone() {
    // Resuming a session appends words to the launch line rather than to argv
    // — see `TerminalSpec::agent_with`'s own doc comment — so the built spec
    // for the same agent plus `extra` must carry a longer startup command
    // than the one built with none.
    let spec = ket_core::config::AgentSpec {
        name: "example".to_owned(),
        transport: ket_core::config::Transport::Pty,
        command: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), "true".to_owned()],
        env: Default::default(),
        env_remove: Vec::new(),
        launch: None,
    };
    let sandbox = Sandbox::new("terminal-agent-with-extra");

    let plain = TerminalSpec::agent(sandbox.root(), &spec);
    let resumed = TerminalSpec::agent_with(
        sandbox.root(),
        &spec,
        &["--resume".to_owned(), "abc123".to_owned()],
    );

    // The startup command types either as a zsh-only env var or as the last
    // shell argument — see `a_launch_line_survives_the_shell_that_types_it`
    // just below for the same split, pinned against explicit shells. Here the
    // real login shell is whichever the test machine runs, so both shapes are
    // accepted.
    let line_of = |built: &TerminalSpec| -> String {
        built
            .env
            .get(ket_core::shell::STARTUP_COMMAND_ENV)
            .cloned()
            .or_else(|| built.args.last().cloned())
            .expect("a startup command was typed somewhere")
    };

    assert_eq!(
        plain.command, resumed.command,
        "both type through the shell"
    );
    assert_eq!(plain.cwd, resumed.cwd);
    assert_ne!(
        line_of(&plain),
        line_of(&resumed),
        "the extra words must reach the line the shell types"
    );
    assert!(line_of(&resumed).contains("abc123"));
}

#[test]
fn a_launch_line_survives_the_shell_that_types_it() {
    // Against an explicit root and explicit shell names, so this pins the
    // launch contract rather than whatever `$SHELL` happens to be — and writes
    // its wrapper somewhere throwaway rather than into the owner's own tree.
    let sandbox = Sandbox::new("terminal-launch-line");
    let root = sandbox.path("wrappers");

    // Arguments are separate words nobody wrote shell syntax in, so they are
    // quoted; the command is shell text and goes through untouched, which is
    // what lets it be an alias.
    let line = ket_core::shell::line(
        "claude-personal",
        &["--model".to_owned(), "it's fine".to_owned()],
    );
    assert_eq!(line, r#"claude-personal --model 'it'\''s fine'"#);

    // zsh is handed the wrapper and told what to type. It is a login shell and
    // nothing more: the command must not appear in argv, or it would run before
    // the user's configuration had been read.
    let zsh = ket_core::shell::launch_in(Some(&root), "/bin/zsh", Some(&line));
    assert_eq!(zsh.args, vec!["-l".to_owned()]);
    assert_eq!(
        zsh.env.get(ket_core::shell::STARTUP_COMMAND_ENV),
        Some(&line)
    );
    assert!(root.join("zsh").join(".zshenv").is_file());

    // Anything else runs it directly. Still interactive: that is what expands
    // an alias and what reads the file a person's PATH is set in.
    let other = ket_core::shell::launch_in(Some(&root), "/bin/bash", Some(&line));
    assert_eq!(other.args, vec!["-i".to_owned(), "-c".to_owned(), line]);

    // No startup command is a plain shell, indistinguishable from the one a
    // terminal emulator would have opened.
    let plain = ket_core::shell::launch_in(Some(&root), "/bin/zsh", None);
    assert!(plain.args.is_empty());
    assert!(plain.env.is_empty());
}
