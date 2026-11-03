//! LZ77 match finder + optimal parser: line-for-line port of `kz_lz` and its helpers
//! (tools/accel_reference.c, "LZ77" section and the optimal-parse code). The token stream must be
//! identical to the C one, so every constant, hash and tie-break is kept as is.
//! (use6 is always false in the C build, BTMODE == 2, so the 6-byte chain is not ported.)

use crate::rc::{enc_tok, Enc, Lenc, Model, Rcp, RC_LPMAX, RC_PB, RC_LC};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

thread_local! {
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = RefCell::new(None);
}

/// Gives the current thread a cancel flag; `lz77` and the block builder poll it and give up early
/// (README note 8). Without a flag nothing changes.
pub fn set_cancel(t: Option<Arc<AtomicBool>>) {
    CANCEL.with(|c| *c.borrow_mut() = t);
}

pub fn cancelled() -> bool {
    CANCEL.with(|c| c.borrow().as_ref().map_or(false, |a| a.load(Ordering::Relaxed)))
}

const WINDOW: i64 = (1 << 26) - 1;
const HIST_NEAR: i64 = 1 << 20;
const HIST_FAR_STRIDE: i64 = 8;
const DBITS: u32 = 32;
const HBITS: u32 = 22;
const H3BITS: u32 = 18;
const LBITS: u32 = 22;
const LONG_MIN: i32 = 24;
pub const RND_BITS: u32 = 16;
const MIN_MATCH: i32 = 3;
const MAX_MATCH: usize = 258;
const DRY_CHAIN: i32 = 8;
const SKIP_INSERT: u32 = 192;
const LAZY_SKIP: i32 = 64;
const MAX_STEP: i32 = 32;
const MIN_RND_MATCH: i32 = 24;
const SPARSE_MATCH: i32 = 12;
const HIST_STRIDE: i64 = 2;
const BTK: i32 = 3;
const PRB_CHAIN: u32 = 25;
const PRB_OPT: u32 = 24;
const HISTD: i32 = 4;
const BLK: i64 = 524_288;
const OPTW: i64 = 2048;
const PINF: i32 = 0x3FFF_FFFF;

#[inline(always)]
fn slotctx(v: u32) -> usize {
    (if v < 1 { 0 } else if v < 4 { v - 1 } else { 3 }) as usize
}
// SAFETY (for at/rd32/rd64): `lz77` checks that the buffer has n + 16 bytes and every position
// handed to these helpers is < n (table entries are inserted positions) plus at most 8, so all
// reads stay inside the padded buffer.
#[inline(always)]
fn at(b: &[u8], i: usize) -> u8 {
    debug_assert!(i < b.len());
    unsafe { *b.get_unchecked(i) }
}
/// Cache hint only (no effect on results). The tables are far bigger than the cache.
#[inline(always)]
fn prefetch<T>(p: *const T) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_prefetch(p as *const i8, core::arch::x86_64::_MM_HINT_T1);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("prfm pldl2keep, [{0}]", in(reg) p, options(nostack, preserves_flags, readonly));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = p;
}
#[inline(always)]
fn gi(v: &[i32], i: usize) -> i32 {
    debug_assert!(i < v.len());
    unsafe { *v.get_unchecked(i) }
}
#[inline(always)]
fn si(v: &mut [i32], i: usize, x: i32) {
    debug_assert!(i < v.len());
    unsafe { *v.get_unchecked_mut(i) = x }
}
#[inline(always)]
fn rd32(b: &[u8], p: usize) -> u32 {
    debug_assert!(p + 4 <= b.len());
    unsafe { u32::from_le((b.as_ptr().add(p) as *const u32).read_unaligned()) }
}
#[inline(always)]
fn rd64(b: &[u8], p: usize) -> u64 {
    debug_assert!(p + 8 <= b.len());
    unsafe { u64::from_le((b.as_ptr().add(p) as *const u64).read_unaligned()) }
}
#[inline(always)]
fn h4(b: &[u8], p: i64) -> usize {
    (rd32(b, p as usize).wrapping_mul(2654435761) >> (32 - HBITS)) as usize
}
#[inline(always)]
fn tri(b: &[u8], p: i64) -> u32 {
    let p = p as usize;
    ((at(b, p) as u32) << 16) | ((at(b, p + 1) as u32) << 8) | at(b, p + 2) as u32
}
#[inline(always)]
fn h3s(b: &[u8], p: i64) -> usize {
    (tri(b, p).wrapping_mul(2654435761) >> (32 - H3BITS)) as usize
}
#[inline(always)]
fn anchor_r(b: &[u8], p: i64) -> bool {
    tri(b, p).wrapping_mul(2654435761) >> (32 - 4) == 0
}
#[inline(always)]
fn anchor3(b: &[u8], p: i64) -> bool {
    tri(b, p).wrapping_mul(2654435761) >> 24 == 0
}
#[inline(always)]
fn h8(b: &[u8], p: i64) -> usize {
    (rd64(b, p as usize).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - LBITS)) as usize
}

struct Ctx<'a> {
    b: &'a [u8], // padded: at least 16 readable bytes after n (C reads the NUL after a bytes object)
    n: i64,
    start: i64,
    last: i64,
    ins: i64,
    head: Vec<i32>,
    head3: Vec<i32>,
    head2: Vec<i32>,
    prev: Vec<i32>,
    lhead: Vec<i32>,
    use_bt: bool,
    btd: i32,
    son: Vec<i32>,
    bmask: i64,
    bt_next: i64,
    pmask: i64,
    plim: i64,
    rnd: &'a [u8],
    nice: i32,
}

fn bt_run(c: &mut Ctx, pos: i64, mut depth: i32, pl: &mut [u32], pd: &mut [u32], mut np: usize, mut best: i32, rec: bool) -> usize {
    let b = c.b;
    let rem = c.n - pos;
    let limit = if rem > MAX_MATCH as i64 { MAX_MATCH as i32 } else { rem as i32 };
    let mut cur = gi(&c.head, h4(b, pos));
    let mask = c.bmask;
    let mut ptr1 = ((pos & mask) << 1) as usize;
    let mut ptr0 = ptr1 + 1;
    let (mut len0, mut len1) = (0i32, 0i32);
    let nice = c.nice;
    let cb = pos as usize;
    loop {
        if cur < 0 {
            si(&mut c.son, ptr0, -1);
            si(&mut c.son, ptr1, -1);
            break;
        }
        let dd = depth;
        depth -= 1;
        if dd <= 0 || pos - cur as i64 > mask {
            si(&mut c.son, ptr0, -1);
            si(&mut c.son, ptr1, -1);
            break;
        }
        let pair = (((cur as i64) & mask) << 1) as usize;
        let pbs = cur as usize;
        let mut len = if len0 < len1 { len0 } else { len1 };
        if at(b, pbs + len as usize) == at(b, cb + len as usize) {
            len += 1;
            'ext: {
                while len + 8 <= limit {
                    let x = rd64(b, pbs + len as usize);
                    let y = rd64(b, cb + len as usize);
                    if x != y {
                        len += ((x ^ y).trailing_zeros() >> 3) as i32;
                        break 'ext;
                    }
                    len += 8;
                }
                while len < limit && at(b, pbs + len as usize) == at(b, cb + len as usize) {
                    len += 1;
                }
            }
            if len > best {
                best = len;
                if rec {
                    pl[np] = len as u32;
                    pd[np] = (pos - cur as i64) as u32;
                    np += 1;
                }
            }
            if len >= limit || len >= nice {
                let s0 = gi(&c.son, pair);
                let s1 = gi(&c.son, pair + 1);
                si(&mut c.son, ptr1, s0);
                si(&mut c.son, ptr0, s1);
                break;
            }
        }
        if at(b, pbs + len as usize) < at(b, cb + len as usize) {
            si(&mut c.son, ptr1, cur);
            ptr1 = pair + 1;
            cur = gi(&c.son, ptr1);
            len1 = len;
        } else {
            si(&mut c.son, ptr0, cur);
            ptr0 = pair;
            cur = gi(&c.son, ptr0);
            len0 = len;
        }
    }
    np
}

fn put1(c: &mut Ctx, p: i64) {
    let b = c.b;
    let h = h4(b, p);
    if c.use_bt && p >= c.bt_next && p + 4 <= c.n {
        let d = if p < c.start { HISTD } else { c.btd };
        bt_run(c, p, d, &mut [], &mut [], 0, 1 << 30, false);
        c.bt_next = p + 1;
    }
    si(&mut c.prev, (p & c.pmask) as usize, gi(&c.head, h));
    si(&mut c.head, h, p as i32);
    c.head3[h3s(b, p)] = p as i32;
    c.head2[((at(b, p as usize) as usize) << 8) | at(b, p as usize + 1) as usize] = p as i32;
}

fn lput(c: &mut Ctx, a: i64, bnd: i64) {
    let lim = c.n - 8;
    let mut a = (a + 3) & !3i64;
    while a < bnd && a <= lim {
        let h = h8(c.b, a);
        c.lhead[h] = a as i32;
        a += 4;
    }
}

fn insert_to(c: &mut Ctx, upto: i64) {
    let mut p = c.ins;
    let stop = if upto < c.last { upto } else { c.last };
    while p < stop {
        let ch = p >> RND_BITS;
        let mut q = (ch + 1) << RND_BITS;
        if q > stop {
            q = stop;
        }
        let k = c.rnd[ch as usize];
        if k != 1 && k != 2 {
            lput(c, p, q);
        }
        if k == 2 {
            for a in p..q {
                if anchor3(c.b, a) {
                    put1(c, a);
                }
            }
        } else if k == 1 {
            for a in p..q {
                if anchor_r(c.b, a) {
                    put1(c, a);
                }
            }
        } else if p < c.start {
            let q2 = if q < c.start { q } else { c.start };
            let nearp = c.start - HIST_NEAR;
            let mut a = p;
            while a < q2 && a < nearp {
                put1(c, a);
                a += HIST_FAR_STRIDE;
            }
            while a < q2 {
                put1(c, a);
                a += HIST_STRIDE;
            }
            for a2 in q2..q {
                put1(c, a2);
            }
        } else {
            for a in p..q {
                put1(c, a);
            }
        }
        p = q;
    }
    if upto > c.ins {
        c.ins = upto;
    }
}

fn ext_len(b: &[u8], j: usize, i: usize, limit: i32) -> i32 {
    let mut l = 0i32;
    while l + 8 <= limit {
        if rd64(b, j + l as usize) != rd64(b, i + l as usize) {
            break;
        }
        l += 8;
    }
    while l < limit && at(b, j + l as usize) == at(b, i + l as usize) {
        l += 1;
    }
    l
}

fn find_chain(c: &Ctx, i: i64, mut chain: i32) -> (i32, i32) {
    if i >= c.last {
        return (0, 0);
    }
    let b = c.b;
    let mut j = c.head[h4(b, i)];
    let (mut best_len, mut best_dist) = (0i32, 0i32);
    let rem = c.n - i;
    let limit = if rem > MAX_MATCH as i64 { MAX_MATCH as i32 } else { rem as i32 };
    while j >= 0 && chain != 0 {
        if i - j as i64 > c.plim {
            break;
        }
        if best_len == 0 || at(b, j as usize + best_len as usize) == at(b, i as usize + best_len as usize) {
            let l = ext_len(b, j as usize, i as usize, limit);
            if l > best_len {
                best_len = l;
                best_dist = (i - j as i64) as i32;
                if l >= limit || l >= c.nice {
                    break;
                }
            }
        }
        j = c.prev[(j as i64 & c.pmask) as usize];
        chain -= 1;
    }
    if best_len < 3 {
        let j3 = c.head3[h3s(b, i)];
        if j3 >= 0 && i - j3 as i64 <= WINDOW {
            let mut l = 0i32;
            while l < limit && at(b, j3 as usize + l as usize) == at(b, i as usize + l as usize) {
                l += 1;
            }
            if l >= 3 {
                best_len = l;
                best_dist = (i - j3 as i64) as i32;
            }
        }
    }
    if best_len < MIN_MATCH
        || (best_len == 3 && best_dist > 8192)
        || (best_len == 4 && best_dist > 131072)
        || (best_len <= 5 && best_dist > (1 << 20))
        || (best_len <= 7 && best_dist > (1 << 22))
    {
        return (0, 0);
    }
    if best_len < SPARSE_MATCH && c.rnd[(i >> RND_BITS) as usize] == 3 {
        return (0, 0);
    }
    (best_len, best_dist)
}

fn find(c: &Ctx, i: i64, chain: i32) -> (i32, i32) {
    let (mut ol, mut od) = find_chain(c, i, chain);
    if ol >= LONG_MIN || i + 8 > c.n {
        return (ol, od);
    }
    let j = c.lhead[h8(c.b, i)];
    if j < 0 || j as i64 >= i || i - j as i64 > WINDOW {
        return (ol, od);
    }
    let rem = c.n - i;
    let limit = if rem > MAX_MATCH as i64 { MAX_MATCH as i32 } else { rem as i32 };
    let l = ext_len(c.b, j as usize, i as usize, limit);
    if l >= LONG_MIN && l > ol {
        ol = l;
        od = (i - j as i64) as i32;
    }
    (ol, od)
}

fn run_len(b: &[u8], pos: i64, n: i64, d: i64) -> i32 {
    let rem = n - pos;
    let limit = if rem > MAX_MATCH as i64 { MAX_MATCH as i32 } else { rem as i32 };
    ext_len(b, (pos - d) as usize, pos as usize, limit)
}

// ------------------------------------------------------------------ prices
fn log2_(x: f64) -> f64 {
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7FF) as i32 - 1023;
    let m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln = 2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0 + t2 / 9.0))));
    e as f64 + ln * 1.4426950408889634
}

pub(crate) fn log2_pub(x: f64) -> f64 {
    log2_(x)
}

fn prc() -> &'static [u16; 4097] {
    static T: std::sync::OnceLock<[u16; 4097]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u16; 4097];
        for p in 1..=4096usize {
            t[p] = (-log2_(p as f64 / 4096.0) * 64.0 + 0.5) as u16;
        }
        t[0] = t[1];
        t
    })
}

#[inline(always)]
fn pb(t: &[u16; 4097], p: &Rcp, bit: u32) -> i32 {
    let q = p.p() as usize;
    t[if bit != 0 { 4096 - q } else { q }] as i32
}
fn ptree(t: &[u16; 4097], tr: &[Rcp], nbits: u32, v: u32) -> i32 {
    let mut m = 1u32;
    let mut s = 0;
    for i in (0..nbits).rev() {
        let b = (v >> i) & 1;
        s += pb(t, &tr[m as usize], b);
        m = (m << 1) | b;
    }
    s
}
fn prtree(t: &[u16; 4097], tr: &[Rcp], nbits: u32, v: u32) -> i32 {
    let mut m = 1u32;
    let mut s = 0;
    for i in 0..nbits {
        let b = (v >> i) & 1;
        s += pb(t, &tr[m as usize], b);
        m = (m << 1) | b;
    }
    s
}
fn plen(t: &[u16; 4097], l: &Lenc, v: u32) -> i32 {
    if v < 8 {
        pb(t, &l.ch[0], 0) + ptree(t, &l.low, 3, v)
    } else if v < 16 {
        pb(t, &l.ch[0], 1) + pb(t, &l.ch[1], 0) + ptree(t, &l.mid, 3, v - 8)
    } else {
        pb(t, &l.ch[0], 1) + pb(t, &l.ch[1], 1) + ptree(t, &l.high, 8, v - 16)
    }
}
fn plit(t: &[u16; 4097], m: &Model, lctx: usize, byte: u32, matched: bool, mut mb: u32) -> i32 {
    let base = lctx * crate::rc::LIT_SIZE;
    let p = &m.lit[base..base + crate::rc::LIT_SIZE];
    if !matched {
        return ptree(t, p, 8, byte);
    }
    let mut offs = 0x100u32;
    let mut sym = 1u32;
    let mut s = 0;
    for i in (0..8).rev() {
        mb <<= 1;
        let mbit = mb & offs;
        let bit = (byte >> i) & 1;
        s += pb(t, &p[(offs + mbit + sym) as usize], bit);
        sym = (sym << 1) | bit;
        if bit != 0 {
            offs &= mbit;
        } else {
            offs &= !mbit;
        }
    }
    s
}

#[derive(Clone, Copy, Default)]
struct Onode {
    price: i32,
    from: i32,
    len: u32,
    dist: u32,
    rep: [u32; 4],
    st: u8,
}

struct Lzs<'a> {
    buf: &'a [u8],
    tok: Vec<u64>,
    nt: i64,
    pstart: i64,
    learn: bool,
    m: Option<Box<Model>>,
    e: Enc,
    rep: [u32; 4],
    st: usize,
    pos: i64,
    bpos0: i64,
    lastref: i64,
    mlp: [i32; 257],
    rlp: [i32; 257],
    nd: Vec<Onode>,
    stk: Vec<u32>,
    dsl: [[i16; 32]; 4],
    dex: [[i16; 8]; 27],
    dal: [i16; 16],
    pr: &'static [u16; 4097],
}

fn dtab(s: &mut Lzs) {
    let m = s.m.as_ref().unwrap();
    for c in 0..4 {
        for sl in 0..32u32 {
            s.dsl[c][sl as usize] = ptree(s.pr, &m.slot[c], 5, sl) as i16;
        }
    }
    for sl in 2..27usize {
        let eb = sl as u32 - 1;
        let nm = if eb < 3 { eb } else { 3 };
        for x in 0..(1u32 << nm) {
            s.dex[sl][x as usize] = ptree(s.pr, &m.ext[sl], nm, x) as i16;
        }
    }
    for a in 0..16u32 {
        s.dal[a as usize] = prtree(s.pr, &m.align, 4, a) as i16;
    }
}

#[inline(always)]
fn pdist2(s: &Lzs, v: u32, dist: u32) -> i32 {
    let d = dist.wrapping_sub(1);
    let slot = if d != 0 { 32 - d.leading_zeros() } else { 0 } as usize;
    let mut r = s.dsl[slotctx(v)][slot] as i32;
    if slot >= 2 {
        let eb = slot as u32 - 1;
        let x = d - (1u32 << eb);
        let nm = if eb < 3 { eb } else { 3 };
        let rr = eb - nm;
        r += s.dex[slot][(x >> rr) as usize] as i32;
        r += if rr >= 4 { (rr as i32 - 4) * 64 + s.dal[(x & 15) as usize] as i32 } else { rr as i32 * 64 };
    }
    r
}

fn reset_block(s: &mut Lzs) {
    s.rep = [1, 1, 1, 1];
    s.st = 0;
    s.bpos0 = s.pos;
}

fn rep_find(rep: &[u32; 4], len: u32, dist: u32) -> i32 {
    if len < 2 {
        return -1;
    }
    for k in 0..4 {
        if rep[k] == dist {
            return k as i32;
        }
    }
    -1
}

fn emit(s: &mut Lzs, t: u64) {
    if !s.learn {
        s.tok.push(t);
        s.nt += 1;
        return;
    }
    if (s.nt - s.pstart) % BLK == 0 {
        reset_block(s);
    }
    if t >= 256 {
        let len = (t >> DBITS) as u32;
        let dist = (t & ((1u64 << DBITS) - 1)) as u32;
        if len < 2 && rep_find(&s.rep, len, dist) < 0 && !(len == 1 && dist == s.rep[0]) {
            for _ in 0..len {
                let bb = s.buf[s.pos as usize] as u64;
                emit(s, bb);
            }
            return;
        }
    }
    s.tok.push(t);
    s.nt += 1;
    s.e.clear_out();
    let mut pos = s.pos as usize;
    let mut st = s.st;
    enc_tok(&mut s.e, s.m.as_mut().unwrap(), t, s.buf, s.bpos0 as usize, &mut pos, &mut st, &mut s.rep);
    s.pos = pos as i64;
    s.st = st;
}

fn refresh_prices(s: &mut Lzs) {
    let m = s.m.as_ref().unwrap();
    for v in 0..257u32 {
        s.mlp[v as usize] = plen(s.pr, &m.ml, v);
    }
    for v in 0..257u32 {
        s.rlp[v as usize] = plen(s.pr, &m.rl, v);
    }
    s.lastref = s.nt;
}

fn try_cand(b: &[u8], i: usize, jj: usize, limit: i32, best: &mut i32, pl: &mut [u32], pd: &mut [u32], np: &mut usize, nice: i32) -> bool {
    if at(b, jj + *best as usize) == at(b, i + *best as usize) {
        let l = ext_len(b, jj, i, limit);
        if l > *best {
            *best = l;
            pl[*np] = l as u32;
            pd[*np] = (i - jj) as u32;
            *np += 1;
            if l >= limit || l >= nice {
                return true;
            }
        }
    }
    false
}

fn find_all(c: &mut Ctx, i: i64, chain: i32, pl: &mut [u32], pd: &mut [u32]) -> usize {
    if i >= c.last {
        return 0;
    }
    let b = c.b;
    let rem = c.n - i;
    let limit = if rem > MAX_MATCH as i64 { MAX_MATCH as i32 } else { rem as i32 };
    let mut best = 2i32;
    let mut np = 0usize;
    let nice = c.nice;
    {
        let j3 = c.head3[h3s(b, i)];
        if j3 >= 0 && i - j3 as i64 <= WINDOW {
            let mut l = 0i32;
            while l < limit && at(b, j3 as usize + l as usize) == at(b, i as usize + l as usize) {
                l += 1;
            }
            if l >= 3 {
                best = l;
                pl[0] = l as u32;
                pd[0] = (i - j3 as i64) as u32;
                np = 1;
                if l >= limit || l >= nice {
                    return np;
                }
            }
        }
    }
    let mut skip_chain = false;
    if c.use_bt && c.bt_next == i && i + 4 <= c.n {
        np = bt_run(c, i, c.btd, pl, pd, np, best, true);
        c.bt_next = i + 1;
        if np != 0 {
            best = pl[np - 1] as i32;
        }
        if np != 0 && best >= nice {
            return np;
        }
        skip_chain = true;
    }
    if !skip_chain {
        let mut j = c.head[h4(b, i)];
        let mut s4 = chain / 4;
        if s4 < 2 {
            s4 = 2;
        }
        while j >= 0 && s4 != 0 {
            if i - j as i64 > c.plim {
                break;
            }
            let jn = c.prev[(j as i64 & c.pmask) as usize];
            if try_cand(b, i as usize, j as usize, limit, &mut best, pl, pd, &mut np, nice) {
                return np;
            }
            j = jn;
            s4 -= 1;
        }
    }
    if best < LONG_MIN && i + 8 <= c.n {
        let k = c.lhead[h8(b, i)];
        if k >= 0 && (k as i64) < i && i - k as i64 <= WINDOW {
            let l = ext_len(b, k as usize, i as usize, limit);
            if l >= LONG_MIN && l > best {
                pl[np] = l as u32;
                pd[np] = (i - k as i64) as u32;
                np += 1;
            }
        }
    }
    np
}

#[inline(always)]
fn relax(nd: &mut [Onode], len_end: &mut i32, cur: i32, t: i32, price: i32, len: u32, dist: u32, r: [u32; 4], st: u32) {
    while *len_end < t {
        *len_end += 1;
        nd[*len_end as usize].price = PINF;
    }
    let q = &mut nd[t as usize];
    if price < q.price {
        q.price = price;
        q.from = cur;
        q.len = len;
        q.dist = dist;
        q.rep = r;
        q.st = st as u8;
    }
}

fn psel(t: &[u16; 4097], m: &Model, sp: usize, k: usize) -> i32 {
    if k == 0 {
        return pb(t, &m.isrep0[sp], 0) + pb(t, &m.isrep0l[sp], 1);
    }
    if k == 1 {
        return pb(t, &m.isrep0[sp], 1) + pb(t, &m.isrep1[sp], 0);
    }
    pb(t, &m.isrep0[sp], 1) + pb(t, &m.isrep1[sp], 1) + pb(t, &m.isrep2[sp], (k == 3) as u32)
}

fn opt_window(s: &mut Lzs, c: &mut Ctx, i0: i64, chain: i32, nice: i32, wend: i64) -> i64 {
    let b = c.b;
    let n = c.n;
    if s.nt - s.lastref >= 256 || s.lastref == 0 {
        refresh_prices(s);
    }
    dtab(s);
    let mut nd = std::mem::take(&mut s.nd);
    let mut stk = std::mem::take(&mut s.stk);
    let mut lim = if wend - i0 < OPTW { wend - i0 } else { OPTW } as i32;
    if lim < 1 {
        lim = 1;
    }
    nd[0].price = 0;
    nd[0].from = 0;
    nd[0].rep = s.rep;
    nd[0].st = s.st as u8;
    let mut cur: i32 = 0;
    let mut len_end: i32 = 0;
    let fin: i32;
    let mut pl = [0u32; MAX_MATCH];
    let mut pd = [0u32; MAX_MATCH];
    let pr = s.pr;
    loop {
        if cur > 0 && cur >= len_end {
            fin = cur;
            break;
        }
        if cur >= lim {
            fin = cur;
            break;
        }
        let pos = i0 + cur as i64;
        let nn = nd[cur as usize];
        let p_ = nn.price;
        let st = nn.st as u32;
        let sp = st as usize + 8 * (((pos - s.bpos0) as usize) & ((1 << RC_PB) - 1));
        let (r0, r1, r2, r3) = (nn.rep[0], nn.rep[1], nn.rep[2], nn.rep[3]);
        let mut stop = false;
        if pos + 24 < n {
            prefetch(c.head.as_ptr().wrapping_add(h4(b, pos + 12)));
            prefetch(c.lhead.as_ptr().wrapping_add(h8(b, pos + 12)));
            let jh = gi(&c.head, h4(b, pos + 5));
            if jh >= 0 {
                prefetch(c.prev.as_ptr().wrapping_add((jh as i64 & c.pmask) as usize));
                prefetch(b.as_ptr().wrapping_add(jh as usize));
            }
        }
        insert_to(c, pos);
        let m: &Model = s.m.as_ref().unwrap();
        {
            let prev = if pos > 0 { at(b, pos as usize - 1) as u32 } else { 0 };
            let matched = (st & 1) != 0 && r0 as i64 <= pos;
            let lctx = ((prev >> (8 - RC_LC)) + 8 * (((pos - s.bpos0) as u32) & m.lpm)) as usize;
            let mbyte = if matched { at(b, (pos - r0 as i64) as usize) as u32 } else { 0 };
            let cost = pb(pr, &m.ism[sp], 0) + plit(pr, m, lctx, at(b, pos as usize) as u32, matched, mbyte);
            relax(&mut nd, &mut len_end, cur, cur + 1, p_ + cost, 1, 0, [r0, r1, r2, r3], (st << 1) & 7);
        }
        let base1 = pb(pr, &m.ism[sp], 1);
        let nst = ((st << 1) | 1) & 7;
        {
            let rr = [r0, r1, r2, r3];
            let isrep1 = base1 + pb(pr, &m.isrep[sp], 1);
            let mut k = 0usize;
            while k < 4 && !stop {
                let d = rr[k];
                let mut dup = false;
                for j in 0..k {
                    if rr[j] == d {
                        dup = true;
                    }
                }
                if dup || d as i64 > pos {
                    k += 1;
                    continue;
                }
                let l = run_len(b, pos, n, d as i64);
                if k == 0 && l >= 1 {
                    relax(&mut nd, &mut len_end, cur, cur + 1,
                          p_ + isrep1 + pb(pr, &m.isrep0[sp], 0) + pb(pr, &m.isrep0l[sp], 0), 1, d, [r0, r1, r2, r3], nst);
                }
                if l < 2 {
                    k += 1;
                    continue;
                }
                let sel = p_ + isrep1 + psel(pr, m, sp, k);
                let (a1, a2, a3) = match k {
                    0 => (r1, r2, r3),
                    1 => (r0, r2, r3),
                    2 => (r0, r1, r3),
                    _ => (r0, r1, r2),
                };
                for ll in 2..=l {
                    relax(&mut nd, &mut len_end, cur, cur + ll, sel + s.rlp[(ll - 2) as usize], ll as u32, d, [d, a1, a2, a3], nst);
                }
                if l >= nice {
                    // fin = cur + l
                    stop = true;
                    cur += l; // reuse cur to carry fin out of the loop
                    break;
                }
                k += 1;
            }
        }
        if stop {
            fin = cur;
            break;
        }
        if pos < c.last {
            let np = find_all(c, pos, chain, &mut pl, &mut pd);
            let basem = base1 + pb(pr, &m.isrep[sp], 0);
            let mut prev_l = 2u32;
            for p in 0..np {
                let d = pd[p];
                let lp_ = pl[p];
                if d == r0 || d == r1 || d == r2 || d == r3 {
                    prev_l = lp_;
                    continue;
                }
                let mut dc = [-1i32; 4];
                let mut ll = prev_l + 1;
                while ll <= lp_ {
                    if ll >= 3 {
                        let v = ll - 2;
                        let ctx = slotctx(v);
                        if dc[ctx] < 0 {
                            dc[ctx] = pdist2(s, v, d);
                        }
                        relax(&mut nd, &mut len_end, cur, cur + ll as i32, p_ + basem + s.mlp[v as usize] + dc[ctx], ll, d, [d, r0, r1, r2], nst);
                    }
                    ll += 1;
                }
                prev_l = lp_;
            }
            {
                let j2 = c.head2[((at(b, pos as usize) as usize) << 8) | at(b, pos as usize + 1) as usize];
                if j2 >= 0 && (j2 as i64) < pos && pos - j2 as i64 <= WINDOW {
                    let d2 = (pos - j2 as i64) as u32;
                    if !(d2 == r0 || d2 == r1 || d2 == r2 || d2 == r3) {
                        relax(&mut nd, &mut len_end, cur, cur + 2, p_ + basem + s.mlp[0] + pdist2(s, 0, d2), 2, d2, [d2, r0, r1, r2], nst);
                    }
                }
            }
            if np != 0 && pl[np - 1] as i32 >= nice {
                fin = cur + pl[np - 1] as i32;
                break;
            }
        }
        cur += 1;
    }
    // trace back and emit
    let mut ne = 0usize;
    let mut x = fin;
    while x > 0 {
        stk[2 * ne] = nd[x as usize].len;
        stk[2 * ne + 1] = nd[x as usize].dist;
        ne += 1;
        x = nd[x as usize].from;
    }
    s.nd = nd;
    let mut p = i0;
    for k in (0..ne).rev() {
        let len = stk[2 * k];
        let dist = stk[2 * k + 1];
        if dist == 0 {
            emit(s, at(b, p as usize) as u64);
        } else {
            emit(s, ((len as u64) << DBITS) | dist as u64);
            if len >= SKIP_INSERT && p + len as i64 > c.ins {
                insert_to(c, p + 4);
                c.ins = p + len as i64;
            }
        }
        p += len as i64;
    }
    s.stk = stk;
    p
}

pub struct LzOut {
    pub tok: Vec<u64>,
    /// (token index, raw byte count) for each stored run
    pub runs: Vec<(i64, i64)>,
}

/// `kz_lz`. `bp` must hold `n` bytes followed by at least 16 zero bytes. `rnd` = chunk classes.
/// `start`: history is bp[..start]. `lp`: literal position bits used by the parser's prices.
pub fn lz77(bp: &[u8], n: i64, start: i64, rnd: &[u8], chain_max: i32, nice: i32, lazy: i32, accel: i32, lp: i32) -> Option<LzOut> {
    if bp.len() < n as usize + 16 {
        return None;
    }
    let nch = rnd.len() as i64;
    let mut pbits = 10u32;
    let cap = if lazy >= 2 { PRB_OPT } else { PRB_CHAIN };
    while pbits < cap && (1i64 << pbits) < n + 32 {
        pbits += 1;
    }
    let pmask = (1i64 << pbits) - 1;
    let plim = if (1i64 << pbits) >= n + 32 { WINDOW } else if pmask - 16 < WINDOW { pmask - 16 } else { WINDOW };
    let use_bt = lazy >= 2;
    let mut bb = 1u32;
    while bb < 24 && (1i64 << bb) < n {
        bb += 1;
    }
    let bmask = (1i64 << bb) - 1;
    let mut c = Ctx {
        b: bp,
        n,
        start,
        last: n - 3,
        ins: 0,
        head: vec![-1; 1 << HBITS],
        head3: vec![-1; 1 << H3BITS],
        head2: vec![-1; 65536],
        prev: vec![-1; (pmask + 1) as usize],
        lhead: vec![-1; 1 << LBITS],
        use_bt,
        btd: chain_max * BTK / 4,
        son: if use_bt { vec![-1; 2 * (bmask + 1) as usize] } else { Vec::new() },
        bmask,
        bt_next: 0,
        pmask,
        plim,
        rnd,
        nice,
    };
    let pr = prc();
    let learn = lazy >= 2;
    let mut s = Lzs {
        buf: bp,
        tok: Vec::with_capacity(((n - start).max(0) as usize / 2) + 16),
        nt: 0,
        pstart: 0,
        learn,
        m: None,
        e: Enc::new(512),
        rep: [1; 4],
        st: 0,
        pos: 0,
        bpos0: 0,
        lastref: 0,
        mlp: [0; 257],
        rlp: [0; 257],
        nd: Vec::new(),
        stk: Vec::new(),
        dsl: [[0; 32]; 4],
        dex: [[0; 8]; 27],
        dal: [0; 16],
        pr,
    };
    if learn {
        let mut m = Box::new(Model::new());
        m.lpm = (1u32 << (if lp < 0 || lp > RC_LPMAX as i32 { 0 } else { lp as u32 })) - 1;
        s.m = Some(m);
        s.nd = vec![Onode::default(); (OPTW as usize) + MAX_MATCH + 8];
        s.stk = vec![0u32; 2 * ((OPTW as usize) + MAX_MATCH + 8)];
        reset_block(&mut s);
    }
    let mut runs: Vec<(i64, i64)> = Vec::new();
    let mut i = start;
    let mut miss: i32 = 0;
    let mut have_cache = false;
    let (mut cl, mut cd) = (0i32, 0i32);
    let mut cpos: i64 = 0;
    insert_to(&mut c, start);
    let b = bp;
    let mut chk = start;
    while i < n {
        if i - chk >= 32768 {
            chk = i;
            if cancelled() {
                return None;
            }
        }
        s.pos = i;
        let k = rnd[(i >> RND_BITS) as usize];
        if k == 2 {
            let mut j = (i >> RND_BITS) + 1;
            while j < nch && rnd[j as usize] == 2 {
                j += 1;
            }
            let mut e = j << RND_BITS;
            if e > n {
                e = n;
            }
            let mut a = i;
            let mut found = false;
            let (mut l, mut d) = (0i32, 0i32);
            loop {
                while a < e && a < c.last && !anchor3(b, a) {
                    a += 1;
                }
                if a >= e || a >= c.last {
                    a = -1;
                    break;
                }
                insert_to(&mut c, a);
                let r = find(&c, a, chain_max);
                l = r.0;
                d = r.1;
                if l >= MIN_RND_MATCH {
                    found = true;
                    break;
                }
                a += 1;
            }
            let end = if found { a } else { e };
            while i < end {
                emit(&mut s, at(b, i as usize) as u64);
                i += 1;
            }
            if found {
                emit(&mut s, ((l as u64) << DBITS) | d as u32 as u64);
                i = a + l as i64;
                while i < n {
                    let l2 = run_len(b, i, n, d as i64);
                    if l2 < 8 {
                        break;
                    }
                    emit(&mut s, ((l2 as u64) << DBITS) | d as u32 as u64);
                    i += l2 as i64;
                }
            }
            have_cache = false;
            miss = 0;
            continue;
        }
        if k == 1 {
            let mut j = (i >> RND_BITS) + 1;
            while j < nch && rnd[j as usize] == 1 {
                j += 1;
            }
            let mut e = j << RND_BITS;
            if e > n {
                e = n;
            }
            let mut a = i;
            let mut found = false;
            let (mut l, mut d) = (0i32, 0i32);
            loop {
                if a >= e {
                    a = -1;
                    break;
                }
                while a < e && a < c.last && !anchor_r(b, a) {
                    a += 1;
                }
                if a >= e || a >= c.last {
                    a = -1;
                    break;
                }
                insert_to(&mut c, a);
                let r = find(&c, a, chain_max);
                l = r.0;
                d = r.1;
                if l >= MIN_RND_MATCH {
                    found = true;
                    break;
                }
                a += 1;
            }
            let mut end = if found { a } else { e };
            if end > i {
                runs.push((s.nt, end - i));
                s.pstart = s.nt;
            }
            s.pos = end;
            if found {
                emit(&mut s, ((l as u64) << DBITS) | d as u32 as u64);
                end += l as i64;
                while end < n {
                    let l2 = run_len(b, end, n, d as i64);
                    if l2 < 8 {
                        break;
                    }
                    emit(&mut s, ((l2 as u64) << DBITS) | d as u32 as u64);
                    end += l2 as i64;
                }
            }
            i = end;
            have_cache = false;
            miss = 0;
            continue;
        }
        let dry = miss >= 32 && accel < 99;
        if s.learn && k == 0 && !dry {
            let mut wend = ((i >> RND_BITS) + 1) << RND_BITS;
            if wend > n {
                wend = n;
            }
            let t0 = s.nt;
            i = opt_window(&mut s, &mut c, i, chain_max, nice, wend);
            for q in t0..s.nt {
                if s.tok[q as usize] < 256 {
                    miss += 1;
                } else {
                    miss = 0;
                }
            }
            have_cache = false;
            continue;
        }
        insert_to(&mut c, i);
        let (l, d);
        if have_cache && cpos == i {
            l = cl;
            d = cd;
        } else {
            let r = find(&c, i, if dry || k == 3 { DRY_CHAIN } else { chain_max });
            l = r.0;
            d = r.1;
        }
        have_cache = false;
        if l != 0 {
            miss = 0;
            if lazy != 0 && l < LAZY_SKIP && i + 1 < n {
                insert_to(&mut c, i + 1);
                let (l2, d2) = find(&c, i + 1, chain_max);
                if l2 > l {
                    emit(&mut s, at(b, i as usize) as u64);
                    have_cache = true;
                    cpos = i + 1;
                    cl = l2;
                    cd = d2;
                    i += 1;
                    continue;
                }
            }
            emit(&mut s, ((l as u64) << DBITS) | d as u32 as u64);
            if l as u32 >= SKIP_INSERT {
                insert_to(&mut c, i + 4);
                c.ins = i + l as i64;
            }
            i += l as i64;
        } else {
            miss += 1;
            let step0 = 1 + if accel >= 31 { 0 } else { miss >> accel };
            if step0 == 1 {
                emit(&mut s, at(b, i as usize) as u64);
                i += 1;
            } else {
                let step = if step0 > MAX_STEP { MAX_STEP } else { step0 };
                let mut e = i + step as i64;
                if e > n {
                    e = n;
                }
                while i < e {
                    emit(&mut s, at(b, i as usize) as u64);
                    i += 1;
                }
            }
        }
    }
    Some(LzOut { tok: s.tok, runs })
}
