//! File-system helpers: robust reads, atomic output files, timestamps and permissions.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, IoContext, Result};

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

/// Set a directory's modification time (best effort on platforms that cannot open directories).
pub fn set_dir_mtime(path: &Path, secs: i64, nanos: u32) -> io::Result<()> {
    set_file_mtime(&File::open(path)?, secs, nanos)
}

/// Apply Unix permission bits. Only the lower nine bits are honoured: set-uid, set-gid and
/// sticky bits are never restored from an archive. No-op on non-Unix platforms.
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
            assert_eq!(mtime_parts(t), (s, n));
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
}
