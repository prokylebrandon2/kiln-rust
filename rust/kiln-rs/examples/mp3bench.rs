//! cargo run --release --example mp3bench -- <file.blob> <file.mp3>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let blob = std::fs::read(&a[1]).unwrap();
    let orig = std::fs::read(&a[2]).unwrap();
    let t = std::time::Instant::now();
    let out = kiln_rs::mp3::mp3_unpack(&blob, orig.len()).unwrap();
    println!("unpack Rust {:.2}s identical={}", t.elapsed().as_secs_f64(), out == orig);
}
