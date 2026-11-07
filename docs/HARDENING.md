# Hardening: hostile archives, hostile destinations, fuzzing

> **Status: written, not compiled or run.** This slice (the hardened extraction directory,
> archives over 4 GiB, the fuzz targets and the scaling harness) was written in an environment
> with no Rust toolchain, and by instruction was not tested. Expect to fix compile errors on
> the first `cargo build`. Nothing below has been measured; every claim about behaviour is a
> statement of what the code and tests are *meant* to do. See "What to run first" at the end.

Kiln defends against two different attackers:

| Attacker | Controls | Defence | Where |
|---|---|---|---|
| A hostile **archive** | the bytes of the `.kiln` file | Every length, offset, count and name is bounded and cross-checked before use; paths are validated; `--max-size` and `--memory-limit` bound the work. | `src/format`, `src/path.rs`, `src/archive/extract.rs`; corruption tests in `tests/corrupt.rs` |
| A hostile **destination** | what is already in, or changes in, the output directory while extraction runs | Descriptor-relative, no-follow extraction (this document). | `src/secure.rs`, `tests/hardening.rs` |

## 1. The hardened extraction directory

### The problem

Before this change, extraction built a path string per file (`dest/a/b/c`), checked each parent
with `symlink_metadata`, and then created the file by that path. That refuses a symbolic link
that is *already there*, but between the check and the use anyone with write access to the
destination can replace a directory with a link, and the write lands wherever the link points
(time-of-check / time-of-use). An archive that itself contains a link `a -> /etc` followed by a
file `a/passwd` is the same attack with the archive as the planter.

### The rule

> Nothing below the destination is ever followed through a symbolic link, not even one that
> appears while extraction is running.

### How (Unix)

`src/secure.rs` opens the destination **once** and does every later operation relative to a
directory file descriptor:

* Every component below the destination is opened with `O_DIRECTORY | O_NOFOLLOW`. A link or a
  regular file where a directory is needed is an error at the moment of use, not at some earlier
  check. Directories the extractor creates (`mkdirat`) are re-opened the same way before use.
* Files are created with `O_CREAT | O_EXCL | O_NOFOLLOW` (`openat`), by default under a hidden
  temporary name beside the target. After the BLAKE3 hash matches, the temporary file is moved
  into place: with `--force` by `renameat`, so a planted link at the final name is *replaced*,
  never written through; without it by `linkat` (which cannot replace anything), so a name that
  appeared in the meantime is reported as "exists" rather than overwritten.
* Mode, times, ownership and extended attributes are applied through the open descriptor, not by
  path. Modes are masked to `0o777`: set-uid, set-gid and sticky bits are never restored.
* Symbolic links are created with `symlinkat` and hard links with `linkat`, both relative to the
  parent descriptor.
* Once the extractor holds a descriptor for a directory, renaming or replacing that directory in
  the tree no longer redirects anything: the descriptor still names the original inode.
* Each worker thread has its own `Lane` (a small cache of at most 32 open directory descriptors,
  so descriptor use is bounded by `threads x 32`). Correctness never depends on the cache.
* A path that keeps changing while it is opened is refused after 8 attempts rather than looped on.

The destination argument itself may be a symbolic link to a directory (the caller chose it).
Everything below it may not be.

### What archives may still contain

Symbolic links and hard links are valid archive members and are restored as links
(`--no-links` skips them). Extracting a link is harmless; *writing through* one is what is
refused. An archive with `a -> /etc` and `a/passwd` therefore extracts the link and fails the
second entry with "`<dest>/a` is a symbolic link inside the destination; refusing to follow it".
Link targets are stored verbatim and are not interpreted during extraction.

### Errors and exit codes

| Situation | Error | CLI exit |
|---|---|---|
| Link where a directory is needed (existing, planted, or swapped in mid-run) | `UnsafePath` | 6 |
| Regular file where a directory is needed | `Invalid` | 6 |
| Name exists and `--force` was not given | `Exists` | 6 |
| Destination is not a directory | I/O error | 3 |
| Directory keeps changing while being opened | `UnsafePath` | 6 |

### What this does **not** protect against

* A hostile party who can **delete or fill** the destination, or create files of their own in it.
  Extraction is not transactional.
* Extended attributes and ownership of a **symbolic link**: there is no `fsetxattr`/`fchown` for a
  link itself. On Linux and Android these go through `/proc/self/fd/<dirfd>/<name>`, which keeps
  them relative to the descriptor. On other Unix systems they use the ordinary path, and that one
  call can still be raced.
* **Windows**: the previous path-based behaviour is kept. A link already in the tree is refused;
  the race is not closed. Windows has no `openat` equivalent in the standard library, and this
  crate is `forbid(unsafe_code)`.
* The *content* of the destination file system: case-insensitive collisions, Unicode
  normalisation and 8.3 short names are not handled specially (names containing a backslash are
  already refused).
* Running as root with `--same-owner` and `--all-xattrs` lets an archive assign ownership and
  attributes (for example `security.capability`). Extract untrusted archives as an unprivileged
  user, and leave those two options off.

### Dependencies

`rustix` (Unix only, features `fs` and `std`) is added to `Cargo.toml`. It is already in
`Cargo.lock` (used by `tempfile` and `xattr`), so only kiln's dependency list changed. It is a
safe wrapper, so `kiln` stays `#![forbid(unsafe_code)]`.

### Tests (`tests/hardening.rs`, Unix only)

| Test | Checks |
|---|---|
| `a_link_planted_where_a_file_goes_is_replaced_never_written_through` | Link at the final name: replaced, victim file untouched |
| `a_link_planted_in_place_of_a_directory_is_refused_at_every_depth` | Links at depth 1, 2, 3 are refused, nothing escapes |
| `a_regular_file_where_a_directory_is_needed_is_refused` | Clear error, nothing created |
| `the_destination_itself_may_be_a_link_to_a_directory` | The caller's own link is allowed |
| `a_destination_that_is_a_file_is_an_error` | Error, no panic |
| `a_directory_swapped_for_a_link_during_extraction_redirects_nothing` | The race itself, with a swapper thread |

The race test cannot *prove* the absence of a race; it makes a regression likely to fail. The
design argument above is what carries the guarantee.

## 2. Fuzzing

The fuzz crate is `fuzz/` (cargo-fuzz / libFuzzer). It is deliberately **not** a workspace member
(`exclude = ["fuzz"]` in the root `Cargo.toml`) because it needs a nightly toolchain.

| Target | Input | Property |
|---|---|---|
| `open` | A whole `.kiln` file | `Archive::open` never panics, hangs or over-allocates; whatever opens survives queries, `verify` and `extract` into a scratch directory with a size cap. A custom mutator re-seals the header, footer and index CRCs so mutations reach the semantic checks. |
| `index` | Raw index bytes (or edits to a valid index) | `Index::decode` never panics; whatever decodes re-encodes to bytes that decode to the same index. |
| `record` | One chunk or index record | `read_record` / `decode_payload` never panic or allocate beyond a 16 MiB cap, including inside the Zstandard, LZ4 and LZMA2 decoders. |
| `header` | 24-byte header and footer | Decode never panics; decode, encode, decode is the identity. |

Seeds come from the golden archive (`tests/golden/v2-adaptive.kiln`) and its pieces, in
`fuzz/corpus/`. `fuzz/kiln.dict` holds the magic numbers, extension tags and method ids.

```sh
cargo install cargo-fuzz
cd /path/to/kiln-rust
cargo +nightly fuzz run open   -- -dict=fuzz/kiln.dict -max_len=65536 -max_total_time=300
cargo +nightly fuzz run index  -- -dict=fuzz/kiln.dict -max_total_time=300
cargo +nightly fuzz run record -- -dict=fuzz/kiln.dict -max_total_time=300
cargo +nightly fuzz run header -- -max_total_time=60
cargo +nightly fuzz cmin open            # minimise a grown corpus
```

A crash lands in `fuzz/artifacts/<target>/`. Turn it into a regression test: keep the file under
`tests/`, and assert in `tests/corrupt.rs` that opening it returns `Err` (or that extraction
fails cleanly) instead of panicking.

`.github/workflows/fuzz.yml` runs every target for a short time each night and on demand, caches
the corpus between runs and uploads crashes as artifacts. A longer campaign (hours per target on
a dedicated machine) is the right way to gain confidence; five minutes only catches shallow bugs.

### What fuzzing has not covered

* No target drives the compressors, only the decoders.
* `open` extracts with one thread; multi-thread scheduling is covered by the round-trip tests,
  not by the fuzzer.
* Nothing has been run yet, so there are no findings, and no claim that the decoders are free of
  bugs.

## 3. Archives larger than 4 GiB (format v2)

An audit of the 32-bit casts in `src/format` and `src/archive` found no place where a size or
offset past 2^32 is narrowed: stream offsets, file sizes, the footer's `index_offset` and the
chunk start table are `u64`. The `u32` values are per-chunk (`raw_len <= block size <= 128 MiB`,
`comp_len`), checked on read (`chunk size is out of range`) and on write (`u32::try_from`). A
single file or archive larger than 4 GiB is therefore many chunks, each below 4 GiB.

Evidence added:

* `archives_beyond_4_gib_keep_exact_offsets` (`src/format/index.rs`): a normal unit test that
  builds an index whose stream passes 2^32 without any I/O, then checks offsets, entry starts,
  the chunk lookup by stream offset and an encode/decode round trip.
* `tests/large.rs` (`#[ignore]`, about 5 GiB of free disk, several minutes): a 4.25 GiB sparse
  source, packed once with `store` (the **archive file** passes 4 GiB) and once with `lz4` (the
  **data stream** passes 4 GiB and the archive stays small). Each is verified, then a member that
  lies beyond 4 GiB in the stream is extracted selectively and compared byte for byte.

```sh
cargo test --release --locked --test large -- --ignored --nocapture
KILN_LARGE_DIR=/mnt/scratch cargo test --release --test large -- --ignored
```

Not covered: a genuinely full disk (the Phase 2 roadmap item), and archives of many terabytes
(`MAX_ENTRIES` and the index size limit apply; see `docs/FORMAT.md`).

## 4. What to run first

Because this slice was never compiled:

```sh
cargo fmt
cargo clippy --locked --all-targets -- -D warnings   # expect to fix a few things
cargo test --locked                                  # includes tests/hardening.rs on Unix
cargo test --release --locked --test large -- --ignored --nocapture
cargo +nightly fuzz build                            # the fuzz crate compiles on its own
```

`Cargo.lock` was edited by hand (one line: `rustix` in kiln's dependency list). Cargo will fix
it silently if it is wrong unless you pass `--locked`, which will tell you.
