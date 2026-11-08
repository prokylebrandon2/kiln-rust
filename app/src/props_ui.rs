//! The Properties sheet: the rows come from `props.rs`. The slow ones (a folder's
//! totals, the MD5, the archive's table) say "Calculating..." and are filled in from worker threads
//! as they finish; closing the sheet stops them.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kiln_ui::PropRow;
use serde_json::json;
use slint::{Model, ModelRc, VecModel};

use crate::browser::Kind;
use crate::{App, app_do, props};

/// `PropRow::kind`: the name line, a heading, a "label  value" row.
const NAME: i32 = 0;
const HEADING: i32 = 1;
const ROW: i32 = 2;

/// The Properties sheet that is up: what stops its workers, its rows, and the folder whose
/// `.nomedia` the check box changes.
#[derive(Default)]
pub(crate) struct Open {
    stop: Arc<AtomicBool>,
    model: Option<Rc<VecModel<PropRow>>>,
    nomedia: Option<PathBuf>,
}

fn kind_code(kind: Kind) -> &'static str {
    match kind {
        Kind::Dir => "dir",
        Kind::Set => "set",
        Kind::Archive => "archive",
        Kind::File => "file",
    }
}

fn file_len(path: &Path) -> i64 {
    std::fs::metadata(path).map_or(0, |m| i64::try_from(m.len()).unwrap_or(i64::MAX))
}

fn heading(rows: &mut Vec<PropRow>, text: &str) {
    rows.push(PropRow {
        label: text.into(),
        kind: HEADING,
        ..Default::default()
    });
}

/// Add a "label  value" row and return its index, so a worker can fill it in later.
fn row(rows: &mut Vec<PropRow>, label: &str, value: &str) -> usize {
    rows.push(PropRow {
        label: label.into(),
        value: value.into(),
        kind: ROW,
    });
    rows.len() - 1
}

/// From a worker thread: change one value. Nothing happens once the sheet is closed.
fn post(stop: &Arc<AtomicBool>, index: usize, text: String) {
    let stop = stop.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if !stop.load(Ordering::Relaxed) {
            app_do(|app| app.props_set(index, &text));
        }
    });
}

/// Count a folder, or the selection, and show the counts as they grow.
fn spawn_totals(
    stop: &Arc<AtomicBool>,
    roots: Vec<PathBuf>,
    folders_too: bool,
    count: usize,
    size: usize,
    folders: Option<usize>,
) {
    let stop = stop.clone();
    std::thread::spawn(move || {
        let paths: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
        let mut report = |t: props::Totals, busy: bool| {
            let dots = if busy { "..." } else { "" };
            let bytes = props::size_text(i64::try_from(t.bytes).unwrap_or(i64::MAX));
            post(&stop, count, format!("{}{dots}", t.files));
            post(&stop, size, format!("{bytes}{dots}"));
            if let Some(i) = folders {
                post(&stop, i, format!("{}{dots}", t.folders));
            }
        };
        props::totals(&paths, folders_too, &stop, &mut report);
    });
}

/// The MD5 row. Nothing is shown if the sheet is closed meanwhile.
fn spawn_md5(stop: &Arc<AtomicBool>, path: PathBuf, index: usize) {
    let stop = stop.clone();
    std::thread::spawn(move || {
        let text = match props::md5_of(&path, &stop) {
            Ok(Some(digest)) => digest,
            Ok(None) => return,
            Err(_) => "unavailable".to_string(),
        };
        post(&stop, index, text);
    });
}

/// The four archive rows, from `first` on: Encrypted, File count, Content size, Compression ratio.
fn spawn_probe(stop: &Arc<AtomicBool>, path: PathBuf, first: usize) {
    let stop = stop.clone();
    std::thread::spawn(move || {
        let answer = kiln_bridge::call("open_archive", &json!([path.to_string_lossy(), ""]), 0);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let facts = props::archive_facts(&answer, file_len(&path));
        let texts = [facts.encrypted, facts.files, facts.content, facts.ratio];
        for (i, text) in texts.into_iter().enumerate() {
            post(&stop, first + i, text);
        }
    });
}

impl App {
    /// For files and folders: the sheet for what is selected.
    pub(crate) fn props_selected(&self) {
        let picked: Vec<(PathBuf, Kind)> = {
            let b = self.browser.borrow();
            b.selected
                .iter()
                .filter_map(|key| {
                    let row = b.rows.iter().find(|r| &r.key == key)?;
                    Some((b.cwd.join(key), row.kind))
                })
                .collect()
        };
        if picked.is_empty() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let mut rows = Vec::new();
        let folder = match picked.as_slice() {
            [(path, kind)] => self.single_rows(&mut rows, &stop, path, *kind),
            _ => {
                several_rows(&mut rows, &stop, &picked);
                None
            }
        };
        let model = Rc::new(VecModel::from(rows));
        let ui = &self.ui;
        ui.set_props_rows(ModelRc::from(model.clone()));
        ui.set_props_has_nomedia(folder.is_some());
        if let Some(dir) = &folder {
            ui.set_props_nomedia(dir.join(".nomedia").exists());
        }
        {
            let mut open = self.props.borrow_mut();
            open.stop.store(true, Ordering::Relaxed);
            *open = Open {
                stop,
                model: Some(model),
                nomedia: folder,
            };
        }
        ui.set_props_shown(true);
    }

    /// Returns the folder when it is one (the check box is for folders).
    fn single_rows(
        &self,
        rows: &mut Vec<PropRow>,
        stop: &Arc<AtomicBool>,
        path: &Path,
        kind: Kind,
    ) -> Option<PathBuf> {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        rows.push(PropRow {
            label: name.as_ref().into(),
            kind: NAME,
            ..Default::default()
        });
        heading(rows, "General information");
        let dir = path.is_dir();
        let type_text = if dir {
            "Folder".to_string()
        } else {
            props::type_of(path, kind_code(kind))
        };
        row(rows, "Type", &type_text);
        let count = dir.then(|| row(rows, "File count", "Calculating..."));
        let size_text = if dir {
            "Calculating...".to_string()
        } else {
            props::size_text(file_len(path))
        };
        let size = row(rows, "Size", &size_text);
        let time = props::modified(path);
        let when = props::modified_text(time, self.host.utc_offset(time));
        row(rows, "Last modification", &when);
        if let Some(permissions) = props::permissions(path) {
            row(rows, "Permissions", &permissions);
        }
        if let Some((mount, fs_type)) = props::mount_of(path) {
            row(rows, "FS path", &mount);
            row(rows, "FS type", &fs_type);
        }
        if let Some(count) = count {
            spawn_totals(stop, vec![path.to_path_buf()], false, count, size, None);
            return Some(path.to_path_buf());
        }
        let md5 = row(rows, "MD5", "Calculating...");
        spawn_md5(stop, path.to_path_buf(), md5);
        if matches!(kind, Kind::Archive | Kind::Set) {
            heading(rows, "Archive properties");
            let set = kind == Kind::Set;
            row(
                rows,
                "Type",
                if set {
                    "Kiln set (random names)"
                } else {
                    "Kiln archive"
                },
            );
            let first = row(rows, "Encrypted", if set { "Yes" } else { "..." });
            row(rows, "File count", "Calculating...");
            row(rows, "Content size", "Calculating...");
            row(rows, "Compression ratio", "Calculating...");
            spawn_probe(stop, path.to_path_buf(), first);
        }
        None
    }

    pub(crate) fn wire_props(&self) {
        let ui = &self.ui;
        ui.on_props_button_clicked(|i| app_do(|app| app.props_button(i)));
        ui.on_props_nomedia_toggled(|on| app_do(|app| app.props_nomedia(on)));
        ui.on_props_cancelled(|| app_do(|app| app.close_props()));
    }

    /// A worker's answer: change the value of row `index`.
    fn props_set(&self, index: usize, text: &str) {
        let model = self.props.borrow().model.clone();
        if let Some(model) = model
            && let Some(mut row) = model.row_data(index)
        {
            row.value = text.into();
            model.set_row_data(index, row);
        }
    }

    /// The sheet's buttons: 0 Copy (the sheet stays up), 1 OK.
    fn props_button(&self, index: i32) {
        if index == 0 {
            self.props_copy();
        } else {
            self.close_props();
        }
    }

    /// The lines as they are now, to the clipboard.
    fn props_copy(&self) {
        let lines: Vec<(String, Option<String>)> = {
            let open = self.props.borrow();
            let Some(model) = &open.model else {
                return;
            };
            model
                .iter()
                .filter_map(|r| match r.kind {
                    HEADING => Some((r.label.to_string(), None)),
                    ROW => Some((r.label.to_string(), Some(r.value.to_string()))),
                    _ => None,
                })
                .collect()
        };
        self.host.set_clipboard(&props::copy_text(&lines));
        self.toast("Copied");
    }

    /// The "Disable media scanning" box: make or remove the folder's `.nomedia` file. If that
    /// fails the box goes back to what the folder really has.
    fn props_nomedia(&self, on: bool) {
        let Some(dir) = self.props.borrow().nomedia.clone() else {
            return;
        };
        let file = dir.join(".nomedia");
        let result = if on && !file.exists() {
            std::fs::File::create_new(&file).map(drop)
        } else if !on && file.exists() {
            std::fs::remove_file(&file)
        } else {
            Ok(())
        };
        if result.is_err() {
            self.toast("Couldn't change that");
            self.ui.set_props_nomedia(file.exists());
        }
    }

    /// Close the sheet and stop what it was counting.
    pub(crate) fn close_props(&self) {
        {
            let mut open = self.props.borrow_mut();
            open.stop.store(true, Ordering::Relaxed);
            open.model = None;
            open.nomedia = None;
        }
        self.ui.set_props_shown(false);
    }
}

/// A key of the archive's list as a path: a folder's "/" at the end goes.
fn entry_path(key: &str) -> &str {
    key.strip_suffix('/').unwrap_or(key)
}

impl App {
    /// The sheet for what is selected inside the open archive. Nothing here
    /// is slow, so there are no workers; Copy and OK work as in `props_selected`.
    pub(crate) fn props_in_archive(&self) {
        let (archive, picked, offset) = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            let picked: Vec<(String, (i64, i64, i64))> = b
                .selected
                .iter()
                .map(|key| (key.clone(), a.totals_of(key)))
                .collect();
            (a.file_name(), picked, b.date_offset_secs())
        };
        if picked.is_empty() {
            return;
        }
        let mut rows = Vec::new();
        if let [(key, _)] = picked.as_slice() {
            let path = entry_path(key);
            let leaf = path.rsplit('/').next().unwrap_or(path);
            rows.push(PropRow {
                label: leaf.into(),
                kind: NAME,
                ..Default::default()
            });
        }
        heading(&mut rows, "General information");
        let size: i64 = picked.iter().map(|(_, t)| t.0).sum();
        let files: i64 = picked.iter().map(|(_, t)| t.1).sum();
        match picked.as_slice() {
            [(key, (_, _, newest))] => {
                let dir = key.ends_with('/');
                let path = entry_path(key);
                let kind = if dir {
                    "Folder".to_string()
                } else {
                    props::type_of(Path::new(path), "file")
                };
                row(&mut rows, "Type", &kind);
                row(&mut rows, "Path", path);
                if dir {
                    row(&mut rows, "File count", &files.to_string());
                }
                row(&mut rows, "Size", &props::size_text(size));
                if *newest > 0 {
                    row(
                        &mut rows,
                        "Last modification",
                        &props::modified_text(*newest, offset),
                    );
                }
            }
            _ => {
                row(&mut rows, "Items", &format!("{} selected", picked.len()));
                row(&mut rows, "File count", &files.to_string());
                row(&mut rows, "Size", &props::size_text(size));
            }
        }
        row(&mut rows, "In archive", &archive);

        let model = Rc::new(VecModel::from(rows));
        let ui = &self.ui;
        ui.set_props_rows(ModelRc::from(model.clone()));
        ui.set_props_has_nomedia(false);
        {
            let mut open = self.props.borrow_mut();
            open.stop.store(true, Ordering::Relaxed);
            *open = Open {
                stop: Arc::new(AtomicBool::new(false)),
                model: Some(model),
                nomedia: None,
            };
        }
        ui.set_props_shown(true);
    }
}
/// What a selection of more than one adds up to.
fn several_rows(rows: &mut Vec<PropRow>, stop: &Arc<AtomicBool>, picked: &[(PathBuf, Kind)]) {
    heading(rows, "General information");
    row(rows, "Items", &format!("{} selected", picked.len()));
    if let Some(parent) = picked[0].0.parent() {
        row(rows, "Location", &parent.to_string_lossy());
    }
    let count = row(rows, "File count", "Calculating...");
    let size = row(rows, "Size", "Calculating...");
    let folders = row(rows, "Folders", "Calculating...");
    let roots = picked.iter().map(|(path, _)| path.clone()).collect();
    spawn_totals(stop, roots, true, count, size, Some(folders));
}
