//! Staged editing inside a Kiln archive: `begin`, `add`, `remove`, `transfer`, `set_file`,
//! `new_text`, `file_for`, `save`, `discard`, `state`, `close`.
//!
//! Nothing touches the archive until `save`. The bridge is one process per call, so a session
//! lives on disk: a folder `kiln-edit-<sid>` under TMPDIR holding `session.json` (the archive, its
//! password, and the list of entries as the person sees them) and `staged/` (files added or
//! changed). The names, messages and result fields are the ones `kiln_edit.py` used.
//!
//! Saving extracts the entries that are kept from the archive into a scratch tree, puts the
//! staged files beside them under their new names, packs the tree into a temporary archive next
//! to the original (locked again when the original was locked) and swaps it into place.
//!
//! Not supported: folders of random-name archives ("sets") can't be edited.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kiln::integrity::FileHasher;
use kiln::path::safe_join;
use kiln::walk;
use kiln::{PackOptions, Profile, pack_with_progress};
use serde_json::{Value, json};

use crate::arc::{self, Fail, Member};
use crate::cache;
use crate::io::{Meter, os_msg};
use crate::lock::{self, LockError};
use crate::text::{self, fmt_size, secs};

/// More kept files than this: extract the whole archive and skip the unwanted ones, instead of
/// naming every member to the engine.
const NAMED_LIMIT: usize = 2000;

type R = Result<Value, Value>;

fn bad(msg: &str) -> Value {
    json!({ "ok": false, "message": msg })
}

fn ioe(e: std::io::Error) -> Value {
    bad(&os_msg(&e))
}

fn lockerr(e: LockError) -> Value {
    match e {
        LockError::Cancelled => text::cancelled(),
        other => bad(&other.message()),
    }
}

fn run(f: impl FnOnce() -> R) -> Value {
    match f() {
        Ok(v) | Err(v) => v,
    }
}

fn ended() -> Value {
    bad("this editing session has ended; reopen the archive")
}

// ---------------------------------------------------------------------------------- session

/// One file as the person sees it in the archive now.
#[derive(Clone)]
struct Ent {
    /// Its name now.
    rel: String,
    /// Its name inside the archive on disk (None for a file that is not from there).
    orig: Option<String>,
    /// A file holding its new contents (added, changed or copied), by absolute path.
    staged: Option<String>,
    size: u64,
    mtime: i64,
    mode: u32,
}

impl Ent {
    fn mark(&self) -> &'static str {
        if self.staged.is_some() || self.orig.is_none() {
            "+"
        } else if self.orig.as_deref() != Some(self.rel.as_str()) {
            "~"
        } else {
            ""
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "rel": self.rel,
            "orig": self.orig,
            "staged": self.staged,
            "size": self.size,
            "mtime": self.mtime,
            "mode": self.mode,
        })
    }

    fn from_json(v: &Value) -> Option<Ent> {
        Some(Ent {
            rel: v.get("rel")?.as_str()?.to_string(),
            orig: v.get("orig").and_then(|x| x.as_str()).map(|s| s.to_string()),
            staged: v
                .get("staged")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            size: v.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
            mtime: v.get("mtime").and_then(|x| x.as_i64()).unwrap_or(0),
            mode: v.get("mode").and_then(|x| x.as_u64()).unwrap_or(0o644) as u32,
        })
    }
}

struct Sess {
    dir: PathBuf,
    path: String,
    password: String,
    encrypted: bool,
    store: bool,
    dirty: bool,
    /// How many files the archive on disk holds.
    orig_count: usize,
    ents: Vec<Ent>,
    dirs: Vec<String>,
}

fn valid_sid(sid: &str) -> bool {
    sid.len() == 32 && sid.bytes().all(|b| b.is_ascii_hexdigit())
}

fn session_dir(sid: &str) -> PathBuf {
    std::env::temp_dir().join(format!("kiln-edit-{sid}"))
}

fn rand_name() -> Result<String, Value> {
    lock::random_name().map_err(lockerr)
}

impl Sess {
    fn staged_dir(&self) -> PathBuf {
        self.dir.join("staged")
    }

    fn load(sid: &str) -> Result<Sess, Value> {
        if !valid_sid(sid) {
            return Err(ended());
        }
        let dir = session_dir(sid);
        let raw = fs::read_to_string(dir.join("session.json")).map_err(|_| ended())?;
        let v: Value = serde_json::from_str(&raw).map_err(|_| ended())?;
        let ents = match v.get("entries") {
            Some(Value::Array(a)) => a.iter().filter_map(Ent::from_json).collect(),
            _ => Vec::new(),
        };
        let dirs = match v.get("dirs") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        };
        let text_of = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let flag = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        Ok(Sess {
            dir,
            path: text_of("path"),
            password: text_of("password"),
            encrypted: flag("encrypted"),
            store: flag("store"),
            dirty: flag("dirty"),
            orig_count: v.get("orig_count").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            ents,
            dirs,
        })
    }

    /// Write `session.json` (private to the app, replaced atomically).
    fn persist(&self) -> Result<(), Value> {
        let v = json!({
            "path": self.path,
            "password": self.password,
            "encrypted": self.encrypted,
            "store": self.store,
            "dirty": self.dirty,
            "orig_count": self.orig_count,
            "entries": self.ents.iter().map(Ent::to_json).collect::<Vec<_>>(),
            "dirs": self.dirs,
        });
        let tmp = self.dir.join("session.json.tmp");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(ioe)?;
        f.write_all(v.to_string().as_bytes()).map_err(ioe)?;
        f.flush().map_err(ioe)?;
        drop(f);
        fs::rename(&tmp, self.dir.join("session.json")).map_err(ioe)
    }

    /// Replace the entries by what the archive on disk holds (start, discard, after a save).
    fn reset_from(&mut self, l: arc::Listing) {
        self.encrypted = l.encrypted;
        self.store = l.store;
        self.orig_count = l.files.len();
        self.ents = l
            .files
            .into_iter()
            .map(|(p, s, m, mode)| Ent {
                rel: p.clone(),
                orig: Some(p),
                staged: None,
                size: s,
                mtime: m,
                mode,
            })
            .collect();
        self.dirs = l.dirs;
        self.dirty = false;
    }

    fn clear_staged(&self) {
        let d = self.staged_dir();
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
    }

    /// A fresh path inside `staged/`.
    fn new_staged(&self) -> Result<PathBuf, Value> {
        Ok(self.staged_dir().join(rand_name()?))
    }
}

/// The session as the app shows it, plus `extra` fields.
fn view(s: &Sess, extra: Vec<(&str, Value)>) -> Value {
    let ents: Vec<Value> = s
        .ents
        .iter()
        .map(|e| json!({ "p": e.rel, "s": e.size, "m": e.mtime, "k": e.mark() }))
        .collect();
    let mut v = json!({ "ok": true, "entries": ents, "dirty": s.dirty });
    for (k, x) in extra {
        v[k] = x;
    }
    v
}

fn norm(p: &str) -> String {
    p.split('/')
        .filter(|x| !x.is_empty() && *x != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn base_of(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn under(rel: &str, m: &Member) -> bool {
    rel == m.p || (m.dir && rel.starts_with(&format!("{}/", m.p)))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Remove edit folders nobody has touched for a day (the app was killed mid-edit).
fn sweep() {
    let base = std::env::temp_dir();
    let Ok(rd) = fs::read_dir(&base) else { return };
    for ent in rd.flatten() {
        if !ent.file_name().to_string_lossy().starts_with("kiln-edit-") {
            continue;
        }
        let old = ent
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 3600));
        if old {
            let _ = fs::remove_dir_all(ent.path());
        }
    }
}

// ---------------------------------------------------------------------------------- files

/// Copy `src` to `dst`, checking for Cancel; a partial copy is deleted. Keeps the modified time
/// and permissions.
fn copy_cancel(src: &Path, dst: &Path, meter: &mut Meter, name: &str) -> Result<(), Value> {
    let md = fs::metadata(src).map_err(ioe)?;
    let total = md.len();
    let result = (|| -> Result<(), Value> {
        let mut fi = fs::File::open(src).map_err(ioe)?;
        let mut fo = fs::File::create(dst).map_err(ioe)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            if kiln::cancel::is_set() {
                return Err(text::cancelled());
            }
            let n = fi.read(&mut buf).map_err(ioe)?;
            if n == 0 {
                break;
            }
            fo.write_all(&buf[..n]).map_err(ioe)?;
            done += n as u64;
            meter.tick(done, total, name, false);
        }
        fo.flush().map_err(ioe)?;
        if let Ok(m) = md.modified() {
            let _ = fo.set_modified(m);
        }
        drop(fo);
        let _ = fs::set_permissions(dst, md.permissions());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(dst);
    } else {
        meter.tick(total, total, name, true);
    }
    result
}

/// A hard link when the two paths are on one filesystem, else a copy.
fn link_or_copy(src: &Path, dst: &Path, meter: &mut Meter, name: &str) -> Result<(), Value> {
    if fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    copy_cancel(src, dst, meter, name)
}

fn fail_json(f: Fail) -> Value {
    f.into_json()
}

/// Extract originals of the archive into `dest` under their archive names. `names` empty means
/// the whole archive.
fn extract_origs(
    s: &Sess,
    names: &[String],
    dest: &Path,
    jobs: usize,
    meter: &mut Meter,
) -> Result<(), Value> {
    let members: Vec<Member> = names
        .iter()
        .map(|p| Member {
            p: p.clone(),
            dir: false,
        })
        .collect();
    let dest_s = dest.to_string_lossy().into_owned();
    arc::do_extract_m(&s.path, &s.password, &members, &dest_s, true, jobs, meter)
        .map(|_| ())
        .map_err(fail_json)
}

fn archive_is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

// ---------------------------------------------------------------------------------- session

/// `begin(path, password)` -> `{ok, sid, entries, dirty, encrypted, is_set}`.
pub fn begin(path: &str, password: &str) -> Value {
    run(|| {
        sweep();
        if archive_is_dir(path) {
            return Err(bad("Editing a random-name set isn't supported yet"));
        }
        let listing = match arc::listing(path, password) {
            Ok(l) => l,
            Err(f) => {
                let v = f.into_json();
                if v.get("cancelled").and_then(|c| c.as_bool()) == Some(true) {
                    return Err(v);
                }
                let detail = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
                return Err(bad(&if detail.is_empty() {
                    "couldn't open the archive for editing".to_string()
                } else {
                    format!("couldn't open the archive for editing: {detail}")
                }));
            }
        };
        let sid = rand_name()?;
        let dir = session_dir(&sid);
        fs::create_dir_all(dir.join("staged")).map_err(ioe)?;
        let mut s = Sess {
            dir,
            path: path.to_string(),
            password: password.to_string(),
            encrypted: false,
            store: false,
            dirty: false,
            orig_count: 0,
            ents: Vec::new(),
            dirs: Vec::new(),
        };
        s.reset_from(listing);
        if let Err(e) = s.persist() {
            let _ = fs::remove_dir_all(&s.dir);
            return Err(e);
        }
        Ok(view(
            &s,
            vec![
                ("sid", json!(sid)),
                ("encrypted", json!(s.encrypted)),
                ("is_set", json!(false)),
            ],
        ))
    })
}

/// `state(sid)`.
pub fn state(sid: &str) -> Value {
    run(|| {
        let s = Sess::load(sid)?;
        Ok(view(&s, vec![]))
    })
}

/// `close(sid)`: forget the session and delete its scratch files.
pub fn close(sid: &str) -> Value {
    if valid_sid(sid) {
        let _ = fs::remove_dir_all(session_dir(sid));
    }
    json!({ "ok": true })
}

/// `discard(sid)`: throw the staged changes away.
pub fn discard(sid: &str) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        let l = arc::listing(&s.path, &s.password).map_err(|f| {
            let v = f.into_json();
            let m = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            bad(&format!("couldn't read the archive: {m}"))
        })?;
        s.reset_from(l);
        s.clear_staged();
        s.persist()?;
        Ok(view(&s, vec![]))
    })
}

// ---------------------------------------------------------------------------------- edits

/// `add(sid, paths, dest)`: add phone files and folders into archive folder `dest` ('' = root).
pub fn add(sid: &str, paths: &[String], dest: &str) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        let inputs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let exclude = [
            PathBuf::from(&s.path),
            PathBuf::from(format!("{}.tmp", s.path)),
        ];
        let found = walk::collect(&inputs, &exclude).map_err(|e| text::from_error(&e))?;
        let dest = norm(dest);
        let pre = if dest.is_empty() {
            String::new()
        } else {
            format!("{dest}/")
        };
        let mut added = 0u64;
        let mut replaced = 0u64;
        for e in &found.entries {
            let rel = format!("{pre}{}", e.name);
            match e.kind {
                kiln::format::index::EntryKind::Dir => {
                    if !s.dirs.contains(&rel) {
                        s.dirs.push(rel);
                        s.dirty = true;
                    }
                }
                kiln::format::index::EntryKind::File => {
                    let before = s.ents.len();
                    s.ents.retain(|x| x.rel != rel);
                    replaced += (before - s.ents.len()) as u64;
                    s.ents.push(Ent {
                        rel,
                        orig: None,
                        staged: Some(absolute(&e.fs_path).to_string_lossy().into_owned()),
                        size: e.size,
                        mtime: e.mtime_secs,
                        mode: e.mode,
                    });
                    added += 1;
                    s.dirty = true;
                }
            }
        }
        s.persist()?;
        Ok(view(
            &s,
            vec![("added", json!(added)), ("replaced", json!(replaced))],
        ))
    })
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `remove(sid, members)`.
pub fn remove(sid: &str, members: &[Member]) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        let before = s.ents.len();
        s.ents.retain(|e| !members.iter().any(|m| under(&e.rel, m)));
        let n = before - s.ents.len();
        let dirs_before = s.dirs.len();
        s.dirs.retain(|d| !members.iter().any(|m| under(d, m)));
        if n > 0 || s.dirs.len() != dirs_before {
            s.dirty = true;
        }
        s.persist()?;
        Ok(view(&s, vec![("removed", json!(n))]))
    })
}

/// What moving or copying `members` into folder `dest` does: `(entry index, new name)` for every
/// file, and `(old folder, new folder)` for every folder.
type Targets = (Vec<(usize, String)>, Vec<(String, String)>);

fn targets(s: &Sess, members: &[Member], dest: &str, rename_to: &str) -> Result<Targets, Value> {
    let dest = norm(dest);
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut roots: Vec<(String, String)> = Vec::new();
    for m in members {
        let base = base_of(&m.p).to_string();
        let name = if rename_to.is_empty() {
            base.clone()
        } else {
            rename_to.to_string()
        };
        if name.contains('/') || name.is_empty() || name == "." || name == ".." {
            return Err(bad("a name can't contain / or be empty"));
        }
        let newroot = if dest.is_empty() {
            name
        } else {
            format!("{dest}/{name}")
        };
        if m.dir && format!("{newroot}/").starts_with(&format!("{}/", m.p)) {
            return Err(bad(&format!("can't put {base} inside itself")));
        }
        if m.dir {
            roots.push((m.p.clone(), newroot.clone()));
        }
        for (i, e) in s.ents.iter().enumerate() {
            if under(&e.rel, m) {
                out.push((i, format!("{newroot}{}", &e.rel[m.p.len()..])));
            }
        }
    }
    if out.is_empty() {
        return Err(bad("nothing matched"));
    }
    let news: HashSet<&String> = out.iter().map(|(_, n)| n).collect();
    if news.len() != out.len() {
        return Err(bad("two files would end up with the same name"));
    }
    Ok((out, roots))
}

/// Python's `os.path.splitext` on a `/`-separated name.
fn splitext(p: &str) -> (&str, &str) {
    let sep = p.rfind('/').map(|i| i as isize).unwrap_or(-1);
    let dot = match p.rfind('.') {
        Some(d) if (d as isize) > sep => d,
        _ => return (p, ""),
    };
    let bytes = p.as_bytes();
    let mut i = (sep + 1) as usize;
    while i < dot {
        if bytes[i] != b'.' {
            return (&p[..dot], &p[dot..]);
        }
        i += 1;
    }
    (p, "")
}

/// `transfer(sid, members, dest, move, policy, rename_to)`: move or copy items within the archive.
/// `policy`: "ask" (report clashes, change nothing), "replace", "skip", "keep".
pub fn transfer(
    sid: &str,
    members: &[Member],
    dest: &str,
    mv: bool,
    policy: &str,
    rename_to: &str,
    jobs: usize,
) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        let (mut pairs, roots) = targets(&s, members, dest, rename_to)?;
        if mv {
            pairs.retain(|(i, n)| s.ents[*i].rel != *n);
            if pairs.is_empty() {
                return Ok(view(&s, vec![("done", json!(0))]));
            }
        }
        let moved: HashSet<usize> = if mv {
            pairs.iter().map(|(i, _)| *i).collect()
        } else {
            HashSet::new()
        };
        let stay: HashSet<&str> = s
            .ents
            .iter()
            .enumerate()
            .filter(|(i, _)| !moved.contains(i))
            .map(|(_, e)| e.rel.as_str())
            .collect();
        let clash: BTreeSet<String> = pairs
            .iter()
            .filter(|(_, n)| stay.contains(n.as_str()))
            .map(|(_, n)| n.clone())
            .collect();
        drop(stay);
        let mut drop_flags = vec![false; s.ents.len()];
        if !clash.is_empty() {
            match policy {
                "ask" => {
                    let list: Vec<&String> = clash.iter().take(20).collect();
                    return Err(json!({
                        "ok": false,
                        "conflicts": list,
                        "n_conflicts": clash.len(),
                    }));
                }
                "skip" => pairs.retain(|(_, n)| !clash.contains(n)),
                "keep" => {
                    let mut taken: HashSet<String> = s.ents.iter().map(|e| e.rel.clone()).collect();
                    taken.extend(pairs.iter().map(|(_, n)| n.clone()));
                    for (_, n) in pairs.iter_mut() {
                        if clash.contains(&*n) {
                            let (stem, ext) = splitext(n);
                            let (stem, ext) = (stem.to_string(), ext.to_string());
                            let mut k = 1;
                            while taken.contains(&format!("{stem} ({k}){ext}")) {
                                k += 1;
                            }
                            *n = format!("{stem} ({k}){ext}");
                            taken.insert(n.clone());
                        }
                    }
                }
                "replace" => {
                    for (i, e) in s.ents.iter().enumerate() {
                        if clash.contains(&e.rel) && !moved.contains(&i) {
                            drop_flags[i] = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if pairs.is_empty() {
            return Ok(view(&s, vec![("done", json!(0))]));
        }
        let done = pairs.len();

        if mv {
            for (i, n) in &pairs {
                s.ents[*i].rel = n.clone();
            }
            for (old, new) in &roots {
                for d in s.dirs.iter_mut() {
                    if d == old || d.starts_with(&format!("{old}/")) {
                        *d = format!("{new}{}", &d[old.len()..]);
                    }
                }
            }
            apply_drops(&mut s, &drop_flags);
        } else {
            let mut meter = Meter::new();
            let copies: Vec<(Ent, String)> = pairs
                .iter()
                .map(|(i, n)| (s.ents[*i].clone(), n.clone()))
                .collect();
            // originals are read from the archive in one go
            let mut wanted: Vec<String> = Vec::new();
            for (e, _) in &copies {
                if let (None, Some(o)) = (&e.staged, &e.orig) {
                    if !wanted.contains(o) {
                        wanted.push(o.clone());
                    }
                }
            }
            let scratch = s.dir.join(format!("x-{}", rand_name()?));
            if !wanted.is_empty() {
                let res = extract_origs(&s, &wanted, &scratch, jobs, &mut meter);
                if let Err(e) = res {
                    let _ = fs::remove_dir_all(&scratch);
                    return Err(e);
                }
            }
            let mut staged_new: Vec<Ent> = Vec::new();
            let mut failure: Option<Value> = None;
            for (e, n) in &copies {
                let src: PathBuf = match (&e.staged, &e.orig) {
                    (Some(p), _) => PathBuf::from(p),
                    (None, Some(o)) => match safe_join(&scratch, o) {
                        Ok(p) => p,
                        Err(_) => {
                            failure = Some(bad(&format!("refusing unsafe path: {o}")));
                            break;
                        }
                    },
                    (None, None) => {
                        failure = Some(bad("couldn't read a file to copy it"));
                        break;
                    }
                };
                let dst = match s.new_staged() {
                    Ok(d) => d,
                    Err(v) => {
                        failure = Some(v);
                        break;
                    }
                };
                if let Err(v) = link_or_copy(&src, &dst, &mut meter, base_of(n)) {
                    failure = Some(v);
                    break;
                }
                staged_new.push(Ent {
                    rel: n.clone(),
                    orig: None,
                    staged: Some(dst.to_string_lossy().into_owned()),
                    size: e.size,
                    mtime: e.mtime,
                    mode: e.mode,
                });
            }
            let _ = fs::remove_dir_all(&scratch);
            if let Some(v) = failure {
                for e in &staged_new {
                    if let Some(p) = &e.staged {
                        let _ = fs::remove_file(p);
                    }
                }
                return Err(v);
            }
            apply_drops(&mut s, &drop_flags);
            s.ents.extend(staged_new);
        }
        s.dirty = true;
        s.persist()?;
        Ok(view(&s, vec![("done", json!(done))]))
    })
}

/// Remove the entries flagged by `flags` (indexes into the entries as they were).
fn apply_drops(s: &mut Sess, flags: &[bool]) {
    if !flags.iter().any(|f| *f) {
        return;
    }
    let mut i = 0;
    s.ents.retain(|_| {
        let keep = !flags.get(i).copied().unwrap_or(false);
        i += 1;
        keep
    });
}

/// Put `src` into `staged/` and make it the contents of `member`.
fn stage_file(s: &mut Sess, member: &str, src: &Path) -> Result<(), Value> {
    let dst = s.new_staged()?;
    fs::copy(src, &dst).map_err(ioe)?;
    let md = fs::metadata(&dst).map_err(ioe)?;
    let mode = s
        .ents
        .iter()
        .find(|e| e.rel == member)
        .map(|e| e.mode)
        .unwrap_or(0o644);
    s.ents.retain(|e| e.rel != member);
    s.ents.push(Ent {
        rel: member.to_string(),
        orig: None,
        staged: Some(dst.to_string_lossy().into_owned()),
        size: md.len(),
        mtime: now_secs(),
        mode,
    });
    s.dirty = true;
    Ok(())
}

/// `set_file(sid, member, src_file)`: replace one file's contents.
pub fn set_file(sid: &str, member: &str, src_file: &str) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        stage_file(&mut s, member, Path::new(src_file))?;
        s.persist()?;
        Ok(view(&s, vec![]))
    })
}

/// `new_text(sid, member)`: stage an empty text file.
pub fn new_text(sid: &str, member: &str) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        let m = norm(member);
        if m.is_empty() {
            return Err(bad("a name can't contain / or be empty"));
        }
        if s.ents.iter().any(|e| e.rel == m) {
            return Err(bad("that name already exists"));
        }
        let empty = s.dir.join(format!("empty-{}", rand_name()?));
        fs::File::create(&empty).map_err(ioe)?;
        let res = stage_file(&mut s, &m, &empty);
        let _ = fs::remove_file(&empty);
        res?;
        s.persist()?;
        Ok(view(&s, vec![]))
    })
}

// ---------------------------------------------------------------------------------- file_for

fn staged_key(archive: &str, member: &str, staged: &Path) -> String {
    let md = fs::metadata(staged).ok();
    let nanos = md
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let len = md.map(|m| m.len()).unwrap_or(0);
    let raw = format!("{archive}\0{member}\0{}\0{nanos}\0{len}", staged.display());
    let mut h = FileHasher::new();
    h.update(raw.as_bytes());
    let digest = h.finalize();
    digest[..12].iter().map(|b| format!("{b:02x}")).collect()
}

/// `file_for(sid, member, cache_dir, readonly)`: a copy of `member`'s current bytes in
/// `cache_dir`, for viewing or editing. Viewing an unchanged file reuses the preview cache.
pub fn file_for(sid: &str, member: &str, cache_dir: &str, readonly: bool, jobs: usize) -> Value {
    run(|| {
        let s = Sess::load(sid)?;
        let e = s
            .ents
            .iter()
            .find(|x| x.rel == member)
            .ok_or_else(|| bad("that file is no longer in the archive"))?;
        let name = base_of(member).to_string();
        fs::create_dir_all(cache_dir).map_err(ioe)?;
        let mut meter = Meter::new();

        if let Some(staged) = &e.staged {
            let staged = PathBuf::from(staged);
            if !staged.is_file() {
                return Err(bad("that file is no longer in the archive"));
            }
            if readonly {
                let key = staged_key(&s.path, member, &staged);
                let dir = Path::new(cache_dir).join(&key);
                let out = dir.join(&name);
                if out.is_file() {
                    return Ok(json!({ "ok": true, "file": out.to_string_lossy(), "cached": true }));
                }
                fs::create_dir_all(&dir).map_err(ioe)?;
                if let Err(v) = link_or_copy(&staged, &out, &mut meter, &name) {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(v);
                }
                cache::trim(cache_dir, None, &key);
                return Ok(json!({ "ok": true, "file": out.to_string_lossy() }));
            }
            let dir = Path::new(cache_dir).join(rand_name()?);
            fs::create_dir_all(&dir).map_err(ioe)?;
            let out = dir.join(&name);
            if let Err(v) = copy_cancel(&staged, &out, &mut meter, &name) {
                let _ = fs::remove_dir_all(&dir);
                return Err(v);
            }
            return Ok(json!({ "ok": true, "file": out.to_string_lossy() }));
        }

        let Some(orig) = e.orig.clone() else {
            return Err(bad("that file is no longer in the archive"));
        };
        if readonly {
            return Ok(arc::preview(
                &s.path,
                &s.password,
                &orig,
                cache_dir,
                e.size,
                jobs,
            ));
        }
        let dir = Path::new(cache_dir).join(rand_name()?);
        let x = dir.join("x");
        fs::create_dir_all(&x).map_err(ioe)?;
        let finish = (|| -> R {
            extract_origs(&s, std::slice::from_ref(&orig), &x, jobs, &mut meter)?;
            let got = safe_join(&x, &orig).map_err(|_| bad("the file wasn't found in the archive"))?;
            let out = dir.join(&name);
            fs::rename(&got, &out).map_err(ioe)?;
            Ok(json!({ "ok": true, "file": out.to_string_lossy() }))
        })();
        let _ = fs::remove_dir_all(&x);
        if finish.is_err() {
            let _ = fs::remove_dir_all(&dir);
        }
        finish
    })
}

// ---------------------------------------------------------------------------------- save

/// Files that are deleted when this goes out of scope (what a failed save leaves half-made).
struct Temps(Vec<PathBuf>);

impl Drop for Temps {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_file(p);
        }
    }
}

fn sibling(path: &str, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{path}{suffix}"))
}

/// `save(sid)`: write the staged changes into the archive (atomic swap through a temporary file).
pub fn save(sid: &str, jobs: usize) -> Value {
    run(|| {
        let mut s = Sess::load(sid)?;
        if !s.dirty {
            return Ok(view(&s, vec![("message", json!("Nothing to save"))]));
        }
        if s.ents.is_empty() && s.dirs.is_empty() {
            return Err(bad(
                "nothing would be left in the archive; delete the archive instead",
            ));
        }
        if archive_is_dir(&s.path) {
            return Err(bad("Editing a random-name set isn't supported yet"));
        }
        let started = Instant::now();
        let mut meter = Meter::new();
        let scratch = lock::Scratch::new().map_err(lockerr)?;
        let orig_dir = scratch.path().join("orig");
        let tree = scratch.path().join("tree");
        fs::create_dir_all(&tree).map_err(ioe)?;

        // 1. the kept originals, read out of the archive
        let kept: Vec<&Ent> = s
            .ents
            .iter()
            .filter(|e| e.staged.is_none() && e.orig.is_some())
            .collect();
        if !kept.is_empty() {
            fs::create_dir_all(&orig_dir).map_err(ioe)?;
            let names: Vec<String> = if kept.len() == s.orig_count || kept.len() > NAMED_LIMIT {
                Vec::new()
            } else {
                kept.iter().filter_map(|e| e.orig.clone()).collect()
            };
            meter.set_outer(0.0, 0.45);
            extract_origs(&s, &names, &orig_dir, jobs, &mut meter)?;
        }

        // 2. the tree as it will be archived
        meter.set_outer(0.45, 0.05);
        meter.set_span(0.0, 1.0);
        let total = s.ents.len() as u64;
        for (n, e) in s.ents.iter().enumerate() {
            if kiln::cancel::is_set() {
                return Err(text::cancelled());
            }
            let dst = safe_join(&tree, &e.rel)
                .map_err(|_| bad(&format!("refusing unsafe path: {}", e.rel)))?;
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).map_err(ioe)?;
            }
            match (&e.staged, &e.orig) {
                (Some(p), _) => {
                    let src = PathBuf::from(p);
                    if !src.is_file() {
                        return Err(bad(&format!("{}: the new version is missing", e.rel)));
                    }
                    link_or_copy(&src, &dst, &mut meter, base_of(&e.rel))?;
                }
                (None, Some(o)) => {
                    let src = safe_join(&orig_dir, o)
                        .map_err(|_| bad(&format!("refusing unsafe path: {o}")))?;
                    fs::rename(&src, &dst).map_err(|_| {
                        bad(&format!("{o}: this file couldn't be read out of the archive"))
                    })?;
                }
                (None, None) => return Err(bad(&format!("{}: nothing to save", e.rel))),
            }
            meter.tick(n as u64 + 1, total, "", false);
        }
        for d in &s.dirs {
            let p = safe_join(&tree, d)
                .map_err(|_| bad(&format!("refusing unsafe path: {d}")))?;
            fs::create_dir_all(p).map_err(ioe)?;
        }

        // 3. pack it
        let mut inputs: Vec<PathBuf> = fs::read_dir(&tree)
            .map_err(ioe)?
            .flatten()
            .map(|e| e.path())
            .collect();
        inputs.sort();
        let profile = if s.store {
            Profile::Store
        } else {
            Profile::Adaptive
        };
        let mut opts = PackOptions::from_profile(profile);
        opts.threads = jobs;
        opts.overwrite = true;
        let packed_to = if s.encrypted {
            sibling(&s.path, ".kilnplain")
        } else {
            sibling(&s.path, ".tmp")
        };
        let lock_tmp = sibling(&s.path, ".tmp");
        let mut temps = Temps(vec![
            packed_to.clone(),
            sibling(&s.path, ".kilnplain.tmp"),
            sibling(&s.path, ".tmp.tmp"),
            lock_tmp.clone(),
        ]);
        meter.set_outer(0.5, if s.encrypted { 0.35 } else { 0.5 });
        meter.set_span(0.0, 1.0);
        let stats = pack_with_progress(&packed_to, &inputs, &opts, &mut |d, t| {
            meter.tick(d, t, "", false)
        })
        .map_err(|e| text::from_error(&e))?;
        let mut archive_bytes = stats.archive_bytes;

        // 4. lock it again, then swap it into place
        if s.encrypted {
            meter.set_outer(0.85, 0.15);
            let setup = lock::Setup::new(&s.password).map_err(lockerr)?;
            archive_bytes = lock::seal(&packed_to, &lock_tmp, &setup, lock::KIND_ARCHIVE, &mut |d, t| {
                meter.tick(d, t, "", false)
            })
            .map_err(lockerr)?;
        }
        fs::rename(&lock_tmp, &s.path).map_err(|e| bad(&format!("{}: {}", s.path, os_msg(&e))))?;
        temps.0.retain(|p| p != &lock_tmp);
        drop(temps);
        meter.set_outer(0.0, 1.0);
        meter.tick(1, 1, "", true);
        drop(scratch);

        // 5. the session now matches the new archive
        let l = arc::listing(&s.path, &s.password).map_err(|f| {
            let v = f.into_json();
            let m = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            bad(&format!("saved, but couldn't read the archive back: {m}"))
        })?;
        s.reset_from(l);
        s.clear_staged();
        s.persist()?;
        let pct = if stats.raw_bytes > 0 {
            100.0 * archive_bytes as f64 / stats.raw_bytes as f64
        } else {
            100.0
        };
        let message = format!(
            "{} file(s): {} -> {} ({:.1}% of original) in {}s",
            stats.files,
            fmt_size(stats.raw_bytes),
            fmt_size(archive_bytes),
            pct,
            secs(started.elapsed())
        );
        Ok(view(&s, vec![("message", json!(message))]))
    })
}
