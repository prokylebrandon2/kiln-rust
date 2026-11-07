# Roadmap

Phase 1 (this delivery) is the audit and a working, tested foundation. Phases 2-5 below are a
**plan**, with each target tied to a measurement in `BASELINE.md`. Targets are hypotheses to
validate, not promises.

## Phase 1 - Audit and foundation  (done)

Audit of the original; baseline against ZIP/7-Zip/xz/zstd/lz4; Rust workspace with a
replaceable codec layer (store, lz4, zstd, lzma), a provisional block container, parallel
ordered pipeline, traversal, integrity, CLI (`pack unpack list test bench`), 81 tests including
exhaustive corruption tests.

## Phase 2 - Working archiver  (done; see `PHASE2.md`)

`create / extract / list / test / info`, exit codes, end-to-end tests, validated on a mixed
corpus and on a 4.25 GiB file. Also delivered from the original throughput plan:
`--memory-limit`, progress reporting, the incompressibility pre-check, and block statistics in
`info`. An Android APK workflow and shell app were included; they are built on every commit now.

## Throughput and memory (remaining items)

*Why:* a single core caps fast codecs near 400-700 MiB/s (producer does read + BLAKE3 +
zero-fill); scaling is only measured up to 2 threads.

* Measure scaling on a multicore machine (1/2/4/8/16 threads) for lz4, zstd:3, zstd:19, lzma:6.
  **Harness written** (`tools/bench/scaling.py`, `docs/SCALING.md`); **not yet run**.
* Move per-file hashing off the producer (hash on workers, combine in order), avoid zero-filling
  blocks, add a buffer pool, read-ahead and parallel file opening for many small files.
* Test archives larger than 4 GiB and a genuinely full disk. >4 GiB on v2: **tests written**
  (`tests/large.rs`, `.github/workflows/large.yml`), not yet run; full disk: still open.
* **Exit:** documented speedup curve; at least 90% of the `store` ceiling for lz4 on all cores;
  memory model validated at more than 2 threads.

## Phase 3 - Ratio and format  (partly done; see `PHASE3.md`)

Done: format v2 + compatibility policy, profiles, classification, parallel extraction, 7-Zip
comparison. **Not done:** BCJ/delta filters, block-level deduplication (whole-file dedup is done: `pack --dedup`), dictionaries.

*Why:* the original is 3.2% (text) and 9.6% (binary) better than the current best; independent
blocks cost ~4% on text.

* x86/ARM64 **BCJ filter** for executable data via liblzma filters (measured potential: -8.6%
  on the binary corpus). Delta filter for tables/audio.
* Reduce the block-independence cost: larger blocks for LZMA, zstd long-distance matching,
  solid groups (several blocks sharing a window), grouping similar files (by extension) before
  blocking, dictionaries for many small files. Each change must show a measured gain.
* Block deduplication (evaluate content-defined chunking). Whole-file deduplication is done (`pack --dedup`, format extension 0x8001).
* Presets (`fast`, `balanced`, `max`) chosen from the data, replacing raw level numbers.
* Symlinks, hard links, ownership, xattrs: **done** (format extensions `0x8002`, `0x0002`, `0x0003`). **Freeze format version 1** (or bump to 2) with a
  written compatibility policy and golden test archives.
* **Exit:** text >= 6.4x and binary >= 5.2x at a documented speed, or a documented reason why not.

## Phase 4 - Features and the legacy question

* Encryption (authenticated, per block, password KDF) and authenticated index.
* Append / update / delete / compact without rewriting everything.
* **Decision gate on `.kln` compatibility:** read-only support for the original `KLNA` format
  requires porting its range-coder decoder, filters and encryption (a large task); an optional
  native range-coder backend behind `BlockCodec` is the other candidate. Decide with data: does
  the ratio gain over LZMA2+BCJ justify the complexity?
* Android: the Rust core could replace the Python engine through JNI. Out of scope until the
  format is frozen.
* Import/export of ZIP/7z/tar for interoperability.

## Phase 5 - Hardening and release

* Fuzzing (`cargo-fuzz`) of `Archive::open`, record and index decoding, and extraction.
  **Targets, dictionary, seeds and a nightly workflow written** (`fuzz/`,
  `.github/workflows/fuzz.yml`); never run, so no findings yet.
* Hardened extraction against hostile output directories (`openat`-style, e.g. `cap-std`).
  **Done on Unix** with `rustix` (`src/secure.rs`, `docs/HARDENING.md`); written without a compiler,
  not yet built. Windows keeps the path-based check.
* CI on macOS and Windows (CI on Linux runs on every push; Windows paths and permission code are
  written but unverified).
* Benchmarks vs 7-Zip/ZIP on large, many-core, SSD and HDD setups; reproducible release
  profile; packaging; choose a license (the original has none).
* Security review, semantic-versioning commitment, user documentation.
