# Container format, version 1 (provisional)

> **Status:** provisional. Version 1 exists to validate the architecture. It may change before
> the format is frozen (see `ROADMAP.md`), and nothing guarantees compatibility yet. It is **not**
> compatible with the original `.kln` (`KLNA`) format. Extension used by the CLI examples: `.kr`.

All integers are **little-endian**. CRC-32 is IEEE (the `crc32fast`/zlib polynomial).

```text
offset 0      header          16 bytes
offset 16     block records   0..n   (data blocks, in stream order)
offset I      index record    1      (I = footer.index_offset)
end-24        footer          24 bytes
```

## Header (16 bytes)

| Bytes | Field | Meaning |
|---|---|---|
| 0-7 | magic | `89 4B 52 53 0D 0A 1A 0A` (`\x89KRS\r\n\x1a\n`, PNG-style) |
| 8-9 | version | `1` |
| 10 | default method | codec the writer was asked to use (informational; each block names its own) |
| 11 | block_log2 | block size = `2^block_log2`, allowed 12..=27 (4 KiB..128 MiB) |
| 12-15 | flags | reserved, must be zero |

## Record (used for data blocks and for the index)

```text
method u8 | raw_len u32 | comp_len u32 | crc32 u32 | payload[comp_len]        (13-byte header)
```

`method`: 0 store, 1 LZ4 block, 2 Zstandard frame, 3 LZMA2 in an xz container (no check).
`crc32` covers the payload bytes as stored. Ids are permanent.

Rules enforced by readers: `raw_len <= block_size` (index: `<= 512 MiB`); `comp_len <= raw_len`
(a block that does not shrink is stored); store requires `comp_len == raw_len`; the record must lie
inside the archive; decoded length must equal `raw_len` exactly.

## The data stream

The contents of all file entries, in index order, are concatenated into a stream of
`total_raw` bytes. Block `i` holds stream bytes `[i * B, min((i+1) * B, total_raw))`; every block
except the last is exactly `B` bytes. A file's start offset is the sum of the sizes of the files
before it.

## Index record (decoded payload)

```text
entry_count u32
entry * entry_count:
    kind u8 (0 file, 1 directory) | path_len u32 | path (UTF-8)
    size u64 | mtime_secs i64 | mtime_nanos u32 | mode u32 | blake3 [u8; 32]
total_raw u64
block_count u32
block_offset u64 * block_count        (absolute offsets of block records)
```

Validation on read: every path passes the name rules below; `mtime_nanos < 10^9`; directories have
size 0; no duplicate paths; sizes sum to `total_raw` without overflow; `block_count ==
ceil(total_raw / B)`; offsets strictly increase, lie at or after byte 16 and before the index; no
trailing bytes; all counts are bounded by the
bytes actually present *before* anything is allocated.

**Name rules:** non-empty, at most 32 KiB, `/`-separated, relative; no empty, `.` or `..`
component; no backslash; no NUL; valid UTF-8. The index is itself a record, compressed with zstd.

## Footer (24 bytes)

| Bytes | Field |
|---|---|
| 0-7 | `index_offset` u64 |
| 8-15 | `index_record_len` u64 (header + payload); must satisfy `index_offset + len == file_len - 24` |
| 16-19 | CRC-32 of bytes 0-15 |
| 20-23 | `4B 52 53 46` (`KRSF`) |

When a data block is read, its record must also end before the next block's offset (or before the
index, for the last block) and its `raw_len` must equal the length the stream layout predicts.

## Integrity

Block CRC-32 is verified before decoding. After decoding, each file's BLAKE3 digest is compared
with the index. Together with the structural checks above, every single-byte change and every
truncation of a test archive was detected (`tests/corrupt.rs`).

## Metadata semantics

* Timestamps: seconds + nanoseconds since the Unix epoch (negative allowed).
* Mode: Unix permission bits as stored; on extraction only the low nine bits are applied. Archives
  created on non-Unix systems store `0644` / `0755`.
* Not stored (yet): symlinks, hard links, ownership, extended attributes.
