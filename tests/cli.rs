//! Black-box tests of the `kiln` binary: arguments, output, exit codes.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::*;

fn kiln(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn ok(args: &[&str], cwd: &Path) -> String {
    let out = kiln(args, cwd);
    assert!(
        out.status.success(),
        "kiln {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fails(args: &[&str], cwd: &Path) -> String {
    let out = kiln(args, cwd);
    assert!(
        !out.status.success(),
        "kiln {args:?} unexpectedly succeeded"
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn pack_list_test_unpack_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());

    let summary = ok(
        &[
            "pack", "a.kiln", "proj", "-c", "zstd:5", "-b", "64KiB", "-j", "2",
        ],
        dir.path(),
    );
    assert!(
        summary.contains("packed") && summary.contains("zstd:5"),
        "{summary}"
    );

    let listing = ok(&["list", "a.kiln"], dir.path());
    assert!(
        listing.lines().any(|l| l == "proj/src/main.rs"),
        "{listing}"
    );
    assert!(
        listing.lines().any(|l| l == "proj/empty_dir/"),
        "directories get a trailing slash"
    );
    let long = ok(&["list", "-l", "a.kiln"], dir.path());
    assert!(
        long.contains("proj/readme.txt") && long.contains("3000"),
        "{long}"
    );

    let t = ok(&["test", "a.kiln", "-j", "2"], dir.path());
    assert!(t.starts_with("OK:"), "{t}");

    let u = ok(&["unpack", "a.kiln", "-o", "out", "-j", "3"], dir.path());
    assert!(u.contains("extracted"), "{u}");
    assert_same_tree(&dir.path().join("proj"), &dir.path().join("out/proj"));

    // Subset extraction.
    ok(
        &["unpack", "a.kiln", "proj/readme.txt", "-o", "one"],
        dir.path(),
    );
    assert!(dir.path().join("one/proj/readme.txt").is_file());
    assert!(!dir.path().join("one/proj/src").exists());
}

#[test]
fn quiet_mode_prints_nothing_on_success() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    assert_eq!(ok(&["pack", "-q", "a.kiln", "proj"], dir.path()), "");
    assert_eq!(ok(&["unpack", "-q", "a.kiln", "-o", "o"], dir.path()), "");
}

#[test]
fn errors_are_reported_with_a_message_and_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    ok(&["pack", "-q", "a.kiln", "proj"], dir.path());

    assert!(fails(&["pack", "a.kiln", "proj"], dir.path()).contains("already exists"));
    ok(&["pack", "-q", "-f", "a.kiln", "proj"], dir.path());
    assert!(fails(&["pack", "b.kiln", "proj", "-c", "gzip"], dir.path()).contains("unknown codec"));
    assert!(fails(&["pack", "b.kiln", "proj", "-c", "zstd:99"], dir.path()).contains("level"));
    assert!(fails(&["pack", "b.kiln", "nope"], dir.path()).starts_with("error: nope"));
    assert!(fails(&["list", "missing.kiln"], dir.path()).starts_with("error:"));
    assert!(
        fails(
            &["unpack", "a.kiln", "no/such/member", "-o", "o"],
            dir.path()
        )
        .contains("no such member")
    );

    fs::write(dir.path().join("junk.kiln"), b"definitely not an archive").unwrap();
    assert!(fails(&["test", "junk.kiln"], dir.path()).contains("not a Kiln archive"));

    // Corrupt one byte in the middle: `test` must fail loudly.
    let mut bytes = fs::read(dir.path().join("a.kiln")).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(dir.path().join("bad.kiln"), bytes).unwrap();
    let err = fails(&["test", "bad.kiln"], dir.path());
    assert!(err.contains("corrupt") || err.contains("checksum"), "{err}");
}

#[test]
fn block_size_must_be_a_sane_power_of_two() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    for bad in ["3000", "1KiB", "1GiB", "lots"] {
        let err = fails(&["pack", "b.kiln", "proj", "-b", bad], dir.path());
        assert!(err.contains("block"), "{bad}: {err}");
    }
    assert!(!dir.path().join("b.kiln").exists());
}

#[test]
fn bench_prints_a_row_per_codec() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    let out = ok(
        &[
            "bench", "proj", "-c", "store", "-c", "lz4", "-c", "zstd:1", "-b", "256KiB",
        ],
        dir.path(),
    );
    for name in ["store", "lz4", "zstd:1"] {
        assert!(
            out.lines().any(|l| l.starts_with(name)),
            "{name} missing:\n{out}"
        );
    }
}

#[test]
fn help_and_version_work() {
    let dir = tempfile::tempdir().unwrap();
    assert!(ok(&["--help"], dir.path()).contains("pack"));
    assert!(ok(&["--version"], dir.path()).contains("kiln"));
    assert!(ok(&["pack", "--help"], dir.path()).contains("--codec"));
}
