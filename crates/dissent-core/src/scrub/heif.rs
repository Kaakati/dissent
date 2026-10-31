//! HEIC, HEIF and AVIF: the still-image files phones write. Their Exif and XMP are items of the `meta` box, located by
//! `iinf` (what each item is) and `iloc` (where its bytes are). Those bytes are zeroed in place, so no offset moves; the
//! picture items are not touched. A top-level XMP `uuid` box becomes a zeroed `free` box. Rotation lives in item
//! properties (`irot`, `imir`), not in Exif, so it stays.

use std::io::{Read, Seek, SeekFrom};

use super::{be16, be32, be64, fail, mp4, Edit, Outcome};

const META_MAX: u64 = 16 * 1024 * 1024;

pub(super) fn edits<R: Read + Seek>(r: &mut R) -> Outcome<Vec<Edit>> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut edits = Vec::new();
    let mut meta = None;
    for b in mp4::top_level(r, len)? {
        match &b.kind {
            b"meta" => meta = Some(b),
            b"uuid" if mp4::is_xmp_uuid(r, &b)? => edits.extend(mp4::free_whole(&b)),
            _ => {}
        }
    }
    let Some(meta) = meta else { return fail("a HEIF file without a meta box") };
    if meta.size - meta.header > META_MAX {
        return fail("the HEIF meta box is too large");
    }
    r.seek(SeekFrom::Start(meta.offset + meta.header))?;
    let payload = super::read_exact_vec(r, (meta.size - meta.header) as usize)?;
    let payload_at = meta.offset + meta.header;
    // `meta` is a full box: version and flags, then its children.
    let children = mp4::children(payload.get(4..).unwrap_or_default());
    let find = |kind: &[u8; 4]| children.iter().find(|c| &c.kind == kind);
    let (Some(iinf), Some(iloc)) = (find(b"iinf"), find(b"iloc")) else {
        return fail("a HEIF meta box without iinf or iloc");
    };
    let targets = metadata_items(&payload[4 + iinf.payload.clone().start..4 + iinf.payload.clone().end])?;
    if targets.is_empty() {
        return Ok(edits);
    }
    let idat_at = find(b"idat").map(|c| payload_at + 4 + c.payload.start as u64);
    let iloc_bytes = &payload[4 + iloc.payload.start..4 + iloc.payload.end];
    for (offset, n) in extents(iloc_bytes, &targets, idat_at)? {
        if offset + n > len {
            return fail("a HEIF item points past the end of the file");
        }
        edits.push(Edit::Zero { offset, len: n });
    }
    Ok(edits)
}

/// The ids of the Exif and XMP items listed in an `iinf` payload.
fn metadata_items(iinf: &[u8]) -> Outcome<Vec<u32>> {
    let version = *iinf.first().ok_or_else(|| super::Failed("an empty iinf".into()))?;
    let start = if version == 0 { 6 } else { 8 };
    let mut ids = Vec::new();
    for infe in mp4::children(iinf.get(start..).unwrap_or_default()) {
        if &infe.kind != b"infe" {
            continue;
        }
        let p = &iinf[start + infe.payload.start..start + infe.payload.end];
        let v = *p.first().unwrap_or(&0);
        if v < 2 {
            continue; // version 0 and 1 entries have no item type
        }
        let (id, rest) = if v == 2 {
            (be16(p, 4).map(u32::from), 6)
        } else {
            (be32(p, 4), 8)
        };
        let Some(id) = id else { continue };
        let kind = p.get(rest + 2..rest + 6).unwrap_or_default();
        let after_name = p.get(rest + 6..).unwrap_or_default();
        let content_type = after_name
            .iter()
            .position(|&b| b == 0)
            .map(|nul| &after_name[nul + 1..])
            .unwrap_or_default();
        let xmp = kind == b"mime" && content_type.windows(3).any(|w| w.eq_ignore_ascii_case(b"xml") || w.eq_ignore_ascii_case(b"xmp"));
        if kind == b"Exif" || xmp {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn sized(b: &[u8], at: &mut usize, size: u8) -> Outcome<u64> {
    let v = match size {
        0 => Some(0),
        4 => be32(b, *at).map(u64::from),
        8 => be64(b, *at),
        _ => None,
    };
    *at += size as usize;
    v.ok_or_else(|| super::Failed("a HEIF iloc field is cut short".into()))
}

/// The file ranges, as `(offset, len)`, of the items in `targets`, from an `iloc` payload.
fn extents(iloc: &[u8], targets: &[u32], idat_at: Option<u64>) -> Outcome<Vec<(u64, u64)>> {
    let version = *iloc.first().ok_or_else(|| super::Failed("an empty iloc".into()))?;
    let sizes = *iloc.get(4).unwrap_or(&0);
    let more = *iloc.get(5).unwrap_or(&0);
    let (offset_size, length_size, base_size) = (sizes >> 4, sizes & 0x0F, more >> 4);
    let index_size = if version >= 1 { more & 0x0F } else { 0 };
    let mut at = 6;
    let count = if version < 2 {
        let n = be16(iloc, at).map(u32::from);
        at += 2;
        n
    } else {
        let n = be32(iloc, at);
        at += 4;
        n
    }
    .ok_or_else(|| super::Failed("a HEIF iloc is cut short".into()))?;
    let mut out = Vec::new();
    for _ in 0..count.min(65_536) {
        let id = if version < 2 {
            let v = be16(iloc, at).map(u32::from);
            at += 2;
            v
        } else {
            let v = be32(iloc, at);
            at += 4;
            v
        }
        .ok_or_else(|| super::Failed("a HEIF iloc entry is cut short".into()))?;
        let method = if version >= 1 {
            let m = be16(iloc, at).ok_or_else(|| super::Failed("a HEIF iloc entry is cut short".into()))? & 0x0F;
            at += 2;
            m
        } else {
            0
        };
        at += 2; // data reference index
        let base = sized(iloc, &mut at, base_size)?;
        let extent_count = be16(iloc, at).ok_or_else(|| super::Failed("a HEIF iloc entry is cut short".into()))?;
        at += 2;
        for _ in 0..extent_count {
            if index_size > 0 {
                sized(iloc, &mut at, index_size)?;
            }
            let offset = sized(iloc, &mut at, offset_size)?;
            let length = sized(iloc, &mut at, length_size)?;
            if !targets.contains(&id) {
                continue;
            }
            if length == 0 {
                return fail("a HEIF metadata item runs to the end of the file");
            }
            let start = match method {
                0 => base + offset,
                1 => idat_at.ok_or_else(|| super::Failed("a HEIF item in a missing idat".into()))? + base + offset,
                _ => return fail("a HEIF metadata item stored by reference"),
            };
            out.push((start, length));
        }
    }
    Ok(out)
}
