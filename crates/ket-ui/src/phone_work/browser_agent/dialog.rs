//! JavaScript dialogs: an `alert`, `confirm` or `prompt`, or the question
//! `beforeunload` asks before a page is left, stops the page until it is
//! answered, and whatever an agent asked of it would wait with it until the
//! window gave up. So every command watches for the page opening one
//! (`Page.javascriptDialogOpening`) and answers at once with what it says;
//! while one is open, every command but `tabs`, `console` and `dialog` answers
//! that way before touching the page — the tab knows it has one from
//! `ket_chromium::Event::Dialog`. `ket browser dialog` answers it, through
//! `Page.handleJavaScriptDialog`. The person sees it in the tab, and
//! whichever of them answers first closes it for the other.

use gpui::{AsyncApp, WeakEntity};
use ket_chromium::protocol::browser_protocol::page::HandleJavaScriptDialogParams;
use ket_chromium::{CdpEvent, Dialog, DialogKind};
use ket_core::host::BrowserAction;
use serde_json::Value;
use tokio::sync::broadcast::{self, error::RecvError};

use super::{Answer, Page};
use crate::Shell;

/// The most of what a dialog says that a terminal is told, in characters.
const SAID_MAX: usize = 300;

/// The dialog `event` says the page opened, if it says one did.
pub(super) fn opened(event: &CdpEvent) -> Option<Dialog> {
    if event.method != "Page.javascriptDialogOpening" {
        return None;
    }
    let text = |key: &str| {
        event
            .params
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let kind = match event.params.get("type").and_then(Value::as_str) {
        Some("confirm") => DialogKind::Confirm,
        Some("prompt") => DialogKind::Prompt,
        // The protocol cannot tell leaving from reloading.
        Some("beforeunload") => DialogKind::Leave { reload: false },
        _ => DialogKind::Alert,
    };
    Some(Dialog {
        kind,
        message: text("message"),
        default_text: text("defaultPrompt"),
        url: text("url"),
    })
}

/// Waits for `events` to say the page opened a dialog, and says which; never,
/// once the page has closed. `events` goes on from there, for [`closing`].
pub(super) async fn opening(events: &mut broadcast::Receiver<CdpEvent>) -> Dialog {
    loop {
        match events.recv().await {
            Ok(event) => {
                if let Some(dialog) = opened(&event) {
                    return dialog;
                }
            }
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

/// Waits for `events`, which heard a dialog open, to say it closed: true
/// once it has — or may have, `events` having fallen behind and missed what
/// the page said — false if the page closed first.
pub(super) async fn closing(events: &mut broadcast::Receiver<CdpEvent>) -> bool {
    loop {
        match events.recv().await {
            Ok(event) if event.method == "Page.javascriptDialogClosed" => return true,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => return true,
            Err(RecvError::Closed) => return false,
        }
    }
}

/// Whether `action` waits on a dialog the page has open: everything that
/// touches the page, which is everything but what the window answers from
/// what it already knows, and answering the dialog itself.
pub(super) fn waits(action: &BrowserAction) -> bool {
    !matches!(
        action,
        BrowserAction::Tabs
            | BrowserAction::Console { .. }
            | BrowserAction::Open { url: None }
            | BrowserAction::RecordStop
            | BrowserAction::Dialog { .. }
    )
}

/// `dialog` as a terminal names it: `a confirm: "Delete 3 files?"`.
pub(super) fn what(dialog: &Dialog) -> String {
    let said = |kind: &str| match dialog.message.as_str() {
        "" => kind.to_owned(),
        message => format!("{kind}: {}", quoted(message)),
    };
    match dialog.kind {
        DialogKind::Alert => said("an alert"),
        DialogKind::Confirm => said("a confirm"),
        DialogKind::Prompt if dialog.default_text.is_empty() => said("a prompt"),
        DialogKind::Prompt => format!(
            "{}, filled in with {}",
            said("a prompt"),
            quoted(&dialog.default_text)
        ),
        DialogKind::Leave { .. } => "a beforeunload prompt".to_owned(),
    }
}

/// What the terminal is told while `dialog` is open: what it says, and how
/// to answer it.
pub(super) fn waiting(dialog: &Dialog) -> String {
    match dialog.kind {
        DialogKind::Alert | DialogKind::Confirm => format!(
            "the page opened {}; ket browser dialog accept or dismiss answers it",
            what(dialog)
        ),
        DialogKind::Prompt => format!(
            "the page opened {}; ket browser dialog accept [--text <answer>] or dismiss answers it",
            what(dialog)
        ),
        DialogKind::Leave { .. } => "the page asked whether to leave it, in a beforeunload \
                                     prompt; ket browser dialog accept leaves, dismiss stays"
            .to_owned(),
    }
}

/// `text` in quotes, on one line, cut to [`SAID_MAX`] characters.
fn quoted(text: &str) -> String {
    let mut shown: String = text.chars().take(SAID_MAX).collect();
    if shown.len() < text.len() {
        shown.push('…');
    }
    format!(
        "\"{}\"",
        shown
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

impl Page<'_> {
    /// Answers the dialog the page has open: OK, with `text` in a prompt's
    /// field, or Cancel; then says where the page is, waiting for a page the
    /// answer loads, as a click does.
    pub(super) async fn answer_dialog(
        &self,
        this: &WeakEntity<Shell>,
        accept: bool,
        text: Option<String>,
        cx: &mut AsyncApp,
    ) -> Answer {
        // Not `watch`: enabling the page's events waits on the page, which
        // the dialog stops.
        let watching = self.cdp.events();
        let mut answer = HandleJavaScriptDialogParams::new(accept);
        answer.prompt_text = text.clone();
        if let Err(why) = self.ask(answer).await {
            // Opened before ket had the page's DevTools say so, Chromium's
            // DevTools has no dialog to answer: the tab's own answers it.
            let tab = self.tab;
            let answered = this
                .update(cx, |shell, _| {
                    shell
                        .chromium_browser(tab)
                        .is_some_and(|page| page.answer_dialog(accept, text.as_deref()))
                })
                .unwrap_or(false);
            if !answered {
                return Err(if why.contains("No dialog is showing") {
                    "no dialog is open".to_owned()
                } else {
                    why
                });
            }
        }
        self.landed(None, watching).await
    }
}

/// What `ket browser dialog` asks the page's dialog, or why it can't be
/// asked: OK keeps a prompt's field as the page filled it in, as a person
/// pressing OK does, unless `text` says otherwise.
pub(super) fn answer(
    dialog: Option<&Dialog>,
    accept: bool,
    text: Option<String>,
) -> Result<Option<String>, String> {
    let Some(dialog) = dialog else {
        return Err("no dialog is open".to_owned());
    };
    match (dialog.kind, text) {
        (DialogKind::Prompt, None) if accept => Ok(Some(dialog.default_text.clone())),
        (DialogKind::Prompt, text) => Ok(text),
        (_, Some(_)) => Err(format!(
            "only a prompt takes --text, and this is {}",
            what(dialog)
        )),
        (_, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opening_event(params: Value) -> CdpEvent {
        CdpEvent {
            method: "Page.javascriptDialogOpening".to_owned(),
            params,
        }
    }

    #[test]
    fn a_dialog_opening_is_heard_and_said_with_how_to_answer_it() {
        let confirm = opened(&opening_event(serde_json::json!({
            "url": "http://localhost:3000/files",
            "frameId": "F1",
            "message": "Delete 3 files?",
            "type": "confirm",
            "hasBrowserHandler": true,
            "defaultPrompt": ""
        })))
        .expect("a confirm");
        assert_eq!(confirm.kind, DialogKind::Confirm);
        assert_eq!(confirm.url, "http://localhost:3000/files");
        assert_eq!(
            waiting(&confirm),
            "the page opened a confirm: \"Delete 3 files?\"; ket browser dialog accept or \
             dismiss answers it"
        );
        let prompt = opened(&opening_event(serde_json::json!({
            "url": "http://localhost:3000/",
            "message": "Your name?",
            "type": "prompt",
            "defaultPrompt": "Sam"
        })))
        .expect("a prompt");
        assert_eq!(
            waiting(&prompt),
            "the page opened a prompt: \"Your name?\", filled in with \"Sam\"; ket browser \
             dialog accept [--text <answer>] or dismiss answers it"
        );
        let alert = opened(&opening_event(serde_json::json!({
            "url": "http://localhost:3000/",
            "message": "Saved \"draft\"\nfor later",
            "type": "alert"
        })))
        .expect("an alert");
        assert_eq!(what(&alert), r#"an alert: "Saved \"draft\"\nfor later""#);
        let leave = opened(&opening_event(serde_json::json!({
            "url": "http://localhost:3000/edit",
            "message": "",
            "type": "beforeunload"
        })))
        .expect("a beforeunload prompt");
        assert_eq!(leave.kind, DialogKind::Leave { reload: false });
        assert!(waiting(&leave).starts_with("the page asked whether to leave it"));
        assert_eq!(what(&leave), "a beforeunload prompt");
        // Anything else is not a dialog opening.
        let closed = CdpEvent {
            method: "Page.javascriptDialogClosed".to_owned(),
            params: serde_json::json!({"result": true, "userInput": ""}),
        };
        assert_eq!(opened(&closed), None);
        // A long message is cut.
        let long = Dialog {
            message: "x".repeat(SAID_MAX + 50),
            ..alert
        };
        assert!(what(&long).ends_with("x…\""));
    }

    #[test]
    fn a_dialog_waits_for_everything_but_what_answers_it() {
        assert!(waits(&BrowserAction::Reload));
        assert!(waits(&BrowserAction::Eval {
            expression: "1".to_owned()
        }));
        assert!(waits(&BrowserAction::Open {
            url: Some("localhost:3000".to_owned())
        }));
        assert!(!waits(&BrowserAction::Tabs));
        assert!(!waits(&BrowserAction::Open { url: None }));
        assert!(!waits(&BrowserAction::Dialog {
            accept: true,
            text: None
        }));
    }

    #[test]
    fn an_answer_keeps_a_prompt_as_filled_in_and_only_a_prompt_takes_text() {
        let dialog = |kind| Dialog {
            kind,
            message: "Your name?".to_owned(),
            default_text: "Sam".to_owned(),
            url: String::new(),
        };
        let prompt = dialog(DialogKind::Prompt);
        assert_eq!(
            answer(Some(&prompt), true, None),
            Ok(Some("Sam".to_owned()))
        );
        assert_eq!(
            answer(Some(&prompt), true, Some("Ana".to_owned())),
            Ok(Some("Ana".to_owned()))
        );
        assert_eq!(answer(Some(&prompt), false, None), Ok(None));
        let confirm = dialog(DialogKind::Confirm);
        assert_eq!(answer(Some(&confirm), true, None), Ok(None));
        assert_eq!(
            answer(Some(&confirm), true, Some("x".to_owned())),
            Err("only a prompt takes --text, and this is a confirm: \"Your name?\"".to_owned())
        );
        assert_eq!(
            answer(None, false, None),
            Err("no dialog is open".to_owned())
        );
    }
}
