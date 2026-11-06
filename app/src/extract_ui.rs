//! The Extract form (`MainActivity.extractDialog`): the folder, Browse and "Replace files that
//! already exist". Running the extraction comes in `run_extract`.

use slint::SharedString;

use crate::dialog::{Btn, BtnKind, Spec};
use crate::{App, app_do};

/// The Extract form that is up: the members it was opened for (empty is everything).
#[derive(Default)]
pub(crate) struct Open {
    keys: Vec<String>,
}

impl App {
    pub(crate) fn wire_extract(&self) {
        let ui = &self.ui;
        ui.on_ext_button_clicked(|i| app_do(|app| app.extract_button(i)));
        // The folder chooser (`pickFolder`) is a later task.
        ui.on_ext_browse_clicked(|| app_do(|app| app.toast("Not in this build yet")));
        ui.on_ext_cancelled(|| app_do(|app| app.close_extract()));
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

    /// `runExtract`. Placeholder until task 12 replaces it.
    fn run_extract(&self, _keys: Vec<String>, _dest: String, _overwrite: bool) {
        self.toast("Not in this build yet");
    }
}
