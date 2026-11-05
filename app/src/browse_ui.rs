//! The file browser on screen: what the Slint window shows of a [`Browser`](crate::browser), and
//! what its taps do (`MainActivity.redraw`, `renderHeader`, `onRowClick`, `loadDir`, ...).

use std::path::PathBuf;
use std::time::Duration;

use kiln_ui::RowData;
use serde_json::{Value, json};
use slint::{Color, Timer};

use crate::browser::{BackAction, Kind, Row, SortKey, Tint};
use crate::{App, app_do};

/// `MainActivity.toast`: `Toast.LENGTH_SHORT` is two seconds.
const TOAST_MS: u64 = 2000;
/// A folder that takes longer than this shows "Loading...".
const LOADING_MS: u64 = 700;
/// The "+ New" button hides while the list moves this far down, and shows when it moves up.
const SCROLL_STEP_DP: f32 = 6.0;

/// One row of the list as the Slint list wants it.
fn row_data(row: &Row, selected: bool, where_line: &str) -> RowData {
    let (tint_kind, tint) = match row.tint {
        Tint::Accent => (0, Color::default()),
        Tint::Text2 => (1, Color::default()),
        Tint::Rgb(rgb) => (
            2,
            Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
        ),
    };
    RowData {
        icon: row.icon.into(),
        tint_kind,
        tint,
        name: row.title.as_str().into(),
        detail: row.sub.as_str().into(),
        where_line: where_line.into(),
        size_text: row.right.as_str().into(),
        selected,
    }
}

impl App {
    /// Connect the file browser's callbacks.
    pub(crate) fn wire_browser(&self) {
        let ui = &self.ui;
        ui.on_row_clicked(|i| app_do(|app| app.row_clicked(i)));
        ui.on_row_long_pressed(|i| app_do(|app| app.row_long_pressed(i)));
        ui.on_list_scrolled(|y| app_do(|app| app.list_scrolled(y)));
        ui.on_left_clicked(|| app_do(|app| app.left_clicked()));
        ui.on_search_clicked(|| app_do(|app| app.search_clicked()));
        ui.on_query_edited(|text| app_do(|app| app.query_edited(&text)));
        ui.on_sort_clicked(|| app_do(|app| app.sort_clicked()));
        ui.on_sort_key_chosen(|k| app_do(|app| app.sort_key_chosen(k)));
        ui.on_sort_asc_chosen(|asc| app_do(|app| app.sort_asc_chosen(asc)));
        ui.on_sort_dismissed(|| app_do(|app| app.ui.set_sort_sheet_shown(false)));
        ui.on_back_pressed(|| app_do(|app| app.on_back()));
        self.wire_marks();
        self.wire_pull();
    }

    /// The phone's time zone offset for a time, asked once per day.
    fn offset_at(&self, epoch_seconds: i64) -> i32 {
        let day = epoch_seconds.div_euclid(86_400);
        if let Some(offset) = self.offsets.borrow().get(&day) {
            return *offset;
        }
        let offset = self.host.utc_offset(day * 86_400 + 43_200);
        self.offsets.borrow_mut().insert(day, offset);
        offset
    }

    // ---- showing ------------------------------------------------------------------------------

    /// Rebuild the list's rows (a folder, a sort or a search changed them).
    pub(crate) fn render_rows(&self) {
        let b = self.browser.borrow();
        let hits = b.searching && !b.query.is_empty();
        let where_line = if hits { "This folder" } else { "" };
        let rows: Vec<RowData> = b
            .rows
            .iter()
            .map(|r| row_data(r, b.selected.contains(&r.key), where_line))
            .collect();
        self.model.set_vec(rows);
    }

    /// Everything around the list: the top bar, the banner, the empty line, the bars and buttons.
    pub(crate) fn render_chrome(&self) {
        self.refresh_volumes(false);
        let b = self.browser.borrow();
        let status = self.status.borrow();
        let ui = &self.ui;
        let header = b.header();
        let selecting = b.selecting();
        ui.set_bar_title(header.title.into());
        ui.set_bar_sub(header.sub.into());
        ui.set_home(header.home);
        ui.set_left_icon(header.left_icon.into());
        ui.set_selecting(selecting);
        ui.set_searching(b.searching);
        ui.set_marks_ok(!selecting && b.storage_ok);
        ui.set_pull_enabled(!selecting && b.storage_ok);
        let banner = if b.plain_set {
            "Plain view of an encrypted set. Renaming or deleting files here can make \
             the set unreadable."
        } else {
            ""
        };
        ui.set_banner_text(banner.into());

        let empty = if let Some(why) = &status.engine_error {
            format!("Kiln couldn't start: {why}")
        } else if !status.engine_up {
            "Starting...".to_string()
        } else {
            b.empty_text(self.loading_hint.get())
        };
        ui.set_empty_text(empty.into());
        let up = b.up_shown();
        ui.set_up_row(up);
        ui.set_empty_top(if up { 96.0 } else { 56.0 });
        ui.set_has_where(b.searching && !b.query.is_empty());
        ui.set_bar_shown(selecting);
        let fab_wanted = !selecting && status.engine_up && b.storage_ok && !b.searching;
        ui.set_fab_shown(fab_wanted && !self.fab_hidden.get());

        // A short pulse on the mark when you arrive at home.
        let home = header.home;
        if home && !self.was_home.get() {
            ui.set_glow_run(false);
            Timer::single_shot(Duration::from_millis(16), || {
                app_do(|app| app.ui.set_glow_run(true));
            });
        }
        self.was_home.set(home);
    }

    /// `toast`.
    pub(crate) fn toast(&self, text: &str) {
        let id = self.toast_gen.get() + 1;
        self.toast_gen.set(id);
        self.ui.set_toast_text(text.into());
        self.ui.set_toast_shown(true);
        Timer::single_shot(Duration::from_millis(TOAST_MS), move || {
            app_do(|app| {
                if app.toast_gen.get() == id {
                    app.ui.set_toast_shown(false);
                }
            });
        });
    }

    // ---- loading ------------------------------------------------------------------------------

    /// `loadDir`: show `dir`. The engine lists it on a worker thread.
    pub(crate) fn load_dir(&self, dir: PathBuf, keep_selection: bool) {
        let (generation, path, all, hidden) = {
            let mut b = self.browser.borrow_mut();
            let generation = b.begin_load(dir, keep_selection);
            let path = b.cwd.to_string_lossy().into_owned();
            (generation, path, b.show_all, b.show_hidden)
        };
        self.loading_hint.set(false);
        self.render_rows();
        self.render_chrome();
        Timer::single_shot(Duration::from_millis(LOADING_MS), move || {
            app_do(|app| app.show_loading(generation));
        });
        std::thread::spawn(move || {
            let result = kiln_bridge::call("scan", &json!([path, all, hidden]), 0);
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|app| app.scan_done(generation, &result));
            });
        });
    }

    fn show_loading(&self, generation: u64) {
        let slow = {
            let b = self.browser.borrow();
            b.pending && b.generation == generation && b.rows.is_empty()
        };
        if slow {
            self.loading_hint.set(true);
            self.render_chrome();
        }
    }

    fn scan_done(&self, generation: u64, result: &Value) {
        self.ui.set_refreshing(false);
        let wanted = {
            let mut offset = |t: i64| self.offset_at(t);
            let mut b = self.browser.borrow_mut();
            b.apply_scan(generation, result, &mut offset)
        };
        if !wanted {
            return;
        }
        self.loading_hint.set(false);
        self.render_rows();
        self.render_chrome();
        // A new folder always starts at the top.
        let cwd = self.browser.borrow().cwd.clone();
        if *self.last_dir.borrow() != cwd {
            *self.last_dir.borrow_mut() = cwd;
            self.ui.invoke_scroll_list_top();
        }
    }

    // ---- what taps do -------------------------------------------------------------------------

    /// `onRowClick`. The index is into the rows, or -1 for the ".." row.
    fn row_clicked(&self, index: i32) {
        let (key, kind, selecting, pending) = {
            let b = self.browser.borrow();
            let row = usize::try_from(index).ok().and_then(|i| b.rows.get(i));
            let found = row.map(|r| (r.key.clone(), r.kind));
            let (key, kind) = found.unzip();
            (key, kind, b.selecting(), b.pending)
        };
        if pending {
            return;
        }
        let Some(key) = key else {
            // the ".." row
            if !selecting {
                self.on_back();
            }
            return;
        };
        if selecting {
            self.toggle(&key);
            return;
        }
        match kind {
            Some(Kind::Dir) => {
                let dir = self.browser.borrow().cwd.join(&key);
                self.load_dir(dir, false);
            }
            // Opening archives and files comes with the next part of Phase B.
            _ => self.toast("Not in this build yet"),
        }
    }

    /// A long press selects a row (`setOnItemLongClickListener`).
    fn row_long_pressed(&self, index: i32) {
        let key = {
            let b = self.browser.borrow();
            if b.pending {
                return;
            }
            usize::try_from(index)
                .ok()
                .and_then(|i| b.rows.get(i))
                .map(|r| r.key.clone())
        };
        if let Some(key) = key {
            self.toggle(&key);
        }
    }

    fn toggle(&self, key: &str) {
        self.browser.borrow_mut().toggle(key);
        self.render_rows();
        self.render_chrome();
    }

    /// `onLeft`: leave selection, or go back; at home the mark only pulses (About comes later).
    fn left_clicked(&self) {
        let (selecting, home) = {
            let b = self.browser.borrow();
            (b.selecting(), !b.searching && b.is_storage_root(&b.cwd))
        };
        if selecting {
            self.browser.borrow_mut().clear_selection();
            self.render_rows();
            self.render_chrome();
        } else if home {
            self.was_home.set(false);
            self.render_chrome();
        } else {
            self.on_back();
        }
    }

    /// `onBackPressed`.
    pub(crate) fn on_back(&self) {
        // A dialog is a window of its own in the Java: Back closes it first.
        if self.dialog_back() {
            return;
        }
        // The bookmarks drop-down is a window of its own in the Java: Back closes it first.
        if self.ui.get_marks_shown() {
            self.ui.set_marks_shown(false);
            return;
        }
        let action = self.browser.borrow_mut().back();
        match action {
            BackAction::Changed => {
                self.render_rows();
                self.render_chrome();
            }
            BackAction::Up(dir) => self.load_dir(dir, false),
            BackAction::ExitHint => self.toast("Tap Back again to exit"),
            BackAction::Exit => self.host.finish(),
        }
    }

    fn search_clicked(&self) {
        {
            let mut b = self.browser.borrow_mut();
            if b.searching {
                b.close_search();
            } else if self.status.borrow().engine_up && b.storage_ok {
                b.open_search();
            } else {
                return;
            }
        }
        self.render_rows();
        self.render_chrome();
    }

    fn query_edited(&self, text: &str) {
        self.browser.borrow_mut().set_query(text);
        self.render_rows();
        self.render_chrome();
        self.ui.invoke_scroll_list_top();
    }

    fn sort_clicked(&self) {
        let (key, asc) = {
            let b = self.browser.borrow();
            (b.sort_key, b.sort_asc)
        };
        self.ui.set_sort_key(key.code());
        self.ui.set_sort_asc(asc);
        self.ui.set_sort_sheet_shown(true);
    }

    fn sort_key_chosen(&self, code: i32) {
        let asc = self.browser.borrow().sort_asc;
        self.set_sort(SortKey::from_code(code), asc);
    }

    fn sort_asc_chosen(&self, asc: bool) {
        let key = self.browser.borrow().sort_key;
        self.set_sort(key, asc);
    }

    /// `saveSort`.
    fn set_sort(&self, key: SortKey, asc: bool) {
        self.prefs.set_int("sort_key", key.code());
        self.prefs.set_bool("sort_asc", asc);
        self.browser.borrow_mut().set_sort(key, asc);
        self.ui.set_sort_sheet_shown(false);
        self.render_rows();
        self.render_chrome();
    }

    /// "+ New" hides while the list scrolls down and shows again on the way up (`onScroll`).
    fn list_scrolled(&self, y: f32) {
        let last = self.scroll_last.get();
        let hide = if y <= 0.0 {
            false
        } else if (y - last).abs() >= SCROLL_STEP_DP {
            y > last
        } else {
            return;
        };
        self.scroll_last.set(y);
        if self.fab_hidden.get() != hide {
            self.fab_hidden.set(hide);
            self.render_chrome();
        }
    }
}
