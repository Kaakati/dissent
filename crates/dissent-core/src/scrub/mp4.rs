//! MP4, MOV, M4V and M4A: every change is a same-length overwrite inside the `moov` box (or of a top-level XMP box), so
//! no chunk offset moves and the media data is never touched.
//!
//! - `udta` and `meta` boxes, wherever they are: location (`©xyz`), make, model, software, dates, author and every
//!   Apple and Android key (`com.apple.quicktime.*`, `com.android.*`) go with them. They become zeroed `free` boxes.
//! - Creation and modification times in `mvhd`, `tkhd` and `mdhd` are set to 0.
//! - Handler names (`hdlr`) and the compressor name and vendor of each video sample entry are zeroed.
//! - A top-level XMP `uuid` box becomes a zeroed `free` box.

use std::io::{Read, Seek, SeekFrom};

use super::{be32, be64, fail, Edit, Outcome};

/// A `moov` box larger than this is not read. Real ones are a few megabytes at most.
const MOOV_MAX: u64 = 64 * 1024 * 1024;
/// The UUID of an XMP packet box.
const XMP_UUID: [u8; 16] = [0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC];
/// Video sample entries, whose compressor name and vendor are zeroed.
const VISUAL: [&[u8; 4]; 16] = [
    b"avc1", b"avc3", b"hvc1", b"hev1", b"av01", b"vp08", b"vp09", b"mp4v", b"jpeg", b"mjpa", b"mjpb", b"apcn", b"apch",
    b"apcs", b"apco", b"ap4h",
];

/// A box at the top of the file.
pub(super) struct TopBox {
    pub kind: [u8; 4],
    pub offset: u64,
    pub header: u64,
    pub size: u64,
}

/// The boxes at the top of a file of `len` bytes.
pub(super) fn top_level<R: Read + Seek>(r: &mut R, len: u64) -> Outcome<Vec<TopBox>> {
    let mut out = Vec::new();
    let mut at = 0u64;
    while at + 8 <= len {
        r.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 16];
        r.read_exact(&mut head[..8])?;
        let mut size = be32(&head, 0).expect("4 bytes") as u64;
        let mut header = 8u64;
        if size == 1 {
            r.read_exact(&mut head[8..16])?;
            size = be64(&head, 8).expect("8 bytes");
            header = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < header || at + size > len {
            return fail("an MP4 box runs past the end of the file");
        }
        out.push(TopBox { kind: head[4..8].try_into().expect("4 bytes"), offset: at, header, size });
        at += size;
    }
    Ok(out)
}

pub(super) fn is_xmp_uuid<R: Read + Seek>(r: &mut R, b: &TopBox) -> Outcome<bool> {
    if b.size < b.header + 16 {
        return Ok(false);
    }
    r.seek(SeekFrom::Start(b.offset + b.header))?;
    let mut id = [0u8; 16];
    r.read_exact(&mut id)?;
    Ok(id == XMP_UUID)
}

/// A top-level box turned into a zeroed `free` box of the same size.
pub(super) fn free_whole(b: &TopBox) -> Vec<Edit> {
    let type_at = b.offset + 4;
    vec![
        Edit::Bytes { offset: type_at, bytes: b"free".to_vec() },
        Edit::Zero { offset: b.offset + b.header, len: b.size - b.header },
    ]
}

/// A box inside a parent's payload, offsets relative to that payload.
pub(super) struct Child {
    pub kind: [u8; 4],
    pub start: usize,
    pub header: usize,
    pub payload: std::ops::Range<usize>,
}

pub(super) fn children(data: &[u8]) -> Vec<Child> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = be32(data, pos).expect("4 bytes") as usize;
        let (size, header) = match size {
            0 => (data.len() - pos, 8),
            1 => match be64(data, pos + 8) {
                Some(s) => (s as usize, 16),
                None => break,
            },
            _ => (size, 8),
        };
        if size < header || pos + size > data.len() {
            break;
        }
        out.push(Child { kind: data[pos + 4..pos + 8].try_into().expect("4 bytes"), start: pos, header, payload: pos + header..pos + size });
        pos += size;
    }
    out
}

/// The `moov` payload, where it starts in the file, and the top-level boxes.
type Moov = (Vec<u8>, u64, Vec<TopBox>);

fn read_moov<R: Read + Seek>(r: &mut R) -> Outcome<Option<Moov>> {
    let len = r.seek(SeekFrom::End(0))?;
    let top = top_level(r, len)?;
    let Some(moov) = top.iter().find(|b| &b.kind == b"moov") else {
        return Ok(None);
    };
    if moov.size - moov.header > MOOV_MAX {
        return fail("the MP4 moov box is too large");
    }
    r.seek(SeekFrom::Start(moov.offset + moov.header))?;
    let data = super::read_exact_vec(r, (moov.size - moov.header) as usize)?;
    let at = moov.offset + moov.header;
    Ok(Some((data, at, top)))
}

pub(super) fn edits<R: Read + Seek>(r: &mut R) -> Outcome<Vec<Edit>> {
    let Some((moov, moov_at, top)) = read_moov(r)? else {
        return fail("an MP4 file without a moov box");
    };
    let mut edits = Vec::new();
    for b in &top {
        if (&b.kind == b"uuid" && is_xmp_uuid(r, b)?) || &b.kind == b"meta" {
            edits.extend(free_whole(b));
        }
    }
    walk(&moov, moov_at, &mut edits);
    Ok(edits)
}

fn free_child(base: u64, c: &Child) -> [Edit; 2] {
    let at = base + c.start as u64;
    // A 64-bit box keeps its size fields; the type is right after the 32-bit size either way.
    [
        Edit::Bytes { offset: at + 4, bytes: b"free".to_vec() },
        Edit::Zero { offset: at + c.header as u64, len: c.payload.len() as u64 },
    ]
}

fn walk(data: &[u8], base: u64, edits: &mut Vec<Edit>) {
    for c in children(data) {
        let p = &data[c.payload.clone()];
        let p_at = base + c.payload.start as u64;
        match &c.kind {
            b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" => walk(p, p_at, edits),
            b"udta" | b"meta" => edits.extend(free_child(base, &c)),
            b"mvhd" | b"tkhd" | b"mdhd" => {
                // Version 0: 32-bit creation and modification times after the version and flags; version 1: 64-bit.
                let n = if p.first() == Some(&1) { 16 } else { 8 };
                if p.len() >= 4 + n {
                    edits.push(Edit::Zero { offset: p_at + 4, len: n as u64 });
                }
            }
            b"hdlr" => {
                // Version and flags, pre_defined (or component type), handler type and 12 reserved bytes: the name follows.
                if p.len() > 24 {
                    edits.push(Edit::Zero { offset: p_at + 24, len: (p.len() - 24) as u64 });
                }
            }
            b"stsd" => {
                // Version and flags and an entry count, then one box per sample entry.
                let entries_at = 8;
                for e in children(p.get(entries_at..).unwrap_or_default()) {
                    if !VISUAL.iter().any(|v| **v == e.kind) {
                        continue;
                    }
                    let e_at = p_at + (entries_at + e.payload.start) as u64;
                    // The vendor (bytes 12..16 of the entry) and the 32-byte compressor name (42..74).
                    if e.payload.len() >= 74 {
                        edits.push(Edit::Zero { offset: e_at + 12, len: 4 });
                        edits.push(Edit::Zero { offset: e_at + 42, len: 32 });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Whether the file names the device or place it was recorded with: a location, a make or model, or an Apple or Android
/// recording key. Such a video is likely personal, so "Remove identifying details" starts on for it.
pub(super) fn has_device_details<R: Read + Seek>(r: &mut R) -> Outcome<bool> {
    let Some((moov, _, _)) = read_moov(r)? else { return Ok(false) };
    let markers: [&[u8]; 6] = [b"\xA9xyz", b"\xA9mak", b"\xA9mod", b"com.apple.quicktime.location", b"com.apple.quicktime.make", b"com.android."];
    Ok(markers.iter().any(|m| moov.windows(m.len()).any(|w| w == *m)))
}

#[cfg(test)]
pub(super) fn tests_xmp_uuid() -> [u8; 16] {
    XMP_UUID
}
