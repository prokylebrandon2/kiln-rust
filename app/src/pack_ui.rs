//! The Pack sheets: the form, the run behind
//! the progress sheet, and the result sheet. The engine's result line is read by hand, because the
//! app crate has no regex.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use kiln_ui::DetailRow;
use serde_json::{Value, json};
use slint::{ModelRc, SharedString, VecModel};

use crate::dialog::{Btn, BtnKind, Click, Spec};
use crate::{App, app_do};

/// The Pack form that is up: the items it was opened for.
#[derive(Default)]
pub(crate) struct Open {
    sources: Vec<PathBuf>,
}

/// One pack to run.
#[derive(Clone)]
struct PackJob {
    sources: Vec<PathBuf>,
    dest: PathBuf,
    encrypt: bool,
    separate: bool,
    store: bool,
    overwrite: bool,
    password: String,
    wipe: bool,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn strip_ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

impl App {
    pub(crate) fn wire_pack(&self) {
        let ui = &self.ui;
        ui.on_pack_button_clicked(|i| app_do(|app| app.pack_button(i)));
        ui.on_pack_browse_clicked(|| app_do(|app| app.pack_browse()));
        ui.on_pack_options_changed(|| app_do(|app| app.pack_options_changed()));
        ui.on_pack_cancelled(|| app_do(|app| app.close_pack()));
        ui.on_pack_res_dismissed(|| app_do(|app| app.close_pack_result()));
    }

    /// Browse: the chooser starts at the folder in the field and writes the pick back into it.
    fn pack_browse(&self) {
        let start = PathBuf::from(self.ui.get_pack_where().trim());
        self.pick_folder(start, |app, picked| {
            app.ui
                .set_pack_where(picked.to_string_lossy().into_owned().into());
            app.ui.set_pack_where_error(SharedString::new());
        });
    }

    /// The form for the selected items.
    pub(crate) fn pack_selected(&self) {
        let (cwd, sources, base) = {
            let b = self.browser.borrow();
            let sources: Vec<PathBuf> = b.selected.iter().map(|key| b.cwd.join(key)).collect();
            let base = match sources.as_slice() {
                [] => return,
                [one] => strip_ext(&file_name(one)),
                _ if b.is_storage_root(&b.cwd) => "archive".to_string(),
                _ => file_name(&b.cwd),
            };
            (b.cwd.clone(), sources, base)
        };
        let title = match sources.as_slice() {
            [one] => format!("Pack \"{}\"", file_name(one)),
            _ => format!("Pack {} items", sources.len()),
        };
        let ui = &self.ui;
        ui.set_pack_title(title.into());
        ui.set_pack_name(format!("{base}.kiln").into());
        ui.set_pack_where(cwd.to_string_lossy().into_owned().into());
        ui.set_pack_pw1(SharedString::new());
        ui.set_pack_pw2(SharedString::new());
        ui.set_pack_name_error(SharedString::new());
        ui.set_pack_where_error(SharedString::new());
        ui.set_pack_pw1_error(SharedString::new());
        ui.set_pack_pw2_error(SharedString::new());
        ui.set_pack_enc(false);
        ui.set_pack_show_pw(false);
        ui.set_pack_store(false);
        ui.set_pack_each(false);
        ui.set_pack_wipe(self.prefs.bool("delete_after_pack", false));
        ui.set_pack_note(SharedString::new());
        *self.pack.borrow_mut() = Open { sources };
        ui.set_pack_shown(true);
    }

    /// The name's ".kiln" and the note follow the boxes. (It cuts four characters, so "x.kiln"
    /// becomes "x.".)
    fn pack_options_changed(&self) {
        let ui = &self.ui;
        let (each, enc) = (ui.get_pack_each(), ui.get_pack_enc());
        let name = ui.get_pack_name().to_string();
        if each && name.ends_with(".kiln") {
            ui.set_pack_name(name[..name.len() - 4].into());
        } else if !each && !name.ends_with(".kiln") && !name.is_empty() {
            ui.set_pack_name(format!("{name}.kiln").into());
        }
        let note = match (each, enc) {
            (true, true) => {
                "Creates a folder. File names are replaced by random names; open the folder in Kiln to see the real ones."
            }
            (true, false) => "Creates a folder with one archive per file.",
            _ => "",
        };
        ui.set_pack_note(note.into());
    }

    /// The form's buttons: 0 Cancel, 1 Create.
    fn pack_button(&self, index: i32) {
        if index == 0 {
            self.close_pack();
        } else {
            self.pack_create();
        }
    }

    /// Close the form and forget the passwords.
    pub(crate) fn close_pack(&self) {
        self.pack.borrow_mut().sources.clear();
        let ui = &self.ui;
        ui.set_pack_shown(false);
        ui.set_pack_pw1(SharedString::new());
        ui.set_pack_pw2(SharedString::new());
    }

    pub(crate) fn close_pack_result(&self) {
        self.ui.set_pack_res_shown(false);
    }

    /// "Create": check the fields, then ask about an existing name or run.
    fn pack_create(&self) {
        let ui = &self.ui;
        let name = ui.get_pack_name().trim().to_string();
        if name.is_empty() || name.contains('/') {
            ui.set_pack_name_error("Enter a name (no slashes)".into());
            return;
        }
        let dir = PathBuf::from(ui.get_pack_where().trim());
        if !dir.is_dir() {
            ui.set_pack_where_error("That folder doesn't exist".into());
            return;
        }
        let encrypt = ui.get_pack_enc();
        let password = ui.get_pack_pw1().to_string();
        if encrypt {
            if password.chars().count() < 8 {
                ui.set_pack_pw1_error("At least 8 characters".into());
                return;
            }
            if password != ui.get_pack_pw2().as_str() {
                ui.set_pack_pw2_error("Passwords don't match".into());
                return;
            }
        }
        let sources = std::mem::take(&mut self.pack.borrow_mut().sources);
        let job = PackJob {
            sources,
            dest: dir.join(&name),
            encrypt,
            separate: ui.get_pack_each(),
            store: ui.get_pack_store(),
            overwrite: false,
            password,
            wipe: ui.get_pack_wipe(),
        };
        self.close_pack();
        if job.dest.exists() {
            self.confirm_replace(job, &name);
        } else {
            self.run_pack(job);
        }
    }

    /// `confirm("Replace ...?")`.
    fn confirm_replace(&self, job: PackJob, name: &str) {
        let mut spec = Spec::new(&format!("Replace \"{name}\"?"));
        spec.body = "It already exists.".to_string();
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        let click: Click = Rc::new(move |app: &App, _text: &str| -> bool {
            app.run_pack(PackJob {
                overwrite: true,
                ..job.clone()
            });
            true
        });
        spec.buttons.push(Btn {
            label: "Replace".to_string(),
            kind: BtnKind::Danger,
            click: Some(click),
        });
        self.show_dialog(spec);
    }

    /// The engine's `pack` behind the progress sheet.
    fn run_pack(&self, job: PackJob) {
        let items: Vec<String> = job
            .sources
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let args = json!([
            items,
            job.dest.to_string_lossy(),
            job.encrypt,
            job.separate,
            job.store,
            job.overwrite,
            job.password,
            job.wipe
        ]);
        let jobs = self.jobs();
        self.run_op(
            "Packing...",
            true,
            move |worker| kiln_bridge::call_job("pack", &args, jobs, worker),
            move |app, result| app.pack_done(&job, result),
        );
    }

    fn pack_done(&self, job: &PackJob, result: &Value) {
        let message = result["message"].as_str().unwrap_or_default();
        if result["ok"].as_bool().unwrap_or(false) {
            self.browser.borrow_mut().selected.clear();
            self.reload_here();
            let deleted = job.wipe.then(|| result["deleted"].as_i64().unwrap_or(0));
            let failed = result["delete_failed"].as_i64().unwrap_or(0);
            self.show_pack_result(job, message, deleted, failed);
        } else if result["cancelled"].as_bool().unwrap_or(false) {
            self.toast("Cancelled");
        } else {
            self.message("Couldn't create the archive", message);
        }
    }
}
impl App {
    /// The head, the numbers read from the engine's line, and the detail rows.
    fn show_pack_result(&self, job: &PackJob, msg: &str, deleted: Option<i64>, failed: i64) {
        let ui = &self.ui;
        let icon = if job.separate {
            "set"
        } else if job.encrypt {
            "lock"
        } else {
            "archive"
        };
        let heading = if job.separate {
            "Archives created"
        } else {
            "Archive created"
        };
        ui.set_pack_res_icon(icon.into());
        ui.set_pack_res_heading(heading.into());
        ui.set_pack_res_name(file_name(&job.dest).into());
        let mut rows = Vec::new();
        match parse_stats(msg) {
            Some(s) => {
                ui.set_pack_res_has_stats(true);
                ui.set_pack_res_n_big(s.count.to_string().into());
                let label = match (s.archives, s.count == 1) {
                    (true, true) => "archive",
                    (true, false) => "archives",
                    (false, true) => "file packed",
                    (false, false) => "files packed",
                };
                ui.set_pack_res_n_label(label.into());
                ui.set_pack_res_stored(s.stored.as_str().into());
                ui.set_pack_res_time(format!("{} s", s.seconds).into());
                let saved = if s.percent >= 100.0 {
                    "No saving".to_string()
                } else {
                    format!("Saved {}%", (100.0 - s.percent).round() as i64)
                };
                ui.set_pack_res_saved(saved.into());
                ui.set_pack_res_frac((s.percent / 100.0).clamp(0.0, 1.0) as f32);
                ui.set_pack_res_caption(format!("{} became {}", s.original, s.stored).into());
            }
            None => {
                ui.set_pack_res_has_stats(false);
                rows.push(detail(
                    "Result",
                    if msg.is_empty() { "Done" } else { msg },
                    true,
                ));
            }
        }
        rows.push(detail("Location", &job.dest.to_string_lossy(), true));
        let layout = match (job.separate, job.encrypt) {
            (true, true) => "One archive per file, random names",
            (true, false) => "One archive per file",
            _ => "Single archive",
        };
        rows.push(detail("Layout", layout, false));
        rows.push(detail(
            "Encryption",
            if job.encrypt { "AES-256-GCM" } else { "None" },
            false,
        ));
        if let Some(note) = parallel_note(msg) {
            rows.push(detail("Parallel", note, true));
        }
        if job.store {
            rows.push(detail("Compression", "Stored only", false));
        }
        if let Some(n) = deleted {
            let what = if n == 1 {
                "file deleted"
            } else {
                "files deleted"
            };
            let mut text = format!("{n} {what}");
            if failed > 0 {
                text.push_str(&format!(", {failed} couldn't be deleted"));
            }
            rows.push(detail("Originals", &text, false));
        }
        ui.set_pack_res_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_pack_res_shown(true);
    }
}

fn detail(label: &str, value: &str, stack: bool) -> DetailRow {
    DetailRow {
        label: label.into(),
        value: value.into(),
        stack,
    }
}

/// What the engine's line says: "3 file(s): 40.0 MB -> 40.0 MB (100.0% of original) in 3.6s" or
/// "18 archive(s) in /path: 65.4 KB -> 16.2 KB (24.8% of original) in 1.6s".
struct Stats {
    count: u64,
    archives: bool,
    original: String,
    stored: String,
    percent: f64,
    seconds: String,
}

/// The numbers of the engine's result line, read by hand (the app has no regex crate).
fn parse_stats(msg: &str) -> Option<Stats> {
    let digits = msg.chars().take_while(char::is_ascii_digit).count();
    let count: u64 = msg[..digits].parse().ok()?;
    let rest = &msg[digits..];
    let (archives, rest) = match rest.strip_prefix(" file(s)") {
        Some(r) => (false, r),
        None => (true, rest.strip_prefix(" archive(s)")?),
    };
    let (_, rest) = rest.split_once(": ")?;
    let (original, rest) = rest.split_once(" -> ")?;
    let (stored, rest) = rest.split_once(" (")?;
    let (percent, rest) = rest.split_once("% of original) in ")?;
    let percent: f64 = percent.parse().ok()?;
    let seconds: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if seconds.is_empty() || !rest[seconds.len()..].starts_with('s') {
        return None;
    }
    Some(Stats {
        count,
        archives,
        original: original.to_string(),
        stored: stored.to_string(),
        percent,
        seconds,
    })
}

/// The first "[...]" of the line that mentions cores or threads ("1 core", "2 threads").
fn parallel_note(msg: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(open) = msg[from..].find('[') {
        let start = from + open + 1;
        let close = msg[start..].find(']')?;
        let inner = &msg[start..start + close];
        if inner.contains("core") || inner.contains("thread") {
            return Some(inner);
        }
        from = start;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_line_of_one_archive() {
        let s = parse_stats("3 file(s): 40.0 MB -> 40.0 MB (100.0% of original) in 3.6s [1 core]")
            .expect("a result line");
        assert_eq!(s.count, 3);
        assert!(!s.archives);
        assert_eq!(s.original, "40.0 MB");
        assert_eq!(s.stored, "40.0 MB");
        assert!((s.percent - 100.0).abs() < 1e-9);
        assert_eq!(s.seconds, "3.6");
    }

    #[test]
    fn reads_the_line_of_many_archives() {
        let s =
            parse_stats("18 archive(s) in /path: 65.4 KB -> 16.2 KB (24.8% of original) in 1.6s")
                .expect("a result line");
        assert_eq!(s.count, 18);
        assert!(s.archives);
        assert_eq!(s.original, "65.4 KB");
        assert_eq!(s.stored, "16.2 KB");
        assert!((s.percent - 24.8).abs() < 1e-9);
        assert_eq!(s.seconds, "1.6");
    }

    #[test]
    fn ignores_a_line_it_does_not_know() {
        assert!(parse_stats("Done").is_none());
        assert!(parse_stats("").is_none());
    }

    #[test]
    fn finds_the_parallel_note() {
        assert_eq!(parallel_note("x [1 core]"), Some("1 core"));
        assert_eq!(
            parallel_note("x [no compression] [2 threads]"),
            Some("2 threads")
        );
        assert_eq!(parallel_note("x [encrypted]"), None);
        assert_eq!(parallel_note("no brackets"), None);
    }
}
