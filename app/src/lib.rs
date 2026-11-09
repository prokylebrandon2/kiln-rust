//! The Kiln Android app: the Slint window (`kiln-ui-next`) wired to the phone (a [`Host`],
//! implemented by the `platform` crate) and to the file system (`kiln-state`).
//!
//! This crate knows nothing about Android. [`run`] is the whole app: it builds the window,
//! applies the system's look, loads the Files tab, and runs Slint's event loop until the app
//! closes. Everything it touches belongs to the UI thread, except the folder listings, whose
//! results are posted back with `slint::invoke_from_event_loop`.
//!
//! The screens are in `screens`. The modules of the old window (`about_ui`, `arc_ui`,
//! `browse_ui`, `dialog`, `extract_ui`, `folder_ui`, `marks`, `open_ui`, `pack_ui`, `props_ui`,
//! `settings_ui`, `thumbs_ui`, `pull`) are not part of the build any more: they are the
//! reference for porting each feature to the new UI (docs/UI-REWRITE.md) and are deleted with
//! `ui/` once that is done.

#![forbid(unsafe_code)]

pub mod arc;
pub mod browser;
pub mod crash;
pub mod engine;
pub mod fmt;
pub mod host;
pub mod icons;
pub mod prefs;
pub mod props;
mod screens;
pub mod sniff;
pub mod thumbs;
// Not used until the recycle bin is wired in; the allow goes with it.
#[allow(dead_code)]
mod trash;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kiln_state::fmt::Units;
use kiln_ui_next::MainWindow;
use slint::ComponentHandle;

pub use crate::host::{Event, Host, Incoming, Look, OpenApp, OpenWith, Volume};
use crate::prefs::Prefs;
use crate::screens::Screens;

/// Off, Fast, Normal, Relaxed. Settings > Animation speed picks one.
const ANIM_SCALE: [f32; 4] = [0.0, 0.6, 0.85, 1.2];

struct App {
    ui: MainWindow,
    host: Rc<dyn Host>,
    prefs: Prefs,
    screens: Screens,
}

thread_local! {
    /// The running app, for code that arrives on the UI thread without a handle to it (a result
    /// from a worker thread, a timer).
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Run `f` with the running app, if there still is one.
fn app_do(f: impl FnOnce(&Rc<App>)) {
    let app = APP.with(|a| a.borrow().clone());
    if let Some(app) = app {
        f(&app);
    }
}

/// Run the app until it is closed. Call it on the UI thread, after Slint's backend is set up.
pub fn run(host: Rc<dyn Host>) -> Result<(), slint::PlatformError> {
    let files = host.files_dir();
    let cache = host.cache_dir();

    crash::install(files.join("last-crash.txt"), host.version_name());
    // Scratch copies from the last run; the preview cache is kept (and trimmed with the engine).
    let _ = std::fs::remove_dir_all(cache.join("incoming"));
    let _ = std::fs::remove_dir_all(cache.join("edit"));

    let ui = MainWindow::new()?;
    let prefs = Prefs::load(&prefs_file(&files));
    let units = if prefs.bool("units_decimal", false) {
        Units::Decimal
    } else {
        Units::Binary
    };
    let screens = Screens::new(&ui, host.clone(), units);
    let app = Rc::new(App {
        ui,
        host: host.clone(),
        prefs,
        screens,
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    app.apply_look();
    app.wire_screens();
    app.refresh_permission();

    let weak = Rc::downgrade(&app);
    host.set_event_handler(Box::new(move |event| {
        if let Some(app) = weak.upgrade() {
            app.on_event(event);
        }
    }));

    app.ui.run()
}

/// The settings file (`SharedPreferences` "kiln").
fn prefs_file(files_dir: &Path) -> PathBuf {
    let data = files_dir.parent().unwrap_or(files_dir);
    data.join("shared_prefs").join("kiln.xml")
}

impl App {
    fn on_event(&self, event: Event) {
        match event {
            // The user may just have granted access in Settings.
            Event::Resumed => self.refresh_permission(),
            Event::ConfigChanged => self.apply_look(),
            Event::LowMemory | Event::Paused => {}
        }
    }
}
