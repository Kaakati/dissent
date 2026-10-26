//! Animated GIF banners. The editor crops a GIF on its first frame but does not re-encode it, since a
//! canvas keeps one frame. It sends the original bytes and the crop, in source pixels after a
//! rotation. The engine decodes every frame with its delay, rotates, crops and scales each one to at
//! most 960 by 540, and writes a new GIF that loops forever with no metadata. A still JPEG of the
//! first frame goes beside it, for reduced motion. Over 2 MB or 200 frames, it first drops every
//! other frame, doubling the delays, then lowers the resolution in steps, and refuses past that.

use std::io::Cursor;

use dissent_events::{Banner, Picture, MAX_ANIMATED_BANNER_BYTES, MAX_BANNER_BYTES};
use dissent_identity::CommunityId;
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::{imageops, AnimationDecoder, Delay, DynamicImage, Frame, ImageDecoder, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::{CoreError, Result};

/// Most frames an animated banner keeps.
pub const MAX_FRAMES: usize = 200;
/// Most frames read from a source GIF before it is refused as too long.
const MAX_SOURCE_FRAMES: usize = 2_000;
/// The widths tried, largest first, when the GIF is over the byte cap. Heights keep 16:9.
const WIDTHS: [u32; 5] = [960, 720, 540, 400, 320];
const POSTER_QUALITY: u8 = 85;

/// The crop of a GIF, in source pixels of the frame after `rot` quarter turns clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Quarter turns clockwise, 0 to 3.
    pub rot: u8,
}

/// A re-encoded animated banner: the looping GIF and its still poster frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedBanner {
    pub gif: Vec<u8>,
    pub poster: Vec<u8>,
}

fn bad(why: impl std::fmt::Display) -> CoreError {
    CoreError::Invalid(format!("animated banner: {why}"))
}

/// Whether `bytes` is a GIF with more than one frame. The editor decides this itself; tests use it.
#[cfg(test)]
fn is_animated_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF8")
        && GifDecoder::new(Cursor::new(bytes)).is_ok_and(|d| d.into_frames().take(2).filter(|f| f.is_ok()).count() > 1)
}

/// Decodes, crops, scales and re-encodes an animated GIF as a looping banner. See the module doc.
pub fn encode_banner(bytes: &[u8], crop: CropRect) -> Result<EncodedBanner> {
    if !bytes.starts_with(b"GIF8") {
        return Err(bad("the file is not a GIF"));
    }
    if crop.w == 0 || crop.h == 0 || crop.rot > 3 {
        return Err(bad("the crop is empty"));
    }
    let decoder = GifDecoder::new(Cursor::new(bytes)).map_err(bad)?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames().take(MAX_SOURCE_FRAMES + 1) {
        let frame = frame.map_err(bad)?;
        let delay = frame.delay();
        let turned = rotate(frame.into_buffer(), crop.rot);
        let (fw, fh) = turned.dimensions();
        let x = crop.x.min(fw.saturating_sub(1));
        let y = crop.y.min(fh.saturating_sub(1));
        let w = crop.w.min(fw - x).max(1);
        let h = crop.h.min(fh - y).max(1);
        frames.push((imageops::crop_imm(&turned, x, y, w, h).to_image(), delay));
    }
    if frames.len() > MAX_SOURCE_FRAMES {
        return Err(bad(format!("the GIF has more than {MAX_SOURCE_FRAMES} frames")));
    }
    if frames.is_empty() {
        return Err(bad("the GIF has no frames"));
    }
    while frames.len() > MAX_FRAMES {
        frames = halve(frames);
    }
    let crop_w = frames[0].0.width();
    let widths: Vec<u32> = WIDTHS.iter().map(|w| (*w).min(crop_w).max(16)).collect();
    // All frames at full size first, then every other frame, then smaller sizes.
    let mut plan: Vec<(bool, u32)> = vec![(false, widths[0])];
    plan.extend(widths.iter().map(|w| (true, *w)));
    let halved = halve(frames.clone());
    for (drop, width) in plan {
        let source = if drop && frames.len() > 1 { &halved } else { &frames };
        let scaled = scale(source, width);
        let gif = encode_gif(&scaled)?;
        if gif.len() as u64 <= MAX_ANIMATED_BANNER_BYTES {
            let poster = encode_poster(&scaled[0].0)?;
            return Ok(EncodedBanner { gif, poster });
        }
    }
    Err(bad("it does not fit in 2 MB even with fewer frames at 320 px; try a shorter or simpler GIF"))
}

fn rotate(img: RgbaImage, rot: u8) -> RgbaImage {
    match rot % 4 {
        1 => imageops::rotate90(&img),
        2 => imageops::rotate180(&img),
        3 => imageops::rotate270(&img),
        _ => img,
    }
}

/// Every other frame, each shown twice as long, so the animation keeps its pace.
fn halve(frames: Vec<(RgbaImage, Delay)>) -> Vec<(RgbaImage, Delay)> {
    frames
        .into_iter()
        .step_by(2)
        .map(|(img, delay)| {
            let (n, d) = delay.numer_denom_ms();
            (img, Delay::from_numer_denom_ms(n.saturating_mul(2), d))
        })
        .collect()
}

/// Each frame scaled to `width` by `width * 9 / 16`.
fn scale(frames: &[(RgbaImage, Delay)], width: u32) -> Vec<(RgbaImage, Delay)> {
    let height = (width * 9 / 16).max(9);
    frames
        .iter()
        .map(|(img, delay)| (imageops::resize(img, width, height, imageops::FilterType::Triangle), *delay))
        .collect()
}

/// A GIF that loops forever. The encoder writes the frames and the loop block only, so no comment,
/// application data or other metadata from the source survives.
fn encode_gif(frames: &[(RgbaImage, Delay)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut out, 10);
        encoder.set_repeat(Repeat::Infinite).map_err(bad)?;
        encoder
            .encode_frames(frames.iter().map(|(img, delay)| Frame::from_parts(img.clone(), 0, 0, *delay)))
            .map_err(bad)?;
    }
    Ok(out)
}

fn encode_poster(first: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let rgb = DynamicImage::ImageRgba8(first.clone()).to_rgb8();
    JpegEncoder::new_with_quality(&mut out, POSTER_QUALITY)
        .encode_image(&rgb)
        .map_err(bad)?;
    Ok(out)
}

/// Checks a prepared banner before it is stored: a GIF within the cap and at most 960 by 540, and a
/// JPEG poster within the still banner cap.
fn check_prepared(gif: &[u8], poster: &[u8]) -> Result<()> {
    if !gif.starts_with(b"GIF8") || gif.is_empty() || gif.len() as u64 > MAX_ANIMATED_BANNER_BYTES {
        return Err(bad("the GIF must be at most 2 MB"));
    }
    let (w, h) = GifDecoder::new(Cursor::new(gif)).map_err(bad)?.dimensions();
    if w > 960 || h > 540 {
        return Err(bad("the GIF must be at most 960 by 540"));
    }
    if !poster.starts_with(&[0xFF, 0xD8, 0xFF]) || poster.len() as u64 > MAX_BANNER_BYTES {
        return Err(bad("the poster must be a JPEG of at most 512 KB"));
    }
    Ok(())
}

impl Engine {
    /// Re-encodes an animated GIF for a banner, in a blocking task so nothing else waits on it. The
    /// editor calls this while it shows its spinner, then hands the result to one of the setters.
    pub async fn prepare_animated_banner(&self, bytes: Vec<u8>, crop: CropRect) -> Result<EncodedBanner> {
        tokio::task::spawn_blocking(move || encode_banner(&bytes, crop))
            .await
            .map_err(|e| bad(format!("the encoder stopped: {e}")))?
    }

    async fn store_animated(&self, gif: Vec<u8>, poster: Vec<u8>) -> Result<Banner> {
        check_prepared(&gif, &poster)?;
        let image: Picture = self.store_avatar_picture(gif, MAX_ANIMATED_BANNER_BYTES).await?;
        let poster: Picture = self.store_avatar_picture(poster, MAX_BANNER_BYTES).await?;
        Ok(Banner::Animated { image, poster })
    }

    /// Sets a community's banner to a prepared animated GIF. Owners and admins only.
    pub async fn set_community_banner_animated(&self, community: CommunityId, gif: Vec<u8>, poster: Vec<u8>) -> Result<()> {
        self.check_can_set_icon(community)?;
        let banner = self.store_animated(gif, poster).await?;
        self.publish_banner(community, Some(banner)).await
    }

    /// Sets this device's profile banner to a prepared animated GIF.
    pub async fn set_profile_banner_animated(&self, gif: Vec<u8>, poster: Vec<u8>) -> Result<()> {
        let banner = self.store_animated(gif, poster).await?;
        self.publish_profile_banner(Some(banner)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A GIF of `frames` solid frames of `w` by `h`, 100 ms each, with a comment and a finite loop
    /// count, as a source from the web might have.
    fn source_gif(frames: usize, w: u32, h: u32, noisy: bool) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new_with_speed(&mut out, 30);
            enc.set_repeat(Repeat::Finite(2)).unwrap();
            for i in 0..frames {
                let img = RgbaImage::from_fn(w, h, |x, y| {
                    if noisy {
                        let v = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503) ^ (i as u32 * 97)) >> 7) as u8;
                        Rgba([v, v.wrapping_mul(3), v.wrapping_mul(7), 255])
                    } else {
                        Rgba([(i * 80) as u8, (x % 256) as u8, (y % 256) as u8, 255])
                    }
                });
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1))).unwrap();
            }
        }
        // Insert a comment extension after the header and screen descriptor, as metadata to strip.
        let comment = [0x21, 0xFE, 5, b's', b'e', b'c', b'r', b't', 0];
        let flags = out[10];
        let table = if flags & 0x80 != 0 { 3 << ((flags & 7) + 1) } else { 0 };
        let at = 13 + table;
        let mut with = out[..at].to_vec();
        with.extend_from_slice(&comment);
        with.extend_from_slice(&out[at..]);
        with
    }

    fn frames_of(gif: &[u8]) -> Vec<Frame> {
        GifDecoder::new(Cursor::new(gif)).unwrap().into_frames().collect_frames().unwrap()
    }

    #[test]
    fn a_three_frame_gif_is_cropped_keeps_its_frames_and_loops_forever() {
        let src = source_gif(3, 400, 300, false);
        assert!(is_animated_gif(&src));
        let out = encode_banner(&src, CropRect { x: 0, y: 0, w: 320, h: 180, rot: 0 }).unwrap();
        let frames = frames_of(&out.gif);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].buffer().dimensions(), (320, 180), "cropped, never scaled up");
        assert_eq!(frames[1].delay().numer_denom_ms(), (100, 1));
        // NETSCAPE2.0 with a loop count of 0, which is forever.
        let netscape = out.gif.windows(11).position(|w| w == b"NETSCAPE2.0").expect("a loop block");
        assert_eq!(&out.gif[netscape + 11..netscape + 15], &[3, 1, 0, 0]);
        assert!(!out.gif.windows(5).any(|w| w == b"secrt"), "the source's comment is gone");
        assert!(!out.gif.windows(2).any(|w| w == [0x21, 0xFE]), "no comment extension at all");
        assert!(out.poster.starts_with(&[0xFF, 0xD8, 0xFF]));
        check_prepared(&out.gif, &out.poster).unwrap();
    }

    #[test]
    fn a_large_source_is_scaled_to_960_by_540_and_rotation_is_applied() {
        let src = source_gif(2, 1200, 2000, false);
        let out = encode_banner(&src, CropRect { x: 0, y: 0, w: 2000, h: 1125, rot: 1 }).unwrap();
        assert_eq!(frames_of(&out.gif)[0].buffer().dimensions(), (960, 540));
    }

    #[test]
    fn too_many_frames_are_halved_with_longer_delays() {
        let src = source_gif(450, 32, 18, false);
        let out = encode_banner(&src, CropRect { x: 0, y: 0, w: 32, h: 18, rot: 0 }).unwrap();
        let frames = frames_of(&out.gif);
        assert!(frames.len() <= MAX_FRAMES, "{}", frames.len());
        assert_eq!(frames.len(), 113);
        assert_eq!(frames[0].delay().numer_denom_ms(), (400, 1));
    }

    #[test]
    fn an_oversize_gif_is_reduced_to_fit() {
        // Noise does not compress, so 40 full-size frames are far over 2 MB.
        let src = source_gif(40, 960, 540, true);
        let out = encode_banner(&src, CropRect { x: 0, y: 0, w: 960, h: 540, rot: 0 }).unwrap();
        assert!(out.gif.len() as u64 <= MAX_ANIMATED_BANNER_BYTES);
        let frames = frames_of(&out.gif);
        assert!(frames.len() == 20 || frames[0].buffer().width() < 960, "reduced: {} frames", frames.len());
    }

    #[test]
    fn a_still_or_foreign_file_is_refused() {
        assert!(!is_animated_gif(&source_gif(1, 16, 9, false)));
        assert!(encode_banner(b"RIFF....WEBP", CropRect { x: 0, y: 0, w: 1, h: 1, rot: 0 }).is_err());
        assert!(check_prepared(b"not a gif", &[0xFF, 0xD8, 0xFF]).is_err());
    }
}
