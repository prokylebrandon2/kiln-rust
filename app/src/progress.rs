//! The progress sheet's logic: run a file operation on a worker thread with a [`Job`], show the
//! sheet if the work lasts, let Cancel stop it, and hand the result back on the UI thread.
//!
//! One operation runs at a time; a second is refused ([`App::run_job`] returns false). The sheet
//! opens only after [`SHOW_AFTER_MS`], so an operation that ends sooner never flashes it. Progress
//! events come from the worker thread (the engine sends about ten a second) and are posted to the
//! UI thread with `slint::invoke_from_event_loop`; an event or a result that carries the number
//! of an operation that has ended is dropped.
//!
//! Cancel does not close the sheet. It asks the job to stop, turns the file line into
//! "Cancelling..." and greys Cancel out; the sheet goes when the work returns, and the result
//! then says "Cancelled" as the engine words it.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use serde_json::{Value, json};

use kiln_bridge::Job;
use kiln_ui_next::ProgressState;
use slint::{ComponentHandle, SharedString, Timer};

use crate::{App, app_do};

/// An operation that ends within this long never shows its sheet.
const SHOW_AFTER_MS: u64 = 250;

/// What an operation does when it ends, with the engine's result.
type OnDone = Box<dyn FnOnce(&App, &Value)>;

/// The operation that is running.
struct Running {
    id: u64,
    job: Job,
    /// Cancel was tapped: the file line says so and no longer follows the work.
    cancelling: bool,
    done: OnDone,
}

/// The one running operation (if any) and the count of operations started.
#[derive(Default)]
pub(crate) struct Ops {
    running: RefCell<Option<Running>>,
    serial: Cell<u64>,
}

/// What a progress event says, ready to show.
#[derive(Debug, PartialEq)]
struct Report {
    /// The file being worked on ("" when the event names none).
    file: String,
    /// How much is done (0..1), or `None` while the amount is not known.
    frac: Option<f32>,
}

/// Read `{"frac": 0..1 or -1, "file": name}`.
fn report(event: &Value) -> Report {
    let file = event["file"].as_str().unwrap_or_default().to_string();
    let frac = event["frac"]
        .as_f64()
        .filter(|f| *f >= 0.0)
        .map(|f| f.min(1.0) as f32);
    Report { file, frac }
}

/// "42%": the percent rounded down, so that 100% means done.
fn percent_text(frac: f32) -> String {
    let percent = (frac.clamp(0.0, 1.0) * 100.0).floor() as u32;
    format!("{percent}%")
}

/// What the engine says when the work itself fell over.
fn crashed() -> Value {
    json!({ "ok": false, "message": "The operation stopped unexpectedly." })
}

impl App {
    /// Connect the sheet's Cancel.
    pub(crate) fn wire_progress(&self) {
        let state = self.ui.global::<ProgressState>();
        state.on_cancel_clicked(|| app_do(|a| a.cancel_job()));
    }

    /// An operation is running (its sheet may not be up yet). Back does nothing meanwhile.
    pub(crate) fn job_running(&self) -> bool {
        self.ops.running.borrow().is_some()
    }

    /// Run `work` on a worker thread with a [`Job`] that reports to the sheet and is cancelled
    /// from it; the sheet shows only if the work lasts. `done` then runs on the UI thread with
    /// the result. Returns false, and does nothing, while another operation runs.
    pub(crate) fn run_job(
        &self,
        title: &str,
        work: impl FnOnce(&Job) -> Value + Send + 'static,
        done: impl FnOnce(&App, &Value) + 'static,
    ) -> bool {
        if self.job_running() {
            return false;
        }
        let id = self.ops.serial.get() + 1;
        self.ops.serial.set(id);

        let state = self.ui.global::<ProgressState>();
        state.set_open(false);
        state.set_title(title.into());
        state.set_line(SharedString::new());
        state.set_percent(SharedString::new());
        state.set_frac(0.0);
        state.set_indeterminate(true);
        state.set_cancel_enabled(true);

        let job = Job::with_progress(move |event| {
            let event = event.clone();
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|a| a.job_progress(id, &event));
            });
        });
        *self.ops.running.borrow_mut() = Some(Running {
            id,
            job: job.clone(),
            cancelling: false,
            done: Box::new(done),
        });

        Timer::single_shot(Duration::from_millis(SHOW_AFTER_MS), move || {
            app_do(|a| a.job_show(id));
        });
        std::thread::spawn(move || {
            let result =
                catch_unwind(AssertUnwindSafe(|| work(&job))).unwrap_or_else(|_| crashed());
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|a| a.job_done(id, &result));
            });
        });
        true
    }

    /// The 250 ms are over: open the sheet if that operation is still running.
    fn job_show(&self, id: u64) {
        if self.is_running(id) {
            self.ui.global::<ProgressState>().set_open(true);
        }
    }

    /// A progress event: the file line, the percent and the bar.
    fn job_progress(&self, id: u64, event: &Value) {
        let cancelling = match self.ops.running.borrow().as_ref() {
            Some(op) if op.id == id => op.cancelling,
            _ => return,
        };
        let state = self.ui.global::<ProgressState>();
        let report = report(event);
        if !cancelling && !report.file.is_empty() {
            state.set_line(report.file.into());
        }
        match report.frac {
            Some(frac) => {
                state.set_indeterminate(false);
                state.set_frac(frac);
                state.set_percent(percent_text(frac).into());
            }
            None => {
                state.set_indeterminate(true);
                state.set_percent(SharedString::new());
            }
        }
    }

    /// The sheet's Cancel: stop the job, say so, and keep the sheet up until the job ends.
    fn cancel_job(&self) {
        let mut slot = self.ops.running.borrow_mut();
        let Some(op) = slot.as_mut() else {
            return;
        };
        if op.cancelling {
            return;
        }
        op.job.cancel();
        op.cancelling = true;
        let state = self.ui.global::<ProgressState>();
        state.set_line("Cancelling...".into());
        state.set_cancel_enabled(false);
    }

    /// The work ended: close the sheet, then hand the result on.
    fn job_done(&self, id: u64, result: &Value) {
        if !self.is_running(id) {
            return;
        }
        let Some(running) = self.ops.running.borrow_mut().take() else {
            return;
        };
        self.ui.global::<ProgressState>().set_open(false);
        (running.done)(self, result);
    }

    fn is_running(&self, id: u64) -> bool {
        self.ops
            .running
            .borrow()
            .as_ref()
            .is_some_and(|op| op.id == id)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A scratch folder under the system temp dir, removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("kiln-progress-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn files(&self, sub: &str, n: usize) -> PathBuf {
            let dir = self.0.join(sub);
            fs::create_dir_all(&dir).unwrap();
            for i in 0..n {
                fs::write(dir.join(format!("f{i:03}.txt")), b"data").unwrap();
            }
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn count(dir: &Path) -> usize {
        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    #[test]
    fn a_known_amount_becomes_a_fraction_and_a_file() {
        let r = report(&json!({ "frac": 0.25, "done": 1, "total": 4, "file": "a.txt" }));
        assert_eq!(
            r,
            Report {
                file: "a.txt".into(),
                frac: Some(0.25)
            }
        );
    }

    #[test]
    fn an_unknown_amount_has_no_fraction() {
        let r = report(&json!({ "frac": -1.0, "file": "a.txt" }));
        assert_eq!(r.frac, None);
        assert_eq!(
            report(&json!({})),
            Report {
                file: String::new(),
                frac: None
            }
        );
    }

    #[test]
    fn a_fraction_past_one_is_held_at_one() {
        assert_eq!(report(&json!({ "frac": 1.3 })).frac, Some(1.0));
    }

    #[test]
    fn the_percent_rounds_down() {
        assert_eq!(percent_text(0.0), "0%");
        assert_eq!(percent_text(0.419), "41%");
        assert_eq!(percent_text(0.999), "99%");
        assert_eq!(percent_text(1.0), "100%");
        assert_eq!(percent_text(-0.5), "0%");
        assert_eq!(percent_text(7.0), "100%");
    }

    #[test]
    fn a_delete_reports_its_progress_through_the_job() {
        let tmp = Scratch::new("progress");
        let dir = tmp.files("many", 20);
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        let job = Job::with_progress(move |event| {
            assert!(event["frac"].is_number());
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let paths = json!([[dir.to_string_lossy()]]);
        let result = kiln_bridge::call_job("fs_delete", &paths, 0, &job);
        assert_eq!(result["ok"], json!(true), "{result}");
        assert!(
            seen.load(Ordering::Relaxed) >= 1,
            "no progress event arrived"
        );
        assert!(!dir.exists());
    }

    #[test]
    fn cancelling_from_the_progress_callback_stops_a_delete() {
        let tmp = Scratch::new("cancel-delete");
        let dir = tmp.files("many", 50);
        // the sink gets the job only after it is made, so it finds it through a shared slot
        let slot: Arc<std::sync::Mutex<Option<Job>>> = Arc::default();
        let from_sink = slot.clone();
        let job = Job::with_progress(move |_| {
            if let Some(job) = from_sink.lock().unwrap().as_ref() {
                job.cancel();
            }
        });
        *slot.lock().unwrap() = Some(job.clone());
        let paths = json!([[dir.to_string_lossy()]]);
        let result = kiln_bridge::call_job("fs_delete", &paths, 0, &job);
        assert_eq!(result["cancelled"], json!(true), "{result}");
        assert_eq!(result["ok"], json!(false));
        let left = count(&dir);
        assert!(left > 0, "everything was deleted anyway");
        assert!(left < 50, "nothing was deleted before the cancel");
    }

    #[test]
    fn cancelling_a_copy_leaves_nothing_behind() {
        let tmp = Scratch::new("cancel-copy");
        let src = tmp.files("src", 30);
        let dest = tmp.0.join("dest");
        fs::create_dir_all(&dest).unwrap();
        let slot: Arc<std::sync::Mutex<Option<Job>>> = Arc::default();
        let from_sink = slot.clone();
        let job = Job::with_progress(move |_| {
            if let Some(job) = from_sink.lock().unwrap().as_ref() {
                job.cancel();
            }
        });
        *slot.lock().unwrap() = Some(job.clone());
        let args = json!([
            [src.to_string_lossy()],
            dest.to_string_lossy(),
            false,
            "replace"
        ]);
        let result = kiln_bridge::call_job("fs_transfer", &args, 0, &job);
        assert_eq!(result["cancelled"], json!(true), "{result}");
        assert_eq!(count(&dest), 0, "a cancelled copy must be undone");
        assert_eq!(count(&src), 30, "the original must be untouched");
    }
}
