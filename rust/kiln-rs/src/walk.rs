//! Input collection and filter choice: ports of `collect`, `bcj_kind` and `pick_bcj` in kiln_v14.py.

use crate::bcj::{bcj_encode, BCJ_ARM64, BCJ_BLOCK, BCJ_X86};
use crate::container::{KilnError, Result};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

pub const BCJ_MIN: u64 = 1 << 14;
pub const WAV_MIN: u64 = 1 << 17;
pub const MP3_MIN: u64 = 16 << 10;
pub const MP3_MAX: u64 = 64 << 20;

const PRECOMPRESSED: [&str; 34] = [
    ".zip", ".gz", ".tgz", ".bz2", ".xz", ".7z", ".rar", ".zst", ".kln", ".apk", ".jar", ".jpg", ".jpeg", ".png", ".gif",
    ".webp", ".heic", ".avif", ".mp3", ".m4a", ".aac", ".ogg", ".opus", ".flac", ".mp4", ".mkv", ".webm", ".avi", ".mov",
    ".m4v", ".docx", ".xlsx", ".pptx", ".epub",
];

#[derive(Debug, Clone)]
pub struct Item {
    pub full: String,
    pub rel: String,
    pub size: u64,
    pub mtime: u64,
    pub mode: u16,
    pub pre: bool,
    pub ext: String,
}

/// Lexical os.path.abspath: absolute, with "." and ".." resolved (no symlink lookups).
pub fn abspath(p: &str) -> PathBuf {
    let mut base = if Path::new(p).is_absolute() { PathBuf::new() } else { std::env::current_dir().unwrap_or_default() };
    base.push(p);
    let mut out = PathBuf::new();
    for c in base.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// os.path.splitext(rel)[1].lower()
fn ext_of(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let trimmed = name.trim_start_matches('.');
    match trimmed.rfind('.') {
        Some(i) => trimmed[i..].to_lowercase(),
        None => String::new(),
    }
}

fn walk_dir(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
    let mut names: Vec<String> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = match e.file_type() {
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            if is_dir {
                dirs.push(name);
            } else {
                names.push(name); // files, symlinks to files, and anything odd (stat decides later)
            }
        }
    }
    names.sort();
    dirs.sort();
    for n in names {
        let full = dir.join(&n);
        let rel = full.strip_prefix(base).unwrap_or(&full).to_string_lossy().into_owned();
        out.push((full.to_string_lossy().into_owned(), rel));
    }
    for d in dirs {
        walk_dir(&dir.join(d), base, out);
    }
}

/// -> (items, warnings). Same order as Python: precompressed last, then by extension, then by path.
pub fn collect(paths: &[String], exclude: &[PathBuf]) -> Result<(Vec<Item>, Vec<String>)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for p in paths {
        let pp = Path::new(p);
        if pp.is_dir() {
            let ap = abspath(p);
            let base = ap.parent().map(|x| x.to_path_buf()).unwrap_or_default();
            walk_dir(&ap, &base, &mut files);
        } else if pp.is_file() {
            let name = pp.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            files.push((p.clone(), name));
        } else {
            return Err(KilnError(format!("no such file or directory: {p}")));
        }
    }
    let mut out: Vec<Item> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut warnings = Vec::new();
    for (full, rel) in files {
        if exclude.iter().any(|x| *x == abspath(&full)) {
            continue;
        }
        let rel = rel.replace('\\', "/");
        if !seen.insert(rel.clone()) {
            return Err(KilnError(format!("two inputs would both be stored as '{rel}'")));
        }
        let md = match std::fs::metadata(&full) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("skipping {full}: {e}"));
                continue;
            }
        };
        if md.is_dir() {
            continue; // a symlink to a folder: os.walk lists it but does not descend
        }
        let ext = ext_of(&rel);
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            (md.permissions().mode() & 0o7777) as u16
        };
        #[cfg(not(unix))]
        let mode = 0o644u16;
        out.push(Item { full, rel, size: md.len(), mtime, mode, pre: PRECOMPRESSED.contains(&ext.as_str()), ext });
    }
    out.sort_by(|a, b| (a.pre, &a.ext, &a.rel).cmp(&(b.pre, &b.ext, &b.rel)));
    Ok((out, warnings))
}

/// x86 / AArch64 executable (ELF or PE) from the first bytes of a file -> filter id or 0.
pub fn bcj_kind(head: &[u8]) -> u16 {
    if head.len() >= 20 && &head[..4] == b"\x7fELF" && head[5] == 1 {
        let m = u16::from_le_bytes([head[18], head[19]]);
        return if m == 3 || m == 62 { BCJ_X86 } else if m == 183 { BCJ_ARM64 } else { 0 };
    }
    if head.len() >= 64 && &head[..2] == b"MZ" {
        let off = u32::from_le_bytes([head[60], head[61], head[62], head[63]]) as usize;
        if off + 6 <= head.len() && &head[off..off + 4] == b"PE\0\0" {
            let m = u16::from_le_bytes([head[off + 4], head[off + 5]]);
            return if m == 0x14c || m == 0x8664 { BCJ_X86 } else if m == 0xAA64 { BCJ_ARM64 } else { 0 };
        }
    }
    0
}

/// Filter id for an executable that the filter measurably helps, else 0 (1 MB sample, deflate level 6).
pub fn pick_bcj(path: &str, size: u64) -> u16 {
    if size < BCJ_MIN {
        return 0;
    }
    let try_it = || -> std::io::Result<(u16, Vec<u8>, u64)> {
        let mut fh = File::open(path)?;
        let mut head = vec![0u8; 4096];
        let n = fh.read(&mut head)?;
        head.truncate(n);
        let fid = bcj_kind(&head);
        if fid == 0 {
            return Ok((0, Vec::new(), 0));
        }
        let off = if size > 4 * BCJ_BLOCK as u64 { (size / 3) & !3 } else { 0 };
        fh.seek(SeekFrom::Start(off))?;
        let mut raw = Vec::new();
        fh.take(BCJ_BLOCK as u64).read_to_end(&mut raw)?;
        Ok((fid, raw, off))
    };
    let (fid, raw, off) = match try_it() {
        Ok(x) => x,
        Err(_) => return 0,
    };
    if fid == 0 {
        return 0;
    }
    let mut enc = raw.clone();
    bcj_encode(fid, &mut enc, off);
    let a = miniz_oxide::deflate::compress_to_vec_zlib(&enc, 6).len() as f64;
    let b = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6).len() as f64;
    if a < b * 0.995 {
        fid
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_rules() {
        assert_eq!(ext_of("a/b.TXT"), ".txt");
        assert_eq!(ext_of("a/.bashrc"), "");
        assert_eq!(ext_of("x.tar.gz"), ".gz");
        assert_eq!(ext_of("noext"), "");
    }

    #[test]
    fn kinds() {
        let mut h = vec![0u8; 64];
        h[..4].copy_from_slice(b"\x7fELF");
        h[5] = 1;
        h[18] = 62;
        assert_eq!(bcj_kind(&h), BCJ_X86);
        h[18] = 183;
        assert_eq!(bcj_kind(&h), BCJ_ARM64);
        h[18] = 40;
        assert_eq!(bcj_kind(&h), 0);
    }

    #[test]
    fn collect_order_and_rel() {
        let d = std::env::temp_dir().join(format!("kiln_walk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj/sub")).unwrap();
        std::fs::write(d.join("proj/b.txt"), b"1").unwrap();
        std::fs::write(d.join("proj/a.zip"), b"2").unwrap();
        std::fs::write(d.join("proj/sub/c.txt"), b"3").unwrap();
        let (items, _) = collect(&[d.join("proj").to_string_lossy().into_owned()], &[]).unwrap();
        let rels: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        assert_eq!(rels, vec!["proj/b.txt", "proj/sub/c.txt", "proj/a.zip"]);
        assert!(collect(&["/no/such/thing".to_string()], &[]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
