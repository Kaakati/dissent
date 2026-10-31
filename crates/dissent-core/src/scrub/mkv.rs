//! Matroska and WebM: the identifying elements are turned into zeroed `Void` elements of the same size, so no offset
//! moves and the clusters are never touched. Gone: the segment's title, date, muxing and writing applications; every
//! track's name; all tags; and all attachments (cover art and the files some tools attach).
//!
//! A segment of unknown size (a live recording) cannot be walked to its end, so such a file is refused rather than sent
//! with tags that may come after its first cluster.

use std::io::{Read, Seek, SeekFrom};

use super::{fail, Edit, Outcome};

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TAGS: u32 = 0x1254_C367;
const ATTACHMENTS: u32 = 0x1941_A469;
/// Children of Info that are cleared: Title, DateUTC, MuxingApp and WritingApp.
const INFO_CLEARED: [u32; 4] = [0x7BA9, 0x4461, 0x4D80, 0x5741];
/// A track's Name.
const TRACK_NAME: u32 = 0x536E;
const VOID: u8 = 0xEC;
/// Info and Tracks are read whole; real ones are a few kilobytes.
const HEADER_MAX: u64 = 16 * 1024 * 1024;

/// An element: where it starts, its id, and where its data is.
struct Element {
    start: u64,
    id: u32,
    data_at: u64,
    data_len: u64,
}

/// A variable-length integer: its value (with the length marker kept for ids) and its length in bytes.
fn vint(bytes: &[u8], keep_marker: bool) -> Option<(u64, usize)> {
    let first = *bytes.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || bytes.len() < len {
        return None;
    }
    let mut v = if keep_marker { first as u64 } else { (first as u64) & ((1u64 << (8 - len)) - 1) };
    for b in &bytes[1..len] {
        v = (v << 8) | *b as u64;
    }
    Some((v, len))
}

fn unknown_size(raw: u64, len: usize) -> bool {
    raw == (1u64 << (7 * len)) - 1
}

/// Reads the element header at `at`.
fn header<R: Read + Seek>(r: &mut R, at: u64, end: u64) -> Outcome<Element> {
    r.seek(SeekFrom::Start(at))?;
    let mut buf = [0u8; 12];
    let n = r.read(&mut buf)?;
    let buf = &buf[..n];
    let (id, id_len) = vint(buf, true).ok_or_else(|| super::Failed("a Matroska id is malformed".into()))?;
    if id_len > 4 {
        return fail("a Matroska id is too long");
    }
    let (size, size_len) = vint(&buf[id_len..], false).ok_or_else(|| super::Failed("a Matroska size is malformed".into()))?;
    if unknown_size(size, size_len) {
        return fail("a Matroska element of unknown size");
    }
    let data_at = at + (id_len + size_len) as u64;
    if data_at + size > end {
        return fail("a Matroska element runs past its parent");
    }
    Ok(Element { start: at, id: id as u32, data_at, data_len: size })
}

fn elements<R: Read + Seek>(r: &mut R, from: u64, end: u64) -> Outcome<Vec<Element>> {
    let mut out = Vec::new();
    let mut at = from;
    while at < end {
        let e = header(r, at, end)?;
        at = e.data_at + e.data_len;
        out.push(e);
    }
    Ok(out)
}

/// The element rewritten as a zeroed Void of the same total size.
fn void(e: &Element) -> Vec<Edit> {
    let total = e.data_at + e.data_len - e.start;
    let size_len = (total - 1).min(8);
    let data_len = total - 1 - size_len;
    let mut head = vec![VOID];
    let marker = 1u64 << (7 * size_len);
    let encoded = (marker | data_len).to_be_bytes();
    head.extend_from_slice(&encoded[8 - size_len as usize..]);
    let mut out = vec![Edit::Bytes { offset: e.start, bytes: head }];
    if data_len > 0 {
        out.push(Edit::Zero { offset: e.start + 1 + size_len, len: data_len });
    }
    out
}

pub(super) fn edits<R: Read + Seek>(r: &mut R) -> Outcome<Vec<Edit>> {
    let len = r.seek(SeekFrom::End(0))?;
    let top = elements(r, 0, len)?;
    if top.first().map(|e| e.id) != Some(EBML) {
        return fail("not a Matroska file");
    }
    let mut edits = Vec::new();
    for segment in top.iter().filter(|e| e.id == SEGMENT) {
        for e in elements(r, segment.data_at, segment.data_at + segment.data_len)? {
            match e.id {
                TAGS | ATTACHMENTS => edits.extend(void(&e)),
                INFO | TRACKS => {
                    if e.data_len > HEADER_MAX {
                        return fail("a Matroska header element is too large");
                    }
                    for child in elements(r, e.data_at, e.data_at + e.data_len)? {
                        if e.id == INFO && INFO_CLEARED.contains(&child.id) {
                            edits.extend(void(&child));
                        } else if e.id == TRACKS && child.id == TRACK_ENTRY {
                            for field in elements(r, child.data_at, child.data_at + child.data_len)? {
                                if field.id == TRACK_NAME {
                                    edits.extend(void(&field));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(edits)
}
