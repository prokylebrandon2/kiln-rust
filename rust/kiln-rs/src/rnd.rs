//! Chunk classifier: port of `kz_rnd` (tools/accel_reference.c). One class per 64 KB chunk:
//! 0 normal, 1 looks random, 2 literal-only, 3 sparse. Uses the same hand-made log2 and the same
//! floating-point operation order as the C code, so the classes are identical.

use crate::lz::{log2_pub as log2_, RND_BITS};

const RND_ENTROPY: f64 = 7.95;
const LIT_ENTROPY: f64 = 0.6;
const EASY_ENTROPY: f64 = 3.0;

fn distinct_grams_m(s: &[u8], len: usize, g: usize, tab: &mut [u32], used: &mut [u32], m: u32) -> i32 {
    let ng = len + 1 - g;
    let mut uniq = 0i32;
    for w in used[..(m / 32) as usize].iter_mut() {
        *w = 0;
    }
    for i in 0..ng {
        let mut h: u64 = 1469598103934665603;
        for k in 0..g {
            h = (h ^ s[i + k] as u64).wrapping_mul(1099511628211);
        }
        let mut slot = ((h >> 40) as u32) & (m - 1);
        loop {
            let (w, bit) = ((slot >> 5) as usize, 1u32 << (slot & 31));
            if used[w] & bit == 0 {
                used[w] |= bit;
                tab[slot as usize] = i as u32;
                uniq += 1;
                break;
            }
            let t = tab[slot as usize] as usize;
            if s[t..t + g] == s[i..i + g] {
                break;
            }
            slot = (slot + 1) & (m - 1);
        }
    }
    uniq
}

fn cond_ent(cnt: &[i32], n: usize) -> f64 {
    let mut acc = 0.0f64;
    for c in 0..8 {
        let mut tot = 0i32;
        for v in 0..256 {
            tot += cnt[c * 256 + v];
        }
        if tot == 0 {
            continue;
        }
        let mut h = 0.0f64;
        let it = 1.0 / tot as f64;
        for v in 0..256 {
            if cnt[c * 256 + v] != 0 {
                let p = cnt[c * 256 + v] as f64 * it;
                h -= p * log2_(p);
            }
        }
        acc += tot as f64 / n as f64 * h;
    }
    acc
}

/// Class of every 64 KB chunk of `buf`.
pub fn random_chunks(buf: &[u8]) -> Vec<u8> {
    let n = buf.len() as i64;
    let nchunks = ((n + (1 << RND_BITS) - 1) >> RND_BITS) as usize;
    let mut out = vec![0u8; nchunks];
    let mut s = [0u8; 8192];
    let mut tab = vec![0u32; 16384];
    let mut used = vec![0u32; 16384 / 32];
    let mut tab_b = vec![0u32; 262144];
    let mut used_b = vec![0u32; 262144 / 32];
    let mut ph = vec![0i32; 8 * 256];
    let mut pv = vec![0i32; 8 * 256];
    for ch in 0..nchunks {
        let a = (ch as i64) << RND_BITS;
        let mut len = 0usize;
        for k in 0..8i64 {
            let from = a + k * 8192;
            if from >= n {
                break;
            }
            let cnt = if n - from < 1024 { n - from } else { 1024 } as usize;
            s[len..len + cnt].copy_from_slice(&buf[from as usize..from as usize + cnt]);
            len += cnt;
        }
        if len < 4096 {
            continue;
        }
        let mut freq = [0i32; 256];
        for i in 0..len {
            freq[s[i] as usize] += 1;
        }
        let mut ent = 0.0f64;
        let inv = 1.0 / len as f64;
        for v in 0..256 {
            if freq[v] != 0 {
                let p = freq[v] as f64 * inv;
                ent -= p * log2_(p);
            }
        }
        if ent >= RND_ENTROPY {
            let u = distinct_grams_m(&s, len, 4, &mut tab, &mut used, 16384);
            out[ch] = if u as f64 >= 0.99 * (len - 3) as f64 { 1 } else { 0 };
        } else if ent >= LIT_ENTROPY {
            if ent >= 3.0 {
                let clen = if n - a < (1 << RND_BITS) { n - a } else { 1 << RND_BITS } as usize;
                let mut fq = [0i64; 256];
                for v in ph.iter_mut() {
                    *v = 0;
                }
                for v in pv.iter_mut() {
                    *v = 0;
                }
                let base = a as usize;
                for i in 0..clen {
                    let x = buf[base + i] as usize;
                    fq[x] += 1;
                    ph[(i & 7) * 256 + x] += 1;
                    let pvi = if i != 0 { (buf[base + i - 1] >> 5) as usize } else { 0 };
                    pv[pvi * 256 + x] += 1;
                }
                let mut p2 = 0.0f64;
                let ic = 1.0 / clen as f64;
                let mut h0 = 0.0f64;
                for v in 0..256 {
                    let p = fq[v] as f64 * ic;
                    p2 += p * p;
                    if p > 0.0 {
                        h0 -= p * log2_(p);
                    }
                }
                let gap1 = h0 - cond_ent(&ph, clen);
                let gap2 = h0 - cond_ent(&pv, clen);
                if gap1 < 0.06 && gap2 < 0.06 {
                    let mut ng_ = 4usize;
                    let mut expd = 0.0f64;
                    for g in 4..=16usize {
                        let mut q = 1.0f64;
                        for _ in 0..g {
                            q *= p2;
                        }
                        let t = (clen + 1 - g) as f64;
                        expd = 0.5 * t * t * q;
                        ng_ = g;
                        if expd <= 0.1 * t {
                            break;
                        }
                    }
                    let ng = clen as i64 - ng_ as i64 + 1;
                    if ng > 0 && expd <= 0.1 * ng as f64 {
                        let u = distinct_grams_m(&buf[base..base + clen], clen, ng_, &mut tab_b, &mut used_b, 262144);
                        let dup = (ng - u as i64) as f64;
                        if dup <= 1.5 * expd + 0.004 * ng as f64 {
                            out[ch] = 2;
                        }
                    }
                }
            } else {
                let mut g = (32.0 / ent) as i32 + 1;
                if g > 64 {
                    g = 64;
                }
                let g = g as usize;
                let u = distinct_grams_m(&s, len, g, &mut tab, &mut used, 16384);
                if u as f64 >= 0.99 * (len + 1 - g) as f64 {
                    out[ch] = 2;
                } else if ent < EASY_ENTROPY {
                    out[ch] = 3;
                }
            }
        }
    }
    out
}
