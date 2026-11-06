//! LZMA2 (in an xz container, no integrity check - Kiln's own CRC-32 and BLAKE3 cover that)
//! via the `liblzma` crate, which bundles the reference xz library.
//!
//! The dictionary is capped near the block size: a bigger one cannot help (blocks are
//! independent) but would make every encoder allocate tens of megabytes it never uses.

use std::io::{Read, Write};

use liblzma::stream::{Check, Filters, LzmaOptions, Stream};

use super::{BlockCodec, Method, check_len};
use crate::error::{Error, Result};

/// A lossless pre-filter run before LZMA2. The xz block header records the filter chain, so
/// decoders need no hint: chunks keep method id 3 and the format version does not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Filter {
    /// Plain LZMA2.
    None,
    /// Branch converter (BCJ) for x86 and x86-64 code.
    X86,
    /// Branch converter (BCJ) for ARM64 code.
    Arm64,
    /// Byte-wise delta at this distance (1..=255), for raw audio and bitmaps.
    Delta(u8),
}

pub(super) struct Lzma2 {
    preset: u32,
    filter: Filter,
}

impl Lzma2 {
    pub(super) const fn new(preset: u32) -> Self {
        Self::with_filter(preset, Filter::None)
    }

    pub(super) const fn with_filter(preset: u32, filter: Filter) -> Self {
        Self { preset, filter }
    }
}

/// Choose the pre-filter for a file from its first bytes: a branch converter for ELF, PE and
/// Mach-O code (x86, x86-64, ARM64) and a delta for PCM WAV audio and uncompressed 24 or 32 bit
/// BMP. [`Filter::None`] when nothing is recognised. A wrong guess only costs ratio.
pub fn detect_filter(head: &[u8]) -> Filter {
    sniff(head).unwrap_or(Filter::None)
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..)?.get(..2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..)?.get(..4)?.try_into().ok()?))
}

fn sniff(head: &[u8]) -> Option<Filter> {
    if head.starts_with(b"\x7FELF") {
        let machine = match *head.get(5)? {
            1 => le16(head, 18)?,
            2 => u16::from_be_bytes(head.get(18..20)?.try_into().ok()?),
            _ => return None,
        };
        return Some(match machine {
            3 | 62 => Filter::X86,
            183 => Filter::Arm64,
            _ => Filter::None,
        });
    }
    if head.starts_with(b"MZ") {
        let pe = le32(head, 0x3C)? as usize;
        let tail = head.get(pe..)?;
        if tail.get(..4)? != b"PE\0\0" {
            return None;
        }
        return Some(match le16(tail, 4)? {
            0x014C | 0x8664 => Filter::X86,
            0xAA64 => Filter::Arm64,
            _ => Filter::None,
        });
    }
    if matches!(head.get(..4), Some([0xCF | 0xCE, 0xFA, 0xED, 0xFE])) {
        return Some(match le32(head, 4)? {
            7 | 0x0100_0007 => Filter::X86,
            0x0100_000C => Filter::Arm64,
            _ => Filter::None,
        });
    }
    if head.starts_with(b"RIFF") && head.get(8..16)? == b"WAVEfmt " {
        let (tag, align, bits) = (le16(head, 20)?, le16(head, 32)?, le16(head, 34)?);
        let pcm = tag == 1 && matches!(bits, 8 | 16 | 24 | 32);
        return Some(match u8::try_from(align) {
            Ok(dist) if pcm && (1..=64).contains(&dist) => Filter::Delta(dist),
            _ => Filter::None,
        });
    }
    if head.starts_with(b"BM") {
        let bpp = le16(head, 28)?;
        let uncompressed = le32(head, 14)? >= 40 && le32(head, 30)? == 0;
        return Some(match bpp {
            24 | 32 if uncompressed => Filter::Delta((bpp / 8) as u8),
            _ => Filter::None,
        });
    }
    None
}

fn err(e: impl std::fmt::Display) -> Error {
    Error::Codec {
        codec: "lzma",
        message: e.to_string(),
    }
}

impl BlockCodec for Lzma2 {
    fn method(&self) -> Method {
        Method::Lzma2
    }

    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        let mut options = LzmaOptions::new_preset(self.preset).map_err(err)?;
        let dict = u32::try_from(input.len().max(4096).next_power_of_two()).unwrap_or(u32::MAX);
        let mut filters = Filters::new();
        // Presets 0-9 have dictionaries of 256 KiB to 64 MiB; shrink only, never grow.
        let preset_dict = [
            1 << 18,
            1 << 20,
            1 << 21,
            1 << 22,
            1 << 22,
            1 << 23,
            1 << 23,
            1 << 24,
            1 << 25,
            1 << 26,
        ][self.preset.min(9) as usize];
        options.dict_size(dict.min(preset_dict));
        match self.filter {
            Filter::None => {}
            Filter::X86 => {
                filters.x86();
            }
            Filter::Arm64 => {
                filters.arm64();
            }
            Filter::Delta(dist) => {
                filters.delta_properties(&[dist.max(1) - 1]).map_err(err)?;
            }
        }
        filters.lzma2(&options);
        let stream = Stream::new_stream_encoder(&filters, Check::None).map_err(err)?;
        let mut encoder =
            liblzma::write::XzEncoder::new_stream(Vec::with_capacity(input.len() / 2), stream);
        encoder.write_all(input).map_err(err)?;
        encoder.finish().map_err(err)
    }

    fn decompress(&self, input: &[u8], raw_len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(raw_len);
        // `take` bounds the output so a decompression bomb cannot exceed `raw_len + 1`.
        let limit = raw_len as u64 + 1;
        liblzma::read::XzDecoder::new(input)
            .take(limit)
            .read_to_end(&mut out)
            .map_err(err)?;
        check_len("lzma", out.len(), raw_len)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{CodecSpec, Method, decoder_for};

    const ALL: [Filter; 5] = [
        Filter::None,
        Filter::X86,
        Filter::Arm64,
        Filter::Delta(1),
        Filter::Delta(4),
    ];

    /// Compress with `filter`, check the stock decoder restores `data`, return the packed size.
    fn packed_len(filter: Filter, data: &[u8]) -> usize {
        let codec = CodecSpec::new(Method::Lzma2, 3)
            .build_filtered(filter)
            .unwrap();
        let packed = codec.compress(data).unwrap();
        let back = decoder_for(Method::Lzma2)
            .decompress(&packed, data.len())
            .unwrap();
        assert_eq!(back, data, "{filter:?} len {}", data.len());
        packed.len()
    }

    fn put(h: &mut [u8], at: usize, bytes: &[u8]) {
        h[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn elf(machine: u16) -> Vec<u8> {
        let mut h = vec![0u8; 64];
        put(&mut h, 0, b"\x7FELF\x02\x01\x01");
        put(&mut h, 18, &machine.to_le_bytes());
        h
    }

    fn pe(machine: u16) -> Vec<u8> {
        let mut h = vec![0u8; 128];
        put(&mut h, 0, b"MZ");
        put(&mut h, 0x3C, &0x40u32.to_le_bytes());
        put(&mut h, 0x40, b"PE\0\0");
        put(&mut h, 0x44, &machine.to_le_bytes());
        h
    }

    #[test]
    fn every_filter_round_trips_every_shape_of_input() {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let noise: Vec<u8> = (0..70_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let inputs: Vec<Vec<u8>> = vec![
            vec![],
            vec![7],
            vec![1, 2, 3],
            noise,
            b"kiln filters ".repeat(5000),
        ];
        for filter in ALL {
            for input in &inputs {
                packed_len(filter, input);
            }
        }
    }

    #[test]
    fn x86_filter_shrinks_call_heavy_code() {
        // 11-byte records: CALL rel32 to one of eight targets, then NOPs. Every relative offset
        // differs; after the filter they are absolute addresses and repeat.
        let mut code = Vec::new();
        for i in 0..6000u32 {
            let target = 0x1000 + (i % 8) * 0x40;
            let rel = target.wrapping_sub(i * 11 + 5);
            code.push(0xE8);
            code.extend_from_slice(&rel.to_le_bytes());
            code.extend_from_slice(&[0x90; 6]);
        }
        let plain = packed_len(Filter::None, &code);
        let bcj = packed_len(Filter::X86, &code);
        assert!(bcj < plain / 2, "x86 {bcj} vs plain {plain}");
    }

    #[test]
    fn delta_filter_shrinks_stereo_pcm() {
        // 16-bit stereo ramps: a byte delta at distance 4 turns them into near-constant runs.
        let mut pcm = Vec::new();
        for n in 0..100_000u32 {
            pcm.extend_from_slice(&((3 * n) as u16).to_le_bytes());
            pcm.extend_from_slice(&((5 * n) as u16).to_le_bytes());
        }
        let plain = packed_len(Filter::None, &pcm);
        let delta = packed_len(Filter::Delta(4), &pcm);
        assert!(delta < plain / 2, "delta {delta} vs plain {plain}");
    }

    #[test]
    fn detect_filter_reads_code_audio_and_bitmap_headers() {
        let mut macho = vec![0u8; 32];
        put(&mut macho, 0, &[0xCF, 0xFA, 0xED, 0xFE]);
        put(&mut macho, 4, &0x0100_000Cu32.to_le_bytes());
        let mut wav = vec![0u8; 44];
        put(&mut wav, 0, b"RIFF");
        put(&mut wav, 8, b"WAVEfmt ");
        put(&mut wav, 16, &16u32.to_le_bytes());
        put(&mut wav, 20, &1u16.to_le_bytes()); // PCM
        put(&mut wav, 22, &2u16.to_le_bytes()); // channels
        put(&mut wav, 32, &4u16.to_le_bytes()); // block align
        put(&mut wav, 34, &16u16.to_le_bytes()); // bits per sample
        let mut bmp = vec![0u8; 54];
        put(&mut bmp, 0, b"BM");
        put(&mut bmp, 14, &40u32.to_le_bytes());
        put(&mut bmp, 28, &24u16.to_le_bytes());

        assert_eq!(detect_filter(&elf(62)), Filter::X86);
        assert_eq!(detect_filter(&elf(3)), Filter::X86);
        assert_eq!(detect_filter(&elf(183)), Filter::Arm64);
        assert_eq!(detect_filter(&elf(40)), Filter::None);
        assert_eq!(detect_filter(&pe(0x8664)), Filter::X86);
        assert_eq!(detect_filter(&pe(0xAA64)), Filter::Arm64);
        assert_eq!(detect_filter(&macho), Filter::Arm64);
        assert_eq!(detect_filter(&wav), Filter::Delta(4));
        assert_eq!(detect_filter(&bmp), Filter::Delta(3));
        assert_eq!(detect_filter(b"plain text, no magic"), Filter::None);
        assert_eq!(detect_filter(b"MZ"), Filter::None);
        assert_eq!(detect_filter(&[]), Filter::None);
    }
}
