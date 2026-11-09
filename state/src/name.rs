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
}
