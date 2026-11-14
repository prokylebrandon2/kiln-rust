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
