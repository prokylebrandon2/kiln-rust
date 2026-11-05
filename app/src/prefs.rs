//! Kiln's settings file, shared with the Java build.
//!
//! The Java app kept its settings in Android's `SharedPreferences` file `kiln`
//! (`<app data>/shared_prefs/kiln.xml`). The Rust app reads and writes that very file, in the same
//! format, so every setting survives a switch from one build to the other. A change is made in
//! place: only the one element of the key is replaced or added, so values this code does not know
//! (a `<set>`, say) are left exactly as they were.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The settings that are stored in `kiln.xml`, as text by key.
#[derive(Debug, Default)]
pub struct Prefs {
    file: PathBuf,
    values: RefCell<HashMap<String, String>>,
}

impl Prefs {
    /// Read the file. A missing or unreadable file is the same as an empty one, which is what
    /// `SharedPreferences` does too.
    pub fn load(file: &Path) -> Self {
        let values = match std::fs::read_to_string(file) {
            Ok(text) => parse_values(&text),
            Err(_) => HashMap::new(),
        };
        Self {
            file: file.to_path_buf(),
            values: RefCell::new(values),
        }
    }

    /// Parse the XML text of a preferences file (nothing is ever written back to such a value).
    pub fn parse(text: &str) -> Self {
        Self {
            file: PathBuf::new(),
            values: RefCell::new(parse_values(text)),
        }
    }

    /// An integer setting (`default` when it is missing or not a number).
    pub fn int(&self, key: &str, default: i32) -> i32 {
        match self.values.borrow().get(key) {
            Some(v) => v.parse().unwrap_or(default),
            None => default,
        }
    }

    /// A flag (`default` when it is missing).
    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.values.borrow().get(key).map(String::as_str) {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        }
    }

    /// A text setting (`default` when it is missing).
    pub fn string(&self, key: &str, default: &str) -> String {
        match self.values.borrow().get(key) {
            Some(v) => v.clone(),
            None => default.to_string(),
        }
    }

    /// `putInt(..).apply()`.
    pub fn set_int(&self, key: &str, value: i32) {
        self.put("int", key, &value.to_string());
    }

    /// `putBoolean(..).apply()`.
    pub fn set_bool(&self, key: &str, value: bool) {
        self.put("boolean", key, if value { "true" } else { "false" });
    }

    /// `putString(..).apply()`.
    pub fn set_string(&self, key: &str, value: &str) {
        self.put("string", key, value);
    }

    /// `Defaults.bookmarks`: the folder paths, in the order they were added. They are kept as a
    /// JSON array of strings under `bookmarks`.
    pub fn bookmarks(&self) -> Vec<String> {
        let text = self.string("bookmarks", "[]");
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for path in items.iter().filter_map(Value::as_str) {
            if !path.is_empty() && !out.iter().any(|p| p == path) {
                out.push(path.to_string());
            }
        }
        out
    }

    /// `Defaults.addBookmark`.
    pub fn add_bookmark(&self, path: &str) {
        let mut list = self.bookmarks();
        if !list.iter().any(|p| p == path) {
            list.push(path.to_string());
            self.save_bookmarks(&list);
        }
    }

    /// `Defaults.removeBookmark`.
    pub fn remove_bookmark(&self, path: &str) {
        let mut list = self.bookmarks();
        let before = list.len();
        list.retain(|p| p != path);
        if list.len() != before {
            self.save_bookmarks(&list);
        }
    }

    /// `new JSONArray(list).toString()`: Android's JSON writer escapes every "/" as "\/".
    fn save_bookmarks(&self, list: &[String]) {
        let text = Value::from(list.to_vec()).to_string();
        self.set_string("bookmarks", &text.replace('/', "\\/"));
    }

    /// `Defaults.folderIcon`: the icon key a person picked for a folder (`folder_icons` holds a
    /// JSON object of path to key), if any.
    pub fn folder_icon(&self, path: &str) -> Option<String> {
        let text = self.string("folder_icons", "{}");
        let value = serde_json::from_str::<Value>(&text).ok()?;
        let key = value.get(path)?.as_str()?;
        if key.is_empty() {
            None
        } else {
            Some(key.to_string())
        }
    }

    /// `Defaults.setFolderIcon`: remember the icon `key` for a folder. An empty key goes back to
    /// the standard look. The JSON object is written the way Android writes it, with "\/" for "/".
    pub fn set_folder_icon(&self, path: &str, key: &str) {
        let mut icons = self.folder_icon_map();
        if key.is_empty() {
            icons.remove(path);
        } else {
            icons.insert(path.to_string(), Value::from(key));
        }
        let text = Value::Object(icons).to_string();
        self.set_string("folder_icons", &text.replace('/', "\\/"));
    }

    /// The saved folder icons (path to key), without entries whose key is empty or not text.
    fn folder_icon_map(&self) -> Map<String, Value> {
        let text = self.string("folder_icons", "{}");
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
            return Map::new();
        };
        map.into_iter()
            .filter(|(_, v)| v.as_str().is_some_and(|s| !s.is_empty()))
            .collect()
    }

    /// `Defaults.moveFolderIcons`: a folder was renamed or moved, so its icon and colour (and
    /// those of its subfolders) follow it.
    pub fn move_folder_icons(&self, from: &str, to: &str) {
        self.move_paths("folder_icons", from, to);
        self.move_paths("folder_colors", from, to);
    }

    /// Re-key the entries of the JSON object stored under `key` that are `from` or below it.
    fn move_paths(&self, key: &str, from: &str, to: &str) {
        let text = self.string(key, "{}");
        let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let under = format!("{from}/");
        let moving: Vec<String> = map
            .keys()
            .filter(|k| k.as_str() == from || k.starts_with(&under))
            .cloned()
            .collect();
        if moving.is_empty() {
            return;
        }
        let mut moved = Vec::new();
        for k in moving {
            if let Some(v) = map.remove(&k) {
                moved.push((format!("{to}{}", &k[from.len()..]), v));
            }
        }
        map.extend(moved);
        let text = Value::Object(map).to_string();
        self.set_string(key, &text.replace('/', "\\/"));
    }

    fn put(&self, kind: &str, key: &str, value: &str) {
        self.values
            .borrow_mut()
            .insert(key.to_string(), value.to_string());
        if self.file.as_os_str().is_empty() {
            return;
        }
        let text = std::fs::read_to_string(&self.file).unwrap_or_default();
        let out = with_value(&text, kind, key, value);
        // `apply()` does not report a failed write either.
        let _ = write_atomic(&self.file, &out);
    }
}

/// The text of a preferences file with `key` set to `value`.
fn with_value(text: &str, kind: &str, key: &str, value: &str) -> String {
    let new = element(kind, key, value);
    if let Some((from, to)) = find_element(text, key) {
        return format!("{}{}{}", &text[..from], new, &text[to..]);
    }
    if let Some(end) = text.rfind("</map>") {
        return format!("{}    {}\n{}", &text[..end], new, &text[end..]);
    }
    let head = "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>";
    format!("{head}\n<map>\n    {new}\n</map>\n")
}

fn element(kind: &str, key: &str, value: &str) -> String {
    let name = escape(key);
    if kind == "string" {
        format!("<string name=\"{name}\">{}</string>", escape(value))
    } else {
        format!("<{kind} name=\"{name}\" value=\"{}\" />", escape(value))
    }
}

/// Where the element for `key` starts and ends in the file text.
fn find_element(text: &str, key: &str) -> Option<(usize, usize)> {
    let needle = format!(" name=\"{}\"", escape(key));
    let mut from = 0;
    while let Some(i) = text[from..].find(&needle) {
        let at = from + i;
        from = at + needle.len();
        let start = text[..at].rfind('<')?;
        let tag_end = start + text[start..].find('>')? + 1;
        if tag_end <= at {
            continue;
        }
        if text[start..tag_end].ends_with("/>") {
            return Some((start, tag_end));
        }
        let kind = text[start + 1..]
            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .next()?;
        let close = format!("</{kind}>");
        let end = tag_end + text[tag_end..].find(&close)? + close.len();
        return Some((start, end));
    }
    None
}

fn write_atomic(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("xml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

fn parse_values(text: &str) -> HashMap<String, String> {
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
    values
}

/// The value of attribute `name` in the text of a tag, still escaped.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let from = tag.find(&key)? + key.len();
    let len = tag[from..].find('"')?;
    Some(&tag[from..from + len])
}

/// The XML escapes `SharedPreferences` writes.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
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
