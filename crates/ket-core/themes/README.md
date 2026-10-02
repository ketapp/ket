# Themes

One file per theme. ket finds every `.toml` here when it is built
(`crates/ket-core/build.rs`), so adding a theme means adding a file. The file's
name is the theme's name in `config.toml` (`[theme] name = "acqua"`), and
`label` is what the Theme picker calls it.

A shipped theme sets **every** token. The test over `Theme::builtins()` fails
on a file that leaves one out or misspells one, and on a file that does not
clear its own contrast check (`ket_core::theme::contrast_failures`).

Your own themes go in `~/.config/ket/themes/<name>.toml`, in the same format,
and are listed in the picker after ket's. There every token is optional:
whatever a file leaves out comes from Desk or Light, according to its
`appearance`. A name that is also one of ket's gives you ket's.

The tokens, and what each one is for, are documented on `Theme` in
`crates/ket-core/src/theme.rs`. `dark.toml` (Desk) is the reference.
