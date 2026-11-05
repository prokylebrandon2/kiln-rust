//! `Icons.java`: which icon and colour a file or folder gets.
//!
//! Icons are named as in `ui/ui/icons.slint` (the Android drawable without its `ic_` prefix).

use crate::fmt;

const PDF: u32 = 0xE5574B;
const DOC: u32 = 0x4A8FE7;
const SHEET: u32 = 0x3FA66B;
const SLIDES: u32 = 0xE8833A;
const APK: u32 = 0x5DB75D;
const ZIP: u32 = 0xE0A030;
const FONT: u32 = 0x8E7CC3;
const DB: u32 = 0x5BA3B8;
const WEB: u32 = 0x3D9BE0;
const TERM: u32 = 0x6FBF73;
const KEY: u32 = 0xD6A53A;
const BOOK: u32 = 0xC77D4E;
const DISK: u32 = 0x8D99AE;
const CODE: u32 = 0x5B8DEF;
const IMAGE: u32 = 0x3FA6A0;
const AUDIO: u32 = 0xC45BA3;
const VIDEO: u32 = 0xE0634F;

/// The icon of a file and its colour as `0xRRGGBB` (`None`: the normal secondary text colour).
pub fn for_file(name: &str) -> (&'static str, Option<u32>) {
    match fmt::ext(name).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic" | "heif" | "svg" | "tif"
        | "tiff" | "ico" | "raw" | "dng" => ("image", Some(IMAGE)),
        "mp3" | "m4a" | "flac" | "ogg" | "oga" | "opus" | "wav" | "aac" | "wma" | "mka" | "amr"
        | "mid" | "midi" => ("music", Some(AUDIO)),
        "mp4" | "mkv" | "webm" | "3gp" | "mov" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => {
            ("video", Some(VIDEO))
        }
        "pdf" => ("f_pdf", Some(PDF)),
        "doc" | "docx" | "odt" | "rtf" | "pages" | "dot" | "dotx" => ("f_doc", Some(DOC)),
        "xls" | "xlsx" | "ods" | "csv" | "tsv" | "numbers" | "xlsm" => ("f_sheet", Some(SHEET)),
        "ppt" | "pptx" | "odp" | "key" | "pps" | "ppsx" => ("f_slides", Some(SLIDES)),
        "apk" | "xapk" | "apks" | "aab" | "apkm" => ("f_apk", Some(APK)),
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "lz4" | "cab"
        | "jar" | "war" | "z" | "lzma" => ("f_zip", Some(ZIP)),
        "ttf" | "otf" | "woff" | "woff2" | "ttc" | "fon" => ("f_font", Some(FONT)),
        "db" | "sqlite" | "sqlite3" | "mdb" | "realm" => ("f_db", Some(DB)),
        "html" | "htm" | "xhtml" | "mhtml" | "url" | "webloc" => ("f_web", Some(WEB)),
        "sh" | "bash" | "zsh" | "fish" | "bat" | "cmd" | "ps1" => ("f_term", Some(TERM)),
        "pem" | "crt" | "cer" | "der" | "p12" | "pfx" | "keystore" | "jks" | "gpg" | "asc"
        | "pub" => ("f_key", Some(KEY)),
        "epub" | "mobi" | "azw" | "azw3" | "fb2" | "cbz" | "cbr" | "djvu" => ("f_book", Some(BOOK)),
        "iso" | "img" | "dmg" | "vhd" | "vmdk" | "bin" => ("f_disk", Some(DISK)),
        "exe" | "dll" | "so" | "dex" | "o" | "class" | "elf" => ("f_bin", Some(DISK)),
        "txt" | "log" | "md" | "rst" | "srt" | "vtt" | "nfo" => ("f_text", None),
        "ini" | "conf" | "cfg" | "toml" | "yml" | "yaml" | "properties" | "env" | "gitignore"
        | "service" | "desktop" | "lock" => ("settings", None),
        "json" | "xml" | "py" | "c" | "h" | "cpp" | "java" | "kt" | "gradle" | "lua" | "rb"
        | "go" | "rs" | "sql" | "js" | "ts" | "css" | "pl" | "php" | "swift" | "dart" | "jsx"
        | "tsx" | "vue" | "cs" | "scala" | "mk" | "cmake" | "diff" | "patch" | "tex" => {
            ("code", Some(CODE))
        }
        _ => ("file", None),
    }
}

/// The icon of an icon key (the plain folder for an unknown key); `Icons.byKey`.
pub fn by_key(key: &str) -> &'static str {
    match key {
        "download" => "d_download",
        "camera" => "d_camera",
        "pictures" => "d_pictures",
        "movies" => "d_movies",
        "music" => "d_music",
        "documents" => "d_documents",
        "android" => "d_android",
        "bell" => "d_bell",
        "zip" => "d_zip",
        "star" => "d_star",
        "heart" => "d_heart",
        "lock" => "d_lock",
        "code" => "d_code",
        "home" => "home",
        "phone" => "d_phone",
        "sd" => "d_sd",
        "usb" => "usb",
        _ => "folder",
    }
}

/// `Icons.defaultKey`: the key Android's standard folders get. `name` and `parent` are the
/// folder's own name and its parent's, and `at_storage_root` says the folder sits directly in the
/// top of a storage.
pub fn default_key(name: &str, parent: &str, at_storage_root: bool) -> Option<&'static str> {
    let n = name.to_lowercase();
    if at_storage_root {
        return match n.as_str() {
            "download" | "downloads" => Some("download"),
            "dcim" => Some("camera"),
            "pictures" => Some("pictures"),
            "movies" | "dcim_video" => Some("movies"),
            "music" | "audiobooks" | "podcasts" | "recordings" => Some("music"),
            "documents" => Some("documents"),
            "android" => Some("android"),
            "alarms" | "notifications" | "ringtones" => Some("bell"),
            _ => None,
        };
    }
    let p = parent.to_lowercase();
    if n == "camera" && p == "dcim" {
        return Some("camera");
    }
    if n == "screenshots" && (p == "pictures" || p == "dcim") {
        return Some("pictures");
    }
    None
}

/// `Icons.forDir`: a folder's standard icon (the plain folder if it has none).
pub fn for_dir(name: &str, parent: &str, at_storage_root: bool) -> &'static str {
    match default_key(name, parent, at_storage_root) {
        Some(key) => by_key(key),
        None => "folder",
    }
}
