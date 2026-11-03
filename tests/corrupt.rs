//! Damaged and hostile archives must be rejected cleanly - never panic, never extract garbage,
//! never write outside the destination.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use kiln::codec::{CodecSpec, Method};
use kiln::format::index::{Entry, EntryKind, Index};
use kiln::format::record::EncodedBlock;
use kiln::format::{FOOTER_LEN, Footer, HEADER_LEN, Header};
use kiln::integrity::{FileHasher, empty_hash};
use kiln::{Archive, Error, ExtractOptions, PackOptions, extract, pack, verify};

/// A small archive with compressed blocks *and* stored blocks, ~12 KB.
fn small_archive(dir: &Path) -> Vec<u8> {
    let root = dir.join("s");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("text.txt"), prose(14_000, 9)).unwrap();
    fs::write(root.join("noise.bin"), noise(6_000, 10)).unwrap();
    fs::write(root.join("zero"), b"").unwrap();
    let arc = dir.join("s.kr");
    let opts = PackOptions {
        block_log2: 12,
        threads: 2,
        ..PackOptions::default()
    };
    let stats = pack(&arc, &[root], &opts).unwrap();
    assert!(
        stats.stored_blocks >= 1 && stats.blocks > stats.stored_blocks,
        "{stats:?}"
    );
    fs::read(arc).unwrap()
}

/// Open + fully verify; Ok only if the archive is entirely sound.
fn accepts(bytes: &[u8], scratch: &Path) -> bool {
    let p = scratch.join("probe.kr");
    fs::write(&p, bytes).unwrap();
    match Archive::open(&p) {
        Ok(a) => verify(&a, 2).is_ok(),
        Err(_) => false,
    }
}

#[test]
fn every_single_byte_flip_is_detected() {
    // Flip each byte of a real archive; header, block records, index and footer are all covered.
    let dir = tempfile::tempdir().unwrap();
    let good = small_archive(dir.path());
    assert!(accepts(&good, dir.path()));
    let mut undetected = Vec::new();
    for i in 0..good.len() {
        let mut bad = good.clone();
        bad[i] ^= 0x5A;
        if accepts(&bad, dir.path()) {
            undetected.push(i);
        }
    }
    assert!(
        undetected.is_empty(),
        "undetected corruption at byte offsets {undetected:?}"
    );
}

#[test]
fn header_default_method_is_informational_only() {
    // Byte 10 names the codec the writer *asked for*. Decoding never consults it - every block
    // record carries its own method - so changing it to another *valid* id is harmless by
    // design (changing it to an unknown id is rejected; see the exhaustive test above).
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = small_archive(dir.path());
    assert_eq!(bytes[10], Method::Zstd.id());
    bytes[10] = Method::Lz4.id();
    assert!(accepts(&bytes, dir.path()));
}

#[test]
fn every_truncation_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let good = small_archive(dir.path());
    for len in 0..good.len() {
        assert!(
            !accepts(&good[..len], dir.path()),
            "prefix of {len} bytes was accepted"
        );
    }
}

#[test]
fn trailing_garbage_and_foreign_files_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = small_archive(dir.path());
    bytes.extend_from_slice(b"garbage");
    assert!(!accepts(&bytes, dir.path()));

    let p = dir.path().join("not.kr");
    fs::write(&p, b"PK\x03\x04 this is a zip, honest").unwrap();
    assert!(matches!(Archive::open(&p), Err(Error::NotAnArchive(_))));
    fs::write(&p, b"").unwrap();
    assert!(matches!(Archive::open(&p), Err(Error::NotAnArchive(_))));
    assert!(matches!(
        Archive::open(&dir.path().join("missing.kr")),
        Err(Error::Io { .. })
    ));
}

#[test]
fn checksum_failures_are_reported_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = small_archive(dir.path());
    bytes[HEADER_LEN + 40] ^= 0xFF; // inside the first block's payload
    let p = dir.path().join("bad.kr");
    fs::write(&p, &bytes).unwrap();
    let archive = Archive::open(&p).expect("only data is damaged; the index is fine");
    let err = verify(&archive, 2).unwrap_err();
    assert!(
        matches!(err, Error::Checksum(ref m) if m.contains("block 0")),
        "{err}"
    );
    // Extraction refuses too, and does not claim success.
    assert!(extract(&archive, &ExtractOptions::new(dir.path().join("out"))).is_err());
}

// ---- hand-crafted archives -------------------------------------------------------------

/// Assemble a syntactically valid archive from parts, so the tests can lie in controlled ways.
fn craft(entries: Vec<Entry>, data: &[u8], block_log2: u8) -> Vec<u8> {
    let store = CodecSpec::parse("store").unwrap().build().unwrap();
    let mut out = Header {
        default_method: Method::Store,
        block_log2,
    }
    .encode()
    .to_vec();
    let mut offsets = Vec::new();
    for chunk in data.chunks(1 << block_log2) {
        offsets.push(out.len() as u64);
        EncodedBlock::from_raw(store.as_ref(), chunk.to_vec())
            .unwrap()
            .write_to(&mut out)
            .unwrap();
    }
    let index = Index {
        entries,
        total_raw: data.len() as u64,
        block_offsets: offsets,
    };
    let rec = EncodedBlock::from_raw(store.as_ref(), index.encode()).unwrap();
    let index_offset = out.len() as u64;
    rec.write_to(&mut out).unwrap();
    out.extend_from_slice(
        &Footer {
            index_offset,
            index_record_len: rec.header.total_len(),
        }
        .encode(),
    );
    assert!(out.len() > HEADER_LEN + FOOTER_LEN);
    out
}

fn file_entry(path: &str, content: &[u8]) -> Entry {
    let mut h = FileHasher::new();
    h.update(content);
    Entry {
        path: path.into(),
        kind: EntryKind::File,
        size: content.len() as u64,
        mtime_secs: 1_600_000_000,
        mtime_nanos: 0,
        mode: 0o644,
        hash: h.finalize(),
    }
}

#[test]
fn the_crafting_helper_produces_archives_that_really_extract() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ok.kr");
    fs::write(
        &p,
        craft(vec![file_entry("ok/file.txt", b"hello")], b"hello", 12),
    )
    .unwrap();
    let out = dir.path().join("out");
    extract(&Archive::open(&p).unwrap(), &ExtractOptions::new(&out)).unwrap();
    assert_eq!(fs::read(out.join("ok/file.txt")).unwrap(), b"hello");
}

#[test]
fn path_traversal_names_are_rejected_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = dir.path().join("sandbox");
    fs::create_dir_all(&sandbox).unwrap();
    for evil in [
        "../evil.txt",
        "/tmp/evil.txt",
        "a/../../evil.txt",
        "..\\evil.txt",
        "a//b",
        "./x",
    ] {
        let p = dir.path().join("evil.kr");
        fs::write(&p, craft(vec![file_entry(evil, b"pwn")], b"pwn", 12)).unwrap();
        let err = Archive::open(&p).unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{evil}: {err}");
    }
    assert_eq!(
        fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != "evil.kr")
            .count(),
        1
    );
    assert_eq!(fs::read_dir(&sandbox).unwrap().count(), 0);
    assert!(!dir.path().join("evil.txt").exists());
}

#[test]
fn a_wrong_file_hash_is_caught_even_when_every_block_checksum_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = file_entry("f.txt", b"hello");
    e.hash = [7; 32];
    let p = dir.path().join("lie.kr");
    fs::write(&p, craft(vec![e], b"hello", 12)).unwrap();
    let archive = Archive::open(&p).unwrap();
    let err = verify(&archive, 1).unwrap_err();
    assert!(
        matches!(err, Error::Checksum(ref m) if m.contains("f.txt")),
        "{err}"
    );
    assert!(extract(&archive, &ExtractOptions::new(dir.path().join("out"))).is_err());

    let mut z = file_entry("z", b"");
    z.hash = [1; 32];
    fs::write(&p, craft(vec![z], b"", 12)).unwrap();
    assert!(matches!(
        verify(&Archive::open(&p).unwrap(), 1),
        Err(Error::Checksum(_))
    ));
}

#[test]
fn inconsistent_indexes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.kr");
    let content = b"0123456789";

    // Declared size larger than the data actually present.
    let mut e = file_entry("a", content);
    e.size = 20;
    fs::write(&p, craft(vec![e], content, 12)).unwrap();
    assert!(Archive::open(&p).is_err());

    // Duplicate names.
    let (a, b) = (file_entry("a", b"01234"), file_entry("a", b"56789"));
    fs::write(&p, craft(vec![a, b], content, 12)).unwrap();
    assert!(matches!(Archive::open(&p), Err(Error::Corrupt(_))));

    // Two empty files with the same hash but different names are fine.
    let mut z1 = file_entry("z1", b"");
    let mut z2 = file_entry("z2", b"");
    z1.hash = empty_hash();
    z2.hash = empty_hash();
    fs::write(&p, craft(vec![z1, z2], b"", 12)).unwrap();
    assert!(verify(&Archive::open(&p).unwrap(), 1).is_ok());
}
