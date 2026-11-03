//! kiln-rs: decoder-side Rust port of the Kiln (.kln, "KLNA") engine.
//! Port of kiln_v14.py + its C helper. Verified only through the Python mirrors in tests/mirror
//! until a Rust toolchain is available; see HANDOFF.md.
pub mod app;
pub mod bcj;
pub mod ffi;
pub mod container;
pub mod crypto;
pub mod mp3;
pub mod mp3enc;
pub mod mp3parse;
pub mod mp3_tables;
pub mod lz;
pub mod pack;
pub mod packapp;
pub mod packeach;
pub mod packer;
pub mod rc;
pub mod rnd;
pub mod sets;
pub mod walk;
pub mod wav;
