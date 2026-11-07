//! Archives larger than 4 GiB on format v2.
//!
//! Ignored by default: they write a 4.25 GiB archive and read 4.25 GiB several times. Run them on
//! a machine with ~5 GiB of free disk:
//!
//! ```sh
//! cargo test --release --locked --test large -- --ignored --nocapture
//! KILN_LARGE_DIR=/mnt/scratch cargo test --release --test large -- --ignored   # pick the disk
//! ```
//!
//! The source is a *sparse* file (it costs no disk), so the only real disk use is the archive.
//! Two cases cover the two ways "larger than 4 GiB" can go wrong:
//!
//! * `store`: the **archive file** is larger than 4 GiB, so chunk offsets in the file, the footer's
//!   `index_offset` and every seek past 2^32 are exercised.
//! * `lz4`: the **data stream** is larger than 4 GiB but compresses to almost nothing, so stream
//!   offsets, file sizes and chunk start arithmetic past 2^32 are exercised on a small archive.
//!
//! Both then extract a member that lies beyond 4 GiB in the stream, which must decode only the
//! chunks it needs and match byte for byte. The index arithmetic alone is also checked, without
//! any I/O, by a normal unit test (`archives_beyond_4_gib_keep_exact_offsets` in
//! `src/format/index.rs`).

mod common;

use std::fs::{self, File};
use std::path::Path;
use std::sync::Mutex;

use common::{noise, prose};
use kiln::codec::CodecSpec;
use kiln::{Archive, ExtractOptions, PackOptions, extract, pack, verify};

/// 4.25 GiB.
const BIG: u64 = (4 << 30) + (256 << 20);

/// One of these tests at a time: each needs several GiB of disk and bandwidth.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn scratch() -> tempfile::TempDir {
    match std::env::var_os("KILN_LARGE_DIR") {
        Some(dir) => tempfile::tempdir_in(dir).unwrap(),
        None => tempfile::tempdir().unwrap(),
    }
}

/// `src/a_head.bin` (noise), `src/b_big.bin` (4.25 GiB of sparse zeros) and `src/c_tail.txt`
/// (prose), which sorts last and so lies beyond 4 GiB in the stream. Returns the tail's content.
fn make_source(root: &Path) -> Vec<u8> {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a_head.bin"), noise(1 << 20, 7)).unwrap();
    File::create(src.join("b_big.bin"))
        .unwrap()
        .set_len(BIG)
        .unwrap();
    let tail = prose(1 << 20, 9);
    fs::write(src.join("c_tail.txt"), &tail).unwrap();
    tail
}

fn round_trip(codec: &str, archive_file_exceeds_4_gib: bool) {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let work = scratch();
    let tail = make_source(work.path());
    let arc = work.path().join("big.kiln");

    let stats = pack(
        &arc,
        &[work.path().join("src")],
        &PackOptions {
            codec: CodecSpec::parse(codec).unwrap(),
            block_log2: 22,
            ..PackOptions::default()
        },
    )
    .unwrap();
    assert!(stats.raw_bytes > 4 << 30, "{} bytes", stats.raw_bytes);
    assert_eq!(stats.raw_bytes, (1 << 20) + BIG + (1 << 20));

    let archive = Archive::open(&arc).unwrap();
    if archive_file_exceeds_4_gib {
        assert!(archive.file_len() > 4 << 30, "{} bytes", archive.file_len());
    }
    let tail_entry = archive
        .entries()
        .iter()
        .position(|e| e.path == "src/c_tail.txt")
        .expect("the tail file is in the archive");
    assert!(
        archive.index().entry_starts[tail_entry] > u64::from(u32::MAX),
        "the tail must start beyond 4 GiB in the stream"
    );
    assert_eq!(archive.total_raw(), stats.raw_bytes);

    // Every checksum of every chunk, beyond 2^32 included.
    let checked = verify(&archive, 0).unwrap();
    assert_eq!(checked.bytes, stats.raw_bytes);
    assert_eq!(checked.files, 3);

    // Random access past 4 GiB: only the chunks the member touches may be decoded.
    let out = work.path().join("out");
    let ex = extract(
        &archive,
        &ExtractOptions {
            members: vec!["src/c_tail.txt".into()],
            ..ExtractOptions::new(&out)
        },
    )
    .unwrap();
    assert_eq!(ex.files, 1);
    assert!(
        ex.blocks < 4,
        "decoded {} chunks for a 1 MiB member",
        ex.blocks
    );
    assert_eq!(fs::read(out.join("src/c_tail.txt")).unwrap(), tail);
}

#[test]
#[ignore = "writes a 4.25 GiB archive; run with --ignored (see the module docs)"]
fn archive_file_larger_than_4_gib_round_trips_on_format_v2() {
    round_trip("store", true);
}

#[test]
#[ignore = "reads 4.25 GiB several times; run with --ignored (see the module docs)"]
fn data_stream_larger_than_4_gib_round_trips_on_format_v2() {
    round_trip("lz4", false);
}
