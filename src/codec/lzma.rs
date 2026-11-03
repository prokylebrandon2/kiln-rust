//! LZMA2 (in an xz container, no integrity check - Kiln's own CRC-32 and BLAKE3 cover that)
//! via the `liblzma` crate, which bundles the reference xz library.
//!
//! The dictionary is capped near the block size: a bigger one cannot help (blocks are
//! independent) but would make every encoder allocate tens of megabytes it never uses.

use std::io::{Read, Write};

use liblzma::stream::{Check, Filters, LzmaOptions, Stream};

use super::{BlockCodec, Method, check_len};
use crate::error::{Error, Result};

pub(super) struct Lzma2 {
    preset: u32,
}

impl Lzma2 {
    pub(super) const fn new(preset: u32) -> Self {
        Self { preset }
    }
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
