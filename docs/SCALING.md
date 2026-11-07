# Multicore scaling

> **Status: harness written, curve not measured.** Every benchmark so far (`PHASE2.md`,
> `PHASE3.md`) ran on 2 vCPUs, so scaling beyond 2 threads is unknown. This slice adds the tool
> that measures it, `tools/bench/scaling.py`, and the method below. It was written on a
> one-core machine and has only been smoke-tested against a stand-in executable, so **there are
> no results in this file yet**. The table at the end is a template, not data.

## Why this matters

The pipeline is one producer thread (read, BLAKE3, block assembly) feeding N workers that
compress, with an ordered writer behind them. With a fast codec (lz4, zstd:1-3) the producer, not
the workers, is expected to become the limit, so lz4 may flatten well before 16 threads; with a
slow codec (zstd:19, lzma:6) the workers dominate and the curve should stay close to ideal until
the physical core count. Extraction has the mirror-image shape: parallel decode, a CRC check, and
ordered writes. These are hypotheses. The roadmap exit criterion for this item is:

* a documented speedup curve for lz4, zstd:3, zstd:19 and lzma:6 at 1/2/4/8/16 threads, and
* lz4 reaching at least 90% of the `store` ceiling on all cores, and
* the memory model (`--memory-limit`, see `PHASE2.md`) validated at more than 2 threads.

The third point is *not* covered by the script's speedup table; see "Memory" below.

## Running it

Use a machine with at least 16 logical CPUs, ideally a dedicated one (a cloud instance with 16
or more vCPUs, or a workstation with nothing else running). Not a laptop on battery, and not a
shared CI runner.

```sh
cargo build --release
python3 tools/bench/make_datasets.py /data/kiln-bench

# the main curve: many mid-sized files, fits in RAM, warm cache
python3 tools/bench/scaling.py --binary target/release/kiln \
    --data /data/kiln-bench/d1_pytree --work /dev/shm/kiln-scratch \
    --block 4MiB --reps 5 --out bench-results/scaling-d1.json

# the slow codecs want large blocks, or there is too little work per block to scale
python3 tools/bench/scaling.py --binary target/release/kiln \
    --data /data/kiln-bench/d4_large.bin --work /fast/nvme/scratch \
    --codecs zstd:19 lzma:6 --block 32MiB --reps 3 --out bench-results/scaling-d4.json

# incompressible data: every codec should degrade to the store curve (the pre-check)
python3 tools/bench/scaling.py --binary target/release/kiln \
    --data /data/kiln-bench/d5_random --work /dev/shm/kiln-scratch --out bench-results/scaling-d5.json
```

Each run writes `<out>.json` (every individual timing), `<out>.md` (the tables below, ready to
paste) and `<out>.svg` (speedup versus threads, with the ideal line).

Options worth knowing: `--threads 1 2 4 8 16 32`, `--codecs ...`, `--oversubscribe` (run thread
counts above the logical CPU count), `--drop-caches` (cold cache, Linux, root), `--no-verify`,
`--no-extract`.

## What is measured, and how to read it

For each codec and thread count the median of `--reps` runs of `kiln create`, `kiln test` and
`kiln extract`: wall seconds, raw MiB/s, **speedup** (wall time at the first thread count divided
by wall time here), **efficiency** (speedup divided by threads), CPU seconds, and peak RSS of the
kiln process (from `wait4`, per child).

* The **`store`** codec is always run. It does no compression, so its curve is the cost of read +
  BLAKE3 + write alone. A codec cannot beat it; how close lz4 gets to it is the exit criterion.
* The **ideal** line (speedup = threads) is drawn on the chart.
* Efficiency falling below about 50% marks the point where adding threads stops being worth it.
* **Determinism** is checked on every run: for one codec, the archive must be byte-identical at
  every thread count. A difference is a bug in the pipeline and fails the run (exit status 1).

### Things that make a curve lie

| Trap | What it does | What to do |
|---|---|---|
| Warm cache on a fast disk | Measures the CPU pipeline only (usually what you want here) | Say "warm" in the write-up; use `--drop-caches` for a cold curve |
| Slow or shared disk | Every curve flattens at the disk's bandwidth | Put `--work` on NVMe or tmpfs; say which |
| tmpfs for `--work` | Hides write limits | Say so; repeat once on a real disk |
| SMT (hyper-threading) | The curve bends at the *physical* core count; 16 logical CPUs may be 8 cores | The JSON records the CPU model and SMT state; read the curve against cores, not logical CPUs |
| Frequency scaling / turbo | Single-thread runs boost higher, flattering nothing and punishing efficiency | Fix the governor to `performance` if you can; otherwise note it |
| Noisy neighbours (cloud) | High variance between reps | Use `--reps 5`, inspect `create_wall_all` in the JSON |
| Too few blocks | `d4_large.bin` at 32 MiB blocks is only ~13 blocks: 16 threads cannot all be busy | Use smaller blocks for the thread sweep, and report the block size |
| Thread count above logical CPUs | Measures the scheduler | Skipped by default |

## Memory

The script records peak RSS per point, but it does not by itself validate the memory *model*
(the estimate behind `--memory-limit`). To do that at more than 2 threads, run a few points with
a limit and compare the model's estimate against the measured peak:

```sh
for j in 4 8 16; do
  target/release/kiln create -q -f -j $j -c zstd:3 -b 8MiB /tmp/t.kiln /data/kiln-bench/d1_pytree
  # peak RSS: see the rss_mib column in scaling-d1.json
done
target/release/kiln create --memory-limit 256MiB -j 16 -c zstd:3 -b 8MiB /tmp/t.kiln /data/kiln-bench/d1_pytree
```

The model is in `PHASE2.md` ("Memory model": 16 MiB + 2 x block x in-flight blocks + threads x
codec working memory, fitted at up to 2 threads and documented as an estimate, not a cap).
Comparing its estimate with the measured peak at 4, 8 and 16 threads is the validation the
roadmap asks for; if the measured peak exceeds the estimate, the per-thread constant needs
refitting. That needs a person to run it and judge it.

## Results

*Not measured.* Replace this section with the contents of the `.md` files, plus: the machine
(CPU model, cores, SMT, RAM), the disk, the kernel, the kiln commit, and whether the cache was
warm. Template:

| codec | -j 1 | -j 2 | -j 4 | -j 8 | -j 16 |
|---|---:|---:|---:|---:|---:|
| store | | | | | |
| lz4 | | | | | |
| zstd:3 | | | | | |
| zstd:19 | | | | | |
| lzma:6 | | | | | |

Exit criterion: lz4 at 16 threads reaches ___% of the store ceiling (target 90%).

## What to do with the result

* If lz4 is well below 90% of `store` at high thread counts, the producer is the bottleneck: the
  next steps are the ones already on the roadmap, hashing on the workers (combined in order),
  not zero-filling blocks, a buffer pool, read-ahead, and parallel file opening for many small
  files. Re-run this script after each change; the JSON files are comparable.
* If a slow codec does not scale to the physical core count, look at block count and the
  in-flight window (`--memory-limit` shrinks it) before suspecting the code.
* If archives differ between thread counts, stop: that is a correctness bug, not a performance
  result.
