//! "One archive per file" (`kiln pack -i`): port of cmd_pack_each / write_index / check_set_target
//! in kiln_v14.py.
//!
//! Without encryption the folder layout is mirrored (photos/a.jpg -> OUT/photos/a.jpg.kln). With
//! encryption every file gets a random name (16 hex digits + ".kln") in one flat folder and an
//! encrypted `_index.kln` maps the names back (a "set", read by sets.rs). Every archive has its own
//! salt and key, so nonces are never reused across archives.

use crate::app::{NoProgress, Progress};
use crate::container::{KilnError, Result};
use crate::packapp::{filter_for, fmt_size, plan, MIN_PASSWORD};
use crate::packer::{self, PackFile, PackOpts, CANCELLED};
use crate::sets::{read_index_meta, INDEX_NAME};
use crate::walk;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Instant, UNIX_EPOCH};

pub struct EachArgs<'a> {
    pub items: &'a [String],
    /// the output folder (python: args.archive)
    pub out_dir: &'a str,
    pub encrypt: bool,
    pub store: bool,
    /// -f: rebuild archives that already exist
    pub force: bool,
    pub password: &'a str,
    /// 0 = automatic
    pub jobs_arg: usize,
}

/// One index entry: where the original was, and its size / date when it was packed (null in an old v1 index).
#[derive(Clone, PartialEq)]
struct Meta {
    path: String,
    size: Value,
    mtime: Value,
}

// ---------------------------------------------------------------- index.json

/// json.dumps string with ensure_ascii=True: everything outside space..~ becomes \uXXXX (lowercase hex,
/// surrogate pairs above the BMP).
fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
        }
    }
    out.push('"');
}

/// json.dumps({"v": 2, "files": mapping}, sort_keys=True): default separators (", " and ": ").
fn index_json(mapping: &[(String, Meta)]) -> String {
    let mut v: Vec<&(String, Meta)> = mapping.iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0)); // UTF-8 byte order == code point order
    let mut s = String::from("{\"files\": {");
    for (i, (name, m)) in v.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        json_str(name, &mut s);
        s.push_str(": {\"mtime\": ");
        s.push_str(&m.mtime.to_string());
        s.push_str(", \"path\": ");
        json_str(&m.path, &mut s);
        s.push_str(", \"size\": ");
        s.push_str(&m.size.to_string());
        s.push('}');
    }
    s.push_str("}, \"v\": 2}");
    s
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// secrets.token_hex(8)
fn token_hex8() -> Result<String> {
    let b: [u8; 8] = packer::random_bytes()?;
    Ok(hex(&b))
}

fn ioerr(what: &str, e: std::io::Error) -> KilnError {
    KilnError(format!("{what}: {e}"))
}

fn trim_slash(d: &str) -> &str {
    let t = d.trim_end_matches('/');
    if t.is_empty() {
        "/"
    } else {
        t
    }
}

/// Packs the index as the encrypted index archive `out_dir/_index.kln` (always encrypted; `store` carries over).
fn write_index(out_dir: &str, mapping: &[(String, Meta)], pw: &str, store: bool) -> Result<()> {
    fs::create_dir_all(out_dir).map_err(|e| ioerr(out_dir, e))?;
    let js = index_json(mapping);
    let tmp = format!("{}/.kiln-index-{}.tmp", trim_slash(out_dir), token_hex8()?);
    {
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut fh = o.open(&tmp).map_err(|e| ioerr(&tmp, e))?;
        fh.write_all(js.as_bytes()).map_err(|e| ioerr(&tmp, e))?;
    }
    let r = (|| -> Result<()> {
        let md = fs::metadata(&tmp).map_err(|e| ioerr(&tmp, e))?;
        let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let size = js.len() as u64;
        let pf = PackFile {
            full: tmp.clone(),
            rel: "index.json".to_string(),
            size,
            mtime,
            mode: 0o600,
            filt: if store { 0 } else { walk::pick_bcj(&tmp, size) },
            pre: false,
        };
        let (segment, jobs) = plan(size, store, 1);
        let opts = PackOpts { store, password: Some(pw), segment, jobs };
        let dest = format!("{}/{}", trim_slash(out_dir), INDEX_NAME);
        packer::pack_archive(&dest, std::slice::from_ref(&pf), &opts, None)?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    r
}

// ---------------------------------------------------------------- set folder checks

/// A -i -e output folder must be a set itself, or new/empty, and not inside another set.
fn check_set_target(out_dir: &str) -> Result<()> {
    let d = walk::abspath(out_dir);
    if d.join(INDEX_NAME).is_file() {
        return Ok(());
    }
    let mut p = d.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| d.clone());
    loop {
        if p.join(INDEX_NAME).is_file() {
            let ps = p.to_string_lossy();
            return Err(KilnError(format!(
                "{out_dir} is inside the kiln set {ps}. A set inside a set is invisible to the outer one.\n  \
                 To add files to that set, pack into {ps} itself; to keep them separate, use a folder outside it."
            )));
        }
        match p.parent() {
            Some(q) => p = q.to_path_buf(),
            None => break,
        }
    }
    if d.is_dir() {
        if let Ok(mut rd) = fs::read_dir(&d) {
            if rd.next().is_some() {
                return Err(KilnError(format!(
                    "{out_dir} isn't empty and isn't a kiln set (no {INDEX_NAME}). Use a new or empty folder."
                )));
            }
        }
    }
    Ok(())
}

/// Random order, so the archives' timestamps don't reveal the original order.
fn shuffle<T>(v: &mut [T]) -> Result<()> {
    for i in (1..v.len()).rev() {
        let b: [u8; 8] = packer::random_bytes()?;
        let j = (u64::from_le_bytes(b) % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    Ok(())
}

// ---------------------------------------------------------------- the op

pub fn pack_each(a: &EachArgs, prog: &mut dyn Progress) -> String {
    match run(a, prog) {
        Ok(v) => v.to_string(),
        Err(e) if e.0 == CANCELLED => json!({"ok": false, "cancelled": true, "message": "Cancelled"}).to_string(),
        Err(e) => {
            let msg = crate::app::app_message(&e.0);
            json!({"ok": false, "message": msg, "bad_password": msg.contains("wrong password")}).to_string()
        }
    }
}

fn nothing_to_do(skipped: usize, warnings: Vec<String>) -> Value {
    json!({"ok": true, "done": 0, "packed": Vec::<String>::new(), "warnings": warnings,
           "message": format!("nothing to do: all {skipped} output(s) already exist (use -f to overwrite)")})
}

fn run(a: &EachArgs, prog: &mut dyn Progress) -> Result<Value> {
    let out_dir = a.out_dir;
    let store = a.store && a.encrypt;
    let rnd = a.encrypt; // -i with -e: random names + encrypted index (always)
    let op = Path::new(out_dir);
    if op.exists() && !op.is_dir() {
        return Err(KilnError(format!("{out_dir} is not a directory (with -i the first argument is the output directory)")));
    }
    if rnd {
        check_set_target(out_dir)?;
    }
    let out_abs = format!("{}/", walk::abspath(out_dir).to_string_lossy());
    let (found, warnings) = walk::collect(a.items, &[])?;
    let files: Vec<walk::Item> = found.into_iter().filter(|e| !walk::abspath(&e.full).to_string_lossy().starts_with(&out_abs)).collect();
    if files.is_empty() {
        return Err(KilnError("nothing to pack".to_string()));
    }
    let nfiles = files.len();
    let mut todo: Vec<(usize, String)> = Vec::new(); // (index into files, archive path)
    let mut skipped = 0usize;
    let mut mapping: Vec<(String, Meta)> = Vec::new();
    if rnd {
        // An existing index is reused, so reruns resume and never orphan files.
        let have = op.join(INDEX_NAME).is_file();
        if a.password.is_empty() {
            return Err(KilnError("enter a password".to_string()));
        }
        if !have && a.password.chars().count() < MIN_PASSWORD {
            return Err(KilnError(format!("password must be at least {MIN_PASSWORD} characters (a long passphrase is best)")));
        }
        let mut old: Vec<(String, Meta)> = Vec::new();
        if have {
            for it in read_index_meta(out_dir, a.password, &mut NoProgress)? {
                old.push((it.name, Meta { path: it.path, size: it.size, mtime: it.mtime }));
            }
        }
        let old_pos: HashMap<String, usize> = old.iter().enumerate().map(|(i, (n, _))| (n.clone(), i)).collect();
        let mut by_rel: HashMap<String, String> = HashMap::new();
        for (n, m) in &old {
            by_rel.insert(m.path.clone(), n.clone()); // a later entry wins, like the dict comprehension
        }
        mapping = old.clone();
        let mut pos: HashMap<String, usize> = old_pos.clone();
        for (i, e) in files.iter().enumerate() {
            let name = match by_rel.get(&e.rel) {
                Some(n) => n.clone(),
                None => loop {
                    let n = format!("{}.kln", token_hex8()?);
                    if !pos.contains_key(&n) {
                        break n;
                    }
                },
            };
            let dest = format!("{}/{}", trim_slash(out_dir), name);
            let mut fresh = Meta { path: e.rel.clone(), size: json!(e.size), mtime: json!(e.mtime) };
            if Path::new(&dest).exists() && !a.force {
                skipped += 1;
                // an archive that is kept keeps its recorded size/date; an old (v1) index has none,
                // so it gets them from the file as it is now
                if let Some(&k) = old_pos.get(&name) {
                    if !old[k].1.size.is_null() {
                        fresh = old[k].1.clone();
                    }
                }
            } else {
                todo.push((i, dest));
            }
            match pos.get(&name) {
                Some(&k) => mapping[k].1 = fresh,
                None => {
                    pos.insert(name.clone(), mapping.len());
                    mapping.push((name, fresh));
                }
            }
        }
        let same = mapping.len() == old.len() && mapping.iter().all(|(n, m)| old_pos.get(n).map_or(false, |&k| old[k].1 == *m));
        if todo.is_empty() && same {
            return Ok(nothing_to_do(skipped, warnings));
        }
        shuffle(&mut todo)?;
        fs::create_dir_all(out_dir).map_err(|e| ioerr(out_dir, e))?;
        write_index(out_dir, &mapping, a.password, store)?; // the index first: a crash leaves only orphans
    } else {
        for (i, e) in files.iter().enumerate() {
            let dest = format!("{}.kln", crate::app::join_rel(out_dir, &e.rel));
            if Path::new(&dest).exists() && !a.force {
                skipped += 1;
            } else {
                todo.push((i, dest));
            }
        }
        if todo.is_empty() {
            return Ok(nothing_to_do(skipped, warnings));
        }
    }
    if rnd && todo.is_empty() {
        return Ok(json!({"ok": true, "done": 0, "packed": Vec::<String>::new(), "warnings": warnings,
            "message": format!("index updated: {} entr(ies) in {}/{} (sizes and dates added); {} archive(s) already exist",
                               mapping.len(), out_dir, INDEX_NAME, skipped)}));
    }

    let pw = if rnd { Some(a.password) } else { None };
    let total_bytes: u64 = todo.iter().map(|(i, _)| files[*i].size).sum();
    let t0 = Instant::now();
    let (mut raw, mut packed_size) = (0u64, 0u64);
    let mut packed: Vec<String> = Vec::new();
    let mut done_bytes = 0u64;
    let mut last = Instant::now();
    let frac_of = |done: u64| if total_bytes > 0 { (done as f64 / total_bytes as f64).min(1.0) } else { -1.0 };
    for (k, (i, dest)) in todo.iter().enumerate() {
        let e = &files[*i];
        if prog.cancelled() {
            return Err(KilnError(CANCELLED.to_string()));
        }
        prog.event(&json!({"frac": frac_of(done_bytes), "done": k, "total": nfiles, "file": e.rel}).to_string());
        if let Some(parent) = Path::new(dest).parent() {
            fs::create_dir_all(parent).map_err(|er| ioerr(&parent.to_string_lossy(), er))?;
        }
        // the archive holds the file under its own name (python packs [full path], so rel = basename)
        let base = Path::new(&e.full).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| e.rel.clone());
        let pf = PackFile { full: e.full.clone(), rel: base, size: e.size, mtime: e.mtime, mode: e.mode, filt: filter_for(e, store), pre: e.pre };
        let (segment, jobs) = plan(pf.size, store, a.jobs_arg);
        let opts = PackOpts { store, password: pw, segment, jobs };
        let mut stopped = false;
        let res = {
            let mut cb = |n: u64| -> bool {
                done_bytes += n;
                if last.elapsed().as_millis() >= 50 {
                    last = Instant::now();
                    prog.event(&json!({"frac": frac_of(done_bytes), "done": k, "total": nfiles, "file": e.rel}).to_string());
                }
                if prog.cancelled() {
                    stopped = true;
                }
                stopped
            };
            packer::pack_archive(dest, std::slice::from_ref(&pf), &opts, Some(&mut cb as &mut dyn FnMut(u64) -> bool))
        };
        if let Err(er) = res {
            if er.0 == CANCELLED {
                return Err(er);
            }
            return Err(KilnError(format!("{}: {}", e.rel, er.0)));
        }
        let asize = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        raw += e.size;
        packed_size += asize;
        packed.push(e.full.clone());
    }
    prog.event(&json!({"frac": 1.0, "done": todo.len(), "total": nfiles, "file": ""}).to_string());
    let ratio = if raw > 0 { 100.0 * packed_size as f64 / raw as f64 } else { 100.0 };
    let msg = format!(
        "{} archive(s) in {}: {} -> {} ({:.1}% of original) in {:.1}s{}{}{}{}",
        todo.len(),
        out_dir,
        fmt_size(raw),
        fmt_size(packed_size),
        ratio,
        t0.elapsed().as_secs_f64(),
        if a.encrypt { " [encrypted: AES-256-GCM]" } else { "" },
        if store { " [no compression]" } else { "" },
        if rnd { format!(" [random names, index: {INDEX_NAME}]") } else { String::new() },
        if skipped > 0 { format!(", {skipped} skipped (already exist)") } else { String::new() },
    );
    Ok(json!({"ok": true, "message": msg, "done": todo.len(), "packed": packed, "warnings": warnings}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::NoProgress;
    use crate::container::Archive;
    use crate::sets::parse_index;
    use std::fs::File;

    #[test]
    fn json_matches_python() {
        let m = vec![
            ("b.kln".to_string(), Meta { path: "dir/é \"q\" \\ \u{1F600}.txt".into(), size: json!(5), mtime: json!(7) }),
            ("a.kln".to_string(), Meta { path: "x\ty\n\u{7f}".into(), size: Value::Null, mtime: Value::Null }),
        ];
        // made by python: json.dumps({"v": 2, "files": ...}, sort_keys=True)
        let want = r#"{"files": {"a.kln": {"mtime": null, "path": "x\ty\n\u007f", "size": null}, "b.kln": {"mtime": 7, "path": "dir/\u00e9 \"q\" \\ \ud83d\ude00.txt", "size": 5}}, "v": 2}"#;
        assert_eq!(index_json(&m), want);
        assert_eq!(index_json(&[]), r#"{"files": {}, "v": 2}"#);
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("kiln_each_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_inputs(d: &Path) -> Vec<String> {
        fs::create_dir_all(d.join("photos/sub")).unwrap();
        fs::write(d.join("photos/a.txt"), b"hello hello hello hello\n".repeat(50)).unwrap();
        fs::write(d.join("photos/sub/b.bin"), (0..3000u32).map(|i| (i * 7 % 251) as u8).collect::<Vec<u8>>()).unwrap();
        fs::write(d.join("c.txt"), b"third file").unwrap();
        vec![d.join("photos").to_string_lossy().into_owned(), d.join("c.txt").to_string_lossy().into_owned()]
    }

    #[test]
    fn plain_mirrors_layout_and_skips_existing() {
        let d = tmpdir("plain");
        let items = make_inputs(&d);
        let out = d.join("out").to_string_lossy().into_owned();
        let args = EachArgs { items: &items, out_dir: &out, encrypt: false, store: false, force: false, password: "", jobs_arg: 1 };
        let r: Value = serde_json::from_str(&pack_each(&args, &mut NoProgress)).unwrap();
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["done"], json!(3));
        for p in ["photos/a.txt.kln", "photos/sub/b.bin.kln", "c.txt.kln"] {
            let fh = File::open(format!("{out}/{p}")).unwrap();
            let mut a = Archive::open(fh, None).unwrap();
            assert_eq!(a.table.entries.len(), 1);
            a.test().unwrap();
        }
        // the entry is stored under the file's own name
        let a = Archive::open(File::open(format!("{out}/photos/sub/b.bin.kln")).unwrap(), None).unwrap();
        assert_eq!(a.table.entries[0].rel, "b.bin");
        // second run: everything exists
        let r: Value = serde_json::from_str(&pack_each(&args, &mut NoProgress)).unwrap();
        assert_eq!(r["ok"], json!(true));
        assert!(r["message"].as_str().unwrap().starts_with("nothing to do: all 3 output(s) already exist"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn encrypted_set_has_index_and_resumes() {
        let d = tmpdir("enc");
        let items = make_inputs(&d);
        let out = d.join("set").to_string_lossy().into_owned();
        let pw = "correct-horse-battery";
        let args = EachArgs { items: &items, out_dir: &out, encrypt: true, store: false, force: false, password: pw, jobs_arg: 1 };
        let r: Value = serde_json::from_str(&pack_each(&args, &mut NoProgress)).unwrap();
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["done"], json!(3));
        assert!(r["message"].as_str().unwrap().contains("[random names, index: _index.kln]"));
        let idx = read_index_meta(&out, pw, &mut NoProgress).unwrap();
        assert_eq!(idx.len(), 3);
        let mut paths: Vec<String> = idx.iter().map(|i| i.path.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec!["c.txt", "photos/a.txt", "photos/sub/b.bin"]);
        for it in &idx {
            assert_eq!(it.name.len(), 16 + 4);
            let mut a = Archive::open(File::open(format!("{out}/{}", it.name)).unwrap(), Some(pw)).unwrap();
            a.test().unwrap();
        }
        // the index parses with the same reader sets.rs uses
        let js = index_json(&idx.iter().map(|i| (i.name.clone(), Meta { path: i.path.clone(), size: i.size.clone(), mtime: i.mtime.clone() })).collect::<Vec<_>>());
        assert_eq!(parse_index(js.as_bytes()).unwrap().len(), 3);
        // rerun with the same password: nothing to do, same names
        let r: Value = serde_json::from_str(&pack_each(&args, &mut NoProgress)).unwrap();
        assert!(r["message"].as_str().unwrap().starts_with("nothing to do"), "{r}");
        let again = read_index_meta(&out, pw, &mut NoProgress).unwrap();
        assert_eq!(again.iter().map(|i| i.name.clone()).collect::<Vec<_>>(), idx.iter().map(|i| i.name.clone()).collect::<Vec<_>>());
        // a wrong password on an existing set is refused before any file work
        let bad = EachArgs { password: "another-long-password", ..args };
        let r: Value = serde_json::from_str(&pack_each(&bad, &mut NoProgress)).unwrap();
        assert_eq!(r["ok"], json!(false));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn set_target_rules() {
        let d = tmpdir("target");
        let inner = d.join("a/b");
        fs::create_dir_all(&inner).unwrap();
        fs::write(d.join("a/x.txt"), b"x").unwrap();
        // not empty and no index
        assert!(check_set_target(d.join("a").to_str().unwrap()).is_err());
        // empty or new is fine
        assert!(check_set_target(inner.to_str().unwrap()).is_ok());
        assert!(check_set_target(d.join("new").to_str().unwrap()).is_ok());
        // inside another set
        fs::write(d.join("a").join(INDEX_NAME), b"x").unwrap();
        assert!(check_set_target(inner.to_str().unwrap()).unwrap_err().0.contains("is inside the kiln set"));
        assert!(check_set_target(d.join("a").to_str().unwrap()).is_ok());
        let _ = fs::remove_dir_all(&d);
    }
}
