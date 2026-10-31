//! TIFF and DNG: the directories are read and the identifying values zeroed in place, so no offset moves and the image
//! data never changes.
//!
//! - In the image directories (IFD0, the ones chained after it, and DNG's SubIFDs): the make and model, software, dates,
//!   artist, host computer, copyright, description and document name, the XMP, IPTC and Photoshop blocks, and DNG's
//!   camera model names, serial number, original file name and private data.
//! - The Exif, GPS and interoperability directories: every value, MakerNote, serial numbers, owner name, unique id and
//!   every date and position included.
//! - An embedded ICC profile keeps its colour data, and its text is cleared.
//!
//! BigTIFF is not read, and is refused.

use std::io::{Read, Seek, SeekFrom};

use super::{fail, icc, Edit, Outcome};

/// Tags of an image directory whose values are zeroed.
const IDENTIFYING: [u16; 26] = [
    0x010D, // DocumentName
    0x010E, // ImageDescription
    0x010F, // Make
    0x0110, // Model
    0x0131, // Software
    0x0132, // DateTime
    0x013B, // Artist
    0x013C, // HostComputer
    0x02BC, // XMP
    0x8298, // Copyright
    0x83BB, // IPTC
    0x8649, // Photoshop
    0x9003, // DateTimeOriginal (some writers put Exif tags in IFD0)
    0x9004, // DateTimeDigitized
    0x927C, // MakerNote
    0xA420, // ImageUniqueID
    0xA430, // CameraOwnerName
    0xA431, // BodySerialNumber
    0xA435, // LensSerialNumber
    0xC614, // UniqueCameraModel
    0xC615, // LocalizedCameraModel
    0xC62F, // CameraSerialNumber
    0xC634, // DNGPrivateData
    0xC65D, // RawDataUniqueID
    0xC68B, // OriginalRawFileName
    0xC68C, // OriginalRawFileData
];
const SUB_IFDS: u16 = 0x014A;
const EXIF_IFD: u16 = 0x8769;
const GPS_IFD: u16 = 0x8825;
const INTEROP_IFD: u16 = 0xA005;
const ICC: u16 = 0x8773;
/// A file with more directories than this is not read.
const MAX_IFDS: usize = 64;

struct Reader<'a, R> {
    r: &'a mut R,
    big: bool,
    len: u64,
}

impl<R: Read + Seek> Reader<'_, R> {
    fn bytes(&mut self, at: u64, n: usize) -> Outcome<Vec<u8>> {
        if at + n as u64 > self.len {
            return fail("a TIFF offset points past the end of the file");
        }
        self.r.seek(SeekFrom::Start(at))?;
        Ok(super::read_exact_vec(self.r, n)?)
    }
    fn u16(&self, b: &[u8]) -> u16 {
        let b: [u8; 2] = b[..2].try_into().expect("2 bytes");
        if self.big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
    }
    fn u32(&self, b: &[u8]) -> u32 {
        let b: [u8; 4] = b[..4].try_into().expect("4 bytes");
        if self.big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
    }
}

fn type_size(kind: u16) -> Option<u64> {
    Some(match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// One directory entry, with where its value lives.
struct Entry {
    tag: u16,
    /// Where the value's bytes are, and how many.
    value_at: u64,
    value_len: u64,
    /// The first four bytes of the value field, for pointers to other directories.
    raw: [u8; 4],
    count: u32,
}

fn entries<R: Read + Seek>(rd: &mut Reader<R>, ifd: u64) -> Outcome<(Vec<Entry>, u64)> {
    let head = rd.bytes(ifd, 2)?;
    let count = rd.u16(&head) as usize;
    let table = rd.bytes(ifd + 2, count * 12 + 4)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = &table[i * 12..i * 12 + 12];
        let (tag, kind, n) = (rd.u16(&e[0..2]), rd.u16(&e[2..4]), rd.u32(&e[4..8]));
        let Some(size) = type_size(kind) else { continue }; // an unknown type: left as it is
        let value_len = size * n as u64;
        let field = ifd + 2 + (i * 12) as u64 + 8;
        let value_at = if value_len <= 4 { field } else { rd.u32(&e[8..12]) as u64 };
        if value_at + value_len > rd.len {
            return fail("a TIFF value points past the end of the file");
        }
        out.push(Entry { tag, value_at, value_len, raw: e[8..12].try_into().expect("4 bytes"), count: n });
    }
    let next = rd.u32(&table[count * 12..]) as u64;
    Ok((out, next))
}

pub(super) fn edits<R: Read + Seek>(r: &mut R) -> Outcome<Vec<Edit>> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut rd = Reader { r, big: false, len };
    let head = rd.bytes(0, 8)?;
    rd.big = match &head[0..2] {
        b"MM" => true,
        b"II" => false,
        _ => return fail("not a TIFF"),
    };
    if rd.u16(&head[2..4]) != 42 {
        return fail("BigTIFF and other variants are not read");
    }
    let mut edits = Vec::new();
    // (offset, whole): a whole directory has every value zeroed; an image directory only its identifying ones.
    let mut queue: Vec<(u64, bool)> = vec![(rd.u32(&head[4..8]) as u64, false)];
    let mut seen = std::collections::BTreeSet::new();
    while let Some((ifd, whole)) = queue.pop() {
        if ifd == 0 || !seen.insert(ifd) {
            continue;
        }
        if seen.len() > MAX_IFDS {
            return fail("too many TIFF directories");
        }
        let (list, next) = entries(&mut rd, ifd)?;
        if !whole {
            queue.push((next, false));
        }
        for e in list {
            let pointer = |rd: &Reader<R>| rd.u32(&e.raw) as u64;
            match e.tag {
                EXIF_IFD | GPS_IFD | INTEROP_IFD => queue.push((pointer(&rd), true)),
                SUB_IFDS if !whole => {
                    // One offset inline, or a list of them.
                    if e.count == 1 {
                        queue.push((pointer(&rd), false));
                    } else {
                        let list = rd.bytes(e.value_at, (e.count as usize).min(64) * 4)?;
                        for chunk in list.chunks(4) {
                            queue.push((rd.u32(chunk) as u64, false));
                        }
                    }
                }
                ICC if !whole => {
                    let profile = rd.bytes(e.value_at, e.value_len as usize)?;
                    for (start, n) in icc::text_ranges(&profile) {
                        edits.push(Edit::Zero { offset: e.value_at + start as u64, len: n as u64 });
                    }
                }
                tag if (whole || IDENTIFYING.contains(&tag)) && e.value_len > 0 => {
                    edits.push(Edit::Zero { offset: e.value_at, len: e.value_len });
                }
                _ => {}
            }
        }
    }
    Ok(edits)
}
