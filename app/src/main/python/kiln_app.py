"""What the Kiln app needs, as plain functions that return JSON strings.

Everything here is testable without Android. The Java side passes a `progress` object with
event(json) and cancelled(); None is fine too."""
import concurrent.futures
import fnmatch
import glob
import json
import os
import shutil
import time
import uuid

import kiln_v14 as kiln
import kiln_bridge as bridge

_MAGIC = b"KLNA"
_MIN_ARCHIVE = 25          # header + footer: nothing smaller can be an archive


def _j(**kw):
    return json.dumps(kw, ensure_ascii=False)


def version():
    return _j(kiln=getattr(kiln, "VERSION", "?"), helper=bridge.accel_note())


# ---------------------------------------------------------------- browsing

def _sniff(path):
    """-> ('archive', encrypted) | ('old', False) | None, from the first bytes of a file.
    Reads straight from the descriptor: a buffered open() would pull in a whole 8 KB block."""
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return None
    try:
        head = os.read(fd, 5)
    except OSError:
        return None
    finally:
        os.close(fd)
    if head[:4] == _MAGIC and len(head) == 5:
        return "archive", bool(head[4] & kiln.FLAG_ENCRYPTED)
    if head[:4] in (b"KLN9", b"KLN2"):
        return "old", False
    return None


_POOL = concurrent.futures.ThreadPoolExecutor(max_workers=8)
_PARALLEL_FROM = 24         # fewer entries than this: not worth handing out


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
            if st.st_size >= _MIN_ARCHIVE:
                sn = _sniff(e.path)
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
    show_all) every other file. Every entry is looked at on its own, so big folders are
    read by several threads at once (the work is waiting on storage, not on the CPU)."""
    try:
        entries = list(os.scandir(path))
    except OSError as e:
        return _j(ok=False, error=f"can't open {path}: {e.strerror or e}")
    if len(entries) < _PARALLEL_FROM:
        found = [_entry(e, show_all, show_hidden) for e in entries]
    else:
        found = list(_POOL.map(lambda e: _entry(e, show_all, show_hidden), entries))
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


def search(path, query, show_all, show_hidden=False):
    """Every file and folder below `path` whose name contains all the words in `query`
    (case-insensitive). Names come back relative to `path`, so the app can treat them like
    entries of the folder itself. A newer search (or search_cancel) stops this one."""
    _search_gen[0] += 1
    mine = _search_gen[0]
    terms = [t for t in query.casefold().split() if t]
    if not terms:
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
                    hit = all(t in low for t in terms)
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
                        if st.st_size >= _MIN_ARCHIVE:
                            sn = _sniff(e.path)
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

def open_archive(path, password):
    """Table of contents of an archive file or random-name set."""
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
        sn = _sniff(path)
        if sn is None:
            return _j(ok=False, error="this file isn't a Kiln archive")
        if sn[0] == "old":
            return _j(ok=False, error="this archive was made by an older Kiln (format 9 or "
                                       "earlier) that this app can't open")
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
        return _j(ok=False, error=msg, bad_password="wrong password" in msg)
    except Exception as e:                       # corrupt files must never crash the app
        return _j(ok=False, error=f"couldn't read this archive: {e}")


# ---------------------------------------------------------------- running commands

class _Sink:
    """Turns kiln's printed lines and byte counters into progress events and a final message."""
    def __init__(self, progress, total, use_bytes):
        self.progress, self.total, self.use_bytes = progress, total, use_bytes
        self.done, self.file, self.bfrac = 0, "", None
        self.lines, self.last = [], 0.0

    def _check(self):
        if self.progress is not None and self.progress.cancelled():
            raise bridge.Cancelled()

    def _emit(self, force=False):
        if self.progress is None:
            return
        now = time.time()
        if not force and now - self.last < 0.05:
            return
        self.last = now
        if self.use_bytes and self.bfrac is not None:
            frac = self.bfrac
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
            self.file = text.strip().split(": ")[0]
            self._emit(self.done == self.total)

    def on_bytes(self, done, total):
        self._check()
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


def _run(args, password, total, progress, use_bytes):
    sink = _Sink(progress, total, use_bytes)
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
    members = json.loads(members_json)
    args = ["unpack", "-v"] + (["-f"] if overwrite else []) + ["-o", dest, "--", path] \
        + _patterns(members)
    return _run(args, password, total, progress, not os.path.isdir(path))


def test(path, password, progress):
    return _run(["test", path], password, 0, progress, not os.path.isdir(path))


def decrypt_in_place(path, password, total, progress):
    return _run(["unpack", "-v", "-f", "-i", "--", path], password, total, progress, False)


def preview(path, password, member, cache_dir):
    """Extract one file into cache_dir for viewing. -> {"ok", "file"}"""
    dest = os.path.join(cache_dir, uuid.uuid4().hex)
    res = json.loads(extract(path, password, json.dumps([{"p": member, "dir": False}]),
                             dest, True, 1, None))
    if res.get("ok"):
        res["file"] = os.path.join(dest, *member.split("/"))
    return json.dumps(res, ensure_ascii=False)


def pack(items_json, dest, encrypt, each, store, overwrite, password, progress):
    items = json.loads(items_json)
    try:
        files = kiln.collect(items)
    except kiln.KilnError as e:
        return _j(ok=False, message=str(e))
    if not files:
        return _j(ok=False, message="there is nothing to pack")
    existed = os.path.lexists(dest)
    args = ["pack"] + (["-e"] if encrypt else []) + (["-s"] if encrypt and store else []) \
        + (["-i"] if each else []) + (["-f"] if overwrite else []) + ["--", dest] + items
    res = _run(args, password, len(files), progress, not each)
    if not json.loads(res).get("ok") and not existed:      # don't leave half-written output
        for leftover in (dest, dest + ".tmp"):
            if os.path.isdir(leftover):
                shutil.rmtree(leftover, ignore_errors=True)
            elif os.path.lexists(leftover):
                try:
                    os.remove(leftover)
                except OSError:
                    pass
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
                os.symlink(os.readlink(src), dst)
            else:
                _copy_file(src, dst, job)
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
