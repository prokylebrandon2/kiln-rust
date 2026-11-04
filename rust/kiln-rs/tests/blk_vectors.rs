// cargo test --release --test blk_vectors   (vectors: tools/make_lz_vectors.py then tools/make_blk_vectors.py)
// build_blocks in Rust must give exactly the blocks of the Python engine (same types, sizes, payload bytes).
use kiln_rs::pack::{build_blocks, Block};

// The Python engine's parse settings, which the vectors were made with. The app's own pack::PARSE is
// tuned for speed and is deliberately different, so it must not be used here.
const REF_PARSE: (i32, i32, i32, i32) = (384, 258, 2, 99);
use std::fs;

#[test]
fn blocks_match_python() {
    let dir = "../tests/blkvec";
    let meta: serde_json::Value = match fs::read_to_string(format!("{dir}/meta.json")) {
        Ok(s) => serde_json::from_str(&s).unwrap(),
        Err(_) => {
            eprintln!("no tests/blkvec: run tools/make_blk_vectors.py first");
            return;
        }
    };
    let mut n = 0;
    for v in meta.as_array().unwrap() {
        let tag = v["tag"].as_str().unwrap();
        let buf = fs::read(format!("../tests/lzvec/{}.buf", v["name"].as_str().unwrap())).unwrap();
        let start = v["start"].as_u64().unwrap() as usize;
        let raw = fs::read(format!("{dir}/{tag}.blk")).unwrap();
        // decode the expected list
        let mut want: Vec<(u8, u64, Vec<u8>)> = Vec::new();
        let mut p = 0;
        while p < raw.len() {
            let t = raw[p];
            let r = u64::from_le_bytes(raw[p + 1..p + 9].try_into().unwrap());
            let l = u64::from_le_bytes(raw[p + 9..p + 17].try_into().unwrap()) as usize;
            want.push((t, r, raw[p + 17..p + 17 + l].to_vec()));
            p += 17 + l;
        }
        let got = build_blocks(&buf, start, REF_PARSE).unwrap_or_else(|| panic!("{tag}: failed"));
        assert_eq!(got.len(), want.len(), "{tag}: block count");
        let mut pos = start;
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            match g {
                Block::Stored { start: s, len } => {
                    assert_eq!((w.0, w.1), (0, *len as u64), "{tag} block {i}: stored");
                    assert_eq!(*s, pos, "{tag} block {i}: position");
                    pos += len;
                }
                Block::Coded { raw, payload } => {
                    assert_eq!((w.0, w.1), (1, *raw as u64), "{tag} block {i}: coded raw");
                    assert!(payload == &w.2, "{tag} block {i}: payload differs ({} vs {})", payload.len(), w.2.len());
                    pos += raw;
                }
            }
        }
        assert_eq!(pos, buf.len(), "{tag}: blocks do not cover the input");
        n += 1;
    }
    eprintln!("{n} build_blocks cases identical to Python");
}
