# Audit of the original Kiln project

Everything here was read from the supplied `kiln.7z` or measured by running its code. Statements
that are inferred from reading code rather than observed are marked *(inferred)*. The original
was never modified: it was extracted to a scratch directory and its engine was run from a copy.

## 1. Provenance

| | |
|---|---|
| File | `kiln.7z`, 578,803 bytes |
| SHA-256 | `fcd47e6773a379d41b5cc66f215634d7af4eac4e6e9e633681e434caa33a5e41` |
| Contents | 140 entries (1,351,856 bytes uncompressed), including a full `.git` directory |
| Git | branch `main`, HEAD `cb038bb` "Kiln 3.13", remote `github.com/AlreadyNoob/kiln` |
| Working tree vs HEAD | 75 files show as modified, but `git diff --ignore-cr-at-eol` is empty: the **only** difference is CRLF line endings in the archive |
| License | **None.** There is no LICENSE file; the licensing terms of the original are unknown |

## 2. What the project actually is

Two things in one repository:

1. **`kiln_v14.py`** - a 7,024-line single-file archiver and archive format (`.kln`, magic
   `KLNA`). It is *its own compressor*, hand-written: an optimal-parsing LZ77 with a 64 MB window
   and an LZMA-style adaptive range coder, plus filters. It works as a command line tool and an
   interactive shell. Only CRC-32 comes from a library (`zlib`); encryption uses the
   `cryptography` package.
2. **An Android app** ("Kiln for Android", `app.kiln`, v3.13) - a Java UI that embeds Python 3.12
   through Chaquopy and drives the engine. It is a file manager for `.kln` archives, not a
   standalone CLI product.

It is **not** a Rust, C or C++ program. The engine's hot loops are C (about 2,470 lines) stored
as string constants inside the Python file and compiled with `cc`/`gcc`/`clang` on first run
(about 8 s here), then loaded through `ctypes`.

## 3. Inventory

| Part | Language | Size | Role |
|---|---|---:|---|
| `app/src/main/python/kiln_v14.py` lines 1-4557 | Python | 4,557 lines | CLI, format, pipeline, shell, self-test, encryption, WAV/BCJ/MP3 glue |
| `kiln_v14.py` `_ACCEL_SRC` (lines 4558-6301) | C (embedded string) | 1,744 lines | LZ77 match search, range coder, WAV filters, BCJ |
| `kiln_v14.py` `_MP3_SRC` (lines 6302-7024) | C (embedded string) | 723 lines | lossless MP3 recompressor |
| `kiln_app.py`, `kiln_bridge.py`, `kiln_edit.py`, `kiln_scripts.py`, `kiln_boot.py` | Python | 692 / 221 / 318 / 130 / 18 lines | JSON-returning API for the UI, progress/cancel, staged editing, user-supplied scripts, boot hook |
| `app/src/main/java/app/kiln/*.java` | Java | 5,847 lines (`MainActivity` alone is 3,518) | Android UI, foreground service for long jobs |
| `app/src/main/res/**`, `AndroidManifest.xml` | XML | ~50 small files | layouts, icons, themes |
| `build.gradle`, `app/build.gradle`, `settings.gradle`, `gradle.properties` | Gradle | small | AGP 8.5.2, Chaquopy 16.0.0, `compileSdk 34`, `minSdk 30`, ABIs `arm64-v8a` + `x86_64` |
| `tools/build_accel.py` | Python | 33 lines | extracts the embedded C with `ast` and builds it with the NDK |
| `.github/workflows/build.yml` | YAML | 1 job | builds the C helper for each ABI, then `gradle :app:assembleDebug`, uploads the APK |
| `kiln_accel_*.so` (x2) | x86-64 ELF | 142,568 bytes each | **committed build-cache artifacts** (see section 7) |

## 4. The `.kln` format (as documented in the source, then checked against the code)

The docstring at the top of `kiln_v14.py` describes the layout. I verified the header, footer,
file-table and checkpoint parsing (`open_archive`, `parse_table`, `build_table_bytes`) against it
and they agree.

* Big-endian. `b"KLNA"` + flags (bit 0 = encrypted); encrypted archives add scrypt parameters and a
  16-byte salt.
* A stream of blocks `type u8 | raw_len u32 | comp_len u32 | payload`; type 0 stored, 1
  range-coded, 2 end of stream. A **single solid stream** holds every file.
* File table at the end: path, size, CRC-32, mtime (whole seconds), 12-bit mode, and a filter id
  packed into the spare mode bits. Footer: table offset, total size, magic.
* Optional checkpoints (about every 64 MB) allow decoding to start mid-archive.
* Encryption: AES-256-GCM per block, key from scrypt (N=2^17, r=8); names and sizes are encrypted too.
* Editing appends new blocks and a new table; removed files stay as hidden entries until `compact`.
* Archives from kiln 13 and older are **not readable** by v14 (`KLN2` and older are refused).

## 5. Features

Engine (`python kiln_v14.py ...`): `pack`, `unpack`, `list`, `test`, `bench`, `selftest`,
`wavcheck`, interactive `shell` (browse, add, rm, mv, extract).

* `pack` options: `-j N` parallel segments, `-e` encrypt, `-s` encrypt without compressing, `-i`
  one archive per file (with `-e`: random names plus an encrypted `_index.kln`), `-f`, `-q`.
* Compression: optimal-parse LZ77 priced by the range coder's own models; per-block choice of
  literal-context modes; optional logistic literal mixer; entropy check stores noise raw and only
  probes it for long repeats; files sorted by extension to group similar data.
* Filters: x86/AArch64 call/jump rewriting (BCJ), WAV prediction + byte planes (integer and
  float), lossless MP3 repacking.
* Integrity: CRC-32 per file; with encryption, per-block authentication detects tampering,
  reordering and truncation.
* Android app: browse/search folders, open archives (by content, not extension), preview text and
  images, extract all or a selection, integrity check, pack (optionally encrypted/one-per-file),
  copy/move/delete, in-archive editing with an explicit Save, foreground service with progress and
  cancel, custom main/fallback script, self-test/benchmark tools.

## 6. Build process and dependencies

* Desktop: `python kiln_v14.py ...` needs Python 3 (tested here on 3.12.3), a C compiler on first
  run (the engine refuses to run without its C helper rather than running slowly), and `cryptography`
  for encryption. No `requirements.txt` exists.
* Android: GitHub Actions only. NDK builds the helper per ABI, then Gradle builds a **debug** APK.
  Chaquopy installs `cryptography` from pip at build time. No release signing, no tests run in CI.
* **I did not build the Android app**: there is no Android SDK/NDK/Gradle in this environment.

## 7. Known limitations and observations

Functional / format
* Only regular files are stored: **no empty directories**, no symlink entries, no hard links, no
  ownership or extended attributes; mtime has 1-second resolution *(inferred from `collect()`)*.
* File symlinks are followed (stored as their target's content); symlinked directories are not
  descended and are skipped silently *(inferred: `os.walk` default)*.
* One solid stream: great for ratio, but extraction of a late file needs decoding from the nearest
  checkpoint (about 64 MB granularity).
* The format has changed incompatibly before (v13 and older unreadable), so there is no long-term
  compatibility record.

Performance (measured in `docs/BASELINE.md`, one core)
* Compression about 1.6 MiB/s at the default "thorough" setting; decompression 25-28 MiB/s
  after subtracting 85 ms of interpreter start-up.
* Peak memory 187-189 MiB compressing a 6 MB input with `-j1`.
* Parallelism is by segment (4-16 MB) with worker *processes*; the README states packing runs on
  one core inside the Android app because worker processes are not safe there.
* Heavy orchestration in Python; all hot work in the C helper.

Engineering
* **Two x86-64 `.so` files are tracked in git** (`kiln_accel_<hash>.so`, named by a hash of
  source + platform + flags). They are cache output of the first-run compile, are not used on
  Android, and `.gitignore` does not exclude them.
* No LICENSE file. No unit-test directory; the 19-check `selftest` lives inside the script
  (and **passes**, see below). CI builds but never tests.
* `MainActivity.java` is a 3,518-line single class; `kiln_v14.py` is a 7,024-line single file.

## 8. Functional baseline

* `python kiln_v14.py selftest` on this machine: **all 19 checks passed in 67.7 s**
  (round trips, encryption, wrong-password rejection, append/compact, checkpoints, WAV/float
  filters, range coder, and 25/25 deliberately corrupted archives rejected). Log:
  `tools/baseline/results/original-selftest.log`.
* Round trips of the benchmark corpus were byte-for-byte identical, and `kiln test` returned 0.
* Performance and ratio numbers against ZIP, 7-Zip, xz, zstd and others: `docs/BASELINE.md`.

## 9. What was not verified

* The Android build and the Java UI (no SDK/NDK here).
* Encrypted-archive performance and the shell, beyond what `selftest` covers.
* Behaviour on very large inputs (>1 GB) or many cores; this machine has one core.
