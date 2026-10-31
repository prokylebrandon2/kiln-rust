#!/usr/bin/env python3
"""Build kiln's C helper for one Android ABI with the NDK.

usage: build_accel.py <kiln_v14.py> <out.so> <clang> [extra cflags...]
The C sources are string constants inside kiln_v14.py. They are read with ast so nothing in
kiln runs (importing it would try to build a helper for the build machine)."""
import ast, os, subprocess, sys, tempfile

src_py, out, cc, *extra = sys.argv[1:]
want = {"_ACCEL_SRC": None, "_MP3_SRC": None}
for node in ast.parse(open(src_py, encoding="utf-8").read()).body:
    if isinstance(node, ast.Assign) and len(node.targets) == 1 \
            and getattr(node.targets[0], "id", None) in want:
        want[node.targets[0].id] = ast.literal_eval(node.value)
if not all(want.values()):
    sys.exit("could not find _ACCEL_SRC / _MP3_SRC in " + src_py)
cflags = ["-O3", "-ffp-contract=off", "-fno-math-errno", "-fno-semantic-interposition"]
with tempfile.TemporaryDirectory() as d:
    files = []
    for name, code in (("accel.c", want["_ACCEL_SRC"]), ("mp3.c", want["_MP3_SRC"])):
        path = os.path.join(d, name)
        with open(path, "w", encoding="utf-8", newline="") as fh:
            fh.write(code)
        files.append(path)
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    subprocess.run([cc, *cflags, *extra, "-shared", "-fPIC", "-o", out, *files, "-lm"], check=True)
print("built", out)
