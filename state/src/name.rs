//! Checking the name a person types for a new file or folder.

/// What the field says when the name can't be used.
pub const BAD_NAME: &str = "Enter a name (no slashes)";

/// The name to create: the text without the spaces around it, or why it can't be used. A name
/// must not be empty, hold a slash or a NUL, or be `.` or `..` (which would name a folder that
/// already exists).
pub fn check(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(BAD_NAME);
    }
    Ok(name.to_string())
}

/// Where the part of a name that a rename selects first ends: at the last dot, so `report.pdf`
/// starts out with `report` selected and a quick retype keeps the extension (as the Java does).
/// A name with no dot, or only a leading one (`.nomedia`), is selected whole. The answer is a
/// byte offset into `name`, which is what a Slint `TextInput` takes.
pub fn stem_end(name: &str) -> usize {
    match name.rfind('.') {
        Some(dot) if dot > 0 => dot,
        _ => name.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_trimmed() {
        assert_eq!(check("  notes.txt "), Ok("notes.txt".to_string()));
        assert_eq!(check("my folder"), Ok("my folder".to_string()));
        assert_eq!(check(".hidden"), Ok(".hidden".to_string()));
    }

    #[test]
    fn empty_slashed_and_dot_names_are_refused() {
        for bad in ["", "   ", "a/b", "/", ".", " .. ", "a\0b"] {
            assert_eq!(check(bad), Err(BAD_NAME), "{bad:?}");
        }
    }

    #[test]
    fn the_stem_ends_at_the_last_dot() {
        assert_eq!(stem_end("report.pdf"), 6);
        assert_eq!(stem_end("archive.tar.gz"), 11);
        assert_eq!(stem_end("notes"), 5);
        assert_eq!(stem_end(""), 0);
    }

    #[test]
    fn a_leading_dot_does_not_end_the_stem() {
        assert_eq!(stem_end(".nomedia"), 8);
        assert_eq!(stem_end(".a.b"), 2);
    }

    #[test]
    fn the_stem_is_counted_in_bytes() {
        // "é" is two bytes in UTF-8
        assert_eq!(stem_end("é.txt"), 2);
        assert_eq!(&"é.txt"[..stem_end("é.txt")], "é");
    }
}
