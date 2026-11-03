//! Shared helpers for the integration tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Deterministic pseudo-random bytes (xorshift64*), incompressible for practical purposes.
pub fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u8
        })
        .collect()
}

/// Compressible pseudo-prose.
pub fn prose(len: usize, seed: u64) -> Vec<u8> {
    let rnd = noise(len / 4 + 64, seed);
    let words: Vec<String> = (0..200u32)
        .map(|w| {
            (0..3 + w % 6)
                .map(|k| (b'a' + ((w * 31 + k * 7) % 26) as u8) as char)
                .collect()
        })
        .collect();
    let mut out = Vec::with_capacity(len + 16);
    let mut i = 0;
    while out.len() < len {
        out.extend_from_slice(words[(rnd[i % rnd.len()] as usize + i) % 200].as_bytes());
        out.push(if i % 13 == 0 { b'\n' } else { b' ' });
        i += 1;
    }
    out.truncate(len);
    out
}

/// Build the standard fixture under `root/proj`.
pub fn make_tree(root: &Path) {
    let p = root.join("proj");
    fs::create_dir_all(p.join("src")).unwrap();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::create_dir_all(p.join("empty_dir")).unwrap();
    fs::create_dir_all(p.join("ünï cödé")).unwrap();
    fs::write(p.join("readme.txt"), prose(3_000, 1)).unwrap();
    fs::write(p.join("src/main.rs"), prose(200_000, 2)).unwrap();
    fs::write(p.join("src/lib.rs"), prose(70_000, 3)).unwrap();
    fs::write(p.join("data/noise.bin"), noise(300_000, 4)).unwrap();
    fs::write(p.join("data/big.log"), prose(1_500_000, 5)).unwrap();
    fs::write(p.join("empty.txt"), b"").unwrap();
    fs::write(p.join("one_byte"), b"x").unwrap();
    fs::write(p.join("ünï cödé/файл.txt"), prose(5_000, 6)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(p.join("run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(p.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(p.join("readme.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Node {
    Dir {
        mtime: i64,
    },
    File {
        data: Vec<u8>,
        mtime: i64,
        mode: u32,
    },
}

/// Everything under `root`, keyed by relative path.
pub fn snapshot(root: &Path) -> BTreeMap<String, Node> {
    fn mtime(m: &fs::Metadata) -> i64 {
        let d = m
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        d.as_secs() as i64
    }
    fn mode(m: &fs::Metadata) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            m.permissions().mode() & 0o777
        }
        #[cfg(not(unix))]
        {
            let _ = m;
            0
        }
    }
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Node>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = format!("{prefix}{}", e.file_name().to_str().unwrap());
            let meta = fs::symlink_metadata(e.path()).unwrap();
            if meta.is_dir() {
                walk(&e.path(), &format!("{name}/"), out);
                out.insert(
                    name,
                    Node::Dir {
                        mtime: mtime(&meta),
                    },
                );
            } else {
                let data = fs::read(e.path()).unwrap();
                out.insert(
                    name,
                    Node::File {
                        data,
                        mtime: mtime(&meta),
                        mode: mode(&meta),
                    },
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// Assert two trees are identical in names, contents, permissions and timestamps.
pub fn assert_same_tree(a: &Path, b: &Path) {
    let (sa, sb) = (snapshot(a), snapshot(b));
    let names = |s: &BTreeMap<String, Node>| s.keys().cloned().collect::<Vec<_>>();
    assert_eq!(names(&sa), names(&sb), "different sets of entries");
    for (name, na) in &sa {
        assert!(na == &sb[name], "entry differs: {name}");
    }
}
