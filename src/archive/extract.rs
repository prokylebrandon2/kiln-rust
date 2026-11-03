//! Extracting and verifying archives.
//!
//! ```text
//!  BlockSource (reads only the blocks the selected files overlap, seeking over the rest)
//!        │  one raw record per item
//!        ▼
//!  N workers: CRC-32 check + decompress                      (parallel)
//!        │  re-ordered by the pipeline
//!        ▼
//!  Demux sink: slice each block across the files it contains, hash, write, verify BLAKE3
//! ```
//!
//! `verify` is the same pipeline with the writing switched off.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::reader::Archive;
use crate::error::{Error, IoContext, Result};
use crate::format::index::{Entry, EntryKind};
use crate::format::record::{RecordHeader, decode_payload, read_record};
use crate::fsutil::{set_dir_mtime, set_file_mtime, set_mode};
use crate::integrity::{FileHasher, empty_hash, to_hex};
use crate::parallel::{Config, Source, run_ordered};
use crate::path::{normalize_selector, safe_join, selector_matches};

/// Settings for [`extract`].
#[derive(Clone, Debug)]
pub struct ExtractOptions {
    /// Directory to extract into (created if missing).
    pub dest: PathBuf,
    /// Replace existing files instead of failing.
    pub overwrite: bool,
    /// Worker threads; `0` means one per available core.
    pub threads: usize,
    /// Only extract these members (a file, or a directory with everything below it).
    /// Empty means everything.
    pub members: Vec<String>,
}

impl ExtractOptions {
    /// Extract everything into `dest` with default settings.
    pub fn new(dest: impl Into<PathBuf>) -> Self {
        Self {
            dest: dest.into(),
            overwrite: false,
            threads: 0,
            members: Vec::new(),
        }
    }
}

/// What [`extract`] or [`verify`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractStats {
    /// Files completed (and hash-verified).
    pub files: u64,
    /// Directories created or checked.
    pub dirs: u64,
    /// File bytes processed.
    pub bytes: u64,
    /// Data blocks decoded (fewer than the archive holds when extracting a subset).
    pub blocks: u64,
    /// Timestamps or permissions that could not be applied (non-fatal).
    pub metadata_errors: u64,
}

/// Extract (a subset of) `archive` into `options.dest`, verifying every file's BLAKE3 hash.
///
/// Extraction is not transactional: if it fails midway, files written so far remain.
pub fn extract(archive: &Archive, options: &ExtractOptions) -> Result<ExtractStats> {
    let selected = select(archive, &options.members)?;
    let output = Output {
        dest: &options.dest,
        overwrite: options.overwrite,
    };
    fs::create_dir_all(&options.dest).at(&options.dest)?;
    run(archive, &selected, Some(output), options.threads)
}

/// Decode every block and check every checksum without writing anything.
pub fn verify(archive: &Archive, threads: usize) -> Result<ExtractStats> {
    let all: Vec<usize> = (0..archive.entries().len()).collect();
    run(archive, &all, None, threads)
}

/// Resolve user-supplied member names to entry indices (ascending = stream order).
fn select(archive: &Archive, members: &[String]) -> Result<Vec<usize>> {
    let entries = archive.entries();
    if members.is_empty() {
        return Ok((0..entries.len()).collect());
    }
    let mut chosen = BTreeSet::new();
    for member in members {
        let selector = normalize_selector(member);
        let matched: Vec<usize> = (0..entries.len())
            .filter(|&i| selector.is_empty() || selector_matches(&entries[i].path, &selector))
            .collect();
        if matched.is_empty() && !selector.is_empty() {
            return Err(Error::Invalid(format!(
                "no such member in the archive: {member}"
            )));
        }
        chosen.extend(matched);
    }
    Ok(chosen.into_iter().collect())
}

struct Output<'a> {
    dest: &'a Path,
    overwrite: bool,
}

fn run(
    archive: &Archive,
    selected: &[usize],
    output: Option<Output<'_>>,
    threads: usize,
) -> Result<ExtractStats> {
    let block_size = archive.header.block_size();
    let offsets = archive.index.stream_offsets();

    // Which blocks do the selected files touch?
    let mut needed = vec![false; archive.block_count()];
    let mut targets = Vec::new();
    for &i in selected {
        let e = &archive.index.entries[i];
        if e.kind == EntryKind::File && e.size > 0 {
            let (start, end) = (offsets[i], offsets[i] + e.size);
            targets.push(Target {
                entry: i,
                start,
                end,
            });
            for b in start / block_size..=(end - 1) / block_size {
                needed[b as usize] = true;
            }
        }
    }
    let block_list: Vec<u32> = needed
        .iter()
        .enumerate()
        .filter(|(_, n)| **n)
        .map(|(b, _)| b as u32)
        .collect();

    let mut demux = Demux {
        entries: &archive.index.entries,
        selected,
        targets,
        next: 0,
        open: None,
        block_size,
        output,
        stats: ExtractStats::default(),
    };
    demux.prepare()?;

    if !block_list.is_empty() {
        let file = File::open(&archive.path).at(&archive.path)?;
        let source = BlockSource {
            reader: BufReader::with_capacity(1 << 16, file),
            path: &archive.path,
            list: block_list,
            cursor: 0,
            position: u64::MAX,
            offsets: &archive.index.block_offsets,
            data_end: archive.data_end,
            block_size,
            total_raw: archive.index.total_raw,
        };
        run_ordered(
            Config::new(threads),
            source,
            |raw: RawBlock| {
                let index = raw.index;
                decode_payload(&raw.header, raw.payload)
                    .map(|data| DecodedBlock { index, data })
                    .map_err(|e| in_block(e, index))
            },
            |block: DecodedBlock| demux.push(block.index, &block.data),
        )
        .map_err(Error::from)?;
    }
    demux.finish()
}

fn in_block(e: Error, index: u32) -> Error {
    match e {
        Error::Checksum(m) => Error::Checksum(format!("block {index}: {m}")),
        Error::Codec { .. } => Error::corrupt(format!("block {index}: {e}")),
        other => other,
    }
}

struct RawBlock {
    index: u32,
    header: RecordHeader,
    payload: Vec<u8>,
}

struct DecodedBlock {
    index: u32,
    data: Vec<u8>,
}

/// Reads the needed block records in order, seeking over blocks nobody asked for.
struct BlockSource<'a> {
    reader: BufReader<File>,
    path: &'a Path,
    list: Vec<u32>,
    cursor: usize,
    /// Where the reader currently is, or `u64::MAX` when unknown.
    position: u64,
    offsets: &'a [u64],
    data_end: u64,
    block_size: u64,
    total_raw: u64,
}

impl Source for BlockSource<'_> {
    type Item = RawBlock;
    type Error = Error;

    fn next_item(&mut self) -> Result<Option<RawBlock>> {
        let Some(&index) = self.list.get(self.cursor) else {
            return Ok(None);
        };
        self.cursor += 1;
        let b = index as usize;
        let offset = self.offsets[b];
        let limit = self.offsets.get(b + 1).copied().unwrap_or(self.data_end);
        if self.position != offset {
            self.reader.seek(SeekFrom::Start(offset)).at(self.path)?;
        }
        let (header, payload) =
            read_record(&mut self.reader, self.block_size as u32, limit - offset)?;
        self.position = offset + header.total_len();
        let expected = (self.total_raw - u64::from(index) * self.block_size).min(self.block_size);
        if u64::from(header.raw_len) != expected {
            return Err(Error::corrupt(format!(
                "block {index} has an unexpected size"
            )));
        }
        Ok(Some(RawBlock {
            index,
            header,
            payload,
        }))
    }
}

/// A file's byte range within the data stream.
#[derive(Clone, Copy)]
struct Target {
    entry: usize,
    start: u64,
    end: u64,
}

struct OpenTarget {
    file: Option<File>,
    path: PathBuf,
    hasher: FileHasher,
}

/// Splits the ordered stream of decoded blocks back into files.
struct Demux<'a> {
    entries: &'a [Entry],
    selected: &'a [usize],
    targets: Vec<Target>,
    next: usize,
    open: Option<OpenTarget>,
    block_size: u64,
    output: Option<Output<'a>>,
    stats: ExtractStats,
}

impl Demux<'_> {
    /// Create directories and empty files, and check the hash of empty files.
    fn prepare(&mut self) -> Result<()> {
        let entries = self.entries;
        for &i in self.selected {
            let e = &entries[i];
            match e.kind {
                EntryKind::Dir => {
                    if let Some(out) = &self.output {
                        let path = safe_join(out.dest, &e.path)?;
                        fs::create_dir_all(&path).at(&path)?;
                    }
                    self.stats.dirs += 1;
                }
                EntryKind::File if e.size == 0 => {
                    if e.hash != empty_hash() {
                        return Err(Error::Checksum(format!(
                            "{}: empty file has a wrong hash",
                            e.path
                        )));
                    }
                    if let Some(out) = &self.output {
                        let path = safe_join(out.dest, &e.path)?;
                        let file = create_file(&path, out.overwrite)?;
                        self.stats.metadata_errors += apply_file_metadata(&file, &path, e);
                    }
                    self.stats.files += 1;
                }
                EntryKind::File => {}
            }
        }
        Ok(())
    }

    fn begin(&mut self, entry: usize) -> Result<()> {
        let e = &self.entries[entry];
        let (file, path) = match &self.output {
            Some(out) => {
                let path = safe_join(out.dest, &e.path)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).at(parent)?;
                }
                (Some(create_file(&path, out.overwrite)?), path)
            }
            None => (None, PathBuf::from(&e.path)),
        };
        self.open = Some(OpenTarget {
            file,
            path,
            hasher: FileHasher::new(),
        });
        Ok(())
    }

    fn complete(&mut self, entry: usize) -> Result<()> {
        let e = &self.entries[entry];
        let open = self.open.take().expect("a file is open");
        if open.hasher.finalize() != e.hash {
            return Err(Error::Checksum(format!(
                "{}: content does not match its stored BLAKE3 hash {}",
                e.path,
                to_hex(&e.hash)
            )));
        }
        if let Some(file) = &open.file {
            self.stats.metadata_errors += apply_file_metadata(file, &open.path, e);
        }
        self.stats.files += 1;
        self.stats.bytes += e.size;
        Ok(())
    }

    /// Consume one decoded block. Blocks arrive in increasing order; gaps only occur where no
    /// selected file has data.
    fn push(&mut self, block_index: u32, data: &[u8]) -> Result<()> {
        self.stats.blocks += 1;
        let b0 = u64::from(block_index) * self.block_size;
        let b1 = b0 + data.len() as u64;
        while self.next < self.targets.len() {
            let t = self.targets[self.next];
            if t.start >= b1 {
                break;
            }
            if t.end <= b0 || (self.open.is_none() && t.start < b0) {
                return Err(Error::Internal("a needed block was skipped".into()));
            }
            if self.open.is_none() {
                self.begin(t.entry)?;
            }
            let (s, e) = (t.start.max(b0), t.end.min(b1));
            let slice = &data[(s - b0) as usize..(e - b0) as usize];
            let open = self.open.as_mut().expect("opened above");
            open.hasher.update(slice);
            if let Some(file) = &mut open.file {
                file.write_all(slice).at(&open.path)?;
            }
            if e < t.end {
                break; // the file continues in the next block
            }
            self.complete(t.entry)?;
            self.next += 1;
        }
        Ok(())
    }

    /// Check nothing is left over and apply directory metadata (children first).
    fn finish(mut self) -> Result<ExtractStats> {
        if self.next != self.targets.len() || self.open.is_some() {
            return Err(Error::corrupt(
                "archive data ended before every file was complete",
            ));
        }
        if let Some(out) = &self.output {
            let mut dirs: Vec<&Entry> = self
                .selected
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|e| e.kind == EntryKind::Dir)
                .collect();
            dirs.sort_by(|a, b| b.path.cmp(&a.path));
            let mut errors = 0;
            for d in dirs {
                let path = safe_join(out.dest, &d.path)?;
                errors += set_dir_mtime(&path, d.mtime_secs, d.mtime_nanos).is_err() as u64;
                errors += set_mode(&path, d.mode).is_err() as u64;
            }
            self.stats.metadata_errors += errors;
        }
        Ok(self.stats)
    }
}

/// Apply timestamp and permissions; returns how many of the two failed (non-fatal).
fn apply_file_metadata(file: &File, path: &Path, e: &Entry) -> u64 {
    u64::from(set_file_mtime(file, e.mtime_secs, e.mtime_nanos).is_err())
        + u64::from(set_mode(path, e.mode).is_err())
}

/// Create an output file. Refuses to clobber unless `overwrite`; when overwriting, an existing
/// file or symlink is removed first so a pre-planted link is replaced, never followed.
fn create_file(path: &Path, overwrite: bool) -> Result<File> {
    if overwrite {
        match fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => {
                return Err(Error::Invalid(format!(
                    "cannot replace directory {} with a file",
                    path.display()
                )));
            }
            Ok(_) => fs::remove_file(path).at(path)?,
            Err(_) => {}
        }
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Exists(path.to_path_buf())
            } else {
                Error::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })
}
