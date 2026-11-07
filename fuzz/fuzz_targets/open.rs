//! `Archive::open` on arbitrary bytes, then everything the library does with an archive that
//! opened: queries, `verify`, and `extract` into a scratch directory.
//!
//! Random mutation rarely survives the header, footer and index checksums, so a custom mutator
//! re-seals those three CRCs after every mutation. Chunk-record CRCs are left alone: a mismatch
//! there is a path worth exercising too.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use kiln::{Archive, ExtractOptions, extract, verify};
use libfuzzer_sys::{fuzz_mutator, fuzz_target};

/// A hostile archive may truthfully describe terabytes; do not decode more than this.
const MAX_RAW: u64 = 16 << 20;

fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("kiln-fuzz-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    })
}

fuzz_target!(|data: &[u8]| {
    let dir = scratch();
    let file = dir.join("input.kiln");
    std::fs::write(&file, data).unwrap();

    let Ok(archive) = Archive::open(&file) else {
        return;
    };
    // Cheap queries on an archive that opened must never panic.
    let _ = archive.block_stats();
    let _ = archive.class_totals();
    for i in 0..archive.entries().len().min(2000) {
        let _ = archive.entry_method(i);
    }
    if archive.index().total_raw > MAX_RAW || archive.total_raw() > MAX_RAW {
        return;
    }
    let _ = verify(&archive, 1);
    let out = dir.join("out");
    let _ = std::fs::remove_dir_all(&out);
    let _ = extract(
        &archive,
        &ExtractOptions {
            threads: 1,
            max_bytes: Some(MAX_RAW),
            ..ExtractOptions::new(&out)
        },
    );
    let _ = std::fs::remove_dir_all(&out);
});

fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, _seed: u32| {
    let n = libfuzzer_sys::fuzzer_mutate(data, size, max_size);
    reseal(&mut data[..n]);
    n
});

/// Recompute the header CRC, force the footer magic and CRC, and recompute the index record's
/// payload CRC (when the footer still points at a plausible record).
fn reseal(d: &mut [u8]) {
    let n = d.len();
    if n >= 24 {
        let crc = crc32fast::hash(&d[..20]);
        d[20..24].copy_from_slice(&crc.to_le_bytes());
    }
    if n < 48 {
        return;
    }
    d[n - 4..].copy_from_slice(b"KLNF");
    let crc = crc32fast::hash(&d[n - 24..n - 8]);
    d[n - 8..n - 4].copy_from_slice(&crc.to_le_bytes());

    let offset = u64::from_le_bytes(d[n - 24..n - 16].try_into().unwrap());
    let Ok(offset) = usize::try_from(offset) else {
        return;
    };
    if offset < 24 || offset.saturating_add(13) > n - 24 {
        return;
    }
    let comp = u32::from_le_bytes(d[offset + 5..offset + 9].try_into().unwrap()) as usize;
    let Some(end) = (offset + 13).checked_add(comp) else {
        return;
    };
    if end > n - 24 {
        return;
    }
    let crc = crc32fast::hash(&d[offset + 13..end]);
    d[offset + 9..offset + 13].copy_from_slice(&crc.to_le_bytes());
}
