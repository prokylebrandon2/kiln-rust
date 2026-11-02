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
        })
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
            out.extend_from_slice(&self.dec(&chunk));
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
