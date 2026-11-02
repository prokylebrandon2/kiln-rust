"""usage: test_app.py <python dir with kiln_app.py + kiln_v14.py> <corpus_dir> [open|test|extract|fnmatch|cancel|unsupported|sets|sets2|sets3|sets4|sets5|all]
Compares app_mirror (mirror of app.rs) with the REAL kiln_app on the corpus: parsed JSON results and the
extracted trees (bytes, mtime, mode)."""
import json, os, random, re, shutil, stat, sys, tempfile, fnmatch as pyfn
sys.path.insert(0, sys.argv[1]); sys.path.insert(0, os.path.dirname(__file__))
import kiln_app as R
import app_mirror as A
corp = sys.argv[2]; part = sys.argv[3] if len(sys.argv) > 3 else "all"
exp = json.load(open(os.path.join(corp, "expected.json"))); PW = exp["password"]
fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)
arc = lambda n: os.path.join(corp, n + ".kln")
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
class Cnt:
    def __init__(s, stop_at=None): s.n = 0; s.stop_at = stop_at; s.polls = 0
    def event(s, j): s.n += 1
    def cancelled(s): s.polls += 1; return s.stop_at is not None and s.polls > s.stop_at
def both(fn_real, fn_mine, name, tree_dirs=None):
    real = norm(fn_real()); mine = norm(fn_mine())
    ok = real == mine
    if ok and tree_dirs: ok = tree(tree_dirs[0]) == tree(tree_dirs[1])
    check(name, ok, "" if ok else f"\n  real={real}\n  mine={mine}")
    return real

if part in ("open", "all"):
    for n, pw in [("plain", ""), ("ckpt", ""), ("enc_store", PW), ("enc_store", ""), ("enc_store", "wrong-pass-word"),
                  ("ckpt_enc", PW), ("bad_truncated", ""), ("bad_flip_data", ""), ("bad_enc_flip", PW)]:
        both(lambda: R.open_archive(arc(n), pw), lambda: A.open_archive(arc(n), pw), f"open {n} pw={'-' if not pw else pw[:5]}")
    tmp = tempfile.mkdtemp()
    for name, data in [("text.kln", b"hello world, this is not an archive at all\n"), ("tiny.kln", b"KLNA\0"), ("old.kln", b"KLN9" + b"x" * 40),
                       ("flag.kln", b"KLNA\x80" + b"x" * 40), ("empty.kln", b"")]:
        p = os.path.join(tmp, name); open(p, "wb").write(data)
        both(lambda: R.open_archive(p, ""), lambda: A.open_archive(p, ""), f"open {name}")

if part in ("test", "all"):
    for n, pw in [("plain", ""), ("ckpt", ""), ("enc_store", PW), ("enc_store", "wrong-pass-word"), ("enc_store", ""),
                  ("bad_truncated", ""), ("bad_flip_data", ""), ("bad_enc_flip", PW)]:
        both(lambda: R.test(arc(n), pw, None), lambda: A.test(arc(n), pw, A.NoProgress()), f"test {n} pw={'-' if not pw else pw[:5]}")

def ext_case(name, n, pw, members, overwrite=False, pre=None):
    d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
    if pre:
        for d in (d1, d2): pre(d)
    both(lambda: R.extract(arc(n), pw, json.dumps(members), d1, overwrite, 0, None),
         lambda: A.extract(arc(n), pw, json.dumps(members), d2, overwrite, A.NoProgress()), name, (d1, d2))

if part in ("extract", "all"):
    names = [e["rel"] for e in json.load(open(os.path.join(corp, "expected.json")))["tables"]["plain"]] if "tables" in exp else None
    t = json.loads(R.open_archive(arc("plain"), ""))["entries"]; names = [e["p"] for e in t]
    print("members:", names, flush=True)
    ext_case("extract plain all", "plain", "", [])
    ext_case("extract plain one file", "plain", "", [{"p": "in/hello.txt"}])
    ext_case("extract plain dir sub", "plain", "", [{"p": "in/sub", "dir": True}])
    ext_case("extract plain 2 members", "plain", "", [{"p": "in/hello.txt"}, {"p": "in/ls_elf"}])
    ext_case("extract plain empty file", "plain", "", [{"p": "in/empty.txt"}])
    ext_case("extract plain no match", "plain", "", [{"p": "nope.txt"}])
    ext_case("extract plain glob chars escaped", "plain", "", [{"p": "*"}])
    ext_case("extract plain dir 'in' (everything)", "plain", "", [{"p": "in", "dir": True}])
    ext_case("extract plain unsafe-looking member", "plain", "", [{"p": "../in/hello.txt"}])
    def pre(d):
        os.makedirs(os.path.join(d, "in"), exist_ok=True); open(os.path.join(d, "in", "hello.txt"), "wb").write(b"old")
    ext_case("extract plain clash no overwrite", "plain", "", [], False, pre)
    def pre_many(d):
        for r in names: os.makedirs(os.path.dirname(os.path.join(d, r)), exist_ok=True); open(os.path.join(d, r), "wb").write(b"old")
    ext_case("extract plain clash all 8", "plain", "", [], False, pre_many)
    ext_case("extract plain clash overwrite", "plain", "", [], True, pre)
    ext_case("extract ckpt all", "ckpt", "", [])
    ext_case("extract ckpt last member (checkpoint start)", "ckpt", "", [{"p": names[-1]}])
    ext_case("extract ckpt middle member", "ckpt", "", [{"p": names[3]}])
    ext_case("extract bad_flip_data", "bad_flip_data", "", [])
    ext_case("extract bad_truncated", "bad_truncated", "", [])
    ext_case("extract enc_store all", "enc_store", PW, [])
    ext_case("extract enc_store wrong pw", "enc_store", "wrong-pass-word", [])
    ext_case("extract enc_store no pw", "enc_store", "", [])
    ext_case("extract ckpt_enc partial", "ckpt_enc", PW, [{"p": names[-1]}])
    ext_case("extract ckpt_enc all", "ckpt_enc", PW, [])
    ext_case("extract bad_enc_flip", "bad_enc_flip", PW, [])

if part in ("unsupported", "all"):
    import importlib, io, contextlib, math, struct
    K = sys.modules["kiln_v14"]
    tmp = tempfile.mkdtemp(); inp = os.path.join(tmp, "w"); os.makedirs(inp)
    n = 40000; hdr = b"RIFF" + struct.pack("<I", 36 + n * 4) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 2, 22050, 22050 * 4, 4, 16) + b"data" + struct.pack("<I", n * 4)
    open(os.path.join(inp, "t.wav"), "wb").write(hdr + b"".join(struct.pack("<hh", int(9000 * math.sin(i / 9)), int(7000 * math.sin(i / 13))) for i in range(n)))
    open(os.path.join(inp, "a.txt"), "wb").write(b"hello" * 100)
    wa = os.path.join(tmp, "wav.kln"); sys.argv = ["kiln", "pack", wa, inp, "-q", "-f"]
    with contextlib.redirect_stdout(io.StringIO()):
        try: K.main()
        except SystemExit: pass
    f, table, *_ = K.open_archive(wa, None); f.close()
    filts = sorted({e["filt"] for e in table}); print("filters in wav.kln:", filts, flush=True)
    check("wav archive really uses a WAV/MP3 filter", any(x not in (0, 121, 122) for x in filts))
    # WAV filters are supported by the Rust engine since CP8a: results must equal the real app's
    names_w = [e["p"] for e in json.loads(R.open_archive(wa, ""))["entries"]]; print("wav.kln members:", names_w, flush=True)
    wavname = [n_ for n_ in names_w if n_.endswith("t.wav")][0]; src_bytes = open(os.path.join(inp, "t.wav"), "rb").read()
    both(lambda: R.open_archive(wa, ""), lambda: A.open_archive(wa, ""), "wav archive: open")
    both(lambda: R.test(wa, "", None), lambda: A.test(wa, "", A.NoProgress()), "wav archive: test")
    for label, mem in (("all", []), ("single wav member", [{"p": wavname}]), ("other member", [{"p": [n_ for n_ in names_w if n_.endswith("a.txt")][0]}])):
        d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
        r = both(lambda: R.extract(wa, "", json.dumps(mem), d1, False, 0, None), lambda: A.extract(wa, "", json.dumps(mem), d2, False, A.NoProgress()), f"wav archive: extract {label}", (d1, d2))
        check(f"wav archive: extract {label}: not 'unsupported'", not r.get("unsupported") and r["ok"] is True, str(r))
        if label != "other member":
            got = open(os.path.join(d2, wavname), "rb").read()
            check(f"wav archive: extract {label}: bytes == source wav", got == src_bytes)
    # an MP3-filter entry is still refused before anything is written
    import copy; K2 = sys.modules["kiln_v14"]
    mp3e = dict(rel="a.mp3", size=100, crc=0, mtime=0, mode=0o644, filt=120, dead=False, slen=50)
    try: A.M.make_unfilter(mp3e); mp3_ok = False
    except A.M.KilnError as x: mp3_ok = "MP3" in str(x)
    check("MP3 filter entry -> unsupported (mirror make_unfilter)", mp3_ok)

if part.startswith("sets") or part == "all":
    import io, contextlib, subprocess
    K = sys.modules["kiln_v14"]
    SPW = "correct-horse-battery"
    def kiln_cmd(args, pw=SPW):
        saved = (sys.argv, K.get_password); sys.argv = ["kiln"] + args; K.get_password = lambda confirm=False: pw
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                try: K.main()
                except SystemExit as e:
                    if e.code not in (None, 0): raise
        finally: sys.argv, K.get_password = saved
    base = "/tmp/kiln_sets_test"; os.makedirs(base, exist_ok=True); src = os.path.join(base, "in"); rnd = random.Random(5)
    files = {"a.txt": b"hello" * 200, "sub/b.bin": bytes(rnd.getrandbits(8) for _ in range(5000)), "deep/er/c.txt": b"x\n",
             "empty.txt": b"", "d\u00e9 \u00fc.txt": b"unicode name\n" * 30, "x[1].txt": b"glob chars\n" * 20}
    for i in range(13): files[f"many/f{i:02d}.txt"] = (b"line %d\n" % i) * 50
    for rel, data in files.items():
        p = os.path.join(src, rel)
        if not os.path.isfile(p): os.makedirs(os.path.dirname(p), exist_ok=True); open(p, "wb").write(data)
    def build(name, pw=SPW):
        out = os.path.join(base, name)
        if not os.path.isdir(out): kiln_cmd(["pack", "-i", "-e", "-f", out, src], pw)      # cached between parts
        return out
    def inner(setdir): return sorted(f for f in os.listdir(setdir) if f != "_index.kln")
    def clone(setdir, name):
        out = os.path.join(base, "tmp_" + name); shutil.rmtree(out, ignore_errors=True); shutil.copytree(setdir, out); return out
    def put_index(setdir, content, pw=SPW, extra=None):
        t = tempfile.mkdtemp(); p = os.path.join(t, "index.json"); open(p, "wb").write(content if isinstance(content, bytes) else json.dumps(content).encode())
        ins = [p]
        for n, data in (extra or {}).items(): q = os.path.join(t, n); open(q, "wb").write(data); ins.append(q)
        os.remove(os.path.join(setdir, "_index.kln")); kiln_cmd(["pack", "-e", "-f", os.path.join(setdir, "_index.kln")] + ins, pw)
    def index_of(setdir):
        d = tempfile.mkdtemp(); kiln_cmd(["unpack", "-f", "-o", d, os.path.join(setdir, "_index.kln")]); return json.load(open(os.path.join(d, "index.json")))
    good = build("good")
    names_all = [e["p"] for e in json.loads(R.open_archive(good, SPW))["entries"]]
    check("set has all files", sorted(names_all) == sorted("in/" + k for k in files), str(names_all))
    def sext(name, p, members, pw=SPW, overwrite=False, pre=None):
        d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
        if pre:
            for d in (d1, d2): pre(d)
        return both(lambda: R.extract(p, pw, json.dumps(members), d1, overwrite, 0, None), lambda: A.extract(p, pw, json.dumps(members), d2, overwrite, A.NoProgress()), name, (d1, d2)), d2
    def pre_clash(d):
        for rel in list(files)[:2] + ["many/f00.txt"]:
            p = os.path.join(d, "in", rel); os.makedirs(os.path.dirname(p), exist_ok=True); open(p, "wb").write(b"old")
    def pre_many(d):
        for rel in files:
            p = os.path.join(d, "in", rel); os.makedirs(os.path.dirname(p), exist_ok=True); open(p, "wb").write(b"old")
    want = {"in/" + k: v for k, v in files.items()}
    idx = None
    if part in ("sets3", "sets4", "all"): idx = index_of(good)
    if part in ("sets", "all"):
        # ---- open
        for label, p, pw in [("ok", good, SPW), ("no password", good, ""), ("wrong password", good, "wrong-pass-word")]:
            both(lambda: R.open_archive(p, pw), lambda: A.open_archive(p, pw), f"set open {label}")
        plain_dir = tempfile.mkdtemp(); open(os.path.join(plain_dir, "x"), "wb").write(b"1")
        both(lambda: R.open_archive(plain_dir, SPW), lambda: A.open_archive(plain_dir, SPW), "set open: folder without index")
        both(lambda: R.test(plain_dir, SPW, None), lambda: A.test(plain_dir, SPW, A.NoProgress()), "set test: folder without index")
        both(lambda: R.extract(plain_dir, SPW, "[]", tempfile.mkdtemp(), False, 0, None), lambda: A.extract(plain_dir, SPW, "[]", tempfile.mkdtemp(), False, A.NoProgress()), "set extract: folder without index")
        # ---- test
        for label, p, pw in [("ok", good, SPW), ("no password", good, ""), ("wrong password", good, "wrong-pass-word")]:
            both(lambda: R.test(p, pw, None), lambda: A.test(p, pw, A.NoProgress()), f"set test {label}")
        # ---- extract
        r, d2 = sext("set extract all", good, [])
        t = tree(d2); want = {"in/" + k: v for k, v in files.items()}
        check("set extract all: bytes == source", all(t.get(k) and t[k][0] == v for k, v in want.items()) and not [k for k in t if ".kiln-" in k])
        sext("set extract one", good, [{"p": "in/a.txt"}])
        sext("set extract dir", good, [{"p": "in/deep", "dir": True}])
        sext("set extract dir many", good, [{"p": "in/many", "dir": True}])
        sext("set extract unicode name", good, [{"p": "in/d\u00e9 \u00fc.txt"}])
        sext("set extract glob chars name", good, [{"p": "in/x[1].txt"}])
        sext("set extract two members", good, [{"p": "in/a.txt"}, {"p": "in/empty.txt"}])
        sext("set extract empty file", good, [{"p": "in/empty.txt"}])
        sext("set extract no match", good, [{"p": "zzz"}])
        sext("set extract no password", good, [], "")
        sext("set extract wrong password", good, [], "wrong-pass-word")
        sext("set extract clash (3), no overwrite", good, [], pre=pre_clash)
        sext("set extract clash (3), overwrite", good, [], overwrite=True, pre=pre_clash)
        sext("set extract clash (19: '... and N more'), no overwrite", good, [], pre=pre_many)
        sext("set extract clash (19), overwrite", good, [], overwrite=True, pre=pre_many)
        both(lambda: R.extract(good, SPW, "[]", os.path.join(tempfile.mkdtemp(), "new", "deeper"), False, 0, None),
             lambda: A.extract(good, SPW, "[]", os.path.join(tempfile.mkdtemp(), "new", "deeper"), False, A.NoProgress()), "set extract into a folder that doesn't exist yet")
    if part in ("sets2", "all"):
        # ---- a missing archive
        miss = clone(good, "missing"); os.remove(os.path.join(miss, inner(miss)[3]))
        both(lambda: R.test(miss, SPW, None), lambda: A.test(miss, SPW, A.NoProgress()), "set test: one archive missing")
        sext("set extract: one archive missing", miss, [])
        miss2 = clone(good, "missing2")
        for n in inner(miss2)[:12]: os.remove(os.path.join(miss2, n))
        both(lambda: R.test(miss2, SPW, None), lambda: A.test(miss2, SPW, A.NoProgress()), "set test: 12 archives missing")
        sext("set extract: 12 archives missing", miss2, [])
        # ---- a damaged archive
        for label, mode in (("flipped byte", "flip"), ("truncated", "cut"), ("garbage", "junk")):
            bad = clone(good, "bad_" + mode); victim = os.path.join(bad, inner(bad)[2]); raw = bytearray(open(victim, "rb").read())
            if mode == "flip": raw[len(raw) // 2] ^= 0x55
            elif mode == "cut": raw = raw[:len(raw) // 2]
            else: raw = bytearray(b"not an archive at all, just text\n" * 3)
            open(victim, "wb").write(bytes(raw))
            both(lambda: R.test(bad, SPW, None), lambda: A.test(bad, SPW, A.NoProgress()), f"set test: damaged archive ({label})")
            sext(f"set extract: damaged archive ({label})", bad, [])
    if part in ("sets3", "all"):
        # ---- index variants
        v1 = clone(good, "v1"); put_index(v1, {"v": 1, "files": {n: m["path"] for n, m in idx["files"].items()}})
        for op in ("open", "test"): pass
        both(lambda: R.open_archive(v1, SPW), lambda: A.open_archive(v1, SPW), "set open: v1 index (size/mtime null)")
        both(lambda: R.test(v1, SPW, None), lambda: A.test(v1, SPW, A.NoProgress()), "set test: v1 index")
        sext("set extract: v1 index", v1, [])
        ren = clone(good, "renamed"); idx2 = json.loads(json.dumps(idx)); nm = sorted(idx2["files"])[0]; idx2["files"][nm]["path"] = "moved/here/new name.txt"; put_index(ren, idx2)
        both(lambda: R.open_archive(ren, SPW), lambda: A.open_archive(ren, SPW), "set open: entry renamed in the index")
        sext("set extract: entry renamed in the index", ren, [])
        for label, content in [("bad layout (v3)", {"v": 3, "files": {}}), ("bad layout (no files)", {"v": 2}), ("bad layout (entry not a dict)", {"v": 2, "files": {"a": 5}}),
                               ("bad layout (size is a string)", {"v": 2, "files": {"a": {"path": "p", "size": "1", "mtime": 0}}}), ("bad layout (size is a float)", {"v": 2, "files": {"a": {"path": "p", "size": 1.5, "mtime": 0}}}),
                               ("bad layout (list)", [1, 2]), ("not json", b"this is not json"), ("not utf-8", b"\xff\xfe\x00{"), ("empty index.json", b"")]:
            c = clone(good, "ix"); put_index(c, content)
            both(lambda: R.open_archive(c, SPW), lambda: A.open_archive(c, SPW), f"set open: index {label}")
            both(lambda: R.test(c, SPW, None), lambda: A.test(c, SPW, A.NoProgress()), f"set test: index {label}")
            sext(f"set extract: index {label}", c, [])
        c = clone(good, "ix_other"); put_index(c, b"{}", extra={"other.txt": b"x"})
        both(lambda: R.open_archive(c, SPW), lambda: A.open_archive(c, SPW), "set open: index archive without index.json")
        c = clone(good, "ix_junk"); open(os.path.join(c, "_index.kln"), "wb").write(b"x" * 100)
        both(lambda: R.open_archive(c, SPW), lambda: A.open_archive(c, SPW), "set open: _index.kln is junk")
        both(lambda: R.test(c, SPW, None), lambda: A.test(c, SPW, A.NoProgress()), "set test: _index.kln is junk")
        c = clone(good, "ix_cut"); raw = open(os.path.join(c, "_index.kln"), "rb").read(); open(os.path.join(c, "_index.kln"), "wb").write(raw[:len(raw) // 2])
        both(lambda: R.open_archive(c, SPW), lambda: A.open_archive(c, SPW), "set open: _index.kln truncated")
        both(lambda: R.test(c, SPW, None), lambda: A.test(c, SPW, A.NoProgress()), "set test: _index.kln truncated")
        c = clone(good, "ix_flip"); raw = bytearray(open(os.path.join(c, "_index.kln"), "rb").read()); raw[len(raw) // 2] ^= 1; open(os.path.join(c, "_index.kln"), "wb").write(bytes(raw))
        both(lambda: R.open_archive(c, SPW), lambda: A.open_archive(c, SPW), "set open: _index.kln has a flipped byte")
        both(lambda: R.test(c, SPW, None), lambda: A.test(c, SPW, A.NoProgress()), "set test: _index.kln has a flipped byte")
    if part in ("sets4", "all"):
        # unsafe / odd paths in the index
        for label, path in [("../evil.txt", "../evil.txt"), ("a/../../evil", "a/../../evil"), ("/abs/path.txt", "/abs/path.txt"), ("trailing slash", "dir/"), ("dot", "a/."), ("empty", ""), ("dotdot", "..")]:
            c = clone(good, "unsafe"); i3 = json.loads(json.dumps(idx)); nm = sorted(i3["files"])[0]; i3["files"][nm]["path"] = path; put_index(c, i3)
            both(lambda: R.test(c, SPW, None), lambda: A.test(c, SPW, A.NoProgress()), f"set test: index path {label!r}")
            sext(f"set extract: index path {label!r}", c, [])
        # the index lists the same archive twice, in odd order
        c = clone(good, "dup"); i4 = json.loads(json.dumps(idx)); ns = sorted(i4["files"]); i4["files"][ns[1]]["path"] = i4["files"][ns[0]]["path"]; put_index(c, i4)
        both(lambda: R.test(c, SPW, None), lambda: A.test(c, SPW, A.NoProgress()), "set test: two entries with one path")
        sext("set extract: two entries with one path (second one clashes)", c, [])
        sext("set extract: two entries with one path, overwrite", c, [], overwrite=True)
        # other password for the whole set
        other = build("otherpw", "another-long-passphrase")
        both(lambda: R.test(other, "another-long-passphrase", None), lambda: A.test(other, "another-long-passphrase", A.NoProgress()), "set test: other password")
    if part in ("sets5", "all"):
        # ---- cancel: result + no temp leftovers + every file that is there is complete
        class RP2:
            def __init__(s, stop_at): s.n = 0; s.stop_at = stop_at
            def event(s, j): s.n += 1
            def cancelled(s): return s.n >= s.stop_at
        for stop in (1, 3, 6, 12):
            d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
            real = norm(R.extract(good, SPW, "[]", d1, False, 0, RP2(stop))); mine = norm(A.extract(good, SPW, "[]", d2, False, Cnt(stop_at=stop)))
            t1, t2 = tree(d1), tree(d2)
            ok_t = lambda t: not [k for k in t if ".kiln-" in k] and all(v is None or v[0] == want.get(k) for k, v in t.items())
            check(f"set extract: cancel after {stop}: result", real.get("cancelled") and mine.get("cancelled"), f"real={real} mine={mine}")
            check(f"set extract: cancel after {stop}: no temp folders, finished files complete", ok_t(t1) and ok_t(t2), f"real={list(t1)} mine={list(t2)}")
        real = norm(R.test(good, SPW, RP2(2))); mine = norm(A.test(good, SPW, Cnt(stop_at=2)))
        check("set test: cancel", real.get("cancelled") and mine.get("cancelled"), f"real={real} mine={mine}")
        # ---- an archive with the MP3 filter in the middle of a set: everything of this run is undone, answer = unsupported
        orig = A._unfilter_fails; hits = []
        def fake(e, _o=orig):
            hits.append(e["rel"]); return len(hits) > 9 or _o(e)
        A._unfilter_fails = fake
        try:
            d2 = tempfile.mkdtemp(); mine = json.loads(A.extract(good, SPW, "[]", d2, False, A.NoProgress()))
            check("set extract: MP3 filter in a later archive -> unsupported", mine.get("unsupported") is True and mine["ok"] is False, str(mine))
            check("set extract: MP3 filter: nothing left behind (files and folders)", tree(d2) == {}, str(list(tree(d2))))
            hits.clear(); d2 = tempfile.mkdtemp(); pre_clash(d2); before = tree(d2)
            mine = json.loads(A.extract(good, SPW, "[]", d2, True, A.NoProgress()))
            check("set extract (overwrite): MP3 filter -> unsupported", mine.get("unsupported") is True, str(mine))
            hits.clear(); mine = json.loads(A.test(good, SPW, A.NoProgress()))
            check("set test: MP3 filter -> unsupported", mine.get("unsupported") is True, str(mine))
        finally: A._unfilter_fails = orig

if part in ("cancel", "all"):
    class RP:      # the real kiln_app progress object: event(json), cancelled()
        def __init__(s, stop_at): s.n = 0; s.stop_at = stop_at
        def event(s, j): s.n += 1
        def cancelled(s): return s.n >= s.stop_at
    for stop in (1, 3, 6):
        d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
        real = norm(R.extract(arc("plain"), "", "[]", d1, False, 0, RP(stop)))
        mine = norm(A.extract(arc("plain"), "", "[]", d2, False, Cnt(stop_at=stop)))
        t1, t2 = tree(d1), tree(d2)
        check(f"cancel after {stop}: result", real.get("cancelled") and mine.get("cancelled"), f"real={real} mine={mine}")
        check(f"cancel after {stop}: nothing left behind", t1 == {} and t2 == {}, f"real={list(t1)} mine={list(t2)}")

if part in ("fnmatch", "all"):
    rnd = random.Random(7); alpha = "ab/.*?[]!-x"
    bad = 0
    for _ in range(40000):
        pat = "".join(rnd.choice(alpha) for _ in range(rnd.randint(0, 7))); s = "".join(rnd.choice("ab/.x-]![") for _ in range(rnd.randint(0, 8)))
        try: want = pyfn.fnmatchcase(s, pat)
        except Exception: continue
        if A.fnmatch(s, pat) != want:
            bad += 1
            if bad < 6: print("  mismatch", repr(s), repr(pat), "py=", want)
    check("fnmatch == python fnmatch (40000 random)", bad == 0, f"{bad} mismatches")
    for p in ["a*b", "x[*]y", "[[]z", "dir/file[?].txt", "w[!]"]:
        check(f"glob_escape {p}", A.glob_escape(p) == __import__("glob").escape(p))
    bad = 0
    for _ in range(500):
        base = "".join(rnd.choice("ab*?[]/.x") for _ in range(rnd.randint(0, 8)))
        if pyfn.fnmatchcase(base, __import__("glob").escape(base)) != A.fnmatch(base, A.glob_escape(base)): bad += 1
    check("escaped name matches itself", bad == 0)
    # ffi dispatcher
    check("ffi version", json.loads(A.call("version", "", None))["kiln"].startswith("rs-"))
    check("ffi unknown op", json.loads(A.call("zap", "{}", None))["ok"] is False)
    check("ffi open", json.loads(A.call("open", json.dumps({"path": arc("plain")}), None))["ok"] is True)
    check("ffi bad json", "bad arguments" in A.call("open", "{oops", None))
sys.exit(1 if fails else 0)
