"""Drop-in for tests/mirror/app_mirror.py that calls the REAL compiled libkiln_rs.so (env KILN_RS_SO)
through kiln_rs_engine's ctypes code, so the test_app.py checks run against the actual Rust.
Needs env KILN_RS_SO (libkiln_rs.so) and KILN_MIRROR_DIR (tests/mirror); see run.py."""
import json, os, sys, importlib.util
sys.path.append(os.environ["KILN_MIRROR_DIR"])
import kiln_rs_engine as E
_lib = E._bind(os.environ["KILN_RS_SO"])
_spec = importlib.util.spec_from_file_location("app_mirror_real", os.path.join(os.environ["KILN_MIRROR_DIR"], "app_mirror.py"))
_real = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(_real)
fnmatch, glob_escape, NoProgress, M = _real.fnmatch, _real.glob_escape, _real.NoProgress, _real.M
_unfilter_fails = _real._unfilter_fails

def _c(op, args, prog):
    return json.dumps(E._call(_lib, op, args, prog), ensure_ascii=False)

def version(): return _c("version", {}, None)
def open_archive(path, password): return _c("open", {"path": path, "password": password}, None)
def test(path, password, prog): return _c("test", {"path": path, "password": password}, prog)
def extract(path, password, members_json, dest, overwrite, prog):
    return _c("extract", {"path": path, "password": password, "members": json.loads(members_json), "dest": dest, "overwrite": overwrite}, prog)
def call(op, args, prog):
    # the raw C entry point, including bad json
    import ctypes
    c_cb = E._CB(lambda ev, u: 0)
    raw = _lib.kiln_rs_call(op.encode(), (args or "").encode(), c_cb, None)
    try: return ctypes.string_at(raw).decode()
    finally: _lib.kiln_rs_free(raw)
