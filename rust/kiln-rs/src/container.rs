//! Archive container: header, records, file table, checkpoints, extraction / verification.
//! Port of open_archive / parse_table / read_stream / Extractor in kiln_v14.py (plain archives;
//! encrypted archives go through the `Records` trait, see crypto.rs).
//! Error texts are the same as the Python engine's, so the app can show them unchanged.

use crate::bcj::{Bcj, BCJ_ARM64, BCJ_X86};
use crate::wav::Wav;
use crate::crypto::{self, CryptSource};
use crate::rc;
use aes_gcm::Aes256Gcm;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};

pub const MAGIC: &[u8; 4] = b"KLNA";
pub const FLAG_ENCRYPTED: u8 = 1;
pub const FOOTER_LEN: u64 = 20;
pub const WINDOW: usize = (1 << 26) - 1;
pub const MP3_FID: u16 = 120;

#[derive(Debug, Clone, PartialEq)]
pub struct KilnError(pub String);
impl fmt::Display for KilnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for KilnError {}
pub type Result<T> = std::result::Result<T, KilnError>;

fn err<T>(s: &str) -> Result<T> {
    Err(KilnError(s.to_string()))
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub rel: String,
    pub size: u64,
    pub crc: u32,
    pub mtime: u64,
    pub mode: u16,
    pub filt: u16,
    pub dead: bool,
    pub slen: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub entries: Vec<Entry>,
    /// (file_index, stream_offset, record_counter)
    pub ckpts: Vec<(u32, u64, u64)>,
}

impl Table {
    /// latest checkpoint at or before file `idx`: (file_index, stream_offset, counter)
    pub fn find_ckpt(&self, idx: usize, hdr_len: u64) -> (usize, u64, u64) {
        let mut best = (0usize, hdr_len, 0u64);
        for c in &self.ckpts {
            if (c.0 as usize) <= idx {
                best = (c.0 as usize, c.1, c.2);
            }
        }
        best
    }
}

/// True when `fid` is a WAV-family filter id the engine knows (python `wav_spec(fid) is not None`).
pub fn wav_spec_valid(fid: u16) -> bool {
    crate::wav::wav_spec(fid).is_some()
}

// ---------------------------------------------------------------- table parsing
struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.p + n > self.b.len() {
            return None;
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|s| {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_be_bytes(a)
        })
    }
}

const BAD_TABLE: &str = "corrupt data: bad file table";
const NEWER: &str = "archive uses features from a newer kiln";

pub fn parse_table(raw: &[u8], hdr_len: u64, table_off: Option<u64>) -> Result<Table> {
    let bad = || KilnError(BAD_TABLE.to_string());
    let mut c = Cur { b: raw, p: 0 };
    let count = c.u32().ok_or_else(bad)?;
    let mut table = Table::default();
    for _ in 0..count {
        let nlen = c.u16().ok_or_else(bad)? as usize;
        let name = c.take(nlen).ok_or_else(bad)?;
        let name = std::str::from_utf8(name).map_err(|_| bad())?.to_string();
        let size = c.u64().ok_or_else(bad)?;
        let crc = c.u32().ok_or_else(bad)?;
        let mtime = c.u64().ok_or_else(bad)?;
        let mode = c.u16().ok_or_else(bad)?;
        let mut filt = mode >> 12;
        let mut dead = false;
        if filt == 15 {
            let v = c.u16().ok_or_else(bad)?;
            dead = (v & 0x8000) != 0;
            filt = v & 0x7FFF;
            if (filt > 11 && !wav_spec_valid(filt) && filt != MP3_FID && filt != BCJ_X86 && filt != BCJ_ARM64)
                || (filt <= 11 && !dead)
            {
                return err(NEWER);
            }
        } else if filt > 11 {
            return err(NEWER);
        }
        let mut slen = size;
        if filt == MP3_FID {
            slen = c.u64().ok_or_else(bad)?;
        }
        table.entries.push(Entry { rel: name, size, crc, mtime, mode: mode & 0x0FFF, filt, dead, slen });
    }
    if raw.len() >= c.p + 4 && &raw[c.p..c.p + 4] == b"CKPT" {
        c.p += 4;
        let nc = c.u32().ok_or_else(bad)?;
        let mut prev: (i64, i64, i64) = (0, hdr_len as i64 - 1, 0);
        for _ in 0..nc {
            let a = c.u32().ok_or_else(bad)?;
            let b = c.u64().ok_or_else(bad)?;
            let k = c.u64().ok_or_else(bad)?;
            let ok = a > 0
                && (a as u64) < count as u64
                && (b as i128) > prev.1 as i128
                && (k as i128) >= prev.2 as i128
                && table_off.map_or(true, |t| b < t);
            if !ok {
                return err("corrupt data: bad checkpoint list");
            }
            table.ckpts.push((a, b, k));
            prev = (a as i64, b as i64, k as i64);
        }
    }
    Ok(table)
}

// ---------------------------------------------------------------- records
/// A source of stream records: a block (type|raw|comp|payload) or the single end byte [2].
pub trait Records {
    fn read_record(&mut self) -> Result<Vec<u8>>;
}

/// f.read(n) semantics: fewer bytes only at end of file.
fn read_upto<R: Read>(r: &mut R, n: usize) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    r.take(n as u64)
        .read_to_end(&mut v)
        .map_err(|e| KilnError(format!("read error: {e}")))?;
    Ok(v)
}

pub struct PlainSource<'a, R: Read> {
    pub f: &'a mut R,
}

impl<'a, R: Read> Records for PlainSource<'a, R> {
    fn read_record(&mut self) -> Result<Vec<u8>> {
        let t = read_upto(self.f, 1)?;
        if t.is_empty() {
            return err("archive is truncated (stream has no end)");
        }
        if t[0] == 2 {
            return Ok(t);
        }
        let hdr = read_upto(self.f, 8)?;
        if hdr.len() < 8 {
            return err("archive is truncated (cut inside a block header)");
        }
        let comp_len = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        if comp_len > (1 << 28) {
            return err("corrupt data: impossible block length");
        }
        let payload = read_upto(self.f, comp_len)?;
        if payload.len() < comp_len {
            return err("archive is truncated (cut inside a block)");
        }
        let mut rec = t;
        rec.extend_from_slice(&hdr);
        rec.extend_from_slice(&payload);
        Ok(rec)
    }
}

/// Port of `read_stream`: yields decoded chunks until the end marker.
pub struct StreamReader<'a> {
    src: &'a mut dyn Records,
    out: Vec<u8>,
    mark: usize,
}

impl<'a> StreamReader<'a> {
    pub fn new(src: &'a mut dyn Records) -> StreamReader<'a> {
        StreamReader { src, out: Vec::new(), mark: 0 }
    }

    /// Next decoded chunk, or None at the end block.
    pub fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        // (python does this after yielding the previous chunk)
        if self.out.len() > 2 * WINDOW {
            let cut = self.out.len() - WINDOW;
            self.out.drain(..cut);
        }
        self.mark = self.out.len();
        let rec = self.src.read_record()?;
        if rec.is_empty() {
            return err("corrupt data: empty record");
        }
        let t = rec[0];
        if t == 2 {
            return Ok(None);
        }
        if (t != 0 && t != 1) || rec.len() < 9 {
            return err("corrupt data: unknown block type");
        }
        let raw_len = u32::from_be_bytes([rec[1], rec[2], rec[3], rec[4]]) as usize;
        let comp_len = u32::from_be_bytes([rec[5], rec[6], rec[7], rec[8]]) as usize;
        let payload = &rec[9..];
        if payload.len() != comp_len {
            return err("corrupt data: block length mismatch");
        }
        if t == 0 {
            if comp_len != raw_len {
                return err("corrupt data: bad stored block");
            }
            self.out.extend_from_slice(payload);
        } else {
            if raw_len > rc::MAX_BLOCK_RAW {
                return err("corrupt data: bad block size");
            }
            rc::decode_block(payload, raw_len, &mut self.out)
                .map_err(|c| KilnError(rc::error_text(c).to_string()))?;
        }
        Ok(Some(self.out[self.mark..].to_vec()))
    }
}

// ---------------------------------------------------------------- unfilters
pub enum Unfilter {
    Bcj(Bcj),
    Wav(Wav),
}

impl Unfilter {
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        match self {
            Unfilter::Bcj(b) => Ok(b.feed(chunk)),
            Unfilter::Wav(w) => w.feed(chunk),
        }
    }
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        match self {
            Unfilter::Bcj(b) => Ok(b.finish()),
            Unfilter::Wav(w) => w.finish(),
        }
    }
}

/// python `make_unfilter`: None for plain/dead entries. MP3 (filter 120) is still Python-only.
pub fn make_unfilter(e: &Entry) -> Result<Option<Unfilter>> {
    if e.filt == 0 || e.dead {
        return Ok(None);
    }
    if e.filt == MP3_FID {
        return err("this file uses the MP3 filter that the Rust engine does not support yet");
    }
    if e.filt == BCJ_X86 || e.filt == BCJ_ARM64 {
        return Ok(Some(Unfilter::Bcj(Bcj::new(e.filt))));
    }
    Ok(Some(Unfilter::Wav(Wav::new(e.filt, e.size)?)))
}

// ---------------------------------------------------------------- archive
pub struct Archive<R: Read + Seek> {
    pub f: R,
    pub table: Table,
    pub table_off: u64,
    pub hdr_len: u64,
    pub encrypted: bool,
    header: Vec<u8>,
    cipher: Option<Aes256Gcm>,
}

/// The record source for one stream: plain, or encrypted with record counter `ctr`.
fn make_source<'a, R: Read>(
    f: &'a mut R,
    cipher: &'a Option<Aes256Gcm>,
    header: &'a [u8],
    stream: u8,
    ctr: u64,
) -> Box<dyn Records + 'a> {
    match cipher {
        Some(c) => Box::new(CryptSource { f, cipher: c, header, stream, n: ctr }),
        None => Box::new(PlainSource { f }),
    }
}

impl<R: Read + Seek> Archive<R> {
    pub fn open(mut f: R, password: Option<&str>) -> Result<Archive<R>> {
        let io = |e: std::io::Error| KilnError(format!("read error: {e}"));
        let head = read_upto(&mut f, 5)?;
        if head.len() >= 4 && &head[..4] == b"KLN2" {
            return err("this is an old KLN2 archive; use the old kiln.py to open it");
        }
        if head.len() < 5 || &head[..4] != MAGIC {
            return err("not a kiln archive (expected format KLNA; archives from kiln 13 and older are not readable)");
        }
        let flags = head[4];
        if flags & !FLAG_ENCRYPTED != 0 {
            return err(NEWER);
        }
        let mut head = head;
        let mut cipher: Option<Aes256Gcm> = None;
        if flags & FLAG_ENCRYPTED != 0 {
            let extra = read_upto(&mut f, 19)?;
            if extra.len() < 19 {
                return err("archive is truncated");
            }
            head.extend_from_slice(&extra);
            let (log_n, r, p) = (extra[0], extra[1], extra[2]);
            if !((10..=20).contains(&log_n) && (1..=16).contains(&r) && (1..=4).contains(&p)) {
                return err("corrupt data: bad key-derivation settings");
            }
            let pw = match password {
                Some(p) => p,
                None => return err("this archive is encrypted: a password is needed"),
            };
            let key = crypto::derive_key(pw, &extra[3..], log_n, r as u32, p as u32)?;
            cipher = Some(crypto::make_cipher(&key));
        }
        let hdr_len = head.len() as u64;
        let size = f.seek(SeekFrom::End(0)).map_err(io)?;
        if size < hdr_len + FOOTER_LEN {
            return err("archive is truncated");
        }
        f.seek(SeekFrom::Start(size - FOOTER_LEN)).map_err(io)?;
        let foot = read_upto(&mut f, FOOTER_LEN as usize)?;
        if foot.len() < FOOTER_LEN as usize {
            return err("archive is truncated");
        }
        let table_off = u64::from_be_bytes(foot[0..8].try_into().unwrap());
        let total = u64::from_be_bytes(foot[8..16].try_into().unwrap());
        if &foot[16..20] != MAGIC || !(hdr_len <= table_off && table_off <= size - FOOTER_LEN) {
            return err("archive is truncated or damaged (missing footer)");
        }
        f.seek(SeekFrom::Start(table_off)).map_err(io)?;
        let mut raw = Vec::new();
        {
            let mut src = make_source(&mut f, &cipher, &head, 1, 0);
            let mut rd = StreamReader::new(&mut *src);
            while let Some(c) = rd.next_chunk()? {
                raw.extend_from_slice(&c);
            }
        }
        let table = parse_table(&raw, hdr_len, Some(table_off))?;
        let sum: u64 = table.entries.iter().map(|e| e.size).sum();
        if cipher.is_none() && sum != total {
            return err("corrupt data: file table doesn't match footer");
        }
        Ok(Archive { f, table, table_off, hdr_len, encrypted: cipher.is_some(), header: head, cipher })
    }

    /// Verify every checksum (`kiln test`).
    pub fn test(&mut self) -> Result<()> {
        let wanted = vec![true; self.table.entries.len()];
        let mut sink = NullSink;
        self.extract(&wanted, false, &mut sink)
    }

    /// Port of run_extract's decode loop. `wanted[i]` selects files for the sink; every file that
    /// goes by has its CRC checked. `select` = the caller passed names (python: `names`), which lets
    /// decoding start at a checkpoint and stop after the last wanted file.
    pub fn extract(&mut self, wanted: &[bool], select: bool, sink: &mut dyn Sink) -> Result<()> {
        let n = self.table.entries.len();
        let io = |e: std::io::Error| KilnError(format!("read error: {e}"));
        let (lo, hi) = if select {
            let lo = match wanted.iter().position(|&w| w) {
                Some(v) => v,
                None => return err("no files in the archive match"),
            };
            let hi = wanted.iter().rposition(|&w| w).unwrap();
            (lo, hi)
        } else {
            (0, n.saturating_sub(1))
        };
        let (k, off, ctr) = self.table.find_ckpt(lo, self.hdr_len);
        let part = select && (k > 0 || hi + 1 < n);
        let start = if part { k } else { 0 };
        let mut ext = Extractor::new(&self.table.entries[start..], &wanted[start..], start, sink)?;
        self.f.seek(SeekFrom::Start(off)).map_err(io)?;
        {
            let mut src = make_source(&mut self.f, &self.cipher, &self.header, 0, ctr);
            let mut rd = StreamReader::new(&mut *src);
            while let Some(chunk) = rd.next_chunk()? {
                ext.feed(&chunk)?;
                if part && ext.i + start > hi {
                    break;
                }
            }
        }
        if !part {
            ext.finish()?;
            let pos = self.f.stream_position().map_err(io)?;
            if pos != self.table_off {
                return err("corrupt data: stream doesn't end where the table starts");
            }
        }
        Ok(())
    }
}

pub trait Sink {
    fn begin(&mut self, _idx: usize, _e: &Entry) -> Result<()> {
        Ok(())
    }
    fn write(&mut self, _idx: usize, _piece: &[u8]) -> Result<()> {
        Ok(())
    }
    fn end(&mut self, _idx: usize, _e: &Entry) -> Result<()> {
        Ok(())
    }
}
pub struct NullSink;
impl Sink for NullSink {}

/// Port of the python `Extractor` (`i` is relative to the slice it was given; sink calls use real indexes).
pub struct Extractor<'a> {
    table: &'a [Entry],
    wanted: &'a [bool],
    sink: &'a mut dyn Sink,
    /// index of table[0] in the whole archive: the sink always sees real file indexes
    base: usize,
    pub i: usize,
    got: u64,
    crc: u32,
    unf: Option<Unfilter>,
    open: bool,
}

impl<'a> Extractor<'a> {
    pub fn new(table: &'a [Entry], wanted: &'a [bool], base: usize, sink: &'a mut dyn Sink) -> Result<Extractor<'a>> {
        let mut e = Extractor { table, wanted, sink, base, i: 0, got: 0, crc: 0, unf: None, open: false };
        e.advance()?;
        Ok(e)
    }

    fn begin(&mut self) -> Result<()> {
        let e = &self.table[self.i];
        self.crc = 0;
        self.got = 0;
        self.unf = make_unfilter(e)?;
        self.open = self.wanted[self.i];
        if self.open {
            self.sink.begin(self.base + self.i, e)?;
        }
        Ok(())
    }

    fn end(&mut self) -> Result<()> {
        let e = &self.table[self.i];
        if self.crc != e.crc && !e.dead {
            return Err(KilnError(format!("checksum failed for {}", e.rel)));
        }
        if self.open {
            self.sink.end(self.base + self.i, e)?;
            self.open = false;
        }
        Ok(())
    }

    fn advance(&mut self) -> Result<()> {
        while self.i < self.table.len() {
            self.begin()?;
            if self.table[self.i].slen == 0 {
                self.end()?;
                self.i += 1;
            } else {
                break;
            }
        }
        Ok(())
    }

    pub fn feed(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            if self.i >= self.table.len() {
                return err("corrupt data: stream is longer than the file table");
            }
            let need = (self.table[self.i].slen - self.got) as usize;
            let take = need.min(data.len());
            let (piece, rest) = data.split_at(take);
            data = rest;
            self.got += piece.len() as u64;
            let done = self.got == self.table[self.i].slen;
            let out: Vec<u8> = match self.unf.as_mut() {
                Some(u) => {
                    let mut o = u.feed(piece)?;
                    if done {
                        o.extend_from_slice(&u.finish()?);
                    }
                    o
                }
                None => piece.to_vec(),
            };
            let mut h = crc32fast::Hasher::new_with_initial(self.crc);
            h.update(&out);
            self.crc = h.finalize();
            if self.open {
                self.sink.write(self.base + self.i, &out)?;
            }
            if done {
                self.end()?;
                self.i += 1;
                self.advance()?;
            }
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<()> {
        if self.i != self.table.len() {
            return err("archive is truncated (data ends early)");
        }
        Ok(())
    }
}
