use super::*;

use image::RgbImage;
use objc2_foundation::NSDataBase64EncodingOptions;

use writer::Scratch;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn a_full_queue_drops_its_oldest_frame() {
    let queue = Queue::new(2);
    for n in 0..4 {
        queue.push(Frame {
            at: ms(n),
            jpeg: n.to_string(),
        });
    }
    queue.end(End::Finish(ms(9)));
    queue.end(End::Abandon);
    let mut left = Vec::new();
    while let Next::Frame(frame) = queue.next() {
        left.push(frame.jpeg);
    }
    assert_eq!(left, ["2", "3"]);
    assert_eq!(queue.dropped(), 2);
    // The first word on how it ended stands.
    assert!(matches!(queue.next(), Next::End(End::Finish(end)) if end == ms(9)));

    let abandoned = Queue::new(2);
    abandoned.push(Frame {
        at: ms(0),
        jpeg: String::new(),
    });
    abandoned.end(End::Abandon);
    assert!(matches!(abandoned.next(), Next::End(End::Abandon)));
}

#[test]
fn a_recorder_can_move_between_threads() {
    fn send<T: Send>() {}
    send::<Recorder>();
}

/// A screencast frame as Chromium sends one: a base64 JPEG, here a coloured
/// field with a bar that moves frame to frame.
fn screencast_frame(width: u32, height: u32, n: u32) -> String {
    let (w, h) = (width as usize, height as usize);
    let shade = (n * 3 % 256) as u8;
    let bar = (n as usize * 8) % w;
    let mut rgb = vec![0u8; w * h * 3];
    let mut i = 0;
    while i < rgb.len() {
        let x = i / 3 % w;
        let on_bar = x >= bar && x < bar + 24;
        rgb[i] = if on_bar { 255 } else { shade };
        rgb[i + 1] = (x % 256) as u8;
        rgb[i + 2] = if on_bar { 255 } else { 255 - shade };
        i += 3;
    }
    let image = RgbImage::from_raw(width, height, rgb).expect("an image");
    let mut jpeg = std::io::Cursor::new(Vec::new());
    DynamicImage::from(image)
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .expect("a JPEG");
    NSData::with_bytes(jpeg.get_ref())
        .base64EncodedStringWithOptions(NSDataBase64EncodingOptions::empty())
        .to_string()
}

#[test]
fn frames_become_an_mp4_a_player_reads_back_with_its_key_frames() {
    let scratch = Scratch::new("video");
    let out = scratch.0.join("page.mp4");
    let queue = Queue::new(128);
    // Two seconds at about 30 frames a second, unevenly, then the pane is
    // resized to a different shape for one more.
    for n in 0..90u32 {
        let (width, height) = if n < 60 { (640, 360) } else { (800, 400) };
        queue.push(Frame {
            at: ms(u64::from(n) * 33 + u64::from(n % 3)),
            jpeg: screencast_frame(width, height, n),
        });
    }
    queue.end(End::Finish(ms(3500)));

    let recording = encode(&queue, &out).expect("a recording");
    assert_eq!(recording.video, out);
    assert_eq!(recording.seconds, 3.5);
    let bytes = std::fs::metadata(&out).expect("the video").len();
    assert!(bytes > 20_000, "only {bytes} bytes");

    let video = writer::probe(&out);
    eprintln!("{bytes} bytes, {video:?}");
    assert!((video.seconds - 3.5).abs() < 0.05, "{video:?}");
    assert_eq!(video.video_tracks, 1);
    assert_eq!(video.size, (640.0, 360.0));

    assert!(
        (2..=6).contains(&recording.frames.len()),
        "{:?}",
        recording.frames
    );
    for (n, frame) in recording.frames.iter().enumerate() {
        assert_eq!(*frame, scratch.0.join(format!("page-{}.png", n + 1)));
    }
    let first = image::open(&recording.frames[0]).expect("a PNG");
    assert_eq!((first.width(), first.height()), (640, 360));
    // The last key frame is the last frame, as the resized pane drew it.
    let last = image::open(recording.frames.last().expect("frames")).expect("a PNG");
    assert_eq!((last.width(), last.height()), (800, 400));
}

#[test]
fn a_recording_with_no_frames_fails_and_leaves_nothing() {
    let scratch = Scratch::new("empty");
    let out = scratch.0.join("page.mp4");
    let queue = Queue::new(4);
    queue.push(Frame {
        at: Duration::ZERO,
        jpeg: "not a frame".into(),
    });
    queue.end(End::Finish(Duration::from_secs(1)));
    let error = encode(&queue, &out).expect_err("nothing to record");
    assert_eq!(
        error.to_string(),
        "the page didn't paint while it was being recorded"
    );
    assert!(!out.exists());
}
