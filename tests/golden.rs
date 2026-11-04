//! Compatibility test: a checked-in archive written by kiln 0.3.0 must stay readable.
//!
//! `tests/golden/v2-adaptive.kiln` was created once with
//! `kiln create v2-adaptive.kiln golden/ -p adaptive` from the five files reconstructed below
//! (see `docs/FORMAT.md`, "Compatibility policy"). If a future change makes this test fail, the
//! change broke compatibility with version 2 archives: bump the format version instead of editing
//! the golden file.

use std::fs;
use std::path::PathBuf;

use kiln::classify::Class;
use kiln::{Archive, ExtractOptions, Profile, extract, verify};

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/v2-adaptive.kiln")
}

/// The exact contents the golden archive was made from.
fn expected() -> Vec<(&'static str, Vec<u8>)> {
    let numbers: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend((0..3000u32).map(|i| ((i * 13 + 1) & 0xff) as u8));
    vec![
        ("golden/hello.txt", b"Hello, Kiln v2!\n".to_vec()),
        ("golden/dir/numbers.txt", numbers.into_bytes()),
        ("golden/dir/empty", Vec::new()),
        (
            "golden/bin/blob.bin",
            (0..5000u32).map(|i| ((i * 7 + 3) & 0xff) as u8).collect(),
        ),
        ("golden/bin/pic.png", png),
    ]
}

#[test]
fn the_checked_in_v2_archive_still_opens_verifies_and_extracts() {
    let archive = Archive::open(&golden()).expect("golden archive must open");
    assert_eq!(archive.header().profile, Profile::Adaptive);
    verify(&archive, 2).expect("every checksum in the golden archive must verify");

    let out = tempfile::tempdir().unwrap();
    extract(&archive, &ExtractOptions::new(out.path())).unwrap();
    for (name, bytes) in expected() {
        let got = fs::read(out.path().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(got, bytes, "{name}");
    }
}

#[test]
fn the_golden_archive_keeps_its_recorded_classes_and_structure() {
    let archive = Archive::open(&golden()).unwrap();
    let entries = archive.entries();
    assert_eq!(entries.len(), 8, "5 files and 3 directories");
    let class_of = |name: &str| {
        entries
            .iter()
            .find(|e| e.path == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .class
    };
    assert_eq!(class_of("golden/hello.txt"), Class::Text);
    assert_eq!(class_of("golden/bin/pic.png"), Class::Compressed);
    assert_eq!(class_of("golden/bin/blob.bin"), Class::Data);
}
