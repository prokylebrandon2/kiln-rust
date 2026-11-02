// cargo test --test app_corpus   (run from kiln-rs/; corpus is ../tests/corpus)
// Same checks as tests/mirror/test_app.py, but only against expected.json (no Python engine needed).
use kiln_rs::app::{NoProgress, Progress};
use kiln_rs::ffi::call;
use serde_json::{json, Value};
use std::fs;

const DIR: &str = "../tests/corpus";

fn exp() -> Value {
    serde_json::from_str(&fs::read_to_string(format!("{DIR}/expected.json")).unwrap()).unwrap()
}
fn pw() -> String {
    exp()["password"].as_str().unwrap().to_string()
}
fn tmpdir(name: &str) -> String {
    let d = format!("{}/kiln_rs_test_{}_{}", std::env::temp_dir().display(), name, std::process::id());
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn open_matches_expected_tables() {
    let e = exp();
    for name in ["plain", "ckpt", "enc_store", "ckpt_enc"] {
        let password = if e["archives"][name]["encrypted"].as_bool().unwrap() { pw() } else { String::new() };
        let r: Value = serde_json::from_str(&call("open", &json!({"path": format!("{DIR}/{name}.kln"), "password": password}).to_string(), &mut NoProgress)).unwrap();
        assert_eq!(r["ok"], true, "{name}: {r}");
        let files = e["archives"][name]["files"].as_array().unwrap();
        let ents = r["entries"].as_array().unwrap();
        assert_eq!(ents.len(), files.len());
        for (a, b) in ents.iter().zip(files) {
            assert_eq!(a["p"], b["rel"]);
            assert_eq!(a["s"], b["size"]);
            assert_eq!(a["m"], b["mtime"]);
        }
    }
}

#[test]
fn encrypted_needs_password_and_wrong_one_is_reported() {
    let p = format!("{DIR}/enc_store.kln");
    let r: Value = serde_json::from_str(&call("open", &json!({"path": p}).to_string(), &mut NoProgress)).unwrap();
    assert_eq!(r["need_password"], true);
    let r: Value = serde_json::from_str(&call("open", &json!({"path": p, "password": "nope-nope-nope"}).to_string(), &mut NoProgress)).unwrap();
    assert_eq!(r["bad_password"], true, "{r}");
}

#[test]
fn extract_all_matches_crcs() {
    let e = exp();
    let out = tmpdir("all");
    let r: Value = serde_json::from_str(&call("extract", &json!({"path": format!("{DIR}/plain.kln"), "dest": out}).to_string(), &mut NoProgress)).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    for f in e["archives"]["plain"]["files"].as_array().unwrap() {
        let data = fs::read(format!("{out}/{}", f["rel"].as_str().unwrap())).unwrap();
        assert_eq!(data.len() as u64, f["size"].as_u64().unwrap());
        assert_eq!(crc32fast::hash(&data) as u64, f["crc"].as_u64().unwrap());
    }
    // second run without overwrite: refused, nothing changed
    let r: Value = serde_json::from_str(&call("extract", &json!({"path": format!("{DIR}/plain.kln"), "dest": out}).to_string(), &mut NoProgress)).unwrap();
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("already exist"));
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn partial_extract_from_checkpoint() {
    let out = tmpdir("part");
    let r: Value = serde_json::from_str(&call(
        "extract",
        &json!({"path": format!("{DIR}/ckpt.kln"), "dest": out, "members": [{"p": "in/sub/text.txt"}]}).to_string(),
        &mut NoProgress,
    ))
    .unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["done"], 1);
    assert!(fs::metadata(format!("{out}/in/sub/text.txt")).is_ok());
    assert!(fs::metadata(format!("{out}/in/rand.bin")).is_err());
    let _ = fs::remove_dir_all(&out);
}

struct StopAfter(u32, u32);
impl Progress for StopAfter {
    fn event(&mut self, _j: &str) {}
    fn cancelled(&mut self) -> bool {
        self.1 += 1;
        self.1 > self.0
    }
}

#[test]
fn cancel_leaves_nothing_behind() {
    let out = tmpdir("cancel");
    let r: Value = serde_json::from_str(&call("extract", &json!({"path": format!("{DIR}/plain.kln"), "dest": out}).to_string(), &mut StopAfter(5, 0))).unwrap();
    assert_eq!(r["cancelled"], true, "{r}");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn damaged_archives_fail_with_checksum_or_truncation() {
    for name in ["bad_flip_data", "bad_truncated"] {
        let r: Value = serde_json::from_str(&call("test", &json!({"path": format!("{DIR}/{name}.kln")}).to_string(), &mut NoProgress)).unwrap();
        assert_eq!(r["ok"], false, "{name}");
    }
}

#[test]
fn unknown_op_and_bad_args() {
    let r: Value = serde_json::from_str(&call("zap", "{}", &mut NoProgress)).unwrap();
    assert_eq!(r["ok"], false);
    let r: Value = serde_json::from_str(&call("open", "{oops", &mut NoProgress)).unwrap();
    assert!(r["error"].as_str().unwrap().starts_with("bad arguments"));
}
