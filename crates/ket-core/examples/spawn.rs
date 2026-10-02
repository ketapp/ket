//! Tries to open a terminal the way the shell does, and prints what happens.

fn main() {
    let dir = std::env::args().nth(1).expect("usage: spawn <directory>");
    let cwd = std::path::PathBuf::from(&dir);

    let config = ket_core::config::Config::load().unwrap_or_default();
    let prober = ket_core::agents::PathProber;
    let agents = &config.agents;

    println!(
        "configured agents: {:?}",
        agents.iter().map(|a| &a.name).collect::<Vec<_>>()
    );

    let runnable = |spec: &ket_core::config::AgentSpec| {
        let binary = match spec.transport {
            ket_core::config::Transport::Acp => &spec.name,
            ket_core::config::Transport::Pty => &spec.command,
        };
        ket_core::agents::Prober::find(&prober, binary).is_some()
    };
    for spec in agents {
        println!("  {} runnable={}", spec.name, runnable(spec));
    }

    let Some(spec) = agents.iter().find(|s| runnable(s)) else {
        println!("no runnable agent; would fall back to a shell");
        return;
    };

    let mut tspec = ket_core::terminal::TerminalSpec::agent(&cwd, spec);
    let sessions = ket_core::sessions::Sessions::new();
    if let Some(list) = sessions.scan(&[cwd.as_path()]).remove(cwd.as_path())
        && let Some(latest) = list.iter().find(|s| s.agent == spec.name)
        && let Some(args) = sessions.resume_args(&spec.name, &latest.id)
    {
        tspec.args.extend(args);
    }

    println!(
        "\nspawning: {} {:?}\n  in {}",
        tspec.command,
        tspec.args,
        tspec.cwd.display()
    );
    match ket_core::terminal::Terminal::open(&tspec) {
        Ok(t) => {
            println!("OPENED ok, closed={}", t.is_closed());
            std::thread::sleep(std::time::Duration::from_millis(1200));
            println!("after 1.2s, closed={}", t.is_closed());
        }
        Err(e) => println!("FAILED: {e}"),
    }
}
