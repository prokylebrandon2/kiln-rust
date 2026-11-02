//! kiln-rs: decoder-side Rust port of the Kiln (.kln, "KLNA") engine.
//! Port of kiln_v14.py + its C helper. Verified only through the Python mirrors in tests/mirror
//! until a Rust toolchain is available; see HANDOFF.md.
pub mod app;
pub mod bcj;
pub mod ffi;
pub mod container;
pub mod crypto;
pub mod rc;
pub mod sets;
pub mod wav;
