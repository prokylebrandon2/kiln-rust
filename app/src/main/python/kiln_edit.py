"""Staged editing inside a Kiln archive, driven by kiln's own Shell.

Nothing touches the archive until save(). Each session is one open archive; the Java side keeps
the id. All functions return JSON strings like the rest of kiln_app."""
import json
import os
import shutil
import tempfile
import time
import uuid

import kiln_v14 as kiln
import kiln_bridge as bridge
import kiln_app as app

_sessions = {}


class _S:
    def __init__(self, sh, pw, tmp):
        self.sh, self.pw, self.tmp = sh, pw, tmp


def _j(**kw):
    return json.dumps(kw, ensure_ascii=False)


def _get(sid):
    s = _sessions.get(sid)
    if s is None:
        raise kiln.KilnError("this editing session has ended; reopen the archive")
    return s


def _view(s, **extra):
    sh = s.sh
    ents = [{"p": e["rel"], "s": e["size"], "m": e["mtime"], "k": sh.mark(e).strip()}
            for e in sh.entries]
    return _j(ok=True, entries=ents, dirty=bool(sh.dirty), **extra)


def _norm(p):
    return "/".join(x for x in p.split("/") if x not in ("", "."))


def _under(rel, m):
    return rel == m["p"] or (m.get("dir") and rel.startswith(m["p"] + "/"))


def _guard(fn):
    def w(*a, **k):
        try:
            return fn(*a, **k)
        except kiln.KilnError as e:
            return _j(ok=False, message=str(e))
        except Exception as e:
            return _j(ok=False, message=f"{type(e).__name__}: {e}")
    return w


# ------------------------------------------------------------------ session

@_guard
def begin(path, password):
    """Start a staged-edit session. -> {ok, sid, entries, dirty, encrypted, is_set}"""
    if path in app._legacy:
        raise kiln.KilnError("this archive was opened with the fallback script, which can't edit")
    sh = kiln.Shell()
    sh.jobs = bridge.JOBS             # 1 unless changed in Settings
    code, _ = bridge.call(lambda: sh.load(path, password or None), _Quiet())
    if code != 0:
        raise kiln.KilnError("couldn't open the archive for editing")
    if sh.set_dir and sh.set_v1:
        raise kiln.KilnError("this set's index is old (no sizes or dates), so it can't be "
                             "edited yet: run the same `kiln pack -esr ...` command once")
    sid = uuid.uuid4().hex
    s = _sessions[sid] = _S(sh, password or None, tempfile.mkdtemp(prefix="kiln-edit-"))
    return _view(s, sid=sid, encrypted=bool(sh.encrypt), is_set=bool(sh.set_dir))


class _Quiet:
    def line(self, t):
        pass


@_guard
def state(sid):
    return _view(_get(sid))


@_guard
def close(sid):
    s = _sessions.pop(sid, None)
    if s:
        try:
            if s.sh.f:
                s.sh.f.close()
        except Exception:
            pass
        shutil.rmtree(s.tmp, ignore_errors=True)
    return _j(ok=True)


@_guard
def discard(sid):
    s = _get(sid)
    s.sh.reset_entries()
    s.sh.pw = s.sh.load_pw
    return _view(s)


# ------------------------------------------------------------------ edits

@_guard
def add(sid, paths_json, dest):
    """Add phone files/folders into archive folder `dest` ('' = root)."""
    s = _get(sid)
    sh = s.sh
    paths = json.loads(paths_json)
    sh.cwd = _norm(dest)
    found = kiln.collect(paths, {sh.path, sh.path + ".tmp"})
    if sh.set_dir:
        sd = sh.set_dir + os.sep
        found = [e for e in found if not os.path.abspath(e["full"]).startswith(sd)]
    pre = sh.cwd + "/" if sh.cwd else ""
    names = {pre + e["rel"] for e in found}
    replaced = sum(1 for e in sh.entries if e["rel"] in names)
    sh.entries = [e for e in sh.entries if e["rel"] not in names]
    for e in found:
        rel = pre + e["rel"]
        sh.entries.append({"rel": rel, "size": e["size"], "crc": None, "mtime": e["mtime"],
                           "mode": e["mode"], "filt": 0, "old": None, "full": e["full"],
                           **sh.info_of(rel)})
    sh.dirty = sh.dirty or bool(found)
    return _view(s, added=len(found), replaced=replaced)


@_guard
def remove(sid, members_json):
    s = _get(sid)
    members = json.loads(members_json)
    before = len(s.sh.entries)
    s.sh.entries = [e for e in s.sh.entries if not any(_under(e["rel"], m) for m in members)]
    n = before - len(s.sh.entries)
    if n:
        s.sh.dirty = True
    return _view(s, removed=n)


def _targets(s, members, dest, rename_to):
    """-> [(entry, new_rel)] for moving/copying `members` into folder `dest`."""
    dest = _norm(dest)
    out = []
    for m in members:
        base = m["p"].rsplit("/", 1)[-1]
        top = m["p"].rsplit("/", 1)[0] if "/" in m["p"] else ""
        name = rename_to or base
        if "/" in name or name in ("", ".", ".."):
            raise kiln.KilnError("a name can't contain / or be empty")
        newroot = (dest + "/" if dest else "") + name
        if m.get("dir") and (newroot + "/").startswith(m["p"] + "/"):
            raise kiln.KilnError(f"can't put {base} inside itself")
        for e in s.sh.entries:
            if _under(e["rel"], m):
                out.append((e, newroot + e["rel"][len(m["p"]):]))
    if not out:
        raise kiln.KilnError("nothing matched")
    news = [n for _, n in out]
    if len(set(news)) != len(news):
        raise kiln.KilnError("two files would end up with the same name")
    return out


def _clashes(s, pairs, moving):
    moved = {id(e) for e, _ in pairs} if moving else set()
    stay = {e["rel"] for e in s.sh.entries if id(e) not in moved}
    return sorted({n for _, n in pairs if n in stay})


def _copy_source(s, e):
    """A file on disk holding entry e's bytes (so a copy can be staged as a new file)."""
    if e.get("full"):
        return e["full"]
    old = s.sh.old_table[e["old"]]["rel"]
    d = os.path.join(s.tmp, uuid.uuid4().hex)
    res = json.loads(app.extract(s.sh.path, s.pw, json.dumps([{"p": old, "dir": False}]),
                                 d, True, 1, None))
    if not res.get("ok"):
        raise kiln.KilnError(res.get("message") or "couldn't read a file to copy it")
    return os.path.join(d, *old.split("/"))


@_guard
def transfer(sid, members_json, dest, move, policy, rename_to=None):
    """Move or copy items within the archive.
    policy: 'ask' (report clashes, change nothing), 'replace', 'skip', 'keep'."""
    s = _get(sid)
    sh = s.sh
    members = json.loads(members_json)
    pairs = _targets(s, members, dest, rename_to)
    if move:
        pairs = [(e, n) for e, n in pairs if e["rel"] != n]
        if not pairs:
            return _view(s, done=0)
    clash = _clashes(s, pairs, move)
    if clash:
        if policy == "ask":
            return _j(ok=False, conflicts=clash[:20], n_conflicts=len(clash))
        if policy == "skip":
            pairs = [(e, n) for e, n in pairs if n not in set(clash)]
        elif policy == "keep":
            taken = {e["rel"] for e in sh.entries} | {n for _, n in pairs}
            fixed = []
            for e, n in pairs:
                if n in clash:
                    stem, ext = os.path.splitext(n)
                    i = 1
                    while f"{stem} ({i}){ext}" in taken:
                        i += 1
                    n = f"{stem} ({i}){ext}"
                    taken.add(n)
                fixed.append((e, n))
            pairs = fixed
        elif policy == "replace":
            gone = set(clash)
            moved = {id(e) for e, _ in pairs} if move else set()
            sh.entries = [e for e in sh.entries if e["rel"] not in gone or id(e) in moved]
    if not pairs:
        return _view(s, done=0)
    if move:
        for e, n in pairs:
            e["rel"] = n
            e.update(sh.info_of(n))
    elif sh.set_dir:                  # kiln copies inside a set natively (no data is read now)
        for e, n in pairs:
            if e["old"] is not None and sh.old_table[e["old"]].get("missing"):
                raise kiln.KilnError(f"{e['rel']}: its archive is missing, can't copy")
            sh.entries.append(dict(e, rel=n, old=None,
                                   copy_of=e["old"] if e["old"] is not None else e.get("copy_of"),
                                   **sh.info_of(n)))
    else:                             # plain archive: stage the bytes as a new file
        staged = []
        for e, n in pairs:
            src = _copy_source(s, e)
            staged.append(dict(e, rel=n, old=None, full=src, crc=None, filt=0,
                               **sh.info_of(n)))
        sh.entries.extend(staged)
    sh.dirty = True
    return _view(s, done=len(pairs))


@_guard
def set_file(sid, member, src_file):
    """Replace one file's contents (text edit, or an edited copy brought back from another app)."""
    s = _get(sid)
    sh = s.sh
    keep = os.path.join(s.tmp, uuid.uuid4().hex)
    shutil.copyfile(src_file, keep)
    st = os.stat(keep)
    old = next((e for e in sh.entries if e["rel"] == member), None)
    new = {"rel": member, "size": st.st_size, "crc": None, "mtime": int(time.time()),
           "mode": old["mode"] if old else 0o644, "filt": 0, "old": None, "full": keep,
           **sh.info_of(member)}
    sh.entries = [e for e in sh.entries if e["rel"] != member] + [new]
    sh.dirty = True
    return _view(s)


@_guard
def new_text(sid, member):
    """Stage an empty text file (the app then opens it in the editor)."""
    s = _get(sid)
    for e in s.sh.entries:
        if e["rel"] == _norm(member):
            raise kiln.KilnError("that name already exists")
    empty = os.path.join(s.tmp, uuid.uuid4().hex)
    open(empty, "wb").close()
    return set_file(sid, _norm(member), empty)


@_guard
def file_for(sid, member, cache_dir):
    """A private copy of `member`'s current bytes in cache_dir, for viewing/editing."""
    s = _get(sid)
    e = next((x for x in s.sh.entries if x["rel"] == member), None)
    if e is None:
        raise kiln.KilnError("that file is no longer in the archive")
    d = os.path.join(cache_dir, uuid.uuid4().hex)
    os.makedirs(d, exist_ok=True)
    out = os.path.join(d, member.rsplit("/", 1)[-1])
    if e.get("full"):
        shutil.copyfile(e["full"], out)
        return _j(ok=True, file=out)
    old = s.sh.old_table[e["old"]]["rel"]
    res = json.loads(app.extract(s.sh.path, s.pw, json.dumps([{"p": old, "dir": False}]),
                                 d + "/x", True, 1, None))
    if not res.get("ok"):
        return _j(ok=False, message=res.get("message") or "couldn't read the file")
    shutil.move(os.path.join(d, "x", *old.split("/")), out)
    shutil.rmtree(os.path.join(d, "x"), ignore_errors=True)
    return _j(ok=True, file=out)


@_guard
def save(sid, progress):
    """Write the staged changes into the archive (atomic swap through a .tmp file)."""
    s = _get(sid)
    sh = s.sh
    if not sh.dirty:
        return _view(s, message="Nothing to save")
    sink = app._Sink(progress, 0, False)
    code, _ = bridge.call(lambda: sh.write(sh.path), sink, hook=sink.on_bytes)
    if code == 130:
        return _j(ok=False, cancelled=True, message="Cancelled")
    if code != 0:
        return _j(ok=False, message=sink.message(False))
    sh.dirty = False
    return _view(s, message=sink.message(True))
