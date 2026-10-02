//! Prints what `ket_core::sessions` finds for a directory, and how it would
//! resume it. A hand-run probe for the discovery path, not a test.

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: sessions <directory>");
    let path = std::path::PathBuf::from(&dir);

    println!("asked about   : {}", path.display());
    println!(
        "canonical     : {}",
        std::fs::canonicalize(&path)
            .unwrap_or_else(|_| path.clone())
            .display()
    );
    println!(
        "claude config : {}",
        ket_core::sessions::claude_config_dir().display()
    );

    let sessions = ket_core::sessions::Sessions::new();
    let mut found = sessions.scan(&[path.as_path()]);
    println!("buckets       : {:?}", found.keys().collect::<Vec<_>>());

    let Some(list) = found.remove(path.as_path()) else {
        println!("\nNOTHING FOUND for that exact key");
        return;
    };

    for s in &list {
        println!(
            "  {} {}  {}",
            s.agent,
            &s.id[..s.id.len().min(8)],
            s.title.as_deref().unwrap_or("(no title)")
        );
    }

    if let Some(latest) = list.iter().find(|s| s.agent == "claude") {
        println!(
            "\nwould launch  : claude {}",
            sessions
                .resume_args("claude", &latest.id)
                .unwrap_or_default()
                .join(" ")
        );
    } else {
        println!("\nno claude session in that bucket");
    }
}
