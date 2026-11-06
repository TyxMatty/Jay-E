//! Jay-E's engine. No Tauri here, so it is tested with `cargo test` alone (docs/ARQUITETURA.md).

pub mod detect;
pub mod markdown;
pub mod pdf;
pub mod text;

pub use detect::{Kind, detect, detect_bytes};
