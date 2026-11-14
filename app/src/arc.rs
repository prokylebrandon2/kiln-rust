//! An open archive: what the file browser keeps about it while it shows the archive's inside, and
//! the hand-made readers of the engine's result lines (the app crate has no regex). No view code.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use kiln_state::{Entry as ListEntry, Kind as ListKind};
use serde_json::{Value, json};

use crate::browser::{Kind, Row, Tint};
use crate::{fmt, icons};

/// One file of the archive. `k` is "+" for a new file and "~" for a renamed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ent {
    pub p: String,
    pub s: i64,
    pub m: i64,
    pub k: String,
}

/// An archive that is open.
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
    /// The archive an `open_archive` result describes.
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

    /// Take the entries and the dirty flag of an editing call's result.
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
    /// The folders and files directly inside `prefix`. Folders add up what is
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
            });
        }
        rows
    }

    /// What is directly inside the folder shown, as the entries of a listing: a file with its size
    /// and date, a folder with the number of files below it (their sizes and the newest date are
    /// added up for it, so that a sort by size or date has something to go by).
    pub fn list_entries(&self) -> Vec<ListEntry> {
        let mut dirs: HashMap<&str, (u32, u64, i64)> = HashMap::new();
        let mut out = Vec::new();
        for e in &self.ents {
            let Some(rest) = e.p.strip_prefix(self.prefix.as_str()) else {
                continue;
            };
            let size = u64::try_from(e.s).unwrap_or(0);
            match rest.find('/') {
                None if rest.is_empty() => {}
                None => out.push(ListEntry::with_meta(rest, ListKind::File, size, e.m)),
                Some(0) => {}
                Some(i) => {
                    let agg = dirs.entry(&rest[..i]).or_insert((0, 0, 0));
                    agg.0 = agg.0.saturating_add(1);
                    agg.1 += size;
                    agg.2 = agg.2.max(e.m);
                }
            }
        }
        for (name, (count, size, time)) in dirs {
            let mut dir = ListEntry::with_meta(name, ListKind::Dir, size, time);
            dir.children = Some(count);
            out.push(dir);
        }
        out
    }

    /// Inside an archive: every file whose name holds all the words (lower case),
    /// at most 1000, each with the folder it lives in.
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

    /// Where a hit lives, in its relative form, seen from the folder being shown.
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

    /// How many files the keys cover; no keys means all of them.
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

    /// The size of all files together, and how many folders hold them.
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
    }
}

/// The keys as the engine wants them (`[{"p": path, "dir": bool}]`).
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

/// "a.kiln" is "a"; a name that starts with its only dot keeps it.
pub fn strip_ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

/// What a finished run's closing line says: "12 file(s), 3.4 MB in 0.8s" and the like.
#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    pub files: u64,
    /// The size, when the line has one.
    pub size: Option<String>,
    pub secs: String,
}

/// The first " in <seconds>s" of `s`: where it starts, and the seconds.
fn find_in_secs(s: &str) -> Option<(usize, String)> {
    let mut from = 0;
    while let Some(i) = s[from..].find(" in ") {
        let at = from + i;
        let tail = &s[at + 4..];
        let secs: String = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !secs.is_empty() && tail[secs.len()..].starts_with('s') {
            return Some((at, secs));
        }
        from = at + 1;
    }
    None
}

/// The number written just before `at` in `msg`.
fn number_before(msg: &str, at: usize) -> Option<u64> {
    let digits = msg[..at]
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .len();
    if digits == at {
        return None;
    }
    msg[digits..at].parse().ok()
}

/// The engine's extract result line, `(\d+) file\(s\)(?: from [^\n]+?)?(?:, ([^\n]+?))? in ([\d.]+)s`:
/// "extracted 2 file(s), 88 B in 0.0s - ..." or "extracted 5 file(s) from /p in 0.4s - ...".
pub fn parse_extract(msg: &str) -> Option<Line> {
    const KEY: &str = " file(s)";
    let mut from = 0;
    while let Some(i) = msg[from..].find(KEY) {
        let at = from + i;
        from = at + 1;
        let Some(files) = number_before(msg, at) else {
            continue;
        };
        let line = msg[at + KEY.len()..].split('\n').next().unwrap_or("");
        let Some((pos, secs)) = find_in_secs(line) else {
            continue;
        };
        let head = &line[..pos];
        let size = match head.strip_prefix(", ") {
            Some(size) if !size.is_empty() => Some(size.to_string()),
            Some(_) => continue,
            None if head.is_empty() || head.starts_with(" from ") => None,
            None => continue,
        };
        return Some(Line { files, size, secs });
    }
    None
}

/// The first form of [`parse_check`]: a size is in the line.
fn check_with_size(msg: &str) -> Option<Line> {
    const WITH_SIZE: &str = " file(s), ";
    let at = msg.find(WITH_SIZE)?;
    let files = number_before(msg, at)?;
    let line = msg[at + WITH_SIZE.len()..].split('\n').next().unwrap_or("");
    let (pos, secs) = find_in_secs(line)?;
    (pos > 0).then(|| Line {
        files,
        size: Some(line[..pos].to_string()),
        secs,
    })
}

/// The two forms of the check result line: "12 file(s), 3.4 MB in 0.8s" (with a size), and "14 file(s) from
/// <path> in 5.5s" (no size: `size` is `None`).
pub fn parse_check(msg: &str) -> Option<Line> {
    const FROM: &str = " file(s) from ";
    if let Some(line) = check_with_size(msg) {
        return Some(line);
    }
    let at = msg.find(FROM)?;
    let files = number_before(msg, at)?;
    let (pos, secs) = find_in_secs(&msg[at + FROM.len()..])?;
    (pos > 0).then_some(Line {
        files,
        size: None,
        secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(paths: &[(&str, i64)]) -> ArcState {
        let r = json!({
            "ok": true,
            "kind": "archive",
            "encrypted": false,
            "packed": 10,
            "entries": paths.iter().map(|(p, s)| json!({"p": p, "s": s, "m": 0})).collect::<Vec<_>>(),
        });
        ArcState::from_open(PathBuf::from("/a/x.kiln"), String::new(), None, &r)
    }

    #[test]
    fn folders_add_up_what_is_below_them() {
        let mut a = arc(&[("a/b.txt", 5), ("a/c/d.txt", 7), ("e.txt", 1)]);
        let rows = a.rows(0);
        let dir = rows.iter().find(|r| r.key == "a/").expect("folder a");
        assert_eq!((dir.size, dir.sub.as_str()), (12, "2 files"));
        assert!(rows.iter().any(|r| r.key == "e.txt" && r.title == "e.txt"));
        a.prefix = "a/".to_string();
        let inner = a.rows(0);
        assert!(inner.iter().any(|r| r.key == "a/c/" && r.title == "c"));
        assert!(
            inner
                .iter()
                .any(|r| r.key == "a/b.txt" && r.title == "b.txt")
        );
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn back_goes_one_folder_up() {
        let mut a = arc(&[]);
        assert_eq!(a.parent_prefix(), None);
        a.prefix = "a/".to_string();
        assert_eq!(a.parent_prefix(), Some(String::new()));
        a.prefix = "a/b/".to_string();
        assert_eq!(a.parent_prefix(), Some("a/".to_string()));
    }

    #[test]
    fn counts_and_totals_follow_the_keys() {
        let a = arc(&[("a/b.txt", 5), ("a/c/d.txt", 7), ("ab.txt", 1)]);
        assert_eq!(a.count_for(&[]), 3);
        assert_eq!(a.count_for(&["a/".to_string()]), 2);
        assert_eq!(a.count_for(&["ab.txt".to_string(), "a/c/".to_string()]), 2);
        assert_eq!(a.totals_of("a/"), (12, 2, 0));
        assert_eq!(a.raw_and_folders(), (13, 2));
    }

    #[test]
    fn search_names_the_folder_of_a_hit() {
        let mut a = arc(&[("a/b/note.txt", 1), ("note2.txt", 1), ("a/other.txt", 1)]);
        let words = vec!["note".to_string()];
        let hits = a.search_rows(&words, 0);
        let subs: Vec<&str> = hits.iter().map(|r| r.sub.as_str()).collect();
        assert_eq!(subs, ["a/b", ""]);
        a.prefix = "a/".to_string();
        let hits = a.search_rows(&words, 0);
        assert_eq!(hits[0].sub, "b");
        assert_eq!(hits[0].key, "a/b/note.txt");
    }

    #[test]
    fn members_are_named_without_the_slash() {
        let text = members_json(&["a/".to_string(), "b.txt".to_string()]);
        let v: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v[0]["p"], "a");
        assert_eq!(v[0]["dir"], true);
        assert_eq!(v[1]["p"], "b.txt");
        assert_eq!(v[1]["dir"], false);
    }

    #[test]
    fn strips_one_extension() {
        assert_eq!(strip_ext("x.kiln"), "x");
        assert_eq!(strip_ext("x.tar.kiln"), "x.tar");
        assert_eq!(strip_ext(".hidden"), ".hidden");
        assert_eq!(strip_ext("plain"), "plain");
    }

    #[test]
    fn reads_the_extraction_line() {
        let l = parse_extract("extracted 2 file(s), 88 B in 0.0s - all checksums OK");
        assert_eq!(
            l,
            Some(Line {
                files: 2,
                size: Some("88 B".to_string()),
                secs: "0.0".to_string()
            })
        );
        let l = parse_extract("extracted 5 file(s) from /sd/x in 0.4s - all checksums OK");
        assert_eq!(
            l,
            Some(Line {
                files: 5,
                size: None,
                secs: "0.4".to_string()
            })
        );
        assert_eq!(parse_extract("nothing here"), None);
    }

    #[test]
    fn reads_the_check_line() {
        let l = parse_check("Checked 12 file(s), 3.4 MB in 0.8s - all checksums OK");
        assert_eq!(
            l,
            Some(Line {
                files: 12,
                size: Some("3.4 MB".to_string()),
                secs: "0.8".to_string()
            })
        );
        let l = parse_check("verified 14 file(s) from /p/q in 5.5s");
        assert_eq!(
            l,
            Some(Line {
                files: 14,
                size: None,
                secs: "5.5".to_string()
            })
        );
        assert_eq!(parse_check("No problems"), None);
    }
}
