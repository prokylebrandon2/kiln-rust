# Phase 3 — custom format and performance

Kiln 0.3.0. This is a **partial** Phase 3 by request ("do around 50%, then ship"): see
[What was not done](#what-was-not-done). Every number below comes from a run recorded in
`bench-results/` (raw JSON/logs) and can be reproduced with `tools/bench/`. Where a measurement
was noisy or inconclusive, it says so.

## 1. Is a custom format justified?

Decision: **yes, but evolved from Phase 2's format rather than rewritten** (version 1 `.kr` →
version 2 `.kiln`). Reasons, each tied to a requirement:

| Need | ZIP | 7z | tar + zstd | `.kiln` v2 |
|---|---|---|---|---|
| Listing without reading data | yes (central directory) | yes | no | yes (index only) |
| Extract one file without decoding everything | yes | only from the start of its solid block | no | yes (decode only overlapping chunks) |
| Parallel (de)compression incl. inside one large file | no (per file) | partly (LZMA2 blocks) | compression only with zstd `-T` | yes (independent chunks) |
| Many small files compress well | poor (per file) | very good (solid) | good | good (shared chunks) |
| Per-chunk + per-file integrity | CRC-32 per file | CRC-32 | none | CRC-32 per chunk, BLAKE3-128 per file, CRC on header/footer/index |
| Skippable, versioned extensions | extra fields | limited | n/a | yes (critical bit, incompat/compat flags) |

What the custom format costs: other tools cannot open it, and compression ratio is lower than 7-Zip's
solid LZMA2 (measured in §6). The format is documented byte-for-byte in [`FORMAT.md`](FORMAT.md).

### Changes from format v1 (each driven by a measured gap)

* 24-byte header with its own CRC (v1: the informational profile byte was unprotected) and
  *incompat/compat feature flag words*.
* **Variable-size chunks**, chunk offsets *implied* by lengths (cannot disagree with data); the
  chunk table lives in the index (`raw_len`, `comp_len`, `method`).
* **Columnar, varint-coded, zstd-compressed index** with delta timestamps: the index is 40–49 %
  smaller than v1's on the benchmark sets (`kiln info` prints it).
* Per-file **content class** (text / data / binary / compressed / random) so already-compressed data
  is grouped and *stored* in its own chunks.
* **TLV extensions** with a critical bit and a creator tag; unknown critical ⇒ refuse, unknown
  optional ⇒ skip.
* 128-bit BLAKE3 per file; CRC-32 per chunk, verified before any decoder runs.
* Defined behaviour for incomplete/corrupt archives (FORMAT.md); chunk record headers are
  cross-checked against the index.

## 2. Compression: codecs, profiles, detection

Codecs (all linked statically; `kiln-phase3 create -c ...`): `store`, `lz4` (lz4_flex 0.14.0),
`zstd` (zstd 0.14.3 / libzstd 1.5.7), `lzma` (liblzma 0.4.9, LZMA2 in xz container).

Measured codec sweep on d1 (source tree, 131 MB), Phase 2 binary, 4 MiB chunks, 2 vCPU
(`bench-results/p2-codec-sweep.json`; single run each):

| Codec | Size MB | Ratio | Create s |
|---|---:|---:|---:|
| lz4 | 47.66 | 2.75 | 0.36 |
| zstd:1 | 32.38 | 4.05 | 0.38 |
| zstd:3 | 29.16 | 4.50 | 0.49 |
| zstd:9 | 25.12 | 5.22 | 1.76 |
| zstd:19 (4 MiB chunks) | 21.58 | 6.08 | 36.9 |
| lzma:6 | 22.02 | 5.96 | 32.0 |
| lzma:6, 16 MiB chunks | 21.16 | 6.20 | 40.5 |

On this machine **zstd:1 is no slower than lz4 but compresses 30 % smaller**, so LZ4 is kept as a
codec but no profile uses it. LZMA2 gives the best ratio and costs ~40× more time than zstd:3.

**Profiles** (`-p`): `fastest` = zstd:1; `balanced` = zstd:3 (default); `maximum` = zstd:19 + 16 MiB
chunks + content classification; `store`; `adaptive` = zstd:3 + classification. `-c`/`-b` override.

**Already-compressed detection** does not rely on extensions: magic numbers (PNG, JPEG, GIF, ZIP,
gzip, bzip2, xz, zstd, LZ4, 7z, RAR, MP4/MKV/Ogg/FLAC/MP3, WOFF2, WebP, JPEG XL → *compressed*;
ELF/PE/Mach-O → *binary*), else printable-ratio (text), else byte-entropy + a fast LZ4 probe on a
≤ 64 KiB sample (*random*). Files < 8 KiB are never classed random. Effect (3 reps, d3 mixed set
with 2 vCPUs, `bench-results/cab-d3-heavy.json`): `maximum` is **30 % faster and 4.1 % smaller**
than zstd:19 without classification (9.5 s vs 14.0 s; 50.66 MB vs 52.83 MB), because incompressible
bytes skip the slow codec and chunks no longer mix data kinds.

## 3. Parallelism, I/O and memory (evidence-based)

Threads are bounded (`-j`, default = cores), errors propagate from any worker, and output is
byte-identical for any thread count (test `archives_are_byte_identical_whatever_the_thread_count`).
No `unsafe` code in the crate.

What was tried and what the evidence said:

| Change | Result | Kept? |
|---|---|---|
| Extraction writer pool (parallel file creation, lanes by parent dir) | many small files, **tmpfs**: 0.945 s → 0.437 s (**2.2×**, 7 interleaved rounds, d2 30k files). ext4, d1 8.3k files: 3.59 s → 2.40 s (−33 %, 5 rounds). | yes |
| Bounded channels in the writer pool | `strace -c` showed a `sched_yield` spin | replaced by unbounded channels + byte budget |
| Byte budget with a mutex/condvar | futex storm (36k calls) | atomic fast path + hysteresis (≈260 futex calls) |
| Hash-verify small files before creating them; `create_new` directly (no temp+rename) | fewer syscalls, no wrong file ever exists under a real name | yes |
| Buffer pool for compression | no wall-time gain (±3 %), **2.5× RSS** | **removed** |
| Creation speed | unchanged vs Phase 2 at the same codec (e.g. d1 balanced 0.50 s vs 0.54 s median of 9, within noise) — 2 cores are the ceiling | no claim of a gain |
| Selective extraction (one file) | unchanged (11.7 vs 12.0 ms on d1; 23.5 vs 23.5 ms on d2) | n/a |

Memory: `--memory-limit` shapes threads and in-flight chunks; creation RSS 35–45 MiB with zstd:3
for any input size (§5 tables), `maximum` 250–260 MiB.

## 4. Security and robustness tests

101 → **130 tests** (`cargo test --locked`), all passing, plus clippy `-D warnings` and `fmt`
clean. Tests relevant to hostile or damaged input:

* `every_single_byte_flip_is_detected`, `every_truncation_is_detected` (plain archives) and
  `every_byte_flip_and_truncation_of_a_classified_archive_is_detected`.
* `header_is_checksummed_...`, `record_headers_that_disagree_with_the_index_are_corruption`,
  `inconsistent_indexes_are_rejected`, `a_wrong_file_hash_is_caught_even_when_every_block_checksum_is_valid`.
* `path_traversal_names_are_rejected_before_anything_is_written`,
  `symlinks_inside_the_destination_are_not_followed`, `forced_extraction_replaces_a_planted_symlink_...`.
* `max_bytes_refuses_oversized_extraction_before_writing_anything` (decompression-bomb limit,
  `--max-size`), `decompression_bomb_shaped_index_is_bounded`.
* `critical_unknown_extensions_are_refused_and_unknown_optional_ones_are_ignored`; unknown
  incompatible header flags refused (unit test).
* `a_small_file_with_a_wrong_hash_never_reaches_the_file_system`,
  `a_tight_memory_limit_still_extracts_correctly`.
* `tests/golden.rs`: a checked-in archive written by kiln 0.3.0 must keep opening and extracting.

Limits: ≤ 8,000,000 entries, ≤ 256 MiB index, names ≤ 32 KiB, all counts bounded by the bytes
actually present before allocation, checked arithmetic on all sums/offsets.

## 5. Benchmarks

### Environment
* 2 vCPU Intel Xeon (reported 2.10 GHz, AVX2/AVX512), 8 GB RAM, Linux 6.18, ext4 on virtio disk
  (and tmpfs where stated), **warm page cache**; a cold-cache/real-disk/many-core run was **not**
  done.
* Rust 1.97.0 (release: opt-level 3, thin LTO, codegen-units 1); Python 3.13 harness; 7-Zip 25.01
  (x64, official release, `-mmt=2`); Info-ZIP Zip 3.0 / UnZip 6.00.
* Memory/CPU are the **child process only** (`wait4` rusage via `tools/bench/rsstime.c`).
  `sync` + settle before every extraction; every round trip verified by a SHA-256 manifest.
* Datasets (`tools/bench/make_datasets.py`): d1 Python-stdlib source, 8,305 files, 131 MB;
  d2 30,000 synthetic log files, 93 MB; d3 mixed .so/.gz/.xz/random/text, 310 files, 89 MB
  (shuffled); d4 one 403 MB **synthetic** file (random slices of text and libc plus noise — has
  long-distance repetition that favours large-window compressors); d5 64×4 MiB random, 268 MB.
* 3 interleaved repetitions, median; "heavy" variants (maximum/ultra) once.

### Noise warning
Extraction of ≥ 100 MB on this VM's ext4 disk varies by up to **10×** run to run for the *same*
command (d5: 0.16–1.9 s; d2 Phase 2: 5.5–19.8 s) depending on writeback state. The
`fastest`-vs-`adaptive` 1.6 s vs 0.16 s on d5 in the 7-Zip table was traced to this (archives
contain identical stored chunks; reversing run order moved the slowness). **Do not read
conclusions from single extraction rows on d4/d5.** Cleaner evidence: interleaved A/B (§3) and
tmpfs.

### Phase 2 vs Phase 3 (final binary, `bench-results/main-final.log`; d2 incomplete — see below)

| Dataset | Variant | Size MB | Create s | Create RSS MiB | Test s |
|---|---|---:|---:|---:|---:|
| d1 | P2 zstd:3 | 29.16 | 0.50 | 37 | 0.13 |
| d1 | P3 balanced | 28.98 | 0.51 | 39 | 0.13 |
| d1 | P3 adaptive | 28.98 | 0.56 | 37 | 0.13 |
| d1 | P2 zstd:1 | 32.38 | 0.39 | 40 | 0.12 |
| d1 | P3 fastest | 32.21 | 0.37 | 41 | 0.13 |
| d3 | P2 zstd:3 | 53.46 | 0.26 | 47 | 0.07 |
| d3 | P3 balanced | 53.45 | 0.24 | 44 | 0.06 |
| d3 | P3 adaptive | **53.12** | 0.19 | 37 | 0.06 |
| d4 | P2 zstd:3 | 134.62 | 0.97 | 36 | 0.26 |
| d4 | P3 balanced | 134.62 | 0.92 | 38 | 0.23 |
| d5 | P2 zstd:3 | 268.44 | 0.43 | 42 | 0.09 |
| d5 | P3 adaptive | 268.44 | 0.40 | 24 | 0.08 |

Summary: **same ratio and creation speed as Phase 2 at the same codec** (differences ≤ 1 %, within
noise), 0.6 % smaller on d3 with `adaptive`, lower RSS on incompressible data. The d2 run
(30,000 files) was interrupted when delivery was prioritised: its three completed rows on ext4
showed Phase 3 *slower* at extraction (14.2 s vs 9.6 s) in a disk-noise regime where the same
Phase 2 command ranged 5.5–19.8 s, so **it is inconclusive**; the tmpfs A/B in §3 (2.2×) is the
controlled measurement.

### Kiln vs 7-Zip vs ZIP (`bench-results/vs7z-*.json`, table in `vs7z-table.md`)

| Dataset | Tool | Size MB | Ratio | Create s | Create RSS MiB | Extract s |
|---|---|---:|---:|---:|---:|---:|
| d1 source (ext4) | Kiln fastest | 32.21 | 4.07 | 0.34 | 40 | 1.36 |
| | 7z -mx1 | 26.28 | 4.99 | 3.02 | 33 | 3.18 |
| | Kiln balanced | 28.98 | 4.53 | 0.48 | 37 | 2.47 |
| | 7z -mx5 (default) | 20.42 | 6.43 | 64.55 | 390 | 4.14 |
| | Info-ZIP -6 | 34.76 | 3.77 | 3.74 | 8 | 4.64 |
| | Kiln maximum | 21.41 | 6.13 | 37.20 | 260 | 2.70 |
| | 7z -mx9 | 19.35 | 6.78 | 91.45 | 1274 | 4.24 |
| d3 mixed (ext4) | Kiln fastest | 55.23 | 1.62 | 0.18 | 41 | 0.12 |
| | 7z -mx1 | 51.44 | 1.74 | 3.35 | 22 | 0.42 |
| | Kiln adaptive | 53.12 | 1.68 | 0.31 | 33 | 0.15 |
| | 7z -mx5 | 49.25 | 1.81 | 13.17 | 381 | 0.61 |
| | Info-ZIP -6 | 55.68 | 1.60 | 2.90 | 2 | 0.63 |
| | Kiln maximum | 50.66 | 1.76 | 8.71 | 249 | 0.17 |
| | 7z -mx9 | 48.92 | 1.83 | 17.63 | 603 | 0.61 |
| d4 synthetic large (ext4) | Kiln balanced | 134.62 | 2.99 | 0.81 | 35 | 2.22 |
| | 7z -mx5 | 72.70 | 5.54 | 51.42 | 381 | 4.69 |
| | Kiln maximum (separate run, 1×) | 95.13 | 4.23 | 24.3 | — | — |
| d2 30k tiny files (tmpfs) | Kiln balanced | 27.87 | 3.33 | 0.51 | 48 | 0.40 |
| | 7z -mx5 | 23.03 | 4.03 | 128.27 | 409 | 2.21 |
| | Info-ZIP -6 | 40.75 | 2.28 | 3.38 | 21 | 1.57 |
| d5 random (ext4) | all tools | ≈268.4 | 1.00 | Kiln 0.35–0.46, 7z -mx5 31.75, -mx9 61.08 (RSS 2571 MiB) | | noisy, see above |

Every row's round trip verified (`rt=OK` in `bench-results/vs7z.log`).

**Honest reading:**
* **7-Zip compresses smaller everywhere** on compressible data: at its default level it is 8 %
  (d3) to 30 % (d1) to 46 % (d4) smaller than Kiln `balanced`. Solid LZMA2 with a large window is
  simply stronger than independent 4 MiB zstd chunks; on the synthetic d4 (long-range repeats)
  the gap is largest.
* **Kiln is far faster to create**: `balanced` is 53–250× faster than 7z -mx5 (d3 53×, d4 63×,
  d1 134×, d2 250×), and `fastest` is 9–19× faster than 7z -mx1 while using similar memory.
  Kiln `maximum` approaches 7-Zip's ratio (d1 4.9 % larger than -mx5, d3 2.9 % larger) in
  34–42 % less time and ~1.5× less memory than -mx5 (4.9× less than -mx9 on d1), but is still
  larger than 7z -mx9 (d1 +10.6 %).
* Kiln `test` (verify) and extraction are faster than 7-Zip's in the tables above (extraction
  gaps on d4/d5 are within disk noise, d1/d3/d2 are consistent).
* Info-ZIP (single-threaded) is both slower and larger than Kiln `balanced` here.
* 7-Zip was run with `-mmt=2`, default dictionary for each level; Kiln with all cores. No tuning of
  either beyond that.

## 6. New CLI surface
`-p/--profile`, `--classify`/`--no-classify`, `--max-size` (extract/test), `list -l` (class and
method per entry), `info` (class totals, index size, chunk methods). Library: `Profile`,
`PackOptions::from_profile`, `ExtractOptions::max_bytes`, `kiln::classify`.

## 7. What was not done
* Phase 3 was built to ~50 % scope on request. Not done: BCJ/delta filters, deduplication,
  dictionaries, symlink/hard-link/ownership/xattr storage, a salvage/repair mode, fuzzing,
  cross-platform runs (Linux only), >4 GiB archives re-tested in Phase 3 (Phase 2 tested one
  4.25 GiB file on the v1 format), cold-cache and many-core benchmarks, HDD/SSD comparison.
* The final d2 ext4 benchmark was interrupted (see §5).
* LZ4/LZMA2 chunks have no golden archive.
* The Android project: not built when this phase was written; it is built on every commit now (see the README).
* Phase 4 has not been started.

## 8. Reproducing
```sh
cargo build --release --locked && cargo test --locked
mkdir bench && cd bench && cp -r ../tools/bench tools && mkdir bin data work results
cc -O2 -o bin/rsstime tools/rsstime.c
cp ../target/release/kiln bin/kiln-phase3        # and a Phase 2 build as bin/kiln-phase2
python3 tools/make_datasets.py data              # needs /usr/lib/x86_64-linux-gnu/libc.so.6
sh tools/run_main.sh results/main.json           # Phase 2 vs 3
# 7-Zip comparison: put 7zz at bin/7zz, then
sh tools/run_vs7z.sh
```
`d1` bytes depend on the Python version used to generate it; absolute sizes will differ slightly.
