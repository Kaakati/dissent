//! Capture sources on every platform. The tests that open a camera or a display are `#[ignore]`d:
//! they need the device, and they run by hand on a machine that has one.
//!
//! The rest need no device. A missing camera, display or window is an error on every platform, and
//! the Linux CI image, which has no camera and no X server, runs them too.

use std::time::Duration;

use dissent_video::camera::CameraSource;
use dissent_video::encode::{self, EncoderConfig};
use dissent_video::Codec;
use dissent_video::screen::{self, Cursor, ScreenSource, ScreenTarget};
use dissent_video::source::{CaptureFormat, VideoSource};

const FORMAT: CaptureFormat = CaptureFormat {
    width: 320,
    height: 240,
    fps: 15,
};

#[test]
fn a_missing_camera_is_an_error() {
    assert!(CameraSource::open(u32::MAX, FORMAT).is_err());
}

#[test]
fn a_missing_display_is_an_error() {
    assert!(ScreenSource::open(ScreenTarget::Display(u32::MAX), FORMAT, Cursor::Excluded).is_err());
}

#[test]
fn a_missing_window_is_an_error() {
    assert!(ScreenSource::open(ScreenTarget::Window(u32::MAX), FORMAT, Cursor::Excluded).is_err());
}

#[test]
fn the_cursor_is_refused_until_a_backend_draws_it() {
    assert!(ScreenSource::open(ScreenTarget::Display(0), FORMAT, Cursor::Included).is_err());
}

#[test]
#[ignore = "needs a webcam: run by hand on a machine that has one"]
fn a_camera_delivers_frames_at_the_requested_size() {
    let mut camera = CameraSource::open(0, FORMAT).expect("camera 0 opens");
    for _ in 0..3 {
        let frame = camera.next_frame().expect("a frame");
        assert_eq!((frame.width(), frame.height()), (FORMAT.width, FORMAT.height));
    }
}

#[test]
#[ignore = "needs a display: run by hand on a machine with a screen"]
fn a_display_delivers_frames_at_the_requested_size() {
    let display = screen::list_displays()
        .expect("displays list")
        .into_iter()
        .find(|d| d.primary)
        .expect("a primary display");
    let mut source =
        ScreenSource::open(ScreenTarget::Display(display.id), FORMAT, Cursor::Excluded)
            .expect("the display opens");
    for _ in 0..3 {
        let frame = source.next_frame().expect("a frame");
        assert_eq!((frame.width(), frame.height()), (FORMAT.width, FORMAT.height));
    }
}

#[test]
#[ignore = "measures a real webcam: run in release with --ignored --nocapture"]
fn a_webcam_opens_and_streams_at_the_camera_sizes() {
    for (width, height, fps) in [(1280, 720, 30), (1920, 1080, 30)] {
        let format = CaptureFormat { width, height, fps };
        let start = std::time::Instant::now();
        let mut camera = CameraSource::open(0, format).expect("camera 0 opens");
        let open_ms = start.elapsed().as_millis();
        let mut frames = 0u32;
        let streamed = std::time::Instant::now();
        while streamed.elapsed() < Duration::from_secs(3) {
            camera.next_frame().expect("a frame");
            frames += 1;
        }
        println!(
            "webcam {width}x{height} @{fps}: open {open_ms} ms, {:.1} frames/s over 3 s, device {:?}",
            frames as f64 / 3.0,
            camera.device_format()
        );
    }
}

/// Encodes live webcam frames and reports the encode time per frame, on real content rather than
/// the test picture. The frame interval is the budget each frame must fit in.
fn webcam_encode_ms(codec: Codec, width: u32, height: u32, fps: u32, kbps: u32) {
    let format = CaptureFormat { width, height, fps };
    let mut camera = CameraSource::open(0, format).expect("camera 0 opens");
    let mut encoder = encode::open(
        codec,
        EncoderConfig {
            width,
            height,
            fps,
            bitrate_kbps: kbps,
            keyframe_interval: fps * 2,
        },
    )
    .expect("encoder opens");
    let frames: Vec<_> = (0..60).map(|_| camera.next_frame().expect("a frame")).collect();
    let mut total = std::time::Duration::ZERO;
    let mut worst = std::time::Duration::ZERO;
    let mut bytes = 0usize;
    for frame in &frames {
        let started = std::time::Instant::now();
        let chunks = encoder.encode(frame).expect("encodes");
        let took = started.elapsed();
        total += took;
        worst = worst.max(took);
        bytes += chunks.iter().map(|c| c.data.len()).sum::<usize>();
    }
    let mean = total.as_secs_f64() * 1_000.0 / frames.len() as f64;
    println!(
        "webcam {codec:?} {width}x{height} @{fps} fps, {kbps} kbps: {mean:.1} ms/frame mean, {:.1} ms worst, budget {:.1} ms, {bytes} bytes over {} frames",
        worst.as_secs_f64() * 1_000.0,
        1_000.0 / fps as f64,
        frames.len()
    );
}

#[test]
#[ignore = "encodes a real webcam: run in release with --ignored --nocapture"]
fn a_webcam_h264_720p30_encode_time() {
    webcam_encode_ms(Codec::H264, 1280, 720, 30, 2_000);
}

#[cfg(feature = "svt-av1")]
#[test]
#[ignore = "encodes a real webcam: run in release with --features svt-av1 --ignored --nocapture"]
fn a_webcam_av1_1080p30_encode_time() {
    webcam_encode_ms(Codec::Av1, 1920, 1080, 30, 3_000);
}

#[cfg(feature = "svt-av1")]
#[test]
#[ignore = "encodes a real webcam: run in release with --features svt-av1 --ignored --nocapture"]
fn a_webcam_av1_720p30_encode_time() {
    webcam_encode_ms(Codec::Av1, 1280, 720, 30, 2_000);
}
