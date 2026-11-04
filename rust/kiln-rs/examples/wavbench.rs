// cargo run --release --example wavbench -- <raw 16-bit stereo pcm file> <order> <ms 0/1>
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let buf = std::fs::read(&a[1]).unwrap();
    let order: u32 = a[2].parse().unwrap();
    let ms = a[3] == "1";
    let n = buf.len() / 4;
    let mut out = vec![0u8; n * 4];
    kiln_rs::wav::wav16_enc(&buf, n, 2, order, ms, 0, &mut out);
    for (name, p) in [("ref 384/258/2/99", (384, 258, 2, 99)), ("app 128/96/2/99", (128, 96, 2, 99)), ("app 128/96/2/50", (128, 96, 2, 50))] {
        let t = Instant::now();
        let b = kiln_rs::pack::build_blocks(&out, 0, p).unwrap();
        let c: usize = b.iter().map(|x| x.comp_len()).sum();
        println!("{name}: {} -> {} ({:.2}%) in {:.2}s", out.len(), c, 100.0 * c as f64 / out.len() as f64, t.elapsed().as_secs_f64());
    }
}
