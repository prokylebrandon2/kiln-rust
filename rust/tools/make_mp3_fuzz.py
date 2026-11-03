#!/usr/bin/env python3
"""Corrupted-blob cases for the MP3 unpacker: <corpus dir> already holds NAME.mp3/NAME.blob from
make_mp3_corpus.py. For some blobs, writes fuzz/NAME_K.blob + NAME_K.exp: the REAL kz_mp3_unpack's answer
(b'\\x00' + output when it accepts the data, b'\\x01' when it refuses). Rust must give the same."""
import importlib.util, os, random, sys, ctypes
kv, d = sys.argv[1], sys.argv[2]
count = int(sys.argv[3]) if len(sys.argv) > 3 else 12
spec = importlib.util.spec_from_file_location("kiln_v14", kv)
K = importlib.util.module_from_spec(spec); sys.modules["kiln_v14"] = K; spec.loader.exec_module(K)
os.makedirs(os.path.join(d, "fuzz"), exist_ok=True)
rnd = random.Random(12345)
names = sorted(f[:-5] for f in os.listdir(d) if f.endswith(".blob"))
pick = [n for n in names if n in ("music_cbr128", "music_mono64", "clicks_cbr128", "shine_mono", "concat_two", "trail_junk",
                                  "crc_flag", "music_32k_48k", "music_vbr9", "pink_vbr0")]
ok = bad = 0
for name in pick:
    blob = open(os.path.join(d, name + ".blob"), "rb").read()
    n = len(open(os.path.join(d, name + ".mp3"), "rb").read())
    for k in range(count):
        b = bytearray(blob)
        kind = k % 6
        if kind == 0:      # flip a few bits somewhere
            for _ in range(rnd.choice((1, 2, 5))):
                b[rnd.randrange(9, len(b))] ^= 1 << rnd.randrange(8)
        elif kind == 1:    # truncate
            del b[rnd.randrange(9, len(b)):]
        elif kind == 2:    # overwrite a run with random bytes
            p = rnd.randrange(9, len(b)); L = min(len(b) - p, rnd.choice((1, 4, 32)))
            b[p:p + L] = bytes(rnd.randrange(256) for _ in range(L))
        elif kind == 4:    # one bit late in the stream (often still decodes: wrong coefficient / raw byte)
            b[rnd.randrange(max(9, len(b) * 7 // 10), len(b))] ^= 1 << rnd.randrange(8)
        elif kind == 5:    # one bit in the middle
            b[rnd.randrange(len(b) * 4 // 10, len(b) * 6 // 10)] ^= 1 << rnd.randrange(8)
        else:              # damage the very beginning of the coded stream (segment table, first frame)
            for _ in range(2):
                b[rnd.randrange(9, min(len(b), 40))] ^= 1 << rnd.randrange(8)
        out = ctypes.create_string_buffer(n + 16)
        r = K._A.kz_mp3_unpack(bytes(b), len(b), out, n)
        exp = b"\x00" + out.raw[:n] if r == 0 else b"\x01"
        ok += r == 0; bad += r != 0
        open(os.path.join(d, "fuzz", f"{name}_{k}.blob"), "wb").write(b)
        open(os.path.join(d, "fuzz", f"{name}_{k}.exp"), "wb").write(n.to_bytes(8, "little") + exp)
print("fuzz cases:", ok + bad, "accepted by the real engine:", ok, "refused:", bad)
