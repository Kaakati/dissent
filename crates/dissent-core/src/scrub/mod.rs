//! Removing identifying details from photos and videos before they are sent: where and when they were made, the device
//! and its serial numbers, the owner's and author's names, editing history, embedded previews, and the software that
//! wrote them. What a viewer needs to show the picture or play the video stays: pixels and samples (never re-encoded,
//! except as a last resort below), dimensions, rotation and colour.
//!
//! Two ways, by format:
//! - **Rewritten** while it is copied, dropping the parts that carry details: JPEG, PNG, WebP and GIF.
//! - **Copied, then patched in place** with same-length overwrites, so no offset in the file moves: TIFF and DNG,
//!   HEIC, HEIF and AVIF, MP4, MOV and M4V (and M4A), and Matroska and WebM.
//!
//! A photo whose format is not handled, or whose structure could not be read safely, is decoded and written as a PNG
//! when the image crate can decode it. Anything else is refused with `Failed`, and the app asks whether to send it as
//! it is. Nothing here can remove what is in the picture itself, the camera sensor's noise pattern, or traits of the
//! compression; see the threat model.

mod gif;
mod heif;
mod icc;
mod jpeg;
mod mkv;
mod mp4;
mod png;
mod tiff;
mod webp;

#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Serialize;

/// What kind of file this is, for the defaults of "Remove identifying details".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Video,
    Audio,
    Other,
}

/// Why a file's details could not be removed. The text is for the log; the app names the file itself.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Failed(pub String);

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        Failed(e.to_string())
    }
}

pub(crate) type Outcome<T> = std::result::Result<T, Failed>;

fn fail<T>(why: &str) -> Outcome<T> {
    Err(Failed(why.to_string()))
}

/// A same-length overwrite of a patched format: bytes at `offset`, or `len` zero bytes there.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Bytes { offset: u64, bytes: Vec<u8> },
    Zero { offset: u64, len: u64 },
}

impl Edit {
    fn offset(&self) -> u64 {
        match self {
            Edit::Bytes { offset, .. } | Edit::Zero { offset, .. } => *offset,
        }
    }
}

/// The container of a file, read from its first bytes rather than its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Jpeg,
    Png,
    Webp,
    Gif,
    Tiff,
    Heif,
    Mp4,
    Matroska,
    Unknown,
}

pub(crate) fn sniff(head: &[u8]) -> Format {
    let at = |range: std::ops::Range<usize>| head.get(range).unwrap_or_default();
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Format::Jpeg
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Format::Png
    } else if at(0..4) == b"RIFF" && at(8..12) == b"WEBP" {
        Format::Webp
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Format::Gif
    } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        Format::Tiff
    } else if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Format::Matroska
    } else if at(4..8) == b"ftyp" {
        // HEIF brands name the still-image family; everything else in a ftyp box is the MP4 family.
        let brands = at(8..head.len().min(64));
        let heif = brands
            .chunks(4)
            .any(|b| matches!(b, b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"mif1" | b"msf1" | b"avif" | b"avis" | b"mif2"));
        let video = at(8..12) != b"mif1" && brands.chunks(4).any(|b| matches!(b, b"isom" | b"mp41" | b"mp42" | b"qt  " | b"M4V " | b"M4A "));
        if heif && !video {
            Format::Heif
        } else {
            Format::Mp4
        }
    } else if matches!(at(4..8), b"moov" | b"mdat" | b"wide" | b"free" | b"skip") {
        // An older QuickTime movie, which may begin without a ftyp box.
        Format::Mp4
    } else {
        Format::Unknown
    }
}

/// The kind of a file, from its MIME type.
pub fn kind_of(mime: &str) -> MediaKind {
    if mime.starts_with("image/") {
        MediaKind::Photo
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else if mime.starts_with("audio/") {
        MediaKind::Audio
    } else {
        MediaKind::Other
    }
}

/// What the app needs to decide the per-file default and to warn before a send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub kind: MediaKind,
    /// The file carries details that point to a device or a place: GPS, a camera make or model, a recording device.
    /// Every photo counts, since a photo almost always does; a video counts only when such details were found.
    pub identifying: bool,
    /// Its details can be removed (or, for a photo, it can be re-encoded without them).
    pub removable: bool,
}

fn head_of(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(64);
    File::open(path)?.take(64).read_to_end(&mut head)?;
    Ok(head)
}

/// Looks at a file without changing it.
pub(crate) fn inspect(path: &Path, mime: &str) -> Inspection {
    let kind = kind_of(mime);
    let format = head_of(path).map(|h| sniff(&h)).unwrap_or(Format::Unknown);
    let plan_ok = |format| match format {
        Format::Jpeg | Format::Png | Format::Webp | Format::Gif => rewrite(path, format, &mut Discard).is_ok(),
        Format::Tiff | Format::Heif | Format::Mp4 | Format::Matroska => edits(path, format).is_ok(),
        Format::Unknown => false,
    };
    let removable = plan_ok(format) || (kind == MediaKind::Photo && decodable(path));
    let identifying = match kind {
        MediaKind::Photo => true,
        MediaKind::Video if format == Format::Mp4 => File::open(path)
            .map(BufReader::new)
            .ok()
            .and_then(|mut f| mp4::has_device_details(&mut f).ok())
            .unwrap_or(false),
        _ => false,
    };
    Inspection { kind, identifying, removable }
}

/// What `scrub` wrote.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scrubbed {
    /// The same format, without its details.
    Cleaned,
    /// A photo that could not be cleaned in its own format, decoded and written as a PNG.
    ReencodedPng,
}

/// Writes a copy of `path` without its identifying details to `out`. The original is never changed.
pub(crate) fn scrub(path: &Path, out: &Path) -> Outcome<Scrubbed> {
    let format = sniff(&head_of(path)?);
    let cleaned = match format {
        Format::Jpeg | Format::Png | Format::Webp | Format::Gif => {
            let mut file = BufWriter::new(File::create(out)?);
            rewrite(path, format, &mut file).and_then(|()| file.flush().map_err(Failed::from))
        }
        Format::Tiff | Format::Heif | Format::Mp4 | Format::Matroska => {
            edits(path, format).and_then(|list| {
                std::fs::copy(path, out)?;
                apply(out, &list)
            })
        }
        Format::Unknown => fail("not a format whose details can be removed"),
    };
    match cleaned {
        Ok(()) => Ok(Scrubbed::Cleaned),
        Err(why) => {
            // A picture can still go without its details if it decodes: as a PNG of its pixels alone.
            let img = image::ImageReader::open(path)
                .and_then(|r| r.with_guessed_format())
                .ok()
                .and_then(|r| r.decode().ok());
            let Some(img) = img else { return Err(why) };
            img.save_with_format(out, image::ImageFormat::Png).map_err(|e| Failed(e.to_string()))?;
            Ok(Scrubbed::ReencodedPng)
        }
    }
}

fn decodable(path: &Path) -> bool {
    image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some()
}

/// A writer that keeps nothing, for checking that a rewrite would work. It also seeks, as WebP's rewrite needs.
struct Discard;

impl Write for Discard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Discard {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Ok(0)
    }
}

fn rewrite<W: Write + Seek>(path: &Path, format: Format, out: &mut W) -> Outcome<()> {
    let mut input = BufReader::new(File::open(path)?);
    match format {
        Format::Jpeg => jpeg::rewrite(&mut input, out),
        Format::Png => png::rewrite(&mut input, out),
        Format::Webp => webp::rewrite(&mut input, out),
        Format::Gif => gif::rewrite(&mut input, out),
        _ => fail("not a rewritten format"),
    }
}

fn edits(path: &Path, format: Format) -> Outcome<Vec<Edit>> {
    let mut file = BufReader::new(File::open(path)?);
    match format {
        Format::Tiff => tiff::edits(&mut file),
        Format::Heif => heif::edits(&mut file),
        Format::Mp4 => mp4::edits(&mut file),
        Format::Matroska => mkv::edits(&mut file),
        _ => fail("not a patched format"),
    }
}

/// Applies `edits` to the copy at `path`. Every edit stays inside the file.
fn apply(path: &Path, edits: &[Edit]) -> Outcome<()> {
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    let zeros = vec![0u8; 64 * 1024];
    for edit in edits {
        file.seek(SeekFrom::Start(edit.offset()))?;
        match edit {
            Edit::Bytes { offset, bytes } => {
                if offset + bytes.len() as u64 > len {
                    return fail("an edit runs past the end of the file");
                }
                file.write_all(bytes)?;
            }
            Edit::Zero { offset, len: n } => {
                if offset + n > len {
                    return fail("an edit runs past the end of the file");
                }
                let mut left = *n;
                while left > 0 {
                    let step = left.min(zeros.len() as u64) as usize;
                    file.write_all(&zeros[..step])?;
                    left -= step as u64;
                }
            }
        }
    }
    file.sync_all()?;
    Ok(())
}

// ---- reading helpers shared by the formats ----

fn read_exact_vec<R: Read>(r: &mut R, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}
