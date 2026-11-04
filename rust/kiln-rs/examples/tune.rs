// cargo run --release --example tune -- <file> <limit_mb> <chain> <nice> <lazy> <accel>
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut buf = std::fs::read(&a[1]).unwrap();
    let lim: usize = a[2].parse::<usize>().unwrap() << 20;
    buf.truncate(lim);
    let p: Vec<i32> = a[3..7].iter().map(|s| s.parse().unwrap()).collect();
    let t = Instant::now();
    let b = kiln_rs::pack::build_blocks(&buf, 0, (p[0], p[1], p[2], p[3])).unwrap();
    let c: usize = b.iter().map(|x| x.comp_len()).sum();
    println!("chain {:4} nice {:3} lazy {} accel {:3}: {:9} bytes {:6.3}%  {:6.2}s  {:5.2} MB/s", p[0], p[1], p[2], p[3], c, 100.0 * c as f64 / buf.len() as f64, t.elapsed().as_secs_f64(), buf.len() as f64 / 1048576.0 / t.elapsed().as_secs_f64());
}
