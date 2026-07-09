//! Minimal little-endian read/write helpers used by the manual footer/preamble codecs.
//!
//! These are crate-internal. `Writer` appends to a `Vec<u8>`; `Reader` walks a byte
//! slice with bounds checks that surface as `FormatError::Truncated`.

use crate::error::{FormatError, Result};

/// Append-only little-endian writer over an owned buffer.
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    /// Length-prefixed (u32) byte blob.
    pub fn lp_bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.bytes(b);
    }
    /// Length-prefixed (u32) UTF-8 string.
    pub fn lp_str(&mut self, s: &str) {
        self.lp_bytes(s.as_bytes());
    }
}

/// Cursor over a byte slice with checked reads.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(FormatError::Truncated("length overflow"))?;
        if end > self.buf.len() {
            return Err(FormatError::Truncated("unexpected end of buffer"));
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    pub fn lp_bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    pub fn lp_str(&mut self) -> Result<String> {
        let b = self.lp_bytes()?;
        String::from_utf8(b).map_err(|e| FormatError::Decode(format!("invalid utf-8: {e}")))
    }
}
