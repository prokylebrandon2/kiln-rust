//! MP3 pack (table filter 120): encoder direction of mp3codec.c (`code_parsed` with c->dec == 0,
//! `mp3c_compress`) on top of the parser in mp3parse.rs. The model (Model, Mixer, slots, tables)
//! is shared with the decoder in mp3.rs, so both directions use the same adaptive state.
//! Output format: byte 3, the file length (8 bytes LE), then the range coder stream.

use crate::mp3::{
    compute_l4, gran_bits, prescale, qm, sf_layout, sf_shared, sm_p, sm_up, Frame, Gran, Model, NQ, NS, STRETCH,
};
use crate::mp3parse::{parse_file, Parsed, Run};

// ---------------------------------------------------------------- range encoder
struct Enc {
    x1: u32,
    x2: u32,
    buf: Vec<u8>,
    pos: usize,
    overflow: bool,
}
impl Enc {
    fn new(cap: usize) -> Enc {
        Enc { x1: 0, x2: 0xffff_ffff, buf: vec![0u8; cap], pos: 0, overflow: false }
    }
    #[inline]
    fn norm(&mut self) {
        while ((self.x1 ^ self.x2) & 0xff00_0000) == 0 {
            if self.pos < self.buf.len() {
                self.buf[self.pos] = (self.x2 >> 24) as u8;
            } else {
                self.overflow = true;
            }
            self.pos += 1;
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
        }
    }
    /// p = P(bit==1) in 12 bits
    #[inline]
    fn code(&mut self, bit: i32, p: i32) -> i32 {
        let xm = self.x1.wrapping_add((((self.x2.wrapping_sub(self.x1)) as u64 * (p as u32) as u64) >> 12) as u32);
        if bit != 0 {
            self.x2 = xm;
        } else {
            self.x1 = xm.wrapping_add(1);
        }
        self.norm();
        bit
    }
    /// n (1..12) equiprobable bits in one step; bitwise when the range is too small
    fn raw(&mut self, v: i32, n: u32) -> i32 {
        let r: u64 = (self.x2.wrapping_sub(self.x1)) as u64 + 1;
        if r < (1u64 << (n + 8)) {
            let mut i = n as i32 - 1;
            while i >= 0 {
                self.code((v >> i) & 1, 2048);
                i -= 1;
            }
            return v;
        }
        let x1 = self.x1;
        self.x2 = x1.wrapping_add(((r * (v as u64 + 1)) >> n) as u32).wrapping_sub(1);
        self.x1 = x1.wrapping_add(((r * v as u64) >> n) as u32);
        self.norm();
        v
    }
    fn flush(&mut self) {
        for _ in 0..4 {
            if self.pos < self.buf.len() {
                self.buf[self.pos] = (self.x1 >> 24) as u8;
            } else {
                self.overflow = true;
            }
            self.pos += 1;
            self.x1 <<= 8;
        }
    }
}

fn code_sm(c: &mut Enc, s: &mut u32, bit: i32, limit: i32) -> i32 {
    let b = c.code(bit, sm_p(*s));
    sm_up(s, b, limit);
    b
}
fn code_tree(c: &mut Enc, sm: &mut [u32], ctx: usize, nb: u32, v: i32) -> i32 {
    let mut node: usize = 1;
    let mut i = nb as i32 - 1;
    while i >= 0 {
        let b = code_sm(c, &mut sm[(ctx << nb) + node], (v >> i) & 1, 60);
        node = node * 2 + b as usize;
        i -= 1;
    }
    (node - (1usize << nb)) as i32
}
fn code_gamma(c: &mut Enc, sm: &mut [u32], v: u32) -> i32 {
    let n = v.wrapping_add(1);
    let mut nb = 0usize;
    while (n >> (nb + 1)) != 0 {
        nb += 1;
    }
    let mut k = 0usize;
    loop {
        let stop = code_sm(c, &mut sm[if k < 31 { k } else { 31 }], (k == nb) as i32, 60);
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
        let mut b = ((n >> i) & 1) as i32;
        if i == nb - 1 {
            b = code_sm(c, &mut sm[32 + if nb < 31 { nb } else { 31 }], b, 60);
        } else {
            b = c.code(b, 2048);
        }
        r = r.wrapping_mul(2).wrapping_add(b as u32);
    }
    r.wrapping_sub(1) as i32
}

struct Packer {
    c: Enc,
    m: Model,
}

/// previous frame's header, mdb and scfsi (the only things the next frame's model looks at)
#[derive(Clone, Copy)]
struct PrevF {
    hdr: [u8; 4],
    mdb: i32,
    scfsi: [[u8; 4]; 2],
}

impl Packer {
    fn cbit(&mut self, i0: usize, i1: usize, i2: usize, mset: usize, bit: i32) -> i32 {
        let m = &mut self.m;
        m.mx.st[0] = STRETCH[sm_p(m.cb[i0]) as usize] as i32;
        m.mx.st[1] = STRETCH[sm_p(m.cd[i1]) as usize] as i32;
        m.mx.st[2] = STRETCH[sm_p(m.cf[i2]) as usize] as i32;
        m.mx.st[3] = 256;
        m.mx.mix(mset);
        let b = self.c.code(bit, m.mx.pr);
        m.mx.up(b);
        sm_up(&mut m.cb[i0], b, 127);
        sm_up(&mut m.cd[i1], b, 127);
        sm_up(&mut m.cf[i2], b, 127);
        b
    }

    /// port of code_coefs, encoder direction. Like the C code it leaves magnitudes in g.xr.
    fn code_coefs(&mut self, g: &mut Gran, pg: Option<&Gran>, og: Option<&Gran>, ch: usize) {
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
            let v = g.xr[i] as i32;
            let mg0 = v.abs();
            let cnt = (i >= nbig) as usize;
            let zn = (a | b | p0 | pm | pp | o0 | om | op) == 0;
            let zi = (((ch * 4 + btc) * 2 + cnt) * 2 + (c3 != 0) as usize) * 72 + (i >> 3);
            let na: i32;
            if cnt != 0 {
                let nzbit = (mg0 != 0) as i32;
                let nz = if zn {
                    code_sm(&mut self.c, &mut self.m.zq[zi], nzbit, 1023)
                } else {
                    self.cbit(ib * NS + 15, id * NS + 15, iff * NS + 15, 15 * 2 + ch, nzbit)
                };
                if nz != 0 {
                    sgnb[nsb] = (v < 0) as u8;
                    nsb += 1;
                    g.xr[i] = 1;
                    na = 1;
                } else {
                    g.xr[i] = 0;
                    na = 0;
                }
            } else {
                let k0: i32 = if mg0 != 0 { 32 - (mg0 as u32).leading_zeros() as i32 } else { 0 };
                let mut s: i32 = 0;
                let mut skip = false;
                if zn {
                    if code_sm(&mut self.c, &mut self.m.zq[zi], (k0 > 0) as i32, 1023) == 0 {
                        skip = true;
                    } else {
                        s = 1;
                    }
                }
                if !skip {
                    while s < 14 {
                        let su = s as usize;
                        let gt = self.cbit(ib * NS + su, id * NS + su, iff * NS + su, su * 2 + ch, (k0 > s) as i32);
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
                    let bit = code_sm(&mut self.c, &mut self.m.mant[base], (mg0 >> j) & 1, 255);
                    r = r * 2 + bit;
                    j -= 1;
                    if j >= 0 {
                        let bit = code_sm(&mut self.c, &mut self.m.mant[base + 1 + (r & 1) as usize], (mg0 >> j) & 1, 255);
                        r = r * 2 + bit;
                        j -= 1;
                    }
                    if j >= 0 {
                        let nb = (j + 1) as u32;
                        let lo = self.c.raw(mg0 & ((1 << nb) - 1), nb);
                        r = (r << nb) | lo;
                    }
                    mg = r;
                } else {
                    mg = k;
                }
                if mg != 0 {
                    sgnb[nsb] = (v < 0) as u8;
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
            let mut v = 0i32;
            for t in 0..n {
                v = v * 2 + sgnb[pos + t] as i32;
            }
            self.c.raw(v, n as u32);
            pos += n;
        }
    }

    /// n raw bits from the pool: all-zero flag, else bit by bit with the previous bit as context
    /// (`zs`/`bs` select the gap or the stuffing slot set)
    fn bitblock(&mut self, pool: &[u8], off: u32, n: u32, which: u8) {
        if n == 0 {
            return;
        }
        let off = off as usize;
        let mut allz = 1;
        for k in 0..n as usize {
            if (pool[off + (k >> 3)] >> (7 - (k & 7))) & 1 != 0 {
                allz = 0;
                break;
            }
        }
        let (zs, bs) = if which == 0 { (&mut self.m.gapz, &mut self.m.gapb) } else { (&mut self.m.stuffz, &mut self.m.stuffb) };
        let allz = code_sm(&mut self.c, &mut zs[0], allz, 60);
        if allz == 0 {
            let mut prev = 0usize;
            for k in 0..n as usize {
                let b = ((pool[off + (k >> 3)] >> (7 - (k & 7))) & 1) as i32;
                let b = code_sm(&mut self.c, &mut bs[prev], b, 60);
                prev = b as usize;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn code_gran(
        &mut self,
        pool: &[u8],
        g: &mut Gran,
        q: Option<&Gran>,
        og: Option<&Gran>,
        scfsi: &[[u8; 4]; 2],
        gi: usize,
        ch: usize,
        sr: usize,
    ) -> i32 {
        let pblk = q.map_or(0, |q| (q.ws) | ((q.bt & 3) << 1) | (q.mixed << 3));
        let blk = (g.ws) | ((g.bt & 3) << 1) | (g.mixed << 3);
        code_tree(&mut self.c, &mut self.m.bt, (pblk + 16 * ch as i32) as usize, 4, blk);
        let pgg = q.map_or(0, |q| q.gg);
        code_tree(&mut self.c, &mut self.m.gg, (q.is_some() as usize) * 2 + ch, 8, (g.gg - pgg) & 255);
        let pbv = q.map_or(0, |q| q.bv);
        let bctx = (ch * 2 + (g.ws != 0 && g.bt == 2) as usize) * 8 + q.map_or(0, |q| (q.bv / 37) as usize);
        code_tree(&mut self.c, &mut self.m.bv, bctx, 9, (g.bv - pbv) & 511);
        code_tree(&mut self.c, &mut self.m.nq, q.map_or(0, |q| (q.nq >> 4) as usize) & 15, 8, g.nq);
        let nts = if g.ws != 0 { 2 } else { 3 };
        for k in 0..3usize {
            if k < nts {
                let pf0 = q.map_or(0, |q| q.ts[k]) as usize;
                code_tree(&mut self.c, &mut self.m.ts, k * 32 + pf0, 5, g.ts[k]);
            } else {
                g.ts[k] = 0;
            }
        }
        if g.ws != 0 {
            for k in 0..3usize {
                code_tree(&mut self.c, &mut self.m.sbg, k * 8, 3, g.sbg[k]);
            }
            g.r0 = 0;
            g.r1 = 0;
        } else {
            code_tree(&mut self.c, &mut self.m.r0, q.map_or(0, |q| q.r0) as usize, 4, g.r0);
            code_tree(&mut self.c, &mut self.m.r1, q.map_or(0, |q| q.r1) as usize, 3, g.r1);
            g.sbg = [0; 3];
        }
        code_tree(&mut self.c, &mut self.m.sfc, q.map_or(0, |q| q.sfc) as usize, 4, g.sfc);
        let pfl = q.map_or(0, |q| q.pre | (q.sfs << 1) | (q.c1t << 2));
        let fl = g.pre | (g.sfs << 1) | (g.c1t << 2);
        code_tree(&mut self.c, &mut self.m.flags, pfl as usize, 3, fl);
        // scalefactors (the shared ones are the first granule's values already, see the parser)
        let mut slen = [0i32; 39];
        let n = sf_layout(g, &mut slen);
        for i in 0..n {
            if sf_shared(scfsi, gi, ch, g, i) {
                continue;
            }
            if slen[i] == 0 {
                g.sf[i] = 0;
                continue;
            }
            let pv = q.map_or(0, |q| q.sf[i]) as usize;
            code_tree(&mut self.c, &mut self.m.sf, i * 16 + (pv & 15), 4, g.sf[i]);
        }
        // stuffing
        code_gamma(&mut self.c, &mut self.m.gam[0..64], g.stuff_n as u32);
        self.bitblock(pool, g.stuff_off, g.stuff_n as u32, 1);
        compute_l4(g, sr);
        self.code_coefs(g, q, og, ch);
        let bits = gran_bits(g, scfsi, gi, ch, sr);
        g.p23 = bits;
        bits
    }

    /// returns the frame's payload size
    fn code_frame(
        &mut self,
        pool: &[u8],
        f: &mut Frame,
        pf: Option<&PrevF>,
        mpos: &mut u64,
        prev_end: &mut u64,
        pg: &mut [Option<Gran>; 2],
    ) {
        for k in 0..4 {
            let ctx = k * 256 + pf.map_or(0, |p| p.hdr[k] as usize);
            code_tree(&mut self.c, &mut self.m.hdr, ctx, 8, f.hdr[k] as i32);
        }
        let sr = ((f.hdr[2] >> 2) & 3) as usize;
        if f.has_crc {
            for k in 0..2 {
                code_tree(&mut self.c, &mut self.m.crc, 0, 8, f.crc[k] as i32);
            }
        }
        let pm = pf.map_or(0, |p| p.mdb);
        code_tree(&mut self.c, &mut self.m.mdb, ((pm >> 5) & 15) as usize, 9, f.mdb);
        code_tree(&mut self.c, &mut self.m.privb, 0, if f.nch == 1 { 5 } else { 3 }, f.privb);
        for ch in 0..f.nch {
            let pv = pf.map_or(0, |p| {
                (p.scfsi[ch][0] | p.scfsi[ch][1] << 1 | p.scfsi[ch][2] << 2 | p.scfsi[ch][3] << 3) as usize
            });
            let v = (f.scfsi[ch][0] | f.scfsi[ch][1] << 1 | f.scfsi[ch][2] << 2 | f.scfsi[ch][3] << 3) as i32;
            code_tree(&mut self.c, &mut self.m.scfsi, pv + 16 * ch, 4, v);
        }
        let s = *mpos as i64 * 8 - f.mdb as i64 * 8;
        self.bitblock(pool, f.gap_off, f.gap_n, 0);
        let scfsi = f.scfsi;
        for gi in 0..2usize {
            for ch in 0..f.nch {
                let (ra, rb) = f.gr[gi].split_at_mut(1);
                let (g, og): (&mut Gran, Option<&Gran>) = if ch == 0 { (&mut ra[0], None) } else { (&mut rb[0], Some(&ra[0])) };
                let q = pg[ch].take();
                let bits = self.code_gran(pool, g, q.as_ref(), og, &scfsi, gi, ch, sr);
                pg[ch] = Some(g.clone());
                if gi == 0 && ch == 0 {
                    *prev_end = s as u64;
                }
                *prev_end += bits as u64;
            }
        }
        *mpos += f.pay as u64;
    }

    fn code_run(&mut self, pool: &[u8], r: &mut Run) {
        let nf = r.f.len();
        code_gamma(&mut self.c, &mut self.m.gam[192..256], nf as u32);
        let mut mpos: u64 = 0;
        let mut prev_end: u64 = 0;
        let mut pg: [Option<Gran>; 2] = [None, None];
        let mut prev: Option<PrevF> = None;
        for fi in 0..nf {
            self.code_frame(pool, &mut r.f[fi], prev.as_ref(), &mut mpos, &mut prev_end, &mut pg);
            let f = &r.f[fi];
            prev = Some(PrevF { hdr: f.hdr, mdb: f.mdb, scfsi: f.scfsi });
        }
        r.total_pay = mpos;
        r.tail_n = (mpos * 8 - prev_end) as u32;
        self.bitblock(pool, r.tail_off, r.tail_n, 0);
    }

    fn code_parsed(&mut self, p: &mut Parsed, orig: &[u8]) {
        code_gamma(&mut self.c, &mut self.m.gam[64..128], p.s.len() as u32);
        let pool = &p.pool;
        for seg in p.s.iter_mut() {
            code_sm(&mut self.c, &mut self.m.segf[0], seg.is_run as i32, 60);
            if !seg.is_run {
                code_gamma(&mut self.c, &mut self.m.gam[128..192], seg.len as u32);
                let mut prev = 0usize;
                for k in 0..seg.len as usize {
                    let b = orig[seg.off as usize + k] as i32;
                    code_tree(&mut self.c, &mut self.m.rawb, prev, 8, b);
                    prev = b as usize;
                }
                continue;
            }
            let r = seg.run.as_mut().unwrap();
            self.code_run(pool, r);
        }
    }
}

/// kz_mp3_pack: packed form of an MP3 file, or None when nothing in it parses / it did not fit.
/// (No size or verification check here; `mp3_pack` below does the python `mp3_pack` checks.)
pub fn mp3c_compress(buf: &[u8], cap: usize) -> Option<Vec<u8>> {
    let n = buf.len();
    if n < 64 || cap < 32 {
        return None;
    }
    let mut p = parse_file(buf);
    if !p.s.iter().any(|s| s.is_run) {
        return None;
    }
    let mut pk = Packer { c: Enc::new(cap - 9), m: Model::new() };
    pk.code_parsed(&mut p, buf);
    pk.c.flush();
    if pk.c.overflow {
        return None;
    }
    let mut out = Vec::with_capacity(pk.c.pos + 9);
    out.push(3u8);
    out.extend_from_slice(&(n as u64).to_le_bytes());
    out.extend_from_slice(&pk.c.buf[..pk.c.pos]);
    Some(out)
}

pub const MP3_GAIN: f64 = 0.97;

/// python `mp3_pack`: packed bytes only if smaller than 97% of the file and decoding gives the file back
pub fn mp3_pack(data: &[u8]) -> Option<Vec<u8>> {
    let n = data.len();
    let cap = n + n / 8 + 4096;
    let blob = mp3c_compress(data, cap)?;
    let m = blob.len();
    if m == 0 || (m as f64) >= (n as f64) * MP3_GAIN {
        return None;
    }
    match crate::mp3::mp3_unpack(&blob, n) {
        Ok(back) if back == data => Some(blob),
        _ => None,
    }
}
