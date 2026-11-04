//! Folder browsing: `scan`, `search` and `scan_params` (what Java needs to recognise archives).

use std::fs;
use std::io::Read;
use std::path::Path;
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

/// What the Java side needs to recognise archives while it lists folders itself.
pub fn scan_params() -> Value {
    let mut sig = String::new();
    for b in kiln::format::MAGIC.iter() {
        sig.push_str(&format!("{b:02x}"));
    }
    json!({
        "sig": sig,
        "min": MIN_ARCHIVE,
        "builtin": true,
        "enc": ENC_FLAG,
        "index": INDEX_NAME,
        "len": SNIFF_LEN,
        "flagbyte": FLAG_BYTE,
    })
}

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

/// One folder for the file manager: folders, archives, sets and (when `show_all`) every file.
pub fn scan(path: &str, show_all: bool, show_hidden: bool) -> Value {
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => {
            return json!({ "ok": false, "error": format!("can't open {path}: {}", io::os_msg(&e)) });
        }
    };
    let mut rows: Vec<(u8, String, Value)> = Vec::new();
    for ent in rd {
        let ent = match ent {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        let p = ent.path();
        let md = match fs::metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if md.is_dir() {
            rows.push((0, name.to_lowercase(), dir_row(&name, &p, &md)));
        } else if md.is_file() {
            if let Some(v) = file_row(&name, &p, &md, show_all) {
                rows.push((1, name.to_lowercase(), v));
            }
        }
    }
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
