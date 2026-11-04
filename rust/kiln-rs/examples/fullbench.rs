// cargo run --release --example fullbench -- <file> <jobs> <segment_mb> [limit_mb]
use kiln_rs::container::Archive;
use kiln_rs::packer::{pack_archive, PackFile, PackOpts};
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let jobs: usize = a[2].parse().unwrap();
    let seg: usize = a[3].parse::<usize>().unwrap() << 20;
    let mut size = std::fs::metadata(path).unwrap().len();
    let mut full = path.clone();
    if let Some(l) = a.get(4) {
        let lim = l.parse::<u64>().unwrap() << 20;
        if lim < size {
            let d = std::fs::read(path).unwrap();
            full = format!("{}.lim", path);
            std::fs::write(&full, &d[..lim as usize]).unwrap();
            size = lim;
        }
    }
    let f = PackFile { full: full.clone(), rel: "data".into(), size, mtime: 0, mode: 0o644, filt: 0, pre: false };
    let dest = format!("{}.kln", full);
    let _ = std::fs::remove_file(&dest);
    let opts = PackOpts { store: false, password: None, segment: seg, jobs };
    let t = Instant::now();
    pack_archive(&dest, &[f], &opts, None).unwrap();
    let ct = t.elapsed().as_secs_f64();
    let asz = std::fs::metadata(&dest).unwrap().len();
    let t = Instant::now();
    let mut ar = Archive::open(std::fs::File::open(&dest).unwrap(), None).unwrap();
    ar.test().unwrap();
    let dt = t.elapsed().as_secs_f64();
    println!("{:9} bytes {:6.3}%  pack {:6.2}s ({:5.2} MB/s)  unpack/test {:5.2}s ({:6.1} MB/s)  jobs {} seg {}MB",
        asz, 100.0 * asz as f64 / size as f64, ct, size as f64 / 1048576.0 / ct, dt, size as f64 / 1048576.0 / dt, jobs, seg >> 20);
    if std::env::var("KEEP").is_err() { let _ = std::fs::remove_file(&dest); }
    if full != *path { let _ = std::fs::remove_file(&full); }
}
