//! MP3 unpack vs the real engine. Corpus: `python3 tools/make_mp3_corpus.py <kiln_v14.py> <dir>` then
//! `python3 tools/make_mp3_fuzz.py <kiln_v14.py> <dir>`, run with KILN_MP3_CORPUS=<dir> (default ../tests/mp3corpus).
//! Without a corpus the tests do nothing (they print a note).
use kiln_rs::mp3::mp3_unpack;
use std::fs;
use std::path::PathBuf;

fn dir() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var("KILN_MP3_CORPUS").unwrap_or_else(|_| "../tests/mp3corpus".to_string()));
    if d.is_dir() {
        Some(d)
    } else {
        eprintln!("no MP3 corpus at {d:?}: skipped");
        None
    }
}

#[test]
fn unpack_matches_original() {
    let Some(d) = dir() else { return };
    let mut n = 0;
    for e in fs::read_dir(&d).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) != Some("blob") {
            continue;
        }
        let orig = fs::read(p.with_extension("mp3")).unwrap();
        let blob = fs::read(&p).unwrap();
        let got = mp3_unpack(&blob, orig.len()).unwrap_or_else(|c| panic!("{p:?}: error {c}"));
        assert!(got == orig, "{p:?}: output differs from the original");
        n += 1;
    }
    assert!(n > 0, "empty corpus");
    eprintln!("{n} MP3 files unpacked byte-exact");
}

#[test]
fn wrong_size_and_header() {
    let Some(d) = dir() else { return };
    let p = d.join("music_cbr128.blob");
    if !p.exists() {
        return;
    }
    let blob = fs::read(&p).unwrap();
    let n = fs::read(d.join("music_cbr128.mp3")).unwrap().len();
    assert_eq!(mp3_unpack(&blob, n + 1), Err(-2));
    assert_eq!(mp3_unpack(&blob[..8], n), Err(-1));
    let mut b = blob.clone();
    b[0] = 4;
    assert_eq!(mp3_unpack(&b, n), Err(-1));
}

/// corrupted blobs: accepted/refused exactly like the real engine, and the same bytes when accepted
#[test]
fn fuzz_matches_real_engine() {
    let Some(d) = dir() else { return };
    let fz = d.join("fuzz");
    if !fz.is_dir() {
        eprintln!("no fuzz cases: skipped");
        return;
    }
    let (mut acc, mut refd) = (0, 0);
    for e in fs::read_dir(&fz).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) != Some("blob") {
            continue;
        }
        let blob = fs::read(&p).unwrap();
        let exp = fs::read(p.with_extension("exp")).unwrap();
        let n = u64::from_le_bytes(exp[..8].try_into().unwrap()) as usize;
        let r = mp3_unpack(&blob, n);
        match exp[8] {
            0 => {
                let got = r.unwrap_or_else(|c| panic!("{p:?}: real engine accepts, Rust says {c}"));
                assert!(got == exp[9..], "{p:?}: accepted by both but bytes differ");
                acc += 1;
            }
            _ => {
                assert!(r.is_err(), "{p:?}: real engine refuses, Rust accepts");
                refd += 1;
            }
        }
    }
    eprintln!("fuzz: {acc} accepted, {refd} refused: all identical to the real engine");
}

/// CP11c: the Rust packer must produce exactly the bytes the real C `kz_mp3_pack` made (NAME.blob)
#[test]
fn pack_matches_real_engine() {
    let Some(d) = dir() else { return };
    let mut n = 0;
    for e in fs::read_dir(&d).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) != Some("blob") {
            continue;
        }
        let orig = fs::read(p.with_extension("mp3")).unwrap();
        let blob = fs::read(&p).unwrap();
        let cap = orig.len() + orig.len() / 8 + 4096;
        let got = kiln_rs::mp3enc::mp3c_compress(&orig, cap).unwrap_or_else(|| panic!("{p:?}: Rust refused to pack"));
        assert!(got == blob, "{p:?}: packed bytes differ from the real engine ({} vs {} bytes)", got.len(), blob.len());
        // and the verified wrapper accepts it when it is small enough
        if let Some(b2) = kiln_rs::mp3enc::mp3_pack(&orig) {
            assert!(b2 == blob);
        }
        n += 1;
    }
    assert!(n > 0, "empty corpus");
    eprintln!("{n} MP3 files packed byte-identical to the real engine");
}
