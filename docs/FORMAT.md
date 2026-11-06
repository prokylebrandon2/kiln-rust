# The `.kiln` container format — version 2

> **Status:** version 2 is the first version with a compatibility policy (see the end). It is
> **not** compatible with the provisional Phase 2 `.kr` format (version 1, magic `\x89KRS`); a
> v1 file is reported as "not a Kiln archive" and must be read with the Phase 2 binary. It is also
> unrelated to the original Android app's `KLNA` format. Recommended extension: `.kiln`.

All integers are **little-endian**. CRC-32 is IEEE (the zlib/`crc32fast` polynomial). "Varint" is
unsigned LEB128, at most 10 bytes, **canonical** (no redundant trailing zero group); "zigzag" is
the usual signed mapping on top of it.

```text
offset 0      header          24 bytes
offset 24     chunk records   0..n   (data chunks, in stream order)
offset I      index record    1      (I = footer.index_offset)
end-24        footer          24 bytes
```

## Why a custom format at all?

Kiln needs, simultaneously: independently decodable chunks (parallel decode), a trailing index
(instant listing, selective extraction), per-chunk and per-file integrity, per-file content
metadata, and a way to evolve. ZIP has the index and per-file CRCs but compresses per *file* (poor
for many small files, no parallelism inside one large file) and its extension story is the Zip64/
extra-field maze. 7z's solid LZMA2 blocks compress better but must be decoded from the start of the
block to reach a file (measured in `PHASE3.md`). tar+compressor has no index at all. The measured
trade-offs are in `PHASE3.md`; where 7-Zip wins on ratio, that is stated there too.

## Header (24 bytes)

| Bytes | Field | Meaning |
|---|---|---|
| 0-7 | magic | `89 4B 49 4C 4E 0D 0A 1A` (`\x89KILN\r\n\x1a`): high-bit byte, name, DOS line ending, DOS EOF |
| 8-9 | version | `2`. Readers reject any other value as *unsupported* |
| 10 | chunk_log2 | maximum chunk size is `2^chunk_log2`; allowed 12..=27 (4 KiB..128 MiB) |
| 11 | profile | informational: 0 custom, 1 fastest, 2 balanced, 3 maximum, 4 store, 5 adaptive |
| 12-15 | flags_incompat | features a reader **must** understand; any unknown bit ⇒ refuse (none defined in v2) |
| 16-19 | flags_compat | features a reader may ignore; unknown bits are ignored |
| 20-23 | crc32 | CRC-32 of bytes 0-19 |

Check order: magic (is it Kiln?) → version (newer layouts are *unsupported*, not *corrupt*) → CRC.

## Record (used for data chunks and for the index)

```text
method u8 | raw_len u32 | comp_len u32 | crc32 u32 | payload[comp_len]      (13-byte header)
```

`method`: 0 store, 1 LZ4 block, 2 Zstandard frame, 3 LZMA2 in an xz container (no check). The xz block header names its filter chain, so a payload may start with a BCJ (x86, ARM64) or delta filter ahead of LZMA2; any xz decoder undoes it, so ids and the version are unchanged. Ids are
permanent. `crc32` covers the payload bytes as stored and is verified **before** any decoder runs.

Rules: `raw_len <= chunk size` (index record: `<= 256 MiB`); `0 < comp_len <= raw_len` (a chunk that
does not shrink is stored); store requires `comp_len == raw_len`; the record must lie inside the
archive; the decoded length must equal `raw_len` exactly; the decoder is never allowed to produce
more than `raw_len` bytes.

## The data stream

The contents of all file entries, **in index order**, are concatenated into a stream of `total_raw`
bytes, which is cut into chunks. A chunk holds `raw_len <= 2^chunk_log2` stream bytes; unlike v1,
chunks need not all be full. The writer ends a chunk early wherever the next file belongs to the
other storage group (compressible vs stored), so a chunk never mixes the two.

Chunk *file offsets are not stored*: chunk 0 starts at byte 24 and each next chunk starts where the
previous record ends (`+13 + comp_len`). The offsets are derived, so they cannot disagree with the
data, and the last chunk must end exactly at `footer.index_offset`.

A file's first stream byte is the sum of the sizes of all earlier entries. To read one file, find
the chunk containing its first byte (binary search over the cumulative `raw_len`s), decode only the
chunks that overlap it, and verify its hash.

## Index record (decoded payload)

The index record is a normal record compressed with zstd. Decoded layout (*columnar*: each field is
stored for all entries before the next field, which compresses far better than interleaving):

```text
index_version u8                 = 1  (layout version of this payload, independent of the container)
entry_count   varint             <= 8,000,000
chunk_count   varint
flags         u8 x entry_count   bit 0 = directory; bits 1-3 = content class; bits 4-7 must be 0
path_len      varint x entry_count        1..=32768
paths         bytes, concatenated (UTF-8)
size          varint x (file entries)
mtime_secs    zigzag varint x entry_count (delta from the previous entry; the first from 0)
mtime_nanos   varint x entry_count        < 1_000_000_000
mode          varint x entry_count        <= 0o7777
hash          [u8; 16] x (file entries)   BLAKE3 of the content, truncated to 128 bits
raw_len       varint x chunk_count        1..=chunk size
comp_len      varint x chunk_count        1..=raw_len
method        u8 x chunk_count
extensions    { tag u16 | len varint | data } ... until the end of the index
```

Content classes: 0 unknown/directory, 1 text, 2 data, 3 binary (executable code), 4 compressed
(known container), 5 random (high entropy). Classes 4 and 5 are stored, never compressed.

**Validation on read** (all before any allocation proportional to a claimed count):
counts are bounded by the bytes actually present *and* by `MAX_ENTRIES`; every path passes the name
rules below; no duplicate paths; directories have no size, hash or class; timestamps, modes and
chunk fields are in range; `sum(entry sizes) == sum(chunk raw_len)` with checked arithmetic; the
chunk records must end exactly at the start of the index; nothing may follow the extensions.

**Name rules:** non-empty, at most 32 KiB, `/`-separated, relative; no empty, `.` or `..` component;
no backslash; no NUL; valid UTF-8.

### Extensions

A tag with bit 15 set (`0x8000`) is **critical**: a reader that does not know it must refuse the
archive. Any other unknown tag is skipped (and preserved if the index is re-encoded). Defined:
`0x0001` creator string (UTF-8, ≤ 256 bytes, non-critical).

## Footer (24 bytes)

| Bytes | Field |
|---|---|
| 0-7 | `index_offset` u64 |
| 8-15 | `index_record_len` u64 (header + payload); `index_offset + len == file_len - 24` |
| 16-19 | CRC-32 of bytes 0-15 |
| 20-23 | `4B 4C 4E 46` (`KLNF`) |

## Integrity and defined behaviour for damaged archives

* Header, footer, every chunk payload and the index are each covered by a CRC-32; every file by a
  128-bit BLAKE3; and the chunk table in the index is cross-checked against every record header
  that is read. In the test suite **every single-byte change and every truncation** of real
  archives (plain and classified) is detected (`tests/corrupt.rs`, `tests/phase3.rs`).
* **Incomplete archive** (writer interrupted): archives are written to a temporary name and renamed
  into place only after the footer is written and fsynced, so a finished name is always complete.
  A truncated file has no valid footer and is rejected as *corrupt*; nothing is guessed.
* **Corrupt chunk:** reported with its number (`chunk N: ...`); extraction stops at the first bad
  chunk. Files completed and verified before it remain; no file is ever left under its real name
  unless its hash verified. (Small files are verified *before* anything is created.)
* **Damaged index or footer:** the archive cannot be opened. There is no salvage mode yet: the
  chunk records carry no file names, so data cannot be mapped back to files without the index.
* **Resource limits:** at most 8,000,000 entries and a 256 MiB index; extraction can be capped with
  `--max-size`; decoders are bounded by `raw_len`.

## Metadata semantics

* Timestamps: seconds + nanoseconds since the Unix epoch (negative allowed).
* Mode: Unix permission bits as stored; on extraction only the low nine bits are applied (no
  set-uid/gid/sticky). Archives created on non-Unix systems store `0644` / `0755`.
* Not stored (yet): symlinks, hard links, ownership, extended attributes. Extraction **refuses to
  follow an existing symbolic link inside the destination tree**.

## Compatibility policy

* `version` changes only for layout changes an old reader cannot safely skip. A reader accepts
  exactly the versions it knows.
* New optional information goes into **extensions** (skippable) or **flags_compat**; anything an
  old reader must understand goes behind **flags_incompat** or a critical extension.
* Chunk method ids, content class ids, profile ids and extension tags are never reused.
* `tests/golden/v2-adaptive.kiln` is a checked-in version 2 archive (written by kiln 0.3.0) that
  `tests/golden.rs` must keep opening, verifying and extracting. If that test fails, the change
  broke version 2: bump the version, never edit the golden file. Only one golden archive exists
  so far (adaptive profile; zstd and store chunks); LZ4 and LZMA2 chunks and the optional
  extensions are covered by unit and integration tests but not by golden files.
