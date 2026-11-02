//! App layer: the JSON operations the Android app needs (open / test / extract), as plain Rust
//! with no FFI. Port of kiln_app.open_archive / extract / test (+ the parts of kiln_v14.run_extract
//! they go through). Results are the same JSON objects kiln_app returns.
//! Verified through tests/mirror/app_mirror.py (line-for-line mirror) against the real kiln_app.

use crate::container::{make_unfilter, Archive, Entry, KilnError, Sink, FLAG_ENCRYPTED, MAGIC};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Error text used internally to unwind a cancelled run (never shown).
pub const CANCELLED: &str = "\u{1}cancelled";
const MIN_ARCHIVE: u64 = 25;

/// What the app gives us: a place to send progress events and a way to ask "stop?".
pub trait Progress {
    fn event(&mut self, json: &str);
    fn cancelled(&mut self) -> bool;
}
pub struct NoProgress;
impl Progress for NoProgress {
    fn event(&mut self, _json: &str) {}
    fn cancelled(&mut self) -> bool {
        false
    }
}

pub fn version() -> String {
    json!({"kiln": format!("rs-{}", env!("CARGO_PKG_VERSION")), "helper": ""}).to_string()
}

// ---------------------------------------------------------------- fnmatch

#[derive(Debug, Clone)]
enum Tok {
    Star,
    Any,
    Lit(char),
    Set(bool, Vec<(char, char)>), // negated, ranges
}

/// Port of Python's fnmatch translate rules (posix, case-sensitive): * ? [seq] [!seq]
fn compile(p: &str) -> Vec<Tok> {
    let c: Vec<char> = p.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let ch = c[i];
        i += 1;
        match ch {
            '*' => {
                if !matches!(out.last(), Some(Tok::Star)) {
                    out.push(Tok::Star);
                }
            }
            '?' => out.push(Tok::Any),
            '[' => {
                let mut j = i;
                if j < n && c[j] == '!' {
                    j += 1;
                }
                if j < n && c[j] == ']' {
                    j += 1;
                }
                while j < n && c[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    out.push(Tok::Lit('['));
                } else {
                    let mut body: &[char] = &c[i..j];
                    let neg = !body.is_empty() && body[0] == '!';
                    if neg {
                        body = &body[1..];
                    }
                    let mut ranges = Vec::new();
                    let mut k = 0;
                    while k < body.len() {
                        if k + 2 < body.len() && body[k + 1] == '-' {
                            ranges.push((body[k], body[k + 2]));
                            k += 3;
                        } else {
                            ranges.push((body[k], body[k]));
                            k += 1;
                        }
                    }
                    out.push(Tok::Set(neg, ranges));
                    i = j + 1;
                }
            }
            _ => out.push(Tok::Lit(ch)),
        }
    }
    out
}

fn one(t: &Tok, ch: char) -> bool {
    match t {
        Tok::Any => true,
        Tok::Lit(l) => *l == ch,
        Tok::Set(neg, rs) => rs.iter().any(|&(a, b)| a <= ch && ch <= b) != *neg,
        Tok::Star => false,
    }
}

pub fn fnmatch(name: &str, pat: &str) -> bool {
    let toks = compile(pat);
    let s: Vec<char> = name.chars().collect();
    let (mut ti, mut si) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while si < s.len() {
        if ti < toks.len() && matches!(toks[ti], Tok::Star) {
            star = Some(ti);
            ti += 1;
            mark = si;
        } else if ti < toks.len() && one(&toks[ti], s[si]) {
            ti += 1;
            si += 1;
        } else if let Some(st) = star {
            mark += 1;
            si = mark;
            ti = st + 1;
        } else {
            return false;
        }
    }
    while ti < toks.len() && matches!(toks[ti], Tok::Star) {
        ti += 1;
    }
    ti == toks.len()
}

/// glob.escape on posix: each of * ? [ becomes a one-character set.
pub fn glob_escape(p: &str) -> String {
    let mut o = String::new();
    for ch in p.chars() {
        match ch {
            '*' | '?' | '[' => {
                o.push('[');
                o.push(ch);
                o.push(']');
            }
            _ => o.push(ch),
        }
    }
    o
}

/// kiln_app._patterns: [{"p":..,"dir":bool}] -> patterns
pub fn patterns(members: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(a) = members.as_array() {
        for m in a {
            let p = m.get("p").and_then(|v| v.as_str()).unwrap_or("");
            let dir = m.get("dir").and_then(|v| v.as_bool()).unwrap_or(false);
            let e = glob_escape(p);
            out.push(if dir { format!("{e}/*") } else { e });
        }
    }
    out
}

// ---------------------------------------------------------------- paths

/// os.path.abspath: lexical, no symlink resolution.
pub fn abspath(p: &str) -> String {
    let full = if p.starts_with('/') {
        p.to_string()
    } else {
        let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());
        format!("{}/{}", cwd.trim_end_matches('/'), p)
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn join_rel(root: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        rel.to_string()
    } else if root.ends_with('/') {
        format!("{root}{rel}")
    } else {
        format!("{root}/{rel}")
    }
}

pub fn is_under(root: &str, dest: &str) -> bool {
    dest.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

// ---------------------------------------------------------------- messages

/// fmt_size from kiln_v14
pub fn fmt_size(n: u64) -> String {
    let mut x = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if x < 1024.0 || unit == "GB" {
            return if unit == "B" { format!("{:.0} {}", x, unit) } else { format!("{:.1} {}", x, unit) };
        }
        x /= 1024.0;
    }
    unreachable!()
}

/// What the app shows for an "error: ..." printed by the engine (kiln_app._Sink.message):
/// the first line, then every further non-blank line stripped.
pub fn app_message(err: &str) -> String {
    let lines: Vec<&str> = err.split(|c| c == '\n' || c == '\r').filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut v = vec![lines[0].trim().to_string()];
    for l in &lines[1..] {
        v.push(l.trim().to_string());
    }
    v.join("\n").trim().to_string()
}

/// Something this engine can't do (yet): the app falls back to the Python engine, nothing was touched.
pub(crate) fn unsupported(msg: &str) -> String {
    json!({"ok": false, "unsupported": true, "error": msg, "message": msg, "done": 0, "bad_password": false}).to_string()
}

pub(crate) fn fail_json(msg: &str, done: u64) -> String {
    json!({"ok": false, "message": msg, "done": done, "bad_password": msg.contains("wrong password")}).to_string()
}

// ---------------------------------------------------------------- open

/// kiln_app._sniff for one file: Ok(Some(encrypted)) = a KLNA archive, Err = a ready JSON error.
fn sniff(path: &str) -> std::result::Result<bool, String> {
    let size = fs::metadata(path).map_err(|e| format!("couldn't read this archive: {e}"))?.len();
    let mut head = [0u8; 5];
    let mut n = 0;
    if size >= MIN_ARCHIVE {
        let mut f = fs::File::open(path).map_err(|e| format!("couldn't read this archive: {e}"))?;
        while n < 5 {
            match f.read(&mut head[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(_) => break,
            }
        }
    }
    if size >= MIN_ARCHIVE && n >= 4 && &head[..4] == MAGIC {
        return if n >= 5 {
            Ok(head[4] & FLAG_ENCRYPTED != 0)
        } else {
            Err("this file isn't a Kiln archive".into())
        };
    }
    if size >= MIN_ARCHIVE && n >= 4 && (&head[..4] == b"KLN9" || &head[..4] == b"KLN2") {
        return Err("this archive was made by an older Kiln (format 9 or earlier) that this app can't open".into());
    }
    Err("this file isn't a Kiln archive".into())
}

fn open_file(path: &str, password: &str) -> std::result::Result<Archive<fs::File>, String> {
    let enc = sniff(path)?;
    let f = fs::File::open(path).map_err(|e| format!("couldn't read this archive: {e}"))?;
    Archive::open(f, if enc { Some(password) } else { None }).map_err(|e| e.0)
}

pub fn open_archive(path: &str, password: &str) -> String {
    if Path::new(path).is_dir() {
        return crate::sets::set_open(path, password);
    }
    let enc = match sniff(path) {
        Ok(e) => e,
        Err(m) => return json!({"ok": false, "error": m}).to_string(),
    };
    if enc && password.is_empty() {
        return json!({"ok": false, "need_password": true}).to_string();
    }
    match open_file(path, password) {
        Ok(a) => {
            let ents: Vec<Value> = a
                .table
                .entries
                .iter()
                .filter(|e| !e.dead)
                .map(|e| json!({"p": e.rel, "s": e.size, "m": e.mtime}))
                .collect();
            json!({"ok": true, "kind": "archive", "encrypted": enc, "entries": ents, "packed": a.table_off - a.hdr_len})
                .to_string()
        }
        Err(m) => json!({"ok": false, "error": m, "bad_password": m.contains("wrong password")}).to_string(),
    }
}

// ---------------------------------------------------------------- the sink

pub(crate) struct FsSink<'a> {
    outer: Option<(u64, u64)>, // (archives finished, archives in the set) when part of a set
    root: String,
    write: bool,
    verbose_done: u64,
    prog: &'a mut dyn Progress,
    total: u64,
    bytes: u64,
    last: Option<Instant>,
    file: String,
    fh: Option<fs::File>,
    dest: Option<String>,
    created: Vec<String>,
    made: Vec<String>,
}

impl<'a> FsSink<'a> {
    pub(crate) fn new(root: String, write: bool, prog: &'a mut dyn Progress, total: u64, outer: Option<(u64, u64)>) -> FsSink<'a> {
        FsSink { outer, root, write, verbose_done: 0, prog, total, bytes: 0, last: None, file: String::new(), fh: None, dest: None, created: vec![], made: vec![] }
    }
    fn emit(&mut self, force: bool) {
        let now = Instant::now();
        if !force {
            if let Some(l) = self.last {
                if now.duration_since(l) < Duration::from_millis(50) {
                    return;
                }
            }
        }
        self.last = Some(now);
        if let Some((od, on)) = self.outer {
            let inner = if self.total > 0 { (self.bytes as f64 / self.total as f64).min(1.0) } else { 0.0 };
            let frac = if on > 0 { ((od as f64 + inner) / on as f64).min(1.0) } else { -1.0 };
            let ev = json!({"frac": frac, "done": od, "total": on, "file": self.file});
            self.prog.event(&ev.to_string());
            return;
        }
        let frac = if self.total > 0 { (self.bytes as f64 / self.total as f64).min(1.0) } else { -1.0 };
        let ev = json!({"frac": frac, "done": self.verbose_done, "total": 0, "file": self.file});
        self.prog.event(&ev.to_string());
    }
    fn check(&mut self) -> crate::container::Result<()> {
        if self.prog.cancelled() {
            return Err(KilnError(CANCELLED.to_string()));
        }
        Ok(())
    }
    /// A half-written file is closed and deleted, never left behind.
    pub(crate) fn abort(&mut self) {
        if self.fh.take().is_some() {
            if let Some(d) = self.dest.take() {
                let _ = fs::remove_file(d);
            }
        }
    }
    /// After a cancel: remove everything this run created.
    pub(crate) fn rollback(&mut self) {
        self.abort();
        for p in self.created.drain(..).rev().collect::<Vec<_>>() {
            let _ = fs::remove_file(p);
        }
        for d in self.made.drain(..).rev().collect::<Vec<_>>() {
            let _ = fs::remove_dir(d);
        }
    }
}

impl<'a> Sink for FsSink<'a> {
    fn begin(&mut self, _idx: usize, e: &Entry) -> crate::container::Result<()> {
        self.check()?;
        self.file = e.rel.clone();
        if self.write {
            let dest = abspath(&join_rel(&self.root, &e.rel));
            let parent = match dest.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(i) => dest[..i].to_string(),
            };
            let mut d = parent.clone();
            let mut missing = Vec::new();
            while d != "/" && !Path::new(&d).is_dir() {
                missing.push(d.clone());
                d = match d.rfind('/') {
                    Some(0) | None => "/".to_string(),
                    Some(i) => d[..i].to_string(),
                };
            }
            fs::create_dir_all(&parent).map_err(|er| KilnError(format!("{parent}: {er}")))?;
            for m in missing.into_iter().rev() {
                self.made.push(m);
            }
            self.emit(true);
            let existed = fs::symlink_metadata(&dest).is_ok();
            let fh = fs::File::create(&dest).map_err(|er| KilnError(format!("{dest}: {er}")))?;
            if !existed {
                self.created.push(dest.clone());
            }
            self.fh = Some(fh);
            self.dest = Some(dest);
        } else {
            self.emit(true);
        }
        Ok(())
    }
    fn write(&mut self, _idx: usize, piece: &[u8]) -> crate::container::Result<()> {
        self.check()?;
        self.bytes += piece.len() as u64;
        if let Some(fh) = self.fh.as_mut() {
            fh.write_all(piece).map_err(|er| KilnError(format!("write error: {er}")))?;
        }
        self.emit(false);
        Ok(())
    }
    fn end(&mut self, _idx: usize, e: &Entry) -> crate::container::Result<()> {
        if let Some(fh) = self.fh.take() {
            let _ = fh.set_modified(UNIX_EPOCH + Duration::from_secs(e.mtime));
            drop(fh);
            if let Some(d) = self.dest.take() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let m = (e.mode as u32) & 0o777;
                    let _ = fs::set_permissions(&d, fs::Permissions::from_mode(if m == 0 { 0o644 } else { m }));
                }
                self.verbose_done += 1;
            }
        } else if !self.write {
            // verify only: nothing to close
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- test / extract

/// kiln_app.test for one archive file.
pub fn test(path: &str, password: &str, prog: &mut dyn Progress) -> String {
    run(path, password, &[], "", false, false, prog)
}

/// kiln_app.extract: members_json = [{"p":..,"dir":bool}], [] = everything.
pub fn extract(path: &str, password: &str, members_json: &str, dest: &str, overwrite: bool, prog: &mut dyn Progress) -> String {
    let members: Value = serde_json::from_str(members_json).unwrap_or(Value::Array(vec![]));
    let pats = patterns(&members);
    run(path, password, &pats, dest, overwrite, true, prog)
}

fn run(path: &str, password: &str, names: &[String], dest: &str, force: bool, write: bool, prog: &mut dyn Progress) -> String {
    if Path::new(path).is_dir() {
        return crate::sets::set_run(path, password, names, dest, force, write, prog);
    }
    let enc = match sniff(path) {
        Ok(e) => e,
        Err(m) => return fail_json(&m, 0),
    };
    if enc && password.is_empty() {
        return fail_json("enter a password", 0);
    }
    let mut a = match open_file(path, password) {
        Ok(a) => a,
        Err(m) => return fail_json(&app_message(&m), 0),
    };
    if a.table.entries.iter().any(|e| make_unfilter(e).is_err()) {
        // WAV/MP3 filters arrive in CP8: decide before anything is written
        return unsupported("this archive uses the MP3 filter, which the Rust engine does not support yet");
    }
    let t0 = Instant::now();
    let root = abspath(if write { dest } else { "." });
    let wanted: Vec<bool> = a
        .table
        .entries
        .iter()
        .map(|e| !e.dead && (names.is_empty() || names.iter().any(|p| fnmatch(&e.rel, p) || &e.rel == p)))
        .collect();
    if !names.is_empty() && !wanted.iter().any(|&w| w) {
        return fail_json(&app_message(&format!("no files in the archive match: {}", names.join(", "))), 0);
    }
    if write {
        let mut clashes: Vec<String> = Vec::new();
        for (e, w) in a.table.entries.iter().zip(&wanted) {
            if !*w {
                continue;
            }
            let d = abspath(&join_rel(&root, &e.rel));
            if !is_under(&root, &d) {
                return fail_json(&app_message(&format!("refusing unsafe path: {}", e.rel)), 0);
            }
            if Path::new(&d).exists() {
                clashes.push(e.rel.clone());
            }
        }
        if !clashes.is_empty() && !force {
            let shown = clashes.iter().take(10).cloned().collect::<Vec<_>>().join("\n  ");
            let more = if clashes.len() > 10 { format!("\n  ... and {} more", clashes.len() - 10) } else { String::new() };
            let m = format!(
                "{} file(s) already exist:\n  {}{}\nnothing was extracted; use -f to overwrite",
                clashes.len(),
                shown,
                more
            );
            // kiln_app's sink counts every printed line that starts with two spaces as "done",
            // so the indented file list of this message counts too; kept for identical results
            let indented = m.split('\n').filter(|l| l.starts_with("  ")).count() as u64;
            return fail_json(&app_message(&m), indented);
        }
    }
    // the same range of files the engine will decode (see Archive::extract)
    let n = a.table.entries.len();
    let select = !names.is_empty();
    let (lo, hi) = if select {
        (wanted.iter().position(|&w| w).unwrap(), wanted.iter().rposition(|&w| w).unwrap())
    } else {
        (0, n.saturating_sub(1))
    };
    let (k, _off, _ctr) = a.table.find_ckpt(lo, a.hdr_len);
    let part = select && (k > 0 || hi + 1 < n);
    let total: u64 = if part {
        a.table.entries[k..=hi].iter().map(|e| e.slen).sum()
    } else {
        a.table.entries.iter().map(|e| e.slen).sum()
    };
    let mut sink = FsSink {
        outer: None,
        root,
        write,
        verbose_done: 0,
        prog,
        total,
        bytes: 0,
        last: None,
        file: String::new(),
        fh: None,
        dest: None,
        created: vec![],
        made: vec![],
    };
    let res = a.extract(&wanted, select, &mut sink);
    match res {
        Ok(()) => {
            sink.abort();
            sink.emit(true);
            let nf = wanted.iter().filter(|&&w| w).count();
            let verb = if write { "extracted" } else { "verified" };
            let msg = format!("{verb} {nf} file(s), {} in {:.1}s - all checksums OK", fmt_size(total), t0.elapsed().as_secs_f64());
            json!({"ok": true, "message": msg, "done": sink.verbose_done, "bad_password": false}).to_string()
        }
        Err(e) if e.0 == CANCELLED => {
            sink.rollback();
            json!({"ok": false, "cancelled": true, "message": "Cancelled"}).to_string()
        }
        Err(e) => {
            sink.abort();
            fail_json(&app_message(&e.0), sink.verbose_done)
        }
    }
}
