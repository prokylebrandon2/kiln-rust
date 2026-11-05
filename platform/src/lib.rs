//! The Android side of Kiln.
//!
//! * [`android_main`] is the entry point `NativeActivity` calls (Android only).
//! * [`AndroidHost`](android::AndroidHost) answers the questions `kiln-app` asks the phone, through
//!   JNI and the NDK.
//!
//! This is the one crate in the workspace where `unsafe` is allowed. Every `unsafe` block says why
//! it is sound; the rest of the workspace keeps `#![forbid(unsafe_code)]`.
//!
//! On other targets the crate is empty, so the workspace still builds and lints on a desktop.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "android")]
mod android;
