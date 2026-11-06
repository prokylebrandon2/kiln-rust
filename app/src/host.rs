//! What the app needs from the phone, as a trait.
//!
//! The `platform` crate is the only one allowed to use `unsafe`, and it is the crate that holds
//! the Android entry point, so it depends on this one and implements [`Host`] for the real phone.
//! Everything here is therefore callable without knowing anything about Android, JNI or the NDK.
//!
//! All methods are called on the UI thread (the one running the Slint event loop).

use std::path::{Path, PathBuf};

/// What the system tells the UI about how to look: the colour scheme and the two user scales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The system is in dark mode (`res/values-night` in the Java app).
    pub dark: bool,
    /// Display density, 1.0 at 160 dpi (`DisplayMetrics.density`).
    pub density: f32,
    /// Settings > Display > Font size (`Configuration.fontScale`); sp = dp * font_scale.
    pub font_scale: f32,
}

/// A mounted removable volume: a USB drive or an SD card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Where it is mounted (`StorageVolume.getDirectory()`).
    pub path: PathBuf,
    /// What the phone calls it (`StorageVolume.getDescription()`, or the folder's name).
    pub label: String,
}

/// Things that happen to the app while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The app came to the front (`onResume`): the right moment to look at permissions again.
    Resumed,
    /// The app left the front (`onPause`).
    Paused,
    /// Dark mode, font size, density or the like changed: read [`Host::look`] again.
    ConfigChanged,
    /// The system is short of memory.
    LowMemory,
}

/// An app that can open a file (`ResolveInfo` of `PackageManager.queryIntentActivities`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenApp {
    /// The package name, e.g. `com.example.viewer`.
    pub package: String,
    /// What the phone calls it (`ResolveInfo.loadLabel`).
    pub label: String,
}

/// What another app asked Kiln to open (the data of an `ACTION_VIEW` intent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// A `file://` URI of a file that exists: it is opened where it is.
    File(PathBuf),
    /// A `content://` URI. `name` is its display name (`OpenableColumns.DISPLAY_NAME`, else the
    /// last path segment, else `archive.kiln`, with `/` replaced by `_`); [`Host::copy_incoming`]
    /// reads the content.
    Shared { name: String },
}

/// Where [`Host::open_file`] sends a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenWith {
    /// Straight to one app (`Intent.setPackage`).
    Package(String),
    /// The system's own "Open with" list (`Intent.createChooser`, the "More options..." row).
    Chooser,
}

/// The phone, as far as the app is concerned.
pub trait Host {
    /// The app's cache folder (`Context.getCacheDir()`).
    fn cache_dir(&self) -> PathBuf;
    /// The app's private files folder (`Context.getFilesDir()`).
    fn files_dir(&self) -> PathBuf;
    /// The top of the phone's own storage (`Environment.getExternalStorageDirectory()`), where
    /// the file browser starts.
    fn storage_root(&self) -> PathBuf;
    /// How many seconds the phone's time zone is ahead of UTC at a time given in epoch seconds
    /// (`TimeZone.getDefault().getOffset(ms)`), for the dates in the file list.
    fn utc_offset(&self, epoch_seconds: i64) -> i32;
    /// The USB drives and SD cards that are plugged in and mounted, as the file browser lists them.
    /// Empty when the phone cannot say.
    fn volumes(&self) -> Vec<Volume>;
    /// `(free, total)` bytes of the storage that holds `path` (`StatFs.getAvailableBytes()` and
    /// `getTotalBytes()`), or `None` when the phone cannot say.
    fn space(&self, path: &Path) -> Option<(u64, u64)>;
    /// The version shown in crash reports and About.
    fn version_name(&self) -> String;
    /// The build number shown in About (`PackageInfo.getLongVersionCode()`), 0 when unknown.
    fn version_code(&self) -> i64;
    /// The current colour scheme, density and font scale.
    fn look(&self) -> Look;
    /// Whether "All files access" is granted (always true before Android 11).
    fn has_all_files_access(&self) -> bool;
    /// Open the system page where the user grants "All files access" to Kiln.
    fn open_all_files_settings(&self);
    /// Put `text` on the system clipboard (`ClipboardManager.setPrimaryClip`).
    fn set_clipboard(&self, text: &str);
    /// Close the app's screen (`Activity.finish()`).
    fn finish(&self);
    /// Replace the handler that is told about [`Event`]s. It runs on the UI thread.
    fn set_event_handler(&self, handler: Box<dyn Fn(Event)>);

    // ---- files other apps send to Kiln (the defaults are the non-Android stub) --------------

    /// The file another app asked Kiln to open when it started it (`ACTION_VIEW` with a `file:`
    /// or `content:` URI), or `None`. Only the launch intent is seen: a later one is not.
    fn incoming(&self) -> Option<Incoming> {
        None
    }
    /// Copy the content of an [`Incoming::Shared`] into `dest`. The error is shown to the person.
    fn copy_incoming(&self, _dest: &Path) -> Result<(), String> {
        Err("can't read that file".to_string())
    }
    /// Copy an incoming file without blocking the UI thread. Android overrides this to use a
    /// thread-attached JNI environment; other hosts use the synchronous fallback.
    fn copy_incoming_async(&self, dest: PathBuf, done: Box<dyn FnOnce(Result<(), String>) + Send>) {
        done(self.copy_incoming(&dest));
    }

    // ---- opening files in other apps (the defaults are the non-Android stub) ----------------

    /// The MIME type Android knows for a file extension, without the dot
    /// (`MimeTypeMap.getMimeTypeFromExtension`), or `None` when it does not know one.
    fn mime_for_extension(&self, _ext: &str) -> Option<String> {
        None
    }
    /// The apps that can open (or, with `edit`, edit) this file as `mime`, in the phone's order.
    /// Kiln itself and repeated packages are left out. Empty when the phone cannot say.
    fn open_apps(&self, _path: &Path, _mime: &str, _edit: bool) -> Vec<OpenApp> {
        Vec::new()
    }
    /// Hand the file to another app: `ACTION_VIEW`, or `ACTION_EDIT` with `edit`. False when no
    /// app could take it.
    fn open_file(&self, _path: &Path, _mime: &str, _edit: bool, _with: &OpenWith) -> bool {
        false
    }
}
