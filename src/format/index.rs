//! The index: every entry's metadata plus the offset of every data block.
//!
//! Layout (little-endian):
//!
//! ```text
//! entry_count u32
//! entry x entry_count:
//!     kind u8 (0 = file, 1 = directory) | path_len u32 | path (UTF-8)
//!     size u64 | mtime_secs i64 | mtime_nanos u32 | mode u32 | blake3 [u8; 32]
//! total_raw u64          total bytes in the data stream (sum of all file sizes)
//! block_count u32
//! block_offset u64 x block_count     absolute archive offset of each block record
//! ```
//!
//! File contents are laid out in the stream in entry order; a file's start offset is the sum of
//! the sizes of the files before it, so it is not stored.

use std::str;

use super::HEADER_LEN;
use super::bytes::{Reader, Writer};
use crate::error::{Error, Result};
use crate::integrity::{FileHash, HASH_LEN};
use crate::path::{MAX_PATH_LEN, validate_archive_path};

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file whose bytes live in the data stream.
    File,
    /// A directory (kept so empty directories survive a round trip).
    Dir,
}

impl EntryKind {
    fn id(self) -> u8 {
        match self {
            Self::File => 0,
            Self::Dir => 1,
        }
    }

    fn from_id(id: u8) -> Result<Self> {
        match id {
            0 => Ok(Self::File),
            1 => Ok(Self::Dir),
            other => Err(Error::Unsupported(format!("unknown entry kind {other}"))),
        }
    }
}

/// One archived file or directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative, `/`-separated name.
    pub path: String,
    /// File or directory.
    pub kind: EntryKind,
    /// Size in bytes (zero for directories).
    pub size: u64,
    /// Modification time, whole seconds since the Unix epoch.
    pub mtime_secs: i64,
    /// Modification time, nanosecond remainder (`0..1_000_000_000`).
    pub mtime_nanos: u32,
    /// Unix permission bits (`0o7777` mask).
    pub mode: u32,
    /// BLAKE3 of the file contents (all zero for directories).
    pub hash: FileHash,
}

/// The decoded index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// Entries in stream order.
    pub entries: Vec<Entry>,
    /// Total bytes in the data stream.
    pub total_raw: u64,
    /// Absolute archive offset of each block record.
    pub block_offsets: Vec<u64>,
}

/// Smallest possible encoded entry: fixed fields plus a one-byte name.
const MIN_ENTRY_LEN: usize = 1 + 4 + 1 + 8 + 8 + 4 + 4 + HASH_LEN;

impl Index {
    /// Serialise the index.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.entries.len() as u32);
        for e in &self.entries {
            w.u8(e.kind.id());
            w.u32(e.path.len() as u32);
            w.raw(e.path.as_bytes());
            w.u64(e.size);
            w.i64(e.mtime_secs);
            w.u32(e.mtime_nanos);
            w.u32(e.mode);
            w.raw(&e.hash);
        }
        w.u64(self.total_raw);
        w.u32(self.block_offsets.len() as u32);
        for &off in &self.block_offsets {
            w.u64(off);
        }
        w.0
    }

    /// Parse and fully validate an index.
    ///
    /// `block_size` is the archive's block size and `data_end` the offset where block records
    /// must stop (the start of the index record). Every count is checked against the bytes that
    /// are actually present before anything is allocated, and every cross-reference is verified,
    /// so a hostile index cannot cause large allocations, overflow, or out-of-range reads later.
    pub fn decode(raw: &[u8], block_size: u64, data_end: u64) -> Result<Self> {
        let mut r = Reader::new(raw);
        let count = r.u32()? as usize;
        if count > r.remaining() / MIN_ENTRY_LEN {
            return Err(Error::corrupt("index entry count exceeds the index size"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut sum: u64 = 0;
        for _ in 0..count {
            let kind = EntryKind::from_id(r.u8()?)?;
            let path_len = r.u32()? as usize;
            if path_len > MAX_PATH_LEN {
                return Err(Error::corrupt("entry name is too long"));
            }
            let path = str::from_utf8(r.take(path_len)?)
                .map_err(|_| Error::corrupt("entry name is not valid UTF-8"))?
                .to_owned();
            validate_archive_path(&path)?;
            let size = r.u64()?;
            let mtime_secs = r.i64()?;
            let mtime_nanos = r.u32()?;
            let mode = r.u32()?;
            let hash = r.bytes::<HASH_LEN>()?;
            if mtime_nanos >= 1_000_000_000 {
                return Err(Error::corrupt(format!("{path}: invalid timestamp")));
            }
            if kind == EntryKind::Dir && size != 0 {
                return Err(Error::corrupt(format!("{path}: directory with a size")));
            }
            sum = sum
                .checked_add(size)
                .ok_or_else(|| Error::corrupt("total size overflows"))?;
            entries.push(Entry {
                path,
                kind,
                size,
                mtime_secs,
                mtime_nanos,
                mode,
                hash,
            });
        }

        let total_raw = r.u64()?;
        if total_raw != sum {
            return Err(Error::corrupt(
                "entry sizes do not add up to the stream length",
            ));
        }
        let block_count = r.u32()? as usize;
        if block_count > r.remaining() / 8 {
            return Err(Error::corrupt("block count exceeds the index size"));
        }
        if block_count as u64 != total_raw.div_ceil(block_size) {
            return Err(Error::corrupt(
                "block count does not match the stream length",
            ));
        }
        let mut block_offsets = Vec::with_capacity(block_count);
        let mut previous = HEADER_LEN as u64 - 1;
        for _ in 0..block_count {
            let off = r.u64()?;
            if off <= previous || off >= data_end {
                return Err(Error::corrupt(
                    "block offsets are out of order or out of range",
                ));
            }
            previous = off;
            block_offsets.push(off);
        }
        if r.remaining() != 0 {
            return Err(Error::corrupt("unexpected data after the index"));
        }

        let mut names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        names.sort_unstable();
        if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
            return Err(Error::corrupt(format!("duplicate entry {:?}", w[0])));
        }
        Ok(Self {
            entries,
            total_raw,
            block_offsets,
        })
    }

    /// Start offset in the data stream of every entry (directories get the running offset).
    pub fn stream_offsets(&self) -> Vec<u64> {
        let mut offset = 0u64;
        self.entries
            .iter()
            .map(|e| {
                let start = offset;
                offset += e.size;
                start
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::empty_hash;

    const BS: u64 = 4096;

    fn file(path: &str, size: u64) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::File,
            size,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 5,
            mode: 0o644,
            hash: [9; HASH_LEN],
        }
    }

    fn dir(path: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::Dir,
            size: 0,
            mtime_secs: -5,
            mtime_nanos: 0,
            mode: 0o755,
            hash: [0; HASH_LEN],
        }
    }

    fn sample() -> Index {
        // 5000 + 3000 = 8000 bytes => 2 blocks of 4096.
        Index {
            entries: vec![
                dir("d"),
                file("d/a.txt", 5000),
                file("empty", 0),
                file("z", 3000),
            ],
            total_raw: 8000,
            block_offsets: vec![16, 700],
        }
    }

    #[test]
    fn round_trip() {
        let idx = sample();
        let back = Index::decode(&idx.encode(), BS, 2000).unwrap();
        assert_eq!(back, idx);
        assert_eq!(back.stream_offsets(), vec![0, 0, 5000, 5000]);
    }

    #[test]
    fn empty_index_round_trips() {
        let idx = Index::default();
        assert_eq!(Index::decode(&idx.encode(), BS, 100).unwrap(), idx);
        let only_empty = Index {
            entries: vec![Entry {
                hash: empty_hash(),
                ..file("e", 0)
            }],
            total_raw: 0,
            block_offsets: vec![],
        };
        assert_eq!(
            Index::decode(&only_empty.encode(), BS, 100).unwrap(),
            only_empty
        );
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let bytes = sample().encode();
        for cut in 0..bytes.len() {
            assert!(
                Index::decode(&bytes[..cut], BS, 2000).is_err(),
                "prefix of {cut} bytes"
            );
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Index::decode(&extra, BS, 2000).is_err());
    }

    #[test]
    fn every_single_byte_corruption_is_survived() {
        // Not all flips are detectable at this layer (e.g. a hash byte), but none may panic.
        let bytes = sample().encode();
        for i in 0..bytes.len() {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut b = bytes.clone();
                b[i] ^= flip;
                let _ = Index::decode(&b, BS, 2000);
            }
        }
    }

    #[test]
    fn rejects_bad_semantics() {
        let ok = sample();
        let enc = |idx: &Index| Index::decode(&idx.encode(), BS, 2000);

        let mut i = ok.clone();
        i.entries[1].path = "../evil".into();
        assert!(matches!(enc(&i), Err(Error::UnsafePath(_))));

        let mut i = ok.clone();
        i.entries[1].path = "/abs".into();
        assert!(matches!(enc(&i), Err(Error::UnsafePath(_))));

        let mut i = ok.clone();
        i.entries[3].path = "empty".into();
        assert!(matches!(enc(&i), Err(Error::Corrupt(ref m)) if m.contains("duplicate")));

        let mut i = ok.clone();
        i.total_raw += 1;
        assert!(enc(&i).is_err(), "sizes must add up");

        let mut i = ok.clone();
        i.block_offsets.pop();
        assert!(enc(&i).is_err(), "block count must match the stream length");

        let mut i = ok.clone();
        i.block_offsets = vec![700, 16];
        assert!(enc(&i).is_err(), "offsets must increase");

        let mut i = ok.clone();
        i.block_offsets = vec![16, 5000];
        assert!(enc(&i).is_err(), "offsets must lie before the index");

        let mut i = ok.clone();
        i.block_offsets[0] = 3;
        assert!(enc(&i).is_err(), "offsets must lie after the header");

        let mut i = ok.clone();
        i.entries[0].size = 1;
        i.total_raw += 1;
        assert!(enc(&i).is_err(), "directories have no size");

        let mut i = ok.clone();
        i.entries[1].mtime_nanos = 1_000_000_000;
        assert!(enc(&i).is_err());

        let mut i = ok;
        i.entries[1].size = u64::MAX;
        assert!(enc(&i).is_err(), "overflowing sizes");
    }

    #[test]
    fn lying_counts_do_not_cause_huge_allocations() {
        // 4 billion entries claimed in a 12-byte index.
        let mut b = vec![];
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&[0; 8]);
        assert!(Index::decode(&b, BS, 100).is_err());
        // Claimed block count far beyond the bytes present.
        let mut idx = Index::default().encode();
        let n = idx.len();
        idx[n - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Index::decode(&idx, BS, 100).is_err());
    }
}
