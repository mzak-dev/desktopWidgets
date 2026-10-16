//! Raywright drives a real Wayfinder with the Automation Bridge on (ADR-0013).
//!
//! Build it first: `cargo build --features automation`, then
//! `cargo test --manifest-path e2e/Cargo.toml -- --test-threads=1` (one desktop, one test at a
//! time). `WAYFINDER_EXE` points at another build.

mod animations;
mod edit_mode;
mod settings;
mod support;
mod widgets;
