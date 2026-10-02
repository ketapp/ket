//! Choosing which theme to draw with, and following the OS when asked.
//!
//! `ket-core` owns what a theme *is* — tokens, parsing, contrast checking — and
//! knows nothing about macOS. This module is the other half: it asks the platform
//! what appearance is in effect, resolves the user's preference against it, and
//! keeps doing so when the OS changes underneath.
//!
//! **Why the shell owns the OS half.** Invariant 1 puts state and computation in
//! core, and core has no business calling AppKit. So core models the *preference*
//! (`AppearancePreference::{Dark, Light, System}`) and exposes `resolve(system)`;
//! the shell supplies the `system` half. Neither knows the other's job.

use gpui::{App, WindowAppearance};
use ket_core::config::ThemeConfig;
use ket_core::theme::{Appearance, Theme};

/// What macOS is currently set to.
///
/// gpui's four values collapse to two: the vibrant variants differ in how a
/// window blends with what is behind it, not in whether the palette is light or
/// dark, and ket draws an opaque window either way.
pub(crate) fn system_appearance(appearance: WindowAppearance) -> Appearance {
    match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    }
}

/// Builds the theme to draw with, given the user's preference and the OS.
///
/// A named theme is used only when it is drawn for the resolved appearance;
/// otherwise the shipped theme for that appearance is. A named theme that will
/// not load falls back the same way rather than failing. Losing a custom palette is a visible
/// annoyance; an app that will not open because a colour file has a typo in it is
/// a much worse trade, and the same reasoning governs `Config::load`.
pub(crate) fn resolve(config: &ThemeConfig, system: Appearance) -> Theme {
    let appearance = config.appearance.resolve(system);

    let Some(name) = config.name.as_deref() else {
        return Theme::shipped(appearance);
    };

    match Theme::load(name) {
        // A named theme is drawn for one appearance. Asked for the other —
        // "System" at noon with a dark theme chosen — the shipped theme for
        // that appearance stands in, so following macOS actually follows it.
        Ok(theme) if theme.appearance == appearance => theme,
        Ok(_) => Theme::shipped(appearance),
        Err(e) => {
            tracing::warn!(%e, name, "theme did not load; using the shipped one");
            Theme::shipped(appearance)
        }
    }
}

/// The theme for the current preference, reading the OS through `cx`.
pub(crate) fn current(config: &ThemeConfig, cx: &App) -> Theme {
    resolve(config, system_appearance(cx.window_appearance()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ket_core::theme::AppearancePreference;

    fn config(appearance: AppearancePreference) -> ThemeConfig {
        ThemeConfig {
            appearance,
            name: None,
            ..ThemeConfig::default()
        }
    }

    #[test]
    fn vibrant_variants_collapse_to_their_plain_form() {
        // They differ in how a window blends with what is behind it, which is not
        // a thing ket's opaque window does.
        assert_eq!(
            system_appearance(WindowAppearance::VibrantDark),
            Appearance::Dark
        );
        assert_eq!(
            system_appearance(WindowAppearance::VibrantLight),
            Appearance::Light
        );
    }

    #[test]
    fn following_the_system_uses_what_the_system_says() {
        let theme = resolve(&config(AppearancePreference::System), Appearance::Light);
        assert_eq!(theme.appearance, Appearance::Light);
    }

    #[test]
    fn an_explicit_preference_ignores_the_system() {
        // Someone who chose dark meant dark, including at 8am when the OS did not.
        let theme = resolve(&config(AppearancePreference::Dark), Appearance::Light);
        assert_eq!(theme.appearance, Appearance::Dark);
    }

    #[test]
    fn a_named_theme_that_does_not_exist_falls_back_rather_than_failing() {
        let config = ThemeConfig {
            appearance: AppearancePreference::Dark,
            name: Some("no-such-theme-exists".to_owned()),
            ..ThemeConfig::default()
        };

        // A typo in a colour file must not be able to stop the window opening.
        let theme = resolve(&config, Appearance::Dark);
        assert_eq!(theme.appearance, Appearance::Dark);
    }
}
