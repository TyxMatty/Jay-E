//! Video: cutting and joining MP4/MOV by stream copy in Rust (mp4.rs), and converting (decoding and
//! encoding) with Windows' Media Foundation (mf.rs), which comes with Windows: 0 MB in the installer.
//! Playing, thumbnails and frame grabs are the WebView's `<video>` (hardware decoding).

#[cfg(windows)]
pub mod mf;
pub mod mp4;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VideoError {
    TooBig,
    Unsupported { what: String },
    Invalid { message: String },
    /// A file to join differs from the first (codec, size or settings).
    Mismatch { file: String },
    Cancelled,
    Io { message: String },
}

impl From<std::io::Error> for VideoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io { message: e.to_string() }
    }
}
