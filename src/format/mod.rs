//! The Kiln-RS container format (version 1, provisional).
//!
//! ```text
//! +----------+----------------------+-------------------------+--------+
//! | header   | block records ...    | index record            | footer |
//! | 16 bytes | (compressed blocks)  | (file table, compressed)| 24 B   |
//! +----------+----------------------+-------------------------+--------+
//! ```
//!
//! All file contents are concatenated into one logical *stream* which is cut into fixed-size
//! blocks (a power of two, 4 MiB by default). Each block is compressed independently, so blocks
//! can be compressed and decompressed in parallel and any file can be extracted by decoding only
//! the blocks it overlaps. The index at the end lists every entry and every block offset.
//! See `docs/FORMAT.md` for the byte-level specification.

pub mod bytes;
pub mod index;
pub mod record;

use crate::codec::Method;
use crate::error::{Error, Result};
use crate::integrity::crc32;

/// First eight bytes of every archive (PNG-style: detects text-mode mangling).
pub const MAGIC: [u8; 8] = *b"\x89KRS\r\n\x1a\n";
/// Last four bytes of every archive.
pub const FOOTER_MAGIC: [u8; 4] = *b"KRSF";
/// Format version written by this build.
pub const FORMAT_VERSION: u16 = 1;
/// Size of the header in bytes.
pub const HEADER_LEN: usize = 16;
/// Size of the footer in bytes.
pub const FOOTER_LEN: usize = 24;
/// Smallest allowed block size, as a power of two (4 KiB).
pub const MIN_BLOCK_LOG2: u8 = 12;
/// Largest allowed block size, as a power of two (128 MiB).
pub const MAX_BLOCK_LOG2: u8 = 27;
/// Default block size, as a power of two (4 MiB).
pub const DEFAULT_BLOCK_LOG2: u8 = 22;
/// Largest uncompressed index accepted, in bytes.
pub const MAX_INDEX_RAW: u32 = 512 << 20;

/// The fixed-size header at the start of an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The codec the writer was asked to use (individual blocks may still be stored).
    pub default_method: Method,
    /// log2 of the block size.
    pub block_log2: u8,
}

impl Header {
    /// Block size in bytes.
    pub fn block_size(&self) -> u64 {
        1u64 << self.block_log2
    }

    /// Serialise to the 16 on-disk bytes.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[..8].copy_from_slice(&MAGIC);
        out[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        out[10] = self.default_method.id();
        out[11] = self.block_log2;
        // bytes 12..16: flags, reserved, must be zero
        out
    }

    /// Parse and validate the 16 on-disk bytes.
    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self> {
        if bytes[..8] != MAGIC {
            return Err(Error::NotAnArchive("bad magic bytes"));
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != FORMAT_VERSION {
            return Err(Error::Unsupported(format!(
                "format version {version} (this build reads version {FORMAT_VERSION})"
            )));
        }
        let default_method = Method::from_id(bytes[10]).ok_or_else(|| {
            Error::Unsupported(format!("unknown compression method {}", bytes[10]))
        })?;
        let block_log2 = bytes[11];
        if !(MIN_BLOCK_LOG2..=MAX_BLOCK_LOG2).contains(&block_log2) {
            return Err(Error::corrupt(format!(
                "block size 2^{block_log2} is out of range"
            )));
        }
        if bytes[12..16] != [0; 4] {
            return Err(Error::Unsupported(
                "header flags from a newer version".into(),
            ));
        }
        Ok(Self {
            default_method,
            block_log2,
        })
    }
}

/// The fixed-size footer at the very end of an archive; it locates the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footer {
    /// Absolute offset of the index record.
    pub index_offset: u64,
    /// Total length of the index record (header plus payload).
    pub index_record_len: u64,
}

impl Footer {
    /// Serialise to the 24 on-disk bytes.
    pub fn encode(&self) -> [u8; FOOTER_LEN] {
        let mut out = [0u8; FOOTER_LEN];
        out[..8].copy_from_slice(&self.index_offset.to_le_bytes());
        out[8..16].copy_from_slice(&self.index_record_len.to_le_bytes());
        let crc = crc32(&out[..16]);
        out[16..20].copy_from_slice(&crc.to_le_bytes());
        out[20..].copy_from_slice(&FOOTER_MAGIC);
        out
    }

    /// Parse and validate the 24 on-disk bytes.
    pub fn decode(bytes: &[u8; FOOTER_LEN]) -> Result<Self> {
        if bytes[20..] != FOOTER_MAGIC {
            return Err(Error::corrupt(
                "missing footer (the archive is truncated or not finished)",
            ));
        }
        let stored = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        if stored != crc32(&bytes[..16]) {
            return Err(Error::Checksum("footer".into()));
        }
        let le = |range: std::ops::Range<usize>| {
            u64::from_le_bytes(bytes[range].try_into().expect("range is 8 bytes"))
        };
        Ok(Self {
            index_offset: le(0..8),
            index_record_len: le(8..16),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_validation() {
        let h = Header {
            default_method: Method::Zstd,
            block_log2: 22,
        };
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
        assert_eq!(h.block_size(), 4 << 20);

        let mut bad = h.encode();
        bad[0] = 0;
        assert!(matches!(Header::decode(&bad), Err(Error::NotAnArchive(_))));
        let mut bad = h.encode();
        bad[8] = 9;
        assert!(matches!(Header::decode(&bad), Err(Error::Unsupported(_))));
        let mut bad = h.encode();
        bad[10] = 200;
        assert!(matches!(Header::decode(&bad), Err(Error::Unsupported(_))));
        for log2 in [0, MIN_BLOCK_LOG2 - 1, MAX_BLOCK_LOG2 + 1, 255] {
            let mut bad = h.encode();
            bad[11] = log2;
            assert!(
                matches!(Header::decode(&bad), Err(Error::Corrupt(_))),
                "{log2}"
            );
        }
        let mut bad = h.encode();
        bad[13] = 1;
        assert!(matches!(Header::decode(&bad), Err(Error::Unsupported(_))));
    }

    #[test]
    fn footer_round_trip_and_validation() {
        let f = Footer {
            index_offset: 123_456,
            index_record_len: 789,
        };
        assert_eq!(Footer::decode(&f.encode()).unwrap(), f);
        let mut bad = f.encode();
        bad[3] ^= 1;
        assert!(matches!(Footer::decode(&bad), Err(Error::Checksum(_))));
        let mut bad = f.encode();
        bad[23] = b'X';
        assert!(matches!(Footer::decode(&bad), Err(Error::Corrupt(_))));
    }
}
