//! ZIP archives: listing, extracting and creating, parallel in both directions.
//!
//! Every file is one independent job. Creating: a pool of workers reads a file, hashes it,
//! deflates it with libdeflate (or stores it when a sample shows it will not shrink) and appends
//! it to the output under a short lock; the central directory is written last, in the original
//! order. Extracting: workers read the compressed bytes with positional reads, inflate with
//! libdeflate, check the CRC-32 and write the file. Entries over 512 MiB go through streaming
//! flate2 instead, and a memory budget keeps the in-flight buffers bounded.
//!
//! Supported: stored and deflate, ZIP64, UTF-8 and legacy (CP437) names, archives with a
//! stub in front. Not supported yet: encryption, other compression methods, multi-disk archives.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use kiln::cancel;
use kiln::error::{Error, Result};
use kiln::fsutil::TempOutput;
use kiln::path::{normalize_selector, safe_join, selector_matches};
use libdeflater::{CompressionLvl, Compressor, Decompressor};
use walkdir::WalkDir;

use crate::dostime;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_EOCD64: u32 = 0x0606_4b50;
const SIG_LOC64: u32 = 0x0706_4b50;
const MAX32: u64 = 0xFFFF_FFFF;
const MAX16: u64 = 0xFFFF;
/// Largest entry (compressed or not) handled in memory; bigger ones are streamed.
const IN_MEMORY_MAX: u64 = 512 << 20;
/// Most bytes of file data held in memory by all workers together (a single file may exceed it).
const MEMORY_BUDGET: u64 = 1 << 30;
const CHUNK: usize = 1 << 20;
/// Files from this size on are sampled first and stored when the sample does not shrink.
const SAMPLE_FROM: usize = 256 << 10;
const SAMPLE_LEN: usize = 64 << 10;

/// CP437 characters for the bytes 0x80..=0xFF (names without the UTF-8 flag).
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn at<T>(r: io::Result<T>, path: &Path) -> Result<T> {
    r.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn le64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([
        b[i],
        b[i + 1],
        b[i + 2],
        b[i + 3],
        b[i + 4],
        b[i + 5],
        b[i + 6],
        b[i + 7],
    ])
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}

fn read_exact_at(f: &File, mut buf: &mut [u8], mut off: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match read_at(f, buf, off) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut std::mem::take(&mut buf)[n..];
                off += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn unix_secs(t: std::time::SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

fn thread_count(requested: usize, jobs: usize) -> usize {
    let t = if requested == 0 {
        thread::available_parallelism().map_or(4, |n| n.get())
    } else {
        requested
    };
    t.clamp(1, jobs.max(1)).min(64)
}

/// Bumps a counter when dropped, so the progress loop ends even if a worker panics.
struct Finish<'a>(&'a AtomicUsize);

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

/// Bounds the bytes of file data in memory across all workers.
struct Budget {
    used: Mutex<u64>,
    freed: Condvar,
    cap: u64,
}

struct Hold<'a> {
    budget: &'a Budget,
    bytes: u64,
}

impl Budget {
    fn new(cap: u64) -> Self {
        Self {
            used: Mutex::new(0),
            freed: Condvar::new(),
            cap,
        }
    }

    /// Wait until `bytes` fit (or nothing else is held, or the run is aborting).
    fn acquire(&self, bytes: u64, abort: &AtomicBool) -> Hold<'_> {
        let mut used = lock(&self.used);
        while *used > 0 && *used + bytes > self.cap && !abort.load(Ordering::Relaxed) {
            used = self
                .freed
                .wait_timeout(used, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *used += bytes;
        Hold {
            budget: self,
            bytes,
        }
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        let mut used = lock(&self.budget.used);
        *used -= self.bytes;
        self.budget.freed.notify_all();
    }
}

fn decode_name(raw: &[u8], utf8_flag: bool) -> String {
    if utf8_flag || raw.is_ascii() {
        return String::from_utf8_lossy(raw).into_owned();
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_owned();
    }
    let table: Vec<char> = CP437_HIGH.chars().collect();
    raw.iter()
        .map(|&b| if b < 0x80 { b as char } else { table[(b - 0x80) as usize] })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------------

/// One entry of a ZIP archive's central directory.
#[derive(Clone, Debug)]
pub struct ZipEntry {
    /// Name with `/` separators and no trailing slash.
    pub name: String,
    /// A directory entry.
    pub is_dir: bool,
    /// A symbolic link (skipped when extracting).
    pub symlink: bool,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Compressed size in bytes.
    pub packed: u64,
    /// Modification time, seconds since the Unix epoch (UTC).
    pub modified: Option<i64>,
    /// CRC-32 of the uncompressed data.
    pub crc: u32,
    /// ZIP compression method number (0 = stored, 8 = deflate).
    pub method: u16,
    /// The entry is encrypted.
    pub encrypted: bool,
    local_off: u64,
}

impl ZipEntry {
    /// Human-readable method name for listings.
    pub fn method_name(&self) -> &'static str {
        match self.method {
            0 => "Store",
            8 => "Deflate",
            9 => "Deflate64",
            12 => "BZip2",
            14 => "LZMA",
            93 => "Zstd",
            95 => "XZ",
            98 => "PPMd",
            99 => "AES",
            _ => "Other",
        }
    }
}

/// An opened ZIP archive.
pub struct ZipArchive {
    path: PathBuf,
    file: File,
    len: u64,
    entries: Vec<ZipEntry>,
}

/// Settings for [`ZipArchive::extract`].
#[derive(Clone, Debug, Default)]
pub struct ExtractOpts {
    /// Directory to extract into (created if missing).
    pub dest: PathBuf,
    /// Replace existing files instead of failing.
    pub overwrite: bool,
    /// Worker threads; 0 means one per core.
    pub threads: usize,
    /// Only these entries (a file, or a folder with everything below it); empty means all.
    pub members: Vec<String>,
}

/// What [`ZipArchive::extract`] did.
#[derive(Clone, Debug, Default)]
pub struct ExtractStats {
    /// Files written.
    pub files: u64,
    /// Directories created.
    pub dirs: u64,
    /// Uncompressed bytes written.
    pub bytes: u64,
    /// Entries skipped (symbolic links).
    pub skipped: u64,
}

struct Job {
    idx: usize,
    target: PathBuf,
}

struct ReadCtx {
    dec: Decompressor,
    inbuf: Vec<u8>,
    outbuf: Vec<u8>,
}

/// A byte range of a file, read with positional reads.
struct Section<'a> {
    file: &'a File,
    pos: u64,
    left: u64,
}

impl Read for Section<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(self.left) as usize;
        let got = read_at(self.file, &mut buf[..n], self.pos)?;
        self.pos += got as u64;
        self.left -= got as u64;
        Ok(got)
    }
}

fn create_target(target: &Path, overwrite: bool) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    match options.open(target) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            Err(Error::Exists(target.to_path_buf()))
        }
        Err(source) => Err(Error::Io {
            path: target.to_path_buf(),
            source,
        }),
    }
}

fn finish_file(out: &File, e: &ZipEntry) {
    if let Some(t) = e.modified.filter(|&t| t >= 0) {
        let _ = out.set_modified(UNIX_EPOCH + Duration::from_secs(t as u64));
    }
}

fn parse_central(cd: &[u8], count: u64, shift: u64) -> Result<Vec<ZipEntry>> {
    let mut entries = Vec::with_capacity(count.min(1 << 16) as usize);
    let mut p = 0usize;
    for _ in 0..count {
        if p + 46 > cd.len() || le32(cd, p) != SIG_CENTRAL {
            return Err(Error::corrupt("bad central directory entry"));
        }
        let made_by = le16(cd, p + 4);
        let flags = le16(cd, p + 8);
        let method = le16(cd, p + 10);
        let time = le16(cd, p + 12);
        let date = le16(cd, p + 14);
        let crc = le32(cd, p + 16);
        let mut packed = le32(cd, p + 20) as u64;
        let mut size = le32(cd, p + 24) as u64;
        let name_len = le16(cd, p + 28) as usize;
        let extra_len = le16(cd, p + 30) as usize;
        let comment_len = le16(cd, p + 32) as usize;
        let ext_attr = le32(cd, p + 38);
        let mut local_off = le32(cd, p + 42) as u64;
        let name_at = p + 46;
        let extra_at = name_at + name_len;
        let next = extra_at + extra_len + comment_len;
        if next > cd.len() {
            return Err(Error::corrupt("central directory entry runs past its end"));
        }
        let raw_name = &cd[name_at..extra_at];
        let extra = &cd[extra_at..extra_at + extra_len];

        let (want_size, want_packed, want_off) =
            (size == MAX32, packed == MAX32, local_off == MAX32);
        let mut modified: Option<i64> = None;
        let mut unicode_name: Option<String> = None;
        let mut q = 0usize;
        while q + 4 <= extra.len() {
            let id = le16(extra, q);
            let sz = le16(extra, q + 2) as usize;
            let body = &extra[q + 4..(q + 4 + sz).min(extra.len())];
            match id {
                0x0001 => {
                    let mut r = 0usize;
                    for (wanted, slot) in [
                        (want_size, &mut size),
                        (want_packed, &mut packed),
                        (want_off, &mut local_off),
                    ] {
                        if wanted {
                            if r + 8 > body.len() {
                                return Err(Error::corrupt("short ZIP64 extra field"));
                            }
                            *slot = le64(body, r);
                            r += 8;
                        }
                    }
                }
                0x5455 => {
                    if body.len() >= 5 && body[0] & 1 != 0 {
                        modified =
                            Some(i32::from_le_bytes([body[1], body[2], body[3], body[4]]) as i64);
                    }
                }
                0x7075 if body.len() > 5 && body[0] == 1 => {
                    if let Ok(s) = std::str::from_utf8(&body[5..]) {
                        unicode_name = Some(s.to_owned());
                    }
                }
                _ => {}
            }
            q += 4 + sz;
        }

        let host = made_by >> 8;
        let mode = ext_attr >> 16;
        let symlink = host == 3 && mode & 0o170_000 == 0o120_000;
        let mut name = unicode_name.unwrap_or_else(|| decode_name(raw_name, flags & 0x800 != 0));
        name = name.replace('\\', "/");
        let mut is_dir = name.ends_with('/');
        if (ext_attr & 0x10 != 0 && size == 0) || (host == 3 && mode & 0o170_000 == 0o040_000) {
            is_dir = true;
        }
        let name = name.trim_end_matches('/').to_owned();
        p = next;
        if name.is_empty() {
            continue;
        }
        entries.push(ZipEntry {
            name,
            is_dir,
            symlink,
            size,
            packed,
            modified: modified.or_else(|| dostime::dos_to_unix(date, time)),
            crc,
            method,
            encrypted: flags & 1 != 0 || method == 99,
            local_off: local_off + shift,
        });
    }
    Ok(entries)
}

impl ZipArchive {
    /// Open `path` and read its central directory.
    pub fn open(path: &Path) -> Result<Self> {
        let file = at(File::open(path), path)?;
        let len = at(file.metadata(), path)?.len();
        if len < 22 {
            return Err(Error::NotAnArchive("too small to be a ZIP file"));
        }
        let tail_len = len.min(22 + 65_535);
        let tail_off = len - tail_len;
        let mut tail = vec![0u8; tail_len as usize];
        at(read_exact_at(&file, &mut tail, tail_off), path)?;
        let eocd_rel = (0..=tail.len() - 22)
            .rev()
            .find(|&i| {
                le32(&tail, i) == SIG_EOCD && i + 22 + le16(&tail, i + 20) as usize <= tail.len()
            })
            .ok_or(Error::NotAnArchive("no ZIP end-of-central-directory record"))?;
        let eocd_abs = tail_off + eocd_rel as u64;
        let mut count = le16(&tail, eocd_rel + 10) as u64;
        let mut cd_size = le32(&tail, eocd_rel + 12) as u64;
        let mut cd_off = le32(&tail, eocd_rel + 16) as u64;
        let mut cd_end = eocd_abs;
        if (count == MAX16 || cd_size == MAX32 || cd_off == MAX32) && eocd_abs >= 20 {
            let mut loc = [0u8; 20];
            at(read_exact_at(&file, &mut loc, eocd_abs - 20), path)?;
            if le32(&loc, 0) == SIG_LOC64 {
                let off64 = le64(&loc, 8);
                let mut rec = [0u8; 56];
                at(read_exact_at(&file, &mut rec, off64), path)?;
                if le32(&rec, 0) != SIG_EOCD64 {
                    return Err(Error::corrupt("bad ZIP64 end-of-central-directory record"));
                }
                count = le64(&rec, 32);
                cd_size = le64(&rec, 40);
                cd_off = le64(&rec, 48);
                cd_end = off64;
            }
        }
        if cd_size > 1 << 30 || count > cd_size / 46 + 1 {
            return Err(Error::corrupt("implausible central directory size"));
        }
        let expected_end = cd_off
            .checked_add(cd_size)
            .ok_or_else(|| Error::corrupt("central directory offset overflows"))?;
        if expected_end > cd_end {
            return Err(Error::corrupt("central directory lies beyond the end record"));
        }
        // Bytes in front of the archive (a self-extractor stub) shift every offset.
        let shift = cd_end - expected_end;
        let mut cd = vec![0u8; cd_size as usize];
        at(read_exact_at(&file, &mut cd, cd_off + shift), path)?;
        let entries = parse_central(&cd, count, shift)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            len,
            entries,
        })
    }

    /// Every entry, in central directory order.
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// Offset of an entry's data, after checking its local header.
    fn data_offset(&self, e: &ZipEntry) -> Result<u64> {
        let mut h = [0u8; 30];
        at(read_exact_at(&self.file, &mut h, e.local_off), &self.path)?;
        if le32(&h, 0) != SIG_LOCAL {
            return Err(Error::corrupt(format!("{}: bad local header", e.name)));
        }
        let off = e.local_off + 30 + le16(&h, 26) as u64 + le16(&h, 28) as u64;
        match off.checked_add(e.packed) {
            Some(end) if end <= self.len => Ok(off),
            _ => Err(Error::corrupt(format!("{}: data runs past the end of the file", e.name))),
        }
    }

    fn select(&self, members: &[String]) -> Result<Vec<usize>> {
        if members.is_empty() {
            return Ok((0..self.entries.len()).collect());
        }
        let selectors: Vec<String> = members.iter().map(|m| normalize_selector(m)).collect();
        let chosen: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                selectors
                    .iter()
                    .any(|s| selector_matches(&self.entries[i].name, s))
            })
            .collect();
        if chosen.is_empty() {
            return Err(Error::Invalid("no entry matches the given names".into()));
        }
        Ok(chosen)
    }

    /// Extract (a subset of) the archive. `progress` gets `(bytes_done, bytes_total)`.
    pub fn extract(
        &self,
        opts: &ExtractOpts,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<ExtractStats> {
        let selected = self.select(&opts.members)?;
        let mut jobs: Vec<Job> = Vec::new();
        let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
        let mut skipped = 0u64;
        let mut total = 0u64;
        for &i in &selected {
            let e = &self.entries[i];
            if e.symlink {
                skipped += 1;
                continue;
            }
            let target = safe_join(&opts.dest, &e.name)?;
            if e.is_dir {
                dirs.insert(target);
            } else {
                if let Some(parent) = target.parent() {
                    dirs.insert(parent.to_path_buf());
                }
                total += e.size;
                jobs.push(Job { idx: i, target });
            }
        }
        at(fs::create_dir_all(&opts.dest), &opts.dest)?;
        for d in &dirs {
            at(fs::create_dir_all(d), d)?;
        }
        jobs.sort_by_key(|j| self.entries[j.idx].local_off);

        let nthreads = thread_count(opts.threads, jobs.len());
        let next = AtomicUsize::new(0);
        let done = AtomicU64::new(0);
        let abort = AtomicBool::new(false);
        let finished = AtomicUsize::new(0);
        let first_err: Mutex<Option<Error>> = Mutex::new(None);
        let budget = Budget::new(MEMORY_BUDGET);

        let panicked = thread::scope(|s| {
            let handles: Vec<_> = (0..nthreads)
                .map(|_| {
                    s.spawn(|| {
                        let _guard = Finish(&finished);
                        let mut ctx = ReadCtx {
                            dec: Decompressor::new(),
                            inbuf: Vec::new(),
                            outbuf: Vec::new(),
                        };
                        loop {
                            if abort.load(Ordering::Relaxed) {
                                break;
                            }
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(job) = jobs.get(k) else { break };
                            let e = &self.entries[job.idx];
                            if let Err(err) = self.extract_one(
                                e,
                                &job.target,
                                opts.overwrite,
                                &mut ctx,
                                &budget,
                                &abort,
                                &done,
                            ) {
                                abort.store(true, Ordering::Relaxed);
                                let mut slot = lock(&first_err);
                                if slot.is_none() {
                                    *slot = Some(err);
                                }
                                break;
                            }
                        }
                    })
                })
                .collect();
            while finished.load(Ordering::Acquire) < nthreads {
                progress(done.load(Ordering::Relaxed), total);
                thread::sleep(Duration::from_millis(40));
            }
            handles.into_iter().any(|h| h.join().is_err())
        });
        progress(done.load(Ordering::Relaxed), total);
        if let Some(err) = first_err.into_inner().unwrap_or_else(|e| e.into_inner()) {
            return Err(err);
        }
        if panicked {
            return Err(Error::Internal("an extraction thread panicked".into()));
        }
        Ok(ExtractStats {
            files: jobs.len() as u64,
            dirs: dirs.len() as u64,
            bytes: total,
            skipped,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_one(
        &self,
        e: &ZipEntry,
        target: &Path,
        overwrite: bool,
        ctx: &mut ReadCtx,
        budget: &Budget,
        abort: &AtomicBool,
        done: &AtomicU64,
    ) -> Result<()> {
        cancel::check()?;
        if e.encrypted {
            return Err(Error::Unsupported(format!(
                "{}: encrypted entries are not supported yet",
                e.name
            )));
        }
        if e.method != 0 && e.method != 8 {
            return Err(Error::Unsupported(format!(
                "{}: compression method {} ({}) is not supported",
                e.name,
                e.method,
                e.method_name()
            )));
        }
        let mut out = create_target(target, overwrite)?;
        let result = self.extract_into(e, target, &mut out, ctx, budget, abort, done);
        if result.is_err() {
            drop(out);
            let _ = fs::remove_file(target);
        } else {
            finish_file(&out, e);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_into(
        &self,
        e: &ZipEntry,
        target: &Path,
        out: &mut File,
        ctx: &mut ReadCtx,
        budget: &Budget,
        abort: &AtomicBool,
        done: &AtomicU64,
    ) -> Result<()> {
        if e.size == 0 {
            return Ok(());
        }
        let data_off = self.data_offset(e)?;
        if e.size > IN_MEMORY_MAX || e.packed > IN_MEMORY_MAX {
            return self.extract_stream(e, data_off, out, target, done);
        }
        let _hold = budget.acquire(e.size + e.packed, abort);
        ctx.inbuf.resize(e.packed as usize, 0);
        at(read_exact_at(&self.file, &mut ctx.inbuf, data_off), &self.path)?;
        let data: &[u8] = if e.method == 0 {
            if e.packed != e.size {
                return Err(Error::corrupt(format!("{}: stored size mismatch", e.name)));
            }
            &ctx.inbuf
        } else {
            ctx.outbuf.resize(e.size as usize, 0);
            let n = ctx
                .dec
                .deflate_decompress(&ctx.inbuf, &mut ctx.outbuf)
                .map_err(|err| Error::corrupt(format!("{}: {err:?}", e.name)))?;
            if n as u64 != e.size {
                return Err(Error::corrupt(format!("{}: size mismatch", e.name)));
            }
            &ctx.outbuf
        };
        if crc32fast::hash(data) != e.crc {
            return Err(Error::Checksum(e.name.clone()));
        }
        at(out.write_all(data), target)?;
        done.fetch_add(e.size, Ordering::Relaxed);
        Ok(())
    }

    /// Entries too big for memory: stream through flate2 (or copy, when stored).
    fn extract_stream(
        &self,
        e: &ZipEntry,
        data_off: u64,
        out: &mut File,
        target: &Path,
        done: &AtomicU64,
    ) -> Result<()> {
        let section = BufReader::with_capacity(
            CHUNK,
            Section {
                file: &self.file,
                pos: data_off,
                left: e.packed,
            },
        );
        let mut reader: Box<dyn Read + '_> = if e.method == 0 {
            Box::new(section)
        } else {
            Box::new(flate2::read::DeflateDecoder::new(section))
        };
        let mut buf = vec![0u8; CHUNK];
        let mut hasher = crc32fast::Hasher::new();
        let mut total = 0u64;
        loop {
            cancel::check()?;
            let n = reader
                .read(&mut buf)
                .map_err(|err| Error::corrupt(format!("{}: {err}", e.name)))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > e.size {
                return Err(Error::corrupt(format!("{}: more data than declared", e.name)));
            }
            hasher.update(&buf[..n]);
            at(out.write_all(&buf[..n]), target)?;
            done.fetch_add(n as u64, Ordering::Relaxed);
        }
        if total != e.size {
            return Err(Error::corrupt(format!("{}: size mismatch", e.name)));
        }
        if hasher.finalize() != e.crc {
            return Err(Error::Checksum(e.name.clone()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------------

/// Settings for [`create`].
#[derive(Clone, Debug)]
pub struct CreateOpts {
    /// 0 = store everything, 1 (fastest) to 9, 10 to 12 (slowest, smallest).
    pub level: u8,
    /// Worker threads; 0 means one per core.
    pub threads: usize,
    /// Replace the archive if it already exists.
    pub overwrite: bool,
}

impl Default for CreateOpts {
    fn default() -> Self {
        Self {
            level: 6,
            threads: 0,
            overwrite: false,
        }
    }
}

/// Something that was found but not archived.
#[derive(Clone, Debug)]
pub struct Skipped {
    /// The file or folder.
    pub path: PathBuf,
    /// Why.
    pub reason: String,
}

/// What [`create`] did.
#[derive(Clone, Debug, Default)]
pub struct CreateStats {
    /// Files archived.
    pub files: u64,
    /// Folders archived.
    pub dirs: u64,
    /// Total uncompressed file bytes.
    pub raw_bytes: u64,
    /// Final archive size.
    pub archive_bytes: u64,
    /// Files stored without compression (incompressible or level 0).
    pub stored: u64,
    /// Items not archived.
    pub skipped: Vec<Skipped>,
}

struct Src {
    path: PathBuf,
    name: String,
    is_dir: bool,
    size: u64,
    mtime: Option<i64>,
    readonly: bool,
}

/// One finished entry, kept for the central directory.
struct CdRec {
    name: String,
    flags: u16,
    method: u16,
    dos_time: u16,
    dos_date: u16,
    mtime: i64,
    crc: u32,
    size: u64,
    packed: u64,
    off: u64,
    ext_attr: u32,
}

/// What goes into a local header.
struct Meta<'a> {
    name: &'a str,
    flags: u16,
    method: u16,
    dos_time: u16,
    dos_date: u16,
    mtime: i64,
    crc: u32,
    size: u64,
    packed: u64,
    ext_attr: u32,
}

enum Body<'a> {
    Bytes(&'a [u8]),
    /// A temporary file, positioned at its start.
    File(File),
}

enum Fail {
    /// This one source cannot be read: report it and go on.
    Skip(String),
    Fatal(Error),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Fatal(e)
    }
}

struct Out {
    w: BufWriter<File>,
    pos: u64,
}

impl Out {
    /// Append `data`; returns the offset it starts at.
    fn put(&mut self, data: &[u8]) -> io::Result<u64> {
        let off = self.pos;
        self.w.write_all(data)?;
        self.pos += data.len() as u64;
        Ok(off)
    }
}

struct Shared<'a> {
    list: &'a [Src],
    order: &'a [usize],
    output: &'a Path,
    level: u8,
    next: AtomicUsize,
    done: AtomicU64,
    abort: AtomicBool,
    finished: AtomicUsize,
    err: Mutex<Option<Error>>,
    out: Mutex<Out>,
    budget: Budget,
}

impl Shared<'_> {
    fn fail(&self, e: Error) {
        self.abort.store(true, Ordering::Relaxed);
        let mut slot = lock(&self.err);
        if slot.is_none() {
            *slot = Some(e);
        }
    }
}

#[derive(Default)]
struct WorkerOut {
    recs: Vec<(usize, CdRec)>,
    skipped: Vec<Skipped>,
}

struct CompCtx {
    comp: Compressor,
    sampler: Compressor,
    inbuf: Vec<u8>,
    outbuf: Vec<u8>,
}

impl CompCtx {
    fn new(level: u8) -> Result<Self> {
        let main = CompressionLvl::new(level.max(1) as i32)
            .map_err(|_| Error::Invalid(format!("invalid compression level {level}")))?;
        let fast = CompressionLvl::new(1)
            .map_err(|_| Error::Invalid("could not set up the sampling compressor".into()))?;
        Ok(Self {
            comp: Compressor::new(main),
            sampler: Compressor::new(fast),
            inbuf: Vec::new(),
            outbuf: Vec::new(),
        })
    }

    /// Deflate `inbuf` into `outbuf`; the compressed length, or `None` when storing is better.
    fn deflate(&mut self) -> Option<usize> {
        let len = self.inbuf.len();
        if len >= SAMPLE_FROM {
            let mid = len / 2;
            let heads = [0usize, mid];
            let mut all_flat = true;
            for start in heads {
                let sample = &self.inbuf[start..(start + SAMPLE_LEN).min(len)];
                if !sample_is_flat(&mut self.sampler, &mut self.outbuf, sample) {
                    all_flat = false;
                    break;
                }
            }
            if all_flat {
                return None;
            }
        }
        let bound = self.comp.deflate_compress_bound(len);
        self.outbuf.resize(bound, 0);
        let n = self.comp.deflate_compress(&self.inbuf, &mut self.outbuf).ok()?;
        if n >= len { None } else { Some(n) }
    }
}

/// True when a quick level-1 pass over `sample` barely shrinks it.
fn sample_is_flat(sampler: &mut Compressor, scratch: &mut Vec<u8>, sample: &[u8]) -> bool {
    let bound = sampler.deflate_compress_bound(sample.len());
    scratch.resize(bound, 0);
    match sampler.deflate_compress(sample, scratch) {
        Ok(n) => n * 100 >= sample.len() * 97,
        Err(_) => false,
    }
}

fn local_header(m: &Meta) -> Vec<u8> {
    let zip64 = m.size >= MAX32 || m.packed >= MAX32;
    let name = m.name.as_bytes();
    let mut v = Vec::with_capacity(30 + name.len() + 32);
    put32(&mut v, SIG_LOCAL);
    put16(&mut v, if zip64 { 45 } else { 20 });
    put16(&mut v, m.flags);
    put16(&mut v, m.method);
    put16(&mut v, m.dos_time);
    put16(&mut v, m.dos_date);
    put32(&mut v, m.crc);
    put32(&mut v, if zip64 { MAX32 as u32 } else { m.packed as u32 });
    put32(&mut v, if zip64 { MAX32 as u32 } else { m.size as u32 });
    put16(&mut v, name.len() as u16);
    put16(&mut v, 9 + if zip64 { 20 } else { 0 });
    v.extend_from_slice(name);
    put16(&mut v, 0x5455);
    put16(&mut v, 5);
    v.push(1);
    put32(&mut v, m.mtime.clamp(0, u32::MAX as i64) as u32);
    if zip64 {
        put16(&mut v, 1);
        put16(&mut v, 16);
        put64(&mut v, m.size);
        put64(&mut v, m.packed);
    }
    v
}

fn central_header(r: &CdRec, v: &mut Vec<u8>) {
    let size64 = r.size >= MAX32;
    let packed64 = r.packed >= MAX32;
    let off64 = r.off >= MAX32;
    let zip64 = size64 || packed64 || off64;
    let name = r.name.as_bytes();
    let mut z: Vec<u8> = Vec::new();
    if size64 {
        put64(&mut z, r.size);
    }
    if packed64 {
        put64(&mut z, r.packed);
    }
    if off64 {
        put64(&mut z, r.off);
    }
    let version = if zip64 { 45 } else { 20 };
    put32(v, SIG_CENTRAL);
    put16(v, version);
    put16(v, version);
    put16(v, r.flags);
    put16(v, r.method);
    put16(v, r.dos_time);
    put16(v, r.dos_date);
    put32(v, r.crc);
    put32(v, if packed64 { MAX32 as u32 } else { r.packed as u32 });
    put32(v, if size64 { MAX32 as u32 } else { r.size as u32 });
    put16(v, name.len() as u16);
    put16(v, 9 + if zip64 { 4 + z.len() as u16 } else { 0 });
    put16(v, 0);
    put16(v, 0);
    put16(v, 0);
    put32(v, r.ext_attr);
    put32(v, if off64 { MAX32 as u32 } else { r.off as u32 });
    v.extend_from_slice(name);
    put16(v, 0x5455);
    put16(v, 5);
    v.push(1);
    put32(v, r.mtime.clamp(0, u32::MAX as i64) as u32);
    if zip64 {
        put16(v, 1);
        put16(v, z.len() as u16);
        v.extend_from_slice(&z);
    }
}

fn end_records(count: u64, cd_off: u64, cd_size: u64, v: &mut Vec<u8>) {
    let need64 = count >= MAX16 || cd_off >= MAX32 || cd_size >= MAX32;
    if need64 {
        put32(v, SIG_EOCD64);
        put64(v, 44);
        put16(v, 45);
        put16(v, 45);
        put32(v, 0);
        put32(v, 0);
        put64(v, count);
        put64(v, count);
        put64(v, cd_size);
        put64(v, cd_off);
        put32(v, SIG_LOC64);
        put32(v, 0);
        put64(v, cd_off + cd_size);
        put32(v, 1);
    }
    put32(v, SIG_EOCD);
    put16(v, 0);
    put16(v, 0);
    let n16 = if need64 { MAX16 as u16 } else { count as u16 };
    put16(v, n16);
    put16(v, n16);
    put32(v, if need64 { MAX32 as u32 } else { cd_size as u32 });
    put32(v, if need64 { MAX32 as u32 } else { cd_off as u32 });
    put16(v, 0);
}

/// Append one entry (header, then body) to the output under the lock.
fn write_entry(sh: &Shared, m: &Meta, body: Body) -> std::result::Result<CdRec, Fail> {
    let header = local_header(m);
    let off;
    {
        let mut out = lock(&sh.out);
        off = at(out.put(&header), sh.output)?;
        match body {
            Body::Bytes(bytes) => {
                at(out.put(bytes), sh.output)?;
            }
            Body::File(mut f) => {
                let n = at(io::copy(&mut f, &mut out.w), sh.output)?;
                out.pos += n;
            }
        }
    }
    Ok(CdRec {
        name: m.name.to_owned(),
        flags: m.flags,
        method: m.method,
        dos_time: m.dos_time,
        dos_date: m.dos_date,
        mtime: m.mtime,
        crc: m.crc,
        size: m.size,
        packed: m.packed,
        off,
        ext_attr: m.ext_attr,
    })
}

/// Read `input` to its end through `sink`, hashing it on the way.
fn pump(input: &mut File, sink: &mut dyn Write, done: &AtomicU64) -> io::Result<(u32, u64)> {
    let mut buf = vec![0u8; CHUNK];
    let mut hasher = crc32fast::Hasher::new();
    let mut total = 0u64;
    loop {
        if cancel::is_set() {
            return Err(io::Error::other(cancel::CANCELLED));
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        sink.write_all(&buf[..n])?;
        total += n as u64;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    Ok((hasher.finalize(), total))
}

/// A file too big for memory: stream it into an anonymous temporary file (deflated or copied),
/// then append that to the output.
fn add_big(sh: &Shared, src: &Src, meta: Meta) -> std::result::Result<CdRec, Fail> {
    let io_err = |source: io::Error| {
        Fail::Fatal(Error::Io {
            path: src.path.clone(),
            source,
        })
    };
    let mut input = File::open(&src.path).map_err(|e| Fail::Skip(e.to_string()))?;
    let mut head = Vec::new();
    (&mut input)
        .take(SAMPLE_LEN as u64)
        .read_to_end(&mut head)
        .map_err(io_err)?;
    input.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let store = sh.level == 0 || {
        let mut scratch = Vec::new();
        let level = CompressionLvl::new(1)
            .map_err(|_| Fail::Fatal(Error::Invalid("sampling compressor".into())))?;
        let mut sampler = Compressor::new(level);
        sample_is_flat(&mut sampler, &mut scratch, &head)
    };
    drop(head);

    let tmp = tempfile::tempfile().map_err(io_err)?;
    let mut w = BufWriter::with_capacity(CHUNK, tmp);
    let (crc, size) = if store {
        pump(&mut input, &mut w, &sh.done).map_err(io_err)?
    } else {
        let level = flate2::Compression::new(sh.level.clamp(1, 9) as u32);
        let mut enc = flate2::write::DeflateEncoder::new(&mut w, level);
        let r = pump(&mut input, &mut enc, &sh.done).map_err(io_err)?;
        enc.finish().map_err(io_err)?;
        r
    };
    let mut tmp = w
        .into_inner()
        .map_err(|e| io_err(e.into_error()))?;
    let packed = tmp.stream_position().map_err(io_err)?;
    tmp.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let meta = Meta {
        method: if store { 0 } else { 8 },
        crc,
        size,
        packed,
        ..meta
    };
    write_entry(sh, &meta, Body::File(tmp))
}

fn add_entry(sh: &Shared, src: &Src, ctx: &mut CompCtx) -> std::result::Result<CdRec, Fail> {
    cancel::check()?;
    let full_name = if src.is_dir {
        format!("{}/", src.name)
    } else {
        src.name.clone()
    };
    let mtime = src.mtime.unwrap_or(315_532_800);
    let (dos_date, dos_time) = dostime::unix_to_dos(mtime);
    let meta = Meta {
        name: &full_name,
        flags: if full_name.is_ascii() { 0 } else { 0x800 },
        method: 0,
        dos_time,
        dos_date,
        mtime,
        crc: 0,
        size: 0,
        packed: 0,
        ext_attr: (if src.is_dir { 0x10 } else { 0x20 }) | u32::from(src.readonly),
    };
    if src.is_dir || src.size == 0 {
        return write_entry(sh, &meta, Body::Bytes(&[]));
    }
    if src.size > IN_MEMORY_MAX {
        return add_big(sh, src, meta);
    }

    let _hold = sh.budget.acquire(src.size * 2 + 4096, &sh.abort);
    ctx.inbuf.clear();
    let mut file = File::open(&src.path).map_err(|e| Fail::Skip(e.to_string()))?;
    file.read_to_end(&mut ctx.inbuf)
        .map_err(|e| Fail::Skip(e.to_string()))?;
    drop(file);
    let size = ctx.inbuf.len() as u64;
    let crc = crc32fast::hash(&ctx.inbuf);
    let packed_len = if sh.level > 0 && size > 0 {
        ctx.deflate()
    } else {
        None
    };
    let result = match packed_len {
        Some(n) => {
            let meta = Meta {
                method: 8,
                crc,
                size,
                packed: n as u64,
                ..meta
            };
            write_entry(sh, &meta, Body::Bytes(&ctx.outbuf[..n]))
        }
        None => {
            let meta = Meta {
                method: 0,
                crc,
                size,
                packed: size,
                ..meta
            };
            write_entry(sh, &meta, Body::Bytes(&ctx.inbuf))
        }
    };
    sh.done.fetch_add(size, Ordering::Relaxed);
    result
}

fn worker(sh: &Shared) -> WorkerOut {
    let _guard = Finish(&sh.finished);
    let mut res = WorkerOut::default();
    let mut ctx = match CompCtx::new(sh.level) {
        Ok(c) => c,
        Err(e) => {
            sh.fail(e);
            return res;
        }
    };
    loop {
        if sh.abort.load(Ordering::Relaxed) {
            break;
        }
        let k = sh.next.fetch_add(1, Ordering::Relaxed);
        let Some(&idx) = sh.order.get(k) else { break };
        let src = &sh.list[idx];
        match add_entry(sh, src, &mut ctx) {
            Ok(rec) => res.recs.push((idx, rec)),
            Err(Fail::Skip(reason)) => res.skipped.push(Skipped {
                path: src.path.clone(),
                reason,
            }),
            Err(Fail::Fatal(e)) => {
                sh.fail(e);
                break;
            }
        }
    }
    res
}

fn src_from(path: &Path, name: String, meta: &fs::Metadata) -> Src {
    let is_dir = meta.is_dir();
    Src {
        path: path.to_path_buf(),
        name,
        is_dir,
        size: if is_dir { 0 } else { meta.len() },
        mtime: meta.modified().ok().map(unix_secs),
        readonly: meta.permissions().readonly(),
    }
}

/// Walk the inputs: files by name, folders recursively. Symbolic links and special files are
/// skipped and reported; the output file itself is left out.
fn collect(inputs: &[PathBuf], output: &Path, skipped: &mut Vec<Skipped>) -> Result<Vec<Src>> {
    let out_abs = std::path::absolute(output).unwrap_or_else(|_| output.to_path_buf());
    let mut list: Vec<Src> = Vec::new();
    for input in inputs {
        let abs = std::path::absolute(input).unwrap_or_else(|_| input.clone());
        let meta = at(fs::symlink_metadata(&abs), &abs)?;
        let Some(base) = abs.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return Err(Error::Invalid(format!(
                "cannot archive {}: it has no name (a drive root?)",
                input.display()
            )));
        };
        if meta.file_type().is_symlink() {
            skipped.push(Skipped {
                path: abs,
                reason: "symbolic link".into(),
            });
            continue;
        }
        if meta.is_file() {
            if abs != out_abs {
                list.push(src_from(&abs, base, &meta));
            }
            continue;
        }
        if !meta.is_dir() {
            skipped.push(Skipped {
                path: abs,
                reason: "not a regular file or folder".into(),
            });
            continue;
        }
        list.push(src_from(&abs, base.clone(), &meta));
        for item in WalkDir::new(&abs)
            .min_depth(1)
            .follow_links(false)
            .sort_by_file_name()
        {
            let item = match item {
                Ok(i) => i,
                Err(err) => {
                    skipped.push(Skipped {
                        path: err.path().map_or_else(|| abs.clone(), Path::to_path_buf),
                        reason: err.to_string(),
                    });
                    continue;
                }
            };
            let ft = item.file_type();
            if ft.is_symlink() || !(ft.is_file() || ft.is_dir()) {
                skipped.push(Skipped {
                    path: item.path().to_path_buf(),
                    reason: "symbolic link or special file".into(),
                });
                continue;
            }
            if item.path() == out_abs {
                continue;
            }
            let Ok(rel) = item.path().strip_prefix(&abs) else {
                continue;
            };
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let name = format!("{base}/{}", rel.join("/"));
            if name.len() > u16::MAX as usize {
                skipped.push(Skipped {
                    path: item.path().to_path_buf(),
                    reason: "name too long for a ZIP entry".into(),
                });
                continue;
            }
            match item.metadata() {
                Ok(m) => list.push(src_from(item.path(), name, &m)),
                Err(err) => skipped.push(Skipped {
                    path: item.path().to_path_buf(),
                    reason: err.to_string(),
                }),
            }
        }
    }
    Ok(list)
}

/// Create the ZIP archive `output` from `inputs`. `progress` gets `(bytes_done, bytes_total)`.
///
/// The archive is built in a temporary file and moved into place on success, so `output` is
/// either complete or untouched.
pub fn create(
    output: &Path,
    inputs: &[PathBuf],
    opts: &CreateOpts,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<CreateStats> {
    if inputs.is_empty() {
        return Err(Error::Invalid("nothing to archive: no input paths given".into()));
    }
    if opts.level > 12 {
        return Err(Error::Invalid("the level must be between 0 and 12".into()));
    }
    let mut skipped = Vec::new();
    let list = collect(inputs, output, &mut skipped)?;
    let total: u64 = list.iter().map(|s| s.size).sum();
    // Biggest first keeps all workers busy to the end; the central directory restores the order.
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(list[i].size));

    let (temp, file) = TempOutput::create(output, opts.overwrite)?;
    let sh = Shared {
        list: &list,
        order: &order,
        output,
        level: opts.level,
        next: AtomicUsize::new(0),
        done: AtomicU64::new(0),
        abort: AtomicBool::new(false),
        finished: AtomicUsize::new(0),
        err: Mutex::new(None),
        out: Mutex::new(Out {
            w: BufWriter::with_capacity(4 << 20, file),
            pos: 0,
        }),
        budget: Budget::new(MEMORY_BUDGET),
    };
    let nthreads = thread_count(opts.threads, order.len());
    let outs: Vec<thread::Result<WorkerOut>> = thread::scope(|s| {
        let handles: Vec<_> = (0..nthreads).map(|_| s.spawn(|| worker(&sh))).collect();
        while sh.finished.load(Ordering::Acquire) < nthreads {
            progress(sh.done.load(Ordering::Relaxed), total);
            thread::sleep(Duration::from_millis(40));
        }
        handles.into_iter().map(|h| h.join()).collect()
    });
    progress(sh.done.load(Ordering::Relaxed), total);
    if let Some(e) = lock(&sh.err).take() {
        return Err(e);
    }
    let mut recs: Vec<(usize, CdRec)> = Vec::new();
    for o in outs {
        match o {
            Ok(w) => {
                recs.extend(w.recs);
                skipped.extend(w.skipped);
            }
            Err(_) => return Err(Error::Internal("a compression thread panicked".into())),
        }
    }
    recs.sort_by_key(|r| r.0);

    let mut out = sh.out.into_inner().unwrap_or_else(|e| e.into_inner());
    let cd_off = out.pos;
    let mut cd: Vec<u8> = Vec::with_capacity(recs.len() * 96 + 128);
    for (_, r) in &recs {
        central_header(r, &mut cd);
    }
    let cd_size = cd.len() as u64;
    end_records(recs.len() as u64, cd_off, cd_size, &mut cd);
    at(out.put(&cd), output)?;
    at(out.w.flush(), output)?;
    let archive_bytes = out.pos;
    let file = out.w.into_inner().map_err(|e| Error::Io {
        path: output.to_path_buf(),
        source: e.into_error(),
    })?;
    temp.commit_fast(file)?;

    let mut stats = CreateStats {
        archive_bytes,
        skipped,
        ..CreateStats::default()
    };
    for (_, r) in &recs {
        if r.name.ends_with('/') {
            stats.dirs += 1;
        } else {
            stats.files += 1;
            stats.raw_bytes += r.size;
            if r.method == 0 && r.size > 0 {
                stats.stored += 1;
            }
        }
    }
    Ok(stats)
}
