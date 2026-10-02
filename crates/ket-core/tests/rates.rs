//! Contracted rates: usable-entry validation, model matching, cost arithmetic
//! and money formatting.

use ket_core::rates::{ModelRate, RateConfig, money};

fn rate(model: &str, input: f64, output: f64) -> ModelRate {
    ModelRate {
        model: model.to_owned(),
        input,
        output,
        cache_read: 0.0,
        cache_write: 0.0,
        cache_write_1h: None,
    }
}

// ---- ModelRate::usable -------------------------------------------------------

#[test]
fn a_named_non_negative_entry_is_usable() {
    assert!(rate("Opus", 15.0, 75.0).usable());
}

#[test]
fn an_all_zero_entry_is_usable() {
    assert!(rate("Free Model", 0.0, 0.0).usable());
}

#[test]
fn a_nameless_entry_is_not_usable() {
    assert!(!rate("  ", 1.0, 1.0).usable());
}

#[test]
fn a_negative_rate_is_not_usable() {
    assert!(!rate("Opus", -1.0, 1.0).usable());
}

#[test]
fn a_non_finite_rate_is_not_usable() {
    let mut model = rate("Opus", f64::NAN, 1.0);
    assert!(!model.usable());
    model.input = f64::INFINITY;
    assert!(!model.usable());
}

// ---- RateConfig::is_empty / for_model ----------------------------------------

#[test]
fn a_table_with_no_usable_entry_is_empty() {
    let config = RateConfig {
        models: vec![rate("  ", 1.0, 1.0)],
    };
    assert!(config.is_empty());
}

#[test]
fn a_table_with_one_usable_entry_is_not_empty() {
    let config = RateConfig {
        models: vec![rate("Opus", 1.0, 1.0)],
    };
    assert!(!config.is_empty());
}

#[test]
fn for_model_matches_exactly_first() {
    let config = RateConfig {
        models: vec![rate("Opus 5", 1.0, 1.0), rate("Opus", 2.0, 2.0)],
    };
    let found = config.for_model("Opus 5").unwrap();
    assert_eq!(found.input, 1.0);
}

#[test]
fn for_model_matches_case_insensitively() {
    let config = RateConfig {
        models: vec![rate("Opus 5", 1.0, 1.0)],
    };
    assert!(config.for_model("OPUS 5").is_some());
}

#[test]
fn for_model_falls_back_to_a_substring_match() {
    let config = RateConfig {
        models: vec![rate("Opus", 1.0, 1.0)],
    };
    // "Claude Opus 5" is not an exact entry, but contains "opus".
    let found = config.for_model("Claude Opus 5").unwrap();
    assert_eq!(found.input, 1.0);
}

#[test]
fn for_model_prefers_the_longest_substring_match() {
    let config = RateConfig {
        models: vec![rate("Opus", 1.0, 1.0), rate("Opus 5.1", 2.0, 2.0)],
    };
    let found = config.for_model("Claude Opus 5.1").unwrap();
    assert_eq!(found.input, 2.0, "the more specific entry should win");
}

#[test]
fn for_model_ignores_entries_that_are_not_usable() {
    let config = RateConfig {
        models: vec![rate("  ", 1.0, 1.0)],
    };
    assert!(config.for_model("anything").is_none());
}

#[test]
fn for_model_is_none_for_a_blank_query() {
    let config = RateConfig {
        models: vec![rate("Opus", 1.0, 1.0)],
    };
    assert!(config.for_model("   ").is_none());
}

#[test]
fn for_model_is_none_with_no_matching_entry() {
    let config = RateConfig {
        models: vec![rate("Opus", 1.0, 1.0)],
    };
    assert!(config.for_model("Gemini").is_none());
}

// ---- ModelRate cost arithmetic -----------------------------------------------

#[test]
fn input_and_output_micros_convert_dollars_per_million_to_micros() {
    let model = rate("Opus", 15.0, 75.0);
    // 1,000,000 tokens at $15/million is exactly $15 = 15,000,000 micros.
    assert_eq!(model.input_micros(1_000_000), 15_000_000);
    assert_eq!(model.output_micros(1_000_000), 75_000_000);
}

#[test]
fn cache_read_micros_uses_the_cache_read_rate() {
    let mut model = rate("Opus", 15.0, 75.0);
    model.cache_read = 1.5;
    assert_eq!(model.cache_read_micros(1_000_000), 1_500_000);
}

#[test]
fn cache_write_micros_uses_the_five_minute_rate_by_default() {
    let mut model = rate("Opus", 15.0, 75.0);
    model.cache_write = 18.75;
    model.cache_write_1h = Some(30.0);

    assert_eq!(model.cache_write_micros(1_000_000, None), 18_750_000);
    assert_eq!(
        model.cache_write_micros(1_000_000, Some("anything-else")),
        18_750_000
    );
}

#[test]
fn cache_write_micros_uses_the_one_hour_rate_when_asked() {
    let mut model = rate("Opus", 15.0, 75.0);
    model.cache_write = 18.75;
    model.cache_write_1h = Some(30.0);
    assert_eq!(model.cache_write_micros(1_000_000, Some("1h")), 30_000_000);
}

#[test]
fn cache_write_micros_falls_back_to_the_five_minute_rate_when_1h_is_unset() {
    let mut model = rate("Opus", 15.0, 75.0);
    model.cache_write = 18.75;
    assert_eq!(model.cache_write_micros(1_000_000, Some("1h")), 18_750_000);
}

#[test]
fn zero_tokens_costs_nothing() {
    let model = rate("Opus", 15.0, 75.0);
    assert_eq!(model.input_micros(0), 0);
}

// ---- money formatting ---------------------------------------------------------

#[test]
fn zero_is_zero_dollars() {
    assert_eq!(money(0), "$0.00");
}

#[test]
fn small_amounts_are_shown_in_cents() {
    assert_eq!(money(420_000), "$0.42");
    assert_eq!(money(9_990_000), "$9.99");
}

#[test]
fn an_amount_that_would_round_to_zero_but_is_not_zero_reads_as_under_a_cent() {
    assert_eq!(money(1), "<$0.01");
    assert_eq!(money(-1), "-<$0.01");
}

#[test]
fn ten_dollars_and_above_is_shown_as_whole_dollars() {
    assert_eq!(money(10_000_000), "$10");
    assert_eq!(money(1_284_370_000), "$1,284");
}

#[test]
fn large_whole_dollar_amounts_are_thousands_grouped() {
    assert_eq!(money(1_000_000_000_000), "$1,000,000");
}

#[test]
fn negative_whole_dollar_amounts_keep_their_sign() {
    assert_eq!(money(-15_000_000), "-$15");
}

#[test]
fn an_amount_that_rounds_up_to_ten_dollars_joins_the_whole_dollar_branch() {
    // The boundary is tested against the *rounded* figure: $9.999999 rounds
    // to $10 and must land in the same branch as $10 itself, not render as
    // "$10.00" immediately below it.
    assert_eq!(money(9_999_999), "$10");
}
