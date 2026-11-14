//! A bounds-checked cursor over a puppet file.

use super::PuppetError;

/// Longer than this is not a name or a path but a missing terminator.
const MAX_STRING: usize = 1 << 20;

/// Reads little-endian values; every read past the end is [`PuppetError::Truncated`].
#[derive(Clone)]
pub(super) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Where the cursor is, counted from the start of the file.
    pub(super) fn position(&self) -> usize {
        self.at
    }

    /// How many bytes are left.
    pub(super) fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    /// Moves the cursor to `position`, which must not be past the end.
    pub(super) fn seek(&mut self, position: usize) -> Result<(), PuppetError> {
        if position > self.bytes.len() {
            return Err(PuppetError::Truncated);
        }
        self.at = position;
        Ok(())
    }

    /// A reader of the bytes up to `end` (a position in the file), starting where this one is.
    pub(super) fn until(&self, end: usize) -> Result<Reader<'a>, PuppetError> {
        if end < self.at || end > self.bytes.len() {
            return Err(PuppetError::Corrupt("a section ends outside the file"));
        }
        Ok(Reader { bytes: &self.bytes[..end], at: self.at })
    }

    pub(super) fn take(&mut self, length: usize) -> Result<&'a [u8], PuppetError> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(PuppetError::Truncated)?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], PuppetError> {
        let mut array = [0; N];
        array.copy_from_slice(self.take(N)?);
        Ok(array)
    }

    pub(super) fn u8(&mut self) -> Result<u8, PuppetError> {
        Ok(self.array::<1>()?[0])
    }

    pub(super) fn u16(&mut self) -> Result<u16, PuppetError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(super) fn u32(&mut self) -> Result<u32, PuppetError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub(super) fn i32(&mut self) -> Result<i32, PuppetError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub(super) fn f32(&mut self) -> Result<f32, PuppetError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub(super) fn floats<const N: usize>(&mut self) -> Result<[f32; N], PuppetError> {
        let mut floats = [0.0; N];
        for float in &mut floats {
            *float = self.f32()?;
        }
        Ok(floats)
    }

    /// A zero-terminated string; bytes that are not UTF-8 are replaced.
    pub(super) fn string(&mut self) -> Result<String, PuppetError> {
        let rest = &self.bytes[self.at..];
        let length =
            rest.iter().take(MAX_STRING).position(|byte| *byte == 0).ok_or(PuppetError::Truncated)?;
        self.at += length + 1;
        Ok(String::from_utf8_lossy(&rest[..length]).into_owned())
    }

    /// The section header at the cursor (`MDL`, a letter, four digits, a zero byte): the
    /// letter and the version. The attachments' header is `MDAT` instead, and reads as
    /// the letter `T`. The cursor moves past it only when there is one.
    pub(super) fn tag(&mut self) -> Option<(u8, u32)> {
        let tag = self.bytes.get(self.at..self.at + 9)?;
        if (&tag[..3] != b"MDL" && &tag[..4] != b"MDAT")
            || !tag[3].is_ascii_uppercase()
            || tag[8] != 0
            || !tag[4..8].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        let version = tag[4..8].iter().fold(0, |version, digit| version * 10 + u32::from(digit - b'0'));
        self.at += 9;
        Some((tag[3], version))
    }

    /// Whether the file ends in the middle of a section header.
    pub(super) fn cut_tag(&self) -> bool {
        let rest = &self.bytes[self.at..];
        !rest.is_empty() && rest.len() < 9 && rest.iter().zip(b"MDL").all(|(byte, letter)| byte == letter)
    }

    /// Moves to the next section header with `letter` at or after the cursor; `false`
    /// (and the cursor stays) when there is none.
    pub(super) fn find_tag(&mut self, letter: u8) -> bool {
        let mut probe = self.clone();
        while probe.remaining() >= 9 {
            let at = probe.at;
            if probe.bytes[at + 3] == letter && probe.tag().is_some() {
                self.at = at;
                return true;
            }
            probe.at = at + 1;
        }
        false
    }
}
