//! One entry of a folder.

use crate::fmt;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An ordinary folder.
    Dir,
    /// An encrypted set (a folder of archives); it opens like a folder.
    Set,
    /// A `.kiln` archive.
    Archive,
    /// Anything else.
    File,
}

impl Kind {
    /// Folders and sets sort first and open like folders.
    pub fn is_folder(self) -> bool {
        matches!(self, Self::Dir | Self::Set)
    }
}

/// What the file system says about an entry, which costs a call per entry and so arrives later
/// than the name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// Bytes; 0 for a folder.
    pub size: u64,
    /// Modification time in epoch seconds; 0 when unknown.
    pub mtime: i64,
}

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name in the folder; also what the UI shows.
    pub name: String,
    pub kind: Kind,
    /// `None` until fetched.
    pub meta: Option<Meta>,
    /// Items directly inside a folder; `None` until counted (and for files).
    pub children: Option<u32>,
}

impl Entry {
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        Self {
            name: name.into(),
            kind,
            meta: None,
            children: None,
        }
    }

    /// An entry whose metadata is already known (tests, and the engine's full scan).
    pub fn with_meta(name: impl Into<String>, kind: Kind, size: u64, mtime: i64) -> Self {
        Self {
            meta: Some(Meta { size, mtime }),
            ..Self::new(name, kind)
        }
    }

    /// Names that start with a dot are hidden.
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// The lower-case text after the last dot, or "".
    pub fn ext(&self) -> String {
        fmt::ext(&self.name)
    }

    pub fn size(&self) -> u64 {
        self.meta.map_or(0, |m| m.size)
    }

    pub fn mtime(&self) -> i64 {
        self.meta.map_or(0, |m| m.mtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_names_are_hidden() {
        assert!(Entry::new(".nomedia", Kind::File).is_hidden());
        assert!(!Entry::new("a.txt", Kind::File).is_hidden());
    }

    #[test]
    fn missing_meta_reads_as_zero() {
        let e = Entry::new("x", Kind::File);
        assert_eq!((e.size(), e.mtime()), (0, 0));
        let e = Entry::with_meta("x", Kind::File, 7, 9);
        assert_eq!((e.size(), e.mtime()), (7, 9));
    }

    #[test]
    fn only_dirs_and_sets_are_folders() {
        assert!(Kind::Dir.is_folder() && Kind::Set.is_folder());
        assert!(!Kind::Archive.is_folder() && !Kind::File.is_folder());
    }
}
