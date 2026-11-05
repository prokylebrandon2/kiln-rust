//! The file browser's state and rules: what `MainActivity` keeps about the folder it shows (rows,
//! selection, sort, search, Back), with no view code in it. `lib.rs` and `browse_ui.rs` put the
//! result on screen.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::host::Volume;
use crate::{fmt, icons};

/// How long a second Back at home has to follow the first to leave the app.
const EXIT_WINDOW: Duration = Duration::from_millis(2000);

/// What an entry of a folder is (the `k` of the engine's scan).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Set,
    Archive,
    File,
}

impl Kind {
    fn from_code(code: &str) -> Self {
        match code {
            "dir" => Self::Dir,
            "set" => Self::Set,
            "archive" => Self::Archive,
            _ => Self::File,
        }
    }
}

/// The colour of a row's icon: the live accent, the live secondary text colour, or a fixed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    Accent,
    Text2,
    Rgb(u32),
}

/// One row of the list (`MainActivity.Row` and `Item` together).
#[derive(Clone, Debug)]
pub struct Row {
    /// The entry's name in the folder; selection is by this.
    pub key: String,
    pub title: String,
    /// The title in lower case, for sorting and searching.
    pub lower: String,
    pub sub: String,
    pub right: String,
    pub ext: String,
    pub icon: &'static str,
    pub tint: Tint,
    pub kind: Kind,
    pub size: i64,
    pub time: i64,
}

impl Row {
    /// Folders and encrypted sets sort first and open like folders.
    pub fn folder(&self) -> bool {
        matches!(self.kind, Kind::Dir | Kind::Set)
    }
}

/// `S_NAME`, `S_SIZE`, `S_DATE`, `S_TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Date,
    Type,
}

impl SortKey {
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Size,
            2 => Self::Date,
            3 => Self::Type,
            _ => Self::Name,
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Name => 0,
            Self::Size => 1,
            Self::Date => 2,
            Self::Type => 3,
        }
    }
}

/// What the top bar shows (`MainActivity.renderHeader`).
pub struct Header {
    pub title: String,
    pub sub: String,
    /// At a storage root the Kiln mark is the left button.
    pub home: bool,
    pub left_icon: &'static str,
}

/// What a Back press did (`MainActivity.onBackPressed`).
#[derive(Debug, PartialEq, Eq)]
pub enum BackAction {
    /// The selection was cleared, or search was left: redraw.
    Changed,
    /// Go up to this folder.
    Up(PathBuf),
    /// First Back at home: say "Tap Back again to exit".
    ExitHint,
    /// Second Back at home: leave.
    Exit,
}

pub struct Browser {
    root: PathBuf,
    pub cwd: PathBuf,
    /// What the list shows: the folder, or the search hits in it.
    pub rows: Vec<Row>,
    /// The folder itself, kept while a search narrows `rows`.
    source: Vec<Row>,
    pub selected: Vec<String>,
    pub sort_key: SortKey,
    pub sort_asc: bool,
    pub show_all: bool,
    pub show_hidden: bool,
    pub up_row_on: bool,
    /// The USB drives and SD cards that are mounted; each is a storage root of its own.
    pub volumes: Vec<Volume>,
    /// "All files access" is granted.
    pub storage_ok: bool,
    pub searching: bool,
    pub query: String,
    pub plain_set: bool,
    /// A folder is being read; `rows` are still the previous folder's.
    pub pending: bool,
    pub generation: u64,
    /// The engine's reason when a folder could not be read.
    error: Option<String>,
    last_back_at_home: Option<Instant>,
}

impl Browser {
    pub fn new(root: PathBuf) -> Self {
        Self {
            cwd: root.clone(),
            root,
            rows: Vec::new(),
            source: Vec::new(),
            selected: Vec::new(),
            sort_key: SortKey::Name,
            sort_asc: true,
            show_all: true,
            show_hidden: false,
            up_row_on: true,
            volumes: Vec::new(),
            storage_ok: false,
            searching: false,
            query: String::new(),
            plain_set: false,
            pending: false,
            generation: 0,
            error: None,
            last_back_at_home: None,
        }
    }

    /// `isStorageRoot`: the top of the phone's storage, or of a USB drive or SD card.
    pub fn is_storage_root(&self, dir: &Path) -> bool {
        dir == self.root || self.volumes.iter().any(|v| v.path == dir)
    }

    /// The top of the phone's own storage.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `storageName`: what a storage root is called.
    pub fn storage_name(&self, dir: &Path) -> String {
        if dir == self.root {
            return "Internal storage".to_string();
        }
        let volume = self.volumes.iter().find(|v| v.path == dir);
        match volume {
            Some(v) => v.label.clone(),
            None => dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        }
    }

    pub fn selecting(&self) -> bool {
        !self.selected.is_empty()
    }

    /// The ".." row shows when there is somewhere to go up to (`upShown`).
    pub fn up_shown(&self) -> bool {
        self.up_row_on
            && !self.is_storage_root(&self.cwd)
            && self.storage_ok
            && !(self.searching && !self.query.is_empty())
    }

    /// Start reading `dir`. Returns the number the result must carry to still be wanted.
    pub fn begin_load(&mut self, dir: PathBuf, keep_selection: bool) -> u64 {
        let moved = dir != self.cwd;
        if self.searching && moved {
            self.close_search_quiet();
        }
        self.cwd = dir;
        self.generation += 1;
        self.pending = true;
        if !keep_selection {
            self.selected.clear();
        }
        self.generation
    }

    /// Take the engine's `scan` result. Returns false when it is for a folder that is no longer
    /// the one wanted.
    pub fn apply_scan(
        &mut self,
        generation: u64,
        result: &Value,
        offset: &mut dyn FnMut(i64) -> i32,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.pending = false;
        self.plain_set = result["is_set"].as_bool().unwrap_or(false);
        if !result["ok"].as_bool().unwrap_or(false) {
            self.rows.clear();
            self.source.clear();
            let why = result["error"]
                .as_str()
                .unwrap_or("Couldn't read this folder");
            self.error = Some(why.to_string());
            return true;
        }
        self.error = None;
        let at_root = self.is_storage_root(&self.cwd);
        let mut rows = parse_rows(result, &self.cwd, at_root, offset);
        sort_rows(&mut rows, self.sort_key, self.sort_asc);
        self.source = rows;
        self.show_source_or_hits();
        true
    }

    fn show_source_or_hits(&mut self) {
        if self.searching && !self.query.is_empty() {
            self.filter_local();
        } else {
            self.rows = self.source.clone();
        }
        let rows = &self.rows;
        self.selected.retain(|k| rows.iter().any(|r| &r.key == k));
    }

    /// The line in the middle of the body (`empty`): empty while there are rows.
    pub fn empty_text(&self, loading_hint: bool) -> String {
        if !self.rows.is_empty() {
            return String::new();
        }
        if let Some(why) = &self.error {
            return why.clone();
        }
        if self.pending {
            let text = if loading_hint { "Loading..." } else { "" };
            return text.to_string();
        }
        if self.searching && !self.query.is_empty() {
            return "No matches.".to_string();
        }
        if self.show_all {
            "This folder is empty.".to_string()
        } else {
            "No Kiln archives here.\nOpen the menu to show all files.".to_string()
        }
    }

    pub fn header(&self) -> Header {
        if self.selecting() {
            return Header {
                title: format!("{} selected", self.selected.len()),
                sub: String::new(),
                home: false,
                left_icon: "close",
            };
        }
        let at_root = self.is_storage_root(&self.cwd);
        let title = if self.cwd == self.root {
            "Kiln".to_string()
        } else if at_root {
            self.storage_name(&self.cwd)
        } else {
            let name = self.cwd.file_name().unwrap_or_default();
            name.to_string_lossy().into_owned()
        };
        Header {
            title,
            sub: self.cwd.to_string_lossy().into_owned(),
            home: at_root,
            left_icon: "up",
        }
    }

    /// `toggle`: select a row, or unselect it.
    pub fn toggle(&mut self, key: &str) {
        if let Some(i) = self.selected.iter().position(|k| k == key) {
            self.selected.remove(i);
        } else {
            self.selected.push(key.to_string());
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// `saveSort` without the saving: re-sort what is shown and what it was narrowed from.
    pub fn set_sort(&mut self, key: SortKey, asc: bool) {
        self.sort_key = key;
        self.sort_asc = asc;
        sort_rows(&mut self.rows, key, asc);
        sort_rows(&mut self.source, key, asc);
    }

    /// `openSearch`.
    pub fn open_search(&mut self) {
        self.searching = true;
        self.query.clear();
    }

    /// `closeSearch`: back to the plain list.
    pub fn close_search(&mut self) {
        if !self.searching {
            return;
        }
        self.close_search_quiet();
        self.show_source_or_hits();
    }

    fn close_search_quiet(&mut self) {
        self.searching = false;
        self.query.clear();
    }

    /// `onQueryChanged`: the words in the box changed.
    pub fn set_query(&mut self, text: &str) {
        if !self.searching {
            return;
        }
        self.query = text.trim().to_string();
        self.show_source_or_hits();
    }

    /// `filterLocal`: the rows of the folder whose names contain every word of the query.
    fn filter_local(&mut self) {
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let mut hits: Vec<Row> = self
            .source
            .iter()
            .filter(|r| words.iter().all(|w| r.lower.contains(w.as_str())))
            .cloned()
            .collect();
        sort_rows(&mut hits, self.sort_key, self.sort_asc);
        self.rows = hits;
    }

    /// `onBackPressed`, for the parts of it that exist so far.
    pub fn back(&mut self) -> BackAction {
        if self.selecting() {
            self.selected.clear();
            return BackAction::Changed;
        }
        if self.searching {
            self.close_search();
            return BackAction::Changed;
        }
        if !self.is_storage_root(&self.cwd) {
            if let Some(parent) = self.cwd.parent() {
                return BackAction::Up(parent.to_path_buf());
            }
        }
        let now = Instant::now();
        let again = self
            .last_back_at_home
            .is_some_and(|before| now.duration_since(before) < EXIT_WINDOW);
        if again {
            return BackAction::Exit;
        }
        self.last_back_at_home = Some(now);
        BackAction::ExitHint
    }
}

/// `sortList`: folders first, then the chosen key, ties by name.
pub fn sort_rows(rows: &mut [Row], key: SortKey, asc: bool) {
    rows.sort_by(|a, b| {
        let (fa, fb) = (a.folder(), b.folder());
        if fa != fb {
            return if fa {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let by_key = match key {
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Date => a.time.cmp(&b.time),
            SortKey::Type if fa => Ordering::Equal,
            SortKey::Type => a.ext.cmp(&b.ext),
            SortKey::Name => a.lower.cmp(&b.lower),
        };
        match by_key {
            Ordering::Equal => a.lower.cmp(&b.lower),
            other if asc => other,
            other => other.reverse(),
        }
    });
}

/// `parseItems`: the rows of an engine `scan` result for folder `cwd`.
fn parse_rows(
    result: &Value,
    cwd: &Path,
    at_root: bool,
    offset: &mut dyn FnMut(i64) -> i32,
) -> Vec<Row> {
    let empty = Vec::new();
    let items = result["items"].as_array().unwrap_or(&empty);
    let parent = cwd.file_name().unwrap_or_default().to_string_lossy();
    items
        .iter()
        .map(|item| {
            let name = item["n"].as_str().unwrap_or("").to_string();
            let kind = Kind::from_code(item["k"].as_str().unwrap_or(""));
            let size = item["s"].as_i64().unwrap_or(0);
            let time = item["m"].as_i64().unwrap_or(0);
            let count = item["c"].as_i64().unwrap_or(0);
            let enc = item["e"].as_bool().unwrap_or(false);
            let when = fmt::date(time, offset(time));
            let mut row = Row {
                key: name.clone(),
                lower: name.to_lowercase(),
                ext: fmt::ext(&name),
                title: name,
                sub: when.clone(),
                right: String::new(),
                icon: "file",
                tint: Tint::Text2,
                kind,
                size: 0,
                time,
            };
            match kind {
                Kind::Dir => {
                    row.size = count.max(0);
                    row.icon = icons::for_dir(&row.title, &parent, at_root);
                    row.tint = Tint::Accent;
                    if count >= 0 {
                        row.right = fmt::plural(count, "item", "items");
                    }
                }
                Kind::Set => {
                    row.icon = "set";
                    row.tint = Tint::Accent;
                    row.sub = format!("Encrypted set \u{b7} {when}");
                }
                Kind::Archive => {
                    row.size = size;
                    row.icon = if enc { "lock" } else { "archive" };
                    let label = if enc { "Encrypted archive" } else { "Archive" };
                    row.sub = format!("{label} \u{b7} {when}");
                    row.right = fmt::size(size);
                }
                Kind::File => {
                    let (icon, color) = icons::for_file(&row.title);
                    row.size = size;
                    row.icon = icon;
                    row.tint = color.map_or(Tint::Text2, Tint::Rgb);
                    row.right = fmt::size(size);
                }
            }
            row
        })
        .collect()
}
