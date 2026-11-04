#!/usr/bin/env python3
"""Alternating create A/B. Variants run round-robin in a rotating order; medians and every run are
reported. usage: create_ab.py DATASET ROUNDS OUT.json LABEL|BIN|ARGS [LABEL|BIN|ARGS ...]"""
import json, os, statistics, sys
sys.path.insert(0, os.path.dirname(__file__))
from bench import run
ds, rounds, outjson, specs = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4:]
V = [s.split("|") for s in specs]
res = {v[0]: [] for v in V}; size = {}
for r in range(rounds):
    for label, binary, args in V[r % len(V):] + V[:r % len(V)]:
        arc = "work/cab." + ("kiln" if "phase3" in binary else "kr")
        if os.path.exists(arc): os.remove(arc)
        m = run([binary, "create", arc, ds, "-q", "-f"] + args.split())
        assert m["rc"] in (0, 7), (label, m)
        res[label].append(m); size[label] = os.path.getsize(arc)
out = {}
for label, ms in res.items():
    out[label] = {"archive_bytes": size[label], **{k: statistics.median(x[k] for x in ms) for k in ("wall", "cpu", "rss_mib")},
                  "walls": [round(x["wall"], 3) for x in ms]}
    o = out[label]
    print(f"{label:16s} size {o['archive_bytes']/1e6:8.2f}MB  wall {o['wall']:6.3f}s  cpu {o['cpu']:6.3f}s  rss {o['rss_mib']:5.1f}MiB   runs {o['walls']}")
json.dump({"dataset": ds, "rounds": rounds, "result": out}, open(outjson, "w"), indent=1)
