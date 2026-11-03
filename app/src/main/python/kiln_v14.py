#!/usr/bin/env python3
"""
kiln.py v14 - a from-scratch compressor and archive format (.kln)

Compression pipeline (all hand-written; zlib is used ONLY for CRC32 checksums
and, in `bench`, as a comparison):
  1. LZ77     64 MB sliding window. The default is the thorough setting: a
              binary tree per 4-byte hash (reaching back 16 MB, up to 384 nodes deep),
              plus a nearest-3-byte table and a long-match table for far repeats. The
              parser is an optimal one: the cheapest path of literals, matches and
              repeat-distance matches, priced with the range coder's own models.
              Chunks that look like random/compressed data (entropy check) are
              stored raw, probing only for long repeated blobs (anchors). That makes
              jpg/mp4/game assets fast.
  2. Range    an adaptive range coder (LZMA-style binary models: literals
              conditioned on the previous byte, matched literals, rep0-3 and
              short-rep matches; matches of length 2 are allowed at any distance)
              codes every block of ~512k tokens; the models restart per block.
              The match/literal state bits also see the position mod 4, and each
              block may add 0, 1 or 2 bits of its position to the literal context
              (the encoder tries all three and keeps the smallest; the choice is the
              first payload byte) - that is what helps floats, samples and records.
              Optionally (also per block, flagged in that byte) the literal bits are a
              logistic mix of the normal model, the full previous byte and a hash of
              the previous two bytes, with learned weights - that helps text and code.
  3. Solid    all files form ONE stream, so matches reach across files. Files
              are grouped by extension to put similar data next to each other.
  3b. Filters (v14) x86/AArch64 executables get their relative call/jump targets
              rewritten as absolute ones (kz_bcj), checked on a sample first.
  4. Stored   a block that doesn't shrink is stored raw. Already-compressed
              files (jpg, mp4, zip, ...) are sorted last and only probed at
              anchors, so they cost almost no time, yet an exact copy of one
              (within the 64 MB window) is still found.
  5. Streaming  input is read in chunks and the output is written as it goes,
              so memory stays bounded no matter how big the files are.
  6. Encryption (optional, -e) AES-256-GCM with a key from scrypt (2^17, r=8).
              Every block is its own authenticated record, so tampering,
              reordering, truncation and a wrong password are all detected.
              File names and sizes are encrypted too. Needs the `cryptography`
              package (Termux: pkg install python-cryptography).
  7. Parallel   segments are compressed on several CPU cores (-j N). The
              archive format doesn't change; only the speed does. Segments are
              4-16 MB: as small as the cores can use, no smaller (fewer boundaries
              means a smaller archive when there are fewer cores than 4 MB pieces).
  8. WAV filter 8-, 16-, 24- and 32-bit integer PCM .wav files (up to 8
              channels, plain or WAVE_FORMAT_EXTENSIBLE) are first turned into
              prediction residuals (each sample minus a fixed-order prediction,
              optionally with R-L stereo and zigzag), then split into byte
              planes. Float32 WAVs that really hold 8/16/24-bit integer audio
              are stored as those integers when every sample converts exactly.
              Other float WAVs get the byte-plane split only (the planes
              holding sign/exponent compress far better apart).
              Lossless; the file table remembers which filter a file used
              (see below), and the CRC covers the original file bytes.
 9. MP3 repack  .mp3 files are parsed and recoded with a context model of their
              Huffman data (table filter id 120); unpacking restores the exact
              original bytes, and files that can't be parsed are stored as usual.
 10. C helper  the C source is embedded in this file and does the heavy work
              (LZ77 match search, range coding, WAV filters, MP3 repack). The first
              run compiles it with cc/gcc/clang (Termux: pkg install clang) and
              loads it through ctypes. kiln needs it: with no compiler it stops with
              an error rather than running slowly. It is built with -ffp-contract=off
              so floating-point results match on every CPU (set KILN_CFLAGS to add flags).
 11. Repeats   Random-looking and high-entropy chunks aren't searched everywhere;
              kiln probes them at content-defined anchors (about 1 position in
              256) and, once a repeat is found, follows it to its end. So an
              exact copy of a jpg, an encrypted blob or noisy text costs about
              1% extra.
              Only chunks that really look like noise get this treatment: a 64 KB
              chunk is skipped when it holds no more repeated 4-byte groups than
              pure noise with its byte distribution would and no positional
              structure. Structured binaries (tables, floats, code, packed-but-
              patterned data) are searched in full.

Archive layout (big-endian):
  header     b"KLNA" + u8 flags            (flag 1 = encrypted)
             if encrypted: u8 log2(N) | u8 r | u8 p | 16-byte random salt
  blocks     repeated:  u8 type | u32 raw_len | u32 comp_len | payload
               type 0 = stored, type 1 = range-coded block, type 2 = end of stream
               (type 2 is a single byte with no other fields)
             Encrypted archives wrap each block in a record:
               u32 length | AES-GCM ciphertext+tag
               nonce = u8 stream(0 data, 1 table) | 3 zero bytes | u64 counter
               AAD   = the whole header | u8 is_final (1 only on the end block)
  table      the file table, itself stored as a block stream:
               u32 count, then per file:
               u16 pathlen | path | u64 size | u32 crc32 | u64 mtime | u16 mode
  footer     u64 table_offset | u64 total_size | b"KLNA"   (20 bytes)

Table entries: the 4 spare mode bits hold the WAV filter id (0-11). Value 15 means
an extended entry: 2 more bytes follow (bit 15 = file was removed and its data is
still in the stream; bits 0-14 = filter id, which may be up to 110).
Saving a plain archive from the shell appends new blocks and a new table instead
of rewriting it; removed files stay in the stream as hidden entries until `compact`.

Checkpoints: the table may end with an optional list - b"CKPT" | u32 count | per checkpoint
u32 file_index | u64 stream_offset | u64 record_counter (encrypted archives; else 0).
At a checkpoint (always at a file start, about every 64 MB of data) the encoder forgets
its match history, so decoding can start there instead of at the beginning: `cat`,
`extract` and `unpack NAME` skip everything before the nearest checkpoint. Appending
matches new files only against data since the last checkpoint.

Because the table sits at the end, `list` is instant and never decompresses.
A missing footer means the archive was cut short, and that is detected.

Range-coded block payload (type 1): one range-coder stream (first byte 0), no tables.
  Per token: is-match bit (context: last 3 token kinds and the position
  within the block mod 4, as in LZMA's pb=2; the rep/short-rep bits use the same context); a literal is an 8-bit tree
  under the previous byte's top 3 bits; a match is length-3 (low/mid/high trees), then
  distance slot = bit_length(dist-1) in a 5-bit tree, then extra bits (top 3 modelled,
  low 4 "align" modelled, the rest direct). The block ends when raw_len bytes are out.

Usage:
  python kiln.py pack   out.kln file_or_dir [more ...] [-j N] [-e] [-s] [-f] [-q]
                 -s / --store  encryption only: NO compression at all (needs -e). Data goes
                               in as stored blocks (type 0), AES-256-GCM encrypted. No LZ77,
                               no range coder, no WAV/MP3 filters. Same .kln format, so every
                               normal command (unpack, list, test, shell) reads it.
  python kiln.py unpack in.kln [names/patterns ...] [-o outdir] [-f] [-v] [-q]
  python kiln.py list   in.kln
  python kiln.py test   in.kln
  python kiln.py bench  file_or_dir [more ...]
  python kiln.py                     interactive shell (browse, add, rm, mv, extract...)
  python kiln.py archive.kln         the same, with that archive already open
Encrypted archives prompt for the password (or read env KILN_PASSWORD).
"""
import argparse
import array
import cmd
import fnmatch
import getpass
import glob
import collections
import hashlib
import math
import os
import shlex
import json
import secrets
import shutil
import struct
import tempfile
import sys
import time
import unicodedata
import zlib

VERSION = "14"             # kiln release shown in banners (the on-disk format id is MAGIC)
MAGIC = b"KLNA"             # archive format 10 (v14: adaptive-rate counters, executable filters)
FLAG_ENCRYPTED = 1
SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P = 17, 8, 1   # 128 MiB, OWASP-recommended
MIN_PASSWORD = 8
FOOTER_LEN = 20
WINDOW = (1 << 26) - 1       # 64 MB match window
DBITS = 32                  # a token is (length << DBITS) | distance, 64 bits
RECLAIM_MIN = 8 << 20       # removed files up to this much (or 30% of the data) stay stored on save
CKPT_SPAN = 64 << 20        # a checkpoint (history reset at a file start) about every this many bytes
MAX_MATCH = 258
BLOCK_TOKENS = 524288       # tokens per range-coded block
SEGMENT = 4 << 20           # new bytes matched per LZ pass (cmd_pack picks it with fit_segment)
SEGMENT_MIN = 4 << 20       # never smaller: short segments re-index their history for little work
SEGMENT_MAX = 16 << 20      # never bigger: each worker holds about 10 bytes of memory per segment byte
CHUNK = 1 << 20             # read size
# The one parser setting: (max search depth, "good enough" length, optimal parser, skip-ahead: 99 = never).
# The other match-finder tuning constants live in the C source below.
PARSE = (384, 258, 2, 99)
RND_BITS = 16               # entropy is judged per 64 KB chunk
_BYTEVALS = [bytes([v]) for v in range(256)]
# Already-compressed formats: sorted last and only anchor-probed (see feed_stored).
PRECOMPRESSED = {
    ".zip", ".gz", ".tgz", ".bz2", ".xz", ".7z", ".rar", ".zst", ".kln", ".apk",
    ".jar", ".jpg", ".jpeg", ".png", ".gif", ".webp", ".heic", ".avif", ".mp3",
    ".m4a", ".aac", ".ogg", ".opus", ".flac", ".mp4", ".mkv", ".webm", ".avi",
    ".mov", ".m4v", ".docx", ".xlsx", ".pptx", ".epub"}


# ---------------------------------------------------------------- C accelerator
_A = None               # the loaded C library (None only if it could not be built)
ACCEL_NOTE = ""         # why the C helper isn't available
_ACCEL_VERSION = 16
# -ffp-contract=off: no fused multiply-add, so the floating-point code (MP3 model, entropy
# estimates) gives bit-identical results on every CPU; archives stay portable between devices.
CFLAGS = ["-O3", "-ffp-contract=off", "-fno-math-errno", "-fno-semantic-interposition"]
_ct = None              # the ctypes module, when the helper is loaded


def _accel_open(path):
    import ctypes
    lib = ctypes.CDLL(path)
    lib.kz_version.restype = ctypes.c_int
    if lib.kz_version() != _ACCEL_VERSION:
        raise OSError("wrong helper version")
    V, I64, I = ctypes.c_void_p, ctypes.c_int64, ctypes.c_int
    sig = {
        "kz_lz": (I64, [V, I64, I64, V, I64, I, I, I, I, V, V, I64, V, I]),
        "kz_rc_enc": (I64, [V, I64, V, I64, V, I64, V, I]),
        "kz_rc_dec": (I, [V, I64, V, I64, I64, I64]),
        "kz_wav16_enc": (I, [V, I64, I, I, I, I, V]),
        "kz_wav16_dec": (I, [V, I64, I, I, I, I, V]),
        "kz_shuf_enc": (None, [V, I64, I, I, V]),
        "kz_shuf_dec": (None, [V, I64, I, I, V]),
        "kz_int_enc": (I, [V, I64, I, I, I, I, I, V]),
        "kz_int_dec": (I, [V, I64, I, I, I, I, I, V]),
        "kz_f2i": (I, [V, I64, I, I, V]),
        "kz_i2f": (None, [V, I64, I, I, V]),
        "kz_rnd": (None, [V, I64, V]),
        "kz_bcj": (I64, [I, V, I64, I64, I]),
        "kz_mp3_pack": (I64, [V, I64, V, I64]),
        "kz_mp3_unpack": (I, [V, I64, V, I64]),
    }
    for name, (res, args) in sig.items():
        fn = getattr(lib, name)
        fn.restype, fn.argtypes = res, args
    return lib


def _accel_init():
    global _A, _ct, ACCEL_NOTE
    if sys.byteorder != "little":
        ACCEL_NOTE = "needs a little-endian CPU"
        return
    here = os.path.dirname(os.path.abspath(__file__))
    code = _ACCEL_SRC.encode("utf-8")
    code2 = _MP3_SRC.encode("utf-8")
    try:
        import ctypes
        import platform
        import tempfile
        if array.array("I").itemsize != 4:
            raise OSError("unusual int size")
    except (OSError, ImportError) as e:
        ACCEL_NOTE = f"unavailable ({e})"
        return
    env_lib = os.environ.get("KILN_ACCEL_LIB")  # prebuilt helper (the Android app sets this)
    if env_lib:
        try:
            _A, _ct = _accel_open(env_lib), ctypes
        except OSError as e:
            ACCEL_NOTE = f"couldn't load KILN_ACCEL_LIB: {e}"
        return
    flags = CFLAGS + os.environ.get("KILN_CFLAGS", "").split()
    tag = hashlib.sha1(code + code2 + (sys.platform + platform.machine() + " ".join(flags)
                               + sys.version[:5]).encode()).hexdigest()[:12]
    name = f"kiln_accel_{tag}.so"
    dirs = [here, os.path.join(os.path.expanduser("~"), ".cache", "kiln"),
            tempfile.gettempdir()]
    for d in dirs:
        p = os.path.join(d, name)
        if os.path.isfile(p):
            try:
                _A, _ct = _accel_open(p), ctypes
                return
            except OSError:
                pass
    import subprocess
    ccs = [c for c in (os.environ.get("CC"), "cc", "gcc", "clang") if c]
    cc = next((shutil.which(c) for c in ccs if shutil.which(c)), None)
    if not cc:
        ACCEL_NOTE = "no C compiler found (Termux: pkg install clang)"
        return
    err = "no writable folder"
    for d in dirs:
        tmp = os.path.join(d, f".{name}.{os.getpid()}.tmp")
        try:
            os.makedirs(d, exist_ok=True)
            src = tmp + ".c"
            src2 = tmp + ".mp3.c"
            with open(src, "wb") as fh:
                fh.write(code)
            with open(src2, "wb") as fh:
                fh.write(code2)
            r = subprocess.run([cc, *flags, "-shared", "-fPIC", "-o", tmp, src, src2, "-lm"],
                               capture_output=True, text=True, timeout=300)
            if r.returncode:
                err = (r.stderr.strip().splitlines() or ["compiler failed"])[-1][:100]
                continue
            os.replace(tmp, os.path.join(d, name))
            _A, _ct = _accel_open(os.path.join(d, name)), ctypes
            return
        except (OSError, subprocess.SubprocessError) as e:
            err = str(e)[:100]
        finally:
            for junk in (tmp, tmp + ".c", tmp + ".mp3.c"):
                try:
                    os.remove(junk)
                except OSError:
                    pass
    ACCEL_NOTE = "couldn't build the C helper: " + err


def require_accel():
    """kiln can't run without its C helper: say why, once, instead of crashing later."""
    if _A is None:
        raise KilnError("kiln needs its C helper, which isn't available: "
                        + (ACCEL_NOTE or "unknown reason"))


def _addr(a):
    """Address of an array.array's data."""
    return a.buffer_info()[0]



class KilnError(Exception):
    pass


# ---------------------------------------------------------------- helpers
def fmt_size(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024


FILE_HOOK = None      # the app sets this to f(name, size): called as each file starts packing/extracting


def file_started(name, size):
    if FILE_HOOK is not None:
        FILE_HOOK(name, size)


FILES_DONE_HOOK = None  # f([full paths]): called when an archive is finished and in place


def files_done(paths):
    if FILES_DONE_HOOK is not None:
        FILES_DONE_HOOK(paths)


class Progress:
    def __init__(self, total, label, enabled=True):
        self.total = max(total, 1)
        self.done = 0
        self.label = label
        self.enabled = enabled and sys.stderr.isatty()
        self.t0 = time.time()
        self.last = 0.0

    def add(self, n):
        self.done += n
        if not self.enabled:
            return
        now = time.time()
        if now - self.last < 0.1 and self.done < self.total:
            return
        self.last = now
        frac = min(self.done / self.total, 1.0)
        fill = int(24 * frac)
        rate = self.done / max(now - self.t0, 1e-6)
        sys.stderr.write(f"\r{self.label} [{'#' * fill}{'.' * (24 - fill)}] "
                         f"{frac * 100:5.1f}%  {fmt_size(rate)}/s   ")
        sys.stderr.flush()

    def close(self):
        if self.enabled:
            sys.stderr.write("\r" + " " * 70 + "\r")
            sys.stderr.flush()


# ---------------------------------------------------------------- WAV filter
WAV_FRAMES = 1 << 18        # frames per filter chunk (planes are 256 KB, so 64 KB
                            # entropy chunks see pure low-byte or high-byte data)
WAV_HEAD = 1 << 16          # the RIFF header must end within the first 64 KB
WAV_MIN = 1 << 17           # smaller audio isn't worth filtering
_NEED = object()
SHUF_WIDTH = {9: 4, 10: 3, 11: 8}      # filter id -> bytes per sample
SHUF_ID = {4: 9, 3: 10, 8: 11}


BCJ_X86, BCJ_ARM64 = 121, 122      # table filter ids: executable branch-target filters
BCJ_FIDS = (BCJ_X86, BCJ_ARM64)
BCJ_BLOCK = 1 << 20           # filtered in independent blocks (x86 encodes a block right to left)
BCJ_MIN = 1 << 14             # smaller executables aren't worth it
MP3_FID = 120                 # table filter id: "MP3 recompressed" (stream holds the packed form)
MP3_MIN = 16 << 10            # smaller files aren't worth it
MP3_MAX = 64 << 20            # the whole file (and its parse tree, ~10x) is held in memory
MP3_GAIN = 0.97               # use the filter only if it saves at least 3%


def mp3_pack(data):
    """-> packed bytes, or None when the file isn't (enough of) an MPEG-1 Layer III stream
    or the packed form doesn't rebuild the exact same bytes. Always verified."""
    n = len(data)
    cap = n + n // 8 + 4096
    out = _ct.create_string_buffer(cap)
    m = _A.kz_mp3_pack(data, n, out, cap)
    if m <= 0 or m >= n * MP3_GAIN:
        return None
    blob = out.raw[:m]
    if mp3_unpack(blob, n) != data:
        return None
    return blob


def mp3_unpack(blob, n):
    out = _ct.create_string_buffer(n + 16)
    if _A.kz_mp3_unpack(blob, len(blob), out, n):
        raise KilnError("corrupt data: bad packed MP3")
    return out.raw[:n]


class Mp3Un:
    """Extraction side of the MP3 filter: collects the packed bytes, decodes at the end."""
    def __init__(self, size):
        self.size, self.buf = size, bytearray()

    def feed(self, piece):
        self.buf += piece
        return b""

    def finish(self):
        return mp3_unpack(bytes(self.buf), self.size)


class Bcj:
    """Executable filter (same length in and out): feeds fixed-size blocks to kz_bcj."""
    def __init__(self, fid, size, decode=False):
        self.fid, self.dec, self.pos, self.buf = fid, decode, 0, bytearray()

    def _run(self, n):
        blk = bytes(self.buf[:n])
        del self.buf[:n]
        b = _ct.create_string_buffer(blk, n)
        _A.kz_bcj(self.fid, b, n, self.pos, 1 if self.dec else 0)
        self.pos += n
        return b.raw[:n]

    def feed(self, chunk):
        self.buf += chunk
        out = []
        while len(self.buf) >= BCJ_BLOCK:
            out.append(self._run(BCJ_BLOCK))
        return b"".join(out)

    def finish(self):
        return self._run(len(self.buf)) if self.buf else b""


def make_filter(fid, size, decode=False):
    return Bcj(fid, size, decode) if fid in BCJ_FIDS else Wav(fid, size, decode)


def bcj_kind(head):
    """x86 / AArch64 executable (ELF or PE) from the first bytes of a file -> filter id or 0."""
    if head[:4] == b"\x7fELF" and len(head) >= 20 and head[5] == 1:
        m = int.from_bytes(head[18:20], "little")
        return BCJ_X86 if m in (3, 62) else BCJ_ARM64 if m == 183 else 0
    if head[:2] == b"MZ" and len(head) >= 64:
        off = int.from_bytes(head[60:64], "little")
        if off + 6 <= len(head) and head[off:off + 4] == b"PE\0\0":
            m = int.from_bytes(head[off + 4:off + 6], "little")
            return BCJ_X86 if m in (0x14c, 0x8664) else BCJ_ARM64 if m == 0xAA64 else 0
    return 0


def pick_bcj(path, size):
    """Filter id for an executable that the filter measurably helps, else 0. A 1 MB sample
    from the middle is deflated with and without the filter; the choice is stored per file."""
    if size < BCJ_MIN:
        return 0
    try:
        with open(path, "rb") as fh:
            head = fh.read(4096)
            fid = bcj_kind(head)
            if not fid:
                return 0
            off = (size // 3) & ~3 if size > 4 * BCJ_BLOCK else 0
            fh.seek(off)
            raw = fh.read(BCJ_BLOCK)
    except OSError:
        return 0
    b = _ct.create_string_buffer(raw, len(raw))
    _A.kz_bcj(fid, b, len(raw), off, 0)
    return fid if len(zlib.compress(b.raw[:len(raw)], 6)) < len(zlib.compress(raw, 6)) * 0.995 else 0


def make_unfilter(e):
    """The decoder object for a table entry, or None."""
    fid = e.get("filt")
    if not fid or e.get("dead"):
        return None
    return Mp3Un(e["size"]) if fid == MP3_FID else make_filter(fid, e["size"], True)


def slen(e):
    """Bytes this entry occupies in the data stream (= size, except for the MP3 filter)."""
    return e.get("slen", e["size"])


def wav_layout(head, size):
    """-> (data_start, region_bytes, channels, block_align, bytes_per_sample, is_float)
    for 8/16-bit PCM or 24/32/64-bit PCM/float (incl. WAVE_FORMAT_EXTENSIBLE),
    _NEED if head is too short to tell, None if this isn't a filterable WAV.
    Only bytes up to the 'data' chunk header are looked at."""
    if len(head) < 12:
        return _NEED if size >= 12 else None
    if head[:4] != b"RIFF" or head[8:12] != b"WAVE":
        return None
    pos, fmt = 12, None
    while True:
        if pos > WAV_HEAD:
            return None
        if len(head) < pos + 8:
            return _NEED if size >= pos + 8 and len(head) < WAV_HEAD else None
        cid, csz = struct.unpack_from("<4sI", head, pos)
        if cid == b"fmt ":
            if csz < 16:
                return None
            if len(head) < pos + 8 + (26 if csz >= 40 else 16):
                return _NEED if len(head) < WAV_HEAD else None
            tag, ch, _rate, _bps, align, bits = struct.unpack_from("<HHIIHH", head, pos + 8)
            if tag == 0xFFFE:                  # extensible: the real format is in the GUID
                tag = struct.unpack_from("<H", head, pos + 8 + 24)[0] if csz >= 40 else 1
            if tag not in (1, 3):
                return None
            ok = tag == 1 if bits in (8, 16) else bits in (24, 32, 64)
            if not ok or not 1 <= ch <= 8 or align != (bits // 8) * ch:
                return None
            fmt = (ch, align, bits // 8, tag == 3)
        elif cid == b"data":
            if fmt is None:
                return None
            start = pos + 8
            dlen = min(csz, size - start)
            if dlen < 0:
                return None
            return start, (dlen // fmt[1]) * fmt[1], fmt[0], fmt[1], fmt[2], fmt[3]
        pos += 8 + csz + (csz & 1)


def wav_spec(fid):
    """Filter id -> (kind, sample_bytes_in_file, order, ms, zigzag, bias, scale_bits)
    or None.  ids: 1-8 16-bit predictor, 9-11 byte planes, 16-62 integer predictor
    (24/32/8-bit), 64-110 float32 that is really integer audio."""
    if fid in SHUF_WIDTH:
        return "shuf", SHUF_WIDTH[fid], 0, False, False, 0, 0
    if 1 <= fid <= 8:
        return ("w16", 2, 1 + ((fid - 1) & 1), bool((fid - 1) & 2), False,
                128 if (fid - 1) & 4 else 0, 0)
    if 16 <= fid <= 62 or 64 <= fid <= 110:
        base = 16 if fid < 64 else 64
        g = fid - base
        order, ms, zz, wc = g & 3, bool((g >> 2) & 1), bool((g >> 3) & 1), g >> 4
        if order > 2 or wc > 2:
            return None
        if base == 16:
            return "int", (3, 4, 1)[wc], order, ms, zz, 0, 0
        return "fint", 4, order, ms, zz, 0, (7, 15, 23)[wc]
    return None


def int_fid(width, order, ms, zz):
    return 16 + order + 4 * ms + 8 * zz + 16 * {3: 0, 4: 1, 1: 2}[width]


def fint_fid(s, order, ms, zz):
    return 64 + order + 4 * ms + 8 * zz + 16 * {7: 0, 15: 1, 23: 2}[s]


def filter_desc(fid):
    sp = wav_spec(fid)
    if sp is None:
        return "unknown"
    kind, w, order, ms, zz, bias, s = sp
    if kind == "shuf":
        return f"byte planes ({8 * w}-bit samples)"
    extra = (", R-L" if ms else "") + (", zigzag" if zz else "")
    if kind == "w16":
        return f"16-bit predictor (order {order}{extra})"
    if kind == "int":
        return f"{8 * w}-bit integer predictor (order {order}{extra})"
    return f"float32 stored as {s + 1}-bit integers (order {order}{extra})"


def int_enc(chunk, n, W, C, order, ms, zz):
    """Interleaved W-byte little-endian samples -> per channel W byte planes.
    Steps per channel: (channel 1 minus channel 0), delta x order, zigzag; all mod 2^(8W)."""
    out = _ct.create_string_buffer(len(chunk))
    if _A.kz_int_enc(chunk, n, W, C, order, int(ms), int(zz), out):
        raise KilnError("out of memory in the WAV filter")
    return out.raw


def int_dec(chunk, n, W, C, order, ms, zz):
    out = _ct.create_string_buffer(len(chunk))
    if _A.kz_int_dec(chunk, n, W, C, order, int(ms), int(zz), out):
        raise KilnError("out of memory in the WAV filter")
    return out.raw


def f2i(chunk, m, s, kw):
    """m float32 samples -> m signed integers of kw bytes (value * 2^s), or None if
    any sample isn't exactly such an integer (NaN, -0.0, out of range, fraction)."""
    out = _ct.create_string_buffer(m * kw + 1)
    return out.raw[:m * kw] if _A.kz_f2i(chunk, m, s, kw, out) == 0 else None


def i2f(buf, m, s, kw):
    out = _ct.create_string_buffer(m * 4 + 1)
    _A.kz_i2f(buf, m, s, kw, out)
    return out.raw[:m * 4]


class Wav:
    """Streaming WAV filter. decode=False turns file bytes into filtered bytes,
    decode=True turns them back. Same length in and out; feed() may return less
    than it was given (it buffers) and finish() returns the rest.
    Filter ids are described by wav_spec()."""
    def __init__(self, fid, size, decode=False):
        sp = wav_spec(fid)
        if sp is None:
            raise KilnError("archive uses features from a newer kiln")
        self.kind, self.W, self.order, self.ms, self.zz, self.bias, self.s = sp
        self.kw = (self.s + 1 + 7) // 8 if self.kind == "fint" else self.W
        self.size, self.decode = size, decode
        self.buf = bytearray()
        self.state = 0           # 0 = waiting for header, 1 = data, 2 = raw tail
        self.left = 0

    def feed(self, data):
        self.buf += data
        out = bytearray()
        if self.state == 0:
            lay = wav_layout(bytes(self.buf[:WAV_HEAD]), self.size)
            if lay is _NEED:
                return b""
            if lay is None or lay[2] < 1 or (self.ms and lay[2] != 2) \
                    or lay[4] != self.W:
                raise KilnError("corrupt data: bad WAV header for the filter")
            self.start, self.left, self.C, self.align = lay[:4]
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
            out += self._dec(chunk) if self.decode else self._enc(chunk)
            self.left -= take
        if self.state == 2:
            out += self.buf
            self.buf.clear()
        return bytes(out)

    def finish(self):
        out = self.feed(b"")
        if self.buf:                       # no usable header: pass through
            out += self.buf
            self.buf.clear()
        return out

    @property
    def nplanes(self):
        return self.C * (2 if self.kind == "w16" else self.W)

    def _enc(self, chunk):
        C = self.C
        if self.kind == "int":
            return int_enc(chunk, len(chunk) // self.align, self.W, C, self.order,
                           self.ms, self.zz)
        if self.kind == "fint":
            n = len(chunk) // self.align
            ints = f2i(chunk, n * C, self.s, self.kw)
            if ints is None:
                raise KilnError("float samples are not whole-number audio (file changed?)")
            enc = int_enc(ints, n, self.kw, C, self.order, self.ms, self.zz)
            return enc + bytes(len(chunk) - len(enc))
        out = _ct.create_string_buffer(len(chunk))
        if self.kind == "shuf":
            _A.kz_shuf_enc(chunk, len(chunk) // self.align, self.W, C, out)
        else:
            _A.kz_wav16_enc(chunk, len(chunk) // (2 * C), C, self.order, int(self.ms),
                            self.bias, out)
        return out.raw

    def _dec(self, chunk):
        C = self.C
        if self.kind == "int":
            return int_dec(chunk, len(chunk) // self.align, self.W, C, self.order,
                           self.ms, self.zz)
        if self.kind == "fint":
            n = len(chunk) // self.align
            ints = int_dec(chunk[:self.kw * C * n], n, self.kw, C, self.order, self.ms, self.zz)
            return i2f(ints, n * C, self.s, self.kw)
        out = _ct.create_string_buffer(len(chunk))
        if self.kind == "shuf":
            _A.kz_shuf_dec(chunk, len(chunk) // self.align, self.W, C, out)
        else:
            _A.kz_wav16_dec(chunk, len(chunk) // (2 * C), C, self.order, int(self.ms),
                            self.bias, out)
        return out.raw


def _entropy_bits(b):
    n = len(b)
    if not n:
        return 0.0
    inv, ent = 1.0 / n, 0.0
    for v in _BYTEVALS:
        k = b.count(v)
        if k:
            p = k * inv
            ent -= p * math.log2(p)
    return ent * n


def wav_why(path, size):
    """Why a .wav wasn't filtered (a short human-readable reason)."""
    try:
        with open(path, "rb") as fh:
            head = fh.read(WAV_HEAD)
    except OSError as e:
        return e.strerror
    if head[:4] != b"RIFF" or head[8:12] != b"WAVE":
        return "not a RIFF/WAVE file (RF64/other)"
    lay = wav_layout(head, size)
    if lay is not None and lay is not _NEED:
        return "tried, but the filter wouldn't shrink it by at least 3%"
    pos = 12
    while pos + 8 <= len(head):
        cid, csz = struct.unpack_from("<4sI", head, pos)
        if cid == b"fmt " and pos + 24 <= len(head):
            tag, ch, rate, _b, _a, bits = struct.unpack_from("<HHIIHH", head, pos + 8)
            kind = {1: "PCM", 3: "float", 0xFFFE: "extensible"}.get(tag, f"format {tag}")
            return (f"{kind}, {bits}-bit, {ch} ch, {rate} Hz (supported: 8/16-bit PCM, "
                    "8-bit PCM, 24/32/64-bit PCM or float, up to 8 channels)")
        pos += 8 + csz + (csz & 1)
    return "no fmt chunk in the first 64 KB"


def _float_scale(samples):
    """Smallest s in (7, 15, 23) for which every float32 in the sample windows is a
    whole number times 2^-s, else 0."""
    for s in (7, 15, 23):
        if all(f2i(w, len(w) // 4, s, (s + 8) // 8) is not None for w in samples):
            return s
    return 0


def _float_verified(fh, start, region, s):
    """Checks the whole data region converts exactly (the encoder can't back out later)."""
    kw = (s + 8) // 8
    fh.seek(start)
    left = region
    while left > 0:
        w = fh.read(min(left, 1 << 22))
        if not w:
            return False
        w = w[:len(w) // 4 * 4]
        if not w or f2i(w, len(w) // 4, s, kw) is None:
            return False
        left -= len(w)
    return True


def pick_wav_filter(path, size):
    """0 = don't filter, else the filter id with the lowest estimated cost."""
    if size < WAV_MIN:
        return 0
    try:
        with open(path, "rb") as fh:
            head = fh.read(WAV_HEAD)
            lay = wav_layout(head, size)
            if lay is None or lay is _NEED:
                return 0
            start, region, C, align, width, isfloat = lay
            if region < WAV_MIN:
                return 0
            win = min(16384, region // align // 4) * align
            samples = []
            for q in (1, 2, 3):                      # 3 windows: 25%, 50%, 75%
                off = start + (region * q // 4) // align * align
                fh.seek(off)
                w = fh.read(win)
                if len(w) == win:
                    samples.append(w)
            if not samples:
                return 0
            cands = []                                # (fid, bits/byte planes, chan count)
            if width == 2 and not isfloat:
                cands = [1, 2, 5, 6] if C != 2 else [1, 2, 3, 4, 5, 6, 7, 8]
            elif width in (1, 3, 4) and not isfloat:
                for order in (1, 2):
                    for zz in (0, 1):
                        for ms in ((0, 1) if C == 2 else (0,)):
                            cands.append(int_fid(width, order, ms, zz))
                if C == 2:
                    cands += [int_fid(width, 0, 1, zz) for zz in (0, 1)]
                if width in SHUF_ID:
                    cands.append(SHUF_ID[width])
            else:
                if width in SHUF_ID:
                    cands.append(SHUF_ID[width])
                if width == 4 and isfloat:
                    s = _float_scale(samples)
                    if s and _float_verified(fh, start, region, s):
                        for order in (1, 2):
                            for zz in (0, 1):
                                for ms in ((0, 1) if C == 2 else (0,)):
                                    cands.append(fint_fid(s, order, ms, zz))
                        if C == 2:
                            cands += [fint_fid(s, 0, 1, zz) for zz in (0, 1)]
    except OSError:
        return 0
    base = sum(_entropy_bits(w) for w in samples)
    best, best_cost = 0, base * 0.97
    for fid in cands:
        f = Wav(fid, 1 << 40)
        f.C, f.align = C, align
        try:
            cost = 0.0
            for w in samples:
                e = f._enc(w)
                m = len(w) // align
                cost += sum(_entropy_bits(e[k * m:(k + 1) * m]) for k in range(f.nplanes))
        except KilnError:
            continue
        if cost < best_cost:
            best, best_cost = fid, cost
    return best


# ---------------------------------------------------------------- LZ77
def random_chunks(buf):
    """Per 64 KB chunk: 0 = normal, 1 = looks random / already compressed (stored
    raw), 2 = no repeats worth coding (matching can't beat plain entropy coding, e.g.
    the noisy planes of a filtered WAV), 3 = sparse structure (search, but only a few
    candidates). Order-0 entropy of an 8 KB sample picks the test. From 3 bits up, class 2
    needs the whole 64 KB chunk to show no structure (the byte's position in an 8-byte
    period and the previous byte's top bits must each tell under 0.06 bits) and no more
    repeated groups than noise with the same byte distribution would give by chance, so
    floats, 16-bit samples, tables and counters are never mistaken for noise. Below 3 bits
    an n-gram uniqueness check on the sample decides."""
    n = len(buf)
    nch = (n + (1 << RND_BITS) - 1) >> RND_BITS
    out = _ct.create_string_buffer(nch or 1)
    _A.kz_rnd(buf, n, out)
    return out.raw[:nch]


def lz77(buf, start, rnd, lp=0):
    """C match finder. Returns a list of parts, each a token array or an int (a count of
    bytes to store raw). A token is a literal byte (0-255) or (length << 32) | distance."""
    chain_max, nice, lazy, accel = PARSE
    n = len(buf)
    cap = n - start + 16
    runcap = cap // 64 + 16
    tok = array.array("Q", [0]) * cap
    runs = array.array("q", [0]) * (2 * runcap)
    nr = _ct.c_int64(0)
    nt = _A.kz_lz(buf, n, start, bytes(rnd), len(rnd), chain_max, nice, int(lazy),
                  accel, _addr(tok), _addr(runs), runcap, _ct.byref(nr), lp)
    if nt < 0:
        raise KilnError("internal error: match finder")
    parts, at = [], 0
    for k in range(nr.value):
        ti, rawlen = runs[2 * k], runs[2 * k + 1]
        if ti > at:
            parts.append(tok[at:ti])
            at = ti
        parts.append(rawlen)
    if nt > at:
        parts.append(tok[at:nt])
    return parts


# ---------------------------------------------------------------- block codec
_DECODE_ERRORS = {
    -1: "corrupt data: bad code", -2: "corrupt data: block ran past its end",
    -3: "corrupt data: bad distance", -4: "corrupt data: block too long",
    -5: "corrupt data: block overread", -6: "corrupt data: block size mismatch",
    -7: "corrupt data: out of memory"}


def encode_block(tokens, buf, pos, lp=0):
    """Range-code a token block whose bytes start at buf[pos]. lp = bits of the position in the
    block added to the literal context (0, 1 or 2); the decoder reads it from the first payload byte.
    Returns (raw_length, payload); payload is None if it didn't fit (store the bytes instead)."""
    if not isinstance(tokens, array.array):
        tokens = array.array("Q", tokens)
    nt = len(tokens)
    cap = nt * 16 + 64
    out = _ct.create_string_buffer(cap)
    raw = _ct.c_int64(0)
    k = _A.kz_rc_enc(_addr(tokens), nt, buf, pos, out, cap, _ct.byref(raw), lp)
    return raw.value, (None if k < 0 else _ct.string_at(out, k))


def decode_block(payload, raw_len, out):
    """Decode one range-coded block, appending raw_len bytes to bytearray `out`."""
    if raw_len > BLOCK_TOKENS * MAX_MATCH:
        raise KilnError("corrupt data: bad block size")
    n0 = len(out)
    out.extend(bytes(raw_len))
    view = (_ct.c_char * len(out)).from_buffer(out)
    try:
        rc = _A.kz_rc_dec(bytes(payload), len(payload), view, len(out), n0, raw_len)
    finally:
        del view
    if rc:
        del out[n0:]
        raise KilnError(_DECODE_ERRORS.get(rc, "corrupt data: bad block"))


# ---------------------------------------------------------------- encryption
def _aesgcm():
    try:
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
        from cryptography.exceptions import InvalidTag
    except ImportError:
        raise KilnError("encryption needs the 'cryptography' package "
                        "(Termux: pkg install python-cryptography, or pip install cryptography)")
    return AESGCM, InvalidTag


def get_password(confirm=False):
    pw = os.environ.get("KILN_PASSWORD")
    if pw is None:
        pw = getpass.getpass("Password: ")
        if confirm:
            if getpass.getpass("Repeat password: ") != pw:
                raise KilnError("passwords don't match")
    if confirm and len(pw) < MIN_PASSWORD:
        raise KilnError(f"password must be at least {MIN_PASSWORD} characters "
                        "(a long passphrase is best)")
    if not pw:
        raise KilnError("empty password")
    return pw


def derive_key(password, salt, log_n, r, p):
    pw = unicodedata.normalize("NFKC", password).encode("utf-8")
    try:
        return hashlib.scrypt(pw, salt=salt, n=1 << log_n, r=r, p=p,
                              maxmem=256 * (1 << log_n) * r, dklen=32)
    except (AttributeError, ValueError, MemoryError):
        raise KilnError("this Python can't run scrypt with the archive's settings")


class Cipher:
    """AES-256-GCM. Each record gets a unique nonce (stream, counter) and
    authenticates the archive header, so nothing can be swapped or dropped."""
    def __init__(self, key, header):
        AESGCM, self.InvalidTag = _aesgcm()
        self.aead = AESGCM(key)
        self.header = header

    def sink(self, f, stream):
        return CryptSink(self, f, stream)

    def source(self, f, stream):
        return CryptSource(self, f, stream)


def _nonce(stream, counter):
    return bytes([stream, 0, 0, 0]) + struct.pack(">Q", counter)


class PlainSink:
    def __init__(self, f):
        self.f = f

    def write_record(self, data, final=False):
        self.f.write(data)

    def pos(self):
        return self.f.tell(), 0


class CryptSink:
    def __init__(self, c, f, stream):
        self.c, self.f, self.stream, self.n = c, f, stream, 0

    def write_record(self, data, final=False):
        aad = self.c.header + (b"\x01" if final else b"\x00")
        ct = self.c.aead.encrypt(_nonce(self.stream, self.n), data, aad)
        self.n += 1
        self.f.write(struct.pack(">I", len(ct)) + ct)

    def pos(self):
        return self.f.tell(), self.n


class PlainSource:
    def __init__(self, f):
        self.f = f

    def read_record(self):
        f = self.f
        t = f.read(1)
        if not t:
            raise KilnError("archive is truncated (stream has no end)")
        if t == b"\x02":
            return t
        hdr = f.read(8)
        if len(hdr) < 8:
            raise KilnError("archive is truncated (cut inside a block header)")
        comp_len = struct.unpack(">II", hdr)[1]
        if comp_len > (1 << 28):   # real blocks are far smaller; don't try to allocate garbage
            raise KilnError("corrupt data: impossible block length")
        payload = f.read(comp_len)
        if len(payload) < comp_len:
            raise KilnError("archive is truncated (cut inside a block)")
        return t + hdr + payload


class CryptSource:
    def __init__(self, c, f, stream):
        self.c, self.f, self.stream, self.n = c, f, stream, 0

    def read_record(self):
        c = self.c
        raw = self.f.read(4)
        if len(raw) < 4:
            raise KilnError("archive is truncated (stream has no end)")
        (length,) = struct.unpack(">I", raw)
        if length < 17:
            raise KilnError("corrupt data: bad record length")
        ct = self.f.read(length)
        if len(ct) < length:
            raise KilnError("archive is truncated (cut inside a record)")
        nonce = _nonce(self.stream, self.n)
        fail = KilnError("authentication failed: wrong password, or the archive "
                         "is damaged or has been tampered with")
        try:
            pt = c.aead.decrypt(nonce, ct, c.header + b"\x00")
            if pt[:1] == b"\x02":   # an end marker is only valid as the final record
                raise fail
        except c.InvalidTag:
            try:
                pt = c.aead.decrypt(nonce, ct, c.header + b"\x01")
            except c.InvalidTag:
                raise fail
            if pt != b"\x02":
                raise fail
        self.n += 1
        return pt


# ---------------------------------------------------------------- streams
def build_blocks(buf, start):
    """Compress buf[start:] (history before it) into [(type, raw_len, payload)]."""
    # Only chunks that look like pure noise skip the search (they are still probed for exact and
    # long copies); everything else gets the full optimal parse, with no skip-ahead.
    rnd = [1 if x == 1 else 0 for x in random_chunks(buf)]

    def total(bl):
        return sum(len(x[2]) for x in bl)

    stat = [0, 0, 0]
    best = _code_parts(lz77(buf, start, rnd), buf, start, stat)
    if stat[0] > 0:
        # Records, samples and floats gain from a literal position setting in the coder, and then
        # the parser should price literals with it too (it otherwise prices them without). The
        # second parse is slow and only paid off when the setting itself gained at least
        # LP_REPARSE of the block sizes (floats, record tables); elsewhere it did nothing or hurt.
        s4, s5, s6 = stat
        lp, sl = (1, s5) if s5 < s6 else (2, s6)
        if lp and s4 >= sl * (1.0 + LP_REPARSE):
            alt = _code_parts(lz77(buf, start, rnd, lp), buf, start)
            if total(alt) < total(best):
                best = alt
    return best


def store_blocks(buf, start):
    """No compression: buf[start:] as stored blocks (type 0) of up to 1 MB each."""
    return [(0, len(piece), piece)
            for piece in (buf[i:i + (1 << 20)] for i in range(start, len(buf), 1 << 20))]


TRIAL_TOKENS = 16384    # a long block's coding mode is picked on its first tokens, not on all of it
MODE_TIE = 1.01         # modes within this factor of the best are both tried on the whole block
LP_REPARSE = 0.02       # re-parse a segment with position bits only if they gained this much
_MODES = (0, 1, 2, 4, 5, 6)   # position setting (0-2), +4 = literal mixer


def _try_modes(tk, buf, pos, modes):
    """{mode: (raw_length, payload or None)} for encoding the block tk in each mode."""
    return {m: encode_block(tk, buf, pos, m) for m in modes}


def _code_block(tk, buf, pos, stat):
    """Range-code one token block in the cheapest mode: (raw_length, payload or None)."""
    big = 1 << 60
    def size(r):
        return len(r[1]) if r[1] is not None else big
    if len(tk) <= 3 * TRIAL_TOKENS:
        res = _try_modes(tk, buf, pos, _MODES)       # small block: just try everything
        sz = {m: size(r) for m, r in res.items()}
        best = res[min(sz, key=sz.get)]
    else:
        # Cold-start size on the block's first tokens ranks the modes the same way the whole
        # block would (the models start fresh at each block); only near-ties need the whole block.
        sz = {m: size(r) for m, r in _try_modes(tk[:TRIAL_TOKENS], buf, pos, _MODES).items()}
        order = sorted(sz, key=sz.get)
        keep = [m for m in order[:2] if sz[m] <= sz[order[0]] * MODE_TIE]
        res = _try_modes(tk, buf, pos, keep)
        best = min(res.values(), key=size)
    if max(sz[4], sz[5], sz[6]) < big:
        stat[0] += sz[4]
        stat[1] += sz[5]
        stat[2] += sz[6]
    return best


def _code_parts(parts, buf, start, stat=None):
    if stat is None:
        stat = [0, 0, 0]
    blocks = []
    pos = start
    for part in parts:
        if isinstance(part, int):  # random-looking data, stored in 1 MB pieces
            for off in range(0, part, 1 << 20):
                piece = buf[pos + off:pos + min(part, off + (1 << 20))]
                blocks.append((0, len(piece), piece))
            pos += part
            continue
        for i in range(0, len(part), BLOCK_TOKENS):
            tk = part[i:i + BLOCK_TOKENS]
            raw, payload = _code_block(tk, buf, pos, stat)
            if payload is None or len(payload) >= raw:  # didn't shrink: store it
                blocks.append((0, raw, buf[pos:pos + raw]))
            else:
                blocks.append((1, raw, payload))
            pos += raw
    return blocks


def _lz_job(job):
    return build_blocks(*job)


def _worker_init(parse=None):
    global PARSE
    if parse:
        PARSE = parse             # also reaches workers started by spawn, not just fork
    import signal
    signal.signal(signal.SIGINT, signal.SIG_IGN)  # Ctrl-C is handled by the parent


def mem_available():
    """Bytes of free memory the OS reports (Linux/Android/Termux), or None if unknown."""
    try:
        with open("/proc/meminfo") as fh:
            for line in fh:
                if line.startswith("MemAvailable:"):
                    return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    return None


def worker_bytes(hist):
    """Peak memory of one match-finder worker whose buffer holds `hist` bytes of history plus one
    segment (measured): the interpreter (~70 MB), the C tables (about 250 MB: tree nodes, hash heads,
    chain ring), the buffer itself (here and in the parent, which keeps the job), and 8 bytes per
    byte of the segment for the token array."""
    return (320 << 20) + 2 * (hist + SEGMENT) + 8 * SEGMENT


def fit_segment(total, jobs):
    """Pick SEGMENT: just small enough to keep `jobs` workers busy, never below SEGMENT_MIN or
    above SEGMENT_MAX. Every extra segment boundary costs ratio (the next segment sees the
    history only sparsely and restarts its price model) and re-indexing time, so a machine with
    fewer cores than 4 MB pieces gets fewer, larger pieces: measured -0.5% size at equal CPU
    time on 19 MB with one core. With enough cores the pieces stay at 4 MB, so nothing is lost."""
    global SEGMENT
    per_core = -(-total // max(1, jobs))
    SEGMENT = min(max(SEGMENT_MIN, (per_core + (1 << 20) - 1) >> 20 << 20), SEGMENT_MAX)
    return SEGMENT


def auto_jobs(total=None):
    """Default number of worker processes: the cores (at most 8), but no more than fit in free
    memory (60% of it, so the phone doesn't start swapping)."""
    n = min(8, os.cpu_count() or 1)
    if total is not None and total <= SEGMENT:
        return 1                    # one segment: nothing to run in parallel
    avail = mem_available()
    if avail:
        hist = min(total if total is not None else WINDOW, WINDOW)
        n = min(n, int(avail * 0.6) // worker_bytes(hist))
    return max(1, n)


def _mp3_job(path, size):
    """Worker process: read, recompress and verify one MP3. -> (crc, blob) | None | 'changed'."""
    try:
        with open(path, "rb") as fh:
            data = fh.read(size + 1)
    except OSError:
        return None
    if len(data) != size:
        return "changed"
    blob = mp3_pack(data)
    return None if blob is None else (zlib.crc32(data), blob)


class Mp3Ahead:
    """Packs upcoming MP3 files on several cores while the main process feeds the stream.
    Files are taken in order; at most 2 x workers results wait in memory."""
    def __init__(self, entries, workers):
        import concurrent.futures as cf
        import multiprocessing as mp
        self.todo = collections.deque(e for e in entries if mp3_eligible(e))
        self.futs = {}
        self.workers = workers
        try:
            ctx = mp.get_context("fork")
        except ValueError:
            ctx = None
        self.pool = cf.ProcessPoolExecutor(max_workers=workers, mp_context=ctx,
                                           initializer=_worker_init)
        self.prime()

    def prime(self):
        while self.todo and len(self.futs) < 2 * self.workers and self.pool:
            e = self.todo.popleft()
            try:
                self.futs[id(e)] = self.pool.submit(_mp3_job, e["full"], e["size"])
            except Exception:
                self.pool = None

    def take(self, e):
        """-> (crc, blob) | None | 'changed'; 'miss' when this file wasn't queued/failed."""
        fut = self.futs.pop(id(e), None)
        if fut is None:
            return "miss"
        try:
            r = fut.result()
        except Exception:
            r = "miss"
        self.prime()
        return r

    def close(self):
        if self.pool:
            self.pool.shutdown(wait=False, cancel_futures=True)
            self.pool = None
        self.futs.clear()


_MP3_AHEAD = None


def mp3_eligible(e):
    return e["ext"] == ".mp3" and MP3_MIN <= e["size"] <= MP3_MAX


def start_mp3_ahead(entries, cap=None):
    """Start the MP3 worker pool for this pack (if several cores and enough memory)."""
    global _MP3_AHEAD
    stop_mp3_ahead()
    mp3s = [e for e in entries if mp3_eligible(e)]
    force = int(os.environ.get("KILN_MP3_JOBS", "0") or 0)     # testing / manual override
    if len(mp3s) < 2 or ((os.cpu_count() or 1) < 2 and not force):
        return
    n = force or min(os.cpu_count() or 1, 8, cap or 8, len(mp3s))
    n = min(n, len(mp3s))
    avail = mem_available()
    if avail:
        per = (96 << 20) + 14 * max(e["size"] for e in mp3s)
        n = min(n, int(avail * 0.5) // per)
    if n < 2:
        return
    try:
        _MP3_AHEAD = Mp3Ahead(entries, n)
    except Exception:
        _MP3_AHEAD = None


def stop_mp3_ahead():
    global _MP3_AHEAD
    if _MP3_AHEAD:
        _MP3_AHEAD.close()
    _MP3_AHEAD = None


import atexit
atexit.register(stop_mp3_ahead)


POOL_ERROR = ""          # why make_pool couldn't start worker processes (shown in the "[N cores]" note)


def make_pool(jobs):
    """A pool of worker processes, or None (the Encoder then tries threads, then runs serially)."""
    global POOL_ERROR
    POOL_ERROR = ""
    if jobs <= 1:
        return None
    try:
        import concurrent.futures as cf
        import multiprocessing as mp
        try:
            ctx = mp.get_context("fork")
        except ValueError:
            ctx = None
        return cf.ProcessPoolExecutor(max_workers=jobs, mp_context=ctx,
                                      initializer=_worker_init, initargs=(PARSE,))
    except Exception as e:
        POOL_ERROR = repr(e)[:80]
        return None


def make_thread_pool(jobs):
    """Fallback when worker processes can't be used (an app can't always fork): the heavy work runs in the
    C helper through ctypes, which releases the GIL, so threads still use several cores."""
    import concurrent.futures as cf
    # the helper fills a few static tables on first use: do that once, here, before threads race on it
    build_blocks(bytes(range(256)) * 8 + bytes(1024), 0)
    return cf.ThreadPoolExecutor(max_workers=jobs)


class Encoder:
    """Streaming encoder: feed() data, finish() when done.
    With a pool, segments are compressed in parallel but written in order."""
    def __init__(self, sink, on_bytes=None, pool=None, jobs=1, store=False):
        self.store = store         # True: write every byte as a stored block (no compression)
        self.sink = sink
        self.on_bytes = on_bytes
        self.pool = pool
        self.max_inflight = max(jobs, 1) + 1
        self.futs = collections.deque()
        self.par_done = 0          # segments finished by worker processes
        self.ser_done = 0          # segments done in this process
        self.fail = None           # why the worker processes weren't used, if they weren't
        self.jobs = max(jobs, 1)
        self.threads = False       # True: segments run on a thread pool (process pool unavailable)
        if pool is None and self.jobs > 1 and not store and POOL_ERROR:
            self._to_threads(POOL_ERROR)
        self.pending = bytearray()
        self.history = b""
        self.since = 0             # bytes fed since the last checkpoint
        self.ckpts = []            # new checkpoints: (file_index, stream_offset, record_counter)

    def checkpoint(self, file_index):
        """Reset the match history here (call only at a file start) and remember where
        this point is, so a reader can start decoding from it."""
        self.flush()
        self._drain()
        self.history = b""
        self.since = 0
        off, ctr = self.sink.pos()
        self.ckpts.append((file_index, off, ctr))

    def maybe_checkpoint(self, file_index):
        if self.since >= CKPT_SPAN:
            self.checkpoint(file_index)

    def _emit(self, btype, raw_len, payload):
        self.sink.write_record(struct.pack(">BII", btype, raw_len, len(payload)) + payload)

    def feed(self, data):
        self.since += len(data)
        self.pending += data
        if len(self.pending) >= SEGMENT:
            self._process()

    def feed_stored(self, data):
        """Already-compressed data: random-looking, so the match finder only probes
        content-defined anchors (cheap), which still finds exact copies of earlier data."""
        self.feed(data)

    def flush(self):
        if self.pending:
            self._process()

    def finish(self):
        self.flush()
        self._drain()
        if self.threads and self.pool is not None:
            self.pool.shutdown(wait=True)
        stop_mp3_ahead()
        self.sink.write_record(b"\x02", final=True)

    def _process(self):
        if self.store:
            data = bytes(self.pending)
            self.pending = bytearray()
            self._write(store_blocks(data, 0), len(data))
            return
        buf = bytes(self.history) + bytes(self.pending)
        job = (buf, len(self.history))
        n_new = len(self.pending)
        self.history = buf[-WINDOW:]
        self.pending = bytearray()
        fut = None
        if self.pool is not None:
            try:
                fut = self.pool.submit(_lz_job, job)
            except Exception as e:
                self._to_threads(repr(e)[:80])      # process pool unusable
                if self.pool is not None:
                    try:
                        fut = self.pool.submit(_lz_job, job)
                    except Exception:
                        self.pool = None            # carry on in this process
        if fut is None:
            self.ser_done += 1
            self._write(build_blocks(*job), n_new)
            return
        self.futs.append((fut, job, n_new))
        while len(self.futs) >= self.max_inflight:
            self._drain_one()

    def _drain_one(self):
        fut, job, n_new = self.futs.popleft()
        try:
            blocks = fut.result()
            self.par_done += 1
        except KilnError:
            raise
        except Exception as e:  # a worker died: redo this segment here, later ones go to threads
            self._to_threads(repr(e)[:80])
            self.ser_done += 1
            blocks = build_blocks(*job)
        self._write(blocks, n_new)

    def _drain(self):
        while self.futs:
            self._drain_one()

    def _to_threads(self, reason):
        """Worker processes failed: remember why and switch to a thread pool (or serial if that fails too)."""
        self.fail = self.fail or reason
        if self.threads:                 # the thread pool itself failed: serial from here on
            self.pool = None
            return
        try:
            self.pool = make_thread_pool(self.jobs)
            self.threads = True
        except Exception as e:
            self.pool = None
            self.fail += f"; threads: {repr(e)[:40]}"

    def _write(self, blocks, n_new):
        for btype, raw, payload in blocks:
            self._emit(btype, raw, payload)
        if self.on_bytes:
            self.on_bytes(n_new)


def read_stream(src):
    """Generator: yields decoded chunks until the end-of-stream marker."""
    out = bytearray()
    mark = 0
    while True:
        rec = src.read_record()
        if not rec:
            raise KilnError("corrupt data: empty record")
        t = rec[0]
        if t == 2:
            return
        if t not in (0, 1) or len(rec) < 9:
            raise KilnError("corrupt data: unknown block type")
        raw_len, comp_len = struct.unpack(">II", rec[1:9])
        payload = rec[9:]
        if len(payload) != comp_len:
            raise KilnError("corrupt data: block length mismatch")
        if t == 0:
            if comp_len != raw_len:
                raise KilnError("corrupt data: bad stored block")
            out += payload
        else:
            decode_block(payload, raw_len, out)
        yield bytes(out[mark:])
        if len(out) > 2 * WINDOW:
            del out[:len(out) - WINDOW]
        mark = len(out)


# ---------------------------------------------------------------- archive
def collect(paths, exclude=()):
    files = []
    for p in paths:
        if os.path.isdir(p):
            base = os.path.dirname(os.path.abspath(p))
            for root, dirs, names in os.walk(p):
                dirs.sort()
                for name in sorted(names):
                    full = os.path.join(root, name)
                    files.append((full, os.path.relpath(os.path.abspath(full), base)))
        elif os.path.isfile(p):
            files.append((p, os.path.basename(p)))
        else:
            raise KilnError(f"no such file or directory: {p}")
    out, seen = [], set()
    for full, rel in files:
        if os.path.abspath(full) in exclude:
            continue
        rel = rel.replace(os.sep, "/")
        if rel in seen:
            raise KilnError(f"two inputs would both be stored as '{rel}'")
        seen.add(rel)
        try:
            st = os.stat(full)
        except OSError as e:
            print(f"warning: skipping {full}: {e.strerror}", file=sys.stderr)
            continue
        ext = os.path.splitext(rel)[1].lower()
        out.append({"full": full, "rel": rel, "size": st.st_size,
                    "mtime": max(int(st.st_mtime), 0), "mode": st.st_mode & 0o7777,
                    "pre": ext in PRECOMPRESSED, "ext": ext})
    # similar files next to each other compress better; precompressed go last
    out.sort(key=lambda e: (e["pre"], e["ext"], e["rel"]))
    return out


def try_mp3(enc, e):
    """MP3 files are recompressed losslessly. -> (size, crc, MP3_FID, packed_len) when the
    packed form was used (it is decoded again and compared before it is accepted), else None."""
    if not MP3_MIN <= e["size"] <= MP3_MAX:
        return None
    if _MP3_AHEAD is not None:
        r = _MP3_AHEAD.take(e)
        if r == "changed":
            raise KilnError(f"{e['full']} changed while it was being read")
        if r != "miss":
            if r is None:
                return None
            enc.feed_stored(r[1])
            return e["size"], r[0], MP3_FID, len(r[1])
    try:
        with open(e["full"], "rb") as fh:
            data = fh.read(e["size"] + 1)
    except OSError:
        return None
    if len(data) != e["size"]:
        raise KilnError(f"{e['full']} changed while it was being read")
    blob = mp3_pack(data)
    if blob is None:
        return None
    enc.feed_stored(blob)
    return len(data), zlib.crc32(data), MP3_FID, len(blob)


def pack_file(enc, e):
    """Reads e['full'] into the encoder (WAV-filtered / MP3-recompressed when that helps).
    Returns (size, crc, filter_id, stream_len); the CRC covers the original bytes."""
    if e["ext"] == ".mp3" and not enc.store:
        r = try_mp3(enc, e)
        if r:
            return r
    filt = (pick_wav_filter(e["full"], e["size"])
            if e["ext"] == ".wav" and not e["pre"] and not enc.store else 0)
    if not filt and not e["pre"] and not enc.store:
        filt = pick_bcj(e["full"], e["size"])
    flt = make_filter(filt, e["size"]) if filt else None
    if e["ext"] == ".wav" and not filt and e["size"] >= WAV_MIN and not enc.store:
        print(f"note: WAV filter not used for {os.path.basename(e['full'])}: "
              + wav_why(e["full"], e["size"]), file=sys.stderr)
    feed = enc.feed_stored if e["pre"] else enc.feed
    crc = size = 0
    with open(e["full"], "rb") as fh:
        while True:
            chunk = fh.read(CHUNK)
            if not chunk:
                break
            crc = zlib.crc32(chunk, crc)
            size += len(chunk)
            data = flt.feed(chunk) if flt else chunk
            if data:
                feed(data)
    if flt:
        if size != e["size"]:
            raise KilnError(f"{e['full']} changed while it was being read")
        tail = flt.finish()
        if tail:
            feed(tail)
    return size, crc, filt, size


class Table(list):
    """The file table: a list of entries, plus .ckpts = [(file_index, stream_offset, counter)]."""
    ckpts = ()


def find_ckpt(table, idx, hdr_len):
    """The latest checkpoint at or before file idx: (file_index, stream_offset, counter)."""
    best = (0, hdr_len, 0)
    for c in getattr(table, "ckpts", ()):
        if c[0] <= idx:
            best = c
    return best


def build_table_bytes(table, ckpts=None):
    ckpts = list(getattr(table, "ckpts", ()) if ckpts is None else ckpts)
    parts = [struct.pack(">I", len(table))]
    for e in table:
        nb = e["rel"].encode("utf-8")
        filt, dead = e.get("filt", 0), bool(e.get("dead"))
        ext = filt > 11 or dead            # extended entry: 2 more bytes (bit 15 = hidden)
        parts.append(struct.pack(">H", len(nb)) + nb +
                     struct.pack(">QIQH", e["size"], e["crc"], e["mtime"],
                                 (e["mode"] & 0o7777) | ((15 if ext else filt) << 12))
                     + (struct.pack(">H", filt | (0x8000 if dead else 0)) if ext else b"")
                     + (struct.pack(">Q", slen(e)) if filt == MP3_FID else b""))
    if ckpts:   # optional trailer: older kilns stop after the entries and just read sequentially
        parts.append(b"CKPT" + struct.pack(">I", len(ckpts)))
        parts += [struct.pack(">IQQ", *c) for c in ckpts]
    return b"".join(parts)


def parse_table(raw, hdr_len=0, table_off=None):
    try:
        (count,) = struct.unpack_from(">I", raw, 0)
        pos, table = 4, Table()
        for _ in range(count):
            (nlen,) = struct.unpack_from(">H", raw, pos)
            pos += 2
            name = raw[pos:pos + nlen].decode("utf-8")
            pos += nlen
            size, crc, mtime, mode = struct.unpack_from(">QIQH", raw, pos)
            pos += 22
            filt, dead = mode >> 12, False
            if filt == 15:                  # extended entry: filter id + hidden flag
                (v,) = struct.unpack_from(">H", raw, pos)
                pos += 2
                dead, filt = bool(v & 0x8000), v & 0x7FFF
                if (filt > 11 and wav_spec(filt) is None and filt != MP3_FID and filt not in BCJ_FIDS) or (filt <= 11 and not dead):
                    raise KilnError("archive uses features from a newer kiln")
            elif filt > 11:
                raise KilnError("archive uses features from a newer kiln")
            sl = size
            if filt == MP3_FID:             # packed MP3: the stream holds fewer bytes than the file
                (sl,) = struct.unpack_from(">Q", raw, pos)
                pos += 8
            table.append({"rel": name, "size": size, "crc": crc, "mtime": mtime,
                          "mode": mode & 0o7777, "filt": filt, "dead": dead, "slen": sl})
        if raw[pos:pos + 4] == b"CKPT":
            (nc,) = struct.unpack_from(">I", raw, pos + 4)
            pos += 8
            ck, prev = [], (0, hdr_len - 1, 0)
            for _ in range(nc):
                c = struct.unpack_from(">IQQ", raw, pos)
                pos += 20
                if not (0 < c[0] < count and c[1] > prev[1] and c[2] >= prev[2]
                        and (table_off is None or c[1] < table_off)):
                    raise KilnError("corrupt data: bad checkpoint list")
                ck.append(c)
                prev = c
            table.ckpts = tuple(ck)
        return table
    except (struct.error, UnicodeDecodeError):
        raise KilnError("corrupt data: bad file table")


def open_archive(path, password=None):
    """Returns (file, table, table_offset, header_length, cipher_or_None)."""
    try:
        f = open(path, "rb")
    except OSError as e:
        raise KilnError(f"cannot open {path}: {e.strerror}")
    try:
        head = f.read(5)
        if head[:4] != MAGIC or len(head) < 5:
            raise KilnError("not a kiln archive (expected format KLNA)")
        flags = head[4]
        if flags & ~FLAG_ENCRYPTED:
            raise KilnError("archive uses features from a newer kiln")
        cipher = None
        if flags & FLAG_ENCRYPTED:
            extra = f.read(19)
            if len(extra) < 19:
                raise KilnError("archive is truncated")
            head += extra
            log_n, r, p = extra[0], extra[1], extra[2]
            if not (10 <= log_n <= 20 and 1 <= r <= 16 and 1 <= p <= 4):
                raise KilnError("corrupt data: bad key-derivation settings")
            key = derive_key(password if password is not None else get_password(),
                             extra[3:], log_n, r, p)
            cipher = Cipher(key, head)
        hdr_len = len(head)
        size = f.seek(0, 2)
        if size < hdr_len + FOOTER_LEN:
            raise KilnError("archive is truncated")
        f.seek(size - FOOTER_LEN)
        table_off, total, magic = struct.unpack(">QQ4s", f.read(FOOTER_LEN))
        if magic != MAGIC or not (hdr_len <= table_off <= size - FOOTER_LEN):
            raise KilnError("archive is truncated or damaged (missing footer)")
        f.seek(table_off)
        src = cipher.source(f, 1) if cipher else PlainSource(f)
        table = parse_table(b"".join(read_stream(src)), hdr_len, table_off)
        if not cipher and sum(e["size"] for e in table) != total:
            raise KilnError("corrupt data: file table doesn't match footer")
        return f, table, table_off, hdr_len, cipher
    except BaseException:
        f.close()
        raise


def core_note(enc, jobs):
    """' [4 cores]' style note for summary lines."""
    if not (enc.par_done or enc.ser_done):
        return ""
    if enc.threads and enc.par_done:
        return f" [{jobs} threads - worker processes failed: {enc.fail}]"
    cores = (f"{jobs} cores" if enc.par_done and not enc.ser_done else
             "1 core" if not enc.par_done else f"{jobs} cores, partly serial")
    return f" [{cores}]" if not enc.fail else f" [1 core - parallel failed: {enc.fail}]"


INDEX_NAME = "_index.kln"
INDEX_FILE = "index.json"


def write_index(out_dir, mapping, args, pw):
    """Pack the index as the encrypted index archive in out_dir.
    mapping = {archive name: {"path": original path, "size": bytes, "mtime": epoch}}."""
    tmp = tempfile.mkdtemp(prefix="kiln-")
    try:
        p = os.path.join(tmp, INDEX_FILE)
        fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as fh:
            fh.write(json.dumps({"v": 2, "files": mapping}, sort_keys=True).encode("utf-8"))
        sub = argparse.Namespace(**vars(args))
        sub.individual = sub.random_names = False
        sub.archive, sub.inputs = os.path.join(out_dir, INDEX_NAME), [p]
        sub.force, sub.quiet, sub._pw, sub.encrypt = True, True, pw, True
        import contextlib, io
        with contextlib.redirect_stdout(io.StringIO()):
            cmd_pack(sub)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def read_index_meta(out_dir, pw):
    """-> {archive name: {"path", "size", "mtime"}}; size/mtime are None in an old (v1) index.
    A wrong password fails here, before any file work."""
    tmp = tempfile.mkdtemp(prefix="kiln-")
    try:
        a = argparse.Namespace(archive=os.path.join(out_dir, INDEX_NAME), output=tmp, members=[],
                               force=True, verbose=False, quiet=True, _pw=pw, _silent=True)
        run_extract(a, True)
        with open(os.path.join(tmp, INDEX_FILE), "rb") as fh:
            data = json.loads(fh.read().decode("utf-8"))
    except (OSError, ValueError):
        raise KilnError("the index archive is damaged or isn't a kiln index")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    files = data.get("files") if isinstance(data, dict) else None
    ver = data.get("v") if isinstance(data, dict) else None
    out = {}
    if ver in (1, 2) and isinstance(files, dict):
        for k, v in files.items():
            if ver == 1 and isinstance(v, str):
                out[k] = {"path": v, "size": None, "mtime": None}
            elif (ver == 2 and isinstance(v, dict) and isinstance(v.get("path"), str)
                  and isinstance(v.get("size"), int) and isinstance(v.get("mtime"), int)):
                out[k] = {"path": v["path"], "size": v["size"], "mtime": v["mtime"]}
            else:
                out = None
                break
    else:
        out = None
    if out is None or not all(isinstance(k, str) for k in out):
        raise KilnError("the index archive has an unknown layout")
    return out


def read_index(out_dir, pw):
    """-> {archive name: original path}"""
    return {n: m["path"] for n, m in read_index_meta(out_dir, pw).items()}


def extract_one(src, out_dir, pw, write=True, name=None):
    if write and name:
        # the archive holds the file under the name it had when it was packed; a file that
        # was renamed (mv) in the index must come out under its current name
        tmp = tempfile.mkdtemp(prefix=".kiln-", dir=out_dir)
        try:
            extract_one(src, tmp, pw, True)
            files = os.listdir(tmp)
            if len(files) != 1:
                raise KilnError("unexpected layout in a set archive")
            os.replace(os.path.join(tmp, files[0]), os.path.join(out_dir, name))
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
        return
    """Extract (or just verify) one archive of a random-name set into out_dir. The checksums
    are always checked; a wrong password or a damaged file raises KilnError."""
    a = argparse.Namespace(archive=src, output=out_dir, members=[],
                           force=True, verbose=False, quiet=True, _pw=pw, _silent=True)
    run_extract(a, write)


def _is_under(root, dest):
    """True when dest is strictly below root (works when root is "/" too)."""
    return dest.startswith(root.rstrip(os.sep) + os.sep)


def safe_dest(root, rel):
    """Where rel goes below root; refuses paths that would escape it."""
    dest = os.path.abspath(os.path.join(root, *rel.split("/")))
    if not _is_under(root, dest) or os.path.basename(dest) != rel.split("/")[-1]:
        raise KilnError(f"refusing unsafe path: {rel}")
    return dest


def run_extract_dir(args, write):
    """unpack/test on a -r output directory: the index says where every file belongs."""
    d = args.archive
    if not os.path.isfile(os.path.join(d, INDEX_NAME)):
        raise KilnError(f"{d} is a directory without {INDEX_NAME} (not a kiln random-name set)")
    pw = get_password()
    idx = read_index(d, pw)
    pats = getattr(args, "members", None) or []
    items = sorted(idx.items(), key=lambda kv: kv[1])
    if pats:
        items = [(n, r) for n, r in items if any(fnmatch.fnmatch(r, p) or r == p for p in pats)]
        if not items:
            raise KilnError("no files in the index match: " + ", ".join(pats))
    root = os.path.abspath(getattr(args, "output", "."))
    inplace = write and getattr(args, "in_place", False)
    strip = ""
    if inplace:
        if pats:
            raise KilnError("--in-place extracts everything; it can't be combined with member names")
        root = os.path.abspath(d)
        tops = {r.split("/")[0] for _, r in items}
        if len(tops) == 1 and all("/" in r for _, r in items):
            strip = tops.pop() + "/"
    plan, missing, clashes = [], [], []
    for n, rel in items:
        src = os.path.join(d, n)
        dest = safe_dest(root, rel[len(strip):])
        if not os.path.isfile(src):
            missing.append(rel)
            continue
        if write and os.path.exists(dest):
            clashes.append(rel)
        plan.append((src, dest, rel))
    if clashes and not args.force:
        shown = "\n  ".join(clashes[:10])
        more = f"\n  ... and {len(clashes) - 10} more" if len(clashes) > 10 else ""
        raise KilnError(f"{len(clashes)} file(s) already exist:\n  {shown}{more}\n"
                        "nothing was extracted; use -f to overwrite")
    t0 = time.time()
    for src, dest, rel in plan:
        if write:
            os.makedirs(os.path.dirname(dest), exist_ok=True)
        try:
            extract_one(src, os.path.dirname(dest), pw, write, os.path.basename(dest))
        except KilnError as err:
            raise KilnError(f"{rel} ({os.path.basename(src)}): {err}")
        if not args.quiet:
            print(f"  {rel}")
    verb = "extracted" if write else "verified"
    print(f"{verb} {len(plan)} file(s) from {d} in {time.time() - t0:.1f}s - all checksums OK")
    if inplace and not missing:
        # every file extracted and verified: now the encrypted copies can go
        for src, _, _ in plan:
            os.remove(src)
        os.remove(os.path.join(d, INDEX_NAME))
        print(f"removed {len(plan) + 1} encrypted archive file(s) from {d}")
    if missing:
        shown = "\n  ".join(missing[:10])
        raise KilnError(f"{len(missing)} archive(s) listed in the index are missing:\n  {shown}")


def list_dir(args):
    d = args.archive
    if not os.path.isfile(os.path.join(d, INDEX_NAME)):
        raise KilnError(f"{d} is a directory without {INDEX_NAME} (not a kiln random-name set)")
    idx = read_index(d, get_password())
    total = 0
    for n, rel in sorted(idx.items(), key=lambda kv: kv[1]):
        p = os.path.join(d, n)
        size = os.path.getsize(p) if os.path.isfile(p) else None
        total += size or 0
        print(f"{size if size is not None else 'MISSING':>12}  {n}  {rel}")
    print(f"{total:>12}  total archive bytes, {len(idx)} file(s) [encrypted index]")


def check_set_target(out_dir):
    """A -r output folder must be a set itself, new/empty, and not inside another set."""
    d = os.path.abspath(out_dir)
    if os.path.isfile(os.path.join(d, INDEX_NAME)):
        return
    p = os.path.dirname(d)
    while True:
        if os.path.isfile(os.path.join(p, INDEX_NAME)):
            raise KilnError(
                f"{out_dir} is inside the kiln set {p}. A set inside a set is invisible to the "
                f"outer one.\n  To add files to that set, pack into {p} itself; to keep them "
                f"separate, use a folder outside it.")
        q = os.path.dirname(p)
        if q == p:
            break
        p = q
    if os.path.isdir(d) and os.listdir(d):
        raise KilnError(f"{out_dir} isn't empty and isn't a kiln set (no {INDEX_NAME}). "
                        "Use a new or empty folder.")


def cmd_pack_each(args):
    """kiln pack -i: one archive per file under the output directory (the first argument).
    Without -e the folder layout is mirrored (photos/a.jpg -> OUT/photos/a.jpg.kln); with -e the
    files get random names in one flat directory and an encrypted _index.kln maps them back.
    One password prompt;
    every file still gets its own salt and key, so nonces are never reused across archives."""
    if getattr(args, "store", False) and not args.encrypt:
        raise KilnError("-s/--store is the encryption-only level and needs -e")
    rnd = bool(args.encrypt)  # -i with -e: random names + encrypted index (always)
    out_dir = args.archive
    if os.path.exists(out_dir) and not os.path.isdir(out_dir):
        raise KilnError(f"{out_dir} is not a directory (with -i the first argument is the "
                        "output directory)")
    if rnd:
        check_set_target(out_dir)
    out_abs = os.path.abspath(out_dir) + os.sep
    files = [e for e in collect(args.inputs)
             if not os.path.abspath(e["full"]).startswith(out_abs)]
    if not files:
        raise KilnError("nothing to pack")
    todo, skipped, mapping, pw = [], 0, {}, None
    if rnd:
        # random names: out_dir/<hex>.kln, and the name -> path map lives in _index.kln.
        # An existing index is reused so reruns resume and never orphan files.
        have = os.path.isfile(os.path.join(out_dir, INDEX_NAME))
        pw = get_password(confirm=not have)
        old = read_index_meta(out_dir, pw) if have else {}
        by_rel = {m["path"]: n for n, m in old.items()}
        mapping = dict(old)
        for e in files:
            name = by_rel.get(e["rel"])
            if name is None:
                while True:
                    name = secrets.token_hex(8) + ".kln"
                    if name not in mapping:
                        break
            dest = os.path.join(out_dir, name)
            fresh = {"path": e["rel"], "size": e["size"], "mtime": e["mtime"]}
            if os.path.exists(dest) and not args.force:
                skipped += 1
                # an archive that is kept keeps its recorded size/date; an old (v1) index
                # has none, so it gets them from the file as it is now
                if name in old and old[name]["size"] is not None:
                    fresh = old[name]
            else:
                todo.append((e, dest))
            mapping[name] = fresh
        if not todo and mapping == old:
            print(f"nothing to do: all {skipped} output(s) already exist (use -f to overwrite)")
            return
        secrets.SystemRandom().shuffle(todo)   # don't let timestamps reveal the original order
        os.makedirs(out_dir, exist_ok=True)
        write_index(out_dir, mapping, args, pw)
    else:
        for e in files:
            dest = os.path.join(out_dir, *e["rel"].split("/")) + ".kln"
            if os.path.exists(dest) and not args.force:
                skipped += 1
            else:
                todo.append((e, dest))
        if not todo:
            print(f"nothing to do: all {skipped} output(s) already exist (use -f to overwrite)")
            return
        pw = get_password(confirm=True) if args.encrypt else None
    if rnd and not todo:
        print(f"index updated: {len(mapping)} entr(ies) in {out_dir}/{INDEX_NAME}"
              f" (sizes and dates added); {skipped} archive(s) already exist")
        return
    sub = argparse.Namespace(**vars(args))
    sub.individual, sub.random_names, sub._each, sub._pw, sub.force = False, False, True, pw, True
    sub.quiet = args.quiet or len(todo) > 1
    t0 = time.time()
    raw = packed = 0
    for e, dest in todo:
        os.makedirs(os.path.dirname(os.path.abspath(dest)), exist_ok=True)
        sub.archive, sub.inputs = dest, [e["full"]]
        try:
            cmd_pack(sub)
        except KilnError as err:
            raise KilnError(f"{e['rel']}: {err}")
        a = os.path.getsize(dest)
        raw += e["size"]
        packed += a
        if not args.quiet:
            print(f"  {e['rel']}: {fmt_size(e['size'])} -> {fmt_size(a)}")
    ratio = 100 * packed / raw if raw else 100
    print(f"{len(todo)} archive(s) in {out_dir}: {fmt_size(raw)} -> {fmt_size(packed)} "
          f"({ratio:.1f}% of original) in {time.time() - t0:.1f}s"
          + (" [encrypted: AES-256-GCM]" if args.encrypt else "")
          + (" [no compression]" if args.store else "")
          + (f" [random names, index: {INDEX_NAME}]" if rnd else "")
          + (f", {skipped} skipped (already exist)" if skipped else ""))


def cmd_pack(args):
    if getattr(args, "random_names", False):
        raise KilnError("-r was removed: use -e -i for one encrypted archive per file "
                        "with random names (plain -i keeps real names)")
    if getattr(args, "individual", False):
        return cmd_pack_each(args)
    store = getattr(args, "store", False)
    if store and not args.encrypt:
        raise KilnError("-s/--store is the encryption-only level and needs -e "
                        "(without encryption, storing without compression is pointless)")
    if os.path.exists(args.archive) and not args.force:
        raise KilnError(f"{args.archive} already exists (use -f to overwrite)")
    cipher = None
    header = MAGIC + b"\x00"
    if args.encrypt:
        pw = getattr(args, "_pw", None) or get_password(confirm=True)
        salt = os.urandom(16)
        header = MAGIC + bytes([FLAG_ENCRYPTED, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P]) + salt
        if not args.quiet:
            print("deriving key...", file=sys.stderr)
        cipher = Cipher(derive_key(pw, salt, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P), header)
        del pw
    tmp = args.archive + ".tmp"
    files = collect(args.inputs, {os.path.abspath(args.archive), os.path.abspath(tmp)})
    if not files:
        raise KilnError("nothing to pack")
    total = sum(e["size"] for e in files)
    # Every segment re-indexes the match history, so fewer, bigger segments are faster and
    # smaller whenever there are fewer cores than segments (see fit_segment).
    fit_segment(total, 1 if store else (args.jobs or min(8, os.cpu_count() or 1)))
    prog = Progress(total, "packing", not args.quiet)
    t0 = time.time()
    table = []
    jobs = 1 if store else (args.jobs or auto_jobs(total))
    if not store and not args.jobs and not args.quiet and total > SEGMENT:
        cores = min(8, os.cpu_count() or 1)
        if jobs < cores:
            print(f"using {jobs} of {cores} cores: free memory fits about {jobs} workers "
                  f"(close other apps or use -j N to change this)", file=sys.stderr)
    pool = None if store else make_pool(jobs)
    if not store:
        start_mp3_ahead(files, args.jobs)
    try:
        with open(tmp, "wb") as out:
            if cipher:
                try:
                    os.chmod(tmp, 0o600)
                except OSError:
                    pass
            out.write(header)
            enc = Encoder(cipher.sink(out, 0) if cipher else PlainSink(out),
                          prog.add, pool, jobs, store=store)
            for e in files:
                try:
                    fh = open(e["full"], "rb")
                except OSError as err:
                    prog.close()
                    print(f"warning: skipping {e['full']}: {err.strerror}", file=sys.stderr)
                    continue
                fh.close()
                enc.maybe_checkpoint(len(table))
                file_started(e["rel"], e["size"])
                size, crc, filt, sl = pack_file(enc, e)
                table.append(dict(e, size=size, crc=crc, filt=filt, slen=sl))
            enc.finish()
            table_off = out.tell()
            tenc = Encoder(cipher.sink(out, 1) if cipher else PlainSink(out))
            tenc.feed(build_table_bytes(table, enc.ckpts))
            tenc.finish()
            # encrypted archives don't reveal the total size in the footer
            out.write(struct.pack(">QQ4s", table_off,
                                  0 if cipher else sum(e["size"] for e in table), MAGIC))
        os.replace(tmp, args.archive)
        files_done([t["full"] for t in table])
    except BaseException:
        prog.close()
        if pool:
            (pool.shutdown(wait=False, cancel_futures=True), stop_mp3_ahead())
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise
    if pool:
        pool.shutdown()
    prog.close()
    note = " [no compression]" if store else core_note(enc, jobs)
    raw = sum(e["size"] for e in table)
    asize = os.path.getsize(args.archive)
    ratio = 100 * asize / raw if raw else 100
    if not getattr(args, "_each", False):
        print(f"{len(table)} file(s): {fmt_size(raw)} -> {fmt_size(asize)} "
              f"({ratio:.1f}% of original) in {time.time() - t0:.1f}s"
              + (" [encrypted: AES-256-GCM]" if cipher else "") + note)


class Extractor:
    """Splits the decoded stream back into files, checking each CRC."""
    def __init__(self, table, root, wanted, write, verbose, prog):
        self.table, self.root, self.wanted = table, root, wanted
        self.write, self.verbose, self.prog = write, verbose, prog
        self.i = self.got = self.crc = 0
        self.fh = None
        self.dest = None
        self.created = []         # files this run made that did not exist before
        self.made = []            # folders this run made, outermost first
        self._advance()

    def _begin(self):
        e = self.table[self.i]
        self.crc = self.got = 0
        self.unf = make_unfilter(e)
        if self.write and self.wanted[self.i]:
            self.dest = os.path.abspath(os.path.join(self.root, e["rel"]))
            d, missing = os.path.dirname(self.dest), []
            while d and not os.path.isdir(d) and os.path.dirname(d) != d:
                missing.append(d)
                d = os.path.dirname(d)
            os.makedirs(os.path.dirname(self.dest), exist_ok=True)
            self.made.extend(reversed(missing))
            file_started(e["rel"], slen(e))
            existed = os.path.lexists(self.dest)
            self.fh = open(self.dest, "wb")
            if not existed:
                self.created.append(self.dest)

    def _end(self):
        e = self.table[self.i]
        if self.fh:
            self.fh.close()
            self.fh = None
        if self.crc != e["crc"] and not e.get("dead"):
            if self.dest:
                try:
                    os.remove(self.dest)
                except OSError:
                    pass
            raise KilnError(f"checksum failed for {e['rel']}")
        if self.dest:
            try:
                os.utime(self.dest, (e["mtime"], e["mtime"]))
                os.chmod(self.dest, e["mode"] & 0o777 or 0o644)
            except OSError:
                pass  # e.g. phone storage that doesn't allow it
            if self.verbose:
                print(f"  extracted {e['rel']} ({e['size']} bytes)")
            self.dest = None

    def _advance(self):
        while self.i < len(self.table):
            self._begin()
            if slen(self.table[self.i]) == 0:
                self._end()
                self.i += 1
            else:
                break

    def feed(self, data):
        self.prog.add(len(data))
        while data:
            if self.i >= len(self.table):
                raise KilnError("corrupt data: stream is longer than the file table")
            need = slen(self.table[self.i]) - self.got
            piece, data = data[:need], data[need:]
            self.got += len(piece)
            done = self.got == slen(self.table[self.i])
            if self.unf:
                piece = self.unf.feed(piece) + (self.unf.finish() if done else b"")
            self.crc = zlib.crc32(piece, self.crc)
            if self.fh:
                self.fh.write(piece)
            if done:
                self._end()
                self.i += 1
                self._advance()

    def finish(self):
        if self.i != len(self.table):
            raise KilnError("archive is truncated (data ends early)")

    def abort(self):
        """Stop. A file that was only half written is closed and deleted, never left behind."""
        if self.fh:
            try:
                self.fh.close()
            except OSError:
                pass
            self.fh = None
            if self.dest:
                try:
                    os.remove(self.dest)
                except OSError:
                    pass
                self.dest = None

    def rollback(self):
        """After a cancel: remove everything this run created (files that were already there
        and got replaced can't come back)."""
        self.abort()
        for p in reversed(self.created):
            try:
                os.remove(p)
            except OSError:
                pass
        for d in reversed(self.made):
            try:
                os.rmdir(d)               # only goes if it is empty now
            except OSError:
                pass
        self.created, self.made = [], []


def run_extract(args, write):
    f, table, table_off, hdr_len, cipher = open_archive(args.archive, getattr(args, "_pw", None))
    with f:
        root = os.path.abspath(getattr(args, "output", "."))
        names = getattr(args, "members", None) or []
        wanted = [not e.get("dead") and ((not names) or any(fnmatch.fnmatch(e["rel"], p) or e["rel"] == p
                                                              for p in names)) for e in table]
        if names and not any(wanted):
            raise KilnError("no files in the archive match: " + ", ".join(names))
        if write:
            clashes = []
            for e, w in zip(table, wanted):
                if not w:
                    continue
                dest = os.path.abspath(os.path.join(root, e["rel"]))
                if not _is_under(root, dest):
                    raise KilnError(f"refusing unsafe path: {e['rel']}")
                if os.path.exists(dest):
                    clashes.append(e["rel"])
            if clashes and not args.force:
                shown = "\n  ".join(clashes[:10])
                more = f"\n  ... and {len(clashes) - 10} more" if len(clashes) > 10 else ""
                raise KilnError(f"{len(clashes)} file(s) already exist:\n  {shown}{more}\n"
                                "nothing was extracted; use -f to overwrite")
        # with names given, start at the checkpoint before the first match and stop after the last
        lo, hi = ((wanted.index(True), len(wanted) - 1 - wanted[::-1].index(True))
                  if names else (0, len(table) - 1))
        k, off, ctr = find_ckpt(table, lo, hdr_len)
        part = names and (k > 0 or hi < len(table) - 1)
        total = sum(slen(e) for e in table[k:hi + 1]) if part else sum(slen(e) for e in table)
        prog = Progress(total, "unpacking" if write else "testing", not args.quiet)
        ext = (Extractor(table[k:], root, wanted[k:], write, getattr(args, "verbose", False), prog)
               if part else Extractor(table, root, wanted, write, getattr(args, "verbose", False), prog))
        t0 = time.time()
        try:
            f.seek(off)
            src = cipher.source(f, 0) if cipher else PlainSource(f)
            if ctr:
                src.n = ctr
            for chunk in read_stream(src):
                ext.feed(chunk)
                if part and ext.i > hi - k:
                    break
            if not part:
                ext.finish()
                if f.tell() != table_off:
                    raise KilnError("corrupt data: stream doesn't end where the table starts")
        except BaseException as ex:
            if not isinstance(ex, KilnError):      # cancelled or interrupted: leave nothing behind
                ext.rollback()
            raise
        finally:
            ext.abort()
            prog.close()
    n = sum(wanted)
    verb = "extracted" if write else "verified"
    if not getattr(args, "_silent", False):
        print(f"{verb} {n} file(s), {fmt_size(total)} in {time.time() - t0:.1f}s - all checksums OK")


def cmd_unpack(args):
    if os.path.isdir(args.archive):
        return run_extract_dir(args, True)
    run_extract(args, True)


def cmd_test(args):
    if os.path.isdir(args.archive):
        return run_extract_dir(args, False)
    run_extract(args, False)


def cmd_list(args):
    if os.path.isdir(args.archive):
        return list_dir(args)
    f, table, table_off, hdr_len, cipher = open_archive(args.archive)
    with f:
        live = [e for e in table if not e.get("dead")]
        for e in live:
            when = time.strftime("%Y-%m-%d %H:%M", time.localtime(e["mtime"]))
            print(f"{e['size']:>12}  {when}  {e['crc']:08x}  {e['rel']}")
        raw = sum(e["size"] for e in live)
        packed = table_off - hdr_len
        ratio = 100 * packed / raw if raw else 100
        print(f"{raw:>12}  total, {len(live)} file(s) -> {packed} bytes of data ({ratio:.1f}%)"
              + (" [encrypted]" if cipher else ""))
        if len(live) != len(table):
            print(f"({len(table) - len(live)} removed file(s) still stored; 'compact' in the shell reclaims the space)")



# ---------------------------------------------------------------- wavcheck / selftest
WAVINFO_FRAMES = 4096


def wav_report(path):
    """Text lines describing a WAV's samples: is it float, and is it really
    integer audio in disguise (which a future filter could exploit)?"""
    size = os.path.getsize(path)
    with open(path, "rb") as fh:
        head = fh.read(WAV_HEAD)
        lay = wav_layout(head, size)
        if lay is None or lay is _NEED:
            return [wav_why(path, size)]
        start, region, C, align, width, isfloat = lay
        tag, rate = struct.unpack_from("<H", head, head.index(b"fmt ") + 8)[0], \
            struct.unpack_from("<I", head, head.index(b"fmt ") + 12)[0]
        frames = region // align
        win = min(WAVINFO_FRAMES, frames // 4) * align      # small windows: this loop runs in Python
        wins = []
        for q in (1, 2, 3):
            fh.seek(start + (region * q // 4) // align * align)
            w = fh.read(win)
            if len(w) == win and win:
                wins.append(w)
    kind = "float" if isfloat else "PCM"
    out = [f"{kind}, {8 * width}-bit, {C} ch, {rate} Hz, {frames / max(rate, 1):.1f} s"]
    if not wins:
        return out + ["too short to analyse"]
    n = 0
    peak = 0.0
    if kind == "float" and width in (4, 8):
        code = "f" if width == 4 else "d"
        ex16 = ex24 = tz_sum = 0
        for w in wins:
            a = array.array(code)
            a.frombytes(w)
            for x in a:
                n += 1
                if x != x or x in (float("inf"), float("-inf")):
                    continue
                ax = abs(x)
                peak = max(peak, ax)
                v = x * 32768.0
                ex16 += v == math.floor(v)
                v = x * 8388608.0
                ex24 += v == math.floor(v)
            if width == 4:
                b = array.array("I")
                b.frombytes(w)
                for u in b:
                    m = u & 0x7FFFFF
                    tz_sum += 23 if m == 0 else (m & -m).bit_length() - 1
        out.append(f"peak level {peak:.3f} (1.0 = full scale), {n} samples checked")
        out.append(f"exact 16-bit multiples: {100 * ex16 / n:.1f}%   exact 24-bit multiples: {100 * ex24 / n:.1f}%")
        if width == 4:
            out.append(f"average unused low mantissa bits: {tz_sum / n:.1f} of 23")
        if ex16 >= 0.99 * n:
            out.append("verdict: 16-bit audio stored as float; kiln stores it as integers.")
        elif ex24 >= 0.99 * n:
            out.append("verdict: 24-bit audio stored as float; kiln stores it as integers.")
        else:
            out.append("verdict: full-precision float (processed, resampled or normalised).")
            out.append("The integer filter can't apply; expect roughly 75-85% with the byte-plane filter.")
    else:
        lo = {2: 16, 3: 24, 4: 32}.get(width, 0)
        if lo:
            ors = 0
            for w in wins:
                for i in range(0, len(w) - width + 1, width):
                    v = int.from_bytes(w[i:i + width], "little", signed=True)
                    n += 1
                    ors |= v
                    peak = max(peak, abs(v))
            tz = (ors & -ors).bit_length() - 1 if ors else lo
            out.append(f"peak {100 * peak / (1 << (lo - 1)):.1f}% of full scale, {n} samples checked")
            if tz:
                out.append(f"the lowest {tz} bit(s) of every sample are zero (padded from a narrower format)")
    fid = pick_wav_filter(path, size)
    out.append("filter kiln will use: " + (filter_desc(fid) if fid else "none"))
    return out


def run_selftest(say=print):
    """End-to-end self check: round trips, encryption, C <-> pure-Python agreement,
    corruption handling. Returns True if everything passed."""
    import argparse as ap
    import contextlib
    import io
    import random
    import shutil
    import tempfile
    t0 = time.time()
    say(f"kiln {VERSION} selftest - Python {sys.version.split()[0]}, {os.cpu_count() or 1} core(s)")
    root = tempfile.mkdtemp(prefix="kiln_selftest_")
    results = []

    def check(name, fn):
        t1 = time.time()
        try:
            msg = fn()
            ok = True
        except Exception as e:                      # noqa: BLE001 - report, don't crash
            ok, msg = False, f"{type(e).__name__}: {e}"[:110]
        results.append(ok)
        say(f"  {'PASS' if ok else 'FAIL'}  {name} ({time.time() - t1:.1f}s)" + (f" - {msg}" if msg else ""))

    def mk_wav(path, bits, tag, ch, frames, seed, from_int=0, ext=False, shift=0):
        """from_int: float samples that are really this many bit integers;
        ext: WAVE_FORMAT_EXTENSIBLE header; shift: left-justify in a wider container."""
        rng = random.Random(seed)
        data = bytearray()
        for i in range(frames):
            base = math.sin(i * 0.031) * 0.4 + math.sin(i * 0.0071) * 0.2
            for c in range(ch):
                x = base * (1 - 0.1 * c) + rng.gauss(0, 0.004)
                if tag == 3 and from_int:
                    data += struct.pack("<f", int(x * (1 << (from_int - 1))) / (1 << (from_int - 1)))
                elif tag == 3 and bits == 64:
                    data += struct.pack("<d", x)
                elif tag == 3:
                    data += struct.pack("<f", x)
                elif bits == 8:
                    data.append(int(x * 127) + 128)
                else:
                    v = int(x * ((1 << (bits - 1 - shift)) - 1)) << shift
                    data += v.to_bytes(bits // 8, "little", signed=True)
        bps = ch * bits // 8
        if ext:
            fmt = (struct.pack("<IHHIIHH", 40, 0xFFFE, ch, 44100, 44100 * bps, bps, bits)
                   + struct.pack("<HHI", 22, bits, 0) + struct.pack("<H", tag)
                   + b"\x00\x00\x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71")
            hdr = b"RIFF" + struct.pack("<I", 4 + 8 + 40 + 8 + len(data)) + b"WAVEfmt " + fmt
        else:
            hdr = (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt "
                   + struct.pack("<IHHIIHH", 16, tag, ch, 44100, 44100 * bps, bps, bits))
        hdr += b"data" + struct.pack("<I", len(data))
        with open(path, "wb") as fh:
            fh.write(hdr + data)

    try:
        src = os.path.join(root, "src")
        os.makedirs(src)
        rng = random.Random(7)
        words = "the quick brown fox jumps over lazy dogs while kilns bake clay pots".split()
        with open(os.path.join(src, "text.txt"), "w") as fh:
            fh.write(" ".join(rng.choice(words) for _ in range(60000)))
        with open(os.path.join(src, "random.bin"), "wb") as fh:
            fh.write(bytes(rng.getrandbits(8) for _ in range(150000)))
        open(os.path.join(src, "empty.dat"), "wb").close()
        with open(os.path.join(src, "tiny.dat"), "wb") as fh:
            fh.write(b"x")
        mk_wav(os.path.join(src, "pcm16.wav"), 16, 1, 2, 40000, 1)
        mk_wav(os.path.join(src, "pcm24.wav"), 24, 1, 2, 25000, 2)
        mk_wav(os.path.join(src, "float32.wav"), 32, 3, 2, 25000, 3)
        mk_wav(os.path.join(src, "pcm8.wav"), 8, 1, 2, 70000, 4)
        mk_wav(os.path.join(src, "pcm32.wav"), 32, 1, 2, 20000, 5)
        mk_wav(os.path.join(src, "pcm24_ext_mono.wav"), 24, 1, 1, 50000, 6, ext=True)
        mk_wav(os.path.join(src, "pcm24in32.wav"), 32, 1, 2, 20000, 7, shift=8)
        mk_wav(os.path.join(src, "pcm16_6ch.wav"), 16, 1, 6, 12000, 8)
        mk_wav(os.path.join(src, "float64.wav"), 64, 3, 1, 20000, 9)
        mk_wav(os.path.join(src, "float_from16.wav"), 32, 3, 2, 25000, 10, from_int=16)
        mk_wav(os.path.join(src, "float_from24_ext.wav"), 32, 3, 2, 25000, 11, from_int=24, ext=True)
        mk_wav(os.path.join(src, "float_from8_mono.wav"), 32, 3, 1, 40000, 12, from_int=8)
        with open(os.path.join(src, "float_nan.wav"), "wb") as fh:   # almost integer audio, one NaN
            fh.write(open(os.path.join(src, "float_from16.wav"), "rb").read()[:20000]
                     + struct.pack("<f", float("nan"))
                     + open(os.path.join(src, "float_from16.wav"), "rb").read()[20004:])
        originals = {}
        for name in os.listdir(src):
            with open(os.path.join(src, name), "rb") as fh:
                originals[name] = fh.read()
        total = sum(map(len, originals.values()))
        say(f"  test set: {len(originals)} files, {fmt_size(total)}")

        def quiet(fn, *a):
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                return fn(*a)

        def pack(arc, encrypt=False):
            quiet(cmd_pack, ap.Namespace(archive=arc, inputs=[src], jobs=1,
                                         encrypt=encrypt, force=True, quiet=True))

        def roundtrip(arc):
            quiet(cmd_test, ap.Namespace(archive=arc, members=[], quiet=True))
            out = arc + ".out"
            shutil.rmtree(out, ignore_errors=True)
            quiet(cmd_unpack, ap.Namespace(archive=arc, members=[], output=out, force=True,
                                           verbose=False, quiet=True))
            seen = 0
            for dp, _dn, fns in os.walk(out):
                for fn in fns:
                    with open(os.path.join(dp, fn), "rb") as fh:
                        if fh.read() != originals.get(fn):
                            raise KilnError(f"{fn} came back different")
                    seen += 1
            if seen != len(originals):
                raise KilnError(f"{seen} of {len(originals)} files came back")
            shutil.rmtree(out, ignore_errors=True)

        plain = os.path.join(root, "plain.kln")

        def t_plain():
            pack(plain)
            roundtrip(plain)
            f, table, *_ = open_archive(plain)
            f.close()
            filt = {e["rel"].split("/")[-1]: e["filt"] for e in table if e["filt"]}
            return f"{fmt_size(os.path.getsize(plain))}, filters: " + (", ".join(f"{k}={v}" for k, v in sorted(filt.items())) or "none")
        check("pack, test, extract, compare", t_plain)

        def t_noraw():
            """A block stored raw (type 0) must really be incompressible: if lzma can
            shrink it, kiln's coder silently gave up on data it should have packed."""
            import lzma
            msgs = []
            for name in ("plain.kln",):
                with open(os.path.join(root, name), "rb") as fh:
                    d = fh.read()
                if d[:4] != MAGIC or d[4] & 1:
                    raise KilnError(f"{name}: unexpected header")
                p, stored = 5, 0
                while d[p] != 2:
                    rl, cl = struct.unpack(">II", d[p + 1:p + 9])
                    if d[p] == 0:
                        pay = d[p + 9:p + 9 + cl]
                        if len(lzma.compress(pay)) < 0.95 * len(pay):
                            raise KilnError(f"{name}: a compressible {len(pay)}-byte block was stored raw")
                        stored += rl
                    p += 9 + cl
                msgs.append(f"{stored // 1024} KB")
            return "raw-stored blocks are incompressible (" + "/".join(msgs) + ")"
        check("no silent raw fallback", t_noraw)

        def t_enc():
            try:
                _aesgcm()
            except KilnError:
                return "skipped (pkg install python-cryptography)"
            old = os.environ.get("KILN_PASSWORD")
            os.environ["KILN_PASSWORD"] = "selftest-password"
            try:
                arc = os.path.join(root, "enc.kln")
                pack(arc, True)
                roundtrip(arc)
                os.environ["KILN_PASSWORD"] = "wrong-password!"
                try:
                    roundtrip(arc)
                except KilnError:
                    return "wrong password rejected"
                raise KilnError("wrong password was accepted")
            finally:
                if old is None:
                    os.environ.pop("KILN_PASSWORD", None)
                else:
                    os.environ["KILN_PASSWORD"] = old
        check("encryption", t_enc)

        def t_wide():
            wr = random.Random(11)
            words2 = [bytes(wr.randrange(97, 123) for _ in range(wr.randint(2, 8))) for _ in range(3000)]
            text = b" ".join(wr.choice(words2) for _ in range(40000))
            gap = wr.randbytes(2 << 20)
            one, two = os.path.join(root, "w1"), os.path.join(root, "w2")
            for d, files in ((one, {"a": text}), (two, {"a": text, "gap": gap, "c": text})):
                os.makedirs(d)
                for k, v in files.items():
                    with open(os.path.join(d, k), "wb") as fh:
                        fh.write(v)
            sizes = []
            for d in (one, two):
                arc = d + ".kln"
                quiet(cmd_pack, ap.Namespace(archive=arc, inputs=[d], jobs=1,
                                             encrypt=False, force=True, quiet=True))
                quiet(cmd_test, ap.Namespace(archive=arc, members=[], quiet=True))
                sizes.append(os.path.getsize(arc))
            extra = sizes[1] - sizes[0] - len(gap)
            if extra > sizes[0] // 4:
                raise KilnError(f"a copy 2 MB away was not matched ({extra} extra bytes)")
            return f"a repeat 2 MB away costs {extra} bytes ({100 * extra // sizes[0]}% of a fresh copy)"
        check("wide match window", t_wide)

        def t_far():
            """A repeat 8-60 MB away is found, and far distances round-trip through the coder."""
            rng2 = random.Random(77)
            blob = rng2.randbytes(600_000)
            sizes = []
            for fill_mb in (0, 14):
                d = os.path.join(root, f"far{fill_mb}")
                os.makedirs(d)
                for k, v in (("a", blob), ("b", rng2.randbytes(fill_mb << 20)), ("c", blob)):
                    if v:
                        with open(os.path.join(d, k + ".dat"), "wb") as fh:
                            fh.write(v)
                arc = d + ".kln"
                quiet(cmd_pack, ap.Namespace(archive=arc, inputs=[d], jobs=1,
                                             encrypt=False, force=True, quiet=True))
                quiet(cmd_test, ap.Namespace(archive=arc, members=[], quiet=True))
                sizes.append(os.path.getsize(arc) - (fill_mb << 20))
            if sizes[1] > sizes[0] + 20_000:
                raise KilnError(f"a copy 14 MB away was not matched ({sizes[1] - sizes[0]} extra bytes)")
            # token level: distances up to the window limit
            hist = rng2.randbytes(WINDOW + 1)
            plan = [(20, WINDOW), (7, 50_000_000), -1, (2, 30_000_000), (9, 9_000_000), (258, 1)]
            exp = bytearray(hist)
            toks = array.array("Q")
            for it in plan:
                if it == -1:
                    toks.append(65); exp.append(65)
                    continue
                ln, dist = it
                toks.append((ln << DBITS) | dist)
                for _ in range(ln):
                    exp.append(exp[-dist])
            raw, payload = encode_block(toks, bytes(exp), len(hist))
            if payload is None:
                raise KilnError("far-distance block did not encode")
            got = bytearray(hist)
            decode_block(payload, raw, got)
            if got != exp:
                raise KilnError("far distances did not round-trip")
            return f"a copy 14 MB away costs {sizes[1] - sizes[0]} extra bytes; distances to 64 MB round-trip"
        check("matches beyond 8 MB", t_far)

        def t_lpparse():
            """The parser may re-run with the block's literal position setting on float data; it must never lose."""
            rng2 = random.Random(3)
            x, vals = 0.0, []
            for i in range(400000):
                x += rng2.gauss(0, 0.05)
                vals.append(math.sin(i / 50) + x)
            data = struct.pack("<400000f", *vals)
            plain = sum(len(b[2]) for b in _code_parts(lz77(data, 0, [1 if x == 1 else 0 for x in random_chunks(data)]), data, 0))
            full = sum(len(b[2]) for b in build_blocks(data, 0))
            if full > plain:
                raise KilnError(f"re-parse with position bits made it bigger ({plain} -> {full})")
            return f"float32: {plain} -> {full} bytes ({100 * (plain - full) / plain:.1f}% smaller)"
        check("parse with literal position bits", t_lpparse)

        def t_wavfilters():
            """Every filter: decode inverts encode."""
            rng2 = random.Random(11)
            tested = 0
            for fid in list(range(1, 12)) + list(range(16, 63)) + list(range(64, 111)):
                sp = wav_spec(fid)
                if sp is None:
                    continue
                kind, W, order, ms, zz, bias, s = sp
                for C in ((2,) if ms else (1, 3)):
                    n = 331
                    if kind == "fint":
                        ks = [rng2.randint(-(1 << s), (1 << s) - 1) for _ in range(n * C)]
                        data = struct.pack(f"<{n * C}f", *[k / (1 << s) for k in ks])
                    else:
                        data = bytes(rng2.getrandbits(8) for _ in range(n * C * W))
                    f = Wav(fid, 1 << 40)
                    f.C, f.align = C, C * W
                    e = f._enc(data)
                    if len(e) != len(data) or f._dec(e) != data:
                        raise KilnError(f"filter {fid} doesn't round-trip")
                    tested += 1
            return f"{tested} filter/channel combinations"

        check("every WAV filter round-trips", t_wavfilters)

        def t_wavpick():
            want = {"pcm16.wav": "16-bit", "pcm24.wav": "24-bit", "pcm32.wav": "32-bit",
                    "pcm8.wav": "8-bit", "pcm24_ext_mono.wav": "24-bit",
                    "pcm24in32.wav": "32-bit", "pcm16_6ch.wav": "16-bit",
                    "float_from16.wav": "stored as 16-bit", "float_from24_ext.wav": "stored as 24-bit",
                    "float_from8_mono.wav": "stored as 8-bit"}
            got = []
            for name, key in want.items():
                fid = pick_wav_filter(os.path.join(src, name), os.path.getsize(os.path.join(src, name)))
                got.append(f"{name}={fid}")
                if not fid or key not in filter_desc(fid):
                    raise KilnError(f"{name}: picked {filter_desc(fid) if fid else 'nothing'}")
            for name in ("float_nan.wav",):
                fid = pick_wav_filter(os.path.join(src, name), os.path.getsize(os.path.join(src, name)))
                if fid and "stored as" in filter_desc(fid):
                    raise KilnError("float with a NaN must not use the integer conversion")
            return ""
        check("filter choice per WAV format", t_wavpick)

        def t_append():
            """Shell saves: append, rename, lazy remove, compact; plain and encrypted."""
            def sh_run(sh, *cmds):
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    for c in cmds:
                        sh.onecmd(c)

            def listing(arc):
                f2, table, *_ = open_archive(arc, os.environ.get("KILN_PASSWORD"))
                f2.close()
                return table

            def contents(arc):
                out2 = arc + ".x"
                shutil.rmtree(out2, ignore_errors=True)
                quiet(cmd_unpack, ap.Namespace(archive=arc, members=[], output=out2, force=True,
                                               verbose=False, quiet=True))
                got = {}
                for r_, _, fs in os.walk(out2):
                    for fn in fs:
                        with open(os.path.join(r_, fn), "rb") as fh:
                            got[os.path.relpath(os.path.join(r_, fn), out2)] = fh.read()
                return got

            notes = []
            for enc_on in (False, True):
                arc = os.path.join(root, "app_e.kln" if enc_on else "app.kln")
                if os.path.exists(arc):
                    os.remove(arc)
                sh = Shell()
                if enc_on:
                    os.environ["KILN_PASSWORD"] = "selftest-password"
                try:
                    sh_run(sh, f"new {shlex.quote(arc)}" + (" -e" if enc_on else ""),
                           f"add {shlex.quote(os.path.join(src, 'text.txt'))} "
                           f"{shlex.quote(os.path.join(src, 'pcm16.wav'))}", "save")
                    want = {"text.txt": originals["text.txt"], "pcm16.wav": originals["pcm16.wav"]}
                    if contents(arc) != want:
                        raise KilnError("first save doesn't round-trip")
                    size1 = os.path.getsize(arc)
                    head1 = open(arc, "rb").read(size1 - 400)[:200]
                    sh_run(sh, f"add {shlex.quote(os.path.join(src, 'pcm24.wav'))}", "save")
                    want["pcm24.wav"] = originals["pcm24.wav"]
                    if contents(arc) != want:
                        raise KilnError("append doesn't round-trip")
                    kept_prefix = open(arc, "rb").read(200) == head1[:200]
                    if enc_on:
                        if kept_prefix:
                            raise KilnError("encrypted archive reused its salt")
                    elif not kept_prefix:
                        raise KilnError("plain save didn't keep the old data")
                    sh_run(sh, "mv pcm24.wav renamed.wav", "rm pcm16.wav", "save")
                    want["renamed.wav"] = want.pop("pcm24.wav")
                    want.pop("pcm16.wav")
                    if contents(arc) != want:
                        raise KilnError("rename + remove doesn't round-trip")
                    hidden = sum(1 for e in listing(arc) if e.get("dead"))
                    if hidden != (0 if enc_on else 1):
                        raise KilnError(f"expected {0 if enc_on else 1} hidden entries, got {hidden}")
                    sh_run(sh, f"add {shlex.quote(os.path.join(src, 'random.bin'))}", "save")
                    want["random.bin"] = originals["random.bin"]
                    if contents(arc) != want:
                        raise KilnError("append after remove doesn't round-trip")
                    sh_run(sh, "compact")
                    if contents(arc) != want or any(e.get("dead") for e in listing(arc)):
                        raise KilnError("compact failed")
                    sh_run(sh, "test")
                    notes.append("encrypted" if enc_on else "plain")
                finally:
                    os.environ.pop("KILN_PASSWORD", None)
                    if sh.f:
                        sh.f.close()
            return " + ".join(notes)
        check("append-only saves, hidden entries, compact", t_append)

        def t_ckpt():
            """Checkpoints: random access to single files, plain and encrypted, packed and appended."""
            global CKPT_SPAN
            saved_span = CKPT_SPAN
            CKPT_SPAN = 150_000
            try:
                rr = random.Random(11)
                words = [bytes(rr.choices(b"abcdefghij ", k=rr.randint(3, 9))) for _ in range(200)]
                ck_src = os.path.join(root, "ck_src")
                shutil.rmtree(ck_src, ignore_errors=True)
                os.makedirs(ck_src)
                files = {}
                for i in range(14):
                    d = bytearray()
                    while len(d) < 60_000:
                        d += rr.choice(words) + b" "
                    files[f"f{i:02d}.txt"] = bytes(d)
                files["empty.txt"] = b""
                for n_, d in files.items():
                    with open(os.path.join(ck_src, n_), "wb") as fh:
                        fh.write(d)

                def read_one(arc, pw, idx):
                    f2, table, toff, hl, ci = open_archive(arc, pw)
                    try:
                        buf = bytearray()
                        for j, piece, last in stream_pieces(f2, table, hl, ci, want=[idx]):
                            if j == idx:
                                buf += piece
                            if j == idx and last:
                                break
                        return table, bytes(buf)
                    finally:
                        f2.close()

                notes = []
                for enc_on in (False, True):
                    pw = "selftest-password" if enc_on else None
                    arc = os.path.join(root, "ck_e.kln" if enc_on else "ck.kln")
                    if enc_on:
                        os.environ["KILN_PASSWORD"] = pw
                    try:
                        quiet(cmd_pack, ap.Namespace(archive=arc, inputs=[ck_src], jobs=1,
                                                     encrypt=enc_on, force=True, quiet=True))
                        table, _ = read_one(arc, pw, 0)
                        if not table.ckpts:
                            raise KilnError("no checkpoints were written")
                        for idx, e in enumerate(table):
                            if read_one(arc, pw, idx)[1] != files[os.path.basename(e["rel"])]:
                                raise KilnError(f"random access to {e['rel']} is wrong")
                        out2 = os.path.join(root, "ck_out")
                        shutil.rmtree(out2, ignore_errors=True)
                        pick = table[-2]["rel"]
                        quiet(cmd_unpack, ap.Namespace(archive=arc, members=[pick], output=out2,
                                                       force=True, verbose=False, quiet=True))
                        with open(os.path.join(out2, pick), "rb") as fh:
                            if fh.read() != files[os.path.basename(pick)]:
                                raise KilnError("selective extract is wrong")
                        # append from the shell: checkpoints must stay valid
                        if not enc_on:
                            sh = Shell()
                            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                                sh.onecmd(f"open {shlex.quote(arc)}")
                                extra = os.path.join(root, "ck_extra.txt")
                                with open(extra, "wb") as fh:
                                    fh.write(files["f03.txt"] + files["f07.txt"])
                                sh.onecmd(f"add {shlex.quote(extra)}")
                                sh.onecmd("save")
                            if sh.f:
                                sh.f.close()
                            t2, got = read_one(arc, None, len(table))
                            if got != files["f03.txt"] + files["f07.txt"] or len(t2.ckpts) < len(table.ckpts):
                                raise KilnError("append after checkpoints is wrong")
                            quiet(cmd_test, ap.Namespace(archive=arc, members=[], output=out2,
                                                         force=True, verbose=False, quiet=True))
                        notes.append(f"{len(table.ckpts)} checkpoints, " + ("encrypted" if enc_on else "plain"))
                    finally:
                        os.environ.pop("KILN_PASSWORD", None)
                return "; ".join(notes)
            finally:
                CKPT_SPAN = saved_span
        check("checkpoints: random access", t_ckpt)

        def t_dups():
            """An exact copy of random-looking or high-entropy data must cost almost nothing, """
            rr = random.Random(21)
            blobs = {"random": bytes(rr.getrandbits(8) for _ in range(200_000)),
                     "high-entropy text": bytes(rr.choices(range(33, 127), k=400_000))}
            notes = []
            for name, blob in blobs.items():
                if True:
                    d = os.path.join(root, "dup_src")
                    shutil.rmtree(d, ignore_errors=True)
                    os.makedirs(d)
                    sizes = []
                    for names in (("a.dat",), ("a.dat", "b.dat")):
                        for n_ in names:
                            with open(os.path.join(d, n_), "wb") as fh:
                                fh.write(blob)
                        arc = os.path.join(root, "dup.kln")
                        quiet(cmd_pack, ap.Namespace(archive=arc, inputs=[d], jobs=1,
                                                     encrypt=False, force=True, quiet=True))
                        sizes.append(os.path.getsize(arc))
                    if sizes[1] > sizes[0] * 1.05:
                        raise KilnError(f"{name}: a copy cost {sizes[1] - sizes[0]} extra bytes")
                    if True:
                        notes.append(f"{name}: +{100 * (sizes[1] - sizes[0]) / sizes[0]:.1f}%")
            return ", ".join(notes)
        check("repeats in random-looking data", t_dups)

        def t_pre_dups():
            """Identical already-compressed files (jpg etc.) must cost about one copy."""
            d = os.path.join(root, "pre")
            shutil.rmtree(d, ignore_errors=True)
            os.makedirs(d)
            rr = random.Random(21)
            a, b = rr.randbytes(300_000), rr.randbytes(100_000)
            files = {"a.jpg": a, "b.jpg": b, "c.jpg": a, "d.zip": a}
            for n_, data_ in files.items():
                with open(os.path.join(d, n_), "wb") as fh:
                    fh.write(data_)
            arc = os.path.join(root, "pre.kln")
            quiet(cmd_pack, ap.Namespace(archive=arc, inputs=[d], jobs=1,
                                         encrypt=False, force=True, quiet=True))
            quiet(cmd_test, ap.Namespace(archive=arc, members=[], quiet=True))
            out = arc + ".out"
            shutil.rmtree(out, ignore_errors=True)
            quiet(cmd_unpack, ap.Namespace(archive=arc, members=[], output=out, force=True,
                                           verbose=False, quiet=True))
            for n_, data_ in files.items():
                with open(os.path.join(out, "pre", n_), "rb") as fh:
                    if fh.read() != data_:
                        raise KilnError(f"{n_} came back different")
            shutil.rmtree(out, ignore_errors=True)
            size = os.path.getsize(arc)
            if size > 1.05 * (len(a) + len(b)) + 2000:
                raise KilnError(f"copies of precompressed files not found: {size} bytes")
            return f"3 copies of a 300 KB jpg/zip + 100 KB jpg -> {size // 1024} KB"
        check("duplicate precompressed files", t_pre_dups)

        def t_classes():
            """Noise (even low-entropy noise like hex) must be skipped for speed, but
            data with positional structure must still be searched."""
            rr = random.Random(33)
            n_ = 1 << 18
            def cls(b):
                got = random_chunks(bytes(b))
                return set(got[1:-1]) or set(got)
            hexnoise = rr.randbytes(n_ // 2).hex().encode()
            gauss = bytes(max(0, min(255, int(rr.gauss(128, 20)))) for _ in range(n_))
            flt = b"".join(struct.pack("<f", math.sin(i / 300) * 10 + rr.gauss(0, 0.05))
                           for i in range(n_ // 4))
            tab = b"".join(struct.pack("<I", (i * 3 + rr.randrange(3)) << 8) for i in range(n_ // 4))
            want = {"hex": (hexnoise, {2}), "gaussian noise": (gauss, {2}),
                    "float32 sensor": (flt, {0}), "counter table": (tab, {0})}
            for name_, (b_, ok_) in want.items():
                got_ = cls(b_)
                if not got_ <= ok_ | {1}:
                    raise KilnError(f"{name_}: classes {sorted(got_)}, wanted {sorted(ok_)}")
            return "noise skipped, structured data searched"
        check("chunk classes", t_classes)

        def t_len2():
            """A match of length 2 at a new distance (not a repeat) must encode and decode,
            near and far, and mixed with repeats and literals."""
            rr = random.Random(77)
            data = rr.randbytes(5000)
            buf = bytearray(data)
            toks = list(data)
            for dist in (3, 40, 1200, 4900, 2, 3, 3):
                for ln in (2, 2, 3, 2):
                    if dist > len(buf):
                        continue
                    toks.append((ln << DBITS) | dist)
                    for _ in range(ln):
                        buf.append(buf[-dist])
                toks.append(rr.randrange(256))
                buf.append(toks[-1])
            raw, payload = encode_block(array.array("Q", toks), bytes(buf), 0)
            if payload is None or raw != len(buf):
                raise KilnError("a length-2 match was rejected by the encoder")
            out = bytearray()
            decode_block(payload, raw, out)
            if bytes(out) != bytes(buf):
                raise KilnError("length-2 matches decoded differently")
            return f"{len(toks)} tokens round-trip"
        check("length-2 matches", t_len2)

        def t_jobs():
            """The default worker count must follow free memory, never drop below 1, and
            never use more than the cores."""
            real_mem, real_cpu = mem_available, os.cpu_count
            g = globals()
            try:
                os.cpu_count = lambda: 8
                got = {}
                for mb in (500, 2500, 6000, 64000):
                    g["mem_available"] = lambda mb=mb: mb << 20
                    got[mb] = auto_jobs(64 << 20)
                g["mem_available"] = lambda: None
                none_known = auto_jobs(64 << 20)
                one_seg = auto_jobs(1 << 20)
            finally:
                g["mem_available"] = real_mem
                os.cpu_count = real_cpu
            if got[500] != 1 or one_seg != 1:
                raise KilnError(f"too many workers for low memory: {got}, small input {one_seg}")
            if not (got[500] <= got[2500] <= got[6000] <= got[64000] <= 8) or got[6000] < 3:
                raise KilnError(f"worker count doesn't follow memory: {got}")
            if none_known != 8:
                raise KilnError("unknown memory should allow the full count")
            return f"8 cores: {got[500]}, {got[2500]}, {got[6000]}, {got[64000]} workers at 0.5, 2.5, 6, 64 GB free"
        check("memory-aware workers", t_jobs)

        def t_litpos():
            """Every literal-position setting must round-trip, an invalid one must be rejected,
            and record-structured data must pick a non-zero setting for some block."""
            rr = random.Random(91)
            data = b"".join(struct.pack("<f", math.sin(i / 40) * 5 + rr.gauss(0, 0.2))
                            for i in range(40000))
            toks = array.array("Q", list(data[:60000]))
            for lp in (0, 1, 2, 4, 5, 6):  # bit 2 = literal mixer
                raw, payload = encode_block(toks, data, 0, lp)
                if payload is None or payload[0] != lp:
                    raise KilnError(f"lp={lp}: bad payload")
                out = bytearray()
                decode_block(payload, raw, out)
                if bytes(out) != data[:raw]:
                    raise KilnError(f"lp={lp} decoded differently")
            for badbyte in (3, 7, 8, 255):
                bad = bytearray(payload)
                bad[0] = badbyte
                try:
                    decode_block(bytes(bad), raw, bytearray())
                except KilnError:
                    pass
                else:
                    raise KilnError(f"an invalid mode byte ({badbyte}) was accepted")
            blocks = build_blocks(data, 0)
            used = sorted({b_[2][0] for b_ in blocks if b_[0] == 1})
            if not any(u for u in used):
                raise KilnError("float data never chose a literal-position setting")
            out = bytearray()
            for t_, raw_, pl_ in blocks:
                if t_ == 0:
                    out += pl_
                else:
                    decode_block(pl_, raw_, out)
            if bytes(out) != data:
                raise KilnError("build_blocks round trip failed")
            return f"settings used by float32 blocks: {used}"
        check("literal position bits", t_litpos)

        def t_mixer():
            """Text-like data must pick the literal mixer for some block, and the result
            must round-trip (matched and unmatched literals both go through it)."""
            rr = random.Random(5)
            words = [bytes(rr.choice(b"abcdefghilmnoprstu") for _ in range(rr.randrange(2, 9)))
                     for _ in range(900)]
            data = b" ".join(rr.choice(words) for _ in range(60000))[:300000]
            blocks = build_blocks(data, 0)
            used = [b_[2][0] >> 2 for b_ in blocks if b_[0] == 1]
            if not any(used):
                raise KilnError("text never chose the literal mixer")
            out = bytearray()
            for t_, raw_, pl_ in blocks:
                if t_ == 0:
                    out += pl_
                else:
                    decode_block(pl_, raw_, out)
            if bytes(out) != data:
                raise KilnError("mixer round trip failed")
            return f"{sum(1 for u in used if u)} of {len(used)} blocks use the mixer"
        check("literal mixer", t_mixer)

        def t_rc():
            """Range-coded blocks: round trip, and bad data is rejected."""
            rr = random.Random(5)
            words3 = [bytes(rr.randrange(97, 123) for _ in range(rr.randint(2, 9))) for _ in range(2000)]
            buf = (b" ".join(rr.choice(words3) for _ in range(60000))
                   + bytes(rr.getrandbits(8) for _ in range(30000))
                   + bytes(range(256)) * 400 + bytes(70000))
            if True:
                blocks = build_blocks(buf, 0)
                if not any(t == 1 for t, _r, _p in blocks):
                    raise KilnError("no range-coded blocks were written")
                out = bytearray()
                for t, raw, pl in blocks:
                    if t == 0:
                        out += pl
                    else:
                        decode_block(pl, raw, out)
                if bytes(out) != buf:
                    raise KilnError("range-coded blocks came back different")
            blocks = build_blocks(buf, 0)
            big = max((b for b in blocks if b[0] == 1), key=lambda b: len(b[2]))
            for k in range(300):
                pl = bytearray(big[2])
                if k % 3 == 0:
                    del pl[rr.randrange(len(pl)):]
                else:
                    for _ in range(rr.choice((1, 2, 5))):
                        pl[rr.randrange(len(pl))] ^= 1 << rr.randrange(8)
                try:
                    decode_block(bytes(pl), big[1], bytearray(b"x" * 100))
                except KilnError:
                    pass
            return f"{len(buf)} bytes -> {sum(len(p) for _t, _r, p in blocks)}"
        check("range coder", t_rc)

        def t_corrupt():
            with open(plain, "rb") as fh:
                good = fh.read()
            rr = random.Random(3)
            bad_arc = os.path.join(root, "bad.kln")
            caught = 0
            for _ in range(25):
                b = bytearray(good)
                for _k in range(rr.choice((1, 3))):
                    b[rr.randrange(5, len(b) - FOOTER_LEN)] ^= 1 << rr.randrange(8)
                with open(bad_arc, "wb") as fh:
                    fh.write(b)
                try:
                    roundtrip(bad_arc)
                except (KilnError, struct.error, UnicodeDecodeError, MemoryError):
                    caught += 1
            if caught < 25:
                raise KilnError(f"only {caught} of 25 corrupted archives were rejected")
            return "25 of 25 corrupted archives rejected"
        check("corruption is detected", t_corrupt)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    bad = results.count(False)
    say(f"{'all ' + str(len(results)) + ' checks passed' if not bad else str(bad) + ' of ' + str(len(results)) + ' checks FAILED'}"
        f" in {time.time() - t0:.1f}s")
    return not bad


BENCH_MAX = 8 << 20


def cmd_bench(args):
    import bz2
    import io
    try:
        import lzma
    except ImportError:
        lzma = None
    files = collect(args.inputs)
    if not files:
        raise KilnError("nothing to benchmark")
    # Only the first BENCH_MAX bytes are timed: lzma/bz2 on a big file froze the phone for minutes.
    data = bytearray()
    total = 0
    for e in files:
        total += e["size"] if "size" in e else os.path.getsize(e["full"])
        room = BENCH_MAX - len(data)
        if room > 0:
            with open(e["full"], "rb") as fh:
                data += fh.read(room)
    data = bytes(data)
    cut = f" (first {fmt_size(len(data))} of {fmt_size(total)})" if total > len(data) else ""
    print(f"input: {len(files)} file(s), {fmt_size(len(data))}{cut}")

    def kiln_c(d):
        buf = io.BytesIO()
        enc = Encoder(PlainSink(buf))
        enc.feed(d)
        enc.finish()
        return buf.getvalue()

    contenders = [(f"kiln {VERSION}", kiln_c), ("zlib -9", lambda d: zlib.compress(d, 9)),
                  ("bz2 -9", lambda d: bz2.compress(d, 9))]
    if lzma:
        contenders.append(("lzma", lzma.compress))
    print(f"{'format':<10}{'size':>12}{'ratio':>9}{'time':>9}")
    for name, fn in contenders:
        t0 = time.time()
        size = len(fn(data))
        print(f"{name:<10}{size:>12}{100 * size / max(len(data), 1):>8.1f}%"
              f"{time.time() - t0:>8.1f}s")


# ---------------------------------------------------------------- interactive shell
USE_COLOR = sys.stdout.isatty() and "NO_COLOR" not in os.environ


def col(text, code):
    return f"\033[{code}m{text}\033[0m" if USE_COLOR else text


def stream_pieces(f, table, hdr_len, cipher, table_off=None, want=None, packed=False):
    """Generator over an archive's data: yields (file_index, piece, is_last_piece).
    Every file's CRC is checked as its last piece goes by.
    With want (file indexes), decoding starts at the nearest checkpoint before the
    first of them, so the files before that point aren't decoded.
    With packed=True, MP3-recompressed files come out as their packed stream bytes (no CRC
    check) instead of the original bytes: that is what a rewrite of the archive copies."""
    k, off, ctr = find_ckpt(table, min(want), hdr_len) if want else (0, hdr_len, 0)
    f.seek(off)
    src = cipher.source(f, 0) if cipher else PlainSource(f)
    if ctr:
        src.n = ctr
    n = len(table)
    i = k
    got = crc = 0
    unf = None

    def empties():
        nonlocal i
        while i < n and slen(table[i]) == 0:
            if table[i]["crc"] != 0:
                raise KilnError(f"checksum failed for {table[i]['rel']}")
            yield i
            i += 1

    for k in empties():
        yield k, b"", True
    for chunk in read_stream(src):
        while chunk:
            if i >= n:
                raise KilnError("corrupt data: stream is longer than the file table")
            need = slen(table[i]) - got
            piece, chunk = chunk[:need], chunk[need:]
            got += len(piece)
            last = got == slen(table[i])
            keep_packed = packed and table[i].get("filt") == MP3_FID
            if table[i].get("filt") and not table[i].get("dead") and not keep_packed:
                if unf is None:
                    unf = make_unfilter(table[i])
                piece = unf.feed(piece) + (unf.finish() if last else b"")
            if not keep_packed:
                crc = zlib.crc32(piece, crc)
                if last and crc != table[i]["crc"] and not table[i].get("dead"):
                    raise KilnError(f"checksum failed for {table[i]['rel']}")
            yield i, piece, last
            if last:
                i += 1
                got = crc = 0
                unf = None
                for k in empties():
                    yield k, b"", True
    if i != n:
        raise KilnError("archive is truncated (data ends early)")
    if table_off is not None and f.tell() != table_off:
        raise KilnError("corrupt data: stream doesn't end where the table starts")


def archive_encrypted(path):
    try:
        with open(path, "rb") as f:
            head = f.read(5)
    except OSError as e:
        raise KilnError(f"cannot open {path}: {e.strerror}")
    return len(head) == 5 and head[:4] == MAGIC and bool(head[4] & FLAG_ENCRYPTED)


def parse_flags(args, valued=()):
    """['-rf', '-o', 'dir', 'x'] -> ({'r': True, 'f': True, 'o': 'dir'}, ['x'])"""
    flags, pos, i, only_pos = {}, [], 0, False
    while i < len(args):
        a = args[i]
        if only_pos or a == "-" or not a.startswith("-"):
            pos.append(a)
        elif a == "--":
            only_pos = True
        else:
            names = [a[2:]] if a.startswith("--") else list(a[1:])
            for j, name in enumerate(names):
                if name in valued:
                    i += 1
                    if i >= len(args):
                        raise KilnError(f"option -{name} needs a value")
                    flags[name] = args[i]
                else:
                    flags[name] = True
        i += 1
    return flags, pos


def _esc(s):
    return "".join("\\" + c if c in " '\"\\" else c for c in s)


def _unesc(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            i += 1
        out.append(s[i])
        i += 1
    return "".join(out)


SHELL_HELP = """\
kiln shell - manage .kln archives interactively

archive   open FILE|SETDIR | new FILE [-e [-s]] | close | save [FILE] | compact | discard
          status | info [NAME]
browse    ls [-l] [PATH] | cd [DIR] | pwd | tree [DIR] | find PATTERN
          du [DIR] | cat [-n BYTES] FILE
change    add PATH... | rm [-r] PATTERN... | mv SRC... DST | cp SRC... DST (sets) | passwd [-r]
extract   extract [PATTERN...] [-o DIR] [-f] | test
phone     lpwd | lcd DIR | lls [-l] [PATH] | !COMMAND
settings  jobs [N] | store [on|off]   (store = encryption only, no compression)
check     selftest | wavcheck FILE
other     help [COMMAND] | exit

PATHs inside the archive work like a normal file manager (cd, .., /, *.txt).
Changes (add / rm / mv / passwd) are kept in memory until you `save`;
`discard` throws them away. Tab completes archive and phone paths.
Shortcuts: ll = ls -l, put = add, get = extract, del = rm, q = exit
A random-name set (kiln pack -r) opens like an archive. add / rm / mv / cp work on it
and `save` applies them: new files are encrypted, removed ones are deleted from the folder.
passwd, store and compact aren't available for a set.
"""


class Shell(cmd.Cmd):
    intro = ""

    def __init__(self):
        super().__init__()
        self.f = None            # the archive on disk, open for reading
        self.path = None         # absolute path of the archive (save target)
        self.old_table, self.hdr_len, self.table_off, self.cipher = [], 0, 0, None
        self.entries = []        # the working set: what the archive will contain
        self.cwd = ""            # current folder inside the archive
        self.encrypt = False     # should the saved archive be encrypted?
        self.was_encrypted = False
        self.store = self.was_store = False   # encryption-only level: no compression
        self.pw = self.load_pw = None
        self.dirty = False
        self.set_dir = None      # a random-name set (-r): its directory
        self.set_v1 = False      # the set's index is old (no sizes/dates)
        self.jobs = auto_jobs()
        self.intro = self._banner()
        self._prompt()

    # ------------------------------------------------------------ plumbing
    def _banner(self):
        return (f"kiln {VERSION} shell - {self.jobs} core(s)\nType 'help' for commands, 'exit' to leave.")

    def _prompt(self):
        rl = "readline" in sys.modules and USE_COLOR

        def c(text, code):
            if not USE_COLOR:
                return text
            return (f"\001\033[{code}m\002{text}\001\033[0m\002" if rl
                    else f"\033[{code}m{text}\033[0m")
        if self.path:
            name = os.path.basename(self.path) + ("*" if self.dirty else "")
            self.prompt = c(name, "1;32") + ":" + c("/" + self.cwd, "1;34") + " > "
        else:
            self.prompt = c("kiln", "1;32") + " > "

    def postcmd(self, stop, line):
        self._prompt()
        return stop

    def emptyline(self):
        pass

    def default(self, line):
        print(f"unknown command: {line.split()[0]}   (type 'help')", file=sys.stderr)

    def onecmd(self, line):
        try:
            return super().onecmd(line)
        except KilnError as e:
            print(f"error: {e}", file=sys.stderr)
        except KeyboardInterrupt:
            print("\n^C interrupted", file=sys.stderr)
        except (OSError, struct.error, UnicodeDecodeError) as e:
            print(f"error: {e}", file=sys.stderr)
        return False

    def split(self, arg):
        try:
            return shlex.split(arg)
        except ValueError:
            raise KilnError("unbalanced quote in command")

    def need(self):
        if not self.path:
            raise KilnError("no archive open (use: open FILE   or   new FILE)")

    def writable(self, set_ok=False):
        self.need()
        if self.set_dir and not set_ok:
            raise KilnError("not available for a random-name set (add, rm, mv, cp and save work)")
        if self.set_dir and self.set_v1:
            raise KilnError("this set's index is old (no sizes or dates): run the same "
                            "`kiln pack -esr ...` command once to upgrade it, then edit")

    def confirm(self, question):
        try:
            return input(question + " [y/N] ").strip().lower() in ("y", "yes")
        except EOFError:
            return False

    # ------------------------------------------------------------ archive model
    @staticmethod
    def info_of(rel):
        ext = os.path.splitext(rel)[1].lower()
        return {"ext": ext, "pre": ext in PRECOMPRESSED}

    def load_set(self, path, pw=None):
        """Open a random-name set (a directory with _index.kln) read-only, browsing the real paths."""
        if not os.path.isfile(os.path.join(path, INDEX_NAME)):
            raise KilnError(f"{path} is a directory without {INDEX_NAME} (not a kiln random-name set)")
        meta = None
        for attempt in range(3):
            attempt_pw = pw if pw is not None else get_password()
            try:
                meta = read_index_meta(path, attempt_pw)
                pw = attempt_pw
                break
            except KilnError as e:
                if "authentication failed" in str(e) and pw is None and attempt < 2 \
                        and "KILN_PASSWORD" not in os.environ:
                    print("wrong password, try again", file=sys.stderr)
                    continue
                raise
        table, approx = [], False
        for n, m in sorted(meta.items(), key=lambda kv: kv[1]["path"]):
            src = os.path.join(path, n)
            have = os.path.isfile(src)
            size, mtime = m["size"], m["mtime"]
            if size is None:            # old index: fall back to what is on disk
                approx = True
                size = os.path.getsize(src) if have else 0
                mtime = int(os.path.getmtime(src)) if have else 0
            table.append({"rel": m["path"], "size": size, "crc": None, "mtime": mtime,
                          "mode": 0o644, "filt": 0, "name": n, "missing": not have})
        if self.f:
            self.f.close()
        self.f, self.path, self.set_dir = None, path, path
        self.set_v1 = approx
        self.old_table, self.table_off, self.hdr_len, self.cipher = table, 0, 0, None
        self.encrypt = self.was_encrypted = True
        self.store = self.was_store = False
        self.pw = self.load_pw = pw
        self.reset_entries()
        self.dirty = False
        if not self.is_dir(self.cwd):
            self.cwd = ""
        gone = sum(1 for e in table if e["missing"])
        if approx:
            print("note: this index has no sizes or dates (made by an older kiln); sizes shown "
                  "are archive sizes.\n      Run the same `kiln pack -esr ...` command again to "
                  "add them (nothing is re-encrypted).", file=sys.stderr)
        if gone:
            print(f"warning: {gone} archive(s) listed in the index are missing (marked '!')",
                  file=sys.stderr)

    def load(self, path, pw=None):
        path = os.path.abspath(os.path.expanduser(path))
        if os.path.isdir(path):
            return self.load_set(path, pw)
        self.set_dir = None
        enc = archive_encrypted(path)
        f = None
        for attempt in range(3):
            attempt_pw = pw
            if enc and attempt_pw is None:
                attempt_pw = get_password()
            try:
                f, table, table_off, hdr_len, cipher = open_archive(path, attempt_pw if enc else None)
                pw = attempt_pw
                break
            except KilnError as e:
                if enc and "authentication failed" in str(e) and pw is None and attempt < 2 \
                        and "KILN_PASSWORD" not in os.environ:
                    print("wrong password, try again", file=sys.stderr)
                    continue
                raise
        if self.f:
            self.f.close()
        self.f, self.path, self.old_table = f, path, table
        self.table_off, self.hdr_len, self.cipher = table_off, hdr_len, cipher
        self.encrypt = self.was_encrypted = enc
        self.pw = self.load_pw = pw if enc else None
        # An encrypted archive whose data region is as big as the raw data, with no filters,
        # was made with the encryption-only level (or holds incompressible data, where it
        # makes no difference). Keep it that way on save; `store off` changes it.
        raw = sum(slen(e) for e in table)
        self.store = self.was_store = bool(
            enc and raw and not any(e.get("filt") for e in table) and table_off - hdr_len >= raw)
        if self.store:
            print("encryption-only archive: it will stay uncompressed when saved "
                  "(`store off` to compress)", file=sys.stderr)
        self.reset_entries()
        if not self.is_dir(self.cwd):
            self.cwd = ""

    def reset_entries(self):
        self.entries = [dict(e, old=i, full=None, **self.info_of(e["rel"]))
                        for i, e in enumerate(self.old_table) if not e.get("dead")]
        self.dirty = False
        self.encrypt = self.was_encrypted
        self.store = self.was_store

    def mark(self, e):
        if self.set_dir and e["old"] is not None and self.old_table[e["old"]].get("missing"):
            return "!"
        if e["old"] is None:
            return "+"
        return "~" if e["rel"] != self.old_table[e["old"]]["rel"] else " "

    def norm(self, p):
        segs = [] if p.startswith("/") else [s for s in self.cwd.split("/") if s]
        for s in p.split("/"):
            if s in ("", "."):
                continue
            if s == "..":
                if segs:
                    segs.pop()
            else:
                segs.append(s)
        return "/".join(segs)

    def is_dir(self, p):
        if not p:
            return True
        pre = p + "/"
        return any(e["rel"].startswith(pre) for e in self.entries)

    def select(self, pattern):
        """Entries matched by a path/glob, shell-style per path segment.
        Returns (entries, k) where k = number of segments in the pattern."""
        psegs = [s for s in self.norm(pattern).split("/") if s]
        k = len(psegs)
        out = []
        for e in self.entries:
            segs = e["rel"].split("/")
            if len(segs) >= k and all(fnmatch.fnmatchcase(segs[j], psegs[j]) for j in range(k)):
                out.append(e)
        return out, k

    def group(self, entries, k):
        out = {}
        for e in entries:
            segs = e["rel"].split("/")
            key = "/".join(segs[:k])
            isdir = len(segs) > k
            d = out.get(key)
            if d is None:
                d = out[key] = {"dir": isdir, "size": 0, "n": 0, "mtime": 0, "mark": " ", "e": None}
            d["size"] += e["size"]
            d["n"] += 1
            d["mtime"] = max(d["mtime"], e["mtime"])
            m = self.mark(e)
            if m != " ":
                d["mark"] = m
            if not isdir:
                d["e"] = e
        return out

    def children(self, d):
        segs = [s for s in d.split("/") if s]
        pre = d + "/" if d else ""
        grouped = self.group([e for e in self.entries if e["rel"].startswith(pre)], len(segs) + 1)
        return {key.split("/")[-1]: v for key, v in grouped.items()}

    # ------------------------------------------------------------ completion
    def _arch_complete(self, text, line, begidx, endidx):
        t = _unesc(text)
        d = t[:t.rfind("/") + 1]
        try:
            kids = self.children(self.norm(d or "."))
        except Exception:
            return []
        return sorted(_esc(d + n + ("/" if v["dir"] else "")) for n, v in kids.items()
                      if (d + n).startswith(t) or (d + n + "/").startswith(t))

    def _local_complete(self, text, line, begidx, endidx):
        t = _unesc(text)
        d = t[:t.rfind("/") + 1]
        try:
            names = os.listdir(os.path.expanduser(d) or ".")
        except OSError:
            return []
        out = []
        for n in names:
            if (d + n).startswith(t):
                full = os.path.join(os.path.expanduser(d) or ".", n)
                out.append(_esc(d + n + ("/" if os.path.isdir(full) else "")))
        return sorted(out)

    # ------------------------------------------------------------ opening/closing/saving
    def do_open(self, arg):
        """open FILE - open an existing .kln archive"""
        p = self.split(arg)
        if len(p) != 1:
            raise KilnError("usage: open FILE")
        if self.dirty and not self.confirm("Unsaved changes will be lost. Continue?"):
            return
        self.cwd = ""
        self.load(p[0])
        self.print_summary()

    def do_new(self, arg):
        """new FILE [-e [-s]] - start a new, empty archive (-e = encrypted, -s = encryption only, no compression); nothing is written until `save`"""
        flags, p = parse_flags(self.split(arg))
        if len(p) != 1:
            raise KilnError("usage: new FILE [-e [-s]]")
        if flags.get("s") and not flags.get("e"):
            raise KilnError("-s (no compression) needs -e (encryption)")
        path = os.path.abspath(os.path.expanduser(p[0]))
        if os.path.exists(path):
            raise KilnError(f"{p[0]} already exists (use: open {p[0]})")
        if self.dirty and not self.confirm("Unsaved changes will be lost. Continue?"):
            return
        pw = get_password(confirm=True) if flags.get("e") else None
        if self.f:
            self.f.close()
        self.f, self.path, self.old_table, self.entries, self.cwd = None, path, [], [], ""
        self.set_dir = None
        self.cipher, self.hdr_len, self.table_off = None, 0, 0
        self.was_encrypted, self.encrypt, self.pw, self.load_pw = False, bool(pw), pw, None
        self.store, self.was_store = False, False
        self.store = bool(pw and flags.get("s"))
        self.dirty = True
        print(f"new archive {os.path.basename(path)}"
              + (" [encryption only, no compression]" if self.store else "")
              + " (use `add` then `save`)")

    def do_close(self, arg):
        """close - close the archive"""
        self.need()
        if self.dirty and not self.confirm("Unsaved changes will be lost. Continue?"):
            return
        if self.f:
            self.f.close()
        self.f = self.path = self.set_dir = None
        self.old_table, self.entries, self.cwd, self.dirty = [], [], "", False

    def do_discard(self, arg):
        """discard - throw away all unsaved changes"""
        self.need()
        if not self.dirty:
            print("nothing to discard")
            return
        self.reset_entries()
        self.pw = self.load_pw
        if not self.is_dir(self.cwd):
            self.cwd = ""
        print("changes discarded")

    def do_save(self, arg):
        """save [FILE] - write the archive (or save a copy as FILE and switch to it)"""
        self.writable(True)
        p = self.split(arg)
        if p and self.set_dir:
            raise KilnError("a set can't be saved under another name; use `save` alone")
        target = os.path.abspath(os.path.expanduser(p[0])) if p else self.path
        if not p and not self.dirty:
            print("nothing to save")
            return
        if target != self.path and os.path.exists(target) \
                and not self.confirm(f"{os.path.basename(target)} exists. Overwrite?"):
            return
        self.write(target)

    def old_epoch(self):
        """What the decoder holds when the appended blocks start. Returns (tail, raw):
        the last WINDOW bytes since the latest checkpoint and how much data that is, or
        (None, raw) when that stretch is already long: new data then starts a fresh
        checkpoint instead of matching against it."""
        k, off, _ = find_ckpt(self.old_table, len(self.old_table), self.hdr_len)
        raw = sum(slen(e) for e in self.old_table[k:])
        if raw >= CKPT_SPAN:
            return None, raw
        if raw:
            print("reading existing data...", file=sys.stderr)
        tail = bytearray()
        self.f.seek(off)
        for chunk in read_stream(PlainSource(self.f)):
            tail += chunk
            if len(tail) > 2 * WINDOW:
                del tail[:len(tail) - WINDOW]
        return bytes(tail[-WINDOW:]), raw

    def write_set(self):
        d, pw = self.set_dir, self.pw
        if self.set_v1:
            raise KilnError("this set's index is old: upgrade it with `kiln pack -esr ...` first")
        kept = {e["old"] for e in self.entries if e["old"] is not None}
        mapping = {}
        renamed = 0
        for e in self.entries:
            if e["old"] is not None:
                t = self.old_table[e["old"]]
                renamed += e["rel"] != t["rel"]
                mapping[t["name"]] = {"path": e["rel"], "size": e["size"], "mtime": e["mtime"]}
        gone = [t["name"] for i, t in enumerate(self.old_table)
                if i not in kept and not t.get("missing")]
        new = [e for e in self.entries if e["old"] is None]
        secrets.SystemRandom().shuffle(new)
        args = argparse.Namespace(jobs=self.jobs, encrypt=True, store=False, quiet=True,
                                  force=True, individual=False, random_names=False,
                                  _each=True, _pw=pw)
        import contextlib, io
        made, tmp = [], tempfile.mkdtemp(prefix="kiln-")
        try:
            for n, e in enumerate(new, 1):
                while True:
                    name = secrets.token_hex(8) + ".kln"
                    if name not in mapping and not os.path.exists(os.path.join(d, name)):
                        break
                print(f"  [{n}/{len(new)}] {e['rel']}", file=sys.stderr)
                if e.get("full"):
                    src = e["full"]
                    try:
                        st = os.stat(src)
                    except OSError as err:
                        raise KilnError(f"{e['rel']}: {err.strerror}")
                    size, mtime = st.st_size, int(st.st_mtime)
                else:      # a copy: decrypt the original and encrypt it again (new salt and key)
                    t = self.old_table[e["copy_of"]]
                    arc = os.path.join(d, t["name"])
                    if not os.path.isfile(arc):
                        raise KilnError(f"{e['rel']}: the archive {t['name']} is missing")
                    sub = os.path.join(tmp, str(n))
                    os.mkdir(sub)
                    extract_one(arc, sub, pw, True)
                    src = os.path.join(sub, os.listdir(sub)[0])
                    size, mtime = e["size"], e["mtime"]
                dest = os.path.join(d, name)
                args.archive, args.inputs = dest, [src]
                made.append(dest)
                with contextlib.redirect_stdout(io.StringIO()):
                    cmd_pack(args)
                mapping[name] = {"path": e["rel"], "size": size, "mtime": int(mtime)}
            write_index(d, mapping, args, pw)       # the index first: a crash leaves only orphans
        except BaseException:
            for p in made:
                try:
                    os.remove(p)
                except OSError:
                    pass
            raise
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
        for name in gone:
            try:
                os.remove(os.path.join(d, name))
            except OSError:
                pass
        print(f"saved: {len(new)} added, {len(gone)} removed, {renamed} renamed")
        self.load_set(d, pw)

    def write(self, target, compact=False):
        if self.set_dir:
            return self.write_set()
        kept = sorted((e for e in self.entries if e["old"] is not None), key=lambda e: e["old"])
        new = sorted((e for e in self.entries if e["old"] is None),
                     key=lambda e: (e["pre"], e["ext"], e["rel"]))
        keep = {e["old"]: e for e in kept}
        fields = ("rel", "size", "crc", "mtime", "mode", "filt", "slen")
        tmp = target + ".tmp"
        t0 = time.time()
        table, note = [], ""
        cleanup = lambda: [os.remove(tmp)] if os.path.exists(tmp) else None
        # Plain archives are saved by keeping the old data as it is: removed files become
        # hidden table entries, new files are appended as more blocks. Only encrypted
        # archives (fresh key per save, so nonces are never reused) or archives with a lot
        # of removed data are rewritten from scratch.
        dead_raw = sum(e["size"] for i, e in enumerate(self.old_table) if i not in keep)
        all_raw = sum(e["size"] for e in self.old_table) + sum(e["size"] for e in new)
        store = bool(self.store and self.encrypt)
        lazy = bool(self.f and not self.encrypt and not self.was_encrypted and not compact
                    and dead_raw <= max(RECLAIM_MIN, 0.3 * all_raw))
        if lazy and new:                  # the old data must end with the 1-byte end block
            self.f.seek(self.table_off - 1)
            if self.f.read(1) != b"\x02":
                lazy = False
        if lazy:
            for i, oe in enumerate(self.old_table):
                e = keep.get(i)
                table.append({k: e[k] for k in fields} if e
                              else dict({k: oe[k] for k in fields}, dead=True))
            pool = prog = None
            try:
                tail, ep_raw = self.old_epoch() if new else (b"", 0)
                if new:
                    prog = Progress(sum(e["size"] for e in new), "saving")
                    pool = make_pool(self.jobs)
                    start_mp3_ahead(new)
                with open(tmp, "wb") as out:
                    self.f.seek(0)
                    left = self.table_off - (1 if new else 0)
                    while left:
                        chunk = self.f.read(min(left, 1 << 20))
                        if not chunk:
                            raise KilnError("archive is truncated")
                        out.write(chunk)
                        left -= len(chunk)
                    if new:
                        enc = Encoder(PlainSink(out), prog.add, pool, self.jobs)
                        if tail is None:           # long stretch since the last checkpoint
                            enc.checkpoint(len(table))
                        else:
                            enc.history, enc.since = tail, ep_raw
                        for e in new:
                            enc.maybe_checkpoint(len(table))
                            size, crc, filt, sl = pack_file(enc, e)
                            table.append({"rel": e["rel"], "size": size, "crc": crc,
                                          "mtime": e["mtime"], "mode": e["mode"], "filt": filt,
                                          "slen": sl})
                        enc.finish()
                        note = core_note(enc, self.jobs) + note
                    table_off = out.tell()
                    tenc = Encoder(PlainSink(out))
                    tenc.feed(build_table_bytes(table, tuple(self.old_table.ckpts) + tuple(enc.ckpts)
                                                if new else self.old_table.ckpts))
                    tenc.finish()
                    out.write(struct.pack(">QQ4s", table_off,
                                          sum(e["size"] for e in table), MAGIC))
                os.replace(tmp, target)
            except BaseException:
                if prog:
                    prog.close()
                if pool:
                    (pool.shutdown(wait=False, cancel_futures=True), stop_mp3_ahead())
                cleanup()
                raise
            if pool:
                pool.shutdown()
            if prog:
                prog.close()
        else:
            total = sum(e["size"] for e in kept) + sum(e["size"] for e in new)
            prog = Progress(total, "saving")
            cipher, header = None, MAGIC + b"\x00"
            if self.encrypt:
                salt = os.urandom(16)  # fresh salt = fresh key on every save
                header = MAGIC + bytes([FLAG_ENCRYPTED, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P]) + salt
                print("deriving key...", file=sys.stderr)
                cipher = Cipher(derive_key(self.pw, salt, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P), header)
            fit_segment(total, self.jobs)
            pool = None if store else make_pool(self.jobs)
            if not store:
                start_mp3_ahead(new)
            try:
                with open(tmp, "wb") as out:
                    if cipher:
                        try:
                            os.chmod(tmp, 0o600)
                        except OSError:
                            pass
                    out.write(header)
                    enc = Encoder(cipher.sink(out, 0) if cipher else PlainSink(out),
                                  prog.add, pool, self.jobs, store=store)
                    if kept and self.f:
                        last_needed = kept[-1]["old"]
                        flts = {}
                        began = -1
                        for i, piece, last in stream_pieces(self.f, self.old_table, self.hdr_len,
                                                            self.cipher, packed=not store):
                            e = keep.get(i)
                            if e is not None:
                                if began != i:
                                    began = i
                                    enc.maybe_checkpoint(len(table))
                                if e["filt"] and e["filt"] != MP3_FID and not store:   # original bytes: filter again
                                    flt = flts.get(i) or flts.setdefault(i, make_filter(e["filt"], e["size"]))
                                    piece = flt.feed(piece) + (flt.finish() if last else b"")
                                    if last:
                                        del flts[i]
                                if piece:
                                    (enc.feed_stored if e["pre"] else enc.feed)(piece)
                                if last:
                                    table.append(dict({k: e[k] for k in fields}, filt=0,
                                                      slen=e["size"]) if store
                                                 else {k: e[k] for k in fields})
                            if last and i >= last_needed:
                                break
                    for e in new:
                        enc.maybe_checkpoint(len(table))
                        size, crc, filt, sl = pack_file(enc, e)
                        table.append({"rel": e["rel"], "size": size, "crc": crc,
                                      "mtime": e["mtime"], "mode": e["mode"], "filt": filt,
                                      "slen": sl})
                    enc.finish()
                    note = " [no compression]" if store else core_note(enc, self.jobs)
                    table_off = out.tell()
                    tenc = Encoder(cipher.sink(out, 1) if cipher else PlainSink(out))
                    tenc.feed(build_table_bytes(table, enc.ckpts))
                    tenc.finish()
                    out.write(struct.pack(">QQ4s", table_off,
                                          0 if cipher else sum(e["size"] for e in table), MAGIC))
                os.replace(tmp, target)
            except BaseException:
                prog.close()
                if pool:
                    (pool.shutdown(wait=False, cancel_futures=True), stop_mp3_ahead())
                cleanup()
                raise
            if pool:
                pool.shutdown()
            prog.close()
        live = [e for e in table if not e.get("dead")]
        raw = sum(e["size"] for e in live)
        asize = os.path.getsize(target)
        self.load(target, self.pw if self.encrypt else None)
        self.store = self.was_store = store
        stale = sum(e["size"] for e in table if e.get("dead"))
        if stale:
            note += f" [{fmt_size(stale)} of removed files still stored; 'compact' reclaims it]"
        print(f"saved {os.path.basename(target)}: {len(live)} file(s), {fmt_size(raw)} -> "
              f"{fmt_size(asize)} ({100 * asize / raw if raw else 100:.1f}%) in {time.time() - t0:.1f}s"
              + (" [encrypted]" if self.encrypt else "") + note)

    def do_compact(self, arg):
        """compact - rewrite the archive, dropping the data of removed files"""
        self.writable()
        if not self.f:
            raise KilnError("archive isn't saved yet")
        if not any(e.get("dead") for e in self.old_table) and not self.dirty:
            print("nothing to reclaim")
            return
        self.write(self.path, compact=True)

    # ------------------------------------------------------------ looking around
    def print_summary(self):
        if self.set_dir:
            raw = sum(e["size"] for e in self.entries)
            print(f"{os.path.basename(self.path)}: random-name set, {len(self.entries)} file(s), "
                  f"{fmt_size(raw)} [encrypted]")
            return
        raw = sum(e["size"] for e in self.entries)
        disk = os.path.getsize(self.path) if os.path.exists(self.path) else 0
        print(f"{os.path.basename(self.path)}: {len(self.entries)} file(s), {fmt_size(raw)}"
              + (f" -> {fmt_size(disk)} on disk ({100 * disk / raw:.1f}%)" if raw and disk else "")
              + (" [encrypted]" if self.encrypt else ""))

    def do_status(self, arg):
        """status - list unsaved changes"""
        self.need()
        added = [e for e in self.entries if e["old"] is None]
        kept = {e["old"] for e in self.entries if e["old"] is not None}
        removed = [e for i, e in enumerate(self.old_table) if i not in kept and not e.get("dead")]
        renamed = [e for e in self.entries if e["old"] is not None and self.mark(e) == "~"]
        crypto = self.encrypt != self.was_encrypted
        if not (added or removed or renamed or crypto or self.dirty):
            print("no unsaved changes")
            return
        for e in added[:15]:
            print(col("+ ", "32") + e["rel"] + f"  ({fmt_size(e['size'])})")
        for e in removed[:15]:
            print(col("- ", "31") + e["rel"])
        for e in renamed[:15]:
            print(col("~ ", "33") + self.old_table[e["old"]]["rel"] + " -> " + e["rel"])
        more = len(added) + len(removed) + len(renamed) - 45
        if more > 0:
            print(f"... and more")
        print(f"{len(added)} added, {len(removed)} removed, {len(renamed)} renamed"
              + ("; encryption will be " + ("ON" if self.encrypt else "OFF") if crypto else "")
              + "   (save to apply, discard to undo)")

    def do_info(self, arg):
        """info [NAME] - details about the archive or a file/folder"""
        self.need()
        p = self.split(arg)
        if not p:
            self.print_summary()
            print(f"  folder: /{self.cwd}   {self.jobs} job(s)")
            if self.dirty:
                print("  (unsaved changes - see `status`)")
            return
        sel, k = self.select(p[0])
        if not sel:
            raise KilnError(f"no such file or directory: {p[0]}")
        if self.is_dir(self.norm(p[0])) or len(sel) > 1:
            print(f"{len(sel)} file(s), {fmt_size(sum(e['size'] for e in sel))}")
            return
        e = sel[0]
        if self.set_dir and e["old"] is not None:
            t = self.old_table[e["old"]]
            print(f"name:     /{e['rel']}")
            print(f"size:     {e['size']} bytes ({fmt_size(e['size'])})")
            print(f"modified: {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(e['mtime']))}")
            print(f"archive:  {t['name']}" + ("   (MISSING)" if t.get("missing") else ""))
            return
        print(f"name:     /{e['rel']}")
        print(f"size:     {e['size']} bytes ({fmt_size(e['size'])})")
        print(f"modified: {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(e['mtime']))}")
        print(f"mode:     {e['mode'] & 0o777:o}")
        print("crc32:    " + (f"{e['crc']:08x}" if e["crc"] is not None else "(computed on save)"))
        print("status:   " + {"+": "new (unsaved)", "~": "renamed (unsaved)", " ": "stored"}[self.mark(e)]
              + (", precompressed type (anchor-probed only)" if e["pre"] else ""))
        if e.get("filt"):
            print("filter:   " + ("MP3 recompressed (lossless), packed to " + fmt_size(slen(e))
                                  if e["filt"] == MP3_FID else "executable branch filter" if e["filt"] in BCJ_FIDS
                                  else "WAV " + filter_desc(e["filt"])))

    def print_columns(self, items):
        if not items:
            return
        width = shutil.get_terminal_size((80, 24)).columns
        w = max(len(p) for p, _ in items) + 2
        ncol = max(1, width // w)
        nrow = -(-len(items) // ncol)
        for r in range(nrow):
            line = ""
            for c in range(ncol):
                idx = r + c * nrow
                if idx < len(items):
                    line += items[idx][1] + " " * (w - len(items[idx][0]))
            print(line.rstrip())

    def do_ls(self, arg):
        """ls [-l] [PATH|PATTERN ...] - list a folder (-l = sizes and dates)"""
        self.need()
        flags, pos = parse_flags(self.split(arg))
        for target in pos or ["."]:
            p = self.norm(target)
            if self.is_dir(p) and not any(ch in target for ch in "*?["):
                kids = self.children(p)
            else:
                sel, k = self.select(target)
                if not sel:
                    raise KilnError(f"no such file or directory: {target}")
                kids = self.group(sel, k)
            if len(pos) > 1:
                print(f"{target}:")
            names = sorted(kids, key=lambda n: (not kids[n]["dir"], n.lower()))
            if flags.get("l"):
                for n in names:
                    v = kids[n]
                    shown = n + ("/" if v["dir"] else "")
                    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(v["mtime"]))
                    print(f"{v['mark']} {fmt_size(v['size']):>9}  {when}  "
                          + (col(shown, "1;34") if v["dir"] else shown))
                print(f"  {len(names)} item(s), {fmt_size(sum(v['size'] for v in kids.values()))}")
            else:
                self.print_columns([(n + ("/" if kids[n]["dir"] else ""),
                                     col(n + "/", "1;34") if kids[n]["dir"] else n)
                                    for n in names])

    def do_ll(self, arg):
        self.do_ls("-l " + arg)

    def do_pwd(self, arg):
        """pwd - current folder inside the archive"""
        self.need()
        print("/" + self.cwd)

    def do_cd(self, arg):
        """cd [DIR] - change folder inside the archive (no argument = top)"""
        self.need()
        p = self.split(arg)
        new = self.norm(p[0]) if p else ""
        if not self.is_dir(new):
            raise KilnError(f"no such folder: {p[0]}")
        self.cwd = new

    def do_tree(self, arg):
        """tree [DIR] - show the folder structure"""
        self.need()
        p = self.split(arg)
        d = self.norm(p[0]) if p else self.cwd
        if not self.is_dir(d):
            raise KilnError(f"no such folder: {p[0]}")
        pre = d + "/" if d else ""
        root = {}
        for e in self.entries:
            if e["rel"].startswith(pre):
                parts = e["rel"][len(pre):].split("/")
                node = root
                for part in parts[:-1]:
                    node = node.setdefault(part, {})
                node[parts[-1]] = (e,)
        counts = [0, 0]

        def show(node, prefix):
            names = sorted(node, key=lambda n: (isinstance(node[n], tuple), n.lower()))
            for idx, n in enumerate(names):
                last = idx == len(names) - 1
                v = node[n]
                branch = ("`-- " if last else "|-- ")
                if isinstance(v, tuple):
                    counts[1] += 1
                    print(prefix + branch + n + f"  ({fmt_size(v[0]['size'])})")
                else:
                    counts[0] += 1
                    print(prefix + branch + col(n + "/", "1;34"))
                    show(v, prefix + ("    " if last else "|   "))
        print(col("/" + d, "1;34") if d else "/")
        show(root, "")
        print(f"{counts[0]} folder(s), {counts[1]} file(s)")

    def do_find(self, arg):
        """find PATTERN - search the whole archive by name (wildcards ok, case-insensitive)"""
        self.need()
        p = self.split(arg)
        if len(p) != 1:
            raise KilnError("usage: find PATTERN")
        pat = p[0].lower()
        if not any(ch in pat for ch in "*?["):
            pat = f"*{pat}*"
        hits = [e for e in self.entries
                if fnmatch.fnmatch(e["rel"].lower(), pat)
                or fnmatch.fnmatch(os.path.basename(e["rel"]).lower(), pat)]
        for e in sorted(hits, key=lambda e: e["rel"]):
            print(f"{fmt_size(e['size']):>9}  /{e['rel']}")
        print(f"{len(hits)} match(es)")

    def do_du(self, arg):
        """du [DIR] - sizes of what's in a folder (uncompressed)"""
        self.need()
        p = self.split(arg)
        d = self.norm(p[0]) if p else self.cwd
        if not self.is_dir(d):
            raise KilnError(f"no such folder: {p[0]}")
        kids = self.children(d)
        for n in sorted(kids, key=lambda n: -kids[n]["size"]):
            print(f"{fmt_size(kids[n]['size']):>9}  {n}" + ("/" if kids[n]["dir"] else ""))
        print(f"{fmt_size(sum(v['size'] for v in kids.values())):>9}  total")

    def do_cat(self, arg):
        """cat [-n BYTES] FILE - print a text file (first 64 KB by default)"""
        self.need()
        flags, pos = parse_flags(self.split(arg), valued=("n",))
        if len(pos) != 1:
            raise KilnError("usage: cat [-n BYTES] FILE")
        try:
            limit = int(flags.get("n", 65536))
        except ValueError:
            raise KilnError("-n needs a number")
        sel, k = self.select(pos[0])
        sel = [e for e in sel if len(e["rel"].split("/")) == k]
        if len(sel) != 1:
            raise KilnError(f"{pos[0]}: " + ("no such file" if not sel else "matches several files"))
        e = sel[0]
        data = bytearray()
        if self.set_dir and e["old"] is not None:
            data = self.set_head(e, limit)
        elif e["old"] is None:
            if not e.get("full"):
                raise KilnError(f"{pos[0]}: a copy that isn't saved yet; `save` first")
            with open(e["full"], "rb") as fh:
                data = bytearray(fh.read(limit + 1))
        else:
            for i, piece, last in stream_pieces(self.f, self.old_table, self.hdr_len, self.cipher,
                                                want=[e["old"]]):
                if i == e["old"]:
                    data += piece
                if (i == e["old"] and (last or len(data) > limit)) or i > e["old"]:
                    break
        if b"\0" in data[:1024]:
            raise KilnError(f"{pos[0]} looks binary ({fmt_size(e['size'])}); use `extract` instead")
        shown = bytes(data[:limit]).decode("utf-8", "replace")
        sys.stdout.write(shown + ("" if shown.endswith("\n") or not shown else "\n"))
        if e["size"] > limit:
            print(f"... ({fmt_size(e['size'] - limit)} more; use -n to show more)")

    def set_src(self, e):
        t = self.old_table[e["old"]]
        src = os.path.join(self.set_dir, t["name"])
        if not os.path.isfile(src):
            raise KilnError(f"{e['rel']}: the archive {t['name']} is missing")
        return src

    def set_head(self, e, limit):
        """The first limit+1 bytes of a file in a random-name set."""
        f, table, table_off, hdr_len, cipher = open_archive(self.set_src(e), self.pw)
        data = bytearray()
        with f:
            for i, piece, last in stream_pieces(f, table, hdr_len, cipher, want=[0]):
                data += piece
                if last or len(data) > limit:
                    break
        return data

    # ------------------------------------------------------------ changing things
    def do_add(self, arg):
        """add PATH... - add files/folders from the phone into the current archive folder"""
        self.writable(True)
        pos = self.split(arg)
        if not pos:
            raise KilnError("usage: add PATH...   (wildcards ok)")
        paths = []
        for p in pos:
            p = os.path.expanduser(p)
            hits = glob.glob(p) if any(ch in p for ch in "*?[") else [p]
            if not hits:
                raise KilnError(f"no such file or directory: {p}")
            paths += hits
        found = collect(paths, {self.path, self.path + ".tmp"})
        if self.set_dir:
            sd = self.set_dir + os.sep
            found = [e for e in found if not os.path.abspath(e["full"]).startswith(sd)]
        pre = self.cwd + "/" if self.cwd else ""
        names = {pre + e["rel"] for e in found}
        replaced = sum(1 for e in self.entries if e["rel"] in names)
        self.entries = [e for e in self.entries if e["rel"] not in names]
        for e in found:
            rel = pre + e["rel"]
            self.entries.append({"rel": rel, "size": e["size"], "crc": None, "mtime": e["mtime"],
                                 "mode": e["mode"], "filt": 0, "old": None, "full": e["full"],
                                 **self.info_of(rel)})
        self.dirty = bool(found) or self.dirty
        print(f"added {len(found)} file(s), {fmt_size(sum(e['size'] for e in found))}"
              + (f" ({replaced} replaced)" if replaced else "") + "   [not saved yet]")

    def do_put(self, arg):
        self.do_add(arg)

    def do_rm(self, arg):
        """rm [-r] PATTERN... - remove files (-r for folders); applied on save"""
        self.writable(True)
        flags, pos = parse_flags(self.split(arg))
        if not pos:
            raise KilnError("usage: rm [-r] PATTERN...")
        gone = set()
        for pat in pos:
            sel, k = self.select(pat)
            if not sel:
                raise KilnError(f"no match: {pat}")
            if any(len(e["rel"].split("/")) > k for e in sel) and not (flags.get("r") or flags.get("R")):
                raise KilnError(f"{pat} is a folder (use -r)")
            gone |= {id(e) for e in sel}
        n = len(gone)
        size = sum(e["size"] for e in self.entries if id(e) in gone)
        self.entries = [e for e in self.entries if id(e) not in gone]
        self.dirty = True
        print(f"removed {n} file(s), {fmt_size(size)}   [not saved yet; `discard` undoes]")

    def do_del(self, arg):
        self.do_rm(arg)

    def do_mv(self, arg):
        """mv SRC... DST - rename or move files/folders inside the archive"""
        self.writable(True)
        flags, pos = parse_flags(self.split(arg))
        if len(pos) < 2:
            raise KilnError("usage: mv SRC... DST")
        *srcs, dst = pos
        dstn = self.norm(dst)
        into = len(srcs) > 1 or self.is_dir(dstn)
        moves = {}
        for pat in srcs:
            sel, k = self.select(pat)
            if not sel:
                raise KilnError(f"no match: {pat}")
            for e in sel:
                segs = e["rel"].split("/")
                rest = segs[k:]
                base = segs[k - 1] if k else ""
                new = "/".join(([dstn] if dstn else []) + ([base] if into else []) + rest)
                if not new:
                    raise KilnError("can't move the archive root")
                moves[id(e)] = new
        targets = list(moves.values())
        if len(set(targets)) != len(targets):
            raise KilnError("two files would end up with the same name")
        staying = {e["rel"] for e in self.entries if id(e) not in moves}
        clash = [t for t in targets if t in staying]
        if clash:
            if not flags.get("f"):
                raise KilnError(f"destination exists: /{clash[0]}   (use -f to replace)")
            self.entries = [e for e in self.entries if id(e) in moves or e["rel"] not in clash]
        for e in self.entries:
            if id(e) in moves:
                e["rel"] = moves[id(e)]
                e.update(self.info_of(e["rel"]))
        self.dirty = True
        print(f"moved {len(moves)} file(s)   [not saved yet]")

    def do_rename(self, arg):
        self.do_mv(arg)

    def do_cp(self, arg):
        """cp [-f] SRC... DST - copy files/folders inside a random-name set; applied on save"""
        self.writable(True)
        if not self.set_dir:
            raise KilnError("cp works only inside a random-name set")
        flags, pos = parse_flags(self.split(arg))
        if len(pos) < 2:
            raise KilnError("usage: cp [-f] SRC... DST")
        *srcs, dst = pos
        dstn = self.norm(dst)
        into = len(srcs) > 1 or self.is_dir(dstn)
        copies = []
        for pat in srcs:
            sel, k = self.select(pat)
            if not sel:
                raise KilnError(f"no match: {pat}")
            for e in sel:
                segs = e["rel"].split("/")
                rest = segs[k:]
                base = segs[k - 1] if k else ""
                new = "/".join(([dstn] if dstn else []) + ([base] if into else []) + rest)
                if not new:
                    raise KilnError("can't copy to the archive root without a name")
                if e["old"] is not None and self.old_table[e["old"]].get("missing"):
                    raise KilnError(f"{e['rel']}: its archive is missing, can't copy")
                copies.append((e, new))
        targets = [n for _, n in copies]
        if len(set(targets)) != len(targets):
            raise KilnError("two files would end up with the same name")
        clash = [t for t in targets if any(e["rel"] == t for e in self.entries)]
        if clash:
            if not flags.get("f"):
                raise KilnError(f"destination exists: /{clash[0]}   (use -f to replace)")
            self.entries = [e for e in self.entries if e["rel"] not in clash]
        for e, new in copies:
            self.entries.append(dict(e, rel=new, old=None,
                                     copy_of=e["old"] if e["old"] is not None else e.get("copy_of"),
                                     **self.info_of(new)))
        self.dirty = True
        print(f"copied {len(copies)} file(s)   [not saved yet]")

    def do_passwd(self, arg):
        """passwd [-r] - set or change the password (-r = remove encryption); applied on save"""
        self.writable()
        flags, pos = parse_flags(self.split(arg))
        if flags.get("r"):
            self.encrypt, self.pw = False, None
            if self.store:
                self.store = False
                print("store turned off (it needs encryption)")
            self.dirty = self.dirty or self.was_encrypted or self.was_store
            print("encryption will be removed on save")
            return
        self.pw = get_password(confirm=True)
        self.encrypt = True
        self.dirty = True
        print("archive will be encrypted with the new password on save")

    # ------------------------------------------------------------ extracting
    def do_extract(self, arg):
        """extract [PATTERN...] [-o DIR] [-f] - extract files (default: this folder, everything below)"""
        self.need()
        flags, pos = parse_flags(self.split(arg), valued=("o",))
        root = os.path.abspath(os.path.expanduser(flags.get("o", ".")))
        if self.set_dir:
            return self.extract_set(pos, root, flags.get("f"))
        want, skipped = {}, 0
        for pat in pos or ["."]:
            sel, k = self.select(pat)
            if not sel:
                raise KilnError(f"no match: {pat}")
            for e in sel:
                if e["old"] is None:
                    skipped += 1
                else:
                    want[e["old"]] = e
        if not want:
            raise KilnError("nothing to extract (new files must be saved first)")
        dests, clashes = {}, []
        for i, e in want.items():
            dest = os.path.abspath(os.path.join(root, e["rel"]))
            if not _is_under(root, dest):
                raise KilnError(f"refusing unsafe path: {e['rel']}")
            dests[i] = dest
            if os.path.exists(dest):
                clashes.append(e["rel"])
        if clashes and not flags.get("f"):
            raise KilnError(f"{len(clashes)} file(s) already exist, e.g. {clashes[0]}; "
                            "nothing extracted (use -f to overwrite)")
        last_wanted = max(want)
        prog = Progress(sum(slen(e) for e in self.old_table), "extracting")
        t0 = time.time()
        cur, fh = None, None
        done = 0
        try:
            for i, piece, last in stream_pieces(self.f, self.old_table, self.hdr_len, self.cipher,
                                                want=want):
                prog.add(len(piece))
                if i in want:
                    if cur != i:
                        os.makedirs(os.path.dirname(dests[i]), exist_ok=True)
                        fh = open(dests[i], "wb")
                        cur = i
                    fh.write(piece)
                    if last:
                        fh.close()
                        fh = None
                        e = want[i]
                        try:
                            os.utime(dests[i], (e["mtime"], e["mtime"]))
                            os.chmod(dests[i], e["mode"] & 0o777 or 0o644)
                        except OSError:
                            pass
                        done += 1
                if last and i >= last_wanted:
                    break
        except BaseException:
            if fh:
                fh.close()
                try:
                    os.remove(dests[cur])
                except OSError:
                    pass
            raise
        finally:
            prog.close()
        print(f"extracted {done} file(s) to {root} in {time.time() - t0:.1f}s"
              + (f"   ({skipped} unsaved new file(s) skipped)" if skipped else ""))

    def extract_set(self, pos, root, force):
        chosen, skipped = {}, 0
        for pat in pos or ["."]:
            sel, k = self.select(pat)
            if not sel:
                raise KilnError(f"no match: {pat}")
            for e in sel:
                if e["old"] is None:
                    skipped += 1
                else:
                    chosen[e["old"]] = e
        if not chosen:
            raise KilnError("nothing to extract (new files must be saved first)")
        plan, missing, clashes = [], [], []
        for i in sorted(chosen):
            e = chosen[i]
            dest = safe_dest(root, e["rel"])
            if self.old_table[i].get("missing"):
                missing.append(e["rel"])
                continue
            if os.path.exists(dest):
                clashes.append(e["rel"])
            plan.append((e, dest))
        if clashes and not force:
            raise KilnError(f"{len(clashes)} file(s) already exist, e.g. {clashes[0]}; "
                            "nothing extracted (use -f to overwrite)")
        t0 = time.time()
        for n, (e, dest) in enumerate(plan, 1):
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            print(f"  [{n}/{len(plan)}] {e['rel']}", file=sys.stderr)
            try:
                extract_one(self.set_src(e), os.path.dirname(dest), self.pw, True,
                            os.path.basename(dest))
            except KilnError as err:
                raise KilnError(f"{e['rel']}: {err}")
        print(f"extracted {len(plan)} file(s) to {root} in {time.time() - t0:.1f}s"
              + (f"   ({skipped} unsaved new file(s) skipped)" if skipped else ""))
        if missing:
            raise KilnError(f"{len(missing)} archive(s) are missing: " + ", ".join(missing[:5])
                            + (" ..." if len(missing) > 5 else ""))

    def do_get(self, arg):
        self.do_extract(arg)

    def do_test(self, arg):
        """test - verify every checksum in the saved archive"""
        self.need()
        if self.set_dir:
            t0, bad = time.time(), []
            for n, t in enumerate(self.old_table, 1):
                if t.get("missing"):
                    bad.append(t["rel"])
                    continue
                print(f"  [{n}/{len(self.old_table)}] {t['rel']}", file=sys.stderr)
                extract_one(os.path.join(self.set_dir, t["name"]), ".", self.pw, False)
            print(f"OK: {len(self.old_table) - len(bad)} file(s), all checksums match "
                  f"({time.time() - t0:.1f}s)")
            if bad:
                raise KilnError(f"{len(bad)} archive(s) are missing: " + ", ".join(bad[:5]))
            return
        if not self.f:
            raise KilnError("archive isn't saved yet")
        prog = Progress(sum(slen(e) for e in self.old_table), "testing")
        t0 = time.time()
        try:
            for i, piece, last in stream_pieces(self.f, self.old_table, self.hdr_len,
                                                self.cipher, self.table_off):
                prog.add(len(piece))
        finally:
            prog.close()
        print(f"OK: {sum(not e.get('dead') for e in self.old_table)} file(s), all checksums match ({time.time() - t0:.1f}s)")

    # ------------------------------------------------------------ settings, phone side
    def do_store(self, arg):
        """store [on|off] - encryption only: save with NO compression (needs encryption; applied on save)"""
        self.writable()
        a = arg.strip().lower()
        if not a:
            print("store is " + ("ON (encryption only, no compression)" if self.store
                                 else "OFF (normal compression)")
                  + ("" if self.store == self.was_store else "; applied on save"))
            return
        if a not in ("on", "off"):
            raise KilnError("usage: store [on|off]")
        if a == "on":
            if not self.encrypt:
                raise KilnError("store needs encryption: set a password first with `passwd`")
            self.store = True
        else:
            self.store = False
        if self.store != self.was_store:
            self.dirty = True
        print("store " + a + ("; the archive will be rewritten on save"
                              if self.store != self.was_store else ""))

    def do_jobs(self, arg):
        """jobs [N] - show or set how many CPU cores saving uses"""
        p = self.split(arg)
        if p:
            if not (p[0].isdigit() and int(p[0]) >= 1):
                raise KilnError("jobs must be a positive number")
            self.jobs = int(p[0])
        print(f"{self.jobs} job(s)")

    def do_selftest(self, arg):
        """selftest - check that packing, extracting and encryption work on this device"""
        run_selftest()

    def do_wavcheck(self, arg):
        """wavcheck FILE - describe a .wav on the phone (is it float? is it integer audio in disguise?)"""
        p = self.split(arg)
        if not p:
            raise KilnError("usage: wavcheck FILE")
        for line in wav_report(os.path.expanduser(p[0])):
            print(line)

    def do_lpwd(self, arg):
        """lpwd - current folder on the phone"""
        print(os.getcwd())

    def do_lcd(self, arg):
        """lcd DIR - change folder on the phone"""
        p = self.split(arg)
        os.chdir(os.path.expanduser(p[0]) if p else os.path.expanduser("~"))
        print(os.getcwd())

    def do_lls(self, arg):
        """lls [-l] [PATH] - list a folder on the phone"""
        flags, pos = parse_flags(self.split(arg))
        for target in pos or ["."]:
            target = os.path.expanduser(target)
            if os.path.isdir(target):
                names = sorted(os.listdir(target), key=str.lower)
                base = target
            else:
                names, base = [target], "."
            rows = []
            for n in names:
                full = os.path.join(base, n) if base != "." or os.path.isdir(target) else n
                try:
                    st = os.stat(full)
                except OSError:
                    continue
                rows.append((n, os.path.isdir(full), st.st_size, st.st_mtime))
            if flags.get("l"):
                for n, d, size, mt in rows:
                    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(mt))
                    print(f"{fmt_size(size):>9}  {when}  " + (col(n + "/", "1;34") if d else n))
            else:
                self.print_columns([(n + ("/" if d else ""), col(n + "/", "1;34") if d else n)
                                    for n, d, _, _ in rows])

    def do_shell(self, arg):
        """!COMMAND - run a phone shell command"""
        import subprocess
        subprocess.run(arg, shell=True)

    # ------------------------------------------------------------ leaving, help
    def do_help(self, arg):
        """help [COMMAND]"""
        if arg.strip():
            super().do_help(arg.strip())
        else:
            print(SHELL_HELP, end="")

    def do_exit(self, arg):
        """exit - leave the shell (asks about unsaved changes)"""
        if self.dirty and self.path:
            try:
                ans = input("Unsaved changes. Save before exit? [y]es / [n]o / [c]ancel: ").strip().lower()
            except EOFError:
                ans = "n"
                print("\ndiscarding unsaved changes")
            if ans.startswith("c") or ans == "":
                return False
            if ans.startswith("y"):
                self.write(self.path)
        if self.f:
            self.f.close()
        return True

    do_quit = do_q = do_EOF = do_exit

    complete_ls = complete_ll = complete_cd = complete_tree = complete_du = _arch_complete
    complete_cat = complete_info = complete_rm = complete_del = complete_mv = _arch_complete
    complete_rename = complete_extract = complete_get = complete_cp = _arch_complete
    complete_add = complete_put = complete_lcd = complete_lls = _local_complete
    complete_open = complete_save = complete_new = complete_wavcheck = _local_complete


def run_shell(archive=None):
    sh = Shell()
    try:
        import readline
        readline.set_completer_delims(" \t\n")
        if "libedit" in (readline.__doc__ or ""):
            readline.parse_and_bind("bind ^I rl_complete")
        else:
            readline.parse_and_bind("tab: complete")
        hist = os.path.expanduser("~/.kiln_history")
        try:
            readline.read_history_file(hist)
        except OSError:
            pass
        import atexit
        atexit.register(lambda: (readline.set_history_length(500), readline.write_history_file(hist)))
    except (ImportError, OSError):
        pass
    if archive:
        sh.onecmd("open " + shlex.quote(archive))
        sh._prompt()
    if not sys.stdin.isatty():
        sh.intro = None
    sh.cmdloop()


def main():
    ap = argparse.ArgumentParser(description="kiln: a homemade compressor / archive format")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("pack", help="create an archive")
    p.add_argument("archive")
    p.add_argument("inputs", nargs="+")
    p.add_argument("-j", "--jobs", type=int, default=0, metavar="N",
                   help="CPU cores to use (default: up to 8, fewer if memory is short)")
    p.add_argument("-e", "--encrypt", action="store_true",
                   help="encrypt with AES-256-GCM (asks for a password)")
    p.add_argument("-s", "--store", action="store_true",
                   help="encryption only, no compression at all (requires -e)")
    p.add_argument("-i", "--individual", action="store_true",
                   help="one archive per file instead of one bundle; the first argument "
                        "becomes the output directory. Plain -i mirrors the folder layout; "
                        "with -e the files get random names (a1b2....kln) in one flat "
                        "directory and an encrypted _index.kln maps them back")
    p.add_argument("-r", "--random-names", action="store_true", help=argparse.SUPPRESS)
    p.add_argument("-f", "--force", action="store_true", help="overwrite an existing archive")
    p.add_argument("-q", "--quiet", action="store_true", help="no progress bar")
    p.set_defaults(fn=cmd_pack)

    u = sub.add_parser("unpack", help="extract an archive")
    u.add_argument("archive")
    u.add_argument("members", nargs="*", help="only extract these names (wildcards ok)")
    u.add_argument("-o", "--output", default=".")
    u.add_argument("-f", "--force", action="store_true", help="overwrite existing files")
    u.add_argument("-v", "--verbose", action="store_true", help="print each file")
    u.add_argument("-q", "--quiet", action="store_true", help="no progress bar")
    u.add_argument("-i", "--in-place", action="store_true",
                   help="random-name set only: decrypt the folder into itself, dropping the "
                        "archive's single top-level folder and deleting the .kln files")
    u.set_defaults(fn=cmd_unpack)

    l = sub.add_parser("list", help="show the contents (instant)")
    l.add_argument("archive")
    l.set_defaults(fn=cmd_list)

    t = sub.add_parser("test", help="check every checksum without extracting")
    t.add_argument("archive")
    t.add_argument("-q", "--quiet", action="store_true", help="no progress bar")
    t.set_defaults(fn=cmd_test)

    sh = sub.add_parser("shell", help="interactive shell for managing archives")
    sh.add_argument("archive", nargs="?")
    sh.set_defaults(fn=lambda a: run_shell(a.archive))

    st = sub.add_parser("selftest", help="check that kiln works on this device")
    st.set_defaults(fn=lambda a: sys.exit(0 if run_selftest() else 1))

    wc = sub.add_parser("wavcheck", help="describe a .wav file's samples")
    wc.add_argument("file")
    wc.set_defaults(fn=lambda a: print("\n".join(wav_report(a.file))))

    b = sub.add_parser("bench", help="compare kiln with zlib, bz2 and lzma")
    b.add_argument("inputs", nargs="+")
    b.set_defaults(fn=cmd_bench)

    argv = sys.argv[1:]
    if not argv or (len(argv) == 1 and argv[0] not in sub.choices and not argv[0].startswith("-")):
        try:
            require_accel()
            return run_shell(argv[0] if argv else None)
        except KilnError as e:
            sys.exit(f"error: {e}")
        except KeyboardInterrupt:
            sys.exit("\ninterrupted")
    args = ap.parse_args()
    try:
        require_accel()
        args.fn(args)
    except KilnError as e:
        sys.exit(f"error: {e}")
    except (struct.error, UnicodeDecodeError, MemoryError):
        sys.exit("error: archive is corrupt")
    except KeyboardInterrupt:
        sys.exit("\ninterrupted")



# ---------------------------------------------------------------- embedded C helper
# The C source lives inside this file, so kiln is a single file. The first run
# compiles it (about a second, with cc/gcc/clang) and caches the .so.
_ACCEL_SRC = r"""/* kiln_accel.c - optional C speed-ups for kiln.py (loaded through ctypes).
 *
 * kiln.py compiles this file the first time it is needed (cc/gcc/clang) and
 * stops with an error if that isn't possible: kiln has no pure-Python fallback.
 *
 *   kz_lz       LZ77 match finder (hash chains, lazy matching, chunk classes)
 *   kz_rc_enc/dec  adaptive range coder + LZ77 block decoder (bounds-checked)
 *   kz_wav16_*  WAV filter for 16-bit PCM (prediction + byte planes)
 *   kz_shuf_*   WAV byte planes (24/32/64-bit audio)
 *   kz_int_*    WAV integer predictor for 8/24/32-bit audio; kz_f2i/kz_i2f float-as-integer
 *   kz_rnd      chunk classifier (random / literal-only / sparse / normal)
 */
#include <stdint.h>
#include <stdlib.h>
#include <stddef.h>
#include <string.h>

#define KZ_VERSION 16

#define WINDOW ((1 << 26) - 1)
#define HIST_NEAR (1 << 20)
#define HIST_FAR_STRIDE 8
#define DBITS 32
#define DMASK ((((uint64_t)1) << DBITS) - 1)
#define HBITS 22
#define H3BITS 18           /* separate table: the nearest 3-byte match */
#define LBITS 22            /* long-match table: 8-byte hashes at every 4th position */
#define LONG_MIN 24
#define RND_BITS 16
#define MIN_MATCH 3
/* length-slot context: len 2 and 3 share one, then 4, 5, 6+ (v = len - 2) */
#define SLOTCTX(v) ((v) < 1 ? 0u : (v) < 4 ? (v) - 1 : 3u)
#define MAX_MATCH 258
#define DRY_CHAIN 8
#define SKIP_INSERT 192
#define LAZY_SKIP 64
#define MAX_STEP 32
#define MIN_RND_MATCH 24
#define SPARSE_MATCH 12
#define HIST_STRIDE 2
#define H6BITS 22
#define R6BITS 24          /* tree finder: the last 2^24 positions (a ring of tree nodes) */
#define R6MASK ((1 << R6BITS) - 1)
#define BTMODE 2
#define BTK 3
#define PRB_CHAIN 25
#define PRB_OPT 24
#define HISTD 4

/* ------------------------------------------------------------ executable filters
 * Same-length, exactly invertible transforms that turn relative branch targets into absolute
 * ones, so repeated calls to one function become repeated bytes. kz_bcj returns how many bytes
 * from the start are final (always n here: callers hand over whole independent blocks).
 * fid 121: x86 E8/E9 (call/jmp rel32), 122: AArch64 BL and ADRP. pos0 = file offset of b[0]. */
int64_t kz_bcj(int fid, uint8_t *b, int64_t n, int64_t pos0, int dec) {
    if (fid == 121) {
        /* encode right to left, decode left to right: each decision then sees the same bytes
         * on both sides even where call opcodes overlap an earlier operand */
        int64_t i = dec ? 0 : n - 5, step = dec ? 1 : -1;
        for (; i >= 0 && i + 4 < n; i += step) {
            if ((b[i] & 0xFE) == 0xE8 && (b[i + 4] == 0 || b[i + 4] == 0xFF)) {
                uint32_t v = (uint32_t)b[i + 1] | ((uint32_t)b[i + 2] << 8) | ((uint32_t)b[i + 3] << 16) | ((uint32_t)(b[i + 4] & 1) << 24);
                uint32_t pc = (uint32_t)(pos0 + i + 5);
                v = (dec ? v - pc : v + pc) & 0x01FFFFFFu;
                b[i + 1] = (uint8_t)v; b[i + 2] = (uint8_t)(v >> 8); b[i + 3] = (uint8_t)(v >> 16);
                b[i + 4] = (uint8_t)(0u - ((v >> 24) & 1));
            }
        }
        return n;
    }
    if (fid == 122) {
        int64_t i = 0;
        for (; i + 4 <= n; i += 4) {
            uint32_t x; memcpy(&x, b + i, 4);
            uint32_t pc = (uint32_t)((pos0 + i) >> 2);
            if ((x & 0xFC000000u) == 0x94000000u) {
                uint32_t v = (dec ? (x - pc) : (x + pc)) & 0x03FFFFFFu;
                x = 0x94000000u | v;
            } else if ((x & 0x9F000000u) == 0x90000000u) {
                uint32_t src = ((x >> 29) & 3u) | ((x >> 3) & 0x001FFFFCu);
                if (((src + 0x00020000u) & 0x001C0000u) == 0) {
                    uint32_t q = (uint32_t)((pos0 + i) >> 12);
                    uint32_t d = dec ? src - q : src + q;
                    x &= 0x9000001Fu;
                    x |= (d & 3u) << 29;
                    x |= (d & 0x0003FFFCu) << 3;
                    x |= (0u - (d & 0x00020000u)) & 0x00E00000u;
                }
            }
            memcpy(b + i, &x, 4);
        }
        return i;
    }
    return n;
}

int kz_version(void) { return KZ_VERSION; }

/* ------------------------------------------------------------ LZ77 */
typedef struct {
    const uint8_t *buf;
    int64_t n, start, last, ins;
    int32_t *head, *head3, *head2, *prev, *lhead, *head6, *prev6;
    int use6, use_bt, btd;
    int32_t *son;
    int64_t bmask, bt_next, pmask, plim;
    const uint8_t *rnd;
    int nice;
} ctx_t;

/* chain hash: 4 bytes (a 3-byte hash would mix far more unrelated positions into each chain) */
static inline uint32_t h4(const uint8_t *p) {
    uint32_t x;
    memcpy(&x, p, 4);
    return (x * 2654435761u) >> (32 - HBITS);
}
static inline uint32_t h3s(const uint8_t *p) {
    return ((((uint32_t)p[0] << 16) | ((uint32_t)p[1] << 8) | p[2]) * 2654435761u) >> (32 - H3BITS);
}

/* content-defined anchor: ~1 in 256 positions, the same wherever a repeated blob sits */
static inline int anchorR(const uint8_t *p) {
    return ((((uint32_t)p[0] << 16) | ((uint32_t)p[1] << 8) | p[2]) * 2654435761u) >> (32 - 4) == 0;
}
static inline int anchor3(const uint8_t *p) {
    return ((((uint32_t)p[0] << 16) | ((uint32_t)p[1] << 8) | p[2]) * 2654435761u) >> 24 == 0;
}

/* long-chain hash: 8 bytes, so every entry of that chain shares a long prefix with the position */
static inline uint32_t h6s(const uint8_t *p) {
    uint64_t x;
    memcpy(&x, p, 8);
    return (uint32_t)((x * 0x9E3779B97F4A7C15ull) >> (64 - H6BITS));
}


/* Binary-tree match finder (as in LZMA's bt4): one tree per 4-byte hash, every inserted
 * position is a node whose two sons are older positions that sort before/after it. Walking
 * the tree finds each longer match in turn (lengths and distances both ascending) and
 * inserts `pos` as the new root in the same pass. rec=0 only inserts. */
static int bt_run(ctx_t *c, int64_t pos, int depth, uint32_t *pl, uint32_t *pd, int np, int best, int rec) {
    const uint8_t *b = c->buf;
    int64_t rem = c->n - pos;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int32_t cur = c->head[h4(b + pos)];
    int32_t *son = c->son;
    int64_t mask = c->bmask;
    int32_t *ptr1 = son + ((pos & mask) << 1);
    int32_t *ptr0 = ptr1 + 1;
    int len0 = 0, len1 = 0, nice = c->nice;
    const uint8_t *cb = b + pos;
    for (;;) {
        if (cur < 0 || depth-- <= 0 || pos - cur > mask) { *ptr0 = *ptr1 = -1; break; }
        int32_t *pair = son + (((int64_t)cur & mask) << 1);
        const uint8_t *pb = b + cur;
        int len = len0 < len1 ? len0 : len1;
        if (pb[len] == cb[len]) {
            len++;
            while (len + 8 <= limit) {
                uint64_t x, y;
                memcpy(&x, pb + len, 8);
                memcpy(&y, cb + len, 8);
                if (x != y) { len += __builtin_ctzll(x ^ y) >> 3; goto ext_done; }
                len += 8;
            }
            while (len < limit && pb[len] == cb[len]) len++;
        ext_done:
            if (len > best) {
                best = len;
                if (rec) { pl[np] = (uint32_t)len; pd[np] = (uint32_t)(pos - cur); np++; }
            }
            if (len >= limit || len >= nice) { *ptr1 = pair[0]; *ptr0 = pair[1]; break; }
        }
        if (pb[len] < cb[len]) { *ptr1 = cur; ptr1 = pair + 1; cur = *ptr1; len1 = len; }
        else { *ptr0 = cur; ptr0 = pair; cur = *ptr0; len0 = len; }
    }
    return np;
}

static inline void put1(ctx_t *c, int64_t p) {
    if (c->use6 && p + 8 <= c->n) {
        uint32_t h6 = h6s(c->buf + p);
        c->prev6[p & R6MASK] = c->head6[h6];
        c->head6[h6] = (int32_t)p;
    }
    uint32_t h = h4(c->buf + p);
    if (c->use_bt && p >= c->bt_next && p + 4 <= c->n) {
        bt_run(c, p, p < c->start ? HISTD : c->btd, NULL, NULL, 0, 1 << 30, 0);
        c->bt_next = p + 1;
    }
    c->prev[p & c->pmask] = c->head[h];
    c->head[h] = (int32_t)p;
    c->head3[h3s(c->buf + p)] = (int32_t)p;
    c->head2[((unsigned)c->buf[p] << 8) | c->buf[p + 1]] = (int32_t)p;
}

static inline uint32_t h8(const uint8_t *p) {
    uint64_t x;
    memcpy(&x, p, 8);
    return (uint32_t)((x * 0x9E3779B97F4A7C15ull) >> (64 - LBITS));
}

/* long-match table: every 4th position in [a, b) (not in random chunks) */
static void lput(ctx_t *c, int64_t a, int64_t b) {
    int64_t lim = c->n - 8;
    a = (a + 3) & ~(int64_t)3;
    for (; a < b && a <= lim; a += 4) c->lhead[h8(c->buf + a)] = (int32_t)a;
}

/* hash every position below `upto` that hasn't been hashed yet */
static void insert_to(ctx_t *c, int64_t upto) {
    int64_t p = c->ins;
    int64_t stop = upto < c->last ? upto : c->last;
    while (p < stop) {
        int64_t ch = p >> RND_BITS;
        int64_t q = (ch + 1) << RND_BITS;
        if (q > stop) q = stop;
        uint8_t k = c->rnd[ch];
        if (k != 1 && k != 2) lput(c, p, q);
        if (k == 2) {
            /* literal-only chunk: hash only content-defined anchors */
            const uint8_t *b = c->buf;
            for (int64_t a = p; a < q; a++)
                if (anchor3(b + a)) put1(c, a);
        } else if (k == 1) {
            /* random chunk: only anchors (0x00 bytes) */
            const uint8_t *b = c->buf;
            for (int64_t a = p; a < q; a++)
                if (anchorR(b + a)) put1(c, a);
        } else if (p < c->start) {
            int64_t q2 = q < c->start ? q : c->start;
            int64_t nearp = c->start - HIST_NEAR;
            int64_t a = p;
            for (; a < q2 && a < nearp; a += HIST_FAR_STRIDE) put1(c, a);
            for (; a < q2; a += HIST_STRIDE) put1(c, a);
            for (int64_t a2 = q2; a2 < q; a2++) put1(c, a2);
        } else {
            for (int64_t a = p; a < q; a++) put1(c, a);
        }
        p = q;
    }
    if (upto > c->ins) c->ins = upto;
}

static void find_chain(ctx_t *c, int64_t i, int chain, int *ol, int *od) {
    *ol = 0;
    *od = 0;
    if (i >= c->last) return;
    const uint8_t *b = c->buf;
    int32_t j = c->head[h4(b + i)];
    int best_len = 0, best_dist = 0;
    int64_t rem = c->n - i;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    while (j >= 0 && chain) {
        if (i - j > c->plim) break;
        if (best_len == 0 || b[j + best_len] == b[i + best_len]) {
            int l = 0;
            while (l + 8 <= limit) {
                uint64_t x, y;
                memcpy(&x, b + j + l, 8);
                memcpy(&y, b + i + l, 8);
                if (x != y) break;
                l += 8;
            }
            while (l < limit && b[j + l] == b[i + l]) l++;
            if (l > best_len) {
                best_len = l;
                best_dist = (int)(i - j);
                if (l >= limit || l >= c->nice) break;
            }
        }
        j = c->prev[j & c->pmask];
        chain--;
    }
    if (best_len < 3) {                      /* the nearest 3-byte match */
        int32_t j3 = c->head3[h3s(b + i)];
        if (j3 >= 0 && i - j3 <= WINDOW) {
            int l = 0;
            while (l < limit && b[j3 + l] == b[i + l]) l++;
            if (l >= 3) { best_len = l; best_dist = (int)(i - j3); }
        }
    }
    /* cost filter: a far short match costs more bits than literals */
    if (best_len < MIN_MATCH || (best_len == 3 && best_dist > 8192) ||
        (best_len == 4 && best_dist > 131072) ||
        (best_len <= 5 && best_dist > (1 << 20)) ||
        (best_len <= 7 && best_dist > (1 << 22)))
        return;
    if (best_len < SPARSE_MATCH && c->rnd[i >> RND_BITS] == 3) return;
    *ol = best_len;
    *od = best_dist;
}

/* chain search, then (when that found nothing long) one probe of the long-match table */
static void find(ctx_t *c, int64_t i, int chain, int *ol, int *od) {
    find_chain(c, i, chain, ol, od);
    if (*ol >= LONG_MIN || i + 8 > c->n) return;
    int32_t j = c->lhead[h8(c->buf + i)];
    if (j < 0 || j >= i || i - j > WINDOW) return;
    const uint8_t *b = c->buf;
    int64_t rem = c->n - i;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int l = 0;
    while (l + 8 <= limit) {
        uint64_t x, y;
        memcpy(&x, b + j + l, 8);
        memcpy(&y, b + i + l, 8);
        if (x != y) break;
        l += 8;
    }
    while (l < limit && b[j + l] == b[i + l]) l++;
    if (l >= LONG_MIN && l > *ol) { *ol = l; *od = (int)(i - j); }
}

/* how far the match at distance d continues from pos */
static int run_len(const uint8_t *b, int64_t pos, int64_t n, int d) {
    int64_t rem = n - pos;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int l = 0;
    while (l + 8 <= limit) {
        uint64_t x, y;
        memcpy(&x, b + pos - d + l, 8);
        memcpy(&y, b + pos + l, 8);
        if (x != y) break;
        l += 8;
    }
    while (l < limit && b[pos - d + l] == b[pos + l]) l++;
    return l;
}

/* ------------------------------------------------------------ range coder */
/* Adaptive binary range coder (LZMA-style carry handling) over the LZ77 tokens.
 * Per block, models start fresh. Each probability is two 16-bit estimators (fast
 * and slow) averaged to 12 bits. Literals use the previous byte's top RC_LC bits
 * as context; right after a match they are coded against the byte the last
 * distance would have predicted ("matched literal"). A match whose distance is one
 * of the last four used (rep0..rep3) is coded as a rep; length 1 at rep0 is a
 * "short rep". Other lengths and distances use bit-trees (distance slot =
 * bit_length(d-1), top 3 extra bits modelled, the low 4 "align" bits modelled,
 * the rest direct).
 * Token rules: a literal is 0..255; a match is (len << 32) | dist with len >= 3,
 * except that a rep distance may have len 2 and rep0 may have len 1. */
#define RC_TOP (1u << 24)
#define RC_LC 3
#ifndef RC_PB
#define RC_PB 2          /* position bits (mod 2^RC_PB) in the match/literal state contexts */
#endif
#define RC_LPMAX 2       /* literal position bits: 0 or 2, chosen per block (stored in the first payload byte) */
#define RC_NS (8 << RC_PB)
#define PHS(x) ((unsigned)(x) & ((1u << RC_PB) - 1))
#define LCTX(m, prev, x) (((unsigned)(prev) >> (8 - RC_LC)) + (8u * ((unsigned)(x) & (m)->lpm)))
#ifndef RC_RA
#define RC_RA 4
#define RC_RB 7
#endif
#ifndef RC_K
#define RC_K 1.6
#endif
#define RC_NMAX 255
#ifndef RC_N0
#define RC_N0 0
#endif
typedef struct { uint16_t a, b; uint8_t n; } rcp_t;
typedef struct { rcp_t ch[2], low[8], mid[8], high[256]; } lenc_t;
typedef struct {
    rcp_t ism[RC_NS], isrep[RC_NS], isrep0[RC_NS], isrep1[RC_NS], isrep2[RC_NS], isrep0l[RC_NS];
    rcp_t lit[(1 << RC_LC) << RC_LPMAX][0x300];
    lenc_t ml, rl;
    rcp_t slot[4][32];
    rcp_t ext[27][8];
    rcp_t align[16];
    unsigned lpm;           /* literal position mask; keep last (rc_init skips it) */
    struct mix_s *mix;      /* literal mixer (NULL = off) */
} rcm_t;

/* Literal mixer: for a literal that does not follow a match, the bit probability is a
 * logistic mix of three models: the usual one (previous byte's top bits + position),
 * the full previous byte, and a hash of the previous two bytes. */
#define MIX_N 3
#define MIX_SHIFT 10
typedef struct mix_s { rcp_t t1[256][256]; rcp_t t2[4096][256]; int32_t w[256][MIX_N]; int32_t wm[256][MIX_N]; } mix_t;

/* count-based adaptive rate: a state moves by 1/(n+RC_K) until the rate reaches its floor
 * (2^-RC_RA fast, 2^-RC_RB slow), so a fresh context learns quickly and a seasoned one is steady */
static uint16_t RCR_A[256], RCR_B[256];
static int rcr_ready;
static void rcr_init(void) {
    for (int n = 0; n < 256; n++) {
        double r = 1.0 / (n + RC_K);
        double fa = 1.0 / (1 << RC_RA), fb = 1.0 / (1 << RC_RB);
        RCR_A[n] = (uint16_t)((r > fa ? r : fa) * 65536.0 + 0.5);
        RCR_B[n] = (uint16_t)((r > fb ? r : fb) * 65536.0 + 0.5);
    }
    rcr_ready = 1;
}
static inline uint32_t rc_p(const rcp_t *p) { return ((uint32_t)p->a + p->b) >> 5; }
static inline void rc_up(rcp_t *p, int bit) {
    unsigned n = p->n;
    if (n < RC_NMAX) p->n = (uint8_t)(n + 1);
    int ra = RCR_A[n], rb = RCR_B[n];
    if (bit) { p->a -= (uint16_t)(((uint32_t)p->a * ra) >> 16); p->b -= (uint16_t)(((uint32_t)p->b * rb) >> 16); }
    else { p->a += (uint16_t)(((uint32_t)(65535 - p->a) * ra) >> 16); p->b += (uint16_t)(((uint32_t)(65535 - p->b) * rb) >> 16); }
}
static void rc_init(rcm_t *m) {
    if (!rcr_ready) rcr_init();
    rcp_t *p = (rcp_t *)m;
    for (size_t i = 0; i < offsetof(rcm_t, lpm) / sizeof(rcp_t); i++) { p[i].a = 32768; p[i].b = 32768; p[i].n = RC_N0; }
    m->lpm = 0;
    m->mix = NULL;
}

static int16_t MIXSTR[4096];
static uint16_t MIXSQ[4096];
static int mix_ready;
static int mix_sq(int d) {
    static const int t[33] = {1,2,3,6,10,16,27,45,73,120,194,310,488,747,1101,1546,2047,2549,2994,3348,3607,3785,3901,3975,4022,4050,4068,4079,4085,4089,4092,4093,4094};
    if (d > 2047) return 4095;
    if (d < -2047) return 0;
    int w = d & 127;
    d = (d >> 7) + 16;
    return (t[d] * (128 - w) + t[d + 1] * w + 64) >> 7;
}
static void mix_tables(void) {
    int pi = 0;
    for (int x = -2047; x <= 2047; x++) {
        int v = mix_sq(x);
        for (int i = pi; i <= v; i++) MIXSTR[i] = (int16_t)x;
        pi = v + 1;
    }
    for (int i = pi; i < 4096; i++) MIXSTR[i] = 2047;
    for (int i = 0; i < 4096; i++) MIXSQ[i] = (uint16_t)mix_sq(i - 2048);
    mix_ready = 1;
}
static mix_t *mix_new(void) {
    if (!mix_ready) mix_tables();
    if (!rcr_ready) rcr_init();
    mix_t *x = (mix_t *)malloc(sizeof(mix_t));
    if (!x) return NULL;
    rcp_t *p = (rcp_t *)x;
    for (size_t i = 0; i < offsetof(mix_t, w) / sizeof(rcp_t); i++) { p[i].a = 32768; p[i].b = 32768; p[i].n = RC_N0; }
    for (int c = 0; c < 256; c++) { x->w[c][0] = 26000; x->w[c][1] = 20000; x->w[c][2] = 20000; x->wm[c][0] = 36000; x->wm[c][1] = 14000; x->wm[c][2] = 14000; }
    return x;
}
static inline unsigned mix_h2(unsigned prev2, unsigned prev) { return ((prev2 * 0x9E3779B1u) ^ (prev * 0x85EBCA6Bu)) >> 20; }
/* one bit: gathers the three inputs, returns P(bit = 0) in 1..4095; *ps receives what update needs */
static inline int mix_pred(mix_t *x, rcp_t *a, rcp_t *b, rcp_t *c, int32_t *w, int *s) {
    s[0] = MIXSTR[rc_p(a)]; s[1] = MIXSTR[rc_p(b)]; s[2] = MIXSTR[rc_p(c)];
    int64_t dot = (int64_t)s[0] * w[0] + (int64_t)s[1] * w[1] + (int64_t)s[2] * w[2];
    int d = (int)(dot >> 16);
    if (d > 2047) d = 2047; else if (d < -2047) d = -2047;
    int p = MIXSQ[d + 2048];
    return p < 1 ? 1 : p > 4095 ? 4095 : p;
}
static inline void mix_upd(rcp_t *a, rcp_t *b, rcp_t *c, int32_t *w, const int *s, int p, int bit) {
    int err = (bit ? 0 : 4095) - p;
    for (int k = 0; k < MIX_N; k++) w[k] += (s[k] * err) >> MIX_SHIFT;
}

typedef struct {
    uint64_t low;
    uint32_t range;
    uint8_t cache;
    int64_t csize, pos, cap;
    uint8_t *out;
    int err;
} renc_t;

static void rc_put(renc_t *e, uint8_t v) {
    if (e->pos < e->cap) e->out[e->pos++] = v; else e->err = 1;
}
static void rc_shift(renc_t *e) {
    if ((uint32_t)e->low < 0xFF000000u || (e->low >> 32)) {
        uint8_t carry = (uint8_t)(e->low >> 32), t = e->cache;
        do { rc_put(e, (uint8_t)(t + carry)); t = 0xFF; } while (--e->csize);
        e->cache = (uint8_t)(e->low >> 24);
    }
    e->csize++;
    e->low = (e->low & 0x00FFFFFFu) << 8;
}
static inline void rc_bit(renc_t *e, rcp_t *p, int bit) {
    uint32_t bound = (e->range >> 12) * rc_p(p);
    if (!bit) e->range = bound; else { e->low += bound; e->range -= bound; }
    rc_up(p, bit);
    while (e->range < RC_TOP) { e->range <<= 8; rc_shift(e); }
}
static inline void rc_bitp(renc_t *e, uint32_t p, int bit) {
    uint32_t bound = (e->range >> 12) * p;
    if (!bit) e->range = bound; else { e->low += bound; e->range -= bound; }
    while (e->range < RC_TOP) { e->range <<= 8; rc_shift(e); }
}
static inline void rc_mixlit(renc_t *e, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2, unsigned byte) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned c0 = 1;
    for (int i = 7; i >= 0; i--) {
        int bit = (byte >> i) & 1, s[MIX_N];
        int32_t *w = x->w[c0];
        int p = mix_pred(x, &lp[c0], &t1[c0], &t2[c0], w, s);
        rc_bitp(e, (uint32_t)p, bit);
        mix_upd(&lp[c0], &t1[c0], &t2[c0], w, s, p, bit);
        rc_up(&lp[c0], bit); rc_up(&t1[c0], bit); rc_up(&t2[c0], bit);
        c0 = (c0 << 1) | (unsigned)bit;
    }
}
static inline void rc_mixlitm(renc_t *e, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2, unsigned mb, unsigned byte) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned offs = 0x100, sym = 1;
    for (int i = 7; i >= 0; i--) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int bit = (byte >> i) & 1, s[MIX_N];
        int32_t *w = x->wm[sym];
        rcp_t *a = &lp[offs + mbit + sym];
        int p = mix_pred(x, a, &t1[sym], &t2[sym], w, s);
        rc_bitp(e, (uint32_t)p, bit);
        mix_upd(&lp[0], &t1[0], &t2[0], w, s, p, bit);
        rc_up(a, bit); rc_up(&t1[sym], bit); rc_up(&t2[sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
}
static inline void rc_direct(renc_t *e, uint32_t v, int n) {
    for (int i = n - 1; i >= 0; i--) {
        e->range >>= 1;
        if ((v >> i) & 1) e->low += e->range;
        while (e->range < RC_TOP) { e->range <<= 8; rc_shift(e); }
    }
}
static inline void rc_tree(renc_t *e, rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1;
    for (int i = nbits - 1; i >= 0; i--) {
        int b = (v >> i) & 1;
        rc_bit(e, &t[m], b);
        m = (m << 1) | b;
    }
}
static inline void rc_rtree(renc_t *e, rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1;
    for (int i = 0; i < nbits; i++) {
        int b = (v >> i) & 1;
        rc_bit(e, &t[m], b);
        m = (m << 1) | b;
    }
}
static inline void rc_len(renc_t *e, lenc_t *l, uint32_t v) {
    if (v < 8) { rc_bit(e, &l->ch[0], 0); rc_tree(e, l->low, 3, v); }
    else if (v < 16) { rc_bit(e, &l->ch[0], 1); rc_bit(e, &l->ch[1], 0); rc_tree(e, l->mid, 3, v - 8); }
    else { rc_bit(e, &l->ch[0], 1); rc_bit(e, &l->ch[1], 1); rc_tree(e, l->high, 8, v - 16); }
}
static inline void rc_dist(renc_t *e, rcm_t *m, uint32_t v, uint32_t dist) {
    uint32_t d = dist - 1;
    int slot = 0;
    while (slot < 32 && (d >> slot)) slot++;
    rc_tree(e, m->slot[SLOTCTX(v)], 5, (uint32_t)slot);
    if (slot >= 2) {
        int eb = slot - 1;
        uint32_t x = d - (1u << eb);
        int nm = eb < 3 ? eb : 3, r = eb - nm;
        rc_tree(e, m->ext[slot], nm, x >> r);
        if (r >= 4) {
            rc_direct(e, (x >> 4) & ((1u << (r - 4)) - 1), r - 4);
            rc_rtree(e, m->align, 4, x & 15);
        } else if (r > 0) {
            rc_direct(e, x & ((1u << r) - 1), r);
        }
    }
}
/* literal coded against mb, the byte a rep0 match would have produced */
static inline void rc_litm(renc_t *e, rcp_t *p, unsigned mb, unsigned byte) {
    unsigned offs = 0x100, sym = 1;
    for (int i = 7; i >= 0; i--) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int bit = (byte >> i) & 1;
        rc_bit(e, &p[offs + mbit + sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
}

static inline int rep_find(const uint32_t *rep, uint32_t len, uint32_t dist) {
    if (len < 2) return -1;
    for (int k = 0; k < 4; k++) if (rep[k] == dist) return k;
    return -1;
}

/* Encode one token (updating the models, the history state and *pos). Returns 1 if the
 * token breaks the rules (see above), else 0. */
static inline int enc_tok(renc_t *e, rcm_t *m, uint64_t t, const uint8_t *buf, int64_t pos0,
                          int64_t *ppos, unsigned *pst, uint32_t *rep) {
    int64_t pos = *ppos;
    unsigned st = *pst;
    unsigned sp = st + 8 * PHS(pos - pos0);
    if (t < 256) {
        unsigned prev = pos > pos0 ? buf[pos - 1] : 0;
        rc_bit(e, &m->ism[sp], 0);
        rcp_t *lp = m->lit[LCTX(m, prev, pos - pos0)];
        if (st & 1) { if (m->mix) rc_mixlitm(e, m->mix, lp, prev, pos - 1 > pos0 ? buf[pos - 2] : 0, buf[pos - rep[0]], t); else rc_litm(e, lp, buf[pos - rep[0]], t); }
        else if (m->mix) rc_mixlit(e, m->mix, lp, prev, pos - 1 > pos0 ? buf[pos - 2] : 0, t);
        else rc_tree(e, lp, 8, t);
        pos++;
        st = (st << 1) & 7;
    } else {
        uint32_t len = (uint32_t)(t >> DBITS), dist = (uint32_t)(t & DMASK);
        int ri = rep_find(rep, len, dist);
        if (len < 2 && ri < 0 && !(len == 1 && dist == rep[0])) return 1;
        rc_bit(e, &m->ism[sp], 1);
        if (ri >= 0 || len == 1) {
            rc_bit(e, &m->isrep[sp], 1);
            if (len == 1) {
                rc_bit(e, &m->isrep0[sp], 0);
                rc_bit(e, &m->isrep0l[sp], 0);
            } else {
                if (ri == 0) {
                    rc_bit(e, &m->isrep0[sp], 0);
                    rc_bit(e, &m->isrep0l[sp], 1);
                } else {
                    rc_bit(e, &m->isrep0[sp], 1);
                    if (ri == 1) rc_bit(e, &m->isrep1[sp], 0);
                    else { rc_bit(e, &m->isrep1[sp], 1); rc_bit(e, &m->isrep2[sp], ri == 3); }
                    for (int k = ri; k > 0; k--) rep[k] = rep[k - 1];
                    rep[0] = dist;
                }
                rc_len(e, &m->rl, len - 2);
            }
        } else {
            rc_bit(e, &m->isrep[sp], 0);
            uint32_t v = len - 2;
            rc_len(e, &m->ml, v);
            rc_dist(e, m, v, dist);
            rep[3] = rep[2]; rep[2] = rep[1]; rep[1] = rep[0]; rep[0] = dist;
        }
        pos += len;
        st = ((st << 1) | 1) & 7;
    }
    *ppos = pos;
    *pst = st;
    return 0;
}

/* Encode a token block. buf[pos0..] holds the bytes the tokens produce (only the byte
 * before each token is read, as literal context; before the first token it counts as 0).
 * Returns the payload size, or -1 if it doesn't fit in cap (or a token breaks the rules).
 * *raw_out = bytes the tokens expand to. */
int64_t kz_rc_enc(const uint64_t *tok, int64_t nt, const uint8_t *buf, int64_t pos0,
                  uint8_t *out, int64_t cap, int64_t *raw_out, int lp) {
    if (lp < 0 || (lp & 3) > RC_LPMAX || (lp >> 2) > 1) return -1;
    int64_t total = 0;
    for (int64_t i = 0; i < nt; i++) total += tok[i] < 256 ? 1 : (int64_t)(tok[i] >> DBITS);
    *raw_out = total;
    rcm_t *m = (rcm_t *)malloc(sizeof(rcm_t));
    if (!m) return -1;
    rc_init(m);
    m->lpm = (1u << (lp & 3)) - 1;
    if (lp >> 2) { m->mix = mix_new(); if (!m->mix) { free(m); return -1; } }
    renc_t e = {0, 0xFFFFFFFFu, 0, 1, 0, cap, out, 0};
    int64_t pos = pos0;
    unsigned st = 0;
    uint32_t rep[4] = {1, 1, 1, 1};
    for (int64_t i = 0; i < nt && !e.err; i++)
        if (enc_tok(&e, m, tok[i], buf, pos0, &pos, &st, rep)) { e.err = 1; break; }
    for (int k = 0; k < 5; k++) rc_shift(&e);
    free(m->mix);
    free(m);
    if (e.err || e.pos < 1) return -1;
    out[0] = (uint8_t)lp;      /* the coder's first byte is always 0; it carries the literal position bits */
    return e.pos;
}

typedef struct {
    const uint8_t *pl;
    int64_t plen, p;
    uint32_t range, code;
} rdec_t;

static inline uint8_t rd_byte(rdec_t *d) {
    uint8_t b = d->p < d->plen ? d->pl[d->p] : 0;
    d->p++;
    return b;
}
static inline int rd_bit(rdec_t *d, rcp_t *p) {
    uint32_t bound = (d->range >> 12) * rc_p(p);
    int bit;
    if (d->code < bound) { d->range = bound; bit = 0; }
    else { d->code -= bound; d->range -= bound; bit = 1; }
    rc_up(p, bit);
    while (d->range < RC_TOP) { d->range <<= 8; d->code = (d->code << 8) | rd_byte(d); }
    return bit;
}
static inline int rd_bitp(rdec_t *d, uint32_t p) {
    uint32_t bound = (d->range >> 12) * p;
    int bit;
    if (d->code < bound) { d->range = bound; bit = 0; }
    else { d->code -= bound; d->range -= bound; bit = 1; }
    while (d->range < RC_TOP) { d->range <<= 8; d->code = (d->code << 8) | rd_byte(d); }
    return bit;
}
static inline unsigned rd_mixlit(rdec_t *d, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned c0 = 1;
    for (int i = 0; i < 8; i++) {
        int s[MIX_N];
        int32_t *w = x->w[c0];
        int p = mix_pred(x, &lp[c0], &t1[c0], &t2[c0], w, s);
        int bit = rd_bitp(d, (uint32_t)p);
        mix_upd(&lp[c0], &t1[c0], &t2[c0], w, s, p, bit);
        rc_up(&lp[c0], bit); rc_up(&t1[c0], bit); rc_up(&t2[c0], bit);
        c0 = (c0 << 1) | (unsigned)bit;
    }
    return c0 & 0xFF;
}
static inline unsigned rd_mixlitm(rdec_t *d, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2, unsigned mb) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned offs = 0x100, sym = 1;
    while (sym < 0x100) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int s[MIX_N];
        int32_t *w = x->wm[sym];
        rcp_t *a = &lp[offs + mbit + sym];
        int p = mix_pred(x, a, &t1[sym], &t2[sym], w, s);
        int bit = rd_bitp(d, (uint32_t)p);
        mix_upd(&lp[0], &t1[0], &t2[0], w, s, p, bit);
        rc_up(a, bit); rc_up(&t1[sym], bit); rc_up(&t2[sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
    return sym & 0xFF;
}
static inline uint32_t rd_direct(rdec_t *d, int n) {
    uint32_t v = 0;
    for (int i = 0; i < n; i++) {
        d->range >>= 1;
        int b = d->code >= d->range;
        if (b) d->code -= d->range;
        v = (v << 1) | (uint32_t)b;
        while (d->range < RC_TOP) { d->range <<= 8; d->code = (d->code << 8) | rd_byte(d); }
    }
    return v;
}
static inline uint32_t rd_tree(rdec_t *d, rcp_t *t, int nbits) {
    uint32_t m = 1;
    for (int i = 0; i < nbits; i++) m = (m << 1) | (uint32_t)rd_bit(d, &t[m]);
    return m - (1u << nbits);
}
static inline uint32_t rd_rtree(rdec_t *d, rcp_t *t, int nbits) {
    uint32_t m = 1, v = 0;
    for (int i = 0; i < nbits; i++) {
        int b = rd_bit(d, &t[m]);
        m = (m << 1) | (uint32_t)b;
        v |= (uint32_t)b << i;
    }
    return v;
}
static inline uint32_t rd_len(rdec_t *d, lenc_t *l) {
    if (!rd_bit(d, &l->ch[0])) return rd_tree(d, l->low, 3);
    if (!rd_bit(d, &l->ch[1])) return 8 + rd_tree(d, l->mid, 3);
    return 16 + rd_tree(d, l->high, 8);
}

/* Decode a range-coded block, appending raw bytes at out[op..op+raw). Same error codes as kz_decode. */
int kz_rc_dec(const uint8_t *pl, int64_t plen, uint8_t *out, int64_t outcap, int64_t op, int64_t raw) {
    int64_t target = op + raw, op0 = op;
    if (raw < 0 || target > outcap) return -4;
    if (plen < 5 || (pl[0] & 3) > RC_LPMAX || (pl[0] >> 2) > 1) return -1;
    rcm_t *m = (rcm_t *)malloc(sizeof(rcm_t));
    if (!m) return -7;
    rc_init(m);
    m->lpm = (1u << (pl[0] & 3)) - 1;
    if (pl[0] >> 2) { m->mix = mix_new(); if (!m->mix) { free(m); return -7; } }
    rdec_t d = {pl, plen, 1, 0xFFFFFFFFu, 0};
    for (int k = 0; k < 4; k++) d.code = (d.code << 8) | rd_byte(&d);
    unsigned st = 0;
    uint32_t rep[4] = {1, 1, 1, 1};
    int rc = 0;
    while (op < target) {
        if (d.p > plen + 8) { rc = -2; break; }
        unsigned sp = st + 8 * PHS(op - op0);
        if (!rd_bit(&d, &m->ism[sp])) {
            unsigned prev = op > op0 ? out[op - 1] : 0;
            rcp_t *lp = m->lit[LCTX(m, prev, op - op0)];
            if ((st & 1) && m->mix) {
                out[op] = (uint8_t)rd_mixlitm(&d, m->mix, lp, prev, op - 1 > op0 ? out[op - 2] : 0, out[op - rep[0]]);
                op++;
            } else             if (st & 1) {
                unsigned mb = out[op - rep[0]], offs = 0x100, sym = 1;
                while (sym < 0x100) {
                    mb <<= 1;
                    unsigned mbit = mb & offs;
                    int bit = rd_bit(&d, &lp[offs + mbit + sym]);
                    sym = (sym << 1) | (unsigned)bit;
                    if (bit) offs &= mbit; else offs &= ~mbit;
                }
                out[op++] = (uint8_t)sym;
            } else {
                if (m->mix) {
                    unsigned v = rd_mixlit(&d, m->mix, lp, prev, op - 1 > op0 ? out[op - 2] : 0);
                    out[op++] = (uint8_t)v;
                }
                else out[op++] = (uint8_t)rd_tree(&d, lp, 8);
            }
            st = (st << 1) & 7;
            continue;
        }
        int64_t len;
        uint32_t dist;
        if (rd_bit(&d, &m->isrep[sp])) {
            if (!rd_bit(&d, &m->isrep0[sp])) {
                dist = rep[0];
                if (!rd_bit(&d, &m->isrep0l[sp])) len = 1;
                else len = (int64_t)rd_len(&d, &m->rl) + 2;
            } else {
                int ri;
                if (!rd_bit(&d, &m->isrep1[sp])) ri = 1;
                else ri = 2 + rd_bit(&d, &m->isrep2[sp]);
                dist = rep[ri];
                for (int k = ri; k > 0; k--) rep[k] = rep[k - 1];
                rep[0] = dist;
                len = (int64_t)rd_len(&d, &m->rl) + 2;
            }
        } else {
            uint32_t v = rd_len(&d, &m->ml);
            len = (int64_t)v + 2;
            uint32_t slot = rd_tree(&d, m->slot[SLOTCTX(v)], 5);
            if (slot > 26) { rc = -3; break; }
            uint32_t dd = slot;                       /* slots 0 and 1: d = slot */
            if (slot >= 2) {
                int eb = (int)slot - 1, nm = eb < 3 ? eb : 3, r = eb - nm;
                uint32_t x = rd_tree(&d, m->ext[slot], nm) << r;
                if (r >= 4) {
                    x |= rd_direct(&d, r - 4) << 4;
                    x |= rd_rtree(&d, m->align, 4);
                } else if (r > 0) {
                    x |= rd_direct(&d, r);
                }
                dd = (1u << eb) + x;
            }
            dist = dd + 1;
            rep[3] = rep[2]; rep[2] = rep[1]; rep[1] = rep[0]; rep[0] = dist;
        }
        if ((int64_t)dist > op) { rc = -3; break; }
        if (op + len > target) { rc = -4; break; }
        const uint8_t *src = out + op - dist;
        uint8_t *dst = out + op;
        if ((int64_t)dist >= len) memcpy(dst, src, (size_t)len);
        else for (int64_t k = 0; k < len; k++) dst[k] = src[k];
        op += len;
        st = ((st << 1) | 1) & 7;
    }
    free(m->mix);
    free(m);
    if (rc) return rc;
    if (d.p > plen) return -5;
    return 0;
}

/* ------------------------------------------------------------ parser state & prices */
/* At the optimal-parsing levels (lazy == 2) the parser keeps a live copy of the range
 * coder's models, updated as tokens are committed, and prices every candidate with it
 * (cost in 1/64 bit). It also tracks the coder's rep distances and history state, so a
 * token with len < 3 (only valid as a rep) is never emitted when the coder couldn't
 * code it: the coder resets at each block (every BLK tokens of a part). */
#define BLK 524288            /* tokens per range-coded block (BLOCK_TOKENS in kiln.py) */
#define OPTW 2048             /* positions parsed per optimal-parse window */
#define PINF 0x3FFFFFFF
static double log2_(double x);

static uint16_t PRC[4097];
static int prc_ready;
static void prc_init(void) {
    for (int p = 1; p <= 4096; p++) PRC[p] = (uint16_t)(-log2_(p / 4096.0) * 64.0 + 0.5);
    PRC[0] = PRC[1];
    prc_ready = 1;
}
static inline int pb(const rcp_t *p, int bit) {
    uint32_t q = rc_p(p);
    return PRC[bit ? 4096 - q : q];
}
static inline int ptree(const rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1; int s = 0;
    for (int i = nbits - 1; i >= 0; i--) { int b = (v >> i) & 1; s += pb(&t[m], b); m = (m << 1) | (uint32_t)b; }
    return s;
}
static inline int prtree(const rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1; int s = 0;
    for (int i = 0; i < nbits; i++) { int b = (v >> i) & 1; s += pb(&t[m], b); m = (m << 1) | (uint32_t)b; }
    return s;
}
static int plen(const lenc_t *l, uint32_t v) {
    if (v < 8) return pb(&l->ch[0], 0) + ptree(l->low, 3, v);
    if (v < 16) return pb(&l->ch[0], 1) + pb(&l->ch[1], 0) + ptree(l->mid, 3, v - 8);
    return pb(&l->ch[0], 1) + pb(&l->ch[1], 1) + ptree(l->high, 8, v - 16);
}
static int pdist(const rcm_t *m, uint32_t v, uint32_t dist) {
    uint32_t d = dist - 1;
    int slot = 0;
    if (d) slot = 32 - __builtin_clz(d);
    int s = ptree(m->slot[SLOTCTX(v)], 5, (uint32_t)slot);
    if (slot >= 2) {
        int eb = slot - 1;
        uint32_t x = d - (1u << eb);
        int nm = eb < 3 ? eb : 3, r = eb - nm;
        s += ptree(m->ext[slot], nm, x >> r);
        if (r >= 4) s += (r - 4) * 64 + prtree(m->align, 4, x & 15);
        else s += r * 64;
    }
    return s;
}
static inline int plit(const rcm_t *m, unsigned lctx, unsigned byte, int matched, unsigned mb) {
    const rcp_t *p = m->lit[lctx];
    if (!matched) return ptree(p, 8, byte);
    unsigned offs = 0x100, sym = 1; int s = 0;
    for (int i = 7; i >= 0; i--) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int bit = (byte >> i) & 1;
        s += pb(&p[offs + mbit + sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
    return s;
}

typedef struct {
    int32_t price, from;
    uint32_t len, dist;
    uint32_t rep[4];
    uint8_t st;
} onode_t;

typedef struct {
    const uint8_t *buf;
    uint64_t *tok;
    int64_t nt, pstart;
    int learn;
    rcm_t *m;
    renc_t e;
    uint8_t scr[512];
    uint32_t rep[4];
    unsigned st;
    int64_t pos, bpos0, lastref;
    int mlp[257], rlp[257];
    onode_t *nd;
    uint32_t *stk;
    int16_t dsl[4][32], dex[27][8], dal[16];   /* distance prices, fixed for one window */
} lzs_t;

/* The models only change between windows (tokens are emitted after a window is parsed), so the
 * distance prices are tabulated once per window instead of walking the bit trees per candidate. */
static void dtab(lzs_t *s) {
    const rcm_t *m = s->m;
    for (int c = 0; c < 4; c++) for (int sl = 0; sl < 32; sl++) s->dsl[c][sl] = (int16_t)ptree(m->slot[c], 5, (uint32_t)sl);
    for (int sl = 2; sl < 27; sl++) {
        int eb = sl - 1, nm = eb < 3 ? eb : 3;
        for (int x = 0; x < (1 << nm); x++) s->dex[sl][x] = (int16_t)ptree(m->ext[sl], nm, (uint32_t)x);
    }
    for (int a = 0; a < 16; a++) s->dal[a] = (int16_t)prtree(m->align, 4, (uint32_t)a);
}
static inline int pdist2(const lzs_t *s, uint32_t v, uint32_t dist) {
    uint32_t d = dist - 1;
    int slot = d ? 32 - __builtin_clz(d) : 0;
    int r = s->dsl[SLOTCTX(v)][slot];
    if (slot >= 2) {
        int eb = slot - 1;
        uint32_t x = d - (1u << eb);
        int nm = eb < 3 ? eb : 3, rr = eb - nm;
        r += s->dex[slot][x >> rr];
        r += rr >= 4 ? (rr - 4) * 64 + s->dal[x & 15] : rr * 64;
    }
    return r;
}

static void lzs_reset_block(lzs_t *s) {
    s->rep[0] = s->rep[1] = s->rep[2] = s->rep[3] = 1;
    s->st = 0;
    s->bpos0 = s->pos;
}

/* append a token; at the optimal levels also feed it to the live models */
static void emit(lzs_t *s, uint64_t t) {
    if (!s->learn) { s->tok[s->nt++] = t; return; }
    if ((s->nt - s->pstart) % BLK == 0) lzs_reset_block(s);
    if (t >= 256) {
        uint32_t len = (uint32_t)(t >> DBITS), dist = (uint32_t)(t & DMASK);
        if (len < 2 && rep_find(s->rep, len, dist) < 0 && !(len == 1 && dist == s->rep[0])) {
            for (uint32_t k = 0; k < len; k++) emit(s, s->buf[s->pos]);   /* not a valid rep here */
            return;
        }
    }
    s->tok[s->nt++] = t;
    s->e.pos = 0; s->e.err = 0; s->e.csize = 1;
    enc_tok(&s->e, s->m, t, s->buf, s->bpos0, &s->pos, &s->st, s->rep);
}

static void refresh_prices(lzs_t *s) {
    for (uint32_t v = 0; v < 257; v++) s->mlp[v] = plen(&s->m->ml, v);
    for (uint32_t v = 0; v < 257; v++) s->rlp[v] = plen(&s->m->rl, v);
    s->lastref = s->nt;
}

/* all matches at i that are longer than every nearer one: lens ascending, dists ascending */
static int find_all(ctx_t *c, int64_t i, int chain, uint32_t *pl, uint32_t *pd) {
    if (i >= c->last) return 0;
    const uint8_t *b = c->buf;
    int64_t rem = c->n - i;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int best = 2, np = 0;
    {
        int32_t j3 = c->head3[h3s(b + i)];
        if (j3 >= 0 && i - j3 <= WINDOW) {
            int l = 0;
            while (l < limit && b[j3 + l] == b[i + l]) l++;
            if (l >= 3) { best = l; pl[0] = (uint32_t)l; pd[0] = (uint32_t)(i - j3); np = 1;
                          if (l >= limit || l >= c->nice) return np; }
        }
    }
#define TRY(J) do {                                                         \
        int32_t jj_ = (J);                                                  \
        if (b[jj_ + best] == b[i + best]) {                                 \
            int l = 0;                                                      \
            while (l + 8 <= limit) {                                        \
                uint64_t x, y;                                              \
                memcpy(&x, b + jj_ + l, 8);                                 \
                memcpy(&y, b + i + l, 8);                                   \
                if (x != y) break;                                          \
                l += 8;                                                     \
            }                                                               \
            while (l < limit && b[jj_ + l] == b[i + l]) l++;                \
            if (l > best) {                                                 \
                best = l;                                                   \
                pl[np] = (uint32_t)l; pd[np] = (uint32_t)(i - jj_); np++;   \
                if (l >= limit || l >= c->nice) return np;                  \
            }                                                               \
        }                                                                   \
    } while (0)
    if (c->use_bt && c->bt_next == i && i + 4 <= c->n && BTMODE == 2) {
        np = bt_run(c, i, c->btd, pl, pd, np, best, 1);
        c->bt_next = i + 1;
        if (np) best = (int)pl[np - 1];
        if (np && best >= c->nice) return np;
        goto long_tbl;
    }
    int32_t j = c->head[h4(b + i)];
    int32_t jlast = 0x7FFFFFFF;
    int s4 = chain / 4;
    if (s4 < 2) s4 = 2;
    while (j >= 0 && s4) {
        if (i - j > c->plim) { j = -1; break; }
        int32_t jn = c->prev[j & c->pmask];
        if (jn >= 0) __builtin_prefetch(b + jn, 0, 1);
        TRY(j);
        jlast = j;
        j = jn;
        s4--;
    }
    if (j >= 0 && c->use6 && i + 8 <= c->n) {
        /* the nearest occurrences of these 4 bytes are done: go further back along the long chain */
        int32_t k = c->head6[h6s(b + i)];
        int skip = 64;
        while (k >= 0 && k >= jlast && skip--) {
            if (i - k > R6MASK) { k = -1; break; }
            int32_t kn = c->prev6[k & R6MASK];
            if (kn >= k) { k = -1; break; }
            k = kn;
        }
        int s6 = chain / 3;
        while (k >= 0 && s6 && k < jlast) {
            if (i - k > WINDOW || i - k > R6MASK) break;
            int32_t kn = c->prev6[k & R6MASK];
            if (kn >= k) kn = -1;
            if (kn >= 0) __builtin_prefetch(b + kn, 0, 1);
            TRY(k);
            k = kn;
            s6--;
        }
    }
long_tbl:
    if (best < LONG_MIN && i + 8 <= c->n) {
        int32_t k = c->lhead[h8(b + i)];
        if (k >= 0 && k < i && i - k <= WINDOW) {
            int l = 0;
            while (l + 8 <= limit) {
                uint64_t x, y;
                memcpy(&x, b + k + l, 8);
                memcpy(&y, b + i + l, 8);
                if (x != y) break;
                l += 8;
            }
            while (l < limit && b[k + l] == b[i + l]) l++;
            if (l >= LONG_MIN && l > best) { pl[np] = (uint32_t)l; pd[np] = (uint32_t)(i - k); np++; }
        }
    }
    return np;
}

static inline void relax(onode_t *nd, int *lenEnd, int cur, int t, int price, uint32_t len,
                         uint32_t dist, uint32_t r0, uint32_t r1, uint32_t r2, uint32_t r3, unsigned st) {
    while (*lenEnd < t) nd[++*lenEnd].price = PINF;
    onode_t *q = &nd[t];
    if (price < q->price) {
        q->price = price; q->from = cur; q->len = len; q->dist = dist;
        q->rep[0] = r0; q->rep[1] = r1; q->rep[2] = r2; q->rep[3] = r3; q->st = (uint8_t)st;
    }
}

/* price of the "rep k" selector bits (after is_match=1 and is_rep=1), k = 0..3 */
static inline int psel(const rcm_t *m, unsigned sp, int k) {
    if (k == 0) return pb(&m->isrep0[sp], 0) + pb(&m->isrep0l[sp], 1);
    if (k == 1) return pb(&m->isrep0[sp], 1) + pb(&m->isrep1[sp], 0);
    return pb(&m->isrep0[sp], 1) + pb(&m->isrep1[sp], 1) + pb(&m->isrep2[sp], k == 3);
}

/* Parse a window starting at i0 for the cheapest token path, emit it, return the new position. */
static int64_t opt_window(lzs_t *s, ctx_t *c, int64_t i0, int chain, int nice, int64_t wend) {
    const uint8_t *b = c->buf;
    int64_t n = c->n;
    const rcm_t *m = s->m;
    onode_t *nd = s->nd;
    if (s->nt - s->lastref >= 256 || s->lastref == 0) refresh_prices(s);
    dtab(s);
    int lim = (int)(wend - i0 < OPTW ? wend - i0 : OPTW);
    if (lim < 1) lim = 1;
    nd[0].price = 0; nd[0].from = 0;
    for (int k = 0; k < 4; k++) nd[0].rep[k] = s->rep[k];
    nd[0].st = (uint8_t)s->st;
    int cur = 0, lenEnd = 0, fin = -1;
    uint32_t pl[MAX_MATCH], pd[MAX_MATCH];
    for (;;) {
        if (cur > 0 && cur >= lenEnd) { fin = cur; break; }
        if (cur >= lim) { fin = cur; break; }
        int64_t pos = i0 + cur;
        onode_t *N = &nd[cur];
        int P = N->price;
        unsigned st = N->st;
        unsigned sp = st + 8 * PHS(pos - s->bpos0);
        uint32_t R0 = N->rep[0], R1 = N->rep[1], R2 = N->rep[2], R3 = N->rep[3];
        int stop = 0;
        if (pos + 24 < n) {
            /* the tables are far bigger than the cache: start the loads a few positions early */
            __builtin_prefetch(&c->head[h4(b + pos + 12)], 1, 1);
            __builtin_prefetch(&c->lhead[h8(b + pos + 12)], 0, 1);
            if (c->use6) __builtin_prefetch(&c->head6[h6s(b + pos + 12)], 1, 1);
            int32_t jh = c->head[h4(b + pos + 5)];
            if (jh >= 0) { __builtin_prefetch(&c->prev[jh & c->pmask], 0, 1); __builtin_prefetch(b + jh, 0, 1); }
        }
        insert_to(c, pos);
        /* literal */
        {
            unsigned prev = pos > 0 ? b[pos - 1] : 0;
            int matched = (st & 1) && R0 <= (uint32_t)pos;
            int cost = pb(&m->ism[sp], 0) +
                       plit(m, LCTX(m, prev, pos - s->bpos0), b[pos], matched, matched ? b[pos - R0] : 0);
            relax(nd, &lenEnd, cur, cur + 1, P + cost, 1, 0, R0, R1, R2, R3, (st << 1) & 7);
        }
        int base1 = pb(&m->ism[sp], 1);
        int nst = ((st << 1) | 1) & 7;
        /* reps */
        {
            uint32_t rr[4] = {R0, R1, R2, R3};
            int isrep1 = base1 + pb(&m->isrep[sp], 1);
            for (int k = 0; k < 4 && !stop; k++) {
                uint32_t d = rr[k];
                int dup = 0;
                for (int j = 0; j < k; j++) if (rr[j] == d) dup = 1;
                if (dup || d > (uint64_t)pos) continue;
                int l = run_len(b, pos, n, (int)d);
                if (k == 0 && l >= 1)
                    relax(nd, &lenEnd, cur, cur + 1,
                          P + isrep1 + pb(&m->isrep0[sp], 0) + pb(&m->isrep0l[sp], 0), 1, d,
                          R0, R1, R2, R3, nst);
                if (l < 2) continue;
                int sel = P + isrep1 + psel(m, sp, k);
                uint32_t a0 = d, a1 = k == 0 ? R1 : R0, a2 = k <= 1 ? R2 : R1, a3 = k <= 2 ? R3 : R2;
                if (k == 0) { a1 = R1; a2 = R2; a3 = R3; }
                else if (k == 1) { a1 = R0; a2 = R2; a3 = R3; }
                else if (k == 2) { a1 = R0; a2 = R1; a3 = R3; }
                else { a1 = R0; a2 = R1; a3 = R2; }
                for (int L = 2; L <= l; L++)
                    relax(nd, &lenEnd, cur, cur + L, sel + s->rlp[L - 2], (uint32_t)L, d, a0, a1, a2, a3, nst);
                if (l >= nice) { fin = cur + l; stop = 1; }
            }
        }
        if (stop) break;
        /* new matches */
        if (pos < c->last) {
            int np = find_all(c, pos, chain, pl, pd);
            int basem = base1 + pb(&m->isrep[sp], 0);
            uint32_t prevL = 2;
            for (int p = 0; p < np; p++) {
                uint32_t d = pd[p], Lp = pl[p];
                if (d == R0 || d == R1 || d == R2 || d == R3) { prevL = Lp; continue; }  /* a rep: done above */
                int dc[4] = {-1, -1, -1, -1};
                for (uint32_t L = prevL + 1; L <= Lp; L++) {
                    if (L < 3) continue;
                    uint32_t v = L - 2;
                    int ctx = (int)SLOTCTX(v);
                    if (dc[ctx] < 0) dc[ctx] = pdist2(s, v, d);
                    relax(nd, &lenEnd, cur, cur + (int)L, P + basem + s->mlp[v] + dc[ctx], L, d,
                          d, R0, R1, R2, nst);
                }
                prevL = Lp;
            }
            /* a length-2 match at the nearest earlier occurrence of these 2 bytes */
            {
                int32_t j2 = c->head2[((unsigned)b[pos] << 8) | b[pos + 1]];
                if (j2 >= 0 && j2 < pos && pos - j2 <= WINDOW) {
                    uint32_t d2 = (uint32_t)(pos - j2);
                    if (!(d2 == R0 || d2 == R1 || d2 == R2 || d2 == R3))
                        relax(nd, &lenEnd, cur, cur + 2, P + basem + s->mlp[0] + pdist2(s, 0, d2), 2, d2,
                              d2, R0, R1, R2, nst);
                }
            }
            if (np && pl[np - 1] >= (uint32_t)nice) { fin = cur + (int)pl[np - 1]; break; }
        }
        cur++;
    }
    /* trace back and emit */
    int ne = 0;
    for (int x = fin; x > 0; x = nd[x].from) { s->stk[2 * ne] = nd[x].len; s->stk[2 * ne + 1] = nd[x].dist; ne++; }
    int64_t p = i0;
    for (int k = ne - 1; k >= 0; k--) {
        uint32_t len = s->stk[2 * k], dist = s->stk[2 * k + 1];
        if (dist == 0) emit(s, b[p]);
        else {
            emit(s, ((uint64_t)len << DBITS) | (uint64_t)dist);
            if (len >= SKIP_INSERT && p + len > c->ins) { insert_to(c, p + 4); c->ins = p + len; }
        }
        p += len;
    }
    return p;
}

/* Returns the number of tokens written to tok (a literal byte, or
 * (length << 23) | distance), or -1 on failure. A raw run (bytes stored
 * uncompressed) is reported as a pair in runs[]: (token index, byte count).
 * lazy: 0 greedy, 1 lazy matching, 2 optimal parsing. At lazy == 2 a match may also be
 * a rep (length 2 or 3.. at one of the last 4 distances, or length 1 at the last one). */
int64_t kz_lz(const uint8_t *buf, int64_t n, int64_t start, const uint8_t *rnd,
              int64_t nch, int chain_max, int nice, int lazy, int accel,
              uint64_t *tok, int64_t *runs, int64_t runcap, int64_t *nruns_out, int lp) {
    ctx_t c;
    lzs_t S;
    memset(&S, 0, sizeof S);
    c.buf = buf; c.n = n; c.start = start; c.last = n - 3; c.ins = 0;
    c.rnd = rnd; c.nice = nice;
    c.lhead = (int32_t *)malloc(sizeof(int32_t) << LBITS);
    c.head = (int32_t *)malloc(sizeof(int32_t) * ((size_t)1 << HBITS) + sizeof(int32_t) * (((size_t)1 << H3BITS) + 65536));
    c.head3 = c.head + ((size_t)1 << HBITS);
    c.head2 = c.head3 + ((size_t)1 << H3BITS);
    /* the hash-chain links are a ring, not one slot per byte: chain levels reach back 2^PRB_CHAIN (32 MB);
     * the optimal-parse levels only use it for short fallback chains, so 2^PRB_OPT is plenty */
    { int cap = lazy >= 2 ? PRB_OPT : PRB_CHAIN, bb = 10; while (bb < cap && ((int64_t)1 << bb) < n + 32) bb++;
      c.pmask = ((int64_t)1 << bb) - 1;
      c.plim = (((int64_t)1 << bb) >= n + 32) ? (int64_t)WINDOW : (c.pmask - 16 < (int64_t)WINDOW ? c.pmask - 16 : (int64_t)WINDOW); }
    c.prev = (int32_t *)malloc(sizeof(int32_t) * (size_t)(c.pmask + 1));
    c.use_bt = lazy >= 2; c.use6 = lazy >= 2 && !(BTMODE == 2);
    c.btd = chain_max * BTK / 4; c.bt_next = 0;
    { int bb = 1; while (bb < 24 && ((int64_t)1 << bb) < n) bb++; c.bmask = ((int64_t)1 << bb) - 1; }
    c.son = c.use_bt ? (int32_t *)malloc(sizeof(int32_t) * 2 * (size_t)(c.bmask + 1)) : (int32_t *)malloc(4);
    c.head6 = c.use6 ? (int32_t *)malloc(sizeof(int32_t) << H6BITS) : (int32_t *)malloc(4);
    c.prev6 = c.use6 ? (int32_t *)malloc(sizeof(int32_t) * (size_t)(n < (1 << R6BITS) ? (n > 0 ? n : 1) : (1 << R6BITS))) : (int32_t *)malloc(4);
    S.buf = buf; S.tok = tok; S.learn = lazy >= 2;
    if (S.learn) {
        if (!prc_ready) prc_init();
        S.m = (rcm_t *)malloc(sizeof(rcm_t));
        S.nd = (onode_t *)malloc(sizeof(onode_t) * (OPTW + MAX_MATCH + 8));
        S.stk = (uint32_t *)malloc(sizeof(uint32_t) * 2 * (OPTW + MAX_MATCH + 8));
        S.e.low = 0; S.e.range = 0xFFFFFFFFu; S.e.cache = 0; S.e.csize = 1;
        S.e.cap = sizeof S.scr; S.e.out = S.scr;
        if (S.m) { rc_init(S.m); S.m->lpm = (1u << (lp < 0 || lp > RC_LPMAX ? 0 : lp)) - 1; }
        lzs_reset_block(&S);
    }
#define LZ_FREE() do { free(c.head); free(c.prev); free(c.head6); free(c.prev6); free(c.son); free(c.lhead); free(S.m); free(S.nd); free(S.stk); } while (0)
    if (!c.head6 || !c.prev6 || !c.son || !c.head || !c.prev || !c.lhead || (S.learn && (!S.m || !S.nd || !S.stk))) { LZ_FREE(); return -1; }
    memset(c.head, 0xFF, sizeof(int32_t) * ((size_t)1 << HBITS) + sizeof(int32_t) * (((size_t)1 << H3BITS) + 65536));
    memset(c.lhead, 0xFF, sizeof(int32_t) << LBITS);
    if (c.use6) memset(c.head6, 0xFF, sizeof(int32_t) << H6BITS);

    int64_t nr = 0, i = start;
    int miss = 0, have_cache = 0, cl = 0, cd = 0;
    int64_t cpos = 0;
    insert_to(&c, start);

    while (i < n) {
        S.pos = i;
        uint8_t k = rnd[i >> RND_BITS];
        if (k == 2) {                        /* literal-only: straight to the entropy coder */
            int64_t j = (i >> RND_BITS) + 1;
            while (j < nch && rnd[j] == 2) j++;
            int64_t e = j << RND_BITS;
            if (e > n) e = n;
            int64_t a = i;
            int found = 0, l = 0, d = 0;
            for (;;) {                       /* probe only at anchors */
                while (a < e && a < c.last && !anchor3(buf + a)) a++;
                if (a >= e || a >= c.last) { a = -1; break; }
                insert_to(&c, a);
                find(&c, a, chain_max, &l, &d);
                if (l >= MIN_RND_MATCH) { found = 1; break; }
                a++;
            }
            int64_t end = found ? a : e;
            while (i < end) { emit(&S, buf[i]); i++; }
            if (found) {
                emit(&S, ((uint64_t)l << DBITS) | (uint64_t)(uint32_t)d);
                i = a + l;
                while (i < n) {              /* a repeated blob goes on: keep following it */
                    int l2 = run_len(buf, i, n, d);
                    if (l2 < 8) break;
                    emit(&S, ((uint64_t)l2 << DBITS) | (uint64_t)(uint32_t)d);
                    i += l2;
                }
            }
            have_cache = 0; miss = 0;
            continue;
        }
        if (k == 1) {                        /* random-looking: raw, probe anchors */
            int64_t j = (i >> RND_BITS) + 1;
            while (j < nch && rnd[j] == 1) j++;
            int64_t e = j << RND_BITS;
            if (e > n) e = n;
            int64_t a = i;
            int found = 0, l = 0, d = 0;
            for (;;) {
                if (a >= e) { a = -1; break; }
                while (a < e && a < c.last && !anchorR(buf + a)) a++;
                if (a >= e || a >= c.last) { a = -1; break; }
                insert_to(&c, a);
                find(&c, a, chain_max, &l, &d);
                if (l >= MIN_RND_MATCH) { found = 1; break; }
                a++;
            }
            int64_t end = found ? a : e;
            if (end > i) {
                if (nr >= runcap) { LZ_FREE(); return -1; }
                runs[2 * nr] = S.nt;
                runs[2 * nr + 1] = end - i;
                nr++;
                S.pstart = S.nt;
            }
            S.pos = end;
            if (found) {
                emit(&S, ((uint64_t)l << DBITS) | (uint64_t)(uint32_t)d);
                end += l;
                while (end < n) {            /* a repeated blob goes on: keep following it */
                    int l2 = run_len(buf, end, n, d);
                    if (l2 < 8) break;
                    emit(&S, ((uint64_t)l2 << DBITS) | (uint64_t)(uint32_t)d);
                    end += l2;
                }
            }
            i = end;
            have_cache = 0; miss = 0;
            continue;
        }
        int dry = (miss >= 32 && accel < 99);
        if (S.learn && k == 0 && !dry) {
            int64_t wend = ((i >> RND_BITS) + 1) << RND_BITS;
            if (wend > n) wend = n;
            int64_t t0 = S.nt;
            i = opt_window(&S, &c, i, chain_max, nice, wend);
            for (int64_t q = t0; q < S.nt; q++) { if (tok[q] < 256) miss++; else miss = 0; }
            have_cache = 0;
            continue;
        }
        insert_to(&c, i);
        int l, d;
        if (have_cache && cpos == i) {
            l = cl; d = cd;
        } else {
            find(&c, i, (dry || k == 3) ? DRY_CHAIN : chain_max, &l, &d);
        }
        have_cache = 0;
        if (l) {
            miss = 0;
            if (lazy && l < LAZY_SKIP && i + 1 < n) {
                insert_to(&c, i + 1);
                int l2, d2;
                find(&c, i + 1, chain_max, &l2, &d2);
                if (l2 > l) {                /* a better match starts one byte later */
                    emit(&S, buf[i]);
                    have_cache = 1; cpos = i + 1; cl = l2; cd = d2;
                    i++;
                    continue;
                }
            }
            emit(&S, ((uint64_t)l << DBITS) | (uint64_t)(uint32_t)d);
            if (l >= SKIP_INSERT) {
                insert_to(&c, i + 4);
                c.ins = i + l;
            }
            i += l;
        } else {
            miss++;
            int step = 1 + (accel >= 31 ? 0 : (miss >> accel));
            if (step == 1) {
                emit(&S, buf[i]); i++;
            } else {
                if (step > MAX_STEP) step = MAX_STEP;
                int64_t e = i + step;
                if (e > n) e = n;
                while (i < e) { emit(&S, buf[i]); i++; }
            }
        }
    }
    int64_t nt = S.nt;
    LZ_FREE();
    *nruns_out = nr;
    return nt;
}

/* ------------------------------------------------------------ WAV filters */
/* 16-bit PCM. in = n frames of C interleaved little-endian samples; out = for
 * each channel a low-byte plane then a high-byte plane (n bytes each).
 * All arithmetic is mod 2^16, which is what the Python version amounts to. */
int kz_wav16_enc(const uint8_t *in, int64_t n, int C, int order, int ms, int bias,
                 uint8_t *out) {
    uint16_t *L = NULL, *t = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
    if (!t) return -1;
    if (ms) {
        L = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
        for (int64_t f = 0; f < n; f++)
            L[f] = (uint16_t)(in[2 * (f * C)] | (in[2 * (f * C) + 1] << 8));
    }
    for (int c = 0; c < C; c++) {
        for (int64_t f = 0; f < n; f++) {
            const uint8_t *s = in + 2 * (f * C + c);
            t[f] = (uint16_t)(s[0] | (s[1] << 8));
        }
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (uint16_t)(t[f] - L[f]);
        for (int o = 0; o < order; o++)
            for (int64_t f = n - 1; f > 0; f--) t[f] = (uint16_t)(t[f] - t[f - 1]);
        uint8_t *lo = out + (int64_t)(2 * c) * n, *hi = lo + n;
        for (int64_t f = 0; f < n; f++) {
            uint16_t v = (uint16_t)(t[f] + bias);
            lo[f] = (uint8_t)v;
            hi[f] = (uint8_t)(v >> 8);
        }
    }
    free(t);
    free(L);
    return 0;
}

int kz_wav16_dec(const uint8_t *in, int64_t n, int C, int order, int ms, int bias,
                 uint8_t *out) {
    uint16_t *L = NULL, *t = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
    if (!t) return -1;
    if (ms) {
        L = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
    }
    for (int c = 0; c < C; c++) {
        const uint8_t *lo = in + (int64_t)(2 * c) * n, *hi = lo + n;
        for (int64_t f = 0; f < n; f++)
            t[f] = (uint16_t)((lo[f] | (hi[f] << 8)) - bias);
        for (int o = 0; o < order; o++)
            for (int64_t f = 1; f < n; f++) t[f] = (uint16_t)(t[f] + t[f - 1]);
        if (ms && c == 0) memcpy(L, t, sizeof(uint16_t) * (size_t)n);
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (uint16_t)(t[f] + L[f]);
        for (int64_t f = 0; f < n; f++) {
            uint8_t *d = out + 2 * (f * C + c);
            d[0] = (uint8_t)t[f];
            d[1] = (uint8_t)(t[f] >> 8);
        }
    }
    free(t);
    free(L);
    return 0;
}

/* 24/32-bit audio: byte planes only. plane index = channel * W + byte (LSB first) */
void kz_shuf_enc(const uint8_t *in, int64_t n, int W, int C, uint8_t *out) {
    for (int64_t f = 0; f < n; f++)
        for (int c = 0; c < C; c++) {
            const uint8_t *s = in + (f * C + c) * W;
            for (int k = 0; k < W; k++) out[(int64_t)(c * W + k) * n + f] = s[k];
        }
}

void kz_shuf_dec(const uint8_t *in, int64_t n, int W, int C, uint8_t *out) {
    for (int64_t f = 0; f < n; f++)
        for (int c = 0; c < C; c++) {
            uint8_t *d = out + (f * C + c) * W;
            for (int k = 0; k < W; k++) d[k] = in[(int64_t)(c * W + k) * n + f];
        }
}

/* Integer audio of W = 1, 3 or 4 bytes per sample (little-endian, interleaved).
 * out = for each channel W byte planes (LSB first) of n bytes each.
 * Per channel: (channel 1 minus channel 0 if ms), delta x order, zigzag; all mod 2^(8W). */
static uint32_t wmask(int W) { return W >= 4 ? 0xFFFFFFFFu : ((1u << (8 * W)) - 1u); }

int kz_int_enc(const uint8_t *in, int64_t n, int W, int C, int order, int ms, int zz,
               uint8_t *out) {
    uint32_t mask = wmask(W), sb = (uint32_t)(8 * W - 1);
    uint32_t *t = (uint32_t *)malloc(4 * (size_t)(n ? n : 1)), *L = NULL;
    if (!t) return -1;
    if (ms) {
        L = (uint32_t *)malloc(4 * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
    }
    for (int c = 0; c < C; c++) {
        for (int64_t f = 0; f < n; f++) {
            const uint8_t *s = in + (f * C + c) * W;
            uint32_t v = 0;
            for (int k = 0; k < W; k++) v |= (uint32_t)s[k] << (8 * k);
            t[f] = v;
        }
        if (ms && c == 0) memcpy(L, t, 4 * (size_t)n);
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (t[f] - L[f]) & mask;
        for (int o = 0; o < order; o++)
            for (int64_t f = n - 1; f > 0; f--) t[f] = (t[f] - t[f - 1]) & mask;
        if (zz)
            for (int64_t f = 0; f < n; f++) {
                uint32_t v = t[f];
                t[f] = ((v << 1) & mask) ^ (((v >> sb) & 1u) ? mask : 0u);
            }
        for (int k = 0; k < W; k++) {
            uint8_t *pl = out + (int64_t)(c * W + k) * n;
            for (int64_t f = 0; f < n; f++) pl[f] = (uint8_t)(t[f] >> (8 * k));
        }
    }
    free(t);
    free(L);
    return 0;
}

int kz_int_dec(const uint8_t *in, int64_t n, int W, int C, int order, int ms, int zz,
               uint8_t *out) {
    uint32_t mask = wmask(W);
    uint32_t *t = (uint32_t *)malloc(4 * (size_t)(n ? n : 1)), *L = NULL;
    if (!t) return -1;
    if (ms) {
        L = (uint32_t *)malloc(4 * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
    }
    for (int c = 0; c < C; c++) {
        for (int64_t f = 0; f < n; f++) {
            uint32_t v = 0;
            for (int k = 0; k < W; k++)
                v |= (uint32_t)in[(int64_t)(c * W + k) * n + f] << (8 * k);
            t[f] = v;
        }
        if (zz)
            for (int64_t f = 0; f < n; f++) {
                uint32_t z = t[f];
                t[f] = ((z >> 1) ^ ((z & 1u) ? mask : 0u)) & mask;
            }
        for (int o = 0; o < order; o++)
            for (int64_t f = 1; f < n; f++) t[f] = (t[f] + t[f - 1]) & mask;
        if (ms && c == 0) memcpy(L, t, 4 * (size_t)n);
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (t[f] + L[f]) & mask;
        for (int64_t f = 0; f < n; f++) {
            uint8_t *d = out + (f * C + c) * W;
            for (int k = 0; k < W; k++) d[k] = (uint8_t)(t[f] >> (8 * k));
        }
    }
    free(t);
    free(L);
    return 0;
}

/* float32 audio that is really integers: sample = k / 2^s, k in [-2^s, 2^s - 1].
 * kz_f2i returns -1 if any sample doesn't convert exactly (NaN, -0.0, fraction, range). */
int kz_f2i(const uint8_t *in, int64_t m, int s, int kw, uint8_t *out) {
    double S = (double)(1 << s);
    for (int64_t i = 0; i < m; i++) {
        const uint8_t *p = in + 4 * i;
        uint32_t u = (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16)
                     | ((uint32_t)p[3] << 24);
        float x;
        if (u == 0x80000000u) return -1;
        memcpy(&x, &u, 4);
        double v = (double)x * S;
        if (!(v >= -S && v <= S - 1.0)) return -1;
        int32_t k = (int32_t)v;
        if ((double)k != v) return -1;
        uint32_t q = (uint32_t)k;
        for (int j = 0; j < kw; j++) out[i * kw + j] = (uint8_t)(q >> (8 * j));
    }
    return 0;
}

void kz_i2f(const uint8_t *in, int64_t m, int s, int kw, uint8_t *out) {
    double S = (double)(1 << s);
    int sh = 32 - 8 * kw;
    for (int64_t i = 0; i < m; i++) {
        uint32_t v = 0;
        for (int j = 0; j < kw; j++) v |= (uint32_t)in[i * kw + j] << (8 * j);
        int32_t k = (int32_t)(v << sh) >> sh;
        float x = (float)((double)k / S);
        uint32_t u;
        memcpy(&u, &x, 4);
        for (int j = 0; j < 4; j++) out[i * 4 + j] = (uint8_t)(u >> (8 * j));
    }
}

/* ------------------------------------------------------------ chunk classes */
#define RND_ENTROPY 7.95
#define LIT_ENTROPY 0.6
#define EASY_ENTROPY 3.0

static double log2_(double x) {
    /* tiny log2 without libm: exponent + polynomial on the mantissa */
    union { double d; uint64_t u; } v;
    v.d = x;
    int e = (int)((v.u >> 52) & 0x7FF) - 1023;
    v.u = (v.u & 0x000FFFFFFFFFFFFFULL) | 0x3FF0000000000000ULL;
    double m = v.d, t = (m - 1.0) / (m + 1.0), t2 = t * t;
    double ln = 2.0 * t * (1.0 + t2 * (1.0 / 3 + t2 * (1.0 / 5 + t2 * (1.0 / 7 + t2 / 9))));
    return e + ln * 1.4426950408889634;
}

/* count distinct g-byte groups in s[0..len) with a small open-addressing set;
 * returns the number of distinct groups */
static int distinct_grams_m(const uint8_t *s, int len, int g, uint32_t *tab, uint32_t *used, uint32_t M) {
    int ng = len - g + 1, uniq = 0;
    memset(used, 0, M / 8);
    for (int i = 0; i < ng; i++) {
        uint64_t h = 1469598103934665603ULL;
        for (int k = 0; k < g; k++) h = (h ^ s[i + k]) * 1099511628211ULL;
        uint32_t slot = (uint32_t)(h >> 40) & (M - 1);
        for (;;) {
            if (!(used[slot >> 5] & (1u << (slot & 31)))) {
                used[slot >> 5] |= 1u << (slot & 31);
                tab[slot] = (uint32_t)i;
                uniq++;
                break;
            }
            if (memcmp(s + tab[slot], s + i, (size_t)g) == 0) break;
            slot = (slot + 1) & (M - 1);
        }
    }
    return uniq;
}

static int distinct_grams(const uint8_t *s, int len, int g, uint32_t *tab, uint32_t *used) {
    return distinct_grams_m(s, len, g, tab, used, 16384);
}

/* entropy of the byte given a context out of 8 (cnt[ctx*256 + byte]) over n bytes */
static double cond_ent(const int *cnt, int n) {
    double acc = 0.0;
    for (int c = 0; c < 8; c++) {
        int tot = 0;
        for (int v = 0; v < 256; v++) tot += cnt[c * 256 + v];
        if (!tot) continue;
        double h = 0.0, it = 1.0 / tot;
        for (int v = 0; v < 256; v++)
            if (cnt[c * 256 + v]) { double p = cnt[c * 256 + v] * it; h -= p * log2_(p); }
        acc += (double)tot / n * h;
    }
    return acc;
}

/* class per 64 KB chunk: 0 normal, 1 random, 2 literal-only, 3 sparse */
void kz_rnd(const uint8_t *buf, int64_t n, uint8_t *rnd) {
    int64_t nchunks = (n + (1 << RND_BITS) - 1) >> RND_BITS;
    uint8_t s[8192];
    uint32_t *tab = (uint32_t *)malloc(sizeof(uint32_t) * 16384);
    uint32_t *used = (uint32_t *)malloc(16384 / 8);
    uint32_t *tabB = (uint32_t *)malloc(sizeof(uint32_t) * 262144);
    uint32_t *usedB = (uint32_t *)malloc(262144 / 8);
    for (int64_t ch = 0; ch < nchunks; ch++) {
        rnd[ch] = 0;
        int64_t a = ch << RND_BITS;
        int len = 0;
        for (int k = 0; k < 8; k++) {
            int64_t from = a + k * 8192;
            if (from >= n) break;
            int64_t cnt = n - from < 1024 ? n - from : 1024;
            memcpy(s + len, buf + from, (size_t)cnt);
            len += (int)cnt;
        }
        if (len < 4096 || !tab || !used) continue;
        int freq[256] = {0};
        for (int i = 0; i < len; i++) freq[s[i]]++;
        double ent = 0.0, inv = 1.0 / len;
        for (int v = 0; v < 256; v++)
            if (freq[v]) { double p = freq[v] * inv; ent -= p * log2_(p); }
        if (ent >= RND_ENTROPY) {
            int u = distinct_grams(s, len, 4, tab, used);
            rnd[ch] = (u >= 0.99 * (len - 3)) ? 1 : 0;
        } else if (ent >= LIT_ENTROPY) {
            if (ent >= 3.0 && tabB && usedB) {
                /* whole-chunk test. Noise passes only if (1) knowing the byte's position
                 * in an 8-byte period or the previous byte's top bits tells (almost)
                 * nothing, and (2) it holds no more repeated groups than chance gives. */
                int clen = (int)(n - a < (1 << RND_BITS) ? n - a : (1 << RND_BITS));
                int64_t fq[256] = {0};
                static int ph[8 * 256], pv[8 * 256];
                memset(ph, 0, sizeof ph);
                memset(pv, 0, sizeof pv);
                for (int i = 0; i < clen; i++) {
                    int x = buf[a + i];
                    fq[x]++;
                    ph[(i & 7) * 256 + x]++;
                    pv[(i ? buf[a + i - 1] >> 5 : 0) * 256 + x]++;
                }
                double p2 = 0.0, ic = 1.0 / clen, h0 = 0.0;
                for (int v = 0; v < 256; v++) {
                    double p = fq[v] * ic;
                    p2 += p * p;
                    if (p > 0) h0 -= p * log2_(p);
                }
                double gap1 = h0 - cond_ent(ph, clen), gap2 = h0 - cond_ent(pv, clen);
                if (gap1 < 0.06 && gap2 < 0.06) {
                    int ng_ = 4;
                    double expd = 0.0;
                    for (int g = 4; g <= 16; g++) {
                        double q = 1.0;
                        for (int k = 0; k < g; k++) q *= p2;
                        expd = 0.5 * (double)(clen - g + 1) * (double)(clen - g + 1) * q;
                        ng_ = g;
                        if (expd <= 0.1 * (clen - g + 1)) break;
                    }
                    int ng = clen - ng_ + 1;
                    if (ng > 0 && expd <= 0.1 * ng) {
                        int u = distinct_grams_m(buf + a, clen, ng_, tabB, usedB, 262144);
                        double dup = (double)(ng - u);
                        if (dup <= 1.5 * expd + 0.004 * ng) rnd[ch] = 2;
                    }
                }
            } else {
                int g = (int)(32 / ent) + 1;
                if (g > 64) g = 64;
                int u = distinct_grams(s, len, g, tab, used);
                if (u >= 0.99 * (len - g + 1)) rnd[ch] = 2;
                else if (ent < EASY_ENTROPY) rnd[ch] = 3;
            }
        }
    }
    free(tab);
    free(used);
    free(tabB);
    free(usedB);
}
"""

_MP3_SRC = r"""/* kiln_mp3.c - lossless MP3 recompressor (MPEG-1 Layer III): parses the frames down to the
 * Huffman-coded coefficients, models them with a context-mixing range coder and rebuilds the
 * exact original bytes on decode. Compiled together with kiln_accel.c.
 */
#include <stdio.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
/* MP3 Huffman tables (ISO 11172-3 numbering). Extracted from libshine; Kraft-checked. */
static const uint16_t HC_1[4]={1,1,1,0};
static const uint8_t HL_1[4]={1,3,2,3};
static const uint16_t HC_2[9]={1,2,1,3,1,1,3,2,0};
static const uint8_t HL_2[9]={1,3,6,3,3,5,5,5,6};
static const uint16_t HC_3[9]={3,2,1,1,1,1,3,2,0};
static const uint8_t HL_3[9]={2,2,6,3,2,5,5,5,6};
static const uint16_t HC_5[16]={1,2,6,5,3,1,4,4,7,5,7,1,6,1,1,0};
static const uint8_t HL_5[16]={1,3,6,7,3,3,6,7,6,6,7,8,7,6,7,8};
static const uint16_t HC_6[16]={7,3,5,1,6,2,3,2,5,4,4,1,3,3,2,0};
static const uint8_t HL_6[16]={3,3,5,7,3,2,4,5,4,4,5,6,6,5,6,7};
static const uint16_t HC_7[36]={1,2,10,19,16,10,3,3,7,10,5,3,11,4,13,17,8,4,12,11,18,15,11,2,7,6,9,14,3,1,6,4,5,3,2,0};
static const uint8_t HL_7[36]={1,3,6,8,8,9,3,4,6,7,7,8,6,5,7,8,8,9,7,7,8,9,9,9,7,7,8,9,9,10,8,8,9,10,10,10};
static const uint16_t HC_8[36]={3,4,6,18,12,5,5,1,2,16,9,3,7,3,5,14,7,3,19,17,15,13,10,4,13,5,8,11,5,1,12,4,4,1,1,0};
static const uint8_t HL_8[36]={2,3,6,8,8,9,3,2,4,8,8,8,6,4,6,8,8,9,8,8,8,9,9,10,8,7,8,9,10,10,9,8,9,9,11,11};
static const uint16_t HC_9[36]={7,5,9,14,15,7,6,4,5,5,6,7,7,6,8,8,8,5,15,6,9,10,5,1,11,7,9,6,4,1,14,4,6,2,6,0};
static const uint8_t HL_9[36]={3,3,5,6,8,9,3,3,4,5,6,8,4,4,5,6,7,8,6,5,6,7,7,8,7,6,7,7,8,9,8,7,8,8,9,9};
static const uint16_t HC_10[64]={1,2,10,23,35,30,12,17,3,3,8,12,18,21,12,7,11,9,15,21,32,40,19,6,14,13,22,34,46,23,18,7,20,19,33,47,27,22,9,3,31,22,41,26,21,20,5,3,14,13,10,11,16,6,5,1,9,8,7,8,4,4,2,0};
static const uint8_t HL_10[64]={1,3,6,8,9,9,9,10,3,4,6,7,8,9,8,8,6,6,7,8,9,10,9,9,7,7,8,9,10,10,9,10,8,8,9,10,10,10,10,10,9,9,10,10,11,11,10,11,8,8,9,10,10,10,11,11,9,8,9,10,10,11,11,11};
static const uint16_t HC_11[64]={3,4,10,24,34,33,21,15,5,3,4,10,32,17,11,10,11,7,13,18,30,31,20,5,25,11,19,59,27,18,12,5,35,33,31,58,30,16,7,5,28,26,32,19,17,15,8,14,14,12,9,13,14,9,4,1,11,4,6,6,6,3,2,0};
static const uint8_t HL_11[64]={2,3,5,7,8,9,8,9,3,3,4,6,8,8,7,8,5,5,6,7,8,9,8,8,7,6,7,9,8,10,8,9,8,8,8,9,9,10,9,10,8,8,9,10,10,11,10,11,8,7,7,8,9,10,10,10,8,7,8,9,10,10,10,10};
static const uint16_t HC_12[64]={9,6,16,33,41,39,38,26,7,5,6,9,23,16,26,11,17,7,11,14,21,30,10,7,17,10,15,12,18,28,14,5,32,13,22,19,18,16,9,5,40,17,31,29,17,13,4,2,27,12,11,15,10,7,4,1,27,12,8,12,6,3,1,0};
static const uint8_t HL_12[64]={4,3,5,7,8,9,9,9,3,3,4,5,7,7,8,8,5,4,5,6,7,8,7,8,6,5,6,6,7,8,8,8,7,6,7,7,8,8,8,9,8,7,8,8,8,9,8,9,8,7,7,8,8,9,9,10,9,8,8,9,9,9,9,10};
static const uint16_t HC_13[256]={1,5,14,21,34,51,46,71,42,52,68,52,67,44,43,19,3,4,12,19,31,26,44,33,31,24,32,24,31,35,22,14,15,13,23,36,59,49,77,65,29,40,30,40,27,33,42,16,22,20,37,61,56,79,73,64,43,76,56,37,26,31,25,14,35,16,60,57,97,75,114,91,54,73,55,41,48,53,23,24,58,27,50,96,76,70,93,84,77,58,79,29,74,49,41,17,47,45,78,74,115,94,90,79,69,83,71,50,59,38,36,15,72,34,56,95,92,85,91,90,86,73,77,65,51,44,43,42,43,20,30,44,55,78,72,87,78,61,46,54,37,30,20,16,53,25,41,37,44,59,54,81,66,76,57,54,37,18,39,11,35,33,31,57,42,82,72,80,47,58,55,21,22,26,38,22,53,25,23,38,70,60,51,36,55,26,34,23,27,14,9,7,34,32,28,39,49,75,30,52,48,40,52,28,18,17,9,5,45,21,34,64,56,50,49,45,31,19,12,15,10,7,6,3,48,23,20,39,36,35,53,21,16,23,13,10,6,1,4,2,16,15,17,27,25,20,29,11,17,12,16,8,1,1,0,1};
static const uint8_t HL_13[256]={1,4,6,7,8,9,9,10,9,10,11,11,12,12,13,13,3,4,6,7,8,8,9,9,9,9,10,10,11,12,12,12,6,6,7,8,9,9,10,10,9,10,10,11,11,12,13,13,7,7,8,9,9,10,10,10,10,11,11,11,11,12,13,13,8,7,9,9,10,10,11,11,10,11,11,12,12,13,13,14,9,8,9,10,10,10,11,11,11,11,12,11,13,13,14,14,9,9,10,10,11,11,11,11,11,12,12,12,13,13,14,14,10,9,10,11,11,11,12,12,12,12,13,13,13,14,16,16,9,8,9,10,10,11,11,12,12,12,12,13,13,14,15,15,10,9,10,10,11,11,11,13,12,13,13,14,14,14,16,15,10,10,10,11,11,12,12,13,12,13,14,13,14,15,16,17,11,10,10,11,12,12,12,12,13,13,13,14,15,15,15,16,11,11,11,12,12,13,12,13,14,14,15,15,15,16,16,16,12,11,12,13,13,13,14,14,14,14,14,15,16,15,16,16,13,12,12,13,13,13,15,14,14,17,15,15,15,17,16,16,12,12,13,14,14,14,15,14,15,15,16,16,19,18,19,16};
static const uint16_t HC_15[256]={7,12,18,53,47,76,124,108,89,123,108,119,107,81,122,63,13,5,16,27,46,36,61,51,42,70,52,83,65,41,59,36,19,17,15,24,41,34,59,48,40,64,50,78,62,80,56,33,29,28,25,43,39,63,55,93,76,59,93,72,54,75,50,29,52,22,42,40,67,57,95,79,72,57,89,69,49,66,46,27,77,37,35,66,58,52,91,74,62,48,79,63,90,62,40,38,125,32,60,56,50,92,78,65,55,87,71,51,73,51,70,30,109,53,49,94,88,75,66,122,91,73,56,42,64,44,21,25,90,43,41,77,73,63,56,92,77,66,47,67,48,53,36,20,71,34,67,60,58,49,88,76,67,106,71,54,38,39,23,15,109,53,51,47,90,82,58,57,48,72,57,41,23,27,62,9,86,42,40,37,70,64,52,43,70,55,42,25,29,18,11,11,118,68,30,55,50,46,74,65,49,39,24,16,22,13,14,7,91,44,39,38,34,63,52,45,31,52,28,19,14,8,9,3,123,60,58,53,47,43,32,22,37,24,17,12,15,10,2,1,71,37,34,30,28,20,17,26,21,16,10,6,8,6,2,0};
static const uint8_t HL_15[256]={3,4,5,7,7,8,9,9,9,10,10,11,11,11,12,13,4,3,5,6,7,7,8,8,8,9,9,10,10,10,11,11,5,5,5,6,7,7,8,8,8,9,9,10,10,11,11,11,6,6,6,7,7,8,8,9,9,9,10,10,10,11,11,11,7,6,7,7,8,8,9,9,9,9,10,10,10,11,11,11,8,7,7,8,8,8,9,9,9,9,10,10,11,11,11,12,9,7,8,8,8,9,9,9,9,10,10,10,11,11,12,12,9,8,8,9,9,9,9,10,10,10,10,10,11,11,11,12,9,8,8,9,9,9,9,10,10,10,10,11,11,12,12,12,9,8,9,9,9,9,10,10,10,11,11,11,11,12,12,12,10,9,9,9,10,10,10,10,10,11,11,11,11,12,13,12,10,9,9,9,10,10,10,10,11,11,11,11,12,12,12,13,11,10,9,10,10,10,11,11,11,11,11,11,12,12,13,13,11,10,10,10,10,11,11,11,11,12,12,12,12,12,13,13,12,11,11,11,11,11,11,11,12,12,12,12,13,13,12,13,12,11,11,11,11,11,11,12,12,12,12,12,13,13,13,13};
static const uint16_t HC_16[256]={1,5,14,44,74,63,110,93,172,149,138,242,225,195,376,17,3,4,12,20,35,62,53,47,83,75,68,119,201,107,207,9,15,13,23,38,67,58,103,90,161,72,127,117,110,209,206,16,45,21,39,69,64,114,99,87,158,140,252,212,199,387,365,26,75,36,68,65,115,101,179,164,155,264,246,226,395,382,362,9,66,30,59,56,102,185,173,265,142,253,232,400,388,378,445,16,111,54,52,100,184,178,160,133,257,244,228,217,385,366,715,10,98,48,91,88,165,157,148,261,248,407,397,372,380,889,884,8,85,84,81,159,156,143,260,249,427,401,392,383,727,713,708,7,154,76,73,141,131,256,245,426,406,394,384,735,359,710,352,11,139,129,67,125,247,233,229,219,393,743,737,720,885,882,439,4,243,120,118,115,227,223,396,746,742,736,721,712,706,223,436,6,202,224,222,218,216,389,386,381,364,888,443,707,440,437,1728,4,747,211,210,208,370,379,734,723,714,1735,883,877,876,3459,865,2,377,369,102,187,726,722,358,711,709,866,1734,871,3458,870,434,0,12,10,7,11,10,17,11,9,13,12,10,7,5,3,1,3};
static const uint8_t HL_16[256]={1,4,6,8,9,9,10,10,11,11,11,12,12,12,13,9,3,4,6,7,8,9,9,9,10,10,10,11,12,11,12,8,6,6,7,8,9,9,10,10,11,10,11,11,11,12,12,9,8,7,8,9,9,10,10,10,11,11,12,12,12,13,13,10,9,8,9,9,10,10,11,11,11,12,12,12,13,13,13,9,9,8,9,9,10,11,11,12,11,12,12,13,13,13,14,10,10,9,9,10,11,11,11,11,12,12,12,12,13,13,14,10,10,9,10,10,11,11,11,12,12,13,13,13,13,15,15,10,10,10,10,11,11,11,12,12,13,13,13,13,14,14,14,10,11,10,10,11,11,12,12,13,13,13,13,14,13,14,13,11,11,11,10,11,12,12,12,12,13,14,14,14,15,15,14,10,12,11,11,11,12,12,13,14,14,14,14,14,14,13,14,11,12,12,12,12,12,13,13,13,13,15,14,14,14,14,16,11,14,12,12,12,13,13,14,14,14,16,15,15,15,17,15,11,13,13,11,12,14,14,13,14,14,15,16,15,17,15,14,11,9,8,8,9,9,10,10,10,11,11,11,11,11,11,11,8};
static const uint16_t HC_24[256]={15,13,46,80,146,262,248,434,426,669,653,649,621,517,1032,88,14,12,21,38,71,130,122,216,209,198,327,345,319,297,279,42,47,22,41,74,68,128,120,221,207,194,182,340,315,295,541,18,81,39,75,70,134,125,116,220,204,190,178,325,311,293,271,16,147,72,69,135,127,118,112,210,200,188,352,323,306,285,540,14,263,66,129,126,119,114,214,202,192,180,341,317,301,281,262,12,249,123,121,117,113,215,206,195,185,347,330,308,291,272,520,10,435,115,111,109,211,203,196,187,353,332,313,298,283,531,381,17,427,212,208,205,201,193,186,177,169,320,303,286,268,514,377,16,335,199,197,191,189,181,174,333,321,305,289,275,521,379,371,11,668,184,183,179,175,344,331,314,304,290,277,530,383,373,366,10,652,346,171,168,164,318,309,299,287,276,263,513,375,368,362,6,648,322,316,312,307,302,292,284,269,261,512,376,370,364,359,4,620,300,296,294,288,282,273,266,515,380,374,369,365,361,357,2,1033,280,278,274,267,264,259,382,378,372,367,363,360,358,356,0,43,20,19,17,15,13,11,9,7,6,4,7,5,3,1,3};
static const uint8_t HL_24[256]={4,4,6,7,8,9,9,10,10,11,11,11,11,11,12,9,4,4,5,6,7,8,8,9,9,9,10,10,10,10,10,8,6,5,6,7,7,8,8,9,9,9,9,10,10,10,11,7,7,6,7,7,8,8,8,9,9,9,9,10,10,10,10,7,8,7,7,8,8,8,8,9,9,9,10,10,10,10,11,7,9,7,8,8,8,8,9,9,9,9,10,10,10,10,10,7,9,8,8,8,8,9,9,9,9,10,10,10,10,10,11,7,10,8,8,8,9,9,9,9,10,10,10,10,10,11,11,8,10,9,9,9,9,9,9,9,9,10,10,10,10,11,11,8,10,9,9,9,9,9,9,10,10,10,10,10,11,11,11,8,11,9,9,9,9,10,10,10,10,10,10,11,11,11,11,8,11,10,9,9,9,10,10,10,10,10,10,11,11,11,11,8,11,10,10,10,10,10,10,10,10,10,11,11,11,11,11,8,11,10,10,10,10,10,10,10,11,11,11,11,11,11,11,8,12,10,10,10,10,10,10,11,11,11,11,11,11,11,11,8,8,7,7,7,7,7,7,7,7,7,7,8,8,8,8,4};
static const uint16_t HC_32[16]={1,5,4,5,6,5,4,4,7,3,6,0,7,2,3,1};
static const uint8_t HL_32[16]={1,4,4,5,4,6,5,6,4,5,5,6,5,6,6,6};
static const uint16_t HC_33[16]={15,14,13,12,11,10,9,8,7,6,5,4,3,2,1,0};
static const uint8_t HL_33[16]={4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,4};
typedef struct{int xlen,linbits;const uint16_t*code;const uint8_t*len;}HuffTab;
static const HuffTab HT[34]={
{0,0,0,0},
{2,0,HC_1,HL_1},
{3,0,HC_2,HL_2},
{3,0,HC_3,HL_3},
{0,0,0,0},
{4,0,HC_5,HL_5},
{4,0,HC_6,HL_6},
{6,0,HC_7,HL_7},
{6,0,HC_8,HL_8},
{6,0,HC_9,HL_9},
{8,0,HC_10,HL_10},
{8,0,HC_11,HL_11},
{8,0,HC_12,HL_12},
{16,0,HC_13,HL_13},
{0,0,0,0},
{16,0,HC_15,HL_15},
{16,1,HC_16,HL_16},
{16,2,HC_16,HL_16},
{16,3,HC_16,HL_16},
{16,4,HC_16,HL_16},
{16,6,HC_16,HL_16},
{16,8,HC_16,HL_16},
{16,10,HC_16,HL_16},
{16,13,HC_16,HL_16},
{16,4,HC_24,HL_24},
{16,5,HC_24,HL_24},
{16,6,HC_24,HL_24},
{16,7,HC_24,HL_24},
{16,8,HC_24,HL_24},
{16,9,HC_24,HL_24},
{16,11,HC_24,HL_24},
{16,13,HC_24,HL_24},
{1,0,HC_32,HL_32},
{1,0,HC_33,HL_33},
};

/* mp3core.c - MPEG-1 Layer III exact parser + re-serializer (checkpoint 2).
   parse: file bytes -> segments (raw bytes | runs of frames with decoded granules)
   build: segments -> identical file bytes.  Anything unparseable stays raw. */
#include <stdio.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

#define QORD 1   /* count1 quad index: 1: v*8+w*4+x*2+y   2: v+2w+4x+8y */

static const int BR[15]={0,32,40,48,56,64,80,96,112,128,160,192,224,256,320};
static const int SR[3]={44100,48000,32000};
static const int SFB[3][23]={
 {0,4,8,12,16,20,24,30,36,44,52,62,74,90,110,134,162,196,238,288,342,418,576},
 {0,4,8,12,16,20,24,30,36,42,50,60,72,88,106,128,156,190,230,276,330,384,576},
 {0,4,8,12,16,20,24,30,36,44,54,66,82,102,126,156,194,240,296,364,448,550,576}};
static const int SLEN[16][2]={{0,0},{0,1},{0,2},{0,3},{3,0},{1,1},{1,2},{1,3},{2,1},{2,2},{2,3},{3,1},{3,2},{3,3},{4,2},{4,3}};

/* ---------- Huffman decode tries ---------- */
typedef struct{int16_t c[2];}Node;   /* child>0: node idx; child<=0: leaf -(val) -1 ... encoded as -(v+1) */
static Node *TR[34]; static int TRN[34]; static int tinit=0;
static void trie_build(int t){
  const HuffTab*h=&HT[t]; if(!h->xlen) return;
  int n=h->xlen*(t>=32?16:h->xlen); if(t>=32) n=16;
  TR[t]=calloc(2*n+2,sizeof(Node)); int cnt=1;
  for(int v=0;v<n;v++){ int L=h->len[v]; if(!L) continue; uint32_t c=h->code[v]; int cur=0;
    for(int b=L-1;b>=0;b--){ int bit=(c>>b)&1;
      if(b==0){ TR[t][cur].c[bit]=-(v+1); }
      else { if(TR[t][cur].c[bit]==0) TR[t][cur].c[bit]=cnt++; cur=TR[t][cur].c[bit]; } } }
  TRN[t]=cnt;
}
static void tinit_all(void){ if(tinit) return; tinit=1; for(int t=0;t<34;t++) trie_build(t); }

/* ---------- bit io ---------- */
typedef struct{const uint8_t*b;uint64_t pos,end;}BR_;
static inline int rbit(BR_*r){ if(r->pos>=r->end) return -1; int v=(r->b[r->pos>>3]>>(7-(r->pos&7)))&1; r->pos++; return v; }
static inline int rbits(BR_*r,int n,uint32_t*o){ uint32_t v=0; for(int i=0;i<n;i++){int b=rbit(r); if(b<0) return 0; v=v<<1|b;} *o=v; return 1; }
typedef struct{uint8_t*b;uint64_t pos,cap;}BW;
static inline void wbits(BW*w,uint32_t v,int n){
  if(n<=0) return;
  if(w->pos+(uint64_t)n<=w->cap){ uint64_t pos=w->pos; int left=n;
    while(left>0){ int bo=pos&7, room=8-bo, take=left<room?left:room; uint32_t bits=(v>>(left-take))&((1u<<take)-1); w->b[pos>>3]|=(uint8_t)(bits<<(room-take)); pos+=take; left-=take; }
    w->pos=pos; return; }
  for(int i=n-1;i>=0;i--){ if(((v>>i)&1) && w->pos<w->cap) w->b[w->pos>>3]|=0x80>>(w->pos&7); w->pos++; } }

/* ---------- data model ---------- */
typedef struct{
  int p23,bv,gg,sfc,ws,bt,mixed,ts[3],sbg[3],r0,r1,pre,sfs,c1t;
  int sf[39]; int nq; int stuff_n; uint32_t stuff_off; int16_t xr[576]; int16_t L4[576]; int lvalid;
}Gran;
typedef struct{
  uint8_t hdr[4]; uint8_t crc[2]; int has_crc,mdb,priv,nch,pay; uint8_t scfsi[2][4]; Gran gr[2][2];
  uint32_t gap_n; uint32_t gap_off; /* raw gap bits preceding this frame's granules */
}Frame;
typedef struct{ Frame*f; int nf,cap; uint32_t tail_n,tail_off; uint64_t total_pay; }Run;
typedef struct{ int is_run; uint64_t off,len; Run*run; }Seg;
typedef struct{ Seg*s; int n,cap; uint8_t*pool; size_t pn,pcap; }Parsed;

static uint32_t pool_put_bits(Parsed*P,const uint8_t*src,uint64_t bitpos,uint32_t nbits){
  size_t nb=(nbits+7)/8; if(P->pn+nb>P->pcap){P->pcap=(P->pn+nb)*2+1024;P->pool=realloc(P->pool,P->pcap);}
  uint32_t off=P->pn; if(nb) memset(P->pool+off,0,nb);
  for(uint32_t i=0;i<nbits;i++){ uint64_t p=bitpos+i; if((src[p>>3]>>(7-(p&7)))&1) P->pool[off+(i>>3)]|=0x80>>(i&7);} 
  P->pn+=nb; return off;
}

/* granule geometry */
static void regions(const Gran*g,int sr,int*r1s,int*r2s){
  if(g->ws){ *r1s=36; *r2s=576; }
  else { int a=g->r0+1,b=g->r0+g->r1+2; *r1s=SFB[sr][a>22?22:a]; *r2s=SFB[sr][b>22?22:b]; }
}
/* scalefactor layout: returns count and sets bands info. idx<nsf; slen per idx */
static int sf_layout(const Gran*g,int*slen){
  int n=0; const int*s=SLEN[g->sfc];
  if(g->ws&&g->bt==2){
    if(g->mixed){ for(int i=0;i<17;i++) slen[n++]=s[0]; for(int i=0;i<18;i++) slen[n++]=s[1]; }
    else { for(int i=0;i<18;i++) slen[n++]=s[0]; for(int i=0;i<18;i++) slen[n++]=s[1]; }
  } else { for(int i=0;i<11;i++) slen[n++]=s[0]; for(int i=0;i<10;i++) slen[n++]=s[1]; }
  return n;
}
static int sf_shared(const Frame*F,int gri,int ch,const Gran*g,int idx){
  if(gri==0||(g->ws&&g->bt==2)) return 0;
  int grp= idx<6?0: idx<11?1: idx<16?2:3; return F->scfsi[ch][grp];
}

/* decode one granule from mainstream bits [start,start+p23) ; returns 1 ok */
static int dec_gran(const uint8_t*ms,uint64_t start,Gran*g,const Frame*F,int gri,int ch,int sr,Parsed*P){
  BR_ r={ms,start,start+g->p23};
  int slen[39]; int n=sf_layout(g,slen); int p2=0;
  /* part2 */
  const Gran*g0=&F->gr[0][ch];
  for(int i=0;i<n;i++){
    if(sf_shared(F,gri,ch,g,i)){ g->sf[i]=g0->sf[i]; continue; }
    uint32_t v; if(!rbits(&r,slen[i],&v)) return 0; g->sf[i]=v; p2+=slen[i]; }
  (void)p2;
  if(g->bv>288) return 0;
  int r1s,r2s; regions(g,sr,&r1s,&r2s);
  int nbig=g->bv*2; if(nbig>576) return 0;
  memset(g->xr,0,sizeof g->xr);
  int i=0;
  for(int reg=0;reg<3&&i<nbig;reg++){
    int e= reg==0? r1s : reg==1? r2s : 576; if(e>nbig) e=nbig;
    int t=g->ts[reg]; if(t==4||t==14||t>31) return 0;
    if(t==0){ i=e>i?e:i; continue; }
    const HuffTab*h=&HT[t]; int xl=h->xlen;
    for(;i<e;i+=2){
      int cur=0,v;
      for(;;){ int b=rbit(&r); if(b<0) return 0; int nx=TR[t][cur].c[b]; if(nx<0){v=-nx-1;break;} if(nx==0) return 0; cur=nx; }
      int x=v/xl,y=v%xl;
      if(x==15&&h->linbits){ uint32_t l; if(!rbits(&r,h->linbits,&l)) return 0; x+=l; }
      if(x){ int b=rbit(&r); if(b<0) return 0; if(b) x=-x; }
      if(y==15&&h->linbits){ uint32_t l; if(!rbits(&r,h->linbits,&l)) return 0; y+=l; }
      if(y){ int b=rbit(&r); if(b<0) return 0; if(b) y=-y; }
      g->xr[i]=x; g->xr[i+1]=y;
    }
  }
  /* count1 */
  int ct=g->c1t?33:32; int q=0; i=nbig;
  while(i+4<=576){
    BR_ save=r; int cur=0,v=-1; int ok=1;
    for(;;){ int b=rbit(&r); if(b<0){ok=0;break;} int nx=TR[ct][cur].c[b]; if(nx<0){v=-nx-1;break;} if(nx==0){ok=0;break;} cur=nx; }
    if(!ok){ r=save; break; }
    int a[4];
#if QORD==1
    a[0]=(v>>3)&1;a[1]=(v>>2)&1;a[2]=(v>>1)&1;a[3]=v&1;
#else
    a[0]=v&1;a[1]=(v>>1)&1;a[2]=(v>>2)&1;a[3]=(v>>3)&1;
#endif
    for(int k=0;k<4&&ok;k++) if(a[k]){ int b=rbit(&r); if(b<0){ok=0;break;} if(b) a[k]=-1; }
    if(!ok){ r=save; break; }
    for(int k=0;k<4;k++) g->xr[i+k]=a[k];
    i+=4; q++;
  }
  g->nq=q;
  uint64_t used=r.pos-start; g->stuff_n=(int)(g->p23-used);
  if(g->stuff_n<0) return 0;
  g->stuff_off=pool_put_bits(P,ms,r.pos,g->stuff_n);
  return 1;
}

static int enc_gran(BW*w,const Gran*g,const Frame*F,int gri,int ch,int sr,const uint8_t*pool){
  uint64_t st=w->pos; int slen[39]; int n=sf_layout(g,slen);
  for(int i=0;i<n;i++){ if(sf_shared(F,gri,ch,g,i)) continue; wbits(w,g->sf[i],slen[i]); }
  int r1s,r2s; regions(g,sr,&r1s,&r2s); int nbig=g->bv*2; int i=0;
  for(int reg=0;reg<3&&i<nbig;reg++){
    int e= reg==0? r1s : reg==1? r2s : 576; if(e>nbig) e=nbig; int t=g->ts[reg];
    if(t==0){ i=e>i?e:i; continue; }
    const HuffTab*h=&HT[t]; int xl=h->xlen;
    for(;i<e;i+=2){
      int x=g->xr[i],y=g->xr[i+1]; int ax=x<0?-x:x, ay=y<0?-y:y; int lx=0,ly=0;
      int cx=ax,cy=ay;
      if(h->linbits){ if(ax>=15){lx=ax-15;cx=15;} if(ay>=15){ly=ay-15;cy=15;} }
      int idx=cx*xl+cy; wbits(w,h->code[idx],h->len[idx]);
      if(h->linbits&&cx==15) wbits(w,lx,h->linbits);
      if(ax) wbits(w,x<0,1);
      if(h->linbits&&cy==15) wbits(w,ly,h->linbits);
      if(ay) wbits(w,y<0,1);
    }
  }
  int ct=g->c1t?33:32; const HuffTab*h=&HT[ct]; i=nbig;
  for(int q=0;q<g->nq;q++,i+=4){
    int a[4]; for(int k=0;k<4;k++) a[k]=g->xr[i+k];
#if QORD==1
    int v=(!!a[0])*8+(!!a[1])*4+(!!a[2])*2+(!!a[3]);
#else
    int v=(!!a[0])+(!!a[1])*2+(!!a[2])*4+(!!a[3])*8;
#endif
    wbits(w,h->code[v],h->len[v]);
    for(int k=0;k<4;k++) if(a[k]) wbits(w,a[k]<0,1);
  }
  for(int k=0;k<g->stuff_n;k++) wbits(w,(pool[g->stuff_off+(k>>3)]>>(7-(k&7)))&1,1);
  return (int)(w->pos-st);
}

/* ---------- side info / header ---------- */
static int hdr_ok(const uint8_t*h,int*fs,int*sr,int*nch){
  if(h[0]!=0xFF||(h[1]&0xFE)!=0xFA) return 0;           /* sync, MPEG1, Layer III */
  int br=h[2]>>4, s=(h[2]>>2)&3, pad=(h[2]>>1)&1;
  if(br==0||br==15||s==3) return 0;
  *sr=s; *nch=((h[3]>>6)==3)?1:2; *fs=144000*BR[br]/SR[s]+pad; return 1;
}
static void parse_side(const uint8_t*si,Frame*F,int sr){
  BR_ r={si,0,(uint64_t)(F->nch==1?17:32)*8}; uint32_t v;
  rbits(&r,9,&v);F->mdb=v; rbits(&r,F->nch==1?5:3,&v);F->priv=v;
  for(int c=0;c<F->nch;c++) for(int k=0;k<4;k++){rbits(&r,1,&v);F->scfsi[c][k]=v;}
  for(int gi=0;gi<2;gi++) for(int c=0;c<F->nch;c++){ Gran*g=&F->gr[gi][c];
    rbits(&r,12,&v);g->p23=v; rbits(&r,9,&v);g->bv=v; rbits(&r,8,&v);g->gg=v; rbits(&r,4,&v);g->sfc=v;
    rbits(&r,1,&v);g->ws=v; g->bt=0;g->mixed=0;g->ts[0]=g->ts[1]=g->ts[2]=0;g->sbg[0]=g->sbg[1]=g->sbg[2]=0;g->r0=g->r1=0;
    if(g->ws){ rbits(&r,2,&v);g->bt=v; rbits(&r,1,&v);g->mixed=v; for(int k=0;k<2;k++){rbits(&r,5,&v);g->ts[k]=v;} for(int k=0;k<3;k++){rbits(&r,3,&v);g->sbg[k]=v;} }
    else { for(int k=0;k<3;k++){rbits(&r,5,&v);g->ts[k]=v;} rbits(&r,4,&v);g->r0=v; rbits(&r,3,&v);g->r1=v; }
    rbits(&r,1,&v);g->pre=v; rbits(&r,1,&v);g->sfs=v; rbits(&r,1,&v);g->c1t=v; }
  (void)sr;
}
static void build_side(uint8_t*si,const Frame*F){
  BW w={si,0,(uint64_t)(F->nch==1?17:32)*8}; memset(si,0,F->nch==1?17:32);
  wbits(&w,F->mdb,9); wbits(&w,F->priv,F->nch==1?5:3);
  for(int c=0;c<F->nch;c++) for(int k=0;k<4;k++) wbits(&w,F->scfsi[c][k],1);
  for(int gi=0;gi<2;gi++) for(int c=0;c<F->nch;c++){ const Gran*g=&F->gr[gi][c];
    wbits(&w,g->p23,12); wbits(&w,g->bv,9); wbits(&w,g->gg,8); wbits(&w,g->sfc,4); wbits(&w,g->ws,1);
    if(g->ws){ wbits(&w,g->bt,2); wbits(&w,g->mixed,1); for(int k=0;k<2;k++) wbits(&w,g->ts[k],5); for(int k=0;k<3;k++) wbits(&w,g->sbg[k],3); }
    else { for(int k=0;k<3;k++) wbits(&w,g->ts[k],5); wbits(&w,g->r0,4); wbits(&w,g->r1,3); }
    wbits(&w,g->pre,1); wbits(&w,g->sfs,1); wbits(&w,g->c1t,1); }
}

static void parsed_free(Parsed*P){
  for(int i=0;i<P->n;i++){ Run*R=P->s[i].run; if(R){ free(R->f); free(R); } }
  free(P->s); free(P->pool); memset(P,0,sizeof *P); }

/* ---------- parse whole file ---------- */
static Seg* seg_add(Parsed*P,int is_run,uint64_t off,uint64_t len){
  if(P->n==P->cap){P->cap=P->cap?P->cap*2:16;P->s=realloc(P->s,P->cap*sizeof(Seg));}
  Seg*s=&P->s[P->n++]; s->is_run=is_run;s->off=off;s->len=len;s->run=0; return s; }

static void parse_file(const uint8_t*buf,uint64_t n,Parsed*P){
  tinit_all(); memset(P,0,sizeof *P);
  uint64_t pos=0, raw0=0; Run*run=0; uint8_t*ms=0; uint64_t mscap=0, mslen=0, prev_end=0, run_off=0;
  #define FLUSH_RAW(upto) do{ if((upto)>raw0) seg_add(P,0,raw0,(upto)-raw0); }while(0)
  #define END_RUN() do{ if(run){ \
      run->tail_n=(uint32_t)(mslen*8-prev_end); run->tail_off=pool_put_bits(P,ms,prev_end,run->tail_n); run->total_pay=mslen; \
      Seg*s=seg_add(P,1,run_off,pos-run_off); s->run=run; run=0; raw0=pos; } }while(0)
  while(pos+4<=n){
    int fs,sr,nch;
    if(hdr_ok(buf+pos,&fs,&sr,&nch) && pos+fs<=n){
      int nxt_ok=1; if(pos+fs+4<=n){ int a,b,c; nxt_ok=hdr_ok(buf+pos+fs,&a,&b,&c);} 
      int ok=nxt_ok;
      Frame F; memset(&F,0,sizeof F);
      int has_crc=!(buf[pos+1]&1); int si=nch==1?17:32; int hl=4+(has_crc?2:0);
      if(ok && fs>=hl+si){
        memcpy(F.hdr,buf+pos,4); F.has_crc=has_crc; if(has_crc) memcpy(F.crc,buf+pos+4,2); F.nch=nch; F.pay=fs-hl-si;
        parse_side(buf+pos+hl,&F,sr);
        if(!run){ FLUSH_RAW(pos); run=calloc(1,sizeof(Run)); run_off=pos; mslen=0; prev_end=0; }
        /* append payload */
        if(mslen+F.pay>mscap){ mscap=(mslen+F.pay)*2+4096; ms=realloc(ms,mscap); }
        uint64_t M=mslen; memcpy(ms+M,buf+pos+hl+si,F.pay); mslen+=F.pay;
        int64_t S=(int64_t)M*8-(int64_t)F.mdb*8; uint64_t cur=0; int good=(S>=0 && (uint64_t)S>=prev_end);
        size_t pool_mark=P->pn; 
        if(good){ cur=S;
          for(int gi=0;gi<2&&good;gi++) for(int c=0;c<nch&&good;c++){ Gran*g=&F.gr[gi][c];
            if(cur+g->p23>mslen*8){good=0;break;}
            if(!dec_gran(ms,cur,g,&F,gi,c,sr,P)) good=0; else cur+=g->p23; }
        }
        if(good){
          F.gap_n=(uint32_t)(S-prev_end); F.gap_off=pool_put_bits(P,ms,prev_end,F.gap_n);
          prev_end=cur;
          if(run->nf==run->cap){run->cap=run->cap?run->cap*2:256;run->f=realloc(run->f,run->cap*sizeof(Frame));}
          run->f[run->nf++]=F; pos+=fs; continue;
        }
        P->pn=pool_mark; mslen=M;           /* roll back */
        { uint64_t p0=pos; END_RUN(); (void)p0; }
        /* frame stays raw: it will be absorbed by next FLUSH_RAW */
        pos+=fs; /* skip whole frame as raw */
        continue;
      }
    }
    /* not a frame: end run, advance one byte */
    END_RUN(); pos++;
  }
  END_RUN(); FLUSH_RAW(n);
  free(ms);
}

static int build_file(const Parsed*P,const uint8_t*orig,uint8_t*out){
  for(int si=0;si<P->n;si++){ const Seg*s=&P->s[si];
    if(!s->is_run){ if(orig!=out && s->len) memcpy(out+s->off,orig+s->off,s->len); continue; }
    const Run*R=s->run; uint8_t*ms=calloc(R->total_pay+8,1); BW w={ms,0,(R->total_pay+8)*8}; uint64_t M=0;
    for(int fi=0;fi<R->nf;fi++){ Frame*F=(Frame*)&R->f[fi]; int sr=(F->hdr[2]>>2)&3;
      /* gap */
      for(uint32_t k=0;k<F->gap_n;k++) wbits(&w,(P->pool[F->gap_off+(k>>3)]>>(7-(k&7)))&1,1);
      for(int gi=0;gi<2;gi++) for(int c=0;c<F->nch;c++){ Gran*g=&F->gr[gi][c]; g->p23=enc_gran(&w,g,F,gi,c,sr,P->pool); }
      M+=F->pay; }
    for(uint32_t k=0;k<R->tail_n;k++) wbits(&w,(P->pool[R->tail_off+(k>>3)]>>(7-(k&7)))&1,1);
    if(w.pos>R->total_pay*8){ free(ms); return 0; }
    uint64_t o=s->off, m=0;
    for(int fi=0;fi<R->nf;fi++){ Frame*F=(Frame*)&R->f[fi]; int si_n=F->nch==1?17:32;
      memcpy(out+o,F->hdr,4); o+=4; if(F->has_crc){memcpy(out+o,F->crc,2);o+=2;}
      build_side(out+o,F); o+=si_n; memcpy(out+o,ms+m,F->pay); o+=F->pay; m+=F->pay; }
    free(ms);
  }
  return 1;
}


/* mp3codec.c - entropy coder for parsed MP3 (checkpoint 3). Encoder and decoder share one code path.
   API: size_t mp3c_compress(buf,n,out,outcap)  /  int mp3c_decompress(in,inlen,out,n)         */
#include <math.h>
#include <time.h>

/* ---------- integer-only logistic helpers (identical on every CPU) ---------- */
static int16_t STRETCH[4096]; static int sq_init=0;
static int squash(int d){ static const int t[33]={1,2,3,6,10,16,27,45,73,120,194,310,488,747,1101,1546,2047,2549,2994,3348,3607,3785,3901,3975,4022,4050,4068,4079,4085,4089,4092,4093,4094};
  if(d>2047) return 4095; if(d<-2047) return 1; int w=d&127; d=(d>>7)+16; return (t[d]*(128-w)+t[d+1]*w+64)>>7; }
static int16_t SQT[4096];
static void sq_setup(void){ if(sq_init) return; sq_init=1; for(int x=-2048;x<2048;x++) SQT[x+2048]=squash(x); int pi=0; for(int x=-2047;x<=2047;x++){ int v=squash(x); for(int i=pi;i<=v;i++) STRETCH[i]=x; pi=v+1; } for(int i=pi;i<4096;i++) STRETCH[i]=2047; }
static int DT[1024];
static void dt_setup(void){ for(int i=0;i<1024;i++) DT[i]=16384/(i+i+3); }

#ifdef COSTSTAT
static double COSTV[8]; static int CAT=0;
#define CATSET(n) (CAT=(n))
#define COSTADD(bit,p) do{ COSTV[CAT]+= -log2(((bit)?(p):4096-(p))/4096.0); }while(0)
#else
#define CATSET(n) ((void)0)
#define COSTADD(bit,p) ((void)0)
#endif
/* ---------- range coder ---------- */
typedef struct{ uint32_t x1,x2,x; uint8_t*buf; size_t pos,cap; int dec; int overflow; }Coder;
static void c_init_enc(Coder*c,uint8_t*b,size_t cap){ c->x1=0;c->x2=0xffffffff;c->buf=b;c->pos=0;c->cap=cap;c->dec=0;c->overflow=0; }
static void c_init_dec(Coder*c,const uint8_t*b,size_t n){ c->x1=0;c->x2=0xffffffff;c->buf=(uint8_t*)b;c->pos=0;c->cap=n;c->dec=1;c->x=0;c->overflow=0;
  for(int i=0;i<4;i++) c->x=(c->x<<8)|(c->pos<n?b[c->pos++]:0); }
/* p = P(bit==1) in 12 bits, 1..4095 */
static inline void c_norm(Coder*c){
  while(((c->x1^c->x2)&0xff000000)==0){
    if(c->dec){ c->x=(c->x<<8)|(c->pos<c->cap?c->buf[c->pos]:0); c->pos++; if(c->pos>c->cap+8) c->overflow=1; }
    else { if(c->pos<c->cap) c->buf[c->pos]=c->x2>>24; else c->overflow=1; c->pos++; }
    c->x1<<=8; c->x2=(c->x2<<8)|255; }
}
static inline int c_code(Coder*c,int bit,int p){
  if(!c->dec) COSTADD(bit,p);
  uint32_t xm=c->x1+(uint32_t)(((uint64_t)(c->x2-c->x1)*(uint32_t)p)>>12);
  if(c->dec) bit = c->x<=xm;
  if(bit) c->x2=xm; else c->x1=xm+1;
  c_norm(c);
  return bit;
}
/* n (1..12) equiprobable bits in one step; falls back to bitwise when the range is too small */
static inline int c_raw(Coder*c,int v,int n){
  uint64_t R=(uint64_t)(c->x2-c->x1)+1;
  if(R<((uint64_t)1<<(n+8))){ int r=0; for(int i=n-1;i>=0;i--) r=r*2+c_code(c,(v>>i)&1,2048); return r; }
  if(c->dec){ uint64_t d=(uint64_t)(c->x-c->x1); v=(int)((((d+1)<<n)-1)/R); }
  c->x2=c->x1+(uint32_t)((R*(uint64_t)(v+1))>>n)-1; c->x1=c->x1+(uint32_t)((R*(uint64_t)v)>>n);
  c_norm(c); return v;
}
static void c_flush(Coder*c){ if(!c->dec){ for(int i=0;i<4;i++){ if(c->pos<c->cap) c->buf[c->pos]=c->x1>>24; else c->overflow=1; c->pos++; c->x1<<=8; } } }

/* ---------- adaptive probability slots (lpaq StateMap style) ---------- */
typedef uint32_t SM;
static SM* sm_new(size_t n){ SM*s=malloc(n*sizeof(SM)); for(size_t i=0;i<n;i++) s[i]=1u<<31; return s; }
static inline int sm_p(const SM*s){ int p=*s>>20; return p<1?1:p>4095?4095:p; }
static inline void sm_up(SM*s,int y,int limit){ int n=*s&1023, p=*s>>10; if(n<limit)++*s; else *s=(*s&0xfffffc00)|limit;
  *s+=(((y<<22)-p)>>3)*DT[n]&0xfffffc00; }
static inline int code_sm(Coder*c,SM*s,int bit,int limit){ bit=c_code(c,bit,sm_p(s)); sm_up(s,bit,limit); return bit; }
static int code_tree(Coder*c,SM*sm,uint32_t ctx,int nb,int v){
  int node=1; for(int i=nb-1;i>=0;i--){ int b=code_sm(c,&sm[(ctx<<nb)+node],(v>>i)&1,60); node=node*2+b; } return node-(1<<nb); }
static int code_gamma(Coder*c,SM*sm,uint32_t v){ /* sm has 64 slots */
  uint32_t n=v+1; int nb=0; if(!c->dec){ while((n>>(nb+1))) nb++; }
  int k=0; for(;;k++){ int stop=code_sm(c,&sm[k<31?k:31],c->dec?0:(k==nb),60); if(stop) break; if(k>=31) break; }
  nb=k; uint32_t r=1;
  for(int i=nb-1;i>=0;i--){ int b=(n>>i)&1; if(i==nb-1) b=code_sm(c,&sm[32+(nb<31?nb:31)],b,60); else b=c_code(c,b,2048); r=r*2+b; }
  return (int)(r-1);
}

/* ---------- mixer ---------- */
#define NIN 8
typedef struct{ int32_t w[NIN]; }MixW;
typedef struct{ MixW*ws; int st[NIN]; int n; int pr; MixW*cur; int shift; }Mixer;
static void mx_init(Mixer*m,int nsets,int n,int shift){ m->ws=malloc(nsets*sizeof(MixW)); for(int s=0;s<nsets;s++){ for(int i=0;i<NIN;i++) m->ws[s].w[i]=0; for(int i=0;i<n-1;i++) m->ws[s].w[i]=65536*3/((n-1)*2); } m->n=n; m->shift=shift; }
#define WCLAMP (6<<16)
#define NMX 4
static inline int mx_mix(Mixer*m,int set){ m->cur=&m->ws[set]; int32_t d=0; for(int i=0;i<NMX;i++) d+=m->st[i]*(m->cur->w[i]>>2); int x=d>>14; if(x>2047)x=2047; if(x<-2047)x=-2047; m->pr=SQT[x+2048]; return m->pr; }
static inline void mx_up(Mixer*m,int y){ int err=((y<<12)-m->pr); if(err<24&&err>-24) return; for(int i=0;i<NMX;i++){ int32_t w=m->cur->w[i]+((m->st[i]*err)>>m->shift); w=w>WCLAMP?WCLAMP:w<-WCLAMP?-WCLAMP:w; m->cur->w[i]=w; } }

/* ---------- model state ---------- */
#define NS 16
static void mult_setup(void);
#define NQ 12
typedef struct{
  SM *hdr, *mdb, *priv, *scfsi, *bt, *ts, *sbg, *r0, *r1, *flags, *sfc, *gg, *bv, *nq, *sf, *stuffn, *stuffz, *stuffb, *gapz, *gapb, *segf, *rawb, *gam, *esc, *crc;
  SM *cB,*cD,*cF,*mant,*zq; Mixer mx;
}Model;
static Model* model_new(void){
  sq_setup(); dt_setup(); mult_setup(); Model*M=calloc(1,sizeof(Model));
  M->hdr=sm_new(4*256*256); M->mdb=sm_new(16*512*2); M->priv=sm_new(64); M->scfsi=sm_new(16*16*2);
  M->bt=sm_new(16*16*2); M->ts=sm_new(3*32*32*32*2); M->sbg=sm_new(3*8*8*2); M->r0=sm_new(16*16*2); M->r1=sm_new(8*8*2);
  M->flags=sm_new(64); M->sfc=sm_new(16*16*2); M->gg=sm_new(256*2*2*2); M->bv=sm_new(512*2*2*2*8); M->nq=sm_new(256*16*2*2);
  M->sf=sm_new(39*16*16*2); M->stuffn=sm_new(64); M->stuffz=sm_new(4); M->stuffb=sm_new(8); M->gapz=sm_new(4); M->gapb=sm_new(8);
  M->segf=sm_new(8); M->rawb=sm_new(256*256); M->gam=sm_new(64*8); M->esc=sm_new(64*4); M->crc=sm_new(256);
  M->cB=sm_new(NQ*NQ*NS); M->cD=sm_new(2*NQ*NQ*NS); M->cF=sm_new(4*20*NQ*NS); M->mant=sm_new(16*NQ*4); M->zq=sm_new(2*4*2*2*72);
  mx_init(&M->mx,NS*2*2,NMX,10); return M; }

static void model_free(Model*M){
  if(!M) return; SM**p=(SM**)M; size_t k=offsetof(Model,mx)/sizeof(SM*);
  for(size_t i=0;i<k;i++) free(p[i]); free(M->mx.ws); free(M); }

/* ---------- granule bit counting (for derived gap sizes) ---------- */
static int gran_bits(const Gran*g,const Frame*F,int gri,int ch,int sr,const uint8_t*pool){
  (void)pool; int slen[39]; int n=sf_layout(g,slen); int tot=0;
  for(int i=0;i<n;i++){ if(sf_shared(F,gri,ch,g,i)) continue; tot+=slen[i]; }
  int r1s,r2s; regions(g,sr,&r1s,&r2s); int nbig=g->bv*2; int i=0;
  for(int reg=0;reg<3&&i<nbig;reg++){
    int e= reg==0? r1s : reg==1? r2s : 576; if(e>nbig) e=nbig; int t=g->ts[reg];
    if(t==0){ i=e>i?e:i; continue; }
    const HuffTab*h=&HT[t]; int xl=h->xlen, lb=h->linbits;
    for(;i<e;i+=2){
      int x=g->xr[i],y=g->xr[i+1]; int ax=x<0?-x:x, ay=y<0?-y:y;
      int cx=ax,cy=ay;
      if(lb){ if(ax>=15){cx=15;tot+=lb;} if(ay>=15){cy=15;tot+=lb;} }
      tot+=h->len[cx*xl+cy]+(ax!=0)+(ay!=0);
    }
  }
  int ct=g->c1t?33:32; const HuffTab*h=&HT[ct]; i=nbig;
  for(int q=0;q<g->nq;q++,i+=4){
    int a0=g->xr[i],a1=g->xr[i+1],a2=g->xr[i+2],a3=g->xr[i+3];
#if QORD==1
    int v=(!!a0)*8+(!!a1)*4+(!!a2)*2+(!!a3);
#else
    int v=(!!a0)+(!!a1)*2+(!!a2)*4+(!!a3)*8;
#endif
    tot+=h->len[v]+(!!a0)+(!!a1)+(!!a2)+(!!a3);
  }
  return tot+g->stuff_n;
}

/* ---------- coefficient model ---------- */

static const uint8_t PRETAB[22]={0,0,0,0,0,0,0,0,0,0,0,1,1,1,1,2,2,3,3,3,2,0};
static uint32_t MULT16[16]; static int mult_init=0;
static void mult_setup(void){ if(mult_init) return; mult_init=1;
  /* 65536*2^(r/16), integer-only (fixed table, computed from a rational chain of 2^(1/16)) */
  static const uint32_t T[16]={65536,68438,71468,74632,77936,81386,84990,88752,92682,96785,101070,105545,110218,115098,120194,125515};
  for(int i=0;i<16;i++) MULT16[i]=T[i]; }
static inline int scale_mag(int v,int e){ /* v * 2^(e/16) */
  if(e>16*12) e=16*12; if(e<-16*12) e=-16*12; int sh=e>>4, fr=e&15; int64_t r=((int64_t)v*MULT16[fr])>>16;
  if(sh>=0) r<<=sh; else r>>=-sh; return r>4095?4095:(int)r; }
static void compute_L4(Gran*g,int sr){
  g->lvalid=0; if(g->ws) return; g->lvalid=1; int b=0; int base=g->gg-210;
  for(int i=0;i<576;i++){ while(b<21&&i>=SFB[sr][b+1]) b++; int sf=b<21?g->sf[b]:0; int pt=g->pre?PRETAB[b]:0; int v=base-2*(1+g->sfs)*(sf+pt); g->L4[i]=v; } }
static const int16_t ZERO576[580];

static const uint8_t QMT[101]={0,1,2,3,4,5,5,6,6,7,7,7,7,8,8,8,8,8,8,8,8,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10};
static inline int qm(int v){ return v>100?11:QMT[v]; }
static inline int lq(int e){ if(e<4) return e; int b=31-__builtin_clz(e); int q=2*b+((e>>(b-1))&1); return q>31?31:q; }
static const uint8_t QMT2[102]={0,1,2,3,4,5,5,6,6,7,7,7,7,8,8,8,8,8,8,8,8,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,11};
#define QM(v) QMT2[(v)>101?101:(v)]
/* neighbour magnitudes of a reference granule, rescaled to this granule's gain; o[j+1] holds |ref[j]| (o[0]=0) */
static void prescale(uint16_t*o,const Gran*src,const Gran*g,int scaled,int end){
  memset(o,0,578*sizeof(uint16_t)); if(!src) return;
  int lim=end<575?end:575;
  if(!scaled){ for(int j=0;j<=lim;j++){ int v=src->xr[j]; o[j+1]=v<0?-v:v; } return; }
  int laste=1<<20; uint32_t mul=0; int sh=0;
  for(int j=0;j<=lim;j++){
    int e=3*(src->L4[j]-g->L4[j]);
    if(e!=laste){ laste=e; int ee=e>192?192:e<-192?-192:e; sh=ee>>4; mul=MULT16[ee&15]; }
    int v=src->xr[j]; v=v<0?-v:v; int64_t r=((int64_t)v*mul)>>16; if(sh>=0) r<<=sh; else r>>=-sh;
    o[j+1]=r>4095?4095:(int)r; }
}
typedef struct{ SM*p[3]; }CtxP;
static inline int cbit(Coder*c,Mixer*m,const CtxP*cp,int s,int mset,int bit){
  SM*p0=cp->p[0]+s,*p1=cp->p[1]+s,*p2=cp->p[2]+s;
  m->st[0]=STRETCH[sm_p(p0)]; m->st[1]=STRETCH[sm_p(p1)]; m->st[2]=STRETCH[sm_p(p2)]; m->st[3]=256;
  mx_mix(m,mset); bit=c_code(c,bit,m->pr);
  mx_up(m,bit); sm_up(p0,bit,127); sm_up(p1,bit,127); sm_up(p2,bit,127);
  return bit;
}
static void code_coefs(Coder*c,Model*M,Gran*g,const Gran*pg,const Gran*og,int ch){
  int16_t*x=g->xr;
  int nbig=g->bv*2, ncnt=g->nq*4, end=nbig+ncnt; int btc=g->ws?g->bt:0;
  if(c->dec) memset(x,0,sizeof g->xr);
  int usep=pg&&pg->lvalid&&g->lvalid, useo=og&&og->lvalid&&g->lvalid;
  uint16_t sP[578], sO[578];
  prescale(sP,pg,g,usep,end); if(ch) prescale(sO,og,g,useo,end);
  Mixer*m=&M->mx; int a=0,b=0,c3=0; uint8_t sgnb[576]; int nsb=0;
  for(int i=0;i<end;i++){
    int p0=sP[i+1], pm=sP[i], pp=sP[i+2];
    int o0=0,om=0,op=0; if(ch){ o0=sO[i+1]; om=sO[i]; op=sO[i+2]; }
    int qa=QM(a), qp0=QM(p0);
    int ib=qp0*NQ+QM((pm+pp+1)>>1);
    int id= ch? (NQ*NQ+QM(o0)*NQ+QM((om+op+1)>>1)) : (qp0*NQ+qa);
    int iff=(btc*20+(i>>5>19?19:i>>5))*NQ+qa;
    CtxP cp={{&M->cB[ib*NS],&M->cD[id*NS],&M->cF[iff*NS]}};
    int v=x[i]; int mg=abs(v); int cnt=i>=nbig; int na;
    int zn=((a|b|p0|pm|pp|o0|om|op)==0); SM*zs=&M->zq[((((ch*4+btc)*2+cnt)*2)+(c3!=0))*72+(i>>3)];
    if(cnt){ CATSET(3);
      int nz= zn? code_sm(c,zs,mg!=0,1023) : cbit(c,m,&cp,15,15*2+ch,mg!=0);
      if(nz){ sgnb[nsb++]=v<0; x[i]=1; na=1; } else { x[i]=0; na=0; }
    } else {
      CATSET(2);
      int k=0; if(!c->dec) k=mg?32-__builtin_clz(mg):0;
      int s=0; if(zn){ if(!code_sm(c,zs,k>0,1023)) goto kdone; s=1; }
      for(;s<14;s++){ int gt=cbit(c,m,&cp,s,s*2+ch,k>s); if(!gt) break; }
      kdone:
      k=s;
      if(k>=2){ int r=1; int mb=k-1;
        int j=mb-1; int bit=code_sm(c,&M->mant[(k*NQ+qa)*4],(mg>>j)&1,255); r=r*2+bit; j--;
        if(j>=0){ bit=code_sm(c,&M->mant[(k*NQ+qa)*4+1+(r&1)],(mg>>j)&1,255); r=r*2+bit; j--; }
        if(j>=0){ int nb=j+1; int lo=c_raw(c,mg&((1<<nb)-1),nb); r=(r<<nb)|lo; }
        mg=r; } else mg=k;
      if(mg){ sgnb[nsb++]=v<0; x[i]=mg; } else x[i]=0;
      na=mg;
    }
    c3=b; b=a; a=na;
  }
  CATSET(4);
  { int pos=0; while(pos<nsb){ int n=nsb-pos>12?12:nsb-pos; int v=0; if(!c->dec) for(int t=0;t<n;t++) v=v*2+sgnb[pos+t]; v=c_raw(c,v,n);
      if(c->dec) for(int t=0;t<n;t++) sgnb[pos+t]=(v>>(n-1-t))&1; pos+=n; } }
  if(c->dec){ int t=0; for(int i=0;i<end;i++) if(x[i]){ if(sgnb[t++]) x[i]=-x[i]; } }
}

/* ---------- side info / frame coding ---------- */
static int alloc_bits(Parsed*P,uint32_t nbits){ size_t nb=(nbits+7)/8; if(P->pn+nb>P->pcap){P->pcap=(P->pn+nb)*2+1024;P->pool=realloc(P->pool,P->pcap);} uint32_t off=P->pn; if(nb) memset(P->pool+off,0,nb); P->pn+=nb; return off; }
static uint32_t code_bitblock(Coder*c,Model*M,Parsed*P,uint32_t off,uint32_t n,SM*zs,SM*bs){
  /* flag: all zero? else each bit with ctx of previous bit */
  int allz=1; if(!c->dec) for(uint32_t k=0;k<n;k++) if((P->pool[off+(k>>3)]>>(7-(k&7)))&1){allz=0;break;}
  if(n==0) return off;
  allz=code_sm(c,&zs[0],allz,60);
  if(c->dec) off=alloc_bits(P,n);
  if(!allz){ int prev=0; for(uint32_t k=0;k<n;k++){ int b=(P->pool[off+(k>>3)]>>(7-(k&7)))&1; b=code_sm(c,&bs[prev],b,60); prev=b;
      if(c->dec&&b) P->pool[off+(k>>3)]|=0x80>>(k&7); } }
  (void)M; return off;
}
/* decoder-side sanity check: only granules that enc_gran can write without reading outside its tables */
static int gran_valid(const Gran*g,int sr){
  if(g->bv>288||g->nq<0||g->bv*2+g->nq*4>576) return 0;
  int r1s,r2s; regions(g,sr,&r1s,&r2s); int nbig=g->bv*2, i=0;
  for(int reg=0;reg<3&&i<nbig;reg++){
    int e= reg==0? r1s : reg==1? r2s : 576; if(e>nbig) e=nbig; int t=g->ts[reg];
    if(t==0){ i=e>i?e:i; continue; }
    if(t<0||t>31) return 0; const HuffTab*h=&HT[t]; if(!h->code||!h->len||h->xlen<=0) return 0;
    for(;i<e;i+=2){ int ax=g->xr[i],ay=g->xr[i+1]; ax=ax<0?-ax:ax; ay=ay<0?-ay:ay;
      if(!h->linbits){ if(ax>=h->xlen||ay>=h->xlen) return 0; } }
  }
  return 1;
}
typedef struct{ int gg,bv,nq,bt,ts[3],r0,r1,sfc,pre,sfs,c1t; int sf[39]; }PrevG;

static void code_frame(Coder*c,Model*M,Parsed*P,Frame*F,Frame*pf,int first,uint64_t*Mpos,uint64_t*prev_end,Gran*pg[2],int*stuffctx){
  /* header bytes */
  for(int k=0;k<4;k++){ uint32_t ctx=k*256+(pf?pf->hdr[k]:0); F->hdr[k]=code_tree(c,M->hdr,ctx,8,F->hdr[k]); }
  if(c->dec){ int a_,b_,d_; if(c->overflow||!hdr_ok(F->hdr,&a_,&b_,&d_)){ c->overflow=1; return; } }
  int sr=(F->hdr[2]>>2)&3; F->nch=((F->hdr[3]>>6)==3)?1:2; F->has_crc=!(F->hdr[1]&1);
  int fs=144000*BR[F->hdr[2]>>4]/SR[sr]+((F->hdr[2]>>1)&1); int hl=4+(F->has_crc?2:0), sin=F->nch==1?17:32; F->pay=fs-hl-sin;
  if(F->has_crc) for(int k=0;k<2;k++) F->crc[k]=code_tree(c,M->crc,0,8,F->crc[k]) , (void)0;
  int pm=pf?pf->mdb:0; F->mdb=code_tree(c,M->mdb,(pm>>5)&15,9,F->mdb);
  F->priv=code_tree(c,M->priv,0,F->nch==1?5:3,F->priv);
  for(int ch=0;ch<F->nch;ch++){ int pv=pf?(pf->scfsi[ch][0]|pf->scfsi[ch][1]<<1|pf->scfsi[ch][2]<<2|pf->scfsi[ch][3]<<3):0;
    int v=F->scfsi[ch][0]|F->scfsi[ch][1]<<1|F->scfsi[ch][2]<<2|F->scfsi[ch][3]<<3; v=code_tree(c,M->scfsi,pv+16*ch,4,v);
    for(int k=0;k<4;k++) F->scfsi[ch][k]=(v>>k)&1; }
  /* derived position */
  int64_t S=(int64_t)*Mpos*8-(int64_t)F->mdb*8;
  if(c->dec){ if(S<(int64_t)*prev_end) { c->overflow=1; return; } F->gap_n=(uint32_t)(S-*prev_end); }
  F->gap_off=code_bitblock(c,M,P,F->gap_off,F->gap_n,M->gapz,M->gapb);
  for(int gi=0;gi<2;gi++) for(int ch=0;ch<F->nch;ch++){
    Gran*g=&F->gr[gi][ch]; const Gran*q=pg[ch]; int pbt=q?(q->ws?q->bt:0):0; int pmx=q?q->mixed:0;
    int blk=(g->ws?1:0)|((g->bt&3)<<1)|(g->mixed<<3); int pblk=q?((q->ws?1:0)|((q->bt&3)<<1)|(q->mixed<<3)):0; (void)pbt;(void)pmx;
    blk=code_tree(c,M->bt,pblk+16*ch,4,blk); g->ws=blk&1; g->bt=(blk>>1)&3; g->mixed=blk>>3; if(!g->ws){g->bt=0;g->mixed=0;}
    int pgg=q?q->gg:0; g->gg=(pgg+code_tree(c,M->gg,((q?1:0)*2+ch)*1+0,8,((g->gg-pgg)&255)))&255;
    int pbv=q?q->bv:0; g->bv=(pbv+code_tree(c,M->bv,((ch*2+(g->ws&&g->bt==2))*8+(q?q->bv/37:0))*1,9,((g->bv-pbv)&511)))&511;
    /* nq (count1 quads), ctx = previous nq bucket */
    g->nq=code_tree(c,M->nq,((q?q->nq>>4:0)&15)*1+16*ch*0,8,g->nq);
    if(c->dec && (c->overflow || g->bv>288 || g->bv*2+g->nq*4>576)){ c->overflow=1; return; }
    int pf0[3]={q?q->ts[0]:0,q?q->ts[1]:0,q?q->ts[2]:0}; int nts=g->ws?2:3;
    for(int k=0;k<3;k++){ if(k<nts) g->ts[k]=code_tree(c,M->ts,(k*32+pf0[k])*32*0+k*32+pf0[k],5,g->ts[k]); else g->ts[k]=0; }
    if(g->ws){ for(int k=0;k<3;k++) g->sbg[k]=code_tree(c,M->sbg,k*8,3,g->sbg[k]); g->r0=g->r1=0; }
    else { g->r0=code_tree(c,M->r0,q?q->r0:0,4,g->r0); g->r1=code_tree(c,M->r1,q?q->r1:0,3,g->r1); for(int k=0;k<3;k++) g->sbg[k]=0; }
    g->sfc=code_tree(c,M->sfc,q?q->sfc:0,4,g->sfc);
    int fl=g->pre|(g->sfs<<1)|(g->c1t<<2); int pfl=q?(q->pre|(q->sfs<<1)|(q->c1t<<2)):0; fl=code_tree(c,M->flags,pfl,3,fl); g->pre=fl&1; g->sfs=(fl>>1)&1; g->c1t=(fl>>2)&1;
    /* scalefactors */
    int slen[39]; int n=sf_layout(g,slen); const Gran*g0=&F->gr[0][ch];
    CATSET(1); for(int i=0;i<n;i++){ if(sf_shared(F,gi,ch,g,i)){ g->sf[i]=g0->sf[i]; continue; }
      if(slen[i]==0){ g->sf[i]=0; continue; }
      int pv=q&&i<39?q->sf[i]:0; g->sf[i]=code_tree(c,M->sf,(i*16+(pv&15))+0,4,g->sf[i]); }
    CATSET(5); /* stuffing */
    g->stuff_n=code_gamma(c,&M->gam[0],g->stuff_n);
    if(c->dec && (g->stuff_n<0 || g->stuff_n>4095)){ c->overflow=1; return; }
    g->stuff_off=code_bitblock(c,M,P,g->stuff_off,g->stuff_n,M->stuffz,M->stuffb);
    compute_L4(g,sr); code_coefs(c,M,g,q,ch?&F->gr[gi][0]:NULL,ch); CATSET(0);
    if(c->dec && (c->overflow || !gran_valid(g,sr))){ c->overflow=1; return; }
    if(c->dec){ g->p23=0; }
    int bits=gran_bits(g,F,gi,ch,sr,P->pool); g->p23=bits;
    if(c->dec && (bits<0 || bits>4095)){ c->overflow=1; return; } *prev_end+=0; pg[ch]=g;
    if(gi==0&&ch==0) *prev_end=(uint64_t)S; 
    *prev_end+=bits;
  }
  *Mpos+=F->pay; (void)first; (void)stuffctx; (void)pf;
}

/* ---------- top level ---------- */
static void code_parsed(Coder*c,Model*M,Parsed*P,uint64_t n,const uint8_t*orig,uint8_t*dec_raw){
  int nseg=code_gamma(c,&M->gam[64],P->n);
  if(c->dec){ if(nseg<0||(uint64_t)nseg>n){ c->overflow=1; return; } P->s=calloc(nseg+1,sizeof(Seg)); P->cap=nseg; P->n=nseg; }
  uint64_t off=0;
  for(int si=0;si<nseg;si++){ Seg*s=&P->s[si];
    s->is_run=code_sm(c,&M->segf[0],s->is_run,60);
    if(!s->is_run){ s->len=code_gamma(c,&M->gam[128],(uint32_t)s->len); s->off=off;
      if(c->dec && (s->len>n-off || c->overflow)){ c->overflow=1; return; }
      int prev=0; for(uint64_t k=0;k<s->len;k++){ if(c->dec&&c->overflow) return; int b=c->dec?0:orig[s->off+k]; b=code_tree(c,M->rawb,prev,8,b); if(c->dec) dec_raw[s->off+k]=b; prev=b; }
      off+=s->len; continue; }
    Run*R; if(c->dec){ R=calloc(1,sizeof(Run)); s->run=R; } else R=s->run;
    R->nf=code_gamma(c,&M->gam[192],R->nf);
    if(c->dec){ if(R->nf<0||(uint64_t)R->nf>n/96+1||c->overflow){ c->overflow=1; return; } R->f=calloc(R->nf+1,sizeof(Frame)); R->cap=R->nf; if(!R->f){ c->overflow=1; return; } }
    uint64_t Mpos=0,prev_end=0; Gran*pg[2]={0,0}; int sc=0;
    for(int fi=0;fi<R->nf;fi++){ code_frame(c,M,P,&R->f[fi],fi?&R->f[fi-1]:NULL,fi==0,&Mpos,&prev_end,pg,&sc); if(c->overflow) return; }
    R->total_pay=Mpos; R->tail_n=(uint32_t)(Mpos*8-prev_end);
    R->tail_off=code_bitblock(c,M,P,R->tail_off,R->tail_n,M->gapz,M->gapb);
    uint64_t len=0; for(int fi=0;fi<R->nf;fi++){ int sr=(R->f[fi].hdr[2]>>2)&3; len+=144000*BR[R->f[fi].hdr[2]>>4]/SR[sr]+((R->f[fi].hdr[2]>>1)&1); }
    s->off=off; s->len=len; if(c->dec && len>n-off){ c->overflow=1; return; } off+=len;
  }
  if(c->dec && off!=n) c->overflow=1;
}

size_t mp3c_compress(const uint8_t*buf,size_t n,uint8_t*out,size_t cap){
  if(cap<16) return 0;
  Parsed P; parse_file(buf,n,&P); int runs=0; for(int i=0;i<P.n;i++) runs+=P.s[i].is_run;
  if(!runs){ parsed_free(&P); return 0; }
  Model*M=model_new(); Coder c; c_init_enc(&c,out+9,cap-9); code_parsed(&c,M,&P,n,buf,NULL); c_flush(&c);
  out[0]=3; for(int i=0;i<8;i++) out[1+i]=(uint64_t)n>>(8*i);
  size_t r=c.overflow?0:c.pos+9; model_free(M); parsed_free(&P); return r;
}
int mp3c_decompress(const uint8_t*in,size_t inlen,uint8_t*out,size_t n){
  if(inlen<9||in[0]!=3) return -1; uint64_t m=0; for(int i=0;i<8;i++) m|=(uint64_t)in[1+i]<<(8*i); if(m!=n) return -2;
  tinit_all(); Parsed P; memset(&P,0,sizeof P); Model*M=model_new(); Coder c; c_init_dec(&c,in+9,inlen-9);
  code_parsed(&c,M,&P,n,NULL,out); int r=0; if(c.overflow) r=-3; else if(!build_file(&P,out,out)) r=-4;
  model_free(M); parsed_free(&P); return r;
}


/* ---- kiln entry points ---- */
#if defined(_WIN32)
#define KZ_EXPORT __declspec(dllexport)
#else
#define KZ_EXPORT __attribute__((visibility("default")))
#endif
/* packs an MP3 file; returns the packed size, or 0 if it can't be (or it overflowed cap) */
KZ_EXPORT int64_t kz_mp3_pack(const uint8_t *in, int64_t n, uint8_t *out, int64_t cap) {
    if (n < 64 || cap < 32) return 0;
    return (int64_t)mp3c_compress(in, (size_t)n, out, (size_t)cap);
}
/* 0 = ok; negative = corrupt data (never writes outside out[0..n)) */
KZ_EXPORT int kz_mp3_unpack(const uint8_t *in, int64_t inlen, uint8_t *out, int64_t n) {
    if (n < 0 || inlen < 0) return -9;
    return mp3c_decompress(in, (size_t)inlen, out, (size_t)n);
}
KZ_EXPORT int kz_mp3_version(void) { return 1; }
"""

_accel_init()


if __name__ == "__main__":
    main()
