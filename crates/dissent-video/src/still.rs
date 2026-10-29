//! The first frame of a video file, for its thumbnail. MP4 and MOV are read with `re_mp4`, WebM and
//! MKV with `matroska-demuxer`, both pure Rust. H.264 is decoded with OpenH264 and AV1 with rav1d,
//! the decoders calls use. Any other codec (VP9, HEVC, ...) is [`VideoError::Unsupported`], so the
//! caller sends the file without a thumbnail. See `docs/design/08-files.md`, "Video thumbnails".

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use image::{imageops, RgbImage};

use crate::codec::Codec;
use crate::decode;
use crate::error::{Result, VideoError};

/// Pictures larger than this many pixels are not decoded: about 8K.
const MAX_PIXELS: u64 = 8192 * 4608;
/// A compressed frame larger than this is not read.
const MAX_SAMPLE_BYTES: u64 = 16 * 1024 * 1024;
/// Compressed frames fed to the decoder before giving up on a picture. A decoder that reorders
/// frames can hold the first ones back.
const MAX_FED: usize = 120;
/// Frames of other tracks skipped in a Matroska file before giving up on the video track.
const MAX_SKIPPED: usize = 4096;

/// A decoded picture as packed RGB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// How a container says to turn the picture for display. MP4 and MOV carry it in the track matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    None,
    Clockwise,
    Half,
    CounterClockwise,
}

/// The first picture of the video track in the file at `path`, turned as the file says to show it,
/// and shrunk so that its longer side is at most `long_side`.
pub fn first_frame(path: &Path, long_side: u32) -> Result<Still> {
    let mut file = File::open(path).map_err(io)?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).map_err(io)?;
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    let (codec, frames, turn) = if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        matroska_frames(file)?
    } else if &magic[4..8] == b"ftyp" {
        mp4_frames(file)?
    } else {
        return Err(VideoError::Unsupported("not an MP4, MOV, WebM or MKV file"));
    };
    let still = decode_first(codec, frames, long_side)?;
    Ok(turned(still, turn))
}

/// A codec, the compressed frames from the first keyframe on, in decode order, and the turn.
type Frames = (Codec, Box<dyn Iterator<Item = Result<Vec<u8>>>>, Turn);

fn mp4_frames(file: File) -> Result<Frames> {
    use re_mp4::{StsdBoxContent, TrackKind};
    let size = file.metadata().map_err(io)?.len();
    let mut reader = BufReader::new(file);
    let mp4 = re_mp4::Mp4::read(&mut reader, size).map_err(|e| VideoError::Decode(format!("mp4: {e}")))?;
    let track = mp4
        .tracks()
        .values()
        .find(|t| t.kind == Some(TrackKind::Video) && !t.samples.is_empty())
        .ok_or(VideoError::Unsupported("no video track"))?;
    check_pixels(u32::from(track.width), u32::from(track.height))?;
    let trak = track.trak(&mp4);
    let (codec, mut prefix, length_size) = match &trak.mdia.minf.stbl.stsd.contents {
        StsdBoxContent::Avc1(avc) => {
            let c = &avc.avcc.contents;
            let sps: Vec<&[u8]> = c.sequence_parameter_sets.iter().map(|n| n.bytes.as_slice()).collect();
            let pps: Vec<&[u8]> = c.picture_parameter_sets.iter().map(|n| n.bytes.as_slice()).collect();
            (Codec::H264, annex_b_params(&sps, &pps), usize::from(c.length_size_minus_one & 3) + 1)
        }
        StsdBoxContent::Av01(av1) => (Codec::Av1, av1.av1c.contents.config_obus.clone(), 0),
        _ => return Err(VideoError::Unsupported("video codec has no decoder here")),
    };
    let m = &trak.tkhd.matrix;
    let turn = turn_of(m.a, m.b, m.c, m.d);
    let start = track.samples.iter().position(|s| s.is_sync).unwrap_or(0);
    let samples: Vec<(u64, u64)> = track.samples[start..].iter().take(MAX_FED).map(|s| (s.offset, s.size)).collect();
    let mut file = reader.into_inner();
    let frames = samples.into_iter().map(move |(offset, size)| {
        if size > MAX_SAMPLE_BYTES {
            return Err(VideoError::Unsupported("video frame too large"));
        }
        let mut data = vec![0u8; size as usize];
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        file.read_exact(&mut data).map_err(io)?;
        let mut out = std::mem::take(&mut prefix);
        if codec == Codec::H264 {
            out.extend(annex_b_sample(&data, length_size)?);
        } else {
            out.extend(data);
        }
        Ok(out)
    });
    Ok((codec, Box::new(frames), turn))
}

fn matroska_frames(file: File) -> Result<Frames> {
    use matroska_demuxer::{Frame, MatroskaFile, TrackType};
    let demux = |e: matroska_demuxer::DemuxError| VideoError::Decode(format!("matroska: {e}"));
    let mut mkv = MatroskaFile::open(BufReader::new(file)).map_err(demux)?;
    let track = mkv
        .tracks()
        .iter()
        .find(|t| t.track_type() == TrackType::Video)
        .ok_or(VideoError::Unsupported("no video track"))?;
    // Header stripping and compression change the frame bytes. They are rare for video, so they are not undone.
    if track.content_encodings().is_some_and(|e| !e.is_empty()) {
        return Err(VideoError::Unsupported("encoded Matroska track"));
    }
    if let Some(v) = track.video() {
        check_pixels(v.pixel_width().get() as u32, v.pixel_height().get() as u32)?;
    }
    let number = track.track_number().get();
    let private = track.codec_private().unwrap_or_default().to_vec();
    let (codec, mut prefix, length_size) = match track.codec_id() {
        "V_MPEG4/ISO/AVC" => {
            let (length_size, sps, pps) = parse_avcc(&private)?;
            (Codec::H264, annex_b_params(&sps, &pps), length_size)
        }
        // The private data is an av1C record: four bytes, then the config OBUs.
        "V_AV1" => (Codec::Av1, private.get(4..).unwrap_or_default().to_vec(), 0),
        _ => return Err(VideoError::Unsupported("video codec has no decoder here")),
    };
    let mut started = false;
    let mut fed = 0;
    let frames = std::iter::from_fn(move || {
        if fed >= MAX_FED {
            return None;
        }
        let mut frame = Frame::default();
        let mut skipped = 0;
        loop {
            match mkv.next_frame(&mut frame) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => return Some(Err(demux(e))),
            }
            // Begin at the first keyframe. A block that does not say is taken as one.
            if frame.track == number && (started || frame.is_keyframe != Some(false)) {
                break;
            }
            skipped += 1;
            if skipped > MAX_SKIPPED {
                return None;
            }
        }
        started = true;
        fed += 1;
        let mut out = std::mem::take(&mut prefix);
        if codec == Codec::H264 {
            match annex_b_sample(&frame.data, length_size) {
                Ok(nals) => out.extend(nals),
                Err(e) => return Some(Err(e)),
            }
        } else {
            out.extend(std::mem::take(&mut frame.data));
        }
        Some(Ok(out))
    });
    Ok((codec, Box::new(frames), Turn::None))
}

/// Feeds frames to a decoder until it gives a picture, then shrinks that picture to `long_side`.
fn decode_first(codec: Codec, frames: Box<dyn Iterator<Item = Result<Vec<u8>>>>, long_side: u32) -> Result<Still> {
    let mut decoder = decode::open(codec)?;
    let mut last_error = None;
    for (i, frame) in frames.enumerate() {
        match decoder.decode(&frame?, i as u64) {
            Ok(pictures) => {
                if let Some(picture) = pictures.first() {
                    let (w, h) = (picture.width(), picture.height());
                    check_pixels(w, h)?;
                    let longest = w.max(h);
                    let max_width = if longest <= long_side { w } else { (u64::from(w) * u64::from(long_side) / u64::from(longest)) as u32 };
                    let (width, height, rgb) = picture.to_rgb_preview(max_width.max(1));
                    return Ok(Still { width, height, rgb });
                }
            }
            // A frame before the decoder has what it needs can fail. Later ones may still decode.
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| VideoError::Decode("no picture in the first frames".into())))
}

/// The turn a track matrix describes, read from its rotation part. `a b / c d` in 16.16 fixed point,
/// as `x' = a x + c y`, `y' = b x + d y`. Anything that is not a quarter turn is shown as stored.
fn turn_of(a: i32, b: i32, c: i32, d: i32) -> Turn {
    const ONE: i32 = 1 << 16;
    const MINUS_ONE: i32 = -ONE;
    match (a, b, c, d) {
        (0, ONE, MINUS_ONE, 0) => Turn::Clockwise,
        (MINUS_ONE, 0, 0, MINUS_ONE) => Turn::Half,
        (0, MINUS_ONE, ONE, 0) => Turn::CounterClockwise,
        _ => Turn::None,
    }
}

fn turned(still: Still, turn: Turn) -> Still {
    if turn == Turn::None {
        return still;
    }
    let Some(picture) = RgbImage::from_raw(still.width, still.height, still.rgb.clone()) else {
        return still;
    };
    let out = match turn {
        Turn::Clockwise => imageops::rotate90(&picture),
        Turn::Half => imageops::rotate180(&picture),
        Turn::CounterClockwise => imageops::rotate270(&picture),
        Turn::None => picture,
    };
    Still {
        width: out.width(),
        height: out.height(),
        rgb: out.into_raw(),
    }
}

fn check_pixels(width: u32, height: u32) -> Result<()> {
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(VideoError::Unsupported("video picture too large"));
    }
    Ok(())
}

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// The parameter sets as Annex B NAL units, which is what OpenH264 reads.
fn annex_b_params(sps: &[&[u8]], pps: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in sps.iter().chain(pps) {
        out.extend(START_CODE);
        out.extend_from_slice(nal);
    }
    out
}

/// A sample of length-prefixed NAL units (MP4 and Matroska store H.264 so) as Annex B.
fn annex_b_sample(data: &[u8], length_size: usize) -> Result<Vec<u8>> {
    let bad = || VideoError::Decode("malformed H.264 sample".into());
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut rest = data;
    while !rest.is_empty() {
        let head = rest.get(..length_size).ok_or_else(bad)?;
        let len = head.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b));
        let nal = rest.get(length_size..length_size + len).ok_or_else(bad)?;
        out.extend(START_CODE);
        out.extend_from_slice(nal);
        rest = &rest[length_size + len..];
    }
    Ok(out)
}

/// The NAL length size of an avcC record, then its SPS and PPS units.
type AvcConfig<'a> = (usize, Vec<&'a [u8]>, Vec<&'a [u8]>);

/// Reads an AVCDecoderConfigurationRecord: the NAL length size, then the SPS and PPS units.
fn parse_avcc(record: &[u8]) -> Result<AvcConfig<'_>> {
    let bad = || VideoError::Decode("malformed avcC record".into());
    let length_size = usize::from(record.get(4).ok_or_else(bad)? & 3) + 1;
    let mut at = 5;
    let sets = |count_mask: u8, at: &mut usize| -> Result<Vec<&[u8]>> {
        let count = record.get(*at).ok_or_else(bad)? & count_mask;
        *at += 1;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = usize::from(u16::from_be_bytes([*record.get(*at).ok_or_else(bad)?, *record.get(*at + 1).ok_or_else(bad)?]));
            out.push(record.get(*at + 2..*at + 2 + len).ok_or_else(bad)?);
            *at += 2 + len;
        }
        Ok(out)
    };
    let sps = sets(0x1F, &mut at)?;
    let pps = sets(0xFF, &mut at)?;
    Ok((length_size, sps, pps))
}

fn io(e: std::io::Error) -> VideoError {
    VideoError::Decode(format!("reading the video: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_prefixed_nals_become_annex_b() {
        let sample = [0, 0, 0, 2, 0x65, 0xAA, 0, 0, 0, 1, 0x41];
        assert_eq!(annex_b_sample(&sample, 4).unwrap(), vec![0, 0, 0, 1, 0x65, 0xAA, 0, 0, 0, 1, 0x41]);
        assert!(annex_b_sample(&[0, 0, 0, 9, 1], 4).is_err(), "a length past the end is refused");
    }

    #[test]
    fn an_avcc_record_gives_its_parameter_sets() {
        let record = [1, 0x64, 0, 0x1F, 0xFF, 0xE1, 0, 2, 0x67, 1, 1, 0, 1, 0x68];
        let (len, sps, pps) = parse_avcc(&record).unwrap();
        assert_eq!(len, 4);
        assert_eq!(sps, vec![&[0x67, 1][..]]);
        assert_eq!(pps, vec![&[0x68][..]]);
        assert!(parse_avcc(&record[..9]).is_err());
    }

    #[test]
    fn quarter_turns_are_read_from_the_matrix() {
        const ONE: i32 = 1 << 16;
        assert_eq!(turn_of(ONE, 0, 0, ONE), Turn::None);
        assert_eq!(turn_of(0, ONE, -ONE, 0), Turn::Clockwise);
        assert_eq!(turn_of(-ONE, 0, 0, -ONE), Turn::Half);
        assert_eq!(turn_of(0, -ONE, ONE, 0), Turn::CounterClockwise);
    }
}
