//! An open archive: what the file browser keeps about it while it shows the archive's inside
//! (`MainActivity.Arc`, `Ent`, `buildArcRows`, `arcSearchRow`, `countFor`, `membersJson`), and the
//! hand-made readers of the engine's result lines (the app crate has no regex). No view code.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

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
