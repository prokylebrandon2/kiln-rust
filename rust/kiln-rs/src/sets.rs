//! Sets (a folder with an encrypted `_index.kln` and one archive per file): open / test / extract.
//! Port of kiln_v14.read_index_meta + run_extract_dir + extract_one and the set paths of kiln_app.
//! Line-for-line with the "sets (CP8b)" section of tests/mirror/app_mirror.py, which is verified
//! against the REAL kiln_app (test_app.py parts sets..sets5). NOT COMPILED.

use crate::app::{abspath, app_message, fail_json, fnmatch, is_under, join_rel, unsupported, FsSink, NoProgress, Progress, CANCELLED};
use crate::container::{make_unfilter, Archive, Entry, KilnError, Result, Sink};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const INDEX_NAME: &str = "_index.kln";
const INDEX_FILE: &str = "index.json";
const MP3_MSG: &str = "this archive uses the MP3 filter, which the Rust engine does not support yet";
/// Unwinds a run that met an MP3 filter (the run is undone, the app then answers "unsupported").
const UNSUPPORTED_ERR: &str = "\u{1}unsupported";
const DAMAGED_INDEX: &str = "the index archive is damaged or isn't a kiln index";

pub struct IndexItem {
    pub name: String,
    pub path: String,
    pub size: Value, // number, or null in a v1 index
    pub mtime: Value,
}

/// kiln_app's sink counts every printed line that starts with two spaces as `done`.
fn indented(msg: &str) -> u64 {
    msg.replace('\r', "\n").split('\n').filter(|l| !l.trim().is_empty() && l.starts_with("  ")).count() as u64
}

fn cancelled_json() -> String {
    json!({"ok": false, "cancelled": true, "message": "Cancelled"}).to_string()
}

/// Collects index.json of the index archive in memory.
struct IndexSink<'a> {
    prog: &'a mut dyn Progress,
    cur: Option<Vec<u8>>,
    buf: Option<Vec<u8>>,
}
impl<'a> IndexSink<'a> {
    fn check(&mut self) -> Result<()> {
        if self.prog.cancelled() {
            return Err(KilnError(CANCELLED.to_string()));
        }
        Ok(())
    }
}
impl<'a> Sink for IndexSink<'a> {
    fn begin(&mut self, _idx: usize, e: &Entry) -> Result<()> {
        self.check()?;
        self.cur = if e.rel == INDEX_FILE { Some(Vec::new()) } else { None };
        Ok(())
    }
    fn write(&mut self, _idx: usize, piece: &[u8]) -> Result<()> {
        self.check()?;
        if let Some(c) = self.cur.as_mut() {
            c.extend_from_slice(piece);
        }
        Ok(())
    }
    fn end(&mut self, _idx: usize, _e: &Entry) -> Result<()> {
        if let Some(c) = self.cur.take() {
            self.buf = Some(c);
        }
        Ok(())
    }
}

fn is_int(v: &Value) -> bool {
    v.is_i64() || v.is_u64()
}

/// index.json bytes -> entries in file order (needs serde_json's preserve_order feature).
pub fn parse_index(buf: &[u8]) -> Result<Vec<IndexItem>> {
    let data: Value = serde_json::from_slice(buf).map_err(|_| KilnError(DAMAGED_INDEX.to_string()))?;
    let obj = data.as_object();
    let files = obj.and_then(|o| o.get("files")).and_then(|f| f.as_object());
    let ver = obj.and_then(|o| o.get("v")).and_then(|v| v.as_i64());
    let mut out: Option<Vec<IndexItem>> = None;
    if let (Some(v), Some(files)) = (ver, files) {
        if v == 1 || v == 2 {
            let mut items = Vec::new();
            let mut ok = true;
            for (k, val) in files {
                if v == 1 {
                    match val.as_str() {
                        Some(s) => items.push(IndexItem { name: k.clone(), path: s.to_string(), size: Value::Null, mtime: Value::Null }),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                } else {
                    let o = val.as_object();
                    let path = o.and_then(|o| o.get("path")).and_then(|p| p.as_str());
                    let size = o.and_then(|o| o.get("size")).filter(|x| is_int(x));
                    let mtime = o.and_then(|o| o.get("mtime")).filter(|x| is_int(x));
                    match (path, size, mtime) {
                        (Some(p), Some(s), Some(m)) => items.push(IndexItem { name: k.clone(), path: p.to_string(), size: s.clone(), mtime: m.clone() }),
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if ok {
                out = Some(items);
            }
        }
    }
    out.ok_or_else(|| KilnError("the index archive has an unknown layout".to_string()))
}

/// Unpack _index.kln (every checksum is checked) and read index.json.
pub fn read_index_meta(d: &str, pw: &str, prog: &mut dyn Progress) -> Result<Vec<IndexItem>> {
    let path = join_rel(d, INDEX_NAME);
    let f = fs::File::open(&path).map_err(|e| KilnError(format!("{path}: {e}")))?;
    let mut a = Archive::open(f, Some(pw))?;
    let root = abspath("/kiln-index");
    let wanted: Vec<bool> = a.table.entries.iter().map(|e| !e.dead).collect();
    for (e, w) in a.table.entries.iter().zip(&wanted) {
        if *w && !is_under(&root, &abspath(&join_rel(&root, &e.rel))) {
            return Err(KilnError(format!("refusing unsafe path: {}", e.rel)));
        }
    }
    let mut sink = IndexSink { prog, cur: None, buf: None };
    a.extract(&wanted, false, &mut sink)?;
    let buf = sink.buf.ok_or_else(|| KilnError(DAMAGED_INDEX.to_string()))?;
    parse_index(&buf)
}

pub fn set_open(path: &str, password: &str) -> String {
    if !Path::new(&join_rel(path, INDEX_NAME)).is_file() {
        return json!({"ok": false, "error": "this folder isn't a Kiln set"}).to_string();
    }
    if password.is_empty() {
        return json!({"ok": false, "need_password": true}).to_string();
    }
    let mut np = NoProgress;
    match read_index_meta(path, password, &mut np) {
        Err(e) => {
            if e.0 == CANCELLED {
                return cancelled_json();
            }
            json!({"ok": false, "error": e.0, "bad_password": e.0.contains("wrong password")}).to_string()
        }
        Ok(meta) => {
            let ents: Vec<Value> = meta.iter().map(|it| json!({"p": it.path, "s": it.size.clone(), "m": it.mtime.clone()})).collect();
            let mut packed: u64 = 0;
            for it in &meta {
                if let Ok(m) = fs::metadata(join_rel(path, &it.name)) {
                    packed += m.len();
                }
            }
            json!({"ok": true, "kind": "set", "encrypted": true, "entries": ents, "packed": packed}).to_string()
        }
    }
}

fn last_part(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or("")
}

fn safe_dest(root: &str, rel: &str) -> Result<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    let dest = abspath(&format!("{}/{}", root.trim_end_matches('/'), parts.join("/")));
    if !is_under(root, &dest) || last_part(&dest) != parts[parts.len() - 1] {
        return Err(KilnError(format!("refusing unsafe path: {rel}")));
    }
    Ok(dest)
}

/// One archive of a set: unpack into `root` when `write`, else only verify.
fn extract_archive(src: &str, root: &str, pw: &str, write: bool, prog: &mut dyn Progress, outer: (u64, u64)) -> Result<()> {
    let f = fs::File::open(src).map_err(|e| KilnError(format!("{src}: {e}")))?;
    let mut a = Archive::open(f, Some(pw))?;
    if a.table.entries.iter().any(|e| make_unfilter(e).is_err()) {
        return Err(KilnError(UNSUPPORTED_ERR.to_string()));
    }
    let wanted: Vec<bool> = a.table.entries.iter().map(|e| !e.dead).collect();
    if write {
        for (e, w) in a.table.entries.iter().zip(&wanted) {
            if *w && !is_under(root, &abspath(&join_rel(root, &e.rel))) {
                return Err(KilnError(format!("refusing unsafe path: {}", e.rel)));
            }
        }
    }
    let total: u64 = a.table.entries.iter().map(|e| e.slen).sum();
    let mut sink = FsSink::new(root.to_string(), write, prog, total, Some(outer));
    match a.extract(&wanted, false, &mut sink) {
        Ok(()) => {
            sink.abort();
            Ok(())
        }
        Err(e) => {
            if write {
                sink.rollback();
            } else {
                sink.abort();
            }
            Err(e)
        }
    }
}

fn make_tmp(out_dir: &str) -> Result<String> {
    let pid = std::process::id();
    for n in 0..1000u32 {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let name = format!("{}/.kiln-{}-{}-{}", out_dir.trim_end_matches('/'), pid, nanos, n);
        match fs::create_dir(&name) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&name, fs::Permissions::from_mode(0o700));
                }
                return Ok(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(KilnError(format!("{out_dir}: {e}"))),
        }
    }
    Err(KilnError(format!("{out_dir}: couldn't make a temporary folder")))
}

/// The archive holds exactly one entry (the file); it comes out under the name the index gives it.
fn move_single(tmp: &str, out_dir: &str, name: &str) -> Result<()> {
    let rd = fs::read_dir(tmp).map_err(|e| KilnError(format!("{tmp}: {e}")))?;
    let mut names = Vec::new();
    for ent in rd {
        names.push(ent.map_err(|e| KilnError(format!("{tmp}: {e}")))?.file_name());
    }
    if names.len() != 1 {
        return Err(KilnError("unexpected layout in a set archive".to_string()));
    }
    fs::rename(Path::new(tmp).join(&names[0]), Path::new(out_dir).join(name)).map_err(|e| KilnError(format!("{name}: {e}")))
}

fn extract_one(src: &str, out_dir: &str, pw: &str, write: bool, name: &str, prog: &mut dyn Progress, outer: (u64, u64)) -> Result<()> {
    if write {
        let tmp = make_tmp(out_dir)?;
        let r = extract_archive(src, &tmp, pw, true, prog, outer).and_then(|_| move_single(&tmp, out_dir, name));
        let _ = fs::remove_dir_all(&tmp);
        r
    } else {
        extract_archive(src, "", pw, false, prog, outer)
    }
}

/// An MP3 filter turned up: leave nothing of this run behind.
fn undo(moved: &[String], made: &[String]) {
    for p in moved.iter().rev() {
        let _ = fs::remove_file(p);
    }
    for q in made.iter().rev() {
        let _ = fs::remove_dir(q);
    }
}

/// kiln_app.test / extract for a set. `names` are the (glob-escaped) member patterns.
pub fn set_run(d: &str, password: &str, names: &[String], dest: &str, force: bool, write: bool, prog: &mut dyn Progress) -> String {
    if !Path::new(&join_rel(d, INDEX_NAME)).is_file() {
        let m = format!("{d} is a directory without {INDEX_NAME} (not a kiln random-name set)");
        return fail_json(&app_message(&m), indented(&m));
    }
    if password.is_empty() {
        return fail_json("enter a password", 0);
    }
    let meta = match read_index_meta(d, password, &mut *prog) {
        Ok(m) => m,
        Err(e) => {
            if e.0 == CANCELLED {
                return cancelled_json();
            }
            return fail_json(&app_message(&e.0), indented(&e.0));
        }
    };
    let mut items: Vec<(String, String)> = meta.into_iter().map(|it| (it.name, it.path)).collect();
    items.sort_by(|a, b| a.1.cmp(&b.1)); // stable, by original path (code point order)
    if !names.is_empty() {
        items.retain(|(_, r)| names.iter().any(|p| fnmatch(r, p) || r == p));
        if items.is_empty() {
            let m = format!("no files in the index match: {}", names.join(", "));
            return fail_json(&app_message(&m), indented(&m));
        }
    }
    let root = abspath(if write { dest } else { "." });
    let mut plan: Vec<(String, String, String)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut clashes: Vec<String> = Vec::new();
    for (n, rel) in &items {
        let src = join_rel(d, n);
        let dst = match safe_dest(&root, rel) {
            Ok(x) => x,
            Err(e) => return fail_json(&app_message(&e.0), indented(&e.0)),
        };
        if !Path::new(&src).is_file() {
            missing.push(rel.clone());
            continue;
        }
        if write && Path::new(&dst).exists() {
            clashes.push(rel.clone());
        }
        plan.push((src, dst, rel.clone()));
    }
    if !clashes.is_empty() && !force {
        let shown = clashes.iter().take(10).cloned().collect::<Vec<_>>().join("\n  ");
        let more = if clashes.len() > 10 { format!("\n  ... and {} more", clashes.len() - 10) } else { String::new() };
        let m = format!("{} file(s) already exist:\n  {}{}\nnothing was extracted; use -f to overwrite", clashes.len(), shown, more);
        return fail_json(&app_message(&m), indented(&m));
    }
    let t0 = Instant::now();
    let mut done: u64 = 0;
    let mut moved: Vec<String> = Vec::new();
    let mut made: Vec<String> = Vec::new();
    let n_plan = plan.len() as u64;
    for (src, dst, rel) in &plan {
        let parent = match dst.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(i) => dst[..i].to_string(),
        };
        if write {
            let mut d2 = parent.clone();
            let mut miss: Vec<String> = Vec::new();
            while d2 != "/" && !Path::new(&d2).is_dir() {
                miss.push(d2.clone());
                d2 = match d2.rfind('/') {
                    Some(0) | None => "/".to_string(),
                    Some(i) => d2[..i].to_string(),
                };
            }
            if let Err(er) = fs::create_dir_all(&parent) {
                let m = format!("{parent}: {er}");
                return fail_json(&app_message(&m), done);
            }
            made.extend(miss.into_iter().rev());
        }
        let existed = fs::symlink_metadata(dst).is_ok();
        match extract_one(src, &parent, password, write, last_part(dst), &mut *prog, (done, n_plan)) {
            Ok(()) => {}
            Err(err) => {
                if err.0 == CANCELLED {
                    return cancelled_json();
                }
                if err.0 == UNSUPPORTED_ERR {
                    if write {
                        undo(&moved, &made);
                    }
                    return unsupported(MP3_MSG);
                }
                let m = format!("{rel} ({}): {}", last_part(src), err.0);
                return fail_json(&app_message(&m), done + indented(&m));
            }
        }
        if write && !existed {
            moved.push(dst.clone());
        }
        done += 1;
    }
    let verb = if write { "extracted" } else { "verified" };
    if !missing.is_empty() {
        let shown = missing.iter().take(10).cloned().collect::<Vec<_>>().join("\n  ");
        let m = format!("{} archive(s) listed in the index are missing:\n  {}", missing.len(), shown);
        return fail_json(&app_message(&m), done + indented(&m));
    }
    let msg = format!("{verb} {} file(s) from {d} in {:.1}s - all checksums OK", plan.len(), t0.elapsed().as_secs_f64());
    json!({"ok": true, "message": msg, "done": done, "bad_password": false}).to_string()
}
