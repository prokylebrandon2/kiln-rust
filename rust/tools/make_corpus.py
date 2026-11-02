#!/usr/bin/env python3
"""Build tests/corpus/ with the REAL kiln engine.
usage: make_corpus.py <kiln_v14.py> <out_dir>
Produces archives (plain, encrypted, store-only, checkpointed, ELF/BCJ), expected.json
(per-archive file table + per-file crc) and blocks/*.bin (raw type-1 block payloads with the
exact expected decoded bytes) for unit-testing the range decoder."""
import importlib.util, json, os, random, shutil, struct, sys, tempfile, zlib, io, contextlib
src, out = sys.argv[1:3]
spec = importlib.util.spec_from_file_location("kiln_v14", src)
K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
PW = "correct-horse-battery"
random.seed(1234)
os.makedirs(out, exist_ok=True)
tmp = tempfile.mkdtemp()
inp = os.path.join(tmp, "in"); os.makedirs(inp + "/sub")
w = lambda p, b: open(os.path.join(inp, p), "wb").write(b)
words = ["".join(random.choice("abcdefghijklmnop") for _ in range(random.randint(3, 9))) for _ in range(2500)]
w("empty.txt", b"")
w("hello.txt", b"hello kiln\n" * 50)
w("sub/text.txt", " ".join(random.choice(words) for _ in range(20000)).encode())
w("sub/floats.bin", b"".join(struct.pack("<f", i * 0.25) for i in range(20000)))
w("sub/records.bin", b"".join(struct.pack("<IHHf", i, i % 7, (i * 3) & 0xFFFF, i / 3) for i in range(8000)))
w("rand.bin", bytes(random.getrandbits(8) for _ in range(40000)))
w("code.py", open(src, "rb").read()[:120000])
shutil.copy("/bin/ls", os.path.join(inp, "ls_elf"))
def run(argv, pw=None):
    if pw: os.environ["KILN_PASSWORD"] = pw
    else: os.environ.pop("KILN_PASSWORD", None)
    old = sys.argv; sys.argv = ["kiln"] + argv
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            K.main()
    except SystemExit as e:
        if e.code not in (0, None): raise
    finally: sys.argv = old
cases = {
  "plain":      dict(args=[], pw=None),
  "enc":        dict(args=["-e"], pw=PW),
  "enc_store":  dict(args=["-e", "-s"], pw=PW),
}
exp = {"password": PW, "archives": {}}
def record(name, pw):
    p = os.path.join(out, name + ".kln")
    f, table, toff, hlen, cipher = K.open_archive(p, pw)
    f.close()
    exp["archives"][name] = {"encrypted": cipher is not None, "table_off": toff, "hdr_len": hlen,
        "ckpts": [list(c) for c in getattr(table, "ckpts", ())],
        "files": [{"rel": e["rel"], "size": e["size"], "crc": e["crc"], "mtime": e["mtime"],
                   "mode": e["mode"], "filt": e["filt"], "dead": e["dead"], "slen": e["slen"]} for e in table]}
for name, c in cases.items():
    p = os.path.join(out, name + ".kln")
    if os.path.exists(p): os.remove(p)
    run(["pack", p, inp, "-q", "-f"] + c["args"], c["pw"]); record(name, c["pw"])
# checkpoints: shrink the span so a small archive gets several
for name, pw, extra in (("ckpt", None, []), ("ckpt_enc", PW, ["-e"])):
    p = os.path.join(out, name + ".kln")
    if os.path.exists(p): os.remove(p)
    old = K.CKPT_SPAN; K.CKPT_SPAN = 60000
    try: run(["pack", p, inp, "-q", "-f"] + extra, pw)
    finally: K.CKPT_SPAN = old
    record(name, pw)
# negative cases
raw = open(os.path.join(out, "plain.kln"), "rb").read()
def put(n, b): open(os.path.join(out, n + ".kln"), "wb").write(b)
put("bad_truncated", raw[:len(raw) // 2])
put("bad_flip_data", raw[:200] + bytes([raw[200] ^ 1]) + raw[201:])
enc = open(os.path.join(out, "enc.kln"), "rb").read()
put("bad_enc_flip", enc[:300] + bytes([enc[300] ^ 1]) + enc[301:])
exp["bad"] = ["bad_truncated", "bad_flip_data", "bad_enc_flip"]
# block vectors: every coding mode (0,1,2 = position bits, +4 = literal mixer) on several kinds
# of data, plus a second block that matches into the first block's history.
bd = os.path.join(out, "blocks"); os.makedirs(bd, exist_ok=True)
vecs = []
def sample(name):
    d = lambda p: open(os.path.join(inp, p), "rb").read()
    return {"text": d("sub/text.txt")[:60000], "floats": d("sub/floats.bin")[:60000],
            "records": d("sub/records.bin")[:60000], "code": d("code.py")[:60000],
            "elf": d("ls_elf")[:60000], "repeat": (d("hello.txt") * 40)[:20000]}[name]
for name in ("text", "floats", "records", "code", "elf", "repeat"):
    data = sample(name)
    h = len(data) // 2
    for start, tag in ((0, "a"), (h, "b")):          # "b" has the first half as history
        buf = bytes(data)
        rnd = [1 if x == 1 else 0 for x in K.random_chunks(buf)]
        parts = K.lz77(buf, start, rnd, 0)
        tk = None
        for part in parts:
            if not isinstance(part, int): tk = part if tk is None else tk
        if tk is None: continue
        for lp in (0, 1, 2, 4, 5, 6):
            raw, payload = K.encode_block(tk, buf, start, lp)
            if payload is None: continue
            histo = bytearray(buf[:start]); n0 = len(histo)
            K.decode_block(payload, raw, histo)             # the real decoder must agree
            if bytes(histo[n0:]) != bytes(buf[start:start + raw]): raise SystemExit("engine roundtrip failed")
            base = f"{name}_{tag}_m{lp}"
            open(os.path.join(bd, base + ".payload"), "wb").write(payload)
            open(os.path.join(bd, base + ".hist"), "wb").write(bytes(buf[:start]))
            vecs.append({"name": base, "raw_len": raw, "flag": payload[0], "hist_len": start,
                         "crc": zlib.crc32(bytes(histo[n0:])), "plen": len(payload)})
exp["blocks"] = vecs
json.dump(exp, open(os.path.join(out, "expected.json"), "w"), indent=1)
shutil.rmtree(tmp)
print("corpus:", ", ".join(sorted(os.listdir(out))), "| blocks:", len(vecs), "flags:", sorted({v['flag'] for v in vecs}))
