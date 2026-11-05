//! The dialog sheet (`Sheet.java`): a title, a line of text, an optional text field and buttons.
//! The window has one such sheet. [`App::show_dialog`] fills it and shows it, and what a button
//! does is a closure that the caller hands in.

use std::rc::Rc;

use kiln_ui::DlgOption;
use slint::{ModelRc, SharedString, VecModel};

use crate::{App, app_do};

/// What a button does, given the text in the field (empty when there is none). Return true to
/// close the sheet, false to keep it open, for example after a bad input (`Sheet.Click`).
pub(crate) type Click = Rc<dyn Fn(&App, &str) -> bool>;

/// Runs when the person backs out: Back, a tap outside, or a button with no action
/// (`Sheet.nextBack`).
pub(crate) type Back = Box<dyn FnOnce(&App)>;

/// What a menu row does (`Sheet.option`'s action). The sheet closes after it, unless the action
/// showed another dialog.
pub(crate) type Action = Rc<dyn Fn(&App)>;

/// `Ui.button` kinds: filled, outline, text, danger, tonal.
#[derive(Clone, Copy)]
pub(crate) enum BtnKind {
    Filled,
    Outline,
    Text,
    Danger,
    Tonal,
}

impl BtnKind {
    fn code(self) -> i32 {
        match self {
            Self::Filled => 0,
            Self::Outline => 1,
            Self::Text => 2,
            Self::Danger => 3,
            Self::Tonal => 4,
        }
    }
}

/// One button, in the order `Sheet.button()` would have been called.
pub(crate) struct Btn {
    pub label: String,
    pub kind: BtnKind,
    /// `None` is a button with no action (Cancel, OK): it only closes the sheet, as a back-out.
    pub click: Option<Click>,
}

/// One option row, in the order `Sheet.option()` would have been called: an icon in a tile, a
/// label, and a check mark when selected. Tapping it runs `action`, then closes the sheet.
pub(crate) struct Opt {
    /// An icon name; empty for no tile.
    pub icon: &'static str,
    pub label: String,
    pub selected: bool,
    pub action: Action,
}

/// The text field of a dialog.
pub(crate) struct Field {
    pub placeholder: String,
    pub text: String,
}

/// Everything a dialog shows.
pub(crate) struct Spec {
    pub title: String,
    pub body: String,
    /// Option rows between the text and the buttons.
    pub options: Vec<Opt>,
    pub field: Option<Field>,
    pub buttons: Vec<Btn>,
    /// Back and a tap outside close it (`Sheet.cancelable`).
    pub cancelable: bool,
    /// What a back-out does, for example bringing the previous menu back (`Sheet.nextBack`).
    pub back: Option<Back>,
}

impl Spec {
    /// A dialog with a title that can be backed out of; fill in the rest.
    pub(crate) fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            body: String::new(),
            options: Vec::new(),
            field: None,
            buttons: Vec::new(),
            cancelable: true,
            back: None,
        }
    }
}

/// What the sheet that is up will do.
#[derive(Default)]
pub(crate) struct Dialog {
    clicks: Vec<Option<Click>>,
    /// What each option row does, in order.
    options: Vec<Action>,
    back: Option<Back>,
    /// Counts the dialogs shown, so that a click that opens another one does not close it.
    serial: u64,
}

impl App {
    /// Connect the dialog sheet's callbacks.
    pub(crate) fn wire_dialog(&self) {
        let ui = &self.ui;
        ui.on_dlg_button_clicked(|i| app_do(|app| app.dialog_button(i)));
        ui.on_dlg_cancelled(|| app_do(|app| app.cancel_dialog()));
        ui.on_dlg_option_clicked(|i| app_do(|app| app.dialog_option(i)));
    }

    /// Show a dialog (`new Sheet(..)...show()`). One that is already up is replaced.
    pub(crate) fn show_dialog(&self, spec: Spec) {
        let ui = &self.ui;
        let labels: Vec<SharedString> = spec
            .buttons
            .iter()
            .map(|b| b.label.as_str().into())
            .collect();
        let kinds: Vec<i32> = spec.buttons.iter().map(|b| b.kind.code()).collect();
        let (rows, actions): (Vec<DlgOption>, Vec<Action>) = spec
            .options
            .into_iter()
            .map(|o| {
                let row = DlgOption {
                    icon: o.icon.into(),
                    label: o.label.as_str().into(),
                    selected: o.selected,
                };
                (row, o.action)
            })
            .unzip();
        let has_field = spec.field.is_some();
        let (placeholder, text) = match spec.field {
            Some(f) => (f.placeholder, f.text),
            None => (String::new(), String::new()),
        };
        ui.set_dlg_title(spec.title.into());
        ui.set_dlg_body(spec.body.into());
        ui.set_dlg_has_field(has_field);
        ui.set_dlg_error(SharedString::new());
        ui.set_dlg_placeholder(placeholder.into());
        ui.set_dlg_text(text.into());
        ui.set_dlg_buttons(ModelRc::new(VecModel::from(labels)));
        ui.set_dlg_kinds(ModelRc::new(VecModel::from(kinds)));
        ui.set_dlg_options(ModelRc::new(VecModel::from(rows)));
        ui.set_dlg_cancelable(spec.cancelable);
        {
            let mut d = self.dialog.borrow_mut();
            d.serial += 1;
            d.clicks = spec.buttons.into_iter().map(|b| b.click).collect();
            d.options = actions;
            d.back = spec.back;
        }
        ui.set_dlg_shown(true);
    }

    /// A button was tapped. One with no action backs out; the others close the sheet when their
    /// action says so, unless the action showed another dialog meanwhile.
    fn dialog_button(&self, index: i32) {
        let (slot, serial) = {
            let d = self.dialog.borrow();
            let slot = usize::try_from(index)
                .ok()
                .and_then(|i| d.clicks.get(i).cloned());
            (slot, d.serial)
        };
        let Some(slot) = slot else {
            return;
        };
        let Some(click) = slot else {
            self.cancel_dialog();
            return;
        };
        let text = self.ui.get_dlg_text();
        if click(self, text.as_str()) && self.dialog.borrow().serial == serial {
            self.close_dialog();
        }
    }

    /// An option row was tapped (`Sheet.option`): run its action, then close the sheet unless the
    /// action showed another dialog. Choosing a row is not a back-out, so `back` does not run.
    fn dialog_option(&self, index: i32) {
        let (action, serial) = {
            let d = self.dialog.borrow();
            let action = usize::try_from(index)
                .ok()
                .and_then(|i| d.options.get(i).cloned());
            (action, d.serial)
        };
        let Some(action) = action else {
            return;
        };
        action(self);
        if self.dialog.borrow().serial == serial {
            self.close_dialog();
        }
    }

    /// The person backed out: close, then run the "go back" action (`userCancel`).
    fn cancel_dialog(&self) {
        let back = self.dialog.borrow_mut().back.take();
        self.close_dialog();
        if let Some(back) = back {
            back(self);
        }
    }

    /// Close without backing out (`dismiss()` after a click that said so).
    pub(crate) fn close_dialog(&self) {
        self.dialog.borrow_mut().back = None;
        self.ui.set_dlg_shown(false);
    }

    /// Back while a dialog is up. True when the key was used up by it.
    pub(crate) fn dialog_back(&self) -> bool {
        if !self.ui.get_dlg_shown() {
            return false;
        }
        if self.ui.get_dlg_cancelable() {
            self.cancel_dialog();
        }
        true
    }
}

impl App {
    /// `MainActivity.nameSheet`: a sheet with a name field, Cancel and an `ok` button. `make` gets
    /// the trimmed name and returns the line to show under the field when it failed, or `None`
    /// when it worked (the sheet then closes).
    pub(crate) fn name_sheet(
        &self,
        heading: &str,
        initial: &str,
        ok: &str,
        make: impl Fn(&App, &str) -> Option<String> + 'static,
    ) {
        let mut spec = Spec::new(heading);
        spec.field = Some(Field {
            placeholder: "Name".to_string(),
            text: initial.to_string(),
        });
        spec.buttons.push(Btn {
            label: "Cancel".to_string(),
            kind: BtnKind::Text,
            click: None,
        });
        let click: Click = Rc::new(move |app: &App, text: &str| -> bool {
            let name = text.trim();
            if name.is_empty() || name.contains('/') {
                app.ui.set_dlg_error("Enter a name (no slashes)".into());
                return false;
            }
            match make(app, name) {
                Some(error) => {
                    app.ui.set_dlg_error(error.into());
                    false
                }
                None => true,
            }
        });
        spec.buttons.push(Btn {
            label: ok.to_string(),
            kind: BtnKind::Filled,
            click: Some(click),
        });
        let end = initial
            .rfind('.')
            .filter(|&dot| dot > 0)
            .unwrap_or(initial.len());
        self.ui.set_dlg_select_end(end as i32);
        self.show_dialog(spec);
    }
}
