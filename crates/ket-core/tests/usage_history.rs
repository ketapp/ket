//! `crate::usage`'s status-line parsing and the spend history it feeds.
//!
//! Distinct from `tests/usage.rs`, which covers `SessionContext`'s own usage
//! tracking and the wire event — this file is about turning a raw status-line
//! payload into a `Reading`, and what `History` does with a stream of them.

use ket_core::id::WorktreeId;
use ket_core::usage::{
    Billing, History, Reading, Record, TokenTotals, Work, WorktreeSavings, billing_from_statusline,
    builtin_pack_id, cache_from_codex, from_statusline, reading_from_codex,
    reading_from_statusline,
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
        totals: None,
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

// ---- command output, measured ---------------------------------------------------

fn output(
    program: &str,
    raw: u64,
    shown: u64,
    failed: bool,
) -> ket_core::agent_hooks::CommandOutput {
    ket_core::agent_hooks::CommandOutput {
        program: program.to_owned(),
        raw_bytes: raw,
        shown_bytes: shown,
        failed,
    }
}

#[test]
fn command_output_accumulates_by_program_and_counts_failures() {
    let sandbox = Sandbox::new("usage-command-output");
    let mut history = history_at(&sandbox);
    let wt = WorktreeId::new("w1");
    history.note_command_output("s1", &wt, 4, &output("cargo test", 4_000, 1_000, false), 1);
    history.note_command_output("s1", &wt, 4, &output("cargo test", 4_000, 4_000, false), 2);
    history.note_command_output("s1", &wt, 4, &output("git status", 400, 400, true), 3);

    let record = history.record("s1").expect("recorded");
    assert_eq!(record.command_calls, 3);
    assert_eq!(record.command_failures, 1);
    assert_eq!(record.command_output_bytes, 8_400);
    assert_eq!(record.command_shown_bytes, 5_400);
    assert_eq!(record.command_output_tokens(), 2_100);
    assert_eq!(record.filter_saved_tokens(), 750);
    assert_eq!(record.noisiest_program(), Some(("cargo test", 2_000)));
}

#[test]
fn programs_past_the_cap_fold_under_other() {
    let sandbox = Sandbox::new("usage-command-programs");
    let mut history = history_at(&sandbox);
    let wt = WorktreeId::new("w1");
    for i in 0..ket_core::usage::PROGRAMS_KEPT + 3 {
        history.note_command_output("s1", &wt, 4, &output(&format!("p{i}"), 10, 10, false), 1);
    }
    let record = history.record("s1").expect("recorded");
    assert_eq!(
        record.command_output_by_program.len(),
        ket_core::usage::PROGRAMS_KEPT + 1
    );
    assert_eq!(record.command_output_by_program.get("other"), Some(&30));
}

#[test]
fn the_share_of_input_is_none_without_input_and_past_the_whole() {
    let sandbox = Sandbox::new("usage-command-share");
    let mut history = history_at(&sandbox);
    let wt = WorktreeId::new("w1");
    history.note_command_output("s1", &wt, 4, &output("ls", 4_000, 4_000, false), 1);
    // No fresh input recorded yet: nothing to divide by.
    assert_eq!(history.record("s1").unwrap().command_output_share(), None);
}

#[test]
fn the_rollup_counts_command_output_for_every_level_before_judging_the_level() {
    let sandbox = Sandbox::new("usage-command-rollup");
    let mut history = history_at(&sandbox);
    let wt = WorktreeId::new("w1");
    let default = ket_core::worktree::default_token_reduction();
    // One session at the default level, one reduced; neither can be priced.
    history.note_command_output("off", &wt, default, &output("ls", 4_000, 4_000, false), 1);
    history.note_command_output(
        "heavy",
        &wt,
        1,
        &output("cargo test", 8_000, 2_000, false),
        1,
    );
    let savings = history.savings_by_worktree(0);
    let figures = savings.get(&wt).expect("the worktree");
    assert_eq!(figures.unreduced, 1);
    assert_eq!(figures.unpriced, 1);
    assert_eq!(figures.command_output_tokens, 3_000);
    assert_eq!(figures.filter_saved_tokens, 1_500);
    assert_eq!(figures.command_output_share(), None);
    let mut total = ket_core::usage::WorktreeSavings::default();
    total.add(figures);
    total.add(figures);
    assert_eq!(total.command_output_tokens, 6_000);
}

// ---- Economy's savings, priced ----------------------------------------------------

/// A model the list price table covers, so every figure below has a rate
/// without a configured one. The expected figures are built from that rate
/// rather than written out, so a price change moves both sides at once.
const PRICED: &str = "Opus 5.5";

fn rate() -> &'static ket_core::rates::ModelRate {
    ket_core::rates::estimate_for(PRICED).expect("a listed model")
}

/// A saved session, as `usage.json` holds it: a reduced session on [`PRICED`]
/// in `w1`, with whatever `fields` set on top.
fn record(fields: serde_json::Value) -> Record {
    let mut base = serde_json::json!({
        "session": "s1",
        "worktree": "w1",
        "level": 1,
        "model": PRICED,
        "firstSeenMs": 1_000,
        "lastSeenMs": 1_000,
    });
    base.as_object_mut()
        .unwrap()
        .extend(fields.as_object().expect("an object").clone());
    serde_json::from_value(base).expect("a record")
}

/// A history holding exactly these sessions, loaded the way ket loads one.
fn history_of(sandbox: &Sandbox, records: &[Record]) -> History {
    let path = sandbox.path("usage.json");
    std::fs::write(&path, serde_json::to_string(records).unwrap()).unwrap();
    History::load_from(path)
}

#[test]
fn the_filter_saving_is_what_it_trimmed_at_the_input_rate_once() {
    // 32,000 bytes kept from the model: 8,000 tokens, priced once as fresh
    // input and never again as the cache reads that would have followed.
    let session = record(serde_json::json!({
        "commandCalls": 3,
        "commandOutputBytes": 40_000,
        "commandShownBytes": 8_000,
        "cacheReadTokens": 5_000_000,
    }));
    assert_eq!(session.filter_saved_tokens(), 8_000);
    assert_eq!(
        session.filter_saved_micros(),
        Some(rate().input_micros(8_000))
    );

    // Nothing trimmed is a saving of nothing, not of nothing yet.
    let unfiltered = record(serde_json::json!({
        "commandCalls": 1,
        "commandOutputBytes": 4_000,
        "commandShownBytes": 4_000,
    }));
    assert_eq!(unfiltered.filter_saved_micros(), Some(0));
}

#[test]
fn nothing_is_priced_without_a_rate() {
    let fields = serde_json::json!({
        "inputTokens": 1_000,
        "outputTokens": 1_000,
        "cacheReadTokens": 1_000,
        "commandOutputBytes": 4_000,
        "commandShownBytes": 0,
    });
    let mut unknown = record(fields.clone());
    unknown.model = Some("a-local-model-nobody-prices".to_owned());
    let mut unnamed = record(fields);
    unnamed.model = None;

    for session in [unknown, unnamed] {
        assert_eq!(session.estimated_cost_micros(), None);
        assert_eq!(session.filter_saved_micros(), None);
        assert_eq!(session.economy_estimate(), None);
    }
}

#[test]
fn spend_prices_every_token_class_at_the_sessions_rate() {
    let session = record(serde_json::json!({
        "inputTokens": 120_000,
        "outputTokens": 30_000,
        "cacheReadTokens": 2_000_000,
        "cacheWriteTokens": 150_000,
        "cacheTtl": "1h",
    }));
    let rate = rate();
    assert_eq!(
        session.estimated_cost_micros(),
        Some(
            rate.input_micros(120_000)
                + rate.output_micros(30_000)
                + rate.cache_read_micros(2_000_000)
                + rate.cache_write_micros(150_000, Some("1h"))
        )
    );
}

#[test]
fn a_tainted_session_is_still_spend_but_never_an_estimate() {
    let session = record(serde_json::json!({
        "inputTokens": 1_000,
        "outputTokens": 1_000,
        "tainted": true,
    }));
    assert!(
        session
            .estimated_cost_micros()
            .is_some_and(|spent| spent > 0)
    );
    assert_eq!(session.economy_estimate(), None);
}

#[test]
fn the_rollup_spends_every_priced_session_and_credits_the_estimate_to_reduced_ones() {
    let sandbox = Sandbox::new("usage-savings-rollup");
    let default = ket_core::worktree::default_token_reduction();
    let off = record(serde_json::json!({
        "session": "off",
        "level": default,
        "inputTokens": 100_000,
        "outputTokens": 10_000,
        "cacheReadTokens": 1_000_000,
        "cacheWriteTokens": 50_000,
        "commandCalls": 2,
        "commandOutputBytes": 8_000,
        "commandShownBytes": 4_000,
    }));
    let on = record(serde_json::json!({
        "session": "on",
        "inputTokens": 50_000,
        "outputTokens": 20_000,
        "cacheReadTokens": 2_000_000,
        "cacheWriteTokens": 100_000,
        "cacheTtl": "5m",
        "turns": 4,
        "commandCalls": 5,
        "commandOutputBytes": 40_000,
        "commandShownBytes": 8_000,
    }));
    // Reduced, but the level moved under it: spent, filtered, not estimated.
    let tainted = record(serde_json::json!({
        "session": "tainted",
        "outputTokens": 5_000,
        "cacheReadTokens": 300_000,
        "tainted": true,
        "commandCalls": 1,
        "commandOutputBytes": 4_000,
        "commandShownBytes": 0,
    }));
    // Reduced, on a model with no rate: counted, never priced.
    let mut unrated = record(serde_json::json!({
        "session": "unrated",
        "outputTokens": 9_000,
    }));
    unrated.model = None;
    // Before the period: left out entirely.
    let old = record(serde_json::json!({
        "session": "old",
        "outputTokens": 99_000,
        "firstSeenMs": 10,
        "lastSeenMs": 10,
    }));

    let history = history_of(
        &sandbox,
        &[off.clone(), on.clone(), tainted.clone(), unrated, old],
    );
    let savings = history.savings_by_worktree(500);
    let figures = savings.get(&WorktreeId::new("w1")).expect("the worktree");

    assert_eq!(figures.priced, 1);
    assert_eq!(figures.unpriced, 2, "tainted and unrated");
    assert_eq!(figures.unreduced, 1);
    assert_eq!(figures.turns, 4);
    assert_eq!(figures.command_calls, 8);

    let sum = |figure: fn(&Record) -> Option<i64>, sessions: &[&Record]| -> i64 {
        sessions
            .iter()
            .map(|session| figure(session).unwrap())
            .sum()
    };
    assert_eq!(
        figures.spent_micros,
        sum(Record::estimated_cost_micros, &[&off, &on, &tainted]),
        "every priced session, at any level"
    );
    assert_eq!(
        figures.filter_saved_micros,
        sum(Record::filter_saved_micros, &[&off, &on, &tainted]),
        "every priced session, at any level"
    );
    assert_eq!(
        figures.estimated_micros,
        on.economy_estimate().unwrap().saved_micros
    );
    assert_eq!(figures.cache_read_tokens, 2_000_000);
    assert_eq!(figures.cache_write_tokens, 100_000);
    assert_eq!(
        figures.saved_micros(),
        figures.filter_saved_micros + figures.estimated_micros
    );
}

#[test]
fn the_share_is_the_saving_over_spend_and_saving_together() {
    let figures = |priced: usize, spent: i64| WorktreeSavings {
        priced,
        spent_micros: spent,
        filter_saved_micros: 250_000,
        estimated_micros: 750_000,
        ..WorktreeSavings::default()
    };
    // Spent $3, saved $1 between the two: $4 without Economy, a quarter less.
    assert_eq!(figures(1, 3_000_000).fraction(), Some(0.25));
    // No Economy session priced: no share at all.
    assert_eq!(figures(0, 3_000_000).fraction(), None);
    // Nothing spent and nothing saved: nothing to divide by.
    let empty = WorktreeSavings {
        priced: 1,
        ..WorktreeSavings::default()
    };
    assert_eq!(empty.fraction(), None);
}

#[test]
fn adding_rollups_sums_every_figure() {
    let one = WorktreeSavings {
        priced: 1,
        unpriced: 2,
        unreduced: 3,
        turns: 4,
        spent_micros: 5,
        filter_saved_micros: 6,
        estimated_micros: 8,
        cache_read_tokens: 9,
        cache_write_tokens: 10,
        input_tokens: 11,
        command_calls: 12,
        command_output_tokens: 13,
        filter_saved_tokens: 14,
    };
    let mut total = WorktreeSavings::default();
    total.add(&one);
    total.add(&one);
    assert_eq!(
        total,
        WorktreeSavings {
            priced: 2,
            unpriced: 4,
            unreduced: 6,
            turns: 8,
            spent_micros: 10,
            filter_saved_micros: 12,
            estimated_micros: 16,
            cache_read_tokens: 18,
            cache_write_tokens: 20,
            input_tokens: 22,
            command_calls: 24,
            command_output_tokens: 26,
            filter_saved_tokens: 28,
        }
    );
}

#[test]
fn the_estimate_is_the_output_a_level_is_assumed_to_have_left_unwritten() {
    let at = |level: u8, output: u64| {
        record(serde_json::json!({ "level": level, "outputTokens": output }))
            .economy_estimate()
            .expect("priced")
            .saved_micros
    };
    // Half trimmed: what was written is the other half, so as much again.
    assert_eq!(at(0, 10_000), rate().output_micros(10_000));
    // A third and a bit trimmed: 13,000 written is 65% of 20,000.
    assert_eq!(at(1, 13_000), rate().output_micros(7_000));
    // The default level, and a level the table does not cover, trim nothing.
    let default = ket_core::worktree::default_token_reduction();
    assert_eq!(at(default, 10_000), 0);
    assert_eq!(at(9, 10_000), 0);
    // A session that wrote nothing is assumed to have saved nothing.
    assert_eq!(at(0, 0), 0);
}

#[test]
fn the_estimates_cost_is_the_sessions_spend() {
    let session = record(serde_json::json!({
        "inputTokens": 40_000,
        "outputTokens": 9_000,
        "cacheReadTokens": 700_000,
        "cacheWriteTokens": 30_000,
    }));
    assert_eq!(
        Some(session.economy_estimate().unwrap().cost_micros),
        session.estimated_cost_micros()
    );
}

#[test]
fn a_filter_that_showed_more_than_it_ran_saves_nothing() {
    // Two counts from two sources; one past the other is a disagreement, not
    // a negative saving.
    let session = record(serde_json::json!({
        "commandCalls": 1,
        "commandOutputBytes": 1_000,
        "commandShownBytes": 5_000,
    }));
    assert_eq!(session.filter_saved_tokens(), 0);
    assert_eq!(session.filter_saved_micros(), Some(0));
}

#[test]
fn a_cache_write_with_no_lifetime_is_spent_at_the_five_minute_rate() {
    let spent = |ttl: Option<&str>| {
        let mut fields = serde_json::json!({ "cacheWriteTokens": 100_000 });
        if let Some(ttl) = ttl {
            fields["cacheTtl"] = ttl.into();
        }
        record(fields).estimated_cost_micros().expect("priced")
    };
    assert_eq!(spent(None), spent(Some("5m")));
    assert_eq!(
        spent(Some("1h")),
        rate().cache_write_micros(100_000, Some("1h"))
    );
    assert!(spent(Some("1h")) > spent(Some("5m")));
}

#[test]
fn the_rollup_keeps_worktrees_apart_and_goes_by_when_a_session_was_last_seen() {
    let sandbox = Sandbox::new("usage-savings-period");
    let session = |id: &str, worktree: &str, first: u64, last: u64| {
        record(serde_json::json!({
            "session": id,
            "worktree": worktree,
            "outputTokens": 1_000,
            "firstSeenMs": first,
            "lastSeenMs": last,
        }))
    };
    let history = history_of(
        &sandbox,
        &[
            // Began before the period and ran into it: counted, all of it.
            session("long", "w1", 100, 2_000),
            // Last seen on the period's first millisecond: counted.
            session("edge", "w1", 1_000, 1_000),
            // Last seen just before: not.
            session("early", "w1", 900, 999),
            session("other", "w2", 1_500, 1_500),
            // A worktree whose only session ended before the period.
            session("gone", "w3", 10, 20),
        ],
    );
    let savings = history.savings_by_worktree(1_000);

    assert_eq!(savings.get(&WorktreeId::new("w1")).unwrap().priced, 2);
    assert_eq!(savings.get(&WorktreeId::new("w2")).unwrap().priced, 1);
    assert!(
        !savings.contains_key(&WorktreeId::new("w3")),
        "nothing in the period, so not in the rollup"
    );
    let one = history
        .record("other")
        .unwrap()
        .estimated_cost_micros()
        .unwrap();
    assert_eq!(
        savings.get(&WorktreeId::new("w1")).unwrap().spent_micros,
        2 * one
    );
}

// ---- Codex totals ------------------------------------------------------------------

/// A Codex `token_count` payload: the session's running totals, and the last
/// response's own counts.
fn codex_tokens(total: (u64, u64, u64), last: (u64, u64, u64)) -> serde_json::Value {
    let usage = |(input, cached, output): (u64, u64, u64)| {
        serde_json::json!({
            "input_tokens": input,
            "cached_input_tokens": cached,
            "output_tokens": output,
        })
    };
    serde_json::json!({
        "info": {
            "total_token_usage": usage(total),
            "last_token_usage": usage(last),
        }
    })
}

#[test]
fn reading_from_codex_reads_the_running_totals_with_cached_input_split_out() {
    let reading = reading_from_codex(
        &codex_tokens((10_000, 6_000, 900), (2_000, 1_500, 300)),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        reading.totals,
        Some(TokenTotals {
            input: 4_000,
            output: 900,
            cache_read: 6_000,
        })
    );

    // A record with only a last response's counts carries no totals.
    let last_only = serde_json::json!({
        "info": { "last_token_usage": { "input_tokens": 10, "output_tokens": 5 } }
    });
    let reading = reading_from_codex(&last_only, None, None, None, None).unwrap();
    assert_eq!(reading.totals, None);
}

#[test]
fn codex_responses_landing_between_two_polls_are_all_counted() {
    let sandbox = Sandbox::new("usage-codex-totals");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);
    let poll = |tokens: &serde_json::Value| {
        reading_from_codex(tokens, Some("sess-1"), None, Some("gpt-5.6-sol"), None).unwrap()
    };

    // The first poll sees one response.
    history.observe(
        "sess-1",
        &wt,
        4,
        &poll(&codex_tokens((1_000, 500, 100), (1_000, 500, 100))),
        1_000,
    );
    // Two more land before the next poll, which sees only the newest one's
    // own counts — but the totals carry both.
    history.observe(
        "sess-1",
        &wt,
        4,
        &poll(&codex_tokens((3_600, 2_000, 400), (1_400, 900, 150))),
        2_000,
    );

    let record = history.record("sess-1").unwrap();
    assert_eq!(record.input_tokens, 1_600, "3,600 less 2,000 cached");
    assert_eq!(record.cache_read_tokens, 2_000);
    assert_eq!(record.output_tokens, 400, "not 100 + 150");
}

#[test]
fn codex_totals_correct_an_undercount_upwards_and_never_go_backwards() {
    let sandbox = Sandbox::new("usage-codex-monotonic");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    // A record built from per-response counts before totals were read.
    let mut early = reading(None);
    early.last_output_tokens = 100;
    history.observe("sess-1", &wt, 4, &early, 1_000);
    assert_eq!(history.record("sess-1").unwrap().output_tokens, 100);

    let totals = |output: u64| {
        reading_from_codex(
            &codex_tokens((0, 0, output), (0, 0, 50)),
            None,
            None,
            None,
            None,
        )
        .unwrap()
    };
    history.observe("sess-1", &wt, 4, &totals(500), 2_000);
    assert_eq!(history.record("sess-1").unwrap().output_tokens, 500);

    // A lower total — a recycled id, a rollout read from its start — leaves
    // what was already counted.
    history.observe("sess-1", &wt, 4, &totals(300), 3_000);
    assert_eq!(history.record("sess-1").unwrap().output_tokens, 500);
}

// ---- the prompt cache --------------------------------------------------------------

#[test]
fn the_prompt_cache_is_reported_but_never_counted_as_a_saving() {
    let sandbox = Sandbox::new("usage-cache-not-saved");
    let claude = record(serde_json::json!({
        "session": "claude",
        "outputTokens": 2_000,
        "cacheReadTokens": 1_000_000,
        "cacheWriteTokens": 20_000,
        "cacheTtl": "1h",
    }));
    let codex = record(serde_json::json!({
        "session": "codex",
        "model": "gpt-5.6-sol",
        "outputTokens": 1_000,
        "cacheReadTokens": 4_000_000,
    }));
    let history = history_of(&sandbox, &[claude.clone(), codex.clone()]);
    let savings = history.savings_by_worktree(0);
    let figures = savings.get(&WorktreeId::new("w1")).unwrap();

    // What the cache cost is spend like any other token.
    assert_eq!(
        figures.spent_micros,
        claude.estimated_cost_micros().unwrap() + codex.estimated_cost_micros().unwrap()
    );
    // What it read back is said, for the line that says it was left out.
    assert_eq!(figures.cache_read_tokens, 5_000_000);
    assert_eq!(figures.cache_write_tokens, 20_000);
    // And the saving is the estimate and the filter, nothing more.
    assert_eq!(
        figures.saved_micros(),
        claude.economy_estimate().unwrap().saved_micros
            + codex.economy_estimate().unwrap().saved_micros
    );
}

#[test]
fn reading_from_codex_has_no_totals_when_codex_wrote_none() {
    let null = serde_json::json!({
        "info": {
            "total_token_usage": null,
            "last_token_usage": { "input_tokens": 10, "output_tokens": 5 },
        }
    });
    let reading = reading_from_codex(&null, None, None, None, None).unwrap();
    assert_eq!(reading.totals, None);
    assert_eq!(reading.last_output_tokens, 5);
}

#[test]
fn a_reading_without_totals_after_one_with_them_adds_only_what_is_new() {
    let sandbox = Sandbox::new("usage-codex-mixed");
    let wt = WorktreeId::new("w1");
    let mut history = history_at(&sandbox);

    let totals = reading_from_codex(
        &codex_tokens((0, 0, 500), (0, 0, 50)),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    history.observe("s1", &wt, 4, &totals, 1_000);

    // The same last response again, from a source with no totals: a redraw,
    // so nothing is added.
    let mut again = reading(None);
    again.last_output_tokens = 50;
    history.observe("s1", &wt, 4, &again, 2_000);
    assert_eq!(history.record("s1").unwrap().output_tokens, 500);

    // A new response is.
    again.last_output_tokens = 70;
    history.observe("s1", &wt, 4, &again, 3_000);
    assert_eq!(history.record("s1").unwrap().output_tokens, 570);
}
