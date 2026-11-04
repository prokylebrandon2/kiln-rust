//! `pack`: one archive, or (`each`) one archive per file, plus "delete the originals" afterwards.
//!
//! The engine does the work (`kiln::pack_with_progress`) with `jobs` as its thread count (0 = every
//! core). Progress comes from the engine's callback, and from the lock layer while an encrypted
//! archive is being locked (`lock`): the engine packs a plain archive next to the destination,
//! which is then locked into place. Encrypted "one archive per file" makes a set: random file
//! names plus a locked index (`sets`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use kiln::format::index::EntryKind;
use kiln::parallel::resolve_threads;
use kiln::path::safe_join;
use kiln::walk::{self, Skipped};
use kiln::{PackOptions, Profile, pack_with_progress};
use serde_json::{Value, json};

use crate::io::Meter;
use crate::lock::{self, LockError};
use crate::sets;
use crate::text::{self, describe, fmt_size, secs};

/// What a finished pack hands back.
struct Packed {
    /// The reply (ok: true).
    value: Value,
    /// Originals that are now safely inside an archive made by this run.
    files: Vec<PathBuf>,
}

/// `pack(items_json, dest, encrypt, each, store, overwrite, password, delete_src)`.
pub struct Request {
    pub items: Vec<String>,
    pub dest: String,
    pub encrypt: bool,
    pub each: bool,
    pub store: bool,
    pub overwrite: bool,
    pub password: String,
    pub delete_src: bool,
    pub jobs: usize,
}

/// A failed or cancelled lock.
fn lock_fail(e: LockError) -> Value {
    match e {
        LockError::Cancelled => text::cancelled(),
        other => text::failed(&other.message(), 0),
    }
}

/// Files that are removed when this goes out of scope (what a failed pack leaves half-made).
struct Temps(Vec<PathBuf>);

impl Drop for Temps {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_file(p);
        }
    }
}

/// `target` with `suffix` added to its name.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn refuse(msg: &str) -> Value {
    json!({ "ok": false, "message": msg })
}

/// Run a pack and shape the reply the app expects.
pub fn pack(r: &Request) -> Value {
    if r.dest.is_empty() || r.items.is_empty() {
        return refuse("there is nothing to pack");
    }
    if r.encrypt && r.password.is_empty() {
        return refuse("enter a password"); // never write a plain archive when a lock was asked for
    }
    let existed = fs::symlink_metadata(&r.dest).is_ok();
    let outcome = if r.each { pack_each(r) } else { pack_one(r) };
    match outcome {
        Err(v) => {
            if !existed {
                remove_leftovers(&r.dest); // don't leave half-written output
            }
            v
        }
        Ok(p) => {
            let mut out = p.value;
            if r.delete_src && fs::symlink_metadata(&r.dest).is_ok() {
                // only after the whole job succeeded, and only files known to be inside an archive
                let (deleted, failed) = delete_sources(&p.files, &r.items, &r.dest);
                out["deleted"] = json!(deleted);
                out["delete_failed"] = json!(failed);
            }
            out
        }
    }
}

fn remove_leftovers(dest: &str) {
    for leftover in [
        dest.to_string(),
        format!("{dest}.tmp"),
        format!("{dest}.kilnplain"),
        format!("{dest}.kilnplain.tmp"),
    ] {
        let p = Path::new(&leftover);
        match fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => {
                let _ = fs::remove_dir_all(p);
            }
            Ok(_) => {
                let _ = fs::remove_file(p);
            }
            Err(_) => {}
        }
    }
}

fn options(r: &Request, threads: usize) -> PackOptions {
    // Adaptive = zstd with content classification: already-compressed files are stored, not
    // recompressed. "No compression" is the Store profile.
    let profile = if r.store {
        Profile::Store
    } else {
        Profile::Adaptive
    };
    let mut o = PackOptions::from_profile(profile);
    o.threads = threads;
    o.overwrite = r.overwrite || r.encrypt; // for a locked archive this is only the temporary plain one
    o
}

fn warnings(skipped: &[Skipped]) -> Vec<String> {
    skipped
        .iter()
        .map(|s| format!("skipping {}: {}", s.path.display(), s.reason))
        .collect()
}

fn percent(raw: u64, packed: u64) -> f64 {
    if raw > 0 {
        100.0 * packed as f64 / raw as f64
    } else {
        100.0
    }
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

// ---------------------------------------------------------------------------------- one archive

fn pack_one(r: &Request) -> Result<Packed, Value> {
    let inputs: Vec<PathBuf> = r.items.iter().map(PathBuf::from).collect();
    // Only when the originals are to be deleted do we list them first, to know exactly which
    // files end up inside the archive. Otherwise the engine's own walk is the only one.
    let listed: Option<Vec<PathBuf>> = if r.delete_src {
        let c =
            walk::collect(&inputs, &[PathBuf::from(&r.dest)]).map_err(|e| text::from_error(&e))?;
        Some(
            c.entries
                .iter()
                .filter(|e| e.kind == EntryKind::File)
                .map(|e| e.fs_path.clone())
                .collect(),
        )
    } else {
        None
    };

    let opts = options(r, r.jobs);
    let started = Instant::now();
    let mut meter = Meter::new();
    // A locked archive is packed plain beside its destination, then locked into place.
    let plain = format!("{}.kilnplain", r.dest);
    let lock_tmp = format!("{}.tmp", r.dest);
    let mut temps = Temps(Vec::new());
    let packed_to: &str = if r.encrypt { &plain } else { &r.dest };
    if r.encrypt {
        if fs::symlink_metadata(&r.dest).is_ok() && !r.overwrite {
            return Err(text::failed(
                &format!("{} already exists (use -f to overwrite)", r.dest),
                0,
            ));
        }
        temps.0 = vec![
            PathBuf::from(&plain),
            PathBuf::from(format!("{plain}.tmp")),
            PathBuf::from(&lock_tmp),
        ];
        meter.set_span(0.0, 0.8);
    }
    let stats = pack_with_progress(Path::new(packed_to), &inputs, &opts, &mut |done, total| {
        meter.tick(done, total, "", false)
    })
    .map_err(|e| text::from_error(&e))?;

    if stats.files == 0 && stats.dirs == 0 {
        let _ = fs::remove_file(packed_to);
        return Err(refuse("there is nothing to pack"));
    }
    let mut archive_bytes = stats.archive_bytes;
    if r.encrypt {
        meter.set_span(0.8, 0.2);
        let setup = lock::Setup::new(&r.password).map_err(lock_fail)?;
        archive_bytes = lock::seal(
            Path::new(&plain),
            Path::new(&lock_tmp),
            &setup,
            lock::KIND_ARCHIVE,
            &mut |d, t| meter.tick(d, t, "", false),
        )
        .map_err(lock_fail)?;
        fs::rename(&lock_tmp, &r.dest)
            .map_err(|e| text::failed(&format!("{}: {}", r.dest, crate::io::os_msg(&e)), 0))?;
    }
    drop(temps);
    meter.tick(1, 1, "", true);

    let threads = resolve_threads(r.jobs);
    let mut note = if r.store {
        " [no compression]".to_string()
    } else if threads > 1 {
        format!(" [{threads} threads]")
    } else {
        String::new()
    };
    if r.encrypt {
        note.push_str(" [encrypted]");
    }
    let message = format!(
        "{} file(s): {} -> {} ({:.1}% of original) in {}s{}",
        stats.files,
        fmt_size(stats.raw_bytes),
        fmt_size(archive_bytes),
        percent(stats.raw_bytes, archive_bytes),
        secs(started.elapsed()),
        note
    );
    // A file that grew while it was read was archived only up to its old size: keep it.
    let files = listed
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !stats.grew.contains(f))
        .collect();
    Ok(Packed {
        value: json!({
            "ok": true,
            "message": message,
            "done": stats.files,
            "warnings": warnings(&stats.skipped),
        }),
        files,
    })
}

// ---------------------------------------------------------------------------------- one per file

/// Why a worker stopped.
enum Stop {
    Cancelled,
    Failed(String),
}

fn pack_each(r: &Request) -> Result<Packed, Value> {
    let out_dir = Path::new(&r.dest);
    if out_dir.exists() && !out_dir.is_dir() {
        return Err(refuse(&format!(
            "{} is not a directory (with -i the first argument is the output directory)",
            r.dest
        )));
    }
    let out_abs = absolute(out_dir);
    let inputs: Vec<PathBuf> = r.items.iter().map(PathBuf::from).collect();
    let collected = walk::collect(&inputs, &[]).map_err(|e| text::from_error(&e))?;
    let files: Vec<&walk::SourceEntry> = collected
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::File && !absolute(&e.fs_path).starts_with(&out_abs))
        .collect();
    if files.is_empty() {
        return Err(refuse("nothing to pack"));
    }
    if r.encrypt && sets::index_path(out_dir).exists() {
        return Err(refuse(&format!(
            "{} already holds a Kiln set; choose another folder",
            r.dest
        )));
    }
    let warn = warnings(&collected.skipped);
    // One key derivation for the whole set (the slow part), one fresh nonce per file.
    let setup = if r.encrypt {
        Some(lock::Setup::new(&r.password).map_err(lock_fail)?)
    } else {
        None
    };

    // Each file becomes <output folder>/<its path>.kiln; one that already exists is kept.
    let mut todo: Vec<(&walk::SourceEntry, PathBuf)> = Vec::new();
    let mut skipped = 0usize;
    for &e in &files {
        if r.encrypt {
            // random names: nothing in the folder says what a file is
            let name = format!("{}.kiln", lock::random_name().map_err(lock_fail)?);
            todo.push((e, out_dir.join(name)));
            continue;
        }
        let base = safe_join(out_dir, &e.name).map_err(|er| text::from_error(&er))?;
        let mut name = base.into_os_string();
        name.push(".kiln");
        let target = PathBuf::from(name);
        if fs::symlink_metadata(&target).is_ok() && !r.overwrite {
            skipped += 1;
        } else {
            todo.push((e, target));
        }
    }
    if todo.is_empty() {
        return Ok(Packed {
            value: json!({
                "ok": true,
                "done": 0,
                "warnings": warn,
                "message": format!(
                    "nothing to do: all {skipped} output(s) already exist (use -f to overwrite)"
                ),
            }),
            files: Vec::new(),
        });
    }

    // A failed or cancelled set must not leave archives behind.
    let mut created = Temps(Vec::new());
    if r.encrypt {
        created.0 = todo.iter().map(|(_, t)| t.clone()).collect();
    }
    let total_bytes: u64 = todo.iter().map(|(e, _)| e.size).sum();
    let started = Instant::now();
    let workers = resolve_threads(r.jobs).min(todo.len());
    let mut raw = 0u64;
    let mut packed_size = 0u64;
    let mut meter = Meter::new();

    if workers > 1 {
        // Many single-file archives: spread the files over the workers instead of splitting each.
        let opts = options(r, 1);
        let next = AtomicUsize::new(0);
        let bytes = AtomicU64::new(0);
        let stop = AtomicBool::new(false);
        let sizes: Mutex<Vec<u64>> = Mutex::new(vec![0; todo.len()]);
        let failure: Mutex<Option<Stop>> = Mutex::new(None);
        let completed = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        loop {
                            if stop.load(Relaxed) || kiln::cancel::is_set() {
                                break;
                            }
                            let k = next.fetch_add(1, Relaxed);
                            let Some((e, target)) = todo.get(k) else {
                                break;
                            };
                            let res = make_archive(e, target, &opts, setup.as_ref(), |delta| {
                                bytes.fetch_add(delta, Relaxed);
                            });
                            match res {
                                Ok(size) => {
                                    if let Ok(mut s) = sizes.lock() {
                                        s[k] = size;
                                    }
                                    completed.fetch_add(1, Relaxed);
                                }
                                Err(st) => {
                                    if let Ok(mut f) = failure.lock() {
                                        // a real error beats the "cancelled" the others report after it
                                        let replace = match (&*f, &st) {
                                            (None, _) => true,
                                            (Some(Stop::Cancelled), Stop::Failed(_)) => true,
                                            _ => false,
                                        };
                                        if replace {
                                            *f = Some(st);
                                        }
                                    }
                                    stop.store(true, Relaxed);
                                    break;
                                }
                            }
                        }
                    })
                })
                .collect();
            while !handles.iter().all(|h| h.is_finished()) {
                meter.tick(bytes.load(Relaxed), total_bytes, "", false);
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        match failure.into_inner().ok().flatten() {
            Some(Stop::Failed(m)) => return Err(text::failed(&m, 0)),
            Some(Stop::Cancelled) => return Err(text::cancelled()),
            None => {}
        }
        if completed.load(Relaxed) < todo.len() {
            return Err(text::cancelled()); // stopped between archives
        }
        let sizes = sizes.into_inner().unwrap_or_default();
        for (k, (e, _)) in todo.iter().enumerate() {
            raw += e.size;
            packed_size += sizes[k];
        }
    } else {
        let mut base = 0u64; // bytes of the archives already finished
        for (e, target) in &todo {
            if kiln::cancel::is_set() {
                return Err(text::cancelled());
            }
            let opts = options(r, r.jobs);
            let size = {
                let mut last = 0u64;
                make_archive(e, target, &opts, setup.as_ref(), |delta| {
                    last += delta;
                    meter.tick(base + last, total_bytes, &e.name, false);
                })
            };
            match size {
                Ok(s) => packed_size += s,
                Err(Stop::Cancelled) => return Err(text::cancelled()),
                Err(Stop::Failed(m)) => return Err(text::failed(&m, 0)),
            }
            raw += e.size;
            base += e.size;
        }
    }
    if let Some(setup) = &setup {
        let items: Vec<sets::Item> = todo
            .iter()
            .map(|(e, t)| sets::Item {
                name: t
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: e.name.clone(),
                size: e.size,
                mtime: e.mtime_secs,
            })
            .collect();
        sets::write_index(out_dir, &items, setup).map_err(lock_fail)?;
    }
    created.0.clear(); // finished: keep what was made
    meter.tick(1, 1, "", true);

    let message = format!(
        "{} archive(s) in {}: {} -> {} ({:.1}% of original) in {}s{}{}{}",
        todo.len(),
        r.dest,
        fmt_size(raw),
        fmt_size(packed_size),
        percent(raw, packed_size),
        secs(started.elapsed()),
        if r.store { " [no compression]" } else { "" },
        if r.encrypt { " [encrypted]" } else { "" },
        if skipped > 0 {
            format!(", {skipped} skipped (already exist)")
        } else {
            String::new()
        },
    );
    Ok(Packed {
        value: json!({ "ok": true, "message": message, "done": todo.len(), "warnings": warn }),
        files: todo.iter().map(|(e, _)| e.fs_path.clone()).collect(),
    })
}

/// Make the archive for one file. `bytes` receives how many more input bytes were done.
fn make_archive(
    e: &walk::SourceEntry,
    target: &Path,
    opts: &PackOptions,
    sealing: Option<&lock::Setup>,
    mut bytes: impl FnMut(u64),
) -> Result<u64, Stop> {
    let fail = |m: String| Stop::Failed(format!("{}: {}", e.name, m));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|er| fail(format!("{}: {}", parent.display(), crate::io::os_msg(&er))))?;
    }
    let mut last = 0u64;
    // With a lock the engine writes a plain archive that is then locked into `target`.
    let plain = if sealing.is_some() {
        sibling(target, ".plain.tmp")
    } else {
        target.to_path_buf()
    };
    let _cleanup = Temps(if sealing.is_some() {
        vec![
            plain.clone(),
            sibling(&plain, ".tmp"),
            sibling(target, ".tmp"),
        ]
    } else {
        Vec::new()
    });
    let stats = pack_with_progress(
        &plain,
        std::slice::from_ref(&e.fs_path),
        opts,
        &mut |done, _| {
            bytes(done.saturating_sub(last));
            last = done;
        },
    )
    .map_err(|er| {
        if kiln::cancel::is_cancelled(&er) {
            Stop::Cancelled
        } else {
            fail(describe(&er))
        }
    })?;
    let Some(setup) = sealing else {
        return Ok(stats.archive_bytes);
    };
    let tmp = sibling(target, ".tmp");
    let size = lock::seal(&plain, &tmp, setup, lock::KIND_ARCHIVE, &mut |_, _| {}).map_err(
        |er| match er {
            LockError::Cancelled => Stop::Cancelled,
            other => fail(other.message()),
        },
    )?;
    fs::rename(&tmp, target)
        .map_err(|er| fail(format!("{}: {}", target.display(), crate::io::os_msg(&er))))?;
    Ok(size)
}

// ---------------------------------------------------------------------------------- delete originals

/// Remove the originals that are now inside a finished archive, then any folder this left empty.
/// Only the files given are touched, never the output itself. -> (deleted, failed)
fn delete_sources(packed: &[PathBuf], items: &[String], dest: &str) -> (u64, u64) {
    let out = absolute(Path::new(dest));
    let (mut deleted, mut failed) = (0u64, 0u64);
    for f in packed {
        let a = absolute(f);
        if a.starts_with(&out) {
            continue;
        }
        match fs::symlink_metadata(&a) {
            Ok(m) if m.is_file() => match fs::remove_file(&a) {
                Ok(()) => deleted += 1,
                Err(_) => failed += 1,
            },
            _ => {}
        }
    }
    for it in items {
        tidy(&absolute(Path::new(it)), &out);
    }
    (deleted, failed)
}

/// Remove `dir` and the folders below it that are empty, unless the output lives in them.
fn tidy(dir: &Path, out: &Path) {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {}
        _ => return,
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            tidy(&ent.path(), out);
        }
    }
    if dir.starts_with(out) || out.starts_with(dir) {
        return; // the output lives here
    }
    let _ = fs::remove_dir(dir); // fails (and is ignored) when something is left inside
}
