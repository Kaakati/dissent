//! Removing location and camera details from photos and videos before they are sent. Audit finding P-28: a photo kept
//! its EXIF block, GPS position included, and a phone video its recorded location.
//!
//! Every change is lossless. Pixels and audio are never decoded or re-encoded:
//! - JPEG: the Exif, XMP, IPTC and comment segments go, and so do the other application segments except JFIF (APP0),
//!   an ICC colour profile (APP2) and Adobe's colour transform (APP14). A rotation the Exif block asked for is kept,
//!   in a new Exif block that holds the Orientation tag alone.
//! - PNG: the text chunks (`tEXt`, `iTXt`, `zTXt`), `eXIf` and `tIME` go.
//! - WebP: the `EXIF` and `XMP ` chunks go, and their flags in `VP8X` are cleared.
//! - MP4 and MOV: the location, make and model entries (`©xyz`, `©mak`, `©mod`, and Apple's `com.apple.quicktime`
//!   location, make and model keys) are overwritten in place as `free` boxes of the same size, so no offset in the
//!   file moves.
//!
//! HEIC, AVIF, TIFF, GIF and other formats are sent as they are.

use std::io::{self, Read, Seek, SeekFrom};

const EXIF_ID: &[u8] = b"Exif\0\0";
const ICC_ID: &[u8] = b"ICC_PROFILE\0";
/// Orientation, the one Exif tag kept: it says how to turn the picture upright.
const ORIENTATION_TAG: u16 = 0x0112;
/// A `moov` box larger than this is not read. Real ones are a few megabytes at most.
const MOOV_MAX: u64 = 64 * 1024 * 1024;

/// The image with its metadata removed, or `None` when it has none to remove or is not a format handled here.
pub(crate) fn strip_image(mime: &str, data: &[u8]) -> Option<Vec<u8>> {
    match mime {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => strip_jpeg(data),
        "image/png" | "image/apng" => strip_png(data),
        "image/webp" => strip_webp(data),
        _ => None,
    }
}

/// Whether `mime` names a video whose metadata `mp4_patches` can remove.
pub(crate) fn is_mp4_family(mime: &str) -> bool {
    matches!(mime, "video/mp4" | "video/quicktime" | "video/x-m4v" | "video/3gpp" | "audio/mp4")
}

// ---- JPEG ----

fn strip_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut kept: Vec<&[u8]> = Vec::new(); // whole segments, marker included
    let mut orientation = None;
    let mut removed = false;
    let mut pos = 2;
    let rest_from;
    loop {
        // A marker is 0xFF and a code; 0xFF fill bytes may come before the code.
        if pos >= data.len() || data[pos] != 0xFF {
            return None; // not a well-formed header: leave the file alone
        }
        let mut code_at = pos + 1;
        while code_at < data.len() && data[code_at] == 0xFF {
            code_at += 1;
        }
        let code = *data.get(code_at)?;
        if code == 0xDA || code == 0xD9 {
            // Start of scan, or an image with no scan: everything from here is the image itself.
            rest_from = pos;
            break;
        }
        if (0xD0..=0xD7).contains(&code) || code == 0x01 {
            kept.push(&data[pos..code_at + 1]);
            pos = code_at + 1;
            continue;
        }
        let len = u16::from_be_bytes([*data.get(code_at + 1)?, *data.get(code_at + 2)?]) as usize;
        if len < 2 || code_at + 1 + len > data.len() {
            return None;
        }
        let end = code_at + 1 + len;
        let payload = &data[code_at + 3..end];
        let drop = match code {
            0xE1 => {
                if payload.starts_with(EXIF_ID) {
                    orientation = orientation.or_else(|| exif_orientation(&payload[EXIF_ID.len()..]));
                }
                true // Exif and XMP
            }
            0xE2 => !payload.starts_with(ICC_ID), // keep the colour profile, drop MPF (it holds more images and their Exif)
            0xE0 | 0xEE => false,                 // JFIF, Adobe colour transform
            0xE3..=0xED | 0xEF => true,           // other application data, IPTC (APP13) included
            0xFE => true,                         // comments
            _ => false,                           // tables, frame headers and the rest of the image structure
        };
        if drop {
            removed = true;
        } else {
            kept.push(&data[pos..end]);
        }
        pos = end;
    }
    if !removed {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let orientation_segment = orientation.filter(|o| (2..=8).contains(o)).map(orientation_only_exif);
    // JFIF must stay the first segment; the new Exif block goes right after it, or first when there is none.
    let mut placed = orientation_segment.is_none();
    for segment in kept {
        let is_app0 = segment.len() > 1 && segment[1] == 0xE0;
        if !placed && !is_app0 {
            out.extend_from_slice(orientation_segment.as_deref().unwrap_or_default());
            placed = true;
        }
        out.extend_from_slice(segment);
    }
    if !placed {
        out.extend_from_slice(orientation_segment.as_deref().unwrap_or_default());
    }
    out.extend_from_slice(&data[rest_from..]);
    Some(out)
}

/// The Orientation value in an Exif TIFF block's first directory, if it has one.
fn exif_orientation(tiff: &[u8]) -> Option<u16> {
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
        let b = [*tiff.get(at)?, *tiff.get(at + 1)?, *tiff.get(at + 2)?, *tiff.get(at + 3)?];
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
fn orientation_only_exif(orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\0\x2A\0\0\0\x08"); // big-endian, first directory at offset 8
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

// ---- PNG ----

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_DROPPED: [&[u8; 4]; 5] = [b"tEXt", b"iTXt", b"zTXt", b"eXIf", b"tIME"];

fn strip_png(data: &[u8]) -> Option<Vec<u8>> {
    if !data.starts_with(PNG_SIGNATURE) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(PNG_SIGNATURE);
    let mut pos = PNG_SIGNATURE.len();
    let mut removed = false;
    while pos < data.len() {
        let len = u32::from_be_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
        let end = pos.checked_add(12)?.checked_add(len)?;
        let kind = data.get(pos + 4..pos + 8)?;
        if end > data.len() {
            return None;
        }
        if PNG_DROPPED.iter().any(|d| d.as_slice() == kind) {
            removed = true;
        } else {
            out.extend_from_slice(&data[pos..end]);
        }
        pos = end;
        if kind == b"IEND" {
            break;
        }
    }
    removed.then_some(out)
}

// ---- WebP ----

fn strip_webp(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[0..12]);
    let mut pos = 12;
    let mut removed = false;
    while pos + 8 <= data.len() {
        let kind = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let end = pos.checked_add(8)?.checked_add(len + (len & 1))?.min(data.len());
        if pos + 8 + len > data.len() {
            return None;
        }
        match kind {
            b"EXIF" | b"XMP " => removed = true,
            b"VP8X" => {
                let start = out.len();
                out.extend_from_slice(&data[pos..end]);
                // Flag bits: 0x08 is Exif, 0x04 is XMP.
                out[start + 8] &= !0x0C;
            }
            _ => out.extend_from_slice(&data[pos..end]),
        }
        pos = end;
    }
    if !removed {
        return None;
    }
    let riff_len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_len.to_le_bytes());
    Some(out)
}

// ---- MP4 and MOV ----

/// The metadata keys of Apple's `mdta` key list whose values are removed.
fn sensitive_key(name: &[u8]) -> bool {
    let name = String::from_utf8_lossy(name).to_ascii_lowercase();
    name.starts_with("com.apple.quicktime.location")
        || name == "com.apple.quicktime.make"
        || name == "com.apple.quicktime.model"
}

/// User-data entries removed: the recorded location, and the camera's make and model.
const UDTA_DROPPED: [&[u8; 4]; 3] = [b"\xA9xyz", b"\xA9mak", b"\xA9mod"];

/// A same-length overwrite at `offset` in the file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Patch {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// The overwrites that remove the location, make and model from an MP4 or MOV file. Empty when it holds none, or
/// when its structure is not one this reads.
pub(crate) fn mp4_patches<R: Read + Seek>(file: &mut R) -> io::Result<Vec<Patch>> {
    let len = file.seek(SeekFrom::End(0))?;
    let mut at = 0u64;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8])?;
        let mut size = u32::from_be_bytes(head[0..4].try_into().expect("4 bytes")) as u64;
        let mut header = 8u64;
        if size == 1 {
            file.read_exact(&mut head[8..16])?;
            size = u64::from_be_bytes(head[8..16].try_into().expect("8 bytes"));
            header = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < header || at + size > len {
            return Ok(Vec::new());
        }
        if &head[4..8] == b"moov" {
            if size > MOOV_MAX {
                return Ok(Vec::new());
            }
            let mut moov = vec![0u8; (size - header) as usize];
            file.seek(SeekFrom::Start(at + header))?;
            file.read_exact(&mut moov)?;
            let mut patches = Vec::new();
            walk(&moov, at + header, &mut patches, false);
            return Ok(patches);
        }
        at += size;
    }
    Ok(Vec::new())
}

/// One box inside a parent: `(type, payload, payload offset, whole box)`, offsets relative to the parent's data.
type Child<'a> = (&'a [u8], &'a [u8], usize, std::ops::Range<usize>);

/// The boxes inside `data`.
fn boxes(data: &[u8]) -> Vec<Child<'_>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().expect("4 bytes")) as usize;
        let (size, header) = match size {
            0 => (data.len() - pos, 8),
            1 if pos + 16 <= data.len() => (u64::from_be_bytes(data[pos + 8..pos + 16].try_into().expect("8 bytes")) as usize, 16),
            _ => (size, 8),
        };
        if size < header || pos + size > data.len() {
            break;
        }
        out.push((&data[pos + 4..pos + 8], &data[pos + header..pos + size], pos + header, pos..pos + size));
        pos += size;
    }
    out
}

/// A `free` box of the same size as `range` (header included), its payload zeroed.
fn free_patch(base: u64, range: &std::ops::Range<usize>, data: &[u8]) -> Patch {
    let mut bytes = vec![0u8; range.len()];
    bytes[..4].copy_from_slice(&data[range.start..range.start + 4]);
    if bytes[..4] == [0, 0, 0, 1] {
        // A 64-bit size: keep it, and the type after the size fields becomes free.
        bytes[8..16].copy_from_slice(&data[range.start + 8..range.start + 16]);
    }
    bytes[4..8].copy_from_slice(b"free");
    Patch { offset: base + range.start as u64, bytes }
}

fn walk(data: &[u8], base: u64, patches: &mut Vec<Patch>, in_udta: bool) {
    for (kind, payload, payload_at, range) in boxes(data) {
        match kind {
            b"trak" | b"udta" | b"mdia" | b"minf" => walk(payload, base + payload_at as u64, patches, in_udta || kind == b"udta"),
            b"meta" => meta(payload, base + payload_at as u64, patches),
            k if in_udta && UDTA_DROPPED.iter().any(|d| d.as_slice() == k) => patches.push(free_patch(base, &range, data)),
            _ => {}
        }
    }
}

/// A `meta` box: QuickTime's holds its children at once, ISO's after four bytes of version and flags.
fn meta(payload: &[u8], base: u64, patches: &mut Vec<Patch>) {
    let skip = if payload.get(4..8) == Some(b"hdlr") || payload.get(4..8) == Some(b"keys") { 0 } else { 4 };
    let Some(children) = payload.get(skip..) else { return };
    let base = base + skip as u64;
    let list = boxes(children);
    // Apple's keyed metadata: `keys` names each entry, and `ilst` holds the values as boxes typed by 1-based index.
    let sensitive: Vec<u32> = list
        .iter()
        .find(|(kind, ..)| *kind == b"keys")
        .map(|(_, keys, ..)| {
            let mut found = Vec::new();
            let mut pos = 8; // version, flags and the entry count
            let mut index = 1u32;
            while pos + 8 <= keys.len() {
                let size = u32::from_be_bytes(keys[pos..pos + 4].try_into().expect("4 bytes")) as usize;
                if size < 8 || pos + size > keys.len() {
                    break;
                }
                if sensitive_key(&keys[pos + 8..pos + size]) {
                    found.push(index);
                }
                index += 1;
                pos += size;
            }
            found
        })
        .unwrap_or_default();
    for (kind, items, items_at, _) in &list {
        if *kind == b"ilst" {
            for (item, _, _, range) in boxes(items) {
                let index = u32::from_be_bytes(item.try_into().expect("4 bytes"));
                if sensitive.contains(&index) || UDTA_DROPPED.iter().any(|d| d.as_slice() == item) {
                    patches.push(free_patch(base + *items_at as u64, &range, items));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
