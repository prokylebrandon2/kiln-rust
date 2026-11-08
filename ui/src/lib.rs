//! Kiln's user interface, written in Slint (`ui/ui/*.slint`).
//!
//! Every view (top bar, list row, sheet, buttons, ...) is a small custom component in
//! `components.slint`; colours, sizes and timings live in the `Theme` global in `theme.slint`.
//!
//! This crate holds nothing but the generated code, so there is nothing to put under
//! `forbid(unsafe_code)` that Slint's code generator does not already control. It is `deny`, not
//! `forbid`, because the generated items carry their own `allow` attributes.

#![deny(unsafe_code)]

slint::include_modules!();
