//! End-to-end behaviour of pack / extract / verify through the public library API.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::*;
use kiln::codec::CodecSpec;
use kiln::{Archive, Error, ExtractOptions, PackOptions, extract, pack, verify};

fn opts(codec: &str, block_log2: u8, threads: usize) -> PackOptions {
    PackOptions {
        codec: CodecSpec::parse(codec).unwrap(),
        block_log2,
        threads,
        overwrite: false,
        ..PackOptions::default()
    }
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    dir
}

fn proj(dir: &Path) -> Vec<PathBuf> {
    vec![dir.join("proj")]
}

#[test]
fn every_codec_and_thread_count_round_trips_exactly() {
    let src = fixture();
    for codec in ["store", "lz4", "zstd", "zstd:19", "lzma:3"] {
        for threads in [1, 3] {
            let work = tempfile::tempdir().unwrap();
            let arc = work.path().join("a.kiln");
            // 64 KiB blocks: the fixture spans dozens of blocks and files straddle boundaries.
            let stats = pack(&arc, &proj(src.path()), &opts(codec, 16, threads)).unwrap();
            assert!(stats.blocks > 20, "{codec}: only {} blocks", stats.blocks);

            let archive = Archive::open(&arc).unwrap();
            let out = work.path().join("out");
            let ex = extract(
                &archive,
                &ExtractOptions {
                    threads,
                    ..ExtractOptions::new(&out)
                },
            )
            .unwrap();
            assert_eq!(ex.files, stats.files, "{codec}");
            assert_eq!(ex.bytes, stats.raw_bytes);
            assert_eq!(ex.metadata_errors, 0, "{codec}");
            assert_same_tree(&src.path().join("proj"), &out.join("proj"));
        }
    }
}

#[test]
fn compression_is_real_and_incompressible_blocks_are_stored() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    let stats = pack(&arc, &proj(src.path()), &opts("zstd", 16, 2)).unwrap();
    assert!(stats.archive_bytes < stats.raw_bytes / 2, "{stats:?}");
    assert!(
        stats.stored_blocks >= 4,
        "noise.bin (300 kB) should be stored: {stats:?}"
    );
    assert_eq!(fs::metadata(&arc).unwrap().len(), stats.archive_bytes);
}

#[test]
fn output_is_byte_identical_regardless_of_thread_count() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let mut outputs = Vec::new();
    for (i, threads) in [1, 2, 7, 0].into_iter().enumerate() {
        let arc = work.path().join(format!("a{i}.kiln"));
        pack(&arc, &proj(src.path()), &opts("zstd:5", 16, threads)).unwrap();
        outputs.push(fs::read(arc).unwrap());
    }
    assert!(
        outputs.windows(2).all(|w| w[0] == w[1]),
        "archives differ between thread counts"
    );
}

#[test]
fn verify_and_listing() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    let stats = pack(&arc, &proj(src.path()), &opts("lz4", 15, 2)).unwrap();
    let archive = Archive::open(&arc).unwrap();
    let v = verify(&archive, 2).unwrap();
    assert_eq!(
        (v.files, v.dirs, v.bytes),
        (stats.files, stats.dirs, stats.raw_bytes)
    );
    assert_eq!(v.blocks as usize, archive.block_count());
    assert_eq!(archive.total_raw(), stats.raw_bytes);

    let names: Vec<&str> = archive.entries().iter().map(|e| e.path.as_str()).collect();
    assert!(names.contains(&"proj/empty_dir"));
    assert!(names.contains(&"proj/ünï cödé/файл.txt"));
    assert!(names.windows(2).all(|w| w[0] < w[1]), "entries are sorted");
}

#[test]
fn selective_extraction_decodes_only_what_it_needs() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    pack(&arc, &proj(src.path()), &opts("zstd", 14, 2)).unwrap();
    let archive = Archive::open(&arc).unwrap();

    // One small file.
    let out = work.path().join("one");
    let ex = extract(
        &archive,
        &ExtractOptions {
            members: vec!["proj/readme.txt".into()],
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_eq!(ex.files, 1);
    assert!(
        ex.blocks <= 2,
        "decoded {} of {} blocks",
        ex.blocks,
        archive.block_count()
    );
    assert_eq!(
        fs::read(out.join("proj/readme.txt")).unwrap(),
        fs::read(src.path().join("proj/readme.txt")).unwrap()
    );
    assert!(!out.join("proj/src").exists());

    // A directory (with a ./ prefix and trailing slash, as a user might type it) plus a file.
    let out = work.path().join("two");
    let ex = extract(
        &archive,
        &ExtractOptions {
            members: vec!["./proj/src/".into(), "proj/one_byte".into()],
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_eq!(ex.files, 3);
    for f in ["src/main.rs", "src/lib.rs", "one_byte"] {
        assert_eq!(
            fs::read(out.join("proj").join(f)).unwrap(),
            fs::read(src.path().join("proj").join(f)).unwrap(),
            "{f}"
        );
    }
    assert!(ex.blocks < archive.block_count() as u64);

    // A prefix that is not a whole path component must not match.
    let err = extract(
        &archive,
        &ExtractOptions {
            members: vec!["proj/rea".into()],
            ..ExtractOptions::new(work.path().join("x"))
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(ref m) if m.contains("proj/rea")),
        "{err}"
    );
}

#[test]
fn empty_inputs_round_trip_with_zero_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("e");
    fs::create_dir_all(root.join("sub/deeper")).unwrap();
    fs::write(root.join("sub/zero.txt"), b"").unwrap();
    let arc = dir.path().join("e.kiln");
    let stats = pack(&arc, std::slice::from_ref(&root), &opts("zstd", 16, 2)).unwrap();
    assert_eq!((stats.blocks, stats.raw_bytes), (0, 0));
    let out = dir.path().join("out");
    extract(&Archive::open(&arc).unwrap(), &ExtractOptions::new(&out)).unwrap();
    assert_same_tree(&root, &out.join("e"));
}

#[test]
fn many_small_files_share_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("many");
    fs::create_dir_all(&root).unwrap();
    for i in 0..1500 {
        fs::write(
            root.join(format!("f{i:04}.txt")),
            prose(50 + i % 300, i as u64),
        )
        .unwrap();
    }
    let arc = dir.path().join("m.kiln");
    let stats = pack(&arc, std::slice::from_ref(&root), &opts("zstd", 16, 3)).unwrap();
    assert_eq!(stats.files, 1500);
    assert!(
        stats.blocks <= 5,
        "1500 tiny files should pack into few blocks: {}",
        stats.blocks
    );
    let out = dir.path().join("out");
    extract(&Archive::open(&arc).unwrap(), &ExtractOptions::new(&out)).unwrap();
    assert_same_tree(&root, &out.join("many"));
}

#[test]
fn refuses_to_overwrite_unless_forced() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    pack(&arc, &proj(src.path()), &opts("lz4", 16, 1)).unwrap();
    let first = fs::read(&arc).unwrap();

    assert!(matches!(
        pack(&arc, &proj(src.path()), &opts("lz4", 16, 1)),
        Err(Error::Exists(_))
    ));
    assert_eq!(fs::read(&arc).unwrap(), first, "existing archive untouched");

    let forced = PackOptions {
        overwrite: true,
        ..opts("lz4", 16, 1)
    };
    pack(&arc, &proj(src.path()), &forced).unwrap();

    // Extraction: same rules.
    let archive = Archive::open(&arc).unwrap();
    let out = work.path().join("out");
    extract(&archive, &ExtractOptions::new(&out)).unwrap();
    assert!(matches!(
        extract(&archive, &ExtractOptions::new(&out)),
        Err(Error::Exists(_))
    ));
    extract(
        &archive,
        &ExtractOptions {
            overwrite: true,
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_same_tree(&src.path().join("proj"), &out.join("proj"));
}

#[cfg(unix)]
#[test]
fn forced_extraction_replaces_a_planted_symlink_instead_of_following_it() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    pack(
        &arc,
        &[src.path().join("proj/readme.txt")],
        &opts("lz4", 16, 1),
    )
    .unwrap();

    let victim = work.path().join("victim.txt");
    fs::write(&victim, b"precious").unwrap();
    let out = work.path().join("out");
    fs::create_dir_all(&out).unwrap();
    std::os::unix::fs::symlink(&victim, out.join("readme.txt")).unwrap();

    let archive = Archive::open(&arc).unwrap();
    extract(
        &archive,
        &ExtractOptions {
            overwrite: true,
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(&victim).unwrap(),
        b"precious",
        "the symlink target must not be written"
    );
    assert!(
        !fs::symlink_metadata(out.join("readme.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn failed_pack_leaves_no_output_and_no_temp_files() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    let err = pack(
        &arc,
        &[src.path().join("proj"), src.path().join("does_not_exist")],
        &opts("zstd", 16, 2),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err}");
    assert_eq!(
        fs::read_dir(work.path()).unwrap().count(),
        0,
        "nothing may be left behind"
    );

    // A failure must also not destroy an archive that already existed.
    pack(&arc, &proj(src.path()), &opts("lz4", 16, 1)).unwrap();
    let before = fs::read(&arc).unwrap();
    let forced = PackOptions {
        overwrite: true,
        ..opts("zstd", 16, 2)
    };
    assert!(pack(&arc, &[src.path().join("does_not_exist")], &forced).is_err());
    assert_eq!(fs::read(&arc).unwrap(), before);
    assert_eq!(fs::read_dir(work.path()).unwrap().count(), 1);
}

#[test]
fn archive_inside_the_input_tree_does_not_archive_itself() {
    let src = fixture();
    let arc = src.path().join("proj/self.kiln");
    let stats = pack(
        &arc,
        &proj(src.path()),
        &PackOptions {
            overwrite: true,
            ..opts("lz4", 16, 2)
        },
    )
    .unwrap();
    let archive = Archive::open(&arc).unwrap();
    assert!(
        archive
            .entries()
            .iter()
            .all(|e| !e.path.ends_with("self.kiln") && !e.path.ends_with(".tmp"))
    );
    assert_eq!(archive.total_raw(), stats.raw_bytes);
}

#[test]
fn invalid_options_are_rejected_up_front() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    let bad_block = PackOptions {
        block_log2: 5,
        ..PackOptions::default()
    };
    assert!(matches!(
        pack(&arc, &proj(src.path()), &bad_block),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        pack(&arc, &[], &PackOptions::default()),
        Err(Error::Invalid(_))
    ));
    assert!(!arc.exists());
}

#[cfg(unix)]
#[test]
fn symlinks_are_reported_not_followed() {
    let src = fixture();
    std::os::unix::fs::symlink("/etc", src.path().join("proj/etc_link")).unwrap();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    let stats = pack(&arc, &proj(src.path()), &opts("lz4", 16, 1)).unwrap();
    assert_eq!(stats.skipped.len(), 1);
    assert_eq!(stats.skipped[0].reason, "symbolic link");
    let archive = Archive::open(&arc).unwrap();
    assert!(
        archive
            .entries()
            .iter()
            .all(|e| !e.path.contains("etc_link"))
    );
}

#[test]
fn restores_modification_times_and_permissions() {
    let src = fixture();
    let work = tempfile::tempdir().unwrap();
    let arc = work.path().join("a.kiln");
    pack(&arc, &proj(src.path()), &opts("zstd", 16, 2)).unwrap();
    let out = work.path().join("out");
    extract(&Archive::open(&arc).unwrap(), &ExtractOptions::new(&out)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| fs::metadata(out.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("proj/run.sh"), 0o755);
        assert_eq!(mode("proj/readme.txt"), 0o640);
    }
    // snapshot() compares mtimes of files and directories; assert_same_tree covers both.
    assert_same_tree(&src.path().join("proj"), &out.join("proj"));
}
