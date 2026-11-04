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

    /// Read an unsigned LEB128 varint. Rejects encodings longer than 10 bytes, values above
    /// `u64::MAX`, and non-canonical encodings (a trailing zero group), so every value has
    /// exactly one encoding.
    pub fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for i in 0..10 {
            let byte = self.u8()?;
            let low = u64::from(byte & 0x7F);
            if i == 9 && low > 1 {
                return Err(Error::corrupt("varint overflows 64 bits"));
            }
            value |= low << (7 * i);
            if byte & 0x80 == 0 {
                if i > 0 && byte == 0 {
                    return Err(Error::corrupt("non-canonical varint"));
                }
                return Ok(value);
            }
        }
        Err(Error::corrupt("varint is too long"))
    }

    /// Read a zigzag-encoded signed varint.
    pub fn svarint(&mut self) -> Result<i64> {
        let v = self.varint()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
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

    /// Append an unsigned LEB128 varint.
    pub fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.0.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
    }

    /// Append a zigzag-encoded signed varint.
    pub fn svarint(&mut self, v: i64) {
        self.varint(((v << 1) ^ (v >> 63)) as u64);
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

    #[test]
    fn varints_round_trip_and_reject_malformed_input() {
        for v in [
            0u64,
            1,
            127,
            128,
            300,
            16_383,
            16_384,
            u32::MAX as u64,
            u64::MAX,
        ] {
            let mut w = Writer::default();
            w.varint(v);
            let mut r = Reader::new(&w.0);
            assert_eq!(r.varint().unwrap(), v);
            assert_eq!(r.remaining(), 0);
        }
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -123_456_789] {
            let mut w = Writer::default();
            w.svarint(v);
            assert_eq!(Reader::new(&w.0).svarint().unwrap(), v);
        }
        assert!(Reader::new(&[0x80]).varint().is_err(), "truncated");
        assert!(
            Reader::new(&[0x80, 0x00]).varint().is_err(),
            "non-canonical"
        );
        assert!(Reader::new(&[0xFF; 11]).varint().is_err(), "too long");
        let mut overflow = [0xFFu8; 9].to_vec();
        overflow.push(0x02);
        assert!(Reader::new(&overflow).varint().is_err(), "overflow");
    }
}
