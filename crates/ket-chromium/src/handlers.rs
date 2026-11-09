//! What CEF calls back, turned into [`Event`]s on a page's channel or handed
//! to the browser module. CEF calls these on its UI thread, the main thread,
//! but may release them from any, so they hold ids and channels only, never
//! a view.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_int;
use std::path::{Path, PathBuf};

use cef::*;
use tokio::sync::mpsc::UnboundedSender;

use crate::browser as pages;
use crate::cdp::Cdp;
use crate::{Dialog, DialogKind, Event};

/// A page's client: its life span, what it shows, its loading, its
/// renderer's health, its downloads and its JavaScript dialogs.
pub(crate) fn page_client(id: u64, events: UnboundedSender<Event>) -> Client {
    KetClient::new(
        Life::new(id, false, Some(events.clone())),
        Some(Display::new(events.clone())),
        Some(Load::new(events.clone())),
        Some(Requests::new(events.clone())),
        Some(Downloads::new(id, events.clone())),
        Some(Dialogs::new(id, events)),
    )
}

/// The client for a page's DevTools, under the DevTools' own id.
pub(crate) fn devtools_client(id: u64) -> Client {
    KetClient::new(Life::new(id, true, None), None, None, None, None, None)
}

fn text(value: Option<&CefString>) -> String {
    value.map(ToString::to_string).unwrap_or_default()
}

wrap_client! {
    struct KetClient {
        life: LifeSpanHandler,
        display: Option<DisplayHandler>,
        load: Option<LoadHandler>,
        requests: Option<RequestHandler>,
        downloads: Option<DownloadHandler>,
        dialogs: Option<JsdialogHandler>,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            self.display.clone()
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            self.load.clone()
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            self.requests.clone()
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            self.downloads.clone()
        }

        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            self.dialogs.clone()
        }
    }
}

wrap_life_span_handler! {
    struct Life {
        id: u64,
        devtools: bool,
        // The page's channel; none for DevTools.
        events: Option<UnboundedSender<Event>>,
    }

    impl LifeSpanHandler {
        // A popup — `window.open`, a link to a new tab or window — is a page
        // of its own, in a tab beside its opener's, with `window.opener`
        // kept. One opened without a click is refused, as Chrome's popup
        // blocker refuses it, and said on the console; DevTools open none.
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            popup_id: c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            let (false, Some(events), Some(window_info), Some(client)) =
                (self.devtools, self.events.as_ref(), window_info, client)
            else {
                return 1;
            };
            let url = text(target_url);
            if user_gesture != 1 {
                let _ = events.send(Event::Console {
                    level: "warning".to_owned(),
                    text: format!("ket blocked a popup the page opened without a click: {url}"),
                    source: String::new(),
                    line: 0,
                });
                return 1;
            }
            let foreground = target_disposition != WindowOpenDisposition::NEW_BACKGROUND_TAB;
            let Some((window, popup)) = pages::popup(self.id, popup_id, &url, foreground) else {
                return 1;
            };
            *window_info = window;
            *client = Some(popup);
            0
        }

        fn on_before_popup_aborted(&self, _browser: Option<&mut Browser>, popup_id: c_int) {
            pages::popup_aborted(self.id, popup_id);
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if self.devtools {
                crate::engine::opening();
            } else if let Some(browser) = browser {
                pages::created(self.id, browser);
            }
        }

        // For a page, true, or CEF sends `performClose:` to ket's own window;
        // the close finishes when CEF's view is freed, which `release_view`
        // does. DevTools have a window of their own, which CEF closes itself
        // only when told false.
        fn do_close(&self, _browser: Option<&mut Browser>) -> c_int {
            if self.devtools {
                return 0;
            }
            pages::release_view(self.id);
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            pages::closed(self.id, self.devtools);
        }
    }
}

wrap_display_handler! {
    struct Display {
        events: UnboundedSender<Event>,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|frame| frame.is_main() == 1) {
                let _ = self.events.send(Event::Address(text(url)));
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let _ = self.events.send(Event::Title(text(title)));
        }

        // Handled, so Chromium does not also write it to its log.
        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: c_int,
        ) -> c_int {
            let level = match level {
                LogSeverity::VERBOSE => "debug",
                LogSeverity::WARNING => "warning",
                LogSeverity::ERROR | LogSeverity::FATAL => "error",
                _ => "info",
            };
            let _ = self.events.send(Event::Console {
                level: level.to_owned(),
                text: text(message),
                source: text(source),
                line: u32::try_from(line).unwrap_or(0),
            });
            1
        }
    }
}

wrap_load_handler! {
    struct Load {
        events: UnboundedSender<Event>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ) {
            let _ = self.events.send(Event::Loading {
                loading: is_loading == 1,
                back: can_go_back == 1,
                forward: can_go_forward == 1,
            });
        }
    }
}

wrap_request_handler! {
    struct Requests {
        events: UnboundedSender<Event>,
    }

    impl RequestHandler {
        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            status: TerminationStatus,
            error_code: c_int,
            error_string: Option<&CefString>,
        ) {
            let how = match status {
                TerminationStatus::PROCESS_CRASHED => "crashed",
                TerminationStatus::PROCESS_WAS_KILLED => "was killed",
                TerminationStatus::PROCESS_OOM => "ran out of memory",
                TerminationStatus::LAUNCH_FAILED => "could not start",
                TerminationStatus::INTEGRITY_FAILURE => "failed an integrity check",
                _ => "ended unexpectedly",
            };
            let detail = text(error_string);
            let detail = if detail.is_empty() { detail } else { format!(": {detail}") };
            let why = format!("the page's renderer {how} (code {error_code}){detail}");
            let _ = self.events.send(Event::Crashed(why));
        }
    }
}

thread_local! {
    /// Where each download under way is going, by the page that started it
    /// and Chromium's id for it, which is only unique within one profile.
    static SAVING: RefCell<HashMap<(u64, u32), PathBuf>> = RefCell::new(HashMap::new());
}

/// Claims `path` for a download by making it, empty, only if nothing is
/// there: atomic, so two downloads at once never get one file, and on
/// macOS's case-blind disk `report.pdf` is taken while `Report.pdf` is
/// there. Chromium writes beside it and moves the finished file over it.
///
/// Only something already there takes a name. Any other failure — the
/// folder can't be written, say — leaves the name to Chromium, which says
/// how the download went: counting it as taken would try every `name (n)`
/// and settle on one with the time in it.
fn claim(path: &Path) -> bool {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::AlreadyExists,
    }
}

/// Takes back an unfinished download's claim: the empty file made for it,
/// if it is still empty. Not one with anything in it, which is not ours.
fn unclaim(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() == 0) {
        let _ = std::fs::remove_file(path);
    }
}

/// Lets go of what page `page` was downloading, as it closes: CEF says no
/// more about a closed page's downloads, so nothing else would. A download
/// still going carries on, and its file lands where it was claimed.
pub(crate) fn forget_downloads(page: u64) {
    SAVING.with_borrow_mut(|saving| saving.retain(|(owner, _), _| *owner != page));
}

/// The most `name (n).ext` tried before a name with the time in it.
const NAMES_TRIED: u32 = 999;

/// The Downloads folder, where everything a page downloads goes.
fn downloads_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join("Downloads"))
}

/// Where a download Chromium suggests `suggested` for goes in `dir`: that
/// name, or `name (1).ext`, `name (2).ext` and on when `taken` says it is.
/// Only the name is kept from what the page suggests, never a path, and
/// never a hidden file's.
fn free_path(dir: &Path, suggested: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let name = Path::new(suggested)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.trim().trim_start_matches('.'))
        .filter(|name| !name.is_empty())
        .unwrap_or("download");
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let named = Path::new(name);
    let stem = named
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name);
    let ext = named
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_default();
    (1..=NAMES_TRIED)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|path| !taken(path))
        .unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis());
            dir.join(format!("{stem} ({now}){ext}"))
        })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// Saves what a page downloads straight to the Downloads folder, with no
// dialog, and reports how it goes on the page's channel.
wrap_download_handler! {
    struct Downloads {
        page: u64,
        events: UnboundedSender<Event>,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else {
                return 0;
            };
            let id = item.id();
            let Some(dir) = downloads_dir() else {
                // Declined, which cancels it.
                let _ = self.events.send(Event::Download {
                    id,
                    name: text(suggested_name),
                    path: PathBuf::new(),
                    received: 0,
                    total: item.total_bytes(),
                    done: false,
                    failed: true,
                });
                return 0;
            };
            // A home with no Downloads folder yet gets one, as Safari makes it.
            let _ = std::fs::create_dir_all(&dir);
            let path = free_path(&dir, &text(suggested_name), |path| !claim(path));
            let Ok(target) = crate::engine::cef_path(&path) else {
                unclaim(&path);
                return 0;
            };
            SAVING.with_borrow_mut(|saving| saving.insert((self.page, id), path.clone()));
            callback.cont(Some(&target), 0);
            let _ = self.events.send(Event::Download {
                id,
                name: file_name(&path),
                path,
                received: item.received_bytes(),
                total: item.total_bytes(),
                done: false,
                failed: false,
            });
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else {
                return;
            };
            let id = item.id();
            // Also called before `on_before_download`, with nowhere to go yet.
            let Some(path) = SAVING.with_borrow(|saving| saving.get(&(self.page, id)).cloned())
            else {
                return;
            };
            let done = item.is_complete() == 1;
            let failed = !done && (item.is_canceled() == 1 || item.is_interrupted() == 1);
            if done || failed {
                SAVING.with_borrow_mut(|saving| saving.remove(&(self.page, id)));
            }
            if failed {
                unclaim(&path);
            }
            let _ = self.events.send(Event::Download {
                id,
                name: file_name(&path),
                path,
                received: item.received_bytes(),
                total: item.total_bytes(),
                done,
                failed,
            });
        }
    }
}

thread_local! {
    /// Each page's open JavaScript dialog, by page: there while it is open,
    /// with what answers it until it has been answered.
    static DIALOGS: RefCell<HashMap<u64, Option<JsdialogCallback>>> =
        RefCell::new(HashMap::new());
}

/// Answers page `page`'s open dialog: OK, with `text` in a prompt's field,
/// or Cancel. False when it has none waiting.
pub(crate) fn answer_dialog(page: u64, accept: bool, text: Option<&str>) -> bool {
    // Taken out first: answering tells `Dialogs` at once that it closed.
    let Some(callback) = DIALOGS.with_borrow_mut(|open| open.get_mut(&page)?.take()) else {
        return false;
    };
    callback.cont(accept.into(), text.map(CefString::from).as_ref());
    true
}

/// Lets go of page `page`'s dialog: it has closed, or the page has. One
/// still waiting is cancelled, as Chromium cancels one it is let go of.
/// Whether there was one.
pub(crate) fn forget_dialog(page: u64) -> bool {
    let open = DIALOGS.with_borrow_mut(|open| open.remove(&page));
    // Dropped outside the borrow: cancelling calls back in.
    open.is_some()
}

/// What a dialog CEF reports is, as [`Dialog`] has it.
fn dialog(
    kind: DialogKind,
    message: Option<&CefString>,
    default: Option<&CefString>,
    url: Option<&CefString>,
) -> Dialog {
    Dialog {
        kind,
        message: text(message),
        default_text: text(default),
        url: text(url),
    }
}

// Shows a page's JavaScript dialogs in its tab, not as CEF's own alert. On a
// Mac, CEF's is an app-modal `NSAlert`: it stops all of ket for a page in a
// tab nobody may be looking at, and when an agent answers the page through
// the DevTools protocol instead, CEF lets go of the alert without closing
// it. Here the page's channel says a dialog opened and when it closed,
// however it was answered — by the person in the tab, by an agent through
// `Page.handleJavaScriptDialog`, by a navigation — and the tab follows it.
wrap_jsdialog_handler! {
    struct Dialogs {
        page: u64,
        events: UnboundedSender<Event>,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut c_int>,
        ) -> c_int {
            let kind = match dialog_type {
                JsdialogType::CONFIRM => DialogKind::Confirm,
                JsdialogType::PROMPT => DialogKind::Prompt,
                _ => DialogKind::Alert,
            };
            self.open(dialog(kind, message_text, default_prompt_text, origin_url), callback)
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            is_reload: c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> c_int {
            let kind = DialogKind::Leave { reload: is_reload == 1 };
            self.open(dialog(kind, None, None, None), callback)
        }

        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            self.closed();
        }

        fn on_dialog_closed(&self, _browser: Option<&mut Browser>) {
            self.closed();
        }
    }
}

impl Dialogs {
    /// The page opened `dialog`: kept with what answers it, and said. CEF's
    /// own takes it with nothing to answer it with.
    fn open(&self, dialog: Dialog, callback: Option<&mut JsdialogCallback>) -> c_int {
        let Some(callback) = callback else {
            return 0;
        };
        DIALOGS.with_borrow_mut(|open| open.insert(self.page, Some(callback.clone())));
        let _ = self.events.send(Event::Dialog(Some(dialog)));
        1
    }

    /// The page's dialog has closed, if it had one.
    fn closed(&self) {
        if forget_dialog(self.page) {
            let _ = self.events.send(Event::Dialog(None));
        }
    }
}

// Creates a page's browser once its profile is ready.
wrap_request_context_handler! {
    pub(crate) struct PageContext {
        id: u64,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            pages::create(self.id, request_context);
        }
    }
}

// Hands a page's DevTools protocol messages, answers and events alike, to
// its `Cdp`.
wrap_dev_tools_message_observer! {
    pub(crate) struct CdpObserver {
        cdp: Cdp,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> c_int {
            if let Some(message) = message {
                self.cdp.receive(message);
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_takes_a_name_nothing_else_has() {
        let dir = Path::new("/Users/me/Downloads");
        let taken = |names: &'static [&'static str]| {
            move |path: &Path| names.iter().any(|name| dir.join(name) == path)
        };
        assert_eq!(
            free_path(dir, "report.pdf", taken(&[])),
            dir.join("report.pdf")
        );
        assert_eq!(
            free_path(dir, "report.pdf", taken(&["report.pdf", "report (1).pdf"])),
            dir.join("report (2).pdf")
        );
        assert_eq!(
            free_path(dir, "notes", taken(&["notes"])),
            dir.join("notes (1)")
        );
        // Only ever a name in the folder, never a path out of it.
        assert_eq!(
            free_path(dir, "../../etc/passwd", taken(&[])),
            dir.join("passwd")
        );
        assert_eq!(free_path(dir, "", taken(&[])), dir.join("download"));
        assert_eq!(free_path(dir, ".zshrc", taken(&[])), dir.join("zshrc"));
    }

    #[test]
    fn two_downloads_never_get_one_file() {
        let dir = std::env::temp_dir().join(format!("ket-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        let first = free_path(&dir, "Report.pdf", |path| !claim(path));
        let second = free_path(&dir, "Report.pdf", |path| !claim(path));
        assert_eq!(first, dir.join("Report.pdf"));
        assert_eq!(second, dir.join("Report (1).pdf"));
        // On a case-blind disk, as macOS's is by default, a name differing
        // only in case is the same file, and is taken.
        let case_blind = dir.join("REPORT.PDF").exists();
        let third = free_path(&dir, "report.pdf", |path| !claim(path));
        if case_blind {
            assert_eq!(third, dir.join("report (2).pdf"));
        }
        // An unfinished download's empty claim goes; a file with anything
        // in it stays.
        unclaim(&second);
        assert!(!second.exists());
        std::fs::write(&first, b"finished").expect("a write");
        unclaim(&first);
        assert!(first.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_does_not_send_every_name_to_the_time() {
        let dir = std::env::temp_dir()
            .join(format!("ket-downloads-missing-{}", std::process::id()))
            .join("Downloads");
        let _ = std::fs::remove_dir_all(dir.parent().expect("a parent"));
        // Unwritable or missing, a name is left to Chromium, not skipped.
        assert_eq!(
            free_path(&dir, "report.pdf", |path| !claim(path)),
            dir.join("report.pdf")
        );
        let _ = std::fs::remove_dir_all(dir.parent().expect("a parent"));
    }
}
