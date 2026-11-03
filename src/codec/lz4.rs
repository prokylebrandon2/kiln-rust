//! LZ4 block compression via the pure-Rust, safe `lz4_flex` crate.

use super::{BlockCodec, Method, check_len};
use crate::error::{Error, Result};

pub(super) struct Lz4;

impl BlockCodec for Lz4 {
    fn method(&self) -> Method {
        Method::Lz4
    }

    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        Ok(lz4_flex::block::compress(input))
    }

    fn decompress(&self, input: &[u8], raw_len: usize) -> Result<Vec<u8>> {
        let out = lz4_flex::block::decompress(input, raw_len).map_err(|e| Error::Codec {
            codec: "lz4",
            message: e.to_string(),
        })?;
        check_len("lz4", out.len(), raw_len)?;
        Ok(out)
    }
}
