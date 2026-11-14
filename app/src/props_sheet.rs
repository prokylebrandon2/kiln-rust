//! The Properties sheet: the rows come from `props.rs`. The slow ones (a folder's totals, the MD5,
//! an archive's table) say "Calculating..." and are filled in from worker threads as they finish;
//! closing the sheet stops them, and an answer that arrives later is dropped.
//!
//! What the sheet shows first is worked out by [`plan_one`] and [`plan_several`], which touch no
//! window and no thread, so they are tested; [`App::open_props`] then shows the rows and starts the
//! workers the plan asks for.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kiln_state::Kind;
use kiln_ui_next::{PropRow, PropsState};
use serde_json::json;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{App, app_do, props};

/// `PropRow::kind`: the item's name, a heading, a "label / value" line.
const NAME: i32 = 0;
const HEADING: i32 = 1;
const ROW: i32 = 2;

const CALCULATING: &str = "Calculating...";

/// The Properties sheet that is up: what stops its workers, its rows, and the folder whose
/// `.nomedia` the check box changes.
#[derive(Default)]
pub(crate) struct Open {
    stop: Arc<AtomicBool>,
    model: Option<Rc<VecModel<PropRow>>>,
    nomedia: Option<PathBuf>,
}

/// A walk that counts files and bytes (and folders, for a selection) into three rows.
#[derive(Debug, PartialEq)]
struct Totals {
    roots: Vec<PathBuf>,
    /// Count the roots that are folders as well (a selection of several, not one folder).
    folders_too: bool,
    count: usize,
    size: usize,
    folders: Option<usize>,
}

/// The first rows of the sheet and the slow rows that something fills in later.
#[derive(Debug, Default)]
struct Plan {
    rows: Vec<PropRow>,
    totals: Option<Totals>,
    /// The MD5 row of a file.
    md5: Option<usize>,
    /// The first of an archive's four rows (Encrypted, File count, Content size, Compression
    /// ratio) and the archive.
    probe: Option<(usize, PathBuf)>,
    /// The folder whose `.nomedia` box is shown.
    folder: Option<PathBuf>,
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

/// Add a "label / value" line and return its index, so a worker can fill it in later.
fn row(rows: &mut Vec<PropRow>, label: &str, value: &str) -> usize {
    rows.push(PropRow {
        label: label.into(),
        value: value.into(),
        kind: ROW,
    });
    rows.len() - 1
}

/// The rows for one item. `utc_offset` gives the zone's offset from UTC for a time.
fn plan_one(path: &Path, kind: Kind, utc_offset: &dyn Fn(i64) -> i32) -> Plan {
    let mut plan = Plan::default();
    let rows = &mut plan.rows;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    rows.push(PropRow {
        label: name.as_ref().into(),
        kind: NAME,
        ..Default::default()
    });
    heading(rows, "General information");
    // A set is a folder of archives, so (as in the Java) it shows the rows of a folder.
    let dir = path.is_dir();
    let type_text = if dir {
        "Folder".to_string()
    } else {
        props::type_of(path, kind_code(kind))
    };
    row(rows, "Type", &type_text);
    let count = dir.then(|| row(rows, "File count", CALCULATING));
    let size_text = if dir {
        CALCULATING.to_string()
    } else {
        props::size_text(file_len(path))
    };
    let size = row(rows, "Size", &size_text);
    let time = props::modified(path);
    row(
        rows,
        "Last modification",
        &props::modified_text(time, utc_offset(time)),
    );
    if let Some(permissions) = props::permissions(path) {
        row(rows, "Permissions", &permissions);
    }
    if let Some((mount, fs_type)) = props::mount_of(path) {
        row(rows, "FS path", &mount);
        row(rows, "FS type", &fs_type);
    }
    if let Some(count) = count {
        plan.totals = Some(Totals {
            roots: vec![path.to_path_buf()],
            folders_too: false,
            count,
            size,
            folders: None,
        });
        plan.folder = Some(path.to_path_buf());
        return plan;
    }
    plan.md5 = Some(row(rows, "MD5", CALCULATING));
    if kind == Kind::Archive {
        heading(rows, "Archive properties");
        row(rows, "Type", "Kiln archive");
        let first = row(rows, "Encrypted", "...");
        row(rows, "File count", CALCULATING);
        row(rows, "Content size", CALCULATING);
        row(rows, "Compression ratio", CALCULATING);
        plan.probe = Some((first, path.to_path_buf()));
    }
    plan
}

/// What a selection of more than one adds up to.
fn plan_several(picked: &[(PathBuf, Kind)]) -> Plan {
    let mut plan = Plan::default();
    let rows = &mut plan.rows;
    heading(rows, "General information");
    row(rows, "Items", &format!("{} selected", picked.len()));
    if let Some(parent) = picked.first().and_then(|(p, _)| p.parent()) {
        row(rows, "Location", &parent.to_string_lossy());
    }
    let count = row(rows, "File count", CALCULATING);
    let size = row(rows, "Size", CALCULATING);
    let folders = row(rows, "Folders", CALCULATING);
    plan.totals = Some(Totals {
        roots: picked.iter().map(|(p, _)| p.clone()).collect(),
        folders_too: true,
        count,
        size,
        folders: Some(folders),
    });
    plan
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

/// Count the folder, or the selection, and show the counts as they grow.
fn spawn_totals(stop: &Arc<AtomicBool>, t: Totals) {
    let stop = stop.clone();
    std::thread::spawn(move || {
        let paths: Vec<&Path> = t.roots.iter().map(PathBuf::as_path).collect();
        let mut report = |n: props::Totals, busy: bool| {
            let dots = if busy { "..." } else { "" };
            let bytes = props::size_text(i64::try_from(n.bytes).unwrap_or(i64::MAX));
            post(&stop, t.count, format!("{}{dots}", n.files));
            post(&stop, t.size, format!("{bytes}{dots}"));
            if let Some(i) = t.folders {
                post(&stop, i, format!("{}{dots}", n.folders));
            }
        };
        props::totals(&paths, t.folders_too, &stop, &mut report);
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
    /// Connect the sheet's buttons and its check box.
    pub(crate) fn wire_props(&self) {
        let state = self.ui.global::<PropsState>();
        state.on_copy_clicked(|| app_do(|a| a.props_copy()));
        state.on_ok_clicked(|| {
            app_do(|a| {
                a.close_props();
            });
        });
        state.on_nomedia_toggled(|on| app_do(|a| a.props_nomedia(on)));
    }

    /// Open the sheet for `picked` (the selected rows that are in view, with their paths).
    pub(crate) fn open_props(&self, picked: &[(PathBuf, Kind)]) {
        let plan = match picked {
            [] => return,
            [(path, kind)] => plan_one(path, *kind, &|t| self.host.utc_offset(t)),
            _ => plan_several(picked),
        };
        let Plan {
            rows,
            totals,
            md5,
            probe,
            folder,
        } = plan;

        let stop = Arc::new(AtomicBool::new(false));
        if let Some(t) = totals {
            spawn_totals(&stop, t);
        }
        if let (Some(index), [(path, _)]) = (md5, picked) {
            spawn_md5(&stop, path.clone(), index);
        }
        if let Some((first, path)) = probe {
            spawn_probe(&stop, path, first);
        }

        let model = Rc::new(VecModel::from(rows));
        let state = self.ui.global::<PropsState>();
        state.set_rows(ModelRc::from(model.clone()));
        state.set_has_nomedia(folder.is_some());
        if let Some(dir) = &folder {
            state.set_nomedia(dir.join(".nomedia").exists());
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
        state.set_open(true);
    }

    /// A worker's answer: change the value of line `index`.
    fn props_set(&self, index: usize, text: &str) {
        let model = self.props.borrow().model.clone();
        if let Some(model) = model
            && let Some(mut row) = model.row_data(index)
        {
            row.value = text.into();
            model.set_row_data(index, row);
        }
    }

    /// Copy: the lines as they are now, to the clipboard. The sheet stays up.
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
        }
        self.ui.global::<PropsState>().set_nomedia(file.exists());
    }

    /// Close the sheet and stop what it was counting; false when none was open.
    pub(crate) fn close_props(&self) -> bool {
        let state = self.ui.global::<PropsState>();
        let was_open = state.get_open();
        {
            let mut open = self.props.borrow_mut();
            open.stop.store(true, Ordering::Relaxed);
            open.model = None;
            open.nomedia = None;
        }
        if was_open {
            state.set_open(false);
        }
        was_open
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A scratch folder under the system temp dir, removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kiln-props-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn utc(_: i64) -> i32 {
        0
    }

    fn value<'a>(rows: &'a [PropRow], label: &str) -> Option<&'a str> {
        rows.iter()
            .find(|r| r.kind == ROW && r.label == label)
            .map(|r| r.value.as_str())
    }

    #[test]
    fn a_file_shows_its_facts_and_waits_for_its_md5() {
        let tmp = Scratch::new("file");
        let path = tmp.0.join("notes.txt");
        fs::write(&path, b"hello").unwrap();
        let plan = plan_one(&path, Kind::File, &utc);
        assert_eq!(plan.rows[0].kind, NAME);
        assert_eq!(plan.rows[0].label, "notes.txt");
        assert_eq!(plan.rows[1].label, "General information");
        assert_eq!(value(&plan.rows, "Type"), Some("text/plain"));
        assert_eq!(value(&plan.rows, "Size"), Some("5 B"));
        assert_ne!(value(&plan.rows, "Last modification"), Some(""));
        let md5 = plan.md5.expect("a file gets an MD5 row");
        assert_eq!(plan.rows[md5].label, "MD5");
        assert_eq!(plan.rows[md5].value, CALCULATING);
        assert!(plan.totals.is_none() && plan.probe.is_none() && plan.folder.is_none());
    }

    #[test]
    fn a_folder_counts_in_the_background_and_offers_nomedia() {
        let tmp = Scratch::new("folder");
        let plan = plan_one(&tmp.0, Kind::Dir, &utc);
        assert_eq!(value(&plan.rows, "Type"), Some("Folder"));
        let t = plan.totals.expect("a folder is counted");
        assert_eq!(plan.rows[t.count].label, "File count");
        assert_eq!(plan.rows[t.size].label, "Size");
        assert_eq!(plan.rows[t.count].value, CALCULATING);
        assert_eq!(plan.rows[t.size].value, CALCULATING);
        assert!(!t.folders_too && t.folders.is_none());
        assert_eq!(plan.folder.as_deref(), Some(tmp.0.as_path()));
        assert!(plan.md5.is_none(), "a folder has no MD5");
    }

    #[test]
    fn an_archive_adds_four_rows_for_the_probe_to_fill() {
        let tmp = Scratch::new("archive");
        let path = tmp.0.join("backup.kiln");
        fs::write(&path, b"not really an archive").unwrap();
        let plan = plan_one(&path, Kind::Archive, &utc);
        assert_eq!(value(&plan.rows, "Type"), Some("application/x-kiln"));
        let (first, probed) = plan.probe.expect("an archive is probed");
        assert_eq!(probed, path);
        let labels: Vec<&str> = plan.rows[first..first + 4]
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "Encrypted",
                "File count",
                "Content size",
                "Compression ratio"
            ]
        );
        assert!(plan.rows.iter().any(|r| r.label == "Archive properties"));
    }

    #[test]
    fn several_items_are_counted_together() {
        let tmp = Scratch::new("several");
        let picked = vec![
            (tmp.0.join("a.txt"), Kind::File),
            (tmp.0.join("b"), Kind::Dir),
        ];
        let plan = plan_several(&picked);
        assert_eq!(plan.rows[0].kind, HEADING, "no name line for several items");
        assert_eq!(value(&plan.rows, "Items"), Some("2 selected"));
        assert_eq!(
            value(&plan.rows, "Location"),
            Some(tmp.0.to_string_lossy().as_ref())
        );
        let t = plan.totals.expect("a selection is counted");
        assert!(t.folders_too);
        assert_eq!(t.roots.len(), 2);
        assert_eq!(plan.rows[t.folders.unwrap()].label, "Folders");
        assert!(plan.folder.is_none(), "the box is for a single folder");
    }

    #[test]
    fn the_time_follows_the_zone() {
        let tmp = Scratch::new("zone");
        let path = tmp.0.join("f.txt");
        fs::write(&path, b"x").unwrap();
        let utc_text = plan_one(&path, Kind::File, &utc);
        let east = plan_one(&path, Kind::File, &|_| 9 * 3600);
        // nine hours on is never the same time of day
        assert_ne!(
            value(&utc_text.rows, "Last modification"),
            value(&east.rows, "Last modification")
        );
    }
}
