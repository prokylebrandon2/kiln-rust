// cargo test --test rc_corpus   (run from kiln-rs/; corpus is ../tests/corpus)
use std::fs;

#[test]
fn block_vectors_match_engine() {
    let dir = "../tests/corpus";
    let exp: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(format!("{dir}/expected.json")).unwrap()).unwrap();
    let blocks = exp["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty());
    for v in blocks {
        let name = v["name"].as_str().unwrap();
        let payload = fs::read(format!("{dir}/blocks/{name}.payload")).unwrap();
        let mut out = fs::read(format!("{dir}/blocks/{name}.hist")).unwrap();
        let n0 = out.len();
        let raw = v["raw_len"].as_u64().unwrap() as usize;
        kiln_rs::rc::decode_block(&payload, raw, &mut out).unwrap_or_else(|e| panic!("{name}: error {e}"));
        assert_eq!(out.len() - n0, raw, "{name}: length");
        assert_eq!(crc32fast::hash(&out[n0..]) as u64, v["crc"].as_u64().unwrap(), "{name}: crc");
    }
}

#[test]
fn bad_flag_is_rejected() {
    let mut out = Vec::new();
    assert_eq!(kiln_rs::rc::decode_block(&[3, 0, 0, 0, 0], 10, &mut out), Err(-1));
    assert!(out.is_empty());
}
