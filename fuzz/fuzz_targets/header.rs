//! The 24-byte header and footer decoders. Whatever decodes must re-encode to something that
//! decodes to the same value.
#![no_main]

use kiln::format::{FOOTER_LEN, Footer, HEADER_LEN, Header};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(bytes) = data.get(..HEADER_LEN)
        && let Ok(header) = Header::decode(bytes.try_into().unwrap())
    {
        assert_eq!(Header::decode(&header.encode()).unwrap(), header);
    }
    let tail = data.get(HEADER_LEN..).unwrap_or_default();
    if let Some(bytes) = tail.get(..FOOTER_LEN)
        && let Ok(footer) = Footer::decode(bytes.try_into().unwrap())
    {
        assert_eq!(Footer::decode(&footer.encode()).unwrap(), footer);
    }
});
