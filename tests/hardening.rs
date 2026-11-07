//! Extraction into a hostile or changing destination (Unix: descriptor-relative, no-follow).
//!
//! These tests describe the contract in `docs/HARDENING.md`: nothing below the destination is ever
//! followed through a symbolic link, not even one that appears while extraction is running.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use common::make_tree;
use kiln::{Archive, Error, ExtractOptions, PackOptions, extract, pack};

/// Pack the standard fixture and open the archive.
fn fixture(work: &Path) -> Archive {
    make_tree(work);
    let arc = work.join("a.kiln");
    pack(
        &arc,
        &[work.join("proj")],
        &PackOptions {
            threads: 2,
            ..PackOptions::default()
        },
    )
    .unwrap();
    Archive::open(&arc).unwrap()
}

fn entries_in(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, Iterator::count)
}

#[test]
fn a_link_planted_where_a_file_goes_is_replaced_never_written_through() {
    let work = tempfile::tempdir().unwrap();
    let archive = fixture(work.path());
    let out = work.path().join("out");
    let victim = work.path().join("victim.txt");
    fs::write(&victim, b"precious").unwrap();
    fs::create_dir_all(out.join("proj")).unwrap();
    symlink(&victim, out.join("proj/readme.txt")).unwrap();

    // Without --force the existing name is simply refused.
    let err = extract(&archive, &ExtractOptions::new(&out)).unwrap_err();
    assert!(matches!(err, Error::Exists(_)), "{err}");
    assert_eq!(fs::read(&victim).unwrap(), b"precious");

    // With it, the link itself is replaced by the real file; the victim is untouched.
    let out2 = work.path().join("out2");
    fs::create_dir_all(out2.join("proj")).unwrap();
    symlink(&victim, out2.join("proj/readme.txt")).unwrap();
    extract(
        &archive,
        &ExtractOptions {
            overwrite: true,
            ..ExtractOptions::new(&out2)
        },
    )
    .unwrap();
    assert_eq!(fs::read(&victim).unwrap(), b"precious");
    let meta = fs::symlink_metadata(out2.join("proj/readme.txt")).unwrap();
    assert!(meta.is_file(), "the link was replaced by a regular file");
    assert_eq!(meta.len(), 3_000);
}

#[test]
fn a_link_planted_in_place_of_a_directory_is_refused_at_every_depth() {
    let work = tempfile::tempdir().unwrap();
    let archive = fixture(work.path());
    let elsewhere = work.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();

    // `proj` itself, and `proj/src` two levels down.
    for (n, planted) in ["proj", "proj/src"].into_iter().enumerate() {
        let out = work.path().join(format!("out{n}"));
        fs::create_dir_all(out.join("proj")).unwrap();
        let at = out.join(planted);
        if planted == "proj" {
            fs::remove_dir(&at).unwrap();
        }
        symlink(&elsewhere, &at).unwrap();
        let err = extract(&archive, &ExtractOptions::new(&out)).unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{planted}: {err}");
        assert_eq!(entries_in(&elsewhere), 0, "{planted}: nothing escaped");
    }
}

#[test]
fn a_regular_file_where_a_directory_is_needed_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let archive = fixture(work.path());
    let out = work.path().join("out");
    fs::create_dir_all(out.join("proj")).unwrap();
    fs::write(out.join("proj/src"), b"in the way").unwrap();
    let err = extract(&archive, &ExtractOptions::new(&out)).unwrap_err();
    assert!(
        matches!(err, Error::Invalid(ref m) if m.contains("not a directory")),
        "{err}"
    );
}

#[test]
fn the_destination_itself_may_be_a_link_to_a_directory() {
    let work = tempfile::tempdir().unwrap();
    let archive = fixture(work.path());
    let real = work.path().join("real");
    let via = work.path().join("via");
    fs::create_dir_all(&real).unwrap();
    symlink(&real, &via).unwrap();
    extract(&archive, &ExtractOptions::new(&via)).unwrap();
    assert!(real.join("proj/src/main.rs").is_file());
}

#[test]
fn a_destination_that_is_a_file_is_an_error() {
    let work = tempfile::tempdir().unwrap();
    let archive = fixture(work.path());
    let out = work.path().join("out");
    fs::write(&out, b"not a directory").unwrap();
    assert!(extract(&archive, &ExtractOptions::new(&out)).is_err());
    assert_eq!(fs::read(&out).unwrap(), b"not a directory");
}

/// The race this module exists for: while extraction runs, another process moves a directory the
/// extractor is writing into out of the way and puts a link to somewhere else in its place. With
/// path-based extraction a later write by name would follow the link; with descriptor-relative
/// extraction it cannot. Extraction may succeed or fail here; nothing may reach `elsewhere`.
#[test]
fn a_directory_swapped_for_a_link_during_extraction_redirects_nothing() {
    let work = tempfile::tempdir().unwrap();
    let archive = fixture(work.path());
    let elsewhere = work.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();

    for round in 0..12 {
        let out = work.path().join(format!("race{round}"));
        fs::create_dir_all(out.join("proj")).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let attacker = {
            let (stop, out, elsewhere) = (stop.clone(), out.clone(), elsewhere.clone());
            thread::spawn(move || {
                let victims: [PathBuf; 3] = [
                    out.join("proj/src"),
                    out.join("proj/data"),
                    out.join("proj/\u{fc}n\u{ef} c\u{f6}d\u{e9}"),
                ];
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    for v in &victims {
                        n += 1;
                        let aside = v.with_extension(format!("moved{n}"));
                        let _ = fs::rename(v, &aside); // move the real directory away...
                        let _ = symlink(&elsewhere, v); // ...and plant a link in its place
                        let _ = fs::remove_file(v); // then take the link away again
                    }
                }
            })
        };

        let _ = extract(
            &archive,
            &ExtractOptions {
                threads: 4,
                ..ExtractOptions::new(&out)
            },
        );
        stop.store(true, Ordering::Relaxed);
        attacker.join().unwrap();

        assert_eq!(
            entries_in(&elsewhere),
            0,
            "round {round}: something was written through a planted link"
        );
    }
}
