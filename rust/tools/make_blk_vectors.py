#!/usr/bin/env python3
"""Build tests/blkvec/ with the REAL Python build_blocks: for each input and history offset the
block list (type, raw length, payload). Used by kiln-rs/tests/blk_vectors.rs.
usage: make_blk_vectors.py <kiln_v14.py> <out_dir> [lzvec_dir]"""
import importlib.util, json, os, random, struct, sys
src, out = sys.argv[1:3]
lzdir = sys.argv[3] if len(sys.argv) > 3 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests", "lzvec")
spec = importlib.util.spec_from_file_location("kiln_v14", src)
K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
os.makedirs(out, exist_ok=True)
names = ["text", "code", "floats", "records", "rand_blob", "mixed", "alpha64", "noise_then_text", "tiny", "zeros", "empty1"]
reparse = {"n": 0}
orig = K.lz77
def counting(buf, start, rnd, lp=0):
    if lp: reparse["n"] += 1
    return orig(buf, start, rnd, lp)
K.lz77 = counting
meta = []
for name in names:
    buf = open(f"{lzdir}/{name}.buf", "rb").read()
    for start in ([0, 50000] if len(buf) > 60000 else [0]):
        reparse["n"] = 0
        blocks = K.build_blocks(buf, start)
        tag = f"{name}_s{start}"
        with open(f"{out}/{tag}.blk", "wb") as f:
            for t, raw, payload in blocks:
                pl = payload if t == 1 else b""
                f.write(struct.pack("<BQQ", t, raw, len(pl)) + pl)
        meta.append(dict(tag=tag, name=name, start=start, nblocks=len(blocks), reparsed=reparse["n"],
                         coded=sum(1 for b in blocks if b[0] == 1)))
        print(tag, len(blocks), "blocks, reparse calls", reparse["n"], flush=True)
json.dump(meta, open(f"{out}/meta.json", "w"))
