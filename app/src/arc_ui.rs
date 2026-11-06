//! An archive on screen (`MainActivity.openItem`, `doOpen`, `onOpened`, `guardLeave`,
//! `saveArchive`, `discardArchive`, `withSession`, `arcTransfer`, `arcRename`, the archive menu and
//! bottom bar). The state is in [`crate::arc`]; Extract, Check and Info are in `extract_ui.rs`.
//!
//! TODO(task 25): remove the `allow` below when every piece of this module is in use.
#![allow(dead_code)]

use std::path::Path;

use serde_json::Value;

use crate::App;
use crate::dialog::{Btn, BtnKind};

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
