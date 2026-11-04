#!/usr/bin/env python3
"""Kiln Phase 3 benchmark harness.

Runs create / test / extract (+ one-file selective extract) for one or more kiln binaries over
one or more datasets, N repetitions each, and prints/records the MEDIAN wall time plus CPU time,
peak RSS and archive size of the child process only (wait4 rusage).

Page cache is WARM (a cold-cache run needs root and a real disk; see docs/PHASE3.md).
Round trip is verified byte-for-byte (SHA-256 manifest of every file + empty dirs).

usage: bench.py --data DIR --work DIR --out results.json  LABEL=BINARY[:ARGSTYLE] ... -- DATASET ...
Each variant spec: LABEL|BINARY|create-args  (create-args e.g. "-c zstd:3 -j 2")
"""
import argparse, hashlib, json, os, shutil, statistics, subprocess, sys, time

RSSTIME = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bin", "rsstime")

def run(cmd):
    """Run cmd under the tiny `rsstime` wrapper (see tools/rsstime.c) and return its metrics."""
    out = f"/tmp/rsstime.{os.getpid()}.json"
    p = subprocess.run([RSSTIME, out, *cmd], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE)
    m = json.load(open(out)); os.remove(out)
    m["err"] = p.stderr.decode(errors="replace").strip()[-300:]
    return m

def manifest(root):
    h = hashlib.sha256()
    if os.path.isfile(root):
        with open(root, "rb") as fp:
            for chunk in iter(lambda: fp.read(1 << 20), b""): h.update(chunk)
        return h.hexdigest()
    for dp, dn, fn in os.walk(root):
        dn.sort()
        rel = os.path.relpath(dp, root)
        h.update(b"D:" + rel.encode() + b"\n")
        for f in sorted(fn):
            fh = hashlib.sha256()
            with open(os.path.join(dp, f), "rb") as fp:
                for chunk in iter(lambda: fp.read(1 << 20), b""): fh.update(chunk)
            h.update(b"F:" + os.path.join(rel, f).encode() + b":" + fh.hexdigest().encode() + b"\n")
    return h.hexdigest()

def med(xs): return statistics.median(xs)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True); ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True); ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--variants", nargs="+", required=True, help="LABEL|BINARY|ARGS")
    ap.add_argument("--datasets", nargs="+", required=True)
    ap.add_argument("--threads", default="0", help="-j value for extract/test")
    a = ap.parse_args()
    results = []
    for ds in a.datasets:
        src = os.path.join(a.data, ds)
        raw = sum(os.path.getsize(os.path.join(dp, f)) for dp, _, fn in os.walk(src) for f in fn) if os.path.isdir(src) else os.path.getsize(src)
        nfiles = sum(len(fn) for _, _, fn in os.walk(src)) if os.path.isdir(src) else 1
        ref = manifest(src) if os.path.isdir(src) else None
        for v in a.variants:
            label, binary, cargs = v.split("|")
            ext = ".kiln" if "phase3" in label or "p3" in label else ".kr"
            rec = {"dataset": ds, "variant": label, "raw_bytes": raw, "files": nfiles, "create": [], "test": [], "extract": []}
            for rep in range(a.reps):
                arc = os.path.join(a.work, "t" + ext)
                outd = os.path.join(a.work, "out")
                for p in (arc, outd): shutil.rmtree(p, ignore_errors=True) if os.path.isdir(p) else (os.path.exists(p) and os.remove(p))
                r = run([binary, "create", arc, src, "-f", "-q"] + cargs.split())
                if r["rc"] not in (0, 7): rec["error"] = f"create rc={r['rc']} {r['err']}"; break
                rec["create"].append(r); rec["archive_bytes"] = os.path.getsize(arc)
                r = run([binary, "test", arc, "-j", a.threads])
                if r["rc"] != 0: rec["error"] = f"test rc={r['rc']} {r['err']}"; break
                rec["test"].append(r)
                os.makedirs(outd)
                r = run([binary, "extract", arc, "-o", outd, "-j", a.threads, "-q"])
                if r["rc"] != 0: rec["error"] = f"extract rc={r['rc']} {r['err']}"; break
                rec["extract"].append(r)
                if rep == 0:
                    got = manifest(os.path.join(outd, os.path.basename(src))) if os.path.isdir(src) else manifest(os.path.join(outd, os.path.basename(src)))
                    want = ref if ref else manifest(src)
                    rec["roundtrip_ok"] = (got == want)
                    if got != want: rec["error"] = "ROUND TRIP MISMATCH"; break
            for k in ("create", "test", "extract"):
                if rec[k]:
                    rec[k + "_wall"] = med([x["wall"] for x in rec[k]]); rec[k + "_cpu"] = med([x["cpu"] for x in rec[k]])
                    rec[k + "_rss"] = max(x["rss_mib"] for x in rec[k])
            if "archive_bytes" in rec: rec["ratio"] = raw / rec["archive_bytes"]
            results.append(rec)
            print(f"{ds:11s} {label:22s} " + (f"ERR {rec['error']}" if "error" in rec else
                  f"size {rec['archive_bytes']/1e6:8.2f}MB x{rec['ratio']:5.2f} | create {rec['create_wall']:6.2f}s cpu {rec['create_cpu']:6.2f} rss {rec['create_rss']:5.0f} | "
                  f"extract {rec['extract_wall']:6.2f}s rss {rec['extract_rss']:4.0f} | test {rec['test_wall']:6.2f}s"), flush=True)
            json.dump(results, open(a.out, "w"), indent=1)
    shutil.rmtree(os.path.join(a.work, "out"), ignore_errors=True)

if __name__ == "__main__": main()
