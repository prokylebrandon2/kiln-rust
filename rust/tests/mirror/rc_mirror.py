"""Line-for-line Python mirror of kiln-rs/src/rc.rs (itself a port of kz_rc_dec).
Same structure, same indexing, same wrapping. If this matches the real engine on the corpus,
the Rust logic is right (modulo compile errors). Keep it in sync with rc.rs."""
RC_TOP = 1 << 24
RC_LC, RC_PB, RC_LPMAX, RC_NS = 3, 2, 2, 32
RC_RA, RC_RB, RC_K, RC_NMAX = 4, 7, 1.6, 255
LIT_SIZE, MIX_SHIFT = 0x300, 10
M32 = 0xFFFFFFFF

def mix_sq(d):
    T = [1,2,3,6,10,16,27,45,73,120,194,310,488,747,1101,1546,2047,2549,2994,3348,3607,3785,3901,3975,4022,4050,4068,4079,4085,4089,4092,4093,4094]
    if d > 2047: return 4095
    if d < -2047: return 0
    w = d & 127
    i = (d >> 7) + 16
    return (T[i] * (128 - w) + T[i + 1] * w + 64) >> 7

def _tables():
    ra, rb = [0] * 256, [0] * 256
    for n in range(256):
        r = 1.0 / (n + RC_K)
        fa, fb = 1.0 / (1 << RC_RA), 1.0 / (1 << RC_RB)
        ra[n] = int((r if r > fa else fa) * 65536.0 + 0.5)
        rb[n] = int((r if r > fb else fb) * 65536.0 + 0.5)
    mixstr, mixsq = [0] * 4096, [0] * 4096
    pi = 0
    for x in range(-2047, 2048):
        v = mix_sq(x)
        for i in range(pi, v + 1): mixstr[i] = x
        pi = v + 1
    for i in range(pi, 4096): mixstr[i] = 2047
    for i in range(4096): mixsq[i] = mix_sq(i - 2048)
    return ra, rb, mixstr, mixsq
RA, RB, MIXSTR, MIXSQ = _tables()

class S:                      # an array of Rcp {a,b,n}
    def __init__(self, n): self.a = [32768] * n; self.b = [32768] * n; self.n = [0] * n
    def p(self, i): return (self.a[i] + self.b[i]) >> 5
    def up(self, i, bit):
        n = self.n[i]
        if n < RC_NMAX: self.n[i] = n + 1
        ra, rb = RA[n], RB[n]
        a, b = self.a[i], self.b[i]
        if bit:
            self.a[i] = (a - ((a * ra) >> 16)) & 0xFFFF
            self.b[i] = (b - ((b * rb) >> 16)) & 0xFFFF
        else:
            self.a[i] = (a + (((65535 - a) * ra) >> 16)) & 0xFFFF
            self.b[i] = (b + (((65535 - b) * rb) >> 16)) & 0xFFFF

class Lenc:
    def __init__(self): self.ch = S(2); self.low = S(8); self.mid = S(8); self.high = S(256)

class Mix:
    def __init__(self):
        self.t1 = S(256 * 256); self.t2 = S(4096 * 256)
        self.w = [0] * 768; self.wm = [0] * 768
        for c in range(256):
            self.w[c*3:c*3+3] = [26000, 20000, 20000]
            self.wm[c*3:c*3+3] = [36000, 14000, 14000]

class Model:
    def __init__(self):
        self.ism, self.isrep, self.isrep0, self.isrep1, self.isrep2, self.isrep0l = (S(RC_NS) for _ in range(6))
        self.lit = S(((1 << RC_LC) << RC_LPMAX) * LIT_SIZE)
        self.ml, self.rl = Lenc(), Lenc()
        self.slot = [S(32) for _ in range(4)]
        self.ext = [S(8) for _ in range(27)]
        self.align = S(16)
        self.lpm = 0; self.mix = None

class Dec:
    def __init__(self, pl): self.pl, self.p, self.range, self.code = pl, 1, 0xFFFFFFFF, 0
    def byte(self):
        b = self.pl[self.p] if self.p < len(self.pl) else 0
        self.p += 1
        return b
    def norm(self):
        while self.range < RC_TOP:
            self.range = (self.range << 8) & M32
            self.code = ((self.code << 8) | self.byte()) & M32
    def bit(self, s, i):
        bound = (self.range >> 12) * s.p(i)
        if self.code < bound: self.range = bound; bit = 0
        else: self.code -= bound; self.range -= bound; bit = 1
        s.up(i, bit); self.norm()
        return bit
    def bitp(self, p):
        bound = (self.range >> 12) * p
        if self.code < bound: self.range = bound; bit = 0
        else: self.code -= bound; self.range -= bound; bit = 1
        self.norm()
        return bit
    def direct(self, n):
        v = 0
        for _ in range(n):
            self.range >>= 1
            b = 1 if self.code >= self.range else 0
            if b: self.code -= self.range
            v = (v << 1) | b
            self.norm()
        return v
    def tree(self, s, base, nbits):
        m = 1
        for _ in range(nbits): m = (m << 1) | self.bit(s, base + m)
        return m - (1 << nbits)
    def rtree(self, s, base, nbits):
        m = 1; v = 0
        for i in range(nbits):
            b = self.bit(s, base + m); m = (m << 1) | b; v |= b << i
        return v
    def len(self, l):
        if self.bit(l.ch, 0) == 0: return self.tree(l.low, 0, 3)
        if self.bit(l.ch, 1) == 0: return 8 + self.tree(l.mid, 0, 3)
        return 16 + self.tree(l.high, 0, 8)

def mix_h2(prev2, prev): return (((prev2 * 0x9E3779B1) & M32) ^ ((prev * 0x85EBCA6B) & M32)) >> 20

def mix_pred(ps, w):          # ps = [(S,i)*3], w = 3 weights
    s = [MIXSTR[a.p(i)] for a, i in ps]
    dot = s[0] * w[0] + s[1] * w[1] + s[2] * w[2]
    d = dot >> 16
    d = 2047 if d > 2047 else -2047 if d < -2047 else d
    p = MIXSQ[d + 2048]
    p = 1 if p < 1 else 4095 if p > 4095 else p
    return p, s

def i32(v): v &= M32; return v - (1 << 32) if v >> 31 else v

def mix_upd(w, wi, s, p, bit):
    err = (0 if bit else 4095) - p
    for k in range(3): w[wi + k] = i32(w[wi + k] + ((s[k] * err) >> MIX_SHIFT))

def mixlit(d, x, lp, lbase, prev, prev2):
    b1 = prev * 256; b2 = mix_h2(prev2, prev) * 256; c0 = 1
    for _ in range(8):
        wi = c0 * 3
        p, s = mix_pred([(lp, lbase + c0), (x.t1, b1 + c0), (x.t2, b2 + c0)], x.w[wi:wi + 3])
        bit = d.bitp(p)
        mix_upd(x.w, wi, s, p, bit)
        lp.up(lbase + c0, bit); x.t1.up(b1 + c0, bit); x.t2.up(b2 + c0, bit)
        c0 = (c0 << 1) | bit
    return c0 & 0xFF

def mixlitm(d, x, lp, lbase, prev, prev2, mb):
    b1 = prev * 256; b2 = mix_h2(prev2, prev) * 256; offs = 0x100; sym = 1
    while sym < 0x100:
        mb <<= 1
        mbit = mb & offs
        wi = sym * 3; ai = lbase + offs + mbit + sym
        p, s = mix_pred([(lp, ai), (x.t1, b1 + sym), (x.t2, b2 + sym)], x.wm[wi:wi + 3])
        bit = d.bitp(p)
        mix_upd(x.wm, wi, s, p, bit)
        lp.up(ai, bit); x.t1.up(b1 + sym, bit); x.t2.up(b2 + sym, bit)
        sym = (sym << 1) | bit
        offs = offs & mbit if bit else offs & ~mbit
    return sym & 0xFF

MAX_BLOCK_RAW = 524288 * 258

def decode_block(payload, raw, out):
    """out: bytearray with history; appends raw bytes. Raises ValueError(code) like Err(code)."""
    op0 = len(out); target = op0 + raw; plen = len(payload)
    if raw > MAX_BLOCK_RAW: raise ValueError(-4)
    if plen < 5 or (payload[0] & 3) > RC_LPMAX or (payload[0] >> 2) > 1: raise ValueError(-1)
    out.extend(bytes(raw))
    try:
        _inner(payload, out, op0, target)
    except ValueError:
        del out[op0:]
        raise

def _inner(pl, out, op0, target):
    plen = len(pl); m = Model()
    m.lpm = (1 << (pl[0] & 3)) - 1
    if pl[0] >> 2: m.mix = Mix()
    d = Dec(pl)
    for _ in range(4): d.code = ((d.code << 8) | d.byte()) & M32
    st = 0; rep = [1, 1, 1, 1]; op = op0
    while op < target:
        if d.p > plen + 8: raise ValueError(-2)
        sp = st + 8 * ((op - op0) & ((1 << RC_PB) - 1))
        if d.bit(m.ism, sp) == 0:
            prev = out[op - 1] if op > op0 else 0
            ctx = (prev >> (8 - RC_LC)) + 8 * (((op - op0)) & m.lpm)
            base = ctx * LIT_SIZE
            prev2 = out[op - 2] if op > op0 + 1 else 0
            if (st & 1) and m.mix is not None:
                mb = out[op - rep[0]]
                out[op] = mixlitm(d, m.mix, m.lit, base, prev, prev2, mb); op += 1
            elif st & 1:
                mb = out[op - rep[0]]; offs = 0x100; sym = 1
                while sym < 0x100:
                    mb <<= 1
                    mbit = mb & offs
                    bit = d.bit(m.lit, base + offs + mbit + sym)
                    sym = (sym << 1) | bit
                    offs = offs & mbit if bit else offs & ~mbit
                out[op] = sym & 0xFF; op += 1
            elif m.mix is not None:
                out[op] = mixlit(d, m.mix, m.lit, base, prev, prev2); op += 1
            else:
                out[op] = d.tree(m.lit, base, 8); op += 1
            st = (st << 1) & 7
            continue
        if d.bit(m.isrep, sp):
            if d.bit(m.isrep0, sp) == 0:
                dist = rep[0]
                length = 1 if d.bit(m.isrep0l, sp) == 0 else d.len(m.rl) + 2
            else:
                ri = 1 if d.bit(m.isrep1, sp) == 0 else 2 + d.bit(m.isrep2, sp)
                dist = rep[ri]
                k = ri
                while k > 0: rep[k] = rep[k - 1]; k -= 1
                rep[0] = dist
                length = d.len(m.rl) + 2
        else:
            v = d.len(m.ml); length = v + 2
            sc = 0 if v < 1 else v - 1 if v < 4 else 3
            slot = d.tree(m.slot[sc], 0, 5)
            if slot > 26: raise ValueError(-3)
            dd = slot
            if slot >= 2:
                eb = slot - 1; nm = eb if eb < 3 else 3; r = eb - nm
                x = d.tree(m.ext[slot], 0, nm) << r
                if r >= 4:
                    x |= d.direct(r - 4) << 4
                    x |= d.rtree(m.align, 0, 4)
                elif r > 0:
                    x |= d.direct(r)
                dd = (1 << eb) + x
            dist = dd + 1
            rep[3], rep[2], rep[1], rep[0] = rep[2], rep[1], rep[0], dist
        if dist > op: raise ValueError(-3)
        if op + length > target: raise ValueError(-4)
        if dist >= length: out[op:op + length] = out[op - dist:op - dist + length]
        else:
            for k in range(length): out[op + k] = out[op + k - dist]
        op += length
        st = ((st << 1) | 1) & 7
    if d.p > plen: raise ValueError(-5)
