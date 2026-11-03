//! The identity codec, used for blocks that do not compress.

use super::{BlockCodec, Method, check_len};
use crate::error::Result;

pub(super) struct Store;

impl BlockCodec for Store {
    fn method(&self) -> Method {
        Method::Store
    }

    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        Ok(input.to_vec())
    }

    fn decompress(&self, input: &[u8], raw_len: usize) -> Result<Vec<u8>> {
        check_len("store", input.len(), raw_len)?;
        Ok(input.to_vec())
    }
}
