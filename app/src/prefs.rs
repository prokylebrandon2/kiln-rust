//! Read access to Kiln's settings file.
//!
//! The Java app kept its settings in Android's `SharedPreferences` file `kiln`
//! (`<app data>/shared_prefs/kiln.xml`). The Rust app reads that very file, so every setting
//! survives the switch from one build to the other. Only plain values are read here (`int`, `long`,
//! `float`, `boolean`, `string`); writing comes with the Settings screen.

use std::collections::HashMap;
use std::path::Path;

/// The settings that were stored in `kiln.xml`, as text by key.
#[derive(Debug, Default)]
pub struct Prefs {
    values: HashMap<String, String>,
}

impl Prefs {
    /// Read the file. A missing or unreadable file is the same as an empty one, which is what
    /// `SharedPreferences` does too.
    pub fn load(file: &Path) -> Self {
        match std::fs::read_to_string(file) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Parse the XML text of a preferences file.
    pub fn parse(text: &str) -> Self {
        let mut values = HashMap::new();
        let mut rest = text;
        while let Some(start) = rest.find('<') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = &rest[..end];
            rest = &rest[end + 1..];
            let kind = tag.split_whitespace().next().unwrap_or("");
            match kind {
                "int" | "long" | "float" | "boolean" => {
                    if let (Some(name), Some(value)) = (attr(tag, "name"), attr(tag, "value")) {
                        values.insert(unescape(name), unescape(value));
                    }
                }
                "string" => {
                    let Some(name) = attr(tag, "name") else {
                        continue;
                    };
                    if tag.ends_with('/') {
                        values.insert(unescape(name), String::new());
                    } else if let Some(close) = rest.find("</string>") {
                        values.insert(unescape(name), unescape(&rest[..close]));
                        rest = &rest[close + "</string>".len()..];
                    }
                }
                _ => {}
            }
        }
        Self { values }
    }

    /// An integer setting (`default` when it is missing or not a number).
    pub fn int(&self, key: &str, default: i32) -> i32 {
        match self.values.get(key) {
            Some(v) => v.parse().unwrap_or(default),
            None => default,
        }
    }

    /// A flag (`default` when it is missing).
    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.values.get(key).map(String::as_str) {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        }
    }

    /// A text setting (`default` when it is missing).
    pub fn string(&self, key: &str, default: &str) -> String {
        match self.values.get(key) {
            Some(v) => v.clone(),
            None => default.to_string(),
        }
    }
}

/// The value of attribute `name` in the text of a tag, still escaped.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let from = tag.find(&key)? + key.len();
    let len = tag[from..].find('"')?;
    Some(&tag[from..from + len])
}

/// Undo the XML escapes `SharedPreferences` writes.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
