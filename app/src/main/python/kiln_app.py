"""What the Kiln app needs, as plain functions that return JSON strings.

Everything here is testable without Android. The Java side passes a `progress` object with
event(json) and cancelled(); None is fine too."""
import fnmatch
import glob
import hashlib
import json
import os
import re
import shutil
import time
import uuid

import kiln_v14 as kiln
import kiln_bridge as bridge
import kiln_rs_engine as rs

_MAGIC = b"KLNA"
_MIN_ARCHIVE = 25          # header + footer: nothing smaller can be an archive


def _j(**kw):
    return json.dumps(kw, ensure_ascii=False)


def version():
    return _j(kiln=getattr(kiln, "VERSION", "?"), helper=bridge.accel_note(),
              engine="rust" if rs.active() else "python", engine_note=rs.note())


# ---------------------------------------------------------------- browsing

_SNIFF_LEN = 5


def _sniff(path, size):
    """-> ('archive', encrypted) | None, from the first bytes of a file.
    Reads straight from the descriptor: a buffered open() would pull in a whole 8 KB block."""
    if size < _MIN_ARCHIVE:
        return None
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return None
    try:
        head = os.read(fd, _SNIFF_LEN)
    except OSError:
        return None
    finally:
        os.close(fd)
    if head.startswith(_MAGIC) and len(head) >= 5:
        return "archive", bool(head[4] & kiln.FLAG_ENCRYPTED)
    return None


def scan_params():
    """What the Java side needs to recognise archives itself (it lists folders natively, which is
    faster than a round trip through Python for every folder)."""
    return _j(sig=_MAGIC.hex(), min=_MIN_ARCHIVE, builtin=True,
              enc=kiln.FLAG_ENCRYPTED, index=kiln.INDEX_NAME)


def _entry(e, show_all, show_hidden):
    """One folder entry -> its row for the file manager, or None to leave it out."""
    try:
        hidden = e.name.startswith(".")
        if hidden and not show_hidden:
            return None
        if e.is_dir():
            st = e.stat()
            is_set = os.path.isfile(os.path.join(e.path, kiln.INDEX_NAME))
            try:
                count = len(os.listdir(e.path))
            except OSError:
                count = 0
            return {"n": e.name, "k": "set" if is_set else "dir",
                    "s": 0, "m": int(st.st_mtime), "c": count}
        if e.is_file():
            st = e.stat()
            kind, enc = "file", False
            sn = _sniff(e.path, st.st_size)
            if sn:
                kind, enc = sn
            if kind == "file" and not show_all:
                return None
            return {"n": e.name, "k": kind, "s": st.st_size,
                    "m": int(st.st_mtime), "e": enc}
    except OSError:
        pass
    return None


def scan(path, show_all, show_hidden=False):
    """One folder for the file manager: folders, kiln archives, random-name sets, and (when
    show_all) every other file."""
    try:
        entries = list(os.scandir(path))
    except OSError as e:
        return _j(ok=False, error=f"can't open {path}: {e.strerror or e}")
    found = [_entry(e, show_all, show_hidden) for e in entries]
    items = [i for i in found if i is not None]
    order = {"dir": 0, "set": 0}
    items.sort(key=lambda i: (order.get(i["k"], 1), i["n"].casefold()))
    return _j(ok=True, path=path, items=items,
              is_set=os.path.isfile(os.path.join(path, kiln.INDEX_NAME)))


# ---------------------------------------------------------------- search

_search_gen = [0]
SEARCH_LIMIT = 1000


def search_cancel():
    """Stops any search that is still walking folders."""
    _search_gen[0] += 1


def _name_matcher(query):
    """-> f(lowercased name) -> bool, or None for an empty/invalid query. Three forms:
    "re:<regex>" (regular expression, found anywhere in the name), a query with * or ?
    (wildcards, must match the whole name, so "*.txt" means ends in .txt), or plain words
    (the name must contain every word)."""
    q = query.strip()
    if not q:
        return None
    if q[:3].lower() == "re:":
        try:
            rx = re.compile(q[3:].strip(), re.IGNORECASE)
        except re.error:
            return None
        return lambda low: rx.search(low) is not None
    if "*" in q or "?" in q:
        rx = re.compile(fnmatch.translate(q.casefold()))
        return lambda low: rx.match(low) is not None
    terms = [t for t in q.casefold().split() if t]
    return lambda low: all(t in low for t in terms)


def search(path, query, show_all, show_hidden=False):
    """Every file and folder below `path` whose name matches `query` (see _name_matcher; plain
    words are case-insensitive). Names come back relative to `path`, so the app can treat them like
    entries of the folder itself. A newer search (or search_cancel) stops this one."""
    _search_gen[0] += 1
    mine = _search_gen[0]
    match = _name_matcher(query)
    if match is None:
        return _j(ok=True, path=path, items=[], truncated=False)
    items = []
    truncated = False
    stack = [""]
    base = path
    while stack:
        if _search_gen[0] != mine:
            return _j(ok=False, cancelled=True)
        rel = stack.pop()
        here = os.path.join(base, rel) if rel else base
        try:
            it = os.scandir(here)
        except OSError:
            continue
        with it:
            for e in it:
                name = e.name
                if name.startswith(".") and not show_hidden:
                    continue
                try:
                    is_dir = e.is_dir(follow_symlinks=False)
                    low = name.casefold()
                    hit = match(low)
                    relname = rel + "/" + name if rel else name
                    if is_dir:
                        if hit:
                            st = e.stat()
                            is_set = os.path.isfile(os.path.join(e.path, kiln.INDEX_NAME))
                            try:
                                count = len(os.listdir(e.path))
                            except OSError:
                                count = 0
                            items.append({"n": relname, "k": "set" if is_set else "dir", "s": 0,
                                          "m": int(st.st_mtime), "c": count})
                        stack.append(relname)
                    elif hit and e.is_file():
                        st = e.stat()
                        kind, enc = "file", False
                        sn = _sniff(e.path, st.st_size)
                        if sn:
                            kind, enc = sn
                        if kind == "file" and not show_all:
                            continue
                        items.append({"n": relname, "k": kind, "s": st.st_size,
                                      "m": int(st.st_mtime), "e": enc})
                except OSError:
                    continue
                if len(items) >= SEARCH_LIMIT:
                    truncated = True
                    break
        if truncated:
            break
    if _search_gen[0] != mine:
        return _j(ok=False, cancelled=True)
    return _j(ok=True, path=path, items=items, truncated=truncated)


# ---------------------------------------------------------------- opening

def _rust(op, args, progress=None):
    """The Rust engine's JSON answer, or None = use Python (no engine, unsupported job, or the
    engine failed)."""
    if not rs.active():
        return None
    r = rs.run(op, args, progress)
    return None if r is None else json.dumps(r, ensure_ascii=False)


def open_archive(path, password):
    """Table of contents of an archive file or random-name set."""
    r = _rust("open", {"path": path, "password": password or ""})
    return r if r is not None else _open_main(path, password)


def _open_main(path, password):
    try:
        if os.path.isdir(path):
            if not os.path.isfile(os.path.join(path, kiln.INDEX_NAME)):
                return _j(ok=False, error="this folder isn't a Kiln set")
            if not password:
                return _j(ok=False, need_password=True)
            meta = kiln.read_index_meta(path, password)
            ents = [{"p": m["path"], "s": m["size"], "m": m["mtime"]} for m in meta.values()]
            packed = 0
            for n in meta:
                try:
                    packed += os.path.getsize(os.path.join(path, n))
                except OSError:
                    pass
            return _j(ok=True, kind="set", encrypted=True, entries=ents, packed=packed)
        sn = _sniff(path, os.path.getsize(path))
        if sn is None:
            return _j(ok=False, error="this file isn't a Kiln archive")
        enc = sn[1]
        if enc and not password:
            return _j(ok=False, need_password=True)
        f, table, table_off, hdr_len, cipher = kiln.open_archive(path, password if enc else None)
        with f:
            ents = [{"p": e["rel"], "s": e["size"], "m": e["mtime"]}
                    for e in table if not e.get("dead")]
            packed = table_off - hdr_len
        return _j(ok=True, kind="archive", encrypted=enc, entries=ents, packed=packed)
    except kiln.KilnError as e:
        msg = str(e)
        if msg == "enter a password" and not password:     # a format whose lock we can't see
            return _j(ok=False, need_password=True)
        return _j(ok=False, error=msg, bad_password="wrong password" in msg)
    except Exception as e:                       # corrupt files must never crash the app
        return _j(ok=False, error=f"couldn't read this archive: {e}")


# ---------------------------------------------------------------- running commands

class _Sink:
    """Turns kiln's printed lines and byte counters into progress events and a final message.

    The name shown is the file being worked on right now (kiln tells us when each one starts),
    and the fraction is by bytes whenever the total is known, so the bar moves steadily rather
    than jumping once per finished file."""
    def __init__(self, progress, total, use_bytes, total_bytes=0, each=False, blend=False):
        self.progress, self.total, self.use_bytes = progress, total, use_bytes
        # blend: a set of separate archives. Progress = finished archives + how far into the current
        # one. The first byte reports belong to reading the index, so they are skipped.
        self.blend, self.index_pending = blend, blend
        self.done, self.file, self.bfrac = 0, "", None
        self.lines, self.last = [], 0.0
        self.total_bytes, self.each = total_bytes, each
        self.base = self.cur = 0          # bytes in finished files / size of the current one
        self.packed = []                  # full paths of files now safely inside a finished archive

    def files_done(self, paths):
        self.packed.extend(paths)

    def _check(self):
        if self.progress is not None and self.progress.cancelled():
            raise bridge.Cancelled()

    def file_started(self, name, size):
        self._check()
        if self.each:
            self.base += self.cur           # the previous archive is finished
            self.bfrac = 0
        self.cur = size or 0
        self.file = name
        self._emit(True)

    def _emit(self, force=False):
        if self.progress is None:
            return
        now = time.time()
        if not force and now - self.last < 0.05:
            return
        self.last = now
        if self.use_bytes and self.bfrac is not None:
            frac = self.bfrac
        elif self.each and self.total_bytes:
            frac = min((self.base + (self.bfrac or 0) * self.cur) / self.total_bytes, 1.0)
        elif self.blend and self.total:
            frac = min((self.done + (self.bfrac or 0)) / self.total, 1.0)
        elif self.total:
            frac = min(self.done / self.total, 1.0)
        else:
            frac = -1                       # unknown: the UI shows a moving bar
        self.progress.event(_j(frac=frac, done=self.done, total=self.total, file=self.file))

    def line(self, text):
        self._check()
        self.lines.append(text)
        if text.startswith("  "):
            self.done += 1
            if self.blend:
                self.bfrac = 0                # the next archive starts from zero
            if not self.file:                # kiln didn't announce a file: fall back to the line
                self.file = text.strip().split(": ")[0]
            self._emit(self.done == self.total)

    def on_bytes(self, done, total):
        self._check()
        if self.index_pending:                # reading the index, not checking files yet
            if total and done >= total:
                self.index_pending = False
            return
        self.bfrac = min(done / total, 1.0) if total else None
        self._emit()

    def message(self, ok):
        if ok:
            tail = [x for x in self.lines if not x.startswith("  ")]
            return tail[-1] if tail else ""
        for i, x in enumerate(self.lines):
            if x.startswith("error:"):
                first = x[len("error:"):].strip()
                return "\n".join([first] + [y.strip() for y in self.lines[i + 1:]]).strip()
        return self.lines[-1] if self.lines else "the operation failed"


def _run(args, password, total, progress, use_bytes, total_bytes=0, each=False, keep_sink=None,
         blend=False):
    sink = _Sink(progress, total, use_bytes, total_bytes, each, blend)
    if keep_sink is not None:
        keep_sink.append(sink)
    rc = bridge.run(args, password, sink, hook=sink.on_bytes)
    if rc == 130:
        return _j(ok=False, cancelled=True, message="Cancelled")
    if rc == 0 and progress is not None:
        sink._emit(True)
    msg = sink.message(rc == 0)
    return _j(ok=rc == 0, message=msg, done=sink.done, bad_password="wrong password" in msg)


def _patterns(members):
    out = []
    for m in members:
        p = glob.escape(m["p"])
        out.append(p + "/*" if m.get("dir") else p)
    return out


def extract(path, password, members_json, dest, overwrite, total, progress):
    """members_json: [{"p": path inside the archive, "dir": bool}, ...]; [] = everything."""
    r = _rust("extract", {"path": path, "password": password or "", "members": json.loads(members_json),
                          "dest": dest, "overwrite": bool(overwrite)}, progress)
    if r is not None:
        return r
    members = json.loads(members_json)
    args = ["unpack", "-v"] + (["-f"] if overwrite else []) + ["-o", dest, "--", path] \
        + _patterns(members)
    return _run(args, password, total, progress, not os.path.isdir(path))


def test(path, password, progress):
    r = _rust("test", {"path": path, "password": password or ""}, progress)
    if r is not None:
        return r
    if os.path.isdir(path):
        # a set of archives: one line per archive, so the count gives a real bar
        try:
            n = sum(1 for f in os.listdir(path)
                    if f != kiln.INDEX_NAME and os.path.isfile(os.path.join(path, f)))
        except OSError:
            n = 0
        return _run(["test", path], password, n, progress, False, blend=n > 0)
    return _run(["test", path], password, 0, progress, True)


def decrypt_in_place(path, password, total, progress):
    return _run(["unpack", "-v", "-f", "-i", "--", path], password, total, progress, False)


# ---------------------------------------------------------------- preview cache
# A file opened from an archive is extracted once into cache_dir/<key>/<name>. The key comes from
# the archive (path, size, modified time) and the member, so opening the same file again finds it
# there instead of extracting it again. A copy is built in "<key>.part" and renamed when it is
# complete, so a cancel or crash never leaves something that looks finished. The cache is kept
# under a size limit by dropping the least recently used entries.

CACHE_LIMIT = 1 << 30          # bytes of extracted files kept between uses
CACHE_MAX_AGE = 7 * 86400      # and nothing older than a week
_FREE_MARGIN = 64 << 20        # keep this much storage free when extracting


def _tree_size(path):
    total = 0
    for root, _dirs, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(root, f)).st_size
            except OSError:
                pass
    return total


def cache_key(path, member):
    """Identifies `member` of the archive as it is on disk now (a changed archive = a new key)."""
    st = os.stat(os.path.join(path, kiln.INDEX_NAME) if os.path.isdir(path) else path)
    raw = f"{os.path.abspath(path)}\0{st.st_size}\0{st.st_mtime_ns}\0{member}"
    return hashlib.sha1(raw.encode("utf-8", "replace")).hexdigest()[:24]


def _rm(path):
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path, ignore_errors=True)
    else:
        try:
            os.remove(path)
        except OSError:
            pass


def trim_cache(cache_dir, limit=None, keep=None):
    """Drop unfinished copies, entries older than a week, then the least recently used ones
    until the cache fits the limit. `keep` (a key) is never dropped. -> bytes left."""
    limit = CACHE_LIMIT if limit is None else limit
    try:
        names = os.listdir(cache_dir)
    except OSError:
        return 0
    rows, now = [], time.time()
    for n in names:
        p = os.path.join(cache_dir, n)
        if n.endswith(".part"):
            _rm(p)                      # an interrupted extraction
            continue
        try:
            rows.append([os.stat(p).st_mtime, n, _tree_size(p) if os.path.isdir(p) else os.path.getsize(p)])
        except OSError:
            continue
    rows.sort()
    keep_rows = []
    for r in rows:
        if r[1] != keep and now - r[0] > CACHE_MAX_AGE:
            _rm(os.path.join(cache_dir, r[1]))
        else:
            keep_rows.append(r)
    total = sum(r[2] for r in keep_rows)
    for r in keep_rows:                 # oldest first
        if total <= limit:
            break
        if r[1] == keep:
            continue
        _rm(os.path.join(cache_dir, r[1]))
        total -= r[2]
    return total


def cache_size(cache_dir):
    return _j(ok=True, bytes=_tree_size(cache_dir) if os.path.isdir(cache_dir) else 0)


def clear_cache(cache_dir):
    try:
        for n in os.listdir(cache_dir):
            _rm(os.path.join(cache_dir, n))
    except OSError:
        pass
    return _j(ok=True)


def _enough_space(cache_dir, size):
    try:
        free = shutil.disk_usage(cache_dir).free
    except OSError:
        return None
    if size and free < size + _FREE_MARGIN:
        return (f"not enough free storage: this file needs {kiln.fmt_size(size)} "
                f"and only {kiln.fmt_size(max(free, 0))} is free")
    return None


def cached_extract(path, password, member, cache_dir, size=0, progress=None):
    """Extract one member into the cache, or find it there already. -> JSON {ok, file, cached}"""
    os.makedirs(cache_dir, exist_ok=True)
    try:
        key = cache_key(path, member)
    except OSError as e:
        return _j(ok=False, message=f"can't read the archive: {e.strerror or e}")
    final = os.path.join(cache_dir, key)
    part = final + ".part"
    target = os.path.join(final, *member.split("/"))
    if os.path.isfile(target):
        try:
            os.utime(final, None)       # most recently used
        except OSError:
            pass
        return _j(ok=True, file=target, cached=True, key=key)
    _rm(final)                          # something half-there under this key: start clean
    trim_cache(cache_dir, keep=key)     # make room first (also clears old unfinished copies)
    low = _enough_space(cache_dir, size)
    if low:
        return _j(ok=False, message=low)
    try:
        res = json.loads(extract(path, password, json.dumps([{"p": member, "dir": False}]),
                                 part, True, 1, progress))
        if res.get("ok") and not os.path.isfile(os.path.join(part, *member.split("/"))):
            res = {"ok": False, "message": "the file wasn't found in the archive"}
        if res.get("ok"):
            os.replace(part, final)
            res["file"] = target
            res["cached"] = False
            res["key"] = key
        return json.dumps(res, ensure_ascii=False)
    except bridge.Cancelled:
        return _j(ok=False, cancelled=True, message="Cancelled")
    finally:
        _rm(part)                       # nothing is left behind unless it finished


def preview(path, password, member, cache_dir, size=0, progress=None):
    """Extract one file into the cache for viewing (reused on later opens). -> {ok, file}"""
    res = cached_extract(path, password, member, cache_dir, size, progress)
    try:
        r = json.loads(res)
        if r.get("ok") and not r.get("cached"):        # a new file may have pushed us over the limit
            trim_cache(cache_dir, keep=r.get("key"))
    except Exception:
        pass
    return res


def _delete_sources(packed, items, dest):
    """Remove the originals that are now inside a finished archive, then any folder this left
    empty. Only files kiln reports as packed are touched, never the output itself."""
    out = os.path.abspath(dest)
    inside = out + os.sep
    deleted = failed = 0
    for f in packed:
        a = os.path.abspath(f)
        if a == out or a.startswith(inside) or not os.path.isfile(a) or os.path.islink(a):
            continue
        try:
            os.remove(a)
            deleted += 1
        except OSError:
            failed += 1
    for it in items:                                  # tidy folders the packing emptied
        if os.path.isdir(it) and not os.path.islink(it):
            for dp, _dn, _fn in os.walk(it, topdown=False):
                a = os.path.abspath(dp)
                if a == out or a.startswith(inside) or out.startswith(a + os.sep):
                    continue                          # the output lives here
                try:
                    os.rmdir(dp)
                except OSError:
                    pass
    return deleted, failed


def pack(items_json, dest, encrypt, each, store, overwrite, password, delete_src, progress):
    items = json.loads(items_json)
    try:
        files = kiln.collect(items)
    except kiln.KilnError as e:
        return _j(ok=False, message=str(e))
    if not files:
        return _j(ok=False, message="there is nothing to pack")
    existed = os.path.lexists(dest)
    packed = None
    res = _rust("pack", {"items": items, "dest": dest, "encrypt": bool(encrypt), "store": bool(store),
                         "overwrite": bool(overwrite), "password": password or "", "each": bool(each),
                         "jobs": bridge.JOBS if bridge.JOBS > 1 else 0}, progress)
    if res is not None:
        out = json.loads(res)
        packed = out.pop("packed", None)
        keep = [type("P", (), {"packed": packed or []})()] if out.get("ok") else []
        res = _j(**out)
    else:
        args = ["pack"] + (["-e"] if encrypt else []) + (["-s"] if encrypt and store else []) \
            + (["-i"] if each else []) + (["-f"] if overwrite else []) + ["--", dest] + items
        keep = []
        res = _run(args, password, len(files), progress, not each,
                   sum(f["size"] for f in files), each, keep)
    out = json.loads(res)
    if not out.get("ok") and not existed:      # don't leave half-written output
        for leftover in (dest, dest + ".tmp"):
            if os.path.isdir(leftover):
                shutil.rmtree(leftover, ignore_errors=True)
            elif os.path.lexists(leftover):
                try:
                    os.remove(leftover)
                except OSError:
                    pass
    elif out.get("ok") and delete_src and keep and os.path.lexists(dest):
        # only after the whole job succeeded, and only files kiln says are inside an archive
        # (anything skipped, unreadable or left over because its archive already existed stays)
        deleted, failed = _delete_sources(keep[0].packed, items, dest)
        out["deleted"], out["delete_failed"] = deleted, failed
        res = _j(**out)
    return res


# ---------------------------------------------------------------- plain file operations
# Copy, move and delete for ordinary files and folders (not archives). Policies for a name that
# already exists at the destination: "replace", "skip", or "keep" (store the new one as "name (1)").

_CHUNK = 1 << 20


class _Job:
    def __init__(self, progress, total):
        self.progress, self.total = progress, total
        self.done = self.skipped = 0
        self.dirs = 0                   # folders created / moved (files are counted in done)
        self.errors = []
        self.made = []                  # what a copy created (files, then folders), to undo on cancel
        self.last = 0.0

    def check(self):
        if self.progress is not None and self.progress.cancelled():
            raise bridge.Cancelled()

    def tick(self, name, force=False):
        if self.progress is None:
            return
        now = time.time()
        if not force and now - self.last < 0.05:
            return
        self.last = now
        frac = min(self.done / self.total, 1.0) if self.total else -1
        self.progress.event(_j(frac=frac, done=self.done, total=self.total, file=name))


def _count(paths):
    n = 0
    for p in paths:
        if os.path.isdir(p) and not os.path.islink(p):
            for _r, _d, files in os.walk(p):
                n += max(len(files), 0)
        else:
            n += 1
    return n


def _count_dirs(path):
    """Folders in a tree, including the top one."""
    n = 1
    for _r, dirs, _f in os.walk(path):
        n += len(dirs)
    return n


def _unique(path):
    """'a/b.txt' -> 'a/b (1).txt' (first free number)."""
    root, ext = os.path.splitext(path)
    if os.path.isdir(path) and not ext:
        pass
    i = 1
    while True:
        cand = f"{root} ({i}){ext}"
        if not os.path.lexists(cand):
            return cand
        i += 1


def _inside(child, parent):
    c, p = os.path.realpath(child), os.path.realpath(parent)
    return c == p or c.startswith(p.rstrip(os.sep) + os.sep)


def fs_plan(srcs_json, dest, move):
    """Check a copy/move before running it. -> {ok, conflicts: [names], error}"""
    srcs = json.loads(srcs_json)
    if not os.path.isdir(dest):
        return _j(ok=False, error="the destination folder doesn't exist")
    conflicts = []
    for s in srcs:
        if not os.path.lexists(s):
            return _j(ok=False, error=f"{os.path.basename(s)} no longer exists")
        if os.path.isdir(s) and _inside(dest, s):
            return _j(ok=False, error=f"can't put \"{os.path.basename(s)}\" inside itself")
        target = os.path.join(dest, os.path.basename(s))
        same = os.path.abspath(os.path.dirname(s)) == os.path.abspath(dest)
        if same and move:
            return _j(ok=False, error="the items are already in this folder")
        if same:
            continue                    # copying into the same folder always makes "name (1)"
        if os.path.lexists(target):
            conflicts.append(os.path.basename(s))
    return _j(ok=True, conflicts=conflicts)


def _copy_file(src, dst, job):
    tmp = dst + ".kilntmp"
    try:
        with open(src, "rb") as fi, open(tmp, "wb") as fo:
            while True:
                job.check()
                buf = fi.read(_CHUNK)
                if not buf:
                    break
                fo.write(buf)
        try:
            shutil.copystat(src, tmp)
        except OSError:
            pass
        os.replace(tmp, dst)
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise


def _put(src, dst, move, policy, job):
    """Copy or move one file or folder src to the exact path dst."""
    job.check()
    name = os.path.basename(src)
    if os.path.islink(src) or not os.path.isdir(src):
        if os.path.lexists(dst):
            if os.path.isdir(dst) and not os.path.islink(dst):
                if policy == "replace":
                    shutil.rmtree(dst)
                elif policy == "skip":
                    job.skipped += 1
                    job.done += 1
                    return
                else:
                    dst = _unique(dst)
            elif policy == "skip":
                job.skipped += 1
                job.done += 1
                return
            elif policy == "keep":
                dst = _unique(dst)
        try:
            if move:
                try:
                    os.replace(src, dst)
                except OSError:
                    _copy_file(src, dst, job)
                    os.remove(src)
            elif os.path.islink(src):
                fresh = not os.path.lexists(dst)
                os.symlink(os.readlink(src), dst)
                if fresh:
                    job.made.append(dst)
            else:
                fresh = not os.path.lexists(dst)
                _copy_file(src, dst, job)
                if fresh:
                    job.made.append(dst)
        except bridge.Cancelled:
            raise
        except OSError as e:
            job.errors.append(f"{name}: {e.strerror or e}")
        job.done += 1
        job.tick(name)
        return
    # a folder
    if os.path.lexists(dst) and not os.path.isdir(dst):
        if policy == "replace":
            os.remove(dst)
        elif policy == "skip":
            job.skipped += _count([src])
            job.done += _count([src])
            return
        else:
            dst = _unique(dst)
    if move and not os.path.lexists(dst):
        try:
            n = _count([src])
            ndirs = _count_dirs(src)
            os.rename(src, dst)         # same filesystem: instant
            job.dirs += ndirs
            job.done += n
            job.tick(name, True)
            return
        except OSError:
            pass
    if not os.path.isdir(dst):
        job.made.append(dst)
    os.makedirs(dst, exist_ok=True)
    job.dirs += 1
    for child in sorted(os.listdir(src)):
        _put(os.path.join(src, child), os.path.join(dst, child), move, policy, job)
    if move:
        try:
            os.rmdir(src)               # only succeeds when nothing was skipped
        except OSError:
            pass
    try:
        shutil.copystat(src, dst)
    except OSError:
        pass


def fs_transfer(srcs_json, dest, move, policy, progress):
    srcs = json.loads(srcs_json)
    pl = json.loads(fs_plan(srcs_json, dest, move))
    if not pl["ok"]:
        return _j(ok=False, message=pl["error"])
    job = _Job(progress, _count(srcs))
    try:
        for s in srcs:
            name = os.path.basename(s)
            target = os.path.join(dest, name)
            same = os.path.abspath(os.path.dirname(s)) == os.path.abspath(dest)
            if same:                    # duplicate in place
                target = _unique(target)
            _put(s, target, move, policy, job)
    except bridge.Cancelled:
        if not move:                    # undo the copy: what was already copied would be clutter
            for p in reversed(job.made):
                try:
                    if os.path.isdir(p) and not os.path.islink(p):
                        shutil.rmtree(p, ignore_errors=True)
                    else:
                        os.remove(p)
                except OSError:
                    pass
        return _j(ok=False, cancelled=True, message="Cancelled")
    verb = "Moved" if move else "Copied"
    files = job.done - job.skipped
    parts = []
    if files > 0 or job.dirs == 0:
        parts.append(f"{files} file" + ("" if files == 1 else "s"))
    if job.dirs:
        parts.append(f"{job.dirs} folder" + ("" if job.dirs == 1 else "s"))
    msg = f"{verb} " + " and ".join(parts)
    if job.skipped:
        msg += f", skipped {job.skipped}"
    if job.errors:
        msg += f"; {len(job.errors)} failed:\n" + "\n".join(job.errors[:5])
    job.tick("", True)
    return _j(ok=not job.errors, message=msg, done=job.done, skipped=job.skipped, dirs=job.dirs)


def fs_delete(paths_json, progress):
    paths = json.loads(paths_json)
    job = _Job(progress, _count(paths))
    try:
        for p in paths:
            job.check()
            try:
                if os.path.isdir(p) and not os.path.islink(p):
                    for root, dirs, files in os.walk(p, topdown=False):
                        for f in files:
                            job.check()
                            os.remove(os.path.join(root, f))
                            job.done += 1
                            job.tick(f)
                        for d in dirs:
                            dp = os.path.join(root, d)
                            if os.path.islink(dp):
                                os.remove(dp)
                            else:
                                os.rmdir(dp)
                    os.rmdir(p)
                else:
                    os.remove(p)
                    job.done += 1
                    job.tick(os.path.basename(p))
            except OSError as e:
                job.errors.append(f"{os.path.basename(p)}: {e.strerror or e}")
    except bridge.Cancelled:
        return _j(ok=False, cancelled=True, message="Cancelled")
    msg = f"Deleted {job.done} file(s)"
    if job.errors:
        msg += f"; {len(job.errors)} failed:\n" + "\n".join(job.errors[:5])
    return _j(ok=not job.errors, message=msg)


# ---------------------------------------------------------------- settings: jobs and tools

def set_jobs(n):
    bridge.set_jobs(n)
    return _j(ok=True, jobs=bridge.JOBS)


class _ToolSink(_Sink):
    """Sink for the Tools screen. The self-test reports one line per finished check, so those
    drive a real progress bar; benchmark and WAV info just show their latest line."""
    _CHECK = re.compile(r"^(PASS|FAIL)\s+(.*?)\s+\(\d+(?:\.\d+)?s\)")

    def __init__(self, progress, name, total):
        _Sink.__init__(self, progress, total, False)
        self.name, self.failed = name, 0

    def _emit(self, force=False):
        if self.progress is None:
            return
        now = time.time()
        if not force and now - self.last < 0.05:
            return
        self.last = now
        # a moving bar until the first check finishes (and always when the count is unknown)
        frac = min(self.done / self.total, 1.0) if self.total and self.done else -1
        self.progress.event(_j(frac=frac, done=self.done, total=self.total, file=self.file))

    def line(self, text):
        self._check()
        self.lines.append(text)
        if self.name == "selftest":
            m = self._CHECK.match(text.strip())
            if m:
                self.done += 1
                self.failed += m.group(1) == "FAIL"
                self.file = (f"{self.done} of {self.total} checks done" if self.total
                             else f"{self.done} checks done") + " \u00b7 " + m.group(2)
                self._emit(True)
        elif text.strip():
            self.file = text.strip()
            self._emit()


def _selftest_total():
    """How many checks the running script's self-test has (0 if it can't be told)."""
    try:
        import inspect
        src = inspect.getsource(kiln.run_selftest)
        return len(re.findall(r'^\s+check\("', src, re.M))
    except Exception:
        return 0


_BENCH_ROW = re.compile(r"^(\S.*?)\s+(\d+)\s+(\d+(?:\.\d+)?)%\s+(\d+(?:\.\d+)?)s$")
_BENCH_HEAD = re.compile(r"^input:\s*(\d+) file\(s\),\s*(.+)$")
_WAV_HEAD = re.compile(r"^(float|PCM),\s*(\d+)-bit,\s*(\d+) ch,\s*(\d+) Hz,\s*(\d+(?:\.\d+)?) s$")


def _bench_view(lines):
    """The benchmark table as data (None if the script printed something we don't know)."""
    m = _BENCH_HEAD.match(lines[0].strip()) if lines else None
    rows = []
    for ln in lines[1:]:
        r = _BENCH_ROW.match(ln.strip())
        if r:
            rows.append({"name": r.group(1), "size": int(r.group(2)),
                         "ratio": float(r.group(3)), "secs": float(r.group(4))})
    if not m or not rows:
        return None
    return {"type": "bench", "files": int(m.group(1)), "input": m.group(2), "rows": rows}


def _wav_view(lines):
    """The WAV report as data (None for 'not a WAV' and anything we don't recognise)."""
    m = _WAV_HEAD.match(lines[0].strip()) if lines else None
    if not m:
        return None
    v = {"type": "wav", "kind": m.group(1), "bits": int(m.group(2)), "ch": int(m.group(3)),
         "rate": int(m.group(4)), "secs": float(m.group(5)),
         "details": [], "verdict": "", "good": False, "note": "", "filter": ""}
    for ln in lines[1:]:
        t = ln.strip()
        if t.startswith("verdict:"):
            v["verdict"] = t[len("verdict:"):].strip()
            v["good"] = "stores it as integers" in t
        elif t.startswith("filter kiln will use:"):
            v["filter"] = t.split(":", 1)[1].strip()
        elif t.startswith("The integer filter"):
            v["note"] = t
        elif t:
            v["details"].append(t)
    return v


_ST_HEAD = re.compile(r"^kiln\s+(\S+)\s+selftest\s+-\s+Python\s+(\S+),\s*(\d+)\s+core")
_ST_CHECK = re.compile(r"^(PASS|FAIL)\s+(.*?)\s+\((\d+(?:\.\d+)?)s\)(?:\s+-\s+(.*))?$")
_ST_SUM = re.compile(r"^(?:all (\d+) checks passed|(\d+) of (\d+) checks FAILED) in (\d+(?:\.\d+)?)s$")


def _selftest_view(lines):
    """The self-test report as data (None if it isn't in the shape we know)."""
    checks, notes, head, summ = [], [], None, None
    for ln in lines:
        t = ln.strip()
        if not t:
            continue
        m = _ST_HEAD.match(t)
        if m and head is None:
            head = {"version": m.group(1), "python": m.group(2), "cores": int(m.group(3))}
            continue
        m = _ST_CHECK.match(t)
        if m:
            checks.append({"ok": m.group(1) == "PASS", "name": m.group(2),
                           "secs": float(m.group(3)), "msg": (m.group(4) or "").strip()})
            continue
        m = _ST_SUM.match(t)
        if m:
            summ = m
            continue
        notes.append(t)
    if not checks:
        return None
    failed = sum(1 for c in checks if not c["ok"])
    return {"type": "selftest", "head": head, "checks": checks, "notes": notes,
            "total": len(checks), "failed": failed,
            "secs": float(summ.group(4)) if summ else sum(c["secs"] for c in checks),
            "finished": summ is not None}


def tool(name, arg, progress):
    """Self-test, benchmark or WAV info. -> {ok, text}  (or cancelled=True when stopped)"""
    args = [name] + ([arg] if arg else [])
    total = _selftest_total() if name == "selftest" else 0
    sink = _ToolSink(progress, name, total)
    if progress is not None:
        sink.progress.event(_j(frac=-1, done=0, total=total,
                               file="Starting..." if name == "selftest" else "Working..."))
    rc = bridge.run(args, None, sink, hook=sink.on_bytes if progress is not None else None)
    if rc == 130:
        return _j(ok=False, cancelled=True, text="Cancelled")
    text = "\n".join(x.rstrip() for x in sink.lines)
    view = None
    if rc == 0 and name == "bench":
        view = _bench_view(sink.lines)
    elif rc == 0 and name == "wavcheck":
        view = _wav_view(sink.lines)
    elif name == "selftest":                       # a failed run still has checks worth showing
        view = _selftest_view(sink.lines)
    return _j(ok=rc == 0, text=text or ("Done" if rc == 0 else "The tool failed"), view=view)
