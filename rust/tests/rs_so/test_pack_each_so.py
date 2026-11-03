"""usage: test_pack_each_so.py <libkiln_rs.so> <python dir with kiln_app.py + kiln_v14.py>
Rust `pack` op in `each` mode (one archive per file) against the real engine:
 - plain: Rust output tree must have the same archive names as `kiln pack -i`, and every archive must extract
   (with the REAL kiln) to the source file;
 - encrypted: Rust set must open/extract with the real kiln_app (index + random names), rerun = "nothing to do",
   a changed file with -f rebuilds only that archive, a wrong password is refused.
Also: the real engine's set must read back through the Rust op `open`."""
import contextlib, ctypes, io, json, os, shutil, sys, tempfile
so, pydir = sys.argv[1:3]
sys.path[:0] = [pydir]
import kiln_app as R
K = sys.modules["kiln_v14"]
PW = "correct-horse-battery"; fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)
lib = ctypes.CDLL(os.path.abspath(so)); lib.kiln_rs_call.restype = ctypes.c_void_p
CB = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p)
def rs(op, **a):
    p = lib.kiln_rs_call(op.encode(), json.dumps(a).encode(), CB(lambda e, u: 0), None)
    s = ctypes.string_at(p).decode(); lib.kiln_rs_free(ctypes.c_void_p(p)); return json.loads(s)
def kiln(args, pw=PW):
    saved = (sys.argv, K.get_password); sys.argv = ["kiln"] + args; K.get_password = lambda confirm=False: pw
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            try: K.main()
            except SystemExit as e:
                if e.code not in (None, 0): raise
    finally: sys.argv, K.get_password = saved
def names(root): return sorted(os.path.relpath(os.path.join(d, f), root) for d, _, fs in os.walk(root) for f in fs)

work = tempfile.mkdtemp(); src = os.path.join(work, "in"); os.makedirs(os.path.join(src, "sub"))
open(os.path.join(src, "a.txt"), "wb").write(b"hello " * 500)
open(os.path.join(src, "sub", "b.bin"), "wb").write(bytes(i * 7 % 251 for i in range(5000)))
open(os.path.join(src, "sub", "é ü.txt"), "wb").write("naïve ✓ ".encode() * 40)
items = [src]

# ---- plain
out_rs, out_py = os.path.join(work, "rs"), os.path.join(work, "py")
r = rs("pack", items=items, dest=out_rs, encrypt=False, store=False, overwrite=False, password="", each=True, jobs=1)
kiln(["pack", "-q", "-i", "-f", out_py, src])
check("plain: ok", r.get("ok") is True, str(r))
check("plain: same archive names as the real engine", names(out_rs) == names(out_py), f"{names(out_rs)} vs {names(out_py)}")
ok = True
for n in names(out_rs):
    dst = os.path.join(work, "x_" + n.replace(os.sep, "_")); kiln(["unpack", "-q", "-o", dst, os.path.join(out_rs, n)])
    orig = os.path.join(work, n[:-4]); got = os.path.join(dst, os.path.basename(orig))
    ok &= os.path.exists(got) and open(got, "rb").read() == open(orig, "rb").read()
check("plain: every Rust archive extracts (real engine) to the source bytes", ok)
r2 = rs("pack", items=items, dest=out_rs, encrypt=False, store=False, overwrite=False, password="", each=True, jobs=1)
check("plain: rerun says nothing to do", "nothing to do" in r2.get("message", ""), str(r2))

# ---- encrypted set
s_rs = os.path.join(work, "set_rs")
r = rs("pack", items=items, dest=s_rs, encrypt=True, store=False, overwrite=False, password=PW, each=True, jobs=1)
check("enc: ok, 3 archives", r.get("ok") is True and r.get("done") == 3, str(r))
o = R.open_archive(s_rs, PW); od = json.loads(o)
check("enc: real kiln_app opens the Rust set", od.get("ok") is True and len(od.get("entries", [])) == 3, o[:200])
dst = os.path.join(work, "set_out"); e = json.loads(R.extract(s_rs, PW, "[]", dst, False, 3, None))
check("enc: real kiln_app extracts it", e.get("ok") is True, str(e))
got = {os.path.relpath(os.path.join(d, f), dst): open(os.path.join(d, f), "rb").read() for d, _, fs in os.walk(dst) for f in fs}
want = {os.path.relpath(os.path.join(d, f), work): open(os.path.join(d, f), "rb").read() for d, _, fs in os.walk(src) for f in fs}
check("enc: extracted tree == source", got == {k: v for k, v in want.items()} or {os.path.basename(k): v for k, v in got.items()} == {os.path.basename(k): v for k, v in want.items()}, f"{sorted(got)} vs {sorted(want)}")
r2 = rs("pack", items=items, dest=s_rs, encrypt=True, store=False, overwrite=False, password=PW, each=True, jobs=1)
check("enc: rerun says nothing to do", "nothing to do" in r2.get("message", ""), str(r2))
names_before = sorted(n for n in os.listdir(s_rs))
open(os.path.join(src, "a.txt"), "ab").write(b"changed")
r3 = rs("pack", items=items, dest=s_rs, encrypt=True, store=False, overwrite=True, password=PW, each=True, jobs=1)
check("enc: -f rebuilds with the same names", r3.get("ok") is True and sorted(os.listdir(s_rs)) == names_before, str(r3))
bad = rs("pack", items=items, dest=s_rs, encrypt=True, store=False, overwrite=True, password="another-long-password", each=True, jobs=1)
check("enc: wrong password refused", bad.get("ok") is False and bad.get("bad_password") is True, str(bad))
# a set made by the real engine reads back through the Rust op
s_py = os.path.join(work, "set_py"); kiln(["pack", "-q", "-i", "-e", "-f", s_py, src])
ro = rs("open", path=s_py, password=PW)
check("real set opens through Rust", ro.get("ok") is True and len(ro.get("entries", [])) == 3, str(ro)[:200])
shutil.rmtree(work, ignore_errors=True)
print("FAILED" if fails else "ALL PASS"); sys.exit(1 if fails else 0)
