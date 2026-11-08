//! Compatibility test: the checked-in `.skln` version 1 files must stay readable.
//!
//! `tests/golden/*.skln` were written once by `tests/golden/make_v1.py`, an independent
//! implementation of `docs/FORMAT.md` in Python (fixed key and salts, so they are reproducible).
//! If a future change makes this test fail, the change broke version 1: bump the format version
//! instead of editing or regenerating the golden files.

use kiln_skln::bundle::{open_bundle, Member};
use kiln_skln::{open, peek, Error, Kind, PASSWORD_BLOCK_LEN};

fn key() -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = i as u8;
    }
    k
}

fn golden(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn the_v1_bundle_still_opens() {
    let file = golden("v1-bundle.skln");
    assert_eq!(file.len(), 14079);

    let info = peek(&file).unwrap();
    assert_eq!(info.kind, Kind::Bundle);
    assert_eq!(info.revision, 42);
    assert_eq!(info.chunk_log2, 12);
    assert!(info.password_block.is_none());

    let lines: String = (1..=1500).map(|i| format!("line {i}\n")).collect();
    let expected = vec![
        Member::new("vault.json", lines.into_bytes()),
        Member::new("readme.txt", b"Silk vault golden file\n".to_vec()),
        Member::new("empty", Vec::new()),
    ];
    let opened = open_bundle(&key(), &file).unwrap();
    assert_eq!(opened.revision, 42);
    assert_eq!(opened.members, expected);

    assert_eq!(open_bundle(&[0u8; 32], &file).err(), Some(Error::Corrupt));
}

#[test]
fn the_v1_archive_with_a_password_block_still_opens() {
    let file = golden("v1-archive-password-block.skln");
    assert_eq!(file.len(), 93);

    let mut block = [0u8; PASSWORD_BLOCK_LEN];
    block[0] = 1; // scrypt
    block[1] = 17; // log_n
    block[2..6].copy_from_slice(&8u32.to_le_bytes()); // r
    block[6..10].copy_from_slice(&1u32.to_le_bytes()); // p
    for (i, b) in block[10..].iter_mut().enumerate() {
        *b = 0xA0 + i as u8; // kdf_salt
    }
    let info = peek(&file).unwrap();
    assert_eq!(info.kind, Kind::Archive);
    assert_eq!(info.revision, 0);
    assert_eq!(info.chunk_log2, 20);
    assert_eq!(info.password_block, Some(block));

    let opened = open(&key(), &file).unwrap();
    assert_eq!(opened.kind, Kind::Archive);
    assert_eq!(&opened.plaintext[..], b"hello, skln");
}
