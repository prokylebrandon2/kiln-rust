#!/usr/bin/env python3
"""Controlled extraction A/B. Variants alternate round-robin (in a rotating order) so drift in the
file system affects all of them equally; `sync` + a settle delay precede every timed run and the
output directory is removed (and synced) OUTSIDE the timed region.
usage: extract_ab.py DATASET_DIR WORKDIR ROUNDS OUT.json"""
import json, os, shutil, statistics, subprocess, sys, time
sys.path.insert(0, os.path.dirname(__file__))
from bench import run

data, work, rounds, outjson = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
P2, P3 = "bin/kiln-phase2", "bin/kiln-phase3"
codec = os.environ.get("CODEC", "store")          # store isolates file-creation cost
os.makedirs(work, exist_ok=True)
a2, a3, tr = f"{work}/ab.kr", f"{work}/ab.kiln", f"{work}/ab.tar"
for p in (a2, a3, tr):
    if os.path.exists(p): os.remove(p)
run([P2, "create", a2, data, "-q", "-c", codec]); run([P3, "create", a3, data, "-q", "-c", codec])
subprocess.run(["tar", "cf", tr, "-C", os.path.dirname(data) or ".", os.path.basename(data)], check=True)
out = f"{work}/ab_out"
variants = {
    "phase2 -j2": lambda: [P2, "extract", a2, "-o", out, "-q", "-j", "2"],
    "phase3 -j1": lambda: [P3, "extract", a3, "-o", out, "-q", "-j", "1"],
    "phase3 -j2": lambda: [P3, "extract", a3, "-o", out, "-q", "-j", "2"],
    "tar xf":     lambda: ["tar", "xf", tr, "-C", out],
}
names = list(variants); res = {n: [] for n in names}
for r in range(rounds):
    order = names[r % len(names):] + names[:r % len(names)]
    for n in order:
        shutil.rmtree(out, ignore_errors=True); os.makedirs(out)
        subprocess.run(["sync"]); time.sleep(2)
        m = run(variants[n]()); assert m["rc"] == 0, (n, m)
        res[n].append(m["wall"])
        shutil.rmtree(out, ignore_errors=True)
        print(f"round {r} {n:11s} {m['wall']:6.2f}s cpu {m['cpu']:5.2f}", flush=True)
summary = {n: {"median": statistics.median(v), "min": min(v), "max": max(v), "runs": v} for n, v in res.items()}
print()
for n, s in summary.items(): print(f"{n:11s} median {s['median']:6.2f}s  min {s['min']:6.2f}  max {s['max']:6.2f}")
json.dump({"dataset": data, "codec": codec, "rounds": rounds, "summary": summary}, open(outjson, "w"), indent=1)
