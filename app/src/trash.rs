//! The recycle bin. Each storage (Internal storage, an SD card or a flash drive)
//! keeps its own bin in a hidden folder at its top, so putting something in is a rename on the
//! same volume: instant, whatever the size, and it needs no extra space. An item is stored as
//! "<id>" (the file or folder itself, renamed) plus "<id>.json" saying what it was called, where
//! it came from and when it was deleted.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::fmt::io_text;

/// The bin's folder, at the top of a storage.
pub(crate) const DIR: &str = ".KilnTrash";

/// One thing in a bin.
#[derive(Clone)]
pub(crate) struct Entry {
    /// The item itself, inside the bin.
    pub data: PathBuf,
    /// Its description (the `.json` file).
    pub meta: PathBuf,
    /// The name it had.
    pub name: String,
    /// The full path it came from.
    pub orig: String,
    /// When it was deleted, in epoch seconds.
    pub when: i64,
    pub dir: bool,
    /// Bytes, once counted with [`size`].
    pub size: Option<u64>,
    /// The storage this bin belongs to.
    pub root: PathBuf,
}

// ---- where an item can go -----------------------------------------------------------------

/// The storage (one of `roots`) that holds `path`, or `None` if none does or
/// `path` is a bin or a storage itself.
pub(crate) fn root_of(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let p = path.to_string_lossy();
    let mut best: Option<&PathBuf> = None;
    for root in roots {
        let rp = root.to_string_lossy();
        if p == rp {
            // a whole storage can't be binned
            return None;
        }
        let inside = p.starts_with(&format!("{rp}/"));
        if inside && best.is_none_or(|b| rp.len() > b.to_string_lossy().len()) {
            best = Some(root);
        }
    }
    let best = best?;
    let rel = &p[best.to_string_lossy().len() + 1..];
    let in_bin = rel
        .strip_prefix(DIR)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
    if in_bin {
        return None;
    }
    Some(best.clone())
}

// ---- put in -------------------------------------------------------------------------------

/// Move `path` into the bin of `root`. `Err` says what went wrong.
pub(crate) fn put(path: &Path, root: &Path) -> Result<(), String> {
    let bin = root.join(DIR);
    if !bin.is_dir() && fs::create_dir_all(&bin).is_err() {
        return Err("can't create the recycle bin".to_string());
    }
    // keeps gallery apps out of it
    let _ = fs::File::create_new(bin.join(".nomedia"));
    let (data, meta) = free_slot(&bin);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let note = json!({
        "name": name,
        "orig": path.to_string_lossy(),
        "when": since_epoch().as_secs(),
        "dir": path.is_dir(),
    });
    if let Err(e) = fs::write(&meta, note.to_string()) {
        let _ = fs::remove_file(&meta);
        return Err(format!("can't write to the recycle bin: {}", io_text(&e)));
    }
    if fs::rename(path, &data).is_err() {
        let _ = fs::remove_file(&meta);
        return Err(format!("couldn't move \"{name}\" to the recycle bin"));
    }
    Ok(())
}

/// The paths of an item and of its note, under an id that nothing in `bin` uses yet.
fn free_slot(bin: &Path) -> (PathBuf, PathBuf) {
    loop {
        let id = new_id();
        let data = bin.join(&id);
        let meta = bin.join(format!("{id}.json"));
        if !data.exists() && !meta.exists() {
            return (data, meta);
        }
    }
}

/// The time in base 36 followed by a random number in base 36.
fn new_id() -> String {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let now = since_epoch();
    let millis = now.as_secs() * 1000 + u64::from(now.subsec_millis());
    let mix = COUNT
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(2_654_435_761);
    let salt = now.subsec_nanos().wrapping_add(mix) % (1 << 20);
    format!("{}{}", base36(millis), base36(u64::from(salt)))
}

fn since_epoch() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

/// `Long.toString(n, 36)`.
fn base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(char::from(DIGITS[(n % 36) as usize]));
        n /= 36;
    }
    out.iter().rev().collect()
}

// ---- look inside --------------------------------------------------------------------------

/// Everything in the bin of `root`, newest first. Sizes are not counted yet.
pub(crate) fn list(root: &Path) -> Vec<Entry> {
    let bin = root.join(DIR);
    let Ok(kids) = fs::read_dir(&bin) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kid in kids.flatten() {
        let file_name = kid.file_name();
        let Some(stem) = file_name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        let data = bin.join(stem);
        let meta = kid.path();
        if !data.exists() {
            // its item is gone: drop the stale note
            let _ = fs::remove_file(&meta);
            continue;
        }
        if let Some(entry) = read_entry(root, data, meta) {
            out.push(entry);
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.when));
    out
}

/// One note of a bin, read as far as 64 KiB; `None` when it isn't JSON.
fn read_entry(root: &Path, data: PathBuf, meta: PathBuf) -> Option<Entry> {
    let mut bytes = Vec::new();
    fs::File::open(&meta)
        .ok()?
        .take(1 << 16)
        .read_to_end(&mut bytes)
        .ok()?;
    let note: Value = serde_json::from_slice(&bytes).ok()?;
    let text = |key: &str| note[key].as_str().map(str::to_string);
    let fallback = data.file_name().unwrap_or_default().to_string_lossy();
    Some(Entry {
        name: text("name").unwrap_or_else(|| fallback.to_string()),
        orig: text("orig").unwrap_or_default(),
        when: note["when"].as_i64().unwrap_or(0),
        dir: note["dir"].as_bool().unwrap_or_else(|| data.is_dir()),
        size: None,
        root: root.to_path_buf(),
        data,
        meta,
    })
}

/// Total bytes of a file or folder (links are not followed).
pub(crate) fn size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        let Ok(kids) = fs::read_dir(path) else {
            return 0;
        };
        kids.flatten().map(|k| size(&k.path())).sum()
    } else if meta.is_symlink() {
        0
    } else {
        meta.len()
    }
}

// ---- take out -----------------------------------------------------------------------------

/// Put an item back where it came from, under a new name if that is taken.
/// Returns the new path, or `None` on failure.
pub(crate) fn restore(entry: &Entry) -> Option<PathBuf> {
    let mut target = PathBuf::from(&entry.orig);
    let parent = target.parent()?.to_path_buf();
    if !parent.is_dir() && fs::create_dir_all(&parent).is_err() {
        return None;
    }
    if fs::symlink_metadata(&target).is_ok() {
        let name = &entry.name;
        let dot = if entry.dir {
            None
        } else {
            name.rfind('.').filter(|&d| d > 0)
        };
        let (stem, ext) = match dot {
            Some(d) => (&name[..d], &name[d..]),
            None => (name.as_str(), ""),
        };
        let mut i = 1;
        loop {
            let tag = if i == 1 {
                " (restored)".to_string()
            } else {
                format!(" (restored {i})")
            };
            target = parent.join(format!("{stem}{tag}{ext}"));
            i += 1;
            if !target.exists() {
                break;
            }
        }
    }
    fs::rename(&entry.data, &target).ok()?;
    let _ = fs::remove_file(&entry.meta);
    Some(target)
}

/// Delete an item for good.
pub(crate) fn purge(entry: &Entry) -> bool {
    let ok = delete_tree(&entry.data);
    if ok {
        let _ = fs::remove_file(&entry.meta);
    }
    ok
}

/// A folder and everything in it; a link is removed, not followed.
fn delete_tree(path: &Path) -> bool {
    let is_dir = fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
    if is_dir {
        // an unreadable folder just has no children to delete
        let kids = fs::read_dir(path).into_iter().flatten().flatten();
        for kid in kids {
            delete_tree(&kid.path());
        }
    }
    let removed = if is_dir {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    };
    removed.is_ok() || !path.exists()
}
