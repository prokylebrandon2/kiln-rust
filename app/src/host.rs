//! What the app needs from the phone, as a trait.
//!
//! The `platform` crate is the only one allowed to use `unsafe`, and it is the crate that holds
//! the Android entry point, so it depends on this one and implements [`Host`] for the real phone.
//! Everything here is therefore callable without knowing anything about Android, JNI or the NDK.
//!
//! All methods are called on the UI thread (the one running the Slint event loop).

use std::path::PathBuf;

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
    /// The version shown in crash reports and About.
    fn version_name(&self) -> String;
    /// The current colour scheme, density and font scale.
    fn look(&self) -> Look;
    /// Whether "All files access" is granted (always true before Android 11).
    fn has_all_files_access(&self) -> bool;
    /// Open the system page where the user grants "All files access" to Kiln.
    fn open_all_files_settings(&self);
    /// Close the app's screen (`Activity.finish()`).
    fn finish(&self);
    /// Replace the handler that is told about [`Event`]s. It runs on the UI thread.
    fn set_event_handler(&self, handler: Box<dyn Fn(Event)>);
}
