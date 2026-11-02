"""A custom main kiln script supplied by the user (kept in the app's private storage).

The built-in kiln_v14 can't be removed. A custom main script replaces it at start-up (so the
whole app uses it)."""
import importlib.util
import json
import os
import shutil
import sys

# What the app calls on the main script. A replacement must have all of these.
REQUIRED = ['ACCEL_NOTE', 'FLAG_ENCRYPTED', 'INDEX_NAME', 'KilnError', 'MIN_PASSWORD', 'Shell', '_A', 'collect', 'get_password', 'main', 'open_archive', 'read_index_meta', 'require_accel', 'start_mp3_ahead', 'stop_mp3_ahead']

_note = ""


def _paths(files_dir):
    d = os.path.join(files_dir, "scripts")
    return d, os.path.join(d, "main.py"), os.path.join(d, "scripts.json")


def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    try:
        spec.loader.exec_module(mod)
    except BaseException:
        sys.modules.pop(name, None)
        raise
    return mod


def _meta(files_dir):
    try:
        with open(_paths(files_dir)[2]) as f:
            return json.load(f)
    except Exception:
        return {}


def _save_meta(files_dir, m):
    d = _paths(files_dir)[0]
    os.makedirs(d, exist_ok=True)
    with open(_paths(files_dir)[2], "w") as f:
        json.dump(m, f)


# ---- signatures: the first bytes of the files a script handles (e.g. b"KLNA")

MAX_MAGIC = 16
DEFAULT_MAGIC = b"KLNA"
_main_loaded = False        # a custom main script is the one running


def parse_magic(text):
    """What the user typed -> bytes. Plain text is taken as is; 0x4B4C4E41 or hex:4B 4C 4E 41 is hex."""
    t = (text or "").strip()
    low = t.lower()
    if low.startswith("0x") or low.startswith("hex:"):
        h = "".join(t[2:].split()) if low.startswith("0x") else "".join(t[4:].split())
        try:
            b = bytes.fromhex(h)
        except ValueError:
            raise ValueError("that isn't valid hex (use pairs of digits, like 0x4B4C4E41)")
    else:
        b = t.encode("utf-8")
    if not b:
        raise ValueError("the signature is empty")
    if len(b) > MAX_MAGIC:
        raise ValueError(f"the signature can be at most {MAX_MAGIC} bytes")
    return b


def show_magic(b):
    """bytes -> what the person sees: the text if it's printable, otherwise 0x..."""
    if all(32 <= c < 127 for c in b):
        return b.decode("ascii")
    return "0x" + b.hex().upper()


def _detect(mod):
    """The script's own MAGIC constant, if it has a usable one."""
    m = getattr(mod, "MAGIC", None)
    if isinstance(m, (bytes, bytearray)) and 1 <= len(m) <= MAX_MAGIC:
        return bytes(m)
    return None


def _meta_magic(m, role):
    h = m.get(role + "_magic", "")
    try:
        return bytes.fromhex(h) if h else None
    except ValueError:
        return None


def signatures(files_dir):
    """-> the main script's signature, for the file browser."""
    m = _meta(files_dir)
    main = DEFAULT_MAGIC
    if _main_loaded:
        main = _meta_magic(m, "main")
        if main is None:
            mod = sys.modules.get("kiln_v14")
            main = (_detect(mod) if mod else None) or DEFAULT_MAGIC
    return main


def _check_main(mod):
    missing = [n for n in REQUIRED if not hasattr(mod, n)]
    if missing:
        raise ValueError("it doesn't provide: " + ", ".join(missing))


def status(files_dir):
    m = _meta(files_dir)
    main_sig = signatures(files_dir)
    return json.dumps({"ok": True, "main": m.get("main", ""), "main_version": m.get("main_version", ""),
                       "main_magic": show_magic(_meta_magic(m, "main") or main_sig) if m.get("main") else show_magic(DEFAULT_MAGIC),
                       "note": _note}, ensure_ascii=False)


def install(files_dir, role, src, name, magic=""):
    """Checks the script loads, then keeps a private copy (role is always 'main').
    magic: what the person typed for its signature; blank = read the script's own MAGIC. If the
    script has none, nothing is installed and need_magic is returned so the app can ask."""
    d, main_p, _ = _paths(files_dir)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, "candidate.py")
    shutil.copyfile(src, tmp)
    try:
        mod = _load(tmp, "kiln_candidate")
        _check_main(mod)
        if magic.strip():
            try:
                sig = parse_magic(magic)
            except ValueError as e:      # a typo: keep the script and ask again
                os.remove(tmp)
                return json.dumps({"ok": False, "need_magic": True, "message": f"Signature: {e}"})
        else:
            sig = _detect(mod)
            if sig is None:
                os.remove(tmp)
                return json.dumps({"ok": False, "need_magic": True,
                                   "message": "This script doesn't say which files it handles."})
    except BaseException as e:
        if os.path.exists(tmp):
            os.remove(tmp)
        return json.dumps({"ok": False, "message": f"This script can't be used: {e}"})
    finally:
        sys.modules.pop("kiln_candidate", None)
    ver = str(getattr(mod, "VERSION", "?"))
    os.replace(tmp, main_p)
    m = _meta(files_dir)
    m["main"] = name
    m["main_version"] = ver
    m["main_magic"] = sig.hex()
    _save_meta(files_dir, m)
    return json.dumps({"ok": True, "version": ver, "magic": show_magic(sig), "restart": True})


def set_magic(files_dir, role, text):
    """Change the signature of the installed main script."""
    try:
        sig = parse_magic(text)
    except ValueError as e:
        return json.dumps({"ok": False, "message": f"Signature: {e}"})
    m = _meta(files_dir)
    m["main_magic"] = sig.hex()
    _save_meta(files_dir, m)
    return json.dumps({"ok": True, "magic": show_magic(sig), "restart": True})


def remove(files_dir, role):
    _, main_p, _ = _paths(files_dir)
    if os.path.exists(main_p):
        os.remove(main_p)
    m = _meta(files_dir)
    for k in ("main", "main_version", "main_magic"):
        m.pop(k, None)
    _save_meta(files_dir, m)
    return json.dumps({"ok": True, "restart": True})


def _drop_old_fallback(files_dir):
    """The fallback script feature was removed: delete any copy an earlier version kept."""
    d, _, _ = _paths(files_dir)
    try:
        os.remove(os.path.join(d, "fallback.py"))
    except OSError:
        pass
    m = _meta(files_dir)
    if any(k.startswith("fallback") for k in m):
        for k in [k for k in m if k.startswith("fallback")]:
            m.pop(k)
        _save_meta(files_dir, m)


def boot(files_dir):
    """Called before the app layer is imported. Puts a custom main script in place of kiln_v14."""
    global _note, _main_loaded
    if not files_dir:
        return
    _drop_old_fallback(files_dir)
    _, main_p, _ = _paths(files_dir)
    if os.path.isfile(main_p):
        try:
            mod = _load(main_p, "kiln_custom_main")
            _check_main(mod)
            sys.modules["kiln_v14"] = mod
            _main_loaded = True
        except BaseException as e:
            sys.modules.pop("kiln_custom_main", None)
            _note = f"Custom main script not used ({e}). Built-in v14 is running."


def note():
    return _note
