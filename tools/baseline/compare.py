#!/usr/bin/env python3
"""Compare original kiln v14 (Python+C), the Rust rewrite, and reference CLI tools.

Every measurement is the best (lowest wall time) of N runs, taken per command, with wall time,
CPU time and peak RSS of that child process only. Round trips are compared byte-for-byte.

  compare.py --corpus DIR --kiln-rs target/release/kiln --kiln-orig kiln_v14.py \
             [--runs 3] [--json out.json] [classes...]
"""
import argparse, filecmp, json, os, shutil, subprocess, sys, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import run

ap = argparse.ArgumentParser()
ap.add_argument("--corpus", required=True)
ap.add_argument("--kiln-rs")
ap.add_argument("--kiln-orig")
ap.add_argument("--runs", type=int, default=3)
ap.add_argument("--json")
ap.add_argument("--archivers-only", action="store_true",
                help="measure only zip and 7-Zip on the directory (no kiln, no tar-based tools)")
ap.add_argument("classes", nargs="*", default=["text", "binary", "random"])
args = ap.parse_args()

def best(make_cmd, prep=None, **kw):
    """Run `runs` times; return the fastest run's numbers."""
    runs = []
    for _ in range(args.runs):
        if prep: prep()
        runs.append(run(make_cmd(), **kw))
    return min(runs, key=lambda r: r["wall"])

def dirsize(p):
    return sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(p) for f in fs)

def same_tree(a, b):
    c = filecmp.dircmp(a, b)
    if c.left_only or c.right_only or c.diff_files or c.funny_files: return False
    if filecmp.cmpfiles(a, b, c.common_files, shallow=False)[1]: return False
    return all(same_tree(os.path.join(a, d), os.path.join(b, d)) for d in c.common_dirs)

def kiln_tool(label, base_cmd, pack_flags, unpack_flags, env, src, cls, tmp, ext):
    arc, out = os.path.join(tmp, "a." + ext), os.path.join(tmp, "out")
    def fresh_arc():
        if os.path.exists(arc): os.remove(arc)
    def fresh_out():
        shutil.rmtree(out, ignore_errors=True); os.makedirs(out)
    c = best(lambda: base_cmd + ["pack", *pack_flags, arc, src], fresh_arc, env=env)
    fresh_out()
    d = best(lambda: base_cmd + ["unpack", *unpack_flags, arc, "-o", out], fresh_out, env=env)
    ok = same_tree(src, os.path.join(out, cls))
    return label, dict(size=os.path.getsize(arc), c=c, d=d, roundtrip_ok=ok)

results = []
for cls in args.classes:
    src = os.path.join(args.corpus, cls)
    raw = dirsize(src)
    row = {"class": cls, "raw": raw, "tools": {}}
    tmp = tempfile.mkdtemp(prefix="cmp_")
    if args.archivers_only:
        pass
    elif args.kiln_orig:
        env = dict(os.environ, HOME=os.path.join(tmp, "home"))
        os.makedirs(env["HOME"], exist_ok=True)
        k, v = kiln_tool("kiln-orig v14 (-j1)", [sys.executable, args.kiln_orig], ["-j", "1", "-q"],
                         ["-q"], env, src, cls, tmp, "kln")
        row["tools"][k] = v
    if args.kiln_rs and not args.archivers_only:
        for codec, blk in [("lz4", "4MiB"), ("zstd:3", "4MiB"), ("zstd:19", "4MiB"),
                           ("zstd:19", "32MiB"), ("lzma:6", "4MiB"), ("lzma:6", "32MiB")]:
            k, v = kiln_tool(f"kiln-rs {codec} blk={blk} (-j1)", [args.kiln_rs],
                             ["-q", "-f", "-c", codec, "-b", blk, "-j", "1"], ["-q", "-f", "-j", "1"],
                             None, src, cls, tmp, "kr")
            row["tools"][k] = v
    if not args.archivers_only:
        tarf = os.path.join(tmp, "in.tar")
        subprocess.run(["tar", "-C", os.path.dirname(src), "-cf", tarf, os.path.basename(src)], check=True)
        for name, cc, dc in [
            ("gzip -6", ["gzip", "-6", "-c", tarf], ["gzip", "-dc"]),
            ("bzip2 -9", ["bzip2", "-9", "-c", tarf], ["bzip2", "-dc"]),
            ("xz -6 -T1", ["xz", "-6", "-T1", "-c", tarf], ["xz", "-dc"]),
            ("zstd -3 -T1", ["zstd", "-3", "-T1", "-c", "-q", tarf], ["zstd", "-dc", "-q"]),
            ("zstd -19 -T1", ["zstd", "-19", "-T1", "-c", "-q", tarf], ["zstd", "-dc", "-q"]),
            ("lz4 -1", ["lz4", "-1", "-c", "-q", tarf], ["lz4", "-dc", "-q"]),
        ]:
            o = os.path.join(tmp, "ref.out")
            def comp():
                with open(o, "wb") as fh: return run(cc, stdout=fh)
            runs = [comp() for _ in range(args.runs)]
            c = min(runs, key=lambda r: r["wall"])
            def decomp():
                with open(o, "rb") as fi: return run(dc, stdin=fi)
            d = min([decomp() for _ in range(args.runs)], key=lambda r: r["wall"])
            row["tools"][name + " (tar)"] = dict(size=os.path.getsize(o), c=c, d=d, roundtrip_ok=None)

    # Real archivers, run on the directory itself (they do their own file handling).
    def archiver(label, make_c, make_d, ext, prefix):
        if not shutil.which(prefix): return
        arc, out = os.path.join(tmp, "arch." + ext), os.path.join(tmp, "aout")
        def fresh_arc():
            if os.path.exists(arc): os.remove(arc)
        def fresh_out():
            shutil.rmtree(out, ignore_errors=True); os.makedirs(out)
        c = best(lambda: make_c(arc, src), fresh_arc, cwd=os.path.dirname(src))
        d = best(lambda: make_d(arc, out), fresh_out)
        row["tools"][label] = dict(size=os.path.getsize(arc), c=c, d=d,
                                   roundtrip_ok=same_tree(src, os.path.join(out, cls)))
    base = os.path.basename(src)
    archiver("zip -6 (Info-ZIP)", lambda a, s_: ["zip", "-r", "-q", "-6", a, base],
             lambda a, o: ["unzip", "-q", a, "-d", o], "zip", "zip")
    archiver("7z -mx=5 -mmt=1 (LZMA2)", lambda a, s_: ["7z", "a", "-bso0", "-bsp0", "-mx=5", "-mmt=1", a, base],
             lambda a, o: ["7z", "x", "-bso0", "-bsp0", "-y", f"-o{o}", a], "7z", "7z")
    archiver("7z -mx=9 -mmt=1 (LZMA2)", lambda a, s_: ["7z", "a", "-bso0", "-bsp0", "-mx=9", "-mmt=1", a, base],
             lambda a, o: ["7z", "x", "-bso0", "-bsp0", "-y", f"-o{o}", a], "7z", "7z")
    shutil.rmtree(tmp, ignore_errors=True)
    results.append(row)

if args.json:
    json.dump(results, open(args.json, "w"), indent=1)
for r in results:
    print(f"\n### {r['class']}  (raw {r['raw']:,} bytes)")
    print("| tool | size | ratio | compress s | decompress s | MiB/s c | MiB/s d | RSS c MiB | RSS d MiB | round-trip |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|:--:|")
    for n, t in r["tools"].items():
        mb = r["raw"] / 1048576
        ok = {True: "yes", False: "**NO**", None: "n/a"}[t["roundtrip_ok"]]
        print(f"| {n} | {t['size']:,} | {r['raw']/t['size']:.3f} | {t['c']['wall']:.2f} | {t['d']['wall']:.2f} | "
              f"{mb/t['c']['wall']:.0f} | {mb/t['d']['wall']:.0f} | {t['c']['rss_mib']:.0f} | {t['d']['rss_mib']:.0f} | {ok} |")
