# kiln-win: brief for the next agent

## Goal (the user's words, condensed)
A separate Windows program (.exe), 7-Zip-like interface. It only extracts and creates archives:
`.zip` and Kiln's own `.kiln`. Minimum features first. Benchmarks must be **significantly faster than
7-Zip** on both archiving and unzipping.

## Working rules (standing preferences from the user)
- **Never compile, test or benchmark locally.** Builds run only in GitHub Actions
  (`.github/workflows/win.yml`). The user runs it, then pastes errors or results back.
- Fix **warnings** too: CI runs `cargo fmt --check` and `clippy -D warnings`.
- The user is on **Windows PowerShell** and pushes everything to GitHub. Deliver changes as small
  tasks, each one copy-paste PowerShell patch (or a zip with a one-line apply command).
- No invented numbers. Only quote benchmark results that CI actually produced.
- Do not change the engine in `src/` (the shared Kiln crate) for the sake of this tool unless asked.
  Add to it only if unavoidable, and never alter existing behaviour or speed.

## Layout
```
win/Cargo.toml        crate kiln-win (edition 2024, rust-version 1.88), depends on kiln = { path = ".." }
win/src/main.rs       CLI: `kiln-win a|x|l`, picks ZIP or .kiln by name or `PK` magic
win/src/zip.rs        our own ZIP engine: parallel read and write, libdeflater per file,
                      flate2 streaming above 512 MiB, zip64, CP437/UTF-8 names, TempOutput commit
win/src/dostime.rs    DOS timestamps with the local UTC offset (windows-sys on Windows)
win/ci/smoke.ps1      round trips (.zip, .kiln), overwrite guard, selective extract,
                      interop with PowerShell Expand-Archive and 7-Zip in both directions
win/ci/bench.ps1      kiln-win vs 7-Zip, best of 3, writes a Markdown table to the job summary
.github/workflows/win.yml   build + smoke, lint, optional benchmark, release on tag `win-v*`
```
The `.kiln` side is the existing engine: `kiln::pack_with_progress`, `kiln::extract_with_progress`,
`kiln::Archive`. Levels map to profiles (0 Store, 1-3 Fastest, 4-6 Adaptive, 7-9 Maximum).

## Status
- Slice 1 (ZIP engine + CLI) is written but **has never been compiled or run**. Expect compile errors.
  Its use of the engine API (`kiln::error::Error` variants, `path`, `cancel`, `fsutil::TempOutput`,
  `PackOptions`, `Entry`) was read against the real source, not against a compiler.
- `win` is in the root workspace; the root release profile gives kiln-win, libdeflater and
  libdeflate-sys `opt-level = 3`.
- The workflow exists but has not run yet. `Cargo.lock` does not yet contain kiln-win's new crates
  (libdeflater, libdeflate-sys, ...). The build step runs without `--locked`; on a push build the
  workflow commits the updated lockfile back to the branch. After that, put `--locked` back on the
  build step. Until then the `--locked` jobs in `ci.yml` and `build.yml` fail.

## Next steps, in order
1. Read the first CI run. Fix compile errors, then clippy and fmt warnings, one small patch at a time.
2. Fix smoke-test failures (they point at real defects: names, dirs, overwrite, interop).
3. Pull the lockfile commit the workflow made, restore `--locked` in `win.yml`.
4. Run the workflow manually with `benchmark` ticked; record the table. This is the first real
   answer to "faster than 7-Zip".
5. Only then: speed work (below), then slice 2.

## Slice 2 and later (not started)
- GUI in the style of 7-Zip File Manager: address bar, file list, toolbar with Add / Extract / Test,
  progress window, open-with for `.zip` and `.kiln`. Toolkit not decided: Slint (already used in the
  Android app) vs native Win32. Decide after the CLI is green.
- Shell integration (file association for `.kiln`, context menu). Icon art: `assets/kiln-icon.svg`.

## Speed ideas (untested; profile first, with `samply`, before any of them)
Assumption to confirm: on Windows the cost is per-file overhead (NTFS, Defender), not the codec.

Extraction, many small files
- Create each file once: no reopen to set timestamps (use the handle), pre-size with
  `SetEndOfFile`/`FileAllocationInfo`.
- Parent-directory-relative opens (`NtCreateFile` with a directory handle) to skip repeated path
  walks and ACL checks.
- Directory affinity: create all directories first, give each worker whole directories (NTFS
  serializes creates in one directory). Cache already-created dirs; no `create_dir_all` per file.
- Opt out of EcoQoS (`SetProcessInformation` / `ProcessPowerThrottling`) so background runs are not
  parked on E-cores.
- Detect Dev Drive (ReFS performance mode) and print a one-line tip.

Archiving
- Cheap incompressible skip: magic bytes/extension, then sample the first 64 KiB. Reuse the root
  crate's `classify` module; check what `zip.rs` already does first.
- One huge file: compress chunks in parallel with sync-flush boundaries (pigz style). libdeflate
  cannot emit non-final blocks, so use flate2/zlib-rs for this case, only above a size threshold.
- Lock-free output: reserve an offset with an atomic `fetch_add` and `seek_write` from the worker,
  instead of a mutex around one `BufWriter`. Pre-extend the file.
- Overlap the directory walk with compression (keep biggest-first as a hybrid).

Rust level
- `mimalloc` as global allocator; per-worker buffer pools instead of allocating per entry.
- `memmap2` for reading archives (needs `unsafe`; handle truncation mid-read).
- PGO with `cargo-pgo`, trained on the smoke dataset, in CI.
- More than 64 logical cores: spread workers over processor groups.

`.kiln` format
- Per-class zstd dictionaries for trees of tiny similar files (listed as "not done" in the Phase 3
  notes). Block size tuned to L2 for the fastest profile. P-core/E-core aware worker placement.

Moonshots: IoRing batched reads, direct MFT enumeration (admin only), a resident process with a warm
thread pool for shell use.

## Baselines (measured earlier on Linux, for context only; not Windows)
- libdeflate, single thread: level 6 compresses at ~100-113 MB/s vs 7-Zip's zip `-mx5` at 10-15 MB/s;
  decompress ~730-1500 MB/s vs ~170-450 MB/s (codec only vs whole command, so not like for like).
- `.kiln` fastest vs 7-Zip zip `-mx1` on 89 MB mixed: create 0.18 s vs 0.98 s, extract 0.12 s vs 0.35 s.
- 7z (LZMA2) still wins on ratio (6.43 vs 4.07 on the 8,305-file set). Random data: kiln "fastest"
  extracted slower than 7-Zip's zip (1.61 s vs 0.21 s); "adaptive" fixed it (0.15 s).
