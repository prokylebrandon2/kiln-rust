//! Range-coded block decoder: exact port of `kz_rc_dec` and its models (tools/accel_reference.c,
//! lines ~340-860). Everything here must stay bit-identical to the C code.

use std::sync::OnceLock;

pub(crate) const RC_TOP: u32 = 1 << 24;
pub(crate) const RC_LC: u32 = 3;
pub(crate) const RC_PB: u32 = 2;
pub(crate) const RC_LPMAX: u8 = 2;
pub(crate) const RC_NS: usize = 8 << RC_PB; // 32
const RC_RA: u32 = 4;
const RC_RB: u32 = 7;
const RC_K: f64 = 1.6;
const RC_NMAX: u8 = 255;
pub(crate) const LIT_SIZE: usize = 0x300;
const MIX_N: usize = 3;
const MIX_SHIFT: i32 = 10;

/// Largest raw size of one block (BLOCK_TOKENS * MAX_MATCH in kiln_v14.py).
pub const MAX_BLOCK_RAW: usize = 524_288 * 258;

/// Error codes of kz_rc_dec (-1 .. -7), see `error_text`.
pub fn error_text(rc: i32) -> &'static str {
    match rc {
        -1 => "corrupt data: bad code",
        -2 => "corrupt data: block ran past its end",
        -3 => "corrupt data: bad distance",
        -4 => "corrupt data: block too long",
        -5 => "corrupt data: block overread",
        -6 => "corrupt data: block size mismatch",
        -7 => "corrupt data: out of memory",
        _ => "corrupt data: bad block",
    }
}

pub(crate) struct Tables {
    pub(crate) ra: [u16; 256],
    pub(crate) rb: [u16; 256],
    pub(crate) mixstr: [i16; 4096],
    pub(crate) mixsq: [u16; 4096],
}

fn mix_sq(d: i32) -> i32 {
    const T: [i32; 33] = [
        1, 2, 3, 6, 10, 16, 27, 45, 73, 120, 194, 310, 488, 747, 1101, 1546, 2047, 2549, 2994,
        3348, 3607, 3785, 3901, 3975, 4022, 4050, 4068, 4079, 4085, 4089, 4092, 4093, 4094,
    ];
    if d > 2047 {
        return 4095;
    }
    if d < -2047 {
        return 0;
    }
    let w = d & 127;
    let i = ((d >> 7) + 16) as usize;
    (T[i] * (128 - w) + T[i + 1] * w + 64) >> 7
}

pub(crate) fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut ra = [0u16; 256];
        let mut rb = [0u16; 256];
        for n in 0..256usize {
            let r = 1.0 / (n as f64 + RC_K);
            let fa = 1.0 / ((1u32 << RC_RA) as f64);
            let fb = 1.0 / ((1u32 << RC_RB) as f64);
            ra[n] = ((if r > fa { r } else { fa }) * 65536.0 + 0.5) as u16;
            rb[n] = ((if r > fb { r } else { fb }) * 65536.0 + 0.5) as u16;
        }
        let mut mixstr = [0i16; 4096];
        let mut mixsq = [0u16; 4096];
        let mut pi: usize = 0;
        for x in -2047i32..=2047 {
            let v = mix_sq(x) as usize;
            let mut i = pi;
            while i <= v {
                mixstr[i] = x as i16;
                i += 1;
            }
            pi = v + 1;
        }
        for i in pi..4096 {
            mixstr[i] = 2047;
        }
        for i in 0..4096usize {
            mixsq[i] = mix_sq(i as i32 - 2048) as u16;
        }
        Tables { ra, rb, mixstr, mixsq }
    })
}

#[derive(Clone, Copy)]
pub(crate) struct Rcp {
    pub(crate) a: u16,
    pub(crate) b: u16,
    pub(crate) n: u8,
}

pub(crate) const RCP_INIT: Rcp = Rcp { a: 32768, b: 32768, n: 0 };

impl Rcp {
    #[inline(always)]
    pub(crate) fn p(&self) -> u32 {
        (self.a as u32 + self.b as u32) >> 5
    }
    #[inline(always)]
    pub(crate) fn up(&mut self, bit: u32, t: &Tables) {
        // Branch-free (the bit is unpredictable): same arithmetic as
        //   bit=1: x -= (x * r) >> 16      bit=0: x += ((65535 - x) * r) >> 16
        let n = self.n as usize;
        self.n += (self.n < RC_NMAX) as u8;
        // SAFETY: n is a u8 and both rate tables have 256 entries
        let (ra, rb) = unsafe { (*t.ra.get_unchecked(n) as u32, *t.rb.get_unchecked(n) as u32) };
        let m = 0u32.wrapping_sub(bit); // all ones when bit = 1
        let a = self.a as u32;
        let b = self.b as u32;
        let ia = 65535 - a;
        let ib = 65535 - b;
        let xa = ia ^ ((ia ^ a) & m);
        let xb = ib ^ ((ib ^ b) & m);
        let da = (xa * ra) >> 16;
        let db = (xb * rb) >> 16;
        self.a = a.wrapping_add((da ^ m).wrapping_sub(m)) as u16;
        self.b = b.wrapping_add((db ^ m).wrapping_sub(m)) as u16;
    }
}

pub(crate) struct Lenc {
    pub(crate) ch: [Rcp; 2],
    pub(crate) low: [Rcp; 8],
    pub(crate) mid: [Rcp; 8],
    pub(crate) high: [Rcp; 256],
}

impl Lenc {
    pub(crate) fn new() -> Lenc {
        Lenc { ch: [RCP_INIT; 2], low: [RCP_INIT; 8], mid: [RCP_INIT; 8], high: [RCP_INIT; 256] }
    }
}

pub(crate) struct Mix {
    pub(crate) t1: Vec<Rcp>, // [256][256]
    pub(crate) t2: Vec<Rcp>, // [4096][256]
    pub(crate) w: Vec<i32>,  // [256][3]
    pub(crate) wm: Vec<i32>, // [256][3]
}

impl Mix {
    pub(crate) fn new() -> Mix {
        let mut w = vec![0i32; 256 * MIX_N];
        let mut wm = vec![0i32; 256 * MIX_N];
        for c in 0..256 {
            w[c * 3] = 26000;
            w[c * 3 + 1] = 20000;
            w[c * 3 + 2] = 20000;
            wm[c * 3] = 36000;
            wm[c * 3 + 1] = 14000;
            wm[c * 3 + 2] = 14000;
        }
        Mix { t1: vec![RCP_INIT; 256 * 256], t2: vec![RCP_INIT; 4096 * 256], w, wm }
    }
}

pub(crate) struct Model {
    pub(crate) ism: [Rcp; RC_NS],
    pub(crate) isrep: [Rcp; RC_NS],
    pub(crate) isrep0: [Rcp; RC_NS],
    pub(crate) isrep1: [Rcp; RC_NS],
    pub(crate) isrep2: [Rcp; RC_NS],
    pub(crate) isrep0l: [Rcp; RC_NS],
    pub(crate) lit: Vec<Rcp>, // [(1<<RC_LC) << RC_LPMAX][0x300]
    pub(crate) ml: Lenc,
    pub(crate) rl: Lenc,
    pub(crate) slot: [[Rcp; 32]; 4],
    pub(crate) ext: [[Rcp; 8]; 27],
    pub(crate) align: [Rcp; 16],
    pub(crate) lpm: u32,
    pub(crate) mix: Option<Box<Mix>>,
}

impl Model {
    pub(crate) fn new() -> Model {
        Model {
            ism: [RCP_INIT; RC_NS],
            isrep: [RCP_INIT; RC_NS],
            isrep0: [RCP_INIT; RC_NS],
            isrep1: [RCP_INIT; RC_NS],
            isrep2: [RCP_INIT; RC_NS],
            isrep0l: [RCP_INIT; RC_NS],
            lit: vec![RCP_INIT; ((1usize << RC_LC) << RC_LPMAX) * LIT_SIZE],
            ml: Lenc::new(),
            rl: Lenc::new(),
            slot: [[RCP_INIT; 32]; 4],
            ext: [[RCP_INIT; 8]; 27],
            align: [RCP_INIT; 16],
            lpm: 0,
            mix: None,
        }
    }
}

struct Dec<'a> {
    pl: &'a [u8],
    p: usize,
    range: u32,
    code: u32,
    t: &'static Tables,
}

impl<'a> Dec<'a> {
    #[inline(always)]
    fn byte(&mut self) -> u8 {
        let b = if self.p < self.pl.len() { self.pl[self.p] } else { 0 };
        self.p += 1;
        b
    }
    #[inline(always)]
    fn norm(&mut self) {
        while self.range < RC_TOP {
            self.range <<= 8;
            self.code = (self.code << 8) | self.byte() as u32;
        }
    }
    #[inline(always)]
    fn bit(&mut self, p: &mut Rcp) -> u32 {
        let bit = self.bitp(p.p());
        p.up(bit, self.t);
        bit
    }
    #[inline(always)]
    fn bitp(&mut self, p: u32) -> u32 {
        // branch-free: bit = code >= bound; bit 0 keeps [0, bound), bit 1 keeps [bound, range)
        let bound = (self.range >> 12) * p;
        let bit = (self.code >= bound) as u32;
        let m = 0u32.wrapping_sub(bit);
        self.code -= bound & m;
        self.range = (bound & !m) | (self.range.wrapping_sub(bound) & m);
        self.norm();
        bit
    }
    fn direct(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            self.range >>= 1;
            let b = if self.code >= self.range { 1 } else { 0 };
            if b != 0 {
                self.code -= self.range;
            }
            v = (v << 1) | b;
            self.norm();
        }
        v
    }
    fn tree(&mut self, t: &mut [Rcp], nbits: u32) -> u32 {
        let mut m = 1u32;
        for _ in 0..nbits {
            m = (m << 1) | self.bit(&mut t[m as usize]);
        }
        m - (1u32 << nbits)
    }
    fn rtree(&mut self, t: &mut [Rcp], nbits: u32) -> u32 {
        let mut m = 1u32;
        let mut v = 0u32;
        for i in 0..nbits {
            let b = self.bit(&mut t[m as usize]);
            m = (m << 1) | b;
            v |= b << i;
        }
        v
    }
    fn len(&mut self, l: &mut Lenc) -> u32 {
        if self.bit(&mut l.ch[0]) == 0 {
            return self.tree(&mut l.low, 3);
        }
        if self.bit(&mut l.ch[1]) == 0 {
            return 8 + self.tree(&mut l.mid, 3);
        }
        16 + self.tree(&mut l.high, 8)
    }
}

#[inline(always)]
fn mix_h2(prev2: u32, prev: u32) -> usize {
    ((prev2.wrapping_mul(0x9E3779B1) ^ prev.wrapping_mul(0x85EBCA6B)) >> 20) as usize
}

/// Cache hint only (no effect on results): the literal tables are bigger than the cache.
#[inline(always)]
fn prefetch<T>(p: *const T) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_prefetch(p as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("prfm pldl1keep, [{0}]", in(reg) p, options(nostack, preserves_flags, readonly));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = p;
}

/// returns (P(bit=0) in 1..4095, the three stretched inputs)
#[inline(always)]
fn mix_pred(t: &Tables, a: &Rcp, b: &Rcp, c: &Rcp, w: &[i32]) -> (u32, [i32; MIX_N]) {
    // SAFETY: Rcp::p() is (a + b) >> 5 <= 4095 and mixstr has 4096 entries; d + 2048 is in 1..=4095
    let s = unsafe {
        [
            *t.mixstr.get_unchecked(a.p() as usize) as i32,
            *t.mixstr.get_unchecked(b.p() as usize) as i32,
            *t.mixstr.get_unchecked(c.p() as usize) as i32,
        ]
    };
    let dot = s[0] as i64 * w[0] as i64 + s[1] as i64 * w[1] as i64 + s[2] as i64 * w[2] as i64;
    let d = ((dot >> 16) as i32).clamp(-2047, 2047);
    let p = (unsafe { *t.mixsq.get_unchecked((d + 2048) as usize) } as i32).clamp(1, 4095);
    (p as u32, s)
}

#[inline(always)]
fn mix_upd(w: &mut [i32], s: &[i32; MIX_N], p: u32, bit: u32) {
    let err = (if bit != 0 { 0 } else { 4095 }) - p as i32;
    for k in 0..MIX_N {
        w[k] = w[k].wrapping_add((s[k] * err) >> MIX_SHIFT);
    }
}

fn mixlit(d: &mut Dec, x: &mut Mix, lp: &mut [Rcp], prev: u32, prev2: u32) -> u32 {
    let b1 = (prev as usize) * 256;
    let b2 = mix_h2(prev2, prev) * 256;
    assert!(lp.len() >= 256 && x.t1.len() >= b1 + 256 && x.t2.len() >= b2 + 256 && x.w.len() >= 256 * 3);
    let (t1, t2, w) = (x.t1.as_mut_ptr(), x.t2.as_mut_ptr(), x.w.as_mut_ptr());
    let lpp = lp.as_mut_ptr();
    let mut c0 = 1usize;
    for _ in 0..8 {
        // SAFETY: c0 < 256 here (it has at most 7 bits before the last step) and the asserts above
        // cover every row; the children 2*c0 and 2*c0+1 are only prefetched, never dereferenced.
        unsafe {
            prefetch(t2.add(b2 + 2 * c0));
            prefetch(t1.add(b1 + 2 * c0));
            prefetch(lpp.add(2 * c0));
            let wi = c0 * 3;
            let ws = std::slice::from_raw_parts_mut(w.add(wi), 3);
            let (p, s) = mix_pred(d.t, &*lpp.add(c0), &*t1.add(b1 + c0), &*t2.add(b2 + c0), ws);
            let bit = d.bitp(p);
            mix_upd(ws, &s, p, bit);
            (*lpp.add(c0)).up(bit, d.t);
            (*t1.add(b1 + c0)).up(bit, d.t);
            (*t2.add(b2 + c0)).up(bit, d.t);
            c0 = (c0 << 1) | bit as usize;
        }
    }
    (c0 & 0xFF) as u32
}

fn mixlitm(d: &mut Dec, x: &mut Mix, lp: &mut [Rcp], prev: u32, prev2: u32, mut mb: u32) -> u32 {
    let b1 = (prev as usize) * 256;
    let b2 = mix_h2(prev2, prev) * 256;
    assert!(lp.len() >= 0x300 && x.t1.len() >= b1 + 256 && x.t2.len() >= b2 + 256 && x.wm.len() >= 256 * 3);
    let (t1, t2, wm) = (x.t1.as_mut_ptr(), x.t2.as_mut_ptr(), x.wm.as_mut_ptr());
    let lpp = lp.as_mut_ptr();
    let mut offs: u32 = 0x100;
    let mut sym: u32 = 1;
    while sym < 0x100 {
        mb <<= 1;
        let mbit = mb & offs;
        let wi = sym as usize * 3;
        let ai = (offs + mbit + sym) as usize;
        // SAFETY: sym < 256 so wi + 3 <= 768 and b1/b2 + sym < row end; ai <= 0x100 + 0x100 + 0xFF < 0x300
        let bit = unsafe {
            prefetch(t2.add(b2 + 2 * sym as usize));
            prefetch(t1.add(b1 + 2 * sym as usize));
            let ws = std::slice::from_raw_parts_mut(wm.add(wi), 3);
            let (p, s) = mix_pred(d.t, &*lpp.add(ai), &*t1.add(b1 + sym as usize), &*t2.add(b2 + sym as usize), ws);
            let bit = d.bitp(p);
            mix_upd(ws, &s, p, bit);
            (*lpp.add(ai)).up(bit, d.t);
            (*t1.add(b1 + sym as usize)).up(bit, d.t);
            (*t2.add(b2 + sym as usize)).up(bit, d.t);
            bit
        };
        sym = (sym << 1) | bit;
        let keep = if bit != 0 { mbit } else { !mbit };
        offs &= keep;
    }
    sym & 0xFF
}

/// Decode one range-coded block (type 1). `out` holds the history; `raw` bytes are appended.
/// On error `out` is truncated back to its original length. Mirrors `kz_rc_dec`.
pub fn decode_block(payload: &[u8], raw: usize, out: &mut Vec<u8>) -> Result<(), i32> {
    let op0 = out.len();
    let target = op0 + raw;
    let plen = payload.len();
    if raw > MAX_BLOCK_RAW {
        return Err(-4);
    }
    if plen < 5 || (payload[0] & 3) > RC_LPMAX || (payload[0] >> 2) > 1 {
        return Err(-1);
    }
    out.resize(target, 0);
    let r = decode_inner(payload, out, op0, target);
    if r.is_err() {
        out.truncate(op0);
    }
    r
}

fn decode_inner(pl: &[u8], out: &mut [u8], op0: usize, target: usize) -> Result<(), i32> {
    let t = tables();
    let plen = pl.len();
    let mut m = Model::new();
    m.lpm = (1u32 << (pl[0] & 3)) - 1;
    if (pl[0] >> 2) != 0 {
        m.mix = Some(Box::new(Mix::new()));
    }
    let mut d = Dec { pl, p: 1, range: 0xFFFF_FFFF, code: 0, t };
    for _ in 0..4 {
        d.code = (d.code << 8) | d.byte() as u32;
    }
    let mut st: usize = 0;
    let mut rep: [u32; 4] = [1, 1, 1, 1];
    let mut op = op0;
    while op < target {
        if d.p > plen + 8 {
            return Err(-2);
        }
        let sp = st + 8 * ((op - op0) & ((1 << RC_PB) - 1));
        if d.bit(&mut m.ism[sp]) == 0 {
            let prev: u32 = if op > op0 { out[op - 1] as u32 } else { 0 };
            let ctx = ((prev >> (8 - RC_LC)) + 8 * (((op - op0) as u32) & m.lpm)) as usize;
            let base = ctx * LIT_SIZE;
            let lp = &mut m.lit[base..base + LIT_SIZE];
            let prev2: u32 = if op > op0 + 1 { out[op - 2] as u32 } else { 0 };
            if (st & 1) != 0 && m.mix.is_some() {
                let mb = out[op - rep[0] as usize] as u32;
                let x = m.mix.as_mut().unwrap();
                out[op] = mixlitm(&mut d, x, lp, prev, prev2, mb) as u8;
                op += 1;
            } else if (st & 1) != 0 {
                let mut mb = out[op - rep[0] as usize] as u32;
                let mut offs: u32 = 0x100;
                let mut sym: u32 = 1;
                while sym < 0x100 {
                    mb <<= 1;
                    let mbit = mb & offs;
                    let bit = d.bit(&mut lp[(offs + mbit + sym) as usize]);
                    sym = (sym << 1) | bit;
                    if bit != 0 {
                        offs &= mbit;
                    } else {
                        offs &= !mbit;
                    }
                }
                out[op] = sym as u8;
                op += 1;
            } else if let Some(x) = m.mix.as_mut() {
                out[op] = mixlit(&mut d, x, lp, prev, prev2) as u8;
                op += 1;
            } else {
                out[op] = d.tree(lp, 8) as u8;
                op += 1;
            }
            st = (st << 1) & 7;
            continue;
        }
        let len: usize;
        let dist: u32;
        if d.bit(&mut m.isrep[sp]) != 0 {
            if d.bit(&mut m.isrep0[sp]) == 0 {
                dist = rep[0];
                if d.bit(&mut m.isrep0l[sp]) == 0 {
                    len = 1;
                } else {
                    len = d.len(&mut m.rl) as usize + 2;
                }
            } else {
                let ri: usize;
                if d.bit(&mut m.isrep1[sp]) == 0 {
                    ri = 1;
                } else {
                    ri = 2 + d.bit(&mut m.isrep2[sp]) as usize;
                }
                dist = rep[ri];
                let mut k = ri;
                while k > 0 {
                    rep[k] = rep[k - 1];
                    k -= 1;
                }
                rep[0] = dist;
                len = d.len(&mut m.rl) as usize + 2;
            }
        } else {
            let v = d.len(&mut m.ml);
            len = v as usize + 2;
            let sc: usize = (if v < 1 { 0 } else if v < 4 { v - 1 } else { 3 }) as usize;
            let slot = d.tree(&mut m.slot[sc], 5);
            if slot > 26 {
                return Err(-3);
            }
            let mut dd = slot;
            if slot >= 2 {
                let eb = slot - 1;
                let nm = if eb < 3 { eb } else { 3 };
                let r = eb - nm;
                let mut x = d.tree(&mut m.ext[slot as usize], nm) << r;
                if r >= 4 {
                    x |= d.direct(r - 4) << 4;
                    x |= d.rtree(&mut m.align, 4);
                } else if r > 0 {
                    x |= d.direct(r);
                }
                dd = (1u32 << eb) + x;
            }
            dist = dd + 1;
            rep[3] = rep[2];
            rep[2] = rep[1];
            rep[1] = rep[0];
            rep[0] = dist;
        }
        if (dist as usize) > op {
            return Err(-3);
        }
        if op + len > target {
            return Err(-4);
        }
        let dsz = dist as usize;
        if dsz >= len {
            out.copy_within(op - dsz..op - dsz + len, op);
        } else {
            for k in 0..len {
                out[op + k] = out[op + k - dsz];
            }
        }
        op += len;
        st = ((st << 1) | 1) & 7;
    }
    if d.p > plen {
        return Err(-5);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Encoder side: exact port of `kz_rc_enc` / `enc_tok` (tools/accel_reference.c). The output must
// be byte-identical to the C encoder, since archives made by either engine must be the same.
// ---------------------------------------------------------------------------------------------

const DBITS: u32 = 32;
const DMASK: u64 = (1u64 << DBITS) - 1;

pub(crate) struct Enc {
    low: u64,
    range: u32,
    cache: u8,
    csize: i64,
    out: Vec<u8>,
    cap: usize,
    pub(crate) err: bool,
    t: &'static Tables,
}

impl Enc {
    pub(crate) fn new(cap: usize) -> Enc {
        Enc { low: 0, range: 0xFFFF_FFFF, cache: 0, csize: 1, out: Vec::with_capacity(cap.min(1 << 20)), cap, err: false, t: tables() }
    }
    /// start a fresh output (the learning parser only wants the model updates, not the bytes)
    pub(crate) fn clear_out(&mut self) {
        self.out.clear();
        self.err = false;
        self.csize = 1;
    }
    #[inline(always)]
    fn put(&mut self, v: u8) {
        if self.out.len() < self.cap {
            self.out.push(v);
        } else {
            self.err = true;
        }
    }
    #[inline(always)]
    fn shift(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut t = self.cache;
            loop {
                self.put(t.wrapping_add(carry));
                t = 0xFF;
                self.csize -= 1;
                if self.csize == 0 {
                    break;
                }
            }
            self.cache = (self.low >> 24) as u8;
        }
        self.csize += 1;
        self.low = (self.low & 0x00FF_FFFF) << 8;
    }
    #[inline(always)]
    fn norm(&mut self) {
        while self.range < RC_TOP {
            self.range <<= 8;
            self.shift();
        }
    }
    #[inline(always)]
    pub(crate) fn bit(&mut self, p: &mut Rcp, bit: u32) {
        let bound = (self.range >> 12) * p.p();
        if bit == 0 {
            self.range = bound;
        } else {
            self.low += bound as u64;
            self.range -= bound;
        }
        p.up(bit, self.t);
        self.norm();
    }
    #[inline(always)]
    fn bitp(&mut self, p: u32, bit: u32) {
        let bound = (self.range >> 12) * p;
        if bit == 0 {
            self.range = bound;
        } else {
            self.low += bound as u64;
            self.range -= bound;
        }
        self.norm();
    }
    fn direct(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.range >>= 1;
            if (v >> i) & 1 != 0 {
                self.low += self.range as u64;
            }
            self.norm();
        }
    }
    fn tree(&mut self, t: &mut [Rcp], nbits: u32, v: u32) {
        let mut m = 1u32;
        for i in (0..nbits).rev() {
            let b = (v >> i) & 1;
            self.bit(&mut t[m as usize], b);
            m = (m << 1) | b;
        }
    }
    fn rtree(&mut self, t: &mut [Rcp], nbits: u32, v: u32) {
        let mut m = 1u32;
        for i in 0..nbits {
            let b = (v >> i) & 1;
            self.bit(&mut t[m as usize], b);
            m = (m << 1) | b;
        }
    }
    fn len(&mut self, l: &mut Lenc, v: u32) {
        if v < 8 {
            self.bit(&mut l.ch[0], 0);
            self.tree(&mut l.low, 3, v);
        } else if v < 16 {
            self.bit(&mut l.ch[0], 1);
            self.bit(&mut l.ch[1], 0);
            self.tree(&mut l.mid, 3, v - 8);
        } else {
            self.bit(&mut l.ch[0], 1);
            self.bit(&mut l.ch[1], 1);
            self.tree(&mut l.high, 8, v - 16);
        }
    }
    /// false if the distance needs a slot the model does not have (C would read out of bounds)
    fn dist(&mut self, m: &mut Model, v: u32, dist: u32) -> bool {
        let d = dist.wrapping_sub(1);
        let mut slot = 0u32;
        while slot < 32 && (d >> slot) != 0 {
            slot += 1;
        }
        if slot > 26 {
            return false;
        }
        let sc = (if v < 1 { 0 } else if v < 4 { v - 1 } else { 3 }) as usize;
        self.tree(&mut m.slot[sc], 5, slot);
        if slot >= 2 {
            let eb = slot - 1;
            let x = d - (1u32 << eb);
            let nm = if eb < 3 { eb } else { 3 };
            let r = eb - nm;
            self.tree(&mut m.ext[slot as usize], nm, x >> r);
            if r >= 4 {
                self.direct((x >> 4) & ((1u32 << (r - 4)) - 1), r - 4);
                self.rtree(&mut m.align, 4, x & 15);
            } else if r > 0 {
                self.direct(x & ((1u32 << r) - 1), r);
            }
        }
        true
    }
}

fn enc_mixlit(e: &mut Enc, x: &mut Mix, lp: &mut [Rcp], prev: u32, prev2: u32, byte: u32) {
    let b1 = (prev as usize) * 256;
    let b2 = mix_h2(prev2, prev) * 256;
    let mut c0 = 1usize;
    for i in (0..8).rev() {
        let bit = (byte >> i) & 1;
        let wi = c0 * 3;
        let (p, s) = mix_pred(e.t, &lp[c0], &x.t1[b1 + c0], &x.t2[b2 + c0], &x.w[wi..wi + 3]);
        e.bitp(p, bit);
        mix_upd(&mut x.w[wi..wi + 3], &s, p, bit);
        lp[c0].up(bit, e.t);
        x.t1[b1 + c0].up(bit, e.t);
        x.t2[b2 + c0].up(bit, e.t);
        c0 = (c0 << 1) | bit as usize;
    }
}

fn enc_mixlitm(e: &mut Enc, x: &mut Mix, lp: &mut [Rcp], prev: u32, prev2: u32, mut mb: u32, byte: u32) {
    let b1 = (prev as usize) * 256;
    let b2 = mix_h2(prev2, prev) * 256;
    let mut offs: u32 = 0x100;
    let mut sym: u32 = 1;
    for i in (0..8).rev() {
        mb <<= 1;
        let mbit = mb & offs;
        let bit = (byte >> i) & 1;
        let wi = sym as usize * 3;
        let ai = (offs + mbit + sym) as usize;
        let (p, s) = mix_pred(e.t, &lp[ai], &x.t1[b1 + sym as usize], &x.t2[b2 + sym as usize], &x.wm[wi..wi + 3]);
        e.bitp(p, bit);
        mix_upd(&mut x.wm[wi..wi + 3], &s, p, bit);
        lp[ai].up(bit, e.t);
        x.t1[b1 + sym as usize].up(bit, e.t);
        x.t2[b2 + sym as usize].up(bit, e.t);
        sym = (sym << 1) | bit;
        if bit != 0 {
            offs &= mbit;
        } else {
            offs &= !mbit;
        }
    }
}

/// Encode one token (updating the models, `pos`, `st` and `rep`). Mirrors C `enc_tok`.
/// Returns true if the token breaks the rules (nothing is changed then), or reads outside `buf`.
pub(crate) fn enc_tok(e: &mut Enc, m: &mut Model, tk: u64, buf: &[u8], pos0: usize,
                      pos: &mut usize, st: &mut usize, rep: &mut [u32; 4]) -> bool {
    let sp = *st + 8 * ((*pos - pos0) & ((1 << RC_PB) - 1));
    if tk < 256 {
        let prev: u32 = if *pos > pos0 { match buf.get(*pos - 1) { Some(&v) => v as u32, None => return true } } else { 0 };
        let prev2: u32 = if *pos > pos0 + 1 { match buf.get(*pos - 2) { Some(&v) => v as u32, None => return true } } else { 0 };
        let mb: u32 = if (*st & 1) != 0 {
            match (*pos).checked_sub(rep[0] as usize).and_then(|i| buf.get(i)) { Some(&v) => v as u32, None => return true }
        } else { 0 };
        e.bit(&mut m.ism[sp], 0);
        let ctx = ((prev >> (8 - RC_LC)) + 8 * (((*pos - pos0) as u32) & m.lpm)) as usize;
        let base = ctx * LIT_SIZE;
        let lpm = &mut m.lit[base..base + LIT_SIZE];
        let byte = tk as u32;
        if (*st & 1) != 0 {
            if let Some(x) = m.mix.as_mut() {
                enc_mixlitm(e, x, lpm, prev, prev2, mb, byte);
            } else {
                let mut mb = mb;
                let mut offs: u32 = 0x100;
                let mut sym: u32 = 1;
                for i in (0..8).rev() {
                    mb <<= 1;
                    let mbit = mb & offs;
                    let bit = (byte >> i) & 1;
                    e.bit(&mut lpm[(offs + mbit + sym) as usize], bit);
                    sym = (sym << 1) | bit;
                    if bit != 0 { offs &= mbit; } else { offs &= !mbit; }
                }
            }
        } else if let Some(x) = m.mix.as_mut() {
            enc_mixlit(e, x, lpm, prev, prev2, byte);
        } else {
            e.tree(lpm, 8, byte);
        }
        *pos += 1;
        *st = (*st << 1) & 7;
    } else {
        let len = (tk >> DBITS) as u32;
        let dist = (tk & DMASK) as u32;
        let ri: i32 = if len < 2 { -1 } else { (0..4).find(|&k| rep[k] == dist).map(|k| k as i32).unwrap_or(-1) };
        if len < 2 && ri < 0 && !(len == 1 && dist == rep[0]) {
            return true;
        }
        if ri < 0 && len != 1 {
            // a new distance: refuse before touching anything if its slot does not exist
            let d = dist.wrapping_sub(1);
            if (32 - d.leading_zeros()) > 26 {
                return true;
            }
        }
        e.bit(&mut m.ism[sp], 1);
        if ri >= 0 || len == 1 {
            e.bit(&mut m.isrep[sp], 1);
            if len == 1 {
                e.bit(&mut m.isrep0[sp], 0);
                e.bit(&mut m.isrep0l[sp], 0);
            } else {
                if ri == 0 {
                    e.bit(&mut m.isrep0[sp], 0);
                    e.bit(&mut m.isrep0l[sp], 1);
                } else {
                    e.bit(&mut m.isrep0[sp], 1);
                    if ri == 1 {
                        e.bit(&mut m.isrep1[sp], 0);
                    } else {
                        e.bit(&mut m.isrep1[sp], 1);
                        e.bit(&mut m.isrep2[sp], (ri == 3) as u32);
                    }
                    let mut k = ri as usize;
                    while k > 0 {
                        rep[k] = rep[k - 1];
                        k -= 1;
                    }
                    rep[0] = dist;
                }
                e.len(&mut m.rl, len - 2);
            }
        } else {
            e.bit(&mut m.isrep[sp], 0);
            let v = len - 2;
            e.len(&mut m.ml, v);
            e.dist(m, v, dist);
            rep[3] = rep[2];
            rep[2] = rep[1];
            rep[1] = rep[0];
            rep[0] = dist;
        }
        *pos += len as usize;
        *st = ((*st << 1) | 1) & 7;
    }
    false
}

/// Range-code a token block whose bytes start at `buf[pos0]` (history before it). Mirrors
/// `kz_rc_enc`: returns `(raw_len, payload)`, or None if the payload does not fit in `cap`
/// bytes or a token breaks the rules (the caller then stores the bytes instead).
/// `lp`: bits 0-1 literal position bits (0..=2), bit 2 = literal mixer.
pub fn encode_block(tok: &[u64], buf: &[u8], pos0: usize, lp: u32, cap: usize) -> Option<(usize, Vec<u8>)> {
    if (lp & 3) > RC_LPMAX as u32 || (lp >> 2) > 1 {
        return None;
    }
    let mut raw: u64 = 0;
    for &t in tok {
        raw += if t < 256 { 1 } else { t >> DBITS };
    }
    let mut m = Model::new();
    m.lpm = (1u32 << (lp & 3)) - 1;
    if (lp >> 2) != 0 {
        m.mix = Some(Box::new(Mix::new()));
    }
    let mut e = Enc::new(cap);
    let mut pos = pos0;
    let mut st: usize = 0;
    let mut rep: [u32; 4] = [1, 1, 1, 1];
    for &tk in tok {
        if e.err {
            break;
        }
        if enc_tok(&mut e, &mut m, tk, buf, pos0, &mut pos, &mut st, &mut rep) {
            return None;
        }
    }
    for _ in 0..5 {
        e.shift();
    }
    if e.err || e.out.is_empty() {
        return None;
    }
    e.out[0] = lp as u8;
    Some((raw as usize, e.out))
}
