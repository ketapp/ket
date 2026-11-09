//! Where each frame of a recording goes — in time, in the picture, and among
//! the key frames — worked out apart from Chromium and AVFoundation, so it
//! can be tested on its own.

use std::time::Duration;

use image::RgbImage;

/// The least time between two frames: the writer's tick.
const TICK: Duration = Duration::from_millis(1);

/// The most frames [`KeyFrames`] holds on to while a recording runs.
const CANDIDATES: usize = 48;

/// How far apart [`KeyFrames`] starts sampling; it doubles as recording goes.
const FIRST_SPACING: Duration = Duration::from_millis(100);

/// Where frames go in the video. The first opens it; each after goes where
/// the page's own timestamps put it relative to the first — never later than
/// it arrived, never at or before the frame before — so the video plays at
/// the speed the page changed, however unevenly it repainted.
#[derive(Debug)]
pub(super) struct Timeline {
    limit: Duration,
    /// When the first frame arrived, and the page's stamp on it in seconds.
    first: Option<(Duration, Option<f64>)>,
    /// Where the latest frame went.
    last: Option<Duration>,
}

impl Timeline {
    pub(super) fn new(limit: Duration) -> Self {
        Self {
            limit,
            first: None,
            last: None,
        }
    }

    /// Where a frame that arrived `arrived` into the recording, stamped
    /// `stamp` by the page, goes: `None` once the video is `limit` long.
    pub(super) fn place(&mut self, arrived: Duration, stamp: Option<f64>) -> Option<Duration> {
        let at = match (self.first, self.last) {
            (Some((first, first_stamp)), Some(last)) => {
                let stamped = first_stamp
                    .zip(stamp)
                    .and_then(|(first_stamp, stamp)| {
                        Duration::try_from_secs_f64(stamp - first_stamp).ok()
                    })
                    .map(|since| first + since);
                stamped
                    .map_or(arrived, |at| at.min(arrived))
                    .max(last + TICK)
            }
            _ => {
                self.first = Some((arrived, stamp));
                Duration::ZERO
            }
        };
        if at >= self.limit {
            return None;
        }
        self.last = Some(at);
        Some(at)
    }

    /// How long the video runs, recording having stopped `stopped` in: until
    /// then, but no longer than `limit`, and past its last frame.
    pub(super) fn end(&self, stopped: Duration) -> Duration {
        let end = stopped.min(self.limit);
        self.last.map_or(end, |last| end.max(last + TICK))
    }
}

/// A frame size H.264 takes: even, and at least two.
pub(super) fn even((width, height): (u32, u32)) -> (u32, u32) {
    ((width & !1).max(2), (height & !1).max(2))
}

/// Where a frame goes in the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placement {
    /// As it is, from the top left: it is the video's size, give or take the
    /// pixel an odd size loses to rounding.
    Copy,
    /// Scaled to `width` by `height` and centred at `x`, `y`, black around.
    Fit {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

/// Where a frame `from` big goes in a video `to` big: copied when it is the
/// video's size, otherwise scaled to fit and letterboxed — the pane was
/// resized, and the video keeps its first frame's size.
pub(super) fn place(from: (u32, u32), to: (u32, u32)) -> Placement {
    if from.0.abs_diff(to.0) <= 1 && from.1.abs_diff(to.1) <= 1 {
        return Placement::Copy;
    }
    let (from_width, from_height) = (u64::from(from.0.max(1)), u64::from(from.1.max(1)));
    let (to_width, to_height) = (u64::from(to.0), u64::from(to.1));
    let (width, height) = if from_width * to_height >= from_height * to_width {
        let height = (from_height * to_width + from_width / 2) / from_width;
        (to_width, height.clamp(1, to_height))
    } else {
        let width = (from_width * to_height + from_height / 2) / from_height;
        (width.clamp(1, to_width), to_height)
    };
    let width = u32::try_from(width).unwrap_or(to.0);
    let height = u32::try_from(height).unwrap_or(to.1);
    Placement::Fit {
        x: (to.0 - width) / 2,
        y: (to.1 - height) / 2,
        width,
        height,
    }
}

/// Draws `image`, already scaled for `placement`, into a BGRA frame `size`
/// big whose rows are `stride` bytes apart.
pub(super) fn compose(
    image: &RgbImage,
    placement: Placement,
    size: (u32, u32),
    pixels: &mut [u8],
    stride: usize,
) {
    let (x, y, width, height) = match placement {
        Placement::Copy => (0, 0, image.width().min(size.0), image.height().min(size.1)),
        Placement::Fit {
            x,
            y,
            width,
            height,
        } => (x, y, width, height),
    };
    if (x, y, width, height) != (0, 0, size.0, size.1) {
        // Black around it. The encoder takes no alpha, so none is set.
        pixels.fill(0);
    }
    let (x, y, width) = (x as usize, y as usize, width as usize);
    let source = image.as_raw();
    let source_stride = image.width() as usize * 3;
    for row in 0..height as usize {
        let from = source
            .get(row * source_stride..)
            .and_then(|r| r.get(..width * 3));
        let start = (y + row) * stride + x * 4;
        let to = pixels.get_mut(start..).and_then(|r| r.get_mut(..width * 4));
        let (Some(from), Some(to)) = (from, to) else {
            break;
        };
        rgb_to_bgra(from, to);
    }
}

/// One row of RGB pixels as BGRA. Indexes rather than iterator adaptors:
/// ket's own crates build unoptimised in debug, where this is three times as
/// fast — about 10 ms a 1080p frame.
fn rgb_to_bgra(from: &[u8], to: &mut [u8]) {
    let (mut i, mut j) = (0, 0);
    while i + 3 <= from.len() && j + 4 <= to.len() {
        to[j] = from[i + 2];
        to[j + 1] = from[i + 1];
        to[j + 2] = from[i];
        to[j + 3] = 255;
        i += 3;
        j += 4;
    }
}

/// The frames a recording's key frames will come from, kept without keeping
/// every frame: the one on screen at each multiple of `spacing`, the spacing
/// doubling whenever more than [`CANDIDATES`] would be kept.
///
/// A frame is kept at the first multiple it was on screen for, and stands
/// for every multiple after it up to the next frame kept: frames in between
/// were on screen for no multiple at all. Doubling the spacing keeps each
/// frame still on screen at a multiple of the new one.
pub(super) struct KeyFrames<T> {
    spacing: Duration,
    /// (multiple, frame number, frame), in order.
    kept: Vec<(Duration, u64, T)>,
    /// The latest frame: from when, its number, and it.
    latest: Option<(Duration, u64, T)>,
    offered: u64,
}

impl<T> KeyFrames<T> {
    pub(super) fn new() -> Self {
        Self {
            spacing: FIRST_SPACING,
            kept: Vec::new(),
            latest: None,
            offered: 0,
        }
    }

    /// `frame` went on screen at `at`, after every frame offered before.
    pub(super) fn offer(&mut self, at: Duration, frame: T) {
        if let Some((from, number, previous)) = self.latest.take() {
            let multiple = at_or_after(from, self.spacing);
            if multiple < at {
                self.kept.push((multiple, number, previous));
            }
        }
        self.latest = Some((at, self.offered, frame));
        self.offered += 1;
        while self.kept.len() > CANDIDATES {
            self.coarsen();
        }
    }

    /// Doubles the spacing, keeping each frame on screen at a multiple of
    /// the new one.
    fn coarsen(&mut self) {
        self.spacing *= 2;
        let latest = self
            .latest
            .as_ref()
            .map_or(Duration::MAX, |latest| latest.0);
        let ends: Vec<Duration> = self
            .kept
            .iter()
            .skip(1)
            .map(|kept| kept.0)
            .chain([latest])
            .collect();
        let kept = std::mem::take(&mut self.kept);
        for ((multiple, number, frame), until) in kept.into_iter().zip(ends) {
            let multiple = at_or_after(multiple, self.spacing);
            if multiple < until {
                self.kept.push((multiple, number, frame));
            }
        }
    }

    /// Up to `count` frames evenly spaced through a video `end` long, the
    /// last its final frame, each as near its moment as the spacing allows.
    /// A frame on screen at several of those moments is given once.
    pub(super) fn pick(self, end: Duration, count: u32) -> Vec<T> {
        let Self {
            spacing,
            kept,
            latest,
            ..
        } = self;
        let Some(latest) = latest else {
            return Vec::new();
        };
        let mut wanted = Vec::new();
        for k in 1..count {
            let multiple = at_or_before(end * k / count, spacing);
            let number = if latest.0 <= multiple {
                Some(latest.1)
            } else {
                kept.iter()
                    .rev()
                    .find(|kept| kept.0 <= multiple)
                    .map(|kept| kept.1)
            };
            if let Some(number) = number
                && wanted.last() != Some(&number)
            {
                wanted.push(number);
            }
        }
        let mut picked: Vec<T> = kept
            .into_iter()
            .filter(|kept| wanted.contains(&kept.1))
            .map(|kept| kept.2)
            .collect();
        picked.push(latest.2);
        picked
    }
}

/// The first multiple of `spacing` at or after `at`.
fn at_or_after(at: Duration, spacing: Duration) -> Duration {
    let step = spacing.as_nanos().max(1);
    nanos(at.as_nanos().div_ceil(step) * step)
}

/// The last multiple of `spacing` at or before `at`.
fn at_or_before(at: Duration, spacing: Duration) -> Duration {
    let step = spacing.as_nanos().max(1);
    nanos(at.as_nanos() / step * step)
}

fn nanos(nanos: u128) -> Duration {
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many key frames a recording keeps, as `record` asks.
    const KEY_FRAMES: u32 = 6;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn the_timeline_follows_the_pages_stamps_but_never_runs_ahead_of_arrival() {
        let mut timeline = Timeline::new(Duration::from_secs(10));
        // The first frame opens the video, whenever it arrived.
        assert_eq!(timeline.place(ms(50), Some(1000.0)), Some(Duration::ZERO));
        // Drawn 250 ms after the first, arrived later: placed by its stamp.
        assert_eq!(timeline.place(ms(400), Some(1000.25)), Some(ms(300)));
        // Stamped later than it arrived: placed at its arrival.
        assert_eq!(timeline.place(ms(500), Some(1001.0)), Some(ms(500)));
        // Stamped no later than the frame before: just after it.
        assert_eq!(timeline.place(ms(520), Some(1000.0)), Some(ms(501)));
        // No stamp: its arrival.
        assert_eq!(timeline.place(ms(700), None), Some(ms(700)));
    }

    #[test]
    fn without_stamps_the_timeline_is_arrival() {
        let mut timeline = Timeline::new(Duration::from_secs(10));
        assert_eq!(timeline.place(ms(80), None), Some(Duration::ZERO));
        assert_eq!(timeline.place(ms(100), Some(5.0)), Some(ms(100)));
        assert_eq!(timeline.place(ms(100), None), Some(ms(101)));
    }

    #[test]
    fn a_full_timeline_takes_no_more_and_ends_at_its_limit() {
        let mut timeline = Timeline::new(Duration::from_secs(1));
        assert_eq!(timeline.place(ms(0), None), Some(Duration::ZERO));
        assert_eq!(timeline.place(ms(999), None), Some(ms(999)));
        assert_eq!(timeline.place(ms(1000), None), None);
        // Stopped long after: the video still runs only to its limit.
        assert_eq!(
            timeline.end(Duration::from_secs(30)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn the_video_runs_until_recording_stopped_and_past_its_last_frame() {
        let mut timeline = Timeline::new(Duration::from_secs(10));
        assert_eq!(timeline.end(ms(400)), ms(400));
        timeline.place(ms(0), None);
        timeline.place(ms(300), None);
        assert_eq!(timeline.end(ms(2500)), ms(2500));
        assert_eq!(timeline.end(ms(100)), ms(301));
    }

    #[test]
    fn key_frames_are_evenly_spaced_and_end_on_the_last_frame() {
        let mut keys = KeyFrames::new();
        // Twelve seconds of a page repainting at 30 frames a second.
        for n in 0..360u64 {
            keys.offer(ms(n * 1000 / 30), n);
            assert!(keys.kept.len() <= CANDIDATES);
        }
        let spacing = keys.spacing;
        let picked = keys.pick(Duration::from_secs(12), KEY_FRAMES);
        assert_eq!(picked.len(), 6);
        assert_eq!(picked.last(), Some(&359));
        // The others are the frames on screen near 2, 4, 6, 8 and 10 seconds,
        // never after and never more than one spacing before.
        let slack = (spacing.as_millis() as u64 * 30).div_ceil(1000) + 1;
        for (k, &frame) in picked.iter().take(5).enumerate() {
            let due = (k as u64 + 1) * 60;
            assert!(
                frame <= due && due - frame <= slack,
                "frame {frame} for {due}"
            );
        }
    }

    #[test]
    fn a_page_that_did_not_change_gives_one_key_frame() {
        let mut keys = KeyFrames::new();
        keys.offer(Duration::ZERO, "still");
        assert_eq!(keys.pick(Duration::from_secs(20), KEY_FRAMES), ["still"]);

        let mut keys = KeyFrames::new();
        keys.offer(Duration::ZERO, "before");
        keys.offer(ms(9900), "after");
        assert_eq!(
            keys.pick(Duration::from_secs(10), KEY_FRAMES),
            ["before", "after"]
        );
    }

    #[test]
    fn key_frames_stay_bounded_through_a_long_busy_recording() {
        let mut keys = KeyFrames::new();
        // Two minutes at 60 frames a second, with a quiet spell in the middle.
        for n in 0..7200u64 {
            let at = if n < 3600 {
                n * 1000 / 60
            } else {
                70_000 + (n - 3600) * 1000 / 60
            };
            keys.offer(ms(at), n);
            assert!(keys.kept.len() <= CANDIDATES);
        }
        let picked = keys.pick(Duration::from_secs(130), KEY_FRAMES);
        assert_eq!(picked.len(), 6);
        assert!(picked.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(picked.last(), Some(&7199));
        // At 65 s the page was quiet: the frame on screen was the last before it.
        assert_eq!(picked[2], 3599);
    }

    #[test]
    fn frames_the_videos_size_are_copied_and_others_letterboxed() {
        assert_eq!(place((640, 360), (640, 360)), Placement::Copy);
        // An odd size, rounded down to the video's even one.
        assert_eq!(place((641, 361), (640, 360)), Placement::Copy);
        // Wider: full width, bars above and below.
        assert_eq!(
            place((800, 400), (640, 360)),
            Placement::Fit {
                x: 0,
                y: 20,
                width: 640,
                height: 320
            }
        );
        // Taller: full height, bars at the sides.
        assert_eq!(
            place((400, 400), (640, 360)),
            Placement::Fit {
                x: 140,
                y: 0,
                width: 360,
                height: 360
            }
        );
        // Same shape, bigger: scaled to fill.
        assert_eq!(
            place((1280, 720), (640, 360)),
            Placement::Fit {
                x: 0,
                y: 0,
                width: 640,
                height: 360
            }
        );
        assert_eq!(even((1919, 1079)), (1918, 1078));
        assert_eq!(even((1, 1)), (2, 2));
    }

    #[test]
    fn a_letterboxed_frame_is_drawn_as_bgra_with_black_around_it() {
        let red = RgbImage::from_pixel(2, 1, image::Rgb([200, 10, 20]));
        let placement = Placement::Fit {
            x: 1,
            y: 1,
            width: 2,
            height: 1,
        };
        // A 4×3 frame with rows padded to 20 bytes, as CoreVideo pads them.
        let mut pixels = vec![7u8; 20 * 3];
        compose(&red, placement, (4, 3), &mut pixels, 20);
        let pixel = |x: usize, y: usize| &pixels[y * 20 + x * 4..][..4];
        assert_eq!(pixel(1, 1), [20, 10, 200, 255]);
        assert_eq!(pixel(2, 1), [20, 10, 200, 255]);
        for (x, y) in [(0, 0), (3, 0), (0, 1), (3, 1), (1, 2)] {
            assert_eq!(pixel(x, y), [0, 0, 0, 0], "({x}, {y})");
        }
    }
}
