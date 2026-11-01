"""Thin layer between the Android UI and kiln_v14.py.

The app calls run(args, password, sink): args is the list you would type after `kiln`, joined
with the \\x1f character (a plain list works too),
the password comes from the app's own password fields (never from the environment), and every
output line is passed to sink.line(text)."""
import io
import os
import sys
import threading

import kiln_v14 as kiln

_lock = threading.Lock()


class Cancelled(BaseException):
    """Raised from a sink to stop a running command (BaseException: kiln can't swallow it)."""


def _no_prompt(confirm=False):
    raise kiln.KilnError("enter a password")


kiln.get_password = _no_prompt           # an app has no terminal to ask on

_Progress = kiln.Progress
_hook = None


class _HookedProgress(_Progress):
    """kiln's own progress counter, but reporting to the app (and able to cancel)."""
    def add(self, n):
        super().add(n)
        if _hook is not None:
            _hook(self.done, self.total)


class _Lines(io.TextIOBase):
    """File-like object that forwards complete lines (split on \\n and \\r) to the UI."""
    def __init__(self, sink):
        self.sink, self.buf = sink, ""

    def writable(self):
        return True

    def write(self, s):
        self.buf += s
        parts = self.buf.replace("\r", "\n").split("\n")
        self.buf = parts.pop()
        for part in parts:
            if part.strip():
                self.sink.line(part)
        return len(s)

    def flush(self):
        text, self.buf = self.buf, ""
        if text.strip():
            try:
                self.sink.line(text)
            except Cancelled:
                pass


def accel_note():
    """'' when the C helper loaded, else why it didn't."""
    return "" if kiln._A is not None else (kiln.ACCEL_NOTE or "C helper not loaded")


def run(args, password, sink, hook=None):
    """Run one kiln command. Returns 0 on success, non-zero on failure."""
    args = args.split("\x1f") if isinstance(args, str) else [str(a) for a in args]
    if not args or args[0] not in ("pack", "unpack", "list", "test"):
        sink.line("error: unsupported command")   # also keeps kiln's interactive shell out
        return 2
    if args[0] == "pack":
        args[1:1] = ["-j", "1"]          # no worker processes: forking inside an app is unsafe
    pw = password or None

    def get_password(confirm=False):
        if not pw:
            raise kiln.KilnError("enter a password")
        if confirm and len(pw) < kiln.MIN_PASSWORD:
            raise kiln.KilnError(f"password must be at least {kiln.MIN_PASSWORD} characters "
                                 "(a long passphrase is best)")
        return pw

    with _lock:                          # kiln keeps module-level state: one command at a time
        global _hook
        saved = (sys.argv, sys.stdout, sys.stderr, kiln.get_password, kiln.start_mp3_ahead)
        out = _Lines(sink)
        sys.argv = ["kiln"] + args
        sys.stdout = sys.stderr = out
        kiln.get_password = get_password
        kiln.start_mp3_ahead = lambda *a, **k: None
        _hook = hook
        kiln.Progress = _HookedProgress if hook else _Progress
        code = 0
        try:
            kiln.require_accel()
            kiln.main()
        except Cancelled:
            code = 130
        except SystemExit as e:
            if e.code not in (None, 0):
                code = 1 if not isinstance(e.code, int) else e.code
                if isinstance(e.code, str):
                    print(e.code)
        except BaseException as e:       # never let an exception kill the app
            code = 1
            print(f"error: {e}")
        finally:
            out.flush()
            _hook = None
            kiln.Progress = _Progress
            kiln.stop_mp3_ahead()
            (sys.argv, sys.stdout, sys.stderr,
             kiln.get_password, kiln.start_mp3_ahead) = saved
        return code
