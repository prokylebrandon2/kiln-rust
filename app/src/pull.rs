//! The pull-down info strip and pull-to-refresh. The strip's look and the way the list is pulled down are in
//! `ui/ui/browser.slint` and `ui/ui/pull.slint`; this is what fills the strip and what a pull does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::Timer;

use crate::{App, app_do, fmt};

/// Shows how far it has counted at most this often.
const POST_EVERY: Duration = Duration::from_millis(120);
/// Folders nested deeper than this are not counted.
const MAX_DEPTH: u32 = 40;
/// The strip goes on counting while it slides back, which takes at most 250 + 220 ms.
const LINGER: Duration = Duration::from_millis(500);
/// The count rests for [`BREATH`] after this many entries, so that walking a big folder does not take
/// the CPU and the storage away from the UI thread while the list is being pulled (that was the
/// pull being smooth in a small folder and stuttering in a big one).
const BREATH_EVERY: u32 = 64;
/// How long the count rests each time.
const BREATH: Duration = Duration::from_millis(1);
/// The size counted for a storage root is shown again for this long, so that a pull does not walk
/// all of the phone's storage each time. Other folders are counted afresh on every pull.
const KEEP_ROOT_SIZE: Duration = Duration::from_secs(600);

/// A running count of a folder's size.
struct Counter {
    total: u64,
    stop: Arc<AtomicBool>,
    posted: Instant,
    /// Entries looked at since the count last rested.
    since_rest: u32,
}

impl Counter {
    /// Add up the sizes of the files under `dir`.
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
            self.since_rest += 1;
            if self.since_rest >= BREATH_EVERY {
                self.since_rest = 0;
                std::thread::sleep(BREATH);
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

/// Keep the size counted for a storage root, to show again for [`KEEP_ROOT_SIZE`].
fn keep_root_size(path: PathBuf, text: String) {
    let _ = slint::invoke_from_event_loop(move || {
        app_do(|app| {
            *app.pull_root_size.borrow_mut() = Some((path, Instant::now(), text));
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

    /// The list has been pulled down from the top, so write the strip for this folder.
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
        if home {
            let kept = self
                .pull_root_size
                .borrow()
                .as_ref()
                .filter(|(path, when, _)| *path == at && when.elapsed() < KEEP_ROOT_SIZE)
                .map(|(_, _, text)| text.clone());
            if let Some(text) = kept {
                ui.set_pull_size(text.into());
                return;
            }
        }
        ui.set_pull_size("Size: ...".into());
        // The folder's size climbs while it is counted.
        std::thread::spawn(move || {
            let mut counter = Counter {
                total: 0,
                stop: stop.clone(),
                posted: Instant::now(),
                since_rest: 0,
            };
            counter.walk(&at, 0);
            if !stop.load(Ordering::Relaxed) {
                let text = format!("Size: {}", size_text(counter.total));
                show_size(&stop, text.clone());
                if home {
                    keep_root_size(at, text);
                }
            }
        });
    }

    /// The finger let go. The strip slides back; the count stops once it is gone.
    fn pull_ended(&self) {
        let stop = self.pull_stop.borrow().clone();
        Timer::single_shot(LINGER, move || stop.store(true, Ordering::Relaxed));
    }

    /// A long pull reads the folder again, keeping the selection.
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
