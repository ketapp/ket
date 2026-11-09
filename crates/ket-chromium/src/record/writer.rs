//! The MP4 itself, written by macOS's own H.264 encoder through
//! AVFoundation's `AVAssetWriter`. Frames go in as 32-bit BGRA pixel buffers
//! from the writer's own CoreVideo pool, each stamped with when it went on
//! screen. A writer lives and dies on the thread that made it; dropped
//! unfinished, it cancels and takes its partial file away.
//!
//! This is where `record`'s `unsafe` is, and all it holds: every message
//! `objc2-av-foundation` generates is `unsafe`, as are reading the frameworks'
//! string constants, making and locking a CoreVideo pixel buffer, and taking
//! ownership of the one made. AVFoundation reports misuse by throwing an
//! Objective-C exception; [`objc`] catches what a call throws and says so,
//! rather than letting it unwind into ket.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::exception;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
    AVVideoMaxKeyFrameIntervalDurationKey, AVVideoWidthKey,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferPool, CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

use crate::{Error, failed};

/// Presentation times are in milliseconds.
const TIMESCALE: i32 = 1000;

/// How long a frame waits for a busy encoder before it is dropped, as a live
/// recording drops what it cannot keep up with.
const READY_WAIT: Duration = Duration::from_millis(250);

/// How long the closing frame waits for a busy encoder.
const CLOSING_WAIT: Duration = Duration::from_secs(2);

/// The longest the file may take to finish once its frames are in.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest stretch of video without a key frame, in seconds, so a player
/// can seek anywhere without decoding from the start.
const KEY_FRAME_EVERY: f64 = 2.0;

/// An MP4 being written: one H.264 video track, every frame one size.
pub(super) struct Writer {
    path: PathBuf,
    width: u32,
    height: u32,
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    /// The latest frame and its time in ticks, repeated at the end so the
    /// video runs until recording stopped.
    last: Option<(CFRetained<CVPixelBuffer>, i64)>,
    finished: bool,
}

impl Writer {
    /// Starts an MP4 at `path`, `width` by `height` pixels, both even. A file
    /// already there is left alone and this fails.
    pub(super) fn create(path: &Path, width: u32, height: u32) -> Result<Self, Error> {
        if path.exists() {
            return Err(failed(format!("{} is already there", path.display())));
        }
        let url = NSURL::from_file_path(path)
            .ok_or_else(|| failed(format!("cannot write a video to {}", path.display())))?;
        // SAFETY: AVFoundation's constants, weakly linked, so each an Option.
        let (file_type, media_type) = unsafe { (AVFileTypeMPEG4, AVMediaTypeVideo) };
        let (Some(file_type), Some(media_type)) = (file_type, media_type) else {
            return Err(failed("this Mac's AVFoundation does not write MP4 video"));
        };
        let writer = objc("starting the video", || {
            // SAFETY: a file URL and a file type AVFoundation declares.
            unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type) }
        })?
        .map_err(|error| {
            failed(format!(
                "could not start a video at {}: {}",
                path.display(),
                error.localizedDescription()
            ))
        })?;
        let settings = video_settings(width, height)?;
        let input = objc("setting up H.264", || {
            // SAFETY: a video input with the keys AVVideoSettings.h names,
            // each value of the type it documents.
            unsafe {
                AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                    media_type,
                    Some(&settings),
                )
            }
        })?;
        // SAFETY: a plain property, set before writing starts.
        unsafe { input.setExpectsMediaDataInRealTime(true) };
        let attributes = pixel_buffer_attributes(width, height);
        let adaptor = objc("setting up frame buffers", || {
            // SAFETY: CoreVideo's own keys, each with the number it takes.
            unsafe {
                AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                    &input,
                    Some(&attributes),
                )
            }
        })?;
        // SAFETY: an input made for this writer's media, before it starts.
        if !unsafe { writer.canAddInput(&input) } {
            return Err(failed(format!(
                "this Mac cannot encode {width}×{height} video"
            )));
        }
        // SAFETY: as above, and only once.
        objc("adding the video track", || unsafe {
            writer.addInput(&input)
        })?;
        let this = Self {
            path: path.to_owned(),
            width,
            height,
            writer,
            input,
            adaptor,
            last: None,
            finished: false,
        };
        // SAFETY: the one input is added and writing has not started.
        if !objc("starting the video", || unsafe {
            this.writer.startWriting()
        })? {
            return Err(this.failure("could not start the video"));
        }
        // SAFETY: writing has started, and no session has.
        objc("starting the video", || unsafe {
            this.writer.startSessionAtSourceTime(time(0));
        })?;
        Ok(this)
    }

    /// The video's frame size.
    pub(super) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether no frame has gone in yet.
    pub(super) fn is_empty(&self) -> bool {
        self.last.is_none()
    }

    /// Adds a frame on screen from `at`, `draw` filling its BGRA pixels — rows
    /// `stride` bytes apart, the frame's size — and says when it went in,
    /// which is `at` unless that was not after the frame before. `None` when
    /// the encoder stayed busy and the frame was dropped.
    pub(super) fn append(
        &mut self,
        at: Duration,
        draw: impl FnOnce(&mut [u8], usize),
    ) -> Result<Option<Duration>, Error> {
        if !self.ready_within(READY_WAIT) {
            return Ok(None);
        }
        let ticks = match &self.last {
            Some((_, last)) => ticks(at).max(last + 1),
            None => ticks(at),
        };
        let buffer = self.pixel_buffer()?;
        fill(&buffer, draw)?;
        self.push(&buffer, ticks)?;
        self.last = Some((buffer, ticks));
        Ok(Some(duration(ticks)))
    }

    /// Finishes the file with the video running until `end`, or just past
    /// its last frame if that is later, and says how long it runs. Fails if
    /// no frame went in, and the file is then removed.
    pub(super) fn finish(mut self, end: Duration) -> Result<Duration, Error> {
        let Some((buffer, last)) = self.last.take() else {
            return Err(failed("the video has no frames"));
        };
        let end = ticks(end).max(last + 1);
        // The last frame again at the end, which gives the real last frame
        // its length: on screen until recording stopped. Past the session's
        // end, the repeat itself is never shown.
        if self.ready_within(CLOSING_WAIT) {
            self.push(&buffer, end)?;
        }
        // SAFETY: the session started in `create` and has not ended.
        objc("ending the video", || unsafe {
            self.writer.endSessionAtSourceTime(time(end));
        })?;
        let (done, finished) = mpsc::channel();
        let handler = RcBlock::new(move || {
            let _ = done.send(());
        });
        // SAFETY: every append has returned, and the handler only sends on a
        // channel, which any thread may.
        objc("finishing the video", || unsafe {
            self.writer.finishWritingWithCompletionHandler(&handler);
        })?;
        if finished.recv_timeout(FINISH_TIMEOUT).is_err() {
            return Err(failed(format!(
                "the video took over {FINISH_TIMEOUT:?} to finish"
            )));
        }
        // SAFETY: a property AVFoundation says any thread may read.
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
            return Err(self.failure("could not finish the video"));
        }
        self.finished = true;
        Ok(duration(end))
    }

    /// Waits up to `wait` for the encoder to take another frame.
    fn ready_within(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        // SAFETY: a property any thread may read.
        while !unsafe { self.input.isReadyForMoreMediaData() } {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }

    /// A buffer from the writer's pool to draw a frame in.
    fn pixel_buffer(&self) -> Result<CFRetained<CVPixelBuffer>, Error> {
        // SAFETY: writing has started, so the pool exists; it throws only if
        // the attributes could not make one, which `objc` catches.
        let pool = objc("getting a frame buffer", || unsafe {
            self.adaptor.pixelBufferPool()
        })?
        .ok_or_else(|| self.failure("there were no frame buffers to draw in"))?;
        let mut made: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: `made` is a valid place for the buffer CoreVideo makes.
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut made))
        };
        // SAFETY: a buffer made by a Create function is ours to release.
        let made = NonNull::new(made).map(|made| unsafe { CFRetained::from_raw(made) });
        match made {
            Some(made) if status == kCVReturnSuccess => Ok(made),
            _ => Err(failed(format!(
                "could not get a frame buffer (CoreVideo error {status})"
            ))),
        }
    }

    /// Hands `buffer` to the encoder at `ticks`.
    fn push(&self, buffer: &CVPixelBuffer, ticks: i64) -> Result<(), Error> {
        // SAFETY: the encoder is ready for it, the session is open, and the
        // time is numeric and after the one before.
        let appended = objc("adding a frame", || unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(buffer, time(ticks))
        })?;
        if appended {
            Ok(())
        } else {
            Err(self.failure("could not add a frame to the video"))
        }
    }

    /// `what` went wrong, and what AVFoundation says about why.
    fn failure(&self, what: &str) -> Error {
        // SAFETY: a property AVFoundation says any thread may read.
        let why = unsafe { self.writer.error() };
        match why {
            Some(why) => failed(format!("{what}: {}", why.localizedDescription())),
            None => failed(what),
        }
    }
}

impl Drop for Writer {
    /// Abandoned: writing stops and the partial file goes.
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // SAFETY: a property any thread may read.
        if unsafe { self.writer.status() } == AVAssetWriterStatus::Writing {
            // SAFETY: nothing is appending; this blocks until cancelled.
            let _ = objc("abandoning the video", || unsafe {
                self.writer.cancelWriting();
            });
        }
        // `create` made sure nothing was there before, so this is ours.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Lets `draw` fill `buffer`'s pixels, rows `stride` bytes apart.
fn fill(buffer: &CVPixelBuffer, draw: impl FnOnce(&mut [u8], usize)) -> Result<(), Error> {
    let flags = CVPixelBufferLockFlags::empty();
    // SAFETY: a buffer from the pool, not yet handed to the encoder.
    if unsafe { CVPixelBufferLockBaseAddress(buffer, flags) } != kCVReturnSuccess {
        return Err(failed("could not draw into a frame buffer"));
    }
    let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
    let stride = CVPixelBufferGetBytesPerRow(buffer);
    let rows = CVPixelBufferGetHeight(buffer);
    let drawn = !base.is_null();
    if drawn {
        // SAFETY: locked, a one-plane buffer's memory is `stride` bytes for
        // each of its rows from `base`, and ours alone until it is unlocked.
        let pixels = unsafe { std::slice::from_raw_parts_mut(base, stride * rows) };
        draw(pixels, stride);
    }
    // SAFETY: locked above with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(buffer, flags) };
    if drawn {
        Ok(())
    } else {
        Err(failed("a frame buffer had no memory to draw in"))
    }
}

/// H.264 at `width` by `height`, at a bit rate sized to the frame.
fn video_settings(
    width: u32,
    height: u32,
) -> Result<Retained<NSDictionary<NSString, AnyObject>>, Error> {
    // SAFETY: AVFoundation's constants, weakly linked, so each an Option.
    let keys = unsafe {
        (
            AVVideoCodecKey,
            AVVideoCodecTypeH264,
            AVVideoWidthKey,
            AVVideoHeightKey,
            AVVideoCompressionPropertiesKey,
            AVVideoAverageBitRateKey,
            AVVideoMaxKeyFrameIntervalDurationKey,
        )
    };
    let (
        Some(codec_key),
        Some(h264),
        Some(width_key),
        Some(height_key),
        Some(compression_key),
        Some(bit_rate_key),
        Some(key_frame_key),
    ) = keys
    else {
        return Err(failed("this Mac's AVFoundation does not encode H.264"));
    };
    let rate = NSNumber::new_u32(bit_rate(width, height));
    let every = NSNumber::new_f64(KEY_FRAME_EVERY);
    let compression: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&[bit_rate_key, key_frame_key], &[&rate, &every]);
    let (width, height) = (NSNumber::new_u32(width), NSNumber::new_u32(height));
    Ok(NSDictionary::from_slices(
        &[codec_key, width_key, height_key, compression_key],
        &[h264, &width, &height, &compression],
    ))
}

/// The pool's buffers: 32-bit BGRA, the video's size.
fn pixel_buffer_attributes(width: u32, height: u32) -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: CoreVideo's constants, always linked.
    let keys: [&NSString; 3] = unsafe {
        [
            kCVPixelBufferPixelFormatTypeKey.as_ref(),
            kCVPixelBufferWidthKey.as_ref(),
            kCVPixelBufferHeightKey.as_ref(),
        ]
    };
    let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let (width, height) = (NSNumber::new_u32(width), NSNumber::new_u32(height));
    NSDictionary::from_slices(&keys, &[&format, &width, &height])
}

/// Bits a second for H.264 at `width` by `height`: three a pixel, which
/// keeps a page's text legible — about 6 Mbit/s at 1920 by 1080.
fn bit_rate(width: u32, height: u32) -> u32 {
    let rate = (u64::from(width) * u64::from(height) * 3).clamp(500_000, 12_000_000);
    u32::try_from(rate).unwrap_or(u32::MAX)
}

fn ticks(at: Duration) -> i64 {
    i64::try_from(at.as_millis()).unwrap_or(i64::MAX)
}

fn duration(ticks: i64) -> Duration {
    Duration::from_millis(u64::try_from(ticks).unwrap_or(0))
}

fn time(ticks: i64) -> CMTime {
    CMTime {
        value: ticks,
        timescale: TIMESCALE,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

/// Runs an AVFoundation call that may throw, saying what was being done if
/// it did.
fn objc<R>(doing: &str, call: impl FnOnce() -> R) -> Result<R, Error> {
    exception::catch(AssertUnwindSafe(call)).map_err(|exception| {
        let why = exception.map_or_else(|| "no reason given".to_owned(), |e| e.to_string());
        failed(format!("{doing} failed: {why}"))
    })
}

/// A video file as a player sees it, read back with `AVURLAsset`.
#[cfg(test)]
#[derive(Debug)]
pub(super) struct Probe {
    pub seconds: f64,
    pub video_tracks: usize,
    pub size: (f64, f64),
}

#[cfg(test)]
pub(super) fn probe(path: &Path) -> Probe {
    use objc2_av_foundation::AVURLAsset;

    let url = NSURL::from_file_path(path).expect("a file URL");
    // SAFETY: test-only reads of a finished file, through AVFoundation's
    // synchronous accessors.
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        let video = AVMediaTypeVideo.expect("AVMediaTypeVideo");
        let tracks: Vec<_> = asset
            .tracks()
            .to_vec()
            .into_iter()
            .filter(|track| &*track.mediaType() == video)
            .collect();
        let size = tracks.first().map(|track| track.naturalSize());
        Probe {
            seconds: asset.duration().seconds(),
            video_tracks: tracks.len(),
            size: size.map_or((0.0, 0.0), |size| (size.width, size.height)),
        }
    }
}

/// A scratch folder for a test's files, removed with them.
#[cfg(test)]
pub(super) struct Scratch(pub PathBuf);

#[cfg(test)]
impl Scratch {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ket-record-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        Self(dir)
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bit_rate_follows_the_frame_within_bounds() {
        assert_eq!(bit_rate(1920, 1080), 6_220_800);
        assert_eq!(bit_rate(64, 64), 500_000);
        assert_eq!(bit_rate(4096, 4096), 12_000_000);
    }

    #[test]
    fn frames_become_an_mp4_that_runs_until_recording_stopped() {
        let scratch = Scratch::new("writer");
        let out = scratch.0.join("frames.mp4");
        let mut video = Writer::create(&out, 640, 360).expect("a writer");
        // Sixty coloured frames, about 30 a second, unevenly.
        let mut added = 0;
        for n in 0..60u8 {
            let at = Duration::from_millis(u64::from(n) * 33 + u64::from(n % 3));
            let colour = [n * 4, 255 - n * 4, 128, 255];
            let drawn = video
                .append(at, |pixels, _| {
                    pixels.as_chunks_mut::<4>().0.fill(colour);
                })
                .expect("a frame");
            added += usize::from(drawn.is_some());
        }
        assert!(added > 50, "only {added} frames went in");
        // A frame no later than the one before goes just after it.
        let last = video.append(Duration::from_millis(100), |_, _| {});
        assert!(matches!(last, Ok(Some(at)) if at > Duration::from_millis(1900)));

        let runs = video.finish(Duration::from_millis(2500)).expect("a video");
        assert_eq!(runs, Duration::from_millis(2500));
        let bytes = std::fs::metadata(&out).expect("the video").len();
        let video = probe(&out);
        eprintln!("{bytes} bytes, {video:?}");
        assert!(bytes > 2_000, "only {bytes} bytes");
        assert!((video.seconds - 2.5).abs() < 0.05, "{video:?}");
        assert_eq!(video.video_tracks, 1);
        assert_eq!(video.size, (640.0, 360.0));
    }

    #[test]
    fn an_abandoned_video_is_removed() {
        let scratch = Scratch::new("abandoned");
        let out = scratch.0.join("page.mp4");
        let mut video = Writer::create(&out, 64, 64).expect("a writer");
        let added = video
            .append(Duration::ZERO, |pixels, _| pixels.fill(90))
            .expect("a frame");
        assert_eq!(added, Some(Duration::ZERO));
        assert!(out.exists());
        drop(video);
        assert!(!out.exists());
        // And a file already there is never written over by the writer itself.
        std::fs::write(&out, b"mine").expect("a file");
        assert!(Writer::create(&out, 64, 64).is_err());
        assert_eq!(std::fs::read(&out).expect("still there"), b"mine");
    }
}
