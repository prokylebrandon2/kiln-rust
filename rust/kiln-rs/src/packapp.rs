//! The `pack` op of the C ABI: JSON in / JSON out, like kiln_app.pack. One archive, or (`each`) one
//! archive per file, which packeach.rs does.
//!
//! args: {items:[paths], dest, encrypt:bool, store:bool, overwrite:bool, password, each:bool, jobs}
//! ok:   {ok:true, message, done, packed:[paths], warnings:[...]}

use crate::app::Progress;
use crate::packeach::{self, EachArgs};
use crate::packer::{self, PackFile, PackOpts, CANCELLED};
use crate::walk::{self, Item, MP3_MAX, MP3_MIN};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

pub(crate) const MIN_PASSWORD: usize = 8;

pub(crate) fn fmt_size(n: u64) -> String {
    let mut x = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if x < 1024.0 || unit == "GB" {
            return if unit == "B" { format!("{:.0} {}", x, unit) } else { format!("{:.1} {}", x, unit) };
        }
        x /= 1024.0;
    }
    unreachable!()
}

pub(crate) fn fail(msg: &str) -> String {
    json!({"ok": false, "message": msg}).to_string()
}


/// The forward filter python's pack_file would choose for one file (0 = none).
pub(crate) fn filter_for(f: &Item, store: bool) -> u16 {
    if store {
        0
    } else if f.ext == ".mp3" && (MP3_MIN..=MP3_MAX).contains(&f.size) {
        120 // python try_mp3 runs before the `pre` rule
    } else if f.pre {
        0
    } else {
        let w = if f.ext == ".wav" { crate::wav::pick_wav_filter(Path::new(&f.full), f.size) } else { 0 };
        if w != 0 {
            w
        } else {
            walk::pick_bcj(&f.full, f.size)
        }
    }
}

/// (segment size, worker threads) for an archive of `total` input bytes; `jobs_arg` 0 = automatic.
pub(crate) fn plan(total: u64, store: bool, jobs_arg: usize) -> (usize, usize) {
    let cores = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).min(8);
    let segment = if store { packer::fit_segment(total, 1) } else { packer::fit_segment(total, if jobs_arg > 0 { jobs_arg } else { cores }) };
    let jobs = if store { 1 } else if jobs_arg > 0 { jobs_arg } else { packer::auto_jobs(total, segment) };
    (segment, jobs)
}

pub fn pack(a: &Value, prog: &mut dyn Progress) -> String {
    let items: Vec<String> = a
        .get("items")
        .and_then(|v| v.as_array())
        .map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let dest = a.get("dest").and_then(|v| v.as_str()).unwrap_or("");
    let encrypt = a.get("encrypt").and_then(|v| v.as_bool()).unwrap_or(false);
    let store = a.get("store").and_then(|v| v.as_bool()).unwrap_or(false) && encrypt;
    let overwrite = a.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false);
    let password = a.get("password").and_then(|v| v.as_str()).unwrap_or("");
    if dest.is_empty() || items.is_empty() {
        return fail("there is nothing to pack");
    }
    let jobs_arg = a.get("jobs").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    if a.get("each").and_then(|v| v.as_bool()).unwrap_or(false) {
        // `dest` is the output folder; the password rules differ (an existing set keeps its old password)
        return packeach::pack_each(&EachArgs { items: &items, out_dir: dest, encrypt, store, force: overwrite, password, jobs_arg }, prog);
    }
    if Path::new(dest).exists() && !overwrite {
        return fail(&format!("{dest} already exists (use -f to overwrite)"));
    }
    if encrypt {
        if password.is_empty() {
            return fail("enter a password");
        }
        if password.chars().count() < MIN_PASSWORD {
            return fail(&format!("password must be at least {MIN_PASSWORD} characters (a long passphrase is best)"));
        }
    }
    let tmp = format!("{dest}.tmp");
    let exclude = [walk::abspath(dest), walk::abspath(&tmp)];
    let (found, warnings) = match walk::collect(&items, &exclude) {
        Ok(x) => x,
        Err(e) => return fail(&e.0),
    };
    if found.is_empty() {
        return fail("nothing to pack");
    }
    let files: Vec<PackFile> = found
        .iter()
        .map(|f| PackFile {
            full: f.full.clone(),
            rel: f.rel.clone(),
            size: f.size,
            mtime: f.mtime,
            mode: f.mode,
            filt: filter_for(f, store),
            pre: f.pre,
        })
        .collect();
    let total: u64 = files.iter().map(|f| f.size).sum();
    let nfiles = files.len();
    let t0 = Instant::now();
    let mut done_bytes = 0u64;
    packer::CUR_FILE.store(0, std::sync::atomic::Ordering::Relaxed);
    let mut last = Instant::now();
    let mut stopped = false;
    let mut jobs_used = 1usize;
    let result = {
        let mut cb = |n: u64| -> bool {
            done_bytes += n;
            if last.elapsed().as_millis() >= 50 {
                last = Instant::now();
                let frac = if total > 0 { (done_bytes as f64 / total as f64).min(1.0) } else { -1.0 };
                let cur = packer::CUR_FILE.load(std::sync::atomic::Ordering::Relaxed).min(nfiles - 1);
                prog.event(&json!({"frac": frac, "done": cur, "total": nfiles, "file": files[cur].rel}).to_string());
            }
            if prog.cancelled() {
                stopped = true;
            }
            stopped
        };
        let (segment, jobs) = plan(total, store, jobs_arg);
        jobs_used = jobs;
        let opts = PackOpts { store, password: if encrypt { Some(password) } else { None }, segment, jobs };
        packer::pack_archive(dest, &files, &opts, Some(&mut cb as &mut dyn FnMut(u64) -> bool))
    };
    match result {
        Ok(table) => {
            let raw: u64 = table.entries.iter().map(|e| e.size).sum();
            let asize = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            let ratio = if raw > 0 { 100.0 * asize as f64 / raw as f64 } else { 100.0 };
            let core_note = if jobs_used > 1 { format!(" [{jobs_used} threads]") } else { String::new() };
            let msg = format!(
                "{} file(s): {} -> {} ({:.1}% of original) in {:.1}s{}{}",
                table.entries.len(),
                fmt_size(raw),
                fmt_size(asize),
                ratio,
                t0.elapsed().as_secs_f64(),
                if encrypt { " [encrypted: AES-256-GCM]" } else { "" },
                if store { " [no compression]".to_string() } else { core_note }
            );
            prog.event(&json!({"frac": 1.0, "done": nfiles, "total": nfiles, "file": ""}).to_string());
            json!({"ok": true, "message": msg, "done": nfiles,
                   "packed": files.iter().map(|f| f.full.clone()).collect::<Vec<_>>(), "warnings": warnings})
            .to_string()
        }
        Err(e) if e.0 == CANCELLED => json!({"ok": false, "cancelled": true, "message": "Cancelled"}).to_string(),
        Err(e) => json!({"ok": false, "message": e.0, "bad_password": false}).to_string(),
    }
}
