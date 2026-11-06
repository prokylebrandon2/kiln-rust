//! The Kiln Android app: the Slint window (`kiln-ui`) wired to the engine (`kiln-bridge`) and to
//! the phone (a [`Host`], implemented by the `platform` crate).
//!
//! This crate knows nothing about Android. [`run`] is the whole app: it builds the window, applies
//! the system's look, starts the engine on a worker thread, and runs Slint's event loop until the
//! app closes. Everything it touches belongs to the UI thread, except the engine start-up and the
//! folder listings, whose results are posted back with `slint::invoke_from_event_loop`.

#![forbid(unsafe_code)]

mod about_ui;
pub mod arc;
mod arc_ui;
mod browse_ui;
pub mod browser;
pub mod crash;
// Not called yet: the menus use it from the next step on, and the allow goes with them.
#[allow(dead_code)]
mod dialog;
pub mod engine;
mod extract_ui;
pub mod fmt;
mod folder_ui;
pub mod host;
pub mod icons;
mod marks;
mod pack_ui;
pub mod prefs;
pub mod props;
mod props_ui;
mod pull;
pub mod sniff;
// Not used until the recycle bin is wired in (B5); the allow goes with it.
#[allow(dead_code)]
mod trash;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use kiln_ui::{MainWindow, MarkRow, RowData, Theme};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::browser::{Browser, SortKey};
pub use crate::host::{Event, Host, Look, Volume};
use crate::prefs::Prefs;

/// `Defaults.ANIM_SCALE`: Off, Fast, Normal, Relaxed. Settings > Animation speed picks one.
const ANIM_SCALE: [f32; 4] = [0.0, 0.6, 0.85, 1.2];

/// What the UI is waiting for before it can show the first folder.
#[derive(Default)]
struct Status {
    engine_up: bool,
    engine_error: Option<String>,
    loaded_once: bool,
}

struct App {
    ui: MainWindow,
    host: Rc<dyn Host>,
    prefs: Prefs,
    status: RefCell<Status>,
    browser: RefCell<Browser>,
    model: Rc<VecModel<RowData>>,
    /// Offsets of the phone's time zone from UTC, by day since 1970, so that a folder of
    /// thousands of files asks the phone once per day and not once per file.
    offsets: RefCell<HashMap<i64, i32>>,
    toast_gen: Cell<u64>,
    /// "Loading..." is only shown when a folder is slow (700 ms).
    loading_hint: Cell<bool>,
    /// Where the list was last scrolled to, and whether "+ New" is hidden because of it.
    scroll_last: Cell<f32>,
    fab_hidden: Cell<bool>,
    was_home: Cell<bool>,
    last_dir: RefCell<PathBuf>,
    /// The lines of the bookmarks drop-down.
    marks_model: Rc<VecModel<MarkRow>>,
    /// When the phone was last asked for its USB drives and SD cards (`volumes()`, cached 3 s).
    volumes_at: Cell<Option<Instant>>,
    /// Set to stop the folder size count of the pull-down strip that is running.
    pull_stop: RefCell<Arc<AtomicBool>>,
    /// What the buttons of the dialog sheet that is up do (see dialog.rs).
    dialog: RefCell<dialog::Dialog>,
    /// The operation whose progress sheet is up, or about to be (see dialog.rs), and a counter
    /// that tells one from the next.
    op: RefCell<Option<dialog::Running>>,
    op_gen: Cell<u64>,
    props: RefCell<props_ui::Open>,
    pack: RefCell<pack_ui::Open>,
    extract: RefCell<extract_ui::Open>,
    chooser: RefCell<folder_ui::Chooser>,
    arcui: RefCell<arc_ui::ArcUi>,
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
    let app = Rc::new(App {
        ui,
        prefs: Prefs::load(&prefs_file(&files)),
        host: host.clone(),
        status: RefCell::default(),
        browser: RefCell::new(Browser::new(host.storage_root())),
        model: Rc::new(VecModel::default()),
        offsets: RefCell::default(),
        toast_gen: Cell::new(0),
        loading_hint: Cell::new(false),
        scroll_last: Cell::new(0.0),
        fab_hidden: Cell::new(false),
        was_home: Cell::new(false),
        last_dir: RefCell::default(),
        marks_model: Rc::new(VecModel::default()),
        volumes_at: Cell::new(None),
        pull_stop: RefCell::new(Arc::new(AtomicBool::new(true))),
        dialog: RefCell::default(),
        op: RefCell::default(),
        op_gen: Cell::new(0),
        props: RefCell::default(),
        pack: RefCell::default(),
        extract: RefCell::default(),
        chooser: RefCell::default(),
        arcui: RefCell::default(),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    app.read_settings();
    app.ui.set_rows(ModelRc::from(app.model.clone()));
    app.ui.set_mark_rows(ModelRc::from(app.marks_model.clone()));
    app.apply_look();
    app.refresh_permission();
    app.wire();
    if let Some(report) = crash::take(&files.join("last-crash.txt")) {
        app.ui.set_sheet_title("Kiln stopped last time".into());
        app.ui.set_sheet_body(report.into());
        app.ui.set_sheet_shown(true);
    }

    let weak = Rc::downgrade(&app);
    host.set_event_handler(Box::new(move |event| {
        if let Some(app) = weak.upgrade() {
            app.on_event(event);
        }
    }));

    start_engine(cache);
    app.ui.run()
}

/// Where the Java app kept its settings, which this app reads and writes too
/// (`SharedPreferences` "kiln").
fn prefs_file(files_dir: &Path) -> PathBuf {
    let data = files_dir.parent().unwrap_or(files_dir);
    data.join("shared_prefs").join("kiln.xml")
}

/// `MainActivity.startEngine()`: get the engine ready off the UI thread, then tell the UI.
fn start_engine(cache: PathBuf) {
    std::thread::spawn(move || {
        let result = engine::init(&cache);
        // The window may be gone by now; then there is nobody to tell.
        let _ = slint::invoke_from_event_loop(move || {
            app_do(|app| app.engine_ready(result));
        });
    });
}

impl App {
    /// `onCreate`: the settings the browser starts with.
    fn read_settings(&self) {
        let mut b = self.browser.borrow_mut();
        b.sort_key = SortKey::from_code(self.prefs.int("sort_key", 0));
        b.sort_asc = self.prefs.bool("sort_asc", true);
        b.show_all = self.prefs.bool("show_all", true);
        b.show_hidden = self.prefs.bool("show_hidden", false);
        b.up_row_on = self.prefs.bool("up_row", true);
    }

    /// Hand the system's colour scheme, density and font scale, and the user's animation speed,
    /// to the `Theme` global that every component reads.
    fn apply_look(&self) {
        let look = self.host.look();
        let level = self.prefs.int("anim_level", 2).clamp(0, 3) as usize;
        let theme = self.ui.global::<Theme>();
        theme.set_dark(look.dark);
        theme.set_density(look.density);
        theme.set_font_scale(look.font_scale);
        theme.set_anim_scale(ANIM_SCALE[level]);
    }

    /// Connect what the window reports to what the app does.
    fn wire(&self) {
        let host = self.host.clone();
        self.ui
            .on_allow_access(move || host.open_all_files_settings());
        let ui = self.ui.as_weak();
        self.ui.on_sheet_dismissed(move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_sheet_shown(false);
            }
        });
        self.wire_browser();
        self.wire_dialog();
        self.wire_props();
        self.wire_pack();
        self.wire_extract();
        self.wire_arc();
        self.wire_folder();
        self.wire_about();
    }

    fn on_event(&self, event: Event) {
        match event {
            // The user may just have granted access in Settings.
            Event::Resumed => self.refresh_permission(),
            Event::ConfigChanged => {
                self.apply_look();
                self.render_rows();
            }
            Event::Paused | Event::LowMemory => {}
        }
    }

    /// `MainActivity.refreshPermission()`: show or hide the permission view, and load the first
    /// folder once there is both access and an engine.
    fn refresh_permission(&self) {
        let ok = self.host.has_all_files_access();
        self.ui.set_permission_missing(!ok);
        self.browser.borrow_mut().storage_ok = ok;
        let first = {
            let mut status = self.status.borrow_mut();
            let first = ok && status.engine_up && !status.loaded_once;
            if first {
                status.loaded_once = true;
            }
            first
        };
        if first {
            let dir = self.browser.borrow().cwd.clone();
            self.load_dir(dir, false);
        } else {
            self.render_chrome();
        }
    }

    fn engine_ready(&self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.status.borrow_mut().engine_up = true;
                self.refresh_permission();
            }
            Err(e) => {
                self.status.borrow_mut().engine_error = Some(e);
                self.render_chrome();
            }
        }
    }
}
