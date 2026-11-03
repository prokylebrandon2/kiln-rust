#!/usr/bin/env python3
"""Run the tests/mirror/test_app.py checks against the REAL compiled Rust library instead of the Python mirror.

usage: run.py <libkiln_rs.so> <python dir with kiln_app.py + kiln_v14.py> <corpus dir> <part>
  part: open | test | extract | cancel | fnmatch | unsupported | sets | sets2 | sets3 | sets4 | sets5
        (run `mp3` through test_mp3_so.py instead). Build the lib: cd kiln-rs && cargo build --release
The two blocks of test_app.py that poke at mirror internals (fake MP3 failure in a set, make_unfilter on a
fake entry) are cut out: they test the mirror, and the Rust engine builds an MP3 filter now."""
import os, subprocess, sys, tempfile
so, pydir, corpus, part = sys.argv[1:5]
here = os.path.dirname(os.path.abspath(__file__)); mirror = os.path.join(os.path.dirname(here), "mirror")
s = open(os.path.join(mirror, "test_app.py")).read()
a = s.index("        # ---- an archive with the MP3 filter in the middle of a set"); b = s.index('if part in ("cancel", "all"):')
s = s[:a] + "\n" + s[b:]
a = s.index("    # an MP3-filter entry is still refused before anything is written"); b = s.index('if part.startswith("sets") or part == "all":')
s = s[:a] + "\n" + s[b:]
t = tempfile.mkdtemp(); open(os.path.join(t, "test_app.py"), "w").write(s)
env = dict(os.environ, KILN_RS_SO=os.path.abspath(so), KILN_MIRROR_DIR=mirror, PYTHONPATH="")
sys.exit(subprocess.call([sys.executable, "-I", "-c",
    f"import sys; sys.path[:0] = [{pydir!r}, {here!r}]; sys.argv = ['test_app.py', {pydir!r}, {corpus!r}, {part!r}]; exec(compile(open({os.path.join(t, 'test_app.py')!r}).read(), 'test_app.py', 'exec'), {{'__file__': {os.path.join(t, 'test_app.py')!r}, '__name__': '__main__'}})"], env=env))
