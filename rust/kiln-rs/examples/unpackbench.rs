// cargo run --release --example unpackbench -- <archive.kln> [repeats]
use kiln_rs::container::Archive;
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(2).map_or(1, |s| s.parse().unwrap());
    let mut best = f64::MAX;
    let mut raw = 0u64;
    for _ in 0..n {
        let t = Instant::now();
        let mut ar = Archive::open(std::fs::File::open(&a[1]).unwrap(), None).unwrap();
        raw = ar.table.entries.iter().map(|e| e.size).sum();
        ar.test().unwrap();
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!("{:.3}s  {:.1} MB/s ({} bytes)", best, raw as f64 / 1048576.0 / best, raw);
}
