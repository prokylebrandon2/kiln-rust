//! An open archive: what the file browser keeps about it while it shows the archive's inside
//! (`MainActivity.Arc`, `Ent`, `buildArcRows`, `arcSearchRow`, `countFor`, `membersJson`), and the
//! hand-made readers of the engine's result lines (the app crate has no regex). No view code.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::browser::{Kind, Row, Tint};
use crate::{fmt, icons};

/// One file of the archive (`MainActivity.Ent`). `k` is "+" for a new file and "~" for a renamed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ent {
    pub p: String,
    pub s: i64,
    pub m: i64,
    pub k: String,
}

/// An archive that is open (`MainActivity.Arc`).
#[derive(Debug)]
pub struct ArcState {
    pub file: PathBuf,
    pub password: String,
    pub is_set: bool,
    pub encrypted: bool,
    pub packed: i64,
    /// The folder inside the archive that is shown: "" or "a/b/".
    pub prefix: String,
    /// Where Extract goes by default (`home`); `None` is beside the file.
    pub home: Option<PathBuf>,
    /// The staged-edit session (`sid`); `None` until the first edit.
    pub sid: Option<String>,
    pub dirty: bool,
    pub ents: Vec<Ent>,
    /// How far each folder of the archive was scrolled, in dp, by its prefix (`pos`).
    pub pos: HashMap<String, f32>,
}

fn entries(v: &Value) -> Vec<Ent> {
    let empty = Vec::new();
    v["entries"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|o| o.is_object())
        .map(|o| Ent {
            p: o["p"].as_str().unwrap_or("").to_string(),
            s: o["s"].as_i64().unwrap_or(0),
            m: o["m"].as_i64().unwrap_or(0),
            k: o["k"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

impl ArcState {
    /// `onOpened`: the archive an `open_archive` result describes.
    pub fn from_open(file: PathBuf, password: String, home: Option<PathBuf>, r: &Value) -> Self {
        Self {
            file,
            password,
            is_set: r["kind"].as_str() == Some("set"),
            encrypted: r["encrypted"].as_bool().unwrap_or(false),
            packed: r["packed"].as_i64().unwrap_or(0),
            prefix: String::new(),
            home,
            sid: None,
            dirty: false,
            ents: entries(r),
            pos: HashMap::new(),
        }
    }

    /// `applyEdit`: take the entries and the dirty flag of an editing call's result.
    pub fn apply_edit(&mut self, r: &Value) {
        if r["entries"].is_array() {
            self.ents = entries(r);
        }
        self.dirty = r["dirty"].as_bool().unwrap_or(self.dirty);
    }

    pub fn file_name(&self) -> String {
        self.file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    /// The folder Back goes to from the one shown, or `None` at the top (Back leaves the archive).
    pub fn parent_prefix(&self) -> Option<String> {
        if self.prefix.is_empty() {
            return None;
        }
        let inner = &self.prefix[..self.prefix.len() - 1];
        Some(match inner.rfind('/') {
            Some(i) => inner[..=i].to_string(),
            None => String::new(),
        })
    }
}

impl ArcState {
    /// `buildArcRows`: the folders and files directly inside `prefix`. Folders add up what is
    /// below them. Dates use `offset` seconds from UTC.
    pub fn rows(&self, offset: i32) -> Vec<Row> {
        let mut dirs: HashMap<&str, (i64, i64, i64)> = HashMap::new();
        let mut rows = Vec::new();
        for e in &self.ents {
            let Some(rest) = e.p.strip_prefix(self.prefix.as_str()) else {
                continue;
            };
            match rest.find('/') {
                None => rows.push(file_row(e, rest, offset)),
                Some(i) => {
                    let agg = dirs.entry(&rest[..i]).or_insert((0, 0, 0));
                    agg.0 += 1;
                    agg.1 += e.s.max(0);
                    agg.2 = agg.2.max(e.m);
                }
            }
        }
        for (name, (count, size, time)) in dirs {
            rows.push(Row {
                key: format!("{}{name}/", self.prefix),
                title: name.to_string(),
                lower: name.to_lowercase(),
                sub: fmt::plural(count, "file", "files"),
                right: fmt::size(size),
                ext: String::new(),
                icon: "folder",
                tint: Tint::Accent,
                kind: Kind::Dir,
                size,
                time,
                enc: false,
            });
        }
        rows
    }

    /// `filterLocal` inside an archive: every file whose name holds all the words (lower case),
    /// at most 1000, each with the folder it lives in (`arcSearchRow`, the relative labels).
    pub fn search_rows(&self, words: &[String], offset: i32) -> Vec<Row> {
        let mut out = Vec::new();
        for e in &self.ents {
            if out.len() >= 1000 {
                break;
            }
            let slash = e.p.rfind('/');
            let name = slash.map_or(e.p.as_str(), |i| &e.p[i + 1..]);
            let lower = name.to_lowercase();
            if !words.iter().all(|w| lower.contains(w.as_str())) {
                continue;
            }
            let mut row = file_row(e, name, offset);
            let when = if e.m > 0 {
                fmt::date(e.m, offset)
            } else {
                String::new()
            };
            let place = match slash {
                Some(i) if i > 0 => self.where_label(&e.p[..i]),
                _ => String::new(),
            };
            row.sub = if place.is_empty() {
                when
            } else if when.is_empty() {
                place
            } else {
                format!("{place} \u{b7} {when}")
            };
            out.push(row);
        }
        out
    }

    /// `arcWhere` (the relative form): where a hit lives, seen from the folder being shown.
    fn where_label(&self, dir: &str) -> String {
        let d = format!("{dir}/");
        if !self.prefix.is_empty() && d.starts_with(self.prefix.as_str()) {
            if d.len() <= self.prefix.len() {
                return "This folder".to_string();
            }
            return d[self.prefix.len()..d.len() - 1].to_string();
        }
        if dir.is_empty() {
            "This folder".to_string()
        } else {
            dir.to_string()
        }
    }

    /// `(size, files, newest modification)` of what `key` names: a file, or a folder ("a/") and
    /// everything below it.
    pub fn totals_of(&self, key: &str) -> (i64, i64, i64) {
        let dir = key.ends_with('/');
        let (mut size, mut files, mut newest) = (0, 0, 0);
        for e in &self.ents {
            let hit = if dir {
                e.p.starts_with(key)
            } else {
                e.p == key
            };
            if hit {
                size += e.s.max(0);
                files += 1;
                newest = newest.max(e.m);
            }
        }
        (size, files, newest)
    }

    /// `countFor`: how many files the keys cover; no keys means all of them.
    pub fn count_for(&self, keys: &[String]) -> usize {
        if keys.is_empty() {
            return self.ents.len();
        }
        self.ents
            .iter()
            .filter(|e| {
                keys.iter().any(|k| {
                    if k.ends_with('/') {
                        e.p.starts_with(k.as_str())
                    } else {
                        e.p == *k
                    }
                })
            })
            .count()
    }

    /// The size of all files together, and how many folders hold them (`infoDialog`).
    pub fn raw_and_folders(&self) -> (i64, usize) {
        let mut raw = 0;
        let mut folders: HashSet<&str> = HashSet::new();
        for e in &self.ents {
            raw += e.s.max(0);
            for (i, c) in e.p.char_indices() {
                if c == '/' {
                    folders.insert(&e.p[..i]);
                }
            }
        }
        (raw, folders.len())
    }
}

/// A file's row, named `title` (the part of its path below the folder shown).
fn file_row(e: &Ent, title: &str, offset: i32) -> Row {
    let when = if e.m > 0 {
        fmt::date(e.m, offset)
    } else {
        String::new()
    };
    let sub = match e.k.as_str() {
        "+" if e.m > 0 => format!("New \u{b7} {when}"),
        "+" => "New".to_string(),
        "~" => "Renamed".to_string(),
        _ => when,
    };
    let (icon, color) = icons::for_file(&e.p);
    Row {
        key: e.p.clone(),
        title: title.to_string(),
        lower: title.to_lowercase(),
        sub,
        right: fmt::size(e.s),
        ext: fmt::ext(title),
        icon,
        tint: color.map_or(Tint::Text2, Tint::Rgb),
        kind: Kind::File,
        size: e.s.max(0),
        time: e.m,
        enc: false,
    }
}

/// `membersJson`: the keys as the engine wants them (`[{"p": path, "dir": bool}]`).
pub fn members_json(keys: &[String]) -> String {
    let list: Vec<Value> = keys
        .iter()
        .map(|k| {
            let dir = k.ends_with('/');
            let p = if dir { &k[..k.len() - 1] } else { k.as_str() };
            json!({ "p": p, "dir": dir })
        })
        .collect();
    Value::Array(list).to_string()
}

/// `Fmt.stripExt`: "a.kiln" is "a"; a name that starts with its only dot keeps it.
pub fn strip_ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}
