<div align="center">

#  Kiln

**A fast, multicore archiver built on Zstandard, LZ4 and LZMA2.**

Create archives up to 250× faster than 7-Zip, list them instantly, extract single folders without unpacking everything, and verify every file with a checksum.

![Status: experimental](https://img.shields.io/badge/status-experimental-orange)
![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue)
![Tested on Linux](https://img.shields.io/badge/tested-Linux%20only-lightgrey)

[Install](#install) · [Quick start](#quick-start) · [Speed vs. size](#choosing-speed-vs-size) · [Safety](#safety-and-resource-controls) · [Limitations](#known-limitations) · [Docs](#documentation)

</div>

---



> [!NOTE]
> **Kiln archives are not ZIP, 7z or tar.** The `.kiln` container is Kiln's own [documented format](docs/FORMAT.md). Other tools can't open it, and Kiln can't open theirs.

## Is Kiln right for you?

**Good fit**

- You want archives created far faster than 7-Zip can. In our benchmarks Kiln was **9–250× faster** at creating archives.
- You want to browse or partially extract an archive without decoding all of it.
- You want built-in integrity checking and protection against hostile archives (path traversal, decompression bombs).

**Not a good fit (yet)**

- You need the smallest possible files. **7-Zip's archives were 8–46% smaller** at its default level.
- You need to share archives with people who only have ZIP or 7z tools.
- You need encryption, updating an existing archive, or archive repair. None of these exist yet.

## Install

**Requirements:** Rust 1.85 or newer, and a C compiler (`gcc`, `clang` or MSVC). The Zstandard and liblzma libraries are compiled from bundled sources, so no other system libraries are needed.

```sh
cargo build --release     # the program is target/release/kiln
```

The first build downloads dependencies from crates.io and takes a few minutes. `Cargo.lock` is committed; add `--locked` for reproducible builds.

## Quick start

```sh
kiln create  backup.kiln photos/ notes.txt     # create an archive
kiln list -l backup.kiln                       # see what's inside (instant)
kiln info    backup.kiln                       # sizes, codec, ratio
kiln test    backup.kiln                       # verify every checksum, write nothing
kiln extract backup.kiln -o restored/          # extract everything
kiln extract backup.kiln photos/2024 -o out/   # extract just one folder
```

Kiln uses all CPU cores by default and never overwrites existing files unless you pass `-f`.

## Choosing speed vs. size

### Profiles

Pick one with `-p`:

| Profile    | What it does                                                  |
| ---------- | ------------------------------------------------------------- |
| `fastest`  | Quickest, largest output                                      |
| `balanced` | Sensible middle ground                                        |
| `maximum`  | Smallest output, slowest                                      |
| `store`    | No compression                                                |
| `adaptive` | Skips data that's already compressed (JPEG, MP4, ZIP, etc.)   |

```sh
kiln create -p adaptive out.kiln data/
```

### Fine control

Choose a codec with `-c` and a block size with `-b`. Larger blocks give a better ratio but use more memory.

```sh
kiln create -c zstd:19 -b 32MiB big.kiln data/   # slower, smaller
kiln create -c lzma:6  -b 32MiB max.kiln data/   # best ratio available
```

- **Codecs:** `store`, `lz4`, `zstd` (levels -7 to 22), `lzma` (levels 0 to 9)
- **Block sizes:** powers of two from 4 KiB to 128 MiB
- **Threads:** `-j N` (default: all cores)

### Not sure what's best for your data? Measure it

```sh
kiln bench data/ -c lz4 -c zstd:3 -c lzma:6 -b 4MiB -b 32MiB
```

## Safety and resource controls

```sh
kiln extract out.kiln -o dir/ --max-size 10GiB            # refuse to write more than 10 GiB (bomb guard)
kiln create  --memory-limit 256MiB --progress big.kiln data/
kiln extract backup.kiln -o restored/ --memory-limit 64MiB
```

- **`--memory-limit`** reduces threads and in-flight blocks until Kiln's *estimate* of peak memory fits. It is an estimate, not a hard cap. If the limit is too small even for one thread, Kiln exits with code 6 and tells you what it needs.
- **`--progress`** draws a progress line on stderr.
- **`--no-precheck`**: for slow codecs (zstd 10+, lzma), Kiln skips blocks that sampling shows to be incompressible. This saves time and never changes output size. Use this flag to turn it off.

**Built-in protections**

- Archives are written to a temporary file and renamed into place, so a failed run leaves nothing behind.
- Extracted files are written under a temporary name and renamed only after their BLAKE3 hash matches, so you never get a truncated file under a real name. Files completed before a failure remain.
- Path traversal and inconsistent archives are rejected.
- On **Unix**, extraction won't follow a symbolic link below the destination, even one planted while it runs. On **Windows**, an older path-based check is used and that race is not closed.

## What gets stored

Kiln stores regular files, directories, symbolic links and hard links.

| Option                        | Effect                                           |
| ----------------------------- | ------------------------------------------------ |
| `--owner`                     | Record numeric owner IDs when creating           |
| `--xattrs`                    | Record extended attributes when creating         |
| `--same-owner`                | Restore owners on extract (usually needs root)   |
| `--all-xattrs`, `--no-xattrs` | Control which extended attributes are restored   |
| `--no-links`                  | Don't recreate links on extract                  |

Sockets, FIFOs and device files are skipped with a warning. File names containing a backslash are refused.

## Exit codes

| Code | Meaning                                                                  |
| ---- | ------------------------------------------------------------------------ |
| 0    | Success                                                                  |
| 1    | Other error                                                              |
| 2    | Usage error                                                              |
| 3    | I/O error (missing file, unwritable output)                              |
| 4    | Not a Kiln archive, or unsupported version                               |
| 5    | Corrupt archive or checksum mismatch                                     |
| 6    | Request refused (output exists, unsafe path, bad option, no such member) |
| 7    | Completed with warnings (e.g. special files skipped); archive is valid   |

`pack`, `unpack`, `x` and `ls` work as aliases for `create`, `extract`, `extract` and `list`. The `.kiln` extension is only a convention.

## Using Kiln as a Rust library

```rust
use std::path::{Path, PathBuf};
use kiln::{Archive, ExtractOptions, PackOptions, extract, pack};

let stats = pack(Path::new("out.kiln"), &[PathBuf::from("my_dir")], &PackOptions::default())?;
let archive = Archive::open(Path::new("out.kiln"))?;
extract(&archive, &ExtractOptions::new("restored"))?;
```

## Known limitations

- **Never compiled or run.** See the warning at the top of this page.
- **Linux only so far.** Windows and macOS code paths exist but are unverified.
- **Compression ratio.** Worse than the original Kiln and 7-Zip on binaries (about 9.6% larger than the original), mainly because there is no x86 filter yet. Text is within 3.2% of the original.
- **Multicore scaling is unmeasured.** All benchmarks used 2 vCPUs. A 4/8/16-thread curve hasn't been run (see [`docs/SCALING.md`](docs/SCALING.md)).
- **Format compatibility.** `.kiln` v2 can't be read by earlier versions (Phase 2's `.kr` v1 or the original `.kln`), and vice versa.
- **Large archives (over 4 GiB).** Tests exist but haven't been run, and a genuinely full disk is untested.
- **Extraction is not transactional.** If it fails midway, files already completed stay on disk.
- **Fuzzing.** Fuzz targets exist but have never been run. That means no findings, not proof the code is bug-free.
- **Missing features:** encryption, append/update, archive repair.
- **Android app.** An experimental shell app lives in [`android/`](android/), but it has never been built. Expect to adjust SDK, NDK and Gradle versions.

## Documentation

| Document                                  | Contents                                           |
| ----------------------------------------- | -------------------------------------------------- |
| [`docs/FORMAT.md`](docs/FORMAT.md)        | The `.kiln` v2 format specification                |
| [`docs/PHASE3.md`](docs/PHASE3.md)        | Benchmarks against 7-Zip and Info-ZIP              |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | How Kiln is put together                      |
| [`docs/HARDENING.md`](docs/HARDENING.md)  | Hostile destinations, fuzzing, >4 GiB archives     |
| [`docs/SCALING.md`](docs/SCALING.md)      | Method for the multicore speedup curve             |
| [`docs/BASELINE.md`](docs/BASELINE.md)    | Measurements of the original implementation        |
| [`docs/AUDIT.md`](docs/AUDIT.md)          | Analysis of the original Android app this rewrites |
| [`docs/ROADMAP.md`](docs/ROADMAP.md)      | What's planned next                                |

## Development

```sh
cargo test --locked
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo test --release --locked --test large -- --ignored   # archives over 4 GiB (~5 GiB disk)
```

## License

**None chosen.** The original repository contains no license file, so none is asserted here. Decide on one before distributing.
