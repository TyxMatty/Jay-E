//! Jay-E's engine. No Tauri here, so it is tested with `cargo test` alone (docs/ARQUITETURA.md).

pub mod detect;
pub mod doc;
pub mod image;
pub mod latex;
pub mod markdown;
pub mod pdf;
pub mod pdf_edit;
pub mod text;
pub mod video;

pub use detect::{Kind, detect, detect_bytes};
