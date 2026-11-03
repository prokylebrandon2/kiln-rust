//! Creating archives.
//!
//! ```text
//!  walk inputs ──> PackSource (reads files, hashes them, cuts the stream into blocks)
//!                      │  bounded, one block per item
//!                      ▼
//!                  N workers: compress block + CRC-32      (parallel)
//!                      │  re-ordered by the pipeline
//!                      ▼
//!                  sink: append record to the temp file, remember its offset
//!  finally: index record + footer, fsync, atomic rename onto the destination
//! ```

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::codec::{CodecSpec, Method};
use crate::error::{Error, IoContext, Result};
use crate::format::index::{Entry, EntryKind, Index};
use crate::format::record::EncodedBlock;
use crate::format::{
    DEFAULT_BLOCK_LOG2, FOOTER_LEN, Footer, HEADER_LEN, Header, MAX_BLOCK_LOG2, MAX_INDEX_RAW,
    MIN_BLOCK_LOG2,
};
use crate::fsutil::{TempOutput, read_full};
use crate::integrity::{FileHash, FileHasher, HASH_LEN, empty_hash};
use crate::parallel::{Config, Source, run_ordered};
use crate::walk::{self, Skipped, SourceEntry};

/// Settings for [`pack`].
#[derive(Clone, Debug)]
pub struct PackOptions {
    /// Which compressor to use and how hard.
    pub codec: CodecSpec,
    /// log2 of the block size (12..=27). Larger blocks compress better; smaller blocks give finer
    /// random access and lower memory use.
    pub block_log2: u8,
    /// Worker threads; `0` means one per available core.
    pub threads: usize,
    /// Replace the destination if it already exists.
    pub overwrite: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            codec: CodecSpec::default_spec(),
            block_log2: DEFAULT_BLOCK_LOG2,
            threads: 0,
            overwrite: false,
        }
    }
}

/// What [`pack`] did.
#[derive(Clone, Debug, Default)]
pub struct PackStats {
    /// Files archived.
    pub files: u64,
    /// Directories archived.
    pub dirs: u64,
    /// Total uncompressed file bytes.
    pub raw_bytes: u64,
    /// Final archive size on disk.
    pub archive_bytes: u64,
    /// Data blocks written.
    pub blocks: u64,
    /// Of those, blocks that did not compress and were stored as-is.
    pub stored_blocks: u64,
    /// Items found but not archived (symlinks, special files).
    pub skipped: Vec<Skipped>,
    /// Files that grew while being read; only their original size was archived.
    pub grew: Vec<PathBuf>,
}

/// Create the archive `output` from `inputs`.
///
/// The archive is built in a temporary file and renamed into place on success, so `output` is
/// either complete or untouched.
pub fn pack(output: &Path, inputs: &[PathBuf], options: &PackOptions) -> Result<PackStats> {
    if inputs.is_empty() {
        return Err(Error::Invalid(
            "nothing to archive: no input paths given".into(),
        ));
    }
    if !(MIN_BLOCK_LOG2..=MAX_BLOCK_LOG2).contains(&options.block_log2) {
        return Err(Error::Invalid(format!(
            "block size must be between {} KiB and {} MiB",
            (1u64 << MIN_BLOCK_LOG2) >> 10,
            (1u64 << MAX_BLOCK_LOG2) >> 20
        )));
    }
    let codec = options.codec.build()?;
    if !options.overwrite && fs::symlink_metadata(output).is_ok() {
        return Err(Error::Exists(output.to_path_buf()));
    }

    let collected = walk::collect(inputs, &[output.to_path_buf()])?;
    let (guard, file) = TempOutput::create(output, options.overwrite)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);

    let header = Header {
        default_method: options.codec.method,
        block_log2: options.block_log2,
    };
    out.write_all(&header.encode()).at(output)?;

    let source = PackSource::new(&collected.entries, 1usize << options.block_log2);
    let mut position = HEADER_LEN as u64;
    let mut block_offsets = Vec::new();
    let mut stats = PackStats::default();

    let source = run_ordered(
        Config::new(options.threads),
        source,
        |raw: Vec<u8>| EncodedBlock::from_raw(codec.as_ref(), raw),
        |block: EncodedBlock| {
            block_offsets.push(position);
            position += block.write_to(&mut out).at(output)?;
            stats.blocks += 1;
            stats.stored_blocks += u64::from(block.header.method == Method::Store);
            Ok(())
        },
    )
    .map_err(Error::from)?;

    // Assemble the index.
    let mut entries = Vec::with_capacity(collected.entries.len());
    let mut total_raw = 0u64;
    for (src, hash) in collected.entries.iter().zip(&source.hashes) {
        total_raw += src.size;
        match src.kind {
            EntryKind::File => stats.files += 1,
            EntryKind::Dir => stats.dirs += 1,
        }
        entries.push(Entry {
            path: src.name.clone(),
            kind: src.kind,
            size: src.size,
            mtime_secs: src.mtime_secs,
            mtime_nanos: src.mtime_nanos,
            mode: src.mode,
            hash: *hash,
        });
    }
    let index = Index {
        entries,
        total_raw,
        block_offsets,
    };
    let index_raw = index.encode();
    if index_raw.len() > MAX_INDEX_RAW as usize {
        return Err(Error::Invalid(
            "too many entries: the index would be too large".into(),
        ));
    }

    // Index record (always zstd: small, and decoded before anything else) and footer.
    let index_codec = CodecSpec::default_spec().build()?;
    let index_block = EncodedBlock::from_raw(index_codec.as_ref(), index_raw)?;
    let index_offset = position;
    position += index_block.write_to(&mut out).at(output)?;
    let footer = Footer {
        index_offset,
        index_record_len: index_block.header.total_len(),
    };
    out.write_all(&footer.encode()).at(output)?;
    position += FOOTER_LEN as u64;

    let file: File = out.into_inner().map_err(|e| Error::Io {
        path: output.to_path_buf(),
        source: e.into_error(),
    })?;
    guard.commit(file)?;

    stats.raw_bytes = total_raw;
    stats.archive_bytes = position;
    stats.skipped = collected.skipped;
    stats.grew = source.grew;
    Ok(stats)
}

/// An open input file while its bytes are being consumed.
struct OpenFile {
    file: File,
    entry: usize,
    remaining: u64,
    hasher: FileHasher,
}

/// Reads the input files in order and cuts their concatenation into fixed-size blocks, hashing
/// every file as it goes. Runs on the pipeline's producer thread.
struct PackSource<'a> {
    entries: &'a [SourceEntry],
    next_entry: usize,
    current: Option<OpenFile>,
    /// BLAKE3 per entry (zero for directories), filled in as files complete.
    hashes: Vec<FileHash>,
    grew: Vec<PathBuf>,
    block_size: usize,
}

impl<'a> PackSource<'a> {
    fn new(entries: &'a [SourceEntry], block_size: usize) -> Self {
        Self {
            entries,
            next_entry: 0,
            current: None,
            hashes: vec![[0u8; HASH_LEN]; entries.len()],
            grew: Vec::new(),
            block_size,
        }
    }

    /// Advance to the next non-empty file; returns false when the input is exhausted.
    fn open_next(&mut self) -> Result<bool> {
        while self.next_entry < self.entries.len() {
            let idx = self.next_entry;
            self.next_entry += 1;
            let entry = &self.entries[idx];
            match entry.kind {
                EntryKind::Dir => {}
                EntryKind::File if entry.size == 0 => self.hashes[idx] = empty_hash(),
                EntryKind::File => {
                    let file = File::open(&entry.fs_path).at(&entry.fs_path)?;
                    self.current = Some(OpenFile {
                        file,
                        entry: idx,
                        remaining: entry.size,
                        hasher: FileHasher::new(),
                    });
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn finish_current(&mut self) -> Result<()> {
        let mut cur = self.current.take().expect("a file is open");
        let path = &self.entries[cur.entry].fs_path;
        let mut probe = [0u8; 1];
        if read_full(&mut cur.file, &mut probe).at(path)? == 1 {
            self.grew.push(path.clone());
        }
        self.hashes[cur.entry] = cur.hasher.finalize();
        Ok(())
    }
}

impl Source for PackSource<'_> {
    type Item = Vec<u8>;
    type Error = Error;

    fn next_item(&mut self) -> Result<Option<Vec<u8>>> {
        let mut block = vec![0u8; self.block_size];
        let mut filled = 0usize;
        while filled < self.block_size {
            if self.current.is_none() && !self.open_next()? {
                break;
            }
            let cur = self.current.as_mut().expect("open_next opened a file");
            let want = cur.remaining.min((self.block_size - filled) as u64) as usize;
            let path = &self.entries[cur.entry].fs_path;
            let got = read_full(&mut cur.file, &mut block[filled..filled + want]).at(path)?;
            if got < want {
                return Err(Error::FileChanged(path.clone()));
            }
            cur.hasher.update(&block[filled..filled + got]);
            cur.remaining -= got as u64;
            filled += got;
            if cur.remaining == 0 {
                self.finish_current()?;
            }
        }
        if filled == 0 {
            return Ok(None);
        }
        block.truncate(filled);
        Ok(Some(block))
    }
}
