//! An archive on screen (`MainActivity.openItem`, `doOpen`, `onOpened`, `guardLeave`,
//! `saveArchive`, `discardArchive`, `withSession`, `arcTransfer`, `arcRename`, the archive menu and
//! bottom bar). The state is in [`crate::arc`]; Extract, Check and Info are in `extract_ui.rs`.
//!
//! TODO(task 25): remove the `allow` below when every piece of this module is in use.
#![allow(dead_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::arc::ArcState;
use crate::browse_ui::opt;
use crate::browser::Kind;
use crate::dialog::{Btn, BtnKind, Click, Field, Spec};
use crate::{App, fmt};

/// What runs once something has been done (`Runnable`).
pub(crate) type After = Box<dyn FnOnce(&App)>;

/// A tile of the archive's bottom bar.
#[derive(Clone, Copy)]
pub(crate) enum Tile {
    ExtractSelected,
    Copy,
    Move,
    Delete,
    ExtractAll,
    Add,
    Check,
    Info,
}

/// What the archive screens keep between taps: the tiles the bar shows now, so that
/// `arc_tile(index)` knows what a tap means.
#[derive(Default)]
pub(crate) struct ArcUi {
    pub(crate) tiles: Vec<Tile>,
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `r.optString("message", r.optString("error"))`.
pub(crate) fn why(r: &Value) -> String {
    r["message"]
        .as_str()
        .or_else(|| r["error"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A "Cancel" button that only closes the sheet.
pub(crate) fn cancel_button() -> Btn {
    Btn {
        label: "Cancel".to_string(),
        kind: BtnKind::Text,
        click: None,
    }
}

impl App {
    /// The offset of the phone's time zone now; the archive's dates use it.
    fn now_offset(&self) -> i32 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        self.offset_at(now)
    }

    /// `openItem`: ask for the password of a set or an encrypted archive, then open it.
    pub(crate) fn open_row(&self, key: &str) {
        let (file, locked) = {
            let b = self.browser.borrow();
            let Some(row) = b.rows.iter().find(|r| r.key == key) else {
                return;
            };
            (b.cwd.join(key), row.kind == Kind::Set || row.icon == "lock")
        };
        if locked {
            let name = file_name(&file);
            self.ask_password(&name, move |app, pw| app.do_open(file, pw, None));
        } else {
            self.do_open(file, String::new(), None);
        }
    }

    /// `askPassword`: a sheet with the password field and "Show password".
    pub(crate) fn ask_password(&self, name: &str, done: impl FnOnce(&App, String) + 'static) {
        let done = Rc::new(RefCell::new(Some(done)));
        let mut spec = Spec::new(&format!("Unlock {name}"));
        spec.field = Some(Field {
            placeholder: "Password".to_string(),
            text: String::new(),
        });
        spec.buttons.push(cancel_button());
        let click: Click = Rc::new(move |app: &App, text: &str| -> bool {
            if text.is_empty() {
                app.ui.set_dlg_error("Enter the password".into());
                return false;
            }
            let run = done.borrow_mut().take();
            if let Some(run) = run {
                run(app, text.to_string());
            }
            true
        });
        spec.buttons.push(Btn {
            label: "Unlock".to_string(),
            kind: BtnKind::Filled,
            click: Some(click),
        });
        self.show_dialog(spec);
        self.ui.set_dlg_secret(true);
    }

    /// `doOpen`: read the archive's table of contents behind the progress sheet.
    pub(crate) fn do_open(&self, file: PathBuf, pw: String, after: Option<After>) {
        let heading = if pw.is_empty() {
            "Opening..."
        } else {
            "Unlocking..."
        };
        let args = json!([file.to_string_lossy(), pw]);
        self.run_op(
            heading,
            false,
            move |_| kiln_bridge::call("open_archive", &args, 0),
            move |app, r| app.on_opened(file, pw, r, after),
        );
    }

    /// `onOpened`.
    fn on_opened(&self, file: PathBuf, pw: String, r: &Value, after: Option<After>) {
        if !r["ok"].as_bool().unwrap_or(false) {
            let name = file_name(&file);
            let bad = r["bad_password"].as_bool().unwrap_or(false);
            if bad || r["need_password"].as_bool().unwrap_or(false) {
                if bad {
                    self.toast("Incorrect password");
                }
                self.ask_password(&name, move |app, p| app.do_open(file, p, after));
            } else {
                let msg = why(r);
                let msg = if msg.is_empty() {
                    "Unknown error"
                } else {
                    &msg
                };
                self.message(&format!("Couldn't open {name}"), msg);
            }
            return;
        }
        // TODO(task 32): the home of a file opened from another app goes here instead of None.
        let arc = ArcState::from_open(file, pw, None, r);
        let offset = self.now_offset();
        self.close_session();
        {
            let mut b = self.browser.borrow_mut();
            b.set_date_offset(offset);
            b.open_arc(arc);
        }
        self.render_rows();
        self.render_chrome();
        // TODO(task 24): `render_arc_bar` shows the bottom bar here.
        self.ui.invoke_scroll_list_top();
        if let Some(after) = after {
            after(self);
        }
    }

    /// `closeSession`: forget the editing session of the open archive.
    pub(crate) fn close_session(&self) {
        let sid = {
            let mut b = self.browser.borrow_mut();
            let Some(arc) = b.arc.as_mut() else {
                return;
            };
            arc.dirty = false;
            arc.sid.take()
        };
        if let Some(sid) = sid {
            std::thread::spawn(move || {
                let _ = kiln_bridge::call("close", &json!([sid]), 0);
            });
        }
    }

    /// `closeArchive`: leave the archive for the folder it was opened from.
    pub(crate) fn close_archive(&self) {
        self.close_session();
        let dir = {
            let mut b = self.browser.borrow_mut();
            b.arc = None;
            b.selected.clear();
            b.cwd.clone()
        };
        self.ui.set_arc_bar_shown(false);
        self.ui.set_arc_dirty(false);
        self.load_dir(dir, false);
    }
}

impl App {
    /// `onRowClick` inside an archive: a folder is entered, a file gets its sheet. False when no
    /// archive is open.
    pub(crate) fn arc_row_clicked(&self, key: &str, kind: Option<Kind>) -> bool {
        if self.browser.borrow().arc.is_none() {
            return false;
        }
        if matches!(kind, Some(Kind::Dir)) {
            self.arc_enter(key);
        } else {
            // TODO(task 25): `arcFileSheet` (open options) comes with the Open part.
            self.toast("Not in this build yet");
        }
        true
    }

    /// `arcEnter`: open a folder of the archive; the one left is remembered where it was scrolled.
    fn arc_enter(&self, prefix: &str) {
        let y = self.ui.get_list_pos();
        {
            let mut b = self.browser.borrow_mut();
            let searching = b.searching;
            if let Some(arc) = b.arc.as_mut() {
                if !searching {
                    arc.pos.insert(arc.prefix.clone(), y);
                }
                arc.prefix = prefix.to_string();
            }
            b.arc_rebuild();
        }
        self.render_rows();
        self.render_chrome();
        self.ui.invoke_scroll_list_top();
    }

    /// `arcBackTo`: Back to a folder of the archive, at the spot it was left.
    pub(crate) fn arc_back_to(&self, prefix: &str) {
        let y = {
            let mut b = self.browser.borrow_mut();
            let y = match b.arc.as_mut() {
                Some(arc) => {
                    arc.prefix = prefix.to_string();
                    arc.pos.get(prefix).copied().unwrap_or(0.0)
                }
                None => 0.0,
            };
            b.arc_rebuild();
            y
        };
        self.render_rows();
        self.render_chrome();
        self.ui.invoke_scroll_list_to(y);
    }
}

impl App {
    fn arc_sid(&self) -> Option<String> {
        self.browser
            .borrow()
            .arc
            .as_ref()
            .and_then(|a| a.sid.clone())
    }

    /// `applyEdit`: take what an editing call answered, and show it.
    pub(crate) fn apply_edit(&self, r: &Value) {
        {
            let mut b = self.browser.borrow_mut();
            let Some(arc) = b.arc.as_mut() else {
                return;
            };
            arc.apply_edit(r);
            b.selected.clear();
            b.arc_rebuild();
        }
        self.render_rows();
        self.render_chrome();
        // TODO(task 24): `render_arc_bar` redraws the bottom bar here.
    }

    /// `apply_edit`, but only when `file` is still the archive that is open.
    pub(crate) fn apply_edit_for(&self, file: &Path, r: &Value) {
        let same = self
            .browser
            .borrow()
            .arc
            .as_ref()
            .is_some_and(|a| a.file == file);
        if same {
            self.apply_edit(r);
        } else if let Some(arc) = self.browser.borrow_mut().arc.as_mut() {
            arc.dirty = false;
        }
    }

    /// `withSession`: make sure the archive has an editing session (`begin`), then run `then`.
    pub(crate) fn with_session(&self, then: After) {
        let (file, pw, has) = {
            let b = self.browser.borrow();
            let Some(arc) = b.arc.as_ref() else {
                return;
            };
            (arc.file.clone(), arc.password.clone(), arc.sid.is_some())
        };
        if has {
            then(self);
            return;
        }
        let args = json!([file.to_string_lossy(), pw]);
        self.run_op(
            "Preparing...",
            true,
            move |job| kiln_bridge::call_job("begin", &args, 0, job),
            move |app, r| {
                if r["cancelled"].as_bool().unwrap_or(false) {
                    app.toast("Cancelled");
                    return;
                }
                if !r["ok"].as_bool().unwrap_or(false) {
                    app.message("Can't edit this archive", &why(r));
                    return;
                }
                let sid = r["sid"].as_str().unwrap_or_default().to_string();
                let same = {
                    let mut b = app.browser.borrow_mut();
                    match b.arc.as_mut() {
                        Some(arc) if arc.file == file => {
                            arc.sid = Some(sid.clone());
                            true
                        }
                        _ => false,
                    }
                };
                if !same {
                    // left the archive while it was loading
                    std::thread::spawn(move || {
                        let _ = kiln_bridge::call("close", &json!([sid]), 0);
                    });
                    return;
                }
                app.apply_edit(r);
                then(app);
            },
        );
    }

    /// `arcTransfer`: move or copy items inside the archive (Rename is a move with a new name).
    pub(crate) fn arc_transfer(
        &self,
        keys: Vec<String>,
        dest: String,
        mv: bool,
        policy: &'static str,
        rename_to: String,
    ) {
        let Some(sid) = self.arc_sid() else {
            return;
        };
        let args = json!([
            sid,
            crate::arc::members_json(&keys),
            dest,
            mv,
            policy,
            rename_to
        ]);
        let heading = if mv { "Moving..." } else { "Copying..." };
        self.run_op(
            heading,
            false,
            move |_| kiln_bridge::call("transfer", &args, 0),
            move |app, r| {
                if r["ok"].as_bool().unwrap_or(false) {
                    // TODO(task 24): the archive's clip (`arcClip`) is cleared here.
                    app.apply_edit(r);
                    return;
                }
                let conflicts = r["conflicts"].as_array().cloned().unwrap_or_default();
                if conflicts.is_empty() {
                    let verb = if mv { "move" } else { "copy" };
                    app.message(&format!("Couldn't {verb}"), &why(r));
                    return;
                }
                let mut names = String::new();
                for c in conflicts.iter().take(5) {
                    names.push_str(&format!("\u{2022} {}\n", c.as_str().unwrap_or_default()));
                }
                let total = r["n_conflicts"]
                    .as_i64()
                    .unwrap_or_else(|| i64::try_from(conflicts.len()).unwrap_or(0));
                if total > 5 {
                    names.push_str(&format!("and {} more\n", total - 5));
                }
                let mut spec = Spec::new(&fmt::plural(
                    total,
                    "item already exists",
                    "items already exist",
                ));
                spec.body = names.trim().to_string();
                for (label, again) in [
                    ("Replace", "replace"),
                    ("Skip existing", "skip"),
                    ("Keep both", "keep"),
                ] {
                    let (keys, dest, rename_to) = (keys.clone(), dest.clone(), rename_to.clone());
                    spec.options.push(opt("", label, move |app| {
                        app.arc_transfer(keys.clone(), dest.clone(), mv, again, rename_to.clone());
                    }));
                }
                spec.buttons.push(cancel_button());
                app.show_dialog(spec);
            },
        );
    }

    /// `arcRename`: rename the one selected item inside the archive.
    pub(crate) fn arc_rename(&self) {
        let key = {
            let b = self.browser.borrow();
            match b.selected.as_slice() {
                [key] if b.arc.is_some() => key.clone(),
                _ => return,
            }
        };
        let full = key.trim_end_matches('/').to_string();
        let old = full.rsplit('/').next().unwrap_or(&full).to_string();
        let parent = full
            .rfind('/')
            .map_or(String::new(), |i| full[..i].to_string());
        let initial = old.clone();
        self.name_sheet("Rename", &initial, "Rename", move |app, name| {
            if name == old {
                return None;
            }
            let (key, parent, name) = (key.clone(), parent.clone(), name.to_string());
            app.with_session(Box::new(move |app| {
                app.arc_transfer(vec![key], parent, true, "ask", name);
            }));
            None
        });
    }
}

impl App {
    fn arc_dirty(&self) -> bool {
        self.browser
            .borrow()
            .arc
            .as_ref()
            .is_some_and(|a| a.dirty)
    }

    /// The open archive's file and editing session.
    fn arc_file_and_sid(&self) -> (PathBuf, Option<String>) {
        let b = self.browser.borrow();
        match b.arc.as_ref() {
            Some(a) => (a.file.clone(), a.sid.clone()),
            None => (PathBuf::new(), None),
        }
    }

    /// `guardLeave`: runs `then` now, or after asking what to do with unsaved changes.
    pub(crate) fn guard_leave(&self, then: After) {
        if !self.arc_dirty() {
            then(self);
            return;
        }
        let then = Rc::new(RefCell::new(Some(then)));
        let mut spec = Spec::new("Unsaved changes");
        spec.body = "This archive has changes that haven't been saved.".to_string();
        spec.buttons.push(Btn {
            label: "Keep editing".to_string(),
            kind: BtnKind::Outline,
            click: None,
        });
        let discard = then.clone();
        let discard_click: Click = Rc::new(move |app: &App, _text: &str| -> bool {
            app.close_session();
            let run = discard.borrow_mut().take();
            if let Some(run) = run {
                run(app);
            }
            true
        });
        spec.buttons.push(Btn {
            label: "Discard".to_string(),
            kind: BtnKind::Danger,
            click: Some(discard_click),
        });
        let save_click: Click = Rc::new(move |app: &App, _text: &str| -> bool {
            let after = then.borrow_mut().take();
            app.save_archive(after);
            true
        });
        spec.buttons.push(Btn {
            label: "Save".to_string(),
            kind: BtnKind::Filled,
            click: Some(save_click),
        });
        self.show_dialog(spec);
    }

    /// `confirm`: a sheet with Cancel and one button that runs `action`.
    pub(crate) fn confirm_sheet(
        &self,
        title: &str,
        body: &str,
        label: &str,
        kind: BtnKind,
        action: After,
    ) {
        let action = Rc::new(RefCell::new(Some(action)));
        let mut spec = Spec::new(title);
        spec.body = body.to_string();
        spec.buttons.push(cancel_button());
        let click: Click = Rc::new(move |app: &App, _text: &str| -> bool {
            let run = action.borrow_mut().take();
            if let Some(run) = run {
                run(app);
            }
            true
        });
        spec.buttons.push(Btn {
            label: label.to_string(),
            kind,
            click: Some(click),
        });
        self.show_dialog(spec);
    }

    /// `saveArchive`. (`Priv` is not in this build, so nothing is written back to a real place.)
    pub(crate) fn save_archive(&self, after: Option<After>) {
        let (file, sid) = self.arc_file_and_sid();
        let Some(sid) = sid else {
            if let Some(after) = after {
                after(self);
            }
            return;
        };
        let args = json!([sid]);
        self.run_op(
            "Saving...",
            true,
            move |job| kiln_bridge::call_job("save", &args, 0, job),
            move |app, r| {
                if r["cancelled"].as_bool().unwrap_or(false) {
                    app.toast("Cancelled");
                } else if r["ok"].as_bool().unwrap_or(false) {
                    app.apply_edit_for(&file, r);
                    app.toast("Saved");
                    if let Some(after) = after {
                        after(app);
                    }
                } else {
                    app.message("Couldn't save", &why(r));
                }
            },
        );
    }

    /// `discardArchive`.
    pub(crate) fn discard_archive(&self, after: Option<After>) {
        let (file, sid) = self.arc_file_and_sid();
        let Some(sid) = sid else {
            if let Some(after) = after {
                after(self);
            }
            return;
        };
        self.confirm_sheet(
            "Discard changes?",
            "Everything you changed since the last save is thrown away.",
            "Discard",
            BtnKind::Danger,
            Box::new(move |app| {
                // TODO(task 24): the archive's clip (`arcClip`) is cleared here.
                let args = json!([sid]);
                app.run_op(
                    "Discarding...",
                    false,
                    move |_| kiln_bridge::call("discard", &args, 0),
                    move |app, r| {
                        if r["ok"].as_bool().unwrap_or(false) {
                            app.apply_edit_for(&file, r);
                            if let Some(after) = after {
                                after(app);
                            }
                        } else {
                            app.message("Couldn't discard", &why(r));
                        }
                    },
                );
            }),
        );
    }
}