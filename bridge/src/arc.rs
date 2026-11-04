//! Archive functions of the bridge: `open_archive`, `extract`, `test` and the preview cache
//! (`cached_extract`, `preview`). They call the `kiln` engine directly; archive data never passes
//! through this layer, and progress comes only from the engine's own callback.
//!
//! A locked archive (see `lock`) is decrypted into a private scratch folder first and the engine
//! works on that plain copy; folders of random-name archives ("sets") are handled in `sets`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use kiln::format::index::EntryKind;
use kiln::integrity::FileHasher;
use kiln::path::{normalize_selector, safe_join, selector_matches};
use kiln::{Archive, Error, ExtractOptions, extract_with_progress, verify_with_progress};
use serde_json::{Value, json};

use crate::cache;
use crate::io::{Meter, os_msg};
use crate::lock::{self, LockError, Opened, Ring, Scratch};
use crate::sets;
use crate::text::{self, fmt_size, from_error, secs, storage_full};

/// Why a run did not finish.
pub(crate) enum Fail {
    /// A ready message (and the "done" count the app has always reported with it).
    Msg(String, u64),
    /// The engine stopped with this error.
    Engine(Error),
    /// The archive is locked and no password was given.
    Need,
    /// Opening or decrypting a locked file failed (wrong password, damaged, cancelled...).
    Lock(LockError),
}

impl Fail {
    fn into_json(self) -> Value {
        match self {
            Fail::Msg(m, done) => text::failed(&m, done),
            Fail::Engine(e) => from_error(&e),
            Fail::Need => json!({
                "ok": false,
                "need_password": true,
                "message": "enter a password",
                "bad_password": false,
            }),
            Fail::Lock(LockError::Cancelled) => text::cancelled(),
            Fail::Lock(e) => text::failed(&e.message(), 0),
        }
    }
}

/// One thing to extract: a file, or a folder with everything below it.
pub(crate) struct Member {
    pub p: String,
    pub dir: bool,
}

/// `[{"p": path inside the archive, "dir": bool}, ...]`; empty = everything.
fn parse_members(json_text: &str) -> Vec<Member> {
    let v: Value = serde_json::from_str(json_text).unwrap_or(Value::Null);
    let Value::Array(items) = v else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|o| {
            let p = o.get("p")?.as_str()?.to_string();
            let dir = o.get("dir").and_then(|d| d.as_bool()).unwrap_or(false);
            Some(Member { p, dir })
        })
        .collect()
}

/// The text shown when an archive cannot be opened.
fn open_msg(e: &Error) -> String {
    match e {
        Error::NotAnArchive(_) => "this file isn't a Kiln archive".to_string(),
        Error::Io { source, .. } | Error::IoBare(source) => {
            format!("couldn't read this archive: {}", os_msg(source))
        }
        other => text::describe(other),
    }
}

/// An archive the engine can read: the file itself, or the plain copy of a locked one (which
/// disappears with this value).
struct Ready {
    archive: Archive,
    _scratch: Option<Scratch>,
}

/// Open the archive file `path`. A locked one needs `password` and is decrypted first: only its
/// footer and index when `full` is false (listing), all of it when true (extract, test). The
/// meter spends the first part of the bar on decrypting.
fn open_file(path: &str, password: &str, full: bool, meter: &mut Meter) -> Result<Ready, Fail> {
    let p = Path::new(path);
    let mut f = fs::File::open(p)
        .map_err(|e| Fail::Msg(format!("couldn't read this archive: {}", os_msg(&e)), 0))?;
    let found = lock::peek(&mut f).map_err(Fail::Lock)?;
    drop(f);
    let Some(found) = found else {
        let archive = Archive::open(p).map_err(|e| Fail::Msg(open_msg(&e), 0))?;
        return Ok(Ready {
            archive,
            _scratch: None,
        });
    };
    if found.kind() != lock::KIND_ARCHIVE {
        return Err(Fail::Msg("this file isn't a Kiln archive".to_string(), 0));
    }
    if password.is_empty() {
        return Err(Fail::Need);
    }
    let mut ring = Ring::new(password);
    let mut opened = Opened::open(p, found, &mut ring).map_err(Fail::Lock)?;
    let scratch = Scratch::new().map_err(Fail::Lock)?;
    let plain = scratch.path().join("archive.kiln");
    if full {
        meter.set_span(0.0, 0.25);
        opened
            .decrypt_to(&plain, &mut |d, t| meter.tick(d, t, "", false))
            .map_err(Fail::Lock)?;
        meter.set_span(0.25, 0.75);
    } else {
        opened.reveal_index(&plain).map_err(Fail::Lock)?;
    }
    let archive = Archive::open(&plain).map_err(|e| Fail::Msg(open_msg(&e), 0))?;
    Ok(Ready {
        archive,
        _scratch: Some(scratch),
    })
}

// ---------------------------------------------------------------------------------- open

/// Table of contents of an archive file: `{ok, kind:"archive", encrypted, entries:[{p,s,m}], packed}`.
pub fn open_archive(path: &str, password: &str) -> Value {
    if Path::new(path).is_dir() {
        return match sets::open(Path::new(path), password) {
            Ok(v) => v,
            Err(f) => open_fail(f),
        };
    }
    let mut meter = Meter::new();
    match open_file(path, password, false, &mut meter) {
        Ok(ready) => {
            let a = &ready.archive;
            let entries: Vec<Value> = a
                .entries()
                .iter()
                .filter(|e| e.kind == EntryKind::File)
                .map(|e| json!({ "p": e.path, "s": e.size, "m": e.mtime_secs }))
                .collect();
            json!({
                "ok": true,
                "kind": "archive",
                "encrypted": ready._scratch.is_some(),
                "entries": entries,
                "packed": a.block_stats().payload_bytes,
            })
        }
        Err(f) => open_fail(f),
    }
}

/// The answer of `open_archive` when it can't open: a password is needed, the password is wrong,
/// or the archive is unreadable.
fn open_fail(f: Fail) -> Value {
    match f {
        Fail::Need => json!({ "ok": false, "need_password": true }),
        Fail::Lock(LockError::Wrong) => {
            json!({ "ok": false, "error": lock::WRONG, "bad_password": true })
        }
        Fail::Msg(m, _) => json!({ "ok": false, "error": m, "bad_password": false }),
        Fail::Engine(e) => json!({ "ok": false, "error": open_msg(&e), "bad_password": false }),
        Fail::Lock(e) => json!({ "ok": false, "error": e.message(), "bad_password": false }),
    }
}

// ---------------------------------------------------------------------------------- extract

pub(crate) struct Done {
    pub files: u64,
    pub bytes: u64,
    pub secs: String,
}

/// "N file(s) already exist" with the list shown to the person.
pub(crate) fn clash_fail(clashes: &[&str]) -> Fail {
    let shown: Vec<&str> = clashes.iter().take(10).copied().collect();
    let more = if clashes.len() > 10 {
        format!("\n  ... and {} more", clashes.len() - 10)
    } else {
        String::new()
    };
    let msg = format!(
        "{} file(s) already exist:\n  {}{}\nnothing was extracted; use -f to overwrite",
        clashes.len(),
        shown.join("\n  "),
        more
    );
    // The app has always counted the indented lines of this message as "done".
    let indented = msg.split('\n').filter(|l| l.starts_with("  ")).count() as u64;
    Fail::Msg(msg, indented)
}

/// Extract `members` (empty = everything) of `path` into `dest`.
fn do_extract(
    path: &str,
    password: &str,
    members: &[Member],
    dest: &str,
    overwrite: bool,
    jobs: usize,
) -> Result<Done, Fail> {
    let mut meter = Meter::new();
    if Path::new(path).is_dir() {
        return sets::extract(
            Path::new(path),
            password,
            members,
            dest,
            overwrite,
            jobs,
            &mut meter,
        );
    }
    let ready = open_file(path, password, true, &mut meter)?;
    let archive = &ready.archive;
    let entries = archive.entries();

    // Which entries are asked for (a member names a file, or a folder and all below it).
    let selectors: Vec<String> = members.iter().map(|m| normalize_selector(&m.p)).collect();
    let mut selected: Vec<usize> = Vec::new();
    let mut engine_members: Vec<String> = Vec::new();
    if selectors.is_empty() {
        selected.extend(0..entries.len());
    } else {
        let mut hit = vec![false; entries.len()];
        for (m, sel) in members.iter().zip(&selectors) {
            let mut any = false;
            for (i, e) in entries.iter().enumerate() {
                if sel.is_empty() || selector_matches(&e.path, sel) {
                    hit[i] = true;
                    any = true;
                }
            }
            if any {
                engine_members.push(m.p.clone());
            }
        }
        selected.extend((0..entries.len()).filter(|&i| hit[i]));
        if selected.is_empty() {
            let names: Vec<String> = members
                .iter()
                .map(|m| {
                    if m.dir {
                        format!("{}/*", m.p)
                    } else {
                        m.p.clone()
                    }
                })
                .collect();
            return Err(Fail::Msg(
                format!("no files in the archive match: {}", names.join(", ")),
                0,
            ));
        }
    }

    // Look at the destination before writing anything: refuse unsafe names, find what would be
    // overwritten, and remember which files are new (to undo them on cancel).
    let root = Path::new(dest);
    let mut clashes: Vec<&str> = Vec::new();
    let mut fresh: Vec<PathBuf> = Vec::new();
    for &i in &selected {
        let e = &entries[i];
        if e.kind != EntryKind::File {
            continue;
        }
        let target = safe_join(root, &e.path)
            .map_err(|_| Fail::Msg(format!("refusing unsafe path: {}", e.path), 0))?;
        if fs::symlink_metadata(&target).is_ok() {
            clashes.push(&e.path);
        } else {
            fresh.push(target);
        }
    }
    if !clashes.is_empty() && !overwrite {
        return Err(clash_fail(&clashes));
    }
    let dest_existed = root.exists();

    let mut options = ExtractOptions::new(dest);
    options.overwrite = overwrite;
    options.threads = jobs;
    options.members = engine_members;
    let started = Instant::now();
    let outcome = extract_with_progress(archive, &options, &mut |done, total| {
        meter.tick(done, total, "", false)
    });
    match outcome {
        Ok(stats) => {
            meter.tick(1, 1, "", true);
            Ok(Done {
                files: stats.files,
                bytes: stats.bytes,
                secs: secs(started.elapsed()),
            })
        }
        Err(e) => {
            if kiln::cancel::is_cancelled(&e) {
                roll_back(root, dest_existed, &fresh);
            }
            Err(Fail::Engine(e))
        }
    }
}

/// Undo a cancelled extraction: remove what it created, and the folders that leaves empty.
pub(crate) fn roll_back(root: &Path, dest_existed: bool, fresh: &[PathBuf]) {
    if !dest_existed {
        let _ = fs::remove_dir_all(root);
        return;
    }
    for f in fresh {
        let _ = fs::remove_file(f);
    }
    for f in fresh {
        let mut dir = f.parent();
        while let Some(d) = dir {
            if d == root || !d.starts_with(root) || fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
}

/// `extract(path, password, members_json, dest, overwrite, total)`.
pub fn extract(
    path: &str,
    password: &str,
    members_json: &str,
    dest: &str,
    overwrite: bool,
    jobs: usize,
) -> Value {
    let members = parse_members(members_json);
    match do_extract(path, password, &members, dest, overwrite, jobs) {
        Ok(d) => json!({
            "ok": true,
            "message": format!(
                "extracted {} file(s), {} in {}s - all checksums OK",
                d.files, fmt_size(d.bytes), d.secs
            ),
            "done": d.files,
            "bad_password": false,
        }),
        Err(f) => f.into_json(),
    }
}

// ---------------------------------------------------------------------------------- test

/// `test(path, password)`: decode every chunk and check every checksum, writing nothing.
pub fn test(path: &str, password: &str, jobs: usize) -> Value {
    let mut meter = Meter::new();
    if Path::new(path).is_dir() {
        return match sets::test(Path::new(path), password, jobs, &mut meter) {
            Ok((files, bytes, took)) => json!({
                "ok": true,
                "message": format!(
                    "verified {files} file(s), {} in {took}s - all checksums OK",
                    fmt_size(bytes)
                ),
                "done": files,
                "bad_password": false,
            }),
            Err(f) => f.into_json(),
        };
    }
    let ready = match open_file(path, password, true, &mut meter) {
        Ok(r) => r,
        Err(f) => return f.into_json(),
    };
    let started = Instant::now();
    let outcome = verify_with_progress(&ready.archive, jobs, None, &mut |done, total| {
        meter.tick(done, total, "", false)
    });
    match outcome {
        Ok(stats) => {
            meter.tick(1, 1, "", true);
            json!({
                "ok": true,
                "message": format!(
                    "verified {} file(s), {} in {}s - all checksums OK",
                    stats.files, fmt_size(stats.bytes), secs(started.elapsed())
                ),
                "done": stats.files,
                "bad_password": false,
            })
        }
        Err(e) => from_error(&e),
    }
}

// ---------------------------------------------------------------------------------- preview cache
// A file opened from an archive is extracted once into cache_dir/<key>/<name>. The key comes from
// the archive (path, size, modified time) and the member, so opening the same file again finds it
// there. A copy is built in "<key>.part" and renamed when complete, so a cancel or crash never
// leaves something that looks finished.

/// 24 hex characters identifying `member` of the archive as it is on disk now.
fn cache_key(path: &str, member: &str) -> std::io::Result<String> {
    let md = fs::metadata(path)?;
    let nanos = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let abs = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
    let raw = format!("{}\0{}\0{}\0{}", abs.display(), md.len(), nanos, member);
    let mut h = FileHasher::new();
    h.update(raw.as_bytes());
    let digest = h.finalize();
    Ok(digest[..12].iter().map(|b| format!("{b:02x}")).collect())
}

/// Extract one member into the cache, or find it there already: `{ok, file, cached, key}`.
pub fn cached_extract(
    path: &str,
    password: &str,
    member: &str,
    cache_dir: &str,
    size: u64,
    jobs: usize,
) -> Value {
    if let Err(e) = fs::create_dir_all(cache_dir) {
        return text::failed(&format!("{cache_dir}: {}", os_msg(&e)), 0);
    }
    let key = match cache_key(path, member) {
        Ok(k) => k,
        Err(e) => {
            return json!({ "ok": false, "message": format!("can't read the archive: {}", os_msg(&e)) });
        }
    };
    let final_dir = Path::new(cache_dir).join(&key);
    let part = Path::new(cache_dir).join(format!("{key}.part"));
    let target = match safe_join(&final_dir, member) {
        Ok(t) => t,
        Err(_) => {
            return json!({ "ok": false, "message": format!("refusing unsafe path: {member}") });
        }
    };
    if target.is_file() {
        if let Ok(f) = fs::File::open(&final_dir) {
            let _ = f.set_modified(SystemTime::now()); // most recently used
        }
        return json!({ "ok": true, "file": target.to_string_lossy(), "cached": true, "key": key });
    }
    cache::rm(&final_dir); // something half-there under this key: start clean
    cache::rm(&part);
    cache::trim(cache_dir, None, &key); // make room first (also clears old unfinished copies)

    let part_s = part.to_string_lossy().into_owned();
    let one = [Member {
        p: member.to_string(),
        dir: false,
    }];
    let result = match do_extract(path, password, &one, &part_s, true, jobs) {
        Ok(d) => match safe_join(&part, member) {
            Ok(p) if p.is_file() => match fs::rename(&part, &final_dir) {
                Ok(()) => json!({
                    "ok": true,
                    "message": format!(
                        "extracted {} file(s), {} in {}s - all checksums OK",
                        d.files, fmt_size(d.bytes), d.secs
                    ),
                    "done": d.files,
                    "bad_password": false,
                    "file": target.to_string_lossy(),
                    "cached": false,
                    "key": key,
                }),
                Err(e) => text::failed(&format!("{}: {}", final_dir.display(), os_msg(&e)), 0),
            },
            _ => json!({ "ok": false, "message": "the file wasn't found in the archive" }),
        },
        Err(Fail::Engine(e)) if storage_full(&e) => {
            let need = if size > 0 {
                format!(" {}", fmt_size(size))
            } else {
                String::new()
            };
            text::failed(
                &format!(
                    "not enough free storage: this file needs{need} and there isn't room for it"
                ),
                0,
            )
        }
        Err(f) => f.into_json(),
    };
    cache::rm(&part); // nothing is left behind unless it finished
    result
}

/// Extract one file into the cache for viewing (reused on later opens): `{ok, file}`.
pub fn preview(
    path: &str,
    password: &str,
    member: &str,
    cache_dir: &str,
    size: u64,
    jobs: usize,
) -> Value {
    let res = cached_extract(path, password, member, cache_dir, size, jobs);
    let fresh = res.get("ok").and_then(|v| v.as_bool()) == Some(true)
        && res.get("cached").and_then(|v| v.as_bool()) != Some(true);
    if fresh {
        // a new file may have pushed the cache over its limit
        let key = res.get("key").and_then(|v| v.as_str()).unwrap_or("");
        cache::trim(cache_dir, None, key);
    }
    res
}

// ---------------------------------------------------------------------------------- decrypt

/// `decrypt_in_place(path, password, ...)`: a set becomes an ordinary folder (real names, no
/// index); a locked archive file becomes a plain archive.
pub fn decrypt_in_place(path: &str, password: &str, jobs: usize) -> Value {
    let mut meter = Meter::new();
    let started = Instant::now();
    let outcome = if Path::new(path).is_dir() {
        sets::decrypt_in_place(Path::new(path), password, jobs, &mut meter).map(|d| d.files)
    } else {
        unlock_file(path, password, &mut meter).map(|()| 1)
    };
    match outcome {
        Ok(n) => json!({
            "ok": true,
            "message": format!("decrypted {n} file(s) in {}s", secs(started.elapsed())),
            "done": n,
            "bad_password": false,
        }),
        Err(f) => f.into_json(),
    }
}

/// Replace a locked archive file by the plain archive inside it.
fn unlock_file(path: &str, password: &str, meter: &mut Meter) -> Result<(), Fail> {
    let p = Path::new(path);
    let mut f = fs::File::open(p)
        .map_err(|e| Fail::Msg(format!("couldn't read this archive: {}", os_msg(&e)), 0))?;
    let found = lock::peek(&mut f).map_err(Fail::Lock)?;
    drop(f);
    let Some(found) = found else {
        return Err(Fail::Msg("this archive isn't encrypted".to_string(), 0));
    };
    if password.is_empty() {
        return Err(Fail::Need);
    }
    let mut ring = Ring::new(password);
    let mut opened = Opened::open(p, found, &mut ring).map_err(Fail::Lock)?;
    let mut part = p.as_os_str().to_os_string();
    part.push(".tmp");
    let part = PathBuf::from(part);
    let res = opened.decrypt_to(&part, &mut |d, t| meter.tick(d, t, "", false));
    match res {
        Ok(()) => fs::rename(&part, p).map_err(|e| {
            let _ = fs::remove_file(&part);
            Fail::Msg(format!("{path}: {}", os_msg(&e)), 0)
        }),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(Fail::Lock(e))
        }
    }
}
