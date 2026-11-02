"""Line-for-line Python mirror of kiln-rs/src/wav.rs (decode side). Pure python, no ctypes."""
import struct

WAV_FRAMES = 1 << 18
WAV_HEAD = 1 << 16
NEED, NO = "need", "no"


class KilnError(Exception):
    pass


def wav_spec(fid):
    if fid in (9, 10, 11):
        return ("shuf", {9: 4, 10: 3, 11: 8}[fid], 0, False, False, 0, 0)
    if 1 <= fid <= 8:
        g = fid - 1
        return ("w16", 2, 1 + (g & 1), (g & 2) != 0, False, 128 if (g & 4) else 0, 0)
    if 16 <= fid <= 62 or 64 <= fid <= 110:
        base = 16 if fid < 64 else 64
        g = fid - base
        order, ms, zz, wc = g & 3, ((g >> 2) & 1) != 0, ((g >> 3) & 1) != 0, g >> 4
        if order > 2 or wc > 2:
            return None
        if base == 16:
            return ("int", (3, 4, 1)[wc], order, ms, zz, 0, 0)
        return ("fint", 4, order, ms, zz, 0, (7, 15, 23)[wc])
    return None


def wav_layout(head, size):
    if len(head) < 12:
        return NEED if size >= 12 else NO
    if head[:4] != b"RIFF" or head[8:12] != b"WAVE":
        return NO
    pos, fmt = 12, None
    while True:
        if pos > WAV_HEAD:
            return NO
        if len(head) < pos + 8:
            return NEED if (size >= pos + 8 and len(head) < WAV_HEAD) else NO
        cid = head[pos:pos + 4]
        csz = struct.unpack_from("<I", head, pos + 4)[0]
        if cid == b"fmt ":
            if csz < 16:
                return NO
            need = pos + 8 + (26 if csz >= 40 else 16)
            if len(head) < need:
                return NEED if len(head) < WAV_HEAD else NO
            tag = struct.unpack_from("<H", head, pos + 8)[0]
            ch = struct.unpack_from("<H", head, pos + 10)[0]
            align = struct.unpack_from("<H", head, pos + 20)[0]
            bits = struct.unpack_from("<H", head, pos + 22)[0]
            if tag == 0xFFFE:
                tag = struct.unpack_from("<H", head, pos + 8 + 24)[0] if csz >= 40 else 1
            if tag != 1 and tag != 3:
                return NO
            ok = (tag == 1) if bits in (8, 16) else bits in (24, 32, 64)
            if not ok or not (1 <= ch <= 8) or align != (bits // 8) * ch:
                return NO
            fmt = (ch, align, bits // 8, tag == 3)
        elif cid == b"data":
            if fmt is None:
                return NO
            start = pos + 8
            if size < start:
                return NO
            dlen = min(csz, size - start)
            region = (dlen // fmt[1]) * fmt[1]
            return (start, region, fmt[0], fmt[1], fmt[2], fmt[3])
        pos += 8 + csz + (csz & 1)


def wmask(w):
    return 0xFFFFFFFF if w >= 4 else (1 << (8 * w)) - 1


def wav16_dec(inp, n, c, order, ms, bias, out):
    t = [0] * n
    L = [0] * n if ms else []
    for ch in range(c):
        lo = inp[2 * ch * n:2 * ch * n + n]
        hi = inp[2 * ch * n + n:2 * ch * n + 2 * n]
        for f in range(n):
            t[f] = ((lo[f] | (hi[f] << 8)) - bias) & 0xFFFF
        for _ in range(order):
            for f in range(1, n):
                t[f] = (t[f] + t[f - 1]) & 0xFFFF
        if ms and ch == 0:
            L[:] = t
        if ms and ch == 1:
            for f in range(n):
                t[f] = (t[f] + L[f]) & 0xFFFF
        for f in range(n):
            d = 2 * (f * c + ch)
            out[d] = t[f] & 0xFF
            out[d + 1] = (t[f] >> 8) & 0xFF


def shuf_dec(inp, n, w, c, out):
    for f in range(n):
        for ch in range(c):
            d = (f * c + ch) * w
            for k in range(w):
                out[d + k] = inp[(ch * w + k) * n + f]


def int_dec(inp, n, w, c, order, ms, zz, out):
    mask = wmask(w)
    t = [0] * n
    L = [0] * n if ms else []
    for ch in range(c):
        for f in range(n):
            v = 0
            for k in range(w):
                v |= inp[(ch * w + k) * n + f] << (8 * k)
            t[f] = v
        if zz:
            for f in range(n):
                z = t[f]
                t[f] = ((z >> 1) ^ (mask if (z & 1) else 0)) & mask
        for _ in range(order):
            for f in range(1, n):
                t[f] = (t[f] + t[f - 1]) & mask
        if ms and ch == 0:
            L[:] = t
        if ms and ch == 1:
            for f in range(n):
                t[f] = (t[f] + L[f]) & mask
        for f in range(n):
            d = (f * c + ch) * w
            for k in range(w):
                out[d + k] = (t[f] >> (8 * k)) & 0xFF


def i2f(inp, m, s, kw, out):
    sc = float(1 << s)
    sh = 32 - 8 * kw
    for i in range(m):
        v = 0
        for j in range(kw):
            v |= inp[i * kw + j] << (8 * j)
        k = ((v << sh) & 0xFFFFFFFF)
        if k & 0x80000000:
            k -= 1 << 32
        k >>= sh                                  # arithmetic shift
        x = struct.unpack("<f", struct.pack("<f", k / sc))[0]   # double -> float32 rounding
        out[i * 4:i * 4 + 4] = struct.pack("<f", x)


class Wav:
    def __init__(self, fid, size):
        sp = wav_spec(fid)
        if sp is None:
            raise KilnError("archive uses features from a newer kiln")
        self.kind, self.w, self.order, self.ms, self.zz, self.bias, self.s = sp
        self.kw = (self.s + 1 + 7) // 8 if self.kind == "fint" else self.w
        self.size = size
        self.buf = bytearray()
        self.state = 0
        self.left = 0
        self.start = 0
        self.c = 0
        self.align = 1

    def feed(self, data):
        self.buf += data
        out = bytearray()
        if self.state == 0:
            lay = wav_layout(bytes(self.buf[:WAV_HEAD]), self.size)
            if lay == NEED:
                return b""
            if lay == NO:
                raise KilnError("corrupt data: bad WAV header for the filter")
            start, left, c, align, bps, _fl = lay
            if c < 1 or (self.ms and c != 2) or bps != self.w:
                raise KilnError("corrupt data: bad WAV header for the filter")
            self.start, self.left, self.c, self.align = start, left, c, align
            out += self.buf[:self.start]
            del self.buf[:self.start]
            self.state = 1
        step = WAV_FRAMES * self.align
        while self.state == 1:
            if self.left == 0:
                self.state = 2
                break
            take = min(step, self.left)
            if len(self.buf) < take:
                break
            chunk = bytes(self.buf[:take])
            del self.buf[:take]
            out += self.dec(chunk)
            self.left -= take
        if self.state == 2:
            out += self.buf
            self.buf.clear()
        return bytes(out)

    def finish(self):
        out = bytearray(self.feed(b""))
        if self.buf:
            out += self.buf
            self.buf.clear()
        return bytes(out)

    def dec(self, chunk):
        c = self.c
        out = bytearray(len(chunk))
        if self.kind == "int":
            int_dec(chunk, len(chunk) // self.align, self.w, c, self.order, self.ms, self.zz, out)
        elif self.kind == "fint":
            n = len(chunk) // self.align
            ints = bytearray(self.kw * c * n)
            int_dec(chunk[:self.kw * c * n], n, self.kw, c, self.order, self.ms, self.zz, ints)
            i2f(ints, n * c, self.s, self.kw, out)
        elif self.kind == "shuf":
            shuf_dec(chunk, len(chunk) // self.align, self.w, c, out)
        else:
            wav16_dec(chunk, len(chunk) // (2 * c), c, self.order, self.ms, self.bias, out)
        return bytes(out)
