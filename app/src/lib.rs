//! The Kiln Android app: the Slint window (`kiln-ui`) wired to the engine (`kiln-bridge`) and to
//! the phone (a [`Host`], implemented by the `platform` crate).
//!
//! This crate knows nothing about Android. [`run`] is the whole app: it builds the window, applies
//! the system's look, starts the engine on a worker thread, and runs Slint's event loop until the
//! app closes. Everything it touches belongs to the UI thread, except the engine start-up, whose
//! result is posted back with `slint::invoke_from_event_loop`.

#![forbid(unsafe_code)]

pub mod crash;
pub mod engine;
pub mod host;
pub mod prefs;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kiln_ui::{MainWindow, Theme};
use slint::ComponentHandle;

pub use crate::host::{Event, Host, Look};
use crate::prefs::Prefs;

/// `Defaults.ANIM_SCALE`: Off, Fast, Normal, Relaxed. Settings > Animation speed picks one.
const ANIM_SCALE: [f32; 4] = [0.0, 0.6, 0.85, 1.2];

/// What the UI is waiting for before it can show the first folder.
#[derive(Default)]
struct Status {
    engine_up: bool,
    loaded_once: bool,
}

struct App {
    ui: MainWindow,
    host: Rc<dyn Host>,
    prefs: Prefs,
    status: RefCell<Status>,
}

thread_local! {
    /// The running app, for code that arrives on the UI thread without a handle to it (the engine
    /// start-up's result).
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
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
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

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

/// Where the Java app kept its settings, which this app reads too (`SharedPreferences` "kiln").
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
            let app = APP.with(|a| a.borrow().clone());
            if let Some(app) = app {
                app.engine_ready(result);
            }
        });
    });
}

impl App {
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
    }

    fn on_event(&self, event: Event) {
        match event {
            // The user may just have granted access in Settings.
            Event::Resumed => self.refresh_permission(),
            Event::ConfigChanged => self.apply_look(),
            Event::Paused | Event::LowMemory => {}
        }
    }

    /// `MainActivity.refreshPermission()`: show or hide the permission view, and load the first
    /// folder once there is both access and an engine.
    fn refresh_permission(&self) {
        let ok = self.host.has_all_files_access();
        self.ui.set_permission_missing(!ok);
        self.ui.set_marks_ok(ok);
        let mut status = self.status.borrow_mut();
        if ok && status.engine_up && !status.loaded_once {
            status.loaded_once = true;
            // Phase B loads the start folder here. Until the file list exists the body is empty.
            self.ui.set_empty_text("".into());
        }
    }

    fn engine_ready(&self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.status.borrow_mut().engine_up = true;
                self.refresh_permission();
            }
            Err(e) => self
                .ui
                .set_empty_text(format!("Kiln couldn't start: {e}").into()),
        }
    }
}
