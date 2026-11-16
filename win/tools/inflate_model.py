#!/usr/bin/env python3
"""Line-for-line model of win/src/inflate.rs, to check the algorithm against zlib.

There is no Rust compiler where this was written, so the decoder's logic is validated here
instead: every function below mirrors the Rust function of the same name, statement by
statement (fast loop, slow loop, table builder, bit reader, match copies). Python integers do not
wrap, so the u64/u32 behaviour of the Rust code is spelled out with masks, and every place where
Rust would panic (index out of bounds, `cnt -= n` below zero, a shift by 64 or more, a copy past
the end of the buffer) raises `RustPanic`. The tests treat a `RustPanic` as a failure, whatever
the input.

Run:  python win/tools/inflate_model.py          (full, about a minute)
      python win/tools/inflate_model.py --quick  (a few seconds)

When inflate.rs changes, change this file the same way and run it again.
"""

import random
import sys
import zlib

M64 = (1 << 64) - 1


class InflateError(Exception):
    """The Rust `InflateError`; `kind` is the variant name."""

    def __init__(self, kind):
        super().__init__(kind)
        self.kind = kind


class RustPanic(AssertionError):
    """Something that would panic (or silently wrap) in the Rust code."""


COV = {}


def cov(name):
    COV[name] = COV.get(name, 0) + 1


def ensure(cond, what):
    if not cond:
        raise RustPanic(what)


LIT_ROOT, DIST_ROOT, PRE_ROOT = 11, 8, 7
LIT_MASK = (1 << LIT_ROOT) - 1
DIST_MASK = (1 << DIST_ROOT) - 1
PRE_MASK = (1 << PRE_ROOT) - 1
MAX_LIT_SYMS = 288
MAX_DIST_SYMS = 32
LIT_SIZE = (1 << LIT_ROOT) + (MAX_LIT_SYMS // 2) * (1 << (15 - LIT_ROOT))
DIST_SIZE = (1 << DIST_ROOT) + (MAX_DIST_SYMS // 2) * (1 << (15 - DIST_ROOT))
PRE_SIZE = 1 << PRE_ROOT
F_LIT, F_SUB, F_EOB, F_BAD = 1 << 31, 1 << 14, 1 << 13, 1 << 12
FAST_IN = 8
FAST_OUT = 288

LEN_BASE = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
            115, 131, 163, 195, 227, 258]
LEN_EXTRA = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0]
DIST_BASE = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
             1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577]
DIST_EXTRA = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11,
              12, 12, 13, 13]
PRE_ORDER = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15]


def lit_entry(sym, ln):
    head = (ln << 8) | ln
    if sym <= 255:
        return F_LIT | (sym << 16) | head
    if sym == 256:
        return F_EOB | head
    if sym <= 285:
        i = sym - 257
        return (LEN_BASE[i] << 16) | (ln << 8) | (ln + LEN_EXTRA[i])
    return F_BAD | head


def dist_entry(sym, ln):
    if sym < 30:
        return (DIST_BASE[sym] << 16) | (ln << 8) | (ln + DIST_EXTRA[sym])
    return F_BAD | (ln << 8) | ln


def pre_entry(sym, ln):
    return (sym << 16) | (ln << 8) | ln


def reverse_bits32(x):
    return int(format(x, "032b")[::-1], 2)


def build_table(lens, table, root, make, allow_incomplete):
    ensure(len(lens) <= MAX_LIT_SYMS, "build_table: too many symbols")
    count = [0] * 16
    for ln in lens:
        ensure(ln < 16, "count[len]: index out of bounds")
        count[ln] += 1
    count[0] = 0

    left = 1
    for c in count[1:]:
        left = (left << 1) - c
        if left < 0:
            raise InflateError("BadHeader")
    used = sum(count[1:])
    trivial = used == 0 or (used == 1 and count[1] == 1)
    if left > 0 and not (allow_incomplete and trivial):
        raise InflateError("BadHeader")

    offs = [0] * 16
    acc = 0
    for ln in range(1, 16):
        offs[ln] = acc
        acc += count[ln]
    sorted_ = [0] * MAX_LIT_SYMS
    for sym, ln in enumerate(lens):
        if ln != 0:
            sorted_[offs[ln]] = sym
            offs[ln] += 1

    rev = [0] * MAX_LIT_SYMS
    code = 0
    prev = 0
    for k in range(used):
        ln = lens[sorted_[k]]
        if k > 0 and ln > prev:
            code <<= ln - prev
        prev = ln
        ensure(code < (1 << ln), "canonical code does not fit its length")
        rev[k] = (reverse_bits32(code) >> (32 - ln)) & 0xFFFF
        code += 1

    root_size = 1 << root
    for i in range(root_size):
        table[i] = F_BAD
    next_sub = root_size
    k = 0
    while k < used:
        sym = sorted_[k]
        ln = lens[sym]
        if ln <= root:
            entry = make(sym, ln)
            i = rev[k]
            while i < root_size:
                table[i] = entry
                i += 1 << ln
            k += 1
            continue
        prefix = rev[k] & (root_size - 1)
        end = k + 1
        while end < used and (rev[end] & (root_size - 1)) == prefix:
            end += 1
        sub_bits = lens[sorted_[end - 1]] - root
        sub_size = 1 << sub_bits
        if next_sub + sub_size > len(table):
            raise InflateError("BadHeader")
        ensure(next_sub < (1 << 15), "subtable index does not fit the entry")
        table[prefix] = F_SUB | (next_sub << 16) | (sub_bits << 8) | root
        for s, r in zip(sorted_[k:end], rev[k:end]):
            sub_len = lens[s] - root
            entry = make(s, sub_len)
            i = r >> root
            while i < sub_size:
                table[next_sub + i] = entry
                i += 1 << sub_len
        next_sub += sub_size
        k = end


class Bits:
    """The bit reader; `buf` is a u64, `cnt` a u32."""

    def __init__(self, data):
        self.input = data
        self.pos = 0
        self.buf = 0
        self.cnt = 0

    def copy(self):
        b = Bits(self.input)
        b.pos, b.buf, b.cnt = self.pos, self.buf, self.cnt
        return b

    def assign(self, other):
        self.pos, self.buf, self.cnt = other.pos, other.buf, other.cnt

    def refill(self):
        if self.pos + 8 <= len(self.input):
            self.refill_fast()
        else:
            self.refill_tail()

    def refill_fast(self):
        ensure(self.pos + 8 <= len(self.input), "refill_fast: slice out of range")
        ensure(self.cnt < 64, "refill: shift by 64 or more")
        word = int.from_bytes(self.input[self.pos:self.pos + 8], "little")
        self.buf |= (word << self.cnt) & M64
        self.pos += (63 - self.cnt) >> 3
        self.cnt |= 56

    def refill_tail(self):
        while self.cnt <= 55:
            byte = self.input[self.pos] if self.pos < len(self.input) else 0
            self.buf |= (byte << self.cnt) & M64
            self.pos += 1
            self.cnt += 8

    def take(self, n):
        ensure(n <= 32, "take: more than 32 bits")
        ensure(n <= self.cnt, "take: more bits than the buffer holds")
        v = self.buf & ((1 << n) - 1)
        self.skip(n)
        return v

    def skip(self, n):
        ensure(n <= self.cnt, "skip: cnt would go below zero")
        self.buf >>= n
        self.cnt -= n

    def overrun(self):
        return self.pos * 8 > len(self.input) * 8 + self.cnt


def sub_entry(table, e, b):
    cov("subtable lookup (lit)" if len(table) == LIT_SIZE else "subtable lookup (dist)")
    b.skip(e & 0xFF)
    index = (e >> 16) + (b.buf & ((1 << ((e >> 8) & 15)) - 1))
    ensure(index < len(table), "sub_entry: index out of bounds")
    return table[index]


def extra_value(saved, e):
    ensure((e & 0xFF) < 64 and ((e >> 8) & 15) < 64, "extra_value: shift out of range")
    return (saved & ((1 << (e & 0xFF)) - 1)) >> ((e >> 8) & 15)


def copy_within(out, src_start, src_end, dest):
    n = src_end - src_start
    ensure(0 <= src_start <= src_end <= len(out), "copy_within: source out of range")
    ensure(dest + n <= len(out), "copy_within: destination out of range")
    out[dest:dest + n] = out[src_start:src_end]


def fill(out, start, end, byte):
    ensure(start <= end <= len(out), "fill: range out of bounds")
    out[start:end] = bytes([byte]) * (end - start)


def stored_block(bits, out, op):
    cov("stored block")
    bits.skip(bits.cnt & 7)
    bits.refill()
    length = bits.take(16)
    nlen = bits.take(16)
    if length != (~nlen & 0xFFFF):
        raise InflateError("BadStored")
    ensure(bits.pos >= (bits.cnt >> 3), "stored_block: pos underflow")
    bits.pos -= bits.cnt >> 3
    bits.buf = 0
    bits.cnt = 0
    end = bits.pos + length
    if end > len(bits.input):
        raise InflateError("Truncated")
    if length > len(out) - op:
        raise InflateError("OutputFull")
    out[op:op + length] = bits.input[bits.pos:end]
    bits.pos = end
    return op + length


def copy_overlapping(out, op, distance, length):
    src = op - distance
    cov("copy_overlapping")
    done = 0
    while done < length:
        n = min(length - done, distance + done)
        copy_within(out, src, src + n, op + done)
        done += n


def copy_fast(out, op, distance, length):
    src = op - distance
    if distance >= 16:
        cov("fast match, distance >= 16")
        i = 0
        while i < length:
            copy_within(out, src + i, src + i + 16, op + i)
            i += 16
    elif distance >= 8:
        cov("fast match, distance 8..15")
        i = 0
        while i < length:
            copy_within(out, src + i, src + i + 8, op + i)
            i += 8
    elif distance == 1:
        cov("fast match, distance 1")
        ensure(src < len(out), "copy_fast: out[src]")
        fill(out, op, op + length, out[src])
    else:
        cov("fast match, distance 2..7")
        copy_overlapping(out, op, distance, length)


def copy_exact(out, op, distance, length):
    src = op - distance
    if distance >= 16 and len(out) - op >= length + 16:
        cov("exact match, 16-byte pieces")
        i = 0
        while i < length:
            copy_within(out, src + i, src + i + 16, op + i)
            i += 16
    elif distance >= length:
        cov("exact match, non-overlapping")
        copy_within(out, src, src + length, op)
    elif distance == 1:
        cov("exact match, distance 1")
        fill(out, op, op + length, out[src])
    else:
        cov("exact match, overlapping")
        copy_overlapping(out, op, distance, length)


def at(table, i):
    ensure(0 <= i < len(table), "table index out of bounds")
    return table[i]


def put(out, i, byte):
    ensure(0 <= i < len(out), "out[i]: index out of bounds")
    out[i] = byte


def decode_block(bits, out, op, lit, dist):
    b = bits.copy()
    out_len = len(out)
    in_len = len(b.input)

    # The fast loop.
    while b.pos + FAST_IN <= in_len and op + FAST_OUT <= out_len:
        b.refill_fast()
        e = at(lit, b.buf & LIT_MASK)
        if e & F_SUB:
            e = sub_entry(lit, e, b)
        if e & F_LIT:
            b.skip(e & 0xFF)
            put(out, op, (e >> 16) & 0xFF)
            e2 = at(lit, b.buf & LIT_MASK)
            if not (e2 & F_LIT):
                cov("fast loop: 1 literal")
                op += 1
                continue
            b.skip(e2 & 0xFF)
            put(out, op + 1, (e2 >> 16) & 0xFF)
            e3 = at(lit, b.buf & LIT_MASK)
            if not (e3 & F_LIT):
                cov("fast loop: 2 literals")
                op += 2
                continue
            b.skip(e3 & 0xFF)
            put(out, op + 2, (e3 >> 16) & 0xFF)
            cov("fast loop: 3 literals")
            op += 3
            continue
        if e & (F_EOB | F_BAD):
            b.skip(e & 0xFF)
            if e & F_BAD:
                cov("fast loop: bad symbol")
                raise InflateError("BadSymbol")
            cov("fast loop: end of block")
            bits.assign(b)
            return op

        saved = b.buf
        b.skip(e & 0xFF)
        length = (e >> 16) + extra_value(saved, e)
        d = at(dist, b.buf & DIST_MASK)
        if d & F_SUB:
            d = sub_entry(dist, d, b)
        if d & F_BAD:
            cov("fast loop: bad distance symbol")
            raise InflateError("BadSymbol")
        saved = b.buf
        b.skip(d & 0xFF)
        distance = (d >> 16) + extra_value(saved, d)
        if distance > op:
            cov("fast loop: distance too far")
            raise InflateError("BadDistance")
        cov("fast loop: match")
        copy_fast(out, op, distance, length)
        op += length

    # The slow loop.
    while True:
        if b.overrun():
            cov("slow loop: truncated")
            raise InflateError("Truncated")
        b.refill()
        e = at(lit, b.buf & LIT_MASK)
        if e & F_SUB:
            e = sub_entry(lit, e, b)
        saved = b.buf
        b.skip(e & 0xFF)
        if e & F_LIT:
            if op >= out_len:
                cov("slow loop: output full (literal)")
                raise InflateError("OutputFull")
            cov("slow loop: literal")
            put(out, op, (e >> 16) & 0xFF)
            op += 1
            continue
        if e & (F_EOB | F_BAD):
            if e & F_BAD:
                cov("slow loop: bad symbol")
                raise InflateError("BadSymbol")
            cov("slow loop: end of block")
            bits.assign(b)
            return op

        length = (e >> 16) + extra_value(saved, e)
        d = at(dist, b.buf & DIST_MASK)
        if d & F_SUB:
            d = sub_entry(dist, d, b)
        if d & F_BAD:
            cov("slow loop: bad distance symbol")
            raise InflateError("BadSymbol")
        saved = b.buf
        b.skip(d & 0xFF)
        distance = (d >> 16) + extra_value(saved, d)
        if distance > op:
            cov("slow loop: distance too far")
            raise InflateError("BadDistance")
        if length > out_len - op:
            cov("slow loop: output full (match)")
            raise InflateError("OutputFull")
        cov("slow loop: match")
        copy_exact(out, op, distance, length)
        op += length


class Decoder:
    def __init__(self):
        # Zero-filled like `vec![0u32; N]`: stale or unwritten entries must never be reachable.
        self.lit = [0] * LIT_SIZE
        self.dist = [0] * DIST_SIZE
        self.fixed_lit = [0] * LIT_SIZE
        self.fixed_dist = [0] * DIST_SIZE
        lens = [8] * MAX_LIT_SYMS
        for i in range(144, 256):
            lens[i] = 9
        for i in range(256, 280):
            lens[i] = 7
        build_table(lens, self.fixed_lit, LIT_ROOT, lit_entry, False)
        build_table([5] * MAX_DIST_SYMS, self.fixed_dist, DIST_ROOT, dist_entry, False)

    def inflate(self, data, out):
        bits = Bits(data)
        op = 0
        while True:
            bits.refill()
            last = bits.take(1) != 0
            kind = bits.take(2)
            if bits.overrun():
                raise InflateError("Truncated")
            if kind == 0:
                op = stored_block(bits, out, op)
            elif kind == 1:
                cov("fixed block")
                op = decode_block(bits, out, op, self.fixed_lit, self.fixed_dist)
            elif kind == 2:
                cov("dynamic block")
                self.read_dynamic(bits)
                op = decode_block(bits, out, op, self.lit, self.dist)
            else:
                raise InflateError("BadHeader")
            if bits.overrun():
                raise InflateError("Truncated")
            if last:
                return op

    def read_dynamic(self, bits):
        bits.refill()
        hlit = bits.take(5) + 257
        hdist = bits.take(5) + 1
        hclen = bits.take(4) + 4

        pre_lens = [0] * 19
        for sym in PRE_ORDER[:hclen]:
            bits.refill()
            pre_lens[sym] = bits.take(3)
        pre = [0] * PRE_SIZE
        build_table(pre_lens, pre, PRE_ROOT, pre_entry, False)

        total = hlit + hdist
        lens = [0] * (MAX_LIT_SYMS + MAX_DIST_SYMS)
        i = 0
        while i < total:
            bits.refill()
            e = at(pre, bits.buf & PRE_MASK)
            bits.skip(e & 0xFF)
            sym = e >> 16
            if sym < 16:
                lens[i] = sym
                i += 1
                continue
            if sym == 16:
                if i == 0:
                    raise InflateError("BadHeader")
                cov("code length repeat 16")
                repeat, value = 3 + bits.take(2), lens[i - 1]
            elif sym == 17:
                cov("code length repeat 17")
                repeat, value = 3 + bits.take(3), 0
            else:
                cov("code length repeat 18")
                repeat, value = 11 + bits.take(7), 0
            if i + repeat > total:
                raise InflateError("BadHeader")
            if i < hlit < i + repeat:
                cov("code length repeat crosses lit/dist boundary")
            for j in range(i, i + repeat):
                lens[j] = value
            i += repeat
        if bits.overrun():
            raise InflateError("Truncated")
        if lens[256] == 0:
            raise InflateError("BadHeader")
        build_table(lens[:hlit], self.lit, LIT_ROOT, lit_entry, True)
        build_table(lens[hlit:total], self.dist, DIST_ROOT, dist_entry, True)


def run(dec, packed, size, spare=0):
    """Decode into an output buffer of `size + spare` bytes; returns (n, bytes) or raises."""
    out = bytearray(b"\xaa" * (size + spare))
    n = dec.inflate(bytes(packed), out)
    return n, bytes(out[:n])


# --------------------------------------------------------------------------------------------
# Tests
# --------------------------------------------------------------------------------------------

def deflate(data, level=6, strategy=zlib.Z_DEFAULT_STRATEGY, chunk=None, flush=zlib.Z_SYNC_FLUSH,
            mem_level=8):
    c = zlib.compressobj(level, zlib.DEFLATED, -15, mem_level, strategy)
    if chunk is None:
        return c.compress(data) + c.flush()
    out = []
    for i in range(0, len(data), chunk):
        out.append(c.compress(data[i:i + chunk]))
        out.append(c.flush(flush))
    out.append(c.flush())
    return b"".join(out)


def sample(kind, n, rng):
    if kind == 0:
        return bytes(n)
    if kind == 1:
        return bytes(rng.getrandbits(8) for _ in range(n))
    if kind == 2:
        words = [bytes(rng.randrange(97, 123) for _ in range(rng.randrange(2, 10)))
                 for _ in range(50)]
        v = bytearray()
        while len(v) < n:
            v += rng.choice(words) + b" "
        return bytes(v[:n])
    if kind == 3:
        period = rng.randrange(1, 41)
        unit = bytes(rng.getrandbits(8) for _ in range(period))
        return bytes(unit[i % period] for i in range(n))
    if kind == 4:  # skewed bytes: geometric frequencies, long Huffman codes (subtables)
        out = bytearray()
        for _ in range(n):
            k = 0
            while k < 40 and rng.random() < 0.62:
                k += 1
            out.append((k * 29) % 256)
        return bytes(out)
    if kind == 5:  # a 32 KiB period: matches at the largest distance
        unit = bytes(rng.getrandbits(8) for _ in range(32768))
        return bytes(unit[i % 32768] for i in range(n))
    # kind 6: matches of every distance and length: copies of earlier random pieces
    out = bytearray(rng.getrandbits(8) for _ in range(min(n, 64)))
    while len(out) < n:
        if rng.random() < 0.5:
            out.append(rng.getrandbits(8))
        else:
            dist = rng.choice([1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 100, 1000, 4096,
                               20000, 32768])
            dist = min(dist, len(out))
            length = rng.choice([3, 4, 5, 8, 15, 16, 17, 31, 32, 100, 257, 258])
            for _ in range(length):
                out.append(out[-dist])
    return bytes(out[:n])


class Report:
    def __init__(self):
        self.checks = 0
        self.failures = []

    def fail(self, msg):
        self.failures.append(msg)
        print("FAIL:", msg)


def check_valid(rep, dec, data, packed, label):
    for spare in (0, 1, 40):
        rep.checks += 1
        try:
            n, got = run(dec, packed, len(data), spare)
        except InflateError as e:
            rep.fail(f"{label} spare={spare}: valid stream rejected: {e.kind}")
            return
        except RustPanic as e:
            rep.fail(f"{label} spare={spare}: PANIC {e}")
            return
        if n != len(data) or got != data:
            rep.fail(f"{label} spare={spare}: wrong output (n={n}, want {len(data)})")
            return


def test_known(rep):
    dec = Decoder()
    cases = [
        (bytes([0x03, 0x00]), b""),
        (bytes([0x4B, 0x04, 0x00]), b"a"),
        (bytes([0x01, 0x02, 0x00, 0xFD, 0xFF, ord("h"), ord("i")]), b"hi"),
    ]
    for packed, want in cases:
        rep.checks += 1
        try:
            n, got = run(dec, packed, len(want))
        except (InflateError, RustPanic) as e:
            rep.fail(f"known {packed!r}: {e}")
            continue
        if got != want:
            rep.fail(f"known {packed!r}: got {got!r}")
    errors = [
        (bytes([0x07, 0x00]), "BadHeader"),
        (bytes([0x01, 0x05, 0x00, 0x00, 0x00]), "BadStored"),
        (bytes([0x03, 0x02, 0x00]), "BadDistance"),
        (b"", "Truncated"),
        # the forbidden symbols: length 286 and 287 (libdeflate accepts these, zlib does not) and
        # distance 30; the same streams as the Rust test invalid_symbols_are_rejected
        (bytes([0x4B, 0x1C, 0x03, 0x00]), "BadSymbol"),
        (bytes([0x4B, 0x1C, 0x07, 0x00]), "BadSymbol"),
        (bytes([0x4B, 0x04, 0x3E, 0x00, 0x00, 0x00]), "BadSymbol"),
    ]
    for packed, kind in errors:
        rep.checks += 1
        try:
            run(dec, packed, 16)
            rep.fail(f"known error {packed!r}: accepted")
        except InflateError as e:
            if e.kind != kind:
                rep.fail(f"known error {packed!r}: {e.kind}, want {kind}")
        except RustPanic as e:
            rep.fail(f"known error {packed!r}: PANIC {e}")


def test_round_trip(rep, quick):
    rng = random.Random(1)
    dec = Decoder()
    sizes = [1, 2, 3, 15, 16, 17, 257, 258, 4096, 20000] if quick else \
        [1, 2, 3, 15, 16, 17, 257, 258, 4096, 65537, 120000]
    levels = [0, 1, 6, 9]
    strategies = [zlib.Z_DEFAULT_STRATEGY, zlib.Z_FILTERED, zlib.Z_HUFFMAN_ONLY, zlib.Z_RLE,
                  zlib.Z_FIXED]
    for kind in range(7):
        for n in sizes:
            data = sample(kind, n, rng)
            for level in levels:
                for strategy in strategies:
                    if level == 0 and strategy != zlib.Z_DEFAULT_STRATEGY:
                        continue
                    if quick and strategy in (zlib.Z_FILTERED, zlib.Z_RLE) and level != 6:
                        continue
                    packed = deflate(data, level, strategy)
                    check_valid(rep, dec, data, packed, f"kind={kind} n={n} L{level} S{strategy}")
            # Block boundaries mid-stream: sync and full flushes end a block with an empty stored one.
            for flush in (zlib.Z_SYNC_FLUSH, zlib.Z_FULL_FLUSH):
                packed = deflate(data, 6, chunk=777, flush=flush)
                check_valid(rep, dec, data, packed, f"kind={kind} n={n} chunked flush={flush}")
    # An empty stream.
    check_valid(rep, dec, b"", deflate(b"", 6), "empty")
    check_valid(rep, dec, b"", deflate(b"", 0), "empty stored")


def test_small_sizes(rep, quick):
    """Every output size: the fast loop starts when 288 bytes are left."""
    rng = random.Random(2)
    dec = Decoder()
    top = 330 if quick else 700
    for kind in (1, 2, 3, 4, 6):
        for n in range(0, top + 1):
            data = sample(kind, n, rng)
            check_valid(rep, dec, data, deflate(data, 6), f"small kind={kind} n={n}")
            if n % 7 == 0:
                check_valid(rep, dec, data, deflate(data, 1, zlib.Z_FIXED), f"small fixed n={n}")


def test_long_codes(rep):
    """Frequencies that fall off geometrically give Huffman codes up to 15 bits: subtables."""
    rng = random.Random(3)
    dec = Decoder()
    seen_sub_lit = seen_sub_dist = False
    for ratio in (0.55, 0.62, 0.7):
        data = bytearray()
        for _ in range(60000):
            k = 0
            while k < 60 and rng.random() < ratio:
                k += 1
            data.append((k * 37) % 256)
        # Distances of every size too: copies at random far offsets.
        for _ in range(300):
            at_ = rng.randrange(0, len(data) - 300)
            data += data[at_:at_ + rng.randrange(3, 258)]
        data = bytes(data)
        packed = deflate(data, 9)
        check_valid(rep, dec, data, packed, f"long codes ratio={ratio}")
        # Look at what the decoder built for the dynamic block(s) to be sure subtables occurred.
        if any(e & F_SUB for e in dec.lit):
            seen_sub_lit = True
        if any(e & F_SUB for e in dec.dist):
            seen_sub_dist = True
    if not seen_sub_lit:
        rep.fail("no literal/length subtable was ever built: the long-code test is too weak")
    if not seen_sub_dist:
        print("note: no distance subtable was built by the long-code test")


def test_truncation(rep, quick):
    rng = random.Random(4)
    dec = Decoder()
    for kind in (0, 1, 2, 4, 6):
        data = sample(kind, 3000 if quick else 12000, rng)
        for packed in (deflate(data, 6), deflate(data, 0), deflate(data, 6, chunk=500)):
            step = 1 if len(packed) < 400 else 53
            cuts = list(range(0, len(packed), step)) + [len(packed) - 1]
            for cut in cuts:
                rep.checks += 1
                try:
                    run(dec, packed[:cut], len(data))
                    rep.fail(f"truncation kind={kind} cut={cut}/{len(packed)}: accepted")
                except InflateError:
                    pass
                except RustPanic as e:
                    rep.fail(f"truncation kind={kind} cut={cut}: PANIC {e}")


def test_output_too_small(rep, quick):
    rng = random.Random(5)
    dec = Decoder()
    for kind in (0, 1, 2, 3, 6):
        data = sample(kind, 4000, rng)
        for packed in (deflate(data, 6), deflate(data, 0), deflate(data, 1, zlib.Z_FIXED)):
            for short in (0, 1, 2, 287, 288, 289, len(data) // 2, len(data) - 1):
                rep.checks += 1
                try:
                    run(dec, packed, short)
                    rep.fail(f"too small kind={kind} short={short}: accepted")
                except InflateError as e:
                    if e.kind != "OutputFull":
                        rep.fail(f"too small kind={kind} short={short}: {e.kind}")
                except RustPanic as e:
                    rep.fail(f"too small kind={kind} short={short}: PANIC {e}")


def zlib_reference(packed, size):
    """What zlib makes of a stream: (n, bytes), or None when it rejects it."""
    d = zlib.decompressobj(-15)
    try:
        out = d.decompress(packed, size + 1)
        if not d.eof:
            return None
    except zlib.error:
        return None
    if len(out) > size:
        return None
    return len(out), out


def test_damaged(rep, quick):
    """A flipped bit: never a panic; wherever zlib accepts the stream, so does the model."""
    rng = random.Random(6)
    dec = Decoder()
    rounds = 150 if quick else 500
    stats = {"zlib_accepts": 0, "model_only": 0, "both_reject": 0}
    for kind in (1, 2, 3, 4, 6):
        data = sample(kind, 6000, rng)
        for good in (deflate(data, 6), deflate(data, 1), deflate(data, 9),
                     deflate(data, 6, chunk=1500)):
            for _ in range(rounds):
                bad = bytearray(good)
                for _ in range(1 + rng.randrange(3)):
                    bad[rng.randrange(len(bad))] ^= 1 << rng.randrange(8)
                bad = bytes(bad)
                ref = zlib_reference(bad, len(data))
                rep.checks += 1
                try:
                    n, got = run(dec, bad, len(data))
                    mine = (n, got)
                except InflateError:
                    mine = None
                except RustPanic as e:
                    rep.fail(f"damaged kind={kind}: PANIC {e}")
                    continue
                if ref is not None:
                    stats["zlib_accepts"] += 1
                    if mine != ref:
                        rep.fail(f"damaged kind={kind}: zlib accepts ({ref[0]} bytes) but the "
                                 f"model {'rejects' if mine is None else 'differs'}")
                elif mine is not None:
                    stats["model_only"] += 1
                else:
                    stats["both_reject"] += 1
    print("damaged streams:", stats)



# --------------------------------------------------------------------------------------------
# Constructed streams: Huffman structures that zlib's compressor never produces
# --------------------------------------------------------------------------------------------

class BitWriter:
    """Writes a DEFLATE bit stream: fields least significant bit first, Huffman codes from the
    most significant bit."""

    def __init__(self):
        self.value = 0
        self.nbits = 0

    def put(self, value, nbits):
        assert 0 <= value < (1 << nbits) or nbits == 0
        self.value |= value << self.nbits
        self.nbits += nbits

    def put_code(self, code, length):
        for i in range(length - 1, -1, -1):
            self.put((code >> i) & 1, 1)

    def align(self):
        self.nbits = (self.nbits + 7) & ~7

    def to_bytes(self):
        return self.value.to_bytes((self.nbits + 7) // 8, "little")


def canonical_codes(lens):
    """RFC 1951 section 3.2.2: {symbol: (code, length)}."""
    maxlen = max(lens) if lens else 0
    bl_count = [0] * (maxlen + 2)
    for ln in lens:
        if ln:
            bl_count[ln] += 1
    next_code = [0] * (maxlen + 2)
    code = 0
    for bits in range(1, maxlen + 1):
        code = (code + bl_count[bits - 1]) << 1
        next_code[bits] = code
    res = {}
    for sym, ln in enumerate(lens):
        if ln:
            res[sym] = (next_code[ln], ln)
            next_code[ln] += 1
    return res


def even_lengths(k):
    """Lengths of a complete code over k >= 2 symbols, as even as possible."""
    fl = k.bit_length() - 1
    short = (1 << (fl + 1)) - k
    return [fl] * short + [fl + 1] * (k - short)


def random_complete_lengths(alphabet, n_used, maxlen, rng, skew, must_use=()):
    """A random complete prefix code on `n_used` of the `alphabet` symbols, depths <= maxlen.
    `skew` is the chance of splitting the deepest leaf: high values give chain-like trees."""
    assert 2 <= n_used <= (1 << maxlen) and n_used <= alphabet
    rest = [x for x in range(alphabet) if x not in must_use]
    syms = list(must_use) + rng.sample(rest, n_used - len(must_use))
    depths = [1, 1]
    while len(depths) < n_used:
        open_ = [i for i, d in enumerate(depths) if d < maxlen]
        if rng.random() < skew:
            i = max(open_, key=lambda j: depths[j])
        else:
            i = rng.choice(open_)
        depths[i] += 1
        depths.append(depths[i])
    rng.shuffle(depths)
    lens = [0] * alphabet
    for sym, d in zip(syms, depths):
        lens[sym] = d
    return lens


def rle_code_lengths(seq, rng, use_rle):
    """The code length sequence as (symbol, extra value, extra bits) triples."""
    out = []
    i = 0
    while i < len(seq):
        v = seq[i]
        run = 1
        while i + run < len(seq) and seq[i + run] == v:
            run += 1
        if use_rle and v == 0 and run >= 3 and rng.random() < 0.9:
            if run >= 11:
                n = min(run, 138)
                out.append((18, n - 11, 7))
            else:
                n = run
                out.append((17, n - 3, 3))
            i += n
        elif use_rle and v != 0 and run >= 4 and rng.random() < 0.9:
            out.append((v, 0, 0))
            n = min(run - 1, 6)
            out.append((16, n - 3, 2))
            i += 1 + n
        else:
            out.append((v, 0, 0))
            i += 1
    return out


def length_symbol(length):
    for i in range(28, -1, -1):
        if length >= LEN_BASE[i]:
            if i == 28:
                assert length == 258
            return i, length - LEN_BASE[i]
    raise AssertionError


def distance_symbol(distance):
    for i in range(29, -1, -1):
        if distance >= DIST_BASE[i]:
            return i, distance - DIST_BASE[i]
    raise AssertionError


def write_dynamic_block(bw, lit_lens, dist_lens, tokens, final, rng, use_rle=True, pre_lens=None,
                        hlit=None, hdist=None):
    """One dynamic block. `tokens` is a list of ("lit", b), ("match", length, distance) and the
    block ends with the end-of-block code."""
    hlit = hlit or len(lit_lens)
    hdist = hdist or len(dist_lens)
    lit_lens = list(lit_lens) + [0] * (hlit - len(lit_lens))
    dist_lens = list(dist_lens) + [0] * (hdist - len(dist_lens))
    seq = lit_lens + dist_lens
    triples = rle_code_lengths(seq, rng, use_rle)
    if pre_lens is None:
        used = sorted({t[0] for t in triples})
        if len(used) < 2:
            used = sorted(set(used) | {0 if used[0] != 0 else 1})
        lens19 = [0] * 19
        for sym, ln in zip(used, even_lengths(len(used))):
            lens19[sym] = ln
        pre_lens = lens19
    pre_codes = canonical_codes(pre_lens)
    hclen = 19
    while hclen > 4 and pre_lens[PRE_ORDER[hclen - 1]] == 0:
        hclen -= 1

    bw.put(1 if final else 0, 1)
    bw.put(2, 2)
    bw.put(hlit - 257, 5)
    bw.put(hdist - 1, 5)
    bw.put(hclen - 4, 4)
    for sym in PRE_ORDER[:hclen]:
        bw.put(pre_lens[sym], 3)
    for sym, extra, nbits in triples:
        code, ln = pre_codes[sym]
        bw.put_code(code, ln)
        bw.put(extra, nbits)

    lit_codes = canonical_codes(lit_lens)
    dist_codes = canonical_codes(dist_lens)
    for tok in tokens:
        if tok[0] == "lit":
            code, ln = lit_codes[tok[1]]
            bw.put_code(code, ln)
        elif tok[0] == "lenonly":      # a length code and its extra bits, no distance after it
            li, lextra = length_symbol(tok[1])
            code, ln = lit_codes[257 + li]
            bw.put_code(code, ln)
            bw.put(lextra, LEN_EXTRA[li])
        elif tok[0] == "raw":          # bits that are not a code
            bw.put(tok[1], tok[2])
        else:
            _, length, distance = tok
            li, lextra = length_symbol(length)
            code, ln = lit_codes[257 + li]
            bw.put_code(code, ln)
            bw.put(lextra, LEN_EXTRA[li])
            di, dextra = distance_symbol(distance)
            code, ln = dist_codes[di]
            bw.put_code(code, ln)
            bw.put(dextra, DIST_EXTRA[di])
    code, ln = lit_codes[256]
    bw.put_code(code, ln)


def random_tokens(lit_lens, dist_lens, history, rng, count):
    """Random tokens that use only symbols the codes have; returns (tokens, new output bytes)."""
    lits = [b for b in range(256) if lit_lens[b]]
    lens_ok = [length for length in range(3, 259)
               if len(lit_lens) > 257 + length_symbol(length)[0]
               and lit_lens[257 + length_symbol(length)[0]]]
    dists_ok = [i for i in range(min(30, len(dist_lens))) if dist_lens[i]]
    out = bytearray(history)
    tokens = []
    for _ in range(count):
        can_match = lens_ok and dists_ok and len(out) > 0
        if can_match and (not lits or rng.random() < 0.45):
            length = rng.choice(lens_ok)
            di = rng.choice(dists_ok)
            lo = DIST_BASE[di]
            hi = min(DIST_BASE[di] + (1 << DIST_EXTRA[di]) - 1, len(out))
            if lo > hi:
                continue
            distance = rng.randrange(lo, hi + 1)
            tokens.append(("match", length, distance))
            for _ in range(length):
                out.append(out[-distance])
        elif lits:
            b = rng.choice(lits)
            tokens.append(("lit", b))
            out.append(b)
    return tokens, bytes(out[len(history):])


def test_constructed(rep, quick):
    rng = random.Random(8)
    dec = Decoder()
    rounds = 120 if quick else 600
    for r in range(rounds):
        bw = BitWriter()
        expected = bytearray()
        nblocks = rng.choice([1, 1, 2, 3])
        for bi in range(nblocks):
            maxlen = rng.choice([9, 11, 12, 13, 14, 15, 15, 15])
            n_lit = rng.randrange(2, min(286, 1 << maxlen) + 1) if rng.random() < 0.7 \
                else rng.randrange(2, 40)
            lit_lens = random_complete_lengths(286, n_lit, maxlen, rng,
                                               rng.choice([0.0, 0.5, 0.9, 1.0]),
                                               must_use=(256,))
            mode = rng.random()
            if mode < 0.12:
                dist_lens = [0] * rng.randrange(1, 31)             # no distance codes at all
            elif mode < 0.25:
                dist_lens = [0] * 30
                dist_lens[rng.randrange(30)] = 1                    # one code of length 1
            else:
                dmax = rng.choice([7, 8, 9, 10, 12, 15])
                n_dist = rng.randrange(2, min(30, 1 << dmax) + 1)
                dist_lens = random_complete_lengths(30, n_dist, dmax, rng,
                                                    rng.choice([0.0, 0.5, 1.0]))
            # hdist covers the highest used symbol; hlit too
            hl = max(257, max(i for i, ln in enumerate(lit_lens) if ln) + 1)
            hd = max(1, max([i for i, ln in enumerate(dist_lens) if ln] or [0]) + 1)
            tokens, new = random_tokens(lit_lens, dist_lens, expected, rng,
                                        rng.choice([5, 60, 400, 1500]))
            final = bi == nblocks - 1
            write_dynamic_block(bw, lit_lens[:hl], dist_lens[:hd], tokens, final, rng,
                                use_rle=rng.random() < 0.8)
            expected += new
            if not final and rng.random() < 0.3:
                # a stored block in between, to cross block kinds
                bw.put(0, 1)
                bw.put(0, 2)
                bw.align()
                piece = bytes(rng.getrandbits(8) for _ in range(rng.randrange(0, 50)))
                bw.put(len(piece), 16)
                bw.put(len(piece) ^ 0xFFFF, 16)
                for byte in piece:
                    bw.put(byte, 8)
                expected += piece
        packed = bw.to_bytes()
        ref = zlib_reference(packed, len(expected))
        if ref is None or ref[1] != bytes(expected):
            rep.fail(f"constructed stream {r}: zlib disagrees with the generator "
                     f"(the test stream itself is wrong)")
            continue
        check_valid(rep, dec, bytes(expected), packed, f"constructed {r}")


def test_invalid_structures(rep):
    """Streams that zlib rejects. The model must reject them too (any InflateError)."""
    rng = random.Random(9)
    dec = Decoder()
    cases = []

    base_lit = [0] * 257
    base_lit[97] = 1
    base_lit[256] = 1                      # 'a' and end of block: a complete code
    dist_none = [0]

    # 1. over-subscribed literal code: three codes of length 1
    over = [0] * 257
    over[97] = over[98] = over[256] = 1
    cases.append(("over-subscribed lit code", block_raw_lens(over, dist_none, rng)))
    # 2. incomplete literal code (lengths 1 and 2)
    inc = [0] * 257
    inc[97] = 1
    inc[256] = 2
    cases.append(("incomplete lit code", block_raw_lens(inc, dist_none, rng, body=[97, 256])))
    # 3. no end-of-block code
    noeob = [0] * 257
    noeob[97] = noeob[98] = 1
    cases.append(("no end-of-block code", block_raw_lens(noeob, dist_none, rng)))
    # 4. incomplete distance code (lengths 1 and 2)
    d_inc = [1, 2]
    cases.append(("incomplete distance code", block_raw_lens(base_lit, d_inc, rng, body=[97, 256])))
    # 5. over-subscribed distance code
    d_over = [1, 1, 1]
    cases.append(("over-subscribed distance code", block_raw_lens(base_lit, d_over, rng)))
    # 6. a precode with a single symbol (incomplete)
    pre = [0] * 19
    pre[1] = 1
    cases.append(("incomplete precode", block_raw_lens(base_lit, dist_none, rng, pre_lens=pre)))
    # 7. an over-subscribed precode
    pre = [0] * 19
    pre[0] = pre[1] = pre[2] = 1
    cases.append(("over-subscribed precode", block_raw_lens(base_lit, dist_none, rng, pre_lens=pre)))
    # 8. a match that reaches before the start of the output
    lit_m = [0] * 258
    lit_m[97] = lit_m[256] = lit_m[257] = 2
    lit_m[98] = 2
    d_one = [1, 1]
    cases.append(("match before start",
                  block_raw(lit_m, d_one, [("lit", 97), ("match", 3, 2)], rng)))
    # 9. a match with no distance code defined: the distance bits decode to nothing
    cases.append(("match without distance code",
                  block_raw(lit_m, [0], [("lit", 97), ("lenonly", 3), ("raw", 0, 8)], rng)))
    # 10. repeat code 16 at the very start
    bw = BitWriter()
    bw.put(1, 1); bw.put(2, 2); bw.put(0, 5); bw.put(0, 5); bw.put(0, 4)
    for sym in PRE_ORDER[:4]:
        bw.put(2 if sym in (16, 17, 18, 0) else 0, 3)   # 16,17,18,0: even complete 4-code
    bw.put_code(0, 2)                                    # first symbol in canonical order
    cases.append(("repeat 16 first", bw.to_bytes() + bytes(8)))
    # 11. a block type 3 and an empty input
    cases.append(("block type 3", bytes([0x07, 0, 0, 0])))
    cases.append(("stored length mismatch", bytes([0x01, 0x05, 0x00, 0x00, 0x00, 1, 2, 3, 4, 5])))
    # 12. fixed-block symbols that must not occur: length symbol 286, distance symbol 30
    bw = BitWriter()
    bw.put(1, 1); bw.put(1, 2)
    # literal 'a' (8 bits 0x30+97), then length symbol 286 (code 0xC0 + (286-280), 8 bits)
    bw.put_code(0x30 + 97, 8)
    bw.put_code(0xC0 + 6, 8)
    cases.append(("fixed: length symbol 286", bw.to_bytes() + bytes(8)))
    bw = BitWriter()
    bw.put(1, 1); bw.put(1, 2)
    bw.put_code(0x30 + 97, 8)
    bw.put_code(1, 7)                                    # length symbol 257 (3 bytes)
    bw.put_code(30, 5)                                   # distance symbol 30
    cases.append(("fixed: distance symbol 30", bw.to_bytes() + bytes(8)))

    for label, packed in cases:
        ref = zlib_reference(packed, 1000)
        rep.checks += 1
        if ref is not None:
            rep.fail(f"invalid case '{label}': zlib accepts it, the test case is wrong")
            continue
        try:
            run(dec, packed, 1000)
            rep.fail(f"invalid case '{label}': the model accepts a stream zlib rejects")
        except InflateError:
            pass
        except RustPanic as e:
            rep.fail(f"invalid case '{label}': PANIC {e}")


def block_raw_lens(lit_lens, dist_lens, rng, pre_lens=None, body=None):
    bw = BitWriter()
    # No tokens: the end-of-block code may not even exist; write what the header says and a
    # few zero bytes for the decoder to chew on.
    hl = len(lit_lens)
    hd = len(dist_lens)
    seq = list(lit_lens) + list(dist_lens)
    triples = [(v, 0, 0) for v in seq]
    if pre_lens is None:
        used = sorted({t[0] for t in triples})
        if len(used) < 2:
            used = sorted(set(used) | {0 if used[0] != 0 else 1})
        pre_lens = [0] * 19
        for sym, ln in zip(used, even_lengths(len(used))):
            pre_lens[sym] = ln
    pre_codes = canonical_codes(pre_lens)
    hclen = 19
    while hclen > 4 and pre_lens[PRE_ORDER[hclen - 1]] == 0:
        hclen -= 1
    bw.put(1, 1); bw.put(2, 2)
    bw.put(hl - 257, 5); bw.put(hd - 1, 5); bw.put(hclen - 4, 4)
    for sym in PRE_ORDER[:hclen]:
        bw.put(pre_lens[sym], 3)
    for sym, _, _ in triples:
        if sym in pre_codes:
            code, ln = pre_codes[sym]
            bw.put_code(code, ln)
        else:
            bw.put(0, 3)
    if body:
        # A real body after the header: a decoder that wrongly accepts the header decodes this
        # cleanly, instead of failing later for some other reason and hiding the mistake.
        lit_codes = canonical_codes(lit_lens)
        for sym in body:
            code, ln = lit_codes[sym]
            bw.put_code(code, ln)
    return bw.to_bytes() + bytes(16)


def block_raw(lit_lens, dist_lens, tokens, rng):
    bw = BitWriter()
    hl = len(lit_lens)
    write_dynamic_block(bw, lit_lens, dist_lens, tokens, True, rng, False, None, hl, len(dist_lens))
    return bw.to_bytes()


def test_random_junk(rep, quick):
    rng = random.Random(7)
    dec = Decoder()
    for _ in range(3000 if quick else 20000):
        junk = bytes(rng.getrandbits(8) for _ in range(rng.randrange(0, 400)))
        out = bytearray(rng.randrange(0, 4000))
        rep.checks += 1
        try:
            dec.inflate(junk, out)
        except InflateError:
            pass
        except RustPanic as e:
            rep.fail(f"junk: PANIC {e}")


def main():
    quick = "--quick" in sys.argv
    rep = Report()
    steps = [
        ("known streams", lambda: test_known(rep)),
        ("round trip vs zlib", lambda: test_round_trip(rep, quick)),
        ("every small size", lambda: test_small_sizes(rep, quick)),
        ("long codes (subtables)", lambda: test_long_codes(rep)),
        ("truncated streams", lambda: test_truncation(rep, quick)),
        ("output too small", lambda: test_output_too_small(rep, quick)),
        ("damaged streams", lambda: test_damaged(rep, quick)),
        ("constructed streams", lambda: test_constructed(rep, quick)),
        ("invalid structures", lambda: test_invalid_structures(rep)),
        ("random bytes", lambda: test_random_junk(rep, quick)),
    ]
    for name, fn in steps:
        before = rep.checks
        fn()
        print(f"{name}: {rep.checks - before} checks, {len(rep.failures)} failures so far")
    print("\ncoverage of the model's branches:")
    expected_cov = [
        "fast loop: 1 literal", "fast loop: 2 literals", "fast loop: 3 literals",
        "fast loop: match", "fast loop: end of block", "fast loop: bad symbol",
        "fast loop: bad distance symbol", "fast loop: distance too far",
        "fast match, distance >= 16", "fast match, distance 8..15", "fast match, distance 1",
        "fast match, distance 2..7", "slow loop: literal", "slow loop: match",
        "slow loop: end of block", "slow loop: truncated", "slow loop: output full (literal)",
        "slow loop: output full (match)", "slow loop: bad symbol",
        "slow loop: bad distance symbol", "slow loop: distance too far",
        "exact match, 16-byte pieces", "exact match, non-overlapping",
        "exact match, distance 1", "exact match, overlapping", "copy_overlapping",
        "subtable lookup (lit)", "subtable lookup (dist)", "stored block", "fixed block",
        "dynamic block", "code length repeat 16", "code length repeat 17",
        "code length repeat 18", "code length repeat crosses lit/dist boundary",
    ]
    for name in expected_cov:
        hits = COV.get(name, 0)
        print(f"  {hits:8d}  {name}")
        if hits == 0:
            rep.fail(f"branch never reached by the tests: {name}")
    print(f"\n{rep.checks} checks, {len(rep.failures)} failures")
    return 1 if rep.failures else 0


if __name__ == "__main__":
    sys.exit(main())
