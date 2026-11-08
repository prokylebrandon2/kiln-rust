//! Folder browsing: `scan` and `search`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use crate::io;
use crate::rx::Regex;

/// Name of the index file inside a random-name "one archive per file" set.
pub const INDEX_NAME: &str = "_index.kiln";
/// Incompatible-feature bit that marks an encrypted archive (header bytes 12..16, little endian).
pub const ENC_FLAG: u32 = 1;
/// Byte of the header that holds the low bits of the incompatible-feature flags.
pub const FLAG_BYTE: usize = 12;
/// How many leading bytes are looked at to recognise an archive.
pub const SNIFF_LEN: usize = 16;
/// Nothing smaller than a header plus a footer can be an archive.
pub const MIN_ARCHIVE: u64 = (kiln::format::HEADER_LEN + kiln::format::FOOTER_LEN) as u64;

const SEARCH_LIMIT: usize = 1000;

/// `Some(encrypted)` when the file starts like a Kiln archive.
fn sniff(path: &Path, size: u64) -> Option<bool> {
    if size < MIN_ARCHIVE {
        return None;
    }
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; SNIFF_LEN];
    let mut got = 0usize;
    while got < SNIFF_LEN {
        match f.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => return None,
        }
    }
    if got <= FLAG_BYTE || head[..8] != kiln::format::MAGIC {
        return None;
    }
    Some(head[FLAG_BYTE] & (ENC_FLAG as u8) != 0)
}

fn mtime(md: &fs::Metadata) -> i64 {
    match md.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

fn dir_row(name: &str, path: &Path, md: &fs::Metadata) -> Value {
    let is_set = path.join(INDEX_NAME).is_file();
    let count = match fs::read_dir(path) {
        Ok(rd) => rd.count(),
        Err(_) => 0,
    };
    let kind = if is_set { "set" } else { "dir" };
    json!({
        "n": name,
        "k": kind,
        "s": 0,
        "m": mtime(md),
        "c": count,
    })
}

/// A file row, or None when it is an ordinary file and only archives are wanted.
fn file_row(name: &str, path: &Path, md: &fs::Metadata, show_all: bool) -> Option<Value> {
    let (kind, enc) = match sniff(path, md.len()) {
        Some(e) => ("archive", e),
        None => ("file", false),
    };
    if kind == "file" && !show_all {
        return None;
    }
    Some(json!({
        "n": name,
        "k": kind,
        "s": md.len(),
        "m": mtime(md),
        "e": enc,
    }))
}

/// A folder smaller than this is read on the calling thread: starting threads would cost more than
/// it saves.
const PARALLEL_MIN: usize = 24;
/// The most threads a folder is read with. The work is mostly waiting for the file system (every
/// subfolder is opened to count what is in it, every file that could be an archive is opened to
/// read its first bytes), and on a phone's shared storage each of those calls is slow, so a few
/// threads at a time make a big folder open several times faster.
const MAX_WORKERS: usize = 8;

/// A row ready to sort: folders (0) before files (1), then the lower-case name.
type ScanRow = (u8, String, Value);

/// Look at one entry of a folder: its row, or None when it is not listed.
fn scan_entry(name: &str, p: &Path, show_all: bool) -> Option<ScanRow> {
    let md = fs::metadata(p).ok()?;
    if md.is_dir() {
        Some((0, name.to_lowercase(), dir_row(name, p, &md)))
    } else if md.is_file() {
        let row = file_row(name, p, &md, show_all)?;
        Some((1, name.to_lowercase(), row))
    } else {
        None
    }
}

/// `scan_entry` for every entry, in the order they were given (the sort after it depends on it
/// only for ties, and those keep the order of the folder as before).
fn scan_entries(entries: &[(String, PathBuf)], show_all: bool) -> Vec<Option<ScanRow>> {
    if entries.len() < PARALLEL_MIN {
        return entries
            .iter()
            .map(|(name, p)| scan_entry(name, p, show_all))
            .collect();
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(4, MAX_WORKERS);
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<ScanRow>> = entries.iter().map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine: Vec<(usize, Option<ScanRow>)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((name, p)) = entries.get(i) else {
                            break;
                        };
                        mine.push((i, scan_entry(name, p, show_all)));
                    }
                    mine
                })
            })
            .collect();
        for handle in handles {
            if let Ok(mine) = handle.join() {
                for (i, row) in mine {
                    out[i] = row;
                }
            }
        }
    });
    out
}

/// One folder for the file manager: folders, archives, sets and (when `show_all`) every file.
pub fn scan(path: &str, show_all: bool, show_hidden: bool) -> Value {
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => {
            return json!({ "ok": false, "error": format!("can't open {path}: {}", io::os_msg(&e)) });
        }
    };
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for ent in rd {
        let ent = match ent {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        entries.push((name, ent.path()));
    }
    let mut rows: Vec<ScanRow> = scan_entries(&entries, show_all)
        .into_iter()
        .flatten()
        .collect();
    rows.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let items: Vec<Value> = rows.into_iter().map(|r| r.2).collect();
    let is_set = Path::new(path).join(INDEX_NAME).is_file();
    json!({ "ok": true, "path": path, "items": items, "is_set": is_set })
}

// ---------------------------------------------------------------- search

enum Matcher {
    Words(Vec<String>),
    Glob(Vec<char>),
    Re(Regex),
}

impl Matcher {
    /// `low` is the lowercased name.
    fn is_match(&self, low: &str) -> bool {
        match self {
            Matcher::Words(w) => w.iter().all(|t| low.contains(t.as_str())),
            Matcher::Glob(p) => {
                let text: Vec<char> = low.chars().collect();
                glob_match(p, &text)
            }
            Matcher::Re(r) => r.is_match(low),
        }
    }
}

/// `*` (any run, even empty) and `?` (any one character); the whole name must match.
fn glob_match(pat: &[char], text: &[char]) -> bool {
    let mut p = 0usize;
    let mut t = 0usize;
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while t < text.len() {
        if p < pat.len() && pat[p] == '*' {
            star = Some(p);
            p += 1;
            mark = t;
        } else if p < pat.len() && (pat[p] == '?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

/// Three forms: `re:<regex>` (found anywhere in the name), a query with `*` or `?` (wildcards,
/// must match the whole name), or plain words (the name must contain every word).
/// None for an empty or invalid query.
fn matcher(query: &str) -> Option<Matcher> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if q.to_lowercase().starts_with("re:") {
        let body: String = q.chars().skip(3).collect();
        return Regex::new(body.trim()).map(Matcher::Re);
    }
    if q.contains('*') || q.contains('?') {
        return Some(Matcher::Glob(q.to_lowercase().chars().collect()));
    }
    let words: Vec<String> = q
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.to_string())
        .collect();
    Some(Matcher::Words(words))
}

/// Every file and folder below `path` whose name matches `query`; names come back relative to
/// `path`. Stops (with `cancelled`) when told to.
pub fn search(path: &str, query: &str, show_all: bool, show_hidden: bool) -> Value {
    let m = match matcher(query) {
        Some(m) => m,
        None => return json!({ "ok": true, "path": path, "items": [], "truncated": false }),
    };
    let mut items: Vec<Value> = Vec::new();
    let mut truncated = false;
    let mut stack: Vec<String> = vec![String::new()];
    let base = Path::new(path);
    while let Some(rel) = stack.pop() {
        if io::cancelled() {
            return json!({ "ok": false, "cancelled": true });
        }
        let here = if rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(&rel)
        };
        let rd = match fs::read_dir(&here) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in rd {
            let ent = match ent {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = ent.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !show_hidden {
                continue;
            }
            let is_dir = match ent.file_type() {
                Ok(t) => t.is_dir(),
                Err(_) => continue,
            };
            let hit = m.is_match(&name.to_lowercase());
            let relname = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let p = ent.path();
            if is_dir {
                if hit {
                    if let Ok(md) = fs::metadata(&p) {
                        items.push(dir_row(&relname, &p, &md));
                    }
                }
                stack.push(relname);
            } else if hit {
                if let Ok(md) = fs::metadata(&p) {
                    if md.is_file() {
                        if let Some(v) = file_row(&relname, &p, &md, show_all) {
                            items.push(v);
                        }
                    }
                }
            }
            if items.len() >= SEARCH_LIMIT {
                truncated = true;
                break;
            }
        }
        if truncated {
            break;
        }
    }
    if io::cancelled() {
        return json!({ "ok": false, "cancelled": true });
    }
    json!({ "ok": true, "path": path, "items": items, "truncated": truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A big folder (read by several threads) lists exactly what the one-thread path lists, in
    /// the same order.
    #[test]
    fn big_folder_scans_like_a_small_one() {
        let dir = std::env::temp_dir().join(format!("kiln-scan-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for i in 0..40 {
            let sub = dir.join(format!("Dir{i:02}"));
            fs::create_dir(&sub).unwrap();
            for j in 0..(i % 5) {
                fs::write(sub.join(format!("f{j}")), b"x").unwrap();
            }
            fs::write(dir.join(format!("file{i:02}.txt")), vec![b'a'; i]).unwrap();
        }
        fs::write(dir.join(".hidden"), b"h").unwrap();

        let path = dir.to_str().unwrap();
        let big = scan(path, true, false);
        let items = big["items"].as_array().unwrap();
        assert_eq!(items.len(), 80);
        // folders first, each sorted by lower-case name
        assert_eq!(items[0]["n"], "Dir00");
        assert_eq!(items[39]["n"], "Dir39");
        assert_eq!(items[40]["n"], "file00.txt");
        assert_eq!(items[79]["n"], "file39.txt");
        assert_eq!(items[3]["c"], 3);
        assert_eq!(items[45]["s"], 5);

        // the same rows, built without threads
        let entries: Vec<(String, PathBuf)> = items
            .iter()
            .map(|v| {
                let n = v["n"].as_str().unwrap().to_string();
                let p = dir.join(&n);
                (n, p)
            })
            .collect();
        let few: Vec<Option<ScanRow>> = entries
            .iter()
            .map(|(n, p)| scan_entry(n, p, true))
            .collect();
        let many = scan_entries(&entries, true);
        assert_eq!(few.len(), many.len());
        for (a, b) in few.iter().zip(&many) {
            assert_eq!(a.as_ref().map(|r| &r.2), b.as_ref().map(|r| &r.2));
        }

        // hidden files only when asked for
        assert_eq!(
            scan(path, true, true)["items"].as_array().unwrap().len(),
            81
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
