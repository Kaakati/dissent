//! JPEG: the header segments are read, filtered and written again; everything from the start of scan on is copied as it
//! is, so the compressed picture never changes.
//!
//! Kept: JFIF (APP0, rebuilt without its embedded thumbnail), an ICC colour profile (APP2, with its text cleared when it
//! is in one piece), Adobe's colour transform (APP14), and the tables and frame headers. Dropped: Exif and XMP (APP1),
//! JFXX thumbnails, MPF previews (APP2), IPTC and Photoshop data (APP13), every other application segment, and comments.
//! A rotation the Exif block asked for is kept, in a new Exif block that holds the Orientation tag alone.

use std::io::{Read, Write};

use super::{fail, icc, Outcome};

const EXIF_ID: &[u8] = b"Exif\0\0";
const ICC_ID: &[u8] = b"ICC_PROFILE\0";
const ORIENTATION_TAG: u16 = 0x0112;
/// The header segments are held in memory; real ones are a few hundred kilobytes at most.
const HEADER_MAX: usize = 32 * 1024 * 1024;

pub(super) fn rewrite<R: Read, W: Write>(input: &mut R, out: &mut W) -> Outcome<()> {
    let mut soi = [0u8; 2];
    input.read_exact(&mut soi)?;
    if soi != [0xFF, 0xD8] {
        return fail("not a JPEG");
    }
    let mut kept: Vec<Vec<u8>> = Vec::new();
    let mut held = 0usize;
    let mut orientation = None;
    // The marker that ends the header (start of scan, or end of image), with its fill bytes dropped.
    let end_marker;
    loop {
        let mut byte = [0u8; 1];
        input.read_exact(&mut byte)?;
        if byte[0] != 0xFF {
            return fail("a JPEG segment does not start with a marker");
        }
        let mut code = 0xFF;
        while code == 0xFF {
            input.read_exact(&mut byte)?;
            code = byte[0];
        }
        if code == 0xDA || code == 0xD9 {
            end_marker = code;
            break;
        }
        if (0xD0..=0xD7).contains(&code) || code == 0x01 {
            kept.push(vec![0xFF, code]);
            continue;
        }
        let mut len = [0u8; 2];
        input.read_exact(&mut len)?;
        let len = u16::from_be_bytes(len) as usize;
        if len < 2 {
            return fail("a JPEG segment has a bad length");
        }
        let payload = super::read_exact_vec(input, len - 2)?;
        held += payload.len();
        if held > HEADER_MAX {
            return fail("the JPEG header is too large");
        }
        let keep = match code {
            0xE0 if payload.starts_with(b"JFIF\0") => Some(jfif_without_thumbnail(&payload)),
            0xE1 => {
                if payload.starts_with(EXIF_ID) {
                    orientation = orientation.or_else(|| exif_orientation(&payload[EXIF_ID.len()..]));
                }
                None
            }
            0xE2 if payload.starts_with(ICC_ID) => {
                let mut payload = payload;
                // A profile in one piece (chunk 1 of 1) is cleaned; a split one keeps its text (see the threat model).
                if payload.get(12) == Some(&1) && payload.get(13) == Some(&1) {
                    icc::scrub(&mut payload[14..]);
                }
                Some(payload)
            }
            0xEE => Some(payload),
            0xE0..=0xEF | 0xFE => None,
            _ => Some(payload),
        };
        if let Some(payload) = keep {
            let mut segment = vec![0xFF, code];
            segment.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            segment.extend_from_slice(&payload);
            kept.push(segment);
        }
    }
    out.write_all(&[0xFF, 0xD8])?;
    let rotation = orientation.filter(|o| (2..=8).contains(o)).map(orientation_only_exif);
    // JFIF must stay first; the rotation goes right after it, or first when there is none.
    let mut placed = rotation.is_none();
    for segment in &kept {
        if !placed && segment.get(1) != Some(&0xE0) {
            out.write_all(rotation.as_deref().unwrap_or_default())?;
            placed = true;
        }
        out.write_all(segment)?;
    }
    if !placed {
        out.write_all(rotation.as_deref().unwrap_or_default())?;
    }
    out.write_all(&[0xFF, end_marker])?;
    std::io::copy(input, out)?;
    Ok(())
}

/// A JFIF header as it was, minus any thumbnail after its fixed fields.
fn jfif_without_thumbnail(payload: &[u8]) -> Vec<u8> {
    let mut fixed = payload.get(..14).map(<[u8]>::to_vec).unwrap_or_else(|| payload.to_vec());
    if fixed.len() == 14 {
        fixed[12] = 0; // no thumbnail width
        fixed[13] = 0; // or height
    }
    fixed
}

/// The Orientation value in an Exif TIFF block's first directory, if it has one.
pub(super) fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    let big = match tiff.get(0..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count.min(512)).find_map(|i| {
        let entry = ifd + 2 + i * 12;
        (u16_at(entry)? == ORIENTATION_TAG && u16_at(entry + 2)? == 3).then(|| u16_at(entry + 8)).flatten()
    })
}

/// An APP1 Exif segment holding the Orientation tag and nothing else.
pub(super) fn orientation_only_exif(orientation: u16) -> Vec<u8> {
    let mut tiff = b"MM\0\x2A\0\0\0\x08".to_vec(); // big-endian, first directory at offset 8
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
    tiff.extend_from_slice(&ORIENTATION_TAG.to_be_bytes());
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes()); // one value
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]); // the value is padded to four bytes
    tiff.extend_from_slice(&0u32.to_be_bytes()); // no next directory
    let len = (2 + EXIF_ID.len() + tiff.len()) as u16;
    let mut segment = vec![0xFF, 0xE1];
    segment.extend_from_slice(&len.to_be_bytes());
    segment.extend_from_slice(EXIF_ID);
    segment.extend_from_slice(&tiff);
    segment
}
