//! A hardened extraction directory.
//!
//! Extraction used to build a path string per file (`dest/a/b/c`), check each directory with
//! `symlink_metadata` and then create the file by that path. That refuses a symbolic link that is
//! *already there*, but there is a window between the check and the use in which someone else
//! with write access to the destination can swap a directory for a link, and the write then lands
//! wherever the link points (a classic time-of-check / time-of-use race, the same family as
//! CVE-2018-1000001-style archive extraction bugs).
//!
//! On Unix this module closes that window the way `openat`-based extractors do:
//!
//! * The destination is opened **once**, and every later operation is relative to a directory
//!   file descriptor (`openat`, `mkdirat`, `symlinkat`, `linkat`, `renameat`, `unlinkat`).
//! * Every component below the destination is opened with `O_NOFOLLOW | O_DIRECTORY`, so a link
//!   (or a file) in place of a directory is an error at the moment of use, not at some earlier
//!   check. A directory the extractor creates is re-opened the same way before use.
//! * Files are created with `O_CREAT | O_EXCL | O_NOFOLLOW`; metadata (mode, times, owner,
//!   extended attributes) is applied through the open descriptor, not through the path.
//! * Once the extractor holds a descriptor for a directory, renaming or replacing that directory
//!   in the tree no longer redirects anything: the descriptor still names the original inode.
//!
//! The destination itself may be a symbolic link (the caller chose it). Everything below it may
//! not be.
//!
//! What this does **not** do: it does not stop a hostile party from deleting or filling the
//! destination, from creating files of their own, or from racing the one place that has no
//! descriptor-relative form: extended attributes and ownership of a *symbolic link* (there is no
//! `fsetxattr` for a link). On Linux and Android those go through `/proc/self/fd/<dirfd>/<name>`,
//! which keeps them descriptor-relative; elsewhere they use the path (see `docs/HARDENING.md`).
//!
//! Other platforms keep the previous path-based behaviour (a symbolic link already in the tree
//! is refused; the race is not closed).

pub(crate) use imp::{DirHandle, Lane, Pending, Root};

#[cfg(unix)]
mod imp {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io;
    use std::os::fd::OwnedFd;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use rustix::fs::{
        AtFlags, CWD, FileType, Mode, OFlags, linkat, mkdirat, openat, renameat, statat, symlinkat,
        unlinkat,
    };
    use rustix::io::Errno;

    use crate::error::{Error, IoContext, Result};
    use crate::fsutil::{LinkOutcome, set_file_mode, set_file_mtime, set_owner, set_xattr_file};
    use crate::path::validate_archive_path;

    /// Directory descriptors one [`Lane`] keeps open. Correctness never depends on the cache; it
    /// only saves repeated walks, and it bounds the descriptors used (`lanes x this`).
    const MAX_OPEN_DIRS: usize = 32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn io_err(e: Errno) -> io::Error {
        io::Error::from(e)
    }

    fn mode(bits: u32) -> Mode {
        Mode::from_bits_truncate(bits as _)
    }

    fn dir_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }

    fn create_flags() -> OFlags {
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }

    /// A hidden, unused name beside `name`.
    fn temp_name(name: &str, suffix: &str) -> String {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(".{name}.{}.{n}.{suffix}", std::process::id())
    }

    fn map_create(shown: &Path, e: io::Error) -> Error {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::Exists(shown.to_path_buf())
        } else {
            Error::Io {
                path: shown.to_path_buf(),
                source: e,
            }
        }
    }

    /// Refuse to replace a directory with a file or link.
    fn refuse_directory(dir: &OwnedFd, name: &str, shown: &Path) -> Result<()> {
        if let Ok(st) = statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
            && FileType::from_raw_mode(st.st_mode) == FileType::Directory
        {
            return Err(Error::Invalid(format!(
                "cannot replace directory {} with a file or link",
                shown.display()
            )));
        }
        Ok(())
    }

    /// Copy `src_name` (a regular file in `src_dir`) to the new file `dst_name` in `dst_dir`,
    /// with its permissions and modification time. Used when the file system has no hard links.
    fn copy_new(
        src_dir: &OwnedFd,
        src_name: &str,
        dst_dir: &OwnedFd,
        dst_name: &str,
    ) -> io::Result<()> {
        // O_NONBLOCK: a FIFO swapped in for the source must not hang the extractor.
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let mut input =
            File::from(openat(src_dir, src_name, flags, Mode::empty()).map_err(io_err)?);
        let meta = input.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the source of a hard link is not a regular file",
            ));
        }
        let mut output =
            File::from(openat(dst_dir, dst_name, create_flags(), mode(0o666)).map_err(io_err)?);
        let result = io::copy(&mut input, &mut output)
            .and_then(|_| output.set_permissions(meta.permissions()))
            .and_then(|()| match meta.modified() {
                Ok(t) => output.set_modified(t),
                Err(_) => Ok(()),
            });
        if result.is_err() {
            drop(output);
            let _ = unlinkat(dst_dir, dst_name, AtFlags::empty());
        }
        result
    }

    /// Where a symbolic link (or a copied hard link) can be reached for the few operations that
    /// have no descriptor-relative form.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn fd_path(dir: &OwnedFd, name: &str, _shown: PathBuf) -> PathBuf {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name)
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn fd_path(_dir: &OwnedFd, _name: &str, shown: PathBuf) -> PathBuf {
        shown
    }

    /// The opened destination directory. Share it between threads; give each thread its own
    /// [`Lane`].
    pub(crate) struct Root {
        fd: OwnedFd,
        path: PathBuf,
    }

    impl Root {
        /// Open `dest`, which must be an existing directory (a symbolic link *to* a directory is
        /// accepted: the caller chose it).
        pub(crate) fn open(dest: &Path) -> Result<Self> {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
            let fd = openat(CWD, dest, flags, Mode::empty()).map_err(|e| Error::Io {
                path: dest.to_path_buf(),
                source: io_err(e),
            })?;
            Ok(Self {
                fd,
                path: dest.to_path_buf(),
            })
        }

        /// A per-thread handle with its own directory cache.
        pub(crate) fn lane(&self) -> Lane<'_> {
            Lane {
                root: self,
                dirs: HashMap::new(),
            }
        }
    }

    /// One thread's view of a [`Root`]: resolves archive-relative paths without ever following a
    /// link below the root.
    pub(crate) struct Lane<'r> {
        root: &'r Root,
        dirs: HashMap<String, OwnedFd>,
    }

    impl Lane<'_> {
        /// The path to show in messages (never used to open anything).
        fn shown(&self, rel: &str) -> PathBuf {
            if rel.is_empty() {
                self.root.path.clone()
            } else {
                self.root.path.join(rel)
            }
        }

        /// Validate `rel` and split it into its parent directory and final name.
        fn split(rel: &str) -> Result<(&str, &str)> {
            validate_archive_path(rel)?;
            Ok(rel.rsplit_once('/').unwrap_or(("", rel)))
        }

        /// Open (creating if needed) the directory `name` inside `parent`, never following a link.
        fn step(&self, parent: &OwnedFd, name: &str, so_far: &str) -> Result<OwnedFd> {
            for _ in 0..8 {
                match openat(parent, name, dir_flags(), Mode::empty()) {
                    Ok(fd) => return Ok(fd),
                    Err(Errno::NOENT) => match mkdirat(parent, name, mode(0o777)) {
                        // Another thread may have made it a moment ago; either way, open it again
                        // (without following) rather than trusting what we just did.
                        Ok(()) | Err(Errno::EXIST) => {}
                        Err(e) => {
                            return Err(Error::Io {
                                path: self.shown(so_far),
                                source: io_err(e),
                            });
                        }
                    },
                    Err(e) if e == Errno::LOOP || e == Errno::NOTDIR => {
                        let shown = self.shown(so_far);
                        return Err(match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                            Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::Symlink => {
                                Error::UnsafePath(format!(
                                    "{} is a symbolic link inside the destination; refusing to follow it",
                                    shown.display()
                                ))
                            }
                            Ok(_) => Error::Invalid(format!(
                                "{} exists and is not a directory",
                                shown.display()
                            )),
                            Err(Errno::NOENT) => continue, // vanished meanwhile: try again
                            Err(e) => Error::Io {
                                path: shown,
                                source: io_err(e),
                            },
                        });
                    }
                    Err(e) => {
                        return Err(Error::Io {
                            path: self.shown(so_far),
                            source: io_err(e),
                        });
                    }
                }
            }
            Err(Error::UnsafePath(format!(
                "{} keeps changing while it is being opened",
                self.shown(so_far).display()
            )))
        }

        /// Walk `rel` (a directory, `""` for the root) one component at a time.
        fn walk(&self, rel: &str) -> Result<OwnedFd> {
            let mut cur = self.root.fd.try_clone().at(&self.root.path)?;
            let mut so_far = String::new();
            for part in rel.split('/').filter(|p| !p.is_empty()) {
                if !so_far.is_empty() {
                    so_far.push('/');
                }
                so_far.push_str(part);
                cur = self.step(&cur, part, &so_far)?;
            }
            Ok(cur)
        }

        /// The cached descriptor of directory `rel`, opening it if needed.
        fn dir_fd(&mut self, rel: &str) -> Result<&OwnedFd> {
            if !self.dirs.contains_key(rel) {
                let fd = self.walk(rel)?;
                if self.dirs.len() >= MAX_OPEN_DIRS {
                    self.dirs.clear();
                }
                self.dirs.insert(rel.to_owned(), fd);
            }
            Ok(&self.dirs[rel])
        }

        /// Create the file `rel` (parents are created) for writing.
        ///
        /// With `via_temp` the data goes to a hidden temporary name in the same directory and
        /// [`Pending::commit`] moves it into place; without it the final name is created directly
        /// and exclusively, and removed again if the [`Pending`] is dropped uncommitted.
        /// Fails with [`Error::Exists`] when the name is taken and `overwrite` is false.
        pub(crate) fn create(
            &mut self,
            rel: &str,
            overwrite: bool,
            via_temp: bool,
        ) -> Result<(Pending, File)> {
            let (parent, name) = Self::split(rel)?;
            let shown = self.shown(rel);
            let dir = self.dir_fd(parent)?.try_clone().at(&shown)?;
            if overwrite {
                refuse_directory(&dir, name, &shown)?;
            }
            if !via_temp {
                let fd = openat(&dir, name, create_flags(), mode(0o666))
                    .map_err(|e| map_create(&shown, io_err(e)))?;
                let pending = Pending {
                    dir,
                    name: name.to_owned(),
                    tmp: None,
                    overwrite,
                    temp_shown: shown.clone(),
                    shown,
                    done: false,
                };
                return Ok((pending, File::from(fd)));
            }
            if !overwrite && statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
                return Err(Error::Exists(shown));
            }
            for _ in 0..64 {
                let tmp = temp_name(name, "tmp");
                match openat(&dir, tmp.as_str(), create_flags(), mode(0o666)) {
                    Ok(fd) => {
                        let pending = Pending {
                            dir,
                            name: name.to_owned(),
                            temp_shown: shown.with_file_name(&tmp),
                            tmp: Some(tmp),
                            overwrite,
                            shown,
                            done: false,
                        };
                        return Ok((pending, File::from(fd)));
                    }
                    Err(Errno::EXIST) => {}
                    Err(e) => {
                        return Err(Error::Io {
                            path: shown.with_file_name(&tmp),
                            source: io_err(e),
                        });
                    }
                }
            }
            Err(Error::Invalid(format!(
                "cannot create a temporary file beside {}",
                shown.display()
            )))
        }

        /// Open an extracted file for reading (never through a link, never blocking on a FIFO).
        pub(crate) fn open_read(&mut self, rel: &str) -> Result<File> {
            let (parent, name) = Self::split(rel)?;
            let shown = self.shown(rel);
            let dir = self.dir_fd(parent)?;
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let file =
                File::from(
                    openat(dir, name, flags, Mode::empty()).map_err(|e| Error::Io {
                        path: shown.clone(),
                        source: io_err(e),
                    })?,
                );
            if !file.metadata().at(&shown)?.is_file() {
                return Err(Error::Invalid(format!(
                    "{} is not a regular file",
                    shown.display()
                )));
            }
            Ok(file)
        }

        /// Open (creating if needed) the directory `rel` to apply its metadata.
        pub(crate) fn open_dir(&mut self, rel: &str) -> Result<DirHandle> {
            Self::split(rel)?;
            let shown = self.shown(rel);
            let fd = self.dir_fd(rel)?.try_clone().at(&shown)?;
            Ok(DirHandle(File::from(fd)))
        }

        /// Create `rel` as a symbolic link to `target` (stored bytes, not interpreted).
        ///
        /// Without `overwrite` anything already there (even a dangling link) is
        /// [`Error::Exists`]; with it a non-directory is replaced atomically.
        pub(crate) fn symlink(&mut self, rel: &str, target: &[u8], overwrite: bool) -> Result<()> {
            let (parent, name) = Self::split(rel)?;
            let shown = self.shown(rel);
            let dir = self.dir_fd(parent)?.try_clone().at(&shown)?;
            if overwrite {
                refuse_directory(&dir, name, &shown)?;
            }
            if !overwrite {
                return symlinkat(target, &dir, name).map_err(|e| map_create(&shown, io_err(e)));
            }
            for _ in 0..64 {
                let tmp = temp_name(name, "lnk");
                match symlinkat(target, &dir, tmp.as_str()) {
                    Ok(()) => {
                        return renameat(&dir, tmp.as_str(), &dir, name).map_err(|e| {
                            let _ = unlinkat(&dir, tmp.as_str(), AtFlags::empty());
                            map_create(&shown, io_err(e))
                        });
                    }
                    Err(Errno::EXIST) => {}
                    Err(e) => return Err(map_create(&shown, io_err(e))),
                }
            }
            Err(Error::Invalid(format!(
                "cannot find an unused temporary name beside {}",
                shown.display()
            )))
        }

        /// Make `to` a hard link to the already extracted file `from`; copies it when the file
        /// system has no hard links ([`LinkOutcome::Copied`]).
        pub(crate) fn hard_link(
            &mut self,
            from: &str,
            to: &str,
            overwrite: bool,
        ) -> Result<LinkOutcome> {
            let (from_parent, from_name) = Self::split(from)?;
            let (to_parent, to_name) = Self::split(to)?;
            let (from_shown, shown) = (self.shown(from), self.shown(to));
            let src = self.dir_fd(from_parent)?.try_clone().at(&from_shown)?;
            let dst = self.dir_fd(to_parent)?.try_clone().at(&shown)?;
            if overwrite {
                refuse_directory(&dst, to_name, &shown)?;
            }
            if !overwrite {
                return match linkat(&src, from_name, &dst, to_name, AtFlags::empty()) {
                    Ok(()) => Ok(LinkOutcome::Linked),
                    Err(Errno::EXIST) => Err(Error::Exists(shown)),
                    Err(_) => copy_new(&src, from_name, &dst, to_name)
                        .map(|()| LinkOutcome::Copied)
                        .map_err(|e| map_create(&shown, e)),
                };
            }
            for _ in 0..64 {
                let tmp = temp_name(to_name, "lnk");
                let made = match linkat(&src, from_name, &dst, tmp.as_str(), AtFlags::empty()) {
                    Ok(()) => Ok(LinkOutcome::Linked),
                    Err(Errno::EXIST) => continue,
                    Err(_) => {
                        copy_new(&src, from_name, &dst, tmp.as_str()).map(|()| LinkOutcome::Copied)
                    }
                };
                let outcome = made.map_err(|e| map_create(&shown, e))?;
                let renamed = renameat(&dst, tmp.as_str(), &dst, to_name);
                // `rename` does nothing when both names already are the same file (extracting the
                // same archive twice with --force); the temporary name must still go.
                let _ = unlinkat(&dst, tmp.as_str(), AtFlags::empty());
                return renamed
                    .map(|()| outcome)
                    .map_err(|e| map_create(&shown, io_err(e)));
            }
            Err(Error::Invalid(format!(
                "cannot find an unused temporary name beside {}",
                shown.display()
            )))
        }

        /// A path through which the entry `rel` (a symbolic link, or a file reached by name) can be
        /// handed to the few calls with no descriptor-relative form (`lchown`, extended
        /// attributes). On Linux and Android it runs through the open parent directory; elsewhere
        /// it is the ordinary path.
        pub(crate) fn stable_path(&mut self, rel: &str) -> Result<PathBuf> {
            let (parent, name) = Self::split(rel)?;
            let shown = self.shown(rel);
            let dir = self.dir_fd(parent)?;
            Ok(fd_path(dir, name, shown))
        }
    }

    /// A file being written. Dropping it without [`commit`](Self::commit) removes what was
    /// created, so a failed run leaves no half-written file under a real name (a crash in the
    /// middle of a direct write can still leave one).
    pub(crate) struct Pending {
        dir: OwnedFd,
        name: String,
        tmp: Option<String>,
        overwrite: bool,
        shown: PathBuf,
        temp_shown: PathBuf,
        done: bool,
    }

    impl Pending {
        /// Where the data is being written (for error messages).
        pub(crate) fn temp_path(&self) -> &Path {
            &self.temp_shown
        }

        /// Finish: move a temporary file to its final name. When overwriting is not allowed a name
        /// that appeared since [`Lane::create`] is never replaced ([`Error::Exists`]).
        pub(crate) fn commit(mut self, file: File) -> Result<()> {
            drop(file);
            let Some(tmp) = self.tmp.clone() else {
                self.done = true;
                return Ok(());
            };
            let (dir, name) = (&self.dir, self.name.as_str());
            if self.overwrite {
                renameat(dir, tmp.as_str(), dir, name).map_err(|e| Error::Io {
                    path: self.shown.clone(),
                    source: io_err(e),
                })?;
            } else {
                match linkat(dir, tmp.as_str(), dir, name, AtFlags::empty()) {
                    Ok(()) => {
                        let _ = unlinkat(dir, tmp.as_str(), AtFlags::empty());
                    }
                    Err(Errno::EXIST) => return Err(Error::Exists(self.shown.clone())),
                    Err(_) => {
                        // A file system without hard links: check, then rename (a tiny race
                        // remains, as before).
                        if statat(dir, name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
                            return Err(Error::Exists(self.shown.clone()));
                        }
                        renameat(dir, tmp.as_str(), dir, name).map_err(|e| Error::Io {
                            path: self.shown.clone(),
                            source: io_err(e),
                        })?;
                    }
                }
            }
            self.done = true;
            Ok(())
        }
    }

    impl Drop for Pending {
        fn drop(&mut self) {
            if !self.done {
                let victim = self.tmp.as_deref().unwrap_or(&self.name);
                let _ = unlinkat(&self.dir, victim, AtFlags::empty());
            }
        }
    }

    /// An open extracted directory; metadata is applied through the descriptor.
    pub(crate) struct DirHandle(File);

    impl DirHandle {
        pub(crate) fn set_mtime(&self, secs: i64, nanos: u32) -> io::Result<()> {
            set_file_mtime(&self.0, secs, nanos)
        }

        pub(crate) fn set_mode(&self, bits: u32) -> io::Result<()> {
            set_file_mode(&self.0, bits)
        }

        pub(crate) fn set_owner(&self, uid: u32, gid: u32) -> io::Result<()> {
            set_owner(&self.0, uid, gid)
        }

        pub(crate) fn set_xattr(&self, name: &[u8], value: &[u8]) -> io::Result<()> {
            set_xattr_file(&self.0, name, value)
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::collections::HashSet;
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::path::{Path, PathBuf};

    use crate::error::{Error, IoContext, Result};
    use crate::fsutil::{
        LinkOutcome, TempOutput, create_hard_link, create_symlink, set_dir_mtime, set_mode,
        set_owner_path, set_xattr,
    };
    use crate::path::safe_join;

    /// The destination directory (path-based on this platform).
    pub(crate) struct Root {
        path: PathBuf,
    }

    impl Root {
        pub(crate) fn open(dest: &Path) -> Result<Self> {
            if !fs::metadata(dest).at(dest)?.is_dir() {
                return Err(Error::Invalid(format!(
                    "{} is not a directory",
                    dest.display()
                )));
            }
            Ok(Self {
                path: dest.to_path_buf(),
            })
        }

        pub(crate) fn lane(&self) -> Lane<'_> {
            Lane {
                root: self,
                known: HashSet::new(),
            }
        }
    }

    /// Creates output directories and remembers which ones it has already checked. An existing
    /// symbolic link inside the destination tree is refused instead of followed. (There is no
    /// descriptor-relative API here, so a link swapped in after the check is not caught.)
    pub(crate) struct Lane<'r> {
        root: &'r Root,
        known: HashSet<PathBuf>,
    }

    fn map_create(path: &Path, e: io::Error) -> Error {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::Exists(path.to_path_buf())
        } else {
            Error::Io {
                path: path.to_path_buf(),
                source: e,
            }
        }
    }

    fn refuse_directory(path: &Path, overwrite: bool) -> Result<()> {
        if overwrite && fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            return Err(Error::Invalid(format!(
                "cannot replace directory {} with a file or link",
                path.display()
            )));
        }
        Ok(())
    }

    impl Lane<'_> {
        fn ensure(&mut self, dir: &Path) -> Result<()> {
            let root = &self.root.path;
            if dir == root.as_path() || self.known.contains(dir) {
                return Ok(());
            }
            let mut missing: Vec<&Path> = Vec::new();
            let mut cur = dir;
            loop {
                if cur == root.as_path() || self.known.contains(cur) {
                    break;
                }
                match fs::symlink_metadata(cur) {
                    Ok(m) if m.is_dir() => {
                        self.known.insert(cur.to_path_buf());
                        break;
                    }
                    Ok(m) if m.file_type().is_symlink() => {
                        return Err(Error::UnsafePath(format!(
                            "{} is a symbolic link inside the destination; refusing to follow it",
                            cur.display()
                        )));
                    }
                    Ok(_) => {
                        return Err(Error::Invalid(format!(
                            "{} exists and is not a directory",
                            cur.display()
                        )));
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        missing.push(cur);
                        cur = cur.parent().ok_or_else(|| {
                            Error::Internal("directory outside the destination".into())
                        })?;
                    }
                    Err(e) => {
                        return Err(Error::Io {
                            path: cur.to_path_buf(),
                            source: e,
                        });
                    }
                }
            }
            for d in missing.into_iter().rev() {
                match fs::create_dir(d) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        if !fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()) {
                            return Err(Error::UnsafePath(format!(
                                "{} is not a plain directory",
                                d.display()
                            )));
                        }
                    }
                    Err(e) => {
                        return Err(Error::Io {
                            path: d.to_path_buf(),
                            source: e,
                        });
                    }
                }
                self.known.insert(d.to_path_buf());
            }
            if self.known.len() > 200_000 {
                self.known.clear();
            }
            Ok(())
        }

        /// The full path of `rel`, with its parent directories in place.
        fn prepare(&mut self, rel: &str) -> Result<PathBuf> {
            let path = safe_join(&self.root.path, rel)?;
            if let Some(parent) = path.parent() {
                self.ensure(parent)?;
            }
            Ok(path)
        }

        pub(crate) fn create(
            &mut self,
            rel: &str,
            overwrite: bool,
            via_temp: bool,
        ) -> Result<(Pending, File)> {
            let path = self.prepare(rel)?;
            refuse_directory(&path, overwrite)?;
            if via_temp {
                let (temp, file) = TempOutput::create(&path, overwrite)?;
                let pending = Pending {
                    temp: Some(temp),
                    path,
                    done: false,
                };
                return Ok((pending, file));
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| map_create(&path, e))?;
            Ok((
                Pending {
                    temp: None,
                    path,
                    done: false,
                },
                file,
            ))
        }

        pub(crate) fn open_read(&mut self, rel: &str) -> Result<File> {
            let path = self.prepare(rel)?;
            File::open(&path).at(&path)
        }

        pub(crate) fn open_dir(&mut self, rel: &str) -> Result<DirHandle> {
            let path = safe_join(&self.root.path, rel)?;
            self.ensure(&path)?;
            Ok(DirHandle(path))
        }

        pub(crate) fn symlink(&mut self, rel: &str, target: &[u8], overwrite: bool) -> Result<()> {
            let path = self.prepare(rel)?;
            refuse_directory(&path, overwrite)?;
            create_symlink(target, &path, overwrite).map_err(|e| map_create(&path, e))
        }

        pub(crate) fn hard_link(
            &mut self,
            from: &str,
            to: &str,
            overwrite: bool,
        ) -> Result<LinkOutcome> {
            let source = safe_join(&self.root.path, from)?;
            let path = self.prepare(to)?;
            refuse_directory(&path, overwrite)?;
            create_hard_link(&source, &path, overwrite).map_err(|e| map_create(&path, e))
        }

        pub(crate) fn stable_path(&mut self, rel: &str) -> Result<PathBuf> {
            safe_join(&self.root.path, rel)
        }
    }

    /// A file being written (see the Unix version).
    pub(crate) struct Pending {
        temp: Option<TempOutput>,
        path: PathBuf,
        done: bool,
    }

    impl Pending {
        pub(crate) fn temp_path(&self) -> &Path {
            match &self.temp {
                Some(t) => t.temp_path(),
                None => &self.path,
            }
        }

        pub(crate) fn commit(mut self, file: File) -> Result<()> {
            match self.temp.take() {
                Some(temp) => {
                    self.done = true;
                    temp.commit_fast(file)
                }
                None => {
                    drop(file);
                    self.done = true;
                    Ok(())
                }
            }
        }
    }

    impl Drop for Pending {
        fn drop(&mut self) {
            if !self.done && self.temp.is_none() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    /// A directory reached by path (there are no directory descriptors here).
    pub(crate) struct DirHandle(PathBuf);

    impl DirHandle {
        pub(crate) fn set_mtime(&self, secs: i64, nanos: u32) -> io::Result<()> {
            set_dir_mtime(&self.0, secs, nanos)
        }

        pub(crate) fn set_mode(&self, bits: u32) -> io::Result<()> {
            set_mode(&self.0, bits)
        }

        pub(crate) fn set_owner(&self, uid: u32, gid: u32) -> io::Result<()> {
            set_owner_path(&self.0, uid, gid)
        }

        pub(crate) fn set_xattr(&self, name: &[u8], value: &[u8]) -> io::Result<()> {
            set_xattr(&self.0, name, value)
        }
    }
}
