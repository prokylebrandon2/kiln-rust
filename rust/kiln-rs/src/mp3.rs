//! MP3 unpack (table filter 120, "MP3 recompressed"). Port of the decode direction of kz_mp3_unpack
//! (mp3codec.c + mp3core.c's build_file, in `_MP3_SRC` of kiln_v14.py).
//!
//! Everything in the model is integer arithmetic (no floats), so the port is bit-exact by
//! construction; the C code's int / uint32 semantics are kept (wrapping where C wraps).
//!
//! Differences from the C code that do not change the result:
//!  * streaming: frames are written straight into the output as they are decoded and the
//!    bit-reservoir bit stream is kept per run, instead of keeping every decoded frame
//!    (the C decoder needs roughly 10x the file size; this needs about 2x);
//!  * only the decode side exists (no parser, no Huffman tries, no encoder).
//! Any inconsistency is reported as an error (the C code returns a negative code, which the
//! Python wrapper turns into "corrupt data: bad packed MP3").

use crate::mp3_tables::HT;

pub(crate) const BR: [i32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
pub(crate) const SR: [i32; 3] = [44100, 48000, 32000];
pub(crate) const SFB: [[i32; 23]; 3] = [
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 52, 62, 74, 90, 110, 134, 162, 196, 238, 288, 342, 418, 576],
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 42, 50, 60, 72, 88, 106, 128, 156, 190, 230, 276, 330, 384, 576],
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 54, 66, 82, 102, 126, 156, 194, 240, 296, 364, 448, 550, 576],
];
pub(crate) const SLEN: [[i32; 2]; 16] = [
    [0, 0], [0, 1], [0, 2], [0, 3], [3, 0], [1, 1], [1, 2], [1, 3],
    [2, 1], [2, 2], [2, 3], [3, 1], [3, 2], [3, 3], [4, 2], [4, 3],
];
pub(crate) const PRETAB: [u8; 22] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 3, 2, 0];
pub(crate) const MULT16: [u32; 16] = [
    65536, 68438, 71468, 74632, 77936, 81386, 84990, 88752, 92682, 96785, 101070, 105545, 110218, 115098, 120194, 125515,
];
pub(crate) const QMT2: [u8; 102] = [0,1,2,3,4,5,5,6,6,7,7,7,7,8,8,8,8,8,8,8,8,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,11];
pub(crate) const NQ: usize = 12;
pub(crate) const NS: usize = 16;
pub(crate) const NMX: usize = 4;
pub(crate) const WCLAMP: i32 = 6 << 16;

// ---------------------------------------------------------------- logistic helpers (const tables)
const fn squash(d: i32) -> i32 {
    const T: [i32; 33] = [
        1, 2, 3, 6, 10, 16, 27, 45, 73, 120, 194, 310, 488, 747, 1101, 1546, 2047, 2549, 2994, 3348, 3607, 3785, 3901,
        3975, 4022, 4050, 4068, 4079, 4085, 4089, 4092, 4093, 4094,
    ];
    if d > 2047 {
        return 4095;
    }
    if d < -2047 {
        return 1;
    }
    let w = d & 127;
    let d = (d >> 7) + 16;
    (T[d as usize] * (128 - w) + T[d as usize + 1] * w + 64) >> 7
}
pub(crate) const SQT: [i16; 4096] = {
    let mut t = [0i16; 4096];
    let mut x = -2048;
    while x < 2048 {
        t[(x + 2048) as usize] = squash(x) as i16;
        x += 1;
    }
    t
};
pub(crate) const STRETCH: [i16; 4096] = {
    let mut t = [0i16; 4096];
    let mut pi: i32 = 0;
    let mut x = -2047;
    while x <= 2047 {
        let v = squash(x);
        let mut i = pi;
        while i <= v {
            t[i as usize] = x as i16;
            i += 1;
        }
        pi = v + 1;
        x += 1;
    }
    let mut i = pi;
    while i < 4096 {
        t[i as usize] = 2047;
        i += 1;
    }
    t
};
pub(crate) const DT: [i32; 1024] = {
    let mut t = [0i32; 1024];
    let mut i = 0;
    while i < 1024 {
        t[i] = 16384 / (i as i32 + i as i32 + 3);
        i += 1;
    }
    t
};

// ---------------------------------------------------------------- range decoder
struct Coder<'a> {
    x1: u32,
    x2: u32,
    x: u32,
    buf: &'a [u8],
    pos: usize,
    overflow: bool,
}

impl<'a> Coder<'a> {
    fn new(buf: &'a [u8]) -> Coder<'a> {
        let mut c = Coder { x1: 0, x2: 0xffff_ffff, x: 0, buf, pos: 0, overflow: false };
        for _ in 0..4 {
            let b = if c.pos < buf.len() {
                let b = buf[c.pos];
                c.pos += 1;
                b
            } else {
                0
            };
            c.x = (c.x << 8) | b as u32;
        }
        c
    }
    #[inline]
    fn norm(&mut self) {
        while ((self.x1 ^ self.x2) & 0xff00_0000) == 0 {
            let b = if self.pos < self.buf.len() { self.buf[self.pos] } else { 0 };
            self.x = (self.x << 8) | b as u32;
            self.pos += 1;
            if self.pos > self.buf.len() + 8 {
                self.overflow = true;
            }
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
        }
    }
    /// p = P(bit==1) in 12 bits (1..4095)
    #[inline]
    fn code(&mut self, p: i32) -> i32 {
        let xm = self.x1.wrapping_add((((self.x2.wrapping_sub(self.x1)) as u64 * (p as u32) as u64) >> 12) as u32);
        let bit = self.x <= xm;
        if bit {
            self.x2 = xm;
        } else {
            self.x1 = xm.wrapping_add(1);
        }
        self.norm();
        bit as i32
    }
    /// n (1..12) equiprobable bits in one step; bitwise when the range is too small
    fn raw(&mut self, n: u32) -> i32 {
        let r: u64 = (self.x2.wrapping_sub(self.x1)) as u64 + 1;
        if r < (1u64 << (n + 8)) {
            let mut v: i32 = 0;
            for _ in 0..n {
                v = v.wrapping_mul(2).wrapping_add(self.code(2048));
            }
            return v;
        }
        let d = self.x.wrapping_sub(self.x1) as u64;
        let v = ((((d + 1) << n) - 1) / r) as i32;
        self.x2 = self.x1.wrapping_add(((r * (v.wrapping_add(1) as u64)) >> n) as u32).wrapping_sub(1);
        self.x1 = self.x1.wrapping_add(((r * (v as u64)) >> n) as u32);
        self.norm();
        v
    }
}

// ---------------------------------------------------------------- adaptive slots, mixer
#[inline]
pub(crate) fn sm_p(s: u32) -> i32 {
    let p = (s >> 20) as i32;
    if p < 1 {
        1
    } else if p > 4095 {
        4095
    } else {
        p
    }
}
#[inline]
pub(crate) fn sm_up(s: &mut u32, y: i32, limit: i32) {
    let n = (*s & 1023) as i32;
    let p = (*s >> 10) as i32;
    if n < limit {
        *s = s.wrapping_add(1);
    } else {
        *s = (*s & 0xffff_fc00) | limit as u32;
    }
    let t = ((((y << 22) - p) >> 3).wrapping_mul(DT[n as usize])) as u32 & 0xffff_fc00;
    *s = s.wrapping_add(t);
}
#[inline]
fn code_sm(c: &mut Coder, s: &mut u32, limit: i32) -> i32 {
    let bit = c.code(sm_p(*s));
    sm_up(s, bit, limit);
    bit
}
pub(crate) fn sm_new(n: usize) -> Vec<u32> {
    vec![1u32 << 31; n]
}
/// decode `nb` bits through a binary tree of slots (ctx selects the tree)
fn code_tree(c: &mut Coder, sm: &mut [u32], ctx: usize, nb: u32) -> i32 {
    let mut node: usize = 1;
    for _ in 0..nb {
        let b = code_sm(c, &mut sm[(ctx << nb) + node], 60);
        node = node * 2 + b as usize;
    }
    (node - (1usize << nb)) as i32
}
/// Elias-gamma style number; `sm` has 64 slots
fn code_gamma(c: &mut Coder, sm: &mut [u32]) -> i32 {
    let mut k: usize = 0;
    loop {
        let stop = code_sm(c, &mut sm[if k < 31 { k } else { 31 }], 60);
        if stop != 0 {
            break;
        }
        if k >= 31 {
            break;
        }
        k += 1;
    }
    let nb = k;
    let mut r: u32 = 1;
    for i in (0..nb).rev() {
        let b = if i == nb - 1 {
            code_sm(c, &mut sm[32 + if nb < 31 { nb } else { 31 }], 60)
        } else {
            c.code(2048)
        };
        r = r.wrapping_mul(2).wrapping_add(b as u32);
    }
    r.wrapping_sub(1) as i32
}

pub(crate) struct Mixer {
    pub(crate) ws: Vec<[i32; 8]>,
    pub(crate) st: [i32; 8],
    pub(crate) pr: i32,
    pub(crate) cur: usize,
    pub(crate) shift: u32,
}
impl Mixer {
    fn new(nsets: usize, n: usize, shift: u32) -> Mixer {
        let mut ws = vec![[0i32; 8]; nsets];
        for w in ws.iter_mut() {
            for i in 0..n - 1 {
                w[i] = 65536 * 3 / ((n as i32 - 1) * 2);
            }
        }
        Mixer { ws, st: [0; 8], pr: 0, cur: 0, shift }
    }
    #[inline]
    pub(crate) fn mix(&mut self, set: usize) -> i32 {
        self.cur = set;
        let w = &self.ws[set];
        let mut d: i32 = 0;
        for i in 0..NMX {
            d = d.wrapping_add(self.st[i].wrapping_mul(w[i] >> 2));
        }
        let mut x = d >> 14;
        if x > 2047 {
            x = 2047;
        }
        if x < -2047 {
            x = -2047;
        }
        self.pr = SQT[(x + 2048) as usize] as i32;
        self.pr
    }
    #[inline]
    pub(crate) fn up(&mut self, y: i32) {
        let err = (y << 12) - self.pr;
        if err < 24 && err > -24 {
            return;
        }
        let w = &mut self.ws[self.cur];
        for i in 0..NMX {
            let mut v = w[i].wrapping_add(self.st[i].wrapping_mul(err) >> self.shift);
            if v > WCLAMP {
                v = WCLAMP;
            }
            if v < -WCLAMP {
                v = -WCLAMP;
            }
            w[i] = v;
        }
    }
}

pub(crate) struct Model {
    pub(crate) hdr: Vec<u32>,
    pub(crate) mdb: Vec<u32>,
    pub(crate) privb: Vec<u32>,
    pub(crate) scfsi: Vec<u32>,
    pub(crate) bt: Vec<u32>,
    pub(crate) ts: Vec<u32>,
    pub(crate) sbg: Vec<u32>,
    pub(crate) r0: Vec<u32>,
    pub(crate) r1: Vec<u32>,
    pub(crate) flags: Vec<u32>,
    pub(crate) sfc: Vec<u32>,
    pub(crate) gg: Vec<u32>,
    pub(crate) bv: Vec<u32>,
    pub(crate) nq: Vec<u32>,
    pub(crate) sf: Vec<u32>,
    pub(crate) stuffz: Vec<u32>,
    pub(crate) stuffb: Vec<u32>,
    pub(crate) gapz: Vec<u32>,
    pub(crate) gapb: Vec<u32>,
    pub(crate) segf: Vec<u32>,
    pub(crate) rawb: Vec<u32>,
    pub(crate) gam: Vec<u32>,
    pub(crate) crc: Vec<u32>,
    pub(crate) cb: Vec<u32>,
    pub(crate) cd: Vec<u32>,
    pub(crate) cf: Vec<u32>,
    pub(crate) mant: Vec<u32>,
    pub(crate) zq: Vec<u32>,
    pub(crate) mx: Mixer,
}
impl Model {
    pub(crate) fn new() -> Model {
        Model {
            hdr: sm_new(4 * 256 * 256),
            mdb: sm_new(16 * 512 * 2),
            privb: sm_new(64),
            scfsi: sm_new(16 * 16 * 2),
            bt: sm_new(16 * 16 * 2),
            ts: sm_new(3 * 32 * 32 * 32 * 2),
            sbg: sm_new(3 * 8 * 8 * 2),
            r0: sm_new(16 * 16 * 2),
            r1: sm_new(8 * 8 * 2),
            flags: sm_new(64),
            sfc: sm_new(16 * 16 * 2),
            gg: sm_new(256 * 2 * 2 * 2),
            bv: sm_new(512 * 2 * 2 * 2 * 8),
            nq: sm_new(256 * 16 * 2 * 2),
            sf: sm_new(39 * 16 * 16 * 2),
            stuffz: sm_new(4),
            stuffb: sm_new(8),
            gapz: sm_new(4),
            gapb: sm_new(8),
            segf: sm_new(8),
            rawb: sm_new(256 * 256),
            gam: sm_new(64 * 8),
            crc: sm_new(256),
            cb: sm_new(NQ * NQ * NS),
            cd: sm_new(2 * NQ * NQ * NS),
            cf: sm_new(4 * 20 * NQ * NS),
            mant: sm_new(16 * NQ * 4),
            zq: sm_new(2 * 4 * 2 * 2 * 72),
            mx: Mixer::new(NS * 2 * 2, NMX, 10),
        }
    }
}

// ---------------------------------------------------------------- frame data
#[derive(Clone)]
pub(crate) struct Gran {
    pub(crate) p23: i32,
    pub(crate) bv: i32,
    pub(crate) gg: i32,
    pub(crate) sfc: i32,
    pub(crate) ws: i32,
    pub(crate) bt: i32,
    pub(crate) mixed: i32,
    pub(crate) ts: [i32; 3],
    pub(crate) sbg: [i32; 3],
    pub(crate) r0: i32,
    pub(crate) r1: i32,
    pub(crate) pre: i32,
    pub(crate) sfs: i32,
    pub(crate) c1t: i32,
    pub(crate) sf: [i32; 39],
    pub(crate) nq: i32,
    pub(crate) stuff_n: i32,
    pub(crate) xr: [i16; 576],
    pub(crate) l4: [i16; 576],
    pub(crate) lvalid: bool,
    pub(crate) stuff_off: u32,
}
impl Gran {
    pub(crate) fn new() -> Gran {
        Gran {
            p23: 0, bv: 0, gg: 0, sfc: 0, ws: 0, bt: 0, mixed: 0, ts: [0; 3], sbg: [0; 3], r0: 0, r1: 0, pre: 0,
            sfs: 0, c1t: 0, sf: [0; 39], nq: 0, stuff_n: 0, stuff_off: 0, xr: [0; 576], l4: [0; 576], lvalid: false,
        }
    }
}
pub(crate) struct Frame {
    pub(crate) hdr: [u8; 4],
    pub(crate) crc: [u8; 2],
    pub(crate) has_crc: bool,
    pub(crate) mdb: i32,
    pub(crate) privb: i32,
    pub(crate) nch: usize,
    pub(crate) pay: usize,
    pub(crate) scfsi: [[u8; 4]; 2],
    pub(crate) gr: [[Gran; 2]; 2],
    pub(crate) gap_n: u32,
    pub(crate) gap_off: u32,
}
impl Frame {
    pub(crate) fn new() -> Frame {
        Frame {
            hdr: [0; 4], crc: [0; 2], has_crc: false, mdb: 0, privb: 0, nch: 0, pay: 0, scfsi: [[0; 4]; 2], gap_n: 0, gap_off: 0,
            gr: [[Gran::new(), Gran::new()], [Gran::new(), Gran::new()]],
        }
    }
    fn reset(&mut self) {
        // like calloc: every field back to zero (mono frames leave channel 1 untouched)
        *self = Frame::new();
    }
}

pub(crate) fn hdr_ok(h: &[u8; 4]) -> bool {
    if h[0] != 0xFF || (h[1] & 0xFE) != 0xFA {
        return false;
    }
    let br = (h[2] >> 4) as usize;
    let s = (h[2] >> 2) & 3;
    !(br == 0 || br == 15 || s == 3)
}

// ---------------------------------------------------------------- granule geometry
pub(crate) fn regions(g: &Gran, sr: usize) -> (i32, i32) {
    if g.ws != 0 {
        (36, 576)
    } else {
        let a = (g.r0 + 1) as usize;
        let b = (g.r0 + g.r1 + 2) as usize;
        (SFB[sr][a.min(22)], SFB[sr][b.min(22)])
    }
}
pub(crate) fn sf_layout(g: &Gran, slen: &mut [i32; 39]) -> usize {
    let mut n = 0;
    let s = SLEN[g.sfc as usize];
    if g.ws != 0 && g.bt == 2 {
        if g.mixed != 0 {
            for _ in 0..17 {
                slen[n] = s[0];
                n += 1;
            }
        } else {
            for _ in 0..18 {
                slen[n] = s[0];
                n += 1;
            }
        }
        for _ in 0..18 {
            slen[n] = s[1];
            n += 1;
        }
    } else {
        for _ in 0..11 {
            slen[n] = s[0];
            n += 1;
        }
        for _ in 0..10 {
            slen[n] = s[1];
            n += 1;
        }
    }
    n
}
pub(crate) fn sf_shared(scfsi: &[[u8; 4]; 2], gri: usize, ch: usize, g: &Gran, idx: usize) -> bool {
    if gri == 0 || (g.ws != 0 && g.bt == 2) {
        return false;
    }
    let grp = if idx < 6 {
        0
    } else if idx < 11 {
        1
    } else if idx < 16 {
        2
    } else {
        3
    };
    scfsi[ch][grp] != 0
}

/// decoder-side sanity check: only granules that `enc_gran` can write without leaving its tables
pub(crate) fn gran_valid(g: &Gran, sr: usize) -> bool {
    if g.bv > 288 || g.nq < 0 || g.bv * 2 + g.nq * 4 > 576 {
        return false;
    }
    let (r1s, r2s) = regions(g, sr);
    let nbig = (g.bv * 2) as usize;
    let mut i = 0usize;
    let mut reg = 0;
    while reg < 3 && i < nbig {
        let mut e = if reg == 0 { r1s } else if reg == 1 { r2s } else { 576 } as usize;
        if e > nbig {
            e = nbig;
        }
        let t = g.ts[reg];
        if t == 0 {
            i = e.max(i);
            reg += 1;
            continue;
        }
        if !(0..=31).contains(&t) {
            return false;
        }
        let (xlen, linbits, code, len) = HT[t as usize];
        if code.is_none() || len.is_none() || xlen == 0 {
            return false;
        }
        while i < e {
            let ax = (g.xr[i] as i32).abs();
            let ay = (g.xr[i + 1] as i32).abs();
            if linbits == 0 && (ax >= xlen as i32 || ay >= xlen as i32) {
                return false;
            }
            i += 2;
        }
        reg += 1;
    }
    true
}

/// bits the granule occupies in the main data (part2 + part3 + stuffing)
pub(crate) fn gran_bits(g: &Gran, scfsi: &[[u8; 4]; 2], gri: usize, ch: usize, sr: usize) -> i32 {
    let mut slen = [0i32; 39];
    let n = sf_layout(g, &mut slen);
    let mut tot: i32 = 0;
    for i in 0..n {
        if sf_shared(scfsi, gri, ch, g, i) {
            continue;
        }
        tot += slen[i];
    }
    let (r1s, r2s) = regions(g, sr);
    let nbig = (g.bv * 2) as usize;
    let mut i = 0usize;
    let mut reg = 0;
    while reg < 3 && i < nbig {
        let mut e = if reg == 0 { r1s } else if reg == 1 { r2s } else { 576 } as usize;
        if e > nbig {
            e = nbig;
        }
        let t = g.ts[reg];
        if t == 0 {
            i = e.max(i);
            reg += 1;
            continue;
        }
        let (xl, lb, _code, len) = HT[t as usize];
        let len = len.unwrap();
        while i < e {
            let ax = (g.xr[i] as i32).abs();
            let ay = (g.xr[i + 1] as i32).abs();
            let (mut cx, mut cy) = (ax, ay);
            if lb != 0 {
                if ax >= 15 {
                    cx = 15;
                    tot += lb as i32;
                }
                if ay >= 15 {
                    cy = 15;
                    tot += lb as i32;
                }
            }
            tot += len[cx as usize * xl + cy as usize] as i32 + (ax != 0) as i32 + (ay != 0) as i32;
            i += 2;
        }
        reg += 1;
    }
    let ct = if g.c1t != 0 { 33 } else { 32 };
    let len = HT[ct].3.unwrap();
    let mut i = nbig;
    for _ in 0..g.nq {
        let a0 = g.xr[i];
        let a1 = g.xr[i + 1];
        let a2 = g.xr[i + 2];
        let a3 = g.xr[i + 3];
        let v = (a0 != 0) as usize * 8 + (a1 != 0) as usize * 4 + (a2 != 0) as usize * 2 + (a3 != 0) as usize;
        tot += len[v] as i32 + (a0 != 0) as i32 + (a1 != 0) as i32 + (a2 != 0) as i32 + (a3 != 0) as i32;
        i += 4;
    }
    tot + g.stuff_n
}

// ---------------------------------------------------------------- bit writing
/// writes the low `n` bits of `v` (MSB first) at bit position `*pos`; bits past the end of `b`
/// are dropped but `*pos` still advances (same as the C `wbits`)
fn put_bits(b: &mut [u8], pos: &mut u64, v: u32, n: i32) {
    if n <= 0 {
        return;
    }
    let cap = b.len() as u64 * 8;
    if *pos + n as u64 <= cap {
        let mut p = *pos;
        let mut left = n;
        while left > 0 {
            let bo = (p & 7) as i32;
            let room = 8 - bo;
            let take = left.min(room);
            let bits = (v >> (left - take)) & ((1u32 << take) - 1);
            b[(p >> 3) as usize] |= (bits << (room - take)) as u8;
            p += take as u64;
            left -= take;
        }
        *pos = p;
        return;
    }
    for i in (0..n).rev() {
        if (v >> i) & 1 != 0 && *pos < cap {
            b[(*pos >> 3) as usize] |= 0x80 >> (*pos & 7);
        }
        *pos += 1;
    }
}

/// bit stream of one run (the frames' main data, i.e. the bit reservoir view)
struct RunBits {
    ms: Vec<u8>,
    pos: u64,
}
impl RunBits {
    fn put(&mut self, v: u32, n: i32) {
        if n <= 0 {
            return;
        }
        let need = ((self.pos + n as u64 + 7) / 8) as usize;
        if need > self.ms.len() {
            let to = need.max(self.ms.len() * 2).max(4096);
            self.ms.resize(to, 0);
        }
        put_bits(&mut self.ms, &mut self.pos, v, n);
    }
}

/// write one granule's bits (scalefactors, Huffman data, count1 quads, stuffing); returns their count
fn enc_gran(w: &mut RunBits, g: &Gran, scfsi: &[[u8; 4]; 2], gri: usize, ch: usize, sr: usize, stuff: &[u8; 512]) -> i32 {
    let st = w.pos;
    let mut slen = [0i32; 39];
    let n = sf_layout(g, &mut slen);
    for i in 0..n {
        if sf_shared(scfsi, gri, ch, g, i) {
            continue;
        }
        w.put(g.sf[i] as u32, slen[i]);
    }
    let (r1s, r2s) = regions(g, sr);
    let nbig = (g.bv * 2) as usize;
    let mut i = 0usize;
    let mut reg = 0;
    while reg < 3 && i < nbig {
        let mut e = if reg == 0 { r1s } else if reg == 1 { r2s } else { 576 } as usize;
        if e > nbig {
            e = nbig;
        }
        let t = g.ts[reg];
        if t == 0 {
            i = e.max(i);
            reg += 1;
            continue;
        }
        let (xl, linbits, code, len) = HT[t as usize];
        let (code, len) = (code.unwrap(), len.unwrap());
        while i < e {
            let x = g.xr[i] as i32;
            let y = g.xr[i + 1] as i32;
            let (ax, ay) = (x.abs(), y.abs());
            let (mut lx, mut ly) = (0, 0);
            let (mut cx, mut cy) = (ax, ay);
            if linbits != 0 {
                if ax >= 15 {
                    lx = ax - 15;
                    cx = 15;
                }
                if ay >= 15 {
                    ly = ay - 15;
                    cy = 15;
                }
            }
            let idx = cx as usize * xl + cy as usize;
            w.put(code[idx] as u32, len[idx] as i32);
            if linbits != 0 && cx == 15 {
                w.put(lx as u32, linbits as i32);
            }
            if ax != 0 {
                w.put((x < 0) as u32, 1);
            }
            if linbits != 0 && cy == 15 {
                w.put(ly as u32, linbits as i32);
            }
            if ay != 0 {
                w.put((y < 0) as u32, 1);
            }
            i += 2;
        }
        reg += 1;
    }
    let ct = if g.c1t != 0 { 33 } else { 32 };
    let (code, len) = (HT[ct].2.unwrap(), HT[ct].3.unwrap());
    let mut i = nbig;
    for _ in 0..g.nq {
        let a = [g.xr[i], g.xr[i + 1], g.xr[i + 2], g.xr[i + 3]];
        let v = (a[0] != 0) as usize * 8 + (a[1] != 0) as usize * 4 + (a[2] != 0) as usize * 2 + (a[3] != 0) as usize;
        w.put(code[v] as u32, len[v] as i32);
        for k in 0..4 {
            if a[k] != 0 {
                w.put((a[k] < 0) as u32, 1);
            }
        }
        i += 4;
    }
    for k in 0..g.stuff_n as usize {
        w.put(((stuff[k >> 3] >> (7 - (k & 7))) & 1) as u32, 1);
    }
    (w.pos - st) as i32
}

/// the 17 / 32 side-info bytes of a frame (granule `p23` fields must be final)
pub(crate) fn build_side(si: &mut [u8], f: &Frame) {
    let n = if f.nch == 1 { 17 } else { 32 };
    for b in si[..n].iter_mut() {
        *b = 0;
    }
    let mut pos = 0u64;
    let w = &mut si[..n];
    put_bits(w, &mut pos, f.mdb as u32, 9);
    put_bits(w, &mut pos, f.privb as u32, if f.nch == 1 { 5 } else { 3 });
    for c in 0..f.nch {
        for k in 0..4 {
            put_bits(w, &mut pos, f.scfsi[c][k] as u32, 1);
        }
    }
    for gi in 0..2 {
        for c in 0..f.nch {
            let g = &f.gr[gi][c];
            put_bits(w, &mut pos, g.p23 as u32, 12);
            put_bits(w, &mut pos, g.bv as u32, 9);
            put_bits(w, &mut pos, g.gg as u32, 8);
            put_bits(w, &mut pos, g.sfc as u32, 4);
            put_bits(w, &mut pos, g.ws as u32, 1);
            if g.ws != 0 {
                put_bits(w, &mut pos, g.bt as u32, 2);
                put_bits(w, &mut pos, g.mixed as u32, 1);
                for k in 0..2 {
                    put_bits(w, &mut pos, g.ts[k] as u32, 5);
                }
                for k in 0..3 {
                    put_bits(w, &mut pos, g.sbg[k] as u32, 3);
                }
            } else {
                for k in 0..3 {
                    put_bits(w, &mut pos, g.ts[k] as u32, 5);
                }
                put_bits(w, &mut pos, g.r0 as u32, 4);
                put_bits(w, &mut pos, g.r1 as u32, 3);
            }
            put_bits(w, &mut pos, g.pre as u32, 1);
            put_bits(w, &mut pos, g.sfs as u32, 1);
            put_bits(w, &mut pos, g.c1t as u32, 1);
        }
    }
}

// ---------------------------------------------------------------- coefficient model
#[inline]
pub(crate) fn qm(v: i32) -> usize {
    QMT2[if v > 101 { 101 } else { v as usize }] as usize
}
pub(crate) fn compute_l4(g: &mut Gran, sr: usize) {
    g.lvalid = false;
    if g.ws != 0 {
        return;
    }
    g.lvalid = true;
    let mut b = 0usize;
    let base = g.gg - 210;
    for i in 0..576usize {
        while b < 21 && i as i32 >= SFB[sr][b + 1] {
            b += 1;
        }
        let sf = if b < 21 { g.sf[b] } else { 0 };
        let pt = if g.pre != 0 { PRETAB[b] as i32 } else { 0 };
        let v = base - 2 * (1 + g.sfs) * (sf + pt);
        g.l4[i] = v as i16;
    }
}
/// neighbour magnitudes of a reference granule, rescaled to this granule's gain;
/// o[j+1] holds |ref[j]| (o[0] = 0)
pub(crate) fn prescale(o: &mut [u16; 578], src: Option<&Gran>, g: &Gran, scaled: bool, end: usize) {
    *o = [0; 578];
    let src = match src {
        Some(s) => s,
        None => return,
    };
    let lim = end.min(575);
    if !scaled {
        for j in 0..=lim {
            o[j + 1] = (src.xr[j] as i32).unsigned_abs() as u16;
        }
        return;
    }
    let mut laste: i32 = 1 << 20;
    let mut mul: u32 = 0;
    let mut sh: i32 = 0;
    for j in 0..=lim {
        let e = 3 * (src.l4[j] as i32 - g.l4[j] as i32);
        if e != laste {
            laste = e;
            let ee = e.clamp(-192, 192);
            sh = ee >> 4;
            mul = MULT16[(ee & 15) as usize];
        }
        let v = (src.xr[j] as i32).abs() as i64;
        let mut r = (v * mul as i64) >> 16;
        if sh >= 0 {
            r <<= sh;
        } else {
            r >>= -sh;
        }
        o[j + 1] = if r > 4095 { 4095 } else { r as u16 };
    }
}

struct Dec<'a> {
    c: Coder<'a>,
    m: Model,
}

impl<'a> Dec<'a> {
    #[allow(clippy::too_many_arguments)]
    fn cbit(&mut self, i0: usize, i1: usize, i2: usize, mset: usize) -> i32 {
        let m = &mut self.m;
        m.mx.st[0] = STRETCH[sm_p(m.cb[i0]) as usize] as i32;
        m.mx.st[1] = STRETCH[sm_p(m.cd[i1]) as usize] as i32;
        m.mx.st[2] = STRETCH[sm_p(m.cf[i2]) as usize] as i32;
        m.mx.st[3] = 256;
        m.mx.mix(mset);
        let bit = self.c.code(m.mx.pr);
        m.mx.up(bit);
        sm_up(&mut m.cb[i0], bit, 127);
        sm_up(&mut m.cd[i1], bit, 127);
        sm_up(&mut m.cf[i2], bit, 127);
        bit
    }

    /// decode the coefficients of granule `g` (pg: previous granule of this channel, og: channel 0 of this frame)
    fn code_coefs(&mut self, g: &mut Gran, pg: Option<&Gran>, og: Option<&Gran>, ch: usize) {
        g.xr = [0; 576];
        let nbig = (g.bv * 2) as usize;
        let ncnt = (g.nq * 4) as usize;
        let end = nbig + ncnt;
        let btc = if g.ws != 0 { g.bt as usize } else { 0 };
        let usep = pg.map_or(false, |p| p.lvalid) && g.lvalid;
        let useo = og.map_or(false, |p| p.lvalid) && g.lvalid;
        let mut sp = [0u16; 578];
        let mut so = [0u16; 578];
        prescale(&mut sp, pg, g, usep, end);
        if ch != 0 {
            prescale(&mut so, og, g, useo, end);
        }
        let (mut a, mut b, mut c3) = (0i32, 0i32, 0i32);
        let mut sgnb = [0u8; 576];
        let mut nsb = 0usize;
        for i in 0..end {
            let p0 = sp[i + 1] as i32;
            let pm = sp[i] as i32;
            let pp = sp[i + 2] as i32;
            let (mut o0, mut om, mut op) = (0i32, 0i32, 0i32);
            if ch != 0 {
                o0 = so[i + 1] as i32;
                om = so[i] as i32;
                op = so[i + 2] as i32;
            }
            let qa = qm(a);
            let qp0 = qm(p0);
            let ib = qp0 * NQ + qm((pm + pp + 1) >> 1);
            let id = if ch != 0 { NQ * NQ + qm(o0) * NQ + qm((om + op + 1) >> 1) } else { qp0 * NQ + qa };
            let iff = (btc * 20 + (i >> 5).min(19)) * NQ + qa;
            let cnt = (i >= nbig) as usize;
            let zn = (a | b | p0 | pm | pp | o0 | om | op) == 0;
            let zi = (((ch * 4 + btc) * 2 + cnt) * 2 + (c3 != 0) as usize) * 72 + (i >> 3);
            let na: i32;
            if cnt != 0 {
                let nz = if zn {
                    code_sm(&mut self.c, &mut self.m.zq[zi], 1023)
                } else {
                    self.cbit(ib * NS + 15, id * NS + 15, iff * NS + 15, 15 * 2 + ch)
                };
                if nz != 0 {
                    sgnb[nsb] = 0;
                    nsb += 1;
                    g.xr[i] = 1;
                    na = 1;
                } else {
                    g.xr[i] = 0;
                    na = 0;
                }
            } else {
                let mut s: i32 = 0;
                let mut skip = false;
                if zn {
                    if code_sm(&mut self.c, &mut self.m.zq[zi], 1023) == 0 {
                        skip = true;
                    } else {
                        s = 1;
                    }
                }
                if !skip {
                    while s < 14 {
                        let su = s as usize;
                        let gt = self.cbit(ib * NS + su, id * NS + su, iff * NS + su, su * 2 + ch);
                        if gt == 0 {
                            break;
                        }
                        s += 1;
                    }
                }
                let k = s;
                let mg: i32;
                if k >= 2 {
                    let mut r: i32 = 1;
                    let mb = k - 1;
                    let mut j = mb - 1;
                    let base = (k as usize * NQ + qa) * 4;
                    let bit = code_sm(&mut self.c, &mut self.m.mant[base], 255);
                    r = r * 2 + bit;
                    j -= 1;
                    if j >= 0 {
                        let bit = code_sm(&mut self.c, &mut self.m.mant[base + 1 + (r & 1) as usize], 255);
                        r = r * 2 + bit;
                        j -= 1;
                    }
                    if j >= 0 {
                        let nb = (j + 1) as u32;
                        let lo = self.c.raw(nb);
                        r = (r << nb) | lo;
                    }
                    mg = r;
                } else {
                    mg = k;
                }
                if mg != 0 {
                    sgnb[nsb] = 0;
                    nsb += 1;
                    g.xr[i] = mg as i16;
                } else {
                    g.xr[i] = 0;
                }
                na = mg;
            }
            c3 = b;
            b = a;
            a = na;
        }
        // sign bits, up to 12 per step
        let mut pos = 0usize;
        while pos < nsb {
            let n = (nsb - pos).min(12);
            let v = self.c.raw(n as u32);
            for t in 0..n {
                sgnb[pos + t] = ((v >> (n - 1 - t)) & 1) as u8;
            }
            pos += n;
        }
        let mut t = 0usize;
        for i in 0..end {
            if g.xr[i] != 0 {
                if sgnb[t] != 0 {
                    g.xr[i] = -g.xr[i];
                }
                t += 1;
            }
        }
    }

    /// n raw "gap"/"tail" bits of the run's bit stream (all-zero flag, else bit by bit)
    fn gap_block(&mut self, n: u32, w: &mut RunBits) {
        if n == 0 {
            return;
        }
        let allz = code_sm(&mut self.c, &mut self.m.gapz[0], 60);
        if allz != 0 {
            w.pos += n as u64;
            return;
        }
        let mut prev = 0usize;
        for _ in 0..n {
            if self.c.overflow {
                return;
            }
            let b = code_sm(&mut self.c, &mut self.m.gapb[prev], 60);
            prev = b as usize;
            w.put(b as u32, 1);
        }
    }

    /// stuffing bits of one granule (n <= 4095) into `out` (MSB first)
    fn stuff_block(&mut self, n: u32, out: &mut [u8; 512]) {
        *out = [0; 512];
        if n == 0 {
            return;
        }
        let allz = code_sm(&mut self.c, &mut self.m.stuffz[0], 60);
        if allz == 0 {
            let mut prev = 0usize;
            for k in 0..n as usize {
                let b = code_sm(&mut self.c, &mut self.m.stuffb[prev], 60);
                prev = b as usize;
                if b != 0 {
                    out[k >> 3] |= 0x80 >> (k & 7);
                }
            }
        }
    }
}

struct RunState {
    mpos: u64,
    prev_end: u64,
    bits: RunBits,
    /// (position of the frame's payload in the output, payload bytes)
    pays: Vec<(usize, usize)>,
    len: u64,
    pg: [Option<Box<Gran>>; 2],
}

impl<'a> Dec<'a> {
    /// decode one frame and write it (header, side info) into `out` at `o`
    fn code_frame(&mut self, f: &mut Frame, pf: Option<&Frame>, rs: &mut RunState, out: &mut [u8], base: usize) {
        for k in 0..4 {
            let ctx = k * 256 + pf.map_or(0, |p| p.hdr[k] as usize);
            f.hdr[k] = code_tree(&mut self.c, &mut self.m.hdr, ctx, 8) as u8;
        }
        if self.c.overflow || !hdr_ok(&f.hdr) {
            self.c.overflow = true;
            return;
        }
        let sr = ((f.hdr[2] >> 2) & 3) as usize;
        f.nch = if (f.hdr[3] >> 6) == 3 { 1 } else { 2 };
        f.has_crc = f.hdr[1] & 1 == 0;
        let fs = (144000 * BR[(f.hdr[2] >> 4) as usize] / SR[sr] + ((f.hdr[2] >> 1) & 1) as i32) as usize;
        let hl = 4 + if f.has_crc { 2 } else { 0 };
        let sin = if f.nch == 1 { 17 } else { 32 };
        f.pay = fs - hl - sin;
        // the frame must fit in the file (the C code checks the run length at the end)
        let o = base as u64 + rs.len;
        if o + fs as u64 > out.len() as u64 {
            self.c.overflow = true;
            return;
        }
        let o = o as usize;
        if f.has_crc {
            for k in 0..2 {
                f.crc[k] = code_tree(&mut self.c, &mut self.m.crc, 0, 8) as u8;
            }
        }
        let pm = pf.map_or(0, |p| p.mdb);
        f.mdb = code_tree(&mut self.c, &mut self.m.mdb, ((pm >> 5) & 15) as usize, 9);
        f.privb = code_tree(&mut self.c, &mut self.m.privb, 0, if f.nch == 1 { 5 } else { 3 });
        for ch in 0..f.nch {
            let pv = pf.map_or(0, |p| {
                (p.scfsi[ch][0] | p.scfsi[ch][1] << 1 | p.scfsi[ch][2] << 2 | p.scfsi[ch][3] << 3) as usize
            });
            let v = code_tree(&mut self.c, &mut self.m.scfsi, pv + 16 * ch, 4);
            for k in 0..4 {
                f.scfsi[ch][k] = ((v >> k) & 1) as u8;
            }
        }
        let s = rs.mpos as i64 * 8 - f.mdb as i64 * 8;
        if s < rs.prev_end as i64 {
            self.c.overflow = true;
            return;
        }
        let gap_n = (s - rs.prev_end as i64) as u32;
        self.gap_block(gap_n, &mut rs.bits);
        let scfsi = f.scfsi;
        for gi in 0..2usize {
            for ch in 0..f.nch {
                let q: Option<Box<Gran>> = rs.pg[ch].take();
                let g0sf = f.gr[0][ch].sf;
                let (ra, rb) = f.gr[gi].split_at_mut(1);
                let (g, og): (&mut Gran, Option<&Gran>) = if ch == 0 { (&mut ra[0], None) } else { (&mut rb[0], Some(&ra[0])) };
                let ok = self.code_gran(g, q.as_deref(), og, &scfsi, &g0sf, gi, ch, sr, rs, s);
                if !ok || self.c.overflow {
                    self.c.overflow = true;
                    return;
                }
            }
        }
        rs.mpos += f.pay as u64;
        // frame header, optional CRC, side info
        out[o..o + 4].copy_from_slice(&f.hdr);
        let mut p = o + 4;
        if f.has_crc {
            out[p..p + 2].copy_from_slice(&f.crc);
            p += 2;
        }
        build_side(&mut out[p..p + sin], f);
        p += sin;
        rs.pays.push((p, f.pay));
        rs.len += fs as u64;
    }

    /// one granule: side parameters, scalefactors, stuffing, coefficients; then its bits go to the run's stream
    #[allow(clippy::too_many_arguments)]
    fn code_gran(
        &mut self,
        g: &mut Gran,
        q: Option<&Gran>,
        og: Option<&Gran>,
        scfsi: &[[u8; 4]; 2],
        g0sf: &[i32; 39],
        gi: usize,
        ch: usize,
        sr: usize,
        rs: &mut RunState,
        s: i64,
    ) -> bool {
        let c = &mut self.c;
        let m = &mut self.m;
        let pblk = q.map_or(0, |q| (q.ws) | ((q.bt & 3) << 1) | (q.mixed << 3));
        let blk = code_tree(c, &mut m.bt, (pblk + 16 * ch as i32) as usize, 4);
        g.ws = blk & 1;
        g.bt = (blk >> 1) & 3;
        g.mixed = blk >> 3;
        if g.ws == 0 {
            g.bt = 0;
            g.mixed = 0;
        }
        let pgg = q.map_or(0, |q| q.gg);
        g.gg = (pgg + code_tree(c, &mut m.gg, (q.is_some() as usize) * 2 + ch, 8)) & 255;
        let pbv = q.map_or(0, |q| q.bv);
        let bctx = (ch * 2 + (g.ws != 0 && g.bt == 2) as usize) * 8 + q.map_or(0, |q| (q.bv / 37) as usize);
        g.bv = (pbv + code_tree(c, &mut m.bv, bctx, 9)) & 511;
        g.nq = code_tree(c, &mut m.nq, q.map_or(0, |q| (q.nq >> 4) as usize) & 15, 8);
        if c.overflow || g.bv > 288 || g.bv * 2 + g.nq * 4 > 576 {
            return false;
        }
        let nts = if g.ws != 0 { 2 } else { 3 };
        for k in 0..3usize {
            if k < nts {
                let pf0 = q.map_or(0, |q| q.ts[k]) as usize;
                g.ts[k] = code_tree(c, &mut m.ts, k * 32 + pf0, 5);
            } else {
                g.ts[k] = 0;
            }
        }
        if g.ws != 0 {
            for k in 0..3usize {
                g.sbg[k] = code_tree(c, &mut m.sbg, k * 8, 3);
            }
            g.r0 = 0;
            g.r1 = 0;
        } else {
            g.r0 = code_tree(c, &mut m.r0, q.map_or(0, |q| q.r0) as usize, 4);
            g.r1 = code_tree(c, &mut m.r1, q.map_or(0, |q| q.r1) as usize, 3);
            g.sbg = [0; 3];
        }
        g.sfc = code_tree(c, &mut m.sfc, q.map_or(0, |q| q.sfc) as usize, 4);
        let pfl = q.map_or(0, |q| q.pre | (q.sfs << 1) | (q.c1t << 2));
        let fl = code_tree(c, &mut m.flags, pfl as usize, 3);
        g.pre = fl & 1;
        g.sfs = (fl >> 1) & 1;
        g.c1t = (fl >> 2) & 1;
        // scalefactors
        let mut slen = [0i32; 39];
        let n = sf_layout(g, &mut slen);
        for i in 0..n {
            if sf_shared(scfsi, gi, ch, g, i) {
                g.sf[i] = g0sf[i];
                continue;
            }
            if slen[i] == 0 {
                g.sf[i] = 0;
                continue;
            }
            let pv = q.map_or(0, |q| q.sf[i]) as usize;
            g.sf[i] = code_tree(c, &mut m.sf, i * 16 + (pv & 15), 4);
        }
        // stuffing
        g.stuff_n = code_gamma(c, &mut m.gam[0..64]);
        if g.stuff_n < 0 || g.stuff_n > 4095 {
            return false;
        }
        let mut stuff = [0u8; 512];
        self.stuff_block(g.stuff_n as u32, &mut stuff);
        compute_l4(g, sr);
        self.code_coefs(g, q, og, ch);
        if self.c.overflow || !gran_valid(g, sr) {
            return false;
        }
        let bits = gran_bits(g, scfsi, gi, ch, sr);
        g.p23 = bits;
        if !(0..=4095).contains(&bits) {
            return false;
        }
        rs.pg[ch] = Some(Box::new(g.clone()));
        if gi == 0 && ch == 0 {
            rs.prev_end = s as u64;
        }
        rs.prev_end += bits as u64;
        // the C decoder rebuilds the bits afterwards; here they are written right away
        g.p23 = enc_gran(&mut rs.bits, g, scfsi, gi, ch, sr, &stuff);
        true
    }
}

/// Unpack a packed MP3 stream (`blob`) into the original `n` bytes.
/// Err(code): -1 bad header, -2 size mismatch, -3 corrupt stream, -4 inconsistent bit stream, -9 too big.
pub fn mp3_unpack(blob: &[u8], n: usize) -> Result<Vec<u8>, i32> {
    if blob.len() < 9 || blob[0] != 3 {
        return Err(-1);
    }
    let mut m: u64 = 0;
    for i in 0..8 {
        m |= (blob[1 + i] as u64) << (8 * i);
    }
    if m != n as u64 {
        return Err(-2);
    }
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve_exact(n).is_err() {
        return Err(-9);
    }
    out.resize(n, 0);
    let mut d = Dec { c: Coder::new(&blob[9..]), m: Model::new() };
    code_parsed(&mut d, &mut out)?;
    Ok(out)
}

fn code_parsed(d: &mut Dec, out: &mut [u8]) -> Result<(), i32> {
    let n = out.len() as u64;
    let nseg = code_gamma(&mut d.c, &mut d.m.gam[64..128]);
    if nseg < 0 || nseg as u64 > n {
        return Err(-3);
    }
    let mut off: u64 = 0;
    for _ in 0..nseg {
        let is_run = code_sm(&mut d.c, &mut d.m.segf[0], 60);
        if is_run == 0 {
            let len = code_gamma(&mut d.c, &mut d.m.gam[128..192]) as i64 as u64;
            if len > n - off || d.c.overflow {
                return Err(-3);
            }
            let mut prev = 0usize;
            for k in 0..len {
                if d.c.overflow {
                    return Err(-3);
                }
                let b = code_tree(&mut d.c, &mut d.m.rawb, prev, 8);
                out[(off + k) as usize] = b as u8;
                prev = b as usize;
            }
            off += len;
            continue;
        }
        let nf = code_gamma(&mut d.c, &mut d.m.gam[192..256]);
        if nf < 0 || nf as u64 > n / 96 + 1 || d.c.overflow {
            return Err(-3);
        }
        let mut rs = RunState {
            mpos: 0,
            prev_end: 0,
            bits: RunBits { ms: Vec::new(), pos: 0 },
            pays: Vec::with_capacity(nf as usize),
            len: 0,
            pg: [None, None],
        };
        let mut cur = Box::new(Frame::new());
        let mut prev = Box::new(Frame::new());
        for fi in 0..nf {
            cur.reset();
            let pf = if fi > 0 { Some(&*prev) } else { None };
            d.code_frame(&mut cur, pf, &mut rs, out, off as usize);
            if d.c.overflow {
                return Err(-3);
            }
            std::mem::swap(&mut cur, &mut prev);
        }
        // tail of the run's bit stream
        if rs.prev_end > rs.mpos * 8 {
            return Err(-3);
        }
        let tail_n = (rs.mpos * 8 - rs.prev_end) as u32;
        d.gap_block(tail_n, &mut rs.bits);
        if d.c.overflow {
            return Err(-3);
        }
        // run length: the frames fit (checked per frame), now the bit stream must fit its frames
        if rs.bits.pos > rs.mpos * 8 {
            return Err(-4);
        }
        rs.bits.ms.resize(rs.mpos as usize + 8, 0);
        let mut mp = 0usize;
        for &(dst, pay) in &rs.pays {
            out[dst..dst + pay].copy_from_slice(&rs.bits.ms[mp..mp + pay]);
            mp += pay;
        }
        off += rs.len;
    }
    if d.c.overflow {
        return Err(-3);
    }
    if off != n {
        return Err(-3);
    }
    Ok(())
}

/// The extraction-side filter object (like the python `Mp3Un`): collects the packed bytes, decodes at the end.
pub struct Mp3Un {
    size: u64,
    buf: Vec<u8>,
}
impl Mp3Un {
    pub fn new(size: u64) -> Mp3Un {
        Mp3Un { size, buf: Vec::new() }
    }
    pub fn feed(&mut self, piece: &[u8]) {
        self.buf.extend_from_slice(piece);
    }
    pub fn finish(&mut self) -> Result<Vec<u8>, crate::container::KilnError> {
        let bad = || crate::container::KilnError("corrupt data: bad packed MP3".to_string());
        let n = usize::try_from(self.size).map_err(|_| bad())?;
        let blob = std::mem::take(&mut self.buf);
        mp3_unpack(&blob, n).map_err(|_| bad())
    }
}
