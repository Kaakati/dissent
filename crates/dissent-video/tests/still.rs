//! The first frame of real video files, for thumbnails. The fixtures are one second of 160x90 at 10 fps,
//! blue over red, made with ffmpeg (libx264, libaom-av1, libvpx-vp9). `rotated.mp4` is `h264.mp4` with a
//! display rotation of 90 degrees counter-clockwise, which ffmpeg shows as 90x160, blue left of red.
//! `av1-10bit.mp4` is plain red at 10 bits, which the 8-bit rav1d build cannot decode.

use std::path::PathBuf;

use dissent_video::still::{first_frame, Still};
use dissent_video::VideoError;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn pixel(s: &Still, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * s.width + x) * 3) as usize;
    [s.rgb[i], s.rgb[i + 1], s.rgb[i + 2]]
}

fn is_blue(p: [u8; 3]) -> bool {
    p[2] > 200 && p[0] < 60 && p[1] < 60
}

fn is_red(p: [u8; 3]) -> bool {
    p[0] > 200 && p[1] < 60 && p[2] < 60
}

#[test]
fn h264_and_av1_in_mp4_mov_webm_and_mkv_give_their_first_picture() {
    for name in ["h264.mp4", "h264.mov", "h264.mkv", "fragmented.mp4", "av1.mp4", "av1.webm"] {
        let still = first_frame(&fixture(name), 320).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((still.width, still.height), (160, 90), "{name}");
        assert_eq!(still.rgb.len(), 160 * 90 * 3, "{name}");
        assert!(is_blue(pixel(&still, 80, 20)), "{name}: top is blue, got {:?}", pixel(&still, 80, 20));
        assert!(is_red(pixel(&still, 80, 70)), "{name}: bottom is red, got {:?}", pixel(&still, 80, 70));
    }
}

#[test]
fn the_picture_shrinks_to_the_long_side() {
    let still = first_frame(&fixture("h264.mp4"), 64).unwrap();
    assert_eq!((still.width, still.height), (64, 36));
}

#[test]
fn a_rotated_track_is_turned_as_players_show_it() {
    let still = first_frame(&fixture("rotated.mp4"), 320).unwrap();
    assert_eq!((still.width, still.height), (90, 160));
    assert!(is_blue(pixel(&still, 20, 80)), "left is blue, got {:?}", pixel(&still, 20, 80));
    assert!(is_red(pixel(&still, 70, 80)), "right is red, got {:?}", pixel(&still, 70, 80));
}

#[test]
fn vp9_has_no_decoder_and_says_so() {
    assert!(matches!(first_frame(&fixture("vp9.webm"), 320), Err(VideoError::Unsupported(_))));
}

#[test]
fn ten_bit_av1_is_refused_rather_than_shown_wrong() {
    assert!(first_frame(&fixture("av1-10bit.mp4"), 320).is_err());
}

#[test]
fn files_that_are_not_video_or_are_cut_short_fail_without_panicking() {
    let dir = std::env::temp_dir().join(format!("dissent-still-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("notes.mp4");
    std::fs::write(&text, b"this is not a video at all").unwrap();
    assert!(first_frame(&text, 320).is_err());
    for name in ["h264.mp4", "av1.webm", "h264.mkv"] {
        let bytes = std::fs::read(fixture(name)).unwrap();
        for cut in [16, bytes.len() / 3, bytes.len() * 2 / 3, bytes.len() - 1] {
            let path = dir.join(format!("cut-{cut}-{name}"));
            std::fs::write(&path, &bytes[..cut]).unwrap();
            // Either a picture or an error: never a panic.
            let _ = first_frame(&path, 320);
        }
    }
    let empty = dir.join("empty.webm");
    std::fs::write(&empty, b"").unwrap();
    assert!(first_frame(&empty, 320).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
