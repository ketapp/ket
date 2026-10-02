//! `Workspace::view_state` / `set_view_state` — the client's opaque window
//! state, stored verbatim and bounded in size.

use ket_core::store::{MAX_LAYOUT_BYTES, Store};
use ket_core::workspace::Workspace;

mod common;
use common::Sandbox;

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

#[test]
fn defaults_to_none() {
    let sandbox = Sandbox::new("view-state-default");
    let ws = workspace(&sandbox);
    assert_eq!(ws.view_state().unwrap(), None);
}

#[test]
fn round_trips_whatever_the_client_sends() {
    let sandbox = Sandbox::new("view-state-roundtrip");
    let ws = workspace(&sandbox);

    let view = serde_json::json!({"sidebar": {"width": 280}, "tabs": ["a", "b"]});
    ws.set_view_state(Some(view.clone())).unwrap();

    assert_eq!(ws.view_state().unwrap(), Some(view));
}

#[test]
fn none_clears_a_previously_stored_view() {
    let sandbox = Sandbox::new("view-state-clear");
    let ws = workspace(&sandbox);

    ws.set_view_state(Some(serde_json::json!({"a": 1})))
        .unwrap();
    assert!(ws.view_state().unwrap().is_some());

    ws.set_view_state(None).unwrap();
    assert_eq!(ws.view_state().unwrap(), None);
}

#[test]
fn an_oversized_view_is_refused_and_leaves_the_prior_value_intact() {
    let sandbox = Sandbox::new("view-state-oversized");
    let ws = workspace(&sandbox);

    ws.set_view_state(Some(serde_json::json!({"kept": true})))
        .unwrap();

    let huge = serde_json::json!({ "blob": "x".repeat(MAX_LAYOUT_BYTES + 1) });
    let err = ws.set_view_state(Some(huge)).expect_err("should refuse");
    assert!(err.to_string().contains("limit"), "{err}");

    assert_eq!(
        ws.view_state().unwrap(),
        Some(serde_json::json!({"kept": true}))
    );
}
