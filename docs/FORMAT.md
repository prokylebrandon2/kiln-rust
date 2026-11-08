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
permanent. `crc32` covers the payload bytes as stored and is verified **before** any decoder runs. Zstd frames may declare a window up to 2^27 bytes (the chunk size limit) and writers pick it from the chunk size.

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
`0x8001` whole-file deduplication table (critical): `count` varint, then `count` pairs of
varints `step`, `back`. Pair *k* names the duplicate entry `d = d(k-1) + step` (`d(0) = 0`, so
entry numbers strictly increase) and its source entry `d - back` (`back >= 1`). A duplicate is a
non-empty file entry with the same size and hash as its source, which is a file entry that is not
itself a duplicate. A duplicate owns no bytes in the data stream and its content is its source's:
the sizes of the non-duplicate entries add up to the chunk `raw_len`s, and a duplicate's stream
offset is its source's. The container version stays 2; a reader that lacks the extension
refuses the archive.
`0x8002` links (critical): `count` varint, then `count` records `step` varint, `kind` u8, body.
`step` is the distance from the previous record's entry (the first from entry 0; every later step
is at least 1), so entry numbers strictly increase. Kind 1 is a symbolic link: `len` varint
(1..=4096) and that many target bytes, stored verbatim (no NUL, not interpreted, may be absolute
or contain `..`). Kind 2 is a hard link: `back` varint (>= 1); its source is entry `n - back`.
A link is an ordinary file entry. A symbolic link has size 0, the hash of empty content and class
0, and owns no bytes. A hard link has the size and hash of its source and owns no bytes; its
source is a file entry that is not itself a link (it may be a deduplicated copy) and always
comes first. An entry cannot be both a link and a duplicate, and **no entry may be stored below
a symbolic link** (`a -> /etc` followed by `a/passwd` is refused when the index is read).
`0x0002` owner table (non-critical): `pair_count` varint, `pair_count` pairs of varints
(`uid`, `gid`), then one varint per entry, in entry order, naming its pair. Absent when the
archive records no ownership.
`0x0003` extended attributes (non-critical): `count` varint, then `count` records `step` varint
(as for links), `n` varint (1..=1024), then `n` attributes of `name_len` varint (1..=255), name
bytes (no NUL), `value_len` varint (at most 65536), value bytes. Only entries that have
attributes appear.

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
* Symbolic links, hard links, numeric ownership and extended attributes are stored by the
  extensions above (`0x8002`, `0x0002`, `0x0003`). Special files (sockets, FIFOs, devices) are not
  stored. Extraction creates links after all file data is in place and never writes through one;
  it restores ownership only on request and, by default, only the `user.` attributes (security
  labels, ACLs and file capabilities need an explicit opt-in). It **refuses to follow an existing
  symbolic link inside the destination tree**.

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

## The `.skln` container (encrypted) — version 1

> **Status:** the raw-key path is implemented by the `kiln-skln` crate (`skln/`): `seal` / `open`
> under a 32-byte key, and the member bundle (`kiln_skln::bundle`). The password layer (the scrypt
> block) is specified here but not built, and the Kiln app does not read or write `.skln` yet.
> Two golden files in `skln/tests/golden/` were written by an independent Python implementation
> of this section (`make_v1.py`). A `.skln` is separate from the `.kiln` container above: it wraps
> data (a plain `.kiln` archive, or a bundle of named members) in authenticated encryption.
> Recommended extension: `.skln`.

**The format is public.** Nothing here depends on it being secret; the only secret is the key.
Everything a reader needs before decrypting is in the header, and the whole header is
authenticated. All integers are little-endian. Version 1 fixes its algorithms and never negotiates
them: ChaCha20-Poly1305 (RFC 8439), HKDF-SHA-256 (RFC 5869), and scrypt (RFC 7914) for the
optional password block.

### Header (36 bytes, plus a 26-byte password block when flag bit 0 is set)

| Bytes | Field | Meaning |
|---|---|---|
| 0-7 | magic | `89 53 4B 4C 4E 0D 0A 1A` (`\x89SKLN\r\n\x1a`) |
| 8 | version | `1`. Readers reject any other value as *unsupported* |
| 9 | flags | bit 0: a password block follows the header; every other bit must be 0 |
| 10 | kind | 0: the payload is a plain `.kiln` v2 archive; 1: the payload is a member bundle |
| 11 | chunk_log2 | records hold up to `2^chunk_log2` plaintext bytes; allowed 12..=24 |
| 12-19 | revision | u64, raised by 1 on every save; writers that do not track it write 0 |
| 20-35 | file_salt | 16 random bytes, new on every write |

Password block (only when flag bit 0 is set), bytes 36-61: `kdf u8` (1 = scrypt) | `log_n u8` |
`r u32` | `p u32` | `kdf_salt[16]`. Allowed: `log_n` 10..=22, `r` 1..=16, `p` 1..=4, and
`128 * r * 2^log_n` at most 512 MiB. Writers use `log_n` 17, `r` 8, `p` 1.

### Keys and nonces

* `key32` is 32 raw bytes. Either the caller supplies it (Silk hands over its VEK), or it is
  `scrypt(NFKC(password), kdf_salt, 2^log_n, r, p)` from the password block.
* `file_key = HKDF-SHA-256(ikm = key32, salt = file_salt, info = "kiln skln v1 file key")`,
  32 bytes. The salt is new on every write, so one `key32` can seal any number of saves without a
  key and nonce pair ever repeating.
* The nonce of record `i` is 4 zero bytes followed by `i` as u64.

### Records

After the header (and password block): one or more records, `len u32 | ciphertext | tag[16]`,
where `len` counts the ciphertext and tag. Every record except the last has `len` exactly
`2^chunk_log2 + 16`; the last has `16 <= len <= 2^chunk_log2 + 16` (an empty last chunk is
allowed). A record is the last one exactly when it ends at the end of the file; nothing may
follow it. The associated data of every record is all header bytes (including the password block)
followed by one byte: 1 for the last record, else 0. So a changed header, an edited, reordered or
cut record, or appended bytes fails authentication, and a wrong key fails record 0.

### Payload

Kind 0: the decrypted stream is a complete plain `.kiln` v2 archive. Kind 1: a bundle,
`count u32 | { name_len u16 | name (UTF-8) | data_len u32 | data }*`. Names are labels, not paths:
not empty, no separators, not `.` or `..`, no control characters, at most 255 bytes, no duplicates; at most
1024 members and 64 MiB in all. (Silk's interim `SKIM` bundle uses big-endian lengths; it stays a
read-only source so existing vaults can be converted.)

### Reader rules

Check order: magic (is it `.skln`?) → version (*unsupported*, not *corrupt*) → flags, kind,
chunk_log2 and password-block limits → key → records. Header values are used before they are
authenticated, which is why the limits above are enforced first; tampering is then caught by
record 0. The revision is readable without the key but is only trustworthy after a successful
open; a reader that wants rollback protection compares it with the highest value it has seen.

### Required tests (written with the engine)

Done for the raw-key path (`skln/src/lib.rs`, `skln/src/bundle.rs`, `skln/tests/golden.rs`); the
password round trip waits for the password layer. Wrong key; every single-byte change and every truncation of a real file; swapped records;
appended bytes; header values outside the limits; a raw-key and a password round trip; and a
checked-in golden file that must keep opening. Never edit the golden file: bump the version.
