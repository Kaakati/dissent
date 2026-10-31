//! PNG: chunk by chunk. The critical chunks and the ancillary chunks a viewer needs to show the picture (transparency,
//! gamma, colour, density, animation) are copied; every other chunk goes, text (`tEXt`, `iTXt`, `zTXt`), `eXIf`, `tIME`
//! and unknown private chunks (C2PA and the like) included. A compressed colour profile (`iCCP`) is kept as it is.

use std::io::{Read, Write};

use super::{fail, Outcome};

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Ancillary chunks a viewer may need, kept. Everything else ancillary is dropped.
const KEPT: [&[u8; 4]; 16] = [
    b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"bKGD", b"pHYs", b"hIST", b"sPLT", b"acTL", b"fcTL", b"fdAT",
    b"cICP", b"mDCv", b"cLLi",
];

pub(super) fn rewrite<R: Read, W: Write>(input: &mut R, out: &mut W) -> Outcome<()> {
    let mut sig = [0u8; 8];
    input.read_exact(&mut sig)?;
    if sig != SIGNATURE {
        return fail("not a PNG");
    }
    out.write_all(SIGNATURE)?;
    loop {
        let mut head = [0u8; 8];
        input.read_exact(&mut head)?;
        let len = u32::from_be_bytes(head[0..4].try_into().expect("4 bytes")) as u64;
        if len > 0x7FFF_FFFF {
            return fail("a PNG chunk is too long");
        }
        let kind: [u8; 4] = head[4..8].try_into().expect("4 bytes");
        let critical = kind[0].is_ascii_uppercase();
        let mut body = input.take(len + 4); // data and CRC
        if critical || KEPT.iter().any(|k| **k == kind) {
            out.write_all(&head)?;
            let copied = std::io::copy(&mut body, out)?;
            if copied != len + 4 {
                return fail("a PNG chunk is cut short");
            }
        } else {
            let skipped = std::io::copy(&mut body, &mut std::io::sink())?;
            if skipped != len + 4 {
                return fail("a PNG chunk is cut short");
            }
        }
        if &kind == b"IEND" {
            return Ok(());
        }
    }
}
