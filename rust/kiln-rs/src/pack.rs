//! Block builder: port of `build_blocks` / `_code_parts` / `_code_block` in kiln_v14.py.
//! Input bytes -> list of blocks (stored or range-coded), choosing the cheapest coding mode.
//! Output must equal the Python engine's blocks byte for byte.

use crate::{lz, rc, rnd};

pub const BLOCK_TOKENS: usize = 524_288;
const TRIAL_TOKENS: usize = 16384;
const MODE_TIE: f64 = 1.01;
const LP_REPARSE: f64 = 0.02;
const MODES: [u32; 6] = [0, 1, 2, 4, 5, 6];
const BIG: u64 = 1 << 60;
/// App default (chain_max, nice, lazy, accel). Faster than kiln_v14.py's (384, 258, 2, 99) at the same ratio;
/// the Python-vector test (tests/blk_vectors.rs) pins the reference values itself.
pub const PARSE: (i32, i32, i32, i32) = (128, 96, 2, 50);

/// One output block. `Stored` points into the input buffer (type 0), `Coded` owns its payload (type 1).
#[derive(Debug, PartialEq, Eq)]
pub enum Block {
    Stored { start: usize, len: usize },
    Coded { raw: usize, payload: Vec<u8> },
}

impl Block {
    pub fn comp_len(&self) -> usize {
        match self {
            Block::Stored { len, .. } => *len,
            Block::Coded { payload, .. } => payload.len(),
        }
    }
}

enum Part {
    Tok(std::ops::Range<usize>),
    Raw(usize),
}

fn code_block(tk: &[u64], buf: &[u8], pos: usize, stat: &mut [u64; 3]) -> Option<(usize, Vec<u8>)> {
    let cap = tk.len() * 16 + 64;
    let size = |r: &Option<(usize, Vec<u8>)>| -> u64 { r.as_ref().map_or(BIG, |x| x.1.len() as u64) };
    let mut res: Vec<(u32, Option<(usize, Vec<u8>)>)> = Vec::new();
    let sz: Vec<(u32, u64)>;
    let best: Option<(usize, Vec<u8>)>;
    if tk.len() <= 3 * TRIAL_TOKENS {
        for &m in &MODES {
            res.push((m, rc::encode_block(tk, buf, pos, m, cap)));
        }
        sz = res.iter().map(|(m, r)| (*m, size(r))).collect();
        // min() over the dict keeps the first minimum in insertion order
        let mut bi = 0;
        for i in 1..sz.len() {
            if sz[i].1 < sz[bi].1 {
                bi = i;
            }
        }
        best = res.swap_remove(bi).1;
        // swap_remove moved the last element: nothing else is read from `res`
    } else {
        let cut = &tk[..TRIAL_TOKENS];
        let tcap = cut.len() * 16 + 64;
        sz = MODES.iter().map(|&m| (m, size(&rc::encode_block(cut, buf, pos, m, tcap)))).collect();
        let mut order: Vec<usize> = (0..sz.len()).collect();
        order.sort_by_key(|&i| sz[i].1); // stable, like sorted()
        let s0 = sz[order[0]].1 as f64 * MODE_TIE;
        let keep: Vec<u32> = order.iter().take(2).filter(|&&i| sz[i].1 as f64 <= s0).map(|&i| sz[i].0).collect();
        let mut b: Option<(usize, Vec<u8>)> = None;
        let mut bs = u64::MAX;
        for m in keep {
            let r = rc::encode_block(tk, buf, pos, m, cap);
            let s = size(&r);
            if s < bs {
                bs = s;
                b = r;
            }
        }
        best = b;
    }
    let get = |m: u32| sz.iter().find(|x| x.0 == m).unwrap().1;
    if get(4).max(get(5)).max(get(6)) < BIG {
        stat[0] += get(4);
        stat[1] += get(5);
        stat[2] += get(6);
    }
    best
}

fn code_parts(parts: &[Part], toks: &[u64], buf: &[u8], start: usize, stat: &mut [u64; 3]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut pos = start;
    for part in parts {
        if lz::cancelled() {
            break; // the caller (build_blocks) notices and returns None
        }
        match part {
            Part::Raw(n) => {
                let mut off = 0;
                while off < *n {
                    let end = (*n).min(off + (1 << 20));
                    blocks.push(Block::Stored { start: pos + off, len: end - off });
                    off += 1 << 20;
                }
                pos += n;
            }
            Part::Tok(r) => {
                let part = &toks[r.clone()];
                let mut i = 0;
                while i < part.len() {
                    if lz::cancelled() {
                        break;
                    }
                    let tk = &part[i..(i + BLOCK_TOKENS).min(part.len())];
                    match code_block(tk, buf, pos, stat) {
                        Some((raw, payload)) if payload.len() < raw => {
                            blocks.push(Block::Coded { raw, payload });
                            pos += raw;
                        }
                        r => {
                            let raw = match r {
                                Some((raw, _)) => raw,
                                None => tk.iter().map(|&t| if t < 256 { 1usize } else { (t >> 32) as usize }).sum(),
                            };
                            blocks.push(Block::Stored { start: pos, len: raw });
                            pos += raw;
                        }
                    }
                    i += BLOCK_TOKENS;
                }
            }
        }
    }
    blocks
}

fn parts_of(out: &lz::LzOut) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut at = 0usize;
    for &(ti, rawlen) in &out.runs {
        let ti = ti as usize;
        if ti > at {
            parts.push(Part::Tok(at..ti));
            at = ti;
        }
        parts.push(Part::Raw(rawlen as usize));
    }
    if out.tok.len() > at {
        parts.push(Part::Tok(at..out.tok.len()));
    }
    parts
}

fn total(bl: &[Block]) -> usize {
    bl.iter().map(|b| b.comp_len()).sum()
}

/// Compress `buf[start..]` (the bytes before `start` are history). Mirrors `build_blocks`.
pub fn build_blocks(buf: &[u8], start: usize, parse: (i32, i32, i32, i32)) -> Option<Vec<Block>> {
    let n = buf.len();
    let mut padded = Vec::with_capacity(n + 16);
    padded.extend_from_slice(buf);
    padded.resize(n + 16, 0);
    let rnd: Vec<u8> = rnd::random_chunks(buf).into_iter().map(|x| if x == 1 { 1 } else { 0 }).collect();
    let (chain, nice, lazy, accel) = parse;
    let first = lz::lz77(&padded, n as i64, start as i64, &rnd, chain, nice, lazy, accel, 0)?;
    let mut stat = [0u64; 3];
    let mut best = code_parts(&parts_of(&first), &first.tok, buf, start, &mut stat);
    if lz::cancelled() {
        return None;
    }
    if stat[0] > 0 {
        let (s4, s5, s6) = (stat[0], stat[1], stat[2]);
        let (lp, sl) = if s5 < s6 { (1, s5) } else { (2, s6) };
        if s4 as f64 >= sl as f64 * (1.0 + LP_REPARSE) {
            let second = lz::lz77(&padded, n as i64, start as i64, &rnd, chain, nice, lazy, accel, lp)?;
            let mut ignore = [0u64; 3];
            let alt = code_parts(&parts_of(&second), &second.tok, buf, start, &mut ignore);
            if lz::cancelled() {
                return None;
            }
            if total(&alt) < total(&best) {
                best = alt;
            }
        }
    }
    Some(best)
}
