//! Extracting and verifying archives.
//!
//! ```text
//!  BlockSource (reads only the chunks the selected files overlap, seeking over the rest;
//!        │      every record header is cross-checked against the index)
//!        │  one raw record per item
//!        ▼
//!  N workers: CRC-32 check + decompress                      (parallel)
//!        │  re-ordered by the pipeline
//!        ▼
//!  Demux sink: slice each chunk across the files it contains
//!        ├─ large file  -> streamed to a temp file right here, hashed, renamed into place
//!        └─ small file  -> assembled in memory, handed to one of W writer threads chosen by the
//!                          file's parent directory; the writer verifies BLAKE3 *before* it
//!                          creates anything, then writes, sets metadata and renames
//!  afterwards (serial): hard links, then symbolic links, then directory metadata
//! ```
//!
//! Links never go through the pipeline: a hard link owns no bytes and a symbolic link has none.
//! They are created once every file is in place, which also means no file can ever be written
//! *through* a link made by the archive (the index refuses entries below a symbolic link, and
//! directories are checked for links before anything is created in them).
//!
//! `verify` is the same pipeline with the writing switched off.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufReader, Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};

use super::reader::Archive;
use super::{Progress, pipeline_config};
use crate::codec::CodecSpec;
use crate::error::{Error, IoContext, Result};
use crate::format::index::{ChunkInfo, Entry, EntryKind, Index, Link};
use crate::format::record::{RecordHeader, decode_payload, read_record};
use crate::fsutil::{
    LinkOutcome, TempOutput, create_hard_link, create_symlink, read_full, set_dir_mtime,
    set_file_mode, set_file_mtime, set_mode, set_owner, set_owner_path, set_xattr,
};
use crate::integrity::{FileHasher, empty_hash, to_hex};
use crate::parallel::{MemoryModel, Source, resolve_threads, run_ordered};
use crate::path::{normalize_selector, safe_join, selector_matches};

/// Files up to this size are assembled in memory and written by the writer pool; larger files
/// are streamed by the sink thread (their per-file overhead is already amortised).
const SMALL_FILE_MAX: u64 = 256 << 10;
/// Most bytes of small files that may be queued or in progress at once.
const SMALL_BUDGET_MAX: u64 = 16 << 20;
/// Every queued job is charged at least this much against the budget, so a flood of tiny or
/// empty files cannot make the (unbounded) queues grow without limit.
const MIN_JOB_CHARGE: u64 = 4 << 10;
/// Most writer threads. Directory and journal locks make more of them counter-productive.
const MAX_WRITERS: usize = 8;

/// Which extended attributes [`extract`] restores.
///
/// Attributes outside the `user.` namespace carry security meaning (`security.capability` grants
/// privileges to an executable, `security.selinux` sets a label, `system.posix_acl_*` are ACLs),
/// so restoring them is opt-in even when the archive holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XattrMode {
    /// Restore none.
    Off,
    /// Restore the `user.` attributes only.
    #[default]
    User,
    /// Restore every attribute in the archive (attributes the process may not set are counted
    /// in [`ExtractStats::metadata_errors`]).
    All,
}

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
    /// Keep estimated peak memory under this many bytes by lowering threads / in-flight blocks.
    pub memory_limit: Option<u64>,
    /// Refuse to extract if the selected files add up to more than this many bytes. Guards
    /// against decompression bombs: a tiny archive can legitimately describe terabytes.
    pub max_bytes: Option<u64>,
    /// Restore symbolic links and hard links (the default). When off, symbolic links are
    /// skipped and hard links are extracted as independent files.
    pub links: bool,
    /// Restore the numeric owner and group recorded in the archive (Unix; usually needs root).
    /// Failures are counted in [`ExtractStats::metadata_errors`]. Off by default.
    pub owner: bool,
    /// Which recorded extended attributes to restore (default: the `user.` ones).
    pub xattrs: XattrMode,
}

impl ExtractOptions {
    /// Extract everything into `dest` with default settings.
    pub fn new(dest: impl Into<PathBuf>) -> Self {
        Self {
            dest: dest.into(),
            overwrite: false,
            threads: 0,
            members: Vec::new(),
            memory_limit: None,
            max_bytes: None,
            links: true,
            owner: false,
            xattrs: XattrMode::default(),
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
    /// Timestamps, permissions, ownership or attributes that could not be applied (non-fatal).
    pub metadata_errors: u64,
    /// Symbolic links created (or, for [`verify`], checked). Not counted in `files`.
    pub symlinks: u64,
    /// Files created as hard links of another extracted file; they are counted in `files` too.
    pub hard_links: u64,
    /// Of the hard links, those that had to be written as copies because the file system
    /// refuses hard links.
    pub hard_link_copies: u64,
}

/// Extract (a subset of) `archive` into `options.dest`, verifying every file's BLAKE3 hash.
///
/// Extraction is not transactional: if it fails midway, files written so far remain.
pub fn extract(archive: &Archive, options: &ExtractOptions) -> Result<ExtractStats> {
    extract_with_progress(archive, options, &mut |_, _| {})
}

/// [`extract`], reporting `(bytes_done, bytes_total)` of decoded chunk data as it goes.
pub fn extract_with_progress(
    archive: &Archive,
    options: &ExtractOptions,
    progress: Progress<'_>,
) -> Result<ExtractStats> {
    let selected = select(archive, &options.members)?;
    if let Some(limit) = options.max_bytes {
        let total: u64 = selected
            .iter()
            .map(|&i| archive.index.entries[i].size)
            .fold(0u64, |a, b| a.saturating_add(b));
        if total > limit {
            return Err(Error::Invalid(format!(
                "refusing to extract {total} bytes: more than the limit of {limit} bytes \
                 (raise it with --max-size if this archive is trusted)"
            )));
        }
    }
    let output = Output {
        dest: &options.dest,
        overwrite: options.overwrite,
        links: options.links,
        owner: options.owner,
        xattrs: options.xattrs,
    };
    fs::create_dir_all(&options.dest).at(&options.dest)?;
    run(
        archive,
        &selected,
        Some(output),
        options.threads,
        options.memory_limit,
        progress,
    )
}

/// Decode every chunk and check every checksum without writing anything.
pub fn verify(archive: &Archive, threads: usize) -> Result<ExtractStats> {
    verify_with(archive, threads, None)
}

/// [`verify`] with an optional memory limit (see [`ExtractOptions::memory_limit`]).
pub fn verify_with(
    archive: &Archive,
    threads: usize,
    memory_limit: Option<u64>,
) -> Result<ExtractStats> {
    verify_with_progress(archive, threads, memory_limit, &mut |_, _| {})
}

/// [`verify_with`], reporting `(bytes_done, bytes_total)` of decoded chunk data as it goes.
pub fn verify_with_progress(
    archive: &Archive,
    threads: usize,
    memory_limit: Option<u64>,
    progress: Progress<'_>,
) -> Result<ExtractStats> {
    let all: Vec<usize> = (0..archive.entries().len()).collect();
    run(archive, &all, None, threads, memory_limit, progress)
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
    links: bool,
    owner: bool,
    xattrs: XattrMode,
}

/// What to restore besides contents, permissions and modification time.
#[derive(Clone, Copy)]
struct Restore<'a> {
    index: &'a Index,
    owner: bool,
    xattrs: XattrMode,
}

impl Restore<'_> {
    /// Ownership and attributes of entry `entry`, which is the open file `file` currently at
    /// `path`. Returns how many could not be applied.
    fn file(&self, entry: usize, file: &File, path: &Path) -> u64 {
        let mut errors = 0;
        if self.owner
            && let Some((uid, gid)) = self.index.owner(entry)
        {
            errors += u64::from(set_owner(file, uid, gid).is_err());
        }
        errors + self.attributes(entry, path)
    }

    /// The same for something reached by name that must not be followed (a directory or a
    /// symbolic link).
    fn path(&self, entry: usize, path: &Path) -> u64 {
        let mut errors = 0;
        if self.owner
            && let Some((uid, gid)) = self.index.owner(entry)
        {
            errors += u64::from(set_owner_path(path, uid, gid).is_err());
        }
        errors + self.attributes(entry, path)
    }

    fn attributes(&self, entry: usize, path: &Path) -> u64 {
        let mut errors = 0;
        for a in self.index.xattrs_of(entry) {
            let wanted = match self.xattrs {
                XattrMode::Off => false,
                XattrMode::User => a.name.starts_with(b"user."),
                XattrMode::All => true,
            };
            if wanted && set_xattr(path, &a.name, &a.value).is_err() {
                errors += 1;
            }
        }
        errors
    }
}

// ---------------------------------------------------------------------------------------------
// Directory handling
// ---------------------------------------------------------------------------------------------

/// Creates output directories and remembers which ones it has already checked.
///
/// Compared with calling `create_dir_all` per file this saves a syscall or two per file, and it
/// adds a safety property: **an existing symbolic link inside the destination tree is refused**
/// instead of followed, so a link planted beforehand cannot redirect extraction elsewhere. (The
/// destination root itself may be a link; the caller chose it.)
struct DirCache {
    root: PathBuf,
    known: HashSet<PathBuf>,
}

impl DirCache {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            known: HashSet::new(),
        }
    }

    /// Make sure `dir` (inside the root) exists as a real directory, creating missing parents.
    fn ensure(&mut self, dir: &Path) -> Result<()> {
        if dir == self.root || self.known.contains(dir) {
            return Ok(());
        }
        let mut missing: Vec<&Path> = Vec::new();
        let mut cur = dir;
        loop {
            if cur == self.root || self.known.contains(cur) {
                break;
            }
            match fs::symlink_metadata(cur) {
                Ok(m) if m.is_dir() => {
                    self.known.insert(cur.to_path_buf());
                    break;
                }
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(Error::UnsafePath(format!(
                        "{} is a symbolic link inside the destination; refusing to follow it",
                        cur.display()
                    )));
                }
                Ok(_) => {
                    return Err(Error::Invalid(format!(
                        "{} exists and is not a directory",
                        cur.display()
                    )));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(cur);
                    cur = cur.parent().ok_or_else(|| {
                        Error::Internal("directory outside the destination".into())
                    })?;
                }
                Err(e) => {
                    return Err(Error::Io {
                        path: cur.to_path_buf(),
                        source: e,
                    });
                }
            }
        }
        for d in missing.into_iter().rev() {
            match fs::create_dir(d) {
                Ok(()) => {}
                // Another writer thread may have created it a moment ago; make sure that it is
                // a real directory and not something planted in between.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()) {
                        return Err(Error::UnsafePath(format!(
                            "{} is not a plain directory",
                            d.display()
                        )));
                    }
                }
                Err(e) => {
                    return Err(Error::Io {
                        path: d.to_path_buf(),
                        source: e,
                    });
                }
            }
            self.known.insert(d.to_path_buf());
        }
        if self.known.len() > 200_000 {
            self.known.clear(); // bound the cache; correctness never depends on it
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Parallel writing of small files
// ---------------------------------------------------------------------------------------------

/// A counting gate that bounds how many bytes of small files are buffered at once.
///
/// The fast path is a lock-free compare-and-swap on an atomic counter. A mutex and condition
/// variable are used only when a thread must actually wait. (An earlier version locked a mutex
/// for every file on both the sink and the writers; the contention showed up as ~37,000 futex
/// calls per 8,000-file extraction and cost more than the parallelism won back.)
struct Budget {
    used: AtomicU64,
    waiters: AtomicU32,
    lock: Mutex<()>,
    freed: Condvar,
    cap: u64,
}

impl Budget {
    fn new(cap: u64) -> Self {
        Self {
            used: AtomicU64::new(0),
            waiters: AtomicU32::new(0),
            lock: Mutex::new(()),
            freed: Condvar::new(),
            cap,
        }
    }

    /// Take `n` bytes if they fit right now. A request larger than the whole cap is admitted
    /// when nothing else is in flight, so progress is always possible.
    fn try_acquire(&self, n: u64) -> bool {
        let mut cur = self.used.load(Ordering::SeqCst);
        loop {
            if cur > 0 && cur + n > self.cap {
                return false;
            }
            match self
                .used
                .compare_exchange_weak(cur, cur + n, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Block until `n` more bytes fit.
    fn acquire(&self, n: u64) {
        if self.try_acquire(n) {
            return;
        }
        let mut guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.waiters.fetch_add(1, Ordering::SeqCst);
        // Re-check after registering as a waiter: a release that happened before this point is
        // seen here, and one that happens after it will notify (it checks `waiters`).
        // Hysteresis: once we have had to wait, wait until the budget has drained to half, then
        // refill in one burst. Admitting one file per wake-up made the sink and the writers
        // ping-pong a futex pair per file (measured: docs/PHASE3.md).
        let low = self.cap / 2;
        while !(self.used.load(Ordering::SeqCst) <= low && self.try_acquire(n)) {
            // The timeout is belt and braces against a missed wake-up; it never decides
            // correctness.
            guard = self
                .freed
                .wait_timeout(guard, std::time::Duration::from_millis(100))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        self.waiters.fetch_sub(1, Ordering::SeqCst);
    }

    fn release(&self, n: u64) {
        let after = self.used.fetch_sub(n, Ordering::SeqCst) - n;
        if after <= self.cap / 2 && self.waiters.load(Ordering::SeqCst) > 0 {
            // Taking the lock orders this notify after the waiter has actually started waiting.
            let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
            self.freed.notify_all();
        }
    }
}

/// State shared between the sink and the writer threads.
struct Shared {
    budget: Budget,
    failed: AtomicBool,
    error: Mutex<Option<Error>>,
    files: AtomicU64,
    bytes: AtomicU64,
    metadata_errors: AtomicU64,
}

impl Shared {
    fn new(budget: u64) -> Self {
        Self {
            budget: Budget::new(budget),
            failed: AtomicBool::new(false),
            error: Mutex::new(None),
            files: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            metadata_errors: AtomicU64::new(0),
        }
    }

    fn fail(&self, e: Error) {
        let mut slot = self.error.lock().unwrap_or_else(|p| p.into_inner());
        slot.get_or_insert(e);
        self.failed.store(true, Ordering::SeqCst);
    }

    fn take_error(&self) -> Option<Error> {
        self.error.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
}

struct Job {
    entry: usize,
    data: Vec<u8>,
}

/// One writer thread: verify, create, write, set metadata, rename - for the files it is sent.
fn writer_loop(
    rx: &Receiver<Vec<Job>>,
    shared: &Shared,
    entries: &[Entry],
    dest: &Path,
    overwrite: bool,
    restore: Restore<'_>,
) {
    let mut dirs = DirCache::new(dest);
    for job in rx.iter().flatten() {
        let size = job.data.len() as u64;
        let charge = size.max(MIN_JOB_CHARGE);
        if !shared.failed.load(Ordering::SeqCst) {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                write_small(
                    &entries[job.entry],
                    job.entry,
                    &job.data,
                    dest,
                    overwrite,
                    &mut dirs,
                    restore,
                )
            }));
            match outcome {
                Ok(Ok(meta_errors)) => {
                    shared.files.fetch_add(1, Ordering::Relaxed);
                    shared.bytes.fetch_add(size, Ordering::Relaxed);
                    shared
                        .metadata_errors
                        .fetch_add(meta_errors, Ordering::Relaxed);
                }
                Ok(Err(e)) => shared.fail(e),
                Err(_) => shared.fail(Error::Internal("a writer thread panicked".into())),
            }
        }
        shared.budget.release(charge);
    }
}

/// Write one fully buffered file. The BLAKE3 hash is checked first, so a corrupt file never
/// touches the file system.
fn write_small(
    e: &Entry,
    entry: usize,
    data: &[u8],
    dest: &Path,
    overwrite: bool,
    dirs: &mut DirCache,
    restore: Restore<'_>,
) -> Result<u64> {
    let mut hasher = FileHasher::new();
    hasher.update(data);
    check_hash(e, hasher.finalize())?;
    let path = safe_join(dest, &e.path)?;
    if let Some(parent) = path.parent() {
        dirs.ensure(parent)?;
    }
    if overwrite && fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
        return Err(Error::Invalid(format!(
            "cannot replace directory {} with a file",
            path.display()
        )));
    }
    if overwrite {
        // Replacing an existing file must be atomic, so go through a temporary name.
        let (temp, mut file) = TempOutput::create(&path, true)?;
        file.write_all(data).at(temp.temp_path())?;
        let errors = apply_file_metadata(&file, temp.temp_path(), e, entry, &restore);
        temp.commit_fast(file)?;
        return Ok(errors);
    }
    // The content is already verified and in memory, so create the final name directly and
    // exclusively (`create_new` fails if anything - including a symlink - is there). This is
    // two directory operations cheaper per file than temp + link + unlink. If writing fails the
    // half-written file is removed; only a crash in the middle of a write can leave one behind.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Exists(path.clone())
            } else {
                Error::Io {
                    path: path.clone(),
                    source: err,
                }
            }
        })?;
    if let Err(err) = file.write_all(data) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(Error::Io { path, source: err });
    }
    let mut errors = restore.file(entry, &file, &path);
    errors += u64::from(set_file_mtime(&file, e.mtime_secs, e.mtime_nanos).is_err());
    errors += u64::from(set_file_mode(&file, e.mode).is_err());
    Ok(errors)
}

fn check_hash(e: &Entry, got: crate::integrity::FileHash) -> Result<()> {
    if got != e.hash {
        return Err(Error::Checksum(format!(
            "{}: content does not match its stored BLAKE3 hash {}",
            e.path,
            to_hex(&e.hash)
        )));
    }
    Ok(())
}

/// Sends small files to the writer threads and bounds what is buffered.
///
/// Jobs are collected per writer and sent in batches (at the end of every decoded chunk): one
/// queue operation and one thread wake-up per batch instead of per file. Measured at ~37,000
/// futex calls per 8,000-file extraction without batching (docs/PHASE3.md).
struct Dispatcher<'a> {
    senders: Vec<Sender<Vec<Job>>>,
    pending: Vec<Vec<Job>>,
    shared: &'a Shared,
}

impl<'a> Dispatcher<'a> {
    fn new(senders: Vec<Sender<Vec<Job>>>, shared: &'a Shared) -> Self {
        let pending = senders.iter().map(|_| Vec::new()).collect();
        Self {
            senders,
            pending,
            shared,
        }
    }

    /// Files in the same directory always go to the same writer: creating files in one directory
    /// serialises on that directory's lock anyway, while different directories proceed in
    /// parallel (measured in docs/PHASE3.md).
    fn lane(&self, path: &str) -> usize {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        let mut h = DefaultHasher::new();
        parent.hash(&mut h);
        (h.finish() % self.senders.len() as u64) as usize
    }

    fn failure(&self) -> Error {
        self.shared
            .take_error()
            .unwrap_or_else(|| Error::Internal("a writer failed".into()))
    }

    /// Hand every collected job to its writer.
    fn flush(&mut self) -> Result<()> {
        for (tx, batch) in self.senders.iter().zip(&mut self.pending) {
            if !batch.is_empty() && tx.send(std::mem::take(batch)).is_err() {
                return Err(Error::Internal("writer pool stopped early".into()));
            }
        }
        Ok(())
    }

    fn send(&mut self, entry: usize, path: &str, data: Vec<u8>) -> Result<()> {
        if self.shared.failed.load(Ordering::SeqCst) {
            return Err(self.failure());
        }
        let charge = (data.len() as u64).max(MIN_JOB_CHARGE);
        if !self.shared.budget.try_acquire(charge) {
            // About to wait for writers to free memory: they must be holding all our work, or
            // we would wait for jobs we never delivered.
            self.flush()?;
            self.shared.budget.acquire(charge);
        }
        let lane = self.lane(path);
        self.pending[lane].push(Job { entry, data });
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The pipeline
// ---------------------------------------------------------------------------------------------

fn run(
    archive: &Archive,
    chosen: &[usize],
    output: Option<Output<'_>>,
    threads: usize,
    memory_limit: Option<u64>,
    progress: Progress<'_>,
) -> Result<ExtractStats> {
    let block_size = archive.header.block_size();
    // Small-file buffering shares the memory limit: a sixteenth of it, between 1 and 16 MiB.
    let budget = memory_limit.map_or(SMALL_BUDGET_MAX, |l| {
        (l / 16).clamp(1 << 20, SMALL_BUDGET_MAX)
    });
    let config = pipeline_config(
        threads,
        memory_limit,
        MemoryModel {
            base: (16u64 << 20) + if output.is_some() { budget } else { 0 },
            per_item: 2 * block_size,
            per_thread: CodecSpec::decompress_context_bytes(block_size),
        },
        "this archive's chunk size sets the floor; the limit cannot go lower",
    )?;
    let index = &archive.index;
    let restore = output.as_ref().map(|o| Restore {
        index,
        owner: o.owner,
        xattrs: o.xattrs,
    });

    // Split off the links. A symbolic link has no data and a hard link shares its source's, so
    // neither goes through the pipeline; they are created afterwards. A hard link whose source
    // is not being extracted (or when links are switched off) is simply extracted as a file.
    let links_wanted = output.as_ref().is_some_and(|o| o.links);
    let mut is_chosen = vec![false; if links_wanted { index.entries.len() } else { 0 }];
    if links_wanted {
        for &i in chosen {
            is_chosen[i] = true;
        }
    }
    let mut selected: Vec<usize> = Vec::with_capacity(chosen.len());
    let mut symlinks: Vec<usize> = Vec::new();
    let mut hard_links: Vec<usize> = Vec::new();
    let mut verified_symlinks = 0u64;
    for &i in chosen {
        match index.link(i) {
            Some(Link::Symlink(_)) if output.is_none() => verified_symlinks += 1,
            Some(Link::Symlink(_)) if links_wanted => symlinks.push(i),
            Some(Link::Symlink(_)) => {} // links switched off: nothing to write
            Some(&Link::HardLink { source }) if links_wanted && is_chosen[source] => {
                hard_links.push(i);
            }
            _ => selected.push(i),
        }
    }
    let selected = &selected[..];

    // Which chunks do the selected files touch?
    let mut needed = vec![false; archive.block_count()];
    // Deduplicated copies of a file share one region of the stream (equal `entry_starts`): it is
    // decoded once, for the first selected copy, and the other copies are written from it.
    let mut regions: BTreeMap<u64, (Target, Vec<usize>)> = BTreeMap::new();
    for &i in selected {
        let e = &index.entries[i];
        if e.kind == EntryKind::File && e.size > 0 {
            let start = index.entry_starts[i];
            if let Some((_, copies)) = regions.get_mut(&start) {
                copies.push(i);
                continue;
            }
            let end = start + e.size; // validated: cannot overflow (sum of sizes == total_raw)
            let target = Target {
                entry: i,
                start,
                end,
            };
            regions.insert(start, (target, Vec::new()));
            let (first, last) = (
                index.chunk_containing(start),
                index.chunk_containing(end - 1),
            );
            needed[first..=last].iter_mut().for_each(|n| *n = true);
        }
    }
    let (targets, aliases): (Vec<Target>, Vec<Vec<usize>>) = regions.into_values().unzip();
    let block_list: Vec<u32> = needed
        .iter()
        .enumerate()
        .filter(|(_, n)| **n)
        .map(|(b, _)| b as u32)
        .collect();
    let total_bytes: u64 = block_list
        .iter()
        .map(|&b| u64::from(index.chunks[b as usize].raw_len))
        .sum();
    let mut done_bytes = 0u64;

    let writers = if output.is_some() {
        resolve_threads(threads).clamp(1, MAX_WRITERS)
    } else {
        0
    };
    let shared = Shared::new(budget);

    thread::scope(|scope| -> Result<ExtractStats> {
        let mut senders = Vec::new();
        let mut handles = Vec::new();
        if let Some(out) = &output {
            for _ in 0..writers {
                // Unbounded on purpose: the byte budget bounds the queue, and a bounded channel
                // makes the sink busy-wait (`sched_yield`) on a full queue, stealing CPU from
                // the very writers it is waiting for (measured: docs/PHASE3.md).
                let (tx, rx) = unbounded::<Vec<Job>>();
                senders.push(tx);
                let (shared, entries) = (&shared, &index.entries[..]);
                let (dest, overwrite) = (out.dest, out.overwrite);
                let restore = restore.expect("an output implies a restore plan");
                handles
                    .push(scope.spawn(move || {
                        writer_loop(&rx, shared, entries, dest, overwrite, restore)
                    }));
            }
        }
        let dispatcher = (!senders.is_empty()).then(|| Dispatcher::new(senders, &shared));
        let mut demux = Demux {
            entries: &index.entries,
            selected,
            targets,
            aliases,
            next: 0,
            open: None,
            chunk_starts: &index.chunk_starts,
            dirs: output.as_ref().map(|o| DirCache::new(o.dest)),
            output,
            dispatcher,
            restore,
            stats: ExtractStats::default(),
        };

        let outcome = (|| -> Result<()> {
            demux.prepare()?;
            if !block_list.is_empty() {
                let file = File::open(&archive.path).at(&archive.path)?;
                let source = BlockSource {
                    reader: BufReader::with_capacity(1 << 16, file),
                    path: &archive.path,
                    list: block_list,
                    cursor: 0,
                    position: u64::MAX,
                    offsets: &index.chunk_offsets,
                    chunks: &index.chunks,
                    block_size,
                };
                run_ordered(
                    config,
                    source,
                    |raw: RawBlock| {
                        let index = raw.index;
                        decode_payload(&raw.header, raw.payload)
                            .map(|data| DecodedBlock { index, data })
                            .map_err(|e| in_block(e, index))
                    },
                    |block: DecodedBlock| {
                        demux.push(block.index, &block.data)?;
                        done_bytes += block.data.len() as u64;
                        progress(done_bytes, total_bytes);
                        crate::cancel::check()?; // UI-HOOK: shared by extract, verify and test
                        Ok(())
                    },
                )
                .map_err(Error::from)?;
            }
            demux.check_complete()
        })();

        // Close the queues so the writers finish, wait for them, then report.
        let Demux {
            mut stats,
            output,
            selected,
            entries,
            dirs,
            dispatcher,
            ..
        } = demux;
        drop(dispatcher);
        for h in handles {
            let _ = h.join();
        }
        let writer_error = shared.take_error();
        outcome?;
        if let Some(e) = writer_error {
            return Err(e);
        }
        stats.files += shared.files.load(Ordering::Relaxed);
        stats.bytes += shared.bytes.load(Ordering::Relaxed);
        stats.metadata_errors += shared.metadata_errors.load(Ordering::Relaxed);
        stats.symlinks += verified_symlinks;
        if let (Some(out), Some(mut dirs), Some(restore)) = (&output, dirs, &restore) {
            // Links first: creating them must not disturb the directory timestamps set below.
            let made = finish_links(&hard_links, &symlinks, out, &mut dirs, restore)?;
            stats.files += made.hard_links;
            stats.bytes = stats.bytes.saturating_add(made.bytes);
            stats.hard_links += made.hard_links;
            stats.hard_link_copies += made.copies;
            stats.symlinks += made.symlinks;
            stats.metadata_errors += made.errors;
            stats.metadata_errors += finish_dirs(entries, selected, out, &mut dirs, restore)?;
        }
        Ok(stats)
    })
}

/// What [`finish_links`] created.
#[derive(Default)]
struct LinkStats {
    symlinks: u64,
    hard_links: u64,
    copies: u64,
    bytes: u64,
    errors: u64,
}

/// Refuse to replace a directory with a file or link.
fn check_not_a_directory(path: &Path, overwrite: bool) -> Result<()> {
    if overwrite && fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        return Err(Error::Invalid(format!(
            "cannot replace directory {} with a file or link",
            path.display()
        )));
    }
    Ok(())
}

/// Map a failure to create `path` onto the crate's errors.
fn link_error(path: &Path, e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        Error::Exists(path.to_path_buf())
    } else {
        Error::Io {
            path: path.to_path_buf(),
            source: e,
        }
    }
}

/// Create the hard links, then the symbolic links, once all file data is on disk.
///
/// Every hard link's source was extracted by this run (that is how it got here), and the
/// parent directories go through the same checks as files: a symbolic link planted in the
/// destination is refused, never followed. Symbolic link targets are stored as they were and are
/// not interpreted; nothing is ever written through them.
fn finish_links(
    hard_links: &[usize],
    symlinks: &[usize],
    out: &Output<'_>,
    dirs: &mut DirCache,
    restore: &Restore<'_>,
) -> Result<LinkStats> {
    let index = restore.index;
    let entries = &index.entries;
    let mut made = LinkStats::default();
    for &i in hard_links {
        let source = index.hard_link_source(i).expect("selected as a hard link");
        let from = safe_join(out.dest, &entries[source].path)?;
        let path = safe_join(out.dest, &entries[i].path)?;
        if let Some(parent) = path.parent() {
            dirs.ensure(parent)?;
        }
        check_not_a_directory(&path, out.overwrite)?;
        match create_hard_link(&from, &path, out.overwrite).map_err(|e| link_error(&path, e))? {
            LinkOutcome::Linked => {}
            LinkOutcome::Copied => {
                // A copy is a new file: it needs the attributes the shared inode would have had.
                made.copies += 1;
                made.errors += restore.path(i, &path);
            }
        }
        made.hard_links += 1;
        made.bytes = made.bytes.saturating_add(entries[i].size);
    }
    for &i in symlinks {
        let target = index
            .symlink_target(i)
            .expect("selected as a symbolic link");
        let path = safe_join(out.dest, &entries[i].path)?;
        if let Some(parent) = path.parent() {
            dirs.ensure(parent)?;
        }
        check_not_a_directory(&path, out.overwrite)?;
        create_symlink(target, &path, out.overwrite).map_err(|e| link_error(&path, e))?;
        made.symlinks += 1;
        made.errors += restore.path(i, &path);
    }
    Ok(made)
}

/// Create explicitly archived directories (so empty ones exist) and apply their metadata,
/// children first so that creating a child cannot disturb a parent's timestamp.
fn finish_dirs(
    entries: &[Entry],
    selected: &[usize],
    out: &Output<'_>,
    dirs: &mut DirCache,
    restore: &Restore<'_>,
) -> Result<u64> {
    let mut list: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&i| entries[i].kind == EntryKind::Dir)
        .collect();
    list.sort_by(|&a, &b| entries[b].path.cmp(&entries[a].path));
    let mut errors = 0;
    for i in list {
        let d = &entries[i];
        let path = safe_join(out.dest, &d.path)?;
        dirs.ensure(&path)?;
        errors += restore.path(i, &path);
        errors += set_dir_mtime(&path, d.mtime_secs, d.mtime_nanos).is_err() as u64;
        errors += set_mode(&path, d.mode).is_err() as u64;
    }
    Ok(errors)
}

fn in_block(e: Error, index: u32) -> Error {
    match e {
        Error::Checksum(m) => Error::Checksum(format!("chunk {index}: {m}")),
        Error::Codec { .. } => Error::corrupt(format!("chunk {index}: {e}")),
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

/// Reads the needed chunk records in order, seeking over chunks nobody asked for.
struct BlockSource<'a> {
    reader: BufReader<File>,
    path: &'a Path,
    list: Vec<u32>,
    cursor: usize,
    /// Where the reader currently is, or `u64::MAX` when unknown.
    position: u64,
    offsets: &'a [u64],
    chunks: &'a [ChunkInfo],
    block_size: u64,
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
        let (offset, info) = (self.offsets[b], &self.chunks[b]);
        if self.position != offset {
            self.reader.seek(SeekFrom::Start(offset)).at(self.path)?;
        }
        let expected = RecordHeader::len_for(info);
        let (header, payload) = read_record(&mut self.reader, self.block_size as u32, expected)?;
        self.position = offset + header.total_len();
        if !header.agrees_with(info) {
            return Err(Error::corrupt(format!(
                "chunk {index}: record header disagrees with the index"
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

/// The file currently being assembled or written.
enum OpenTarget {
    /// Verify-only: just hash.
    Hash(FileHasher),
    /// Small file: collect bytes, then hand them to a writer thread.
    Buffer(Vec<u8>),
    /// Large file: stream to a temporary file; renamed into place only after its hash verified,
    /// so a failed run never leaves a truncated or unverified file under a real name (dropping
    /// this value removes the temporary file).
    Stream {
        temp: TempOutput,
        file: File,
        hasher: FileHasher,
    },
}

/// Splits the ordered stream of decoded chunks back into files.
struct Demux<'a> {
    entries: &'a [Entry],
    selected: &'a [usize],
    targets: Vec<Target>,
    /// For each target, the other selected entries with the same content.
    aliases: Vec<Vec<usize>>,
    next: usize,
    open: Option<OpenTarget>,
    chunk_starts: &'a [u64],
    output: Option<Output<'a>>,
    dirs: Option<DirCache>,
    dispatcher: Option<Dispatcher<'a>>,
    restore: Option<Restore<'a>>,
    stats: ExtractStats,
}

impl Demux<'_> {
    fn dispatch(&mut self, entry: usize, data: Vec<u8>) -> Result<()> {
        let path = &self.entries[entry].path;
        self.dispatcher
            .as_mut()
            .expect("buffering implies writers")
            .send(entry, path, data)
    }

    /// Validate empty files and hand them to the writers (they are files like any other).
    fn prepare(&mut self) -> Result<()> {
        let entries = self.entries;
        for &i in self.selected {
            let e = &entries[i];
            match e.kind {
                EntryKind::Dir => self.stats.dirs += 1,
                EntryKind::File if e.size == 0 => {
                    check_hash(e, empty_hash()).map_err(|_| {
                        Error::Checksum(format!("{}: empty file has a wrong hash", e.path))
                    })?;
                    match &self.dispatcher {
                        Some(_) => self.dispatch(i, Vec::new())?,
                        None => self.stats.files += 1,
                    }
                }
                EntryKind::File => {}
            }
        }
        Ok(())
    }

    fn begin(&mut self, entry: usize) -> Result<()> {
        let e = &self.entries[entry];
        self.open = Some(match (&self.output, &mut self.dirs) {
            (Some(out), Some(dirs)) if e.size > SMALL_FILE_MAX => {
                let path = safe_join(out.dest, &e.path)?;
                if let Some(parent) = path.parent() {
                    dirs.ensure(parent)?;
                }
                if out.overwrite && fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    return Err(Error::Invalid(format!(
                        "cannot replace directory {} with a file",
                        path.display()
                    )));
                }
                let (temp, file) = TempOutput::create(&path, out.overwrite)?;
                OpenTarget::Stream {
                    temp,
                    file,
                    hasher: FileHasher::new(),
                }
            }
            (Some(_), _) => OpenTarget::Buffer(Vec::with_capacity(e.size as usize)),
            _ => OpenTarget::Hash(FileHasher::new()),
        });
        Ok(())
    }

    /// Finish the file in `entry`, and write the other copies of its content named in `copies`
    /// (deduplicated files that share its region of the stream).
    fn complete(&mut self, entry: usize, copies: &[usize]) -> Result<()> {
        let e = &self.entries[entry];
        match self.open.take().expect("a file is open") {
            OpenTarget::Hash(hasher) => {
                check_hash(e, hasher.finalize())?;
                let n = 1 + copies.len() as u64;
                self.stats.files += n;
                self.stats.bytes = self.stats.bytes.saturating_add(e.size.saturating_mul(n));
            }
            OpenTarget::Buffer(data) => {
                for &copy in copies {
                    self.dispatch(copy, data.clone())?;
                }
                self.dispatch(entry, data)?;
            }
            OpenTarget::Stream { temp, file, hasher } => {
                check_hash(e, hasher.finalize())?;
                if let Some(restore) = &self.restore {
                    self.stats.metadata_errors +=
                        apply_file_metadata(&file, temp.temp_path(), e, entry, restore);
                }
                temp.commit_fast(file)?;
                self.stats.files += 1;
                self.stats.bytes += e.size;
                for &copy in copies {
                    self.copy_file(entry, copy)?;
                }
            }
        }
        Ok(())
    }

    /// Write `to` as a copy of `from`, a file that was just streamed to disk and verified. The
    /// copy is hashed again while it is written.
    fn copy_file(&mut self, from: usize, to: usize) -> Result<()> {
        let entries = self.entries;
        let (Some(out), Some(dirs)) = (&self.output, &mut self.dirs) else {
            return Ok(());
        };
        let source = safe_join(out.dest, &entries[from].path)?;
        let e = &entries[to];
        let path = safe_join(out.dest, &e.path)?;
        if let Some(parent) = path.parent() {
            dirs.ensure(parent)?;
        }
        if out.overwrite && fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            return Err(Error::Invalid(format!(
                "cannot replace directory {} with a file",
                path.display()
            )));
        }
        let (temp, mut file) = TempOutput::create(&path, out.overwrite)?;
        let mut input = File::open(&source).at(&source)?;
        // The source was verified when it was written but is read back from disk here, so the
        // copy is hashed too: a bad read can never produce a wrong file.
        let mut hasher = FileHasher::new();
        let mut buf = vec![0u8; 256 << 10];
        loop {
            let n = read_full(&mut input, &mut buf).at(&source)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).at(temp.temp_path())?;
        }
        check_hash(e, hasher.finalize())?;
        if let Some(restore) = &self.restore {
            self.stats.metadata_errors +=
                apply_file_metadata(&file, temp.temp_path(), e, to, restore);
        }
        temp.commit_fast(file)?;
        self.stats.files += 1;
        self.stats.bytes += e.size;
        Ok(())
    }

    /// Consume one decoded chunk. Chunks arrive in increasing order; gaps only occur where no
    /// selected file has data.
    fn push(&mut self, block_index: u32, data: &[u8]) -> Result<()> {
        self.stats.blocks += 1;
        let b0 = self.chunk_starts[block_index as usize];
        let b1 = b0 + data.len() as u64;
        while self.next < self.targets.len() {
            let t = self.targets[self.next];
            if t.start >= b1 {
                break;
            }
            if t.end <= b0 || (self.open.is_none() && t.start < b0) {
                return Err(Error::Internal("a needed chunk was skipped".into()));
            }
            if self.open.is_none() {
                self.begin(t.entry)?;
            }
            let (s, e) = (t.start.max(b0), t.end.min(b1));
            let slice = &data[(s - b0) as usize..(e - b0) as usize];
            match self.open.as_mut().expect("opened above") {
                OpenTarget::Hash(h) => h.update(slice),
                OpenTarget::Buffer(buf) => buf.extend_from_slice(slice),
                OpenTarget::Stream { temp, file, hasher } => {
                    hasher.update(slice);
                    file.write_all(slice).at(temp.temp_path())?;
                }
            }
            if e < t.end {
                break; // the file continues in the next chunk
            }
            let copies = std::mem::take(&mut self.aliases[self.next]);
            self.complete(t.entry, &copies)?;
            self.next += 1;
        }
        if let Some(d) = &mut self.dispatcher {
            d.flush()?;
        }
        Ok(())
    }

    /// Check nothing is left over and deliver any jobs still waiting to be sent.
    fn check_complete(&mut self) -> Result<()> {
        if let Some(d) = &mut self.dispatcher {
            d.flush()?;
        }
        if self.next != self.targets.len() || self.open.is_some() {
            return Err(Error::corrupt(
                "archive data ended before every file was complete",
            ));
        }
        Ok(())
    }
}

/// Apply ownership, attributes, timestamp and permissions of entry `entry` (`e`) to the file
/// `file` currently at `path`; returns how many could not be applied (non-fatal).
fn apply_file_metadata(
    file: &File,
    path: &Path,
    e: &Entry,
    entry: usize,
    restore: &Restore<'_>,
) -> u64 {
    restore.file(entry, file, path)
        + u64::from(set_file_mtime(file, e.mtime_secs, e.mtime_nanos).is_err())
        + u64::from(set_mode(path, e.mode).is_err())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn budget_bounds_use_admits_oversized_requests_alone_and_never_deadlocks() {
        let b = Budget::new(100);
        assert!(b.try_acquire(60));
        assert!(!b.try_acquire(60), "would exceed the cap");
        assert!(b.try_acquire(40));
        b.release(100);
        assert!(
            b.try_acquire(500),
            "oversized request is admitted when idle"
        );
        assert!(!b.try_acquire(1), "...and holds everything else off");
        b.release(500);
        assert!(b.try_acquire(1));
    }

    #[test]
    fn budget_many_threads_stay_within_the_cap_and_all_finish() {
        let b = Arc::new(Budget::new(1000));
        let peak = Arc::new(AtomicU64::new(0));
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let (b, peak) = (b.clone(), peak.clone());
                thread::spawn(move || {
                    for i in 0..2000u64 {
                        let n = 50 + (i * 7 + t) % 300;
                        b.acquire(n);
                        peak.fetch_max(b.used.load(Ordering::SeqCst), Ordering::SeqCst);
                        if i % 97 == 0 {
                            thread::yield_now();
                        }
                        b.release(n);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 1000, "cap was exceeded");
        assert_eq!(b.used.load(Ordering::SeqCst), 0);
    }
}
