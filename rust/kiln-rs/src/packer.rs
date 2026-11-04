//! Container writer: port of `Encoder`, `PlainSink`/`CryptSink`, `pack_file`, `build_table_bytes` and
//! the single-archive part of `cmd_pack` in kiln_v14.py. Serial for now (threads are the next step).
//! Filters supported here: none, BCJ x86 (121), BCJ AArch64 (122), WAV filters (see wav.rs), MP3 recompression
//! (120, see mp3enc.rs). The caller (packapp) chooses the filter id per file (see `PackFile::filt`).

use crate::bcj::{BcjEnc, BCJ_ARM64, BCJ_X86};
use crate::container::{Entry, KilnError, Result, Table};
use crate::crypto;
use crate::lz;
use crate::pack::{self, Block};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use std::collections::VecDeque;
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::io::{Read, Write};

pub const MAGIC: &[u8; 4] = b"KLNA";
pub const FLAG_ENCRYPTED: u8 = 1;
pub const SCRYPT_LOG_N: u8 = 17;
pub const SCRYPT_R: u32 = 8;
pub const SCRYPT_P: u32 = 1;
pub const WINDOW: usize = (1 << 26) - 1;
pub const CKPT_SPAN: u64 = 64 << 20;
pub const SEGMENT: usize = 4 << 20;
const CHUNK: usize = 1 << 20;
pub const CANCELLED: &str = "cancelled";

fn ioerr(e: std::io::Error) -> KilnError {
    KilnError(format!("write error: {e}"))
}

/// Called with the number of new input bytes done; returns true to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(u64) -> bool;

// ---------------------------------------------------------------- sinks

pub trait Sink {
    fn write_record(&mut self, data: &[u8], fin: bool) -> Result<()>;
    /// (stream offset in the file, record counter)
    fn pos(&self) -> (u64, u64);
}

pub struct PlainSink<'a, W: Write> {
    w: &'a mut W,
    off: u64,
}

impl<'a, W: Write> PlainSink<'a, W> {
    pub fn new(w: &'a mut W, off: u64) -> Self {
        PlainSink { w, off }
    }
}

impl<'a, W: Write> Sink for PlainSink<'a, W> {
    fn write_record(&mut self, data: &[u8], _fin: bool) -> Result<()> {
        self.w.write_all(data).map_err(ioerr)?;
        self.off += data.len() as u64;
        Ok(())
    }
    fn pos(&self) -> (u64, u64) {
        (self.off, 0)
    }
}

pub struct CryptSink<'a, W: Write> {
    w: &'a mut W,
    cipher: &'a Aes256Gcm,
    header: &'a [u8],
    stream: u8,
    n: u64,
    off: u64,
}

impl<'a, W: Write> CryptSink<'a, W> {
    pub fn new(w: &'a mut W, cipher: &'a Aes256Gcm, header: &'a [u8], stream: u8, off: u64) -> Self {
        CryptSink { w, cipher, header, stream, n: 0, off }
    }
}

impl<'a, W: Write> Sink for CryptSink<'a, W> {
    fn write_record(&mut self, data: &[u8], fin: bool) -> Result<()> {
        let mut aad = self.header.to_vec();
        aad.push(if fin { 1 } else { 0 });
        let nonce = crypto::nonce(self.stream, self.n);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: data, aad: &aad })
            .map_err(|_| KilnError("encryption failed".to_string()))?;
        self.n += 1;
        self.w.write_all(&(ct.len() as u32).to_be_bytes()).map_err(ioerr)?;
        self.w.write_all(&ct).map_err(ioerr)?;
        self.off += 4 + ct.len() as u64;
        Ok(())
    }
    fn pos(&self) -> (u64, u64) {
        (self.off, self.n)
    }
}

// ---------------------------------------------------------------- encoder

pub struct Encoder<'a, S: Sink> {
    pub sink: S,
    store: bool,
    segment: usize,
    progress: Option<Progress<'a>>,
    jobs: usize,
    inflight: VecDeque<(JoinHandle<Option<(Vec<Block>, Vec<u8>)>>, usize)>,
    /// set when the person cancels: worker threads see it through `lz::set_cancel` and stop early
    cancel: Arc<AtomicBool>,
    pending: Vec<u8>,
    history: Vec<u8>,
    since: u64,
    pub ckpts: Vec<(u32, u64, u64)>,
}

impl<'a, S: Sink> Encoder<'a, S> {
    pub fn new(sink: S, store: bool, segment: usize, jobs: usize, progress: Option<Progress<'a>>) -> Self {
        Encoder {
            sink,
            store,
            segment,
            progress,
            jobs: jobs.max(1),
            inflight: VecDeque::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            pending: Vec::new(),
            history: Vec::new(),
            since: 0,
            ckpts: Vec::new(),
        }
    }

    fn emit(&mut self, btype: u8, raw: usize, payload: &[u8]) -> Result<()> {
        let mut rec = Vec::with_capacity(9 + payload.len());
        rec.push(btype);
        rec.extend_from_slice(&(raw as u32).to_be_bytes());
        rec.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        rec.extend_from_slice(payload);
        self.sink.write_record(&rec, false)
    }

    pub fn checkpoint(&mut self, file_index: u32) -> Result<()> {
        self.flush()?;
        self.drain()?;
        self.history.clear();
        self.since = 0;
        let (off, ctr) = self.sink.pos();
        self.ckpts.push((file_index, off, ctr));
        Ok(())
    }

    pub fn maybe_checkpoint(&mut self, file_index: u32) -> Result<()> {
        if self.since >= CKPT_SPAN {
            self.checkpoint(file_index)?;
        }
        Ok(())
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<()> {
        self.since += data.len() as u64;
        self.pending.extend_from_slice(data);
        if self.pending.len() >= self.segment {
            self.process()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        if !self.pending.is_empty() {
            self.process()?;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        self.flush()?;
        self.drain()?;
        self.sink.write_record(&[2u8], true)
    }

    fn tick(&mut self, n_new: usize) -> Result<()> {
        if let Some(p) = self.progress.as_mut() {
            if p(n_new as u64) {
                return Err(KilnError(CANCELLED.to_string()));
            }
        }
        Ok(())
    }

    fn write_blocks(&mut self, blocks: &[Block], buf: &[u8]) -> Result<()> {
        for b in blocks {
            match b {
                Block::Stored { start, len } => self.emit(0, *len, &buf[*start..*start + *len])?,
                Block::Coded { raw, payload } => self.emit(1, *raw, payload)?,
            }
        }
        Ok(())
    }

    /// Cancel seen: tell every worker to stop, then wait for them (they stop within a few ms).
    fn abort(&mut self) -> KilnError {
        self.cancel.store(true, Ordering::Relaxed);
        while let Some((h, _)) = self.inflight.pop_front() {
            let _ = h.join();
        }
        KilnError(CANCELLED.to_string())
    }

    /// Wait for the oldest segment, write it, report its bytes.
    fn drain_one(&mut self) -> Result<()> {
        if self.inflight.is_empty() {
            return Ok(());
        }
        if self.progress.is_some() {
            // poll the callback (0 new bytes) until the oldest segment is done: a cancel is noticed
            // within ~20 ms even in the middle of a segment; the other workers keep running meanwhile
            while !self.inflight.front().unwrap().0.is_finished() {
                if let Some(p) = self.progress.as_mut() {
                    if p(0) {
                        return Err(self.abort());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        if let Some((h, n_new)) = self.inflight.pop_front() {
            let r = h
                .join()
                .map_err(|_| KilnError("a compression thread stopped unexpectedly".to_string()))?;
            let (blocks, buf) = match r {
                Some(x) => x,
                None if self.cancel.load(Ordering::Relaxed) => return Err(self.abort()),
                None => return Err(KilnError("out of memory while compressing".to_string())),
            };
            self.write_blocks(&blocks, &buf)?;
            self.tick(n_new)?;
        }
        Ok(())
    }

    fn drain(&mut self) -> Result<()> {
        while !self.inflight.is_empty() {
            self.drain_one()?;
        }
        Ok(())
    }

    fn process(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending);
        let n_new = pending.len();
        if self.store {
            let mut i = 0;
            while i < n_new {
                let end = n_new.min(i + (1 << 20));
                self.emit(0, end - i, &pending[i..end])?;
                i = end;
            }
            return self.tick(n_new);
        }
        let mut buf = std::mem::take(&mut self.history);
        let start = buf.len();
        buf.extend_from_slice(&pending);
        drop(pending);
        let keep = buf.len().min(WINDOW);
        self.history = buf[buf.len() - keep..].to_vec();
        if self.jobs <= 1 {
            let blocks = if self.progress.is_none() {
                pack::build_blocks(&buf, start, pack::PARSE)
            } else {
                // run on a helper thread so this one can keep asking whether to cancel
                let flag = self.cancel.clone();
                let bref: &[u8] = &buf;
                let res = std::thread::scope(|sc| -> Result<Option<Vec<Block>>> {
                    let h = std::thread::Builder::new()
                        .stack_size(64 << 20)
                        .spawn_scoped(sc, move || {
                            lz::set_cancel(Some(flag));
                            pack::build_blocks(bref, start, pack::PARSE)
                        })
                        .map_err(|e| KilnError(format!("can't start a compression thread: {e}")))?;
                    while !h.is_finished() {
                        if let Some(p) = self.progress.as_mut() {
                            if p(0) {
                                self.cancel.store(true, Ordering::Relaxed);
                                let _ = h.join();
                                return Err(KilnError(CANCELLED.to_string()));
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    h.join().map_err(|_| KilnError("a compression thread stopped unexpectedly".to_string()))
                });
                res?
            };
            let blocks = blocks.ok_or_else(|| KilnError("out of memory while compressing".to_string()))?;
            self.write_blocks(&blocks, &buf)?;
            return self.tick(n_new);
        }
        // at most `jobs` segments run at once; the oldest is written first (archive order is fixed)
        while self.inflight.len() >= self.jobs {
            self.drain_one()?;
        }
        let flag = self.cancel.clone();
        let h = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                lz::set_cancel(Some(flag));
                let r = pack::build_blocks(&buf, start, pack::PARSE);
                r.map(|bl| (bl, buf))
            })
            .map_err(|e| KilnError(format!("can't start a compression thread: {e}")))?;
        self.inflight.push_back((h, n_new));
        Ok(())
    }
}

/// `fit_segment` in kiln_v14.py: just small enough to keep `jobs` workers busy, 4..16 MiB.
pub fn fit_segment(total: u64, jobs: usize) -> usize {
    let per_core = (total + jobs.max(1) as u64 - 1) / jobs.max(1) as u64;
    let rounded = ((per_core + (1 << 20) - 1) >> 20) << 20;
    rounded.clamp(4 << 20, 16 << 20) as usize
}

fn mem_available() -> Option<u64> {
    let t = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in t.lines() {
        if let Some(r) = line.strip_prefix("MemAvailable:") {
            return r.split_whitespace().next()?.parse::<u64>().ok().map(|kb| kb * 1024);
        }
    }
    None
}

/// Peak memory of one segment worker: match-finder tables (~250 MB) + buffer + token array.
fn worker_bytes(hist: u64, segment: usize) -> u64 {
    (250u64 << 20) + 2 * (hist + segment as u64) + 8 * segment as u64
}

/// Default number of threads: the cores (at most 8), no more than 60% of free memory allows.
pub fn auto_jobs(total: u64, segment: usize) -> usize {
    let mut n = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).min(8);
    if total <= segment as u64 {
        return 1;
    }
    if let Some(avail) = mem_available() {
        let hist = total.min(WINDOW as u64);
        n = n.min(((avail as f64 * 0.6) as u64 / worker_bytes(hist, segment)) as usize);
    }
    n.max(1)
}

// ---------------------------------------------------------------- table

pub fn build_table_bytes(table: &Table) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(table.entries.len() as u32).to_be_bytes());
    for e in &table.entries {
        let nb = e.rel.as_bytes();
        let ext = e.filt > 11 || e.dead;
        v.extend_from_slice(&(nb.len() as u16).to_be_bytes());
        v.extend_from_slice(nb);
        v.extend_from_slice(&e.size.to_be_bytes());
        v.extend_from_slice(&e.crc.to_be_bytes());
        v.extend_from_slice(&e.mtime.to_be_bytes());
        let mf: u16 = (e.mode & 0o7777) | ((if ext { 15 } else { e.filt }) << 12);
        v.extend_from_slice(&mf.to_be_bytes());
        if ext {
            v.extend_from_slice(&(e.filt | if e.dead { 0x8000 } else { 0 }).to_be_bytes());
        }
        if e.filt == 120 {
            v.extend_from_slice(&e.slen.to_be_bytes());
        }
    }
    if !table.ckpts.is_empty() {
        v.extend_from_slice(b"CKPT");
        v.extend_from_slice(&(table.ckpts.len() as u32).to_be_bytes());
        for c in &table.ckpts {
            v.extend_from_slice(&c.0.to_be_bytes());
            v.extend_from_slice(&c.1.to_be_bytes());
            v.extend_from_slice(&c.2.to_be_bytes());
        }
    }
    v
}

// ---------------------------------------------------------------- one archive

/// Index (in the `files` list) of the file the packer is reading right now, for progress display.
/// Only the UI reads it; it never affects the archive bytes.
pub static CUR_FILE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct PackFile {
    pub full: String,
    pub rel: String,
    pub size: u64,
    pub mtime: u64,
    pub mode: u16,
    /// 0 = none, 121 = BCJ x86, 122 = BCJ AArch64. Anything else is refused.
    pub filt: u16,
    /// already-compressed data (only changes how the python engine feeds it: same bytes here)
    pub pre: bool,
}

pub struct PackOpts<'a> {
    pub store: bool,
    pub password: Option<&'a str>,
    pub segment: usize,
    /// segment worker threads (1 = in this thread)
    pub jobs: usize,
}

pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| KilnError(format!("no random numbers available: {e}")))?;
    Ok(b)
}

/// Writes one archive to `dest` (through `dest.tmp`). Returns the table that was written.
pub fn pack_archive(dest: &str, files: &[PackFile], opts: &PackOpts, progress: Option<Progress>) -> Result<Table> {
    let tmp = format!("{dest}.tmp");
    let r = pack_to(&tmp, files, opts, progress);
    match r {
        Ok(t) => {
            std::fs::rename(&tmp, dest).map_err(|e| KilnError(format!("can't write {dest}: {e}")))?;
            Ok(t)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn pack_to(tmp: &str, files: &[PackFile], opts: &PackOpts, progress: Option<Progress>) -> Result<Table> {
    for f in files {
        if f.filt != 0 && f.filt != BCJ_X86 && f.filt != BCJ_ARM64 && f.filt != 120 && crate::wav::wav_spec(f.filt).is_none() {
            return Err(KilnError("a filter the Rust packer does not support".to_string()));
        }
    }
    let mut header: Vec<u8> = MAGIC.to_vec();
    let mut cipher = None;
    if let Some(pw) = opts.password {
        let salt: [u8; 16] = random_bytes()?;
        header.push(FLAG_ENCRYPTED);
        header.extend_from_slice(&[SCRYPT_LOG_N, SCRYPT_R as u8, SCRYPT_P as u8]);
        header.extend_from_slice(&salt);
        let key = crypto::derive_key(pw, &salt, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P)?;
        cipher = Some(crypto::make_cipher(&key));
    } else {
        header.push(0);
    }
    let mut out = std::io::BufWriter::with_capacity(1 << 20, File::create(tmp).map_err(|e| KilnError(format!("can't write {tmp}: {e}")))?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if cipher.is_some() {
            let _ = std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o600));
        }
    }
    out.write_all(&header).map_err(ioerr)?;
    let hl = header.len() as u64;
    let mut table = Table::default();
    let table_off;
    let mut data_ckpts = Vec::new();
    {
        macro_rules! run_data {
            ($sink:expr) => {{
                let mut enc = Encoder::new($sink, opts.store, opts.segment, if opts.store { 1 } else { opts.jobs }, progress);
                for f in files {
                    enc.maybe_checkpoint(table.entries.len() as u32)?;
                    CUR_FILE.store(table.entries.len(), std::sync::atomic::Ordering::Relaxed);
                    let e = pack_file(&mut enc, f, opts.store)?;
                    table.entries.push(e);
                }
                enc.finish()?;
                data_ckpts = enc.ckpts.clone();
            }};
        }
        match &cipher {
            Some(c) => run_data!(CryptSink::new(&mut out, c, &header, 0, hl)),
            None => run_data!(PlainSink::new(&mut out, hl)),
        }
    }
    out.flush().map_err(ioerr)?;
    table_off = out.get_ref().metadata().map_err(ioerr)?.len();
    table.ckpts = data_ckpts;
    let tb = build_table_bytes(&table);
    match &cipher {
        Some(c) => {
            let mut e = Encoder::new(CryptSink::new(&mut out, c, &header, 1, table_off), false, SEGMENT, 1, None);
            e.feed(&tb)?;
            e.finish()?;
        }
        None => {
            let mut e = Encoder::new(PlainSink::new(&mut out, table_off), false, SEGMENT, 1, None);
            e.feed(&tb)?;
            e.finish()?;
        }
    }
    let total: u64 = if cipher.is_some() { 0 } else { table.entries.iter().map(|e| e.size).sum() };
    out.write_all(&table_off.to_be_bytes()).map_err(ioerr)?;
    out.write_all(&total.to_be_bytes()).map_err(ioerr)?;
    out.write_all(MAGIC).map_err(ioerr)?;
    out.flush().map_err(ioerr)?;
    Ok(table)
}

/// Forward filter of one file: BCJ (121/122) or a WAV filter (1-11, 16-62, 64-110).
enum Fwd {
    Bcj(BcjEnc),
    Wav(crate::wav::Wav),
}

impl Fwd {
    fn new(filt: u16, size: u64) -> Result<Fwd> {
        if filt == BCJ_X86 || filt == BCJ_ARM64 {
            Ok(Fwd::Bcj(BcjEnc::new(filt)))
        } else {
            Ok(Fwd::Wav(crate::wav::Wav::new_enc(filt, size)?))
        }
    }
    fn feed(&mut self, d: &[u8]) -> Result<Vec<u8>> {
        match self {
            Fwd::Bcj(b) => Ok(b.feed(d)),
            Fwd::Wav(w) => w.feed(d),
        }
    }
    fn finish(&mut self) -> Result<Vec<u8>> {
        match self {
            Fwd::Bcj(b) => Ok(b.finish()),
            Fwd::Wav(w) => w.finish(),
        }
    }
}

fn pack_file<S: Sink>(enc: &mut Encoder<S>, f: &PackFile, store: bool) -> Result<Entry> {
    let mut filt = if store { 0 } else { f.filt };
    if filt == 120 {
        // MP3 recompression (python try_mp3): the stream holds the packed form; if it does not
        // help, the file is packed like any other (BCJ pick for non-precompressed files)
        match std::fs::read(&f.full) {
            Ok(data) => {
                if data.len() as u64 != f.size {
                    return Err(KilnError(format!("{} changed while it was being read", f.full)));
                }
                if let Some(blob) = crate::mp3enc::mp3_pack(&data) {
                    enc.feed(&blob)?;
                    return Ok(Entry {
                        rel: f.rel.clone(),
                        size: data.len() as u64,
                        crc: crc32fast::hash(&data),
                        mtime: f.mtime,
                        mode: f.mode,
                        filt: 120,
                        dead: false,
                        slen: blob.len() as u64,
                    });
                }
            }
            Err(_) => {}
        }
        filt = if f.pre { 0 } else { crate::walk::pick_bcj(&f.full, f.size) };
    }
    let mut flt = if filt != 0 { Some(Fwd::new(filt, f.size)?) } else { None };
    let mut fh = File::open(&f.full).map_err(|e| KilnError(format!("{}: {e}", f.full)))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut size = 0u64;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let n = fh.read(&mut chunk).map_err(|e| KilnError(format!("{}: {e}", f.full)))?;
        if n == 0 {
            break;
        }
        hasher.update(&chunk[..n]);
        size += n as u64;
        match flt.as_mut() {
            Some(b) => {
                let d = b.feed(&chunk[..n])?;
                if !d.is_empty() {
                    enc.feed(&d)?;
                }
            }
            None => enc.feed(&chunk[..n])?,
        }
    }
    if let Some(b) = flt.as_mut() {
        if size != f.size {
            return Err(KilnError(format!("{} changed while it was being read", f.full)));
        }
        let tail = b.finish()?;
        if !tail.is_empty() {
            enc.feed(&tail)?;
        }
    }
    Ok(Entry { rel: f.rel.clone(), size, crc: hasher.finalize(), mtime: f.mtime, mode: f.mode, filt, dead: false, slen: size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::Archive;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn table_bytes_equal_python() {
        let mk = |rel: &str, size, crc, mtime, mode, filt| Entry { rel: rel.into(), size, crc, mtime, mode, filt, dead: false, slen: size };
        let t = Table { entries: vec![mk("a/b.txt", 5, 7, 9, 0o644, 0), mk("x", 1, 2, 3, 0o755, 121)], ckpts: vec![(1, 30, 0)] };
        // made by the real kiln_v14.build_table_bytes
        let want = "000000020007612f622e747874000000000000000500000007000000000000000901a40001780000000000000001000000020000000000000003f1ed0079434b50540000000100000001000000000000001e0000000000000000";
        assert_eq!(hex(&build_table_bytes(&t)), want);
    }

    fn sample(n: usize) -> Vec<u8> {
        let mut s = 99u32;
        (0..n)
            .map(|i| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                if i % 5 == 0 { 0xE8 } else if i % 3 == 0 { b'a' + (i % 7) as u8 } else { (s >> 24) as u8 }
            })
            .collect()
    }

    fn roundtrip(password: Option<&str>, store: bool, filt: u16) {
        let dir = std::env::temp_dir().join(format!("kiln_pk_{}_{}_{}", std::process::id(), store as u8, filt));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.bin");
        std::fs::write(&src, sample(300_000)).unwrap();
        let dest = dir.join("out.kln");
        let f = PackFile { full: src.to_string_lossy().into(), rel: "in.bin".into(), size: 300_000, mtime: 1, mode: 0o644, filt, pre: false };
        let opts = PackOpts { store, password, segment: 100_000, jobs: 1 };
        let t = pack_archive(dest.to_str().unwrap(), &[f], &opts, None).unwrap();
        assert_eq!(t.entries.len(), 1);
        let fh = File::open(&dest).unwrap();
        let mut a = Archive::open(fh, password).unwrap();
        a.test().unwrap();
        assert!(!dest.with_extension("kln.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn threaded_equals_serial() {
        let dir = std::env::temp_dir().join(format!("kiln_thr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.bin");
        std::fs::write(&src, sample(500_000)).unwrap();
        let mk = || PackFile { full: src.to_string_lossy().into(), rel: "in.bin".into(), size: 500_000, mtime: 1, mode: 0o644, filt: 0, pre: false };
        let a = dir.join("a.kln");
        let b = dir.join("b.kln");
        pack_archive(a.to_str().unwrap(), &[mk()], &PackOpts { store: false, password: None, segment: 100_000, jobs: 1 }, None).unwrap();
        pack_archive(b.to_str().unwrap(), &[mk()], &PackOpts { store: false, password: None, segment: 100_000, jobs: 3 }, None).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert_eq!(fit_segment(10 << 20, 4), 4 << 20);
        assert_eq!(fit_segment(100 << 20, 2), 16 << 20);
        assert_eq!(fit_segment(9 << 20, 1), 9 << 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_mid_segment() {
        let dir = std::env::temp_dir().join(format!("kiln_cnl_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.bin");
        let n = 6_000_000;
        std::fs::write(&src, sample(n)).unwrap();
        for jobs in [1usize, 2] {
            let f = PackFile { full: src.to_string_lossy().into(), rel: "in.bin".into(), size: n as u64, mtime: 1, mode: 0o644, filt: 0, pre: false };
            let dest = dir.join(format!("c{jobs}.kln"));
            let mut stop = |_n: u64| true;
            let t = std::time::Instant::now();
            let r = pack_archive(dest.to_str().unwrap(), &[f], &PackOpts { store: false, password: None, segment: n, jobs }, Some(&mut stop as &mut dyn FnMut(u64) -> bool));
            assert_eq!(r.err().map(|e| e.0), Some(CANCELLED.to_string()));
            assert!(t.elapsed().as_secs() < 5, "cancel took {:?}", t.elapsed());
            assert!(!dest.exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archives_read_back() {
        roundtrip(None, false, 0);
        roundtrip(None, false, BCJ_X86);
        roundtrip(None, false, BCJ_ARM64);
        roundtrip(Some("pw"), true, 0);
    }
}
