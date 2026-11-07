//! Documents (DOCX, ODT, RTF) for the viewer: read into `jay_docs::doc::Document`, shown as HTML
//! (escaped in jay_docs), images served by `jaye://localhost/doc/<id>/img/<n>`, and exported to
//! DOCX, Markdown, HTML or plain text where the person picks. A DOCX can also be edited in place
//! (`docx_edit_*`, jay_docs::doc::edit): only what changed is rewritten.

use crate::files::OpenedPaths;
use jay_docs::doc::edit::{Editable, Edits, NewImage, View, open_editor};
use jay_docs::doc::{self, DocError, Document};
use jay_docs::text::Stamp;
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
pub fn doc_close(id: u32, docs: State<Documents>, editors: State<Editors>) {
    docs.open.lock().unwrap().remove(&id);
    editors.open.lock().unwrap().remove(&id);
}

/// DOCX files open in the editor: the file, its edit session, its stamp when read or saved, and the
/// pictures added since (they go into the file on save).
#[derive(Default)]
pub struct Editors {
    open: Mutex<HashMap<u32, (PathBuf, Arc<dyn Editable>, Stamp, Vec<NewImage>)>>,
}

/// A picture added to the page: its number for the save, where the page loads it, its size (points).
#[derive(Serialize)]
pub struct AddedImage {
    n: usize,
    url: String,
    w: f32,
    h: f32,
}

/// Picks a picture file and keeps it for the document `id`. `None` if the dialog was cancelled.
#[tauri::command]
pub async fn docx_edit_add_image(id: u32, app: AppHandle) -> Result<Option<AddedImage>, DocError> {
    let dir = app.state::<Editors>().open.lock().unwrap().get(&id).and_then(|e| e.0.parent().map(Path::to_path_buf));
    let mut dialog = app.dialog().file().add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff"]);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.blocking_pick_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let image = tauri::async_runtime::spawn_blocking(move || {
        if std::fs::metadata(&file)?.len() > 64 << 20 {
            return Err(DocError::TooBig);
        }
        jay_docs::doc::edit::prepare_image(&std::fs::read(&file)?)
    })
    .await
    .map_err(|e| DocError::Io { message: e.to_string() })??;
    keep_image(&app, id, image).map(Some)
}

/// A picture pasted on the page (its bytes, as the clipboard had them).
#[tauri::command]
pub async fn docx_edit_paste_image(id: u32, bytes: Vec<u8>, app: AppHandle) -> Result<AddedImage, DocError> {
    if bytes.len() > 64 << 20 {
        return Err(DocError::TooBig);
    }
    let image = tauri::async_runtime::spawn_blocking(move || jay_docs::doc::edit::prepare_image(&bytes)).await.map_err(|e| DocError::Io { message: e.to_string() })??;
    keep_image(&app, id, image)
}

/// Keeps a new picture for the save and serves it to the page.
fn keep_image(app: &AppHandle, id: u32, image: NewImage) -> Result<AddedImage, DocError> {
    let (w, h) = (image.w, image.h);
    // Served like the file's own pictures, after them.
    let docs = app.state::<Documents>();
    let mut open = docs.open.lock().unwrap();
    let (path, doc) = open.get(&id).cloned().ok_or(DocError::Io { message: "document closed".into() })?;
    let mut images = doc.images.clone();
    images.push(jay_docs::doc::Image { mime: format!("image/{}", image.ext), bytes: image.bytes.clone() });
    let url = format!("{}doc/{id}/img/{}", base_url(), images.len() - 1);
    open.insert(id, (path, Arc::new(Document { blocks: vec![], images })));
    let mut editors = app.state::<Editors>().open.lock().unwrap().get_mut(&id).map(|e| {
        e.3.push(image);
        e.3.len() - 1
    });
    let n = editors.take().ok_or(DocError::Io { message: "document closed".into() })?;
    Ok(AddedImage { n, url, w, h })
}

#[derive(Serialize)]
pub struct EditorOpened {
    id: u32,
    view: View,
}

fn stamp(path: &Path) -> Result<Stamp, DocError> {
    jay_docs::text::stamp(path).map_err(|e| DocError::Io { message: format!("{e:?}") })
}

fn open_session(path: &Path, id: u32) -> Result<(Box<dyn Editable>, Stamp), DocError> {
    let stamp = stamp(path)?;
    if stamp.len > doc::MAX_FILE_BYTES {
        return Err(DocError::TooBig);
    }
    let bytes = std::fs::read(path)?;
    Ok((open_editor(&bytes, &|n| format!("{}doc/{id}/img/{n}", base_url()))?, stamp))
}

/// Opens a DOCX or ODT in the editor. Its images are served like the viewer's (`doc/<id>/img/<n>`).
#[tauri::command]
pub async fn docx_edit_open(path: PathBuf, paths: State<'_, OpenedPaths>, docs: State<'_, Documents>, editors: State<'_, Editors>) -> Result<EditorOpened, DocError> {
    if !paths.contains(&path) {
        return Err(DocError::Io { message: "not an opened file".into() });
    }
    let id = docs.next.fetch_add(1, Ordering::Relaxed) + 1;
    let source = path.clone();
    let (session, stamp) = tauri::async_runtime::spawn_blocking(move || open_session(&source, id)).await.map_err(|e| DocError::Io { message: e.to_string() })??;
    let view = session.view().clone();
    docs.open.lock().unwrap().insert(id, (path.clone(), Arc::new(Document { blocks: vec![], images: session.images().to_vec() })));
    editors.open.lock().unwrap().insert(id, (path, Arc::from(session), stamp, vec![]));
    Ok(EditorOpened { id, view })
}

/// Saves the page's changes over the file (atomically), unless another program changed it since
/// (`force` saves anyway). Returns the saved file as the editor shows it.
#[tauri::command]
pub async fn docx_edit_save(id: u32, edits: Edits, force: bool, app: AppHandle) -> Result<View, DocError> {
    let (path, session, was, images) = app.state::<Editors>().open.lock().unwrap().get(&id).cloned().ok_or(DocError::Io { message: "document closed".into() })?;
    let target = path.clone();
    let (next, stamp) = tauri::async_runtime::spawn_blocking(move || {
        if !force && stamp(&target)? != was {
            return Err(DocError::Changed);
        }
        let bytes = session.save(&edits, &images)?;
        // What is written has to open again.
        let next = open_editor(&bytes, &|n| format!("{}doc/{id}/img/{n}", base_url()))?;
        let tmp = target.with_extension("jaye-tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &target).inspect_err(|_| drop(std::fs::remove_file(&tmp)))?;
        Ok((next, stamp(&target)?))
    })
    .await
    .map_err(|e| DocError::Io { message: e.to_string() })??;
    let view = next.view().clone();
    app.state::<Documents>().open.lock().unwrap().insert(id, (path.clone(), Arc::new(Document { blocks: vec![], images: next.images().to_vec() })));
    app.state::<Editors>().open.lock().unwrap().insert(id, (path, Arc::from(next), stamp, vec![]));
    Ok(view)
}

/// Exports the saved file (the page saves first) as another format.
#[tauri::command]
pub async fn docx_edit_export(id: u32, format: String, app: AppHandle) -> Result<Option<String>, DocError> {
    let (path, kind) = app.state::<Editors>().open.lock().unwrap().get(&id).map(|e| (e.0.clone(), e.1.format())).ok_or(DocError::Io { message: "document closed".into() })?;
    let source = path.clone();
    let document = tauri::async_runtime::spawn_blocking(move || doc::read(&source, kind)).await.map_err(|e| DocError::Io { message: e.to_string() })??;
    // The exported HTML embeds the images; the Markdown writes them beside it.
    save_as(&app, &path, &document, &format).await
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
