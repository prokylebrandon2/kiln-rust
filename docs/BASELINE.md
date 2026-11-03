# Baseline: original Kiln v14, the Rust foundation, and reference tools

## Method

* **Machine:** 1 vCPU (Intel Xeon @ 2.10 GHz), 4 GB RAM, Ubuntu 24.04, Python 3.12.3, Rust 1.91.1.
  **There is one core, so multicore scaling is NOT measured here.** Every tool was run
  single-threaded (`-j 1`, `-T1`, `-mmt=1`). The multi-threaded pipeline is verified for
  correctness and determinism (tests run it with 1, 2, 3 and 7 threads and require identical
  output), not for speedup.
* **Corpus** (`tools/baseline/make_corpus.py`): `text` = 361 Python stdlib sources, 5,989,576 B;
  `binary` = 23 shared libraries, 5,997,751 B; `random` = 2 MiB of seeded noise. The corpus is
  small, so fixed costs matter; treat ratios as indicative.
* **Measurement:** best (lowest wall time) of 3 runs per command, from a warm page cache; wall
  time, CPU time and peak RSS of *that child process only* (`wait4` rusage). Round trips are
  compared byte-for-byte (`filecmp`). Reproduce with `tools/baseline/compare.py`.
* **Tools:** Kiln original v14 (Python 3.12 + C helper), `kiln` Rust 0.1.0, gzip 1.12, bzip2
  1.0.8, xz 5.4.5, zstd 1.5.5, lz4 1.9.4, Info-ZIP zip 3.0, 7-Zip 23.01. gzip/bzip2/xz/zstd/lz4 are
  single-stream compressors and were run on a `tar` of the corpus; zip, 7-Zip and both Kilns are
  archivers working on the directory.
* Original Kiln's decompression time includes about 85 ms of interpreter start-up.

## Results

Sorted by size (smaller is better). Full data: `tools/baseline/results/baseline-2026-11-02.json`.


#### text  (raw 5,989,576 bytes)

| tool | size (bytes) | ratio | compress s | decompress s | RSS comp MiB | RSS decomp MiB | round-trip |
|---|---:|---:|---:|---:|---:|---:|:--:|
| kiln-orig v14 (-j1) | 922,253 | 6.495 | 3.61 | 0.31 | 189 | 56 | yes |
| 7z -mx=9 -mmt=1 (LZMA2) | 938,089 | 6.385 | 2.42 | 0.06 | 74 | 14 | yes |
| xz -6 -T1 (tar) | 943,372 | 6.349 | 2.49 | 0.05 | 73 | 14 | n/a |
| kiln-rs lzma:6 blk=32MiB (-j1) | 951,859 | 6.293 | 2.71 | 0.05 | 78 | 16 | yes |
| 7z -mx=5 -mmt=1 (LZMA2) | 956,411 | 6.263 | 1.88 | 0.06 | 74 | 14 | yes |
| zstd -19 -T1 (tar) | 964,287 | 6.211 | 2.33 | 0.01 | 91 | 14 | n/a |
| kiln-rs zstd:19 blk=32MiB (-j1) | 971,543 | 6.165 | 2.35 | 0.02 | 91 | 14 | yes |
| kiln-rs lzma:6 blk=4MiB (-j1) | 989,088 | 6.056 | 2.16 | 0.05 | 55 | 14 | yes |
| kiln-rs zstd:19 blk=4MiB (-j1) | 1,011,992 | 5.919 | 1.80 | 0.02 | 59 | 14 | yes |
| bzip2 -9 (tar) | 1,099,549 | 5.447 | 0.35 | 0.17 | 14 | 14 | n/a |
| zstd -3 -T1 (tar) | 1,311,132 | 4.568 | 0.03 | 0.01 | 14 | 14 | n/a |
| kiln-rs zstd:3 blk=4MiB (-j1) | 1,320,171 | 4.537 | 0.04 | 0.02 | 14 | 14 | yes |
| gzip -6 (tar) | 1,433,615 | 4.178 | 0.16 | 0.03 | 14 | 14 | n/a |
| zip -6 (Info-ZIP) | 1,562,081 | 3.834 | 0.17 | 0.05 | 13 | 13 | yes |
| lz4 -1 (tar) | 2,255,364 | 2.656 | 0.02 | 0.01 | 14 | 14 | n/a |
| kiln-rs lz4 blk=4MiB (-j1) | 2,268,329 | 2.641 | 0.02 | 0.02 | 14 | 14 | yes |

#### binary  (raw 5,997,751 bytes)

| tool | size (bytes) | ratio | compress s | decompress s | RSS comp MiB | RSS decomp MiB | round-trip |
|---|---:|---:|---:|---:|---:|---:|:--:|
| 7z -mx=9 -mmt=1 (LZMA2) | 1,118,999 | 5.360 | 1.81 | 0.09 | 80 | 14 | yes |
| kiln-orig v14 (-j1) | 1,150,011 | 5.215 | 3.59 | 0.29 | 187 | 54 | yes |
| 7z -mx=5 -mmt=1 (LZMA2) | 1,156,972 | 5.184 | 1.52 | 0.09 | 76 | 13 | yes |
| kiln-rs lzma:6 blk=32MiB (-j1) | 1,260,685 | 4.758 | 2.48 | 0.07 | 78 | 16 | yes |
| xz -6 -T1 (tar) | 1,261,440 | 4.755 | 2.34 | 0.10 | 70 | 13 | n/a |
| kiln-rs lzma:6 blk=4MiB (-j1) | 1,272,442 | 4.714 | 2.13 | 0.06 | 55 | 13 | yes |
| kiln-rs zstd:19 blk=32MiB (-j1) | 1,413,368 | 4.244 | 1.98 | 0.01 | 92 | 13 | yes |
| kiln-rs zstd:19 blk=4MiB (-j1) | 1,426,717 | 4.204 | 1.77 | 0.02 | 59 | 13 | yes |
| zstd -19 -T1 (tar) | 1,428,584 | 4.198 | 2.13 | 0.01 | 92 | 13 | n/a |
| bzip2 -9 (tar) | 1,808,359 | 3.317 | 0.71 | 0.19 | 13 | 13 | n/a |
| kiln-rs zstd:3 blk=4MiB (-j1) | 1,822,744 | 3.291 | 0.03 | 0.01 | 13 | 13 | yes |
| zip -6 (Info-ZIP) | 1,834,306 | 3.270 | 0.21 | 0.04 | 13 | 13 | yes |
| gzip -6 (tar) | 1,834,582 | 3.269 | 0.19 | 0.03 | 13 | 13 | n/a |
| zstd -3 -T1 (tar) | 1,872,399 | 3.203 | 0.04 | 0.01 | 13 | 13 | n/a |
| lz4 -1 (tar) | 2,869,207 | 2.090 | 0.02 | 0.01 | 13 | 13 | n/a |
| kiln-rs lz4 blk=4MiB (-j1) | 2,874,714 | 2.086 | 0.02 | 0.01 | 13 | 13 | yes |

#### random  (raw 2,097,152 bytes)

| tool | size (bytes) | ratio | compress s | decompress s | RSS comp MiB | RSS decomp MiB | round-trip |
|---|---:|---:|---:|---:|---:|---:|:--:|
| kiln-orig v14 (-j1) | 2,097,245 | 1.000 | 0.19 | 0.08 | 108 | 39 | yes |
| kiln-rs lz4 blk=4MiB (-j1) | 2,097,340 | 1.000 | 0.01 | 0.00 | 13 | 13 | yes |
| kiln-rs zstd:3 blk=4MiB (-j1) | 2,097,340 | 1.000 | 0.01 | 0.00 | 13 | 13 | yes |
| kiln-rs zstd:19 blk=4MiB (-j1) | 2,097,340 | 1.000 | 0.15 | 0.00 | 40 | 13 | yes |
| kiln-rs zstd:19 blk=32MiB (-j1) | 2,097,340 | 1.000 | 0.15 | 0.00 | 40 | 13 | yes |
| kiln-rs lzma:6 blk=4MiB (-j1) | 2,097,340 | 1.000 | 0.41 | 0.00 | 31 | 13 | yes |
| kiln-rs lzma:6 blk=32MiB (-j1) | 2,097,340 | 1.000 | 0.51 | 0.00 | 30 | 13 | yes |
| zstd -19 -T1 (tar) | 2,097,440 | 1.000 | 0.24 | 0.00 | 56 | 13 | n/a |
| 7z -mx=5 -mmt=1 (LZMA2) | 2,097,469 | 1.000 | 0.45 | 0.01 | 28 | 13 | yes |
| 7z -mx=9 -mmt=1 (LZMA2) | 2,097,469 | 1.000 | 0.40 | 0.01 | 28 | 13 | yes |
| gzip -6 (tar) | 2,097,717 | 1.000 | 0.06 | 0.01 | 13 | 13 | n/a |
| zip -6 (Info-ZIP) | 2,097,796 | 1.000 | 0.06 | 0.01 | 13 | 13 | yes |
| zstd -3 -T1 (tar) | 2,098,253 | 0.999 | 0.01 | 0.00 | 13 | 13 | n/a |
| xz -6 -T1 (tar) | 2,098,768 | 0.999 | 0.55 | 0.01 | 37 | 13 | n/a |
| lz4 -1 (tar) | 2,099,219 | 0.999 | 0.00 | 0.00 | 13 | 13 | n/a |
| bzip2 -9 (tar) | 2,107,145 | 0.995 | 0.17 | 0.17 | 13 | 13 | n/a |

## What the numbers say

1. **The original has the best ratio on text and is second on binaries.** Text: 6.495x vs
   7-Zip `-mx=9` 6.385x. Binaries: 5.215x, behind 7-Zip `-mx=9` (5.360x), because 7-Zip applies an x86 BCJ filter
   (see point 5). Against ZIP the original is 1.7x (text) and 1.6x
   (binary) smaller.
2. **The original is slow and memory-hungry by comparison**: ~1.6 MiB/s compression (3.6 s for
   6 MB), 187-189 MiB peak RSS, and 25-28 MiB/s decode after start-up. The same data decodes in
   0.01-0.02 s with zstd and 0.05-0.07 s with LZMA2.
3. **The Rust foundation, using only off-the-shelf codecs, is already in the right class** but
   does not beat the original's ratio:
   * `lzma:6`, 32 MiB blocks: text 6.293x (archive 3.2% larger than the original, **0.5% smaller than 7-Zip
     `-mx=5`**), binary 4.758x (archive 9.6% larger than the original).
   * `zstd:19`, 32 MiB blocks: text 6.165x, binary 4.244x, decoding at 300-470 MiB/s.
   * `zstd:3`: 4.54x text at ~160 MiB/s compress and ~340 MiB/s decode (ZIP: 3.83x, 33 and 126
     MiB/s). `lz4` matches the `lz4` CLI.
   * Memory: 13-14 MiB (lz4, zstd:3), 55-92 MiB (zstd:19, lzma:6) at one thread, versus 189 MiB.
4. **Independent blocks cost ratio, bigger blocks recover it.** Going from 4 MiB to 32 MiB blocks
   shrank text archives by 4.0% (`zstd:19`) and 3.8% (`lzma:6`) but binaries by only 0.9%, at the
   price of 23-32 MiB more memory per thread. This is the central trade-off of the block architecture and the first thing
   Phase 3 must attack (long-range matching, grouping similar files, solid groups).
5. **The binary gap to 7-Zip is a missing filter, not a codec limit.** Tested directly on the
   binary corpus (tar): plain `xz -6` 1,261,440 B; `xz -6 --x86` **1,152,676 B (-8.6%)**;
   7-Zip `-mx=5` 1,156,972 B, and with its filters disabled (`-mf=off`) 1,257,946 B - back to
   plain. The Rust `lzma:6` (no filter) equals plain xz (1,260,685 vs 1,261,440). liblzma
   already ships BCJ filters, so this is a cheap, well-defined Phase 3 item.
6. **Fast codecs are limited by the pipeline, not the codec.** On this core even `store`
   (no compression) tops out around 400 MiB/s inside `kiln bench`, because the producer thread
   reads, hashes (BLAKE3) and zero-fills each block. `lz4_flex` (pure Rust) ran at ~180 MiB/s there,
   below `zstd:1` (~230 MiB/s). Parallel hashing and buffer reuse are Phase 2 items.
7. **Incompressible data is handled correctly**: noise costs 188 bytes of overhead (0.009%), and
   blocks that do not shrink are stored, so no codec ever expands the data.
8. **Not yet measured:** multi-core speedup, inputs larger than RAM, many-file workloads beyond
   1,500 files, SSD/HDD effects, and performance on Windows/macOS.
