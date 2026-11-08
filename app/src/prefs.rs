//! Kiln's settings file: Android's `SharedPreferences` file `kiln`
//! (`<app data>/shared_prefs/kiln.xml`). A change is made in place: only the one element of the key
//! is replaced or added, so values this code does not know (a `<set>`, say) are left exactly as
//! they were.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A default app is stored per extension as `def_<ext>`.
const OPEN_PREFIX: &str = "def_";

/// The settings that go into a backup, and their type: b boolean, i int, s string. Root and
/// Shizuku access are never exported or imported.
const BACKUP_KEYS: &[(&str, char)] = &[
    ("up_row", 'b'),
    ("previews", 'b'),
    ("delete_after_pack", 'b'),
    ("search_deep", 'b'),
    ("units_decimal", 'b'),
    ("sort_asc", 'b'),
    ("show_all", 'b'),
    ("show_hidden", 'b'),
    ("viewer_wrap", 'b'),
    ("delete_mode", 'i'),
    ("jobs", 'i'),
    ("rows_on_screen", 'i'),
    ("anim_level", 'i'),
    ("search_path", 'i'),
    ("folder_color_default", 'i'),
    ("sort_key", 'i'),
    ("viewer_size", 'i'),
    ("bookmarks", 's'),
    ("folder_icons", 's'),
    ("folder_colors", 's'),
];

/// What a stored "open with" value shows as.
pub fn open_with_label(value: &str) -> String {
    match value {
        "kiln:view" => "Kiln (view)".to_string(),
        "kiln:edit" => "Kiln (edit)".to_string(),
        _ => value
            .split_once('|')
            .map_or(value, |(_, label)| label)
            .to_string(),
    }
}

/// The settings that are stored in `kiln.xml`, as text by key.
#[derive(Debug, Default)]
pub struct Prefs {
    file: PathBuf,
    values: RefCell<HashMap<String, String>>,
}

impl Prefs {
    /// Read the file. A missing or unreadable file is the same as an empty one, which is what
    /// Does too.
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

    /// The folder paths, in the order they were added. They are kept as a
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

    pub fn add_bookmark(&self, path: &str) {
        let mut list = self.bookmarks();
        if !list.iter().any(|p| p == path) {
            list.push(path.to_string());
            self.save_bookmarks(&list);
        }
    }

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

    /// The icon key a person picked for a folder (`folder_icons` holds a
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

    /// Remember the icon `key` for a folder. An empty key goes back to
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
    pub fn folder_icon_map(&self) -> Map<String, Value> {
        let text = self.string("folder_icons", "{}");
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
            return Map::new();
        };
        map.into_iter()
            .filter(|(_, v)| v.as_str().is_some_and(|s| !s.is_empty()))
            .collect()
    }

    /// The saved folder colours (path to ARGB as a signed number), without entries that are 0.
    pub fn folder_color_map(&self) -> Map<String, Value> {
        let text = self.string("folder_colors", "{}");
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
            return Map::new();
        };
        map.into_iter()
            .filter(|(_, v)| v.as_i64().is_some_and(|c| c != 0))
            .collect()
    }

    /// The ARGB colour picked for a folder's icon, or 0 for none.
    pub fn folder_color(&self, path: &str) -> u32 {
        let colors = self.folder_color_map();
        let value = colors.get(path).and_then(Value::as_i64);
        value.map_or(0, |c| c as u32)
    }

    /// Remember a colour for a folder; 0 goes back to the accent. The
    /// number is written, so negative), with "\/" for "/".
    pub fn set_folder_color(&self, path: &str, color: u32) {
        let mut colors = self.folder_color_map();
        if color == 0 {
            colors.remove(path);
        } else {
            colors.insert(path.to_string(), Value::from(color as i32));
        }
        let text = Value::Object(colors).to_string();
        self.set_string("folder_colors", &text.replace('/', "\\/"));
    }

    /// The ARGB colour every folder gets unless it has its own, or
    /// 0 for the normal accent. Set in Settings.
    pub fn default_folder_color(&self) -> u32 {
        self.int("folder_color_default", 0) as u32
    }

    pub fn set_default_folder_color(&self, color: u32) {
        self.set_int("folder_color_default", color as i32);
    }

    /// A folder's own colour, else the default, else 0.
    pub fn folder_color_or_default(&self, path: &str) -> u32 {
        match self.folder_color(path) {
            0 => self.default_folder_color(),
            own => own,
        }
    }

    /// How many folders have a colour of their own.
    pub fn custom_folder_colors(&self) -> usize {
        self.folder_color_map().len()
    }

    /// Every folder follows the default again.
    pub fn clear_folder_colors(&self) {
        self.set_string("folder_colors", "{}");
    }

    /// The "always open with" choices, `(extension, stored value)`, sorted by
    /// extension.
    pub fn open_with_all(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .values
            .borrow()
            .iter()
            .filter_map(|(k, v)| {
                let ext = k.strip_prefix(OPEN_PREFIX)?;
                (!ext.is_empty()).then(|| (ext.to_string(), v.clone()))
            })
            .collect();
        out.sort();
        out
    }

    /// Forget the app chosen for one extension.
    pub fn clear_open_with(&self, ext: &str) {
        self.remove(&format!("{OPEN_PREFIX}{ext}"));
    }

    /// Forget every "always open with" choice.
    pub fn clear_all_open_with(&self) {
        for (ext, _) in self.open_with_all() {
            self.clear_open_with(&ext);
        }
    }

    /// Remove a setting.
    pub fn remove(&self, key: &str) {
        if self.values.borrow_mut().remove(key).is_none() || self.file.as_os_str().is_empty() {
            return;
        }
        let text = std::fs::read_to_string(&self.file).unwrap_or_default();
        if let Some((from, to)) = find_element(&text, key) {
            let rest = text[to..].trim_start_matches(['\n', ' ']);
            let out = format!("{}{rest}", &text[..from]);
            let _ = write_atomic(&self.file, &out);
        }
    }

    /// Every backed-up setting that has been set, plus the "always
    /// open with" choices, as JSON text. Root and Shizuku access are never part of it.
    pub fn export_settings(&self) -> String {
        let mut settings = Map::new();
        {
            let values = self.values.borrow();
            for (key, kind) in BACKUP_KEYS {
                let Some(text) = values.get(*key) else {
                    continue;
                };
                let value = match *kind {
                    'b' => match text.as_str() {
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        _ => continue,
                    },
                    'i' => match text.parse::<i32>() {
                        Ok(n) => Value::from(n),
                        Err(_) => continue,
                    },
                    _ => Value::from(text.clone()),
                };
                settings.insert((*key).to_string(), value);
            }
        }
        let open: Map<String, Value> = self
            .open_with_all()
            .into_iter()
            .map(|(ext, value)| (ext, Value::from(value)))
            .collect();
        let root = serde_json::json!({
            "kiln_settings": 1,
            "settings": settings,
            "open_with": open,
        });
        serde_json::to_string_pretty(&root).unwrap_or_default()
    }

    /// Apply a backup made by [`Prefs::export_settings`]. Unknown keys
    /// and values of the wrong type are skipped. Returns how many settings were applied.
    pub fn import_settings(&self, text: &str) -> Result<usize, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if root.get("kiln_settings").and_then(Value::as_i64) != Some(1) {
            return Err("not a Kiln settings file".to_string());
        }
        let mut n = 0;
        if let Some(Value::Object(settings)) = root.get("settings") {
            for (key, kind) in BACKUP_KEYS {
                let Some(value) = settings.get(*key) else {
                    continue;
                };
                match (*kind, value) {
                    ('b', Value::Bool(b)) => self.set_bool(key, *b),
                    ('i', Value::Number(num)) => {
                        let Some(i) = num.as_i64().and_then(|i| i32::try_from(i).ok()) else {
                            continue;
                        };
                        if *key == "sort_key" && !(0..=3).contains(&i) {
                            continue;
                        }
                        self.set_int(key, i);
                    }
                    ('s', Value::String(text)) => self.set_string(key, text),
                    _ => continue,
                }
                n += 1;
            }
        }
        if let Some(Value::Object(open)) = root.get("open_with") {
            for (ext, value) in open {
                if let (false, Some(value)) = (ext.is_empty(), value.as_str()) {
                    self.set_string(&format!("{OPEN_PREFIX}{ext}"), value);
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// A folder was renamed or moved, so its icon and colour (and
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
