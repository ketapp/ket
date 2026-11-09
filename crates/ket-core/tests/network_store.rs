//! Clearing a worktree's network log, including the bodies kept beside it.

use std::fs;
use std::io::Write;
use std::process::Command;

use ket_core::network::store::{self, RequestSummary};

mod common;
use common::Sandbox;

// Paths read process-wide environment variables. Run each case in its own
// child, with a data directory owned and removed by the parent sandbox.
fn sandboxed(name: &str, test: impl FnOnce()) {
    if std::env::var("KET_NETWORK_STORE_TEST").as_deref() == Ok(name) {
        test();
        return;
    }
    let sandbox = Sandbox::new(name);
    let output = Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture"])
        .env("KET_NETWORK_STORE_TEST", name)
        .env("XDG_DATA_HOME", sandbox.root())
        .env("XDG_CONFIG_HOME", sandbox.path("config"))
        .output()
        .unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn log(worktree: &str) -> Vec<RequestSummary> {
    let dir = store::worktree_dir(worktree).unwrap();
    fs::create_dir_all(dir.join("bodies")).unwrap();
    let mut index = fs::File::create(dir.join("index.jsonl")).unwrap();
    (1..=2)
        .map(|id| {
            let summary = RequestSummary {
                id: format!("request-{id}"),
                body: true,
                ..RequestSummary::default()
            };
            writeln!(index, "{}", serde_json::to_string(&summary).unwrap()).unwrap();
            let file = fs::File::create(dir.join("bodies").join(format!("{}.json.gz", summary.id)))
                .unwrap();
            let mut gzip = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            gzip.write_all(b"a request body").unwrap();
            gzip.finish().unwrap();
            summary
        })
        .collect()
}

#[test]
fn clearing_a_log_removes_all_its_requests_and_bodies_and_keeps_other_worktrees() {
    sandboxed(
        "clearing_a_log_removes_all_its_requests_and_bodies_and_keeps_other_worktrees",
        || {
            let cleared = log("cleared");
            let kept = log("kept");
            assert_eq!(store::summaries("cleared"), cleared);
            assert_eq!(
                store::body("cleared", "request-1").as_deref(),
                Some("a request body")
            );

            store::remove_worktree("cleared");

            assert!(!store::worktree_dir("cleared").unwrap().exists());
            assert!(store::summaries("cleared").is_empty());
            for request in cleared {
                assert_eq!(store::body("cleared", &request.id), None);
            }
            assert_eq!(store::summaries("kept"), kept);
            assert_eq!(
                store::body("kept", "request-1").as_deref(),
                Some("a request body")
            );
            assert_eq!(
                store::stored()
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>(),
                ["kept"]
            );

            // Clearing an already empty log is harmless.
            store::remove_worktree("cleared");
            assert_eq!(store::summaries("kept"), kept);
        },
    );
}

#[test]
fn an_invalid_worktree_id_cannot_clear_the_network_root_or_escape_it() {
    sandboxed(
        "an_invalid_worktree_id_cannot_clear_the_network_root_or_escape_it",
        || {
            let kept = log("kept");
            let outside = ket_core::paths::data_dir().unwrap().join("sentinel");
            fs::write(&outside, "keep me").unwrap();
            for id in ["", ".", "..", "../", "../sentinel", "/", "kept/.."] {
                store::remove_worktree(id);
                assert_eq!(store::summaries("kept"), kept, "id: {id:?}");
                assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me");
            }
        },
    );
}
