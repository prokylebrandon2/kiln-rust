#!/usr/bin/env python3
"""Regenerate the benchmark datasets used in docs/PHASE3.md.

usage: make_datasets.py OUTDIR

d1_pytree   Python 3.13 standard-library *source* files (.py .txt .rst .html .json .pem .css .js,
            <= 2 MB each, no tests/site-packages)  ~8.3k files, ~131 MB.  Depends on the
            interpreter you run this with: the exact bytes (and so the sizes in PHASE3.md) vary
            with the Python patch version.  Pass --pylib to point at a particular lib directory.
d2_tiny     30,000 synthetic log-like files of 0.2-6 KB (seeded, fully deterministic), ~93 MB.
d3_mixed    shared libraries + already-compressed .gz/.xz + random "media" + text, in shuffled
            order.  The .so files come from /usr/lib/x86_64-linux-gnu, so they depend on the host.
d4_large.bin  one ~403 MB file: random-offset slices of a text file and of libc, mixed with noise.
            SYNTHETIC: it has long-distance repetition that flatters large-window compressors.
d5_random   64 files x 4 MiB of seeded pseudo-random bytes (incompressible).
"""
import glob, os, random, shutil, sys, zlib, lzma, sysconfig

def main(out, pylib):
    os.makedirs(out, exist_ok=True); os.chdir(out)
    # d1
    dst = "d1_pytree"; shutil.rmtree(dst, ignore_errors=True)
    n = tot = 0
    for dp, dn, fn in os.walk(pylib):
        dn[:] = sorted(d for d in dn if d not in ("site-packages", "__pycache__", "test", "tests", "idlelib", "turtledemo"))
        for f in sorted(fn):
            if not f.endswith((".py", ".txt", ".rst", ".html", ".json", ".pem", ".css", ".js")): continue
            p = os.path.join(dp, f)
            if os.path.islink(p) or not os.path.isfile(p) or os.path.getsize(p) > 2_000_000: continue
            o = os.path.join(dst, os.path.relpath(p, pylib)); os.makedirs(os.path.dirname(o), exist_ok=True)
            shutil.copyfile(p, o); n += 1; tot += os.path.getsize(p)
    print("d1", n, tot)
    # d2
    r = random.Random(42); dst = "d2_tiny"; shutil.rmtree(dst, ignore_errors=True)
    words = ["alpha", "beta", "gamma", "delta", "kiln", "archive", "chunk", "index", "error", "warn", "info",
             "user", "session", "token", "value"]
    tot = 0
    for i in range(30000):
        d = f"{dst}/{i % 200:03d}/{(i // 200) % 50:02d}"; os.makedirs(d, exist_ok=True)
        size = r.randint(200, 6000)
        s = " ".join(r.choice(words) + str(r.randint(0, 999)) for _ in range(size // 8))[:size]
        open(f"{d}/f{i:05d}.log", "w").write(s); tot += len(s)
    print("d2", 30000, tot)
    # d3
    dst = "d3_mixed"; shutil.rmtree(dst, ignore_errors=True); os.makedirs(dst)
    tot = k = 0
    bins = sorted(p for p in glob.glob("/usr/lib/x86_64-linux-gnu/*.so*")
                  if os.path.isfile(p) and not os.path.islink(p) and os.path.getsize(p) < 4_000_000)[:60]
    for p in bins:
        shutil.copyfile(p, f"{dst}/{k:03d}_{os.path.basename(p)}"); tot += os.path.getsize(p); k += 1
    rr = random.Random(7)
    txt = open(os.path.join(pylib, "argparse.py"), "rb").read()
    for i in range(60):
        blob = b"".join(txt[rr.randint(0, len(txt) - 2000):][:rr.randint(1000, 1900)] for _ in range(rr.randint(200, 900)))
        data = zlib.compress(blob, 6) if i % 2 else lzma.compress(blob)
        open(f"{dst}/{k:03d}_pack{i}.{'gz' if i % 2 else 'xz'}", "wb").write(data); tot += len(data); k += 1
    for i in range(40):
        data = rr.randbytes(rr.randint(200_000, 1_500_000))
        open(f"{dst}/{k:03d}_media{i}.jpg", "wb").write(data); tot += len(data); k += 1
    for i in range(150):
        blob = b"".join(txt[rr.randint(0, len(txt) - 4000):][:rr.randint(500, 3000)] for _ in range(rr.randint(40, 100)))
        open(f"{dst}/{k:03d}_doc{i}.txt", "wb").write(blob); tot += len(blob); k += 1
    # shuffle the order so the kinds are interleaved in walk order
    r = random.Random(2024)
    names = sorted(os.listdir(dst)); order = list(range(len(names))); r.shuffle(order)
    for nme, o in zip(names, order):
        os.rename(f"{dst}/{nme}", f"{dst}/tmp_{o:03d}_{nme.split('_', 1)[1]}")
    for nme in os.listdir(dst): os.rename(f"{dst}/{nme}", f"{dst}/{nme[4:]}")
    print("d3", k, tot)
    # d4
    r4 = random.Random(99); outf = open("d4_large.bin", "wb"); tot = 0
    bsrc = open("/usr/lib/x86_64-linux-gnu/libc.so.6", "rb").read()
    while tot < 384 * 1024 * 1024:
        c = r4.random()
        if c < 0.4: b = txt[r4.randint(0, len(txt) - 60000):][:r4.randint(5000, 60000)]
        elif c < 0.75: b = bsrc[r4.randint(0, len(bsrc) - 60000):][:r4.randint(5000, 60000)]
        else: b = r4.randbytes(r4.randint(2000, 40000))
        outf.write(b); tot += len(b)
    outf.close(); print("d4", 1, tot)
    # d5
    os.makedirs("d5_random", exist_ok=True); r5 = random.Random(5)
    for i in range(64): open(f"d5_random/r{i:02d}.bin", "wb").write(r5.randbytes(4 * 1024 * 1024))
    print("d5", 64, 64 * 4 * 1024 * 1024)

if __name__ == "__main__":
    args = sys.argv[1:]
    pylib = sysconfig.get_paths()["stdlib"]
    if "--pylib" in args:
        i = args.index("--pylib"); pylib = args[i + 1]; del args[i:i + 2]
    if len(args) != 1: sys.exit(__doc__)
    main(args[0], pylib)
