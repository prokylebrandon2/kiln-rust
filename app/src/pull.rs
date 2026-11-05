//! The pull-down info strip and pull-to-refresh (`MainActivity.fillPull`, `walkSize`, `pullEnd`
//! and the `onPull` refresh). The strip's look and the way the list is pulled down are in
//! `ui/ui/browser.slint` and `ui/ui/pull.slint`; this is what fills the strip and what a pull does.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::Timer;

use crate::{App, app_do, fmt};

/// `walkSize` shows how far it has counted at most this often.
const POST_EVERY: Duration = Duration::from_millis(120);
/// Folders nested deeper than this are not counted (`walkSize`).
const MAX_DEPTH: u32 = 40;
/// The strip goes on counting while it slides back, which takes at most 250 + 220 ms.
const LINGER: Duration = Duration::from_millis(500);

/// A running count of a folder's size.
struct Counter {
    total: u64,
    stop: Arc<AtomicBool>,
    posted: Instant,
}

impl Counter {
    /// `walkSize`: add up the sizes of the files under `dir`.
    fn walk(&mut self, dir: &Path, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        let kids = match std::fs::read_dir(dir) {
            Ok(kids) => kids,
            Err(_) => return,
        };
        for kid in kids.flatten() {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let path = kid.path();
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => self.walk(&path, depth + 1),
                Ok(meta) => self.total += meta.len(),
                Err(_) => {}
            }
            if self.posted.elapsed() >= POST_EVERY {
                self.posted = Instant::now();
                let text = format!("Size: {}...", size_text(self.total));
                show_size(&self.stop, text);
            }
        }
    }
}

fn size_text(bytes: u64) -> String {
    fmt::size(i64::try_from(bytes).unwrap_or(i64::MAX))
}

/// Put `text` in the strip's size line, unless that count has been stopped.
fn show_size(stop: &Arc<AtomicBool>, text: String) {
    let stop = stop.clone();
    let _ = slint::invoke_from_event_loop(move || {
        app_do(|app| {
            if !stop.load(Ordering::Relaxed) {
                app.ui.set_pull_size(text.into());
            }
        });
    });
}

impl App {
    /// Connect the pull's callbacks.
    pub(crate) fn wire_pull(&self) {
        let ui = &self.ui;
        ui.on_pull_started(|| app_do(|app| app.pull_started()));
        ui.on_pull_ended(|| app_do(|app| app.pull_ended()));
        ui.on_refresh_requested(|| app_do(|app| app.refresh_requested()));
    }

    /// `fillPull`: the list has been pulled down from the top, so write the strip for this folder.
    fn pull_started(&self) {
        let stop = Arc::new(AtomicBool::new(false));
        let old = self.pull_stop.replace(stop.clone());
        old.store(true, Ordering::Relaxed);
        let (at, home, name) = {
            let b = self.browser.borrow();
            let home = b.is_storage_root(&b.cwd);
            let name = if home {
                format!("Storage: {}", b.storage_name(&b.cwd))
            } else {
                let folder = b.cwd.file_name().unwrap_or_default();
                format!("Folder: {}", folder.to_string_lossy())
            };
            (b.cwd.clone(), home, name)
        };
        let space = self.host.space(&at);
        let ui = &self.ui;
        let free = match space {
            Some((free, total)) => format!("Free: {} / {}", size_text(free), size_text(total)),
            None => String::new(),
        };
        let used = match space {
            Some((free, total)) if total > 0 => {
                let used = total.saturating_sub(free) as f64 / total as f64;
                used.max(0.008) as f32
            }
            _ => 0.0,
        };
        let kind = if home {
            "Type: Storage"
        } else {
            "Type: Folder"
        };
        ui.set_pull_type(kind.into());
        ui.set_pull_free(free.into());
        ui.set_pull_used(used);
        ui.set_pull_name(name.into());
        ui.set_pull_size("Size: ...".into());
        // The folder's size climbs while it is counted.
        std::thread::spawn(move || {
            let mut counter = Counter {
                total: 0,
                stop: stop.clone(),
                posted: Instant::now(),
            };
            counter.walk(&at, 0);
            if !stop.load(Ordering::Relaxed) {
                show_size(&stop, format!("Size: {}", size_text(counter.total)));
            }
        });
    }

    /// `pullEnd`: the finger let go. The strip slides back; the count stops once it is gone.
    fn pull_ended(&self) {
        let stop = self.pull_stop.borrow().clone();
        Timer::single_shot(LINGER, move || stop.store(true, Ordering::Relaxed));
    }

    /// `onPull`: a long pull reads the folder again, keeping the selection.
    fn refresh_requested(&self) {
        let ready = self.status.borrow().engine_up && self.browser.borrow().storage_ok;
        if !ready {
            self.ui.set_refreshing(false);
            return;
        }
        let dir = self.browser.borrow().cwd.clone();
        self.load_dir(dir, true);
    }
}
