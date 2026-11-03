# Kiln (Rust rewrite) - Phase 1: audit and foundation

A multicore archiver built on proven compression libraries (Zstandard, LZ4, LZMA2), aiming to
compete with ZIP and 7-Zip. This is **Phase 1 of 5**: an audit of the original project, a measured
baseline, an architecture, and a working, tested foundation. It is a foundation, not a finished
product - see [Status](#status-and-honest-limitations).

## What this is rewriting

The supplied `kiln.7z` is an **Android app** (Java UI + Python 3.12 via Chaquopy) around a
7,000-line Python/C engine that implements its own compressor and `.kln` format. It is not a Rust
or C program. The audit is in [`docs/AUDIT.md`](docs/AUDIT.md); the original was left untouched.

The main finding: the original has an excellent compression ratio (best on text, 2nd on binaries
among the tools tested) but compresses at ~1.6 MiB/s, uses ~190 MiB of memory, and decodes at
~26 MiB/s. See [`docs/BASELINE.md`](docs/BASELINE.md).

## Build

Requirements: **Rust 1.85 or newer** (stable; developed on 1.91.1) and a **C compiler** (`gcc`,
`clang` or MSVC) - the Zstandard and liblzma libraries are compiled from bundled C sources. No other
system libraries are needed. The first build downloads crates from crates.io and takes a few minutes.

```sh
cargo build --release          # binary: target/release/kiln
cargo test                     # 81 tests, ~15 s
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

With `rustup`, `rust-toolchain.toml` selects stable with rustfmt and clippy automatically.
`Cargo.lock` is committed; use `--locked` for reproducible builds.

## Use

```sh
kiln pack   backup.kr  photos/ notes.txt      # zstd level 3, 4 MiB blocks, all cores
kiln pack   -c zstd:19 -b 32MiB big.kr data/  # slower, smaller
kiln pack   -c lzma:6  -b 32MiB max.kr data/  # best ratio currently available
kiln list   -l backup.kr                      # instant: reads only the index
kiln test   backup.kr                         # verify every checksum, write nothing
kiln unpack backup.kr -o restored/            # extract everything
kiln unpack backup.kr photos/2024 -o out/     # extract a subtree (decodes only needed blocks)
kiln bench  data/ -c lz4 -c zstd:3 -c lzma:6 -b 4MiB -b 32MiB   # measure on YOUR data
```

Codecs: `store`, `lz4`, `zstd[:-7..22]`, `lzma[:0..9]`. Block size: a power of two from 4 KiB to
128 MiB (larger = better ratio, more memory). `-j N` sets threads (default: all cores). Existing
files are never overwritten without `-f`. Archives are written to a temporary file and renamed
into place, so a failed run leaves nothing behind.

Library use:

```rust
use std::path::{Path, PathBuf};
use kiln::{Archive, ExtractOptions, PackOptions, extract, pack};

let stats = pack(Path::new("out.kr"), &[PathBuf::from("my_dir")], &PackOptions::default())?;
let archive = Archive::open(Path::new("out.kr"))?;
extract(&archive, &ExtractOptions::new("restored"))?;
```

## Layout

```text
src/            library (codec/, format/, archive/, parallel, walk, path, integrity, fsutil, bench) + CLI
tests/          integration tests: round trips, corruption/hostile archives, CLI
docs/           AUDIT.md  BASELINE.md  ARCHITECTURE.md  FORMAT.md  ROADMAP.md
tools/baseline/ scripts and raw results behind BASELINE.md (Python 3; not needed to build)
.github/        CI workflow (untested here)
```

Reproduce the baseline: `python3 tools/baseline/make_corpus.py corpus`, then
`python3 tools/baseline/compare.py --corpus corpus --kiln-rs target/release/kiln [--kiln-orig kiln_v14.py]`
(needs `gzip bzip2 xz zstd lz4 zip 7z` for the reference rows).

## Status and honest limitations

What works and is tested: pack / list / test / unpack with four codecs; parallel, order-preserving,
memory-bounded pipeline with byte-identical output for any thread count; random-access extraction;
atomic output; detection of every single-byte corruption and truncation tried; rejection of path
traversal and inconsistent archives.

What it is **not** yet:

* **Not ratio-competitive with the original or with 7-Zip on binaries** (9.6% larger than the
  original on the binary corpus; the missing x86 filter alone accounts for ~8.6%). Text is within
  3.2% of the original and 0.5% *better* than 7-Zip `-mx=5`. Details and plan: BASELINE, ROADMAP.
* **Multicore speedup is unmeasured** - the development machine had one core. Correctness under
  many threads is tested; speed is not.
* **The format is provisional** (version 1) and **not compatible with `.kln`**. Do not rely on
  archives surviving format changes until the format is frozen (Phase 3).
* Stores regular files and directories only: symlinks and special files are skipped with a
  warning; no ownership or extended attributes. Names containing a backslash are refused.
* No encryption, no append/update, no progress bar, no fuzzing yet.
* Extraction is not transactional and is not hardened against a hostile output directory;
  extract into a directory you control.
* Tested on Linux only. Windows/macOS code paths are written but unverified; the CI file has not
  been run.
* **License: none chosen.** The original repository contains no LICENSE file, so no license is
  asserted here. Decide before distributing.
