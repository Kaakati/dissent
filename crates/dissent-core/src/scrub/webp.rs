//! WebP: the picture, alpha, animation and colour-profile chunks are copied (the profile with its text cleared); `EXIF`,
//! `XMP ` and any unknown chunk go, their flags in `VP8X` are cleared, and the RIFF size is written again at the end.

use std::io::{Read, Seek, SeekFrom, Write};

use super::{fail, icc, Outcome};

const KEPT: [&[u8; 4]; 7] = [b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP"];

pub(super) fn rewrite<R: Read, W: Write + Seek>(input: &mut R, out: &mut W) -> Outcome<()> {
    let mut head = [0u8; 12];
    input.read_exact(&mut head)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WEBP" {
        return fail("not a WebP");
    }
    let riff_len = u32::from_le_bytes(head[4..8].try_into().expect("4 bytes")) as u64;
    out.write_all(&head)?;
    let mut written = 4u64; // "WEBP"
    let mut read = 4u64;
    while read < riff_len {
        let mut chunk = [0u8; 8];
        input.read_exact(&mut chunk)?;
        let len = u32::from_le_bytes(chunk[4..8].try_into().expect("4 bytes")) as u64;
        let padded = len + (len & 1);
        read += 8 + padded;
        let kind: [u8; 4] = chunk[0..4].try_into().expect("4 bytes");
        if !KEPT.iter().any(|k| **k == kind) {
            let skipped = std::io::copy(&mut input.take(padded), &mut std::io::sink())?;
            if skipped != padded {
                return fail("a WebP chunk is cut short");
            }
            continue;
        }
        if &kind == b"VP8X" || &kind == b"ICCP" {
            if len > 16 * 1024 * 1024 {
                return fail("a WebP header chunk is too long");
            }
            let mut data = super::read_exact_vec(input, padded as usize)?;
            if &kind == b"VP8X" {
                if let Some(flags) = data.first_mut() {
                    *flags &= !0x0C; // 0x08 is Exif, 0x04 is XMP
                }
            } else {
                icc::scrub(&mut data[..len as usize]);
            }
            out.write_all(&chunk)?;
            out.write_all(&data)?;
        } else {
            out.write_all(&chunk)?;
            let copied = std::io::copy(&mut input.take(padded), out)?;
            if copied != padded {
                return fail("a WebP chunk is cut short");
            }
        }
        written += 8 + padded;
    }
    let written = u32::try_from(written).map_err(|_| super::Failed("the WebP is too large".into()))?;
    out.seek(SeekFrom::Start(4))?;
    out.write_all(&written.to_le_bytes())?;
    out.seek(SeekFrom::End(0))?;
    Ok(())
}
