//! Starting Chromium inside ket, stopping it again, and its helper processes.
//!
//! [`start`] finds the framework inside ket.app, loads it, makes gpui's
//! `NSApplication` speak Chromium's protocols, installs the message pump and
//! initializes CEF: sandboxed helpers, the external pump, a root cache ket
//! names, and no remote-debugging port. [`shutdown`] closes every browser
//! and pumps until each has reported closed, synchronously, because gpui
//! gives its quit handlers only 100 ms of its own.
//!
//! [`helper_main`] is the other side: what each subprocess runs.

use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cef::library_loader::LibraryLoader;
// What the `wrap_*` macros below call `add_ref` through.
use cef::rc::Rc as _;
use cef::{
    App, BrowserProcessHandler, CefString, CommandLine, ImplApp, ImplBrowserProcessHandler,
    ImplCommandLine, ImplTask, LogSeverity, MainArgs, Settings, Task, ThreadId, WrapApp,
    WrapBrowserProcessHandler, WrapTask, wrap_app, wrap_browser_process_handler, wrap_task,
};
use objc2::MainThreadMarker;

use crate::{Error, failed};

/// The framework's directory name, in `Contents/Frameworks`.
const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
/// The helper bundle's name, and its binary's. Chromium finds the GPU,
/// Renderer, Plugin and Alerts variants beside it by adding " (GPU)" and
/// the rest to both.
const HELPER: &str = "ket Helper";
/// The sandbox library inside the framework, which a helper loads first.
const SANDBOX: &str = "Libraries/libcef_sandbox.dylib";

/// How Chromium is set up for ket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Where Chromium keeps everything: its own state, and each profile in a
    /// directory beneath this one ([`Browser::open`](crate::Browser::open)
    /// refuses a profile anywhere else). Absolute. Two processes cannot share
    /// it: Chromium locks it, and the second one's [`start`] fails.
    pub root_cache: PathBuf,
    /// A development build: Chromium's cookie encryption uses a stand-in
    /// key instead of the login keychain, so each ad-hoc-signed rebuild does
    /// not ask for "ket Safe Storage" again.
    pub dev: bool,
}

const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const STOPPING: u8 = 2;
const STOPPED: u8 = 3;

/// Where Chromium is: not started, running, shutting down, or finished for
/// good — CEF cannot be started twice in one process.
static STATE: AtomicU8 = AtomicU8::new(IDLE);

/// The loaded framework. Never dropped: dropping unloads it, and CEF
/// objects ket still holds would then call into nothing.
static LOADER: OnceLock<LibraryLoader> = OnceLock::new();

/// The root cache, for checking profiles against.
static ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

thread_local! {
    /// What CEF was given and must outlive it.
    static ENGINE: RefCell<Option<(App, Argv)>> = const { RefCell::new(None) };
    /// Browsers asked for and not yet reported closed, DevTools included.
    static LIVE: Cell<usize> = const { Cell::new(0) };
    /// How [`shutdown`] closes the browsers; set by the browser module.
    static CLOSER: Cell<Option<Closer>> = const { Cell::new(None) };
}

/// What [`shutdown`] needs from the browser module, handed over when a page
/// opens so this module needs nothing from it.
#[derive(Clone, Copy)]
pub(crate) struct Closer {
    /// Asks every browser to close.
    pub(crate) close_all: fn(),
    /// Lets go of whatever has not closed, once the wait is over.
    pub(crate) forget_all: fn(),
}

pub(crate) fn on_shutdown(closer: Closer) {
    CLOSER.set(Some(closer));
}

pub(crate) fn opening() {
    LIVE.set(LIVE.get() + 1);
}

pub(crate) fn closed() {
    LIVE.set(LIVE.get().saturating_sub(1));
}

/// Whether Chromium is running. Any thread.
pub fn started() -> bool {
    STATE.load(Ordering::Acquire) == RUNNING
}

/// Whether CEF will still take calls: running, or closing its browsers.
pub(crate) fn alive() -> bool {
    matches!(STATE.load(Ordering::Acquire), RUNNING | STOPPING)
}

/// `profile`, checked: absolute, beneath the root cache and not it.
pub(crate) fn profile_path(profile: &Path) -> Result<PathBuf, Error> {
    let root = ROOT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or_else(|| failed("Chromium has not been started"))?;
    if within(&root, profile) {
        Ok(profile.to_path_buf())
    } else {
        Err(failed(format!(
            "the browser profile {} is not inside Chromium's root cache {}",
            profile.display(),
            root.display()
        )))
    }
}

/// Whether `profile` is a directory strictly beneath `root`, said without
/// `..` — CEF refuses a profile anywhere else.
fn within(root: &Path, profile: &Path) -> bool {
    profile.is_absolute()
        && profile != root
        && profile.starts_with(root)
        && !profile
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
}

/// A path as CEF takes it. CEF's strings are UTF-16, so a path that is not
/// valid Unicode cannot be handed over faithfully.
pub(crate) fn cef_path(path: &Path) -> Result<CefString, Error> {
    path.to_str()
        .map(CefString::from)
        .ok_or_else(|| failed(format!("{} is not a valid Unicode path", path.display())))
}

/// Starts Chromium. Once, on the main thread, after gpui has made
/// `NSApplication`; calling it again while running does nothing.
///
/// Fails with [`Error::NotBundled`] when ket is not running from a bundle
/// with Chromium inside it, and with [`Error::Failed`] when Chromium would
/// not start — including when another process already has
/// [`Config::root_cache`].
pub fn start(config: Config) -> Result<(), Error> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| failed("Chromium has to be started on the main thread"))?;
    match STATE.load(Ordering::Acquire) {
        IDLE => {}
        RUNNING => return Ok(()),
        _ => return Err(failed("Chromium has already been shut down")),
    }
    if !config.root_cache.is_absolute() {
        return Err(failed(format!(
            "Chromium's root cache has to be an absolute path, not {}",
            config.root_cache.display()
        )));
    }
    let bundle = Bundle::find()?;

    if LOADER.get().is_none() {
        let exe = std::env::current_exe().map_err(failed)?;
        let loader = LibraryLoader::new(&exe, false);
        if !loader.load() {
            return Err(failed(format!(
                "the Chromium framework at {} would not load",
                bundle.framework.display()
            )));
        }
        let _ = LOADER.set(loader);
        // Before any other CEF call: says which API version ket was built
        // against.
        let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    }
    crate::app::adopt(mtm)?;

    let settings = Settings {
        no_sandbox: 0,
        browser_subprocess_path: cef_path(&bundle.helper)?,
        framework_dir_path: cef_path(&bundle.framework)?,
        main_bundle_path: cef_path(&bundle.app)?,
        multi_threaded_message_loop: 0,
        external_message_pump: 1,
        // ket's own arguments are not Chromium switches.
        command_line_args_disabled: 1,
        root_cache_path: cef_path(&config.root_cache)?,
        // Chromium's INFO lines run to thousands per page.
        log_severity: LogSeverity::WARNING,
        // Beside the profiles, where a failed check's reason can be read
        // after the crash: left unset, it goes to whatever terminal started
        // ket, if any.
        log_file: cef_path(&config.root_cache.join("chromium.log"))?,
        // Its own keychain item, not the "Chromium Safe Storage" every CEF
        // app shares by default.
        keychain_service_name: CefString::from("ket Safe Storage"),
        keychain_account_name: CefString::from("ket"),
        ..Default::default()
    };
    let mut app = KetApp::new(config.dev, KetProcess::new());
    let mut argv = Argv::new();
    let args = argv.main_args();

    crate::pump::install(mtm)?;
    *ROOT.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.root_cache.clone());
    if cef::initialize(
        Some(&args),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        crate::pump::uninstall();
        STATE.store(STOPPED, Ordering::Release);
        let code = cef::get_exit_code();
        let notified = cef::sys::cef_resultcode_t::CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED;
        return Err(if code == notified as c_int {
            failed(format!(
                "another process is already using Chromium's cache at {}",
                config.root_cache.display()
            ))
        } else {
            failed(format!("Chromium did not start (exit code {code})"))
        });
    }
    ENGINE.set(Some((app, argv)));
    STATE.store(RUNNING, Ordering::Release);
    Ok(())
}

/// Shuts Chromium down: closes every browser, pumps CEF until each has
/// reported [`Event::Closed`](crate::Event::Closed) or `timeout` passes,
/// then stops CEF. Synchronous, on the main thread; afterwards nothing here
/// works again until the process restarts.
pub fn shutdown(timeout: Duration) {
    if MainThreadMarker::new().is_none() {
        tracing::warn!("Chromium can only be shut down from the main thread");
        return;
    }
    if STATE
        .compare_exchange(RUNNING, STOPPING, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    crate::pump::uninstall();
    let closer = CLOSER.get();
    if let Some(closer) = closer {
        (closer.close_all)();
    }
    let deadline = Instant::now() + timeout;
    while LIVE.get() > 0 && Instant::now() < deadline {
        crate::pump::work();
        std::thread::sleep(Duration::from_millis(1));
    }
    let left = LIVE.get();
    if left > 0 {
        tracing::warn!(
            left,
            ?timeout,
            "Chromium browsers had not closed; stopping anyway"
        );
    }
    if let Some(closer) = closer {
        (closer.forget_all)();
    }
    STATE.store(STOPPED, Ordering::Release);
    cef::shutdown();
    ENGINE.set(None);
}

/// Runs `job` on CEF's UI thread — the main thread — at its next turn, or
/// after `delay`. Any thread. False when CEF is not taking work, in which
/// case `job` is dropped unrun.
pub(crate) fn on_ui(delay: Option<Duration>, job: impl FnOnce() + Send + 'static) -> bool {
    if !alive() {
        return false;
    }
    let job: Job = Box::new(job);
    let mut task = KetTask::new(std::sync::Arc::new(Mutex::new(Some(job))));
    let posted = match delay {
        None => cef::post_task(ThreadId::UI, Some(&mut task)),
        Some(delay) => {
            let ms = i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
            cef::post_delayed_task(ThreadId::UI, Some(&mut task), ms)
        }
    };
    posted == 1
}

type Job = Box<dyn FnOnce() + Send>;

wrap_task! {
    struct KetTask {
        job: std::sync::Arc<Mutex<Option<Job>>>,
    }

    impl Task {
        fn execute(&self) {
            let job = self.job.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(job) = job {
                job();
            }
        }
    }
}

wrap_app! {
    struct KetApp {
        dev: bool,
        process: BrowserProcessHandler,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            // The browser process is the one with no type.
            let browser = process_type.is_none_or(|kind| kind.to_string().is_empty());
            if let (true, true, Some(command_line)) = (self.dev, browser, command_line) {
                command_line.append_switch(Some(&CefString::from("use-mock-keychain")));
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(self.process.clone())
        }
    }
}

wrap_browser_process_handler! {
    struct KetProcess;

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            crate::pump::schedule(delay_ms);
        }

        fn on_already_running_app_relaunch(
            &self,
            _command_line: Option<&mut CommandLine>,
            _current_directory: Option<&CefString>,
        ) -> c_int {
            // Another ket started with the same root cache; its own start
            // fails and says so. Nothing should open here.
            1
        }
    }
}

/// ket.app's parts that CEF is told about.
struct Bundle {
    app: PathBuf,
    framework: PathBuf,
    helper: PathBuf,
}

impl Bundle {
    /// The bundle around the running binary, which has to be
    /// `<ket>.app/Contents/MacOS/<binary>` with the framework and the helper
    /// in `Contents/Frameworks`.
    fn find() -> Result<Self, Error> {
        let exe = std::env::current_exe().map_err(failed)?;
        let not_bundled = |what: &str| {
            Error::NotBundled(format!(
                "ket is not running from an app bundle with Chromium inside it: {what} (running {})",
                exe.display()
            ))
        };
        let contents = exe
            .parent()
            .filter(|dir| dir.ends_with("Contents/MacOS"))
            .and_then(Path::parent)
            .ok_or_else(|| not_bundled("it is not inside Contents/MacOS"))?;
        let app = contents
            .parent()
            .ok_or_else(|| not_bundled("Contents has no bundle around it"))?
            .to_path_buf();
        let frameworks = contents.join("Frameworks");
        let framework = frameworks.join(FRAMEWORK);
        let library = framework.join("Chromium Embedded Framework");
        if !library.is_file() {
            return Err(not_bundled(&format!("there is no {}", library.display())));
        }
        let helper = frameworks
            .join(format!("{HELPER}.app"))
            .join("Contents/MacOS")
            .join(HELPER);
        if !helper.is_file() {
            return Err(not_bundled(&format!("there is no {}", helper.display())));
        }
        Ok(Self {
            app,
            framework,
            helper,
        })
    }
}

/// The process's arguments as C strings, the way CEF takes them. Built from
/// `args_os` because `std::env::args` would panic on one that is not UTF-8.
struct Argv {
    _strings: Vec<CString>,
    pointers: Vec<*mut c_char>,
}

impl Argv {
    fn new() -> Self {
        let strings: Vec<CString> = std::env::args_os()
            .filter_map(|arg| CString::new(arg.as_bytes()).ok())
            .collect();
        let pointers = strings.iter().map(|arg| arg.as_ptr().cast_mut()).collect();
        Self {
            _strings: strings,
            pointers,
        }
    }

    fn main_args(&mut self) -> MainArgs {
        MainArgs {
            argc: c_int::try_from(self.pointers.len()).unwrap_or(c_int::MAX),
            argv: self.pointers.as_mut_ptr(),
        }
    }
}

/// The whole of a Chromium helper process: what `ket Helper` and its GPU,
/// Renderer, Plugin and Alerts variants run, and the exit code to leave
/// with. Not for ket itself.
///
/// The sandbox comes first, then the framework, as macOS's sandbox requires;
/// a helper without the sandbox started is killed by Chromium (exit code 5).
pub fn helper_main() -> i32 {
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("ket Helper: cannot tell where it is running from");
        return 1;
    };
    // `<Frameworks>/ket Helper.app/Contents/MacOS/ket Helper`.
    let Some(frameworks) = exe.ancestors().nth(4) else {
        eprintln!(
            "ket Helper: {} is not inside a helper bundle",
            exe.display()
        );
        return 1;
    };
    for needed in [
        frameworks.join(FRAMEWORK).join(SANDBOX),
        frameworks
            .join(FRAMEWORK)
            .join("Chromium Embedded Framework"),
    ] {
        if !needed.is_file() {
            eprintln!("ket Helper: there is no {}", needed.display());
            return 1;
        }
    }
    let mut argv = Argv::new();
    let args = argv.main_args();
    let mut sandbox = cef::sandbox::Sandbox::new();
    sandbox.initialize(&args);
    let loader = LibraryLoader::new(&exe, true);
    if !loader.load() {
        eprintln!("ket Helper: the Chromium framework would not load");
        return 1;
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let code = cef::execute_process(Some(&args), None, std::ptr::null_mut());
    // -1 means this is the browser process, which a helper never is.
    if code < 0 { 1 } else { code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_has_to_be_beneath_the_root_cache() {
        let root = Path::new("/data/ket/chromium");
        assert!(within(root, Path::new("/data/ket/chromium/profiles/abc")));
        assert!(!within(root, root));
        assert!(!within(root, Path::new("/data/ket/other")));
        assert!(!within(root, Path::new("/data/ket/chromium-2/abc")));
        assert!(!within(root, Path::new("/data/ket/chromium/../escape")));
        assert!(!within(root, Path::new("chromium/abc")));
    }
}
