//! The Extract form: the folder, Browse and "Replace files that
//! already exist". Running the extraction comes in `run_extract`.

use std::path::PathBuf;
use std::rc::Rc;

use kiln_ui::DetailRow;
use serde_json::{Value, json};
use slint::{ModelRc, SharedString, VecModel};

use crate::arc::{members_json, parse_check, parse_extract};
use crate::dialog::{Btn, BtnKind, Click, Spec};
use crate::{App, app_do};

/// The Extract form that is up: the members it was opened for (empty is everything).
#[derive(Default)]
pub(crate) struct Open {
    keys: Vec<String>,
    /// The folder the result sheet's "Open folder" goes to.
    dest: String,
}

impl App {
    pub(crate) fn wire_extract(&self) {
        let ui = &self.ui;
        ui.on_ext_button_clicked(|i| app_do(|app| app.extract_button(i)));
        ui.on_ext_browse_clicked(|| app_do(|app| app.extract_browse()));
        ui.on_ext_cancelled(|| app_do(|app| app.close_extract()));
        ui.on_ext_res_button_clicked(|i| app_do(|app| app.extract_result_button(i)));
        ui.on_ext_res_dismissed(|| app_do(|app| app.extract_result_button(0)));
        ui.on_chk_res_dismissed(|| app_do(|app| app.ui.set_chk_res_shown(false)));
        ui.on_inf_dismissed(|| app_do(|app| app.ui.set_inf_shown(false)));
    }

    /// The form for the given members (empty is all of them).
    pub(crate) fn extract_dialog(&self, keys: Vec<String>) {
        let form = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            if a.dirty {
                None
            } else {
                let parent = a.file.parent().unwrap_or(std::path::Path::new(""));
                let base = a.home.as_deref().unwrap_or(parent);
                let name = a.file_name();
                let leaf = if a.is_set {
                    format!("{name}_files")
                } else {
                    crate::arc::strip_ext(&name)
                };
                let text = if keys.is_empty() {
                    format!("Extract all {} files to:", a.ents.len())
                } else {
                    format!("Extract {} file(s) to:", a.count_for(&keys))
                };
                Some((base.join(leaf).to_string_lossy().into_owned(), text))
            }
        };
        let Some((dest, text)) = form else {
            let mut spec = Spec::new("Unsaved changes");
            spec.body = "Save or discard your changes first, then extract.".to_string();
            spec.buttons.push(Btn {
                label: "OK".to_string(),
                kind: BtnKind::Text,
                click: None,
            });
            self.show_dialog(spec);
            return;
        };
        let ui = &self.ui;
        ui.set_ext_text(text.into());
        ui.set_ext_where(dest.into());
        ui.set_ext_where_error(SharedString::new());
        ui.set_ext_replace(false);
        self.extract.borrow_mut().keys = keys;
        ui.set_ext_shown(true);
    }

    /// The form's buttons: 0 Cancel, 1 Extract.
    fn extract_button(&self, index: i32) {
        if index == 0 {
            self.close_extract();
            return;
        }
        let ui = &self.ui;
        let dest = ui.get_ext_where().trim().to_string();
        if dest.is_empty() {
            ui.set_ext_where_error("Choose a folder".into());
            return;
        }
        let overwrite = ui.get_ext_replace();
        let keys = std::mem::take(&mut self.extract.borrow_mut().keys);
        ui.set_ext_shown(false);
        self.run_extract(keys, dest, overwrite);
    }

    /// Browse: the chooser starts at the folder in the field and writes the pick back into it.
    fn extract_browse(&self) {
        let start = PathBuf::from(self.ui.get_ext_where().to_string());
        self.pick_folder(start, |app, picked| {
            app.ui
                .set_ext_where(picked.to_string_lossy().into_owned().into());
            app.ui.set_ext_where_error(SharedString::new());
        });
    }

    fn close_extract(&self) {
        self.extract.borrow_mut().keys.clear();
        self.ui.set_ext_shown(false);
    }

    /// The engine's `extract` behind the progress sheet.
    fn run_extract(&self, keys: Vec<String>, dest: String, overwrite: bool) {
        let (file, password, name, total) = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            (
                a.file.to_string_lossy().into_owned(),
                a.password.clone(),
                a.file_name(),
                a.count_for(&keys),
            )
        };
        let args = json!([file, password, members_json(&keys), dest, overwrite, total]);
        let jobs = self.jobs();
        self.run_op(
            "Extracting...",
            true,
            move |job| kiln_bridge::call_job("extract", &args, jobs, job),
            move |app, result| app.extract_done(keys, dest, overwrite, &name, result),
        );
    }

    fn extract_done(
        &self,
        keys: Vec<String>,
        dest: String,
        overwrite: bool,
        name: &str,
        result: &Value,
    ) {
        let msg = result["message"].as_str().unwrap_or_default();
        if result["ok"].as_bool().unwrap_or(false) {
            self.browser.borrow_mut().selected.clear();
            self.render_rows();
            self.render_chrome();
            self.show_extract_result(name, &dest, msg, overwrite);
        } else if result["cancelled"].as_bool().unwrap_or(false) {
            self.toast("Cancelled");
        } else if !overwrite && msg.contains("already exist") {
            self.confirm_replace_existing(keys, dest, msg);
        } else {
            self.message("Couldn't extract", msg);
        }
    }

    /// `confirm("Replace existing files?", ...)`: Replace runs the extraction again, overwriting.
    fn confirm_replace_existing(&self, keys: Vec<String>, dest: String, msg: &str) {
        let mut spec = Spec::new("Replace existing files?");
        spec.body = msg.to_string();
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        let click: Click = Rc::new(move |app: &App, _text: &str| -> bool {
            app.run_extract(keys.clone(), dest.clone(), true);
            true
        });
        spec.buttons.push(Btn {
            label: "Replace".to_string(),
            kind: BtnKind::Filled,
            click: Some(click),
        });
        self.show_dialog(spec);
    }

    /// The numbers from kiln's line, then the detail rows.
    fn show_extract_result(&self, name: &str, dest: &str, msg: &str, overwrite: bool) {
        let ui = &self.ui;
        let mut rows = Vec::new();
        match parse_extract(msg) {
            Some(line) => {
                ui.set_ext_res_has_stats(true);
                ui.set_ext_res_n_big(line.files.to_string().into());
                let label = if line.files == 1 { "file" } else { "files" };
                ui.set_ext_res_n_label(label.into());
                ui.set_ext_res_has_size(line.size.is_some());
                ui.set_ext_res_size(line.size.unwrap_or_default().into());
                ui.set_ext_res_time(format!("{} s", line.secs).into());
            }
            None => {
                ui.set_ext_res_has_stats(false);
                let text = if msg.is_empty() { "Done" } else { msg };
                rows.push(detail("Result", text, true));
            }
        }
        rows.push(detail("Saved to", dest, true));
        rows.push(detail("From", name, true));
        if msg.contains("all checksums OK") {
            rows.push(detail("Verification", "Every checksum matches", false));
        }
        if overwrite {
            rows.push(detail("Existing files", "Replaced", false));
        }
        ui.set_ext_res_name(name.into());
        ui.set_ext_res_rows(ModelRc::new(VecModel::from(rows)));
        self.extract.borrow_mut().dest = dest.to_string();
        ui.set_ext_res_shown(true);
    }

    /// The result sheet's buttons: 0 OK (or a back-out), 1 Open folder (`arc = null; loadDir`).
    fn extract_result_button(&self, index: i32) {
        let dest = std::mem::take(&mut self.extract.borrow_mut().dest);
        self.ui.set_ext_res_shown(false);
        if index == 1 && !dest.is_empty() {
            self.browser.borrow_mut().arc = None;
            self.ui.set_arc_bar_shown(false);
            self.ui.set_arc_dirty(false);
            self.load_dir(PathBuf::from(dest), false);
        }
    }
}

impl App {
    /// The engine's `test` behind the progress sheet.
    pub(crate) fn run_check(&self) {
        let (file, password, dirty) = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            (
                a.file.to_string_lossy().into_owned(),
                a.password.clone(),
                a.dirty,
            )
        };
        if dirty {
            self.message(
                "Unsaved changes",
                "Save your changes first; Check works on the saved archive.",
            );
            return;
        }
        let args = json!([file, password]);
        let jobs = self.jobs();
        self.run_op(
            "Checking...",
            true,
            move |job| kiln_bridge::call_job("test", &args, jobs, job),
            |app, result| app.check_done(result),
        );
    }

    fn check_done(&self, result: &Value) {
        let msg = result["message"].as_str().unwrap_or_default();
        if result["ok"].as_bool().unwrap_or(false) {
            self.show_check_result(true, msg);
        } else if result["cancelled"].as_bool().unwrap_or(false) {
            self.toast("Cancelled");
        } else {
            self.show_check_result(false, msg);
        }
    }

    /// A verdict, the numbers as tiles, and the details underneath.
    fn show_check_result(&self, ok: bool, msg: &str) {
        let (name, encrypted, count, raw) = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            (
                a.file_name(),
                a.encrypted,
                a.ents.len(),
                a.raw_and_folders().0,
            )
        };
        let ui = &self.ui;
        let mut rows = Vec::new();
        match if ok { parse_check(msg) } else { None } {
            Some(line) => {
                ui.set_chk_res_has_stats(true);
                ui.set_chk_res_n_big(line.files.to_string().into());
                let label = if line.files == 1 {
                    "file checked"
                } else {
                    "files checked"
                };
                ui.set_chk_res_n_label(label.into());
                // The folder-of-archives line has no size: it comes from the archive's listing.
                let size = line
                    .size
                    .unwrap_or_else(|| crate::fmt::size(raw).to_string());
                ui.set_chk_res_size(size.into());
                ui.set_chk_res_time(format!("{} s", line.secs).into());
                rows.push(detail("Result", "Every checksum matches", false));
            }
            None if ok => {
                ui.set_chk_res_has_stats(false);
                let text = if msg.is_empty() {
                    "No problems found"
                } else {
                    msg
                };
                rows.push(detail("Result", text, true));
            }
            None => {
                ui.set_chk_res_has_stats(false);
                let (first, more) = if msg.is_empty() {
                    ("The check failed.", String::new())
                } else {
                    let mut lines = msg.split('\n');
                    let first = lines.next().unwrap_or_default().trim();
                    let more: Vec<&str> = lines.map(str::trim).filter(|l| !l.is_empty()).collect();
                    (first, more.join("\n"))
                };
                rows.push(detail("What went wrong", first, true));
                if !more.is_empty() {
                    rows.push(detail("Details", &more, true));
                }
            }
        }
        rows.push(detail(
            "Encryption",
            if encrypted { "AES-256-GCM" } else { "None" },
            false,
        ));
        rows.push(detail("Files in archive", &count.to_string(), false));
        let hint = if ok {
            ""
        } else {
            "The archive may be damaged. Keep your original files until you've restored or re-created it."
        };
        ui.set_chk_res_hint(hint.into());
        ui.set_chk_res_ok(ok);
        ui.set_chk_res_name(name.into());
        ui.set_chk_res_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_chk_res_shown(true);
    }
}
impl App {
    /// The Archive info sheet.
    pub(crate) fn info_dialog(&self) {
        let (file, name, is_set, encrypted, packed, count, raw, folders, offset) = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            let (raw, folders) = a.raw_and_folders();
            (
                a.file.clone(),
                a.file_name(),
                a.is_set,
                a.encrypted,
                a.packed,
                a.ents.len(),
                raw,
                folders,
                b.date_offset_secs(),
            )
        };
        let size = crate::fmt::size;
        let ui = &self.ui;
        let icon = if is_set {
            "set"
        } else if encrypted {
            "lock"
        } else {
            "archive"
        };
        ui.set_inf_icon(icon.into());
        ui.set_inf_name(name.into());
        let lock = if encrypted {
            "Encrypted \u{b7} AES-256-GCM"
        } else {
            "Not encrypted"
        };
        ui.set_inf_lock(lock.into());
        ui.set_inf_n_big(count.to_string().into());
        ui.set_inf_n_label(if count == 1 { "file" } else { "files" }.into());
        ui.set_inf_orig(size(raw).into());
        ui.set_inf_stored(size(packed).into());

        // how much the packing saved: a bar of the stored size against the original
        ui.set_inf_has_cmp(raw > 0);
        if raw > 0 {
            let rawf = raw as f64;
            let pct = |n: i64| (100.0 * n as f64 / rawf).round() as i64;
            let saved = (raw - packed).max(0);
            let text = if saved > 0 {
                format!("Saved {} ({}%)", size(saved), pct(saved))
            } else {
                "No saving".to_string()
            };
            ui.set_inf_saved(text.into());
            // never an empty-looking bar
            let ratio = (packed as f64 / rawf).clamp(0.008, 1.0);
            ui.set_inf_frac(ratio as f32);
            ui.set_inf_caption(format!("Stored size is {}% of the original", pct(packed)).into());
        }

        let meta = std::fs::metadata(&file).ok();
        let mut rows = vec![
            detail(
                "Type",
                if is_set {
                    "Encrypted set (one archive per file)"
                } else {
                    "Archive"
                },
                false,
            ),
            detail("Folders", &folders.to_string(), false),
        ];
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
            .filter(|secs| *secs > 0);
        if let Some(secs) = modified {
            rows.push(detail("Modified", &crate::fmt::date(secs, offset), false));
        }
        if let Some(m) = meta.as_ref().filter(|m| !is_set && m.is_file()) {
            let len = i64::try_from(m.len()).unwrap_or(i64::MAX);
            rows.push(detail("Size on disk", &size(len), false));
        }
        rows.push(detail("Location", &file.to_string_lossy(), true));
        ui.set_inf_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_inf_shown(true);
    }
}
fn detail(label: &str, value: &str, stack: bool) -> DetailRow {
    DetailRow {
        label: label.into(),
        value: value.into(),
        stack,
    }
}

impl App {
    /// The confirm sheet. Decrypt runs `run_decrypt`.
    pub(crate) fn decrypt_dialog(&self) {
        let name = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            a.file_name()
        };
        let mut spec = Spec::new("Decrypt in place?");
        spec.body = format!(
            "\"{name}\" becomes an ordinary folder: the encryption and the random file names are removed."
        );
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        let click: Click = Rc::new(|app: &App, _text: &str| -> bool {
            app.run_decrypt();
            true
        });
        spec.buttons.push(Btn {
            label: "Decrypt".to_string(),
            kind: BtnKind::Filled,
            click: Some(click),
        });
        self.show_dialog(spec);
    }

    /// The engine's `decrypt_in_place` behind the progress sheet.
    fn run_decrypt(&self) {
        let (file, password, count) = {
            let b = self.browser.borrow();
            let Some(a) = b.arc.as_ref() else { return };
            (
                a.file.to_string_lossy().into_owned(),
                a.password.clone(),
                a.ents.len(),
            )
        };
        let args = json!([file, password, count]);
        let jobs = self.jobs();
        self.run_op(
            "Decrypting...",
            true,
            move |job| kiln_bridge::call_job("decrypt_in_place", &args, jobs, job),
            move |app, result| app.decrypt_done(result),
        );
    }

    /// "Decrypted" and the archive is closed.
    fn decrypt_done(&self, result: &Value) {
        if result["ok"].as_bool().unwrap_or(false) {
            self.toast("Decrypted");
            self.close_archive();
        } else if result["cancelled"].as_bool().unwrap_or(false) {
            self.toast("Cancelled");
        } else {
            let msg = result["message"].as_str().unwrap_or_default();
            self.message("Couldn't decrypt", msg);
        }
    }
}
