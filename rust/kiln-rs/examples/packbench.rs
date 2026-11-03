// cargo run --release --example packbench -- <file> [start]
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let buf = std::fs::read(&a[1]).unwrap();
    let start: usize = a.get(2).map_or(0, |s| s.parse().unwrap());
    let t = Instant::now();
    let b = kiln_rs::pack::build_blocks(&buf, start, kiln_rs::pack::PARSE).unwrap();
    let c: usize = b.iter().map(|x| x.comp_len()).sum();
    println!("{} -> {} bytes in {:.2}s ({} blocks)", buf.len() - start, c, t.elapsed().as_secs_f64(), b.len());
}
