"""usage: test_container_part2.py <kiln_v14.py> <corpus_dir> [ckpt|bad]
The two slower parts of test_container.py, runnable separately."""
import importlib.util, json, os, sys, tempfile, argparse
sys.path.insert(0, os.path.dirname(__file__))
import container_mirror as M
spec = importlib.util.spec_from_file_location("kiln_v14", sys.argv[1]); K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
corp = sys.argv[2]; part = sys.argv[3]; exp = json.load(open(os.path.join(corp, "expected.json"))); fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)
class Collect:
    def __init__(s): s.d = {}
    def begin(s, i, e): s.d[i] = bytearray()
    def write(s, i, b): s.d[i] += b
    def end(s, i, e): pass
if part == "ckpt":
    with open(os.path.join(corp, "ckpt.kln"), "rb") as f:
        a = M.Archive.open(f); n = len(a.entries)
        for pick in ([n - 1], [3]):
            c = Collect(); a.extract([i in pick for i in range(n)], True, c)
            ok = set(c.d) == set(pick) and all(len(c.d[i]) == a.entries[i]["size"] for i in pick)
            import zlib
            ok = ok and all(zlib.crc32(bytes(c.d[i])) == a.entries[i]["crc"] for i in pick)
            check(f"ckpt: partial {pick} from file {a.last_start[0]} (offset {a.last_start[1]})", ok and a.last_start[0] > 0 or pick == [3] and ok)
else:
    for name in exp["bad"]:
        if name.endswith("_enc_flip") and "--enc" not in sys.argv: continue   # encrypted: CP5
        p = os.path.join(corp, name + ".kln")
        try:
            with open(p, "rb") as f: a = M.Archive.open(f); a.test()
            mine = "ok"
        except M.KilnError as e: mine = str(e)
        try:
            K.run_extract(argparse.Namespace(archive=p, output=tempfile.mkdtemp(), members=[], force=True, verbose=False, quiet=True, _pw=None, _silent=True), False)
            real = "ok"
        except K.KilnError as e: real = str(e)
        check(f"{name}: error text", mine == real, f"[mine={mine}] [real={real}]")
sys.exit(1 if fails else 0)
