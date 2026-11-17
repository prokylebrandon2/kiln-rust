//! Native DEFLATE (RFC 1951) decoder for ZIP entries, in safe Rust.
//!
//! Whole-buffer decoding: the compressed bytes and an output buffer of the exact size are in
//! memory, as for a ZIP entry whose size the central directory states. The design follows what
//! makes libdeflate fast:
//!
//! - a 64-bit bit buffer, refilled with one unaligned 8-byte load and no per-byte loop; it holds
//!   at least 56 bits afterwards, enough for a whole length and distance pair (at most 48 bits);
//! - two-level lookup tables (11 root bits for literals and lengths, 8 for distances) whose
//!   entries carry the literal, the length or distance base, the number of bits to consume and
//!   the flags, so a symbol costs one table load and the bits of the codeword and of its extra
//!   field leave the buffer with a single shift;
//! - a **fast loop** that runs while at least [`FAST_IN`] input bytes and [`FAST_OUT`] output
//!   bytes are left. Then no step can run out of either buffer, so it has no per-symbol room
//!   checks: one refill per round, up to three literals per refill, matches copied in 16-byte
//!   (or 8-byte) pieces that may write up to 15 bytes past the end of the match. Later output
//!   overwrites those bytes, and the margin keeps them inside the buffer;
//! - a **slow loop** that checks everything and finishes the last bytes of both buffers.
//!
//! Input past the end of the data reads as zero bits. The bits actually consumed are checked, so a
//! truncated stream is an error, never a panic or an out-of-bounds read. The only `unsafe` is in
//! the fast loop (`refill_fast`, `copy_fast` and the literal stores in `decode_block`): it drops
//! bounds checks that the loop's own guard has already made redundant, and each use is justified
//! where it stands. The slow loop, the table builder and everything that handles damaged input are
//! safe code.
//!
//! What the RFC and zlib reject, this decoder rejects. libdeflate is more lenient with corrupt
//! streams: it decodes the length symbols 286 and 287 as 258, and it gives a meaning to an unused
//! codeword of a one-codeword (or empty) distance code. On damaged input this decoder can
//! therefore reject a stream that libdeflate accepts.
//!
//! Used by the ZIP extractor only when `KILN_INFLATE=native` is set (see [`enabled`]); libdeflate
//! stays the default until the benchmark (`kiln-win bench-inflate`) says otherwise.
//! `win/tools/inflate_model.py` is a line-for-line model of the algorithm that was checked against
//! zlib; change both together.

use std::fmt;

/// Why a stream was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The input ended before the final block was complete.
    Truncated,
    /// The output buffer is too small for the decoded data.
    OutputFull,
    /// A block header, code length table or Huffman code is invalid.
    BadHeader,
    /// A code that cannot occur: unassigned, length symbol 286/287 or distance symbol 30/31.
    BadSymbol,
    /// A match reaches back before the start of the output.
    BadDistance,
    /// The length check of a stored block failed.
    BadStored,
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "deflate stream is truncated",
            Self::OutputFull => "deflate stream is longer than its stated size",
            Self::BadHeader => "invalid deflate block header or Huffman code",
            Self::BadSymbol => "invalid deflate symbol",
            Self::BadDistance => "deflate match reaches before the start of the data",
            Self::BadStored => "invalid deflate stored block",
        })
    }
}

impl std::error::Error for InflateError {}

/// True when `KILN_INFLATE=native` asks for this decoder instead of libdeflate.
pub fn enabled() -> bool {
    std::env::var("KILN_INFLATE").is_ok_and(|v| v == "native")
}

const LIT_ROOT: u32 = 11;
const DIST_ROOT: u32 = 8;
const PRE_ROOT: u32 = 7;
const LIT_MASK: usize = (1 << LIT_ROOT) - 1;
const DIST_MASK: usize = (1 << DIST_ROOT) - 1;
const PRE_MASK: usize = (1 << PRE_ROOT) - 1;

const MAX_LIT_SYMS: usize = 288;
const MAX_DIST_SYMS: usize = 32;

// Table sizes are upper bounds that hold for every complete code: a second-level table exists
// only under a root prefix that has at least two codewords below it, so there are at most half
// as many of them as symbols, and each has at most 2^(15 - root) entries.
const LIT_SIZE: usize = (1 << LIT_ROOT) + (MAX_LIT_SYMS / 2) * (1 << (15 - LIT_ROOT));
const DIST_SIZE: usize = (1 << DIST_ROOT) + (MAX_DIST_SYMS / 2) * (1 << (15 - DIST_ROOT));
const PRE_SIZE: usize = 1 << PRE_ROOT;

// A table entry (u32):
//   bits 0..=7    bits to consume: the codeword plus its extra bits; for a subtable pointer,
//                 the root bits
//   bits 8..=11   codeword length (the extra bits start behind it); for a subtable pointer,
//                 the number of bits that index the subtable
//   bit 12        F_BAD: a code that cannot occur
//   bit 13        F_EOB: end of block
//   bit 14        F_SUB: a subtable pointer
//   bits 16..=30  literal byte, length or distance base, or the subtable's first index
//   bit 31        F_LIT: a literal
// In a subtable the codeword length counts only the bits behind the root bits.
const F_LIT: u32 = 1 << 31;
const F_SUB: u32 = 1 << 14;
const F_EOB: u32 = 1 << 13;
const F_BAD: u32 = 1 << 12;

/// Input bytes the fast loop needs per round: one 8-byte load.
const FAST_IN: usize = 8;
/// Output bytes the fast loop needs per round: the longest match (258 bytes) plus the up to 15
/// bytes that the last 16-byte piece of a copy can write past it, with some room to spare.
const FAST_OUT: usize = 288;

#[rustfmt::skip]
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
#[rustfmt::skip]
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
#[rustfmt::skip]
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
#[rustfmt::skip]
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
#[rustfmt::skip]
const PRE_ORDER: [u8; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Entry of a literal/length symbol whose codeword is `len` bits long (in its table).
fn lit_entry(sym: usize, len: u32) -> u32 {
    let head = (len << 8) | len;
    match sym {
        0..=255 => F_LIT | ((sym as u32) << 16) | head,
        256 => F_EOB | head,
        257..=285 => {
            let i = sym - 257;
            (u32::from(LEN_BASE[i]) << 16) | (len << 8) | (len + u32::from(LEN_EXTRA[i]))
        }
        _ => F_BAD | head,
    }
}

/// Entry of a distance symbol whose codeword is `len` bits long (in its table).
fn dist_entry(sym: usize, len: u32) -> u32 {
    if sym < 30 {
        (u32::from(DIST_BASE[sym]) << 16) | (len << 8) | (len + u32::from(DIST_EXTRA[sym]))
    } else {
        F_BAD | (len << 8) | len
    }
}

/// Entry of a code length code symbol whose codeword is `len` bits long.
fn pre_entry(sym: usize, len: u32) -> u32 {
    ((sym as u32) << 16) | (len << 8) | len
}

/// Fills `table` (root of `1 << root` entries, subtables after it) for the canonical code given
/// by `lens` (one length per symbol, 0 = unused). Over-subscribed codes are rejected. Incomplete
/// ones are rejected too, except, when `allow_incomplete` is set, a code with no symbol or with
/// one symbol of length 1 (a block without matches, a single distance).
fn build_table(
    lens: &[u8],
    table: &mut [u32],
    root: u32,
    make: fn(usize, u32) -> u32,
    allow_incomplete: bool,
) -> Result<(), InflateError> {
    debug_assert!(lens.len() <= MAX_LIT_SYMS);
    let mut count = [0u16; 16];
    for &len in lens {
        count[usize::from(len)] += 1;
    }
    count[0] = 0;

    let mut left: i32 = 1;
    for &c in &count[1..] {
        left = (left << 1) - i32::from(c);
        if left < 0 {
            return Err(InflateError::BadHeader);
        }
    }
    let used: usize = count[1..].iter().map(|&c| usize::from(c)).sum();
    let trivial = used == 0 || (used == 1 && count[1] == 1);
    if left > 0 && !(allow_incomplete && trivial) {
        return Err(InflateError::BadHeader);
    }

    // Symbols ordered by (length, symbol): the order in which canonical codes are assigned.
    let mut offs = [0u16; 16];
    let mut acc = 0u16;
    for len in 1..16 {
        offs[len] = acc;
        acc += count[len];
    }
    let mut sorted = [0u16; MAX_LIT_SYMS];
    for (sym, &len) in lens.iter().enumerate() {
        if len != 0 {
            let slot = &mut offs[usize::from(len)];
            sorted[usize::from(*slot)] = sym as u16;
            *slot += 1;
        }
    }

    // Codewords, bit-reversed because the stream is read least significant bit first.
    let mut rev = [0u16; MAX_LIT_SYMS];
    let mut code = 0u32;
    let mut prev = 0u32;
    for (k, &sym) in sorted[..used].iter().enumerate() {
        let len = u32::from(lens[usize::from(sym)]);
        if k > 0 && len > prev {
            code <<= len - prev;
        }
        prev = len;
        rev[k] = (code.reverse_bits() >> (32 - len)) as u16;
        code += 1;
    }

    let root_size = 1usize << root;
    table[..root_size].fill(F_BAD);
    let mut next_sub = root_size;
    let mut k = 0;
    while k < used {
        let sym = usize::from(sorted[k]);
        let len = u32::from(lens[sym]);
        if len <= root {
            let entry = make(sym, len);
            let mut i = usize::from(rev[k]);
            while i < root_size {
                table[i] = entry;
                i += 1 << len;
            }
            k += 1;
            continue;
        }
        // The codes that share these root bits are consecutive in `sorted`, shortest first, so
        // the last of them is the longest and sizes the subtable.
        let prefix = usize::from(rev[k]) & (root_size - 1);
        let mut end = k + 1;
        while end < used && (usize::from(rev[end]) & (root_size - 1)) == prefix {
            end += 1;
        }
        let sub_bits = u32::from(lens[usize::from(sorted[end - 1])]) - root;
        let sub_size = 1usize << sub_bits;
        if next_sub + sub_size > table.len() {
            return Err(InflateError::BadHeader);
        }
        table[prefix] = F_SUB | ((next_sub as u32) << 16) | (sub_bits << 8) | root;
        for (&s, &r) in sorted[k..end].iter().zip(&rev[k..end]) {
            let sub_len = u32::from(lens[usize::from(s)]) - root;
            let entry = make(usize::from(s), sub_len);
            let mut i = usize::from(r) >> root;
            while i < sub_size {
                table[next_sub + i] = entry;
                i += 1 << sub_len;
            }
        }
        next_sub += sub_size;
        k = end;
    }
    Ok(())
}

/// The entry that the subtable pointer `e` selects. Consumes the root bits; `b.buf` then starts
/// with the bits that index the subtable.
#[inline(always)]
fn sub_entry(table: &[u32], e: u32, b: &mut Bits<'_>) -> u32 {
    b.skip(e & 0xFF);
    let index = (e >> 16) as usize + ((b.buf as usize) & ((1usize << ((e >> 8) & 15)) - 1));
    table[index]
}

/// The value of the extra bits of the entry `e`, given the bit buffer `saved` from before its
/// codeword and extra bits were consumed.
#[inline(always)]
fn extra_value(saved: u64, e: u32) -> usize {
    ((saved & ((1u64 << (e & 0xFF)) - 1)) >> ((e >> 8) & 15)) as usize
}

/// The bit reader. `pos * 8 - cnt` is the number of bits consumed; bits of `buf` above `cnt` are
/// either zero or the true following bits of the stream, which later refills OR in again.
#[derive(Clone, Copy)]
struct Bits<'a> {
    input: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
}

impl Bits<'_> {
    /// Makes sure that at least 56 bits are in the buffer.
    #[inline(always)]
    fn refill(&mut self) {
        if let Some(chunk) = self.input.get(self.pos..self.pos + 8) {
            let word = u64::from_le_bytes(chunk.try_into().unwrap());
            self.buf |= word << self.cnt;
            self.pos += ((63 - self.cnt) >> 3) as usize;
            self.cnt |= 56;
        } else {
            let (pos, buf, cnt) = refill_tail(self.input, self.pos, self.buf, self.cnt);
            self.pos = pos;
            self.buf = buf;
            self.cnt = cnt;
        }
    }

    /// `refill` for a caller that has checked `pos + 8 <= input.len()`. The assert repeats that
    /// check, so this is sound on its own; in the fast loop it is the loop's own condition, which
    /// the compiler already knows.
    #[inline(always)]
    fn refill_fast(&mut self) {
        assert!(self.pos + 8 <= self.input.len());
        // SAFETY: the assert puts the 8 bytes `pos..pos + 8` inside `input`, and `read_unaligned`
        // has no alignment requirement.
        let word = u64::from_le(unsafe {
            std::ptr::read_unaligned(self.input.as_ptr().add(self.pos).cast::<u64>())
        });
        self.buf |= word << self.cnt;
        self.pos += ((63 - self.cnt) >> 3) as usize;
        self.cnt |= 56;
    }

    /// Removes and returns `n` bits (at most 32, and at most `cnt`).
    #[inline(always)]
    fn take(&mut self, n: u32) -> u32 {
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.skip(n);
        v
    }

    #[inline(always)]
    fn skip(&mut self, n: u32) {
        self.buf >>= n;
        self.cnt -= n;
    }

    /// True when bits past the end of the input were consumed.
    #[inline(always)]
    fn overrun(&self) -> bool {
        self.pos * 8 > self.input.len() * 8 + self.cnt as usize
    }
}

/// Byte-wise refill near the end of the input; bytes past the end count as zeros. Takes and
/// returns the reader's fields so that the reader itself never has its address taken, which would
/// keep it out of registers in the hot loop.
#[cold]
#[inline(never)]
fn refill_tail(input: &[u8], mut pos: usize, mut buf: u64, mut cnt: u32) -> (usize, u64, u32) {
    while cnt <= 55 {
        let byte = input.get(pos).copied().unwrap_or(0);
        buf |= u64::from(byte) << cnt;
        pos += 1;
        cnt += 8;
    }
    (pos, buf, cnt)
}

/// A stored block: LEN and NLEN after the next byte boundary, then LEN raw bytes.
fn stored_block(bits: &mut Bits<'_>, out: &mut [u8], op: usize) -> Result<usize, InflateError> {
    bits.skip(bits.cnt & 7);
    bits.refill();
    let len = bits.take(16) as usize;
    let nlen = bits.take(16) as usize;
    if len != !nlen & 0xFFFF {
        return Err(InflateError::BadStored);
    }
    // Whole bytes still in the bit buffer belong to the data: give them back to the input.
    bits.pos -= (bits.cnt >> 3) as usize;
    bits.buf = 0;
    bits.cnt = 0;
    let end = bits
        .pos
        .checked_add(len)
        .filter(|&end| end <= bits.input.len())
        .ok_or(InflateError::Truncated)?;
    if len > out.len() - op {
        return Err(InflateError::OutputFull);
    }
    out[op..op + len].copy_from_slice(&bits.input[bits.pos..end]);
    bits.pos = end;
    Ok(op + len)
}

/// Copies a match of `length` bytes (3 to 258) from `distance` bytes back, for the fast loop,
/// which guarantees that at least 16 bytes are free after the match. Whole 16-byte pieces (8-byte
/// ones for distances 8 to 15) are copied, so up to 15 bytes past the match are written too.
/// Every piece reads only bytes that are already final: the distance is at least the piece size.
///
/// The single assert is what makes the pointer code below sound: `1 <= distance <= op` and 16 free
/// bytes after the match. The fast loop guarantees both (it has checked `distance <= op`, and
/// `op + FAST_OUT <= out.len()` with a length of at most 258), so the assert never fires.
#[inline(always)]
fn copy_fast(out: &mut [u8], op: usize, distance: usize, length: usize) {
    assert!(
        (1..=op).contains(&distance) && op.saturating_add(length).saturating_add(16) <= out.len()
    );
    let base = out.as_mut_ptr();
    // SAFETY: every access below has `i < length`. A read of `n` bytes at `src + i` ends at
    // `op - distance + i + n`, which is at most `op + i` when `distance >= n` (and the byte reads
    // for distances 2 to 7 are `n = 1` below `op + i`), so it is inside `out`. A write of `n <= 16`
    // bytes at `dst + i` ends at `op + i + n <= op + length + 15`, inside `out` by the assert. For
    // `copy_nonoverlapping`, `dst + i` is `distance >= n` bytes after `src + i`, so the two ranges
    // of `n` bytes are disjoint. A piece may read bytes an earlier piece wrote: the calls run in
    // order.
    unsafe {
        let src = base.add(op - distance);
        let dst = base.add(op);
        if distance >= 16 {
            let mut i = 0;
            while i < length {
                std::ptr::copy_nonoverlapping(src.add(i), dst.add(i), 16);
                i += 16;
            }
        } else if distance >= 8 {
            let mut i = 0;
            while i < length {
                std::ptr::copy_nonoverlapping(src.add(i), dst.add(i), 8);
                i += 8;
            }
        } else if distance == 1 {
            std::ptr::write_bytes(dst, *src, length);
        } else {
            // 2 to 7: the pattern repeats with a period shorter than a word. One byte at a time,
            // in order, so every byte read is one that was already written.
            for i in 0..length {
                *dst.add(i) = *src.add(i);
            }
        }
    }
}

/// Copies a match exactly, without writing past it, for the slow loop.
fn copy_exact(out: &mut [u8], op: usize, distance: usize, length: usize) {
    let src = op - distance;
    if distance >= 16 && out.len() - op >= length + 16 {
        let mut i = 0;
        while i < length {
            out.copy_within(src + i..src + i + 16, op + i);
            i += 16;
        }
    } else if distance >= length {
        out.copy_within(src..src + length, op);
    } else if distance == 1 {
        let byte = out[src];
        out[op..op + length].fill(byte);
    } else {
        copy_overlapping(out, op, distance, length);
    }
}

/// A match that overlaps its own output (`distance < length`): copy the pattern once, then keep
/// doubling what is copied.
#[inline(always)]
fn copy_overlapping(out: &mut [u8], op: usize, distance: usize, length: usize) {
    let src = op - distance;
    let mut done = 0;
    while done < length {
        let n = (length - done).min(distance + done);
        out.copy_within(src..src + n, op + done);
        done += n;
    }
}

/// Decodes the symbols of one Huffman block up to its end-of-block code; returns the new output
/// position.
fn decode_block(
    bits: &mut Bits<'_>,
    out: &mut [u8],
    mut op: usize,
    lit: &[u32; LIT_SIZE],
    dist: &[u32; DIST_SIZE],
) -> Result<usize, InflateError> {
    // Work on a copy so that the compiler keeps the reader in registers.
    let mut b = *bits;
    let out_len = out.len();
    // With this, `op + FAST_OUT` cannot wrap (a slice is at most `isize::MAX` bytes long), so the
    // fast loop's guard below really means that `FAST_OUT` bytes are free.
    assert!(op <= out_len);
    let in_len = b.input.len();

    // The fast loop. After the refill there are at least 56 bits: three literal codewords are at
    // most 45, a length with its extra bits and a distance with its extra bits at most 48.
    while b.pos + FAST_IN <= in_len && op + FAST_OUT <= out_len {
        b.refill_fast();
        let mut e = lit[(b.buf as usize) & LIT_MASK];
        if e & F_SUB != 0 {
            e = sub_entry(lit, e, &mut b);
        }
        if e & F_LIT != 0 {
            b.skip(e & 0xFF);
            // SAFETY (the three stores): the loop guard gives `op + FAST_OUT <= out_len`, and
            // `FAST_OUT` is far more than 3, so `op`, `op + 1` and `op + 2` are inside `out`.
            unsafe { *out.get_unchecked_mut(op) = (e >> 16) as u8 };
            // More literals while they are at hand; anything else is looked up again after the
            // next refill.
            let e2 = lit[(b.buf as usize) & LIT_MASK];
            if e2 & F_LIT == 0 {
                op += 1;
                continue;
            }
            b.skip(e2 & 0xFF);
            unsafe { *out.get_unchecked_mut(op + 1) = (e2 >> 16) as u8 };
            let e3 = lit[(b.buf as usize) & LIT_MASK];
            if e3 & F_LIT == 0 {
                op += 2;
                continue;
            }
            b.skip(e3 & 0xFF);
            unsafe { *out.get_unchecked_mut(op + 2) = (e3 >> 16) as u8 };
            op += 3;
            continue;
        }
        if e & (F_EOB | F_BAD) != 0 {
            b.skip(e & 0xFF);
            if e & F_BAD != 0 {
                return Err(InflateError::BadSymbol);
            }
            *bits = b;
            return Ok(op);
        }

        // A length, then its distance.
        let saved = b.buf;
        b.skip(e & 0xFF);
        let length = (e >> 16) as usize + extra_value(saved, e);
        let mut d = dist[(b.buf as usize) & DIST_MASK];
        if d & F_SUB != 0 {
            d = sub_entry(dist, d, &mut b);
        }
        if d & F_BAD != 0 {
            return Err(InflateError::BadSymbol);
        }
        let saved = b.buf;
        b.skip(d & 0xFF);
        let distance = (d >> 16) as usize + extra_value(saved, d);
        if distance > op {
            return Err(InflateError::BadDistance);
        }
        copy_fast(out, op, distance, length);
        op += length;
    }

    // The slow loop: the last bytes of the input or the output, or a damaged stream.
    loop {
        if b.overrun() {
            return Err(InflateError::Truncated);
        }
        b.refill();
        let mut e = lit[(b.buf as usize) & LIT_MASK];
        if e & F_SUB != 0 {
            e = sub_entry(lit, e, &mut b);
        }
        let saved = b.buf;
        b.skip(e & 0xFF);
        if e & F_LIT != 0 {
            if op >= out_len {
                return Err(InflateError::OutputFull);
            }
            out[op] = (e >> 16) as u8;
            op += 1;
            continue;
        }
        if e & (F_EOB | F_BAD) != 0 {
            if e & F_BAD != 0 {
                return Err(InflateError::BadSymbol);
            }
            *bits = b;
            return Ok(op);
        }

        // A length, then its distance. At most 20 + 28 bits, and 56 are in the buffer.
        let length = (e >> 16) as usize + extra_value(saved, e);
        let mut d = dist[(b.buf as usize) & DIST_MASK];
        if d & F_SUB != 0 {
            d = sub_entry(dist, d, &mut b);
        }
        if d & F_BAD != 0 {
            return Err(InflateError::BadSymbol);
        }
        let saved = b.buf;
        b.skip(d & 0xFF);
        let distance = (d >> 16) as usize + extra_value(saved, d);
        if distance > op {
            return Err(InflateError::BadDistance);
        }
        if length > out_len - op {
            return Err(InflateError::OutputFull);
        }
        copy_exact(out, op, distance, length);
        op += length;
    }
}

/// A reusable DEFLATE decoder; it owns the Huffman tables, so keep one per worker thread.
pub struct Decoder {
    lit: Box<[u32; LIT_SIZE]>,
    dist: Box<[u32; DIST_SIZE]>,
    fixed_lit: Box<[u32; LIT_SIZE]>,
    fixed_dist: Box<[u32; DIST_SIZE]>,
}

fn boxed_table<const N: usize>() -> Box<[u32; N]> {
    vec![0u32; N]
        .into_boxed_slice()
        .try_into()
        .expect("the vector has N elements")
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// A decoder with the fixed-code tables of RFC 1951 already built.
    pub fn new() -> Self {
        let mut d = Self {
            lit: boxed_table(),
            dist: boxed_table(),
            fixed_lit: boxed_table(),
            fixed_dist: boxed_table(),
        };
        let mut lens = [8u8; MAX_LIT_SYMS];
        lens[144..256].fill(9);
        lens[256..280].fill(7);
        let lit_built = build_table(&lens, &mut d.fixed_lit[..], LIT_ROOT, lit_entry, false);
        let dist_built = build_table(
            &[5u8; MAX_DIST_SYMS],
            &mut d.fixed_dist[..],
            DIST_ROOT,
            dist_entry,
            false,
        );
        debug_assert!(lit_built.is_ok() && dist_built.is_ok());
        d
    }

    /// Decodes the raw DEFLATE stream `input` into `out` and returns the number of bytes
    /// written. `out` should have exactly the size the stream is expected to produce: a stream
    /// that would write more is an error, and the caller compares the returned count with the
    /// expected size. Bytes of `out` past the returned count are unspecified.
    pub fn inflate(&mut self, input: &[u8], out: &mut [u8]) -> Result<usize, InflateError> {
        let mut bits = Bits {
            input,
            pos: 0,
            buf: 0,
            cnt: 0,
        };
        let mut op = 0;
        loop {
            bits.refill();
            let last = bits.take(1) != 0;
            let kind = bits.take(2);
            if bits.overrun() {
                return Err(InflateError::Truncated);
            }
            op = match kind {
                0 => stored_block(&mut bits, out, op)?,
                1 => decode_block(&mut bits, out, op, &self.fixed_lit, &self.fixed_dist)?,
                2 => {
                    self.read_dynamic(&mut bits)?;
                    decode_block(&mut bits, out, op, &self.lit, &self.dist)?
                }
                _ => return Err(InflateError::BadHeader),
            };
            if bits.overrun() {
                return Err(InflateError::Truncated);
            }
            if last {
                return Ok(op);
            }
        }
    }

    /// Reads the header of a dynamic block and builds its two tables.
    fn read_dynamic(&mut self, bits: &mut Bits<'_>) -> Result<(), InflateError> {
        bits.refill();
        let hlit = bits.take(5) as usize + 257;
        let hdist = bits.take(5) as usize + 1;
        let hclen = bits.take(4) as usize + 4;

        let mut pre_lens = [0u8; 19];
        for &sym in &PRE_ORDER[..hclen] {
            bits.refill();
            pre_lens[usize::from(sym)] = bits.take(3) as u8;
        }
        let mut pre = [0u32; PRE_SIZE];
        build_table(&pre_lens, &mut pre, PRE_ROOT, pre_entry, false)?;

        // The literal/length and distance code lengths form one sequence, and a repeat may run
        // from the first part into the second.
        let total = hlit + hdist;
        let mut lens = [0u8; MAX_LIT_SYMS + MAX_DIST_SYMS];
        let mut i = 0;
        while i < total {
            bits.refill();
            let e = pre[(bits.buf as usize) & PRE_MASK];
            bits.skip(e & 0xFF);
            let sym = (e >> 16) as usize;
            if sym < 16 {
                lens[i] = sym as u8;
                i += 1;
                continue;
            }
            let (repeat, value) = match sym {
                16 => {
                    if i == 0 {
                        return Err(InflateError::BadHeader);
                    }
                    (3 + bits.take(2) as usize, lens[i - 1])
                }
                17 => (3 + bits.take(3) as usize, 0),
                _ => (11 + bits.take(7) as usize, 0),
            };
            if i + repeat > total {
                return Err(InflateError::BadHeader);
            }
            lens[i..i + repeat].fill(value);
            i += repeat;
        }
        if bits.overrun() {
            return Err(InflateError::Truncated);
        }
        // The end-of-block code must exist.
        if lens[256] == 0 {
            return Err(InflateError::BadHeader);
        }
        build_table(&lens[..hlit], &mut self.lit[..], LIT_ROOT, lit_entry, true)?;
        build_table(
            &lens[hlit..total],
            &mut self.dist[..],
            DIST_ROOT,
            dist_entry,
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use libdeflater::{CompressionLvl, Compressor, Decompressor};

    use super::*;

    /// xorshift64: deterministic test data without another dependency.
    struct Rng(u64);

    impl Rng {
        fn step(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.step() % n
        }
    }

    const SIZES: [usize; 11] = [1, 2, 3, 15, 16, 17, 257, 258, 4096, 65_537, 300_000];

    /// Kinds of data: zeros, random bytes, words, short periods, skewed bytes, a 32 KiB period
    /// (matches at the largest distance).
    fn sample(kind: usize, n: usize, rng: &mut Rng) -> Vec<u8> {
        match kind {
            0 => vec![0; n],
            1 => (0..n).map(|_| rng.step() as u8).collect(),
            2 => {
                let mut words = Vec::new();
                for _ in 0..50 {
                    let len = 2 + rng.below(8) as usize;
                    let word: Vec<u8> = (0..len).map(|_| b'a' + rng.below(26) as u8).collect();
                    words.push(word);
                }
                let mut v = Vec::with_capacity(n + 16);
                while v.len() < n {
                    v.extend_from_slice(&words[rng.below(50) as usize]);
                    v.push(b' ');
                }
                v.truncate(n);
                v
            }
            3 => {
                let period = 1 + rng.below(40) as usize;
                let unit: Vec<u8> = (0..period).map(|_| rng.step() as u8).collect();
                (0..n).map(|i| unit[i % period]).collect()
            }
            4 => (0..n)
                .map(|_| (rng.step().trailing_zeros() * 29 % 256) as u8)
                .collect(),
            _ => {
                let unit: Vec<u8> = (0..32_768).map(|_| rng.step() as u8).collect();
                (0..n).map(|i| unit[i % 32_768]).collect()
            }
        }
    }

    fn libdeflate(data: &[u8], level: i32) -> Vec<u8> {
        let mut c = Compressor::new(CompressionLvl::new(level).unwrap());
        let mut out = vec![0u8; c.deflate_compress_bound(data.len())];
        let n = c.deflate_compress(data, &mut out).unwrap();
        out.truncate(n);
        out
    }

    /// zlib-rs through flate2; every chunk is followed by a sync flush, which ends a block.
    fn zlib(data: &[u8], level: Compression, chunk: usize) -> Vec<u8> {
        let mut e = DeflateEncoder::new(Vec::new(), level);
        for piece in data.chunks(chunk) {
            e.write_all(piece).unwrap();
            e.flush().unwrap();
        }
        e.finish().unwrap()
    }

    /// True when zlib (through flate2) accepts the stream: the reference for what the RFC allows.
    fn zlib_accepts(packed: &[u8]) -> bool {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::DeflateDecoder::new(packed)
            .read_to_end(&mut out)
            .is_ok()
    }

    fn check(dec: &mut Decoder, data: &[u8], packed: &[u8]) {
        for spare in [0usize, 1, 40] {
            let mut out = vec![0xAAu8; data.len() + spare];
            let n = dec.inflate(packed, &mut out).expect("a valid stream");
            assert_eq!(n, data.len());
            assert!(&out[..n] == data, "output differs (spare {spare})");
        }
    }

    #[test]
    fn known_streams() {
        let mut dec = Decoder::new();
        assert_eq!(dec.inflate(&[0x03, 0x00], &mut []), Ok(0));
        let mut out = [0u8; 1];
        assert_eq!(dec.inflate(&[0x4b, 0x04, 0x00], &mut out), Ok(1));
        assert_eq!(&out, b"a");
        let mut out = [0u8; 2];
        let stored = [0x01, 0x02, 0x00, 0xfd, 0xff, b'h', b'i'];
        assert_eq!(dec.inflate(&stored, &mut out), Ok(2));
        assert_eq!(&out, b"hi");
    }

    #[test]
    fn invalid_streams_are_rejected() {
        let mut dec = Decoder::new();
        let mut out = [0u8; 16];
        // Block type 3.
        assert_eq!(
            dec.inflate(&[0x07, 0x00], &mut out),
            Err(InflateError::BadHeader)
        );
        // Stored block whose NLEN is not the complement of LEN.
        let stored = [0x01, 0x05, 0x00, 0x00, 0x00];
        assert_eq!(dec.inflate(&stored, &mut out), Err(InflateError::BadStored));
        // A fixed block that starts with a match.
        assert_eq!(
            dec.inflate(&[0x03, 0x02, 0x00], &mut out),
            Err(InflateError::BadDistance)
        );
        assert_eq!(dec.inflate(&[], &mut out), Err(InflateError::Truncated));
    }

    #[test]
    fn libdeflate_streams_round_trip() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut dec = Decoder::new();
        for kind in 0..6 {
            for n in SIZES {
                let data = sample(kind, n, &mut rng);
                for level in [1, 6, 12] {
                    check(&mut dec, &data, &libdeflate(&data, level));
                }
            }
        }
    }

    #[test]
    fn zlib_streams_round_trip() {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        let mut dec = Decoder::new();
        let levels = [
            Compression::none(),
            Compression::fast(),
            Compression::default(),
            Compression::best(),
        ];
        for kind in 0..6 {
            for n in SIZES {
                let data = sample(kind, n, &mut rng);
                for level in levels {
                    for chunk in [usize::MAX, 777] {
                        check(&mut dec, &data, &zlib(&data, level, chunk));
                    }
                }
            }
        }
        // An empty stream is one empty final block.
        check(&mut dec, &[], &zlib(&[], Compression::default(), 1));
    }

    /// Every output size from 0 to 700 bytes: the fast loop starts when 288 bytes are left, so
    /// this walks its entry and exit past every possible position in a block.
    #[test]
    fn small_sizes_round_trip() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let mut dec = Decoder::new();
        for kind in [1, 2, 3, 4] {
            for n in 0..=700 {
                let data = sample(kind, n, &mut rng);
                check(&mut dec, &data, &libdeflate(&data, 6));
                check(
                    &mut dec,
                    &data,
                    &zlib(&data, Compression::fast(), usize::MAX),
                );
            }
        }
    }

    #[test]
    fn truncated_streams_are_errors() {
        let mut rng = Rng(7);
        let mut dec = Decoder::new();
        for kind in 0..6 {
            let data = sample(kind, 20_000, &mut rng);
            let streams = [
                libdeflate(&data, 6),
                zlib(&data, Compression::default(), 4000),
                zlib(&data, Compression::none(), usize::MAX),
            ];
            for packed in streams {
                for cut in (0..packed.len()).step_by(97).chain([packed.len() - 1]) {
                    let mut out = vec![0u8; data.len()];
                    let res = dec.inflate(&packed[..cut], &mut out);
                    assert!(
                        res.is_err(),
                        "kind {kind}, cut at {cut} of {}",
                        packed.len()
                    );
                }
            }
        }
    }

    #[test]
    fn output_that_is_too_small_is_an_error() {
        let mut rng = Rng(11);
        let mut dec = Decoder::new();
        for kind in 0..6 {
            let data = sample(kind, 20_000, &mut rng);
            for packed in [
                libdeflate(&data, 6),
                zlib(&data, Compression::none(), usize::MAX),
            ] {
                for short in [0, 1, data.len() / 2, data.len() - 1] {
                    let mut out = vec![0u8; data.len()];
                    let res = dec.inflate(&packed, &mut out[..short]);
                    assert_eq!(
                        res,
                        Err(InflateError::OutputFull),
                        "kind {kind}, short {short}"
                    );
                }
            }
        }
    }

    #[test]
    fn damaged_streams_do_not_panic() {
        let mut rng = Rng(99);
        let mut dec = Decoder::new();
        for kind in [1, 2, 3] {
            let data = sample(kind, 30_000, &mut rng);
            for good in [libdeflate(&data, 6), zlib(&data, Compression::fast(), 9000)] {
                for _ in 0..600 {
                    let mut bad = good.clone();
                    for _ in 0..1 + rng.below(4) {
                        let at = rng.below(bad.len() as u64) as usize;
                        bad[at] ^= 1 << rng.below(8);
                    }
                    let mut out = vec![0u8; data.len()];
                    let _ = dec.inflate(&bad, &mut out);
                }
            }
        }
    }

    /// Wherever libdeflate accepts a damaged stream, this decoder must accept it too, with the
    /// same bytes, unless it is `BadSymbol` and zlib rejects the stream as well: libdeflate is
    /// more lenient than the RFC there (see the module docs). Where libdeflate rejects the stream,
    /// anything but a panic is fine.
    #[test]
    fn agrees_with_libdeflate_on_damaged_streams() {
        let mut rng = Rng(2024);
        let mut dec = Decoder::new();
        let mut reference = Decompressor::new();
        let mut accepted = 0;
        let mut lenient = 0;
        for kind in [1, 2, 3, 4] {
            let data = sample(kind, 6_000, &mut rng);
            for good in [
                libdeflate(&data, 6),
                libdeflate(&data, 1),
                libdeflate(&data, 12),
            ] {
                for _ in 0..500 {
                    let mut bad = good.clone();
                    let at = rng.below(bad.len() as u64) as usize;
                    bad[at] ^= 1 << rng.below(8);
                    let mut want = vec![0u8; data.len()];
                    let Ok(n) = reference.deflate_decompress(&bad, &mut want) else {
                        continue;
                    };
                    accepted += 1;
                    let mut got = vec![0u8; data.len()];
                    let result = dec.inflate(&bad, &mut got);
                    if result == Err(InflateError::BadSymbol) && !zlib_accepts(&bad) {
                        lenient += 1;
                        continue;
                    }
                    assert_eq!(result, Ok(n), "kind {kind}, byte {at}");
                    assert!(
                        got[..n] == want[..n],
                        "kind {kind}, byte {at}: bytes differ"
                    );
                }
            }
        }
        // Most flips are rejected by both; a few leave a valid stream, and those were compared.
        assert!(accepted < 6000);
        // The leniency of libdeflate shows up in a small share of the accepted streams only.
        assert!(lenient * 20 <= accepted, "{lenient} of {accepted} streams");
    }

    /// Symbols the RFC forbids are errors. libdeflate decodes the length symbols 286 and 287 as
    /// 258 and accepts these two streams; zlib and this decoder do not.
    #[test]
    fn invalid_symbols_are_rejected() {
        let mut dec = Decoder::new();
        let mut out = [0u8; 300];
        // A fixed block: the literal 'a', a length symbol, distance 1, end of block.
        let length_symbols: [[u8; 4]; 2] = [[0x4B, 0x1C, 0x03, 0x00], [0x4B, 0x1C, 0x07, 0x00]];
        for stream in length_symbols {
            assert_eq!(dec.inflate(&stream, &mut out), Err(InflateError::BadSymbol));
        }
        // 'a', length 3, the distance symbol 30 (which libdeflate rejects too), end of block.
        let distance_symbol: [u8; 6] = [0x4B, 0x04, 0x3E, 0x00, 0x00, 0x00];
        assert_eq!(
            dec.inflate(&distance_symbol, &mut out),
            Err(InflateError::BadSymbol)
        );
    }

    #[test]
    fn random_bytes_do_not_panic() {
        let mut rng = Rng(5);
        let mut dec = Decoder::new();
        for _ in 0..20_000 {
            let len = rng.below(400) as usize;
            let junk: Vec<u8> = (0..len).map(|_| rng.step() as u8).collect();
            let mut out = vec![0u8; rng.below(4000) as usize];
            let _ = dec.inflate(&junk, &mut out);
        }
    }
}
