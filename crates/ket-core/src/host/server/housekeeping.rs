//! The host's two background loops: the second-by-second housekeeping —
//! terminals' foregrounds, reaping, sweeping old network logs, leaving when
//! idle, and starting automations when they come due — and passing agents'
//! hook reports on to the windows.

use super::*;

pub(in crate::host) mod automations;

/// Foregrounds, reaping, sweeping old network logs, and leaving when idle.
pub(super) fn housekeeping(host: &Arc<Host>, socket: &std::path::Path) {
    let mut swept: Option<Instant> = None;
    let mut automations = automations::Clock::new();
    loop {
        std::thread::sleep(TICK);
        let now = Instant::now();
        automations.tick(host);
        // On starting, then hourly: logs past their time go whether or not
        // the inspector is on now.
        if swept.is_none_or(|at| now.duration_since(at) >= NETWORK_SWEEP) {
            crate::network::store::sweep_expired();
            swept = Some(now);
        }
        // Changes are collected under the lock and sent after it: a window
        // that is slow to read must not hold up every other request.
        let mut changes: Vec<(u64, Option<Running>, Vec<Arc<Out>>)> = Vec::new();
        let mut exited: Vec<String> = Vec::new();
        {
            let mut terminals = lock(&host.terminals);
            for (&id, entry) in terminals.iter_mut() {
                entry.watchers.retain(Watcher::watching);
                if entry.terminal.is_closed() {
                    // The moment it is first seen gone, its agent's status
                    // goes with it: nothing is left running to have one.
                    if entry.closed_at.is_none()
                        && let Some(key) = &entry.key
                    {
                        exited.push(key.clone());
                    }
                    entry.closed_at.get_or_insert(now);
                    continue;
                }
                if entry.watchers.is_empty() {
                    continue;
                }
                let foreground = entry.terminal.foreground().map(Running::from);
                if foreground != entry.foreground {
                    entry.foreground = foreground.clone();
                    let outs = entry.watchers.iter().map(|w| w.out.clone()).collect();
                    changes.push((id, foreground, outs));
                }
            }
            terminals.retain(|_, entry| match entry.closed_at {
                Some(at) => !entry.killed && now.duration_since(at) < EXITED_KEPT,
                None => true,
            });
        }
        for (id, foreground, outs) in changes {
            for out in outs {
                out.reply(&Reply::Foreground {
                    id,
                    foreground: foreground.clone(),
                });
            }
        }
        if !exited.is_empty() {
            host.clear_panes(&exited);
        }
        // A Codex pane on a prompt may have a dialog drawn over it.
        if host.read_codex_dialogs() {
            host.broadcast_status();
        }

        let phones = host.phones().map_or(0, |phones| phones.connected());
        // An automation with a start to come keeps the host up, to start it
        // with every window closed.
        if host.live() > 0
            || host.clients.load(Ordering::Acquire) > 0
            || phones > 0
            || automations.keeps_alive()
        {
            *lock(&host.busy_at) = now;
        } else if now.duration_since(*lock(&host.busy_at)) >= IDLE_EXIT {
            tracing::info!("ket host idle; exiting");
            let _ = std::fs::remove_file(socket);
            std::process::exit(0);
        }
    }
}

/// Applies hook reports to the host's status store, and passes them and
/// status lines on to every window, or keeps them until one connects.
///
/// The store takes every report whether or not a window is listening: it is
/// the status, not a message for one. The raw reports still go to windows
/// for what is not status — usage and Codex rollouts.
pub(super) fn forward_hooks(host: &Host, listener: &Listener) {
    loop {
        std::thread::sleep(HOOK_TICK);
        let reports = listener.drain();
        let lines = listener.drain_statuslines();
        if !reports.is_empty() {
            let known = host.pane_keys();
            let (changed, ended) = {
                let mut status = lock(&host.status);
                let changed = reports.iter().fold(false, |changed, report| {
                    status.report(report, |pane| known.contains(pane)) | changed
                });
                (changed, status.take_ended())
            };
            if changed {
                host.broadcast_status();
            }
            host.limits.heard(ended, &lines);
        } else if !lines.is_empty() {
            host.limits.heard(Vec::new(), &lines);
        }
        // Every tick, reports or not: a held hook whose script has gone —
        // its session ended, or its prompt was dismissed — is let go of as
        // well as one answered at the desktop. A dismissal moves a row.
        if host.release_prompts() {
            host.broadcast_status();
        }
        if reports.is_empty() && lines.is_empty() {
            continue;
        }
        pass_on_hooks(host, reports, lines);
    }
}

/// Sends hook traffic to every window, or keeps it for the next one when
/// none is listening. Windows only: the CLI says hello too but skips hook
/// traffic to get to its answer, so a `ket host devices` that happened to be
/// connected would have taken reports a window wanted for usage.
pub(super) fn pass_on_hooks(host: &Host, reports: Vec<HookReport>, lines: Vec<StatusLine>) {
    // Whether a window is there, and keeping these when none is, under the
    // backlog's lock: a window greeted between the two would take the
    // backlog before these reached it, and they would wait for the next one.
    let mut backlog = lock(&host.backlog);
    let outs = windows(&host.outs);
    if outs.is_empty() {
        backlog.0.extend(reports);
        backlog.1.extend(lines);
        let excess = backlog.0.len().saturating_sub(BACKLOG_REPORTS);
        backlog.0.drain(..excess);
        let excess = backlog.1.len().saturating_sub(BACKLOG_LINES);
        backlog.1.drain(..excess);
        return;
    }
    drop(backlog);
    for out in &outs {
        send_hooks(out, &reports, &lines);
    }
}

/// Sends hook reports and status lines to one window.
pub(super) fn send_hooks(out: &Out, reports: &[HookReport], lines: &[StatusLine]) {
    for report in reports {
        out.reply(&Reply::Hook {
            report: Box::new(report.clone()),
        });
    }
    for line in lines {
        out.reply(&Reply::StatusLine {
            line: Box::new(line.clone()),
        });
    }
}
