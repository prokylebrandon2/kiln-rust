"""usage: test_container.py <kiln_v14.py> <corpus_dir>
1. bcj mirror == real kz_bcj (decode, both filters, random + real ELF)
2. plain/ckpt archives: mirror table == expected.json, every file's bytes == real engine's, partial extracts
3. bad_* archives: mirror error text == real engine's error text"""
import importlib.util, json, os, random, sys, tempfile, argparse, zlib
sys.path.insert(0, os.path.dirname(__file__))
import container_mirror as M
spec = importlib.util.spec_from_file_location("kiln_v14", sys.argv[1]); K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
corp = sys.argv[2]; fails = 0
def check(name, ok, extra=""):
    global fails
    print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""))
    if not ok: fails += 1
# 1. BCJ
import ctypes as ct
rnd = random.Random(5)
elf = open("/bin/ls", "rb").read()[:300000]
def real_bcj(fid, data, pos0):
    b = ct.create_string_buffer(bytes(data), len(data)); K._A.kz_bcj(fid, b, len(data), pos0, 1); return b.raw[:len(data)]
words = b"".join((rnd.choice([0x94000000, 0x90000000, 0x10000000, rnd.getrandbits(32)]) | rnd.getrandbits(24)).to_bytes(4, "little") for _ in range(20000))
x86 = bytes(rnd.choice([0xE8, 0xE9, 0, 0xFF, rnd.randrange(256)]) for _ in range(60000))
for fid in (121, 122):
    for nm, data in (("elf", elf), ("words", words), ("x86like", x86), ("tiny", elf[:7]), ("empty", b"")):
        for pos0 in (0, 12345 * 4, (1 << 20) * 3):
            a = bytearray(data); M.bcj_decode(fid, a, pos0)
            if bytes(a) != real_bcj(fid, data, pos0): check(f"bcj {fid} {nm} pos0={pos0}", False)
check("bcj mirror == kz_bcj (decode)", True)
# streaming class across block boundary
big = (elf * 8)[:2500000]; b = M.Bcj(121); got = b"".join(b.feed(big[i:i + 777777]) for i in range(0, len(big), 777777)) + b.finish()
ref = b"".join(real_bcj(121, big[i:i + (1 << 20)], i) for i in range(0, len(big), 1 << 20))
check("Bcj streaming == per-block kz_bcj", got == ref)
# 2. archives
exp = json.load(open(os.path.join(corp, "expected.json")))
class Collect:
    def __init__(s): s.d = {}
    def begin(s, i, e): s.d[i] = bytearray()
    def write(s, i, b): s.d[i] += b
    def end(s, i, e): pass
for name in ("plain", "ckpt", "enc_store_skip"):
    if name not in exp["archives"]: continue
    ex = exp["archives"][name]
    with open(os.path.join(corp, name + ".kln"), "rb") as f:
        a = M.Archive.open(f)
        tab_ok = [dict(rel=e["rel"], size=e["size"], crc=e["crc"], mtime=e["mtime"], mode=e["mode"], filt=e["filt"], dead=e["dead"], slen=e["slen"]) for e in a.entries] == ex["files"]
        check(f"{name}: table == expected", tab_ok and [list(c) for c in a.ckpts] == ex["ckpts"] and a.table_off == ex["table_off"])
        a.test(); check(f"{name}: test() all CRCs", True)
        c = Collect(); a.extract([True] * len(a.entries), False, c)
        # compare with the real engine's extraction
        with tempfile.TemporaryDirectory() as d:
            K.run_extract(argparse.Namespace(archive=os.path.join(corp, name + ".kln"), output=d, members=[], force=True, verbose=False, quiet=True, _pw=None, _silent=True), True)
            same = all(open(os.path.join(d, e["rel"]), "rb").read() == bytes(c.d[i]) for i, e in enumerate(a.entries))
        check(f"{name}: extracted bytes == real engine", same)
        # partial extracts
        n = len(a.entries)
        for pick in ([n - 1], [3], [2, 4], [0]):
            wanted = [i in pick for i in range(n)]; c = Collect()
            a.extract(wanted, True, c)
            ok = all(bytes(c.d[i]).__len__() == a.entries[i]["size"] for i in pick) and set(c.d) == set(pick)
            check(f"{name}: partial {pick} starts at file {a.last_start[0]} offset {a.last_start[1]}", ok)
# 3. errors
for name in exp["bad"]:
    p = os.path.join(corp, name + ".kln")
    try:
        with open(p, "rb") as f: a = M.Archive.open(f); a.test()
        mine = "ok"
    except M.KilnError as e: mine = str(e)
    try:
        K.run_extract(argparse.Namespace(archive=p, output=tempfile.mkdtemp(), members=[], force=True, verbose=False, quiet=True, _pw=None, _silent=True), False)
        real = "ok"
    except K.KilnError as e: real = str(e)
    check(f"{name}: error text", mine == real, f"[{mine}]")
sys.exit(1 if fails else 0)
