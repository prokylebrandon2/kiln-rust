//! The folder chooser (`MainActivity.pickFolder`): a sheet with the path, the folders of one
//! directory, "New folder..." and the other storages. "Select this folder" hands the folder to
//! whoever asked. (`Priv` is not in this build, so no folder is read through it.)

use std::path::{Path, PathBuf};

use kiln_ui::FcRow;
use slint::{ModelRc, SharedString, VecModel};

use crate::{App, app_do};

/// What the chooser does with the folder that was picked (`OnFile`).
type Picked = Box<dyn FnOnce(&App, PathBuf)>;

// `Pick` types: "New folder...", "..", a folder, a file (dimmed), another storage, and a file
// that can be picked (`pickFile`).
const NEW: i32 = 0;
const UP: i32 = 1;
const FILE: i32 = 3;
const VOL: i32 = 4;
const PICKABLE: i32 = 5;

/// The chooser that is up.
#[derive(Default)]
pub(crate) struct Chooser {
    /// The folder that is shown (`cur[0]`).
    cur: PathBuf,
    /// The chooser picks a file (`FilePicker.open`) instead of a folder.
    files: bool,
    /// What each row is: its type and the folder or file it leads to.
    entries: Vec<(i32, PathBuf)>,
    done: Option<Picked>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn fc_row(icon: &str, label: &str, kind: i32) -> FcRow {
    FcRow {
        icon: icon.into(),
        label: label.into(),
        kind,
    }
}

impl App {
    pub(crate) fn wire_folder(&self) {
        let ui = &self.ui;
        ui.on_fc_row_clicked(|i| app_do(|app| app.fc_row_clicked(i)));
        ui.on_fc_button_clicked(|i| app_do(|app| app.fc_button(i)));
        ui.on_fc_cancelled(|| app_do(|app| app.fc_cancelled()));
    }

    /// `pickFolder`: start at `start` (or the nearest folder above it, or the storage root).
    pub(crate) fn pick_folder(&self, start: PathBuf, done: impl FnOnce(&App, PathBuf) + 'static) {
        self.open_chooser("Choose a folder", false, start, Box::new(done));
    }

    /// `FilePicker.open`: the same sheet, but a tap on a file picks it. There is no "New folder..."
    /// row and no "Select this folder" button.
    pub(crate) fn pick_file(
        &self,
        title: &str,
        start: PathBuf,
        done: impl FnOnce(&App, PathBuf) + 'static,
    ) {
        self.open_chooser(title, true, start, Box::new(done));
    }

    fn open_chooser(&self, title: &str, files: bool, start: PathBuf, done: Picked) {
        if !self.browser.borrow().storage_ok {
            self.host.open_all_files_settings();
            return;
        }
        let cur = start
            .ancestors()
            .find(|d| d.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.browser.borrow().root().to_path_buf());
        *self.chooser.borrow_mut() = Chooser {
            cur,
            files,
            entries: Vec::new(),
            done: Some(done),
        };
        let ui = &self.ui;
        ui.set_fc_title(title.into());
        ui.set_fc_files(files);
        ui.set_fc_naming(false);
        ui.set_fc_name(SharedString::new());
        ui.set_fc_name_error(SharedString::new());
        self.fc_fill();
        ui.set_fc_shown(true);
    }

    /// The `fill` runnable: the rows for the folder that is shown.
    fn fc_fill(&self) {
        let (cur, files_mode) = {
            let c = self.chooser.borrow();
            (c.cur.clone(), c.files)
        };
        let mut entries: Vec<(i32, PathBuf)> = Vec::new();
        let mut rows = Vec::new();
        if !files_mode {
            entries.push((NEW, PathBuf::new()));
            rows.push(fc_row("new_folder", "New folder...", NEW));
        }
        {
            let b = self.browser.borrow();
            let at_root = b.is_storage_root(&cur);
            if let Some(parent) = cur.parent().filter(|_| !at_root) {
                rows.push(fc_row("up_dir", "..", UP));
                entries.push((UP, parent.to_path_buf()));
            }
            if at_root {
                // the other storages sit beside this one
                if cur.as_path() != b.root() {
                    rows.push(fc_row("home", "Internal storage", VOL));
                    entries.push((VOL, b.root().to_path_buf()));
                }
                for v in &b.volumes {
                    if v.path.as_path() != cur.as_path() {
                        rows.push(fc_row("usb", &v.label, VOL));
                        entries.push((VOL, v.path.clone()));
                    }
                }
            }
            let mut dirs: Vec<PathBuf> = Vec::new();
            let mut files: Vec<PathBuf> = Vec::new();
            if let Ok(read) = std::fs::read_dir(&cur) {
                for kid in read.flatten() {
                    if kid.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    let path = kid.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else {
                        files.push(path);
                    }
                }
            }
            dirs.sort_by_cached_key(|p| file_name(p).to_lowercase());
            files.sort_by_cached_key(|p| file_name(p).to_lowercase());
            let parent_name = file_name(&cur);
            for path in dirs {
                let name = file_name(&path);
                // `dirIcon`: the picked icon, else the folder's standard one
                let icon = match self.prefs.folder_icon(&path.to_string_lossy()) {
                    Some(key) => crate::icons::by_key(&key),
                    None => crate::icons::for_dir(&name, &parent_name, at_root),
                };
                rows.push(fc_row(icon, &name, 2));
                entries.push((2, path));
            }
            for path in files {
                let name = file_name(&path);
                let kind = if files_mode { PICKABLE } else { FILE };
                rows.push(fc_row(crate::icons::for_file(&name).0, &name, kind));
                entries.push((kind, path));
            }
        }
        self.chooser.borrow_mut().entries = entries;
        self.ui
            .set_fc_path(cur.to_string_lossy().into_owned().into());
        self.ui.set_fc_rows(ModelRc::new(VecModel::from(rows)));
    }

    fn fc_row_clicked(&self, index: i32) {
        let entry = {
            let c = self.chooser.borrow();
            usize::try_from(index)
                .ok()
                .and_then(|i| c.entries.get(i).cloned())
        };
        let Some((kind, path)) = entry else { return };
        match kind {
            FILE => {}
            PICKABLE => {
                let done = self.chooser.borrow_mut().done.take();
                self.fc_close();
                if let Some(done) = done {
                    done(self, path);
                }
            }
            NEW => {
                let ui = &self.ui;
                ui.set_fc_name(SharedString::new());
                ui.set_fc_name_error(SharedString::new());
                ui.set_fc_naming(true);
            }
            _ => {
                self.chooser.borrow_mut().cur = path;
                self.fc_fill();
            }
        }
    }

    /// The buttons: 0 Cancel, 1 "Select this folder" (or Create while a name is asked for).
    fn fc_button(&self, index: i32) {
        if self.ui.get_fc_naming() {
            if index == 0 {
                self.ui.set_fc_naming(false);
            } else {
                self.fc_create();
            }
            return;
        }
        if index == 0 {
            self.fc_close();
            return;
        }
        let cur = self.chooser.borrow().cur.clone();
        let done = self.chooser.borrow_mut().done.take();
        self.fc_close();
        if let Some(done) = done {
            done(self, cur);
        }
    }

    /// Back or a tap outside: back from the name field to the list, else close.
    pub(crate) fn fc_cancelled(&self) {
        if self.ui.get_fc_naming() {
            self.ui.set_fc_naming(false);
        } else {
            self.fc_close();
        }
    }

    fn fc_close(&self) {
        let mut c = self.chooser.borrow_mut();
        c.entries.clear();
        c.done = None;
        drop(c);
        self.ui.set_fc_naming(false);
        self.ui.set_fc_shown(false);
    }

    /// "New folder...": create it, go into it and show the list again.
    fn fc_create(&self) {
        let ui = &self.ui;
        let name = ui.get_fc_name().trim().to_string();
        if name.is_empty() || name.contains('/') {
            ui.set_fc_name_error("Enter a name (no slashes)".into());
            return;
        }
        let new_dir = self.chooser.borrow().cur.join(&name);
        if new_dir.exists() {
            ui.set_fc_name_error("That name is taken".into());
            return;
        }
        if std::fs::create_dir_all(&new_dir).is_err() {
            ui.set_fc_name_error("Couldn't create it".into());
            return;
        }
        self.chooser.borrow_mut().cur = new_dir;
        ui.set_fc_naming(false);
        self.fc_fill();
    }
}
