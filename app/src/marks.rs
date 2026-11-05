//! The bookmarks drop-down under the title (`MainActivity.showBookmarks`, `markRow`, `goTo`), and
//! the list of USB drives and SD cards it shares with the rest of the browser.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kiln_ui::MarkRow;
use slint::Model;

use crate::browser::Browser;
use crate::{App, app_do, fmt, icons};

/// `volumes()` asks the phone for its USB drives and SD cards at most this often.
const VOLUMES_TTL: Duration = Duration::from_millis(3000);

/// What a folder is called on screen: its name, or the whole path for a storage root.
fn display_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

impl App {
    /// Connect the bookmarks drop-down's callbacks.
    pub(crate) fn wire_marks(&self) {
        let ui = &self.ui;
        ui.on_titles_clicked(|| app_do(|app| app.marks_clicked()));
        ui.on_marks_dismissed(|| app_do(|app| app.ui.set_marks_shown(false)));
        ui.on_mark_clicked(|i| app_do(|app| app.mark_clicked(i)));
        ui.on_mark_removed(|i| app_do(|app| app.mark_removed(i)));
        ui.on_mark_icon_clicked(|_| app_do(|app| app.toast("Not in this build yet")));
        ui.on_mark_action_clicked(|| app_do(|app| app.mark_action_clicked()));
    }

    /// `volumes()`: tell the browser which USB drives and SD cards are plugged in. The phone is
    /// asked at most every 3 seconds unless `force` is set.
    pub(crate) fn refresh_volumes(&self, force: bool) {
        let fresh = self
            .volumes_at
            .get()
            .is_some_and(|at| at.elapsed() < VOLUMES_TTL);
        if fresh && !force {
            return;
        }
        self.volumes_at.set(Some(Instant::now()));
        let volumes = self.host.volumes();
        self.browser.borrow_mut().volumes = volumes;
    }

    /// `showBookmarks`: open the drop-down (or close it if it is open). It needs access to the
    /// files and nothing selected.
    pub(crate) fn marks_clicked(&self) {
        if self.ui.get_marks_shown() {
            self.ui.set_marks_shown(false);
            return;
        }
        {
            let b = self.browser.borrow();
            if !b.storage_ok || b.selecting() {
                return;
            }
        }
        self.refresh_volumes(true);
        let saved = self.prefs.bookmarks();
        let b = self.browser.borrow();
        let cwd = b.cwd.to_string_lossy().into_owned();
        let mut rows = vec![self.storage_row(&b, "home", b.root(), &cwd)];
        for volume in &b.volumes {
            rows.push(self.storage_row(&b, "usb", &volume.path, &cwd));
        }
        for path in &saved {
            rows.push(self.bookmark_row(&b, path, &cwd));
        }
        self.marks_model.set_vec(rows);
        let here = b.cwd.to_string_lossy().into_owned();
        let has_action = !b.is_storage_root(&b.cwd);
        let marked = saved.contains(&here);
        let ui = &self.ui;
        ui.set_mark_has_action(has_action);
        ui.set_mark_action_icon(if marked { "close" } else { "add" }.into());
        let label = if marked {
            "Remove this bookmark"
        } else {
            "Bookmark this folder"
        };
        ui.set_mark_action_label(label.into());
        ui.set_marks_shown(true);
    }

    /// A storage line: its name, its path and what is free on it.
    fn storage_row(&self, b: &Browser, icon: &str, path: &Path, cwd: &str) -> MarkRow {
        let right = match self.host.space(path) {
            Some((free, total)) => {
                let (free, total) = (fmt::size(free as i64), fmt::size(total as i64));
                format!("{free} free\nof {total}")
            }
            None => String::new(),
        };
        let text = path.to_string_lossy().into_owned();
        MarkRow {
            icon: icon.into(),
            name: b.storage_name(path).into(),
            current: text == cwd,
            path: text.into(),
            right: right.into(),
            removable: false,
        }
    }

    /// A bookmark's line: the folder's icon (picked, or its standard one), name and path.
    fn bookmark_row(&self, b: &Browser, path: &str, cwd: &str) -> MarkRow {
        let dir = Path::new(path);
        MarkRow {
            icon: self.dir_icon(b, dir).into(),
            name: display_name(dir).into(),
            path: path.into(),
            right: "".into(),
            removable: true,
            current: path == cwd,
        }
    }

    /// The icon of a folder: the one a person picked, else its standard one (`Icons.forDir`).
    fn dir_icon(&self, b: &Browser, dir: &Path) -> &'static str {
        if let Some(key) = self.prefs.folder_icon(&dir.to_string_lossy()) {
            return icons::by_key(&key);
        }
        let parent = dir.parent();
        let at_root = parent.is_some_and(|p| b.is_storage_root(p));
        let parent_name = parent.map(display_name).unwrap_or_default();
        icons::for_dir(&display_name(dir), &parent_name, at_root)
    }

    /// The path of a line of the drop-down, and whether it is a bookmark.
    fn mark_at(&self, index: i32) -> Option<MarkRow> {
        let i = usize::try_from(index).ok()?;
        self.marks_model.row_data(i)
    }

    /// A line was tapped: close the drop-down and go there.
    fn mark_clicked(&self, index: i32) {
        self.ui.set_marks_shown(false);
        if let Some(row) = self.mark_at(index) {
            self.go_to(&row.path);
        }
    }

    /// `goTo`: open a folder, unless it is gone or already open.
    fn go_to(&self, path: &str) {
        let dir = PathBuf::from(path);
        if !dir.is_dir() {
            self.toast("That folder isn't there any more");
            return;
        }
        if dir == self.browser.borrow().cwd {
            return;
        }
        self.load_dir(dir, false);
    }

    /// A bookmark's cross.
    fn mark_removed(&self, index: i32) {
        self.ui.set_marks_shown(false);
        if let Some(row) = self.mark_at(index) {
            self.prefs.remove_bookmark(&row.path);
            self.toast("Bookmark removed");
        }
    }

    /// "Bookmark this folder" or "Remove this bookmark".
    fn mark_action_clicked(&self) {
        self.ui.set_marks_shown(false);
        let cwd = self.browser.borrow().cwd.clone();
        let here = cwd.to_string_lossy().into_owned();
        if self.prefs.bookmarks().contains(&here) {
            self.prefs.remove_bookmark(&here);
            self.toast("Bookmark removed");
        } else {
            self.prefs.add_bookmark(&here);
            self.toast(&format!("Bookmarked {}", display_name(&cwd)));
        }
    }
}
