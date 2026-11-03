"""Line-for-line Python mirror of kiln-rs/src/bcj.rs + container.rs (plain archives)."""
import struct, zlib, sys, os, hashlib, unicodedata
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.exceptions import InvalidTag
sys.path.insert(0, os.path.dirname(__file__))
import rc_mirror
import wav_mirror

class KilnError(Exception): pass
MAGIC = b"KLNA"; FOOTER_LEN = 20; WINDOW = (1 << 26) - 1; MP3_FID = 120; BCJ_X86, BCJ_ARM64 = 121, 122
BCJ_BLOCK = 1 << 20
NEWER = "archive uses features from a newer kiln"
M32 = 0xFFFFFFFF
ERR = {-1: "corrupt data: bad code", -2: "corrupt data: block ran past its end", -3: "corrupt data: bad distance",
       -4: "corrupt data: block too long", -5: "corrupt data: block overread", -6: "corrupt data: block size mismatch",
       -7: "corrupt data: out of memory"}

# ------------------------------------------------------------ bcj.rs
def bcj_decode(fid, b, pos0):
    n = len(b)
    if fid == BCJ_X86:
        i = 0
        while i + 4 < n:
            if (b[i] & 0xFE) == 0xE8 and (b[i + 4] == 0 or b[i + 4] == 0xFF):
                v = b[i + 1] | (b[i + 2] << 8) | (b[i + 3] << 16) | ((b[i + 4] & 1) << 24)
                pc = (pos0 + i + 5) & M32
                v = ((v - pc) & M32) & 0x01FFFFFF
                b[i + 1] = v & 255; b[i + 2] = (v >> 8) & 255; b[i + 3] = (v >> 16) & 255
                b[i + 4] = ((0 - ((v >> 24) & 1)) & M32) & 255
            i += 1
    elif fid == BCJ_ARM64:
        i = 0
        while i + 4 <= n:
            x = int.from_bytes(b[i:i + 4], "little")
            pc = ((pos0 + i) >> 2) & M32
            if (x & 0xFC000000) == 0x94000000:
                v = ((x - pc) & M32) & 0x03FFFFFF
                x = 0x94000000 | v
            elif (x & 0x9F000000) == 0x90000000:
                src = ((x >> 29) & 3) | ((x >> 3) & 0x001FFFFC)
                if (((src + 0x00020000) & M32) & 0x001C0000) == 0:
                    q = ((pos0 + i) >> 12) & M32
                    d = (src - q) & M32
                    x &= 0x9000001F
                    x |= (d & 3) << 29
                    x |= (d & 0x0003FFFC) << 3
                    x |= (((0 - (d & 0x00020000)) & M32) & 0x00E00000)
            b[i:i + 4] = (x & M32).to_bytes(4, "little")
            i += 4

class Bcj:
    def __init__(self, fid): self.fid, self.pos, self.buf = fid, 0, bytearray()
    def _run(self, n):
        blk = bytearray(self.buf[:n]); del self.buf[:n]
        bcj_decode(self.fid, blk, self.pos); self.pos += n
        return bytes(blk)
    def feed(self, chunk):
        self.buf += chunk; out = []
        while len(self.buf) >= BCJ_BLOCK: out.append(self._run(BCJ_BLOCK))
        return b"".join(out)
    def finish(self): return self._run(len(self.buf)) if self.buf else b""

# ------------------------------------------------------------ container.rs
def wav_spec_valid(fid):
    return wav_mirror.wav_spec(fid) is not None

def parse_table(raw, hdr_len, table_off):
    bad = KilnError("corrupt data: bad file table")
    p = 0
    def take(n):
        nonlocal p
        if p + n > len(raw): raise bad
        s = raw[p:p + n]; p += n; return s
    count = struct.unpack(">I", take(4))[0]
    entries = []
    for _ in range(count):
        nlen = struct.unpack(">H", take(2))[0]
        try: name = take(nlen).decode("utf-8")
        except UnicodeDecodeError: raise bad
        size = struct.unpack(">Q", take(8))[0]; crc = struct.unpack(">I", take(4))[0]
        mtime = struct.unpack(">Q", take(8))[0]; mode = struct.unpack(">H", take(2))[0]
        filt, dead = mode >> 12, False
        if filt == 15:
            v = struct.unpack(">H", take(2))[0]
            dead, filt = bool(v & 0x8000), v & 0x7FFF
            if (filt > 11 and not wav_spec_valid(filt) and filt != MP3_FID and filt != BCJ_X86 and filt != BCJ_ARM64) or (filt <= 11 and not dead):
                raise KilnError(NEWER)
        elif filt > 11: raise KilnError(NEWER)
        slen = size
        if filt == MP3_FID: slen = struct.unpack(">Q", take(8))[0]
        entries.append(dict(rel=name, size=size, crc=crc, mtime=mtime, mode=mode & 0xFFF, filt=filt, dead=dead, slen=slen))
    ck = []
    if len(raw) >= p + 4 and raw[p:p + 4] == b"CKPT":
        p += 4
        nc = struct.unpack(">I", take(4))[0]
        prev = (0, hdr_len - 1, 0)
        for _ in range(nc):
            a = struct.unpack(">I", take(4))[0]; b = struct.unpack(">Q", take(8))[0]; k = struct.unpack(">Q", take(8))[0]
            if not (a > 0 and a < count and b > prev[1] and k >= prev[2] and (table_off is None or b < table_off)):
                raise KilnError("corrupt data: bad checkpoint list")
            ck.append((a, b, k)); prev = (a, b, k)
    return entries, ck

def find_ckpt(ck, idx, hdr_len):
    best = (0, hdr_len, 0)
    for c in ck:
        if c[0] <= idx: best = c
    return best

def read_upto(f, n): return f.read(n)

class PlainSource:
    def __init__(self, f): self.f = f
    def read_record(self):
        t = read_upto(self.f, 1)
        if not t: raise KilnError("archive is truncated (stream has no end)")
        if t[0] == 2: return t
        hdr = read_upto(self.f, 8)
        if len(hdr) < 8: raise KilnError("archive is truncated (cut inside a block header)")
        comp_len = struct.unpack(">I", hdr[4:8])[0]
        if comp_len > (1 << 28): raise KilnError("corrupt data: impossible block length")
        payload = read_upto(self.f, comp_len)
        if len(payload) < comp_len: raise KilnError("archive is truncated (cut inside a block)")
        return t + hdr + payload

class StreamReader:
    def __init__(self, src): self.src, self.out, self.mark = src, bytearray(), 0
    def next_chunk(self):
        if len(self.out) > 2 * WINDOW:
            del self.out[:len(self.out) - WINDOW]
        self.mark = len(self.out)
        rec = self.src.read_record()
        if not rec: raise KilnError("corrupt data: empty record")
        t = rec[0]
        if t == 2: return None
        if (t != 0 and t != 1) or len(rec) < 9: raise KilnError("corrupt data: unknown block type")
        raw_len, comp_len = struct.unpack(">II", rec[1:9]); payload = rec[9:]
        if len(payload) != comp_len: raise KilnError("corrupt data: block length mismatch")
        if t == 0:
            if comp_len != raw_len: raise KilnError("corrupt data: bad stored block")
            self.out += payload
        else:
            if raw_len > rc_mirror.MAX_BLOCK_RAW: raise KilnError("corrupt data: bad block size")
            try: rc_mirror.decode_block(payload, raw_len, self.out)
            except ValueError as e: raise KilnError(ERR.get(e.args[0], "corrupt data: bad block"))
        return bytes(self.out[self.mark:])

def make_unfilter(e):
    if e["filt"] == 0 or e["dead"]: return None
    if e["filt"] == MP3_FID: raise KilnError("this file uses the MP3 filter that the Rust engine does not support yet")
    if e["filt"] in (BCJ_X86, BCJ_ARM64): return Bcj(e["filt"])
    try: return wav_mirror.Wav(e["filt"], e["size"])
    except wav_mirror.KilnError as x: raise KilnError(str(x))

# ------------------------------------------------------------ crypto.rs
AUTH_FAIL = "authentication failed: wrong password, or the archive is damaged or has been tampered with"

def derive_key(password, salt, log_n, r, p):
    pw = unicodedata.normalize("NFKC", password).encode("utf-8")
    return hashlib.scrypt(pw, salt=salt, n=1 << log_n, r=r, p=p, maxmem=256 * (1 << log_n) * r, dklen=32)

def nonce(stream, counter): return bytes([stream, 0, 0, 0]) + struct.pack(">Q", counter)

def open_with(cipher, nc, ct, header, fin):
    try: return cipher.decrypt(nc, ct, header + bytes([fin]))
    except InvalidTag: return None

class CryptSource:
    def __init__(self, f, cipher, header, stream, n): self.f, self.cipher, self.header, self.stream, self.n = f, cipher, header, stream, n
    def read_record(self):
        raw = read_upto(self.f, 4)
        if len(raw) < 4: raise KilnError("archive is truncated (stream has no end)")
        length = struct.unpack(">I", raw)[0]
        if length < 17: raise KilnError("corrupt data: bad record length")
        ct = read_upto(self.f, length)
        if len(ct) < length: raise KilnError("archive is truncated (cut inside a record)")
        nc = nonce(self.stream, self.n)
        pt = open_with(self.cipher, nc, ct, self.header, 0)
        if pt is not None:
            if pt[:1] == b"\x02": raise KilnError(AUTH_FAIL)
        else:
            pt = open_with(self.cipher, nc, ct, self.header, 1)
            if pt is None or pt != b"\x02": raise KilnError(AUTH_FAIL)
        self.n += 1
        return pt

def make_source(f, cipher, header, stream, ctr):
    return CryptSource(f, cipher, header, stream, ctr) if cipher is not None else PlainSource(f)

class Extractor:
    def __init__(self, table, wanted, base, sink):
        self.table, self.wanted, self.base, self.sink = table, wanted, base, sink
        self.i = self.got = self.crc = 0; self.unf = None; self.open = False
        self.advance()
    def begin(self):
        e = self.table[self.i]; self.crc = self.got = 0; self.unf = make_unfilter(e)
        self.open = self.wanted[self.i]
        if self.open: self.sink.begin(self.base + self.i, e)
    def end(self):
        e = self.table[self.i]
        if self.crc != e["crc"] and not e["dead"]: raise KilnError(f"checksum failed for {e['rel']}")
        if self.open: self.sink.end(self.base + self.i, e); self.open = False
    def advance(self):
        while self.i < len(self.table):
            self.begin()
            if self.table[self.i]["slen"] == 0: self.end(); self.i += 1
            else: break
    def feed(self, data):
        while data:
            if self.i >= len(self.table): raise KilnError("corrupt data: stream is longer than the file table")
            need = self.table[self.i]["slen"] - self.got
            piece, data = data[:need], data[need:]
            self.got += len(piece)
            done = self.got == self.table[self.i]["slen"]
            if self.unf is not None:
                try:
                    out = self.unf.feed(piece)
                    if done: out += self.unf.finish()
                except wav_mirror.KilnError as x: raise KilnError(str(x))
            else: out = piece
            self.crc = zlib.crc32(out, self.crc)
            if self.open: self.sink.write(self.base + self.i, out)
            if done: self.end(); self.i += 1; self.advance()
    def finish(self):
        if self.i != len(self.table): raise KilnError("archive is truncated (data ends early)")

class NullSink:
    def begin(self, i, e): pass
    def write(self, i, b): pass
    def end(self, i, e): pass

class Archive:
    def __init__(self, f, entries, ckpts, table_off, hdr_len, header=b"", cipher=None):
        self.f, self.entries, self.ckpts, self.table_off, self.hdr_len = f, entries, ckpts, table_off, hdr_len
        self.header, self.cipher = header, cipher
        self.last_start = None
    @staticmethod
    def open(f, password=None):
        head = f.read(5)
        if head[:4] == b"KLN2": raise KilnError("this is an old KLN2 archive; use the old kiln.py to open it")
        if len(head) < 5 or head[:4] != MAGIC: raise KilnError("not a kiln archive (expected format KLNA)")
        flags = head[4]
        if flags & ~1: raise KilnError(NEWER)
        cipher = None
        if flags & 1:
            extra = f.read(19)
            if len(extra) < 19: raise KilnError("archive is truncated")
            head += extra
            log_n, r, p = extra[0], extra[1], extra[2]
            if not (10 <= log_n <= 20 and 1 <= r <= 16 and 1 <= p <= 4): raise KilnError("corrupt data: bad key-derivation settings")
            if password is None: raise KilnError("this archive is encrypted: a password is needed")
            cipher = AESGCM(derive_key(password, extra[3:], log_n, r, p))
        hdr_len = len(head)
        size = f.seek(0, 2)
        if size < hdr_len + FOOTER_LEN: raise KilnError("archive is truncated")
        f.seek(size - FOOTER_LEN); foot = f.read(FOOTER_LEN)
        if len(foot) < FOOTER_LEN: raise KilnError("archive is truncated")
        table_off, total = struct.unpack(">QQ", foot[:16])
        if foot[16:] != MAGIC or not (hdr_len <= table_off <= size - FOOTER_LEN):
            raise KilnError("archive is truncated or damaged (missing footer)")
        f.seek(table_off); raw = bytearray()
        rd = StreamReader(make_source(f, cipher, head, 1, 0))
        while True:
            c = rd.next_chunk()
            if c is None: break
            raw += c
        entries, ck = parse_table(bytes(raw), hdr_len, table_off)
        if cipher is None and sum(e["size"] for e in entries) != total: raise KilnError("corrupt data: file table doesn't match footer")
        return Archive(f, entries, ck, table_off, hdr_len, head, cipher)
    def test(self): self.extract([True] * len(self.entries), False, NullSink())
    def extract(self, wanted, select, sink):
        n = len(self.entries)
        if select:
            if not any(wanted): raise KilnError("no files in the archive match")
            lo = wanted.index(True); hi = n - 1 - wanted[::-1].index(True)
        else: lo, hi = 0, max(n - 1, 0)
        k, off, ctr = find_ckpt(self.ckpts, lo, self.hdr_len)
        part = select and (k > 0 or hi + 1 < n)
        start = k if part else 0
        self.last_start = (start, off)
        ext = Extractor(self.entries[start:], wanted[start:], start, sink)
        self.f.seek(off)
        rd = StreamReader(make_source(self.f, self.cipher, self.header, 0, ctr))
        while True:
            chunk = rd.next_chunk()
            if chunk is None: break
            ext.feed(chunk)
            if part and ext.i + start > hi: break
        if not part:
            ext.finish()
            if self.f.tell() != self.table_off: raise KilnError("corrupt data: stream doesn't end where the table starts")
