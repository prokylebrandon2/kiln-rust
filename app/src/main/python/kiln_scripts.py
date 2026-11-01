"""Main and fallback kiln scripts supplied by the user (kept in the app's private storage).

The built-in kiln_v14 can't be removed. A custom main script replaces it at start-up (so the
whole app uses it); a fallback script is used only for archives the main one can't read."""
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
    return d, os.path.join(d, "main.py"), os.path.join(d, "fallback.py"), os.path.join(d, "scripts.json")


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
        with open(_paths(files_dir)[3]) as f:
            return json.load(f)
    except Exception:
        return {}


def _save_meta(files_dir, m):
    d = _paths(files_dir)[0]
    os.makedirs(d, exist_ok=True)
    with open(_paths(files_dir)[3], "w") as f:
        json.dump(m, f)


def _check_main(mod):
    missing = [n for n in REQUIRED if not hasattr(mod, n)]
    if missing:
        raise ValueError("it doesn't provide: " + ", ".join(missing))


def status(files_dir):
    m = _meta(files_dir)
    return json.dumps({"ok": True, "main": m.get("main", ""), "main_version": m.get("main_version", ""),
                       "fallback": m.get("fallback", ""), "fallback_version": m.get("fallback_version", ""),
                       "note": _note}, ensure_ascii=False)


def install(files_dir, role, src, name):
    """role: 'main' or 'fallback'. Checks the script loads, then keeps a private copy."""
    d, main_p, fb_p, _ = _paths(files_dir)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, "candidate.py")
    shutil.copyfile(src, tmp)
    try:
        mod = _load(tmp, "kiln_candidate")
        if role == "main":
            _check_main(mod)
        elif not hasattr(mod, "main"):
            raise ValueError("it has no main() function")
    except BaseException as e:
        os.remove(tmp)
        return json.dumps({"ok": False, "message": f"This script can't be used: {e}"})
    finally:
        sys.modules.pop("kiln_candidate", None)
    ver = str(getattr(mod, "VERSION", "?"))
    os.replace(tmp, main_p if role == "main" else fb_p)
    m = _meta(files_dir)
    m[role] = name
    m[role + "_version"] = ver
    _save_meta(files_dir, m)
    return json.dumps({"ok": True, "version": ver, "restart": role == "main"})


def remove(files_dir, role):
    _, main_p, fb_p, _ = _paths(files_dir)
    p = main_p if role == "main" else fb_p
    if os.path.exists(p):
        os.remove(p)
    m = _meta(files_dir)
    m.pop(role, None)
    m.pop(role + "_version", None)
    _save_meta(files_dir, m)
    return json.dumps({"ok": True, "restart": role == "main"})


def boot(files_dir):
    """Called before the app layer is imported. Puts a custom main script in place of kiln_v14."""
    global _note
    if not files_dir:
        return
    _, main_p, _, _ = _paths(files_dir)
    if os.path.isfile(main_p):
        try:
            mod = _load(main_p, "kiln_custom_main")
            _check_main(mod)
            sys.modules["kiln_v14"] = mod
        except BaseException as e:
            sys.modules.pop("kiln_custom_main", None)
            _note = f"Custom main script not used ({e}). Built-in v14 is running."


def load_fallback(files_dir):
    global _note
    if not files_dir:
        return None
    _, _, fb_p, _ = _paths(files_dir)
    if os.path.isfile(fb_p):
        try:
            return _load(fb_p, "kiln_fallback")
        except BaseException as e:
            _note = (_note + " " if _note else "") + f"Fallback script not loaded ({e})."
    return None


def note():
    return _note
