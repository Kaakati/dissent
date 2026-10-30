//! GIF: block by block. Frames, colour tables and graphic control blocks are copied; comment extensions go, and so do
//! application extensions (XMP among them) except the two that make an animation loop, `NETSCAPE2.0` and `ANIMEXTS1.0`.

use std::io::{Read, Write};

use super::{fail, Outcome};

fn byte<R: Read>(r: &mut R) -> std::io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

/// Copies (or, with `out` None, skips) a run of data sub-blocks up to and including its zero-length terminator.
fn sub_blocks<R: Read, W: Write>(input: &mut R, mut out: Option<&mut W>) -> Outcome<()> {
    loop {
        let len = byte(input)?;
        let data = super::read_exact_vec(input, len as usize)?;
        if let Some(w) = out.as_deref_mut() {
            w.write_all(&[len])?;
            w.write_all(&data)?;
        }
        if len == 0 {
            return Ok(());
        }
    }
}

fn colour_table<R: Read, W: Write>(input: &mut R, out: &mut W, packed: u8) -> Outcome<()> {
    if packed & 0x80 != 0 {
        let size = 3 * (1usize << ((packed & 0x07) + 1));
        out.write_all(&super::read_exact_vec(input, size)?)?;
    }
    Ok(())
}

pub(super) fn rewrite<R: Read, W: Write>(input: &mut R, out: &mut W) -> Outcome<()> {
    let header = super::read_exact_vec(input, 13)?; // signature, version and the logical screen descriptor
    if !header.starts_with(b"GIF8") {
        return fail("not a GIF");
    }
    out.write_all(&header)?;
    colour_table(input, out, header[10])?;
    loop {
        match byte(input)? {
            0x21 => {
                let label = byte(input)?;
                match label {
                    0xF9 | 0x01 => {
                        out.write_all(&[0x21, label])?;
                        sub_blocks(input, Some(out))?;
                    }
                    0xFF => {
                        let len = byte(input)?;
                        let id = super::read_exact_vec(input, len as usize)?;
                        if id == b"NETSCAPE2.0" || id == b"ANIMEXTS1.0" {
                            out.write_all(&[0x21, 0xFF, len])?;
                            out.write_all(&id)?;
                            sub_blocks(input, Some(out))?;
                        } else {
                            sub_blocks::<R, W>(input, None)?;
                        }
                    }
                    _ => sub_blocks::<R, W>(input, None)?, // comments and unknown extensions
                }
            }
            0x2C => {
                let descriptor = super::read_exact_vec(input, 9)?;
                out.write_all(&[0x2C])?;
                out.write_all(&descriptor)?;
                colour_table(input, out, descriptor[8])?;
                out.write_all(&[byte(input)?])?; // LZW minimum code size
                sub_blocks(input, Some(out))?;
            }
            0x3B => {
                out.write_all(&[0x3B])?;
                return Ok(());
            }
            _ => return fail("an unknown GIF block"),
        }
    }
}
