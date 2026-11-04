//! Sets: a folder holding one locked archive per file under random names, plus a locked index
//! (`_index.kiln`) that says which random name is which file. Without the password nothing in the
//! folder says what is inside. Every archive and the index are made with the same password and
//! share one key derivation (`lock::Setup`).
//!
//! The index is JSON, locked as a `KIND_INDEX` file:
//! `{"v":1,"files":[{"n":"<random>.kiln","p":"path/inside","s":size,"m":mtime}]}`.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Instant;

use kiln::path::{normalize_selector, safe_join, selector_matches, validate_archive_path};
use kiln::{Archive, ExtractOptions, extract_with_progress, verify_with_progress};
use serde_json::{Value, json};

use crate::arc::{Done, Fail, Member, clash_fail, roll_back};
use crate::browse::INDEX_NAME;
use crate::io::{Meter, os_msg};
use crate::lock::{self, LockError, Opened, Ring, Scratch, Setup};
use crate::text::secs;

const DAMAGED_INDEX: &str = "the index archive is damaged or isn't a kiln index";

/// One file of a set.
pub struct Item {
    /// The random file name inside the folder.
    pub name: String,
    /// The path the file has inside the set.
    pub path: String,
    pub size: u64,
    pub mtime: i64,
}

/// Where the index of the set `dir` lives.
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join(INDEX_NAME)
}

/// Lock `items` into the set's index file (written beside it, then renamed into place).
pub fn write_index(dir: &Path, items: &[Item], setup: &Setup) -> Result<(), LockError> {
    let files: Vec<Value> = items
        .iter()
        .map(|i| json!({ "n": i.name, "p": i.path, "s": i.size, "m": i.mtime }))
        .collect();
    let body = json!({ "v": 1, "files": files }).to_string();
    let part = dir.join(format!("{INDEX_NAME}.part"));
    lock::seal_bytes(body.as_bytes(), &part, setup, lock::KIND_INDEX)?;
    if let Err(e) = fs::rename(&part, index_path(dir)) {
        let _ = fs::remove_file(&part);
        return Err(LockError::from(e));
    }
    Ok(())
}

fn plain_name(n: &str) -> bool {
    !n.is_empty() && !n.contains('/') && !n.contains('\\') && n != "." && n != ".."
}

/// Decrypt and parse the index of the set `dir`.
pub fn read_index(dir: &Path, ring: &mut Ring) -> Result<Vec<Item>, Fail> {
    let ix = index_path(dir);
    let damaged = || Fail::Msg(DAMAGED_INDEX.to_string(), 0);
    let mut f = File::open(&ix).map_err(|e| Fail::Msg(format!("{}: {}", ix.display(), os_msg(&e)), 0))?;
    let lock = match lock::peek(&mut f).map_err(Fail::Lock)? {
        Some(l) if l.kind() == lock::KIND_INDEX => l,
        _ => return Err(damaged()),
    };
    drop(f);
    let mut op = Opened::open(&ix, lock, ring).map_err(Fail::Lock)?;
    let data = op.read_all().map_err(Fail::Lock)?;
    let v: Value = serde_json::from_slice(&data).map_err(|_| damaged())?;
    let files = v.get("files").and_then(|f| f.as_array()).ok_or_else(damaged)?;
    let mut items = Vec::with_capacity(files.len());
    for o in files {
        let name = o.get("n").and_then(|x| x.as_str()).ok_or_else(damaged)?;
        let path = o.get("p").and_then(|x| x.as_str()).ok_or_else(damaged)?;
        if !plain_name(name) || validate_archive_path(path).is_err() {
            return Err(damaged());
        }
        items.push(Item {
            name: name.to_string(),
            path: path.to_string(),
            size: o.get("s").and_then(|x| x.as_u64()).unwrap_or(0),
            mtime: o.get("m").and_then(|x| x.as_i64()).unwrap_or(0),
        });
    }
    Ok(items)
}

fn need_password(password: &str) -> Result<(), Fail> {
    if password.is_empty() {
        Err(Fail::Need)
    } else {
        Ok(())
    }
}

/// `open_archive` for a folder: `{ok, kind:"set", encrypted:true, entries:[{p,s,m}], packed}`.
pub fn open(dir: &Path, password: &str) -> Result<Value, Fail> {
    if !index_path(dir).is_file() {
        return Err(Fail::Msg("this folder isn't a Kiln set".to_string(), 0));
    }
    need_password(password)?;
    let items = read_index(dir, &mut Ring::new(password))?;
    let packed: u64 = items
        .iter()
        .filter_map(|i| fs::metadata(dir.join(&i.name)).ok())
        .map(|m| m.len())
        .sum();
    let entries: Vec<Value> = items
        .iter()
        .map(|i| json!({ "p": i.path, "s": i.size, "m": i.mtime }))
        .collect();
    Ok(json!({
        "ok": true,
        "kind": "set",
        "encrypted": true,
        "entries": entries,
        "packed": packed,
    }))
}

/// Decrypt one archive of the set into the scratch folder and open it.
fn open_item(dir: &Path, item: &Item, ring: &mut Ring, scratch: &Scratch) -> Result<Archive, Fail> {
    let path = dir.join(&item.name);
    let mut f = File::open(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Fail::Msg(format!("{}: its archive is missing", item.path), 0)
        } else {
            Fail::Msg(format!("{}: {}", item.path, os_msg(&e)), 0)
        }
    })?;
    let lock = match lock::peek(&mut f).map_err(Fail::Lock)? {
        Some(l) if l.kind() == lock::KIND_ARCHIVE => l,
        _ => return Err(Fail::Msg(format!("{}: not a locked Kiln archive", item.path), 0)),
    };
    drop(f);
    let mut op = Opened::open(&path, lock, ring).map_err(Fail::Lock)?;
    let plain = scratch.path().join("item.kiln");
    op.decrypt_to(&plain, &mut |_, _| {}).map_err(Fail::Lock)?;
    Archive::open(&plain).map_err(Fail::Engine)
}

/// The one file below `dir` (a set's archive holds a single file).
fn only_file(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for ent in fs::read_dir(&d).ok()?.flatten() {
            let p = ent.path();
            match ent.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => return Some(p),
                _ => {}
            }
        }
    }
    None
}

/// Move a finished file into place (a copy when the folders are on different volumes).
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)?;
    fs::remove_file(from)
}

/// Extract `members` (empty = everything) of the set `dir` into `dest`.
pub fn extract(
    dir: &Path,
    password: &str,
    members: &[Member],
    dest: &str,
    overwrite: bool,
    jobs: usize,
    meter: &mut Meter,
) -> Result<Done, Fail> {
    need_password(password)?;
    let mut ring = Ring::new(password);
    let items = read_index(dir, &mut ring)?;

    let selectors: Vec<String> = members.iter().map(|m| normalize_selector(&m.p)).collect();
    let selected: Vec<&Item> = items
        .iter()
        .filter(|it| {
            selectors.is_empty()
                || selectors
                    .iter()
                    .any(|s| s.is_empty() || selector_matches(&it.path, s))
        })
        .collect();
    if selected.is_empty() {
        let names: Vec<String> = members
            .iter()
            .map(|m| if m.dir { format!("{}/*", m.p) } else { m.p.clone() })
            .collect();
        return Err(Fail::Msg(
            format!("no files in the archive match: {}", names.join(", ")),
            0,
        ));
    }

    let root = Path::new(dest);
    let mut clashes: Vec<&str> = Vec::new();
    for it in &selected {
        let target = safe_join(root, &it.path)
            .map_err(|_| Fail::Msg(format!("refusing unsafe path: {}", it.path), 0))?;
        if fs::symlink_metadata(&target).is_ok() {
            clashes.push(&it.path);
        }
    }
    if !clashes.is_empty() && !overwrite {
        return Err(clash_fail(&clashes));
    }

    let dest_existed = root.exists();
    fs::create_dir_all(root).map_err(|e| Fail::Msg(format!("{dest}: {}", os_msg(&e)), 0))?;
    let stage = root.join(format!(".kiln-stage-{}", lock::random_name().map_err(Fail::Lock)?));
    let stage_s = stage.to_string_lossy().into_owned();
    let scratch = Scratch::new().map_err(Fail::Lock)?;
    let started = Instant::now();
    let total: u64 = selected.iter().map(|i| i.size).sum::<u64>().max(1);
    let (mut done_bytes, mut bytes) = (0u64, 0u64);
    let mut placed = 0u64;
    let mut fresh: Vec<PathBuf> = Vec::new(); // files this run created (undone on cancel)

    let result = (|| -> Result<(), Fail> {
        for it in &selected {
            if kiln::cancel::is_set() {
                return Err(Fail::Lock(LockError::Cancelled));
            }
            let archive = open_item(dir, it, &mut ring, &scratch)?;
            let _ = fs::remove_dir_all(&stage);
            let mut options = ExtractOptions::new(stage_s.as_str());
            options.overwrite = true;
            options.threads = jobs;
            let stats = extract_with_progress(&archive, &options, &mut |_, _| {}).map_err(Fail::Engine)?;
            let Some(file) = only_file(&stage) else {
                return Err(Fail::Msg(format!("{}: the archive is empty", it.path), 0));
            };
            let target = safe_join(root, &it.path)
                .map_err(|_| Fail::Msg(format!("refusing unsafe path: {}", it.path), 0))?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| Fail::Msg(format!("{}: {}", parent.display(), os_msg(&e)), 0))?;
            }
            let existed = fs::symlink_metadata(&target).is_ok();
            if existed {
                let _ = fs::remove_file(&target); // only reached with overwrite
            }
            move_file(&file, &target)
                .map_err(|e| Fail::Msg(format!("{}: {}", target.display(), os_msg(&e)), 0))?;
            placed += 1;
            if !existed {
                fresh.push(target);
            }
            bytes += stats.bytes;
            done_bytes += it.size;
            meter.tick(done_bytes, total, &it.path, false);
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&stage);
    match result {
        Ok(()) => {
            meter.tick(1, 1, "", true);
            Ok(Done { files: placed, bytes, secs: secs(started.elapsed()) })
        }
        Err(e) => {
            if matches!(&e, Fail::Lock(LockError::Cancelled))
                || matches!(&e, Fail::Engine(x) if kiln::cancel::is_cancelled(x))
            {
                roll_back(root, dest_existed, &fresh);
            }
            Err(e)
        }
    }
}

/// `test` for a set: every archive is decrypted (which checks its lock) and verified.
pub fn test(dir: &Path, password: &str, jobs: usize, meter: &mut Meter) -> Result<(u64, u64, String), Fail> {
    need_password(password)?;
    let mut ring = Ring::new(password);
    let items = read_index(dir, &mut ring)?;
    let scratch = Scratch::new().map_err(Fail::Lock)?;
    let started = Instant::now();
    let total = items.len().max(1) as u64;
    let (mut files, mut bytes) = (0u64, 0u64);
    for (k, it) in items.iter().enumerate() {
        if kiln::cancel::is_set() {
            return Err(Fail::Lock(LockError::Cancelled));
        }
        let archive = open_item(dir, it, &mut ring, &scratch)?;
        let stats = verify_with_progress(&archive, jobs, None, &mut |_, _| {}).map_err(Fail::Engine)?;
        files += stats.files;
        bytes += stats.bytes;
        meter.tick(k as u64 + 1, total, &it.path, false);
    }
    meter.tick(1, 1, "", true);
    Ok((files, bytes, secs(started.elapsed())))
}

/// `decrypt_in_place` for a set: the files come out under their real names in the same folder,
/// then the random-name archives and the index are removed. Nothing is removed unless every file
/// came out.
pub fn decrypt_in_place(
    dir: &Path,
    password: &str,
    jobs: usize,
    meter: &mut Meter,
) -> Result<Done, Fail> {
    need_password(password)?;
    let items = read_index(dir, &mut Ring::new(password))?;
    let done = extract(dir, password, &[], &dir.to_string_lossy(), true, jobs, meter)?;
    for it in &items {
        let _ = fs::remove_file(dir.join(&it.name));
    }
    let _ = fs::remove_file(index_path(dir));
    Ok(done)
}
