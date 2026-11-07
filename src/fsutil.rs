//! File-system helpers: robust reads, atomic output files, timestamps and permissions.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, IoContext, Result};
use crate::format::index::Xattr;

/// Read until `buf` is full or EOF; returns the number of bytes read.
pub fn read_full<R: Read + ?Sized>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// An output file that only appears at its final name once it is complete.
///
/// Data is written to a hidden temporary file beside the destination; [`TempOutput::commit`]
/// flushes it to disk and renames it into place. If the guard is dropped first - on an error or
/// a panic - the temporary file is deleted, so a failed run never leaves a truncated archive.
pub struct TempOutput {
    tmp: PathBuf,
    dest: PathBuf,
    overwrite: bool,
    committed: bool,
}

static TMP_COUNTER: AtomicU32 = AtomicU32::new(0);

impl TempOutput {
    /// Create the temporary file next to `dest`.
    ///
    /// Fails with [`Error::Exists`] when `dest` exists and `overwrite` is false.
    pub fn create(dest: &Path, overwrite: bool) -> Result<(Self, File)> {
        if !overwrite && fs::symlink_metadata(dest).is_ok() {
            return Err(Error::Exists(dest.to_path_buf()));
        }
        let name = dest
            .file_name()
            .ok_or_else(|| Error::Invalid(format!("not a file name: {}", dest.display())))?
            .to_string_lossy()
            .into_owned();
        let dir = match dest.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        for _ in 0..64 {
            let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let tmp = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Ok(file) => {
                    let guard = Self {
                        tmp,
                        dest: dest.to_path_buf(),
                        overwrite,
                        committed: false,
                    };
                    return Ok((guard, file));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(Error::Io {
                        path: tmp,
                        source: e,
                    });
                }
            }
        }
        Err(Error::Invalid(format!(
            "cannot create a temporary file in {}",
            dir.display()
        )))
    }

    /// Where the data is being written until it is committed.
    pub fn temp_path(&self) -> &Path {
        &self.tmp
    }

    /// Move a finished file to its final name without the `fsync` of [`commit`](Self::commit)
    /// (acceptable for extracted files, where a crash just means extracting again).
    ///
    /// When overwriting is not allowed, a file that appeared since [`create`](Self::create) is
    /// never replaced: the result is [`Error::Exists`] and the temporary file is removed.
    pub fn commit_fast(mut self, file: File) -> Result<()> {
        drop(file);
        if self.overwrite {
            if cfg!(windows) {
                let _ = fs::remove_file(&self.dest);
            }
            fs::rename(&self.tmp, &self.dest).at(&self.dest)?;
        } else {
            match fs::hard_link(&self.tmp, &self.dest) {
                Ok(()) => {
                    let _ = fs::remove_file(&self.tmp);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(Error::Exists(self.dest.clone()));
                }
                Err(_) => {
                    // Filesystem without hard links: check, then rename (tiny race remains).
                    if fs::symlink_metadata(&self.dest).is_ok() {
                        return Err(Error::Exists(self.dest.clone()));
                    }
                    fs::rename(&self.tmp, &self.dest).at(&self.dest)?;
                }
            }
        }
        self.committed = true;
        Ok(())
    }

    /// Flush `file` to stable storage and move it to its final name.
    pub fn commit(mut self, file: File) -> Result<()> {
        file.sync_all().at(&self.tmp)?;
        drop(file);
        if self.overwrite && cfg!(windows) {
            // Windows cannot rename over an existing file in every configuration.
            let _ = fs::remove_file(&self.dest);
        }
        fs::rename(&self.tmp, &self.dest).at(&self.dest)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.tmp);
        }
    }
}

/// Split a [`SystemTime`] into whole seconds since the Unix epoch (negative before it) and
/// a non-negative nanosecond remainder.
pub fn mtime_parts(time: SystemTime) -> (i64, u32) {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let secs = d.as_secs() as i64;
            match d.subsec_nanos() {
                0 => (-secs, 0),
                n => (-secs - 1, 1_000_000_000 - n),
            }
        }
    }
}

/// Inverse of [`mtime_parts`]; `None` if the value is not representable.
pub fn parts_to_mtime(secs: i64, nanos: u32) -> Option<SystemTime> {
    if nanos >= 1_000_000_000 {
        return None;
    }
    if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(secs.unsigned_abs()))?
            .checked_add(Duration::from_nanos(u64::from(nanos)))
    }
}

/// Set a file's modification time from archive metadata.
pub fn set_file_mtime(file: &File, secs: i64, nanos: u32) -> io::Result<()> {
    match parts_to_mtime(secs, nanos) {
        Some(t) => file.set_modified(t),
        None => Ok(()),
    }
}

/// Set a directory's modification time.
pub fn set_dir_mtime(path: &Path, secs: i64, nanos: u32) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let file = OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;

        set_file_mtime(&file, secs, nanos)
    }

    #[cfg(not(windows))]
    {
        set_file_mtime(&File::open(path)?, secs, nanos)
    }
}

/// Apply Unix permission bits. Only the lower nine bits are honoured: set-uid, set-gid and
/// sticky bits are never restored from an archive. No-op on non-Unix platforms.
pub fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode & 0o777))
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
        Ok(())
    }
}

/// Set permissions by path (used for directories and files reached by name).
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Links, ownership and extended attributes
// ---------------------------------------------------------------------------------------------

/// True for "this file system / platform has no such feature" errors.
#[cfg(unix)]
fn is_unsupported(e: &io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const NOT_SUPPORTED: i32 = 95; // EOPNOTSUPP
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "netbsd"))]
    const NOT_SUPPORTED: i32 = 45; // ENOTSUP
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd"
    )))]
    const NOT_SUPPORTED: i32 = -1;
    e.kind() == io::ErrorKind::Unsupported || e.raw_os_error() == Some(NOT_SUPPORTED)
}

/// Read every extended attribute of `path` itself (a symbolic link is not followed), sorted by
/// name. A file system without extended attributes yields an empty list; attributes that
/// disappear while being read are skipped.
#[cfg(unix)]
pub fn read_xattrs(path: &Path) -> io::Result<Vec<Xattr>> {
    use std::os::unix::ffi::OsStrExt;

    let names = match xattr::list(path) {
        Ok(names) => names,
        Err(e) if is_unsupported(&e) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for name in names {
        match xattr::get(path, &name) {
            Ok(Some(value)) => out.push(Xattr {
                name: name.as_bytes().to_vec(),
                value,
            }),
            Ok(None) => {}
            Err(e) if is_unsupported(&e) => {}
            Err(e) => return Err(e),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Extended attributes are not read on this platform.
#[cfg(not(unix))]
pub fn read_xattrs(_path: &Path) -> io::Result<Vec<Xattr>> {
    Ok(Vec::new())
}

/// Set one extended attribute on `path` itself (a symbolic link is not followed).
#[cfg(unix)]
pub fn set_xattr(path: &Path, name: &[u8], value: &[u8]) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    xattr::set(path, std::ffi::OsStr::from_bytes(name), value)
}

/// Extended attributes cannot be set on this platform.
#[cfg(not(unix))]
pub fn set_xattr(_path: &Path, _name: &[u8], _value: &[u8]) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// Set one extended attribute on an open file or directory. Unlike [`set_xattr`] this cannot be
/// redirected by a link swapped in after the file was opened.
#[cfg(unix)]
pub fn set_xattr_file(file: &File, name: &[u8], value: &[u8]) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    xattr::FileExt::set_xattr(file, std::ffi::OsStr::from_bytes(name), value)
}

/// Extended attributes cannot be set on this platform.
#[cfg(not(unix))]
pub fn set_xattr_file(_file: &File, _name: &[u8], _value: &[u8]) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// Change the owner and group of an open file.
#[cfg(unix)]
pub fn set_owner(file: &File, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::fchown(file, Some(uid), Some(gid))
}

/// Ownership cannot be set on this platform.
#[cfg(not(unix))]
pub fn set_owner(_file: &File, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// Change the owner and group of `path` itself (a symbolic link is not followed).
#[cfg(unix)]
pub fn set_owner_path(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}

/// Ownership cannot be set on this platform.
#[cfg(not(unix))]
pub fn set_owner_path(_path: &Path, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// The target of the symbolic link `path`, as raw bytes.
#[cfg(unix)]
pub fn read_link_bytes(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::unix::ffi::OsStringExt;
    Ok(fs::read_link(path)?.into_os_string().into_vec())
}

/// The target of the symbolic link `path`, as raw bytes.
#[cfg(not(unix))]
pub fn read_link_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let target = fs::read_link(path)?;
    target
        .to_str()
        .map(|t| t.as_bytes().to_vec())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "link target is not UTF-8"))
}

#[cfg(unix)]
fn make_symlink(target: &[u8], link: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(target), link)
}

#[cfg(not(unix))]
fn make_symlink(_target: &[u8], _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are only restored on Unix",
    ))
}

/// A hidden, unused name beside `dest`, for building something that is then renamed into place.
fn sibling_name(dest: &Path, suffix: &str) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dest.with_file_name(format!(".{name}.{}.{n}.{suffix}", std::process::id()))
}

/// Create `dest` as a symbolic link to `target`.
///
/// Without `overwrite` anything already at `dest` (even a dangling link) fails with
/// [`io::ErrorKind::AlreadyExists`]. With it, a non-directory at `dest` is replaced atomically
/// (the link is built under a temporary name and renamed over it). A directory is never
/// replaced.
pub fn create_symlink(target: &[u8], dest: &Path, overwrite: bool) -> io::Result<()> {
    if !overwrite {
        return make_symlink(target, dest);
    }
    for _ in 0..64 {
        let tmp = sibling_name(dest, "lnk");
        match make_symlink(target, &tmp) {
            Ok(()) => {
                return fs::rename(&tmp, dest).inspect_err(|_| {
                    let _ = fs::remove_file(&tmp);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot find an unused temporary name",
    ))
}

/// How [`create_hard_link`] satisfied a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    /// A real hard link.
    Linked,
    /// The file system refused a hard link (FAT, some network and Android shared storage), so
    /// the content was copied instead.
    Copied,
}

/// Copy `source` to a new file `dest` (which must not exist), including permissions and
/// modification time.
fn copy_new(source: &Path, dest: &Path) -> io::Result<()> {
    let meta = fs::metadata(source)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(dest)?;
    let result = io::copy(&mut input, &mut output)
        .and_then(|_| output.set_permissions(meta.permissions()))
        .and_then(|()| match meta.modified() {
            Ok(t) => output.set_modified(t),
            Err(_) => Ok(()),
        });
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(dest);
    }
    result
}

/// Make `dest` a hard link to the existing file `source`.
///
/// Without `overwrite`, an existing `dest` fails with [`io::ErrorKind::AlreadyExists`]. With it,
/// a non-directory at `dest` is replaced atomically. When the file system cannot make hard links
/// the file is copied instead and [`LinkOutcome::Copied`] says so.
pub fn create_hard_link(source: &Path, dest: &Path, overwrite: bool) -> io::Result<LinkOutcome> {
    if !overwrite {
        return match fs::hard_link(source, dest) {
            Ok(()) => Ok(LinkOutcome::Linked),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
            Err(_) => copy_new(source, dest).map(|()| LinkOutcome::Copied),
        };
    }
    for _ in 0..64 {
        let tmp = sibling_name(dest, "lnk");
        let made = match fs::hard_link(source, &tmp) {
            Ok(()) => Ok(LinkOutcome::Linked),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => copy_new(source, &tmp).map(|()| LinkOutcome::Copied),
        };
        let outcome = made?;
        let renamed = fs::rename(&tmp, dest);
        // `rename` does nothing when both names already are the same file (extracting the same
        // archive twice with --force); the temporary name must still go.
        let _ = fs::remove_file(&tmp);
        return renamed.map(|()| outcome);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot find an unused temporary name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_full_handles_short_reads() {
        struct OneByte<'a>(&'a [u8]);
        impl Read for OneByte<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                match self.0.split_first() {
                    Some((b, rest)) if !buf.is_empty() => {
                        buf[0] = *b;
                        self.0 = rest;
                        Ok(1)
                    }
                    _ => Ok(0),
                }
            }
        }
        let mut buf = [0u8; 8];
        assert_eq!(read_full(&mut OneByte(b"abcde"), &mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"abcde");
    }

    #[test]
    fn mtime_round_trips_including_before_epoch() {
        for (s, n) in [
            (0, 0),
            (1_700_000_000, 123_456_789),
            (-1, 500_000_000),
            (-86_400, 0),
        ] {
            let t = parts_to_mtime(s, n).unwrap();
            // Windows represents SystemTime in 100-nanosecond ticks, so sub-tick precision
            // is truncated when constructing the value. Compare at the platform's resolution.
            let expected_nanos = if cfg!(windows) { n / 100 * 100 } else { n };
            assert_eq!(mtime_parts(t), (s, expected_nanos));
        }
        assert!(parts_to_mtime(0, 1_000_000_000).is_none());
    }

    #[test]
    fn temp_output_commits_atomically_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");

        // Dropped without commit: nothing at the destination, no temp file left behind.
        let (guard, file) = TempOutput::create(&dest, false).unwrap();
        drop(file);
        drop(guard);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        // Commit: the destination appears.
        let (guard, mut file) = TempOutput::create(&dest, false).unwrap();
        io::Write::write_all(&mut file, b"hello").unwrap();
        guard.commit(file).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

        // Refuses to clobber unless asked.
        assert!(matches!(
            TempOutput::create(&dest, false),
            Err(Error::Exists(_))
        ));
        let (guard, file) = TempOutput::create(&dest, true).unwrap();
        guard.commit(file).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"");
    }

    #[cfg(windows)]
    #[test]
    fn directory_mtime_can_be_restored() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test_dir");
        std::fs::create_dir(&path).unwrap();

        let expected_secs = 1_700_000_000_i64;

        super::set_dir_mtime(&path, expected_secs, 0).unwrap();

        let expected = std::time::UNIX_EPOCH + std::time::Duration::from_secs(expected_secs as u64);
        let actual = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(actual, expected);
    }
}
