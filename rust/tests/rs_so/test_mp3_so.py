"""usage: test_mp3_so.py <libkiln_rs.so> <python dir with kiln_app.py + kiln_v14.py> <mp3 corpus dir (make_mp3_corpus.py)>
Real archives and a real set containing MP3 files (filter 120): the compiled Rust engine against the real kiln_app
(JSON results + extracted trees) and against the source files."""
import contextlib, io, json, os, random, re, shutil, stat, sys, tempfile
so, pydir, mp3dir = sys.argv[1:4]
here = os.path.dirname(os.path.abspath(__file__))
os.environ["KILN_RS_SO"] = os.path.abspath(so); os.environ["KILN_MIRROR_DIR"] = os.path.join(os.path.dirname(here), "mirror")
sys.path[:0] = [pydir, here]
import kiln_app as R
import app_mirror as A
K = sys.modules["kiln_v14"]
PW = "correct-horse-battery"; fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)
def norm(js):
    d = json.loads(js)
    if "message" in d: d["message"] = re.sub(r" in \d+\.\ds ", " in Ns ", d["message"])
    return d
def tree(root):
    out = {}
    for dp, dn, fn in os.walk(root):
        for d in dn: out[os.path.relpath(os.path.join(dp, d), root) + "/"] = None
        for f in fn:
            p = os.path.join(dp, f); st = os.lstat(p)
            out[os.path.relpath(p, root)] = (open(p, "rb").read(), int(st.st_mtime), stat.S_IMODE(st.st_mode))
    return out
def kiln(args, pw=PW):
    saved = (sys.argv, K.get_password); sys.argv = ["kiln"] + args; K.get_password = lambda confirm=False: pw
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            try: K.main()
            except SystemExit as e:
                if e.code not in (None, 0): raise
    finally: sys.argv, K.get_password = saved
def both(real_fn, rs_fn, name, dirs=None, expect_ok=None):
    real, mine = norm(real_fn()), norm(rs_fn())
    ok = real == mine and not mine.get("unsupported")
    if ok and dirs: ok = tree(dirs[0]) == tree(dirs[1])
    if ok and expect_ok is not None: ok = mine.get("ok") is expect_ok
    check(name, ok, "" if ok else f"\n  real={real}\n  rust={mine}")
    return mine

work = tempfile.mkdtemp(); src = os.path.join(work, "in"); os.makedirs(os.path.join(src, "sub"))
mp3s = ["music_cbr128", "music_mono64", "trail_junk", "shine_mono", "concat_two", "crc_flag", "music_tags"]
for i, n in enumerate(mp3s):
    shutil.copy(os.path.join(mp3dir, n + ".mp3"), os.path.join(src, "sub" if i % 2 else ".", n + ".mp3"))
open(os.path.join(src, "a.txt"), "wb").write(b"hello" * 300)
srcfiles = {os.path.relpath(os.path.join(dp, f), work): open(os.path.join(dp, f), "rb").read() for dp, _, fn in os.walk(src) for f in fn}

plain, enc, store = (os.path.join(work, x) for x in ("plain.kln", "enc.kln", "store.kln"))
kiln(["pack", "-q", "-f", plain, src]); kiln(["pack", "-q", "-e", "-f", enc, src]); kiln(["pack", "-q", "-e", "-s", "-f", store, src])
setdir = os.path.join(work, "set"); kiln(["pack", "-q", "-i", "-e", "-f", setdir, src])
for label, p, pw in (("plain", plain, ""), ("enc", enc, PW)):
    f, table, *_ = K.open_archive(p, pw or None); f.close()
    n120 = sum(1 for e in table if e["filt"] == 120)
    check(f"{label}: archive really holds MP3-filter entries ({n120})", n120 >= 4)

for label, p, pw in (("plain", plain, ""), ("enc", enc, PW)):
    both(lambda: R.open_archive(p, pw), lambda: A.open_archive(p, pw), f"{label}: open")
    both(lambda: R.test(p, pw, None), lambda: A.test(p, pw, A.NoProgress()), f"{label}: test", expect_ok=True)
    names = [e["p"] for e in json.loads(R.open_archive(p, pw))["entries"]]
    for what, mem in (("all", []), ("one mp3", [{"p": [n for n in names if n.endswith("trail_junk.mp3")][0]}]),
                      ("two members", [{"p": [n for n in names if n.endswith("a.txt")][0]}, {"p": [n for n in names if n.endswith("crc_flag.mp3")][0]}])):
        d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
        both(lambda: R.extract(p, pw, json.dumps(mem), d1, False, 0, None), lambda: A.extract(p, pw, json.dumps(mem), d2, False, A.NoProgress()),
             f"{label}: extract {what}", (d1, d2), expect_ok=True)
        got = {k: v[0] for k, v in tree(d2).items() if v}
        check(f"{label}: extract {what}: bytes == source files", bool(got) and all(srcfiles.get(k) == v for k, v in got.items()))
store_only = norm(A.test(store, PW, A.NoProgress())); check("enc store (no MP3 filter): test ok", store_only.get("ok") is True)

# the set (random names + encrypted index): open / test / extract
both(lambda: R.open_archive(setdir, PW), lambda: A.open_archive(setdir, PW), "set: open")
both(lambda: R.test(setdir, PW, None), lambda: A.test(setdir, PW, A.NoProgress()), "set: test", expect_ok=True)
d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
both(lambda: R.extract(setdir, PW, "[]", d1, False, 0, None), lambda: A.extract(setdir, PW, "[]", d2, False, A.NoProgress()), "set: extract all", (d1, d2), expect_ok=True)
got = {k: v[0] for k, v in tree(d2).items() if v}
check("set: extract all: every source file, byte-identical", sorted(os.path.basename(k) for k in got) == sorted(os.path.basename(k) for k in srcfiles)
      and all(srcfiles["in/" + k.split("in/", 1)[-1]] == v for k, v in got.items() if "in/" + k.split("in/", 1)[-1] in srcfiles))
mems = [e["p"] for e in json.loads(R.open_archive(setdir, PW))["entries"]]
d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp(); mem = [{"p": [m for m in mems if m.endswith("music_mono64.mp3")][0]}]
both(lambda: R.extract(setdir, PW, json.dumps(mem), d1, False, 0, None), lambda: A.extract(setdir, PW, json.dumps(mem), d2, False, A.NoProgress()), "set: extract one mp3", (d1, d2), expect_ok=True)

# damaged archives: same verdict and message as the real engine
rnd = random.Random(3); bad = 0; same = 0
raw = open(plain, "rb").read()
for k in range(24):
    b = bytearray(raw); pos = rnd.randrange(len(b) // 8, len(b) - 24); b[pos] ^= 1 << rnd.randrange(8)
    q = os.path.join(work, f"dmg{k}.kln"); open(q, "wb").write(b)
    r, m = norm(R.test(q, "", None)), norm(A.test(q, "", A.NoProgress()))
    same += r == m; bad += r != m
    if r != m: print("  differs:", k, pos, "\n   real:", r, "\n   rust:", m)
check(f"24 damaged plain archives: Rust verdict + message == real engine ({same} equal)", bad == 0)
print("FAILS:", fails); sys.exit(1 if fails else 0)
