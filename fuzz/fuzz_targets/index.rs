//! `Index::decode` on arbitrary index bytes, bypassing the record layer (and its CRC).
//!
//! Input layout: `[block size selector: 1 byte][data_end: 8 bytes LE][raw index bytes ...]`.
//! If the first byte has its top bit set the rest is instead a list of `(position: u16 LE,
//! xor: u8)` edits applied to a valid, in-process index, which reaches the semantic checks
//! (names, sizes, chunk table, extensions) far more often than random bytes do.
//!
//! The property asserted: whatever decodes must encode to bytes that decode to the same index.
#![no_main]

use kiln::classify::Class;
use kiln::codec::Method;
use kiln::format::index::{ChunkInfo, Entry, EntryKind, Index};
use libfuzzer_sys::fuzz_target;

fn pristine() -> (Vec<u8>, u64, u64) {
    let file = |path: &str, size: u64| Entry {
        path: path.into(),
        kind: EntryKind::File,
        class: Class::Text,
        size,
        mtime_secs: 1_700_000_000,
        mtime_nanos: 5,
        mode: 0o644,
        hash: [9; 32],
    };
    let dir = Entry {
        path: "d".into(),
        kind: EntryKind::Dir,
        class: Class::Unknown,
        size: 0,
        mtime_secs: -5,
        mtime_nanos: 0,
        mode: 0o755,
        hash: [0; 32],
    };
    let chunk = |raw_len, comp_len, method| ChunkInfo {
        raw_len,
        comp_len,
        method,
    };
    let index = Index::build(
        vec![dir, file("d/a.txt", 5000), file("z", 3000)],
        vec![
            chunk(4096, 1000, Method::Zstd),
            chunk(904, 904, Method::Store),
            chunk(3000, 700, Method::Lz4),
        ],
        vec![],
    )
    .expect("the pristine index is valid");
    (index.encode(), 4096, index.data_end)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }
    let (raw, block_size, data_end) = if data[0] & 0x80 != 0 {
        let (mut raw, block_size, data_end) = pristine();
        for edit in data[1..].chunks_exact(3) {
            let pos = usize::from(u16::from_le_bytes([edit[0], edit[1]]));
            if !raw.is_empty() {
                let at = pos % raw.len();
                raw[at] ^= edit[2];
            }
        }
        (raw, block_size, data_end)
    } else {
        let block_size = 1u64 << (12 + (data[0] % 16));
        let data_end = u64::from_le_bytes(data[1..9].try_into().unwrap());
        (data[9..].to_vec(), block_size, data_end)
    };

    if let Ok(index) = Index::decode(&raw, block_size, data_end) {
        let again = index.encode();
        let back = Index::decode(&again, block_size, index.data_end)
            .expect("an index that decoded must re-encode to something that decodes");
        assert_eq!(back, index, "decode(encode(x)) != x");
    }
});
