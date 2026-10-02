//! Level packs: parsing and validating the JSON document, and the
//! level-table math built on top of it.
//!
//! The network fetch (`fetch`) and the cache-path-dependent functions
//! (`load`, `store`, `promote`, `refresh`, `sweep_temps`, `is_cache`) all
//! resolve against the real `$XDG_CACHE_HOME`/`$HOME`, which this crate's own
//! `paths` module deliberately keeps unmockable without mutating process-wide
//! env — a race under a parallel test runner. `store_at` is covered by an
//! inline unit test in `src/pack.rs` instead, since it already takes an
//! explicit path.

use ket_core::pack::{Pack, PackLevel, PackPolicy, SCHEMA_VERSION, parse};

fn level(id: &str) -> PackLevel {
    PackLevel {
        id: id.to_owned(),
        label: id.to_owned(),
        description: "a level".to_owned(),
        tag: None,
        instruction: None,
        effort: None,
        model: None,
        cache_ttl: None,
        autocompact: None,
        subagent_model: None,
        subagent_cache_ttl: None,
        agent_teams: None,
        cross_session_inbound: None,
        verbosity: None,
        reasoning_summary: None,
        claude: PackPolicy::default(),
        codex: PackPolicy::default(),
        opencode: PackPolicy::default(),
        grok: PackPolicy::default(),
    }
}

fn pack_json(levels: &str, default_level: &str) -> String {
    format!(
        r#"{{"schemaVersion":2,"packId":"test-pack","packVersion":1,"levels":[{levels}],"defaultLevel":"{default_level}"}}"#
    )
}

fn minimal_level(id: &str) -> String {
    format!(r#"{{"id":"{id}","label":"L","description":"D"}}"#)
}

// ---- parse: schema and shape ------------------------------------------------

#[test]
fn a_well_formed_pack_parses() {
    let json = pack_json(&minimal_level("full"), "full");
    let pack = parse(json.as_bytes()).expect("parse");
    assert_eq!(pack.pack_id, "test-pack");
    assert_eq!(pack.levels.len(), 1);
    assert_eq!(pack.default_level, "full");
}

#[test]
fn invalid_json_is_refused() {
    let err = parse(b"not json").expect_err("should refuse");
    assert!(err.to_string().contains("not valid JSON"), "{err}");
}

#[test]
fn a_document_over_the_byte_limit_is_refused() {
    let huge = vec![b' '; 300 * 1024];
    let err = parse(&huge).expect_err("should refuse");
    assert!(err.to_string().contains("larger than"), "{err}");
}

#[test]
fn a_missing_schema_version_is_refused() {
    let json = r#"{"packId":"x","packVersion":1,"levels":[],"defaultLevel":"a"}"#;
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("no schemaVersion"), "{err}");
}

#[test]
fn schema_zero_is_refused() {
    let json = r#"{"schemaVersion":0,"packId":"x","packVersion":1,"levels":[],"defaultLevel":"a"}"#;
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("understands"), "{err}");
}

#[test]
fn a_schema_from_the_future_is_refused() {
    let json = format!(
        r#"{{"schemaVersion":{},"packId":"x","packVersion":1,"levels":[{}],"defaultLevel":"a"}}"#,
        SCHEMA_VERSION + 1,
        minimal_level("a")
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("upgrade ket"), "{err}");
}

#[test]
fn an_unknown_field_is_refused() {
    let json = r#"{"schemaVersion":2,"packId":"x","packVersion":1,"levels":[],"defaultLevel":"a","mystery":true}"#;
    assert!(parse(json.as_bytes()).is_err());
}

// ---- parse: level-list validation -------------------------------------------

#[test]
fn a_pack_with_no_levels_is_refused() {
    let json = pack_json("", "a");
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("no levels"), "{err}");
}

#[test]
fn a_blank_pack_id_is_refused() {
    let json = r#"{"schemaVersion":2,"packId":"  ","packVersion":1,"levels":[{"id":"a","label":"L","description":"D"}],"defaultLevel":"a"}"#;
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("pack id is blank"), "{err}");
}

#[test]
fn a_blank_default_level_is_refused() {
    let json = pack_json(&minimal_level("a"), "  ");
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("default level is blank"), "{err}");
}

#[test]
fn a_blank_level_id_is_refused() {
    let json = pack_json(r#"{"id":"  ","label":"L","description":"D"}"#, "  ");
    let err = parse(json.as_bytes()).expect_err("should refuse");
    // Blank default_level is checked first; use a non-blank default to reach
    // the level-id check specifically.
    let _ = err;
    let json = format!(
        r#"{{"schemaVersion":2,"packId":"p","packVersion":1,"levels":[{}],"defaultLevel":"a"}}"#,
        r#"{"id":"  ","label":"L","description":"D"}"#
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("blank level id"), "{err}");
}

#[test]
fn a_default_level_naming_no_real_level_is_refused() {
    let json = pack_json(&minimal_level("a"), "nonexistent");
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn duplicate_level_ids_are_refused() {
    let json = pack_json(
        &format!("{},{}", minimal_level("dup"), minimal_level("dup")),
        "dup",
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("same id"), "{err}");
}

#[test]
fn more_than_256_levels_is_refused() {
    let levels: Vec<String> = (0..300).map(|i| minimal_level(&format!("l{i}"))).collect();
    let json = pack_json(&levels.join(","), "l0");
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("more than 256"), "{err}");
}

// ---- parse: per-policy validation, applied to both a level and its provider
// overrides ---------------------------------------------------------------

#[test]
fn an_invalid_verbosity_is_refused() {
    let json = pack_json(
        r#"{"id":"a","label":"L","description":"D","verbosity":"loud"}"#,
        "a",
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("verbosity"), "{err}");
}

#[test]
fn valid_verbosity_values_are_accepted() {
    for value in ["low", "medium", "high"] {
        let json = pack_json(
            &format!(r#"{{"id":"a","label":"L","description":"D","verbosity":"{value}"}}"#),
            "a",
        );
        parse(json.as_bytes()).unwrap_or_else(|e| panic!("{value} should be valid: {e}"));
    }
}

#[test]
fn an_invalid_reasoning_summary_is_refused() {
    let json = pack_json(
        r#"{"id":"a","label":"L","description":"D","reasoningSummary":"maybe"}"#,
        "a",
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("reasoning summary"), "{err}");
}

#[test]
fn an_invalid_cross_session_inbound_is_refused() {
    let json = pack_json(
        r#"{"id":"a","label":"L","description":"D","crossSessionInbound":"maybe"}"#,
        "a",
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("cross-session"), "{err}");
}

#[test]
fn autocompact_accepts_auto_or_a_positive_integer() {
    for value in [r#""auto""#, r#""1000""#] {
        let json = pack_json(
            &format!(r#"{{"id":"a","label":"L","description":"D","autocompact":{value}}}"#),
            "a",
        );
        parse(json.as_bytes()).unwrap_or_else(|e| panic!("{value} should be valid: {e}"));
    }
}

#[test]
fn autocompact_rejects_zero_and_nonsense() {
    for value in [r#""0""#, r#""soon""#] {
        let json = pack_json(
            &format!(r#"{{"id":"a","label":"L","description":"D","autocompact":{value}}}"#),
            "a",
        );
        let err = parse(json.as_bytes()).expect_err(&format!("{value} should be refused"));
        assert!(err.to_string().contains("autocompact"), "{err}");
    }
}

#[test]
fn an_invalid_provider_override_is_also_refused() {
    // The top-level policy is fine; the Codex override is not.
    let json = pack_json(
        r#"{"id":"a","label":"L","description":"D","codex":{"verbosity":"loud"}}"#,
        "a",
    );
    let err = parse(json.as_bytes()).expect_err("should refuse");
    assert!(err.to_string().contains("verbosity"), "{err}");
}

// ---- Pack::levels / Pack::default_level -------------------------------------

fn pack_with(levels: Vec<PackLevel>, default_level: &str) -> Pack {
    Pack {
        schema_version: SCHEMA_VERSION,
        pack_id: "p".to_owned(),
        pack_version: 1,
        name: None,
        published_at: None,
        levels,
        default_level: default_level.to_owned(),
    }
}

#[test]
fn levels_are_numbered_counting_down_from_the_last() {
    let pack = pack_with(vec![level("most"), level("some"), level("none")], "most");
    let levels = pack.levels();
    assert_eq!(levels[0].id, "most");
    assert_eq!(levels[0].level, 2);
    assert_eq!(levels[1].id, "some");
    assert_eq!(levels[1].level, 1);
    assert_eq!(levels[2].id, "none");
    assert_eq!(levels[2].level, 0);
}

#[test]
fn a_single_level_pack_numbers_it_zero() {
    let pack = pack_with(vec![level("only")], "only");
    assert_eq!(pack.levels()[0].level, 0);
}

#[test]
fn default_level_resolves_the_named_id() {
    let pack = pack_with(vec![level("most"), level("some"), level("none")], "some");
    assert_eq!(pack.default_level(), 1);
}

#[test]
fn default_level_falls_back_to_the_least_reducing_level_when_the_id_is_unknown() {
    let mut pack = pack_with(vec![level("most"), level("some"), level("none")], "some");
    // Bypass `parse`'s own validation, which would refuse this — the method
    // itself must still be defensive for whatever constructs a `Pack`
    // directly.
    pack.default_level = "ghost".to_owned();
    assert_eq!(
        pack.default_level(),
        2,
        "falls back to the first, least-reducing level"
    );
}

// ---- PackPolicy -> EconomyPolicy --------------------------------------------

#[test]
fn a_pack_policy_converts_field_for_field_into_an_economy_policy() {
    let policy = PackPolicy {
        effort: Some("high".to_owned()),
        model: Some("opus".to_owned()),
        cache_ttl: Some("1h".to_owned()),
        autocompact: Some("auto".to_owned()),
        subagent_model: Some("haiku".to_owned()),
        subagent_cache_ttl: Some("5m".to_owned()),
        agent_teams: Some(true),
        cross_session_inbound: Some("hold".to_owned()),
        verbosity: Some("low".to_owned()),
        reasoning_summary: Some("concise".to_owned()),
    };
    let economy: ket_core::worktree::EconomyPolicy = (&policy).into();
    assert_eq!(economy.effort.as_deref(), Some("high"));
    assert_eq!(economy.model.as_deref(), Some("opus"));
    assert_eq!(economy.agent_teams, Some(true));
    assert_eq!(economy.verbosity.as_deref(), Some("low"));
}
