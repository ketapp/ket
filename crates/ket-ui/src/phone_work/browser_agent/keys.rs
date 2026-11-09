//! Keys as `ket browser press` names them — `Enter`, `Shift+Tab`, `Meta+A`
//! — and `type --keys` types them, as the DevTools protocol sends them: the
//! `key` a page reads, the `code` of the key on a US keyboard, the Windows
//! virtual key code pages still read as `keyCode`, the text it types, and,
//! on a Mac, the editing commands the system would turn it into — without
//! them ⌘A selects nothing and ⌥⌫ deletes a character, not a word.

/// A key, as the protocol sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Key {
    /// What it means: `Enter`, `a`, `A`, `Shift`.
    pub key: String,
    /// Which key it is: `Enter`, `KeyA`, `ShiftLeft`. Empty for a character
    /// no US key types.
    pub code: String,
    /// Windows' virtual key code; `0` for a character no US key types.
    pub vk: i64,
    /// What it types: `\r` for Enter, nothing for Tab or an arrow.
    pub text: String,
}

/// A modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Modifier {
    Alt,
    Control,
    Meta,
    Shift,
}

impl Modifier {
    /// Its bit in the protocol's `modifiers`.
    pub(super) fn bit(self) -> i64 {
        match self {
            Self::Alt => 1,
            Self::Control => 2,
            Self::Meta => 4,
            Self::Shift => 8,
        }
    }

    /// The key itself, the left one.
    pub(super) fn key(self) -> Key {
        let (key, code, vk) = match self {
            Self::Alt => ("Alt", "AltLeft", 18),
            Self::Control => ("Control", "ControlLeft", 17),
            Self::Meta => ("Meta", "MetaLeft", 91),
            Self::Shift => ("Shift", "ShiftLeft", 16),
        };
        Key {
            key: key.to_owned(),
            code: code.to_owned(),
            vk,
            text: String::new(),
        }
    }

    /// The modifier `key` is, if it is one.
    pub(super) fn of(key: &Key) -> Option<Self> {
        [Self::Alt, Self::Control, Self::Meta, Self::Shift]
            .into_iter()
            .find(|modifier| modifier.key().key == key.key)
    }

    fn named(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "alt" | "option" | "opt" => Self::Alt,
            "control" | "ctrl" => Self::Control,
            "meta" | "cmd" | "command" | "super" => Self::Meta,
            "shift" => Self::Shift,
            _ => return None,
        })
    }
}

/// A key pressed with modifiers held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Chord {
    /// Held down first, in this order, and let go last, in the other.
    pub modifiers: Vec<Modifier>,
    /// The key — its text gone when a modifier other than Shift is held,
    /// since ⌘A types nothing.
    pub key: Key,
}

impl Chord {
    /// The protocol's `modifiers` with all of them held.
    pub(super) fn held(&self) -> i64 {
        self.modifiers
            .iter()
            .fold(0, |held, modifier| held | modifier.bit())
    }

    /// The editing commands macOS makes of this chord, as Chromium names
    /// them. Commands that insert text are left out: the key's own text
    /// inserts it.
    pub(super) fn commands(&self) -> &'static [&'static str] {
        let held = self.held();
        MAC_COMMANDS
            .iter()
            .find(|(modifiers, code, _)| *modifiers == held && *code == self.key.code)
            .map_or(&[], |(_, _, commands)| commands)
    }
}

const SHIFT: i64 = 8;
const ALT: i64 = 1;
const META: i64 = 4;

/// What macOS's key bindings make of a chord, by its modifiers and the
/// key's code: the editing commands Chromium runs for it, as Playwright
/// sends them on a Mac.
const MAC_COMMANDS: &[(i64, &str, &[&str])] = &[
    (0, "Backspace", &["deleteBackward"]),
    (0, "Delete", &["deleteForward"]),
    (0, "Escape", &["cancelOperation"]),
    (0, "ArrowUp", &["moveUp"]),
    (0, "ArrowDown", &["moveDown"]),
    (0, "ArrowLeft", &["moveLeft"]),
    (0, "ArrowRight", &["moveRight"]),
    (0, "Home", &["scrollToBeginningOfDocument"]),
    (0, "End", &["scrollToEndOfDocument"]),
    (0, "PageUp", &["scrollPageUp"]),
    (0, "PageDown", &["scrollPageDown"]),
    (SHIFT, "Backspace", &["deleteBackward"]),
    (SHIFT, "ArrowUp", &["moveUpAndModifySelection"]),
    (SHIFT, "ArrowDown", &["moveDownAndModifySelection"]),
    (SHIFT, "ArrowLeft", &["moveLeftAndModifySelection"]),
    (SHIFT, "ArrowRight", &["moveRightAndModifySelection"]),
    (
        SHIFT,
        "Home",
        &["moveToBeginningOfDocumentAndModifySelection"],
    ),
    (SHIFT, "End", &["moveToEndOfDocumentAndModifySelection"]),
    (SHIFT, "PageUp", &["pageUpAndModifySelection"]),
    (SHIFT, "PageDown", &["pageDownAndModifySelection"]),
    (ALT, "Backspace", &["deleteWordBackward"]),
    (ALT, "Delete", &["deleteWordForward"]),
    (
        ALT,
        "ArrowUp",
        &["moveBackward", "moveToBeginningOfParagraph"],
    ),
    (ALT, "ArrowDown", &["moveForward", "moveToEndOfParagraph"]),
    (ALT, "ArrowLeft", &["moveWordLeft"]),
    (ALT, "ArrowRight", &["moveWordRight"]),
    (
        SHIFT | ALT,
        "ArrowLeft",
        &["moveWordLeftAndModifySelection"],
    ),
    (
        SHIFT | ALT,
        "ArrowRight",
        &["moveWordRightAndModifySelection"],
    ),
    (META, "Backspace", &["deleteToBeginningOfLine"]),
    (META, "ArrowUp", &["moveToBeginningOfDocument"]),
    (META, "ArrowDown", &["moveToEndOfDocument"]),
    (META, "ArrowLeft", &["moveToLeftEndOfLine"]),
    (META, "ArrowRight", &["moveToRightEndOfLine"]),
    (
        SHIFT | META,
        "ArrowUp",
        &["moveToBeginningOfDocumentAndModifySelection"],
    ),
    (
        SHIFT | META,
        "ArrowDown",
        &["moveToEndOfDocumentAndModifySelection"],
    ),
    (
        SHIFT | META,
        "ArrowLeft",
        &["moveToLeftEndOfLineAndModifySelection"],
    ),
    (
        SHIFT | META,
        "ArrowRight",
        &["moveToRightEndOfLineAndModifySelection"],
    ),
    (META, "KeyA", &["selectAll"]),
    (META, "KeyC", &["copy"]),
    (META, "KeyX", &["cut"]),
    (META, "KeyV", &["paste"]),
    (META, "KeyZ", &["undo"]),
    (SHIFT | META, "KeyZ", &["redo"]),
];

/// Keys known by name: the name, `code`, virtual key code and text.
const NAMED: &[(&str, &str, i64, &str)] = &[
    ("Enter", "Enter", 13, "\r"),
    ("Tab", "Tab", 9, ""),
    ("Escape", "Escape", 27, ""),
    ("Backspace", "Backspace", 8, ""),
    ("Delete", "Delete", 46, ""),
    ("Insert", "Insert", 45, ""),
    ("ArrowUp", "ArrowUp", 38, ""),
    ("ArrowDown", "ArrowDown", 40, ""),
    ("ArrowLeft", "ArrowLeft", 37, ""),
    ("ArrowRight", "ArrowRight", 39, ""),
    ("Home", "Home", 36, ""),
    ("End", "End", 35, ""),
    ("PageUp", "PageUp", 33, ""),
    ("PageDown", "PageDown", 34, ""),
    ("F1", "F1", 112, ""),
    ("F2", "F2", 113, ""),
    ("F3", "F3", 114, ""),
    ("F4", "F4", 115, ""),
    ("F5", "F5", 116, ""),
    ("F6", "F6", 117, ""),
    ("F7", "F7", 118, ""),
    ("F8", "F8", 119, ""),
    ("F9", "F9", 120, ""),
    ("F10", "F10", 121, ""),
    ("F11", "F11", 122, ""),
    ("F12", "F12", 123, ""),
];

/// Other names for keys in [`NAMED`].
const ALIASES: &[(&str, &str)] = &[
    ("Return", "Enter"),
    ("Esc", "Escape"),
    ("Del", "Delete"),
    ("Up", "ArrowUp"),
    ("Down", "ArrowDown"),
    ("Left", "ArrowLeft"),
    ("Right", "ArrowRight"),
    ("PgUp", "PageUp"),
    ("PgDn", "PageDown"),
];

/// The punctuation keys of a US keyboard: what each types, and with Shift,
/// its `code` and virtual key code.
const PUNCTUATION: &[(char, char, &str, i64)] = &[
    ('-', '_', "Minus", 189),
    ('=', '+', "Equal", 187),
    ('[', '{', "BracketLeft", 219),
    (']', '}', "BracketRight", 221),
    ('\\', '|', "Backslash", 220),
    (';', ':', "Semicolon", 186),
    ('\'', '"', "Quote", 222),
    (',', '<', "Comma", 188),
    ('.', '>', "Period", 190),
    ('/', '?', "Slash", 191),
    ('`', '~', "Backquote", 192),
];

/// What the digits type with Shift, `0` first.
const SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];

/// The chord `text` names: a key — by name, or a character — after any
/// modifiers, joined with `+`: `Enter`, `Shift+Tab`, `Meta+A`, `Control++`.
pub(super) fn chord(text: &str) -> Result<Chord, String> {
    let text = text.trim();
    let (held, key) = match text.strip_suffix("++") {
        Some(held) => (held, "+"),
        None if text == "+" => ("", "+"),
        None => text.rsplit_once('+').unwrap_or(("", text)),
    };
    let mut modifiers = Vec::new();
    for name in held.split('+').filter(|name| !name.is_empty()) {
        let modifier = Modifier::named(name.trim()).ok_or_else(|| {
            format!("\"{name}\" is not a modifier; hold Alt, Control, Meta or Shift")
        })?;
        if !modifiers.contains(&modifier) {
            modifiers.push(modifier);
        }
    }
    let shift = modifiers.contains(&Modifier::Shift);
    // A letter held with a modifier is the key, whichever case it was
    // written in: ⌘A is `a`, ⇧A is `A`.
    let key = match key.trim() {
        letter
            if !modifiers.is_empty()
                && letter.len() == 1
                && letter.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            letter.to_ascii_lowercase()
        }
        key => key.to_owned(),
    };
    let mut key = named(&key, shift).ok_or_else(|| {
        format!(
            "\"{text}\" is not a key ket knows; try Enter, Tab, Escape, Backspace, ArrowDown, \
             F5, a single character, or a chord like Meta+A"
        )
    })?;
    if modifiers
        .iter()
        .any(|modifier| *modifier != Modifier::Shift)
    {
        key.text.clear();
    }
    Ok(Chord { modifiers, key })
}

/// The chord that types `c`, for `type --keys`: Enter for a new line, Tab
/// for a tab, else the character's own key — with no Shift held, as the
/// character says what it types.
pub(super) fn typed(c: char) -> Chord {
    let key = match c {
        '\n' | '\r' => named("Enter", false),
        '\t' => named("Tab", false),
        ' ' => named("Space", false),
        c => Some(character(c, false)),
    };
    Chord {
        modifiers: Vec::new(),
        key: key.unwrap_or_else(|| character(c, false)),
    }
}

/// The key `name` names, with Shift held or not: by name, any case, or a
/// single character.
fn named(name: &str, shift: bool) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    let name = ALIASES
        .iter()
        .find(|(alias, _)| alias.to_ascii_lowercase() == lower)
        .map_or(name, |(_, name)| name);
    if let Some((key, code, vk, text)) = NAMED
        .iter()
        .find(|(key, ..)| key.eq_ignore_ascii_case(name))
    {
        return Some(Key {
            key: (*key).to_owned(),
            code: (*code).to_owned(),
            vk: *vk,
            text: (*text).to_owned(),
        });
    }
    if name.eq_ignore_ascii_case("space") || name == " " {
        return Some(Key {
            key: " ".to_owned(),
            code: "Space".to_owned(),
            vk: 32,
            text: " ".to_owned(),
        });
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(character(c, shift)),
        _ => None,
    }
}

/// The key that types `c` on a US keyboard — what it types with Shift if
/// `shift` — or, for a character none does, one that types it all the same.
fn character(c: char, shift: bool) -> Key {
    let key = |typed: char, code: String, vk: i64| Key {
        key: typed.to_string(),
        code,
        vk,
        text: typed.to_string(),
    };
    if c.is_ascii_alphabetic() {
        let upper = c.to_ascii_uppercase();
        let typed = if shift { upper } else { c };
        return key(typed, format!("Key{upper}"), i64::from(upper as u8));
    }
    if let Some(digit) = c.to_digit(10) {
        let typed = if shift {
            SHIFTED_DIGITS[digit as usize]
        } else {
            c
        };
        return key(typed, format!("Digit{digit}"), 48 + i64::from(digit));
    }
    if let Some(digit) = SHIFTED_DIGITS.iter().position(|shifted| *shifted == c) {
        return key(c, format!("Digit{digit}"), 48 + digit as i64);
    }
    if let Some((plain, shifted, code, vk)) = PUNCTUATION
        .iter()
        .find(|(plain, shifted, ..)| *plain == c || *shifted == c)
    {
        let typed = if shift || c == *shifted {
            *shifted
        } else {
            *plain
        };
        return key(typed, (*code).to_owned(), *vk);
    }
    key(c, String::new(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str, code: &str, vk: i64, text: &str) -> Key {
        Key {
            key: key.to_owned(),
            code: code.to_owned(),
            vk,
            text: text.to_owned(),
        }
    }

    #[test]
    fn named_keys_send_their_key_code_and_text() {
        let plain = |text: &str| chord(text).unwrap();
        assert_eq!(plain("Enter").key, key("Enter", "Enter", 13, "\r"));
        assert_eq!(plain("return").key, key("Enter", "Enter", 13, "\r"));
        assert_eq!(plain("Tab").key, key("Tab", "Tab", 9, ""));
        assert_eq!(plain("esc").key, key("Escape", "Escape", 27, ""));
        assert_eq!(plain("Backspace").key, key("Backspace", "Backspace", 8, ""));
        assert_eq!(
            plain("ArrowDown").key,
            key("ArrowDown", "ArrowDown", 40, "")
        );
        assert_eq!(plain("F5").key, key("F5", "F5", 116, ""));
        assert_eq!(plain("Space").key, key(" ", "Space", 32, " "));
        assert_eq!(plain("a").key, key("a", "KeyA", 65, "a"));
        assert_eq!(plain("A").key, key("A", "KeyA", 65, "A"));
        assert_eq!(plain("7").key, key("7", "Digit7", 55, "7"));
        assert_eq!(plain("?").key, key("?", "Slash", 191, "?"));
        assert_eq!(plain("é").key, key("é", "", 0, "é"));
        assert!(plain("Enter").modifiers.is_empty());
    }

    #[test]
    fn modifiers_are_held_in_order_and_take_the_text_away() {
        let shift_tab = chord("Shift+Tab").unwrap();
        assert_eq!(shift_tab.modifiers, [Modifier::Shift]);
        assert_eq!(shift_tab.held(), 8);
        assert_eq!(shift_tab.key.code, "Tab");

        let select_all = chord("Meta+A").unwrap();
        assert_eq!(select_all.key, key("a", "KeyA", 65, ""));
        assert_eq!(select_all.held(), 4);
        assert_eq!(select_all.commands(), ["selectAll"]);

        // Shift types the shifted character; anything else, nothing.
        assert_eq!(chord("Shift+a").unwrap().key, key("A", "KeyA", 65, "A"));
        assert_eq!(chord("Shift+1").unwrap().key, key("!", "Digit1", 49, "!"));
        let held = chord("ctrl+Alt+Shift+x").unwrap();
        assert_eq!(
            held.modifiers,
            [Modifier::Control, Modifier::Alt, Modifier::Shift]
        );
        assert_eq!(held.held(), 1 | 2 | 8);
        assert_eq!(held.key.text, "");
        assert_eq!(Modifier::of(&Modifier::Alt.key()), Some(Modifier::Alt));
        assert_eq!(Modifier::of(&held.key), None);
        assert_eq!(chord("Cmd+Shift+Z").unwrap().commands(), ["redo"]);
        assert_eq!(
            chord("Alt+Backspace").unwrap().commands(),
            ["deleteWordBackward"]
        );
        assert_eq!(chord("Backspace").unwrap().commands(), ["deleteBackward"]);
        assert!(chord("Enter").unwrap().commands().is_empty());
        // The plus key itself.
        assert_eq!(chord("+").unwrap().key.text, "+");
        let plus = chord("Control++").unwrap();
        assert_eq!((plus.held(), plus.key.code.as_str()), (2, "Equal"));
    }

    #[test]
    fn a_key_nobody_has_says_what_to_try() {
        for bad in ["Hyper+A", "Enterr", "Meta+", ""] {
            assert!(chord(bad).is_err(), "{bad:?}");
        }
        assert!(chord("Hyper+A").unwrap_err().contains("not a modifier"));
        assert!(chord("Enterr").unwrap_err().contains("try Enter"));
    }

    #[test]
    fn typing_a_character_presses_its_key() {
        assert_eq!(typed('x').key, key("x", "KeyX", 88, "x"));
        assert_eq!(typed('X').key, key("X", "KeyX", 88, "X"));
        assert_eq!(typed('"').key, key("\"", "Quote", 222, "\""));
        assert_eq!(typed('\n').key, key("Enter", "Enter", 13, "\r"));
        assert_eq!(typed('\t').key.code, "Tab");
        assert_eq!(typed(' ').key, key(" ", "Space", 32, " "));
        assert!(typed('X').modifiers.is_empty());
    }
}
