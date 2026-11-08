//! The Settings screen (`SettingsActivity`): the page, the sheets it opens (choices, the rows
//! wheel, the default folder colour), backup, access, the cache, and the Self-test and Benchmark
//! tools with their result sheets.
//!
//! The page (`ui/ui/settings.slint`) draws a [`SettingsState`] and reports a tap as `act(id, on)`;
//! every setting is read from and written to [`Prefs`] here. Most of them take effect when the
//! page is left ([`App::close_settings`] reads them all again).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

use kiln_ui::{BenchRow, CheckRow, OpenWithRow, SettingsState};
use serde_json::{Value, json};
use slint::{Color, ModelRc, VecModel};

use crate::browse_ui::{FOLDER_COLORS, argb, color_index};
use crate::dialog::{Action, Btn, BtnKind, Click, Opt, Spec};
use crate::prefs::open_with_label;
use crate::{App, app_do, fmt};

// What `KSettingsPage` reports (`root.act(id, on)`).
const ST_BACK: i32 = 0;
const ST_UP_ROW: i32 = 1;
const ST_PREVIEWS: i32 = 2;
const ST_ROWS: i32 = 3;
const ST_FOLDER_COLOR: i32 = 4;
const ST_ANIM: i32 = 5;
const ST_UNITS: i32 = 6;
const ST_DEEP: i32 = 7;
const ST_SEARCH_PATH: i32 = 8;
const ST_DELETING: i32 = 9;
const ST_WIPE: i32 = 10;
const ST_EXPORT: i32 = 11;
const ST_IMPORT: i32 = 12;
const ST_SELFTEST: i32 = 13;
const ST_BENCH: i32 = 14;
const ST_ACCESS: i32 = 15;
const ST_UNSAFE: i32 = 16;
const ST_CHECK: i32 = 17;
const ST_JOBS: i32 = 18;
const ST_CACHE: i32 = 19;

/// `ROWS_MAX` and `ROWS_DEFAULT`.
const ROWS_MIN: i32 = 10;
const ROWS_MAX: i32 = 24;
const ROWS_DEFAULT: i32 = 16;

/// `DELETE_NAMES` and `PATH_NAMES`.
const ANIM_NAMES: [&str; 4] = ["Off", "Fast", "Normal", "Relaxed"];
const DELETE_NAMES: [&str; 3] = ["Ask", "Recycle bin", "Permanently"];
const PATH_NAMES: [&str; 2] = ["Relative", "Absolute"];
const ACCESS_NAMES: [&str; 3] = ["Off", "Root", "Shizuku"];

/// The numbers of the "Parallel jobs" sheet: 0 is every core of the device.
const JOBS_CHOICES: [i32; 5] = [0, 1, 2, 4, 8];

/// A settings file bigger than this is not read (it is a few hundred bytes in practice).
const MAX_BACKUP: u64 = 4 << 20;

/// What the Settings screen remembers between taps.
#[derive(Default)]
pub(crate) struct Open {
    /// Bytes in the cache, once counted.
    cache: Option<u64>,
    /// The folder the last backup was saved to or loaded from.
    last_dir: Option<PathBuf>,
    /// How the last root check went: `Ok` or the reason.
    root: Option<Result<(), String>>,
}

impl App {
    pub(crate) fn wire_settings(&self) {
        let ui = &self.ui;
        ui.on_st_act(|id, on| app_do(|app| app.st_act(id, on)));
        ui.on_st_reset(|i| app_do(|app| app.st_reset(i)));
        ui.on_wh_button_clicked(|i| app_do(|app| app.wheel_button(i)));
        ui.on_wh_cancelled(|| app_do(|app| app.ui.set_wh_shown(false)));
        ui.on_dc_swatch_clicked(|i| app_do(|app| app.color_picked(i)));
        ui.on_dc_cancelled(|| app_do(|app| app.ui.set_dc_shown(false)));
        ui.on_bn_dismissed(|| app_do(|app| app.ui.set_bn_shown(false)));
        ui.on_stt_dismissed(|| app_do(|app| app.ui.set_stt_shown(false)));
    }

    /// How many workers the heavy calls may use, 0 for every core.
    pub(crate) fn jobs(&self) -> usize {
        self.prefs.int("jobs", 0).clamp(0, 8) as usize
    }

    /// What the file list and the window read from the settings: row height, size units and the
    /// colour of the ".." row.
    pub(crate) fn apply_prefs(&self) {
        fmt::set_decimal(self.prefs.bool("units_decimal", false));
        let ui = &self.ui;
        ui.set_rows_on_screen(
            self.prefs
                .int("rows_on_screen", ROWS_DEFAULT)
                .clamp(ROWS_MIN, ROWS_MAX),
        );
        let color = self.prefs.default_folder_color();
        ui.set_has_up_color(color != 0);
        ui.set_up_color(argb(color | 0xFF00_0000));
    }

    /// The menu's "Settings": show the page and count the cache.
    pub(crate) fn open_settings(&self) {
        self.render_settings();
        self.ui.set_st_shown(true);
        self.load_cache_size();
    }

    /// Back from the page: read the settings again so the list shows them.
    pub(crate) fn close_settings(&self) {
        self.ui.set_st_shown(false);
        self.apply_prefs();
        self.browser.borrow_mut().up_row_on = self.prefs.bool("up_row", true);
        self.render_rows();
        self.render_chrome();
        self.thumb_request();
    }

    /// Back while a Settings sheet is up: closes it. The result sheets, the wheel, the colour sheet
    /// and the file chooser sit above the other dialogs.
    pub(crate) fn settings_sheets_back(&self) -> bool {
        let ui = &self.ui;
        if ui.get_stt_shown() {
            ui.set_stt_shown(false);
        } else if ui.get_bn_shown() {
            ui.set_bn_shown(false);
        } else if ui.get_wh_shown() {
            ui.set_wh_shown(false);
        } else if ui.get_dc_shown() {
            ui.set_dc_shown(false);
        } else if ui.get_st_shown() && ui.get_fc_shown() {
            self.fc_cancelled();
        } else {
            return false;
        }
        true
    }

    // ---- drawing the page --------------------------------------------------------------------

    /// The page from the settings.
    fn render_settings(&self) {
        let p = &self.prefs;
        let mode = p.int("priv_mode", 0).clamp(0, 2);
        let jobs = self.jobs();
        let default_color = p.default_folder_color();
        let (cache, root) = {
            let s = self.settings.borrow();
            (s.cache, s.root.clone())
        };
        let status = match (mode, root) {
            (1, None) => "Not checked yet".to_string(),
            (1, Some(Ok(()))) => "Working: root".to_string(),
            (1, Some(Err(note))) if note.is_empty() => "Root not available".to_string(),
            (1, Some(Err(note))) => note,
            (2, _) => "Not available in this version of Kiln yet".to_string(),
            _ => "Off".to_string(),
        };
        let state = SettingsState {
            up_row: p.bool("up_row", true),
            previews: p.bool("previews", true),
            rows: p
                .int("rows_on_screen", ROWS_DEFAULT)
                .clamp(ROWS_MIN, ROWS_MAX)
                .to_string()
                .into(),
            folder_default: default_color == 0,
            folder_color: argb(default_color | 0xFF00_0000),
            anim: ANIM_NAMES[p.int("anim_level", 2).clamp(0, 3) as usize].into(),
            units: if p.bool("units_decimal", false) {
                "Decimal"
            } else {
                "Binary"
            }
            .into(),
            deep: p.bool("search_deep", false),
            search_path: PATH_NAMES[usize::from(p.int("search_path", 0) == 1)].into(),
            deleting: DELETE_NAMES[p.int("delete_mode", 0).clamp(0, 2) as usize].into(),
            wipe: p.bool("delete_after_pack", false),
            access: ACCESS_NAMES[mode as usize].into(),
            access_on: mode != 0,
            access_root: mode == 1,
            access_status: status.into(),
            access_unsafe: p.bool("priv_unsafe", false),
            access_check: if mode == 2 {
                "Check Shizuku"
            } else {
                "Check root access"
            }
            .into(),
            jobs: if jobs == 0 {
                "Automatic".to_string()
            } else {
                jobs.to_string()
            }
            .into(),
            cache: cache
                .map_or("...".to_string(), |n| fmt::size(n as i64))
                .into(),
        };
        let open: Vec<OpenWithRow> = p
            .open_with_all()
            .into_iter()
            .map(|(ext, value)| OpenWithRow {
                ext: format!(".{ext}").into(),
                label: open_with_label(&value).into(),
            })
            .collect();
        self.ui.set_st(state);
        self.ui.set_st_open(ModelRc::new(VecModel::from(open)));
    }

    // ---- taps --------------------------------------------------------------------------------

    fn st_act(&self, id: i32, on: bool) {
        let p = &self.prefs;
        match id {
            ST_BACK => self.close_settings(),
            ST_UP_ROW => p.set_bool("up_row", on),
            ST_PREVIEWS => p.set_bool("previews", on),
            ST_ROWS => self.rows_sheet(),
            ST_FOLDER_COLOR => self.color_sheet(),
            ST_ANIM => self.anim_sheet(),
            ST_UNITS => self.units_sheet(),
            ST_DEEP => p.set_bool("search_deep", on),
            ST_SEARCH_PATH => self.search_path_sheet(),
            ST_DELETING => self.delete_sheet(),
            ST_WIPE => p.set_bool("delete_after_pack", on),
            ST_EXPORT => self.export_settings(),
            ST_IMPORT => self.import_settings(),
            ST_SELFTEST => self.run_tool("Self-test", "selftest", String::new()),
            ST_BENCH => self.bench_pick(),
            ST_ACCESS => self.access_sheet(),
            ST_UNSAFE => self.unsafe_toggled(on),
            ST_CHECK => self.check_access(),
            ST_JOBS => self.jobs_sheet(),
            ST_CACHE => self.cache_sheet(),
            _ => {}
        }
        self.render_settings();
    }

    /// One "open with" choice, or all of them with -1.
    fn st_reset(&self, index: i32) {
        match usize::try_from(index) {
            Ok(i) => {
                if let Some((ext, _)) = self.prefs.open_with_all().get(i) {
                    self.prefs.clear_open_with(ext);
                }
            }
            Err(_) => self.prefs.clear_all_open_with(),
        }
        self.render_settings();
    }

    // ---- choices -----------------------------------------------------------------------------

    /// A sheet of radio rows with Cancel. `pick` stores the row that was tapped.
    fn radio_sheet(
        &self,
        title: &str,
        body: &str,
        items: &[(String, &str)],
        current: usize,
        pick: impl Fn(&App, usize) + 'static,
    ) {
        let pick = Rc::new(pick);
        let mut spec = Spec::new(title);
        spec.body = body.to_string();
        spec.radio = true;
        for (i, (label, detail)) in items.iter().enumerate() {
            let pick = pick.clone();
            let action: Action = Rc::new(move |app: &App| {
                pick(app, i);
                app.render_settings();
            });
            spec.options.push(Opt {
                icon: "",
                label: label.clone(),
                sub: (*detail).to_string(),
                selected: i == current,
                action,
            });
        }
        spec.buttons.push(cancel());
        self.show_dialog(spec);
    }

    fn anim_sheet(&self) {
        let details = ["No animations", "", "Default", ""];
        let items: Vec<(String, &str)> = ANIM_NAMES
            .iter()
            .zip(details)
            .map(|(n, d)| ((*n).to_string(), d))
            .collect();
        let now = self.prefs.int("anim_level", 2).clamp(0, 3) as usize;
        self.radio_sheet("Animation speed", "", &items, now, |app, i| {
            app.prefs.set_int("anim_level", i as i32);
            app.apply_look();
        });
    }

    fn units_sheet(&self) {
        let items = [
            ("Binary".to_string(), "1 KB = 1024 bytes (as before)"),
            ("Decimal".to_string(), "1 kB = 1000 bytes"),
        ];
        let now = usize::from(self.prefs.bool("units_decimal", false));
        self.radio_sheet("File sizes", "", &items, now, |app, i| {
            app.prefs.set_bool("units_decimal", i == 1);
            fmt::set_decimal(i == 1);
        });
    }

    fn search_path_sheet(&self) {
        let items = [
            ("Relative".to_string(), "e.g. Docs/Notes"),
            (
                "Absolute".to_string(),
                "e.g. /storage/emulated/0/Docs/Notes",
            ),
        ];
        let now = usize::from(self.prefs.int("search_path", 0) == 1);
        self.radio_sheet("Path shown in results", "", &items, now, |app, i| {
            app.prefs.set_int("search_path", i as i32);
        });
    }

    fn delete_sheet(&self) {
        let details = [
            "Choose each time",
            "Keep deleted items so they can be restored",
            "Delete at once, can't be undone",
        ];
        let items: Vec<(String, &str)> = DELETE_NAMES
            .iter()
            .zip(details)
            .map(|(n, d)| ((*n).to_string(), d))
            .collect();
        let now = self.prefs.int("delete_mode", 0).clamp(0, 2) as usize;
        self.radio_sheet("When deleting", "", &items, now, |app, i| {
            app.prefs.set_int("delete_mode", i as i32);
        });
    }

    fn jobs_sheet(&self) {
        let details = ["Uses every core of the device", "One at a time", "", "", ""];
        let items: Vec<(String, &str)> = JOBS_CHOICES
            .iter()
            .zip(details)
            .map(|(n, d)| {
                let label = if *n == 0 {
                    "Automatic".to_string()
                } else {
                    n.to_string()
                };
                (label, d)
            })
            .collect();
        let now = JOBS_CHOICES
            .iter()
            .position(|n| *n == self.jobs() as i32)
            .unwrap_or(usize::MAX);
        self.radio_sheet(
            "Parallel jobs",
            "How many files are packed or unpacked at the same time.",
            &items,
            now,
            |app, i| app.prefs.set_int("jobs", JOBS_CHOICES[i]),
        );
    }

    // ---- rows on screen ----------------------------------------------------------------------

    /// The wheel sheet: pick how many rows fit the list.
    fn rows_sheet(&self) {
        let ui = &self.ui;
        ui.set_wh_min(ROWS_MIN);
        ui.set_wh_max(ROWS_MAX);
        ui.set_wh_text("Scroll to choose. More rows means smaller rows.".into());
        ui.set_wh_value(
            self.prefs
                .int("rows_on_screen", ROWS_DEFAULT)
                .clamp(ROWS_MIN, ROWS_MAX),
        );
        ui.set_wh_shown(true);
    }

    /// 0 Cancel, 1 Set.
    fn wheel_button(&self, index: i32) {
        if index == 1 {
            let n = self.ui.get_wh_value().clamp(ROWS_MIN, ROWS_MAX);
            self.prefs.set_int("rows_on_screen", n);
            self.render_settings();
        }
        self.ui.set_wh_shown(false);
    }

    // ---- default folder colour ---------------------------------------------------------------

    /// The swatches; the first is the app's own colour.
    fn color_sheet(&self) {
        let swatches: Vec<Color> = FOLDER_COLORS.iter().map(|c| argb(*c)).collect();
        let ui = &self.ui;
        ui.set_pick_colors(ModelRc::new(VecModel::from(swatches)));
        ui.set_dc_index(color_index(self.prefs.default_folder_color()));
        ui.set_dc_shown(true);
    }

    fn color_picked(&self, index: i32) {
        self.ui.set_dc_shown(false);
        let color = match usize::try_from(index) {
            Ok(0) => 0,
            Ok(i) => match FOLDER_COLORS.get(i - 1) {
                Some(c) => *c,
                None => return,
            },
            Err(_) => return,
        };
        self.apply_default_folder_color(color);
    }

    /// Set the default; when some folders have their own colour, ask
    /// whether they change as well.
    fn apply_default_folder_color(&self, color: u32) {
        let custom = self.prefs.custom_folder_colors();
        if custom == 0 {
            self.prefs.set_default_folder_color(color);
            self.render_settings();
            return;
        }
        let (who, keep, them) = if custom == 1 {
            ("1 folder has".to_string(), "it as it is", "it")
        } else {
            (format!("{custom} folders have"), "them as they are", "them")
        };
        let mut spec = Spec::new("Change folders with their own color?");
        spec.body = format!(
            "{who} a color you picked yourself. Keep {keep}, or change {them} to the new default too?"
        );
        let change: Click = Rc::new(move |app: &App, _: &str| {
            app.prefs.set_default_folder_color(color);
            app.prefs.clear_folder_colors();
            app.render_settings();
            true
        });
        let keep: Click = Rc::new(move |app: &App, _: &str| {
            app.prefs.set_default_folder_color(color);
            app.render_settings();
            true
        });
        spec.buttons.push(Btn {
            label: "Change all".to_string(),
            kind: BtnKind::Filled,
            click: Some(change),
        });
        spec.buttons.push(Btn {
            label: "Keep my colors".to_string(),
            kind: BtnKind::Outline,
            click: Some(keep),
        });
        spec.buttons.push(cancel());
        self.show_dialog(spec);
    }

    // ---- backup ------------------------------------------------------------------------------

    /// Where a chooser starts: the folder of the last backup, else the folder on screen.
    fn backup_start(&self) -> PathBuf {
        let last = self.settings.borrow().last_dir.clone();
        last.unwrap_or_else(|| self.browser.borrow().cwd.clone())
    }

    /// Choose a folder, name the file, save.
    fn export_settings(&self) {
        self.pick_folder(self.backup_start(), |app, dir| {
            app.settings.borrow_mut().last_dir = Some(dir.clone());
            app.name_sheet(
                "Save settings as",
                "kiln-settings.json",
                "Save",
                move |app, name| {
                    let target = dir.join(name);
                    if target.exists() {
                        app.replace_sheet(target);
                    } else {
                        app.write_settings(&target);
                    }
                    None
                },
            );
        });
    }

    /// The file is there already: ask before it is replaced.
    fn replace_sheet(&self, target: PathBuf) {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut spec = Spec::new("Replace the file?");
        spec.body = format!("\"{name}\" already exists in that folder.");
        let replace: Click = Rc::new(move |app: &App, _: &str| {
            app.write_settings(&target);
            true
        });
        spec.buttons.push(cancel());
        spec.buttons.push(Btn {
            label: "Replace".to_string(),
            kind: BtnKind::Filled,
            click: Some(replace),
        });
        self.show_dialog(spec);
    }

    fn write_settings(&self, target: &std::path::Path) {
        match std::fs::write(target, self.prefs.export_settings()) {
            Ok(()) => self.toast("Settings saved"),
            Err(e) => self.message("Couldn't save the settings", &e.to_string()),
        }
    }

    /// Choose a file, confirm, load.
    fn import_settings(&self) {
        self.pick_file("Load settings", self.backup_start(), |app, file| {
            if let Some(dir) = file.parent() {
                app.settings.borrow_mut().last_dir = Some(dir.to_path_buf());
            }
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut spec = Spec::new("Overwrite your settings?");
            spec.body = format!(
                "Importing \"{name}\" will overwrite your current Kiln settings and your saved \"Open with\" choices with the ones in that file. This can't be undone."
            );
            let load: Click = Rc::new(move |app: &App, _: &str| {
                app.load_settings(&file);
                true
            });
            spec.buttons.push(cancel());
            spec.buttons.push(Btn {
                label: "Import".to_string(),
                kind: BtnKind::Filled,
                click: Some(load),
            });
            app.show_dialog(spec);
        });
    }

    fn load_settings(&self, file: &std::path::Path) {
        let read = || -> Result<usize, String> {
            let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
            if size > MAX_BACKUP {
                return Err("that file is too big to be a settings file".to_string());
            }
            let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            self.prefs.import_settings(&text)
        };
        match read() {
            Ok(n) => {
                self.apply_prefs();
                self.apply_look();
                self.render_settings();
                let what = if n == 1 { "setting" } else { "settings" };
                self.toast(&format!(
                    "{n} {what} loaded. Sorting and hidden-file options apply the next time Kiln starts."
                ));
            }
            Err(e) => self.message("Couldn't load the settings", &e),
        }
    }

    // ---- protected folders -------------------------------------------------------------------

    /// Off, Root or Shizuku.
    fn access_sheet(&self) {
        let items = [
            ("Off".to_string(), "Normal access only"),
            (
                "Root".to_string(),
                "Needs a rooted phone; a superuser prompt appears",
            ),
            (
                "Shizuku".to_string(),
                "Needs the Shizuku app running; reaches Android/data and more",
            ),
        ];
        let now = self.prefs.int("priv_mode", 0).clamp(0, 2) as usize;
        self.radio_sheet(
            "Protected folders",
            "Lets Kiln browse and change folders Android keeps from apps. Anything done here is real: deleting or moving system files can break apps or the phone.",
            &items,
            now,
            |app, i| {
                app.prefs.set_int("priv_mode", i as i32);
                if i != 0 {
                    app.check_access();
                }
            },
        );
    }

    /// "Allow changes (dangerous)": turning it off is at once, turning it on asks first.
    fn unsafe_toggled(&self, on: bool) {
        if !on {
            self.prefs.set_bool("priv_unsafe", false);
            return;
        }
        let mut spec = Spec::new("Allow changes?");
        spec.body = "With this on, Kiln can delete, move and overwrite system and app files as root. Deleting the wrong file can brick your phone, and there is no undo or recycle bin. Only continue if you know what you are doing.".to_string();
        let turn_on: Click = Rc::new(|app: &App, _: &str| {
            app.prefs.set_bool("priv_unsafe", true);
            app.render_settings();
            true
        });
        spec.buttons.push(cancel());
        spec.buttons.push(Btn {
            label: "I understand, turn on".to_string(),
            kind: BtnKind::Danger,
            click: Some(turn_on),
        });
        self.show_dialog(spec);
    }

    /// Try the chosen way in. Root runs `su -c id`; a superuser prompt may appear.
    fn check_access(&self) {
        match self.prefs.int("priv_mode", 0) {
            1 => {
                self.toast("Asking for root...");
                std::thread::spawn(|| {
                    let outcome = probe_root();
                    let _ = slint::invoke_from_event_loop(move || {
                        app_do(|app| app.root_checked(outcome));
                    });
                });
            }
            2 => self.message(
                "Shizuku isn't available",
                "Shizuku support isn't in this version of Kiln yet. Protected folders work with root for now.",
            ),
            _ => {}
        }
    }

    fn root_checked(&self, outcome: Result<(), String>) {
        let ok = outcome.is_ok();
        self.settings.borrow_mut().root = Some(outcome);
        self.render_settings();
        let status = self.ui.get_st().access_status;
        self.toast(if ok {
            "Root access works"
        } else {
            status.as_str()
        });
    }

    // ---- cache -------------------------------------------------------------------------------

    /// Count the cache folder off the UI thread.
    fn load_cache_size(&self) {
        let dir = self.host.cache_dir();
        std::thread::spawn(move || {
            let result = kiln_bridge::call("cache_size", &json!([dir.to_string_lossy()]), 0);
            let bytes = result["bytes"].as_u64();
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|app| {
                    app.settings.borrow_mut().cache = bytes;
                    if app.ui.get_st_shown() {
                        app.render_settings();
                    }
                });
            });
        });
    }

    /// "Clear the cache?"
    fn cache_sheet(&self) {
        let mut spec = Spec::new("Clear the cache?");
        spec.body =
            "Files you opened from archives will be extracted again the next time.".to_string();
        let clear: Click = Rc::new(|app: &App, _: &str| {
            let dir = app.host.cache_dir();
            app.settings.borrow_mut().cache = None;
            app.render_settings();
            std::thread::spawn(move || {
                let _ = kiln_bridge::call("clear_cache", &json!([dir.to_string_lossy()]), 0);
                // the engine's scratch folder lives in the cache and is needed again at once
                let _ = std::fs::create_dir_all(dir.join("engine-tmp"));
                let _ = slint::invoke_from_event_loop(|| app_do(|app| app.load_cache_size()));
            });
            true
        });
        spec.buttons.push(cancel());
        spec.buttons.push(Btn {
            label: "Clear".to_string(),
            kind: BtnKind::Filled,
            click: Some(clear),
        });
        self.show_dialog(spec);
    }

    // ---- tools -------------------------------------------------------------------------------

    /// "Benchmark a file": choose the file, then measure.
    fn bench_pick(&self) {
        self.pick_file("Benchmark a file", self.backup_start(), |app, file| {
            if let Some(dir) = file.parent() {
                app.settings.borrow_mut().last_dir = Some(dir.to_path_buf());
            }
            app.run_tool("Benchmark", "bench", file.to_string_lossy().into_owned());
        });
    }

    /// Run the engine's `selftest` or `bench` with a progress sheet and Cancel, then
    /// show the result.
    fn run_tool(&self, title: &'static str, tool: &'static str, arg: String) {
        let jobs = self.jobs();
        self.run_op(
            &format!("{title}..."),
            true,
            move |job| kiln_bridge::call_job("tool", &json!([tool, arg]), jobs, job),
            move |app, result| app.tool_done(title, result),
        );
    }

    fn tool_done(&self, title: &str, result: &Value) {
        if result["cancelled"].as_bool().unwrap_or(false) {
            self.toast(&format!("{title} cancelled"));
            return;
        }
        let view = &result["view"];
        match view["type"].as_str() {
            Some("bench") => self.show_bench(view),
            Some("selftest") => self.show_selftest(view),
            _ if result["ok"].as_bool().unwrap_or(false) => {
                self.message(title, result["text"].as_str().unwrap_or_default());
            }
            _ => {
                let why = result["message"].as_str().unwrap_or_default();
                self.message(&format!("{title} failed"), why);
            }
        }
    }

    /// A bar for each codec's packed size, with a tag on the smallest and the fastest.
    fn show_bench(&self, view: &Value) {
        let rows = view["rows"].as_array().cloned().unwrap_or_default();
        let sizes: Vec<i64> = rows
            .iter()
            .map(|r| r["size"].as_i64().unwrap_or(0))
            .collect();
        let secs: Vec<f64> = rows
            .iter()
            .map(|r| r["secs"].as_f64().unwrap_or(0.0))
            .collect();
        let biggest = sizes.iter().copied().max().unwrap_or(0).max(1);
        let smallest = sizes.iter().copied().min().unwrap_or(0);
        let fastest = secs.iter().copied().fold(f64::MAX, f64::min);
        let slowest = secs.iter().copied().fold(0.0, f64::max);
        let many = rows.len() > 1;
        let sizes_differ = sizes.iter().any(|s| *s != smallest);
        let secs_differ = slowest - fastest >= 0.05;
        let out: Vec<BenchRow> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| BenchRow {
                name: r["name"].as_str().unwrap_or_default().into(),
                ratio: format!("{:.1}%", r["ratio"].as_f64().unwrap_or(0.0)).into(),
                value: sizes[i] as f32 / biggest as f32,
                sub: format!("{}  \u{b7}  {:.1} s", fmt::size(sizes[i]), secs[i]).into(),
                smallest: many && sizes_differ && sizes[i] == smallest,
                fastest: many && secs_differ && secs[i] <= fastest,
            })
            .collect();
        let files = view["files"].as_i64().unwrap_or(0);
        let input = view["input"].as_str().unwrap_or_default();
        let ui = &self.ui;
        ui.set_bn_title("Benchmark".into());
        ui.set_bn_head(format!("Input: {files} file(s), {input}").into());
        ui.set_bn_rows(ModelRc::new(VecModel::from(out)));
        ui.set_bn_note(
            "Percentages are the packed size as a share of the input. Bars compare the sizes against each other."
                .into(),
        );
        ui.set_bn_shown(true);
    }

    /// The verdict, three tiles, the checks that failed first.
    fn show_selftest(&self, view: &Value) {
        let checks = view["checks"].as_array().cloned().unwrap_or_default();
        let total = view["total"].as_u64().unwrap_or(checks.len() as u64);
        let failed = view["failed"].as_u64().unwrap_or(0);
        let finished = view["finished"].as_bool().unwrap_or(true);
        let ok = failed == 0 && finished;
        let row = |c: &Value| CheckRow {
            name: c["name"].as_str().unwrap_or_default().into(),
            secs: format!("{:.1} s", c["secs"].as_f64().unwrap_or(0.0)).into(),
            msg: c["msg"].as_str().unwrap_or_default().into(),
            ok: c["ok"].as_bool().unwrap_or(false),
        };
        let bad: Vec<CheckRow> = checks
            .iter()
            .filter(|c| !c["ok"].as_bool().unwrap_or(false))
            .map(row)
            .collect();
        let good: Vec<CheckRow> = checks
            .iter()
            .filter(|c| c["ok"].as_bool().unwrap_or(false))
            .map(row)
            .collect();
        let notes: Vec<&str> = view["notes"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let version = view["head"]["version"].as_str().unwrap_or_default();
        let cores = view["head"]["cores"].as_u64().unwrap_or(0);
        let head = if failed == 0 {
            "All checks passed".to_string()
        } else {
            format!(
                "{failed} check{} failed",
                if failed == 1 { "" } else { "s" }
            )
        };
        let ui = &self.ui;
        ui.set_stt_ok(ok);
        ui.set_stt_title("Self-test".into());
        ui.set_stt_head(head.into());
        ui.set_stt_sub(
            format!(
                "Kiln {version}  \u{b7}  {cores} core{}",
                if cores == 1 { "" } else { "s" }
            )
            .into(),
        );
        ui.set_stt_n_checks(total.to_string().into());
        ui.set_stt_checks_label(if total == 1 { "check" } else { "checks" }.into());
        ui.set_stt_n_passed(total.saturating_sub(failed).to_string().into());
        ui.set_stt_took(format!("{:.1} s", view["secs"].as_f64().unwrap_or(0.0)).into());
        ui.set_stt_passed_head(if failed > 0 { "PASSED" } else { "CHECKS" }.into());
        ui.set_stt_failed(ModelRc::new(VecModel::from(bad)));
        ui.set_stt_passed(ModelRc::new(VecModel::from(good)));
        ui.set_stt_notes(notes.join("\n").into());
        ui.set_stt_unfinished(
            if finished {
                ""
            } else {
                "The test did not finish, so some checks did not run."
            }
            .into(),
        );
        ui.set_stt_shown(true);
    }
}

/// Ask `su` for `id` and look for uid 0. May block while a superuser prompt is
/// up, so it runs on a worker thread. The `Err` is what Settings shows.
fn probe_root() -> Result<(), String> {
    let out = Command::new("su")
        .args(["-c", "id"])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "Root isn't available on this device.".to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && text.contains("uid=0") {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.is_empty() {
        "Root access was refused or isn't available.".to_string()
    } else {
        err
    })
}

/// A "Cancel" button that only closes the sheet.
fn cancel() -> Btn {
    Btn {
        label: "Cancel".to_string(),
        kind: BtnKind::Text,
        click: None,
    }
}
