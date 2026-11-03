# Kiln (Rust rewrite) - Phase 2: working archiver (complete)

A multicore archiver built on proven compression libraries (Zstandard, LZ4, LZMA2), aiming to
compete with ZIP and 7-Zip. This is **Phase 2 of 5**: the Phase 1 foundation (audit, baseline,
architecture, block container) is now a usable command-line archiver with `create`, `extract`,
`list`, `test` and `info`. See [`docs/PHASE2.md`](docs/PHASE2.md) for what was verified and what
is not done, and [Status](#status-and-honest-limitations).

**Kiln archives are not ZIP, 7z or tar.** The container is Kiln's own documented format
([`docs/FORMAT.md`](docs/FORMAT.md)); other tools cannot open it and Kiln cannot open theirs.

## What this is rewriting

The supplied `kiln.7z` is an **Android app** (Java UI + Python 3.12 via Chaquopy) around a
7,000-line Python/C engine that implements its own compressor and `.kln` format. It is not a Rust
or C program. The audit is in [`docs/AUDIT.md`](docs/AUDIT.md); the original was left untouched.

The main finding: the original has an excellent compression ratio (best on text, 2nd on binaries
among the tools tested) but compresses at ~1.6 MiB/s, uses ~190 MiB of memory, and decodes at
~26 MiB/s. See [`docs/BASELINE.md`](docs/BASELINE.md).

## Build

Requirements: **Rust 1.85 or newer** (stable; developed on 1.91; the 1.85 minimum is declared in Cargo.toml but was not re-checked in Phase 2) and a **C compiler** (`gcc`,
`clang` or MSVC) - the Zstandard and liblzma libraries are compiled from bundled C sources. No other
system libraries are needed. The first build downloads crates from crates.io and takes a few minutes.

```sh
cargo build --release          # binary: target/release/kiln
cargo test                     # 101 tests, ~20 s
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

With `rustup`, `rust-toolchain.toml` selects stable with rustfmt and clippy automatically.
`Cargo.lock` is committed; use `--locked` for reproducible builds.

## Use

```sh
kiln create  backup.kiln photos/ notes.txt        # zstd level 3, 4 MiB blocks, all cores
kiln create  -c zstd:19 -b 32MiB big.kiln data/   # slower, smaller
kiln create  -c lzma:6  -b 32MiB max.kiln data/   # best ratio currently available
kiln list    -l backup.kiln                       # instant: reads only the index
kiln info    backup.kiln                          # format, codec, sizes, counts, ratio
kiln test    backup.kiln                          # verify every checksum, write nothing
kiln extract backup.kiln --output restored/       # extract everything
kiln extract backup.kiln photos/2024 -o out/      # extract a subtree (decodes only needed blocks)
kiln create  --memory-limit 256MiB --progress big.kiln data/   # cap estimated peak memory, show progress
kiln extract backup.kiln -o restored/ --memory-limit 64MiB      # same limit option on extract and test
kiln bench   data/ -c lz4 -c zstd:3 -c lzma:6 -b 4MiB -b 32MiB   # measure on YOUR data
```

`pack`, `unpack`, `x` and `ls` remain as aliases of `create`, `extract`, `extract` and `list`.
The `.kiln` extension is a convention, not enforced.

**Exit codes:** 0 success - 1 other error - 2 usage error - 3 I/O error (missing file, unwritable
output) - 4 not a Kiln archive / unsupported version - 5 corrupt archive or checksum mismatch -
6 request refused (output exists, unsafe path, bad option, no such member) - 7 completed with
warnings (e.g. symlinks skipped; the archive is still valid).

`--memory-limit SIZE` lowers threads and the in-flight block window until an *estimate* of peak
memory fits (it is a model checked against measured peak RSS, not an enforced cap); a limit too
small for even one thread exits 6 and says what is needed. `--progress` draws a progress line on
stderr. For slow codecs (zstd 10+, lzma) blocks that sampling shows to be incompressible are stored
without being attempted (`--no-precheck` turns this off); this saves time and never changes the
output size. `kiln info` reports how many blocks were stored versus compressed.

Codecs: `store`, `lz4`, `zstd[:-7..22]`, `lzma[:0..9]`. Block size: a power of two from 4 KiB to
128 MiB (larger = better ratio, more memory). `-j N` sets threads (default: all cores). Existing
files are never overwritten without `-f`. Archives are written to a temporary file and renamed
into place, so a failed run leaves nothing behind. Extraction writes each file under a temporary
name and renames it only after its BLAKE3 hash matches, so a failed run never leaves a truncated
file under a real name (files completed before the failure remain).

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
tests/          integration tests: round trips, corruption/hostile archives, CLI, write failures
android/        minimal Android shell that runs the CLI as libkiln.so (see below)
docs/           AUDIT.md  BASELINE.md  ARCHITECTURE.md  FORMAT.md  ROADMAP.md
tools/baseline/ scripts and raw results behind BASELINE.md (Python 3; not needed to build)
.github/        ci.yml and android-apk.yml workflows (neither has been run on a real runner)
```

Reproduce the baseline: `python3 tools/baseline/make_corpus.py corpus`, then
`python3 tools/baseline/compare.py --corpus corpus --kiln-rs target/release/kiln [--kiln-orig kiln_v14.py]`
(needs `gzip bzip2 xz zstd lz4 zip 7z` for the reference rows).

## Android APK (experimental, unverified)

`android/` is a minimal app that runs a create / list / info / test / extract self-test with the
Kiln binary shipped as `lib/<abi>/libkiln.so`. `.github/workflows/android-apk.yml` cross-compiles
`kiln` with `cargo-ndk` (arm64-v8a, x86_64), builds a debug APK (and a release APK on tags or on
request; signing needs the secrets listed in the workflow header) and checks the binaries are in
the APK. **Nothing here has been built:** the authoring environment had no Android SDK/NDK or
GitHub runner. The Java was only compile-checked against hand-written stubs, no Gradle wrapper is
committed (the workflow installs Gradle 8.14.3), and cross-compiling the bundled zstd/lzma C code
with the NDK is untested. Expect to adjust pinned SDK/NDK/AGP versions on the first real run.
Android has no JNI integration; the app just executes the CLI binary.

## Status and honest limitations

What works and is tested: create / list / info / test / extract with four codecs; parallel, order-preserving,
memory-bounded pipeline with byte-identical output for any thread count; random-access extraction;
atomic output; detection of every single-byte corruption and truncation tried; rejection of path
traversal and inconsistent archives.

What it is **not** yet:

* **Not ratio-competitive with the original or with 7-Zip on binaries** (9.6% larger than the
  original on the binary corpus; the missing x86 filter alone accounts for ~8.6%). Text is within
  3.2% of the original and 0.5% *better* than 7-Zip `-mx=5`. Details and plan: BASELINE, ROADMAP.
* **Multicore speedup is not characterised** - development machines had 1-2 cores. Correctness
  under many threads is tested; a 4/8/16-thread speedup curve has not been measured.
* **The format is provisional** (version 1) and **not compatible with `.kln`**. Do not rely on
  archives surviving format changes until the format is frozen (Phase 3).
* Stores regular files and directories only: symlinks and special files are skipped with a
  warning; no ownership or extended attributes. Names containing a backslash are refused.
* No encryption, no append/update, no fuzzing yet.
* Still open from the Phase 1 throughput plan: per-file hashing runs on the producer thread, no
  buffer pool, no read-ahead. See `docs/PHASE2.md`.
* Extraction is not transactional and is not hardened against a hostile output directory;
  extract into a directory you control.
* Tested with a 4.25 GiB file; archives larger than 4 GiB have not been tested.
* Tested on Linux only. Windows/macOS code paths are written but unverified; neither CI workflow
  has been run.
* **License: none chosen.** The original repository contains no LICENSE file, so no license is
  asserted here. Decide before distributing.
