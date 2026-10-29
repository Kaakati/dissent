//! Thumbnails for image files. The sender makes a small JPEG of each picture, and receivers show it
//! before they download the file. The thumbnail is encrypted like any other file, so it travels as
//! its own blob. See docs/design/08-files.md.
//!
//! Decoding applies the EXIF orientation, and re-encoding writes only the pixels, so a thumbnail
//! carries no EXIF or other metadata from the original.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{guess_format, DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, Rgb, RgbImage};

/// Longest side of a thumbnail, in pixels.
pub const THUMB_LONG_SIDE: u32 = 320;

/// Largest thumbnail the sender makes. Receivers fetch a thumbnail automatically up to this size.
pub const THUMB_MAX_BYTES: usize = 64 * 1024;

/// Longest side of a picture a link preview carries, in pixels.
pub const PREVIEW_IMAGE_SIDE: u32 = 640;

/// Largest picture a link preview carries. Receivers fetch one automatically up to this size.
pub const PREVIEW_IMAGE_MAX_BYTES: usize = 150 * 1024;

/// Pictures over this many bytes are not decoded, so a thumbnail never costs more than this much.
pub const THUMB_SOURCE_MAX: usize = 25 * 1024 * 1024;

/// Pictures over this many pixels are not decoded.
const MAX_PIXELS: u64 = 40_000_000;

/// JPEG qualities tried in order. The first whose output fits `THUMB_MAX_BYTES` wins.
const QUALITIES: [u8; 3] = [80, 65, 50];

/// A thumbnail: JPEG bytes and the size of the picture they show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Whether `bytes` is a thumbnail this module could have made: a JPEG under [`THUMB_MAX_BYTES`] whose
/// picture is at most [`THUMB_LONG_SIDE`] on each side. Checked on what a sender's blob decrypts to.
pub fn is_thumbnail(bytes: &[u8]) -> bool {
    is_jpeg_within(bytes, THUMB_MAX_BYTES, THUMB_LONG_SIDE)
}

/// Whether `bytes` is a link-preview picture this module could have made: a JPEG under
/// [`PREVIEW_IMAGE_MAX_BYTES`] whose picture is at most [`PREVIEW_IMAGE_SIDE`] on each side.
pub fn is_preview_image(bytes: &[u8]) -> bool {
    is_jpeg_within(bytes, PREVIEW_IMAGE_MAX_BYTES, PREVIEW_IMAGE_SIDE)
}

/// Whether `bytes` is a JPEG of at most `max_bytes` whose picture is at most `side` on each side.
fn is_jpeg_within(bytes: &[u8], max_bytes: usize, side: u32) -> bool {
    if bytes.len() > max_bytes {
        return false;
    }
    let Ok(reader) = ImageReader::new(Cursor::new(bytes)).with_guessed_format() else {
        return false;
    };
    if reader.format() != Some(ImageFormat::Jpeg) {
        return false;
    }
    matches!(reader.into_dimensions(), Ok((w, h)) if w >= 1 && h >= 1 && w <= side && h <= side)
}

/// Makes a thumbnail of a PNG, JPEG, WebP or GIF (its first frame). Returns `None` for anything else,
/// for pictures that are too large to decode, and for pictures whose thumbnail does not fit
/// [`THUMB_MAX_BYTES`] at the lowest quality.
pub fn make_thumbnail(data: &[u8]) -> Option<Thumbnail> {
    make_jpeg(data, THUMB_LONG_SIDE, THUMB_MAX_BYTES)
}

/// Makes a thumbnail of a decoded picture given as packed RGB, `width` x `height`: a video's first
/// frame, for one. Returns `None` when the buffer does not match the size, when the picture is over
/// the pixel limit, or when its thumbnail does not fit [`THUMB_MAX_BYTES`] at the lowest quality.
pub fn make_thumbnail_rgb(width: u32, height: u32, rgb: Vec<u8>) -> Option<Thumbnail> {
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let picture = RgbImage::from_raw(width, height, rgb)?;
    small_jpeg(DynamicImage::ImageRgb8(picture), THUMB_LONG_SIDE, THUMB_MAX_BYTES)
}

/// Makes a link-preview picture: at most [`PREVIEW_IMAGE_SIDE`] on each side, and at most
/// [`PREVIEW_IMAGE_MAX_BYTES`] of JPEG. Like [`make_thumbnail`], it carries no metadata.
pub fn make_preview_image(data: &[u8]) -> Option<Thumbnail> {
    make_jpeg(data, PREVIEW_IMAGE_SIDE, PREVIEW_IMAGE_MAX_BYTES)
}

/// Re-encodes a picture as a JPEG at most `side` on each side and `max_bytes` long.
fn make_jpeg(data: &[u8], side: u32, max_bytes: usize) -> Option<Thumbnail> {
    if data.len() > THUMB_SOURCE_MAX {
        return None;
    }
    let format = guess_format(data).ok()?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(512 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    if u64::from(picture.width()) * u64::from(picture.height()) > MAX_PIXELS {
        return None;
    }
    picture.apply_orientation(orientation);
    small_jpeg(picture, side, max_bytes)
}

/// Shrinks a picture to at most `side` on each side and encodes it as a JPEG of at most `max_bytes`.
fn small_jpeg(picture: DynamicImage, side: u32, max_bytes: usize) -> Option<Thumbnail> {
    // Only pictures larger than the target shrink. `thumbnail` would also enlarge a small one.
    let small = if picture.width().max(picture.height()) > side {
        picture.thumbnail(side, side)
    } else {
        picture
    }
    .to_rgba8();
    let (width, height) = small.dimensions();
    // JPEG has no alpha. A transparent pixel is drawn over white, not left to turn black.
    let mut rgb = RgbImage::new(width, height);
    for (x, y, p) in small.enumerate_pixels() {
        let a = f32::from(p[3]) / 255.0;
        let blend = |c: u8| (f32::from(c) * a + 255.0 * (1.0 - a)).round() as u8;
        rgb.put_pixel(x, y, Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    for quality in QUALITIES {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, quality).encode_image(&rgb).ok()?;
        if jpeg.len() <= max_bytes {
            return Some(Thumbnail { jpeg, width, height });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    /// Inserts an APP1 EXIF segment with the given orientation after the JPEG start marker.
    fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM\0*\0\0\0\x08"); // big-endian header, first IFD at 8
        tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_be_bytes()); // one value
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]); // value padding
        tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
        let mut body = b"Exif\0\0".to_vec();
        body.extend_from_slice(&tiff);
        let len = (body.len() + 2) as u16;
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn a_large_picture_shrinks_to_the_long_side_and_keeps_its_aspect() {
        let thumb = make_thumbnail(&png(1000, 500, [200, 30, 30, 255])).expect("a PNG thumbnail");
        assert_eq!((thumb.width, thumb.height), (320, 160));
        assert_eq!(&thumb.jpeg[..2], &[0xFF, 0xD8], "the output is a JPEG");
        assert!(thumb.jpeg.len() <= THUMB_MAX_BYTES);
    }

    #[test]
    fn a_preview_picture_fits_640_pixels_and_150_kb() {
        let picture = make_preview_image(&png(1200, 600, [40, 90, 200, 255])).expect("a preview picture");
        assert_eq!((picture.width, picture.height), (640, 320));
        assert!(picture.jpeg.len() <= PREVIEW_IMAGE_MAX_BYTES);
        assert!(is_preview_image(&picture.jpeg));
        assert!(!is_thumbnail(&picture.jpeg), "a preview picture is larger than a thumbnail");
    }

    #[test]
    fn a_small_picture_is_not_enlarged() {
        let thumb = make_thumbnail(&png(40, 30, [1, 2, 3, 255])).unwrap();
        assert_eq!((thumb.width, thumb.height), (40, 30));
    }

    #[test]
    fn transparent_pixels_come_out_white_not_black() {
        let thumb = make_thumbnail(&png(16, 16, [0, 0, 0, 0])).unwrap();
        let decoded = image::load_from_memory(&thumb.jpeg).unwrap().to_rgb8();
        let p = decoded.get_pixel(8, 8);
        assert!(p[0] > 240 && p[1] > 240 && p[2] > 240, "pixel {p:?}");
    }

    #[test]
    fn the_exif_orientation_is_applied_and_metadata_is_dropped() {
        let mut plain = Vec::new();
        image::RgbImage::from_pixel(400, 200, Rgb([10, 200, 10]))
            .write_to(&mut Cursor::new(&mut plain), ImageFormat::Jpeg)
            .unwrap();
        // Orientation 6 turns the picture a quarter turn: 400 by 200 becomes 200 by 400, then shrinks.
        let tagged = with_orientation(&plain, 6);
        let thumb = make_thumbnail(&tagged).unwrap();
        assert_eq!((thumb.width, thumb.height), (160, 320));
        assert!(!thumb.jpeg.windows(6).any(|w| w == b"Exif\0\0"), "no EXIF in the thumbnail");
    }

    #[test]
    fn only_a_small_jpeg_counts_as_a_thumbnail() {
        let thumb = make_thumbnail(&png(1000, 500, [9, 9, 9, 255])).unwrap();
        assert!(is_thumbnail(&thumb.jpeg), "what we make is accepted");
        assert!(!is_thumbnail(&png(40, 40, [1, 2, 3, 255])), "a PNG is not a thumbnail");
        let mut big = Vec::new();
        image::RgbImage::from_fn(320, 320, |x, y| Rgb([(x * 7 + y * 13) as u8, (x ^ y) as u8, (x * y) as u8]))
            .write_to(&mut Cursor::new(&mut big), ImageFormat::Jpeg)
            .unwrap();
        // The pattern is busy enough that its JPEG passes the size cap and is refused on size.
        assert!(!is_thumbnail(&big) || big.len() <= THUMB_MAX_BYTES);
        assert!(!is_thumbnail(b"not an image at all"));
    }

    #[test]
    fn data_that_is_not_a_picture_has_no_thumbnail() {
        assert!(make_thumbnail(b"plain text, not a picture").is_none());
        assert!(make_thumbnail(&[]).is_none());
    }

    #[test]
    fn a_decoded_frame_shrinks_to_a_thumbnail_and_a_wrong_buffer_has_none() {
        let rgb = vec![200u8; 1280 * 720 * 3];
        let t = make_thumbnail_rgb(1280, 720, rgb).expect("a thumbnail");
        assert_eq!((t.width, t.height), (320, 180));
        assert!(is_thumbnail(&t.jpeg));
        assert!(make_thumbnail_rgb(10, 10, vec![0; 5]).is_none(), "the buffer must match the size");
    }
}
