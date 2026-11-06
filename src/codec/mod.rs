//! Compression backends.
//!
//! A backend is anything implementing [`BlockCodec`]: it compresses one independent block and
//! decompresses one independent block. Nothing else in Kiln knows which library does the work, so
//! a backend can be replaced or added without touching the archive format code: assign a new
//! on-disk [`Method`] id, implement the trait, and register it in [`CodecSpec::build`] and
//! [`decoder_for`].

mod lz4;
mod lzma;
mod store;
mod zstd;

pub use lzma::{Filter, detect_filter};

use std::fmt;

use crate::error::{Error, Result};

/// On-disk identifier of a compression method (one byte in every block record).
///
/// These numbers are part of the archive format and must never be reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Method {
    /// The block is stored verbatim.
    Store = 0,
    /// LZ4 block format (very fast, modest ratio).
    Lz4 = 1,
    /// Zstandard frame (fast, strong ratio, excellent decode speed).
    Zstd = 2,
    /// LZMA2 inside an xz container (slowest, best ratio).
    Lzma2 = 3,
}

impl Method {
    /// The byte written to disk.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Parse the byte read from disk.
    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Store),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            3 => Some(Self::Lzma2),
            _ => None,
        }
    }

    /// Lower-case name used on the command line and in listings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
            Self::Lzma2 => "lzma",
        }
    }
}

/// Compresses and decompresses single, independent blocks. Implementations must be thread-safe:
/// one instance is shared by every worker thread.
pub trait BlockCodec: Send + Sync {
    /// The method id this codec writes.
    fn method(&self) -> Method;

    /// Compress `input` (which may be empty) into a fresh buffer.
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>>;

    /// Decompress `input`, which must expand to exactly `raw_len` bytes. A decoder must never
    /// produce more than `raw_len` bytes, whatever `input` contains.
    fn decompress(&self, input: &[u8], raw_len: usize) -> Result<Vec<u8>>;
}

/// A codec choice: the method plus its tuning level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecSpec {
    /// Which backend.
    pub method: Method,
    /// Backend-specific level (zstd -7..=22, lzma 0..=9; ignored by store and lz4).
    pub level: i32,
}

impl CodecSpec {
    /// Default level for a method.
    pub fn default_level(method: Method) -> i32 {
        match method {
            Method::Zstd => 3,
            Method::Lzma2 => 6,
            Method::Store | Method::Lz4 => 0,
        }
    }

    /// The default codec: zstd level 3.
    pub fn default_spec() -> Self {
        Self::new(Method::Zstd, Self::default_level(Method::Zstd))
    }

    /// Construct without validation; [`CodecSpec::build`] validates.
    pub fn new(method: Method, level: i32) -> Self {
        Self { method, level }
    }

    /// Parse `store`, `lz4`, `zstd`, `zstd:19`, `lzma` or `lzma:9`.
    pub fn parse(text: &str) -> Result<Self> {
        let (name, level) = match text.split_once(':') {
            Some((n, l)) => (n, Some(l)),
            None => (text, None),
        };
        let method = match name.trim().to_ascii_lowercase().as_str() {
            "store" | "none" => Method::Store,
            "lz4" => Method::Lz4,
            "zstd" | "zstandard" => Method::Zstd,
            "lzma" | "lzma2" | "xz" => Method::Lzma2,
            other => {
                return Err(Error::Invalid(format!(
                    "unknown codec {other:?} (choose store, lz4, zstd[:LEVEL] or lzma[:LEVEL])"
                )));
            }
        };
        let level = match level {
            Some(l) => l
                .trim()
                .parse::<i32>()
                .map_err(|_| Error::Invalid(format!("invalid compression level {l:?}")))?,
            None => Self::default_level(method),
        };
        let spec = Self { method, level };
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<()> {
        let (lo, hi) = match self.method {
            Method::Zstd => (-7, 22),
            Method::Lzma2 => (0, 9),
            Method::Store | Method::Lz4 => (0, 0),
        };
        if self.level < lo || self.level > hi {
            let what = if lo == hi {
                format!("{} has no compression levels", self.method.name())
            } else {
                format!("{} level must be {lo}..={hi}", self.method.name())
            };
            return Err(Error::Invalid(format!("{what} (got {})", self.level)));
        }
        Ok(())
    }

    /// Conservative estimate of the working memory one *compression* worker needs for blocks of
    /// `block` bytes, on top of the block buffers themselves. Derived from measured peak RSS
    /// (see `docs/PHASE2.md`) and rounded up; used by `--memory-limit`.
    pub fn compress_context_bytes(&self, block: u64) -> u64 {
        const MIB: u64 = 1 << 20;
        match self.method {
            Method::Store => 0,
            Method::Lz4 => MIB,
            Method::Zstd => match self.level {
                ..=3 => 4 * MIB + 2 * block.min(2 * MIB),
                4..=9 => 8 * MIB + 4 * block.min(4 * MIB),
                _ => 8 * MIB + 10 * block.min(16 * MIB),
            },
            Method::Lzma2 => {
                let dict = match self.level {
                    0 => MIB / 4,
                    1 => MIB,
                    2 => 2 * MIB,
                    3 | 4 => 4 * MIB,
                    5 | 6 => 8 * MIB,
                    7 => 16 * MIB,
                    8 => 32 * MIB,
                    _ => 64 * MIB,
                };
                2 * MIB + 12 * dict.min(block)
            }
        }
    }

    /// Estimated working memory of one *decompression* worker for blocks of `block` bytes.
    pub fn decompress_context_bytes(block: u64) -> u64 {
        block + (1 << 20)
    }

    /// True for codecs slow enough that sampling a block to skip hopeless ones pays off.
    pub fn is_heavy(&self) -> bool {
        match self.method {
            Method::Zstd => self.level >= 10,
            Method::Lzma2 => true,
            Method::Store | Method::Lz4 => false,
        }
    }

    /// Instantiate the backend.
    pub fn build(&self) -> Result<Box<dyn BlockCodec>> {
        self.validate()?;
        Ok(match self.method {
            Method::Store => Box::new(store::Store),
            Method::Lz4 => Box::new(lz4::Lz4),
            Method::Zstd => Box::new(zstd::Zstd::new(self.level)),
            Method::Lzma2 => Box::new(lzma::Lzma2::new(self.level as u32)),
        })
    }

    /// Like [`CodecSpec::build`], with a pre-filter for LZMA2 (other methods ignore it).
    pub fn build_filtered(&self, filter: Filter) -> Result<Box<dyn BlockCodec>> {
        if self.method != Method::Lzma2 || filter == Filter::None {
            return self.build();
        }
        self.validate()?;
        Ok(Box::new(lzma::Lzma2::with_filter(
            self.level as u32,
            filter,
        )))
    }
}

impl fmt::Display for CodecSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.method {
            Method::Store | Method::Lz4 => f.write_str(self.method.name()),
            _ => write!(f, "{}:{}", self.method.name(), self.level),
        }
    }
}

/// The decoder for an on-disk method. Decoding needs no level, so one static instance serves all.
pub fn decoder_for(method: Method) -> &'static dyn BlockCodec {
    static STORE: store::Store = store::Store;
    static LZ4: lz4::Lz4 = lz4::Lz4;
    static ZSTD: zstd::Zstd = zstd::Zstd::new(3);
    static LZMA: lzma::Lzma2 = lzma::Lzma2::new(6);
    match method {
        Method::Store => &STORE,
        Method::Lz4 => &LZ4,
        Method::Zstd => &ZSTD,
        Method::Lzma2 => &LZMA,
    }
}

/// Cheap test for "this block will not compress": samples four 16 KiB windows spread across the
/// block and requires **both** a near-flat byte histogram (order-0 entropy above 7.98 bits/byte)
/// **and** that LZ4 saves less than 2% on every sample together. Either signal alone has
/// well-known false positives (PCM audio has no LZ matches; repeated random bytes have a flat
/// histogram), so only data that fails both is skipped. Blocks under 128 KiB are never skipped.
///
/// Known blind spot: matches farther apart than the sampled windows (for example a 2 MiB random
/// chunk repeated twice in one block) are not seen, so such a block is stored instead of
/// compressed. That costs ratio, never correctness.
pub fn looks_incompressible(block: &[u8]) -> bool {
    const WINDOW: usize = 16 << 10;
    const WINDOWS: usize = 4;
    if block.len() < 2 * WINDOW * WINDOWS {
        return false;
    }
    let span = block.len() - WINDOW;
    let mut histogram = [0u32; 256];
    let mut lz_out = 0usize;
    for k in 0..WINDOWS {
        let start = span / (WINDOWS - 1) * k;
        let window = &block[start..start + WINDOW];
        for &b in window {
            histogram[b as usize] += 1;
        }
        lz_out += lz4_flex::block::compress(window).len();
    }
    let n = (WINDOW * WINDOWS) as f64;
    let entropy: f64 = histogram
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum();
    entropy > 7.98 && lz_out as f64 > n * 0.98
}

/// Reject decoder output of the wrong size (shared by every backend).
pub(crate) fn check_len(codec: &'static str, got: usize, want: usize) -> Result<()> {
    if got == want {
        Ok(())
    } else {
        Err(Error::Codec {
            codec,
            message: format!("decoded {got} bytes, expected {want}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes (xorshift), no external RNG needed.
    fn noise(len: usize, mut x: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    fn text(len: usize) -> Vec<u8> {
        let words = [
            "archive ", "kiln ", "block ", "stream ", "entropy ", "window ", "\n",
        ];
        let mut out = Vec::new();
        let mut i = 0usize;
        while out.len() < len {
            out.extend_from_slice(words[(i * i + i / 3) % words.len()].as_bytes());
            i += 1;
        }
        out.truncate(len);
        out
    }

    fn all_specs() -> Vec<CodecSpec> {
        [
            "store", "lz4", "zstd", "zstd:1", "zstd:19", "zstd:-3", "lzma", "lzma:0", "lzma:9",
        ]
        .iter()
        .map(|s| CodecSpec::parse(s).unwrap())
        .collect()
    }

    #[test]
    fn every_codec_round_trips_every_shape_of_input() {
        let inputs: Vec<Vec<u8>> = vec![
            vec![],
            vec![42],
            vec![0; 100_000],
            text(300_000),
            noise(100_000, 7),
            noise(1, 3),
        ];
        for spec in all_specs() {
            let codec = spec.build().unwrap();
            for input in &inputs {
                let packed = codec.compress(input).unwrap();
                let back = decoder_for(spec.method)
                    .decompress(&packed, input.len())
                    .unwrap();
                assert_eq!(&back, input, "{spec} len {}", input.len());
            }
        }
    }

    #[test]
    fn zstd_window_and_long_range_matching_reach_a_repeat_ten_mebibytes_back() {
        let half = noise(10 << 20, 11);
        let mut data = half.clone();
        data.extend_from_slice(&half);
        let packed = CodecSpec::parse("zstd:10")
            .unwrap()
            .build()
            .unwrap()
            .compress(&data)
            .unwrap();
        assert!(packed.len() < 12 << 20, "packed to {} bytes", packed.len());
        let back = decoder_for(Method::Zstd)
            .decompress(&packed, data.len())
            .unwrap();
        assert!(back == data, "round trip differs");
    }

    /// Pseudo-prose: random picks from a 300-word vocabulary built from random letters.
    /// Unlike a short repeating pattern it has real entropy, so codec strength shows.
    fn prose(len: usize) -> Vec<u8> {
        let rnd = noise(300 * 8 + len, 42);
        let vocab: Vec<Vec<u8>> = (0..300)
            .map(|w| (0..3 + w % 7).map(|k| b'a' + rnd[w * 8 + k] % 26).collect())
            .collect();
        let picks = &rnd[300 * 8..];
        let mut out = Vec::with_capacity(len + 16);
        let mut i = 0;
        while out.len() < len {
            out.extend_from_slice(&vocab[(picks[i] as usize * 7 + i) % 300]);
            out.push(if picks[i].is_multiple_of(11) {
                b'\n'
            } else {
                b' '
            });
            i = (i + 1) % picks.len();
        }
        out.truncate(len);
        out
    }

    #[test]
    fn compressible_data_actually_shrinks_and_strength_is_ordered() {
        let data = prose(2 << 20);
        let size = |s: &str| {
            CodecSpec::parse(s)
                .unwrap()
                .build()
                .unwrap()
                .compress(&data)
                .unwrap()
                .len()
        };
        let (lz4, zstd3, zstd19, lzma6) =
            (size("lz4"), size("zstd:3"), size("zstd:19"), size("lzma:6"));
        let msg = format!(
            "lz4 {lz4}, zstd:3 {zstd3}, zstd:19 {zstd19}, lzma:6 {lzma6} of {}",
            data.len()
        );
        assert!(lz4 < data.len() * 3 / 4, "{msg}");
        assert!(zstd3 < lz4, "zstd:3 should beat lz4: {msg}");
        assert!(
            zstd19 <= zstd3,
            "a higher zstd level should not lose: {msg}"
        );
        assert!(lzma6 < zstd3, "lzma:6 should beat zstd:3 on text: {msg}");
    }

    #[test]
    fn decoders_reject_garbage_and_wrong_lengths_without_panicking() {
        let data = text(50_000);
        for spec in all_specs() {
            let codec = spec.build().unwrap();
            let packed = codec.compress(&data).unwrap();
            let dec = decoder_for(spec.method);
            // Wrong expected length (both too small and too large).
            if spec.method != Method::Store {
                assert!(
                    dec.decompress(&packed, data.len() - 1).is_err(),
                    "{spec} short"
                );
                assert!(
                    dec.decompress(&packed, data.len() + 1).is_err(),
                    "{spec} long"
                );
            }
            // Truncation and bit flips must be errors or wrong-but-bounded output, never panics.
            for cut in [0, 1, packed.len() / 2, packed.len().saturating_sub(1)] {
                let _ = dec.decompress(&packed[..cut], data.len());
            }
            let mut flipped = packed.clone();
            let mid = flipped.len() / 2;
            flipped[mid] ^= 0xFF;
            if let Ok(out) = dec.decompress(&flipped, data.len()) {
                assert_eq!(out.len(), data.len());
            }
            // Pure noise as "compressed" data.
            let _ = dec.decompress(&noise(1000, 99), 4096);
        }
    }

    #[test]
    fn spec_parsing_and_validation() {
        assert_eq!(
            CodecSpec::parse("zstd").unwrap(),
            CodecSpec::new(Method::Zstd, 3)
        );
        assert_eq!(CodecSpec::parse("ZSTD:19").unwrap().level, 19);
        assert_eq!(CodecSpec::parse("xz").unwrap().method, Method::Lzma2);
        assert_eq!(CodecSpec::parse("lz4").unwrap().to_string(), "lz4");
        assert_eq!(CodecSpec::parse("lzma:9").unwrap().to_string(), "lzma:9");
        for bad in [
            "", "gzip", "zstd:99", "zstd:x", "lzma:10", "lz4:5", "store:1",
        ] {
            assert!(CodecSpec::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn method_ids_are_stable() {
        // These values are part of the on-disk format.
        assert_eq!(
            [Method::Store, Method::Lz4, Method::Zstd, Method::Lzma2].map(Method::id),
            [0, 1, 2, 3]
        );
        for id in 0..=3 {
            assert_eq!(Method::from_id(id).unwrap().id(), id);
        }
        assert!(Method::from_id(4).is_none());
    }
}

#[cfg(test)]
mod sampler_tests {
    use super::*;

    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect()
    }

    #[test]
    fn random_data_is_flagged_but_small_blocks_never_are() {
        assert!(looks_incompressible(&noise(1 << 20, 7)));
        assert!(!looks_incompressible(&noise(64 << 10, 7)));
        assert!(!looks_incompressible(&[]));
    }

    #[test]
    fn compressible_data_is_never_flagged() {
        assert!(!looks_incompressible(&vec![0u8; 1 << 20]));
        let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog\n"
            .iter()
            .cycle()
            .take(1 << 20)
            .copied()
            .collect();
        assert!(!looks_incompressible(&text));
        // Flat histogram but highly repetitive: LZ4 finds the matches, so it is not skipped.
        let repeated: Vec<u8> = noise(256, 3).into_iter().cycle().take(1 << 20).collect();
        assert!(!looks_incompressible(&repeated));
        // Low-entropy "sensor" data with no long matches: entropy test keeps it out.
        let slow: Vec<u8> = noise(1 << 20, 5).iter().map(|b| b & 0x0F).collect();
        assert!(!looks_incompressible(&slow));
    }

    #[test]
    fn memory_estimates_grow_with_effort_and_block_size() {
        let mib = 1u64 << 20;
        let z3 = CodecSpec::new(Method::Zstd, 3);
        let z19 = CodecSpec::new(Method::Zstd, 19);
        let lz = CodecSpec::new(Method::Lzma2, 6);
        assert_eq!(
            CodecSpec::new(Method::Store, 0).compress_context_bytes(4 * mib),
            0
        );
        assert!(z3.compress_context_bytes(4 * mib) < z19.compress_context_bytes(4 * mib));
        assert!(z19.compress_context_bytes(mib) < z19.compress_context_bytes(4 * mib));
        assert!(
            lz.compress_context_bytes(32 * mib) >= 90 * mib,
            "lzma:6 needs ~96 MiB"
        );
        assert!(z19.is_heavy() && lz.is_heavy() && !z3.is_heavy());
    }
}
