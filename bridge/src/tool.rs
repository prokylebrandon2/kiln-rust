//! `tool(name, arg)`: the benchmark and the self-test that Settings offers.
//!
//! Both answer `{ok, text, view}` (or `{ok:false, cancelled:true}`): `text` is a plain report and
//! `view` is the same thing as data for the result sheets in `SettingsActivity`
//! (`showBench`, `showSelfTest`). Progress events carry `{frac, done, total, file}`.
//!
//! * `bench`: packs and verifies the chosen file or folder with a few codecs and reports size,
//!   ratio and time for each (the engine's own `kiln::bench`).
//! * `selftest`: runs the real pack, extract, lock, set and edit code on small generated data in a
//!   scratch folder, one check at a time; a failing check never stops the rest.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use kiln::bench::{BenchConfig, BenchRow};
use kiln::codec::{CodecSpec, Method};
use kiln::format::DEFAULT_BLOCK_LOG2;
use kiln::format::index::EntryKind;
use kiln::walk;
use kiln::{Archive, ExtractOptions, PackOptions, Profile};
use serde_json::{Value, json};

use crate::io::Meter;
use crate::lock::{self, Scratch};
use crate::text::{describe, fmt_size};
use crate::{arc, edit, packing};

fn fail(msg: &str) -> Value {
    json!({ "ok": false, "text": msg, "message": msg })
}

fn cancelled() -> Value {
    json!({ "ok": false, "cancelled": true, "text": "Cancelled", "message": "Cancelled" })
}

/// `tool(name, arg)`: `"selftest"` or `"bench"` (arg = the file or folder to measure).
pub fn tool(name: &str, arg: &str, jobs: usize) -> Value {
    match name {
        "selftest" => selftest(jobs),
        "bench" => bench(arg, jobs),
        other => fail(&format!("unknown tool: {other}")),
    }
}

// ---------------------------------------------------------------------------------- benchmark

fn bench(arg: &str, jobs: usize) -> Value {
    if arg.is_empty() {
        return fail("choose a file or folder to test");
    }
    let input = PathBuf::from(arg);
    if !input.exists() {
        return fail(&format!("{arg}: not found"));
    }
    let collected = match walk::collect(std::slice::from_ref(&input), &[]) {
        Ok(c) => c,
        Err(e) => return fail(&describe(&e)),
    };
    let files = collected
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .count();
    let raw: u64 = collected.entries.iter().map(|e| e.size).sum();
    if files == 0 {
        return fail("there are no files to test");
    }
    // The slow codecs are left out for big inputs, so the test ends in reasonable time.
    let mut codecs = vec![
        CodecSpec::new(Method::Lz4, 0),
        CodecSpec::new(Method::Zstd, 1),
        CodecSpec::new(Method::Zstd, 3),
    ];
    if raw <= 256 << 20 {
        codecs.push(CodecSpec::new(Method::Zstd, 9));
        codecs.push(CodecSpec::new(Method::Lzma2, 6));
    }
    let total = codecs.len() as u64;
    let config = BenchConfig {
        inputs: vec![input],
        codecs: codecs.clone(),
        block_log2s: vec![DEFAULT_BLOCK_LOG2],
        threads: jobs,
    };
    let mut meter = Meter::new();
    meter.tick(0, total, "Working...", true);
    let mut finished = 0u64;
    let outcome = kiln::bench::run(&config, |row: &BenchRow| {
        finished += 1;
        meter.tick(finished, total, &row.codec.to_string(), true);
    });
    let rows = match outcome {
        Ok(r) => r,
        Err(e) if kiln::cancel::is_cancelled(&e) => return cancelled(),
        Err(e) => return fail(&describe(&e)),
    };
    let input_text = fmt_size(raw);
    let mut lines = vec![format!("input: {files} file(s), {input_text}")];
    let mut view_rows: Vec<Value> = Vec::new();
    for r in &rows {
        let ratio = if r.raw_bytes > 0 {
            100.0 * r.archive_bytes as f64 / r.raw_bytes as f64
        } else {
            100.0
        };
        let name = r.codec.to_string();
        lines.push(format!(
            "{name}  {}  {ratio:.1}%  {:.1}s",
            r.archive_bytes, r.pack_secs
        ));
        view_rows.push(json!({
            "name": name,
            "size": r.archive_bytes,
            "ratio": ratio,
            "secs": r.pack_secs,
        }));
    }
    json!({
        "ok": true,
        "text": lines.join("\n"),
        "view": { "type": "bench", "files": files, "input": input_text, "rows": view_rows },
    })
}

// ---------------------------------------------------------------------------------- self-test

struct Ctx {
    root: PathBuf,
    jobs: usize,
}

impl Ctx {
    /// A fresh folder for one check.
    fn dir(&self, name: &str) -> Result<PathBuf, String> {
        let d = self.root.join(name);
        fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        Ok(d)
    }
}

type Check = fn(&Ctx) -> Result<(), String>;

const CHECKS: &[(&str, Check)] = &[
    ("pack and extract (zstd)", check_zstd),
    ("pack and extract (no compression)", check_store),
    ("pack and extract (lz4)", check_lz4),
    ("pack and extract (lzma)", check_lzma),
    ("many small chunks", check_chunks),
    ("empty file and empty folder", check_empty),
    ("unicode and long names", check_names),
    ("a changed byte is caught", check_damage),
    ("a non-archive is refused", check_garbage),
    ("list an archive", check_list),
    ("locked archive", check_locked),
    ("locked set", check_set),
    ("edit an archive", check_edit),
    ("edit a locked archive", check_edit_locked),
];

fn check_zstd(ctx: &Ctx) -> Result<(), String> {
    roundtrip(ctx, "zstd", |_| {})
}

fn check_store(ctx: &Ctx) -> Result<(), String> {
    roundtrip(ctx, "store", |o| {
        *o = PackOptions::from_profile(Profile::Store);
    })
}

fn check_lz4(ctx: &Ctx) -> Result<(), String> {
    roundtrip(ctx, "lz4", |o| {
        o.codec = CodecSpec::new(Method::Lz4, 0);
        o.profile = Profile::Custom;
    })
}

fn check_lzma(ctx: &Ctx) -> Result<(), String> {
    roundtrip(ctx, "lzma", |o| {
        o.codec = CodecSpec::new(Method::Lzma2, 3);
        o.profile = Profile::Custom;
    })
}

fn check_chunks(ctx: &Ctx) -> Result<(), String> {
    roundtrip(ctx, "chunks", |o| o.block_log2 = 12)
}

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// `len` bytes: random (does not compress) or word-like text (does).
fn data(seed: u64, len: usize, random: bool) -> Vec<u8> {
    let mut s = seed | 1;
    let mut out = Vec::with_capacity(len + 16);
    if random {
        while out.len() < len {
            out.extend_from_slice(&rng(&mut s).to_le_bytes());
        }
    } else {
        const WORDS: [&str; 8] = [
            "alpha", "beta", "gamma", "delta", "kiln", "archive", "chunk", "index",
        ];
        while out.len() < len {
            let w = WORDS[(rng(&mut s) % 8) as usize];
            out.extend_from_slice(w.as_bytes());
            out.push(b'0' + (rng(&mut s) % 7) as u8);
            out.push(b' ');
        }
    }
    out.truncate(len);
    out
}

fn put(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Make the sample folder `<dir>/src` and return its files as `(name, bytes)`, names starting
/// with `src/` (the way they are stored).
fn sample(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let files = vec![
        ("src/docs/readme.txt", data(1, 3_000, false)),
        ("src/docs/notes/big.txt", data(2, 300_000, false)),
        ("src/bin/random.bin", data(3, 200_000, true)),
        ("src/empty.txt", Vec::new()),
        ("src/top.dat", data(4, 70_000, false)),
    ];
    let mut out = Vec::new();
    for (rel, bytes) in files {
        put(&dir.join(rel), &bytes)?;
        out.push((rel.to_string(), bytes));
    }
    Ok(out)
}

fn pack_dir(root: &Path, out: &Path, opts: &PackOptions) -> Result<(), String> {
    kiln::pack(out, &[root.to_path_buf()], opts)
        .map(|_| ())
        .map_err(|e| format!("pack: {}", describe(&e)))
}

fn plain_options(ctx: &Ctx) -> PackOptions {
    let mut o = PackOptions::from_profile(Profile::Adaptive);
    o.threads = ctx.jobs;
    o.overwrite = true;
    o
}

fn extract_to(archive: &Path, dest: &Path, jobs: usize) -> Result<(), String> {
    let a = Archive::open(archive).map_err(|e| format!("open: {}", describe(&e)))?;
    let mut o = ExtractOptions::new(dest);
    o.threads = jobs;
    kiln::extract(&a, &o)
        .map(|_| ())
        .map_err(|e| format!("extract: {}", describe(&e)))
}

fn verify_all(archive: &Path, jobs: usize) -> Result<(), String> {
    let a = Archive::open(archive).map_err(|e| format!("open: {}", describe(&e)))?;
    kiln::verify_with_progress(&a, jobs, None, &mut |_, _| {})
        .map(|_| ())
        .map_err(|e| format!("verify: {}", describe(&e)))
}

fn same_files(out: &Path, expect: &[(String, Vec<u8>)]) -> Result<(), String> {
    for (rel, bytes) in expect {
        let got = fs::read(out.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        if &got != bytes {
            return Err(format!("{rel}: the contents differ"));
        }
    }
    Ok(())
}

/// The reply of a bridge function must say ok.
fn must_ok(v: &Value, what: &str) -> Result<(), String> {
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        return Ok(());
    }
    let m = ["message", "error"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .unwrap_or("failed");
    Err(format!("{what}: {m}"))
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()) == Some(true)
}

fn listed(v: &Value) -> Vec<String> {
    match v.get("entries") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|e| e.get("p").and_then(|p| p.as_str()).map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Pack, verify, extract and compare.
fn roundtrip(ctx: &Ctx, name: &str, tune: impl FnOnce(&mut PackOptions)) -> Result<(), String> {
    let d = ctx.dir(name)?;
    let expect = sample(&d.join("in"))?;
    let mut opts = plain_options(ctx);
    tune(&mut opts);
    opts.threads = ctx.jobs;
    opts.overwrite = true;
    let archive = d.join("a.kiln");
    pack_dir(&d.join("in/src"), &archive, &opts)?;
    verify_all(&archive, ctx.jobs)?;
    extract_to(&archive, &d.join("out"), ctx.jobs)?;
    same_files(&d.join("out"), &expect)
}

fn check_empty(ctx: &Ctx) -> Result<(), String> {
    let d = ctx.dir("empty")?;
    let root = d.join("in/src");
    fs::create_dir_all(root.join("empty_dir")).map_err(|e| e.to_string())?;
    put(&root.join("zero.txt"), b"")?;
    put(&root.join("sub/a.txt"), b"a")?;
    let archive = d.join("a.kiln");
    pack_dir(&root, &archive, &plain_options(ctx))?;
    let a = Archive::open(&archive).map_err(|e| describe(&e))?;
    let has_dir = a
        .entries()
        .iter()
        .any(|e| e.kind == EntryKind::Dir && e.path == "src/empty_dir");
    let has_zero = a
        .entries()
        .iter()
        .any(|e| e.kind == EntryKind::File && e.path == "src/zero.txt" && e.size == 0);
    if !has_dir || !has_zero {
        return Err("the empty folder or the empty file is not in the archive".to_string());
    }
    drop(a);
    extract_to(&archive, &d.join("out"), ctx.jobs)?;
    if !d.join("out/src/empty_dir").is_dir() {
        return Err("the empty folder was not restored".to_string());
    }
    match fs::metadata(d.join("out/src/zero.txt")) {
        Ok(m) if m.len() == 0 => Ok(()),
        _ => Err("the empty file was not restored".to_string()),
    }
}

fn check_names(ctx: &Ctx) -> Result<(), String> {
    let d = ctx.dir("names")?;
    let root = d.join("in/src");
    let long = format!("{}.txt", "x".repeat(120));
    let files = vec![
        (
            format!("src/\u{fc}n\u{ef}-\u{6587}\u{4ef6}-\u{1f642}.txt"),
            data(7, 5_000, false),
        ),
        (format!("src/{long}"), data(8, 2_000, false)),
        (
            "src/with space/and (parens).txt".to_string(),
            data(9, 1_000, false),
        ),
    ];
    for (rel, bytes) in &files {
        put(&d.join("in").join(rel), bytes)?;
    }
    let archive = d.join("a.kiln");
    pack_dir(&root, &archive, &plain_options(ctx))?;
    extract_to(&archive, &d.join("out"), ctx.jobs)?;
    same_files(&d.join("out"), &files)
}

fn check_damage(ctx: &Ctx) -> Result<(), String> {
    let d = ctx.dir("damage")?;
    let root = d.join("in/src");
    put(&root.join("r.bin"), &data(11, 65_536, true))?;
    let archive = d.join("a.kiln");
    let mut o = PackOptions::from_profile(Profile::Store);
    o.threads = ctx.jobs;
    o.overwrite = true;
    pack_dir(&root, &archive, &o)?;
    let mut bytes = fs::read(&archive).map_err(|e| e.to_string())?;
    if bytes.len() < 4_000 {
        return Err("the test archive is unexpectedly small".to_string());
    }
    bytes[1_000] ^= 0x55;
    fs::write(&archive, &bytes).map_err(|e| e.to_string())?;
    match Archive::open(&archive) {
        Err(_) => Ok(()),
        Ok(a) => match kiln::verify_with_progress(&a, ctx.jobs, None, &mut |_, _| {}) {
            Err(_) => Ok(()),
            Ok(_) => Err("a changed byte went unnoticed".to_string()),
        },
    }
}

fn check_garbage(ctx: &Ctx) -> Result<(), String> {
    let d = ctx.dir("garbage")?;
    let p = d.join("x.kiln");
    put(&p, &data(13, 5_000, true))?;
    match Archive::open(&p) {
        Err(_) => Ok(()),
        Ok(_) => Err("random bytes were accepted as an archive".to_string()),
    }
}

/// A plain archive of the sample folder at `<dir>/a.kiln`.
fn make_plain(ctx: &Ctx, name: &str) -> Result<(PathBuf, PathBuf, Vec<(String, Vec<u8>)>), String> {
    let d = ctx.dir(name)?;
    let expect = sample(&d.join("in"))?;
    let archive = d.join("a.kiln");
    pack_dir(&d.join("in/src"), &archive, &plain_options(ctx))?;
    Ok((d, archive, expect))
}

fn check_list(ctx: &Ctx) -> Result<(), String> {
    let (_, archive, expect) = make_plain(ctx, "list")?;
    let v = arc::open_archive(&archive.to_string_lossy(), "");
    must_ok(&v, "open_archive")?;
    if v.get("kind").and_then(|k| k.as_str()) != Some("archive") || flag(&v, "encrypted") {
        return Err("the listing has the wrong kind".to_string());
    }
    let n = listed(&v).len();
    if n != expect.len() {
        return Err(format!("{n} files listed, {} expected", expect.len()));
    }
    Ok(())
}

const PW: &str = "selftest-password";

/// Lock the plain archive `plain` into `<dir>/locked.kiln`.
fn lock_it(plain: &Path, out: &Path) -> Result<(), String> {
    let setup = lock::Setup::new(PW).map_err(|e| e.message())?;
    lock::seal(plain, out, &setup, lock::KIND_ARCHIVE, &mut |_, _| {})
        .map(|_| ())
        .map_err(|e| e.message())
}

fn check_locked(ctx: &Ctx) -> Result<(), String> {
    let (d, plain, expect) = make_plain(ctx, "locked")?;
    let locked = d.join("locked.kiln");
    lock_it(&plain, &locked)?;
    let p = locked.to_string_lossy().into_owned();
    if !flag(&arc::open_archive(&p, ""), "need_password") {
        return Err("a locked archive opened without a password".to_string());
    }
    if !flag(&arc::open_archive(&p, "wrong password"), "bad_password") {
        return Err("a wrong password was not refused".to_string());
    }
    let v = arc::open_archive(&p, PW);
    must_ok(&v, "open with the password")?;
    if !flag(&v, "encrypted") {
        return Err("the listing does not say the archive is locked".to_string());
    }
    let dest = d.join("out");
    let r = arc::extract(&p, PW, "[]", &dest.to_string_lossy(), false, ctx.jobs);
    must_ok(&r, "extract")?;
    same_files(&dest, &expect)?;
    let t = arc::test(&p, PW, ctx.jobs);
    must_ok(&t, "test")
}

/// Files below `dir` and their total size.
fn tree_totals(dir: &Path) -> (u64, u64) {
    let mut files = 0;
    let mut bytes = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => {
                    files += 1;
                    bytes += m.len();
                }
                Err(_) => {}
            }
        }
    }
    (files, bytes)
}

fn check_set(ctx: &Ctx) -> Result<(), String> {
    let d = ctx.dir("set")?;
    let expect = sample(&d.join("in"))?;
    let set_dir = d.join("set");
    let r = packing::pack(&packing::Request {
        items: vec![d.join("in/src").to_string_lossy().into_owned()],
        dest: set_dir.to_string_lossy().into_owned(),
        encrypt: true,
        each: true,
        store: false,
        overwrite: false,
        password: PW.to_string(),
        delete_src: false,
        jobs: ctx.jobs,
    });
    must_ok(&r, "pack a set")?;
    let p = set_dir.to_string_lossy().into_owned();
    if !flag(&arc::open_archive(&p, ""), "need_password") {
        return Err("a locked set opened without a password".to_string());
    }
    let v = arc::open_archive(&p, PW);
    must_ok(&v, "open the set")?;
    if v.get("kind").and_then(|k| k.as_str()) != Some("set") {
        return Err("the folder was not listed as a set".to_string());
    }
    let dest = d.join("out");
    let e = arc::extract(&p, PW, "[]", &dest.to_string_lossy(), false, ctx.jobs);
    must_ok(&e, "extract the set")?;
    let (files, bytes) = tree_totals(&dest);
    let want_bytes: u64 = expect.iter().map(|(_, b)| b.len() as u64).sum();
    if files != expect.len() as u64 || bytes != want_bytes {
        return Err(format!(
            "the set gave back {files} files / {bytes} bytes, expected {} / {want_bytes}",
            expect.len()
        ));
    }
    Ok(())
}

fn member(p: &str) -> String {
    json!([{ "p": p, "dir": false }]).to_string()
}

fn run_edit(archive: &Path, password: &str, d: &Path, jobs: usize) -> Result<(), String> {
    let p = archive.to_string_lossy().into_owned();
    let b = edit::begin(&p, password);
    must_ok(&b, "begin")?;
    let sid = b
        .get("sid")
        .and_then(|s| s.as_str())
        .ok_or_else(|| "begin gave no session".to_string())?
        .to_string();
    let result = (|| -> Result<(), String> {
        let m = |json_text: String| arc::parse_members(&json_text);
        must_ok(&edit::remove(&sid, &m(member("src/top.dat"))), "remove")?;
        must_ok(&edit::new_text(&sid, "src/new.txt"), "new file")?;
        let src = d.join("hello.txt");
        put(&src, b"hello")?;
        must_ok(
            &edit::set_file(&sid, "src/new.txt", &src.to_string_lossy()),
            "set file",
        )?;
        let t = edit::transfer(
            &sid,
            &m(member("src/docs/readme.txt")),
            "src/moved",
            true,
            "ask",
            "",
            jobs,
        );
        must_ok(&t, "move")?;
        let added = d.join("added.bin");
        put(&added, &data(21, 10_000, true))?;
        must_ok(
            &edit::add(&sid, &[added.to_string_lossy().into_owned()], "src/bin"),
            "add",
        )?;
        let st = edit::state(&sid);
        if !flag(&st, "dirty") {
            return Err("the session does not know it has changes".to_string());
        }
        let ff = edit::file_for(
            &sid,
            "src/new.txt",
            &d.join("cache").to_string_lossy(),
            true,
            jobs,
        );
        must_ok(&ff, "file_for")?;
        must_ok(&edit::save(&sid, jobs), "save")
    })();
    let _ = edit::close(&sid);
    result?;

    let names = listed(&arc::open_archive(&p, password));
    for gone in ["src/top.dat", "src/docs/readme.txt"] {
        if names.iter().any(|n| n == gone) {
            return Err(format!("{gone} is still in the archive"));
        }
    }
    for want in [
        "src/new.txt",
        "src/moved/readme.txt",
        "src/bin/random.bin",
        "src/bin/added.bin",
        "src/docs/notes/big.txt",
    ] {
        if !names.iter().any(|n| n == want) {
            return Err(format!("{want} is missing after saving"));
        }
    }
    let dest = d.join("after");
    let r = arc::extract(&p, password, "[]", &dest.to_string_lossy(), false, jobs);
    must_ok(&r, "extract after saving")?;
    let got = fs::read(dest.join("src/new.txt")).map_err(|e| e.to_string())?;
    if got != b"hello" {
        return Err("the edited file has the wrong contents".to_string());
    }
    let want = data(3, 200_000, true);
    let kept = fs::read(dest.join("src/bin/random.bin")).map_err(|e| e.to_string())?;
    if kept != want {
        return Err("an untouched file changed".to_string());
    }
    Ok(())
}

fn check_edit(ctx: &Ctx) -> Result<(), String> {
    let (d, archive, _) = make_plain(ctx, "edit")?;
    run_edit(&archive, "", &d, ctx.jobs)
}

fn check_edit_locked(ctx: &Ctx) -> Result<(), String> {
    let (d, plain, _) = make_plain(ctx, "editlocked")?;
    let locked = d.join("locked.kiln");
    lock_it(&plain, &locked)?;
    run_edit(&locked, PW, &d, ctx.jobs)?;
    if !flag(
        &arc::open_archive(&locked.to_string_lossy(), ""),
        "need_password",
    ) {
        return Err("the archive is no longer locked after saving".to_string());
    }
    Ok(())
}

fn selftest(jobs: usize) -> Value {
    let total = CHECKS.len() as u64;
    let scratch = match Scratch::new() {
        Ok(s) => s,
        Err(e) => return fail(&e.message()),
    };
    let ctx = Ctx {
        root: scratch.path().join("selftest"),
        jobs,
    };
    if let Err(e) = fs::create_dir_all(&ctx.root) {
        return fail(&format!("{}: {e}", ctx.root.display()));
    }
    let version = env!("CARGO_PKG_VERSION");
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let mut meter = Meter::new();
    meter.tick(0, total, "Starting...", true);
    let started = Instant::now();
    let mut checks: Vec<Value> = Vec::new();
    let mut lines = vec![format!("kiln {version} selftest - {cores} core(s)")];
    let mut failed = 0u64;
    for (i, (name, check)) in CHECKS.iter().enumerate() {
        if kiln::cancel::is_set() {
            return cancelled();
        }
        let t = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| check(&ctx)));
        let secs = t.elapsed().as_secs_f64();
        if kiln::cancel::is_set() {
            return cancelled();
        }
        let (ok, msg) = match outcome {
            Ok(Ok(())) => (true, String::new()),
            Ok(Err(m)) => (false, m),
            Err(_) => (false, "the check stopped unexpectedly".to_string()),
        };
        if !ok {
            failed += 1;
        }
        lines.push(if ok {
            format!("PASS {name} ({secs:.1}s)")
        } else {
            format!("FAIL {name} ({secs:.1}s) - {msg}")
        });
        checks.push(json!({ "ok": ok, "name": name, "secs": secs, "msg": msg }));
        let done = i as u64 + 1;
        meter.tick(
            done,
            total,
            &format!("{done} of {total} checks done \u{b7} {name}"),
            true,
        );
    }
    let took = started.elapsed().as_secs_f64();
    lines.push(if failed == 0 {
        format!("all {total} checks passed in {took:.1}s")
    } else {
        format!("{failed} of {total} checks FAILED in {took:.1}s")
    });
    drop(scratch);
    json!({
        "ok": failed == 0,
        "text": lines.join("\n"),
        "view": {
            "type": "selftest",
            "head": { "version": version, "cores": cores },
            "checks": checks,
            "notes": [],
            "total": total,
            "failed": failed,
            "secs": took,
            "finished": true,
        },
    })
}
