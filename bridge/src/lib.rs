//! `kiln-bridge`: the Kiln engine's functions, called directly by the Android app.
//!
//! One call is `call("scan", &json!([path, ...]), jobs)` and returns the same JSON object the old
//! child process wrote as its `{"result": ...}` line, so the code that reads these results did not
//! have to change. A call that should report progress or be cancellable goes through
//! [`call_job`] with a [`Job`]. Calls block, so the app makes them on a worker thread.
//!
//! No `unsafe` code lives here. Packing, extracting and testing call the `kiln` engine directly;
//! the only place archive bytes pass through this layer is `lock`, which encrypts and decrypts
//! whole files and never looks inside them.

#![forbid(unsafe_code)]

mod arc;
mod args;
mod browse;
mod cache;
mod edit;
mod fsops;
mod io;
mod lock;
mod packing;
mod rx;
mod sets;
mod text;
mod tool;

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};

use crate::args::Args;

pub use crate::io::{Job, Sink};

fn fail(msg: &str) -> Value {
    json!({ "ok": false, "error": msg, "message": msg })
}

fn version() -> Value {
    json!({
        "kiln": env!("CARGO_PKG_VERSION"),
        "helper": "",
        "engine": "rust",
        "engine_note": "Kiln engine (.kiln format)",
    })
}

/// Run one function. `jobs` is the thread count for the archive engine (0 = every core).
fn dispatch(name: &str, a: &Args, jobs: usize) -> Value {
    match name {
        "version" => version(),
        "scan_params" => browse::scan_params(),
        "scan" => browse::scan(&a.str(0), a.bool(1), a.bool(2)),
        "search" => browse::search(&a.str(0), &a.str(1), a.bool(2), a.bool(3)),
        "trim_cache" => {
            let left = cache::trim(&a.str(0), None, "");
            json!({ "ok": true, "bytes": left })
        }
        "cache_size" => cache::size(&a.str(0)),
        "clear_cache" => cache::clear(&a.str(0)),
        "fs_plan" => fsops::plan(&a.list(0), &a.str(1), a.bool(2)),
        "fs_transfer" => fsops::transfer(&a.list(0), &a.str(1), a.bool(2), &a.str(3)),
        "fs_delete" => fsops::delete(&a.list(0)),
        // Archives: (path, password, ...). The Progress argument never reaches us.
        "open_archive" => arc::open_archive(&a.str(0), &a.str(1)),
        "extract" => arc::extract(&a.str(0), &a.str(1), &a.str(2), &a.str(3), a.bool(4), jobs),
        "test" => arc::test(&a.str(0), &a.str(1), jobs),
        "preview" => arc::preview(
            &a.str(0),
            &a.str(1),
            &a.str(2),
            &a.str(3),
            a.i64(4).max(0) as u64,
            jobs,
        ),
        "cached_extract" => arc::cached_extract(
            &a.str(0),
            &a.str(1),
            &a.str(2),
            &a.str(3),
            a.i64(4).max(0) as u64,
            jobs,
        ),
        "decrypt_in_place" => arc::decrypt_in_place(&a.str(0), &a.str(1), jobs),
        // pack(items_json, dest, encrypt, each, store, overwrite, password, delete_src)
        "pack" => packing::pack(&packing::Request {
            items: a.list(0),
            dest: a.str(1),
            encrypt: a.bool(2),
            each: a.bool(3),
            store: a.bool(4),
            overwrite: a.bool(5),
            password: a.str(6),
            delete_src: a.bool(7),
            jobs,
        }),
        // Editing sessions (session state lives on disk between calls). The Progress argument
        // never reaches us, so file_for is (sid, member, cache_dir, readonly) and save is (sid).
        "begin" => edit::begin(&a.str(0), &a.str(1)),
        "state" => edit::state(&a.str(0)),
        "close" => edit::close(&a.str(0)),
        "discard" => edit::discard(&a.str(0)),
        "add" => edit::add(&a.str(0), &a.list(1), &a.str(2)),
        "remove" => edit::remove(&a.str(0), &arc::parse_members(&a.str(1))),
        "transfer" => edit::transfer(
            &a.str(0),
            &arc::parse_members(&a.str(1)),
            &a.str(2),
            a.bool(3),
            &a.str(4),
            &a.str(5),
            jobs,
        ),
        "set_file" => edit::set_file(&a.str(0), &a.str(1), &a.str(2)),
        "new_text" => edit::new_text(&a.str(0), &a.str(1)),
        "file_for" => edit::file_for(&a.str(0), &a.str(1), &a.str(2), a.bool(3), jobs),
        "save" => edit::save(&a.str(0), jobs),
        // tool(name, arg): "selftest" or "bench"
        "tool" => tool::tool(&a.str(0), &a.str(1), jobs),
        other => fail(&format!("unknown engine function: {other}")),
    }
}

/// Call engine function `name` with the JSON array `args`. `jobs` is the thread count for the
/// archive engine (0 = every core). Never panics: a failure inside the engine comes back as
/// `{"ok": false, "error": ...}`.
pub fn call(name: &str, args: &Value, jobs: usize) -> Value {
    call_job(name, args, jobs, &Job::new())
}

/// Like [`call`], for an operation that reports progress to and can be cancelled through `job`.
/// A cancelled operation returns the same `Cancelled` result the child process used to.
pub fn call_job(name: &str, args: &Value, jobs: usize, job: &Job) -> Value {
    let a = Args::new(Some(args));
    let _cancel = kiln::cancel::enter(job.token());
    let _progress = io::set_sink(job.sink());
    match catch_unwind(AssertUnwindSafe(|| dispatch(name, &a, jobs))) {
        Ok(v) => v,
        Err(_) => fail("the engine stopped unexpectedly"),
    }
}
