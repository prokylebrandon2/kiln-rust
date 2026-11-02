"""usage: python3 test_rc.py <corpus_dir> [N workers]  - checks rc_mirror against block vectors."""
import json, os, sys, zlib
from multiprocessing import Pool
sys.path.insert(0, os.path.dirname(__file__))
import rc_mirror
corp = sys.argv[1]
def one(v):
    b = os.path.join(corp, "blocks", v["name"])
    pay = open(b + ".payload", "rb").read(); hist = bytearray(open(b + ".hist", "rb").read())
    n0 = len(hist)
    rc_mirror.decode_block(pay, v["raw_len"], hist)
    ok = zlib.crc32(bytes(hist[n0:])) == v["crc"] and len(hist) - n0 == v["raw_len"]
    return v["name"], v["flag"], ok
if __name__ == "__main__":
    vecs = json.load(open(os.path.join(corp, "expected.json")))["blocks"]
    with Pool(int(sys.argv[2]) if len(sys.argv) > 2 else 4) as p: res = p.map(one, vecs)
    bad = [r for r in res if not r[2]]
    print(f"{len(res) - len(bad)}/{len(res)} block vectors match; flags covered: {sorted({r[1] for r in res})}")
    for r in bad: print("FAIL", r)
    sys.exit(1 if bad else 0)
