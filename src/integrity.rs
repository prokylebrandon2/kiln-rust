//! Integrity primitives.
//!
//! Two layers, with different jobs:
//! * **CRC-32** of every stored block payload: cheap, computed on the worker threads, and checked
//!   *before* a decoder ever sees the bytes, so corrupt data is rejected early.
//! * **BLAKE3** of every file's original content, truncated to 128 bits: the end-to-end check that
//!   extraction produced exactly what was packed. 128 bits is far beyond what accidental
//!   corruption can collide with, and halves the per-file metadata (see `docs/PHASE3.md`).

/// Length of a stored file hash in bytes (BLAKE3 output truncated to 128 bits).
pub const HASH_LEN: usize = 16;

/// A truncated BLAKE3 digest of one file's contents.
pub type FileHash = [u8; HASH_LEN];

/// Streaming file hasher.
#[derive(Default)]
pub struct FileHasher(blake3::Hasher);

impl FileHasher {
    /// Start a new hash.
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    /// Feed more bytes.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// Finish and return the (truncated) digest.
    pub fn finalize(&self) -> FileHash {
        let full = self.0.finalize();
        let mut out = [0u8; HASH_LEN];
        out.copy_from_slice(&full.as_bytes()[..HASH_LEN]);
        out
    }
}

/// Hash of the empty file.
pub fn empty_hash() -> FileHash {
    FileHasher::new().finalize()
}

/// CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Lower-case hexadecimal rendering of a hash.
pub fn to_hex(hash: &FileHash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(to_hex(&empty_hash()), "af1349b9f5f9a1a6a0404dea36dcc949");
    }

    #[test]
    fn chunked_equals_whole() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i * 7) as u8).collect();
        let mut h = FileHasher::new();
        for chunk in data.chunks(333) {
            h.update(chunk);
        }
        let mut whole = FileHasher::new();
        whole.update(&data);
        assert_eq!(h.finalize(), whole.finalize());
    }
}
