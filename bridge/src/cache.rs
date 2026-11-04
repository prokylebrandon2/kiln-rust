//! The preview cache: size, trim and clear. (Extraction into it comes with the archive functions.)

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Bytes of extracted files kept between uses.
const CACHE_LIMIT: u64 = 1 << 30;
/// And nothing older than a week.
const CACHE_MAX_AGE: u64 = 7 * 86_400;

/// Total size of the files below `path` (symbolic links are not followed).
pub fn tree_size(path: &Path) -> u64 {
    let md = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if !md.is_dir() {
        return md.len();
    }
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for ent in rd.flatten() {
            total += tree_size(&ent.path());
        }
    }
    total
}

/// Remove a file, a link, or a whole folder; failures are ignored.
pub fn rm(path: &Path) {
    let is_dir = match fs::symlink_metadata(path) {
        Ok(m) => m.is_dir(),
        Err(_) => return,
    };
    if is_dir {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

fn mtime_secs(md: &fs::Metadata) -> u64 {
    match md.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

/// Drop unfinished copies, entries older than a week, then the least recently used ones until the
/// cache fits `limit`. `keep` (a cache key) is never dropped. Returns the bytes left.
pub fn trim(cache_dir: &str, limit: Option<u64>, keep: &str) -> u64 {
    let limit = limit.unwrap_or(CACHE_LIMIT);
    let dir = Path::new(cache_dir);
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut rows: Vec<(u64, String, u64)> = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        let p = ent.path();
        if name.ends_with(".part") {
            rm(&p); // an interrupted extraction
            continue;
        }
        let md = match fs::metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = if md.is_dir() { tree_size(&p) } else { md.len() };
        rows.push((mtime_secs(&md), name, size));
    }
    rows.sort();
    let mut kept: Vec<(u64, String, u64)> = Vec::new();
    for r in rows {
        if r.1 != keep && now.saturating_sub(r.0) > CACHE_MAX_AGE {
            rm(&dir.join(&r.1));
        } else {
            kept.push(r);
        }
    }
    let mut total: u64 = kept.iter().map(|r| r.2).sum();
    for r in kept.iter() {
        if total <= limit {
            break;
        }
        if r.1 == keep {
            continue;
        }
        rm(&dir.join(&r.1));
        total = total.saturating_sub(r.2);
    }
    total
}

/// `{ok, bytes}`: how much the cache folder holds.
pub fn size(cache_dir: &str) -> Value {
    let p = Path::new(cache_dir);
    let bytes = if p.is_dir() { tree_size(p) } else { 0 };
    json!({ "ok": true, "bytes": bytes })
}

/// Empty the cache folder.
pub fn clear(cache_dir: &str) -> Value {
    if let Ok(rd) = fs::read_dir(cache_dir) {
        for ent in rd.flatten() {
            rm(&ent.path());
        }
    }
    json!({ "ok": true })
}
