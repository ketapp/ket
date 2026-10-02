//! The Admin API's report types: deserialization, aggregation and the cost
//! arithmetic. `get`/`messages`/`costs` themselves shell out to `curl` for a
//! real network call and are not exercised here — see the module doc comment
//! in `src/admin.rs` for why it has no injectable HTTP seam (arguments are
//! deliberately kept off argv entirely, piped to curl over stdin instead).

use ket_core::admin::{AdminConfig, CostReport, CostRow, UsageReport};

#[test]
fn key_is_none_when_the_variable_is_unset() {
    let config = AdminConfig {
        key_env: "KET_TEST_ADMIN_KEY_UNSET".to_owned(),
        ..AdminConfig::default()
    };
    assert_eq!(config.key(), None);
}

// `key()`'s trimming and blank-filtering (once a variable is actually set)
// are not covered here: this crate forbids `unsafe` code, and setting an
// environment variable from safe Rust requires it. `key_is_none_when_the_
// variable_is_unset` covers the one path reachable without it.

#[test]
fn default_config_names_anthropics_own_variable() {
    assert_eq!(AdminConfig::default().key_env, "ANTHROPIC_ADMIN_KEY");
    assert!(AdminConfig::default().workspace_ids.is_empty());
    assert!(AdminConfig::default().api_key_ids.is_empty());
}

#[test]
fn cost_row_micros_converts_cents_as_a_decimal_string() {
    let row = CostRow {
        amount: "123.45".to_owned(),
        ..CostRow::default()
    };
    // "123.45" is 123.45 cents, per the module's own documented unit.
    assert_eq!(row.micros(), Some(1_234_500));
}

#[test]
fn cost_row_micros_rounds_rather_than_truncates() {
    let row = CostRow {
        amount: "0.001".to_owned(),
        ..CostRow::default()
    };
    assert_eq!(row.micros(), Some(10));
}

#[test]
fn cost_row_micros_is_none_for_unparseable_text() {
    let row = CostRow {
        amount: "not a number".to_owned(),
        ..CostRow::default()
    };
    assert_eq!(row.micros(), None);
}

#[test]
fn cost_row_micros_is_none_for_non_finite_values() {
    let row = CostRow {
        amount: "inf".to_owned(),
        ..CostRow::default()
    };
    assert_eq!(row.micros(), None);

    let row = CostRow {
        amount: "NaN".to_owned(),
        ..CostRow::default()
    };
    assert_eq!(row.micros(), None);
}

#[test]
fn cost_row_micros_handles_a_zero_and_a_negative_amount() {
    assert_eq!(
        CostRow {
            amount: "0".to_owned(),
            ..CostRow::default()
        }
        .micros(),
        Some(0)
    );
    assert_eq!(
        CostRow {
            amount: "-5.00".to_owned(),
            ..CostRow::default()
        }
        .micros(),
        Some(-50_000)
    );
}

#[test]
fn cost_report_total_micros_sums_every_row_across_every_bucket() {
    let json = serde_json::json!({
        "data": [
            {
                "starting_at": "2026-01-01T00:00:00Z",
                "ending_at": "2026-01-02T00:00:00Z",
                "results": [
                    {"amount": "10.00", "currency": "USD"},
                    {"amount": "5.50", "currency": "USD"}
                ]
            },
            {
                "starting_at": "2026-01-02T00:00:00Z",
                "ending_at": "2026-01-03T00:00:00Z",
                "results": [
                    {"amount": "1.00", "currency": "USD"}
                ]
            }
        ],
        "has_more": false,
        "next_page": null
    });
    let report: CostReport = serde_json::from_value(json).unwrap();

    assert_eq!(report.rows().count(), 3);
    assert_eq!(report.total_micros().unwrap(), 165_000);
}

#[test]
fn cost_report_total_micros_refuses_rather_than_undercounts() {
    let json = serde_json::json!({
        "data": [{
            "starting_at": "2026-01-01T00:00:00Z",
            "ending_at": "2026-01-02T00:00:00Z",
            "results": [
                {"amount": "10.00", "currency": "USD"},
                {"amount": "garbage", "currency": "USD"}
            ]
        }],
        "has_more": false,
        "next_page": null
    });
    let report: CostReport = serde_json::from_value(json).unwrap();

    let err = report.total_micros().expect_err("should refuse");
    assert!(err.to_string().contains("garbage"), "{err}");
}

#[test]
fn an_empty_cost_report_totals_to_zero() {
    let report = CostReport::default();
    assert_eq!(report.rows().count(), 0);
    assert_eq!(report.total_micros().unwrap(), 0);
}

#[test]
fn cost_report_deserializes_every_documented_field() {
    let json = serde_json::json!({
        "data": [{
            "starting_at": "2026-01-01T00:00:00Z",
            "ending_at": "2026-01-02T00:00:00Z",
            "results": [{
                "amount": "42.00",
                "currency": "USD",
                "cost_type": "tokens",
                "token_type": "uncached_input_tokens",
                "model": "claude-opus-5",
                "description": "Claude Opus 5 Usage - Input Tokens",
                "workspace_id": "wrkspc_123"
            }]
        }],
        "has_more": true,
        "next_page": "cursor-abc"
    });
    let report: CostReport = serde_json::from_value(json).unwrap();

    assert!(report.has_more);
    assert_eq!(report.next_page.as_deref(), Some("cursor-abc"));
    let row = &report.data[0].results[0];
    assert_eq!(row.cost_type.as_deref(), Some("tokens"));
    assert_eq!(row.token_type.as_deref(), Some("uncached_input_tokens"));
    assert_eq!(row.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(row.workspace_id.as_deref(), Some("wrkspc_123"));
}

#[test]
fn a_bucket_with_no_usage_is_an_empty_results_list_not_an_absent_bucket() {
    let json = serde_json::json!({
        "data": [{
            "starting_at": "2026-01-01T00:00:00Z",
            "ending_at": "2026-01-02T00:00:00Z",
            "results": []
        }],
        "has_more": false,
        "next_page": null
    });
    let report: CostReport = serde_json::from_value(json).unwrap();
    assert_eq!(report.data.len(), 1);
    assert!(report.data[0].results.is_empty());
}

#[test]
fn usage_report_by_model_sums_every_token_class_per_model() {
    let json = serde_json::json!({
        "data": [{
            "starting_at": "2026-01-01T00:00:00Z",
            "ending_at": "2026-01-02T00:00:00Z",
            "results": [
                {
                    "model": "claude-opus-5",
                    "uncached_input_tokens": 100,
                    "cache_read_input_tokens": 10,
                    "output_tokens": 50,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 5,
                        "ephemeral_1h_input_tokens": 2
                    }
                },
                {
                    "model": "claude-opus-5",
                    "uncached_input_tokens": 200,
                    "cache_read_input_tokens": 20,
                    "output_tokens": 60,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 1,
                        "ephemeral_1h_input_tokens": 0
                    }
                },
                {
                    "model": "claude-haiku-4.5",
                    "uncached_input_tokens": 1,
                    "cache_read_input_tokens": 0,
                    "output_tokens": 1,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 0,
                        "ephemeral_1h_input_tokens": 0
                    }
                }
            ]
        }],
        "has_more": false,
        "next_page": null
    });
    let report: UsageReport = serde_json::from_value(json).unwrap();

    let totals = report.by_model();
    assert_eq!(totals.len(), 2);

    let opus = &totals[&Some("claude-opus-5".to_owned())];
    assert_eq!(opus.uncached_input_tokens, 300);
    assert_eq!(opus.cache_read_input_tokens, 30);
    assert_eq!(opus.output_tokens, 110);
    assert_eq!(opus.cache_creation.ephemeral_5m_input_tokens, 6);
    assert_eq!(opus.cache_creation.ephemeral_1h_input_tokens, 2);

    let haiku = &totals[&Some("claude-haiku-4.5".to_owned())];
    assert_eq!(haiku.uncached_input_tokens, 1);
}

#[test]
fn usage_report_by_model_groups_rows_with_no_model_under_none() {
    let json = serde_json::json!({
        "data": [{
            "starting_at": "2026-01-01T00:00:00Z",
            "ending_at": "2026-01-02T00:00:00Z",
            "results": [{
                "uncached_input_tokens": 7,
                "cache_read_input_tokens": 0,
                "output_tokens": 0,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0}
            }]
        }],
        "has_more": false,
        "next_page": null
    });
    let report: UsageReport = serde_json::from_value(json).unwrap();

    let totals = report.by_model();
    assert_eq!(totals.len(), 1);
    assert_eq!(totals[&None].uncached_input_tokens, 7);
}

#[test]
fn an_empty_usage_report_has_no_rows_and_no_models() {
    let report = UsageReport::default();
    assert_eq!(report.rows().count(), 0);
    assert!(report.by_model().is_empty());
}

#[test]
fn an_unknown_field_on_a_usage_row_is_ignored_rather_than_refused() {
    // `UsageRow`/`CostRow` carry `#[serde(default)]` without
    // `deny_unknown_fields`, deliberately: the wire can grow a field this
    // crate does not read yet without breaking every existing install.
    let json = serde_json::json!({
        "data": [{
            "starting_at": "2026-01-01T00:00:00Z",
            "ending_at": "2026-01-02T00:00:00Z",
            "results": [{
                "uncached_input_tokens": 1,
                "some_future_field_ket_does_not_know_about": "whatever"
            }]
        }],
        "has_more": false,
        "next_page": null
    });
    let report: UsageReport = serde_json::from_value(json).expect("unknown fields are tolerated");
    assert_eq!(report.rows().next().unwrap().uncached_input_tokens, 1);
}
