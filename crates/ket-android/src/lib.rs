//! The Android Emulator, for ket's emulator tab.
//!
//! Everything here goes through the emulator's public control API — gRPC,
//! `emulator_controller.proto` from the SDK — the one Android Studio uses to
//! show the emulator in its own window. ket draws the screen itself from the
//! frames this streams, and sends touches and keys back; nothing is embedded
//! and nothing private is called. Checked against emulator 36.6 on macOS:
//!
//! - A running emulator writes a discovery file, `pid_<pid>.ini`, holding its
//!   gRPC port and a random per-instance token; see [`running`].
//! - The token, as `authorization: Bearer`, is all the authentication a local
//!   client needs — in the default mode and in `-grpc-use-token` alike.
//! - Frames are asked for at a size (width *and* height, or the native size
//!   comes back) and arrive only when the screen changes. Touches are in the
//!   device's native pixels whatever size the frames are.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

mod proto {
    #![allow(clippy::all, clippy::pedantic, missing_docs)]
    tonic::include_proto!("android.emulation.control");
}

use proto::emulator_controller_client::EmulatorControllerClient;

/// Why the tab cannot show an emulator, in terms a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No Android SDK where Android Studio puts one, and none named by
    /// `ANDROID_HOME`.
    NoSdk,
    /// An SDK without the emulator installed in it.
    NoEmulator(PathBuf),
    /// An emulator, and no virtual device made for it.
    NoDevices,
    /// Anything else, said plainly.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSdk => write!(f, "no Android SDK was found"),
            Self::NoEmulator(sdk) => write!(f, "the SDK at {} has no emulator", sdk.display()),
            Self::NoDevices => write!(f, "no virtual device has been made"),
            Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

fn failed(why: impl std::fmt::Display) -> Error {
    Error::Failed(why.to_string())
}

// ---- the SDK ------------------------------------------------------------------

/// An Android SDK with an emulator in it.
#[derive(Debug, Clone)]
pub struct Sdk {
    /// The SDK's root.
    pub root: PathBuf,
    emulator: PathBuf,
}

/// The SDK: `ANDROID_HOME`, then `ANDROID_SDK_ROOT`, then where Android
/// Studio installs it.
pub fn sdk() -> Result<Sdk, Error> {
    let named = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let root = named
        .chain(default_sdk())
        .find(|root| root.is_dir())
        .ok_or(Error::NoSdk)?;
    let emulator = root.join("emulator").join(if cfg!(windows) {
        "emulator.exe"
    } else {
        "emulator"
    });
    if !emulator.is_file() {
        return Err(Error::NoEmulator(root));
    }
    Ok(Sdk { root, emulator })
}

/// Where Android Studio puts the SDK on this platform.
fn default_sdk() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("Android").join("Sdk"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(if cfg!(target_os = "macos") {
        home.join("Library").join("Android").join("sdk")
    } else {
        home.join("Android").join("Sdk")
    })
}

/// Whether an SDK with an emulator is installed — cheap: file-system reads
/// only, for deciding whether to offer the tab at all.
pub fn installed() -> bool {
    sdk().is_ok()
}

impl Sdk {
    /// The virtual devices made for this emulator, by id — `Pixel_8_Pro_API_36`.
    pub fn devices(&self) -> Result<Vec<String>, Error> {
        let output = Command::new(&self.emulator)
            .arg("-list-avds")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| failed(format!("could not run the emulator: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            // It prints warnings on the same stream on some setups.
            .filter(|line| !line.is_empty() && !line.contains(' ') && !line.starts_with("INFO"))
            .map(str::to_owned)
            .collect())
    }

    /// Boots `avd` without a window of its own, for ket to show, and returns
    /// once it has said where to reach it — not once Android has finished
    /// starting; see [`Device::status`]. Its gRPC server listens on loopback
    /// only and takes only its token (`-grpc-use-token`); a bare `-grpc`
    /// would listen on every interface with no authentication at all.
    pub fn boot(&self, avd: &str) -> Result<Running, Error> {
        if let Some(running) = running().into_iter().find(|r| r.avd == avd) {
            return Ok(running);
        }
        let grpc = free_port().ok_or_else(|| failed("no free port for the emulator"))?;
        let console =
            free_console_port().ok_or_else(|| failed("no free console port for the emulator"))?;
        let mut command = Command::new(&self.emulator);
        command
            .args(["-avd", avd, "-no-window", "-no-audio", "-no-boot-anim"])
            .args(["-grpc", &grpc.to_string(), "-grpc-use-token"])
            .args(["-port", &console.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            // Its own process group: a Ctrl-C meant for `cargo run` in a
            // terminal is not for the emulator.
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|e| failed(format!("could not start the emulator: {e}")))?;
        let pid = child.id();
        // The launcher execs the emulator itself, so the discovery file is
        // named after this pid. It appears within a second or so.
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline {
            if let Some(running) = running().into_iter().find(|r| r.pid == pid) {
                return Ok(running);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(failed(format!(
                    "the emulator stopped as it started ({status})"
                )));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = child.kill();
        Err(failed("the emulator did not start in time"))
    }
}

/// A port nothing is listening on now.
fn free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()?
        .local_addr()
        .ok()
        .map(|address| address.port())
}

/// An even console port, with the odd one above it free for adb — the pair
/// every emulator takes, from 5554.
fn free_console_port() -> Option<u16> {
    let free = |port: u16| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
    (5554..=5680)
        .step_by(2)
        .find(|&port| free(port) && free(port + 1))
}

// ---- running emulators ----------------------------------------------------------

/// An emulator that is running, as its discovery file describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// Its process.
    pub pid: u32,
    /// The virtual device's id — `Pixel_8_Pro_API_36`.
    pub avd: String,
    /// Its name for people — `Pixel 8 Pro API 36`.
    pub name: String,
    /// Where its gRPC server listens, on loopback.
    pub port: u16,
    /// What authenticates a client.
    token: String,
    /// Its adb serial — `emulator-5554`.
    pub serial: String,
    /// Its console's port, which the serial is named after.
    console: Option<u16>,
}

impl Running {
    /// Stops it, as Android Studio does: `kill` on its console, which ends it
    /// in a second or two. Not the gRPC shutdown, which is a graceful one
    /// that takes most of a minute, and not through adb, whose server —
    /// started by the call — may not have found the emulator yet.
    pub fn stop(&self) -> Result<(), Error> {
        use std::io::{Read, Write};
        let port = self
            .console
            .ok_or_else(|| failed("the emulator did not say where its console is"))?;
        let mut console = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_secs(2),
        )
        .map_err(|e| failed(format!("could not reach the emulator's console: {e}")))?;
        console
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(failed)?;
        // Reads until the console's `OK` prompt, keeping what it said.
        let mut said = String::new();
        let prompt = |console: &mut std::net::TcpStream, said: &mut String| {
            let mut buffer = [0u8; 1024];
            while !said.contains("OK\r\n") && !said.contains("KO") {
                match console.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => said.push_str(&String::from_utf8_lossy(&buffer[..n])),
                }
            }
        };
        prompt(&mut console, &mut said);
        if said.contains("Authentication required") {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .ok_or_else(|| failed("no home folder for the console's token"))?;
            let token = std::fs::read_to_string(home.join(".emulator_console_auth_token"))
                .map_err(|e| failed(format!("could not read the console's token: {e}")))?;
            said.clear();
            console
                .write_all(format!("auth {}\n", token.trim()).as_bytes())
                .map_err(failed)?;
            prompt(&mut console, &mut said);
            if !said.contains("OK") {
                return Err(failed("the emulator's console refused the token"));
            }
        }
        console.write_all(b"kill\n").map_err(failed)?;
        Ok(())
    }
}

/// Where emulators write their discovery files.
fn discovery_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        return Some(home.join("Library/Caches/TemporaryItems/avd/running"));
    }
    if cfg!(windows) {
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        return Some(local.join("Temp").join("avd").join("running"));
    }
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("avd").join("running"))
}

/// Whether process `pid` exists. An emulator stopped with `adb emu kill`
/// leaves its discovery file behind.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // Signal 0 checks without sending; EPERM is a process that exists.
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(error) => error == nix::errno::Errno::EPERM,
    }
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// The emulators that say they are running, newest first — only those whose
/// process is still there. A process id can be reused;
/// [`Device::connect`] is what proves an emulator is really there.
pub fn running() -> Vec<Running> {
    let Some(dir) = discovery_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, Running)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, read_discovery(&path)?))
        })
        .filter(|(_, running)| alive(running.pid))
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, running)| running).collect()
}

fn read_discovery(path: &Path) -> Option<Running> {
    let pid = path
        .file_name()?
        .to_str()?
        .strip_prefix("pid_")?
        .strip_suffix(".ini")?
        .parse()
        .ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    Some(Running {
        pid,
        avd: value("avd.id")?,
        name: value("avd.name").unwrap_or_default(),
        port: value("grpc.port")?.parse().ok()?,
        token: value("grpc.token")?,
        serial: value("port.serial").map_or_else(String::new, |port| format!("emulator-{port}")),
        console: value("port.serial").and_then(|port| port.parse().ok()),
    })
}

// ---- a device -------------------------------------------------------------------

/// One frame of the screen, ready to draw: BGRA, rows top-down.
#[derive(Debug)]
pub struct Frame {
    /// In pixels.
    pub width: u32,
    /// In pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub bgra: Vec<u8>,
}

/// Where a device is in coming up, and whether it is still there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Android is still starting.
    Booting,
    /// Android is up.
    Ready,
    /// The connection is over, and why.
    Ended(String),
}

/// A key the emulator has a name for — the hardware buttons among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Home.
    Home,
    /// Back.
    Back,
    /// Overview — the recent apps.
    Overview,
    /// Power: sleep and wake.
    Power,
    /// Volume up.
    VolumeUp,
    /// Volume down.
    VolumeDown,
}

impl Button {
    /// The emulator's name for it. `GoHome`, not `Home`, which it sends as a
    /// different key altogether.
    fn key(self) -> &'static str {
        match self {
            Self::Home => "GoHome",
            Self::Back => "GoBack",
            Self::Overview => "AppSwitch",
            Self::Power => "Power",
            Self::VolumeUp => "AudioVolumeUp",
            Self::VolumeDown => "AudioVolumeDown",
        }
    }
}

enum Command_ {
    Touch { x: i32, y: i32, down: bool },
    Key(String),
    Text(String),
    Screenshot(std::sync::mpsc::Sender<Result<Vec<u8>, String>>),
}

/// A connection to one running emulator: its frames and status as they
/// change, and its input. Runs on a thread of its own, which ends when this
/// is dropped or the emulator goes.
pub struct Device {
    /// The device's screen in its own pixels — what touches are measured in.
    pub native: (u32, u32),
    /// Its name for people.
    pub name: String,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    status: watch::Receiver<Status>,
    size: watch::Sender<(u32, u32)>,
    commands: mpsc::UnboundedSender<Command_>,
}

type Client = EmulatorControllerClient<
    tonic::service::interceptor::InterceptedService<tonic::transport::Channel, Bearer>,
>;

/// Adds the emulator's token to every call.
#[derive(Clone)]
struct Bearer(tonic::metadata::MetadataValue<tonic::metadata::Ascii>);

impl tonic::service::Interceptor for Bearer {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        request
            .metadata_mut()
            .insert("authorization", self.0.clone());
        Ok(request)
    }
}

impl Device {
    /// Connects to `running`, and blocks until it has answered — which is
    /// what proves a discovery file is not left over from an emulator that
    /// has gone.
    pub fn connect(running: &Running) -> Result<Device, Error> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (frames_tx, frames) = watch::channel(None);
        let (status_tx, status) = watch::channel(Status::Booting);
        let (size, size_rx) = watch::channel((0, 0));
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let port = running.port;
        let token = running.token.clone();
        std::thread::Builder::new()
            .name("ket-android".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        let _ = ready_tx.send(Err(failed(e)));
                        return;
                    }
                };
                runtime.block_on(serve(
                    port,
                    token,
                    ready_tx,
                    frames_tx,
                    status_tx,
                    size_rx,
                    commands_rx,
                ));
            })
            .map_err(failed)?;
        let native = ready_rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| failed("the emulator did not answer"))??;
        Ok(Device {
            native,
            name: if running.name.is_empty() {
                running.avd.replace('_', " ")
            } else {
                running.name.clone()
            },
            frames,
            status,
            size,
            commands,
        })
    }

    /// The latest frame, as it changes.
    pub fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.frames.clone()
    }

    /// Where it is in coming up, as it changes.
    pub fn status(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    /// Asks for frames of about this size, in pixels, keeping the screen's
    /// shape; the stream starts again at it. `(0, 0)` stops frames.
    pub fn frame_size(&self, width: u32, height: u32) {
        let (w, h) = fit(self.native, width, height);
        self.size.send_if_modified(|size| {
            let changed = *size != (w, h);
            *size = (w, h);
            changed
        });
    }

    /// A finger down, moved or lifted, at `(x, y)` in [`Device::native`] pixels.
    pub fn touch(&self, x: i32, y: i32, down: bool) {
        let _ = self.commands.send(Command_::Touch { x, y, down });
    }

    /// Presses and releases a button.
    pub fn press(&self, button: Button) {
        let _ = self.commands.send(Command_::Key(button.key().to_owned()));
    }

    /// Presses and releases a key by the emulator's name for it —
    /// `Backspace`, `Enter`, `ArrowLeft` — the names a browser gives keys.
    pub fn key(&self, name: &str) {
        let _ = self.commands.send(Command_::Key(name.to_owned()));
    }

    /// Types text. Printable ASCII; anything else is dropped by the emulator.
    pub fn text(&self, text: &str) {
        let _ = self.commands.send(Command_::Text(text.to_owned()));
    }

    /// Asks for a PNG of the screen at its own size — not the frames' size,
    /// which is the pane's. The answer arrives on the receiver; wait for it
    /// off the UI thread.
    pub fn screenshot(&self) -> std::sync::mpsc::Receiver<Result<Vec<u8>, String>> {
        let (answer, answered) = std::sync::mpsc::channel();
        if let Err(tokio::sync::mpsc::error::SendError(Command_::Screenshot(answer))) =
            self.commands.send(Command_::Screenshot(answer))
        {
            let _ = answer.send(Err("the emulator has gone".to_owned()));
        }
        answered
    }
}

/// The largest size at the screen's shape that fits in `width` × `height`,
/// never larger than the screen itself. Both sides are asked for: the
/// emulator ignores a size with only one.
fn fit(native: (u32, u32), width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 || native.0 == 0 || native.1 == 0 {
        return (0, 0);
    }
    let scale = (f64::from(width) / f64::from(native.0))
        .min(f64::from(height) / f64::from(native.1))
        .min(1.0);
    let w = (f64::from(native.0) * scale).round().max(1.0) as u32;
    let h = (f64::from(native.1) * scale).round().max(1.0) as u32;
    (w, h)
}

async fn serve(
    port: u16,
    token: String,
    ready: std::sync::mpsc::Sender<Result<(u32, u32), Error>>,
    frames: watch::Sender<Option<Arc<Frame>>>,
    status: watch::Sender<Status>,
    mut size: watch::Receiver<(u32, u32)>,
    mut commands: mpsc::UnboundedReceiver<Command_>,
) {
    // A just-started emulator writes its discovery file a moment before its
    // gRPC server is listening: it gets a few seconds to start answering.
    let deadline = Instant::now() + Duration::from_secs(15);
    let (client, native) = loop {
        let attempt = match open(port, &token).await {
            Ok(client) => native_size(&client).await.map(|native| (client, native)),
            Err(e) => Err(e),
        };
        match attempt {
            Ok(found) => break found,
            Err(e) if Instant::now() >= deadline => {
                let _ = ready.send(Err(e));
                return;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    };
    let _ = ready.send(Ok(native));

    // Booted yet: asked every half second until it is.
    let booting = {
        let mut client = client.clone();
        let status = status.clone();
        tokio::spawn(async move {
            loop {
                match client.get_status(()).await {
                    Ok(reply) if reply.get_ref().booted => {
                        status.send_if_modified(|s| {
                            let was = *s == Status::Booting;
                            if was {
                                *s = Status::Ready;
                            }
                            was
                        });
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = status.send(Status::Ended(e.message().to_owned()));
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
    };

    // Input, in order, as it comes.
    let input = {
        let mut client = client.clone();
        tokio::spawn(async move {
            while let Some(command) = commands.recv().await {
                let sent = match command {
                    Command_::Touch { x, y, down } => client
                        .send_touch(proto::TouchEvent {
                            touches: vec![proto::Touch {
                                x,
                                y,
                                identifier: 1,
                                pressure: i32::from(down),
                                ..Default::default()
                            }],
                            display: 0,
                        })
                        .await
                        .map(|_| ()),
                    Command_::Key(key) => client
                        .send_key(proto::KeyboardEvent {
                            key,
                            event_type: proto::keyboard_event::KeyEventType::Keypress as i32,
                            ..Default::default()
                        })
                        .await
                        .map(|_| ()),
                    Command_::Text(text) => client
                        .send_key(proto::KeyboardEvent {
                            text,
                            event_type: proto::keyboard_event::KeyEventType::Keypress as i32,
                            ..Default::default()
                        })
                        .await
                        .map(|_| ()),
                    Command_::Screenshot(answer) => {
                        let shot = client
                            .get_screenshot(proto::ImageFormat {
                                format: proto::image_format::ImgFormat::Png as i32,
                                ..Default::default()
                            })
                            .await
                            .map(|reply| reply.into_inner().image)
                            .map_err(|e| e.message().to_owned());
                        let _ = answer.send(shot);
                        Ok(())
                    }
                };
                if let Err(e) = sent {
                    tracing::debug!(error = %e.message(), "an emulator input was refused");
                }
            }
        })
    };

    // Frames at the size asked for, again from the start whenever it changes.
    let mut client = client;
    loop {
        let (width, height) = *size.borrow_and_update();
        if width == 0 {
            if size.changed().await.is_err() {
                break;
            }
            continue;
        }
        let stream = client
            .stream_screenshot(proto::ImageFormat {
                format: proto::image_format::ImgFormat::Rgba8888 as i32,
                width,
                height,
                ..Default::default()
            })
            .await;
        let mut stream = match stream {
            Ok(stream) => stream.into_inner(),
            Err(e) => {
                let _ = status.send(Status::Ended(e.message().to_owned()));
                break;
            }
        };
        let ended = loop {
            tokio::select! {
                message = stream.message() => match message {
                    Ok(Some(image)) => {
                        if let Some(frame) = to_frame(image) {
                            frames.send_replace(Some(Arc::new(frame)));
                        }
                    }
                    Ok(None) => break Some("the emulator stopped sending its screen".to_owned()),
                    Err(e) => break Some(e.message().to_owned()),
                },
                changed = size.changed() => {
                    if changed.is_err() {
                        break Some(String::new());
                    }
                    break None;
                }
            }
        };
        if let Some(why) = ended {
            if !why.is_empty() {
                let _ = status.send(Status::Ended(why));
            }
            break;
        }
    }
    booting.abort();
    input.abort();
}

async fn open(port: u16, token: &str) -> Result<Client, Error> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .map_err(failed)?
        .connect_timeout(Duration::from_secs(3))
        .connect()
        .await
        .map_err(|_| failed("the emulator is not answering — it may have stopped"))?;
    let bearer = format!("Bearer {token}").parse().map_err(failed)?;
    Ok(
        EmulatorControllerClient::with_interceptor(channel, Bearer(bearer))
            // A frame at the device's full size is 16 MB; tonic's default is 4.
            .max_decoding_message_size(64 << 20),
    )
}

/// The screen's size in its own pixels, from the display configuration.
async fn native_size(client: &Client) -> Result<(u32, u32), Error> {
    let mut client = client.clone();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        client.get_display_configurations(()),
    )
    .await
    .map_err(|_| failed("the emulator did not answer"))?
    .map_err(|e| failed(format!("the emulator refused ket: {}", e.message())))?;
    reply
        .into_inner()
        .displays
        .into_iter()
        .find(|display| display.display == 0)
        .map(|display| (display.width, display.height))
        .filter(|&(w, h)| w > 0 && h > 0)
        .ok_or_else(|| failed("the emulator has no screen"))
}

/// An RGBA frame from the emulator as BGRA, the order ket draws in.
fn to_frame(image: proto::Image) -> Option<Frame> {
    let format = image.format?;
    let (width, height) = (format.width, format.height);
    let mut bgra = image.image;
    if bgra.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Frame {
        width,
        height,
        bgra,
    })
}
