//! Creating archives.
//!
//! ```text
//!  walk inputs ──> (optional) classify every file from a 64 KiB sample, in parallel
//!                      │  files that cannot compress are moved behind the compressible ones
//!                      ▼
//!                  PackSource (reads files, hashes them, cuts the stream into chunks,
//!                              never mixing "store" and "compress" data in one chunk)
//!                      │  bounded, one chunk per item
//!                      ▼
//!                  N workers: compress chunk + CRC-32      (parallel)
//!                      │  re-ordered by the pipeline
//!                      ▼
//!                  sink: append record to the temp file, remember its lengths
//!  finally: index record + footer, fsync, atomic rename onto the destination
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::{Progress, pipeline_config};
use crate::classify::{self, Class, SAMPLE_HEAD, SAMPLE_MID};
use crate::codec::{BlockCodec, CodecSpec, Filter, Method, detect_filter, looks_incompressible};
use crate::error::{Error, IoContext, Result};
use crate::format::index::{
    ChunkInfo, Entry, EntryKind, Extension, Index, TAG_CREATOR, dedup_extension,
};
use crate::format::record::EncodedBlock;
use crate::format::{
    FOOTER_LEN, Footer, HEADER_LEN, Header, MAX_BLOCK_LOG2, MAX_ENTRIES, MAX_INDEX_RAW,
    MIN_BLOCK_LOG2,
};
use crate::fsutil::{TempOutput, read_full};
use crate::integrity::{FileHash, FileHasher, HASH_LEN, empty_hash};
use crate::parallel::{MemoryModel, Source, resolve_threads, run_ordered};
use crate::profile::Profile;
use crate::walk::{self, Skipped, SourceEntry};

/// Name written into the index so `info` can say which program made an archive.
const CREATOR: &str = concat!("kiln ", env!("CARGO_PKG_VERSION"));

/// Settings for [`pack`].
#[derive(Clone, Debug)]
pub struct PackOptions {
    /// The profile requested (recorded in the header; [`Profile::Custom`] when `codec` was chosen
    /// by hand).
    pub profile: Profile,
    /// Which compressor to use and how hard.
    pub codec: CodecSpec,
    /// log2 of the maximum chunk size (12..=27). Larger chunks compress better; smaller chunks
    /// give finer random access and lower memory use.
    pub block_log2: u8,
    /// Worker threads; `0` means one per available core.
    pub threads: usize,
    /// Replace the destination if it already exists.
    pub overwrite: bool,
    /// Keep estimated peak memory under this many bytes by lowering threads / in-flight chunks.
    /// `None` = unlimited. It is an estimate (see `docs/PHASE2.md`), not an enforced cap.
    pub memory_limit: Option<u64>,
    /// For slow codecs (zstd >= 10, lzma), store chunks that sampling shows are incompressible
    /// instead of spending seconds failing to compress them.
    pub skip_incompressible: bool,
    /// Sample every file first, store the incompressible ones in their own chunks, and record
    /// each file's content class in the index.
    pub classify: bool,
    /// Store files with identical content once (whole-file deduplication). Files that share
    /// their size with another file are read one extra time to compare content. Archives that
    /// use it need a reader that knows format extension `0x8001`.
    pub dedup: bool,
}

impl PackOptions {
    /// Options for a named profile (everything else at its default).
    pub fn from_profile(profile: Profile) -> Self {
        let s = profile.settings();
        Self {
            profile,
            codec: s.codec,
            block_log2: s.block_log2,
            threads: 0,
            overwrite: false,
            memory_limit: None,
            skip_incompressible: true,
            classify: s.classify,
            dedup: false,
        }
    }
}

impl Default for PackOptions {
    fn default() -> Self {
        Self::from_profile(Profile::Balanced)
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
    /// Size of the index record on disk (header and payload).
    pub index_bytes: u64,
    /// Data chunks written.
    pub blocks: u64,
    /// Of those, chunks stored as-is (incompressible, classified as such, or a `store` archive).
    pub stored_blocks: u64,
    /// Of the stored chunks, those skipped by the incompressibility sampler (never attempted).
    pub precheck_skipped: u64,
    /// Files the classifier routed straight to storage without trying to compress them.
    pub classified_stored_files: u64,
    /// Their total size in bytes.
    pub classified_stored_bytes: u64,
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
    pack_with_progress(output, inputs, options, &mut |_, _| {})
}

/// What flows through the pipeline: raw bytes, and whether they must be stored verbatim.
struct Chunk {
    raw: Vec<u8>,
    store: bool,
    filter: Filter,
}

/// [`pack`], reporting `(raw_bytes_done, raw_bytes_total)` after every chunk is written.
pub fn pack_with_progress(
    output: &Path,
    inputs: &[PathBuf],
    options: &PackOptions,
    progress: Progress<'_>,
) -> Result<PackStats> {
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
    if collected.entries.len() as u64 > MAX_ENTRIES {
        return Err(Error::Invalid(format!(
            "too many entries: {} (the format allows at most {MAX_ENTRIES})",
            collected.entries.len()
        )));
    }

    // Classify (optional), then order: everything that will be compressed first, in walk order,
    // then the files that will be stored verbatim, in walk order.
    let threads = resolve_threads(options.threads);
    let dups = if options.dedup {
        find_duplicates(&collected.entries, threads)?
    } else {
        Vec::new()
    };
    let mut is_dup = vec![false; collected.entries.len()];
    for d in &dups {
        is_dup[d.dup] = true;
    }
    let want_classes = options.classify && options.codec.method != Method::Store;
    let want_filters = options.codec.method == Method::Lzma2;
    let (mut classes, mut filters): (Vec<Class>, Vec<Filter>) = if want_classes || want_filters {
        sample_files(&collected.entries, threads)?
            .into_iter()
            .unzip()
    } else {
        (
            vec![Class::Unknown; collected.entries.len()],
            vec![Filter::None; collected.entries.len()],
        )
    };
    if !want_classes {
        classes.fill(Class::Unknown);
    }
    if !want_filters {
        filters.fill(Filter::None);
    }
    let stored_by_class = |i: usize| classes[i].is_incompressible();
    let mut order: Vec<usize> = (0..collected.entries.len())
        .filter(|&i| !is_dup[i] && !stored_by_class(i))
        .collect();
    // Files that share a filter sit together, so a chunk never mixes filters.
    if options.classify {
        // Similar files (same class, same extension) end up in the same chunk; the sort is
        // stable, so walk order is kept inside a group.
        order.sort_by_cached_key(|&i| {
            let e = &collected.entries[i];
            let plain = e.kind == EntryKind::File && e.size > 0;
            (
                filters[i],
                similarity_key(&e.name, plain.then_some(classes[i])),
            )
        });
    } else {
        order.sort_by_key(|&i| filters[i]);
    }
    order.extend((0..collected.entries.len()).filter(|&i| !is_dup[i] && stored_by_class(i)));
    let ordered: Vec<&SourceEntry> = order.iter().map(|&i| &collected.entries[i]).collect();
    let store_flags: Vec<bool> = order.iter().map(|&i| stored_by_class(i)).collect();
    let chunk_filters: Vec<Filter> = order
        .iter()
        .map(|&i| {
            if stored_by_class(i) {
                Filter::None
            } else {
                filters[i]
            }
        })
        .collect();
    // One encoder per filter in use (LZMA2 only); chunks look theirs up by filter.
    let mut used: Vec<Filter> = chunk_filters.clone();
    used.sort();
    used.dedup();
    used.retain(|f| *f != Filter::None);
    let filtered = used
        .into_iter()
        .map(|f| options.codec.build_filtered(f).map(|c| (f, c)))
        .collect::<Result<Vec<_>>>()?;

    let (guard, file) = TempOutput::create(output, options.overwrite)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let header = Header::new(options.profile, options.block_log2);
    out.write_all(&header.encode()).at(output)?;

    let block_bytes = 1u64 << options.block_log2;
    let config = pipeline_config(
        options.threads,
        options.memory_limit,
        MemoryModel {
            base: 16 << 20,
            per_item: 2 * block_bytes,
            per_thread: options.codec.compress_context_bytes(block_bytes),
        },
        "lower --block-size or use a lighter codec",
    )?;
    let total_bytes: u64 = ordered.iter().map(|e| e.size).sum();
    let mut done_bytes = 0u64;
    let sample = options.skip_incompressible && options.codec.is_heavy();

    let source = PackSource::new(&ordered, &store_flags, &chunk_filters, block_bytes as usize);
    let mut chunks: Vec<ChunkInfo> = Vec::new();
    let mut position = HEADER_LEN as u64;
    let mut stats = PackStats::default();

    let source = run_ordered(
        config,
        source,
        |chunk: Chunk| {
            if chunk.store {
                EncodedBlock::stored(chunk.raw).map(|b| (b, false))
            } else if sample && looks_incompressible(&chunk.raw) {
                EncodedBlock::stored(chunk.raw).map(|b| (b, true))
            } else {
                let codec: &dyn BlockCodec = filtered
                    .iter()
                    .find(|(f, _)| *f == chunk.filter)
                    .map_or(codec.as_ref(), |(_, c)| c.as_ref());
                EncodedBlock::from_raw(codec, chunk.raw).map(|b| (b, false))
            }
        },
        |(block, skipped): (EncodedBlock, bool)| {
            chunks.push(block.info());
            position += block.write_to(&mut out).at(output)?;
            stats.blocks += 1;
            stats.stored_blocks += u64::from(block.header.method == Method::Store);
            stats.precheck_skipped += u64::from(skipped);
            done_bytes += u64::from(block.header.raw_len);
            progress(done_bytes, total_bytes);
            crate::cancel::check()?; // UI-HOOK
            Ok(())
        },
    )
    .map_err(Error::from)?;

    // Assemble the index in data order.
    let mut entries = Vec::with_capacity(ordered.len());
    for (pos, (src, hash)) in ordered.iter().zip(&source.hashes).enumerate() {
        match src.kind {
            EntryKind::File => {
                stats.files += 1;
                if store_flags[pos] {
                    stats.classified_stored_files += 1;
                    stats.classified_stored_bytes += src.size;
                }
            }
            EntryKind::Dir => stats.dirs += 1,
        }
        entries.push(Entry {
            path: src.name.clone(),
            kind: src.kind,
            class: classes[order[pos]],
            size: src.size,
            mtime_secs: src.mtime_secs,
            mtime_nanos: src.mtime_nanos,
            mode: src.mode,
            hash: *hash,
        });
    }
    let mut extensions = vec![Extension {
        tag: TAG_CREATOR,
        data: CREATOR.as_bytes().to_vec(),
    }];
    if !dups.is_empty() {
        // Duplicates come after everything that has data and own no bytes in the stream. Each
        // carries its source's hash, which must still be what was measured beforehand.
        let mut position_of = vec![usize::MAX; collected.entries.len()];
        for (pos, &i) in order.iter().enumerate() {
            position_of[i] = pos;
        }
        let mut pairs = Vec::with_capacity(dups.len());
        for d in &dups {
            let source_pos = position_of[d.source];
            if source.hashes[source_pos] != d.hash {
                return Err(Error::FileChanged(
                    collected.entries[d.source].fs_path.clone(),
                ));
            }
            let copy = &collected.entries[d.dup];
            pairs.push((entries.len(), source_pos));
            stats.files += 1;
            entries.push(Entry {
                path: copy.name.clone(),
                kind: EntryKind::File,
                class: classes[d.dup],
                size: copy.size,
                mtime_secs: copy.mtime_secs,
                mtime_nanos: copy.mtime_nanos,
                mode: copy.mode,
                hash: d.hash,
            });
        }
        extensions.push(dedup_extension(&pairs));
    }
    let index = Index::build(entries, chunks, extensions)?;
    debug_assert_eq!(
        index.data_end, position,
        "chunk table disagrees with the file"
    );
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

    stats.raw_bytes = index
        .entries
        .iter()
        .fold(0u64, |sum, e| sum.saturating_add(e.size));
    stats.archive_bytes = position;
    stats.index_bytes = index_block.header.total_len();
    stats.skipped = collected.skipped;
    stats.grew = source.grew;
    Ok(stats)
}

/// A file whose content is identical to that of an earlier file in walk order.
struct Duplicate {
    /// Index of the duplicate in the walk results.
    dup: usize,
    /// Index of the first file with the same content.
    source: usize,
    /// BLAKE3 of the shared content, as measured now.
    hash: FileHash,
}

/// BLAKE3 of a whole file, read through `buf`.
fn hash_file(path: &Path, buf: &mut [u8]) -> std::io::Result<FileHash> {
    let mut file = File::open(path)?;
    let mut hasher = FileHasher::new();
    loop {
        match file.read(buf) {
            Ok(0) => return Ok(hasher.finalize()),
            Ok(n) => {
                hasher.update(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Find files with identical content. Only files that share their size with another file are
/// read; they are hashed with up to `threads` workers, and the first file in walk order of each
/// group of identical files is its source.
fn find_duplicates(entries: &[SourceEntry], threads: usize) -> Result<Vec<Duplicate>> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.kind == EntryKind::File && e.size > 0 {
            by_size.entry(e.size).or_default().push(i);
        }
    }
    let mut candidates: Vec<usize> = by_size
        .into_values()
        .filter(|group| group.len() > 1)
        .flatten()
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    candidates.sort_unstable();
    let workers = threads.min(candidates.len()).max(1);
    let cursor = AtomicUsize::new(0);
    let results: Vec<Result<Vec<(usize, FileHash)>>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<(usize, FileHash)>> {
                    let mut done = Vec::new();
                    let mut buf = vec![0u8; 1 << 20];
                    loop {
                        let n = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = candidates.get(n) else {
                            return Ok(done);
                        };
                        crate::cancel::check()?;
                        let path = &entries[i].fs_path;
                        done.push((i, hash_file(path, &mut buf).at(path)?));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(Error::Internal("hashing thread panicked".into())))
            })
            .collect()
    });
    let mut hashed: Vec<(usize, FileHash)> = Vec::with_capacity(candidates.len());
    for part in results {
        hashed.extend(part?);
    }
    hashed.sort_unstable_by_key(|(i, _)| *i);
    let mut first: HashMap<(u64, FileHash), usize> = HashMap::new();
    let mut dups = Vec::new();
    for (i, hash) in hashed {
        let key = (entries[i].size, hash);
        match first.get(&key) {
            Some(&source) => dups.push(Duplicate {
                dup: i,
                source,
                hash,
            }),
            None => {
                first.insert(key, i);
            }
        }
    }
    Ok(dups)
}

/// Grouping key for the compressible lane: `(class rank, lower-case extension)`.
///
/// `None` stands for directories and empty files, which keep rank 0 and no extension. The
/// extension is the text after the last `.` of the file-name part of `name`; it is empty when
/// there is no dot or the only dot is the first character.
fn similarity_key(name: &str, class: Option<Class>) -> (u8, String) {
    let Some(class) = class else {
        return (0, String::new());
    };
    let rank = match class {
        Class::Text => 0,
        Class::Data => 1,
        Class::Binary => 2,
        _ => 3,
    };
    let file_name = name.rsplit_once('/').map_or(name, |(_, file)| file);
    let ext = match file_name.rfind('.') {
        Some(dot) if dot > 0 => file_name[dot + 1..].to_lowercase(),
        _ => String::new(),
    };
    (rank, ext)
}

/// Read the sampling windows of one file.
fn read_sample(path: &Path, size: u64) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut file = File::open(path)?;
    let mut head = vec![0u8; size.min(SAMPLE_HEAD as u64) as usize];
    let got = read_full(&mut file, &mut head)?;
    head.truncate(got);
    let mut mid = Vec::new();
    if size > 2 * SAMPLE_HEAD as u64 {
        file.seek(SeekFrom::Start(size / 2))?;
        mid = vec![0u8; SAMPLE_MID];
        let got = read_full(&mut file, &mut mid)?;
        mid.truncate(got);
    }
    Ok((head, mid))
}

/// A file's content class and the pre-filter suited to it.
type Sample = (Class, Filter);

/// Sample every file with up to `threads` workers. Directories and empty files are
/// [`Class::Unknown`] with no filter. Memory: at most 64 KiB of samples per worker.
fn sample_files(entries: &[SourceEntry], threads: usize) -> Result<Vec<Sample>> {
    let files: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].kind == EntryKind::File && entries[i].size > 0)
        .collect();
    let mut samples = vec![(Class::Unknown, Filter::None); entries.len()];
    let workers = threads.min(files.len().div_ceil(64)).max(1);
    let cursor = AtomicUsize::new(0);
    let results: Vec<Result<Vec<(usize, Sample)>>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<(usize, Sample)>> {
                    let mut done = Vec::new();
                    loop {
                        let n = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = files.get(n) else {
                            return Ok(done);
                        };
                        let e = &entries[i];
                        let (head, mid) = read_sample(&e.fs_path, e.size).at(&e.fs_path)?;
                        let class = classify::classify(&head, &mid, e.size);
                        done.push((i, (class, detect_filter(&head))));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(Error::Internal("classifier thread panicked".into())))
            })
            .collect()
    });
    for part in results {
        for (i, sample) in part? {
            samples[i] = sample;
        }
    }
    Ok(samples)
}

/// An open input file while its bytes are being consumed.
struct OpenFile {
    file: File,
    entry: usize,
    remaining: u64,
    hasher: FileHasher,
}

/// Reads the input files in order and cuts their concatenation into chunks, hashing every file
/// as it goes. A chunk ends early where the next file belongs to the other storage group, so a
/// chunk is either all "store" data or all "compress" data. Runs on the pipeline's producer
/// thread.
struct PackSource<'a> {
    entries: &'a [&'a SourceEntry],
    store: &'a [bool],
    filters: &'a [Filter],
    next_entry: usize,
    current: Option<OpenFile>,
    /// BLAKE3 per entry (zero for directories), filled in as files complete.
    hashes: Vec<FileHash>,
    grew: Vec<PathBuf>,
    block_size: usize,
}

impl<'a> PackSource<'a> {
    fn new(
        entries: &'a [&'a SourceEntry],
        store: &'a [bool],
        filters: &'a [Filter],
        block_size: usize,
    ) -> Self {
        Self {
            entries,
            store,
            filters,
            next_entry: 0,
            current: None,
            hashes: vec![[0u8; HASH_LEN]; entries.len()],
            grew: Vec::new(),
            block_size,
        }
    }

    /// Skip directories and empty files (recording the empty hash) and return the index of the
    /// next file that has bytes, without consuming it.
    fn peek_file(&mut self) -> Option<usize> {
        while self.next_entry < self.entries.len() {
            let idx = self.next_entry;
            let entry = self.entries[idx];
            match entry.kind {
                EntryKind::Dir => self.next_entry += 1,
                EntryKind::File if entry.size == 0 => {
                    self.hashes[idx] = empty_hash();
                    self.next_entry += 1;
                }
                EntryKind::File => return Some(idx),
            }
        }
        None
    }

    fn open(&mut self, idx: usize) -> Result<()> {
        let entry = self.entries[idx];
        let file = File::open(&entry.fs_path).at(&entry.fs_path)?;
        self.next_entry = idx + 1;
        self.current = Some(OpenFile {
            file,
            entry: idx,
            remaining: entry.size,
            hasher: FileHasher::new(),
        });
        Ok(())
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
    type Item = Chunk;
    type Error = Error;

    fn next_item(&mut self) -> Result<Option<Chunk>> {
        // Reading appends into spare capacity (`read_to_end`), so the buffer is never zero-filled.
        // (A recycling buffer pool was tried and removed: no speedup, 2.5x the peak memory.)
        let mut block = Vec::with_capacity(self.block_size);
        let mut group: Option<(bool, Filter)> = None;
        while block.len() < self.block_size {
            if self.current.is_none() {
                let Some(idx) = self.peek_file() else { break };
                if group.is_some_and(|g| g != (self.store[idx], self.filters[idx])) {
                    break; // next file belongs to the other group: close this chunk here
                }
                self.open(idx)?;
            }
            let cur = self.current.as_mut().expect("a file is open");
            group = Some((self.store[cur.entry], self.filters[cur.entry]));
            let want = cur.remaining.min((self.block_size - block.len()) as u64);
            let path = &self.entries[cur.entry].fs_path;
            let before = block.len();
            (&mut cur.file)
                .take(want)
                .read_to_end(&mut block)
                .at(path)?;
            let got = block.len() - before;
            if (got as u64) < want {
                return Err(Error::FileChanged(path.clone()));
            }
            cur.hasher.update(&block[before..]);
            cur.remaining -= got as u64;
            if cur.remaining == 0 {
                self.finish_current()?;
            }
        }
        match group {
            None => Ok(None),
            Some((store, filter)) => Ok(Some(Chunk {
                raw: block,
                store,
                filter,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, class: Class) -> (u8, String) {
        similarity_key(name, Some(class))
    }

    #[test]
    fn extension_is_the_lower_cased_text_after_the_last_dot_of_the_file_name() {
        assert_eq!(key("p/Main.RS", Class::Text).1, "rs");
        assert_eq!(key("p/a.tar.GZ", Class::Data).1, "gz");
        assert_eq!(key("p.d/Makefile", Class::Text).1, "");
        assert_eq!(key("p/trailing.", Class::Text).1, "");
        // Dotfiles: the only dot is the first character, so there is no extension.
        assert_eq!(key("p/.gitignore", Class::Text).1, "");
        assert_eq!(key(".bashrc", Class::Text).1, "");
        assert_eq!(key("p/.config.JSON", Class::Text).1, "json");
    }

    #[test]
    fn class_rank_orders_text_data_binary_unknown() {
        let classes = [Class::Text, Class::Data, Class::Binary, Class::Unknown];
        assert_eq!(classes.map(|c| key("f", c).0), [0, 1, 2, 3]);
    }

    #[test]
    fn directories_and_empty_files_have_no_key() {
        assert_eq!(similarity_key("dir/x.txt", None), (0, String::new()));
    }
}
