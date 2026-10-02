//! Keybinding parsing, chords, context precedence, and conflict detection.
//!
//! The two failure modes worth telling apart, the same way `tests/theme.rs`
//! tells apart a typo'd token from an omitted one: a file with a genuine
//! mistake — an unknown command id, an unparseable key, two bindings racing
//! for the same chord — must refuse to load and name the mistake, while a
//! file that only overrides one binding must leave every other default
//! binding intact. Getting either backwards (silently dropping the bad
//! binding, or losing the rest of the defaults over one override) is worse
//! than either mistake alone, so both directions are asserted here.

use ket_core::KetError;
use ket_core::command::Registry;
use ket_core::keybinding::{Chord, Keymap, Keystroke, Resolution};

#[test]
fn a_plain_keystroke_parses_with_no_modifiers() {
    let k = Keystroke::parse("k").unwrap();
    assert!(!k.ctrl && !k.alt && !k.shift && !k.cmd);
    assert_eq!(k.key, "k");
}

#[test]
fn modifiers_parse_regardless_of_order_or_case() {
    let a = Keystroke::parse("ctrl+shift+n").unwrap();
    let b = Keystroke::parse("Shift+Ctrl+N").unwrap();
    assert_eq!(a, b);
    assert!(a.ctrl && a.shift && !a.alt && !a.cmd);
    assert_eq!(a.key, "n");
}

#[test]
fn cmd_super_and_meta_are_the_same_modifier() {
    for text in ["cmd+k", "meta+k", "super+k", "command+k"] {
        let k = Keystroke::parse(text).unwrap();
        assert!(k.cmd, "{text} did not set cmd");
    }
}

#[test]
fn named_keys_and_function_keys_parse() {
    for text in ["enter", "escape", "up", "backspace", "f1", "f24"] {
        assert!(Keystroke::parse(text).is_ok(), "{text} should parse");
    }
}

#[test]
fn an_unparseable_key_name_is_a_clear_error_not_a_skip() {
    // The failure this whole module refuses to hide: a typo in a key name
    // must stop the load and say what it did not understand, rather than
    // quietly binding nothing.
    let err = Keystroke::parse("ctrl+zzzz").unwrap_err();
    let message = err.to_string();
    assert!(matches!(err, KetError::Config(_)));
    assert!(message.contains("zzzz"), "error did not name it: {message}");
}

#[test]
fn an_unknown_modifier_is_also_a_clear_error() {
    let err = Keystroke::parse("cntrl+k").unwrap_err();
    assert!(matches!(err, KetError::Config(_)));
    assert!(err.to_string().contains("cntrl"));
}

#[test]
fn an_empty_key_after_a_trailing_plus_is_an_error() {
    assert!(Keystroke::parse("ctrl+").is_err());
    assert!(Keystroke::parse("").is_err());
}

#[test]
fn a_chord_is_keystrokes_split_on_whitespace() {
    let chord = Chord::parse("ctrl+k ctrl+p").unwrap();
    assert_eq!(chord.len(), 2);
    assert_eq!(chord.to_string(), "ctrl+k ctrl+p");
}

#[test]
fn a_single_keystroke_is_a_one_long_chord() {
    let chord = Chord::parse("ctrl+w").unwrap();
    assert_eq!(chord.len(), 1);
    assert!(!chord.is_empty());
}

#[test]
fn chord_display_round_trips_through_parse() {
    for text in ["k", "ctrl+w", "ctrl+k ctrl+p", "ctrl+shift+backspace"] {
        let chord = Chord::parse(text).unwrap();
        assert_eq!(Chord::parse(&chord.to_string()).unwrap(), chord);
    }
}

#[test]
fn a_bad_keystroke_inside_a_chord_fails_the_whole_chord() {
    assert!(Chord::parse("ctrl+k ctrl+zzzz").is_err());
}

#[test]
fn prefix_relation_is_strict_and_order_sensitive() {
    let short = Chord::parse("ctrl+k").unwrap();
    let long = Chord::parse("ctrl+k ctrl+p").unwrap();
    let other = Chord::parse("ctrl+k ctrl+q").unwrap();

    assert!(short.is_prefix_of(&long));
    assert!(!long.is_prefix_of(&short));
    assert!(!short.is_prefix_of(&short), "a chord is not its own prefix");
    assert!(!long.is_prefix_of(&long));
    assert!(!Chord::parse("ctrl+q").unwrap().is_prefix_of(&other));
}

#[test]
fn the_shipped_defaults_validate_against_the_builtin_registry() {
    // Proves the mechanism end to end: every id `Keymap::builtin` names is
    // real, and the defaults do not collide with themselves.
    let registry = Registry::builtin();
    Keymap::builtin().validate(&registry).unwrap();
}

#[test]
fn a_chord_the_registry_does_not_know_fires_nothing() {
    let keymap = Keymap::builtin();
    let chord = Chord::parse("ctrl+k ctrl+z").unwrap();
    assert_eq!(keymap.resolve("global", &chord), Resolution::NoMatch);
}

#[test]
fn a_chord_prefix_is_pending_not_fired_and_not_a_miss() {
    let keymap = Keymap::builtin();
    let prefix = Chord::parse("ctrl+k").unwrap();
    assert_eq!(keymap.resolve("global", &prefix), Resolution::Pending);

    let full = Chord::parse("ctrl+k ctrl+p").unwrap();
    assert_eq!(
        keymap.resolve("global", &full),
        Resolution::Fired("app.commands".into())
    );
}

#[test]
fn a_context_specific_binding_shadows_the_same_chord_globally() {
    // `ctrl+w` means something different with a terminal focused than it
    // does everywhere else — the whole reason contexts exist rather than a
    // single flat map from chord to command.
    let keymap = Keymap::builtin();
    let chord = Chord::parse("ctrl+w").unwrap();

    assert_eq!(
        keymap.resolve("global", &chord),
        Resolution::Fired("worktree.close".into())
    );
    assert_eq!(
        keymap.resolve("terminal", &chord),
        Resolution::Fired("worktree.status".into())
    );
    // A pane with no override of its own still falls back to global.
    assert_eq!(
        keymap.resolve("diff", &chord),
        Resolution::Fired("worktree.close".into())
    );
}

#[test]
fn overriding_one_binding_leaves_every_other_default_intact() {
    // The property the whole override-not-replace design exists for:
    // someone who rebinds `ctrl+n` should not lose `ctrl+l`, the `ctrl+k
    // ctrl+p` chord, or the terminal-specific `ctrl+w`.
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        chord = "ctrl+n"
        command = "project.add"
    "#;
    let keymap = Keymap::from_toml_str(text, &registry).unwrap();

    assert_eq!(
        keymap.resolve("global", &Chord::parse("ctrl+n").unwrap()),
        Resolution::Fired("project.add".into()),
        "the override itself should take effect"
    );
    assert_eq!(
        keymap.resolve("global", &Chord::parse("ctrl+l").unwrap()),
        Resolution::Fired("worktree.list".into()),
        "an untouched default should survive"
    );
    assert_eq!(
        keymap.resolve("global", &Chord::parse("ctrl+k ctrl+p").unwrap()),
        Resolution::Fired("app.commands".into()),
        "the default chord should survive"
    );
    assert_eq!(
        keymap.resolve("terminal", &Chord::parse("ctrl+w").unwrap()),
        Resolution::Fired("worktree.status".into()),
        "a context-specific default should survive"
    );
}

#[test]
fn a_binding_may_target_a_context_other_than_global() {
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        context = "diff"
        chord = "ctrl+shift+x"
        command = "worktree.diff"
    "#;
    let keymap = Keymap::from_toml_str(text, &registry).unwrap();

    let chord = Chord::parse("ctrl+shift+x").unwrap();
    assert_eq!(
        keymap.resolve("diff", &chord),
        Resolution::Fired("worktree.diff".into())
    );
    // A binding scoped to "diff" must not leak into an unrelated context.
    assert_eq!(keymap.resolve("global", &chord), Resolution::NoMatch);
}

#[test]
fn two_user_bindings_for_the_same_chord_in_the_same_context_is_a_conflict() {
    // Not an override — nothing in the defaults occupies this chord — just
    // two entries in one file racing for it. Last-write-wins would pick one
    // silently; this must fail and name both.
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        context = "terminal"
        chord = "ctrl+x"
        command = "worktree.list"

        [[binding]]
        context = "terminal"
        chord = "ctrl+x"
        command = "worktree.diff"
    "#;

    let err = Keymap::from_toml_str(text, &registry).unwrap_err();
    assert!(matches!(err, KetError::Conflict(_)));
    let message = err.to_string();
    assert!(message.contains("worktree.list"), "{message}");
    assert!(message.contains("worktree.diff"), "{message}");
}

#[test]
fn a_chord_that_is_a_prefix_of_another_in_the_same_context_is_a_conflict() {
    // The subtle case named in the plan: `ctrl+x` firing immediately would
    // make `ctrl+x ctrl+s` unreachable, and vice versa if `ctrl+x` waited.
    // Neither resolution is right, so this is rejected outright.
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        context = "terminal"
        chord = "ctrl+x"
        command = "worktree.list"

        [[binding]]
        context = "terminal"
        chord = "ctrl+x ctrl+s"
        command = "worktree.diff"
    "#;

    let err = Keymap::from_toml_str(text, &registry).unwrap_err();
    assert!(matches!(err, KetError::Conflict(_)));
    let message = err.to_string();
    assert!(message.contains("worktree.list"), "{message}");
    assert!(message.contains("worktree.diff"), "{message}");
}

#[test]
fn a_prefix_conflict_reaches_across_a_context_and_the_global_fallback() {
    // The same hazard as above, but the two halves live in different
    // layers: one binding is scoped to "terminal", the other inherited from
    // "global". Both are reachable with a terminal pane focused, so the
    // collision is just as real.
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        context = "global"
        chord = "ctrl+y"
        command = "worktree.list"

        [[binding]]
        context = "terminal"
        chord = "ctrl+y ctrl+z"
        command = "worktree.diff"
    "#;

    let err = Keymap::from_toml_str(text, &registry).unwrap_err();
    assert!(matches!(err, KetError::Conflict(_)));
}

#[test]
fn an_unknown_command_id_is_rejected_and_named() {
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        chord = "ctrl+y"
        command = "nope.nope"
    "#;

    let err = Keymap::from_toml_str(text, &registry).unwrap_err();
    assert!(matches!(err, KetError::Conflict(_)));
    assert!(err.to_string().contains("nope.nope"));
}

#[test]
fn an_unparseable_chord_in_the_file_fails_the_whole_load() {
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        chord = "ctrl+n"
        command = "worktree.create"

        [[binding]]
        chord = "ctrl+zzzz"
        command = "worktree.list"
    "#;

    let err = Keymap::from_toml_str(text, &registry).unwrap_err();
    assert!(matches!(err, KetError::Config(_)));
    assert!(err.to_string().contains("zzzz"));
}

#[test]
fn an_unknown_top_level_key_is_rejected() {
    // Guards the same class of typo `config.rs` and `theme.rs` guard against:
    // a misspelled section should not be silently ignored.
    let registry = Registry::builtin();
    let result = Keymap::from_toml_str("bindings = []", &registry);
    assert!(result.is_err());
}

#[test]
fn a_missing_file_yields_the_shipped_defaults() {
    let registry = Registry::builtin();
    let keymap = Keymap::load_from(
        std::path::Path::new("/nonexistent/ket/keybindings.toml"),
        &registry,
    )
    .unwrap();

    assert_eq!(
        keymap.resolve("global", &Chord::parse("ctrl+l").unwrap()),
        Resolution::Fired("worktree.list".into())
    );
}

#[test]
fn continuations_reports_what_could_follow_a_pending_chord() {
    let keymap = Keymap::builtin();
    let prefix = Chord::parse("ctrl+k").unwrap();
    let continuations = keymap.continuations("global", &prefix);

    assert_eq!(continuations.len(), 1);
    assert_eq!(continuations[0].command.as_str(), "app.commands");
}

#[test]
fn context_default_is_global_when_omitted() {
    let registry = Registry::builtin();
    let text = r#"
        [[binding]]
        chord = "ctrl+shift+z"
        command = "worktree.prune"
    "#;
    let keymap = Keymap::from_toml_str(text, &registry).unwrap();

    assert_eq!(
        keymap.resolve("global", &Chord::parse("ctrl+shift+z").unwrap()),
        Resolution::Fired("worktree.prune".into())
    );
}
