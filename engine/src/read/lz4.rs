//! LZ4 block decompression, which is all texture containers use.

use std::fmt;

/// The block is corrupt: it reads past its input, writes past its promised size, or
/// copies from before the start of its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Corrupt;

impl fmt::Display for Corrupt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("corrupt LZ4 block")
    }
}

impl std::error::Error for Corrupt {}

/// Decompresses one LZ4 block that unpacks to exactly `size` bytes.
pub fn decompress(block: &[u8], size: usize) -> Result<Vec<u8>, Corrupt> {
    // A block cannot unpack to more than 255 times its length: room is set aside
    // for what it can hold, not for what a header claims.
    let mut out: Vec<u8> = Vec::with_capacity(size.min(block.len().saturating_mul(255)));
    let mut at = 0;
    // Reads a length that continues in following bytes while they are 255.
    let extend = |at: &mut usize, mut length: usize| -> Result<usize, Corrupt> {
        if length == 15 {
            loop {
                let byte = *block.get(*at).ok_or(Corrupt)?;
                *at += 1;
                length += usize::from(byte);
                if byte != 255 {
                    break;
                }
            }
        }
        Ok(length)
    };
    while at < block.len() {
        let token = block[at];
        at += 1;
        let literals = extend(&mut at, usize::from(token >> 4))?;
        let run = block.get(at..at + literals).ok_or(Corrupt)?;
        if out.len() + literals > size {
            return Err(Corrupt);
        }
        out.extend_from_slice(run);
        at += literals;
        if at >= block.len() {
            break; // The last sequence is literals only.
        }
        let offset = usize::from(u16::from_le_bytes([block[at], *block.get(at + 1).ok_or(Corrupt)?]));
        at += 2;
        let length = extend(&mut at, usize::from(token & 15))? + 4;
        if offset == 0 || offset > out.len() || out.len() + length > size {
            return Err(Corrupt);
        }
        let start = out.len() - offset;
        if offset >= length {
            out.extend_from_within(start..start + length);
        } else {
            // The match overlaps what it writes: it repeats its own output.
            for i in 0..length {
                out.push(out[start + i]);
            }
        }
    }
    if out.len() == size { Ok(out) } else { Err(Corrupt) }
}
