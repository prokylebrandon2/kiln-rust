# Architecture

> Updated for Phase 3: the container is now `.kiln` v2 (see `FORMAT.md`; chunks are variable-size,
> offsets implied, columnar index). Added modules: `profile.rs`, `classify.rs`; extraction uses a
> parent-directory-lane writer pool with a byte budget (`archive/extract.rs`). The data-flow
> text below is otherwise still the Phase 2 description of the same pipeline.

## Goals, in priority order

1. **Correctness and integrity** - never silently produce wrong data.
2. **Extraction speed** and **compression speed** - use every core.
3. **Compression ratio** - competitive with 7-Zip/LZMA2 at the top end.
4. **Low, predictable memory** - bounded by configuration, not by input size.
5. **Replaceable parts** - the compressor in particular.

## The central idea

```text
 files ──► one logical byte stream ──► cut into fixed-size blocks (4 MiB default)
                                            │
              each block compressed independently, in parallel, by a pluggable codec
                                            │
 archive = header │ block records ... │ index (entries + block offsets) │ footer
```

Independent blocks give, for free: parallel compression, parallel decompression, random access
(extract one file by decoding only the blocks it touches), bounded memory (one block per
in-flight item) and byte-identical output regardless of thread count. The price is ratio:
matches cannot cross block boundaries. `docs/BASELINE.md` measures that price (about 4% on text
going from 32 MiB to 4 MiB blocks) and the roadmap is organised around reducing it.

## Module map

| Module | Responsibility |
|---|---|
| `codec/` | `BlockCodec` trait; `Method` (on-disk ids); `CodecSpec` (`zstd:19`); backends `store`, `lz4` (lz4_flex), `zstd` (zstd), `lzma` (liblzma) |
| `format/` | Byte-level container: `Header`, `Footer`, block `record`s, `index` (entries, block table) with strict validation; bounds-checked `bytes` reader/writer |
| `parallel.rs` | `run_ordered`: the ordered, bounded, panic-safe `Source -> workers -> sink` pipeline |
| `walk.rs` | File traversal: sorted entries, symlink/special-file policy, duplicate detection, self-exclusion |
| `path.rs` | Archive-name validation and `safe_join` (path-traversal defence), member selectors |
| `integrity.rs` | CRC-32 (per block) and BLAKE3 (per file) |
| `fsutil.rs` | Short-read-safe reads, `TempOutput` (atomic create-then-rename), timestamps and permissions |
| `archive/writer.rs` | `pack`: the compression pipeline |
| `archive/reader.rs` | `Archive::open`: header, footer, index; never touches data blocks |
| `archive/extract.rs` | `extract` and `verify`: block selection, decode pipeline, stream demultiplexer |
| `bench.rs` | Runs the real pipelines over a codec x block-size grid |
| `cli.rs` / `main.rs` | `clap` command line (binary only; the library has no CLI dependency in its API) |
| `error.rs`, `util.rs` | One error enum; size/date formatting |

Dependencies point downward only: `cli -> archive -> {format, codec, parallel, walk, fsutil} -> {integrity, path, error}`.

## Data flow

**Pack** (`archive/writer.rs`)

```text
walk::collect ─► PackSource (producer thread) ─► N workers ─► sink (calling thread)
                 read file, BLAKE3 it,           compress, CRC-32,   append record,
                 fill one block                  store if no gain    remember offset
                                       └── order restored, at most `inflight` blocks alive ──┘
then: index record (zstd) + footer ─► fsync ─► atomic rename onto the destination
```

**Extract / verify** (`archive/extract.rs`)

```text
select members ─► mark needed blocks ─► BlockSource (seeks past unneeded blocks)
                                          ─► N workers: CRC-32 check + decompress
                                          ─► Demux sink: slice block across files, hash, write,
                                             compare BLAKE3, then restore mtime/mode
```

`verify` is the same pipeline with writing disabled.

## Concurrency model

`parallel::run_ordered` is the only place threads are created. One producer thread, `threads`
worker threads, and the caller as the sink, connected by crossbeam channels.

* **Ordering:** items carry a sequence number; the sink buffers early arrivals and releases them
  in order. Output never depends on scheduling.
* **Bounded memory:** a fixed pool of `inflight` credits (default `1.5 x threads + 2`). The producer
  needs a credit per item; the sink returns it after consuming the item. Tested: never more than
  `inflight` items alive.
* **Failure:** the first error *in item order* is returned. An error sets an abort flag and closes
  the channels, which wakes every blocked stage. A panic in the producer or a worker is caught and
  returned as `Error::Internal` rather than deadlocking the pipeline. Tests cover source errors,
  worker errors, sink errors, endless sources and panics.

Memory estimate (not yet measured with more than one thread): about
`inflight x (block + compressed block)` plus one codec context per thread. Measured single-thread
peaks: 13-14 MiB for lz4/zstd:3, 55-92 MiB for zstd:19 and lzma:6 (their context size dominates).

## Key decisions and why

| Decision | Reason | Cost / revisit |
|---|---|---|
| **Use zstd, lz4, liblzma instead of porting the original range coder** | Proven, fast, maintained; the original's coder is ~2,400 lines of C inside a Python string. The baseline shows liblzma reaches within 3.2% (text) of the original's ratio before any tuning. | Does not yet beat the original's ratio. Phase 4 decides whether a port is worth it, behind the same `BlockCodec` trait. |
| **Independent fixed-size blocks** | Parallelism, random access, bounded memory, determinism | Ratio loss (measured). Mitigations in Phase 3. |
| **Index at the end + footer** | Single pass writing, instant listing, no seeking back | Needs a seekable output; a truncated archive is detected, not recoverable. |
| **Never expand: store blocks that don't shrink** | Noise costs 188 B total; worst-case overhead is 13 B per block | A cheap entropy pre-check could skip the failed attempt (Phase 2). |
| **Two checksums: CRC-32 per block, BLAKE3 per file** | CRC is checked before any decoder sees the bytes and runs on workers; BLAKE3 proves end-to-end equality. | Per-file hashing currently happens on the producer/sink threads (Phase 2). |
| **Create-then-rename output** | A failed or interrupted pack never leaves a truncated archive, and never destroys an existing one | Temp file needs space beside the destination. |
| **Reject, don't sanitise, unsafe names** | `..`, absolute paths, backslashes, NUL rejected at pack time *and* when reading an index | Files named with a backslash cannot be archived (documented). |
| **Sorted, deterministic entry order** | Reproducible archives; testable | Sorting by type for ratio is a Phase 3 option. |
| **`#![forbid(unsafe_code)]`** | The crate itself has no `unsafe`; dependencies (zstd, liblzma) wrap C libraries | The C code is outside this guarantee. |
| **`panic = unwind` in release** | Guards (temp-file cleanup) must run on panic | Slightly larger binary. |
| **Provisional format, not frozen** | The brief says not to over-build a format before validating the architecture | Format version 1 may change before Phase 3 completes; no compatibility promise yet. |

## Security model (what is and is not defended)

Defended and tested: path traversal via archive names; oversized or lying counts and lengths in
the index and block headers (checked against bytes actually present before allocating);
decompression bombs (decoders are capped at the declared block size); every single-byte flip and
every truncation of a real archive; a wrong hash with valid block checksums; trailing garbage;
overwriting through a pre-planted symlink with `--force`; setuid/setgid/sticky bits (never restored).

**Not** defended yet: a malicious *output directory* (an attacker who controls symlinks inside the
extraction directory can still redirect intermediate path components; extract into a directory you
control - hardening with `openat`-style APIs is Phase 5); fuzzing has not been run (Phase 5);
there is no encryption or authentication of archives (Phase 4).

## Known performance limits of this foundation

* The producer thread reads, hashes and zero-fills blocks serially; with `store` this caps one
  core at roughly 400 MiB/s. On many cores with a fast codec this thread will be the bottleneck.
* Each block allocates fresh buffers (no pool).
* Small files are read one at a time (open/read/close per file).
* No pre-check for incompressible data, so compressible-looking noise costs a failed attempt.
* Multi-core scaling is unmeasured (single-core test machine).
