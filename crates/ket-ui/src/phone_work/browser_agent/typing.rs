//! `type` and `press`: the page's keyboard, as the DevTools protocol drives
//! it — trusted key events a page cannot tell from a person's, a form
//! submitting on Enter — with the keys named as `keys` has them. A key that
//! starts loading another page waits for it, as a click does.

use ket_chromium::protocol::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, InsertTextParams,
};
use serde_json::Value;

use super::keys::{self, Chord, Key};
use super::{Answer, Page};

/// The most characters `type --keys` presses one by one.
const KEYS_MAX: usize = 1_000;

/// Whether something on the page has focus to type into.
const FOCUSED: &str = "(() => { const at = document.activeElement; \
     return document.designMode === 'on' \
     || (!!at && at !== document.body && at !== document.documentElement); })()";

impl Page<'_> {
    /// Types `text` into what has focus: as one insertion, as typing does,
    /// or a key at a time.
    pub(super) async fn type_text(&self, text: &str, keys: bool) -> Answer {
        if !keys {
            if self.value(FOCUSED).await == Some(Value::Bool(false)) {
                return Err(
                    "nothing on the page has focus to type into; `ket browser click` or \
                     `ket browser fill` a field first"
                        .to_owned(),
                );
            }
            self.ask(InsertTextParams::new(text)).await?;
            return self.here(None).await;
        }
        if text.chars().count() > KEYS_MAX {
            return Err(format!(
                "--keys types at most {KEYS_MAX} characters at a time; type the rest after, \
                 or leave --keys off"
            ));
        }
        let from = self.spot().await;
        let watching = self.watch().await;
        for c in text.chars() {
            self.chord(&keys::typed(c)).await?;
        }
        self.landed(from, watching).await
    }

    /// Presses the key `name` names, with its modifiers, and says where the
    /// page is afterwards.
    pub(super) async fn press(&self, name: &str) -> Answer {
        let chord = keys::chord(name)?;
        let from = self.spot().await;
        let watching = self.watch().await;
        self.chord(&chord).await?;
        self.landed(from, watching).await
    }

    /// Presses `chord`: each modifier down, the key down — typing its
    /// character, if it has one — and up, then the modifiers up.
    async fn chord(&self, chord: &Chord) -> Result<(), String> {
        let mut held = 0;
        for modifier in &chord.modifiers {
            held |= modifier.bit();
            self.key(DispatchKeyEventType::RawKeyDown, &modifier.key(), held, &[])
                .await?;
        }
        // A key that types is sent as one `keyDown`, which Chromium splits
        // into the raw key down and the character itself — and drops the
        // character when the page cancels the key down, as it would a
        // person's. Sent as two, the character would type regardless.
        let down = if chord.key.text.is_empty() {
            DispatchKeyEventType::RawKeyDown
        } else {
            DispatchKeyEventType::KeyDown
        };
        self.key(down, &chord.key, held, chord.commands()).await?;
        self.key(DispatchKeyEventType::KeyUp, &chord.key, held, &[])
            .await?;
        for modifier in chord.modifiers.iter().rev() {
            held &= !modifier.bit();
            self.key(DispatchKeyEventType::KeyUp, &modifier.key(), held, &[])
                .await?;
        }
        Ok(())
    }

    /// Sends one key event — noting, in the same turn, a key it holds down
    /// or lets go of: see `held`.
    async fn key(
        &self,
        kind: DispatchKeyEventType,
        key: &Key,
        modifiers: i64,
        commands: &[&str],
    ) -> Result<(), String> {
        self.held.borrow_mut().key(&kind, key);
        self.ask(key_event(kind, key, modifiers, commands))
            .await
            .map(drop)
    }
}

/// One key event, as the protocol sends it.
pub(super) fn key_event(
    kind: DispatchKeyEventType,
    key: &Key,
    modifiers: i64,
    commands: &[&str],
) -> DispatchKeyEventParams {
    let types = kind == DispatchKeyEventType::KeyDown;
    let mut event = DispatchKeyEventParams::new(kind);
    event.modifiers = Some(modifiers);
    event.key = Some(key.key.clone());
    event.code = Some(key.code.clone());
    event.windows_virtual_key_code = Some(key.vk);
    if types {
        event.text = Some(key.text.clone());
        event.unmodified_text = Some(key.text.clone());
    }
    if !commands.is_empty() {
        event.commands = Some(
            commands
                .iter()
                .map(|command| (*command).to_owned())
                .collect(),
        );
    }
    event
}
