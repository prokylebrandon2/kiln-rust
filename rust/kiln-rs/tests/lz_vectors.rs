// cargo test --release --test lz_vectors   (vectors: tools/make_lz_vectors.py)
// The Rust match finder / optimal parser must produce exactly the C token stream and raw runs.
use std::fs;

#[test]
fn lz_matches_c() {
    let dir = "../tests/lzvec";
    let meta: serde_json::Value = match fs::read_to_string(format!("{dir}/meta.json")) {
        Ok(s) => serde_json::from_str(&s).unwrap(),
        Err(_) => {
            eprintln!("no tests/lzvec: run tools/make_lz_vectors.py first");
            return;
        }
    };
    let only = std::env::var("LZ_ONLY").ok();
    let mut n = 0;
    for v in meta.as_array().unwrap() {
        let tag = v["tag"].as_str().unwrap();
        if let Some(o) = &only {
            if !tag.contains(o.as_str()) {
                continue;
            }
        }
        let mut buf = fs::read(format!("{dir}/{}.buf", v["name"].as_str().unwrap())).unwrap();
        let nlen = buf.len() as i64;
        buf.resize(buf.len() + 16, 0);
        let rnd = fs::read(format!("{dir}/{}.rnd", v["name"].as_str().unwrap())).unwrap();
        let want_tok: Vec<u64> = fs::read(format!("{dir}/{tag}.tok")).unwrap().chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let want_runs: Vec<i64> = fs::read(format!("{dir}/{tag}.runs")).unwrap().chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect();
        let g = |k: &str| v[k].as_i64().unwrap() as i32;
        let out = kiln_rs::lz::lz77(&buf, nlen, v["start"].as_i64().unwrap(), &rnd, g("chain"), g("nice"), g("lazy"), g("accel"), g("lp"))
            .unwrap_or_else(|| panic!("{tag}: Rust failed"));
        if out.tok != want_tok {
            let at = out.tok.iter().zip(&want_tok).position(|(a, b)| a != b);
            panic!("{tag}: tokens differ (rust {} vs c {}), first difference at {:?}", out.tok.len(), want_tok.len(), at);
        }
        let got_runs: Vec<i64> = out.runs.iter().flat_map(|r| [r.0, r.1]).collect();
        assert_eq!(got_runs, want_runs, "{tag}: runs");
        n += 1;
    }
    eprintln!("{n} lz cases identical to C");
}

#[test]
fn chunk_classes_match_c() {
    let dir = "../tests/lzvec";
    let meta: serde_json::Value = match fs::read_to_string(format!("{dir}/meta.json")) {
        Ok(s) => serde_json::from_str(&s).unwrap(),
        Err(_) => return,
    };
    let mut names: Vec<String> = meta.as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    names.dedup();
    let mut seen = [0usize; 4];
    for name in &names {
        let buf = fs::read(format!("{dir}/{name}.buf")).unwrap();
        let want = fs::read(format!("{dir}/{name}.rnd")).unwrap();
        let got = kiln_rs::rnd::random_chunks(&buf);
        assert_eq!(got, want, "{name}: chunk classes");
        for c in got {
            seen[c as usize] += 1;
        }
    }
    eprintln!("chunk classes identical to C on {} buffers; class counts {:?}", names.len(), seen);
    assert!(seen.iter().all(|&c| c > 0), "a class is never exercised: {:?}", seen);
}
