//! The `.kiln` container format, version 2.
//!
//! ```text
//! +----------+----------------------+-------------------------+--------+
//! | header   | chunk records ...    | index record            | footer |
//! | 24 bytes | (compressed chunks)  | (entries + chunk table) | 24 B   |
//! +----------+----------------------+-------------------------+--------+
//! ```
//!
//! File contents are concatenated, in index order, into one logical *stream* which is cut into
//! independently compressed **chunks** of at most `2^block_log2` bytes (4 MiB by default). Chunks
//! may be shorter than the maximum: writers cut at content-class boundaries so that already
//! compressed data never shares a chunk with compressible data. The index lists every entry and
//! every chunk's `(raw_len, comp_len, method)`; chunk file offsets are *implied* by the lengths
//! of the records before them, so they cannot disagree with the data. See `docs/FORMAT.md` for
//! the byte-level specification.

pub mod bytes;
pub mod index;
pub mod record;

use crate::error::{Error, Result};
use crate::integrity::crc32;
use crate::profile::Profile;

/// First eight bytes of every archive. PNG-style: the high-bit byte, the format name, a DOS line
/// ending and the DOS end-of-file byte make text-mode transfers and accidental `cat` visible.
pub const MAGIC: [u8; 8] = *b"\x89KILN\r\n\x1a";
/// Last four bytes of every archive.
pub const FOOTER_MAGIC: [u8; 4] = *b"KLNF";
/// Format version written by this build. Readers reject any other value.
pub const FORMAT_VERSION: u16 = 2;
/// Size of the header in bytes.
pub const HEADER_LEN: usize = 24;
/// Size of the footer in bytes.
pub const FOOTER_LEN: usize = 24;
/// Smallest allowed chunk size, as a power of two (4 KiB).
pub const MIN_BLOCK_LOG2: u8 = 12;
/// Largest allowed chunk size, as a power of two (128 MiB).
pub const MAX_BLOCK_LOG2: u8 = 27;
/// Default chunk size, as a power of two (4 MiB).
pub const DEFAULT_BLOCK_LOG2: u8 = 22;
/// Largest uncompressed index accepted, in bytes.
pub const MAX_INDEX_RAW: u32 = 256 << 20;
/// Most entries (files plus directories) an archive may hold. Bounds the memory a hostile index
/// can make a reader allocate (roughly 130 bytes per entry in memory).
pub const MAX_ENTRIES: u64 = 8_000_000;

/// Incompatible-feature bits understood by this build (none yet). A reader that meets a set bit
/// it does not know must refuse the archive.
pub const KNOWN_INCOMPAT_FLAGS: u32 = 0;

/// The fixed-size header at the start of an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The profile the writer was asked to use (informational; every chunk names its own method).
    pub profile: Profile,
    /// log2 of the maximum chunk size.
    pub block_log2: u8,
    /// Features a reader must understand to read the archive correctly (unknown bit = refuse).
    pub incompat_flags: u32,
    /// Features a reader may ignore (unknown bit = ignore).
    pub compat_flags: u32,
}

impl Header {
    /// A header for a new archive.
    pub fn new(profile: Profile, block_log2: u8) -> Self {
        Self {
            profile,
            block_log2,
            incompat_flags: 0,
            compat_flags: 0,
        }
    }

    /// Maximum chunk size in bytes.
    pub fn block_size(&self) -> u64 {
        1u64 << self.block_log2
    }

    /// Serialise to the 24 on-disk bytes (the last four are a CRC-32 of the first twenty).
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[..8].copy_from_slice(&MAGIC);
        out[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        out[10] = self.block_log2;
        out[11] = self.profile.id();
        out[12..16].copy_from_slice(&self.incompat_flags.to_le_bytes());
        out[16..20].copy_from_slice(&self.compat_flags.to_le_bytes());
        let crc = crc32(&out[..20]);
        out[20..24].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse and validate the 24 on-disk bytes.
    ///
    /// Check order matters for good errors: magic first (is this a Kiln file at all?), then the
    /// version (so a newer layout is reported as "unsupported", not "corrupt"), then the CRC.
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
        let stored = u32::from_le_bytes(bytes[20..24].try_into().expect("4 bytes"));
        if stored != crc32(&bytes[..20]) {
            return Err(Error::Checksum("header".into()));
        }
        let block_log2 = bytes[10];
        if !(MIN_BLOCK_LOG2..=MAX_BLOCK_LOG2).contains(&block_log2) {
            return Err(Error::corrupt(format!(
                "chunk size 2^{block_log2} is out of range"
            )));
        }
        let profile = Profile::from_id(bytes[11])
            .ok_or_else(|| Error::Unsupported(format!("unknown profile id {}", bytes[11])))?;
        let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
        let incompat_flags = word(12);
        if incompat_flags & !KNOWN_INCOMPAT_FLAGS != 0 {
            return Err(Error::Unsupported(format!(
                "archive requires features this build lacks (flags {:#x})",
                incompat_flags & !KNOWN_INCOMPAT_FLAGS
            )));
        }
        Ok(Self {
            profile,
            block_log2,
            incompat_flags,
            compat_flags: word(16),
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
                "missing footer (the archive is truncated or was never finished)",
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
        let h = Header::new(Profile::Balanced, 22);
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
        assert_eq!(h.block_size(), 4 << 20);

        let mut bad = h.encode();
        bad[0] = 0;
        assert!(matches!(Header::decode(&bad), Err(Error::NotAnArchive(_))));
        // A different version is "unsupported" even though the CRC no longer matches.
        let mut bad = h.encode();
        bad[8] = 9;
        assert!(matches!(Header::decode(&bad), Err(Error::Unsupported(_))));
        // Any other single-byte change breaks the CRC (or a range check behind it).
        for i in 10..HEADER_LEN {
            let mut bad = h.encode();
            bad[i] ^= 0x10;
            assert!(Header::decode(&bad).is_err(), "byte {i}");
        }
    }

    #[test]
    fn unknown_incompatible_flags_are_refused_but_compat_flags_are_ignored() {
        let seal = |mut b: [u8; HEADER_LEN]| {
            let crc = crc32(&b[..20]);
            b[20..].copy_from_slice(&crc.to_le_bytes());
            b
        };
        let mut raw = Header::new(Profile::Fastest, 20).encode();
        raw[12] = 1; // incompat bit 0
        assert!(matches!(
            Header::decode(&seal(raw)),
            Err(Error::Unsupported(_))
        ));
        let mut raw = Header::new(Profile::Fastest, 20).encode();
        raw[16] = 0xFF; // compat bits: ignorable
        let h = Header::decode(&seal(raw)).unwrap();
        assert_eq!(h.compat_flags, 0xFF);
        let mut raw = Header::new(Profile::Fastest, 20).encode();
        raw[10] = 3; // chunk size out of range
        assert!(matches!(Header::decode(&seal(raw)), Err(Error::Corrupt(_))));
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
