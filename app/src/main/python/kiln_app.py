"""What the Kiln app needs, as plain functions that return JSON strings.

Everything here is testable without Android. The Java side passes a `progress` object with
event(json) and cancelled(); None is fine too."""
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
    """-> ('archive', encrypted) | ('old', False) | None, from the first bytes of a file."""
    try:
        with open(path, "rb") as f:
            head = f.read(5)
    except OSError:
        return None
    if head[:4] == _MAGIC and len(head) == 5:
        return "archive", bool(head[4] & kiln.FLAG_ENCRYPTED)
    if head[:4] in (b"KLN9", b"KLN2"):
        return "old", False
    return None


def scan(path, show_all):
    """One folder for the file manager: folders, kiln archives, random-name sets, and (when
    show_all) every other file."""
    try:
        entries = list(os.scandir(path))
    except OSError as e:
        return _j(ok=False, error=f"can't open {path}: {e.strerror or e}")
    items = []
    for e in entries:
        try:
            if e.is_dir():
                st = e.stat()
                is_set = os.path.isfile(os.path.join(e.path, kiln.INDEX_NAME))
                if e.name.startswith(".") and not show_all:
                    continue
                items.append({"n": e.name, "k": "set" if is_set else "dir",
                              "s": 0, "m": int(st.st_mtime)})
            elif e.is_file():
                st = e.stat()
                kind, enc = "file", False
                if st.st_size >= _MIN_ARCHIVE:
                    sn = _sniff(e.path)
                    if sn:
                        kind, enc = sn
                if kind == "file" and not show_all:
                    continue
                items.append({"n": e.name, "k": kind, "s": st.st_size,
                              "m": int(st.st_mtime), "e": enc})
        except OSError:
            continue
    order = {"dir": 0, "set": 0}
    items.sort(key=lambda i: (order.get(i["k"], 1), i["n"].casefold()))
    return _j(ok=True, path=path, items=items)


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
