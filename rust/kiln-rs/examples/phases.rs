use std::time::Instant;
use kiln_rs::{lz, pack, rc, rnd};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let buf = std::fs::read(&a[1]).unwrap();
    let n = buf.len();
    let mut p = buf.clone(); p.resize(n + 16, 0);
    let t = Instant::now();
    let r: Vec<u8> = rnd::random_chunks(&buf).into_iter().map(|x| (x == 1) as u8).collect();
    println!("rnd   {:.3}s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let o = lz::lz77(&p, n as i64, 0, &r, 384, 258, 2, 99, 0).unwrap();
    println!("lz77  {:.3}s ({} tokens)", t.elapsed().as_secs_f64(), o.tok.len());
    let t = Instant::now();
    for m in [0u32, 1, 2, 4, 5, 6] {
        let t1 = Instant::now();
        let _ = rc::encode_block(&o.tok, &buf, 0, m, o.tok.len() * 16 + 64);
        println!("  enc mode {m}: {:.3}s", t1.elapsed().as_secs_f64());
    }
    println!("enc all {:.3}s", t.elapsed().as_secs_f64());
    let _ = pack::PARSE;
}
