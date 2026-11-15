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
win/src/lib.rs        crate `kiln_win`: what both programs share (zip, dostime, ops)
win/src/main.rs       CLI: `kiln-win a|x|l`, picks ZIP or .kiln by name or `PK` magic
win/src/zip.rs        our own ZIP engine: parallel read and write, libdeflater per file,
                      flate2 streaming above 512 MiB, zip64, CP437/UTF-8 names, TempOutput commit
win/src/dostime.rs    DOS timestamps with the local UTC offset (windows-sys on Windows)
win/src/ops.rs        whole-archive operations for the GUI (extract, test, add) returning an
                      `Outcome`; plain std, so its unit tests also run on Linux
win/src/gui/          `kiln-gui.exe`, Windows only (native Win32 through windows-sys, no UI crate):
  main.rs             entry; windows subsystem (no console)
  app.rs              the commands: x, xf, t, az, ak, register, unregister
  progress.rs         the progress window: worker thread + a 50 ms poll, Cancel, shows after 400 ms
  shell.rs            the Explorer menu, as registry verbs under HKCU\Software\Classes
  w.rs                message boxes (silent with KILN_GUI_QUIET), DPI helpers
win/ci/smoke.ps1      round trips (.zip, .kiln), overwrite guard, selective extract,
                      interop with PowerShell Expand-Archive and 7-Zip in both directions
win/ci/bench.ps1      kiln-win vs 7-Zip, best of 3, writes a Markdown table to the job summary
.github/workflows/win.yml   build + smoke, lint, optional benchmark, release on tag `win-v*`
```
The `.kiln` side is the existing engine: `kiln::pack_with_progress`, `kiln::extract_with_progress`,
`kiln::Archive`. Levels map to profiles (0 Store, 1-3 Fastest, 4-6 Adaptive, 7-9 Maximum).

## Status
- Slice 1 (ZIP engine + CLI): the user reports it green in CI (build, smoke, fmt, clippy).
- Slice 2 (Explorer menu + progress window, `kiln-gui`): **written, never compiled** (the sandbox it
  was written in had no Rust toolchain). The Win32 code was written against windows-sys 0.61 from
  memory of its API; expect a few compile errors on the first CI run, and fix them one at a time.
  What the first CI run adds: `cargo test -p kiln-win` (ops unit tests), smoke checks that drive
  `kiln-gui` (add, test, extract here, extract to folder, overwrite refusal, register/unregister).
- `win` is in the root workspace; the root release profile gives kiln-win, libdeflater and
  libdeflate-sys `opt-level = 3`.
- The workflow has run: git history holds the "Update Cargo.lock for kiln-win [skip ci]" commit it made.
  The build step still runs without `--locked`; once nothing changes the lockfile any more, put
  `--locked` back (step 3 below).

## Next steps, in order
1. Read the first CI run. Fix compile errors, then clippy and fmt warnings, one small patch at a time.
2. Fix smoke-test failures (they point at real defects: names, dirs, overwrite, interop).
3. Pull the lockfile commit the workflow made, restore `--locked` in `win.yml`.
4. Run the workflow manually with `benchmark` ticked; record the table. This is the first real
   answer to "faster than 7-Zip".
5. Then speed work (below). Slice 2 is written; see its section.

## Slice 2 (written) and later
Decided: native Win32, like 7-Zip. The Explorer context menu works like 7-Zip's: a cascading "Kiln"
entry, and a small helper with a progress window (7-Zip's `7zG.exe` is our `kiln-gui.exe`).
- Menu on `.zip` / `.kiln`: Extract Here, Extract to subfolder, Test archive. On every other file and
  on folders: Add to .kiln, Add to .zip. Installed per user with `kiln-gui.exe register` (HKCU, no
  administrator); `unregister` removes it.
- Limits, all deliberate for now:
  - Labels are fixed ("Extract to subfolder"), not 7-Zip's `Extract to "name\"`; a per-file label
    and acting on a whole multi-selection need a shell extension DLL (`IExplorerCommand`).
  - On Windows 11 the entry sits under "Show more options" (the classic menu), as 7-Zip's does
    without its own Win11 package.
  - Controls use the classic look: no manifest yet (visual styles / common controls v6 need one;
    it can be embedded with linker args `/MANIFEST:EMBED /MANIFESTINPUT:...` from a build script).
  - "Test archive" of a ZIP unpacks into a scratch folder (no read-only ZIP check exists yet).
  - No "Extract files..." (folder picker), no "Add to archive..." (options dialog), no file manager.
- Later: the File Manager window (address bar, file list, toolbar Add / Extract / Test, open-with),
  file association for `.kiln`, icons (`assets/kiln-icon.svg`), the manifest above.

## Speed ideas (untested; profile first, with `samply`, before any of them)
Measured (see "Defender and many small files" below): with Defender on, the cost is Defender's
per-file scan, not the codec and not our file calls.

Extraction, many small files
- Create each file once: no reopen to set timestamps (done: times are set on the handle).
  Pre-sizing with `SetEndOfFile` was tried in a Defender-on test and made no difference.
- Parent-directory-relative opens (`NtCreateFile` with a directory handle) to skip repeated path
  walks and ACL checks.
- Directory affinity: create all directories first, give each worker whole directories (NTFS
  serializes creates in one directory). Cache already-created dirs; no `create_dir_all` per file.
- Opt out of EcoQoS (`SetProcessInformation` / `ProcessPowerThrottling`) so background runs are not
  parked on E-cores.
- Detect Dev Drive (ReFS performance mode) and print a one-line tip.

Extraction, big files (written 2026-11-15, not yet measured)
- Entries over 512 MiB take `extract_stream`: flate2 with the zlib-rs backend (was the default
  miniz_oxide) and two threads, one reading and inflating, one checking the CRC and writing. Compare
  an old and a new exe on one zip with a single entry over 512 MiB before keeping or extending it.
- Open: whether entries of tens to hundreds of MiB (now whole-buffer libdeflate on one worker) are
  faster through the same path: libdeflate may be quicker per byte, but cannot overlap the write.
- Answer (measured 2026-11-15): the 1 GiB memory budget made workers wait 8-19% of their time on
  `something.zip` (trace slot `wait_ms`). Streaming mid-size entries was not faster, so that switch is
  gone. The budget is now a quarter of the RAM, kept between 1 and 4 GiB (`memory_budget`);
  `KILN_BUDGET_MIB` still overrides it, to try a value without a new build.

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

## Defender and many small files (measured 2026-11-15, by hand, not CI)
Machine: i7-13620H (16 logical cores), Windows 11 build 10.0.26300, 6015 files / 120.5 MiB
synthetic set from `bench-local.ps1`, warm cache, zip made by 7-Zip or kiln-win.

| Extract time | Defender off | Defender on |
|---|---|---|
| kiln-win | 0.43 s | 5.5-9 s |
| 7-Zip | 1.56 s | 1.8-2.6 s |

- Off: kiln-win is about 3.6x faster. On: about 3x slower. Run-to-run drift on this machine is large
  (the same test varied 2x within one session), so compare tools only inside one session.
- Cause, from a Defender performance recording (`New-MpPerformanceRecording`): both tools get
  6015 on-close scans. All of 7-Zip's (`C:\Program Files\7-Zip\7z.exe`) are skipped with reason
  "Optimization" (0.14 ms each); none of kiln-win's are (median 10 ms, total 180 s summed over workers).
  Microsoft's analyzer reference does not define "Optimization".
- Tested and ruled out as the cause: thread count (1 thread = 16 threads when Defender is on),
  extraction order, `heavy_first` (reverted), pre-sizing, setting times on the handle,
  `FILE_FLAG_OPEN_REPARSE_POINT` (Rust adds it for `create_new`), share flags, Authenticode signing
  (7-Zip is unsigned too), the download mark, Defender exclusions (none set), folder (kiln-win in
  `Program Files` and in the 7-Zip folder is still slow), 7-Zip's file hash (a copy of 7z.exe in
  Downloads is slow). A plain C program and plain .NET writing the same files were as slow as kiln-win.
  Signed `tar.exe` (1.2 s) and `robocopy` (0.9-1.5 s) are fast.
- Not tested: whether an installer, code signing or accumulated reputation earns the skip.
- Where kiln-win wins with Defender on: few large files (`something.zip`, 1088 files, 2.3 GB:
  3.4-4.0 s in the fast runs against 15-22 s for 7-Zip).
- Consequence: do not claim "faster than 7-Zip" without saying which workload and whether
  Defender was on. Quote the benchmark report's Defender line (`bench-local.ps1` prints it).

## Open questions (unexplained; check these before trusting a result)
- Big zip, write stalls: some runs spend 30-70 s (summed over workers) in `write`. They persist with
  Defender off, so they look like the disk or Windows write-back, not Defender. Cause unconfirmed.
- Big zip, idle workers: busy share (sum of trace slots / 16 x `workers_ms`) is only 25-36%. Starting
  big files first did not change it. Idea: add a "longest single write" figure to the trace to see
  whether one stuck write holds the other workers up (memory budget, big file on one worker).
- Why Defender skips 7-Zip's scans (above), and whether any program-side step can earn it.
- Not measured on another machine: all numbers come from one laptop.

## What worked and what did not (extraction speed)
One laptop (i7-13620H, 16 logical cores, Windows 11), 2026-11-15, by hand. Sets of 2-5 runs per
setting: good enough to rule ideas out, not to promise a number. Read `KILN_TRACE=1` before trusting
an idea.

Worked
- Chunks of up to 32 files (`RUN_FILES`) or 4 MiB (`RUN_BYTES`) from one folder, taken round-robin
  across folders: 12-17% faster than archive order on 6000 small files (Defender off).
- libdeflate per file on 16 workers: `something.zip` (1088 files, 2.1 GiB) takes 2-3.5 s in the fast
  runs against 15-22 s for 7-Zip (Defender on).
- Memory budget 4 GiB instead of 1 GiB: wait time 8-19% of worker time down to 0, fast runs about
  3.0 s down to about 2.4 s; in a 5-run set the median was 2.5 s against 3.2 s.
- Small files with Defender off: 0.43 s against 1.56 s for 7-Zip. Packing the small set: 0.17 s
  against 1.05 s (not measured with Defender on a big folder).
- Sampling the first 64 KiB and storing when it does not shrink: random data no longer extracts
  slower than 7-Zip (1.61 s down to 0.15 s).

Did not work (no gain, or worse)
- 8 or 16 files per chunk instead of 32: within the 0.02 s run-to-run noise.
- Starting the biggest chunks first (`heavy_first`): same wall time and busy share; reverted.
- Streaming mid-size entries through zlib-rs (`KILN_STREAM_MIB` 16 or 64): not faster, and stream16
  had two of the three slowest runs. Switch removed.
- A 2 GiB budget: median 4.35 s against 2.54 s for 4 GiB (a stall costs more with less room).
- Waiting 15 s between runs: slow runs still happened (3 of 15), so they are not left over from
  the previous run.
- Everything listed under the Defender section (thread count, order, pre-sizing, share flags, ...).

Still open
- About 1 run in 5 spends 30-60 s (summed over workers) in `write` instead of 4-11 s, with any budget
  and with Defender off. Untested: giving each output file its final size before writing.
- Workers are busy about 50-60% with the 4 GiB budget (it was 25-36%). The rest is not explained.

## Baselines (measured earlier on Linux, for context only; not Windows)
- libdeflate, single thread: level 6 compresses at ~100-113 MB/s vs 7-Zip's zip `-mx5` at 10-15 MB/s;
  decompress ~730-1500 MB/s vs ~170-450 MB/s (codec only vs whole command, so not like for like).
- `.kiln` fastest vs 7-Zip zip `-mx1` on 89 MB mixed: create 0.18 s vs 0.98 s, extract 0.12 s vs 0.35 s.
- 7z (LZMA2) still wins on ratio (6.43 vs 4.07 on the 8,305-file set). Random data: kiln "fastest"
  extracted slower than 7-Zip's zip (1.61 s vs 0.21 s); "adaptive" fixed it (0.15 s).
