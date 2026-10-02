//! `crate::usage`'s status-line parsing and the spend history it feeds.
//!
//! Distinct from `tests/usage.rs`, which covers `SessionContext`'s own usage
//! tracking and the wire event — this file is about turning a raw status-line
//! payload into a `Reading`, and what `History` does with a stream of them.

use ket_core::id::WorktreeId;
use ket_core::usage::{
    Billing, History, Reading, Work, billing_from_statusline, builtin_pack_id, cache_from_codex,
    from_statusline, reading_from_codex, reading_from_statusline,
};

mod common;
use common::Sandbox;

fn history_at(sandbox: &Sandbox) -> History {
    History::load_from(sandbox.path("usage.json"))
}

fn reading(cost_micros: Option<i64>) -> Reading {
    Reading {
        session: None,
        name: None,
        model: Some("claude-opus".to_owned()),
        version: None,
        effort: None,
        fast_mode: false,
        cwd: None,
        last_output_tokens: 0,
        last_input_tokens: 0,
        last_cache_read_tokens: 0,
        work: Work::default(),
        cache: None,
        billing: None,
        usage: ket_core::event::SessionUsage {
            used: 0,
            size: 0,
            cost: cost_micros.map(|micros| ket_core::event::Money {
                micros,
                currency: "USD".to_owned(),
            }),
        },
    }
}

// ---- from_statusline ---------------------------------------------------------

#[test]
fn a_payload_with_neither_a_window_nor_a_cost_is_none() {
    assert_eq!(from_statusline(&serde_json::json!({})), None);
}

#[test]
fn an_unmeasured_payload_with_only_a_cost_reports_zero_window() {
    let payload = serde_json::json!({ "cost": { "total_cost_usd": 1.5 } });
    let usage = from_statusline(&payload).expect("has a cost");
    assert_eq!(usage.used, 0);
    assert_eq!(usage.size, 0);
    assert_eq!(usage.cost.unwrap().micros, 1_500_000);
}

#[test]
fn a_measured_window_reports_the_totals() {
    let payload = serde_json::json!({
        "context_window": {
            "current_usage": { "input_tokens": 10 },
            "total_input_tokens": 5_000,
            "context_window_size": 200_000,
        }
    });
    let usage = from_statusline(&payload).expect("measured");
    assert_eq!(usage.used, 5_000);
    assert_eq!(usage.size, 200_000);
    assert!(usage.cost.is_none());
}

#[test]
fn a_negative_or_non_finite_cost_is_dropped_not_zeroed() {
    let payload = serde_json::json!({ "cost": { "total_cost_usd": -1.0 } });
    assert_eq!(from_statusline(&payload), None);

    let payload = serde_json::json!({ "cost": { "total_cost_usd": f64::NAN } });
    assert_eq!(from_statusline(&payload), None);
}

// ---- billing_from_statusline --------------------------------------------------

#[test]
fn rate_limits_present_means_a_plan() {
    let payload = serde_json::json!({ "rate_limits": {} });
    assert_eq!(billing_from_statusline(&payload), Some(Billing::Plan));
}

#[test]
fn no_rate_limits_but_a_measured_response_means_metered() {
    let payload = serde_json::json!({ "prompt_cache": {} });
    assert_eq!(billing_from_statusline(&payload), Some(Billing::Metered));

    let payload = serde_json::json!({
        "context_window": { "current_usage": {} }
    });
    assert_eq!(billing_from_statusline(&payload), Some(Billing::Metered));

    let payload = serde_json::json!({ "cost": { "total_cost_usd": 0.5 } });
    assert_eq!(billing_from_statusline(&payload), Some(Billing::Metered));
}

#[test]
fn a_session_that_has_not_responded_yet_is_neither() {
    assert_eq!(billing_from_statusline(&serde_json::json!({})), None);
    // A zero cost is not evidence of a response, only a field that exists.
    let payload = serde_json::json!({ "cost": { "total_cost_usd": 0.0 } });
    assert_eq!(billing_from_statusline(&payload), None);
}

// ---- reading_from_statusline ---------------------------------------------------

#[test]
fn reading_is_none_when_from_statusline_is() {
    assert!(reading_from_statusline(&serde_json::json!({})).is_none());
}

#[test]
fn reading_fills_in_the_cache_block_with_rounding_and_clamping() {
    let payload = serde_json::json!({
        "cost": { "total_cost_usd": 1.0 },
        "session_id": "s1",
        "prompt_cache": {
            "warm": true,
            "caching_observed": true,
            "ttl": "1h",
            "hit_ratio": 0.5,
            "requests": 4,
            "misses": 1,
            "expected_rebuilds": 0,
            "last_miss_cause": { "causes": ["ttl_expired_5m", "tools_changed"] },
            "miss_causes": { "ttl_expired_5m": 1 },
            "recache_tokens_if_cold": 1200,
            "cache_write_tokens": 340,
        }
    });

    let reading = reading_from_statusline(&payload).expect("has a reading");
    assert_eq!(reading.session.as_deref(), Some("s1"));
    let cache = reading.cache.expect("cache block present");
    assert!(cache.warm);
    assert!(cache.observed);
    assert_eq!(cache.ttl.as_deref(), Some("1h"));
    assert_eq!(cache.hit_percent, Some(50));
    assert_eq!(cache.requests, 4);
    assert_eq!(cache.misses, 1);
    assert_eq!(cache.last_miss_cause.as_deref(), Some("ttl_expired_5m"));
    assert_eq!(cache.miss_causes.get("ttl_expired_5m"), Some(&1));
    assert_eq!(cache.recache_if_cold, Some(1200));
    assert_eq!(cache.write_tokens, Some(340));
}

#[test]
fn a_hit_ratio_above_one_is_clamped_to_one_hundred_percent() {
    let payload = serde_json::json!({
        "cost": { "total_cost_usd": 1.0 },
        "prompt_cache": { "caching_observed": true, "hit_ratio": 4.0 },
    });
    let cache = reading_from_statusline(&payload).unwrap().cache.unwrap();
    assert_eq!(cache.hit_percent, Some(100));
}

#[test]
fn hit_percent_is_none_when_caching_was_not_observed_even_with_a_ratio_present() {
    // A gateway stripping cache_control markers reports a truthful, meaningless
    // zero; the module must not draw it as a ratio.
    let payload = serde_json::json!({
        "cost": { "total_cost_usd": 1.0 },
        "prompt_cache": { "caching_observed": false, "hit_ratio": 0.0 },
    });
    let cache = reading_from_statusline(&payload).unwrap().cache.unwrap();
    assert_eq!(cache.hit_percent, None);
}

#[test]
fn builtin_pack_id_is_a_stable_name() {
    assert_eq!(builtin_pack_id(), "builtin");
}

// ---- codex ---------------------------------------------------------------------

#[test]
fn cache_from_codex_is_none_without_input_tokens() {
    let payload = serde_json::json!({ "info": { "total_token_usage": { "input_tokens": 0 } } });
    assert_eq!(cache_from_codex(&payload), None);

    assert_eq!(cache_from_codex(&serde_json::json!({})), None);
}

#[test]
fn cache_from_codex_clamps_cached_to_input_and_computes_the_ratio() {
    let payload = serde_json::json!({
        "info": {
            "total_token_usage": { "input_tokens": 100, "cached_input_tokens": 500 },
            "last_token_usage": { "cached_input_tokens": 10 },
        }
    });
    let cache = cache_from_codex(&payload).expect("has cache");
    assert!(cache.observed);
    assert!(cache.warm);
    // Clamped to input (100), so the ratio never exceeds 100%.
    assert_eq!(cache.hit_percent, Some(100));
    assert_eq!(cache.write_tokens, None);
    assert_eq!(cache.ttl, None);
}

#[test]
fn cache_from_codex_is_not_observed_when_nothing_was_cached() {
    let payload = serde_json::json!({
        "info": {
            "total_token_usage": { "input_tokens": 50, "cached_input_tokens": 0 },
        }
    });
    let cache = cache_from_codex(&payload).unwrap();
    assert!(!cache.observed);
    assert!(!cache.warm);
    assert_eq!(cache.hit_percent, None);
}

#[test]
fn reading_from_codex_subtracts_cached_tokens_from_fresh_input() {
    let tokens = serde_json::json!({
        "info": {
            "total_token_usage": { "input_tokens": 1000 },
            "last_token_usage": { "input_tokens": 300, "cached_input_tokens": 250, "output_tokens": 40 },
            "model_context_window": 128_000,
        }
    });

    let reading = reading_from_codex(
        &tokens,
        Some("sess-1"),
        Some("/repo"),
        Some("gpt-5-codex"),
        Some("high"),
    )
    .expect("has a reading");

    assert_eq!(reading.session.as_deref(), Some("sess-1"));
    assert_eq!(reading.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(reading.effort.as_deref(), Some("high"));
    assert_eq!(reading.last_output_tokens, 40);
    assert_eq!(reading.last_input_tokens, 50, "300 - 250 cached");
    assert_eq!(reading.last_cache_read_tokens, 250);
    assert_eq!(reading.usage.used, 1000);
    assert_eq!(reading.usage.size, 128_000);
    assert!(reading.usage.cost.is_none(), "Codex reports no cost");
}

#[test]
fn reading_from_codex_never_underflows_when_cached_exceeds_reported_input() {
    let tokens = serde_json::json!({
        "info": {
            "total_token_usage": { "input_tokens": 10 },
            "last_token_usage": { "input_tokens": 5, "cached_input_tokens": 999 },
        }
    });
    let reading = reading_from_codex(&tokens, None, None, None, None).unwrap();
    assert_eq!(
        reading.last_input_tokens, 0,
        "saturates rather than wrapping"
    );
}

#[test]
fn reading_from_codex_is_none_without_an_info_block() {
    assert!(reading_from_codex(&serde_json::json!({}), None, None, None, None).is_none());
}

// ---- History -------------------------------------------------------------------

#[test]
fn loading_a_missing_file_is_an_empty_history() {
    let sandbox = Sandbox::new("usage-history-missing");
    let history = history_at(&sandbox);
    assert!(!history.dirty());
    assert_eq!(history.record("nope"), None);
}

#[test]
fn loading_a_corrupt_file_is_empty_rather_than_an_error() {
    let sandbox = Sandbox::new("usage-history-corrupt");
    let path = sandbox.path("usage.json");
    std::fs::write(&path, "not json at all").unwrap();
    let history = History::load_from(path);
    assert_eq!(history.record("nope"), None);
}

#[test]
fn save_and_reload_round_trips_a_record() {
    let sandbox = Sandbox::new("usage-history-roundtrip");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    history.note_turn("s1", &wt, 4, 1_000);
    history.observe("s1", &wt, 4, &reading(Some(500_000)), 1_000);
    history.save().expect("save");
    assert!(!history.dirty(), "clean right after a save");

    let reloaded = History::load_from(history.path().to_path_buf());
    let record = reloaded.record("s1").expect("persisted");
    assert_eq!(record.turns, 1);
    assert_eq!(record.cost_micros, 500_000);
}

#[test]
fn observe_stamps_a_new_record_with_the_active_pack_and_level() {
    let sandbox = Sandbox::new("usage-history-new-record");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    history.observe("s1", &wt, 4, &reading(None), 1_000);
    let record = history.record("s1").expect("created");
    assert_eq!(record.level, 4);
    assert_eq!(record.pack_id, builtin_pack_id());
    assert_eq!(record.first_seen_ms, 1_000);
    assert_eq!(record.last_seen_ms, 1_000);
    assert!(!record.tainted);
}

#[test]
fn a_level_change_mid_session_taints_the_record() {
    let sandbox = Sandbox::new("usage-history-taint-level");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    history.observe("s1", &wt, 4, &reading(None), 1_000);
    history.observe("s1", &wt, 2, &reading(None), 2_000);

    assert!(history.record("s1").unwrap().tainted);
}

#[test]
fn repeated_output_tokens_are_not_double_counted() {
    let sandbox = Sandbox::new("usage-history-dedupe");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    let mut r = reading(None);
    r.last_output_tokens = 100;
    history.observe("s1", &wt, 4, &r, 1_000);
    // Same reading again — a redraw, not a new response.
    history.observe("s1", &wt, 4, &r, 1_100);
    assert_eq!(history.record("s1").unwrap().output_tokens, 100);

    r.last_output_tokens = 150;
    history.observe("s1", &wt, 4, &r, 1_200);
    // Each new value is added whole, not diffed against the one before it —
    // 100 from the first response, 150 from the second: 250 total.
    assert_eq!(history.record("s1").unwrap().output_tokens, 250);
}

#[test]
fn cost_is_monotonic_and_baseline_is_set_only_once() {
    let sandbox = Sandbox::new("usage-history-cost-baseline");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    history.observe("s1", &wt, 4, &reading(Some(100_000)), 1_000);
    assert_eq!(
        history.record("s1").unwrap().baseline_cost_micros,
        Some(100_000)
    );

    history.observe("s1", &wt, 4, &reading(Some(200_000)), 1_100);
    let record = history.record("s1").unwrap();
    assert_eq!(record.cost_micros, 200_000);
    assert_eq!(record.baseline_cost_micros, Some(100_000), "set only once");

    // A status line drawn early in a new session with a recycled id must not
    // erase a higher figure already recorded.
    history.observe("s1", &wt, 4, &reading(Some(50_000)), 1_200);
    assert_eq!(history.record("s1").unwrap().cost_micros, 200_000);
}

#[test]
fn turns_already_counted_before_a_baseline_taints_the_record() {
    let sandbox = Sandbox::new("usage-history-taint-baseline");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    // Turns counted before any cost has ever been reported for this session.
    history.observe("s1", &wt, 4, &reading(None), 1_000);
    history.note_turn("s1", &wt, 4, 1_100);
    // Now a cost arrives; spend before this point cannot be separated out.
    history.observe("s1", &wt, 4, &reading(Some(10_000)), 1_200);

    assert!(history.record("s1").unwrap().tainted);
}

#[test]
fn note_tool_call_counts_the_total_and_the_per_server_breakdown() {
    let sandbox = Sandbox::new("usage-history-tool-calls");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    history.note_tool_call("s1", &wt, 4, Some("github"), 1_000);
    history.note_tool_call("s1", &wt, 4, Some("github"), 1_100);
    history.note_tool_call("s1", &wt, 4, None, 1_200);

    let record = history.record("s1").unwrap();
    assert_eq!(record.tool_calls, 3, "every call counts toward the total");
    assert_eq!(record.mcp_calls.get("github"), Some(&2));
    assert_eq!(
        record.mcp_calls.len(),
        1,
        "a built-in tool has no server entry"
    );
}

#[test]
fn latest_billing_is_the_most_recently_seen_regardless_of_session() {
    let sandbox = Sandbox::new("usage-history-latest-billing");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    let mut plan_reading = reading(None);
    plan_reading.billing = Some(Billing::Plan);
    history.observe("older", &wt, 4, &plan_reading, 1_000);

    let mut metered_reading = reading(None);
    metered_reading.billing = Some(Billing::Metered);
    history.observe("newer", &wt, 4, &metered_reading, 2_000);

    assert_eq!(history.latest_billing(), Some(Billing::Metered));
}

#[test]
fn latest_billing_is_none_until_something_has_reported_it() {
    let sandbox = Sandbox::new("usage-history-no-billing");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);
    history.observe("s1", &wt, 4, &reading(None), 1_000);
    assert_eq!(history.latest_billing(), None);
}

#[test]
fn latest_per_worktree_keeps_only_the_newest_session_in_each() {
    let sandbox = Sandbox::new("usage-history-latest-per-worktree");
    let wt1 = WorktreeId::new("w1");
    let wt2 = WorktreeId::new("w2");
    let mut history = history_at(&sandbox);

    history.observe("s1-old", &wt1, 4, &reading(None), 1_000);
    history.observe("s1-new", &wt1, 4, &reading(None), 2_000);
    history.observe("s2", &wt2, 4, &reading(None), 1_500);

    let latest = history.latest_per_worktree();
    assert_eq!(latest.len(), 2);
    assert_eq!(latest[&wt1].session, "s1-new");
    assert_eq!(latest[&wt2].session, "s2");
}

// ---- Record::attributed_micros / restored_reading -------------------------------

#[test]
fn attributed_micros_subtracts_the_baseline_and_never_goes_negative() {
    let sandbox = Sandbox::new("usage-history-attributed");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    // A resumed session: opens already costing 38.62 (38_620_000 micros).
    history.observe("s1", &wt, 4, &reading(Some(38_620_000)), 1_000);
    history.note_turn("s1", &wt, 4, 1_100);
    history.observe("s1", &wt, 4, &reading(Some(38_720_000)), 1_200);

    let record = history.record("s1").unwrap();
    assert_eq!(record.attributed_micros(), 100_000);
}

#[test]
fn restored_reading_carries_cost_but_not_context_pressure() {
    let sandbox = Sandbox::new("usage-history-restored");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);
    history.observe("s1", &wt, 4, &reading(Some(250_000)), 1_000);

    let restored = history.record("s1").unwrap().restored_reading();
    assert_eq!(restored.session.as_deref(), Some("s1"));
    assert_eq!(restored.usage.used, 0, "context pressure is not stored");
    assert_eq!(restored.usage.size, 0);
    assert_eq!(restored.usage.cost.unwrap().micros, 250_000);
}

#[test]
fn restored_reading_has_no_cost_when_none_was_ever_recorded() {
    let sandbox = Sandbox::new("usage-history-restored-no-cost");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);
    history.observe("s1", &wt, 4, &reading(None), 1_000);

    let restored = history.record("s1").unwrap().restored_reading();
    assert!(restored.usage.cost.is_none());
}

// ---- eviction --------------------------------------------------------------------

#[test]
fn the_oldest_record_is_evicted_once_the_cap_is_reached() {
    let sandbox = Sandbox::new("usage-history-eviction");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    // MAX_RECORDS is 500; filling it and one more forces an eviction.
    for i in 0..501 {
        history.observe(&format!("s{i}"), &wt, 4, &reading(None), 1_000 + i as u64);
    }

    assert!(history.record("s0").is_none(), "the oldest was evicted");
    assert!(history.record("s500").is_some(), "the newest survives");
}
