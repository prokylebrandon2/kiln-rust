"""Line-for-line Python mirror of kiln-rs/src/app.rs (and ffi.call). Uses container_mirror for the archive."""
import json, os, time, zlib
import container_mirror as M

CANCELLED = "\x01cancelled"
MIN_ARCHIVE = 25
MAGIC = b"KLNA"

class NoProgress:
    def event(self, j): pass
    def cancelled(self): return False

def version(): return json.dumps({"kiln": "rs-0.1.0", "helper": ""})

# ---- fnmatch (tokens), same algorithm as app.rs
def compile_pat(p):
    c = list(p); n = len(c); out = []; i = 0
    while i < n:
        ch = c[i]; i += 1
        if ch == "*":
            if not (out and out[-1][0] == "star"): out.append(("star",))
        elif ch == "?": out.append(("any",))
        elif ch == "[":
            j = i
            if j < n and c[j] == "!": j += 1
            if j < n and c[j] == "]": j += 1
            while j < n and c[j] != "]": j += 1
            if j >= n: out.append(("lit", "["))
            else:
                body = c[i:j]
                neg = bool(body) and body[0] == "!"
                if neg: body = body[1:]
                rs = []; k = 0
                while k < len(body):
                    if k + 2 < len(body) and body[k + 1] == "-": rs.append((body[k], body[k + 2])); k += 3
                    else: rs.append((body[k], body[k])); k += 1
                out.append(("set", neg, rs)); i = j + 1
        else: out.append(("lit", ch))
    return out

def one(t, ch):
    if t[0] == "any": return True
    if t[0] == "lit": return t[1] == ch
    if t[0] == "set": return any(a <= ch <= b for a, b in t[2]) != t[1]
    return False

def fnmatch(name, pat):
    toks = compile_pat(pat); s = list(name); ti = si = 0; star = None; mark = 0
    while si < len(s):
        if ti < len(toks) and toks[ti][0] == "star": star = ti; ti += 1; mark = si
        elif ti < len(toks) and one(toks[ti], s[si]): ti += 1; si += 1
        elif star is not None: mark += 1; si = mark; ti = star + 1
        else: return False
    while ti < len(toks) and toks[ti][0] == "star": ti += 1
    return ti == len(toks)

def glob_escape(p): return "".join("[" + ch + "]" if ch in "*?[" else ch for ch in p)

def patterns(members):
    out = []
    for m in members:
        e = glob_escape(m.get("p", "")); out.append(e + "/*" if m.get("dir") else e)
    return out

# ---- paths
def abspath(p):
    full = p if p.startswith("/") else os.getcwd().rstrip("/") + "/" + p
    parts = []
    for c in full.split("/"):
        if c in ("", "."): continue
        if c == "..":
            if parts: parts.pop()
        else: parts.append(c)
    return "/" + "/".join(parts)

def join_rel(root, rel):
    if rel.startswith("/"): return rel
    return root + rel if root.endswith("/") else root + "/" + rel

def is_under(root, dest): return dest.startswith(root.rstrip("/") + "/")

def fmt_size(n):
    x = float(n)
    for unit in ("B", "KB", "MB", "GB"):
        if x < 1024 or unit == "GB": return f"{x:.0f} {unit}" if unit == "B" else f"{x:.1f} {unit}"
        x /= 1024

def app_message(err):
    lines = [l for l in err.replace("\r", "\n").split("\n") if l.strip()]
    if not lines: return ""
    return "\n".join([lines[0].strip()] + [l.strip() for l in lines[1:]]).strip()

def _unfilter_fails(e):
    try: M.make_unfilter(e); return False
    except M.KilnError: return True

def unsupported(msg): return json.dumps({"ok": False, "unsupported": True, "error": msg, "message": msg, "done": 0, "bad_password": False})

def fail_json(msg, done): return json.dumps({"ok": False, "message": msg, "done": done, "bad_password": "wrong password" in msg}, ensure_ascii=False)

# ---- open
class Err(Exception): pass

def sniff(path):
    try: size = os.path.getsize(path)
    except OSError as e: raise Err(f"couldn't read this archive: {e}")
    head = b""
    if size >= MIN_ARCHIVE:
        try:
            with open(path, "rb") as f: head = f.read(5)
        except OSError as e: raise Err(f"couldn't read this archive: {e}")
    if size >= MIN_ARCHIVE and len(head) >= 4 and head[:4] == MAGIC:
        if len(head) >= 5: return bool(head[4] & 1)
        raise Err("this file isn't a Kiln archive")
    if size >= MIN_ARCHIVE and len(head) >= 4 and head[:4] in ():
        raise Err("this archive was made by an older Kiln (format 9 or earlier) that this app can't open")
    raise Err("this file isn't a Kiln archive")

def open_file(path, password):
    enc = sniff(path)
    f = open(path, "rb")
    try: return M.Archive.open(f, password if enc else None), f
    except M.KilnError as e: f.close(); raise Err(str(e))

def open_archive(path, password):
    if os.path.isdir(path): return set_open(path, password)
    try: enc = sniff(path)
    except Err as e: return json.dumps({"ok": False, "error": str(e)}, ensure_ascii=False)
    if enc and not password: return json.dumps({"ok": False, "need_password": True})
    try: a, f = open_file(path, password)
    except Err as e:
        m = str(e); return json.dumps({"ok": False, "error": m, "bad_password": "wrong password" in m}, ensure_ascii=False)
    with f:
        ents = [{"p": e["rel"], "s": e["size"], "m": e["mtime"]} for e in a.entries if not e["dead"]]
        return json.dumps({"ok": True, "kind": "archive", "encrypted": enc, "entries": ents, "packed": a.table_off - a.hdr_len}, ensure_ascii=False)

# ---- sink
class FsSink:
    def __init__(s, root, write, prog, total, outer=None):
        s.root, s.write_, s.prog, s.total = root, write, prog, total
        s.outer = outer          # (finished archives, archives in the set) when this archive is part of a set
        s.done = 0; s.bytes = 0; s.file = ""; s.fh = None; s.dest = None; s.created = []; s.made = []
        s.events = []
    def emit(s, force=False):
        if s.outer is not None:
            od, on = s.outer
            inner = min(s.bytes / s.total, 1.0) if s.total > 0 else 0.0
            ev = json.dumps({"frac": min((od + inner) / on, 1.0) if on > 0 else -1.0, "done": od, "total": on, "file": s.file})
            s.events.append(ev); s.prog.event(ev); return
        frac = min(s.bytes / s.total, 1.0) if s.total > 0 else -1.0
        ev = json.dumps({"frac": frac, "done": s.done, "total": 0, "file": s.file}); s.events.append(ev); s.prog.event(ev)
    def check(s):
        if s.prog.cancelled(): raise M.KilnError(CANCELLED)
    def abort(s):
        if s.fh is not None:
            s.fh.close(); s.fh = None
            if s.dest is not None:
                try: os.remove(s.dest)
                except OSError: pass
                s.dest = None
    def rollback(s):
        s.abort()
        for p in reversed(s.created):
            try: os.remove(p)
            except OSError: pass
        s.created = []
        for d in reversed(s.made):
            try: os.rmdir(d)
            except OSError: pass
        s.made = []
    def begin(s, idx, e):
        s.check(); s.file = e["rel"]
        if s.write_:
            dest = abspath(join_rel(s.root, e["rel"]))
            i = dest.rfind("/"); parent = "/" if i <= 0 else dest[:i]
            d = parent; missing = []
            while d != "/" and not os.path.isdir(d):
                missing.append(d); i = d.rfind("/"); d = "/" if i <= 0 else d[:i]
            try: os.makedirs(parent, exist_ok=True)
            except OSError as er: raise M.KilnError(f"{parent}: {er}")
            s.made.extend(reversed(missing))
            s.emit(True)
            existed = os.path.lexists(dest)
            try: fh = open(dest, "wb")
            except OSError as er: raise M.KilnError(f"{dest}: {er}")
            if not existed: s.created.append(dest)
            s.fh = fh; s.dest = dest
        else: s.emit(True)
    def write(s, idx, piece):
        s.check(); s.bytes += len(piece)
        if s.fh is not None: s.fh.write(piece)
        s.emit(False)
    def end(s, idx, e):
        if s.fh is not None:
            s.fh.close(); s.fh = None
            d, s.dest = s.dest, None
            if d is not None:
                try: os.utime(d, (e["mtime"], e["mtime"]))
                except OSError: pass
                m = e["mode"] & 0o777
                try: os.chmod(d, m if m else 0o644)
                except OSError: pass
                s.done += 1

def test(path, password, prog): return run(path, password, [], "", False, False, prog)

def extract(path, password, members_json, dest, overwrite, prog):
    try: members = json.loads(members_json)
    except ValueError: members = []
    return run(path, password, patterns(members), dest, overwrite, True, prog)

def run(path, password, names, dest, force, write, prog):
    if os.path.isdir(path): return set_run(path, password, names, dest, force, write, prog)
    try: enc = sniff(path)
    except Err as e: return fail_json(str(e), 0)
    if enc and not password: return fail_json("enter a password", 0)
    try: a, f = open_file(path, password)
    except Err as e: return fail_json(app_message(str(e)), 0)
    with f:
        if any(_unfilter_fails(e) for e in a.entries):
            return unsupported("this archive uses the MP3 filter, which the Rust engine does not support yet")
        t0 = time.time()
        root = abspath(dest if write else ".")
        wanted = [(not e["dead"]) and (not names or any(fnmatch(e["rel"], p) or e["rel"] == p for p in names)) for e in a.entries]
        if names and not any(wanted): return fail_json(app_message("no files in the archive match: " + ", ".join(names)), 0)
        if write:
            clashes = []
            for e, w in zip(a.entries, wanted):
                if not w: continue
                d = abspath(join_rel(root, e["rel"]))
                if not is_under(root, d): return fail_json(app_message(f"refusing unsafe path: {e['rel']}"), 0)
                if os.path.exists(d): clashes.append(e["rel"])
            if clashes and not force:
                shown = "\n  ".join(clashes[:10])
                more = f"\n  ... and {len(clashes) - 10} more" if len(clashes) > 10 else ""
                m = f"{len(clashes)} file(s) already exist:\n  {shown}{more}\nnothing was extracted; use -f to overwrite"
                # kiln_app's sink counts every printed line that starts with two spaces as "done"
                return fail_json(app_message(m), sum(1 for l in m.split("\n") if l.startswith("  ")))
        n = len(a.entries); select = bool(names)
        lo, hi = ((wanted.index(True), n - 1 - wanted[::-1].index(True)) if select else (0, max(n - 1, 0)))
        k, _off, _ctr = M.find_ckpt(a.ckpts, lo, a.hdr_len)
        part = select and (k > 0 or hi + 1 < n)
        total = sum(e["slen"] for e in a.entries[k:hi + 1]) if part else sum(e["slen"] for e in a.entries)
        sink = FsSink(root, write, prog, total)
        try:
            a.extract(wanted, select, sink)
        except M.KilnError as e:
            if str(e) == CANCELLED:
                sink.rollback(); return json.dumps({"ok": False, "cancelled": True, "message": "Cancelled"})
            sink.abort(); return fail_json(app_message(str(e)), sink.done)
        sink.abort(); sink.emit(True)
        nf = sum(wanted); verb = "extracted" if write else "verified"
        msg = f"{verb} {nf} file(s), {fmt_size(total)} in {time.time() - t0:.1f}s - all checksums OK"
        run.last_events = sink.events
        return json.dumps({"ok": True, "message": msg, "done": sink.done, "bad_password": False}, ensure_ascii=False)


# ================================================================ sets (CP8b): mirror of src/sets.rs
import shutil, tempfile
INDEX_NAME = "_index.kln"; INDEX_FILE = "index.json"
MP3_MSG = "this archive uses the MP3 filter, which the Rust engine does not support yet"
UNSUPPORTED = "\x01unsupported"          # unwinds a run that met an MP3 filter (nothing is kept)
DAMAGED_INDEX = "the index archive is damaged or isn't a kiln index"

def indented(msg):
    """kiln_app's sink counts every printed line that starts with two spaces as `done`."""
    return sum(1 for l in msg.replace("\r", "\n").split("\n") if l.strip() and l.startswith("  "))

def cancelled_json(): return json.dumps({"ok": False, "cancelled": True, "message": "Cancelled"})

class IndexSink:
    """Collects index.json of the index archive in memory."""
    def __init__(s, prog): s.prog, s.cur, s.buf = prog, None, None
    def check(s):
        if s.prog.cancelled(): raise M.KilnError(CANCELLED)
    def begin(s, idx, e): s.check(); s.cur = bytearray() if e["rel"] == INDEX_FILE else None
    def write(s, idx, piece):
        s.check()
        if s.cur is not None: s.cur += piece
    def end(s, idx, e):
        if s.cur is not None: s.buf = bytes(s.cur); s.cur = None

def _is_int(v): return type(v) is int

def parse_index(buf):
    """index.json bytes -> [(archive name, original path, size|None, mtime|None)] in file order."""
    try: data = json.loads(buf.decode("utf-8"))
    except ValueError: raise M.KilnError(DAMAGED_INDEX)
    files = data.get("files") if isinstance(data, dict) else None
    ver = data.get("v") if isinstance(data, dict) else None
    out = None
    if _is_int(ver) and ver in (1, 2) and isinstance(files, dict):
        out = []
        for k, v in files.items():
            if ver == 1 and isinstance(v, str): out.append((k, v, None, None))
            elif ver == 2 and isinstance(v, dict) and isinstance(v.get("path"), str) and _is_int(v.get("size")) and _is_int(v.get("mtime")):
                out.append((k, v["path"], v["size"], v["mtime"]))
            else: out = None; break
    if out is None: raise M.KilnError("the index archive has an unknown layout")
    return out

def read_index_meta(d, pw, prog):
    """Port of kiln_v14.read_index_meta: unpack _index.kln (always checking every checksum), read index.json."""
    path = join_rel(d, INDEX_NAME)
    try: f = open(path, "rb")
    except OSError as er: raise M.KilnError(f"{path}: {er}")
    with f:
        a = M.Archive.open(f, pw)
        root = abspath("/kiln-index")
        wanted = [not e["dead"] for e in a.entries]
        for e, w in zip(a.entries, wanted):
            if w and not is_under(root, abspath(join_rel(root, e["rel"]))):
                raise M.KilnError(f"refusing unsafe path: {e['rel']}")
        sink = IndexSink(prog)
        a.extract(wanted, False, sink)
    if sink.buf is None: raise M.KilnError(DAMAGED_INDEX)
    return parse_index(sink.buf)

def set_open(path, password):
    if not os.path.isfile(join_rel(path, INDEX_NAME)): return json.dumps({"ok": False, "error": "this folder isn't a Kiln set"})
    if not password: return json.dumps({"ok": False, "need_password": True})
    try: meta = read_index_meta(path, password, NoProgress())
    except M.KilnError as e:
        m = str(e)
        if m == CANCELLED: return cancelled_json()
        return json.dumps({"ok": False, "error": m, "bad_password": "wrong password" in m}, ensure_ascii=False)
    ents = [{"p": p, "s": sz, "m": mt} for _n, p, sz, mt in meta]
    packed = 0
    for n, _p, _s, _m in meta:
        try: packed += os.path.getsize(join_rel(path, n))
        except OSError: pass
    return json.dumps({"ok": True, "kind": "set", "encrypted": True, "entries": ents, "packed": packed}, ensure_ascii=False)

def safe_dest(root, rel):
    parts = rel.split("/")
    dest = abspath(root.rstrip("/") + "/" + "/".join(parts))
    if not is_under(root, dest) or dest.rsplit("/", 1)[-1] != parts[-1]:
        raise M.KilnError(f"refusing unsafe path: {rel}")
    return dest

def extract_archive(src, root, pw, write, prog, outer):
    """One archive of a set (kiln_v14.run_extract with names=[] and -f): into `root` when write, else verify."""
    f = open(src, "rb")
    with f:
        a = M.Archive.open(f, pw)
        if any(_unfilter_fails(e) for e in a.entries): raise M.KilnError(UNSUPPORTED)
        wanted = [not e["dead"] for e in a.entries]
        if write:
            for e, w in zip(a.entries, wanted):
                if w and not is_under(root, abspath(join_rel(root, e["rel"]))):
                    raise M.KilnError(f"refusing unsafe path: {e['rel']}")
        total = sum(e["slen"] for e in a.entries)
        sink = FsSink(root, write, prog, total, outer)
        try: a.extract(wanted, False, sink)
        except M.KilnError:
            if write: sink.rollback()
            else: sink.abort()
            raise
        sink.abort()

def extract_one(src, out_dir, pw, write, name, prog, outer):
    if write:
        tmp = tempfile.mkdtemp(prefix=".kiln-", dir=out_dir)
        try:
            extract_archive(src, tmp, pw, True, prog, outer)
            files = os.listdir(tmp)
            if len(files) != 1: raise M.KilnError("unexpected layout in a set archive")
            os.replace(os.path.join(tmp, files[0]), os.path.join(out_dir, name))
        finally: shutil.rmtree(tmp, ignore_errors=True)
    else:
        extract_archive(src, "", pw, False, prog, outer)

def set_run(d, password, names, dest, force, write, prog):
    if not os.path.isfile(join_rel(d, INDEX_NAME)):
        m = f"{d} is a directory without {INDEX_NAME} (not a kiln random-name set)"
        return fail_json(app_message(m), indented(m))
    if not password: return fail_json("enter a password", 0)
    try: meta = read_index_meta(d, password, prog)
    except M.KilnError as e:
        m = str(e)
        if m == CANCELLED: return cancelled_json()
        return fail_json(app_message(m), indented(m))
    items = sorted(((n, p) for n, p, _s, _m in meta), key=lambda kv: kv[1])
    if names:
        items = [(n, r) for n, r in items if any(fnmatch(r, p) or r == p for p in names)]
        if not items:
            m = "no files in the index match: " + ", ".join(names)
            return fail_json(app_message(m), indented(m))
    root = abspath(dest if write else ".")
    plan, missing, clashes = [], [], []
    for n, rel in items:
        src = join_rel(d, n)
        try: dst = safe_dest(root, rel)
        except M.KilnError as e: return fail_json(app_message(str(e)), indented(str(e)))
        if not os.path.isfile(src): missing.append(rel); continue
        if write and os.path.exists(dst): clashes.append(rel)
        plan.append((src, dst, rel))
    if clashes and not force:
        shown = "\n  ".join(clashes[:10])
        more = f"\n  ... and {len(clashes) - 10} more" if len(clashes) > 10 else ""
        m = f"{len(clashes)} file(s) already exist:\n  {shown}{more}\nnothing was extracted; use -f to overwrite"
        return fail_json(app_message(m), indented(m))
    t0 = time.time(); done = 0; moved = []; made = []
    def undo():                               # an MP3 filter turned up: leave nothing of this run behind
        for p in reversed(moved):
            try: os.remove(p)
            except OSError: pass
        for q in reversed(made):
            try: os.rmdir(q)
            except OSError: pass
    for src, dst, rel in plan:
        parent = dst.rsplit("/", 1)[0] or "/"
        if write:
            d2 = parent; miss = []
            while d2 != "/" and not os.path.isdir(d2):
                miss.append(d2); i = d2.rfind("/"); d2 = "/" if i <= 0 else d2[:i]
            try: os.makedirs(parent, exist_ok=True)
            except OSError as er: return fail_json(app_message(f"{parent}: {er}"), done)
            made.extend(reversed(miss))
        existed = os.path.lexists(dst)
        try: extract_one(src, parent, password, write, dst.rsplit("/", 1)[-1], prog, (done, len(plan)))
        except M.KilnError as err:
            if str(err) == CANCELLED: return cancelled_json()
            if str(err) == UNSUPPORTED:
                if write: undo()
                return unsupported(MP3_MSG)
            m = f"{rel} ({os.path.basename(src)}): {err}"
            return fail_json(app_message(m), done + indented(m))
        if write and not existed: moved.append(dst)
        done += 1
    verb = "extracted" if write else "verified"
    if missing:
        shown = "\n  ".join(missing[:10])
        m = f"{len(missing)} archive(s) listed in the index are missing:\n  {shown}"
        return fail_json(app_message(m), done + indented(m))
    msg = f"{verb} {len(plan)} file(s) from {d} in {time.time() - t0:.1f}s - all checksums OK"
    return json.dumps({"ok": True, "message": msg, "done": done, "bad_password": False}, ensure_ascii=False)

def call(op, args, prog):
    try: a = json.loads(args or "{}")
    except ValueError as e: return json.dumps({"ok": False, "error": f"bad arguments: {e}"})
    g = lambda k: a.get(k) if isinstance(a.get(k), str) else ""
    if op == "version": return version()
    if op == "open": return open_archive(g("path"), g("password"))
    if op == "test": return test(g("path"), g("password"), prog)
    if op == "extract":
        return extract(g("path"), g("password"), json.dumps(a.get("members", [])), g("dest"), bool(a.get("overwrite", False)), prog)
    return json.dumps({"ok": False, "error": f"unknown operation: {op}"})
