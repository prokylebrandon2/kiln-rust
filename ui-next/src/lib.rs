//! Kiln's new user interface, written in Slint (`ui-next/ui/*.slint`).
//!
//! Colours, sizes and type live in `tokens.slint`; the controls in `controls.slint`; each screen
//! in its own file; and what Rust tells a screen (and the screen tells Rust) in one global per
//! screen in `state.slint`. The design they implement is `docs/design/home-dark.png`; the plan is
//! `docs/UI-REWRITE.md`.
//!
//! This crate holds only the generated code. It is `deny`, not `forbid`, because the generated
//! items carry their own `allow` attributes.

#![deny(unsafe_code)]

slint::include_modules!();
