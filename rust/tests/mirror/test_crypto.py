"""usage: test_crypto.py <kiln_v14.py> <corpus_dir> <part>
parts: store (enc_store.kln full test+bytes) | ckpt (ckpt_enc.kln partial extracts) | bad (bad_enc_flip, wrong password,
bad kdf header, truncated) | full (enc.kln, slow)"""
import importlib.util, json, os, sys, tempfile, argparse, zlib
sys.path.insert(0, os.path.dirname(__file__))
import container_mirror as M
spec = importlib.util.spec_from_file_location("kiln_v14", sys.argv[1]); K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
corp, part = sys.argv[2], sys.argv[3]; exp = json.load(open(os.path.join(corp, "expected.json"))); PW = exp["password"]; fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)
class Collect:
    def __init__(s): s.d = {}
    def begin(s, i, e): s.d[i] = bytearray()
    def write(s, i, b): s.d[i] += b
    def end(s, i, e): pass
def table_ok(name, a):
    ex = exp["archives"][name]
    rows = [dict(rel=e["rel"], size=e["size"], crc=e["crc"], mtime=e["mtime"], mode=e["mode"], filt=e["filt"], dead=e["dead"], slen=e["slen"]) for e in a.entries]
    return rows == ex["files"] and [list(c) for c in a.ckpts] == ex["ckpts"] and a.table_off == ex["table_off"] and a.hdr_len == ex["hdr_len"]
def real_err(p, pw):
    try:
        K.run_extract(argparse.Namespace(archive=p, output=tempfile.mkdtemp(), members=[], force=True, verbose=False, quiet=True, _pw=pw, _silent=True), False); return "ok"
    except K.KilnError as e: return str(e)
def mine_err(p, pw):
    try:
        with open(p, "rb") as f: a = M.Archive.open(f, pw); a.test()
        return "ok"
    except M.KilnError as e: return str(e)
P = lambda n: os.path.join(corp, n + ".kln")
if part in ("store", "full"):
    name = "enc_store" if part == "store" else "enc"
    with open(P(name), "rb") as f:
        a = M.Archive.open(f, PW); check(f"{name}: encrypted table == expected", a.cipher is not None and table_ok(name, a))
        c = Collect(); a.extract([True] * len(a.entries), False, c)
        with tempfile.TemporaryDirectory() as d:
            K.run_extract(argparse.Namespace(archive=P(name), output=d, members=[], force=True, verbose=False, quiet=True, _pw=PW, _silent=True), True)
            same = all(open(os.path.join(d, e["rel"]), "rb").read() == bytes(c.d[i]) for i, e in enumerate(a.entries))
        check(f"{name}: extracted bytes == real engine", same)
elif part == "ckpt":
    with open(P("ckpt_enc"), "rb") as f:
        a = M.Archive.open(f, PW); check("ckpt_enc: table + checkpoints == expected", table_ok("ckpt_enc", a))
        n = len(a.entries)
        for pick in ([n - 1], [3]):
            c = Collect(); a.extract([i in pick for i in range(n)], True, c)
            ok = set(c.d) == set(pick) and all(zlib.crc32(bytes(c.d[i])) == a.entries[i]["crc"] for i in pick)
            check(f"ckpt_enc: partial {pick} from file {a.last_start[0]} offset {a.last_start[1]} (counter used)", ok and a.last_start[0] > 0)
elif part == "bad":
    check("bad_enc_flip error text", mine_err(P("bad_enc_flip"), PW) == real_err(P("bad_enc_flip"), PW), mine_err(P("bad_enc_flip"), PW))
    check("wrong password error text", mine_err(P("enc_store"), "wrong-password-xx") == real_err(P("enc_store"), "wrong-password-xx"), mine_err(P("enc_store"), "wrong-password-xx"))
    raw = open(P("enc_store"), "rb").read()
    tests = {"kdf_params": raw[:5] + bytes([30]) + raw[6:], "short_header": raw[:12], "trunc_half": raw[:len(raw) // 2],
             "flip_footer": raw[:-3] + bytes([raw[-3] ^ 1]) + raw[-2:]}
    for nm, data in tests.items():
        p = os.path.join(tempfile.mkdtemp(), nm + ".kln"); open(p, "wb").write(data)
        check(f"{nm} error text", mine_err(p, PW) == real_err(p, PW), mine_err(p, PW))
sys.exit(1 if fails else 0)
