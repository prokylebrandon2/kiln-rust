//! The About sheet: the version, and what the engine says
//! about itself, asked on a worker thread.

use kiln_ui::DetailRow;
use serde_json::{Value, json};
use slint::{ModelRc, VecModel};

use crate::{App, app_do};

fn row(label: &str, value: &str) -> DetailRow {
    DetailRow {
        label: label.into(),
        value: value.into(),
        stack: false,
    }
}

impl App {
    pub(crate) fn wire_about(&self) {
        self.ui
            .on_about_dismissed(|| app_do(|app| app.ui.set_about_shown(false)));
    }

    /// Ask the engine for its version off the UI thread, then show the sheet.
    pub(crate) fn about(&self) {
        let version = self.host.version_name();
        let build = self.host.version_code();
        let engine_up = self.status.borrow().engine_up;
        std::thread::spawn(move || {
            let info = engine_up.then(|| kiln_bridge::call("version", &json!([]), 0));
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|app| app.show_about(&version, build, info.as_ref()));
            });
        });
    }

    /// The chip and the detail rows.
    fn show_about(&self, version: &str, build: i64, info: Option<&Value>) {
        let kiln = info.and_then(|v| v["kiln"].as_str()).unwrap_or_default();
        let helper = info.and_then(|v| v["helper"].as_str()).unwrap_or_default();
        let shown = if build > 0 {
            format!("{version} (build {build})")
        } else {
            version.to_string()
        };
        let mut rows = vec![row("Version", &shown)];
        if !kiln.is_empty() {
            rows.push(row("Engine", &format!("kiln {kiln}")));
            let fast = if helper.is_empty() {
                "On".to_string()
            } else {
                format!("Off \u{2013} {helper}")
            };
            rows.push(row("Fast mode", &fast));
        }
        rows.push(row("Encryption", "AES-256-GCM"));
        let ui = &self.ui;
        ui.set_about_version(format!("Version {version}").into());
        ui.set_about_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_about_shown(true);
    }
}
