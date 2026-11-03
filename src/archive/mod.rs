//! High-level archive operations: [`pack`], [`Archive::open`], [`extract`] and [`verify`].

mod extract;
mod reader;
mod writer;

use crate::error::{Error, Result};
use crate::parallel::{Config, MemoryModel};

pub use extract::{
    ExtractOptions, ExtractStats, extract, extract_with_progress, verify, verify_with,
};
pub use reader::{Archive, BlockStats};
pub use writer::{PackOptions, PackStats, pack, pack_with_progress};

/// Callback receiving `(bytes_done, bytes_total)` as work proceeds.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

/// Pick the pipeline shape for `threads` (0 = all cores), honouring an optional memory limit.
pub(crate) fn pipeline_config(
    threads: usize,
    memory_limit: Option<u64>,
    model: MemoryModel,
    advice: &str,
) -> Result<Config> {
    match memory_limit {
        None => Ok(Config::new(threads)),
        Some(limit) => Config::with_memory_limit(threads, limit, model).map_err(|need| {
            Error::Invalid(format!(
                "memory limit of {limit} bytes is too small: this operation needs about {need} \
                 bytes even with one thread ({advice})"
            ))
        }),
    }
}
