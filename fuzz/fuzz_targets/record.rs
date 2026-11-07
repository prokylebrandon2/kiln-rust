//! `read_record` and `decode_payload`: one chunk or index record from arbitrary bytes, including
//! whatever the Zstandard, LZ4 and LZMA2 decoders make of a payload whose CRC happens to match.
//!
//! Input layout: `[size cap selector: 1 byte][bytes_available: u32 LE][record bytes ...]`.
//! The decoded-size cap is at most 16 MiB so a hostile header cannot make the fuzzer allocate
//! gigabytes.
#![no_main]

use std::io::Cursor;

use kiln::format::record::{decode_payload, read_record};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 5 {
        return;
    }
    let max_raw = 1u32 << (12 + (data[0] % 13));
    let available = u64::from(u32::from_le_bytes(data[1..5].try_into().unwrap()));
    let mut cursor = Cursor::new(&data[5..]);
    if let Ok((header, payload)) = read_record(&mut cursor, max_raw, available) {
        if let Ok(decoded) = decode_payload(&header, payload) {
            assert_eq!(decoded.len(), header.raw_len as usize);
        }
    }
});
