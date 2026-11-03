//! MP3 parser for the PACK direction (table filter 120). Port of `parse_file` / `dec_gran` /
//! `parse_side` / `trie_build` of mp3core.c (inside `_MP3_SRC` of kiln_v14.py).
//! File bytes -> segments (raw bytes | runs of frames with decoded granules) + a bit pool that
//! holds the gap / tail / stuffing bits. Anything that does not parse stays raw.
//! The decoder (mp3.rs) rebuilds the file from these pieces; the entropy coder (mp3enc.rs) packs them.
//!
//! Memory: like the C code, one `Frame` (~10 KB) is kept per parsed frame.

use crate::mp3::{hdr_ok, regions, sf_layout, sf_shared, Frame, Gran, BR, SR};
use crate::mp3_tables::HT;
use std::sync::OnceLock;

/// Huffman decode trie: child > 0 = node index, child < 0 = leaf -(value + 1), 0 = none
type Node = [i16; 2];

fn tries() -> &'static Vec<Vec<Node>> {
    static T: OnceLock<Vec<Vec<Node>>> = OnceLock::new();
    T.get_or_init(|| {
        let mut all: Vec<Vec<Node>> = Vec::with_capacity(34);
        for t in 0..34usize {
            let (xlen, _lb, code, len) = HT[t];
            if xlen == 0 {
                all.push(Vec::new());
                continue;
            }
            let n = if t >= 32 { 16 } else { xlen * xlen };
            let mut tr: Vec<Node> = vec![[0i16; 2]; 2 * n + 2];
            let mut cnt: usize = 1;
            let code = code.unwrap();
            let len = len.unwrap();
            for v in 0..n {
                let l = len[v] as i32;
                if l == 0 {
                    continue;
                }
                let c = code[v] as u32;
                let mut cur = 0usize;
                let mut b = l - 1;
                while b >= 0 {
                    let bit = ((c >> b) & 1) as usize;
                    if b == 0 {
                        tr[cur][bit] = -((v + 1) as i16);
                    } else {
                        if tr[cur][bit] == 0 {
                            tr[cur][bit] = cnt as i16;
                            cnt += 1;
                        }
                        cur = tr[cur][bit] as usize;
                    }
                    b -= 1;
                }
            }
            all.push(tr);
        }
        all
    })
}

struct BitReader<'a> {
    b: &'a [u8],
    pos: u64,
    end: u64,
}
impl<'a> BitReader<'a> {
    fn bit(&mut self) -> Option<u32> {
        if self.pos >= self.end {
            return None;
        }
        let v = (self.b[(self.pos >> 3) as usize] >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Some(v as u32)
    }
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }
    /// walk the trie of table `t`; None = ran out of bits or no such code
    fn sym(&mut self, t: usize) -> Option<usize> {
        let tr = &tries()[t];
        let mut cur = 0usize;
        loop {
            let b = self.bit()? as usize;
            let nx = tr[cur][b];
            if nx < 0 {
                return Some((-(nx as i32) - 1) as usize);
            }
            if nx == 0 {
                return None;
            }
            cur = nx as usize;
        }
    }
}

pub(crate) struct Run {
    pub f: Vec<Frame>,
    pub tail_n: u32,
    pub tail_off: u32,
    pub total_pay: u64,
}
pub(crate) struct Seg {
    pub is_run: bool,
    pub off: u64,
    pub len: u64,
    pub run: Option<Run>,
}
pub(crate) struct Parsed {
    pub s: Vec<Seg>,
    pub pool: Vec<u8>,
}

fn pool_put_bits(pool: &mut Vec<u8>, src: &[u8], bitpos: u64, nbits: u32) -> u32 {
    let nb = (nbits as usize + 7) / 8;
    let off = pool.len();
    pool.resize(off + nb, 0);
    for i in 0..nbits as u64 {
        let p = bitpos + i;
        if (src[(p >> 3) as usize] >> (7 - (p & 7))) & 1 != 0 {
            pool[off + (i as usize >> 3)] |= 0x80 >> (i & 7);
        }
    }
    off as u32
}

/// decode one granule from main-data bits [start, start + p23); false = does not parse
fn dec_gran(
    ms: &[u8],
    start: u64,
    g: &mut Gran,
    scfsi: &[[u8; 4]; 2],
    g0sf: &[i32; 39],
    gri: usize,
    ch: usize,
    sr: usize,
    pool: &mut Vec<u8>,
) -> bool {
    let mut r = BitReader { b: ms, pos: start, end: start + g.p23 as u64 };
    let mut slen = [0i32; 39];
    let n = sf_layout(g, &mut slen);
    for i in 0..n {
        if sf_shared(scfsi, gri, ch, g, i) {
            g.sf[i] = g0sf[i];
            continue;
        }
        match r.bits(slen[i] as u32) {
            Some(v) => g.sf[i] = v as i32,
            None => return false,
        }
    }
    if g.bv > 288 {
        return false;
    }
    let (r1s, r2s) = regions(g, sr);
    let nbig = (g.bv * 2) as usize;
    if nbig > 576 {
        return false;
    }
    g.xr = [0i16; 576];
    let mut i = 0usize;
    let mut reg = 0;
    while reg < 3 && i < nbig {
        let mut e = (if reg == 0 { r1s } else if reg == 1 { r2s } else { 576 }) as usize;
        if e > nbig {
            e = nbig;
        }
        let t = g.ts[reg] as usize;
        if t == 4 || t == 14 || t > 31 {
            return false;
        }
        if t == 0 {
            if e > i {
                i = e;
            }
            reg += 1;
            continue;
        }
        let (xl, lb, _, _) = HT[t];
        while i < e {
            let v = match r.sym(t) {
                Some(v) => v,
                None => return false,
            };
            let mut x = (v / xl) as i32;
            let mut y = (v % xl) as i32;
            if x == 15 && lb != 0 {
                match r.bits(lb) {
                    Some(l) => x += l as i32,
                    None => return false,
                }
            }
            if x != 0 {
                match r.bit() {
                    Some(1) => x = -x,
                    Some(_) => {}
                    None => return false,
                }
            }
            if y == 15 && lb != 0 {
                match r.bits(lb) {
                    Some(l) => y += l as i32,
                    None => return false,
                }
            }
            if y != 0 {
                match r.bit() {
                    Some(1) => y = -y,
                    Some(_) => {}
                    None => return false,
                }
            }
            g.xr[i] = x as i16;
            g.xr[i + 1] = y as i16;
            i += 2;
        }
        reg += 1;
    }
    // count1 quads
    let ct = if g.c1t != 0 { 33 } else { 32 };
    let mut q = 0i32;
    i = nbig;
    while i + 4 <= 576 {
        let save = r.pos;
        let v = match r.sym(ct) {
            Some(v) => v,
            None => {
                r.pos = save;
                break;
            }
        };
        let mut a = [((v >> 3) & 1) as i16, ((v >> 2) & 1) as i16, ((v >> 1) & 1) as i16, (v & 1) as i16];
        let mut ok = true;
        for k in 0..4 {
            if a[k] != 0 {
                match r.bit() {
                    Some(b) => {
                        if b != 0 {
                            a[k] = -1;
                        }
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        if !ok {
            r.pos = save;
            break;
        }
        for k in 0..4 {
            g.xr[i + k] = a[k];
        }
        i += 4;
        q += 1;
    }
    g.nq = q;
    let used = r.pos - start;
    g.stuff_n = (g.p23 as i64 - used as i64) as i32;
    if g.stuff_n < 0 {
        return false;
    }
    g.stuff_off = pool_put_bits(pool, ms, r.pos, g.stuff_n as u32);
    true
}

fn parse_side(si: &[u8], f: &mut Frame) {
    let nch = f.nch;
    let mut r = BitReader { b: si, pos: 0, end: (if nch == 1 { 17 } else { 32 }) as u64 * 8 };
    let mut rb = |n: u32| -> i32 { r.bits(n).unwrap_or(0) as i32 };
    f.mdb = rb(9);
    f.privb = rb(if nch == 1 { 5 } else { 3 });
    for c in 0..nch {
        for k in 0..4 {
            f.scfsi[c][k] = rb(1) as u8;
        }
    }
    for gi in 0..2 {
        for c in 0..nch {
            let g = &mut f.gr[gi][c];
            g.p23 = rb(12);
            g.bv = rb(9);
            g.gg = rb(8);
            g.sfc = rb(4);
            g.ws = rb(1);
            g.bt = 0;
            g.mixed = 0;
            g.ts = [0; 3];
            g.sbg = [0; 3];
            g.r0 = 0;
            g.r1 = 0;
            if g.ws != 0 {
                g.bt = rb(2);
                g.mixed = rb(1);
                for k in 0..2 {
                    g.ts[k] = rb(5);
                }
                for k in 0..3 {
                    g.sbg[k] = rb(3);
                }
            } else {
                for k in 0..3 {
                    g.ts[k] = rb(5);
                }
                g.r0 = rb(4);
                g.r1 = rb(3);
            }
            g.pre = rb(1);
            g.sfs = rb(1);
            g.c1t = rb(1);
        }
    }
}

/// (frame size in bytes, sample-rate index, channels) of a valid MPEG-1 Layer III header
fn hdr_info(h: &[u8]) -> Option<(usize, usize, usize)> {
    let hh = [h[0], h[1], h[2], h[3]];
    if !hdr_ok(&hh) {
        return None;
    }
    let br = (h[2] >> 4) as usize;
    let s = ((h[2] >> 2) & 3) as usize;
    let pad = ((h[2] >> 1) & 1) as i32;
    let nch = if (h[3] >> 6) == 3 { 1 } else { 2 };
    let fs = 144000 * BR[br] / SR[s] + pad;
    Some((fs as usize, s, nch))
}

pub(crate) fn parse_file(buf: &[u8]) -> Parsed {
    let n = buf.len() as u64;
    let mut p = Parsed { s: Vec::new(), pool: Vec::new() };
    let mut pos: u64 = 0;
    let mut raw0: u64 = 0;
    let mut run: Option<Run> = None;
    let mut ms: Vec<u8> = Vec::new();
    let mut prev_end: u64 = 0;
    let mut run_off: u64 = 0;

    // END_RUN: close the open run (if any) and make it a segment
    fn end_run(
        p: &mut Parsed,
        run: &mut Option<Run>,
        ms: &[u8],
        prev_end: u64,
        run_off: u64,
        pos: u64,
        raw0: &mut u64,
    ) {
        if let Some(mut r) = run.take() {
            let mslen = ms.len() as u64;
            r.tail_n = (mslen * 8 - prev_end) as u32;
            r.tail_off = pool_put_bits(&mut p.pool, ms, prev_end, r.tail_n);
            r.total_pay = mslen;
            p.s.push(Seg { is_run: true, off: run_off, len: pos - run_off, run: Some(r) });
            *raw0 = pos;
        }
    }

    while pos + 4 <= n {
        let pu = pos as usize;
        if let Some((fs, sr, nch)) = hdr_info(&buf[pu..pu + 4]) {
            if pos + fs as u64 <= n {
                let nxt_ok = if pos + fs as u64 + 4 <= n { hdr_info(&buf[pu + fs..pu + fs + 4]).is_some() } else { true };
                let has_crc = (buf[pu + 1] & 1) == 0;
                let si = if nch == 1 { 17 } else { 32 };
                let hl = 4 + if has_crc { 2 } else { 0 };
                if nxt_ok && fs >= hl + si {
                    let mut f = Frame::new();
                    f.hdr.copy_from_slice(&buf[pu..pu + 4]);
                    f.has_crc = has_crc;
                    if has_crc {
                        f.crc.copy_from_slice(&buf[pu + 4..pu + 6]);
                    }
                    f.nch = nch;
                    f.pay = fs - hl - si;
                    parse_side(&buf[pu + hl..pu + hl + si], &mut f);
                    if run.is_none() {
                        if pos > raw0 {
                            p.s.push(Seg { is_run: false, off: raw0, len: pos - raw0, run: None });
                        }
                        run = Some(Run { f: Vec::new(), tail_n: 0, tail_off: 0, total_pay: 0 });
                        run_off = pos;
                        ms.clear();
                        prev_end = 0;
                    }
                    let m0 = ms.len() as u64;
                    ms.extend_from_slice(&buf[pu + hl + si..pu + fs]);
                    let mslen = ms.len() as u64;
                    let s_bits = m0 as i64 * 8 - f.mdb as i64 * 8;
                    let mut good = s_bits >= 0 && s_bits as u64 >= prev_end;
                    let pool_mark = p.pool.len();
                    let mut cur: u64 = 0;
                    if good {
                        cur = s_bits as u64;
                        'gl: for gi in 0..2usize {
                            for c in 0..nch {
                                let p23 = f.gr[gi][c].p23 as u64;
                                if cur + p23 > mslen * 8 {
                                    good = false;
                                    break 'gl;
                                }
                                let g0sf = f.gr[0][c].sf;
                                let ok = dec_gran(&ms, cur, &mut f.gr[gi][c], &f.scfsi, &g0sf, gi, c, sr, &mut p.pool);
                                if !ok {
                                    good = false;
                                    break 'gl;
                                }
                                cur += p23;
                            }
                        }
                    }
                    if good {
                        f.gap_n = (s_bits as u64 - prev_end) as u32;
                        f.gap_off = pool_put_bits(&mut p.pool, &ms, prev_end, f.gap_n);
                        prev_end = cur;
                        run.as_mut().unwrap().f.push(f);
                        pos += fs as u64;
                        continue;
                    }
                    // roll back, the frame stays raw
                    p.pool.truncate(pool_mark);
                    ms.truncate(m0 as usize);
                    end_run(&mut p, &mut run, &ms, prev_end, run_off, pos, &mut raw0);
                    pos += fs as u64;
                    continue;
                }
            }
        }
        end_run(&mut p, &mut run, &ms, prev_end, run_off, pos, &mut raw0);
        pos += 1;
    }
    end_run(&mut p, &mut run, &ms, prev_end, run_off, pos, &mut raw0);
    if n > raw0 {
        p.s.push(Seg { is_run: false, off: raw0, len: n - raw0, run: None });
    }
    p
}
