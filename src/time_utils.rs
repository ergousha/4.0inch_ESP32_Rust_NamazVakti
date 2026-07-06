//! Re-exports the pure calendar/timezone arithmetic from the
//! `namaz-vakti-logic` crate (see `logic/src/time_utils.rs`), which has no
//! ESP-IDF/hardware dependency and is unit tested with a plain host Rust
//! toolchain in CI (`.github/workflows/rust.yml`).

pub use namaz_vakti_logic::time_utils::*;
