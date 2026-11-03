//! Zstandard via the `zstd` crate (bindings to the reference C library).
//!
//! Compression and decompression contexts are cached per thread: creating a context is far more
//! expensive than resetting one, and blocks are processed by long-lived worker threads.

use std::cell::RefCell;

use super::{BlockCodec, Method, check_len};
use crate::error::{Error, Result};

pub(super) struct Zstd {
    level: i32,
}

impl Zstd {
    pub(super) const fn new(level: i32) -> Self {
        Self { level }
    }
}

type CompressorSlot = Option<(i32, zstd::bulk::Compressor<'static>)>;

thread_local! {
    static COMPRESSOR: RefCell<CompressorSlot> = const { RefCell::new(None) };
    static DECOMPRESSOR: RefCell<Option<zstd::bulk::Decompressor<'static>>> =
        const { RefCell::new(None) };
}

fn err(e: std::io::Error) -> Error {
    Error::Codec {
        codec: "zstd",
        message: e.to_string(),
    }
}

impl BlockCodec for Zstd {
    fn method(&self) -> Method {
        Method::Zstd
    }

    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        COMPRESSOR.with(|slot| {
            let mut slot = slot.borrow_mut();
            if !matches!(&*slot, Some((level, _)) if *level == self.level) {
                *slot = Some((
                    self.level,
                    zstd::bulk::Compressor::new(self.level).map_err(err)?,
                ));
            }
            let (_, compressor) = slot.as_mut().expect("compressor was just created");
            compressor.compress(input).map_err(err)
        })
    }

    fn decompress(&self, input: &[u8], raw_len: usize) -> Result<Vec<u8>> {
        DECOMPRESSOR.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(zstd::bulk::Decompressor::new().map_err(err)?);
            }
            let decompressor = slot.as_mut().expect("decompressor was just created");
            // `raw_len` is the capacity limit: a frame claiming more fails instead of allocating.
            let out = decompressor.decompress(input, raw_len).map_err(err)?;
            check_len("zstd", out.len(), raw_len)?;
            Ok(out)
        })
    }
}
