#!/usr/bin/env python3
"""Kiln vs 7-Zip vs ZIP head-to-head.

For every (dataset, tool-variant): create, test, extract, and verify the extracted tree against
the source with a SHA-256 manifest. Repetitions are interleaved (all variants once, then again) so
drift hits everyone equally; medians reported. Heavy variants (max/ultra) run once.
Page cache is warm; `sync` + 1 s settle precede every extraction.
usage: vs7z.py --data DIR --work DIR --out FILE.json [--reps N] DATASET...
"""
import argparse, json, os, shutil, statistics, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench import run, manifest

B = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bin")
K3, Z7 = f"{B}/kiln-phase3", f"{B}/7zz"
# label, tier, heavy, ext, create(src_name, arc), test(arc), extract(arc, out)
def kiln(profile):
    return dict(ext="kiln",
        create=lambda s, a: [K3, "create", a, s, "-q", "-f", "-p", profile],
        test=lambda a: [K3, "test", a],
        extract=lambda a, o: [K3, "extract", a, "-o", o, "-q"])
def sevenz(args):
    return dict(ext="7z" if "-tzip" not in args else "zip",
        create=lambda s, a: [Z7, "a", a, s, "-bso0", "-bsp0", "-mmt=2"] + args,
        test=lambda a: [Z7, "t", a, "-bso0", "-bsp0"],
        extract=lambda a, o: [Z7, "x", a, f"-o{o}", "-y", "-bso0", "-bsp0"])
def infozip(level):
    return dict(ext="zip",
        create=lambda s, a: ["zip", "-q", "-r", f"-{level}", a, s],
        test=lambda a: ["unzip", "-tq", a],
        extract=lambda a, o: ["unzip", "-q", "-d", o, a])
V = {
 "kiln fastest":  (kiln("fastest"),  "fast", False),
 "7z -mx1":       (sevenz(["-mx=1"]), "fast", False),
 "zip (7z, deflate -mx1)": (sevenz(["-tzip", "-mx=1"]), "fast", False),
 "kiln balanced": (kiln("balanced"), "balanced", False),
 "kiln adaptive": (kiln("adaptive"), "balanced", False),
 "7z -mx5 (default)": (sevenz(["-mx=5"]), "balanced", False),
 "zip -6 (Info-ZIP)": (infozip(6), "balanced", False),
 "kiln maximum":  (kiln("maximum"),  "max", True),
 "7z -mx9 (ultra)": (sevenz(["-mx=9"]), "max", True),
}
ap = argparse.ArgumentParser()
ap.add_argument("--data", required=True); ap.add_argument("--work", required=True)
ap.add_argument("--out", required=True); ap.add_argument("--reps", type=int, default=3)
ap.add_argument("--skip-max", action="store_true"); ap.add_argument("datasets", nargs="+")
a = ap.parse_args()
os.makedirs(a.work, exist_ok=True)
results = []
def sh_in(cwd, cmd):  # run cmd with cwd=data, via exec so rsstime measures the tool itself
    return ["sh", "-c", 'cd "$0" && exec "$@"', cwd] + cmd
for ds in a.datasets:
    src = os.path.join(a.data, ds)
    isdir = os.path.isdir(src)
    raw = sum(os.path.getsize(os.path.join(dp, f)) for dp, _, fn in os.walk(src) for f in fn) if isdir else os.path.getsize(src)
    nfiles = sum(len(fn) for _, _, fn in os.walk(src)) if isdir else 1
    ref = manifest(src)
    names = [n for n, (_, _, heavy) in V.items() if not (heavy and a.skip_max)]
    rec = {n: {"dataset": ds, "tool": n, "tier": V[n][1], "raw_bytes": raw, "files": nfiles,
               "create": [], "test": [], "extract": []} for n in names}
    for rep in range(a.reps):
        for n in names:
            spec, tier, heavy = V[n]
            if heavy and rep > 0: continue          # heavy variants: one run
            r = rec[n]; arc = os.path.join(a.work, "arc." + spec["ext"]); out = os.path.join(a.work, "out")
            for p in (arc,): 
                if os.path.exists(p): os.remove(p)
            shutil.rmtree(out, ignore_errors=True)
            m = run(sh_in(a.data, spec["create"](ds, os.path.abspath(arc))))
            if m["rc"] not in (0, 7): r["error"] = f"create rc={m['rc']} {m['err']}"; continue
            r["create"].append(m); r["archive_bytes"] = os.path.getsize(arc)
            m = run(spec["test"](arc))
            if m["rc"] != 0: r["error"] = f"test rc={m['rc']} {m['err']}"; continue
            r["test"].append(m)
            os.makedirs(out); subprocess.run(["sync"]); time.sleep(1)
            m = run(spec["extract"](arc, out))
            if m["rc"] != 0: r["error"] = f"extract rc={m['rc']} {m['err']}"; continue
            r["extract"].append(m)
            if rep == 0:
                got = manifest(os.path.join(out, os.path.basename(src)))
                r["roundtrip_ok"] = got == ref
                if not r["roundtrip_ok"]: r["error"] = "ROUND TRIP MISMATCH"
            shutil.rmtree(out, ignore_errors=True)
        json.dump(list(rec.values()), open(a.out, "w"), indent=1)
    for n in names:
        r = rec[n]
        for k in ("create", "test", "extract"):
            if r[k]:
                r[k + "_wall"] = statistics.median(x["wall"] for x in r[k]); r[k + "_cpu"] = statistics.median(x["cpu"] for x in r[k])
                r[k + "_rss"] = max(x["rss_mib"] for x in r[k])
        if "archive_bytes" in r: r["ratio"] = raw / r["archive_bytes"]
        print(f"{ds:12s} {n:24s} " + (f"ERR {r['error']}" if "error" in r else
              f"{r['archive_bytes']/1e6:8.2f}MB x{r['ratio']:5.2f} | create {r['create_wall']:7.2f}s cpu {r['create_cpu']:7.2f} rss {r['create_rss']:5.0f} | "
              f"test {r['test_wall']:6.2f}s | extract {r['extract_wall']:6.2f}s rss {r['extract_rss']:4.0f} | rt={'OK' if r.get('roundtrip_ok') else 'FAIL'}"), flush=True)
    results += list(rec.values())
    json.dump(results, open(a.out, "w"), indent=1)
