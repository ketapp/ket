//! Keystrokes to the bytes a program in the pty expects.
//!
//! Only the keys that have an escape sequence or control code of their own
//! are encoded here: the named editing and navigation keys, the function
//! keys, `Ctrl`+key control codes, and the `Esc`-prefix convention for
//! `Alt`+key that shells rely on for word navigation. Plain printable text
//! is deliberately *not* — see [`keystroke_to_bytes`] — because it reaches
//! the terminal through the window's text-input path instead, which is what
//! makes dead keys, `Option`-accents and CJK composition work.
//!
//! Application cursor mode (`DECCKM`, which `vim`, `less` and `tmux` all
//! enable) switches the arrow and home/end keys from `CSI` to `SS3`
//! sequences; a modified key uses the xterm `CSI 1;m` form either way. This
//! is not the kitty keyboard protocol: a modified key that collides with an
//! unmodified one (there is no separate code for `Ctrl+Shift+A` versus
//! `Ctrl+A`) cannot be told apart on the other end.

use gpui::{Keystroke, Modifiers};
use ket_core::terminal::alacritty_terminal::term::TermMode;

/// Translates one keystroke into the bytes a program in the pty expects.
///
/// Returns `None` for two different reasons, and the caller must not need to
/// tell them apart: a key this table does not know, and plain printable
/// text, which the window delivers through its text-input path (see the
/// module docs). `Cmd` chords are refused outright — the window's own
/// shortcuts live on `cmd` (macOS), and letting those through would make
/// every cmd-chord also type into the shell.
pub(crate) fn keystroke_to_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform {
        return None;
    }

    let key = keystroke.key.as_str();
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let param = modifier_param(modifiers);

    if let Some(bytes) = named_key(key, param, app_cursor, modifiers.shift) {
        return Some(bytes);
    }

    if modifiers.control
        && let Some(byte) = control_byte(key)
    {
        return Some(vec![byte]);
    }

    if modifiers.alt {
        // `Esc` then the *unmodified* key: macOS reports `Option-b` as `∫` in
        // `key_char`, which is the accent-composition meaning of the key,
        // not the meta meaning a shell wants.
        let mut chars = key.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let ch = if modifiers.shift {
            ch.to_ascii_uppercase()
        } else {
            ch
        };
        let mut out = vec![0x1b];
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        return Some(out);
    }

    None
}

/// Whether these bytes are somebody telling an agent to stop.
///
/// `Ctrl-C` is the terminal's own interrupt; a bare escape is what Claude
/// Code takes to cancel a turn, and Codex with it. *Bare* is the whole of the
/// test: an arrow key is an escape too, but it reaches the pty as the whole
/// of `ESC [ A` in a single write, so matching one byte on its own separates
/// them without parsing anything.
///
/// Treated downstream as what each one is — see [`ket_core::agent_status`]:
/// `Ctrl-C` a cancel verdict held against the turn's late reports, Escape a
/// guess the next report replaces. Neither acts unless there is a turn in
/// flight in that very pane.
pub(crate) fn is_interrupt(bytes: &[u8]) -> bool {
    matches!(bytes, [0x03] | [0x1b])
}

/// The xterm modifier parameter: `1` plus shift (1), alt (2) and control
/// (4), or `None` when no modifier is held.
fn modifier_param(modifiers: &Modifiers) -> Option<u8> {
    let bits = u8::from(modifiers.shift)
        | (u8::from(modifiers.alt) << 1)
        | (u8::from(modifiers.control) << 2);
    (bits > 0).then_some(1 + bits)
}

/// The named editing, navigation and function keys.
fn named_key(key: &str, param: Option<u8>, app_cursor: bool, shift: bool) -> Option<Vec<u8>> {
    // Cursor keys: `CSI A` normally, `SS3 A` in application cursor mode, and
    // `CSI 1;m A` with any modifier regardless of the mode.
    let cursor = |letter: u8| -> Vec<u8> {
        match param {
            Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
            None if app_cursor => vec![0x1b, b'O', letter],
            None => vec![0x1b, b'[', letter],
        }
    };
    // Tilde keys: `CSI n ~`, or `CSI n;m ~` with a modifier.
    let tilde = |code: u8| -> Vec<u8> {
        match param {
            Some(m) => format!("\x1b[{code};{m}~").into_bytes(),
            None => format!("\x1b[{code}~").into_bytes(),
        }
    };
    // The first four function keys are `SS3` letters, the rest tilde keys.
    let ss3 = |letter: u8| -> Vec<u8> {
        match param {
            Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
            None => vec![0x1b, b'O', letter],
        }
    };

    Some(match key {
        "enter" | "return" => b"\r".to_vec(),
        "backspace" => match param {
            // `Alt-Backspace` deletes a word in readline and zsh.
            Some(m) if m & 0b10 != 0 => vec![0x1b, 0x7f],
            _ => vec![0x7f],
        },
        "delete" => tilde(3),
        "insert" => tilde(2),
        "tab" if shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => vec![0x1b],
        "up" => cursor(b'A'),
        "down" => cursor(b'B'),
        "right" => cursor(b'C'),
        "left" => cursor(b'D'),
        "home" => cursor(b'H'),
        "end" => cursor(b'F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => ss3(b'P'),
        "f2" => ss3(b'Q'),
        "f3" => ss3(b'R'),
        "f4" => ss3(b'S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        _ => return None,
    })
}

/// The control byte for `Ctrl`+`key`, for the keys a shell actually uses:
/// the letters, plus the handful of punctuation keys with their own
/// long-standing control codes (`Ctrl-[` for escape, `Ctrl-?` for delete...).
fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    match ch.to_ascii_lowercase() {
        'a'..='z' => Some(ch.to_ascii_uppercase() as u8 - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '-' => Some(0x1f),
        '?' => Some(0x7f),
        ' ' => Some(0x00),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    fn with(mut keystroke: Keystroke, modifiers: Modifiers) -> Keystroke {
        keystroke.modifiers = modifiers;
        keystroke
    }

    #[test]
    fn plain_text_is_left_to_the_text_input_path() {
        assert_eq!(
            keystroke_to_bytes(&key("a", Some("a")), TermMode::NONE),
            None
        );
    }

    #[test]
    fn ctrl_c_sends_the_interrupt_byte() {
        let modifiers = Modifiers {
            control: true,
            ..Default::default()
        };
        let keystroke = with(key("c", Some("c")), modifiers);
        assert_eq!(
            keystroke_to_bytes(&keystroke, TermMode::NONE),
            Some(vec![0x03])
        );
    }

    #[test]
    fn a_cmd_chord_is_never_sent_to_the_program() {
        let modifiers = Modifiers {
            platform: true,
            ..Default::default()
        };
        let keystroke = with(key("k", Some("k")), modifiers);
        assert_eq!(keystroke_to_bytes(&keystroke, TermMode::NONE), None);
    }

    #[test]
    fn enter_sends_carriage_return() {
        assert_eq!(
            keystroke_to_bytes(&key("enter", None), TermMode::NONE),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn arrow_keys_send_csi_normally_and_ss3_in_application_cursor_mode() {
        assert_eq!(
            keystroke_to_bytes(&key("up", None), TermMode::NONE),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&key("up", None), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn a_modified_arrow_uses_the_xterm_parameter_form_in_either_mode() {
        let modifiers = Modifiers {
            alt: true,
            ..Default::default()
        };
        let keystroke = with(key("right", None), modifiers);
        assert_eq!(
            keystroke_to_bytes(&keystroke, TermMode::APP_CURSOR),
            Some(b"\x1b[1;3C".to_vec())
        );
    }

    #[test]
    fn alt_prefixes_an_escape_the_way_shells_expect_for_meta() {
        let modifiers = Modifiers {
            alt: true,
            ..Default::default()
        };
        // macOS reports the accent meaning in `key_char`; the meta meaning
        // wants the key itself.
        let keystroke = with(key("b", Some("∫")), modifiers);
        assert_eq!(
            keystroke_to_bytes(&keystroke, TermMode::NONE),
            Some(b"\x1bb".to_vec())
        );
    }

    #[test]
    fn alt_backspace_deletes_a_word() {
        let modifiers = Modifiers {
            alt: true,
            ..Default::default()
        };
        let keystroke = with(key("backspace", None), modifiers);
        assert_eq!(
            keystroke_to_bytes(&keystroke, TermMode::NONE),
            Some(vec![0x1b, 0x7f])
        );
    }

    #[test]
    fn function_keys_split_between_ss3_and_tilde_forms() {
        assert_eq!(
            keystroke_to_bytes(&key("f1", None), TermMode::NONE),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&key("f5", None), TermMode::NONE),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn control_byte_covers_the_letters_and_the_punctuation_shells_use() {
        assert_eq!(control_byte("c"), Some(0x03));
        assert_eq!(control_byte("C"), Some(0x03));
        assert_eq!(control_byte("["), Some(0x1b));
        assert_eq!(control_byte("?"), Some(0x7f));
        assert_eq!(control_byte(" "), Some(0x00));
    }

    #[test]
    fn control_byte_refuses_anything_that_is_not_one_character() {
        assert_eq!(control_byte("up"), None);
        assert_eq!(control_byte(""), None);
    }
}
