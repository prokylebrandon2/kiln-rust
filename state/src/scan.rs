//! Reading a folder in two phases, with `std::fs` only.
//!
//! [`list`] reads the names and kinds: one `read_dir`, no per-file system call on most file
//! systems, so even a folder of thousands of entries is ready for its first paint almost at once.
//! [`fetch_meta`] and [`count_children`] are the slow calls (on a phone's shared storage each one
//! is a round trip); the app makes them on worker threads for the rows in view first.
//!
//! Every function blocks: call them off the UI thread.

use std::fs;
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::entry::{Entry, Kind, Meta};
use crate::fmt;

/// The name of the index file inside an encrypted set (a folder of archives).
pub const SET_INDEX: &str = "_index.kiln";

/// Names and kinds of everything in `dir`, in the folder's own order (sort with a
/// [`crate::Listing`]). Entries that are neither files nor folders (sockets, pipes) and broken
/// links are left out. A `.kiln` file is listed as an archive; whether it really is one, and
/// whether it is encrypted, is for the engine to say when it is opened.
pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let name = item.file_name().to_string_lossy().into_owned();
        let (is_dir, is_file) = if file_type.is_symlink() {
            // follow the link; a broken one is skipped
            match fs::metadata(item.path()) {
                Ok(md) => (md.is_dir(), md.is_file()),
                Err(_) => continue,
            }
        } else {
            (file_type.is_dir(), file_type.is_file())
        };
        let kind = if is_dir {
            Kind::Dir
        } else if is_file {
            if fmt::ext(&name) == "kiln" {
                Kind::Archive
            } else {
                Kind::File
            }
        } else {
            continue;
        };
        out.push(Entry::new(name, kind));
    }
    Ok(out)
}

/// Size and modification time of `dir/name`, or `None` when it cannot be read.
pub fn fetch_meta(dir: &Path, name: &str) -> Option<Meta> {
    let md = fs::metadata(dir.join(name)).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    Some(Meta {
        size: if md.is_dir() { 0 } else { md.len() },
        mtime,
    })
}

/// How many entries are directly inside the folder `dir/name`, or `None` when it cannot be read.
pub fn count_children(dir: &Path, name: &str) -> Option<u32> {
    let n = fs::read_dir(dir.join(name)).ok()?.count();
    Some(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Whether the folder `dir/name` is an encrypted set.
pub fn is_set(dir: &Path, name: &str) -> bool {
    dir.join(name).join(SET_INDEX).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("kiln-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn lists_names_and_kinds_without_metadata() {
        let d = temp("list");
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("a.txt"), b"hello").unwrap();
        fs::write(d.join("b.KILN"), b"x").unwrap();
        let mut got = list(&d).unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let view: Vec<(&str, Kind)> = got.iter().map(|e| (e.name.as_str(), e.kind)).collect();
        assert_eq!(
            view,
            [
                ("a.txt", Kind::File),
                ("b.KILN", Kind::Archive),
                ("sub", Kind::Dir)
            ]
        );
        assert!(got.iter().all(|e| e.meta.is_none() && e.children.is_none()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn metadata_and_counts_come_separately() {
        let d = temp("meta");
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("sub").join("x"), b"1").unwrap();
        fs::write(d.join("sub").join("y"), b"2").unwrap();
        fs::write(d.join("a.txt"), b"hello").unwrap();
        let m = fetch_meta(&d, "a.txt").unwrap();
        assert_eq!(m.size, 5);
        assert!(m.mtime > 0);
        assert_eq!(fetch_meta(&d, "sub").unwrap().size, 0);
        assert_eq!(count_children(&d, "sub"), Some(2));
        assert_eq!(count_children(&d, "missing"), None);
        assert_eq!(fetch_meta(&d, "missing"), None);
        assert!(!is_set(&d, "sub"));
        fs::write(d.join("sub").join(SET_INDEX), b"").unwrap();
        assert!(is_set(&d, "sub"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        assert!(list(Path::new("/definitely/not/here")).is_err());
    }
}
