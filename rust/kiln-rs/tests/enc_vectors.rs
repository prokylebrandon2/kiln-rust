// cargo test --release --test enc_vectors   (from kiln-rs/; vectors: tools/make_enc_vectors.py)
// The Rust range encoder must give exactly the payload the real C kz_rc_enc gives.
use std::fs;

#[test]
fn encoder_matches_c() {
    let dir = "../tests/encvec";
    let meta: serde_json::Value = match fs::read_to_string(format!("{dir}/meta.json")) {
        Ok(s) => serde_json::from_str(&s).unwrap(),
        Err(_) => {
            eprintln!("no tests/encvec: run tools/make_enc_vectors.py first");
            return;
        }
    };
    let mut n = 0;
    for v in meta.as_array().unwrap() {
        let tag = v["tag"].as_str().unwrap();
        let buf = fs::read(format!("{dir}/{}.buf", v["name"].as_str().unwrap())).unwrap();
        let raw_tok = fs::read(format!("{dir}/{tag}.tok")).unwrap();
        let tok: Vec<u64> = raw_tok.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let pos = v["pos"].as_u64().unwrap() as usize;
        let mode = v["mode"].as_u64().unwrap() as u32;
        let cap = tok.len() * 16 + 64;
        let got = kiln_rs::rc::encode_block(&tok, &buf, pos, mode, cap);
        if v["ok"].as_bool().unwrap() {
            let want = fs::read(format!("{dir}/{tag}.pay")).unwrap();
            let (raw, pay) = got.unwrap_or_else(|| panic!("{tag}: Rust refused, C encoded"));
            assert_eq!(raw as u64, v["raw"].as_u64().unwrap(), "{tag}: raw length");
            assert!(pay == want, "{tag}: payload differs ({} vs {} bytes)", pay.len(), want.len());
        } else {
            assert!(got.is_none(), "{tag}: C refused, Rust encoded");
        }
        n += 1;
    }
    assert!(n > 100, "too few vectors ({n})");
    eprintln!("{n} encoder vectors identical to C");
}
