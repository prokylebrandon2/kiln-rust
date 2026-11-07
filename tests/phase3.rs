//! Phase 3: the `.kiln` v2 format, profiles, classification, parallel extraction, selective
//! extraction and the new resource limits.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use kiln::classify::Class;
use kiln::codec::{CodecSpec, Method};
use kiln::format::index::{
    ChunkInfo, Entry, EntryKind, Extension, Index, TAG_CRITICAL, dedup_extension,
};
use kiln::format::record::EncodedBlock;
use kiln::format::{FOOTER_LEN, Footer, HEADER_LEN, Header};
use kiln::integrity::FileHasher;
use kiln::{Archive, Error, ExtractOptions, PackOptions, Profile, extract, pack, verify};

fn opts(profile: Profile, threads: usize) -> PackOptions {
    PackOptions {
        threads,
        block_log2: 16,
        ..PackOptions::from_profile(profile)
    }
}

/// A tree mixing text, noise, a PNG-looking file and an executable-looking file.
fn mixed_tree(root: &Path) {
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::create_dir_all(root.join("media")).unwrap();
    fs::write(root.join("docs/a.txt"), prose(150_000, 1)).unwrap();
    fs::write(root.join("docs/b.txt"), prose(90_000, 2)).unwrap();
    fs::write(root.join("media/noise.dat"), noise(200_000, 3)).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(prose(50_000, 4)); // magic says compressed even though the body is text
    fs::write(root.join("media/pic.png"), png).unwrap();
    let mut elf = b"\x7FELF\x02\x01\x01\x00".to_vec();
    elf.extend(noise(100, 5));
    elf.extend(prose(40_000, 6));
    fs::write(root.join("prog"), elf).unwrap();
    fs::write(root.join("tiny"), b"x").unwrap();
    fs::write(root.join("empty"), b"").unwrap();
}

#[test]
fn every_profile_round_trips_exactly_and_is_recorded_in_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    for profile in Profile::NAMED {
        let arc = dir.path().join(format!("{}.kiln", profile.name()));
        let mut o = opts(profile, 2);
        if profile == Profile::Maximum {
            o.codec = CodecSpec::parse("zstd:9").unwrap(); // keep the test quick; same code path
        }
        pack(&arc, std::slice::from_ref(&src), &o).unwrap();
        let a = Archive::open(&arc).unwrap();
        assert_eq!(a.header().profile, profile);
        verify(&a, 2).unwrap();
        let out = dir.path().join(format!("out-{}", profile.name()));
        extract(&a, &ExtractOptions::new(&out)).unwrap();
        assert_same_tree(&src, &out.join("src"));
    }
}

#[test]
fn explicit_codec_overrides_and_is_recorded_as_custom() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    for spec in ["lz4", "zstd:19", "lzma:3", "store"] {
        let arc = dir.path().join("c.kiln");
        let o = PackOptions {
            profile: Profile::Custom,
            codec: CodecSpec::parse(spec).unwrap(),
            overwrite: true,
            block_log2: 15,
            ..PackOptions::default()
        };
        pack(&arc, std::slice::from_ref(&src), &o).unwrap();
        let a = Archive::open(&arc).unwrap();
        assert_eq!(a.header().profile, Profile::Custom);
        let out = dir.path().join("out");
        let _ = fs::remove_dir_all(&out);
        extract(&a, &ExtractOptions::new(&out)).unwrap();
        assert_same_tree(&src, &out.join("src"));
    }
}

#[test]
fn classification_stores_incompressible_files_in_their_own_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    let arc = dir.path().join("a.kiln");
    let stats = pack(
        &arc,
        std::slice::from_ref(&src),
        &opts(Profile::Adaptive, 2),
    )
    .unwrap();
    assert_eq!(
        stats.classified_stored_files, 2,
        "noise.dat and pic.png: {stats:?}"
    );

    let a = Archive::open(&arc).unwrap();
    let class_of = |name: &str| {
        a.entries()
            .iter()
            .find(|e| e.path == format!("src/{name}"))
            .unwrap()
            .class
    };
    assert_eq!(class_of("docs/a.txt"), Class::Text);
    assert_eq!(class_of("media/noise.dat"), Class::Random);
    assert_eq!(class_of("media/pic.png"), Class::Compressed);
    assert_eq!(class_of("prog"), Class::Binary);

    // Data order: compressible files first, stored files last, and no chunk mixes the two.
    let idx = a.index();
    let first_stored = idx
        .entries
        .iter()
        .position(|e| e.class.is_incompressible())
        .unwrap();
    assert!(
        idx.entries[first_stored..]
            .iter()
            .all(|e| e.kind == EntryKind::Dir || e.class.is_incompressible() || e.size == 0)
    );
    for (i, e) in idx.entries.iter().enumerate() {
        match a.entry_method(i) {
            Some(m) if e.class.is_incompressible() => assert_eq!(m, Method::Store, "{}", e.path),
            Some(m) if e.size > 1000 => assert_eq!(m, Method::Zstd, "{}", e.path),
            _ => {}
        }
        if e.kind == EntryKind::File && e.size > 0 {
            // Last byte lives in a chunk of the same kind as the first byte.
            let last = idx.chunk_containing(idx.entry_starts[i] + e.size - 1);
            let first = idx.chunk_containing(idx.entry_starts[i]);
            assert_eq!(
                idx.chunks[first].method == Method::Store,
                idx.chunks[last].method == Method::Store,
                "{} straddles a store/compress boundary",
                e.path
            );
        }
    }
    // Listing still round-trips.
    let out = dir.path().join("out");
    extract(&a, &ExtractOptions::new(&out)).unwrap();
    assert_same_tree(&src, &out.join("src"));
}

#[test]
fn adaptive_profile_groups_files_with_equal_extensions_in_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let exts = ["txt", "rs", "md"];
    for i in 0..18u64 {
        let name = format!("f{i:02}.{}", exts[i as usize % exts.len()]);
        fs::write(src.join(name), prose(3_000 + i as usize * 111, i)).unwrap();
    }
    let arc = dir.path().join("g.kiln");
    pack(
        &arc,
        std::slice::from_ref(&src),
        &PackOptions::from_profile(Profile::Adaptive),
    )
    .unwrap();
    let a = Archive::open(&arc).unwrap();
    let order: Vec<&str> = a
        .entries()
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| e.path.rsplit('.').next().unwrap())
        .collect();
    assert_eq!(order.len(), 18);
    let mut runs = order.clone();
    runs.dedup();
    assert_eq!(
        runs,
        ["md", "rs", "txt"],
        "extensions interleaved: {order:?}"
    );

    let out = dir.path().join("out");
    extract(&a, &ExtractOptions::new(&out)).unwrap();
    assert_same_tree(&src, &out.join("src"));
}

#[test]
fn non_classifying_profiles_keep_walk_order_and_unknown_class() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    let arc = dir.path().join("a.kiln");
    pack(&arc, &[src], &opts(Profile::Balanced, 2)).unwrap();
    let a = Archive::open(&arc).unwrap();
    assert!(a.entries().iter().all(|e| e.class == Class::Unknown));
    let names: Vec<&str> = a.entries().iter().map(|e| e.path.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "walk order is sorted order");
}

#[test]
fn selective_extraction_decodes_only_the_chunks_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("d")).unwrap();
    for i in 0..40u64 {
        fs::write(src.join(format!("d/f{i:02}.txt")), prose(60_000, 100 + i)).unwrap();
    }
    fs::write(src.join("lone.bin"), noise(30_000, 9)).unwrap();
    let arc = dir.path().join("a.kiln");
    pack(
        &arc,
        std::slice::from_ref(&src),
        &opts(Profile::Balanced, 2),
    )
    .unwrap();
    let a = Archive::open(&arc).unwrap();
    let total_chunks = a.block_count() as u64;
    assert!(total_chunks > 20);

    let out = dir.path().join("one");
    let stats = extract(
        &a,
        &ExtractOptions {
            members: vec!["src/d/f20.txt".into()],
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_eq!(stats.files, 1);
    assert!(
        stats.blocks <= 2,
        "a 60 KB file spans at most two 64 KiB chunks: {stats:?}"
    );
    assert_eq!(
        fs::read(out.join("src/d/f20.txt")).unwrap(),
        fs::read(src.join("d/f20.txt")).unwrap()
    );
    assert!(!out.join("src/d/f21.txt").exists());

    // A directory selection and an unknown member.
    let out2 = dir.path().join("dir");
    let stats = extract(
        &a,
        &ExtractOptions {
            members: vec!["src/d".into()],
            ..ExtractOptions::new(&out2)
        },
    )
    .unwrap();
    assert_eq!(stats.files, 40);
    assert!(!out2.join("src/lone.bin").exists());
    let err = extract(
        &a,
        &ExtractOptions {
            members: vec!["src/nope".into()],
            ..ExtractOptions::new(dir.path().join("x"))
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)));
}

#[test]
fn archives_are_byte_identical_whatever_the_thread_count() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    for profile in [Profile::Balanced, Profile::Adaptive] {
        let mut previous: Option<Vec<u8>> = None;
        for threads in [1, 2, 3, 8] {
            let arc = dir.path().join(format!("t{threads}.kiln"));
            let _ = fs::remove_file(&arc);
            pack(&arc, std::slice::from_ref(&src), &opts(profile, threads)).unwrap();
            let bytes = fs::read(&arc).unwrap();
            if let Some(p) = &previous {
                assert!(
                    *p == bytes,
                    "{profile:?}: output differs with {threads} threads"
                );
            }
            previous = Some(bytes);
        }
    }
}

#[test]
fn parallel_extraction_of_many_small_files_is_exact_for_every_thread_count() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    // 1500 files in 300 directories, plus a flat directory, empty files, and empty directories.
    for i in 0..1500u64 {
        let d = src.join(format!("d{:03}/s{}", i % 300, i % 7));
        fs::create_dir_all(&d).unwrap();
        let content = if i % 11 == 0 {
            Vec::new()
        } else {
            prose(100 + (i as usize * 37) % 9000, i)
        };
        fs::write(d.join(format!("f{i}.txt")), content).unwrap();
    }
    fs::create_dir_all(src.join("flat")).unwrap();
    for i in 0..200u64 {
        fs::write(src.join(format!("flat/g{i}")), noise(50 + i as usize, i)).unwrap();
    }
    fs::create_dir_all(src.join("hollow/deeper")).unwrap();
    fs::write(src.join("big.bin"), prose(700_000, 77)).unwrap(); // above the small-file cutoff
    let arc = dir.path().join("a.kiln");
    pack(
        &arc,
        std::slice::from_ref(&src),
        &opts(Profile::Balanced, 2),
    )
    .unwrap();
    let a = Archive::open(&arc).unwrap();
    for threads in [1, 2, 4, 8] {
        let out = dir.path().join(format!("out{threads}"));
        let stats = extract(
            &a,
            &ExtractOptions {
                threads,
                ..ExtractOptions::new(&out)
            },
        )
        .unwrap();
        assert_eq!(stats.files, 1500 + 200 + 1, "threads={threads}");
        assert_same_tree(&src, &out.join("src"));
    }
}

#[test]
fn a_tight_memory_limit_still_extracts_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    let arc = dir.path().join("a.kiln");
    pack(
        &arc,
        std::slice::from_ref(&src),
        &opts(Profile::Balanced, 2),
    )
    .unwrap();
    let a = Archive::open(&arc).unwrap();
    let out = dir.path().join("out");
    extract(
        &a,
        &ExtractOptions {
            memory_limit: Some(48 << 20),
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_same_tree(&src, &out.join("src"));
}

#[test]
fn max_bytes_refuses_oversized_extraction_before_writing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    let arc = dir.path().join("a.kiln");
    pack(&arc, &[src], &opts(Profile::Balanced, 2)).unwrap();
    let a = Archive::open(&arc).unwrap();
    let out = dir.path().join("out");
    let err = extract(
        &a,
        &ExtractOptions {
            max_bytes: Some(1000),
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(ref m) if m.contains("limit")),
        "{err}"
    );
    assert!(!out.exists() || fs::read_dir(&out).unwrap().count() == 0);
    // Just enough is fine.
    extract(
        &a,
        &ExtractOptions {
            max_bytes: Some(a.total_raw()),
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn symlinks_inside_the_destination_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    mixed_tree(&src);
    let arc = dir.path().join("a.kiln");
    pack(&arc, &[src], &opts(Profile::Balanced, 2)).unwrap();
    let a = Archive::open(&arc).unwrap();

    // The destination already holds `src -> elsewhere`: extraction must not write through it.
    let out = dir.path().join("out");
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, out.join("src")).unwrap();
    let err = extract(&a, &ExtractOptions::new(&out)).unwrap_err();
    assert!(matches!(err, Error::UnsafePath(_)), "{err}");
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "nothing escaped"
    );
}

// ---- hand-crafted archives -------------------------------------------------------------

fn file_entry(path: &str, content: &[u8]) -> Entry {
    let mut h = FileHasher::new();
    h.update(content);
    Entry {
        path: path.into(),
        kind: EntryKind::File,
        class: Class::Unknown,
        size: content.len() as u64,
        mtime_secs: 1_600_000_000,
        mtime_nanos: 0,
        mode: 0o644,
        hash: h.finalize(),
    }
}

/// Build an archive from stored chunks; `tweak` can lie about the index.
fn craft(entries: Vec<Entry>, data: &[u8], tweak: impl FnOnce(&mut Index)) -> Vec<u8> {
    let store = CodecSpec::parse("store").unwrap().build().unwrap();
    let mut out = Header::new(Profile::Store, 12).encode().to_vec();
    let mut chunks = Vec::new();
    for c in data.chunks(1 << 12) {
        let b = EncodedBlock::from_raw(store.as_ref(), c.to_vec()).unwrap();
        chunks.push(b.info());
        b.write_to(&mut out).unwrap();
    }
    let mut index = Index {
        entries,
        chunks,
        ..Index::default()
    };
    tweak(&mut index);
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

#[test]
fn a_small_file_with_a_wrong_hash_never_reaches_the_file_system() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = file_entry("bad.txt", b"hello");
    e.hash = [3; 16];
    let p = dir.path().join("lie.kiln");
    fs::write(
        &p,
        craft(
            vec![e, file_entry("fine.txt", b"world")],
            b"helloworld",
            |_| {},
        ),
    )
    .unwrap();
    let out = dir.path().join("out");
    let err = extract(&Archive::open(&p).unwrap(), &ExtractOptions::new(&out)).unwrap_err();
    assert!(
        matches!(err, Error::Checksum(ref m) if m.contains("bad.txt")),
        "{err}"
    );
    assert!(!out.join("bad.txt").exists());
    let leftovers: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().ends_with(".tmp") && n != "bad.txt"),
        "{leftovers:?}"
    );
}

#[test]
fn record_headers_that_disagree_with_the_index_are_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.kiln");
    let content = vec![b'q'; 6000];
    // The index claims the first chunk is LZ4 although its record says stored.
    let bytes = craft(vec![file_entry("f", &content)], &content, |i| {
        i.chunks[0].method = Method::Lz4;
    });
    fs::write(&p, bytes).unwrap();
    // Lz4 chunk with comp_len == raw_len is rejected already when the index is parsed or, at
    // the latest, when the record is read.
    let res = Archive::open(&p).and_then(|a| verify(&a, 1).map(|_| ()));
    assert!(
        matches!(res, Err(Error::Corrupt(_)) | Err(Error::Checksum(_))),
        "{res:?}"
    );
}

#[test]
fn critical_unknown_extensions_are_refused_and_unknown_optional_ones_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.kiln");
    let content = b"hello".to_vec();
    let with = |tag: u16| {
        craft(vec![file_entry("f", &content)], &content, move |i| {
            i.extensions.push(Extension {
                tag,
                data: vec![1, 2, 3],
            })
        })
    };
    fs::write(&p, with(0x0042)).unwrap();
    let a = Archive::open(&p).expect("optional unknown extension is skipped");
    verify(&a, 1).unwrap();
    fs::write(&p, with(TAG_CRITICAL | 0x0042)).unwrap();
    assert!(matches!(Archive::open(&p), Err(Error::Unsupported(_))));
}

#[test]
fn decompression_bomb_shaped_index_is_bounded() {
    // An index claiming a 100 TiB file in a tiny archive: it must be rejected at open (the
    // chunk table cannot cover it), not by trying to allocate.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bomb.kiln");
    let mut e = file_entry("huge", b"x");
    e.size = 100 << 40;
    fs::write(&p, craft(vec![e], b"x", |_| {})).unwrap();
    assert!(Archive::open(&p).is_err());
}

#[test]
fn every_byte_flip_and_truncation_of_a_classified_archive_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.txt"), prose(9_000, 5)).unwrap();
    fs::write(src.join("n.bin"), noise(9_000, 6)).unwrap();
    let arc = dir.path().join("c.kiln");
    let o = PackOptions {
        block_log2: 12,
        threads: 2,
        ..PackOptions::from_profile(Profile::Adaptive)
    };
    let stats = pack(&arc, &[src], &o).unwrap();
    assert_eq!(stats.classified_stored_files, 1, "{stats:?}");
    let good = fs::read(&arc).unwrap();
    let probe = dir.path().join("probe.kiln");
    let accepts = |bytes: &[u8]| {
        fs::write(&probe, bytes).unwrap();
        Archive::open(&probe).is_ok_and(|a| verify(&a, 2).is_ok())
    };
    assert!(accepts(&good));
    for i in 0..good.len() {
        let mut bad = good.clone();
        bad[i] ^= 0xA5;
        assert!(!accepts(&bad), "flip at byte {i} was not detected");
    }
    for len in 0..good.len() {
        assert!(!accepts(&good[..len]), "prefix of {len} bytes accepted");
    }
}

#[test]
fn dedup_stores_identical_files_once_and_every_copy_extracts() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("a")).unwrap();
    fs::create_dir_all(src.join("b")).unwrap();
    let big = prose(600_000, 7); // above the in-memory limit of the extractor: streamed
    let small = prose(20_000, 8);
    for d in ["a", "b"] {
        fs::write(src.join(d).join("big.txt"), &big).unwrap();
        fs::write(src.join(d).join("small.txt"), &small).unwrap();
    }
    // Same size as small.txt, different content: must not be taken for a copy.
    fs::write(src.join("a/other.txt"), prose(20_000, 10)).unwrap();
    fs::write(src.join("empty1"), b"").unwrap();
    fs::write(src.join("empty2"), b"").unwrap();

    let plain = dir.path().join("plain.kiln");
    let deduped = dir.path().join("deduped.kiln");
    let o = opts(Profile::Adaptive, 2);
    let before = pack(&plain, std::slice::from_ref(&src), &o).unwrap();
    let dedup_options = PackOptions { dedup: true, ..o };
    let after = pack(&deduped, std::slice::from_ref(&src), &dedup_options).unwrap();
    assert_eq!(after.raw_bytes, before.raw_bytes);
    assert!(
        after.archive_bytes < before.archive_bytes,
        "{after:?} vs {before:?}"
    );

    let a = Archive::open(&deduped).unwrap();
    assert_eq!(a.total_raw(), before.raw_bytes);
    assert_eq!(
        a.index().total_raw,
        before.raw_bytes - (big.len() + small.len()) as u64
    );
    assert_eq!(verify(&a, 2).unwrap().files, 7);
    let out = dir.path().join("out");
    extract(&a, &ExtractOptions::new(&out)).unwrap();
    assert_same_tree(&src, &out.join("src"));

    // Selecting only the copies still extracts them.
    let one = dir.path().join("one");
    let options = ExtractOptions {
        members: vec!["src/b/big.txt".into(), "src/b/small.txt".into()],
        ..ExtractOptions::new(&one)
    };
    extract(&a, &options).unwrap();
    assert_eq!(fs::read(one.join("src/b/big.txt")).unwrap(), big);
    assert_eq!(fs::read(one.join("src/b/small.txt")).unwrap(), small);
}

#[test]
fn a_dedup_table_must_describe_identical_files() {
    let file = |path: &str, size: u64, hash: u8| Entry {
        path: path.into(),
        kind: EntryKind::File,
        class: Class::Unknown,
        size,
        mtime_secs: 0,
        mtime_nanos: 0,
        mode: 0o644,
        hash: [hash; 16],
    };
    let chunk = ChunkInfo {
        raw_len: 5,
        comp_len: 5,
        method: Method::Store,
    };
    let build = |entries: Vec<Entry>, pairs: &[(usize, usize)]| {
        Index::build(entries, vec![chunk], vec![dedup_extension(pairs)])
    };
    let index = build(vec![file("a", 5, 1), file("b", 5, 1)], &[(1, 0)]).unwrap();
    assert_eq!(index.total_raw, 5);
    assert_eq!(index.entry_starts, [0, 0]);

    let bad = |entries: Vec<Entry>, pairs: &[(usize, usize)]| build(entries, pairs).is_err();
    // Different hash, different size, a duplicate of a duplicate, an entry out of range.
    assert!(bad(vec![file("a", 5, 1), file("b", 5, 2)], &[(1, 0)]));
    assert!(bad(vec![file("a", 5, 1), file("b", 4, 1)], &[(1, 0)]));
    assert!(bad(
        vec![file("a", 5, 1), file("b", 5, 1), file("c", 5, 1)],
        &[(1, 0), (2, 1)]
    ));
    assert!(bad(vec![file("a", 5, 1)], &[(1, 0)]));
}

#[test]
fn dedup_copies_extract_without_their_source_and_over_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let big = prose(600_000, 11); // streamed by the extractor
    let small = prose(20_000, 12); // buffered, written by the writer pool
    let random = noise(80_000, 13); // classified incompressible: the stored lane
    for d in ["a", "b", "c"] {
        fs::create_dir_all(src.join(d)).unwrap();
        fs::write(src.join(d).join("big.txt"), &big).unwrap();
        fs::write(src.join(d).join("small.txt"), &small).unwrap();
    }
    for d in ["a", "b"] {
        fs::write(src.join(d).join("noise.dat"), &random).unwrap();
    }
    let arc = dir.path().join("d.kiln");
    let o = PackOptions {
        dedup: true,
        ..opts(Profile::Adaptive, 2)
    };
    let stats = pack(&arc, std::slice::from_ref(&src), &o).unwrap();
    let once = (big.len() + small.len() + random.len()) as u64;
    let logical = 3 * (big.len() + small.len()) as u64 + 2 * random.len() as u64;
    assert_eq!((stats.files, stats.raw_bytes), (8, logical));
    assert_eq!(stats.classified_stored_files, 1, "{stats:?}");

    let a = Archive::open(&arc).unwrap();
    assert_eq!((a.total_raw(), a.index().total_raw), (logical, once));
    let v = verify(&a, 2).unwrap();
    assert_eq!((v.files, v.bytes), (8, logical));

    // Only copies selected: the file their content was stored under is not extracted at all.
    let members = [
        "src/b/big.txt",
        "src/c/big.txt",
        "src/b/small.txt",
        "src/c/small.txt",
        "src/b/noise.dat",
    ];
    let one = dir.path().join("one");
    let options = ExtractOptions {
        members: members.iter().map(|m| (*m).to_string()).collect(),
        ..ExtractOptions::new(&one)
    };
    let s = extract(&a, &options).unwrap();
    let wanted = 2 * (big.len() + small.len()) as u64 + random.len() as u64;
    assert_eq!((s.files, s.bytes), (5, wanted));
    for m in members {
        assert_eq!(
            fs::read(one.join(m)).unwrap(),
            fs::read(dir.path().join(m)).unwrap(),
            "{m}"
        );
    }
    assert!(!one.join("src/a").exists());

    // Overwrite mode replaces damaged files, copies included.
    let out = dir.path().join("out");
    extract(&a, &ExtractOptions::new(&out)).unwrap();
    for m in ["src/b/big.txt", "src/c/small.txt", "src/b/noise.dat"] {
        fs::write(out.join(m), b"damaged").unwrap();
    }
    let again = ExtractOptions {
        overwrite: true,
        ..ExtractOptions::new(&out)
    };
    extract(&a, &again).unwrap();
    assert_same_tree(&src, &out.join("src"));
}

#[test]
fn every_byte_flip_and_truncation_of_a_deduplicated_archive_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir_all(&src).unwrap();
    for name in ["t1.txt", "t2.txt"] {
        fs::write(src.join(name), prose(9_000, 5)).unwrap();
    }
    for name in ["n1.bin", "n2.bin"] {
        fs::write(src.join(name), noise(9_000, 6)).unwrap();
    }
    let arc = dir.path().join("c.kiln");
    let o = PackOptions {
        block_log2: 12,
        threads: 2,
        dedup: true,
        ..PackOptions::from_profile(Profile::Adaptive)
    };
    let stats = pack(&arc, &[src], &o).unwrap();
    assert_eq!(stats.files, 4);
    assert_eq!(Archive::open(&arc).unwrap().index().total_raw, 18_000);
    let good = fs::read(&arc).unwrap();
    let probe = dir.path().join("probe.kiln");
    let accepts = |bytes: &[u8]| {
        fs::write(&probe, bytes).unwrap();
        Archive::open(&probe).is_ok_and(|a| verify(&a, 2).is_ok())
    };
    assert!(accepts(&good));
    for i in 0..good.len() {
        let mut bad = good.clone();
        bad[i] ^= 0xA5;
        assert!(!accepts(&bad), "flip at byte {i} was not detected");
    }
    for len in 0..good.len() {
        assert!(!accepts(&good[..len]), "prefix of {len} bytes accepted");
    }
}

#[test]
fn hard_linked_inputs_are_stored_once_and_extract_as_separate_files() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let body = prose(30_000, 21);
    fs::write(src.join("orig.txt"), &body).unwrap();
    if fs::hard_link(src.join("orig.txt"), src.join("link.txt")).is_err() {
        return; // this file system has no hard links
    }
    let arc = dir.path().join("h.kiln");
    let o = PackOptions {
        dedup: true,
        ..opts(Profile::Balanced, 2)
    };
    pack(&arc, std::slice::from_ref(&src), &o).unwrap();
    let a = Archive::open(&arc).unwrap();
    assert_eq!(a.index().total_raw, body.len() as u64);
    assert_eq!(a.total_raw(), 2 * body.len() as u64);
    let out = dir.path().join("out");
    extract(&a, &ExtractOptions::new(&out)).unwrap();
    assert_same_tree(&src, &out.join("src"));
}
