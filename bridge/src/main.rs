//! `kiln-bridge`: one process per user operation, speaking JSON lines to the Java app.
//!
//! Request (stdin, first line): `{"fn": "scan", "args": [...], "jobs": 0}`.
//! Replies (stdout): `{"progress": {...}}` lines, then one `{"result": {...}}`.
//! Writing `cancel` to stdin (or closing it) makes the running job stop cleanly.
//!
//! No `unsafe` code lives here. Packing, extracting and testing call the `kiln` engine directly;
//! the only place archive bytes pass through this layer is `lock`, which encrypts and decrypts
//! whole files and never looks inside them.

#![forbid(unsafe_code)]

mod arc;
mod args;
mod browse;
mod cache;
mod fsops;
mod io;
mod lock;
mod packing;
mod rx;
mod sets;
mod text;

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};

use crate::args::Args;

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
        "preview" => arc::preview(&a.str(0), &a.str(1), &a.str(2), &a.str(3), a.i64(4).max(0) as u64, jobs),
        "cached_extract" => {
            arc::cached_extract(&a.str(0), &a.str(1), &a.str(2), &a.str(3), a.i64(4).max(0) as u64, jobs)
        }
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
        other => fail(&format!("unknown engine function: {other}")),
    }
}

fn main() {
    let req = match io::read_request() {
        Ok(v) => v,
        Err(e) => {
            io::result(fail(&e));
            return;
        }
    };
    io::watch_cancel();
    let name = match req.get("fn").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => String::new(),
    };
    let jobs = match req.get("jobs").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => 0,
    };
    let a = Args::new(req.get("args"));
    let out = match catch_unwind(AssertUnwindSafe(|| dispatch(&name, &a, jobs))) {
        Ok(v) => v,
        Err(_) => fail("the engine stopped unexpectedly"),
    };
    io::result(out);
}
