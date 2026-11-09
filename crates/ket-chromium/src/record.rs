//! Recording a page to a video: Chromium's screencast — a JPEG each time the
//! page repaints — written to an MP4 by macOS's own H.264 encoder, with a few
//! key frames beside it as PNGs for coding agents that read images but not
//! video.
//!
//! Two threads of its own, neither the caller's nor the main one:
//!
//! - the listener takes each `Page.screencastFrame` as it comes, acknowledges
//!   it at once so Chromium keeps sending, and places it on the video's
//!   timeline ([`Timeline`]);
//! - the encoder decodes the JPEGs, fits each to the video's frame
//!   ([`place`]), hands it to [`writer`], and keeps a few for key frames
//!   ([`KeyFrames`]); [`frames`] works out all three.
//!
//! Between them a [`Queue`] of a few frames, which drops the oldest it has
//! not decoded when the encoder falls behind, rather than growing.
//!
//! Frames come only when the page repaints, so each goes where Chromium's own
//! timestamp puts it and the video plays at the speed the page changed; the
//! last is held until the moment recording stopped.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use chromiumoxide_cdp::cdp::browser_protocol::page::{
    EventScreencastFrame, ScreencastFrameAckParams, StartScreencastFormat, StartScreencastParams,
    StopScreencastParams,
};
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use objc2::AllocAnyThread;
use objc2_foundation::{NSData, NSDataBase64DecodingOptions, NSString};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, oneshot};

use crate::{Cdp, CdpEvent, Error, failed};

mod frames;
/// AVFoundation, CoreMedia and CoreVideo, which have no safe binding: every
/// message `objc2-av-foundation` generates is `unsafe`, as are CoreVideo's
/// pixel-buffer calls. This module holds those calls and nothing else.
#[allow(unsafe_code)]
mod writer;

use frames::{KeyFrames, Placement, Timeline, compose, even, place};
use writer::Writer;

/// How many key frames a recording saves beside its video.
const KEY_FRAMES: u32 = 6;

/// How many frames may wait for the encoder before the oldest is dropped.
const QUEUE: usize = 4;

/// The JPEG quality Chromium encodes the screencast at.
const QUALITY: i64 = 80;

/// The narrowest and widest [`RecordOptions::max_width`] may ask for; 4096 is
/// as wide as H.264 encoders on a Mac go.
const WIDTHS: (u32, u32) = (64, 4096);

/// How a recording is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordOptions {
    /// The longest the video may run, in seconds, at least one. Once it is
    /// that long no more frames are taken, and the recording waits to be
    /// stopped.
    pub max_seconds: u32,
    /// The widest the video's frames may be, in pixels, from 64 to 4096;
    /// Chromium scales a wider page down. A page taller than it is wide is
    /// held to the same height.
    pub max_width: u32,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self {
            max_seconds: 120,
            max_width: 1920,
        }
    }
}

/// What a finished recording made.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    /// The MP4.
    pub video: PathBuf,
    /// Up to six PNGs beside it, `<stem>-1.png` on: frames evenly spaced
    /// through the video, in order, the last its final frame. A frame on
    /// screen for several of those moments is saved once.
    pub frames: Vec<PathBuf>,
    /// How long the video runs.
    pub seconds: f64,
}

/// A page being recorded. [`Recorder::stop`] finishes the video; dropped
/// without it, the recording is abandoned: the screencast stops and the
/// partial file is removed. A page that closes while it is being recorded
/// ends the video there, and `stop` still finishes it.
pub struct Recorder {
    cdp: Cdp,
    stop: oneshot::Sender<Instant>,
    done: oneshot::Receiver<Result<Recording, Error>>,
}

impl Recorder {
    /// Starts recording the page behind `cdp` into `out`, an `.mp4` whose
    /// folder exists. A file already at `out` is replaced.
    pub async fn start(cdp: Cdp, out: PathBuf, options: RecordOptions) -> Result<Self, Error> {
        make_way(&out)?;
        let limit = Duration::from_secs(u64::from(options.max_seconds.max(1)));
        let widest = i64::from(options.max_width.clamp(WIDTHS.0, WIDTHS.1));
        let listener = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| failed(format!("could not start recording: {e}")))?;
        let queue = Arc::new(Queue::new(QUEUE));

        let (done, finished) = oneshot::channel();
        let encoder = Arc::clone(&queue);
        spawn("ket-record-encode", move || {
            let _ = done.send(encode(&encoder, &out));
        })?;

        // Listening before the screencast starts, so no frame is missed.
        let events = cdp.events();
        let started = Instant::now();
        let (stop, stopped) = oneshot::channel();
        let (page, frames) = (cdp.clone(), Arc::clone(&queue));
        let listening = spawn("ket-record-listen", move || {
            // However the listener ends, the encoder hears of it.
            let _ending = Ending(Arc::clone(&frames));
            listener.block_on(listen(page, events, &frames, started, limit, stopped));
        });
        if let Err(error) = listening {
            queue.end(End::Abandon);
            return Err(error);
        }

        let screencast = StartScreencastParams::builder()
            .format(StartScreencastFormat::Jpeg)
            .quality(QUALITY)
            .max_width(widest)
            .max_height(widest)
            .build();
        // On failure `stop` drops, and the listener abandons the recording.
        cdp.call(screencast)
            .await
            .map_err(|e| failed(format!("could not start recording the page: {e}")))?;
        Ok(Self {
            cdp,
            stop,
            done: finished,
        })
    }

    /// Stops, finishes the file, and says what was made. Takes as long as the
    /// last few frames, the file's finishing and the key frames' PNGs do:
    /// usually well under a second.
    pub async fn stop(self) -> Result<Recording, Error> {
        let Self { cdp, stop, done } = self;
        // The listener first: a frame after this moment is not the
        // recording's.
        let _ = stop.send(Instant::now());
        // A closed page has no screencast left to stop.
        let _ = cdp.call(StopScreencastParams::default()).await;
        done.await
            .unwrap_or_else(|_| Err(failed("the recording stopped unexpectedly")))
    }
}

/// Clears the way for a video at `out`: its folder has to exist, and what is
/// there already goes.
fn make_way(out: &Path) -> Result<(), Error> {
    let folder = out
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !folder.is_dir() {
        return Err(failed(format!(
            "cannot record to {}: there is no folder {}",
            out.display(),
            folder.display()
        )));
    }
    if out.is_dir() {
        return Err(failed(format!(
            "cannot record to {}: it is a folder",
            out.display()
        )));
    }
    match std::fs::remove_file(out) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failed(format!("cannot replace {}: {error}", out.display()))),
    }
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), Error> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .map(drop)
        .map_err(|e| failed(format!("could not start recording: {e}")))
}

/// Takes the page's screencast frames, `started` being when recording began,
/// until told to stop — or, if the recorder is dropped, abandons.
async fn listen(
    cdp: Cdp,
    mut events: broadcast::Receiver<CdpEvent>,
    queue: &Queue,
    started: Instant,
    limit: Duration,
    mut stop: oneshot::Receiver<Instant>,
) {
    let mut timeline = Timeline::new(limit);
    // Chromium's id for the screencast, which every acknowledgement names.
    let mut session = None;
    // When the page closed, if it has: the video ends there.
    let mut closed = None;
    let mut taking = true;
    let stopped = loop {
        tokio::select! {
            biased;
            stopped = &mut stop => break stopped.ok(),
            event = events.recv(), if closed.is_none() => match event {
                Ok(event) if event.method == EventScreencastFrame::IDENTIFIER => {
                    let arrived = started.elapsed();
                    let frame: EventScreencastFrame = match serde_json::from_value(event.params) {
                        Ok(frame) => frame,
                        Err(error) => {
                            tracing::debug!(%error, "an unreadable screencast frame");
                            continue;
                        }
                    };
                    session = Some(frame.session_id);
                    acknowledge(&cdp, frame.session_id);
                    if !taking {
                        continue;
                    }
                    let stamp = frame.metadata.timestamp.as_ref().map(|t| *t.inner());
                    match timeline.place(arrived, stamp) {
                        Some(at) => queue.push(Frame { at, jpeg: frame.data.into() }),
                        None => {
                            // Full: Chromium need not encode frames nobody takes.
                            taking = false;
                            let cdp = cdp.clone();
                            tokio::spawn(async move {
                                let _ = cdp.call(StopScreencastParams::default()).await;
                            });
                        }
                    }
                }
                Ok(_) => {}
                // Frames missed here were never acknowledged, and Chromium
                // sends no more while two are outstanding.
                Err(RecvError::Lagged(missed)) => {
                    if let Some(session) = session {
                        for _ in 0..missed.min(2) {
                            acknowledge(&cdp, session);
                        }
                    }
                }
                // The page closed. What was recorded is still the stop's to
                // finish.
                Err(RecvError::Closed) => closed = Some(started.elapsed()),
            },
        }
    };
    match stopped {
        Some(at) => {
            let stopped = at.saturating_duration_since(started);
            let stopped = closed.map_or(stopped, |closed: Duration| closed.min(stopped));
            queue.end(End::Finish(timeline.end(stopped)));
        }
        None => {
            queue.end(End::Abandon);
            let _ = cdp.call(StopScreencastParams::default()).await;
        }
    }
}

/// Tells Chromium a frame arrived. Sent, not called: an answer and a
/// timeout waiting for each of sixty frames a second would add up.
fn acknowledge(cdp: &Cdp, session: i64) {
    cdp.send(ScreencastFrameAckParams::new(session));
}

/// A frame waiting for the encoder: where it goes, and Chromium's JPEG, still
/// in base64.
struct Frame {
    at: Duration,
    jpeg: String,
}

/// How a recording ended, as the encoder hears it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum End {
    /// Stopped: the video runs this long.
    Finish(Duration),
    /// Dropped without stopping, or the listener could not go on.
    Abandon,
}

/// What the encoder is handed next.
enum Next {
    Frame(Frame),
    End(End),
}

/// The frames between listener and encoder, at most `capacity` of them.
struct Queue {
    waiting: Mutex<Waiting>,
    changed: Condvar,
    capacity: usize,
}

#[derive(Default)]
struct Waiting {
    frames: VecDeque<Frame>,
    end: Option<End>,
    dropped: u64,
}

impl Queue {
    fn new(capacity: usize) -> Self {
        Self {
            waiting: Mutex::default(),
            changed: Condvar::new(),
            capacity: capacity.max(1),
        }
    }

    /// Queues `frame`, dropping the oldest still waiting if it is full.
    fn push(&self, frame: Frame) {
        let mut waiting = lock(&self.waiting);
        if waiting.frames.len() >= self.capacity {
            waiting.frames.pop_front();
            waiting.dropped += 1;
        }
        waiting.frames.push_back(frame);
        self.changed.notify_one();
    }

    /// Tells the encoder how the recording ended; the first word stands.
    fn end(&self, end: End) {
        lock(&self.waiting).end.get_or_insert(end);
        self.changed.notify_one();
    }

    /// The next frame, waiting for one; once there are none left, how the
    /// recording ended. Abandoning skips what is left.
    fn next(&self) -> Next {
        let mut waiting = lock(&self.waiting);
        loop {
            if waiting.end == Some(End::Abandon) {
                return Next::End(End::Abandon);
            }
            if let Some(frame) = waiting.frames.pop_front() {
                return Next::Frame(frame);
            }
            if let Some(end) = waiting.end {
                return Next::End(end);
            }
            waiting = self
                .changed
                .wait(waiting)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// How many frames were dropped because the encoder was behind.
    fn dropped(&self) -> u64 {
        lock(&self.waiting).dropped
    }
}

/// Abandons the recording when the listener ends without saying how, as a
/// panic would end it, so the encoder never waits for nothing.
struct Ending(Arc<Queue>);

impl Drop for Ending {
    fn drop(&mut self) {
        self.0.end(End::Abandon);
    }
}

/// Writes the frames `queue` hands over into the video at `out`, and its key
/// frames beside it, until it hears how the recording ended.
fn encode(queue: &Queue, out: &Path) -> Result<Recording, Error> {
    let mut video = None;
    let mut keys = KeyFrames::new();
    let mut failure = None;
    let end = loop {
        match queue.next() {
            Next::End(End::Finish(end)) => break end,
            Next::End(End::Abandon) => return Err(failed("the recording was abandoned")),
            Next::Frame(_) if failure.is_some() => {}
            Next::Frame(frame) => {
                let added = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    objc2::rc::autoreleasepool(|_| add(&mut video, &mut keys, frame, out))
                }));
                failure = match added {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some(failed("a frame could not be encoded")),
                };
            }
        }
    };
    // Dropping the writer abandons the file.
    if let Some(error) = failure {
        return Err(error);
    }
    let Some(video) = video else {
        return Err(failed("the page didn't paint while it was being recorded"));
    };
    let dropped = queue.dropped();
    if dropped > 0 {
        tracing::debug!(dropped, "frames dropped while the encoder was behind");
    }
    let runs = video.finish(end)?;
    Ok(Recording {
        video: out.to_owned(),
        frames: save_key_frames(keys.pick(runs, KEY_FRAMES), out),
        seconds: runs.as_secs_f64(),
    })
}

/// Decodes `frame` and adds it to the video, which the first frame makes, at
/// its size. A frame that will not decode is passed over.
fn add(
    video: &mut Option<Writer>,
    keys: &mut KeyFrames<Rc<[u8]>>,
    frame: Frame,
    out: &Path,
) -> Result<(), Error> {
    let Some(jpeg) = from_base64(&frame.jpeg) else {
        tracing::debug!("a screencast frame that is not base64");
        return Ok(());
    };
    let image = match image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg) {
        Ok(image) => image.into_rgb8(),
        Err(error) => {
            tracing::debug!(%error, "a screencast frame that would not decode");
            return Ok(());
        }
    };
    if video.is_none() {
        let (width, height) = even(image.dimensions());
        *video = Some(Writer::create(out, width, height)?);
    }
    let Some(writer) = video.as_mut() else {
        return Ok(());
    };
    // The video opens on its first frame, wherever that one fell: frames
    // before it may have been dropped.
    let at = if writer.is_empty() {
        Duration::ZERO
    } else {
        frame.at
    };
    let size = writer.size();
    let placement = place(image.dimensions(), size);
    let image = match placement {
        Placement::Copy => image,
        Placement::Fit { width, height, .. } => DynamicImage::from(image)
            .resize_exact(width, height, FilterType::Triangle)
            .into_rgb8(),
    };
    if let Some(at) = writer.append(at, |pixels, stride| {
        compose(&image, placement, size, pixels, stride);
    })? {
        keys.offer(at, Rc::from(jpeg));
    }
    Ok(())
}

/// Base64, decoded by Foundation.
fn from_base64(text: &str) -> Option<Vec<u8>> {
    let data = NSData::initWithBase64EncodedString_options(
        NSData::alloc(),
        &NSString::from_str(text),
        NSDataBase64DecodingOptions::empty(),
    )?;
    Some(data.to_vec())
}

/// Saves `frames`, JPEGs, as PNGs beside `out` — `<stem>-1.png` on — and says
/// where. One that will not save is left out.
fn save_key_frames(frames: Vec<Rc<[u8]>>, out: &Path) -> Vec<PathBuf> {
    let stem = out
        .file_stem()
        .map_or_else(|| "recording".into(), |stem| stem.to_string_lossy());
    let mut saved = Vec::new();
    for (n, jpeg) in frames.iter().enumerate() {
        let path = out.with_file_name(format!("{stem}-{}.png", n + 1));
        let written = image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg)
            .and_then(|image| image.save_with_format(&path, ImageFormat::Png));
        match written {
            Ok(()) => saved.push(path),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "a key frame could not be saved");
            }
        }
    }
    saved
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
