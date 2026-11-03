//! C ABI. One entry point, JSON in / JSON out, like kiln_app.py:
//!
//!   char *kiln_rs_call(const char *op, const char *args_json, kiln_rs_cb cb, void *user);
//!   void  kiln_rs_free(char *s);
//!
//! ops: "version" | "open" | "test" | "extract" | "pack"   (see include/kiln_rs.h for the arguments)
//! `cb` (may be NULL) is called with a progress event (JSON) or with NULL to ask "cancelled?";
//! a non-zero return means cancel. The result is always a Rust-owned C string that
//! the caller releases with kiln_rs_free. A panic is caught and returned as {"ok":false,...}.

use crate::app::{self, Progress};
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

pub type KilnRsCb = Option<unsafe extern "C" fn(event: *const c_char, user: *mut c_void) -> c_int>;

struct CbProgress {
    cb: KilnRsCb,
    user: *mut c_void,
    stop: bool,
}

impl Progress for CbProgress {
    fn event(&mut self, json: &str) {
        if let (Some(cb), Ok(s)) = (self.cb, CString::new(json)) {
            if unsafe { cb(s.as_ptr(), self.user) } != 0 {
                self.stop = true;
            }
        }
    }
    fn cancelled(&mut self) -> bool {
        if !self.stop {
            if let Some(cb) = self.cb {
                if unsafe { cb(std::ptr::null(), self.user) } != 0 {
                    self.stop = true;
                }
            }
        }
        self.stop
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// The whole dispatcher, usable from Rust tests without any C pointers.
pub fn call(op: &str, args: &str, prog: &mut dyn Progress) -> String {
    let a: Value = match serde_json::from_str(if args.is_empty() { "{}" } else { args }) {
        Ok(v) => v,
        Err(e) => return json!({"ok": false, "error": format!("bad arguments: {e}")}).to_string(),
    };
    match op {
        "version" => app::version(),
        "open" => app::open_archive(s(&a, "path"), s(&a, "password")),
        "test" => app::test(s(&a, "path"), s(&a, "password"), prog),
        "extract" => {
            let members = a.get("members").map(|m| m.to_string()).unwrap_or_else(|| "[]".into());
            let overwrite = a.get("overwrite").and_then(|x| x.as_bool()).unwrap_or(false);
            app::extract(s(&a, "path"), s(&a, "password"), &members, s(&a, "dest"), overwrite, prog)
        }
        "pack" => crate::packapp::pack(&a, prog),
        _ => json!({"ok": false, "error": format!("unknown operation: {op}")}).to_string(),
    }
}

/// # Safety
/// `op` and `args_json` must be NUL-terminated strings (args may be NULL); `cb`/`user` as documented.
#[no_mangle]
pub unsafe extern "C" fn kiln_rs_call(op: *const c_char, args_json: *const c_char, cb: KilnRsCb, user: *mut c_void) -> *mut c_char {
    let res = catch_unwind(AssertUnwindSafe(|| {
        if op.is_null() {
            return json!({"ok": false, "error": "no operation"}).to_string();
        }
        let op = CStr::from_ptr(op).to_string_lossy().into_owned();
        let args = if args_json.is_null() { String::new() } else { CStr::from_ptr(args_json).to_string_lossy().into_owned() };
        let mut p = CbProgress { cb, user, stop: false };
        call(&op, &args, &mut p)
    }))
    .unwrap_or_else(|_| json!({"ok": false, "error": "internal error in the Rust engine"}).to_string());
    CString::new(res.replace('\0', " ")).unwrap().into_raw()
}

/// # Safety
/// `p` must come from kiln_rs_call (or be NULL) and be freed once.
#[no_mangle]
pub unsafe extern "C" fn kiln_rs_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}
