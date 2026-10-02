//! Theme parsing, fallback, and the WCAG contrast bar the shipped themes must
//! clear.
//!
//! The two things worth failing loudly for are opposite mistakes: a theme
//! file with a typo'd token name should refuse to load and say which token,
//! while a theme file simply missing a token should load fine and borrow the
//! default. Getting these backwards — silently ignoring a typo, or refusing
//! to load over an omission — is worse than either mistake alone, so both
//! directions are asserted here rather than just one.

use ket_core::KetError;
use ket_core::theme::{
    Appearance, AppearancePreference, Color, Prominence, Theme, contrast_failures, contrast_ratio,
};

/// Unique-enough temp path per test, so parallel test runs do not collide.
fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("ket-theme-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}-{:?}.toml", std::thread::current().id()))
}

#[test]
fn black_on_white_is_the_textbook_ratio() {
    // The one fixture everyone can check by hand: WCAG's own formula gives
    // exactly 21:1 for the two extremes.
    let black = Color::new(0, 0, 0);
    let white = Color::new(255, 255, 255);
    assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-9);
    // Order must not matter — the ratio is a property of the pair.
    assert!((contrast_ratio(white, black) - 21.0).abs() < 1e-9);
}

#[test]
fn identical_colours_have_no_contrast() {
    let c = Color::new(100, 150, 200);
    assert!((contrast_ratio(c, c) - 1.0).abs() < 1e-9);
}

#[test]
fn hex_parsing_accepts_the_leading_hash_or_not() {
    assert_eq!(Color::from_hex("#ff00aa").unwrap(), Color::new(255, 0, 170));
    assert_eq!(Color::from_hex("ff00aa").unwrap(), Color::new(255, 0, 170));
    assert_eq!(Color::from_hex("#FF00AA").unwrap(), Color::new(255, 0, 170));
}

#[test]
fn hex_parsing_round_trips() {
    let c = Color::new(18, 52, 86);
    assert_eq!(Color::from_hex(&c.to_hex()).unwrap(), c);
}

#[test]
fn bad_hex_is_a_clear_error_naming_the_bad_text() {
    for bad in ["not-a-colour", "#ff00", "#ff00aabb", "#gggggg", ""] {
        let err = Color::from_hex(bad).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains(bad) || bad.is_empty(),
            "error for {bad:?} did not name it: {message}"
        );
    }
}

#[test]
fn both_shipped_themes_clear_wcag_aa() {
    for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let failures = ket_core::theme::contrast_failures(&theme);
        assert!(
            failures.is_empty(),
            "{name} theme fails its own contrast check: {failures:?}"
        );
    }
}

#[test]
fn shipped_themes_carry_their_own_appearance() {
    assert_eq!(Theme::dark().appearance, Appearance::Dark);
    assert_eq!(Theme::light().appearance, Appearance::Light);
}

#[test]
fn an_empty_theme_file_is_the_shipped_dark_theme() {
    assert_eq!(Theme::from_toml_str("").unwrap(), Theme::dark());
}

#[test]
fn appearance_alone_selects_the_light_defaults() {
    assert_eq!(
        Theme::from_toml_str("appearance = \"light\"\n").unwrap(),
        Theme::light()
    );
}

#[test]
fn one_overridden_token_leaves_every_other_token_at_the_default() {
    let theme = Theme::from_toml_str("surface = \"#ff00ff\"\n").unwrap();
    assert_eq!(theme.surface, Color::from_hex("#ff00ff").unwrap());

    // Nothing else moved: same as the dark default in every other field.
    let mut expected = Theme::dark();
    expected.surface = Color::from_hex("#ff00ff").unwrap();
    assert_eq!(theme, expected);
}

#[test]
fn a_nested_override_leaves_its_siblings_at_the_default() {
    let theme =
        Theme::from_toml_str("[diff]\nadded = \"#00ff00\"\n\n[terminal.ansi]\nred = \"#ff0000\"\n")
            .unwrap();

    assert_eq!(theme.diff.added, Color::from_hex("#00ff00").unwrap());
    assert_eq!(theme.diff.removed, Theme::dark().diff.removed);
    assert_eq!(theme.terminal.ansi.red, Color::from_hex("#ff0000").unwrap());
    assert_eq!(theme.terminal.ansi.blue, Theme::dark().terminal.ansi.blue);
}

#[test]
fn a_typo_in_a_top_level_token_names_itself_in_the_error() {
    let err = Theme::from_toml_str("sruface = \"#ffffff\"\n").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("sruface"),
        "error did not name the bad token: {message}"
    );
}

#[test]
fn a_typo_in_a_nested_token_names_itself_in_the_error() {
    let err = Theme::from_toml_str("[text]\nprimray = \"#ffffff\"\n").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("primray"),
        "error did not name the bad token: {message}"
    );
}

#[test]
fn a_typo_in_a_doubly_nested_token_names_itself_in_the_error() {
    let err = Theme::from_toml_str("[terminal.ansi]\nrde = \"#ffffff\"\n").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("rde"),
        "error did not name the bad token: {message}"
    );
}

#[test]
fn an_invalid_colour_value_is_an_error_not_a_panic() {
    let result = Theme::from_toml_str("surface = \"not-a-colour\"\n");
    assert!(matches!(result, Err(KetError::Config(_))), "{result:?}");
}

#[test]
fn a_missing_theme_file_is_the_shipped_dark_theme() {
    let path = std::path::Path::new("/nonexistent/ket/themes/whatever.toml");
    assert_eq!(Theme::load_from(path).unwrap(), Theme::dark());
}

#[test]
fn a_present_but_malformed_theme_file_is_an_error() {
    let path = temp_path("malformed");
    std::fs::write(&path, "this is not = = toml").unwrap();

    let result = Theme::load_from(&path);
    std::fs::remove_file(&path).ok();

    assert!(matches!(result, Err(KetError::Config(_))), "{result:?}");
}

#[test]
fn loading_a_file_with_one_override_merges_onto_the_default() {
    let path = temp_path("partial");
    std::fs::write(&path, "[status]\nfailed = \"#ab00cd\"\n").unwrap();

    let theme = Theme::load_from(&path).unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(theme.status.failed, Color::from_hex("#ab00cd").unwrap());
    assert_eq!(theme.status.running, Theme::dark().status.running);
}

#[test]
fn a_full_theme_round_trips_through_toml() {
    for theme in [Theme::dark(), Theme::light()] {
        let text = toml::to_string_pretty(&theme).unwrap();
        let parsed = Theme::from_toml_str(&text).unwrap();
        assert_eq!(parsed, theme);
    }
}

#[test]
fn appearance_preference_resolves_dark_and_light_unconditionally() {
    assert_eq!(
        AppearancePreference::Dark.resolve(Appearance::Light),
        Appearance::Dark
    );
    assert_eq!(
        AppearancePreference::Light.resolve(Appearance::Dark),
        Appearance::Light
    );
}

#[test]
fn appearance_preference_system_defers_to_whatever_it_is_given() {
    assert_eq!(
        AppearancePreference::System.resolve(Appearance::Dark),
        Appearance::Dark
    );
    assert_eq!(
        AppearancePreference::System.resolve(Appearance::Light),
        Appearance::Light
    );
}

#[test]
fn appearance_preference_defaults_to_dark() {
    // Deferring to the OS is the more polite default and is still one setting
    // away, but ket is read for hours at a time and dark is the palette it is
    // designed and contrast-checked against. `System` handed a light-mode Mac
    // the light palette on first run without anyone having chosen it.
    assert_eq!(AppearancePreference::default(), AppearancePreference::Dark);
}

#[test]
fn following_the_system_is_still_available_and_still_works() {
    // Changing the default must not quietly cost the behaviour it replaced.
    assert_eq!(
        AppearancePreference::System.resolve(Appearance::Light),
        Appearance::Light
    );
    assert_eq!(
        AppearancePreference::System.resolve(Appearance::Dark),
        Appearance::Dark
    );
}

#[test]
fn ansi_index_wraps_at_sixteen() {
    let theme = Theme::dark();
    assert_eq!(theme.terminal.ansi.get(0), theme.terminal.ansi.black);
    assert_eq!(
        theme.terminal.ansi.get(15),
        theme.terminal.ansi.bright_white
    );
    assert_eq!(theme.terminal.ansi.get(16), theme.terminal.ansi.black);
    assert_eq!(theme.terminal.ansi.get(17), theme.terminal.ansi.red);
}

// ---- mix, Display, Prominence ------------------------------------------------

#[test]
fn mix_at_zero_is_self_and_at_one_is_the_other() {
    let a = Color::new(0, 0, 0);
    let b = Color::new(200, 100, 50);
    assert_eq!(a.mix(b, 0.0), a);
    assert_eq!(a.mix(b, 1.0), b);
}

#[test]
fn mix_clamps_amount_outside_zero_to_one() {
    let a = Color::new(0, 0, 0);
    let b = Color::new(200, 100, 50);
    assert_eq!(a.mix(b, -5.0), a);
    assert_eq!(a.mix(b, 5.0), b);
}

#[test]
fn mix_halfway_averages_each_channel() {
    let a = Color::new(0, 0, 0);
    let b = Color::new(100, 200, 10);
    assert_eq!(a.mix(b, 0.5), Color::new(50, 100, 5));
}

#[test]
fn color_display_matches_to_hex() {
    let c = Color::new(18, 52, 86);
    assert_eq!(c.to_string(), c.to_hex());
    assert_eq!(c.to_string(), "#123456");
}

#[test]
fn prominence_minimum_ratios_match_wcag_aa() {
    assert_eq!(Prominence::Body.minimum_ratio(), 4.5);
    assert_eq!(Prominence::Accent.minimum_ratio(), 3.0);
}

#[test]
fn contrast_failure_display_names_the_pair_and_both_ratios() {
    let failures = contrast_failures(&Theme::dark());
    // The shipped theme passes, so fabricate a failure shape to check the
    // message rather than depend on one existing.
    let message = format!(
        "{}",
        ket_core::theme::ContrastFailure {
            foreground: "text.primary",
            background: "surface",
            ratio: 2.345,
            required: 4.5,
        }
    );
    assert_eq!(message, "text.primary on surface is 2.35:1, needs 4.5:1");
    assert!(failures.is_empty());
}

// ---- every shipped theme, not just dark and light -----------------------------

#[test]
fn every_builtin_theme_clears_wcag_aa() {
    for entry in Theme::builtins() {
        let (name, label) = (&entry.name, &entry.label);
        let theme = Theme::builtin(name).unwrap_or_else(|| panic!("{name} ({label}) not builtin"));
        let failures = contrast_failures(&theme);
        assert!(
            failures.is_empty(),
            "{name} ({label}) fails its own contrast check: {failures:?}"
        );
    }
}

#[test]
fn every_builtin_theme_name_resolves_through_builtin() {
    for entry in Theme::builtins() {
        assert!(
            Theme::builtin(&entry.name).is_some(),
            "{} did not resolve",
            entry.name
        );
    }
    // A shipped file that does not parse is left out at run time; here it
    // is a failure. One entry per file under `themes/`.
    let files = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/themes"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "toml"))
        })
        .count();
    assert_eq!(
        Theme::builtins().len(),
        files,
        "a shipped theme file did not parse"
    );
}

#[test]
fn an_unknown_name_is_not_builtin() {
    assert!(Theme::builtin("not-a-real-theme").is_none());
}

#[test]
fn azure_is_an_alias_for_acqua() {
    assert_eq!(Theme::builtin("azure"), Theme::builtin("acqua"));
    assert!(Theme::builtin("azure").is_some());
}

#[test]
fn shipped_selects_by_appearance() {
    assert_eq!(Theme::shipped(Appearance::Dark), Theme::dark());
    assert_eq!(Theme::shipped(Appearance::Light), Theme::light());
}

#[test]
fn load_prefers_a_builtin_name_over_a_file_of_the_same_name() {
    // "dark" is builtin, so `load` must not even look for a theme file named
    // dark.toml — there is no filesystem fixture to find one in this test,
    // and a working answer proves the builtin table won.
    assert_eq!(Theme::load("dark").unwrap(), Theme::dark());
    assert_eq!(Some(Theme::load("acqua").unwrap()), Theme::builtin("acqua"));
}

#[test]
fn on_backdrop_leaves_a_theme_alone_when_it_already_clears_the_bar() {
    // The shipped dark theme's text.dim already clears body contrast against
    // its backdrop, so on_backdrop should be a no-op.
    let theme = Theme::dark();
    assert_eq!(theme.on_backdrop(), theme);
}

#[test]
fn on_backdrop_rewrites_text_when_it_would_not_clear_the_bar() {
    let mut theme = Theme::dark();
    // Force a backdrop indistinguishable from the dim text colour, so the
    // pair fails body contrast and on_backdrop has to intervene.
    theme.backdrop = theme.text.dim;

    let adjusted = theme.on_backdrop();
    assert_eq!(adjusted.text.primary, theme.surface);
    assert_ne!(adjusted.text.dim, theme.text.dim);
}
