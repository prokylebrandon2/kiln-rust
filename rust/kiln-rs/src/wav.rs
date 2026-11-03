//! WAV-family unfilters (table filter ids 1-11, 16-62, 64-110). Port of the python `Wav` class
//! (decode direction only) and of kz_wav16_dec / kz_shuf_dec / kz_int_dec / kz_i2f.
//! Same length in and out; `feed` may return less than it was given (it buffers).

use crate::container::KilnError;

pub const WAV_FRAMES: usize = 1 << 18;
pub const WAV_HEAD: usize = 1 << 16;

type Result<T> = std::result::Result<T, KilnError>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Shuf,
    W16,
    Int,
    Fint,
}

/// python `wav_spec`: (kind, sample bytes in file, order, ms, zigzag, bias, scale bits)
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub kind: Kind,
    pub w: usize,
    pub order: u32,
    pub ms: bool,
    pub zz: bool,
    pub bias: u16,
    pub s: u32,
}

pub fn wav_spec(fid: u16) -> Option<Spec> {
    if fid == 9 || fid == 10 || fid == 11 {
        let w = match fid {
            9 => 4,
            10 => 3,
            _ => 8,
        };
        return Some(Spec { kind: Kind::Shuf, w, order: 0, ms: false, zz: false, bias: 0, s: 0 });
    }
    if (1..=8).contains(&fid) {
        let g = (fid - 1) as u32;
        return Some(Spec {
            kind: Kind::W16,
            w: 2,
            order: 1 + (g & 1),
            ms: (g & 2) != 0,
            zz: false,
            bias: if (g & 4) != 0 { 128 } else { 0 },
            s: 0,
        });
    }
    if (16..=62).contains(&fid) || (64..=110).contains(&fid) {
        let base: u16 = if fid < 64 { 16 } else { 64 };
        let g = (fid - base) as u32;
        let order = g & 3;
        let ms = ((g >> 2) & 1) != 0;
        let zz = ((g >> 3) & 1) != 0;
        let wc = g >> 4;
        if order > 2 || wc > 2 {
            return None;
        }
        if base == 16 {
            let w = [3usize, 4, 1][wc as usize];
            return Some(Spec { kind: Kind::Int, w, order, ms, zz, bias: 0, s: 0 });
        }
        let s = [7u32, 15, 23][wc as usize];
        return Some(Spec { kind: Kind::Fint, w: 4, order, ms, zz, bias: 0, s });
    }
    None
}

// ---------------------------------------------------------------- RIFF layout
/// python `wav_layout`
#[derive(Debug, PartialEq)]
pub enum Layout {
    Need,
    No,
    /// data_start, region_bytes, channels, block_align, bytes_per_sample, is_float
    Ok(usize, usize, usize, usize, usize, bool),
}

fn le16(h: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([h[p], h[p + 1]])
}
fn le32(h: &[u8], p: usize) -> u32 {
    u32::from_le_bytes([h[p], h[p + 1], h[p + 2], h[p + 3]])
}

pub fn wav_layout(head: &[u8], size: u64) -> Layout {
    if head.len() < 12 {
        return if size >= 12 { Layout::Need } else { Layout::No };
    }
    if &head[..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Layout::No;
    }
    let mut pos: u64 = 12;
    // (channels, align, bytes per sample, float)
    let mut fmt: Option<(usize, usize, usize, bool)> = None;
    loop {
        if pos > WAV_HEAD as u64 {
            return Layout::No;
        }
        let p = pos as usize;
        if head.len() < p + 8 {
            return if size >= pos + 8 && head.len() < WAV_HEAD { Layout::Need } else { Layout::No };
        }
        let cid = &head[p..p + 4];
        let csz = le32(head, p + 4) as u64;
        if cid == b"fmt " {
            if csz < 16 {
                return Layout::No;
            }
            let need = p + 8 + if csz >= 40 { 26 } else { 16 };
            if head.len() < need {
                return if head.len() < WAV_HEAD { Layout::Need } else { Layout::No };
            }
            let mut tag = le16(head, p + 8);
            let ch = le16(head, p + 10) as usize;
            let align = le16(head, p + 20) as usize;
            let bits = le16(head, p + 22) as usize;
            if tag == 0xFFFE {
                tag = if csz >= 40 { le16(head, p + 8 + 24) } else { 1 };
            }
            if tag != 1 && tag != 3 {
                return Layout::No;
            }
            let ok = if bits == 8 || bits == 16 { tag == 1 } else { bits == 24 || bits == 32 || bits == 64 };
            if !ok || !(1..=8).contains(&ch) || align != (bits / 8) * ch {
                return Layout::No;
            }
            fmt = Some((ch, align, bits / 8, tag == 3));
        } else if cid == b"data" {
            let f = match fmt {
                None => return Layout::No,
                Some(f) => f,
            };
            let start = pos + 8;
            if size < start {
                return Layout::No;
            }
            let dlen = csz.min(size - start);
            let region = (dlen / f.1 as u64) * f.1 as u64;
            return Layout::Ok(start as usize, region as usize, f.0, f.1, f.2, f.3);
        }
        pos += 8 + csz + (csz & 1);
    }
}

// ---------------------------------------------------------------- sample kernels (decode)
fn wmask(w: usize) -> u32 {
    if w >= 4 {
        0xFFFF_FFFF
    } else {
        (1u32 << (8 * w)) - 1
    }
}

/// kz_wav16_dec. `inp` = per channel a low plane then a high plane (n bytes each).
pub fn wav16_dec(inp: &[u8], n: usize, c: usize, order: u32, ms: bool, bias: u16, out: &mut [u8]) {
    let mut t = vec![0u16; n];
    let mut l: Vec<u16> = if ms { vec![0u16; n] } else { Vec::new() };
    for ch in 0..c {
        let lo = &inp[2 * ch * n..2 * ch * n + n];
        let hi = &inp[2 * ch * n + n..2 * ch * n + 2 * n];
        for f in 0..n {
            t[f] = ((lo[f] as u32) | ((hi[f] as u32) << 8)).wrapping_sub(bias as u32) as u16;
        }
        for _ in 0..order {
            for f in 1..n {
                t[f] = t[f].wrapping_add(t[f - 1]);
            }
        }
        if ms && ch == 0 {
            l.copy_from_slice(&t);
        }
        if ms && ch == 1 {
            for f in 0..n {
                t[f] = t[f].wrapping_add(l[f]);
            }
        }
        for f in 0..n {
            let d = 2 * (f * c + ch);
            out[d] = t[f] as u8;
            out[d + 1] = (t[f] >> 8) as u8;
        }
    }
}

/// kz_shuf_dec
pub fn shuf_dec(inp: &[u8], n: usize, w: usize, c: usize, out: &mut [u8]) {
    for f in 0..n {
        for ch in 0..c {
            let d = (f * c + ch) * w;
            for k in 0..w {
                out[d + k] = inp[(ch * w + k) * n + f];
            }
        }
    }
}

/// kz_int_dec
pub fn int_dec(inp: &[u8], n: usize, w: usize, c: usize, order: u32, ms: bool, zz: bool, out: &mut [u8]) {
    let mask = wmask(w);
    let mut t = vec![0u32; n];
    let mut l: Vec<u32> = if ms { vec![0u32; n] } else { Vec::new() };
    for ch in 0..c {
        for f in 0..n {
            let mut v = 0u32;
            for k in 0..w {
                v |= (inp[(ch * w + k) * n + f] as u32) << (8 * k);
            }
            t[f] = v;
        }
        if zz {
            for f in 0..n {
                let z = t[f];
                t[f] = ((z >> 1) ^ if (z & 1) != 0 { mask } else { 0 }) & mask;
            }
        }
        for _ in 0..order {
            for f in 1..n {
                t[f] = t[f].wrapping_add(t[f - 1]) & mask;
            }
        }
        if ms && ch == 0 {
            l.copy_from_slice(&t);
        }
        if ms && ch == 1 {
            for f in 0..n {
                t[f] = t[f].wrapping_add(l[f]) & mask;
            }
        }
        for f in 0..n {
            let d = (f * c + ch) * w;
            for k in 0..w {
                out[d + k] = (t[f] >> (8 * k)) as u8;
            }
        }
    }
}

/// kz_i2f: m signed integers of kw bytes -> m float32 (value / 2^s), little-endian bytes.
pub fn i2f(inp: &[u8], m: usize, s: u32, kw: usize, out: &mut [u8]) {
    let sc = (1u32 << s) as f64;
    let sh = 32 - 8 * kw as u32;
    for i in 0..m {
        let mut v = 0u32;
        for j in 0..kw {
            v |= (inp[i * kw + j] as u32) << (8 * j);
        }
        let k = ((v << sh) as i32) >> sh;
        let x = (k as f64 / sc) as f32;
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_bits().to_le_bytes());
    }
}

// ---------------------------------------------------------------- encode kernels (CP11a)
/// kz_wav16_enc. `inp` = n frames of c interleaved 16-bit samples; `out` = per channel low plane, high plane.
pub fn wav16_enc(inp: &[u8], n: usize, c: usize, order: u32, ms: bool, bias: u16, out: &mut [u8]) {
    let mut t = vec![0u16; n];
    let mut l: Vec<u16> = Vec::new();
    if ms {
        l = (0..n).map(|f| (inp[2 * (f * c)] as u16) | ((inp[2 * (f * c) + 1] as u16) << 8)).collect();
    }
    for ch in 0..c {
        for f in 0..n {
            let p = 2 * (f * c + ch);
            t[f] = (inp[p] as u16) | ((inp[p + 1] as u16) << 8);
        }
        if ms && ch == 1 {
            for f in 0..n {
                t[f] = t[f].wrapping_sub(l[f]);
            }
        }
        for _ in 0..order {
            for f in (1..n).rev() {
                t[f] = t[f].wrapping_sub(t[f - 1]);
            }
        }
        let (lo, hi) = out[2 * ch * n..2 * ch * n + 2 * n].split_at_mut(n);
        for f in 0..n {
            let v = t[f].wrapping_add(bias);
            lo[f] = v as u8;
            hi[f] = (v >> 8) as u8;
        }
    }
}

/// kz_shuf_enc
pub fn shuf_enc(inp: &[u8], n: usize, w: usize, c: usize, out: &mut [u8]) {
    for f in 0..n {
        for ch in 0..c {
            let s = (f * c + ch) * w;
            for k in 0..w {
                out[(ch * w + k) * n + f] = inp[s + k];
            }
        }
    }
}

/// kz_int_enc
pub fn int_enc(inp: &[u8], n: usize, w: usize, c: usize, order: u32, ms: bool, zz: bool, out: &mut [u8]) {
    let mask = wmask(w);
    let sb = (8 * w - 1) as u32;
    let mut t = vec![0u32; n];
    let mut l: Vec<u32> = if ms { vec![0u32; n] } else { Vec::new() };
    for ch in 0..c {
        for f in 0..n {
            let s = (f * c + ch) * w;
            let mut v = 0u32;
            for k in 0..w {
                v |= (inp[s + k] as u32) << (8 * k);
            }
            t[f] = v;
        }
        if ms && ch == 0 {
            l.copy_from_slice(&t);
        }
        if ms && ch == 1 {
            for f in 0..n {
                t[f] = t[f].wrapping_sub(l[f]) & mask;
            }
        }
        for _ in 0..order {
            for f in (1..n).rev() {
                t[f] = t[f].wrapping_sub(t[f - 1]) & mask;
            }
        }
        if zz {
            for f in 0..n {
                let v = t[f];
                t[f] = ((v << 1) & mask) ^ if ((v >> sb) & 1) != 0 { mask } else { 0 };
            }
        }
        for k in 0..w {
            let pl = &mut out[(ch * w + k) * n..(ch * w + k) * n + n];
            for f in 0..n {
                pl[f] = (t[f] >> (8 * k)) as u8;
            }
        }
    }
}

/// kz_f2i: m float32 -> m signed integers of kw bytes (value * 2^s). false = a sample does not convert exactly.
pub fn f2i(inp: &[u8], m: usize, s: u32, kw: usize, out: &mut [u8]) -> bool {
    let sc = (1u32 << s) as f64;
    for i in 0..m {
        let u = u32::from_le_bytes([inp[4 * i], inp[4 * i + 1], inp[4 * i + 2], inp[4 * i + 3]]);
        if u == 0x8000_0000 {
            return false;
        }
        let v = (f32::from_bits(u) as f64) * sc;
        if !(v >= -sc && v <= sc - 1.0) {
            return false;
        }
        let k = v as i32;
        if (k as f64) != v {
            return false;
        }
        let q = k as u32;
        for j in 0..kw {
            out[i * kw + j] = (q >> (8 * j)) as u8;
        }
    }
    true
}

// ---------------------------------------------------------------- streaming decoder
pub struct Wav {
    kind: Kind,
    w: usize,
    order: u32,
    ms: bool,
    zz: bool,
    bias: u16,
    s: u32,
    kw: usize,
    size: u64,
    buf: Vec<u8>,
    state: u8, // 0 = waiting for header, 1 = data, 2 = raw tail
    left: usize,
    start: usize,
    c: usize,
    align: usize,
    encode: bool,
}

impl Wav {
    /// python `Wav(fid, size, decode=True)`
    pub fn new(fid: u16, size: u64) -> Result<Wav> {
        let sp = match wav_spec(fid) {
            Some(sp) => sp,
            None => return Err(KilnError("archive uses features from a newer kiln".to_string())),
        };
        let kw = if sp.kind == Kind::Fint { (sp.s as usize + 1 + 7) / 8 } else { sp.w };
        Ok(Wav {
            kind: sp.kind,
            w: sp.w,
            order: sp.order,
            ms: sp.ms,
            zz: sp.zz,
            bias: sp.bias,
            s: sp.s,
            kw,
            size,
            buf: Vec::new(),
            state: 0,
            left: 0,
            start: 0,
            c: 0,
            align: 1,
            encode: false,
        })
    }

    /// python `Wav(fid, size, decode=False)`: file bytes -> filtered bytes
    pub fn new_enc(fid: u16, size: u64) -> Result<Wav> {
        let mut w = Wav::new(fid, size)?;
        w.encode = true;
        Ok(w)
    }

    /// number of byte planes per chunk (python `nplanes`); valid after the header was seen
    pub fn nplanes(&self) -> usize {
        self.c * if self.kind == Kind::W16 { 2 } else { self.w }
    }

    /// For the filter picker: set channel count and frame size by hand (python `f.C, f.align = C, align`).
    pub fn set_layout(&mut self, c: usize, align: usize) {
        self.c = c;
        self.align = align;
    }

    /// python `_enc` on one chunk of whole frames.
    pub fn enc_chunk(&self, chunk: &[u8]) -> Result<Vec<u8>> {
        let c = self.c;
        let mut out = vec![0u8; chunk.len()];
        match self.kind {
            Kind::Int => {
                int_enc(chunk, chunk.len() / self.align, self.w, c, self.order, self.ms, self.zz, &mut out);
            }
            Kind::Fint => {
                let n = chunk.len() / self.align;
                let mut ints = vec![0u8; self.kw * c * n];
                if !f2i(chunk, n * c, self.s, self.kw, &mut ints) {
                    return Err(KilnError("float samples are not whole-number audio (file changed?)".to_string()));
                }
                let mut enc = vec![0u8; ints.len()];
                int_enc(&ints, n, self.kw, c, self.order, self.ms, self.zz, &mut enc);
                out[..enc.len()].copy_from_slice(&enc); // the rest stays zero
            }
            Kind::Shuf => {
                shuf_enc(chunk, chunk.len() / self.align, self.w, c, &mut out);
            }
            Kind::W16 => {
                wav16_enc(chunk, chunk.len() / (2 * c), c, self.order, self.ms, self.bias, &mut out);
            }
        }
        Ok(out)
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut out: Vec<u8> = Vec::new();
        if self.state == 0 {
            let hl = self.buf.len().min(WAV_HEAD);
            match wav_layout(&self.buf[..hl], self.size) {
                Layout::Need => return Ok(Vec::new()),
                Layout::No => return Err(KilnError("corrupt data: bad WAV header for the filter".to_string())),
                Layout::Ok(start, left, c, align, bps, _fl) => {
                    if c < 1 || (self.ms && c != 2) || bps != self.w {
                        return Err(KilnError("corrupt data: bad WAV header for the filter".to_string()));
                    }
                    self.start = start;
                    self.left = left;
                    self.c = c;
                    self.align = align;
                }
            }
            out.extend_from_slice(&self.buf[..self.start]);
            self.buf.drain(..self.start);
            self.state = 1;
        }
        let step = WAV_FRAMES * self.align;
        while self.state == 1 {
            if self.left == 0 {
                self.state = 2;
                break;
            }
            let take = step.min(self.left);
            if self.buf.len() < take {
                break;
            }
            let chunk: Vec<u8> = self.buf.drain(..take).collect();
            if self.encode {
                out.extend_from_slice(&self.enc_chunk(&chunk)?);
            } else {
                out.extend_from_slice(&self.dec(&chunk));
            }
            self.left -= take;
        }
        if self.state == 2 {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
        }
        Ok(out)
    }

    pub fn finish(&mut self) -> Result<Vec<u8>> {
        let mut out = self.feed(&[])?;
        if !self.buf.is_empty() {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
        }
        Ok(out)
    }

    fn dec(&self, chunk: &[u8]) -> Vec<u8> {
        let c = self.c;
        let mut out = vec![0u8; chunk.len()];
        match self.kind {
            Kind::Int => {
                int_dec(chunk, chunk.len() / self.align, self.w, c, self.order, self.ms, self.zz, &mut out);
            }
            Kind::Fint => {
                let n = chunk.len() / self.align;
                let mut ints = vec![0u8; self.kw * c * n];
                int_dec(&chunk[..self.kw * c * n], n, self.kw, c, self.order, self.ms, self.zz, &mut ints);
                i2f(&ints, n * c, self.s, self.kw, &mut out);
            }
            Kind::Shuf => {
                shuf_dec(chunk, chunk.len() / self.align, self.w, c, &mut out);
            }
            Kind::W16 => {
                wav16_dec(chunk, chunk.len() / (2 * c), c, self.order, self.ms, self.bias, &mut out);
            }
        }
        out
    }
}

#[cfg(test)]
mod enc_tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u8 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 24) as u8
    }

    /// every kernel: encode then decode gives the input back
    #[test]
    fn kernels_round_trip() {
        let mut seed = 7u32;
        for &n in &[0usize, 1, 2, 331] {
            for c in 1..=3usize {
                // 16-bit
                let buf: Vec<u8> = (0..n * c * 2).map(|_| lcg(&mut seed)).collect();
                for order in 0..=2u32 {
                    for &ms in &[false, true] {
                        if ms && c != 2 {
                            continue;
                        }
                        for &bias in &[0u16, 128] {
                            let mut e = vec![0u8; buf.len()];
                            wav16_enc(&buf, n, c, order, ms, bias, &mut e);
                            let mut d = vec![0u8; buf.len()];
                            wav16_dec(&e, n, c, order, ms, bias, &mut d);
                            assert_eq!(d, buf);
                        }
                    }
                }
                // integer widths 1, 3, 4 and byte planes
                for &w in &[1usize, 3, 4] {
                    let buf: Vec<u8> = (0..n * c * w).map(|_| lcg(&mut seed)).collect();
                    let mut e = vec![0u8; buf.len()];
                    shuf_enc(&buf, n, w, c, &mut e);
                    let mut d = vec![0u8; buf.len()];
                    shuf_dec(&e, n, w, c, &mut d);
                    assert_eq!(d, buf);
                    for order in 0..=2u32 {
                        for &ms in &[false, true] {
                            if ms && c != 2 {
                                continue;
                            }
                            for &zz in &[false, true] {
                                let mut e = vec![0u8; buf.len()];
                                int_enc(&buf, n, w, c, order, ms, zz, &mut e);
                                let mut d = vec![0u8; buf.len()];
                                int_dec(&e, n, w, c, order, ms, zz, &mut d);
                                assert_eq!(d, buf);
                            }
                        }
                    }
                }
            }
        }
    }

    /// i2f then f2i is the identity on integers in range
    #[test]
    fn float_round_trip() {
        for &(s, kw) in &[(7u32, 1usize), (15, 2), (23, 3)] {
            let m = 500usize;
            let mut seed = 99u32;
            let ints: Vec<u8> = (0..m * kw).map(|_| lcg(&mut seed)).collect();
            let mut fl = vec![0u8; m * 4];
            i2f(&ints, m, s, kw, &mut fl);
            let mut back = vec![0u8; m * kw];
            assert!(f2i(&fl, m, s, kw, &mut back));
            assert_eq!(back, ints);
        }
        // -0.0 and NaN refuse
        let mut o = vec![0u8; 3];
        assert!(!f2i(&0x8000_0000u32.to_le_bytes(), 1, 15, 2, &mut o));
        assert!(!f2i(&0x7fc0_0000u32.to_le_bytes(), 1, 15, 2, &mut o));
        assert!(f2i(&0.5f32.to_le_bytes(), 1, 15, 2, &mut o));
        assert!(!f2i(&(1.0f32 / 65536.0).to_le_bytes(), 1, 15, 2, &mut o));
    }
}

// ---------------------------------------------------------------- filter picker (CP11b)
// Port of python `pick_wav_filter` with its helpers (_entropy_bits, _float_scale, _float_verified).
use std::io::{Read, Seek, SeekFrom};

fn int_fid(width: usize, order: u32, ms: bool, zz: bool) -> u16 {
    let wc = match width {
        3 => 0,
        4 => 1,
        _ => 2,
    };
    (16 + order + 4 * ms as u32 + 8 * zz as u32 + 16 * wc) as u16
}

fn fint_fid(s: u32, order: u32, ms: bool, zz: bool) -> u16 {
    let wc = match s {
        7 => 0,
        15 => 1,
        _ => 2,
    };
    (64 + order + 4 * ms as u32 + 8 * zz as u32 + 16 * wc) as u16
}

/// python SHUF_ID: bytes per sample -> byte-plane filter id
fn shuf_id(width: usize) -> Option<u16> {
    match width {
        4 => Some(9),
        3 => Some(10),
        8 => Some(11),
        _ => None,
    }
}

/// python `_entropy_bits` (same summation order over byte values 0..255)
fn entropy_bits(b: &[u8]) -> f64 {
    let n = b.len();
    if n == 0 {
        return 0.0;
    }
    let mut cnt = [0usize; 256];
    for &x in b {
        cnt[x as usize] += 1;
    }
    let inv = 1.0 / n as f64;
    let mut ent = 0.0f64;
    for v in 0..256 {
        let k = cnt[v];
        if k > 0 {
            let p = k as f64 * inv;
            ent -= p * p.log2();
        }
    }
    ent * n as f64
}

fn f2i_ok(w: &[u8], s: u32) -> bool {
    let kw = ((s + 8) / 8) as usize;
    let m = w.len() / 4;
    let mut out = vec![0u8; m * kw + 1];
    f2i(w, m, s, kw, &mut out)
}

fn float_scale(samples: &[Vec<u8>]) -> u32 {
    for &s in &[7u32, 15, 23] {
        if samples.iter().all(|w| f2i_ok(w, s)) {
            return s;
        }
    }
    0
}

fn float_verified(fh: &mut std::fs::File, start: usize, region: usize, s: u32) -> bool {
    if fh.seek(SeekFrom::Start(start as u64)).is_err() {
        return false;
    }
    let mut left = region;
    while left > 0 {
        let want = left.min(1 << 22);
        let mut w = vec![0u8; want];
        let mut got = 0;
        while got < want {
            match fh.read(&mut w[got..]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(_) => return false,
            }
        }
        w.truncate(got);
        if w.is_empty() {
            return false;
        }
        let l4 = w.len() / 4 * 4;
        w.truncate(l4);
        if w.is_empty() || !f2i_ok(&w, s) {
            return false;
        }
        left -= w.len();
    }
    true
}

fn read_full(fh: &mut std::fs::File, n: usize) -> Vec<u8> {
    let mut w = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match fh.read(&mut w[got..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => got += k,
        }
    }
    w.truncate(got);
    w
}

/// python `pick_wav_filter`: 0 = don't filter, else the filter id with the lowest estimated cost.
pub fn pick_wav_filter(path: &std::path::Path, size: u64) -> u16 {
    use crate::walk::WAV_MIN;
    if size < WAV_MIN {
        return 0;
    }
    let mut fh = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let head = read_full(&mut fh, WAV_HEAD);
    let (start, region, c, align, width, isfloat) = match wav_layout(&head, size) {
        Layout::Ok(a, b, c, d, e, f) => (a, b, c, d, e, f),
        _ => return 0,
    };
    if (region as u64) < WAV_MIN {
        return 0;
    }
    let win = 16384usize.min(region / align / 4) * align;
    let mut samples: Vec<Vec<u8>> = Vec::new();
    for q in 1..=3usize {
        let off = start + (region * q / 4) / align * align;
        if fh.seek(SeekFrom::Start(off as u64)).is_err() {
            return 0;
        }
        let w = read_full(&mut fh, win);
        if w.len() == win {
            samples.push(w);
        }
    }
    if samples.is_empty() {
        return 0;
    }
    let mut cands: Vec<u16> = Vec::new();
    if width == 2 && !isfloat {
        if c != 2 {
            cands = vec![1, 2, 5, 6];
        } else {
            cands = vec![1, 2, 3, 4, 5, 6, 7, 8];
        }
    } else if (width == 1 || width == 3 || width == 4) && !isfloat {
        for order in 1..=2u32 {
            for &zz in &[false, true] {
                let mss: &[bool] = if c == 2 { &[false, true] } else { &[false] };
                for &ms in mss {
                    cands.push(int_fid(width, order, ms, zz));
                }
            }
        }
        if c == 2 {
            for &zz in &[false, true] {
                cands.push(int_fid(width, 0, true, zz));
            }
        }
        if let Some(f) = shuf_id(width) {
            cands.push(f);
        }
    } else {
        if let Some(f) = shuf_id(width) {
            cands.push(f);
        }
        if width == 4 && isfloat {
            let s = float_scale(&samples);
            if s != 0 && float_verified(&mut fh, start, region, s) {
                for order in 1..=2u32 {
                    for &zz in &[false, true] {
                        let mss: &[bool] = if c == 2 { &[false, true] } else { &[false] };
                        for &ms in mss {
                            cands.push(fint_fid(s, order, ms, zz));
                        }
                    }
                }
                if c == 2 {
                    for &zz in &[false, true] {
                        cands.push(fint_fid(s, 0, true, zz));
                    }
                }
            }
        }
    }
    let mut base = 0.0f64;
    for w in &samples {
        base += entropy_bits(w);
    }
    let mut best: u16 = 0;
    let mut best_cost = base * 0.97;
    for fid in cands {
        let mut f = match Wav::new_enc(fid, 1u64 << 40) {
            Ok(f) => f,
            Err(_) => continue,
        };
        f.set_layout(c, align);
        let np = f.nplanes();
        let mut cost = 0.0f64;
        let mut failed = false;
        for w in &samples {
            let e = match f.enc_chunk(w) {
                Ok(e) => e,
                Err(_) => {
                    failed = true;
                    break;
                }
            };
            let m = w.len() / align;
            let mut inner = 0.0f64;
            for k in 0..np {
                inner += entropy_bits(&e[k * m..(k + 1) * m]);
            }
            cost += inner;
        }
        if failed {
            continue;
        }
        if cost < best_cost {
            best = fid;
            best_cost = cost;
        }
    }
    best
}
