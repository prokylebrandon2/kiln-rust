//! The Extract form (`MainActivity.extractDialog`): the folder, Browse and "Replace files that
//! already exist". Running the extraction comes in `run_extract`.

use std::path::PathBuf;
use std::rc::Rc;

use kiln_ui::DetailRow;
use serde_json::{Value, json};
use slint::{ModelRc, SharedString, VecModel};

use crate::arc::{members_json, parse_extract};
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
        // The folder chooser (`pickFolder`) is a later task.
        ui.on_ext_browse_clicked(|| app_do(|app| app.toast("Not in this build yet")));
        ui.on_ext_cancelled(|| app_do(|app| app.close_extract()));
        ui.on_ext_res_button_clicked(|i| app_do(|app| app.extract_result_button(i)));
        ui.on_ext_res_dismissed(|| app_do(|app| app.extract_result_button(0)));
    }

    /// `extractDialog`: the form for the given members (empty is all of them).
    // TODO(task 24): remove the allow when the archive menu calls this.
    #[allow(dead_code)]
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

    fn close_extract(&self) {
        self.extract.borrow_mut().keys.clear();
        self.ui.set_ext_shown(false);
    }

    /// `runExtract`: the engine's `extract` behind the progress sheet. (`Priv` is not in this
    /// build, so no destination counts as protected.)
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
        self.run_op(
            "Extracting...",
            true,
            move |job| kiln_bridge::call_job("extract", &args, 0, job),
            move |app, result| app.extract_done(keys, dest, overwrite, &name, result),
        );
    }

    /// The end of `runExtract`.
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
            // The redraw of the archive list comes with the archive screens.
            self.browser.borrow_mut().selected.clear();
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

    /// `extractResult`: the numbers from kiln's line, then the detail rows.
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

fn detail(label: &str, value: &str, stack: bool) -> DetailRow {
    DetailRow {
        label: label.into(),
        value: value.into(),
        stack,
    }
}
