//! Plain file-manager operations: `fs_plan`, `fs_transfer` (copy / move) and `fs_delete`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::io::{self, Meter};

/// Why one item could not be finished.
enum Fail {
    Cancel,
    Io(std::io::Error),
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Fail {
        Fail::Io(e)
    }
}

const COPY_BUF: usize = 1 << 20;

struct Job {
    meter: Meter,
    total: u64,
    done: u64,
    skipped: u64,
    dirs: u64,
    errors: Vec<String>,
    /// What a copy created (files, then folders), to undo on cancel.
    made: Vec<PathBuf>,
}

impl Job {
    fn new(total: u64) -> Job {
        Job {
            meter: Meter::new(),
            total,
            done: 0,
            skipped: 0,
            dirs: 0,
            errors: Vec::new(),
            made: Vec::new(),
        }
    }

    fn tick(&mut self, name: &str, force: bool) {
        self.meter.tick(self.done, self.total, name, force);
    }
}

fn check() -> Result<(), Fail> {
    if io::cancelled() {
        Err(Fail::Cancel)
    } else {
        Ok(())
    }
}

fn base_name(p: &Path) -> String {
    match p.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn is_link(p: &Path) -> bool {
    match fs::symlink_metadata(p) {
        Ok(m) => m.file_type().is_symlink(),
        Err(_) => false,
    }
}

/// A real folder (a link to a folder does not count).
fn is_real_dir(p: &Path) -> bool {
    match fs::symlink_metadata(p) {
        Ok(m) => m.is_dir(),
        Err(_) => false,
    }
}

/// A folder, or a link that leads to one.
fn is_dir_follow(p: &Path) -> bool {
    match fs::metadata(p) {
        Ok(m) => m.is_dir(),
        Err(_) => false,
    }
}

/// Files below the given paths (a plain file or a link counts as one).
fn count(paths: &[PathBuf]) -> u64 {
    let mut n = 0u64;
    for p in paths {
        if is_real_dir(p) {
            if let Ok(rd) = fs::read_dir(p) {
                let kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
                n += count(&kids);
            }
        } else {
            n += 1;
        }
    }
    n
}

/// Folders in a tree, including the top one.
fn count_dirs(p: &Path) -> u64 {
    let mut n = 1u64;
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let q = e.path();
            if is_real_dir(&q) {
                n += count_dirs(&q);
            }
        }
    }
    n
}

/// "a/b.txt" -> "a/b (1).txt" (the first free number).
fn unique(path: &Path) -> PathBuf {
    let name = base_name(path);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let parent = match path.parent() {
        Some(p) => p.to_path_buf(),
        None => PathBuf::new(),
    };
    let mut i = 1u32;
    loop {
        let cand = parent.join(format!("{stem} ({i}){ext}"));
        if !lexists(&cand) {
            return cand;
        }
        i += 1;
    }
}

fn inside(child: &Path, parent: &Path) -> bool {
    let c = fs::canonicalize(child).unwrap_or_else(|_| child.to_path_buf());
    let p = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    c.starts_with(&p)
}

// ---------------------------------------------------------------- fs_plan

/// Check a copy or move before running it: `{ok, conflicts: [names]}` or `{ok: false, error}`.
pub fn plan(srcs: &[String], dest: &str, mv: bool) -> Value {
    let dest_path = Path::new(dest);
    if !dest_path.is_dir() {
        return json!({ "ok": false, "error": "the destination folder doesn't exist" });
    }
    let mut conflicts: Vec<String> = Vec::new();
    for s in srcs {
        let sp = Path::new(s);
        let name = base_name(sp);
        if !lexists(sp) {
            return json!({ "ok": false, "error": format!("{name} no longer exists") });
        }
        if sp.is_dir() && inside(dest_path, sp) {
            return json!({ "ok": false, "error": format!("can't put \"{name}\" inside itself") });
        }
        let target = dest_path.join(&name);
        let same = sp.parent() == Some(dest_path);
        if same && mv {
            return json!({ "ok": false, "error": "the items are already in this folder" });
        }
        if same {
            continue; // copying into the same folder always makes "name (1)"
        }
        if lexists(&target) {
            conflicts.push(name);
        }
    }
    json!({ "ok": true, "conflicts": conflicts })
}

// ---------------------------------------------------------------- copy and move

fn copy_inner(src: &Path, tmp: &Path) -> Result<(), Fail> {
    let mut fi = fs::File::open(src)?;
    let md = fi.metadata()?;
    let mut fo = fs::File::create(tmp)?;
    let cap = std::cmp::min(md.len() as usize + 1, COPY_BUF).max(4096);
    let mut buf = vec![0u8; cap];
    loop {
        check()?;
        let n = fi.read(&mut buf)?;
        if n == 0 {
            break;
        }
        fo.write_all(&buf[..n])?;
    }
    let _ = fo.set_permissions(md.permissions());
    if let Ok(m) = md.modified() {
        let _ = fo.set_modified(m);
    }
    Ok(())
}

/// Copy through a temporary name so a half-written file never sits under the real name.
fn copy_file(src: &Path, dst: &Path) -> Result<(), Fail> {
    let mut tmp_name = dst.as_os_str().to_os_string();
    tmp_name.push(".kilntmp");
    let tmp = PathBuf::from(tmp_name);
    let r = match copy_inner(src, &tmp) {
        Ok(()) => fs::rename(&tmp, dst).map_err(Fail::Io),
        Err(e) => Err(e),
    };
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

fn transfer_file(src: &Path, dst: &Path, mv: bool, job: &mut Job) -> Result<(), Fail> {
    if mv {
        if fs::rename(src, dst).is_err() {
            copy_file(src, dst)?;
            fs::remove_file(src)?;
        }
    } else if is_link(src) {
        let fresh = !lexists(dst);
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(&target, dst)?;
        if fresh {
            job.made.push(dst.to_path_buf());
        }
    } else {
        let fresh = !lexists(dst);
        copy_file(src, dst)?;
        if fresh {
            job.made.push(dst.to_path_buf());
        }
    }
    Ok(())
}

fn copy_stat(src: &Path, dst: &Path) {
    if let Ok(md) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, md.permissions());
        if let Ok(m) = md.modified() {
            if let Ok(f) = fs::File::open(dst) {
                let _ = f.set_modified(m);
            }
        }
    }
}

/// Copy or move one file or folder `src` to the exact path `dst`.
fn put(src: &Path, dst: PathBuf, mv: bool, policy: &str, job: &mut Job) -> Result<(), Fail> {
    check()?;
    let name = base_name(src);
    let mut dst = dst;
    if !is_real_dir(src) {
        if lexists(&dst) {
            if is_real_dir(&dst) {
                if policy == "replace" {
                    if let Err(e) = fs::remove_dir_all(&dst) {
                        job.errors.push(format!("{name}: {}", io::os_msg(&e)));
                        job.done += 1;
                        return Ok(());
                    }
                } else if policy == "skip" {
                    job.skipped += 1;
                    job.done += 1;
                    return Ok(());
                } else {
                    dst = unique(&dst);
                }
            } else if policy == "skip" {
                job.skipped += 1;
                job.done += 1;
                return Ok(());
            } else if policy == "keep" {
                dst = unique(&dst);
            }
        }
        match transfer_file(src, &dst, mv, job) {
            Ok(()) => {}
            Err(Fail::Cancel) => return Err(Fail::Cancel),
            Err(Fail::Io(e)) => job.errors.push(format!("{name}: {}", io::os_msg(&e))),
        }
        job.done += 1;
        job.tick(&name, false);
        return Ok(());
    }
    // a folder
    if lexists(&dst) && !is_dir_follow(&dst) {
        if policy == "replace" {
            let _ = fs::remove_file(&dst);
        } else if policy == "skip" {
            let n = count(&[src.to_path_buf()]);
            job.skipped += n;
            job.done += n;
            return Ok(());
        } else {
            dst = unique(&dst);
        }
    }
    if mv && !lexists(&dst) {
        let n = count(&[src.to_path_buf()]);
        let nd = count_dirs(src);
        if fs::rename(src, &dst).is_ok() {
            // same filesystem: instant
            job.dirs += nd;
            job.done += n;
            job.tick(&name, true);
            return Ok(());
        }
    }
    if !is_dir_follow(&dst) {
        job.made.push(dst.clone());
    }
    if let Err(e) = fs::create_dir_all(&dst) {
        job.errors.push(format!("{name}: {}", io::os_msg(&e)));
        return Ok(());
    }
    job.dirs += 1;
    let mut kids: Vec<String> = Vec::new();
    if let Ok(rd) = fs::read_dir(src) {
        for e in rd.flatten() {
            kids.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    kids.sort();
    for k in kids {
        put(&src.join(&k), dst.join(&k), mv, policy, job)?;
    }
    if mv {
        let _ = fs::remove_dir(src); // only succeeds when nothing was skipped
    }
    copy_stat(src, &dst);
    Ok(())
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Copy or move `srcs` into `dest`. `policy` is "replace", "skip" or "keep" (keep both).
pub fn transfer(srcs: &[String], dest: &str, mv: bool, policy: &str) -> Value {
    let pl = plan(srcs, dest, mv);
    if pl.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let msg = pl.get("error").and_then(|v| v.as_str()).unwrap_or("");
        return json!({ "ok": false, "message": msg });
    }
    let paths: Vec<PathBuf> = srcs.iter().map(PathBuf::from).collect();
    let mut job = Job::new(count(&paths));
    let dest_path = Path::new(dest);
    let mut cancelled = false;
    for s in srcs {
        let sp = Path::new(s);
        let mut target = dest_path.join(base_name(sp));
        let same = sp.parent() == Some(dest_path);
        if same {
            target = unique(&target); // duplicate in place
        }
        match put(sp, target, mv, policy, &mut job) {
            Ok(()) => {}
            Err(Fail::Cancel) => {
                cancelled = true;
                break;
            }
            Err(Fail::Io(e)) => job
                .errors
                .push(format!("{}: {}", base_name(sp), io::os_msg(&e))),
        }
    }
    if cancelled {
        if !mv {
            // undo the copy: what was already copied would only be clutter
            for p in job.made.iter().rev() {
                crate::cache::rm(p);
            }
        }
        return json!({ "ok": false, "cancelled": true, "message": "Cancelled" });
    }
    let verb = if mv { "Moved" } else { "Copied" };
    let files = job.done.saturating_sub(job.skipped);
    let mut parts: Vec<String> = Vec::new();
    if files > 0 || job.dirs == 0 {
        parts.push(plural(files, "file", "files"));
    }
    if job.dirs > 0 {
        parts.push(plural(job.dirs, "folder", "folders"));
    }
    let mut msg = format!("{verb} {}", parts.join(" and "));
    if job.skipped > 0 {
        msg.push_str(&format!(", skipped {}", job.skipped));
    }
    if !job.errors.is_empty() {
        let first: Vec<String> = job.errors.iter().take(5).cloned().collect();
        msg.push_str(&format!(
            "; {} failed:\n{}",
            job.errors.len(),
            first.join("\n")
        ));
    }
    job.tick("", true);
    json!({
        "ok": job.errors.is_empty(),
        "message": msg,
        "done": job.done,
        "skipped": job.skipped,
        "dirs": job.dirs,
    })
}

// ---------------------------------------------------------------- fs_delete

fn delete_tree(p: &Path, job: &mut Job) -> Result<(), Fail> {
    let mut kids: Vec<PathBuf> = Vec::new();
    for e in fs::read_dir(p)? {
        kids.push(e?.path());
    }
    for k in kids {
        check()?;
        if is_real_dir(&k) {
            delete_tree(&k, job)?;
        } else {
            fs::remove_file(&k)?;
            job.done += 1;
            job.tick(&base_name(&k), false);
        }
    }
    fs::remove_dir(p)?;
    Ok(())
}

/// Delete files and folders (folders with everything inside them).
pub fn delete(paths: &[String]) -> Value {
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let mut job = Job::new(count(&list));
    for p in &list {
        if check().is_err() {
            return json!({ "ok": false, "cancelled": true, "message": "Cancelled" });
        }
        let r = if is_real_dir(p) {
            delete_tree(p, &mut job)
        } else {
            match fs::remove_file(p) {
                Ok(()) => {
                    job.done += 1;
                    job.tick(&base_name(p), false);
                    Ok(())
                }
                Err(e) => Err(Fail::Io(e)),
            }
        };
        match r {
            Ok(()) => {}
            Err(Fail::Cancel) => {
                return json!({ "ok": false, "cancelled": true, "message": "Cancelled" });
            }
            Err(Fail::Io(e)) => job
                .errors
                .push(format!("{}: {}", base_name(p), io::os_msg(&e))),
        }
    }
    let mut msg = format!("Deleted {} file(s)", job.done);
    if !job.errors.is_empty() {
        let first: Vec<String> = job.errors.iter().take(5).cloned().collect();
        msg.push_str(&format!(
            "; {} failed:\n{}",
            job.errors.len(),
            first.join("\n")
        ));
    }
    json!({ "ok": job.errors.is_empty(), "message": msg })
}
