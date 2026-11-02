"""usage: test_engine.py <python dir (kiln_app.py, kiln_scripts.py, kiln_rs_engine.py ...)> <kiln-rust-port dir>
Tests the app-side engine plumbing with tools/stub_kiln_rs.c standing in for libkiln_rs.so."""
import json, os, stat, subprocess, sys, tempfile
pydir, root = sys.argv[1], sys.argv[2]
sys.path.insert(0, pydir)
import kiln_scripts as S, kiln_rs_engine as E, kiln_app as App
fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)
tmp = tempfile.mkdtemp(); files = os.path.join(tmp, "files"); os.makedirs(files)
def gcc(src_text, out):
    c = os.path.join(tmp, "x.c"); open(c, "w").write(src_text)
    subprocess.check_call(["gcc", "-shared", "-fPIC", "-o", out, c])
stub = os.path.join(tmp, "libkiln_rs.so")
subprocess.check_call(["gcc", "-shared", "-fPIC", "-o", stub, os.path.join(root, "tools", "stub_kiln_rs.c")])
class P:
    def __init__(s, stop_after=None): s.events = []; s.polls = 0; s.stop_after = stop_after
    def event(s, j): s.events.append(json.loads(j))
    def cancelled(s): s.polls += 1; return s.stop_after is not None and s.polls > s.stop_after
a = tmp + "/a.kln"; open(a, "wb").write(b"KLNA\0" + b"x" * 40)

# nothing installed: Python engine
check("no engine: inactive", not E.active() and E.run("open", {"path": a}) is None)
check("no engine: status", json.loads(S.status(files))["engine"] == "" and json.loads(S.status(files))["engine_active"] is False)
check("no engine: version says python", json.loads(App.version())["engine"] == "python")

# rejected candidates
bad = tmp + "/notelf.so"; open(bad, "wb").write(b"hello, not a library")
r = json.loads(S.install(files, "engine", bad, "notelf.so")); check("install: non-ELF refused", r["ok"] is False and ".so" in r["message"], str(r))
nosym = tmp + "/nosym.so"; gcc("int foo(void){return 1;}", nosym)
r = json.loads(S.install(files, "engine", nosym, "nosym.so")); check("install: library without kiln_rs_call refused", r["ok"] is False, str(r))
wrongver = tmp + "/wrongver.so"; gcc('#include <stdlib.h>\n#include <string.h>\nchar*kiln_rs_call(const char*o,const char*a,void*c,void*u){char*p=malloc(20);strcpy(p,"{\\"kiln\\":\\"14\\"}");return p;}\nvoid kiln_rs_free(char*p){free(p);}', wrongver)
r = json.loads(S.install(files, "engine", wrongver, "wrongver.so")); check("install: library with another protocol refused", r["ok"] is False, str(r))
check("refused installs leave nothing behind", not os.path.exists(os.path.join(files, "scripts", "engine.so")) and not [f for f in os.listdir(os.path.join(files, "scripts")) if f.startswith("candidate")])

# good install
r = json.loads(S.install(files, "engine", stub, "libkiln_rs.so"))
check("install: stub accepted, no restart", r["ok"] and r["version"] == "9.9.9" and r["restart"] is False, str(r))
ep = os.path.join(files, "scripts", "engine.so")
check("installed file is read-only (Android 14)", stat.S_IMODE(os.stat(ep).st_mode) & 0o222 == 0)
st = json.loads(S.status(files)); check("status shows the engine", st["engine"] == "libkiln_rs.so" and st["engine_version"] == "9.9.9" and st["engine_active"])
check("version says rust", json.loads(App.version())["engine"] == "rust")

# dispatch
App.set_signature(b"KLNA")
o = json.loads(App.open_archive(a, "")); check("open goes to the engine (utf-8 survives)", o.get("via") == "stub" and o["entries"][0]["p"] == "é.txt", str(o))
p = P(); t = json.loads(App.test(a, "pw", p)); check("test goes to the engine, progress events arrive", t["message"] == "done by stub" and len(p.events) == 5 and p.events[0]["file"] == "x", str(t))
p = P(); e = json.loads(App.extract(a, "", "[]", tmp + "/out", False, 0, p)); check("extract goes to the engine", e["ok"] and e["message"] == "done by stub")
p = P(stop_after=2); e = json.loads(App.extract(a, "", "[]", tmp + "/out", False, 0, p)); check("cancel reaches the engine", e.get("cancelled") is True, str(e))
check("cancel stopped it early", p.polls < 10, f"polls={p.polls}")
p = P(); e = json.loads(App.extract(a, "", "[]", tmp + "/out", False, 0, None)); check("progress=None is fine", e["ok"])
App.set_signature(b"OTHER")
o = json.loads(App.open_archive(a, "")); check("custom main script signature -> Python engine, never Rust", o.get("via") != "stub", str(o)[:80])
App.set_signature(b"KLNA")

# fallbacks
ua = tmp + "/unsupported.kln"; open(ua, "wb").write(b"KLNA\0" + b"x" * 40)
o = json.loads(App.open_archive(ua, "")); check("'unsupported' -> falls back to Python (which gives its own answer)", o.get("via") != "stub" and "unsupported" not in o, str(o)[:100])
check("...and the engine stays active afterwards", E.active())
ca = tmp + "/crash.kln"; open(ca, "wb").write(b"KLNA\0" + b"x" * 40)
o = json.loads(App.open_archive(ca, "")); check("engine returning NULL -> Python answers", o.get("via") != "stub")
check("...engine is switched off with a note", not E.active() and "stopped" in E.note(), E.note())

# boot: engine is configured from files_dir at start-up
S._note = ""; S.boot(files)
check("boot re-enables the installed engine", E.active() and S._note == "", S._note)
# broken file at boot -> Python + note
os.chmod(ep, 0o600); os.remove(ep); open(ep, "wb").write(b"garbage"); S._note = ""; S.boot(files)
check("boot with a damaged engine: Python + note", not E.active() and "Rust engine not used" in S.note(), S.note())

# remove
S.install(files, "engine", stub, "libkiln_rs.so"); r = json.loads(S.remove(files, "engine"))
check("remove: engine gone", r["ok"] and not E.active() and not os.path.exists(ep) and json.loads(S.status(files))["engine"] == "")
check("main script meta untouched by engine install/remove", json.loads(S.status(files))["main"] == "")
sys.exit(1 if fails else 0)
