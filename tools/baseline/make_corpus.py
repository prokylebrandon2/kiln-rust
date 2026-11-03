#!/usr/bin/env python3
"""Build the benchmark corpus used in docs/BASELINE.md.

text   - Python standard-library sources, up to ~6 MB
binary - native shared libraries from /usr/lib/x86_64-linux-gnu, up to ~6 MB
random - 2 MiB of seeded pseudo-random (incompressible) data

text/binary depend on what is installed on the machine, so byte counts differ between hosts;
compare tools on ONE machine, as the harness does. Usage: make_corpus.py [OUT_DIR]
"""
import glob, hashlib, os, random, shutil, sys

root = sys.argv[1] if len(sys.argv) > 1 else "corpus"
shutil.rmtree(root, ignore_errors=True)
for d in ("text", "binary", "random"):
    os.makedirs(f"{root}/{d}")

total = 0
for p in sorted(glob.glob("/usr/lib/python3*/**/*.py", recursive=True)):
    if not os.path.isfile(p) or os.path.islink(p):
        continue
    size = os.path.getsize(p)
    if total + size > 6_000_000:
        break
    name = hashlib.md5(p.encode()).hexdigest()[:8] + "-" + os.path.basename(p)
    shutil.copyfile(p, f"{root}/text/{name}")
    total += size

total = 0
for p in sorted(glob.glob("/usr/lib/x86_64-linux-gnu/*.so*")):
    if not os.path.isfile(p) or os.path.islink(p):
        continue
    size = os.path.getsize(p)
    if size > 3_000_000 or total + size > 6_000_000:
        continue
    shutil.copyfile(p, f"{root}/binary/{os.path.basename(p)}")
    total += size

open(f"{root}/random/noise.bin", "wb").write(random.Random(1234).randbytes(2 * 1024 * 1024))
for d in ("text", "binary", "random"):
    files = [os.path.join(dp, f) for dp, _, fn in os.walk(f"{root}/{d}") for f in fn]
    print(f"{d:7s} {len(files):5d} files {sum(map(os.path.getsize, files)):>10d} bytes")
