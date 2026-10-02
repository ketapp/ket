//! Adopts a discovered worktree, by hand, without a window.
//!
//! The UI half of adoption needs a click; this half does not, and running it
//! against a throwaway `XDG_DATA_HOME` is how the registry write gets checked
//! without waiting on someone to press something.
//!
//! ```sh
//! XDG_DATA_HOME="$(mktemp -d)" cargo run -p ket-core --example adopt -- <project-id> <path>
//! ```

use ket_core::id::ProjectId;
use ket_core::workspace::Workspace;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(project), Some(path)) = (args.next(), args.next()) else {
        eprintln!("usage: adopt <project-id> <worktree-path>");
        std::process::exit(2);
    };

    let workspace = Workspace::open().expect("open workspace");
    match workspace.adopt_worktree(&ProjectId::new(project), std::path::Path::new(&path)) {
        Ok(worktree) => println!(
            "adopted {} branch={} base={} base_commit={:?} exists={}",
            worktree.id,
            worktree.branch,
            worktree.base,
            worktree.base_commit,
            worktree.exists()
        ),
        Err(e) => {
            eprintln!("refused: {e}");
            std::process::exit(1);
        }
    }
}
