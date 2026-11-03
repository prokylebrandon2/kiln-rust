//! Built-in benchmark: run the *real* pack and verify pipelines over your own data for a grid
//! of codecs and block sizes. This is how architectural choices (block size, codec) are
//! validated against measurements rather than guesses.

use std::path::PathBuf;
use std::time::Instant;

use crate::archive::{Archive, PackOptions, pack, verify};
use crate::codec::CodecSpec;
use crate::error::{IoContext, Result};

/// What to measure.
#[derive(Clone, Debug)]
pub struct BenchConfig {
    /// Files and directories to compress.
    pub inputs: Vec<PathBuf>,
    /// Codecs to try.
    pub codecs: Vec<CodecSpec>,
    /// Block sizes (log2) to try.
    pub block_log2s: Vec<u8>,
    /// Worker threads (`0` = all cores).
    pub threads: usize,
}

/// One measurement.
#[derive(Clone, Debug)]
pub struct BenchRow {
    /// Codec used.
    pub codec: CodecSpec,
    /// Block size (log2).
    pub block_log2: u8,
    /// Uncompressed bytes.
    pub raw_bytes: u64,
    /// Archive size.
    pub archive_bytes: u64,
    /// Wall-clock seconds to create the archive (includes reading the inputs).
    pub pack_secs: f64,
    /// Wall-clock seconds to decode and hash everything (no output is written).
    pub verify_secs: f64,
}

/// Run every combination, calling `on_row` as each finishes.
pub fn run(config: &BenchConfig, mut on_row: impl FnMut(&BenchRow)) -> Result<Vec<BenchRow>> {
    let scratch = tempfile::tempdir().at(std::env::temp_dir())?;
    let target = scratch.path().join("bench.kr");
    let mut rows = Vec::new();
    for &block_log2 in &config.block_log2s {
        for &codec in &config.codecs {
            let options = PackOptions {
                codec,
                block_log2,
                threads: config.threads,
                overwrite: true,
            };
            let started = Instant::now();
            let stats = pack(&target, &config.inputs, &options)?;
            let pack_secs = started.elapsed().as_secs_f64();

            let archive = Archive::open(&target)?;
            let started = Instant::now();
            verify(&archive, config.threads)?;
            let verify_secs = started.elapsed().as_secs_f64();

            let row = BenchRow {
                codec,
                block_log2,
                raw_bytes: stats.raw_bytes,
                archive_bytes: stats.archive_bytes,
                pack_secs,
                verify_secs,
            };
            on_row(&row);
            rows.push(row);
        }
    }
    Ok(rows)
}
