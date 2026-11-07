//! Documents (DOCX, ODT, RTF) for the viewer: read into `jay_docs::doc::Document`, shown as HTML
//! (escaped in jay_docs), images served by `jaye://localhost/doc/<id>/img/<n>`, and exported to
//! DOCX, Markdown, HTML or plain text where the person picks.

use crate::files::OpenedPaths;
use jay_docs::doc::{self, DocError, Document};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub struct Documents {
    open: Mutex<HashMap<u32, (PathBuf, Arc<Document>)>>,
    next: AtomicU32,
}

impl Documents {
    fn get(&self, id: u32) -> Result<(PathBuf, Arc<Document>), DocError> {
        self.open.lock().unwrap().get(&id).cloned().ok_or(DocError::Io { message: "document closed".into() })
    }
}

#[derive(Serialize)]
pub struct Opened {
    id: u32,
    html: String,
    images: usize,
}

/// `jaye://localhost/` on macOS and Linux, `http://jaye.localhost/` on Windows.
fn base_url() -> &'static str {
    if cfg!(windows) { "http://jaye.localhost/" } else { "jaye://localhost/" }
}

#[tauri::command]
pub async fn doc_open(path: PathBuf, format: String, paths: State<'_, OpenedPaths>, docs: State<'_, Documents>) -> Result<Opened, DocError> {
    if !paths.contains(&path) {
        return Err(DocError::Io { message: "not an opened file".into() });
    }
    let source = path.clone();
    let document = tauri::async_runtime::spawn_blocking(move || doc::read(&source, &format)).await.map_err(|e| DocError::Io { message: e.to_string() })??;
    let id = docs.next.fetch_add(1, Ordering::Relaxed) + 1;
    let html = doc::html::to_html(&document, &|n| format!("{}doc/{id}/img/{n}", base_url()));
    let images = document.images.len();
    docs.open.lock().unwrap().insert(id, (path, Arc::new(document)));
    Ok(Opened { id, html, images })
}

#[tauri::command]
pub fn doc_close(id: u32, docs: State<Documents>) {
    docs.open.lock().unwrap().remove(&id);
}

/// Exports the open document as `format` ("docx", "markdown", "html", "txt") where the person
/// picks. Returns the saved path, or `None` if they cancel.
#[tauri::command]
pub async fn doc_export(id: u32, format: String, app: AppHandle) -> Result<Option<String>, DocError> {
    let (path, document) = app.state::<Documents>().get(id)?;
    save_as(&app, &path, &document, &format).await
}

/// Shared with the Markdown editor's "Export DOCX" (text.rs).
pub async fn save_as(app: &AppHandle, source: &Path, document: &Document, format: &str) -> Result<Option<String>, DocError> {
    let ext = match format {
        "docx" => "docx",
        "markdown" => "md",
        "html" => "html",
        "txt" => "txt",
        _ => return Err(DocError::Unsupported),
    };
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let mut dialog = app.dialog().file().set_file_name(format!("{stem}.{ext}")).add_filter(ext.to_uppercase(), &[ext]);
    if let Some(dir) = source.parent() {
        dialog = dialog.set_directory(dir);
    }
    // The save dialog asks before replacing the file itself.
    let Some(target) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let (bytes, images) = match format {
        "docx" => (doc::docx::write(document)?, vec![]),
        "markdown" => {
            let dir = target.parent().unwrap_or(Path::new("."));
            let base = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
            // Image files beside the .md never replace existing files: another prefix if needed.
            let mut prefix = base.clone();
            let mut n = 2;
            let (text, images) = loop {
                let (text, images) = doc::markdown::write(document, &prefix);
                if images.iter().all(|(name, _)| !dir.join(name).exists()) || n > 999 {
                    break (text, images);
                }
                prefix = format!("{base}-{n}");
                n += 1;
            };
            (text.into_bytes(), images)
        }
        "html" => {
            let body = doc::html::to_html(document, &|n| data_url(document, n));
            let title = doc::html::escape(&stem);
            (format!("<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n<style>{}</style>\n</head>\n<body>\n<main class=\"markdown\">\n{body}</main>\n</body>\n</html>\n", crate::text::EXPORT_CSS).into_bytes(), vec![])
        }
        _ => (doc::text::write(document).into_bytes(), vec![]),
    };
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        for (name, data) in images {
            std::fs::write(dir.join(name), data)?;
        }
        let tmp = target.with_extension(format!("{ext}.jaye-tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &target).inspect_err(|_| drop(std::fs::remove_file(&tmp)))?;
        Ok(Some(target.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| DocError::Io { message: e.to_string() })?
}

/// An image as a data: URL, for the exported HTML file (one file, nothing beside it).
fn data_url(document: &Document, n: usize) -> String {
    use std::fmt::Write;
    let Some(img) = document.images.get(n) else { return String::new() };
    // Base64 by hand: a few lines, no new dependency.
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = format!("data:{};base64,", img.mime);
    for chunk in img.bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                let _ = out.write_char(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `jaye://localhost/doc/<id>/img/<n>`: an image of an open document, as stored in the file.
pub fn respond<R: Runtime>(app: &AppHandle<R>, parts: &[&str]) -> Response<Vec<u8>> {
    let not_found = || crate::pdf::response(StatusCode::NOT_FOUND, vec![]);
    let (Some(id), Some(&"img"), Some(n)) = (parts.first().and_then(|p| p.parse::<u32>().ok()), parts.get(1), parts.get(2).and_then(|p| p.parse::<usize>().ok())) else {
        return not_found();
    };
    let Ok((_, document)) = app.state::<Documents>().get(id) else { return not_found() };
    let Some(img) = document.images.get(n) else { return not_found() };
    let mut response = crate::pdf::response(StatusCode::OK, img.bytes.clone());
    if let Ok(value) = img.mime.parse() {
        response.headers_mut().insert("Content-Type", value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use jay_docs::doc::Image;

    #[test]
    fn base64_matches_the_standard() {
        let doc = |bytes: &[u8]| Document { blocks: vec![], images: vec![Image { mime: "image/png".into(), bytes: bytes.to_vec() }] };
        for (input, expected) in [(&b""[..], ""), (b"f", "Zg=="), (b"fo", "Zm8="), (b"foo", "Zm9v"), (b"foobar", "Zm9vYmFy")] {
            assert_eq!(data_url(&doc(input), 0), format!("data:image/png;base64,{expected}"));
        }
    }
}
