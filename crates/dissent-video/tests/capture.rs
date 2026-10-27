//! Capture sources on every platform. The tests that open a camera or a display are `#[ignore]`d:
//! they need the device, and they run by hand on a machine that has one.
//!
//! The rest need no device. A missing camera, display or window is an error on every platform, and
//! the Linux CI image, which has no camera and no X server, runs them too.

use dissent_video::camera::CameraSource;
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
