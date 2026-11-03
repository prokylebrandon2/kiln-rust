//! Archive member names: validation and safe mapping onto the file system.
//!
//! Names inside an archive always use `/` as the separator, are valid UTF-8 and are relative.
//! These rules are enforced both when packing and (independently) when reading an archive, so a
//! hostile archive cannot write outside the extraction directory.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Longest member name accepted, in bytes.
pub const MAX_PATH_LEN: usize = 32 * 1024;

/// Check that `name` is an acceptable archive member name.
///
/// Rejected: empty names, absolute names, `.`/`..`/empty components (this also rejects
/// trailing and doubled slashes), backslashes and NUL bytes.
pub fn validate_archive_path(name: &str) -> Result<()> {
    let bad = || Err(Error::UnsafePath(name.to_owned()));
    if name.is_empty() || name.len() > MAX_PATH_LEN {
        return bad();
    }
    if name.contains(['\0', '\\']) || name.starts_with('/') {
        return bad();
    }
    if name
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return bad();
    }
    Ok(())
}

/// Join a validated archive name onto `root`, refusing anything that is not a plain component.
///
/// This is a second line of defence behind [`validate_archive_path`]: on Windows, for example,
/// a name such as `C:evil` parses as a drive prefix and is rejected here.
pub fn safe_join(root: &Path, name: &str) -> Result<PathBuf> {
    validate_archive_path(name)?;
    let mut out = root.to_path_buf();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return Err(Error::UnsafePath(name.to_owned())),
        }
    }
    Ok(out)
}

/// Normalise a user-supplied member selector (`./a//b/` becomes `a/b`).
pub fn normalize_selector(selector: &str) -> String {
    selector
        .replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// True when `path` is exactly `selector` or lies below it as a directory.
pub fn selector_matches(path: &str, selector: &str) -> bool {
    path == selector
        || path
            .strip_prefix(selector)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for ok in [
            "a",
            "a/b",
            "dir/sub dir/file.txt",
            "ünïcode/файл",
            ".hidden",
            "a..b",
            "a/.b",
        ] {
            assert!(validate_archive_path(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_dangerous_names() {
        for bad in [
            "",
            "/",
            "/etc/passwd",
            "..",
            "../x",
            "a/../x",
            "a/..",
            ".",
            "./a",
            "a/./b",
            "a//b",
            "a/",
            "a\\b",
            "..\\x",
            "a\0b",
            "C:\\x",
        ] {
            assert!(
                validate_archive_path(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(validate_archive_path(&"x".repeat(MAX_PATH_LEN + 1)).is_err());
    }

    #[test]
    fn safe_join_stays_inside_root() {
        let root = Path::new("/tmp/out");
        assert_eq!(
            safe_join(root, "a/b.txt").unwrap(),
            Path::new("/tmp/out/a/b.txt")
        );
        assert!(safe_join(root, "../escape").is_err());
        assert!(safe_join(root, "/abs").is_err());
    }

    #[test]
    fn selectors_normalise_and_match_on_boundaries() {
        assert_eq!(normalize_selector("./a//b/"), "a/b");
        assert_eq!(normalize_selector("a\\b"), "a/b");
        assert_eq!(normalize_selector("."), "");
        assert!(selector_matches("a/b", "a"));
        assert!(selector_matches("a", "a"));
        assert!(
            !selector_matches("ab", "a"),
            "prefix must end at a component boundary"
        );
        assert!(!selector_matches("a", "a/b"));
    }
}
