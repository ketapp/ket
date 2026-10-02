//! What ket would pass to an agent to get a session back on screen.
//!
//! ```sh
//! cargo run -p ket-core --example openargs -- claude /Users/me/dev/ket <session-id>
//! ```

use ket_core::sessions::Sessions;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(agent), Some(dir), Some(session)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: openargs <agent> <directory> <session-id>");
        std::process::exit(2);
    };
    let sessions = Sessions::new();
    println!(
        "resume_args = {:?}\nopen_args   = {:?}",
        sessions.resume_args(&agent, &session),
        sessions.open_args(&agent, std::path::Path::new(&dir), &session)
    );
}
