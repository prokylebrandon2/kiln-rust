#!/usr/bin/env python3
"""Ablation of the chunk buffer pool: same binary, KILN_NO_POOL unset vs set, alternating.
usage: pool_ab.py DATASET ROUNDS OUT.json -- create-args..."""
import json, os, statistics, sys, time
sys.path.insert(0, os.path.dirname(__file__))
from bench import run
ds, rounds, outjson = sys.argv[1], int(sys.argv[2]), sys.argv[3]
cargs = sys.argv[sys.argv.index("--") + 1:]
res = {"pool": [], "no-pool": []}
for r in range(rounds):
    for name in (["pool", "no-pool"] if r % 2 == 0 else ["no-pool", "pool"]):
        if name == "no-pool": os.environ["KILN_NO_POOL"] = "1"
        else: os.environ.pop("KILN_NO_POOL", None)
        if os.path.exists("work/pool.kiln"): os.remove("work/pool.kiln")
        m = run(["bin/kiln-phase3", "create", "work/pool.kiln", ds, "-q", "-f"] + cargs)
        assert m["rc"] == 0, m
        res[name].append(m)
out = {}
for n, ms in res.items():
    out[n] = {k: statistics.median(x[k] for x in ms) for k in ("wall", "cpu", "rss_mib")}
    out[n]["walls"] = [round(x["wall"], 3) for x in ms]
    print(f"{n:8s} wall {out[n]['wall']:.3f}s  cpu {out[n]['cpu']:.3f}s  rss {out[n]['rss_mib']:.0f} MiB   runs {out[n]['walls']}")
json.dump({"dataset": ds, "args": cargs, "result": out}, open(outjson, "w"), indent=1)
