//! Opening a file in another app: the options sheet (`fileSheet`), the app chooser
//! (`appChooser`) and the saved default app (`Defaults.get`, `openFile`).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::json;

use crate::browse_ui::opt;
use crate::browser::Kind;
use crate::dialog::{Btn, BtnKind, Spec};
use crate::host::{OpenApp, OpenWith};
use crate::{App, fmt, sniff};

/// `Defaults.PREFIX`: a default app is stored per extension as `def_<ext>`.
const DEFAULT_PREFIX: &str = "def_";

/// An archive file that was handed to an editor (`editFile`, `editMember`, `editStamp`).
struct Staged {
    archive: PathBuf,
    member: String,
    file: PathBuf,
    stamp: i64,
}

thread_local! {
    static STAGED: RefCell<Option<Staged>> = const { RefCell::new(None) };
}

/// `lastModified() * 31 + length()`: changes when the editor saved something.
fn stamp(modified_ms: i64, len: i64) -> i64 {
    modified_ms.wrapping_mul(31).wrapping_add(len)
}

fn stamp_of(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let ms = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    Some(stamp(
        i64::try_from(ms).unwrap_or(0),
        i64::try_from(meta.len()).unwrap_or(0),
    ))
}

/// The preferences key that holds the default app for an extension.
pub(crate) fn default_key(ext: &str) -> String {
    format!("{DEFAULT_PREFIX}{ext}")
}

/// `Defaults.pkgOf`: a stored value is `package|label`, or just the package.
pub(crate) fn pkg_of(value: &str) -> &str {
    value.split_once('|').map_or(value, |(package, _)| package)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl App {
    /// `mimeOf`: the type Android knows for the extension, else what the content says.
    pub(crate) fn mime_of(&self, path: &Path) -> String {
        let ext = fmt::ext(&file_name(path));
        self.host
            .mime_for_extension(&ext)
            .unwrap_or_else(|| sniff::mime(path).to_string())
    }

    /// `openFile`: a tap on a plain file. The app saved for its extension opens it at once; with
    /// none (or one that is gone) the options sheet is shown.
    pub(crate) fn open_file_row(&self, key: &str) {
        let (path, size, time) = {
            let b = self.browser.borrow();
            let Some(row) = b.rows.iter().find(|r| r.key == key) else {
                return;
            };
            (b.cwd.join(key), row.size, row.time)
        };
        let ext = fmt::ext(&file_name(&path));
        let saved = if ext.is_empty() {
            String::new()
        } else {
            self.prefs.string(&default_key(&ext), "")
        };
        // "kiln:view" / "kiln:edit" belong to the viewer and editor of Phase C: show the sheet.
        if saved.is_empty() || saved.starts_with("kiln:") {
            self.file_sheet(path, size, time);
            return;
        }
        let mime = self.mime_of(&path);
        let with = OpenWith::Package(pkg_of(&saved).to_string());
        if !self.host.open_file(&path, &mime, false, &with) {
            // The app is gone: forget it (Defaults.clear).
            self.prefs.set_string(&default_key(&ext), "");
            self.toast("That app isn't available any more");
            self.file_sheet(path, size, time);
        }
    }

    /// `M_OPTIONS`: the options sheet for the one selected plain file. The selection is cleared
    /// first, as the Java does.
    pub(crate) fn menu_open_options(&self) {
        let (path, size, time) = {
            let b = self.browser.borrow();
            match b.only_selected() {
                Some(row) if row.kind == Kind::File => (b.cwd.join(&row.key), row.size, row.time),
                _ => return,
            }
        };
        self.browser.borrow_mut().clear_selection();
        self.render_rows();
        self.render_chrome();
        self.file_sheet(path, size, time);
    }

    /// `fileSheet`: size and date, then "Open with..." and "Edit with...". The "in Kiln" rows
    /// wait for the viewer and editor of Phase C.
    pub(crate) fn file_sheet(&self, path: PathBuf, size: i64, time: i64) {
        let mut spec = Spec::new(&file_name(&path));
        spec.body = format!(
            "{} \u{b7} {}",
            fmt::size(size),
            fmt::date(time, self.offset_at(time))
        );
        let open_path = path.clone();
        spec.options.push(opt("file", "Open with...", move |app| {
            app.app_chooser(open_path.clone(), false);
        }));
        spec.options.push(opt("edit", "Edit with...", move |app| {
            app.app_chooser(path.clone(), true);
        }));
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        self.show_dialog(spec);
    }

    /// `arcFileSheet`: size and date, then "Open with...", "Edit with..." and, when nothing is
    /// waiting to be saved, "Extract...". The "in Kiln" rows wait for Phase C.
    pub(crate) fn arc_file_sheet(&self, key: &str) {
        let (name, ent, dirty) = {
            let b = self.browser.borrow();
            let Some(arc) = b.arc.as_ref() else {
                return;
            };
            let Some(row) = b.rows.iter().find(|r| r.key == key) else {
                return;
            };
            let ent = arc.ents.iter().find(|e| e.p == key).map(|e| (e.s, e.m));
            (row.title.clone(), ent, arc.dirty)
        };
        let mut spec = Spec::new(&name);
        if let Some((size, time)) = ent {
            spec.body = fmt::size(size);
            if time > 0 {
                let date = fmt::date(time, self.offset_at(time));
                spec.body.push_str(&format!(" \u{b7} {date}"));
            }
        }
        let member = key.to_string();
        spec.options.push(opt("file", "Open with...", move |app| {
            app.preview_file(&member);
        }));
        let edit_member = key.to_string();
        spec.options.push(opt("edit", "Edit with...", move |app| {
            app.edit_external(&edit_member);
        }));
        if !dirty {
            let member = key.to_string();
            spec.options.push(opt("extract", "Extract...", move |app| {
                app.extract_dialog(vec![member.clone()]);
            }));
        }
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        self.show_dialog(spec);
    }

    /// `previewFile`, the "Open with..." mode: extract one file to the cache (or find it there
    /// already), then hand it to another app. It can be cancelled; nothing partial stays.
    fn preview_file(&self, member: &str) {
        let (file, password, sid, size) = {
            let b = self.browser.borrow();
            let Some(arc) = b.arc.as_ref() else {
                return;
            };
            let size = arc.ents.iter().find(|e| e.p == member).map_or(0, |e| e.s);
            (
                arc.file.clone(),
                arc.password.clone(),
                arc.sid.clone(),
                size,
            )
        };
        let cache = self.host.cache_dir().join("preview");
        let _ = std::fs::create_dir_all(&cache);
        let cache = cache.to_string_lossy().into_owned();
        let method = if sid.is_some() { "file_for" } else { "preview" };
        let args = match sid {
            Some(sid) => json!([sid, member, cache, true]),
            None => json!([file.to_string_lossy(), password, member, cache, size]),
        };
        self.run_op(
            "Opening...",
            true,
            move |job| kiln_bridge::call_job(method, &args, 0, job),
            |app, r| {
                if r["cancelled"].as_bool().unwrap_or(false) {
                    app.toast("Cancelled");
                    return;
                }
                if !r["ok"].as_bool().unwrap_or(false) {
                    app.message("Couldn't open", r["message"].as_str().unwrap_or_default());
                    return;
                }
                app.app_chooser(PathBuf::from(r["file"].as_str().unwrap_or_default()), false);
            },
        );
    }
    /// `editExternalFile` / `stageForEdit`: copy the file out of the archive's editing session and
    /// open it in another app. When we are back and it changed, `check_edited_file` offers to
    /// put the new version in.
    pub(crate) fn edit_external(&self, member: &str) {
        let member = member.to_string();
        self.with_session(Box::new(move |app| app.stage_for_edit(member)));
    }

    fn stage_for_edit(&self, member: String) {
        let (archive, sid) = {
            let b = self.browser.borrow();
            let Some(arc) = b.arc.as_ref() else {
                return;
            };
            let Some(sid) = arc.sid.clone() else {
                return;
            };
            (arc.file.clone(), sid)
        };
        let cache = self.host.cache_dir().join("edit");
        let _ = std::fs::create_dir_all(&cache);
        let args = json!([sid, member, cache.to_string_lossy(), false]);
        self.run_op(
            "Preparing...",
            true,
            move |job| kiln_bridge::call_job("file_for", &args, 0, job),
            move |app, r| {
                if r["cancelled"].as_bool().unwrap_or(false) {
                    app.toast("Cancelled");
                    return;
                }
                if !r["ok"].as_bool().unwrap_or(false) {
                    let msg = r["message"].as_str().or(r["error"].as_str());
                    app.message("Couldn't open", msg.unwrap_or_default());
                    return;
                }
                let file = PathBuf::from(r["file"].as_str().unwrap_or_default());
                let stamp = stamp_of(&file).unwrap_or(0);
                let staged = Staged {
                    archive,
                    member,
                    file: file.clone(),
                    stamp,
                };
                STAGED.with(|s| *s.borrow_mut() = Some(staged));
                app.app_chooser(file, true);
            },
        );
    }

    /// `checkEditedFile`, run when the app comes back to the front: if the copy that went to an
    /// editor changed, ask whether to put it in the archive.
    pub(crate) fn check_edited_file(&self) {
        let sid = self
            .browser
            .borrow()
            .arc
            .as_ref()
            .and_then(|a| a.sid.clone());
        let Some(sid) = sid else {
            return;
        };
        let Some(staged) = STAGED.with(|s| s.borrow_mut().take()) else {
            return;
        };
        let Some(now) = stamp_of(&staged.file) else {
            return; // the copy is gone: forgotten
        };
        if now == staged.stamp {
            STAGED.with(|s| *s.borrow_mut() = Some(staged));
            return;
        }
        let name = staged
            .member
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let body = format!(
            "\"{name}\" was changed. Put the new version in the archive? (You still need to Save.)"
        );
        let Staged {
            archive,
            member,
            file,
            ..
        } = staged;
        self.confirm_sheet(
            "Update the archive?",
            &body,
            "Update",
            BtnKind::Filled,
            Box::new(move |app| app.update_member(archive, sid, member, file)),
        );
    }

    /// `editOp("Updating...", set_file)`.
    fn update_member(&self, archive: PathBuf, sid: String, member: String, file: PathBuf) {
        let args = json!([sid, member, file.to_string_lossy()]);
        self.run_op(
            "Updating...",
            false,
            move |job| kiln_bridge::call_job("set_file", &args, 0, job),
            move |app, r| {
                if r["ok"].as_bool().unwrap_or(false) {
                    app.apply_edit_for(&archive, r);
                } else {
                    let msg = r["message"].as_str().or(r["error"].as_str());
                    app.message("Couldn't update", msg.unwrap_or_default());
                }
            },
        );
    }
    /// `appChooser`: the apps that can open (or edit) the file, "More options..." for the
    /// system's own list, and for Open a choice to remember the app for this extension.
    pub(crate) fn app_chooser(&self, path: PathBuf, edit: bool) {
        let mime = self.mime_of(&path);
        let apps = self.host.open_apps(&path, &mime, edit);
        self.chooser_sheet(path, edit, mime, apps, false);
    }

    /// The chooser sheet. Tapping the "Always open ..." row shows it again with the box flipped.
    fn chooser_sheet(
        &self,
        path: PathBuf,
        edit: bool,
        mime: String,
        apps: Vec<OpenApp>,
        always: bool,
    ) {
        let ext = fmt::ext(&file_name(&path));
        let mut spec = Spec::new(if edit { "Edit with" } else { "Open with" });
        if !edit && !ext.is_empty() {
            let (p, m, a) = (path.clone(), mime.clone(), apps.clone());
            let mut row = opt(
                "",
                &format!("Always open .{ext} files with this app"),
                move |app| {
                    app.chooser_sheet(p.clone(), edit, m.clone(), a.clone(), !always);
                },
            );
            row.selected = always;
            spec.options.push(row);
        }
        for found in &apps {
            let (p, m, e) = (path.clone(), mime.clone(), ext.clone());
            let (pkg, label) = (found.package.clone(), found.label.clone());
            spec.options.push(opt("file", &found.label, move |app| {
                let with = OpenWith::Package(pkg.clone());
                if app.host.open_file(&p, &m, edit, &with) {
                    if always && !edit {
                        app.prefs
                            .set_string(&default_key(&e), &format!("{pkg}|{label}"));
                    }
                } else {
                    app.toast("That app couldn't open the file");
                }
            }));
        }
        let (p, m) = (path, mime);
        spec.options
            .push(opt("file", "More options...", move |app| {
                if !app.host.open_file(&p, &m, edit, &OpenWith::Chooser) {
                    app.toast("No app can open this file");
                }
            }));
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        self.show_dialog(spec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_value_is_cut_at_the_bar() {
        assert_eq!(pkg_of("com.example.viewer|Viewer"), "com.example.viewer");
        assert_eq!(pkg_of("com.example.viewer"), "com.example.viewer");
    }

    #[test]
    fn key_has_the_java_prefix() {
        assert_eq!(default_key("txt"), "def_txt");
    }

    #[test]
    fn stamp_changes_with_time_or_size() {
        assert_eq!(stamp(1000, 5), 31005);
        assert_ne!(stamp(1000, 5), stamp(1001, 5));
        assert_ne!(stamp(1000, 5), stamp(1000, 6));
    }
}
