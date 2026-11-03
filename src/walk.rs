//! File traversal: turn the paths given on the command line into a sorted list of entries.
//!
//! Rules (deliberately conservative for a first version):
//! * Each input is stored under its own base name, like `tar` and the original Kiln: packing
//!   `/data/photos` yields names starting with `photos/`.
//! * Regular files and directories are stored. Symbolic links and special files (sockets, FIFOs,
//!   devices) are **skipped and reported**, never silently followed.
//! * A hard I/O error (for example an unreadable directory) aborts the run rather than quietly
//!   producing an incomplete archive.
//! * Entries are sorted by name, so the archive does not depend on directory iteration order.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::error::{Error, IoContext, Result};
use crate::format::index::EntryKind;
use crate::fsutil::mtime_parts;
use crate::path::validate_archive_path;

/// A file or directory found on disk, ready to be archived.
#[derive(Clone, Debug)]
pub struct SourceEntry {
    /// Where it lives on disk.
    pub fs_path: PathBuf,
    /// The name it will have inside the archive.
    pub name: String,
    /// File or directory.
    pub kind: EntryKind,
    /// Size in bytes at the time of traversal (zero for directories).
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime_secs: i64,
    /// Modification time, nanosecond remainder.
    pub mtime_nanos: u32,
    /// Unix permission bits.
    pub mode: u32,
}

/// Something that was found but deliberately not archived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The path on disk.
    pub path: PathBuf,
    /// Why.
    pub reason: &'static str,
}

/// The result of a traversal.
#[derive(Debug, Default)]
pub struct Collected {
    /// Everything to archive, sorted by archive name.
    pub entries: Vec<SourceEntry>,
    /// Everything that was skipped, in discovery order.
    pub skipped: Vec<Skipped>,
}

/// Walk `inputs` and build the entry list. Paths in `exclude` (for example the archive being
/// written) are left out.
pub fn collect(inputs: &[PathBuf], exclude: &[PathBuf]) -> Result<Collected> {
    let mut out = Collected::default();
    let exclude: Vec<PathBuf> = exclude.iter().map(|p| absolute(p)).collect();

    for input in inputs {
        let meta = fs::metadata(input).at(input)?; // follows a symlink given explicitly
        let base = base_name(input)?;
        if meta.is_dir() {
            walk_dir(input, &base, &exclude, &mut out)?;
        } else if meta.is_file() {
            if !exclude.contains(&absolute(input)) {
                out.entries
                    .push(make_entry(input, base, &meta, EntryKind::File)?);
            }
        } else {
            out.skipped.push(Skipped {
                path: input.clone(),
                reason: "not a regular file",
            });
        }
    }

    out.entries.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(w) = out.entries.windows(2).find(|w| w[0].name == w[1].name) {
        return Err(Error::Invalid(format!(
            "two inputs would both be stored as {:?}; rename one or pack them separately",
            w[0].name
        )));
    }
    Ok(out)
}

fn walk_dir(root: &Path, base: &str, exclude: &[PathBuf], out: &mut Collected) -> Result<()> {
    for item in WalkDir::new(root).follow_links(false) {
        let item = item.map_err(|e| {
            let path = e
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf());
            match e.into_io_error() {
                Some(source) => Error::Io { path, source },
                None => Error::Invalid(format!("{}: filesystem loop", path.display())),
            }
        })?;
        if exclude.contains(&absolute(item.path())) {
            continue;
        }
        let file_type = item.file_type();
        let kind = if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            let reason = if file_type.is_symlink() {
                "symbolic link"
            } else {
                "special file"
            };
            out.skipped.push(Skipped {
                path: item.path().to_path_buf(),
                reason,
            });
            continue;
        };
        let relative = item
            .path()
            .strip_prefix(root)
            .expect("walkdir yields paths under its root");
        let mut name = base.to_owned();
        for component in relative.components() {
            name.push('/');
            name.push_str(component.as_os_str().to_str().ok_or_else(|| {
                Error::Invalid(format!(
                    "{}: file name is not valid UTF-8",
                    item.path().display()
                ))
            })?);
        }
        let meta = item.metadata().map_err(|e| {
            let path = item.path().to_path_buf();
            match e.into_io_error() {
                Some(source) => Error::Io { path, source },
                None => Error::Invalid(format!("{}: cannot read metadata", path.display())),
            }
        })?;
        out.entries
            .push(make_entry(item.path(), name, &meta, kind)?);
    }
    Ok(())
}

fn make_entry(path: &Path, name: String, meta: &Metadata, kind: EntryKind) -> Result<SourceEntry> {
    validate_archive_path(&name).map_err(|_| {
        Error::Invalid(format!(
            "{}: names containing a backslash, NUL or empty/dot components are not supported",
            path.display()
        ))
    })?;
    let (mtime_secs, mtime_nanos) = meta.modified().map(mtime_parts).unwrap_or((0, 0));
    Ok(SourceEntry {
        fs_path: path.to_path_buf(),
        name,
        kind,
        size: if kind == EntryKind::File {
            meta.len()
        } else {
            0
        },
        mtime_secs,
        mtime_nanos,
        mode: mode_of(meta, kind),
    })
}

#[cfg(unix)]
fn mode_of(meta: &Metadata, _kind: EntryKind) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_of(_meta: &Metadata, kind: EntryKind) -> u32 {
    if kind == EntryKind::Dir { 0o755 } else { 0o644 }
}

/// The name an input is stored under: its last component, resolving `.` and `..`.
fn base_name(input: &Path) -> Result<String> {
    let resolved;
    let path = match input.file_name() {
        Some(_) => input,
        None => {
            resolved = fs::canonicalize(input).at(input)?;
            &resolved
        }
    };
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("cannot derive a name from {}", input.display())))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join("empty_dir")).unwrap();
        fs::write(root.join("b.txt"), b"bbb").unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        fs::write(root.join("src/deep/x.rs"), b"fn main() {}").unwrap();
        fs::write(root.join("zero"), b"").unwrap();
        dir
    }

    fn names(c: &Collected) -> Vec<&str> {
        c.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn directory_is_stored_under_its_base_name_and_sorted() {
        let dir = tree();
        let c = collect(&[dir.path().join("proj")], &[]).unwrap();
        assert_eq!(
            names(&c),
            [
                "proj",
                "proj/a.txt",
                "proj/b.txt",
                "proj/empty_dir",
                "proj/src",
                "proj/src/deep",
                "proj/src/deep/x.rs",
                "proj/zero"
            ]
        );
        let by = |n: &str| c.entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(by("proj/b.txt").size, 3);
        assert_eq!(by("proj/b.txt").kind, EntryKind::File);
        assert_eq!(by("proj/empty_dir").kind, EntryKind::Dir);
        assert_eq!(by("proj/empty_dir").size, 0);
        assert!(by("proj/b.txt").mtime_secs > 1_000_000_000);
        assert!(c.skipped.is_empty());
    }

    #[test]
    fn single_files_and_mixed_inputs() {
        let dir = tree();
        let c = collect(
            &[dir.path().join("proj/b.txt"), dir.path().join("proj/src")],
            &[],
        )
        .unwrap();
        assert_eq!(names(&c), ["b.txt", "src", "src/deep", "src/deep/x.rs"]);
    }

    #[test]
    fn dot_resolves_to_the_real_directory_name() {
        let dir = tree();
        let proj = dir.path().join("proj");
        let c = collect(&[proj.join("src/deep/..")], &[]).unwrap();
        assert!(names(&c).contains(&"src/deep"), "{:?}", names(&c));
        let c = collect(&[proj.join(".")], &[]).unwrap();
        assert_eq!(names(&c)[0], "proj");
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let dir = tree();
        let a = dir.path().join("proj/a.txt");
        let err = collect(&[a.clone(), a], &[]).unwrap_err();
        assert!(
            matches!(err, Error::Invalid(ref m) if m.contains("a.txt")),
            "{err}"
        );
    }

    #[test]
    fn missing_input_is_an_error() {
        let dir = tree();
        assert!(matches!(
            collect(&[dir.path().join("nope")], &[]),
            Err(Error::Io { .. })
        ));
    }

    #[test]
    fn excluded_paths_are_left_out() {
        let dir = tree();
        let skip = dir.path().join("proj/a.txt");
        let c = collect(&[dir.path().join("proj")], &[skip]).unwrap();
        assert!(!names(&c).contains(&"proj/a.txt"));
        assert!(names(&c).contains(&"proj/b.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_special_files_are_skipped_and_reported() {
        let dir = tree();
        let root = dir.path().join("proj");
        std::os::unix::fs::symlink("a.txt", root.join("link")).unwrap();
        std::os::unix::fs::symlink("src", root.join("dirlink")).unwrap();
        std::os::unix::fs::symlink("/nonexistent", root.join("dangling")).unwrap();
        let c = collect(&[root], &[]).unwrap();
        let skipped: Vec<_> = c
            .skipped
            .iter()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(skipped.len(), 3, "{skipped:?}");
        assert!(c.skipped.iter().all(|s| s.reason == "symbolic link"));
        assert!(
            !names(&c)
                .iter()
                .any(|n| n.ends_with("link") || n.ends_with("dangling"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn backslash_names_are_refused_with_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a\\b"), b"x").unwrap();
        let err = collect(&[dir.path().to_path_buf()], &[]).unwrap_err();
        assert!(
            matches!(err, Error::Invalid(ref m) if m.contains("backslash")),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn permission_bits_are_recorded() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tree();
        let f = dir.path().join("proj/a.txt");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o751)).unwrap();
        let c = collect(&[f], &[]).unwrap();
        assert_eq!(c.entries[0].mode & 0o777, 0o751);
    }
}
