//! Bounds-checked little-endian reading and writing for the index.

use crate::error::{Error, Result};

/// Cursor over a byte slice; every read is bounds-checked and returns [`Error::Corrupt`] on
/// truncation, so malformed input can never panic or over-read.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Start reading at the beginning of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Consume exactly `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::corrupt("unexpected end of index data"));
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// Read one byte.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    /// Read a little-endian `u32`.
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// Read a little-endian `u64`.
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// Read a little-endian `i64`.
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// Read a fixed-size array.
    pub fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.array()
    }
}

/// Growable little-endian writer.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    /// Append one byte.
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }

    /// Append a little-endian `u32`.
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    /// Append a little-endian `u64`.
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    /// Append a little-endian `i64`.
    pub fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    /// Append raw bytes.
    pub fn raw(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_truncation() {
        let mut w = Writer::default();
        w.u8(7);
        w.u32(0xDEAD_BEEF);
        w.u64(u64::MAX - 1);
        w.i64(-42);
        w.raw(b"xyz");
        let mut r = Reader::new(&w.0);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.u64().unwrap(), u64::MAX - 1);
        assert_eq!(r.i64().unwrap(), -42);
        assert_eq!(r.bytes::<3>().unwrap(), *b"xyz");
        assert_eq!(r.remaining(), 0);
        assert!(r.u8().is_err());
        assert!(Reader::new(&[1, 2, 3]).u32().is_err());
        assert!(Reader::new(&[]).take(usize::MAX).is_err());
    }
}
