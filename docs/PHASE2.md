# Phase 2 - Working archiver (complete)

## Result

Kiln is now a usable command-line archiver:

| Command | Purpose |
|---|---|
| `kiln create ARCHIVE INPUT...` | Create an archive from files and directories (aliases: `pack`) |
| `kiln extract ARCHIVE [MEMBER...] --output DIR` | Extract all or part of an archive (aliases: `unpack`, `x`) |
| `kiln list [-l] ARCHIVE` | List entries; reads only the index (alias: `ls`) |
| `kiln test ARCHIVE` | Verify every block CRC-32 and every file BLAKE3 without writing |
| `kiln info ARCHIVE` | Format version, codec, block size, counts, sizes, ratio |
| `kiln bench INPUT...` | Phase 1 codec/block-size benchmark (unchanged) |

Exit codes: 0 ok, 1 other, 2 usage, 3 I/O, 4 not an archive / unsupported version, 5 corrupt or
checksum mismatch, 6 refused (exists / unsafe path / bad option / no such member), 7 completed
with warnings.

## Design decisions

* **Container:** no existing archive format (ZIP, 7z, tar) offers independent, parallel-decodable
  blocks with per-block CRC-32, per-file BLAKE3 and an end-of-file index, so the Phase 1 Kiln
  container (`docs/FORMAT.md`, version 1, explicitly versioned, strictly validated on read) is
  kept. **ZIP and 7z are not implemented and not claimed.**
* **Compression:** Zstandard (default `zstd:3`) via the `zstd` crate; LZ4 (`lz4_flex`) and LZMA2
  (`liblzma`) were already present from Phase 1 and remain selectable with `-c`.
* **Streaming:** file data flows through a bounded, ordered pipeline one block at a time (4 MiB by
  default), on both create and extract; memory does not scale with archive or file size. Only the
  index (one small record per entry) is held in memory.
* Phase 1 architecture is kept. The first half of Phase 2 changed `src/cli.rs` and `src/main.rs`
  (command names, `info`, exit codes) and added `tests/archiver.rs`. The second half touched the
  library: `archive/{writer,extract,reader,mod}.rs`, `codec/mod.rs`, `parallel.rs`,
  `format/record.rs`, `fsutil.rs`, plus the CLI flags below.
* **Second-half additions:** `--memory-limit` (create/extract/test), `--progress`, an
  incompressibility sampler for slow codecs (`--no-precheck` to disable), per-block
  stored/compressed counts in `info`, and extraction through temporary files.
* **Memory model:** estimate = 16 MiB + 2 x block x in-flight blocks + threads x codec working
  memory. Threads are reduced last (the in-flight window shrinks first). The constants were fitted
  to measured peak RSS and rounded up; it is an estimate, not an enforced cap.
* **Sampler:** four 16 KiB windows per block; a block is skipped only if the byte histogram is
  near-flat (entropy > 7.98 bits/byte) *and* LZ4 saves < 2% on the samples. Blocks < 128 KiB and
  codecs that are not slow are never sampled. Blind spot: long-range repeats farther apart than
  the windows are stored instead of compressed (costs ratio, not correctness).

## Verification performed

All numbers below were produced by the final code unless marked otherwise.

* `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`:
  clean; **101 tests pass** (57 unit, 14 end-to-end, 6 CLI, 9 corruption, 14 round-trip,
  1 doctest). Run on Rust 1.91; the declared 1.85 minimum was not re-checked.
* **Representative directory** (84 files, 60 dirs, 49.0 MiB; nested dirs, 2 empty files, empty
  directories, Unicode names in CJK/Cyrillic/Arabic/accented/emoji/spaces, text, ELF binaries,
  random data, gzip/xz/zip, PNG/JPEG/MP3-style media, a 24 MiB mixed file): created, tested,
  listed, `info`'d and extracted with `zstd:3`, `lz4`, `zstd:19`, `lzma:6` and `store`. Every
  `kiln test` printed OK (exit 0); `diff -r` and a SHA-256 manifest matched for all five.
  Archive sizes: zstd:3 36.46 MiB (1.34x), lz4 45.17, zstd:19 35.13, lzma:6 35.01, store 49.00.
  The ratio is low because about half of this corpus is random or already compressed; 5 of 13
  blocks were stored.
* **File over 4 GiB:** one 4,563,402,752-byte (4.25 GiB) file, sparse on disk, with random data
  placed before, across and after the 4 GiB offset, plus a small file and an empty file.
  Create (1 thread, zstd:3): 104 MiB archive, peak RSS 21 MiB. `test`: OK, 32 MiB. Extract:
  1089 of 1089 blocks, 36 MiB. SHA-256 manifests of source and output matched. Because the data is
  mostly zeros the archive is small, so **archives larger than 4 GiB were not tested.** (An earlier
  run on a larger machine, with the pre-second-half binary, round-tripped a 5 GiB mostly-random
  file into a 2.5 GiB archive with peak RSS of 55/71/72 MiB; it was not repeated with the final
  binary.)
* **Failure cases (CLI, final binary):** truncated archive (test and list) exit 5; flipped byte
  exit 5, with no temp files left and only files completed before the bad block present; non-Kiln
  file exit 4; missing archive or input exit 3 (and no archive created); output exists without
  `--force` exit 6; extracting into a path that is a file exit 3; unknown member exit 6.
* **Write failures:** tests run `create` and `extract` under a file-size `ulimit` so that
  `write(2)` fails. Both exit 3, leave no temp files, and extraction leaves no truncated file
  (this test found and fixed a real defect: a failed write used to leave a truncated file under
  its final name). A genuinely full disk was not tested.
* **`--memory-limit`** (49 MiB corpus, measured peak RSS vs limit): zstd:3 25 MiB under 64,
  14 under 32; zstd:19 69 under 128; lzma:6 65 under 128 and 56 under 80; lz4 with 32 MiB blocks
  64 under 160. zstd:19 with a 64 MiB limit is refused (exit 6) because it needs about 72 MiB with
  one thread. Extraction under limits of 200/64/40 MiB used 24/25/12 MiB and was identical to the
  source.
* **Progress and `info`:** covered by tests (`--progress` writes only to stderr and reaches
  100.0%; `info` reports stored and compressed block counts).
* From the earlier session on a 2-core machine, with the same code, not re-run here: on 32 MiB of
  random data the sampler cut `lzma:6` from 6.1 s to 0.1 s and `zstd:19` from 2.5 s to 0.1 s with
  byte-identical archives; on compressible data it cost nothing measurable (3.0 s vs 3.2 s).
  Going from 1 to 2 threads took `zstd:12` from 2.7 s to 1.4 s on the 60 MiB corpus.

## Not done / known limitations

* **Multicore scaling is only characterised up to 2 threads** (the machines available had 1 or 2
  cores). A 4/8/16-thread curve, buffer pool, read-ahead and moving per-file hashing off the
  producer thread are still open.
* Only regular files and directories are stored. Symlinks and special files are skipped with a
  warning and exit code 7. No ownership, hard links or extended attributes. Names with a
  backslash are refused.
* Extraction is not transactional (files completed before a failure remain) and is not hardened
  against a hostile output directory; extract into a directory you control. Without hard-link
  support the "do not overwrite" check falls back to check-then-rename, which has a small race.
* `--memory-limit` is an estimate fitted to measurements on Linux x86-64, not a hard cap.
* Archives larger than 4 GiB, a genuinely full disk, and files of tens of gigabytes were not tested.
* The format is provisional (version 1) with no compatibility promise until Phase 3.
* Linux only; Windows/macOS unverified. No encryption, append/update or fuzzing.
* **Android APK (as of Phase 2, since superseded).** This phase's report was written without an
  Android SDK/NDK or a GitHub runner, so the APK workflow and `ci.yml` had not run yet. Both now run
  on every commit and the app is tested on a device; see the README's "Android APK" section.
* One I/O error message formatting bug (message printed twice) was found and fixed in the second
  half; error text elsewhere has not been audited.
