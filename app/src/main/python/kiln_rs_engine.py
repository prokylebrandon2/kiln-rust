"""The Rust engine: libkiln_rs.so is bundled in the APK (see kiln-rs/include/kiln_rs.h).

It reads archives (open / test / extract); packing and editing are still Python until the Rust
packer is wired in. If the library can't be loaded, or says "unsupported", the caller uses Python.
Loaded with ctypes (like the C helper), so no JNI is needed."""
import ctypes
import json
import os

_lib = None
_path = None
_note = ""

_CB = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p)


def _bind(path):
    lib = ctypes.CDLL(path)
    lib.kiln_rs_call.argtypes = [ctypes.c_char_p, ctypes.c_char_p, _CB, ctypes.c_void_p]
    lib.kiln_rs_call.restype = ctypes.c_void_p            # not c_char_p: we must free the pointer ourselves
    lib.kiln_rs_free.argtypes = [ctypes.c_void_p]
    lib.kiln_rs_free.restype = None
    return lib


def _call(lib, op, args, progress=None):
    """One call into a library -> the parsed JSON result (a dict). Raises on a broken library."""
    state = {"stop": False}

    def cb(event, _user):
        try:
            if progress is None:
                return 0
            if event is None:
                if progress.cancelled():
                    state["stop"] = True
            else:
                progress.event(event.decode("utf-8", "replace"))
                if progress.cancelled():
                    state["stop"] = True
        except Exception:
            pass
        return 1 if state["stop"] else 0

    c_cb = _CB(cb)                                         # alive until the call returns
    raw = lib.kiln_rs_call(op.encode(), json.dumps(args, ensure_ascii=False).encode("utf-8"), c_cb, None)
    if not raw:
        raise RuntimeError("the Rust engine returned nothing")
    try:
        text = ctypes.string_at(raw).decode("utf-8", "replace")
    finally:
        lib.kiln_rs_free(raw)
    return json.loads(text)


def probe(path):
    """Checks a candidate library. -> its version string; raises ValueError saying what's wrong."""
    try:
        with open(path, "rb") as f:
            if f.read(4) != b"\x7fELF":
                raise ValueError("this isn't a shared library (.so) file")
        lib = _bind(path)
    except ValueError:
        raise
    except (OSError, AttributeError) as e:
        raise ValueError(f"it can't be loaded: {e}")
    try:
        v = _call(lib, "version", {})
    except Exception as e:
        raise ValueError(f"it doesn't answer like the Kiln Rust engine: {e}")
    ver = v.get("kiln") if isinstance(v, dict) else None
    if not isinstance(ver, str) or not ver.startswith("rs-"):
        raise ValueError("it doesn't answer like the Kiln Rust engine (no version)")
    return ver[3:]


def configure(path):
    """Use the library at `path` (None = the Python engine only). Never raises."""
    global _lib, _path, _note
    _lib, _path, _note = None, None, ""
    if not path:
        return
    try:
        probe(path)
        _lib, _path = _bind(path), path
    except Exception as e:
        _note = f"Rust engine not used ({e}). The Python engine is running."


def active():
    return _lib is not None


def note():
    return _note


def run(op, args, progress=None):
    """-> the JSON result dict, or None when the caller should use the Python engine instead
    (no engine, it failed, or it says the job is unsupported)."""
    global _lib, _note
    if _lib is None:
        return None
    try:
        r = _call(_lib, op, args, progress)
    except Exception as e:
        _lib, _note = None, f"Rust engine stopped ({e}); using the Python engine."
        return None
    if not isinstance(r, dict) or r.get("unsupported"):
        return None
    return r
