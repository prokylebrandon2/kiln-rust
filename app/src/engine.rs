//! Starting the engine. `kiln_bridge` is linked into the app and its functions are called
//! directly with JSON arguments, getting JSON results back.

use std::path::Path;

use serde_json::json;

/// Get the engine ready: make its scratch folder, check that it answers, trim the preview cache.
///
/// Itself is set by the platform crate before anything else runs (it needs `unsafe`);
/// the folder it points at is `<cache>/engine-tmp`, inside the app cache so that Settings > Cache
/// covers it. This blocks for as long as trimming takes, so call it from a worker thread.
pub fn init(cache_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir.join("engine-tmp")).map_err(|e| e.to_string())?;
    let version = kiln_bridge::call("version", &json!([]), 0);
    if version.get("kiln").is_none() {
        return Err("the Kiln engine is missing from this install".to_string());
    }
    // Failing to trim the cache must never stop the app.
    let preview = cache_dir.join("preview");
    let _ = kiln_bridge::call("trim_cache", &json!([preview.to_string_lossy()]), 0);
    Ok(())
}
