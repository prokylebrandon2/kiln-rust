#!/usr/bin/env python3
"""Multicore scaling curve for kiln: create, test and extract at 1, 2, 4, 8, 16 ... threads.

Closes the Phase 2 roadmap item "measure scaling on a multicore machine (1/2/4/8/16 threads) for
lz4, zstd:3, zstd:19, lzma:6" (docs/SCALING.md). Python 3.8+, standard library
only, Linux or macOS. Needs no helper binary: CPU time and peak RSS come from wait4's rusage.

    cargo build --release
    python3 tools/bench/make_datasets.py /data/kiln-bench           # once (see that file)
    python3 tools/bench/scaling.py --binary target/release/kiln \\
        --data /data/kiln-bench/d1_pytree --work /fast/scratch --out bench-results/scaling-d1.json

What is measured, per codec and thread count (median of --reps runs):

  wall seconds, raw MiB/s, speedup vs 1 thread, parallel efficiency (speedup / threads), CPU
  seconds, peak RSS of the kiln process, and archive size.

Two reference lines are added so the curve can be judged, not just drawn:

  * the `store` curve (no compression: read + BLAKE3 + write only). It is the ceiling a codec can
    approach; the roadmap exit criterion is "lz4 reaches 90% of the store ceiling on all cores".
  * the ideal line (speedup == threads).

Also checked on every run: the archive must be byte-identical for every thread count of the same
codec (a documented property of the pipeline); a difference is reported as a failure.

Read the result carefully:

  * Page cache is WARM unless you pass --drop-caches (needs root). A warm run measures the CPU
    pipeline; a cold run on a slow disk measures the disk, and every curve flattens at its limit.
  * Thread counts above the number of logical CPUs are skipped (they measure the scheduler, not
    the pipeline) unless --oversubscribe is given.
  * Logical CPUs are not cores: on an SMT machine the curve bends at the physical core count.
    The CPU model and logical CPU count are recorded in the output; say which you ran on.
  * Put --work on a fast disk (tmpfs hides disk limits on purpose; a spinning disk hides CPU
    limits). State which in the write-up.
"""

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import threading
import time

DEFAULT_CODECS = ["store", "lz4", "zstd:3", "zstd:19", "lzma:6"]
DEFAULT_THREADS = [1, 2, 4, 8, 16]


# --------------------------------------------------------------------------------------------
# running and measuring
# --------------------------------------------------------------------------------------------
def measure(cmd):
    """Wall, CPU and the peak RSS of this one command, via wait4 (accurate per child)."""
    start = time.perf_counter()
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.PIPE)
    # Drain stderr on a thread so a chatty child can never block on a full pipe.
    err_chunks = []
    t = threading.Thread(target=lambda: err_chunks.append(proc.stderr.read()), daemon=True)
    t.start()
    _, status, usage = os.wait4(proc.pid, 0)
    wall = time.perf_counter() - start
    t.join(timeout=5)
    if hasattr(os, "waitstatus_to_exitcode"):
        rc = os.waitstatus_to_exitcode(status)
    else:
        rc = (status >> 8) & 0xFF if os.WIFEXITED(status) else -1
    rss_div = (1024 * 1024) if sys.platform == "darwin" else 1024
    return {
        "wall": wall,
        "cpu": usage.ru_utime + usage.ru_stime,
        "rss_mib": usage.ru_maxrss / rss_div,
        "rc": rc,
        "err": b"".join(err_chunks).decode(errors="replace").strip()[-300:],
    }


def drop_caches():
    try:
        os.sync()
        with open("/proc/sys/vm/drop_caches", "w") as f:
            f.write("3\n")
    except OSError as e:
        sys.exit(f"--drop-caches needs root on Linux ({e})")


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def raw_size(path):
    if os.path.isfile(path):
        return os.path.getsize(path)
    return sum(os.path.getsize(os.path.join(dp, f)) for dp, _, fn in os.walk(path) for f in fn
               if not os.path.islink(os.path.join(dp, f)))


def remove(path):
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path, ignore_errors=True)
    elif os.path.lexists(path):
        os.remove(path)


def machine_info():
    model = ""
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.lower().startswith("model name"):
                    model = line.split(":", 1)[1].strip()
                    break
    except OSError:
        model = platform.processor()
    info = {"cpu_model": model, "logical_cpus": os.cpu_count(), "platform": platform.platform(),
            "python": platform.python_version()}
    try:
        with open("/sys/devices/system/cpu/smt/active") as f:
            info["smt_active"] = f.read().strip() == "1"
    except OSError:
        pass
    try:
        with open("/proc/meminfo") as f:
            info["mem_total_mib"] = int(f.readline().split()[1]) // 1024
    except (OSError, ValueError, IndexError):
        pass
    return info


# --------------------------------------------------------------------------------------------
# the experiment
# --------------------------------------------------------------------------------------------
def med(xs):
    return statistics.median(xs)


def one_point(args, codec, threads, src, raw):
    """Create, test and extract `reps` times at one (codec, threads); return a record."""
    arc = os.path.join(args.work, "scaling.kiln")
    out = os.path.join(args.work, "scaling-out")
    rec = {"codec": codec, "threads": threads, "create": [], "test": [], "extract": []}
    digest = None
    for rep in range(args.reps):
        remove(arc)
        remove(out)
        if args.drop_caches:
            drop_caches()
        r = measure([args.binary, "create", arc, src, "-f", "-q", "-c", codec,
                     "-b", args.block, "-j", str(threads)])
        if r["rc"] not in (0, 7):
            rec["error"] = f"create exited {r['rc']}: {r['err']}"
            break
        rec["create"].append(r)
        rec["archive_bytes"] = os.path.getsize(arc)
        if rep == 0:
            digest = sha256_file(arc)
            rec["sha256"] = digest
        if not args.no_verify:
            if args.drop_caches:
                drop_caches()
            r = measure([args.binary, "test", arc, "-j", str(threads)])
            if r["rc"] != 0:
                rec["error"] = f"test exited {r['rc']}: {r['err']}"
                break
            rec["test"].append(r)
        if not args.no_extract:
            os.makedirs(out)
            if args.drop_caches:
                drop_caches()
            r = measure([args.binary, "extract", arc, "-o", out, "-j", str(threads), "-q"])
            if r["rc"] not in (0, 7):
                rec["error"] = f"extract exited {r['rc']}: {r['err']}"
                break
            rec["extract"].append(r)
    for key in ("create", "test", "extract"):
        runs = rec[key]
        if runs:
            rec[key + "_wall"] = med([x["wall"] for x in runs])
            rec[key + "_cpu"] = med([x["cpu"] for x in runs])
            rec[key + "_rss_mib"] = max(x["rss_mib"] for x in runs)
            rec[key + "_mibs"] = raw / (1 << 20) / rec[key + "_wall"]
            rec[key + "_wall_all"] = [x["wall"] for x in runs]
    remove(arc)
    remove(out)
    return rec


def derive(results, base_threads):
    """Add speedup, efficiency and the fraction of the `store` ceiling to every record."""
    by = {(r["codec"], r["threads"]): r for r in results if "error" not in r}
    for (codec, threads), r in by.items():
        one = by.get((codec, base_threads))
        store = by.get(("store", threads))
        for key in ("create", "test", "extract"):
            if key + "_wall" not in r:
                continue
            if one and key + "_wall" in one:
                sp = one[key + "_wall"] / r[key + "_wall"]
                r[key + "_speedup"] = sp
                r[key + "_efficiency"] = sp * base_threads / threads
            if store and key + "_mibs" in store and codec != "store":
                r[key + "_of_store"] = r[key + "_mibs"] / store[key + "_mibs"]
    return by


def determinism(results):
    """Archive bytes must not depend on the thread count."""
    problems = []
    first = {}
    for r in results:
        if "sha256" not in r:
            continue
        ref = first.setdefault(r["codec"], r)
        if r["sha256"] != ref["sha256"]:
            problems.append(f"{r['codec']}: -j {r['threads']} differs from -j {ref['threads']}")
    return problems


# --------------------------------------------------------------------------------------------
# reporting
# --------------------------------------------------------------------------------------------
def markdown(meta, results, by, threads_list, codecs, base_threads):
    lines = []
    m = meta["machine"]
    lines.append(f"Machine: {m.get('cpu_model') or 'unknown CPU'}, {m.get('logical_cpus')} logical "
                 f"CPUs" + (", SMT on" if m.get("smt_active") else "")
                 + (f", {m['mem_total_mib']} MiB RAM" if "mem_total_mib" in m else "")
                 + f". Data: `{meta['data']}` ({meta['raw_bytes'] / (1 << 20):.0f} MiB). "
                 f"Block {meta['block']}, median of {meta['reps']}. "
                 f"Cache: {'cold (dropped before every run)' if meta['drop_caches'] else 'warm'}.")
    lines.append("")
    for op in ("create", "extract", "test"):
        if not any(f"{op}_wall" in r for r in results):
            continue
        lines.append(f"### {op}: throughput (MiB/s of raw data), speedup, efficiency")
        lines.append("")
        head = "| codec | " + " | ".join(f"-j {t}" for t in threads_list) + " |"
        lines.append(head)
        lines.append("|---|" + "---:|" * len(threads_list))
        for codec in codecs:
            cells = []
            for t in threads_list:
                r = by.get((codec, t))
                if not r or f"{op}_mibs" not in r:
                    cells.append("-")
                    continue
                cell = f"{r[op + '_mibs']:.0f}"
                if f"{op}_speedup" in r and t != base_threads:
                    cell += f" ({r[op + '_speedup']:.2f}x, {r[op + '_efficiency'] * 100:.0f}%)"
                cells.append(cell)
            lines.append(f"| {codec} | " + " | ".join(cells) + " |")
        lines.append("")
    lines.append("Cells read `MiB/s (speedup vs -j %d, parallel efficiency)`." % base_threads)
    lines.append("")
    top = threads_list[-1]
    lz4, store = by.get(("lz4", top)), by.get(("store", top))
    if lz4 and store and "create_of_store" in lz4:
        frac = lz4["create_of_store"] * 100
        verdict = "meets" if frac >= 90 else "does not meet"
        lines.append(f"Roadmap exit criterion (lz4 create at -j {top} reaches 90% of the store "
                     f"ceiling): lz4 reaches **{frac:.0f}%**, which {verdict} it.")
        lines.append("")
    if meta["skipped_threads"]:
        lines.append("Skipped (more threads than logical CPUs): "
                     + ", ".join(str(t) for t in meta["skipped_threads"]) + ".")
        lines.append("")
    if meta["determinism_problems"]:
        lines.append("**FAILURE: archive bytes depend on the thread count:** "
                     + "; ".join(meta["determinism_problems"]))
        lines.append("")
    else:
        lines.append("Archives were byte-identical across all thread counts of each codec.")
        lines.append("")
    errs = [r for r in results if "error" in r]
    for r in errs:
        lines.append(f"* ERROR {r['codec']} -j {r['threads']}: {r['error']}")
    return "\n".join(lines) + "\n"


COLORS = ["#4e79a7", "#f28e2b", "#59a14f", "#e15759", "#b07aa1", "#76b7b2", "#9c755f"]


def svg(by, threads_list, codecs, base_threads, op="create"):
    """A dependency-free speedup chart: speedup vs threads, with the ideal line."""
    w, h, ml, mr, mt, mb = 720, 460, 64, 150, 40, 56
    pw, ph = w - ml - mr, h - mt - mb
    tmax = max(threads_list)
    ymax = float(tmax) / base_threads
    xs = {t: ml + pw * (i / max(1, len(threads_list) - 1)) for i, t in enumerate(threads_list)}

    def y(v):
        return mt + ph * (1 - min(v, ymax) / ymax)

    o = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="sans-serif" '
         f'font-size="12">',
         f'<rect width="{w}" height="{h}" fill="white"/>',
         f'<text x="{w / 2}" y="22" text-anchor="middle" font-size="15">kiln {op}: speedup vs '
         f'threads</text>']
    for k in range(0, int(ymax) + 1, max(1, int(ymax) // 8)):
        o.append(f'<line x1="{ml}" x2="{ml + pw}" y1="{y(k):.1f}" y2="{y(k):.1f}" stroke="#e6e6e6"/>')
        o.append(f'<text x="{ml - 8}" y="{y(k) + 4:.1f}" text-anchor="end">{k}x</text>')
    for t in threads_list:
        o.append(f'<line x1="{xs[t]:.1f}" x2="{xs[t]:.1f}" y1="{mt}" y2="{mt + ph}" stroke="#f0f0f0"/>')
        o.append(f'<text x="{xs[t]:.1f}" y="{mt + ph + 18}" text-anchor="middle">{t}</text>')
    o.append(f'<text x="{ml + pw / 2}" y="{h - 12}" text-anchor="middle">threads (-j)</text>')
    ideal = " ".join(f"{xs[t]:.1f},{y(t / base_threads):.1f}" for t in threads_list)
    o.append(f'<polyline points="{ideal}" fill="none" stroke="#999" stroke-dasharray="5 4"/>')
    o.append(f'<text x="{ml + pw + 10}" y="{mt + 12}" fill="#777">ideal</text>')
    for i, codec in enumerate(codecs):
        pts = [(xs[t], y(by[(codec, t)][op + "_speedup"])) for t in threads_list
               if (codec, t) in by and op + "_speedup" in by[(codec, t)]]
        if not pts:
            continue
        c = COLORS[i % len(COLORS)]
        o.append('<polyline points="' + " ".join(f"{a:.1f},{b:.1f}" for a, b in pts)
                 + f'" fill="none" stroke="{c}" stroke-width="2"/>')
        for a, b in pts:
            o.append(f'<circle cx="{a:.1f}" cy="{b:.1f}" r="3.5" fill="{c}"/>')
        o.append(f'<text x="{ml + pw + 10}" y="{mt + 32 + 18 * i}" fill="{c}">{codec}</text>')
    o.append("</svg>")
    return "\n".join(o) + "\n"


# --------------------------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--binary", default="target/release/kiln", help="the kiln executable")
    ap.add_argument("--data", required=True, help="file or directory to archive")
    ap.add_argument("--work", required=True, help="scratch directory (fast disk; needs space "
                    "for one archive and one extraction)")
    ap.add_argument("--out", default="bench-results/scaling.json", help="JSON results; a .md "
                    "table and .svg chart are written beside it")
    ap.add_argument("--codecs", nargs="+", default=DEFAULT_CODECS,
                    help="codec specs (default: %(default)s); `store` is the ceiling and is "
                    "always measured")
    ap.add_argument("--threads", nargs="+", type=int, default=DEFAULT_THREADS)
    ap.add_argument("--block", default="4MiB", help="chunk size passed to -b (default 4MiB; "
                    "lzma and zstd:19 usually want 32MiB)")
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--oversubscribe", action="store_true",
                    help="also run thread counts above the logical CPU count")
    ap.add_argument("--drop-caches", action="store_true", help="cold cache before every run "
                    "(Linux, root)")
    ap.add_argument("--no-verify", action="store_true", help="skip the `test` runs")
    ap.add_argument("--no-extract", action="store_true", help="skip the `extract` runs")
    args = ap.parse_args()

    if not os.access(args.binary, os.X_OK):
        sys.exit(f"{args.binary} is not executable (cargo build --release)")
    if not os.path.exists(args.data):
        sys.exit(f"{args.data} does not exist")
    os.makedirs(args.work, exist_ok=True)
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)

    cpus = os.cpu_count() or 1
    threads_list = sorted(set(args.threads))
    skipped = [t for t in threads_list if t > cpus and not args.oversubscribe]
    threads_list = [t for t in threads_list if t not in skipped]
    if not threads_list:
        sys.exit(f"every requested thread count exceeds the {cpus} logical CPUs here")
    base = threads_list[0]
    if base != 1:
        print(f"note: no -j 1 point; speedups are relative to -j {base}", file=sys.stderr)
    codecs = list(dict.fromkeys(["store"] + args.codecs))
    src = os.path.abspath(args.data)
    raw = raw_size(src)

    results = []
    total = len(codecs) * len(threads_list)
    n = 0
    for codec in codecs:
        for t in threads_list:
            n += 1
            rec = one_point(args, codec, t, src, raw)
            results.append(rec)
            if "error" in rec:
                print(f"[{n}/{total}] {codec:8s} -j {t:<3d} ERROR {rec['error']}", flush=True)
            else:
                print(f"[{n}/{total}] {codec:8s} -j {t:<3d} create {rec['create_wall']:7.2f}s "
                      f"{rec['create_mibs']:7.0f} MiB/s  cpu {rec['create_cpu']:7.2f}s  "
                      f"rss {rec['create_rss_mib']:6.0f} MiB  size {rec['archive_bytes'] / 1e6:9.2f} MB",
                      flush=True)
            # Write as we go: a long run that dies still leaves its finished points.
            json.dump({"results": results}, open(args.out, "w"), indent=1)

    by = derive(results, base)
    meta = {
        "binary": args.binary, "data": args.data, "raw_bytes": raw, "block": args.block,
        "reps": args.reps, "drop_caches": args.drop_caches, "threads": threads_list,
        "skipped_threads": skipped, "codecs": codecs, "machine": machine_info(),
        "when": time.strftime("%Y-%m-%d %H:%M:%S %z"),
        "determinism_problems": determinism(results),
    }
    json.dump({"meta": meta, "results": results}, open(args.out, "w"), indent=1)
    stem = os.path.splitext(args.out)[0]
    md = markdown(meta, results, by, threads_list, codecs, base)
    open(stem + ".md", "w").write(md)
    open(stem + ".svg", "w").write(svg(by, threads_list, codecs, base))
    print("\n" + md)
    print(f"wrote {args.out}, {stem}.md, {stem}.svg")
    return 1 if meta["determinism_problems"] or any("error" in r for r in results) else 0


if __name__ == "__main__":
    sys.exit(main())
