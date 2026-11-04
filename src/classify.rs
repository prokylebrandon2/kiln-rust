//! Content classification: decide, from a small sample of a file's bytes, whether it is worth
//! compressing and which kind of data it is.
//!
//! Extensions are never consulted. Two independent signals are used instead:
//!
//! 1. **Magic numbers** of well-known containers (PNG, JPEG, ZIP, gzip, xz, zstd, MP4, ...) mark a
//!    file as already compressed; ELF / PE / Mach-O mark it as executable code.
//! 2. **Statistics** of up to 64 KiB of sampled bytes: printable-ASCII share (text) and, for the
//!    rest, byte-histogram entropy together with how much LZ4 can save on the sample.
//!
//! A misclassification costs ratio or time, never correctness: the class only decides *how* a
//! file is stored, and every chunk is still checksummed and decoded by its own method id.

use crate::error::{Error, Result};

/// What kind of data a file holds, as judged from a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Class {
    /// Not classified (the profile does not classify, or the entry is a directory).
    Unknown = 0,
    /// Mostly printable text.
    Text = 1,
    /// Compressible data that is neither text nor code.
    Data = 2,
    /// Executable or object code (ELF, PE, Mach-O).
    Binary = 3,
    /// A known already-compressed container or media format.
    Compressed = 4,
    /// High-entropy data that LZ4 cannot shrink (encrypted, random, unknown compressed format).
    Random = 5,
}

impl Class {
    /// The value stored in the index.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Parse a stored value.
    pub fn from_id(id: u8) -> Result<Self> {
        match id {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Text),
            2 => Ok(Self::Data),
            3 => Ok(Self::Binary),
            4 => Ok(Self::Compressed),
            5 => Ok(Self::Random),
            other => Err(Error::Unsupported(format!("unknown content class {other}"))),
        }
    }

    /// Lower-case name for listings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "-",
            Self::Text => "text",
            Self::Data => "data",
            Self::Binary => "binary",
            Self::Compressed => "compressed",
            Self::Random => "random",
        }
    }

    /// True when compressing this class is pointless, so it is stored.
    pub fn is_incompressible(self) -> bool {
        matches!(self, Self::Compressed | Self::Random)
    }
}

/// How many bytes are sampled from the start, and from the middle, of a file.
pub const SAMPLE_HEAD: usize = 32 << 10;
/// Size of the second sample taken from the middle of files larger than twice [`SAMPLE_HEAD`].
pub const SAMPLE_MID: usize = 32 << 10;
/// Files smaller than this are judged by magic number and text share only: a histogram of a few
/// kilobytes is too noisy to call data random.
const MIN_STATISTICAL: u64 = 8 << 10;
/// Order-0 entropy (bits per byte) above which a sample is a candidate for "incompressible".
const ENTROPY_MIN: f64 = 7.95;
/// If LZ4 saves at least this fraction of the sample, it is compressible whatever its entropy.
const LZ4_MIN_SAVING: f64 = 0.02;

/// Known already-compressed or media container signatures: `(offset, bytes)`.
const COMPRESSED_MAGIC: &[(usize, &[u8])] = &[
    (0, b"\x89PNG\r\n\x1a\n"),
    (0, b"\xFF\xD8\xFF"),         // JPEG
    (0, b"GIF8"),                 // GIF
    (0, b"PK\x03\x04"),           // ZIP, JAR, APK, DOCX, XLSX, ...
    (0, b"\x1F\x8B"),             // gzip
    (0, b"BZh"),                  // bzip2
    (0, b"\xFD7zXZ\x00"),         // xz
    (0, b"\x28\xB5\x2F\xFD"),     // zstd frame
    (0, b"\x04\x22\x4D\x18"),     // LZ4 frame
    (0, b"7z\xBC\xAF\x27\x1C"),   // 7-Zip
    (0, b"Rar!\x1A\x07"),         // RAR
    (4, b"ftyp"),                 // MP4, MOV, M4A, HEIC, AVIF
    (0, b"\x1A\x45\xDF\xA3"),     // Matroska / WebM
    (0, b"OggS"),                 // Ogg Vorbis / Opus
    (0, b"fLaC"),                 // FLAC
    (0, b"ID3"),                  // MP3 with tag
    (0, b"wOF2"),                 // WOFF2
    (0, b"\xFF\x0A"),             // JPEG XL codestream
    (0, b"\x00\x00\x00\x0CJXL "), // JPEG XL container
];

fn is_riff_media(head: &[u8]) -> bool {
    // WebP is lossy-compressed; WAV and AVI usually are not, so only WebP is matched.
    head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP"
}

const BINARY_MAGIC: &[&[u8]] = &[
    b"\x7FELF",
    b"MZ",
    b"\xFE\xED\xFA\xCE",
    b"\xFE\xED\xFA\xCF",
    b"\xCE\xFA\xED\xFE",
    b"\xCF\xFA\xED\xFE",
];

/// Classify by magic number alone.
pub fn sniff_magic(head: &[u8]) -> Option<Class> {
    if COMPRESSED_MAGIC.iter().any(|(off, magic)| {
        head.len() >= off + magic.len() && &head[*off..off + magic.len()] == *magic
    }) || is_riff_media(head)
    {
        return Some(Class::Compressed);
    }
    // MPEG audio frame sync without a tag: 0xFF followed by 111x xxxx.
    if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 && head[1] & 0x06 != 0 {
        return Some(Class::Compressed);
    }
    if BINARY_MAGIC.iter().any(|m| head.starts_with(m)) {
        return Some(Class::Binary);
    }
    None
}

/// True when at least 90% of `sample` is printable ASCII or tab/newline/return and there are no
/// NUL bytes. Random data is about 37% printable, so the margin is wide.
fn looks_like_text(sample: &[u8]) -> bool {
    if sample.is_empty() {
        return false;
    }
    let mut printable = 0usize;
    for &b in sample {
        match b {
            0 => return false,
            b'\t' | b'\n' | b'\r' | 0x20..=0x7E => printable += 1,
            _ => {}
        }
    }
    printable * 10 >= sample.len() * 9
}

/// Order-0 entropy of `sample` in bits per byte.
fn entropy(sample: &[u8]) -> f64 {
    let mut histogram = [0u32; 256];
    for &b in sample {
        histogram[b as usize] += 1;
    }
    let n = sample.len() as f64;
    histogram
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}

/// True when the sample has near-uniform bytes *and* LZ4 cannot shrink it by 2%.
fn sample_is_incompressible(sample: &[u8]) -> bool {
    if entropy(sample) < ENTROPY_MIN {
        return false;
    }
    let packed = lz4_flex::block::compress(sample).len();
    (sample.len() - packed.min(sample.len())) as f64 / (sample.len() as f64) < LZ4_MIN_SAVING
}

/// Classify a file of `size` bytes from its first bytes (`head`) and, for files larger than
/// twice [`SAMPLE_HEAD`], a second sample from the middle (`mid`; may be empty).
pub fn classify(head: &[u8], mid: &[u8], size: u64) -> Class {
    if let Some(class) = sniff_magic(head) {
        return class;
    }
    if looks_like_text(head) {
        return Class::Text;
    }
    if size < MIN_STATISTICAL || head.len() < (MIN_STATISTICAL as usize).min(size as usize) {
        return Class::Data;
    }
    let mut sample = Vec::with_capacity(head.len() + mid.len());
    sample.extend_from_slice(head);
    sample.extend_from_slice(mid);
    if sample_is_incompressible(&sample) {
        Class::Random
    } else {
        Class::Data
    }
}

#[cfg(test)]
mod tests {
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

    fn prose(len: usize) -> Vec<u8> {
        b"the quick brown fox jumps over the lazy dog\n"
            .iter()
            .cycle()
            .take(len)
            .copied()
            .collect()
    }

    #[test]
    fn magic_numbers_win_regardless_of_content() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(prose(20_000));
        assert_eq!(classify(&png, &[], png.len() as u64), Class::Compressed);
        let mut mp4 = vec![0, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend(prose(100));
        assert_eq!(classify(&mp4, &[], mp4.len() as u64), Class::Compressed);
        for magic in [
            &b"\x1F\x8B\x08"[..],
            b"PK\x03\x04",
            b"\xFD7zXZ\x00",
            b"\x28\xB5\x2F\xFD",
        ] {
            assert_eq!(classify(magic, &[], 100), Class::Compressed, "{magic:?}");
        }
        assert_eq!(classify(b"\x7FELF\x02\x01", &[], 6), Class::Binary);
        assert_eq!(classify(b"MZ\x90\x00", &[], 4), Class::Binary);
        assert_eq!(
            classify(b"RIFF\x10\x00\x00\x00WEBPVP8 ", &[], 20),
            Class::Compressed
        );
        assert_eq!(
            classify(b"RIFF\x10\x00\x00\x00WAVEfmt ", &[], 20),
            Class::Data
        );
    }

    #[test]
    fn statistics_separate_text_noise_and_structured_data() {
        let text = prose(50_000);
        assert_eq!(
            classify(&text[..SAMPLE_HEAD], &text[..SAMPLE_MID], 50_000),
            Class::Text
        );
        let rnd = noise(200_000, 7);
        assert_eq!(
            classify(
                &rnd[..SAMPLE_HEAD],
                &rnd[100_000..100_000 + SAMPLE_MID],
                200_000
            ),
            Class::Random
        );
        let zeros = vec![0u8; 100_000];
        assert_eq!(
            classify(&zeros[..SAMPLE_HEAD], &zeros[..SAMPLE_MID], 100_000),
            Class::Data
        );
        // Repeated random bytes have a flat histogram but LZ4 finds the repeats.
        let block = noise(1_000, 3);
        let repeated: Vec<u8> = block.iter().cycle().take(100_000).copied().collect();
        assert_eq!(
            classify(&repeated[..SAMPLE_HEAD], &repeated[..SAMPLE_MID], 100_000),
            Class::Data
        );
    }

    #[test]
    fn tiny_files_are_never_called_random() {
        let rnd = noise(500, 1);
        assert_eq!(classify(&rnd, &[], 500), Class::Data);
        assert_eq!(classify(&[], &[], 0), Class::Data);
    }

    #[test]
    fn ids_round_trip() {
        for id in 0..=5 {
            assert_eq!(Class::from_id(id).unwrap().id(), id);
        }
        assert!(Class::from_id(6).is_err());
        assert!(Class::Random.is_incompressible() && Class::Compressed.is_incompressible());
        assert!(!Class::Text.is_incompressible() && !Class::Binary.is_incompressible());
    }
}
