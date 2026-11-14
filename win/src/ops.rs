//! Whole-archive operations behind the graphical helper. Each one returns an [`Outcome`] instead
//! of printing, so a window can show it. ZIP and `.kiln` are told apart the way the command line
//! does it.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use kiln::cancel;
use kiln::error::Error;
use kiln::{Archive, ExtractOptions, PackOptions, Profile};

use crate::zip::{self, CreateOpts, ExtractOpts, ZipArchive};

/// How an operation ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Finished. The text, if there is one, is worth showing (a test result, skipped items).
    Done(Option<String>),
    /// Cancel was pressed.
    Cancelled,
    /// A file that would have been replaced is in the way (its path); nothing was replaced.
    Exists(String),
    /// Anything else, in words.
    Failed(String),
}

impl From<Error> for Outcome {
    fn from(e: Error) -> Self {
        // After a Cancel any error is the cancel: a reader reports it as a plain I/O error.
        if cancel::is_cancelled(&e) || cancel::is_set() {
            Outcome::Cancelled
        } else if let Error::Exists(path) = &e {
            Outcome::Exists(path.display().to_string())
        } else {
            Outcome::Failed(e.to_string())
        }
    }
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// True for a ZIP: the file starts with `PK`, or its name ends in `.zip`.
fn is_zip(path: &Path) -> Result<bool, Error> {
    let mut magic = [0u8; 2];
    let mut file = File::open(path).map_err(io_err(path))?;
    let n = file.read(&mut magic).map_err(io_err(path))?;
    let zip_ext = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    Ok((n == 2 && &magic == b"PK") || zip_ext)
}

/// True when the name ends in `.kiln`.
pub fn is_kiln_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("kiln"))
}

/// A size for people: `12.3 MiB`.
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// `path`, or `name_2.ext`, `name_3.ext`... when something of that name already exists.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let dir = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2u32..)
        .map(|n| dir.join(format!("{stem}_{n}{ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Where "Add to" puts the archive: next to `input`, named after it, never over another file.
pub fn archive_path_for(input: &Path, ext: &str) -> PathBuf {
    let name = if input.is_dir() {
        input.file_name()
    } else {
        input.file_stem()
    };
    let name = name
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string());
    let dir = input.parent().map(Path::to_path_buf).unwrap_or_default();
    unique_path(&dir.join(format!("{name}.{ext}")))
}

/// Where "Extract Here" (the archive's folder) or "Extract to subfolder" (a folder named after
/// the archive) puts the files.
pub fn extract_target(archive: &Path, subfolder: bool) -> PathBuf {
    let dir = archive.parent().map(Path::to_path_buf).unwrap_or_default();
    match archive.file_stem() {
        Some(stem) if subfolder => dir.join(stem),
        _ => dir,
    }
}

/// Unpack everything in `archive` into `dest`.
pub fn extract(
    archive: &Path,
    dest: &Path,
    overwrite: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> Outcome {
    match extract_inner(archive, dest, overwrite, progress) {
        Ok(()) => Outcome::Done(None),
        Err(e) => e.into(),
    }
}

fn extract_inner(
    archive: &Path,
    dest: &Path,
    overwrite: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), Error> {
    if is_zip(archive)? {
        let zip = ZipArchive::open(archive)?;
        let options = ExtractOpts {
            dest: dest.to_path_buf(),
            overwrite,
            threads: 0,
            members: Vec::new(),
        };
        zip.extract(&options, progress)?;
    } else {
        let arc = Archive::open(archive)?;
        let mut options = ExtractOptions::new(dest.to_path_buf());
        options.overwrite = overwrite;
        // Windows cannot restore symbolic links: skip them instead of failing.
        options.links = false;
        kiln::extract_with_progress(&arc, &options, progress)?;
    }
    Ok(())
}

/// Read every byte of `archive` and check it. A `.kiln` is verified without writing anything; a
/// ZIP is unpacked into a scratch folder that is removed again (there is no read-only ZIP check
/// yet, so that needs room for the unpacked files).
pub fn test(archive: &Path, progress: &mut dyn FnMut(u64, u64)) -> Outcome {
    match test_inner(archive, progress) {
        Ok(text) => Outcome::Done(Some(text)),
        Err(e) => e.into(),
    }
}

fn test_inner(archive: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<String, Error> {
    let (files, dirs, bytes) = if is_zip(archive)? {
        let zip = ZipArchive::open(archive)?;
        let scratch = tempfile::tempdir()?;
        let options = ExtractOpts {
            dest: scratch.path().to_path_buf(),
            overwrite: true,
            threads: 0,
            members: Vec::new(),
        };
        let stats = zip.extract(&options, progress)?;
        (stats.files, stats.dirs, stats.bytes)
    } else {
        let arc = Archive::open(archive)?;
        let stats = kiln::verify_with_progress(&arc, 0, None, progress)?;
        (stats.files, stats.dirs, stats.bytes)
    };
    Ok(format!(
        "Everything is Ok\n\n{files} files, {dirs} folders, {}",
        fmt_bytes(bytes)
    ))
}

/// Pack `input` (a file or a folder) into `output`: a `.kiln` if the name says so, else a ZIP.
pub fn add(output: &Path, input: &Path, progress: &mut dyn FnMut(u64, u64)) -> Outcome {
    match add_inner(output, input, progress) {
        Ok(text) => Outcome::Done(text),
        Err(e) => e.into(),
    }
}

fn add_inner(
    output: &Path,
    input: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Option<String>, Error> {
    let inputs = [input.to_path_buf()];
    let skipped: Vec<String> = if is_kiln_name(output) {
        let options = PackOptions::from_profile(Profile::Adaptive);
        let stats = kiln::pack_with_progress(output, &inputs[..], &options, progress)?;
        stats.skipped.iter().map(|s| format!("{s:?}")).collect()
    } else {
        let options = CreateOpts {
            level: 6,
            threads: 0,
            overwrite: false,
        };
        let stats = zip::create(output, &inputs[..], &options, progress)?;
        stats
            .skipped
            .iter()
            .map(|s| format!("{}: {}", s.path.display(), s.reason))
            .collect()
    };
    Ok(skipped_text(&skipped))
}

fn skipped_text(skipped: &[String]) -> Option<String> {
    if skipped.is_empty() {
        return None;
    }
    let mut text = format!("{} item(s) could not be added:", skipped.len());
    for line in skipped.iter().take(5) {
        text.push_str("\n  ");
        text.push_str(line);
    }
    if skipped.len() > 5 {
        text.push_str("\n  ...");
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn unique_path_counts_up() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("a.zip");
        assert_eq!(unique_path(&first), first);
        fs::write(&first, "x").unwrap();
        let second = unique_path(&first);
        assert_eq!(second, tmp.path().join("a_2.zip"));
        fs::write(&second, "x").unwrap();
        assert_eq!(unique_path(&first), tmp.path().join("a_3.zip"));
    }

    #[test]
    fn extract_target_names() {
        let archive = Path::new("base").join("y.zip");
        let here = extract_target(&archive, false);
        let sub = extract_target(&archive, true);
        assert_eq!(here, Path::new("base"));
        assert_eq!(sub, Path::new("base").join("y"));
    }

    #[test]
    fn archive_path_is_next_to_the_input() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("my.project");
        fs::create_dir(&folder).unwrap();
        let file = tmp.path().join("notes.txt");
        fs::write(&file, "x").unwrap();
        let zip = archive_path_for(&folder, "zip");
        let kiln = archive_path_for(&file, "kiln");
        assert_eq!(zip, tmp.path().join("my.project.zip"));
        assert_eq!(kiln, tmp.path().join("notes.kiln"));
    }

    fn round_trip(ext: &str) {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("data");
        fs::create_dir_all(src.join("sub")).unwrap();
        let body = "hello kiln\n".repeat(1000);
        fs::write(src.join("a.txt"), body).unwrap();
        fs::write(src.join("sub").join("b.txt"), "deep").unwrap();
        let mut none = |_: u64, _: u64| {};

        let archive = archive_path_for(&src, ext);
        let added = add(&archive, &src, &mut none);
        assert_eq!(added, Outcome::Done(None));
        let tested = test(&archive, &mut none);
        assert!(matches!(tested, Outcome::Done(Some(_))));

        let dest = tmp.path().join("out");
        let made = extract(&archive, &dest, false, &mut none);
        assert_eq!(made, Outcome::Done(None));
        let deep = dest.join("data").join("sub").join("b.txt");
        assert_eq!(fs::read(deep).unwrap(), b"deep");

        // Unpacking over the same files is refused until it may replace them.
        let again = extract(&archive, &dest, false, &mut none);
        assert!(matches!(again, Outcome::Exists(_)));
        let replaced = extract(&archive, &dest, true, &mut none);
        assert_eq!(replaced, Outcome::Done(None));
    }

    #[test]
    fn zip_round_trip() {
        round_trip("zip");
    }

    #[test]
    fn kiln_round_trip() {
        round_trip("kiln");
    }
}
