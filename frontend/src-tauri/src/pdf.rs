//! The PDF viewer's side in Rust: commands over `jay_docs::pdf`, and the `jaye://` protocol that
//! hands drawn pages to the page as fast PNGs (no base64 in the IPC).
//!
//! Page bitmaps: GET `jaye://localhost/pdf/<doc>/<page>/<width>` (`?thumb` for thumbnails), on
//! Windows `http://jaye.localhost/…`. A PNG, or 204 when there's nothing to draw (a page no
//! longer on screen, or a page that failed).

use crate::files::OpenedPaths;
use jay_docs::pdf::{Link, OpenError, PdfDocument, SearchEvent, TextLine};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::http::{Request, Response, StatusCode};
use tauri::ipc::Channel;
use tauri::{Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

#[derive(Default)]
pub struct Docs {
    open: Mutex<HashMap<u32, Arc<PdfDocument>>>,
    next: AtomicU32,
}

impl Docs {
    fn get(&self, doc: u32) -> Result<Arc<PdfDocument>, String> {
        self.open.lock().unwrap().get(&doc).cloned().ok_or_else(|| "document closed".into())
    }
}

#[derive(Serialize)]
pub struct Opened {
    doc: u32,
    /// [width, height] of each page in points.
    pages: Vec<[f32; 2]>,
}

/// Opens a PDF the person opened before (files.rs): the page can't name any other path.
#[tauri::command]
pub async fn pdf_open(path: PathBuf, password: Option<String>, opened: State<'_, OpenedPaths>, docs: State<'_, Docs>) -> Result<Opened, OpenError> {
    if !opened.contains(&path) {
        return Err(OpenError::Io("not an opened file".into()));
    }
    let doc = tauri::async_runtime::spawn_blocking(move || PdfDocument::open(&path, password.as_deref()))
        .await
        .map_err(|e| OpenError::Io(e.to_string()))??;
    let id = docs.next.fetch_add(1, Ordering::Relaxed) + 1;
    let pages = doc.info.pages.clone();
    docs.open.lock().unwrap().insert(id, Arc::new(doc));
    Ok(Opened { doc: id, pages })
}

#[tauri::command]
pub fn pdf_close(doc: u32, docs: State<Docs>) {
    if let Some(d) = docs.open.lock().unwrap().remove(&doc) {
        d.cancel_search();
    }
}

#[tauri::command]
pub fn pdf_visible(doc: u32, first: usize, last: usize, docs: State<Docs>) -> Result<(), String> {
    docs.get(doc)?.set_visible(first, last);
    Ok(())
}

#[tauri::command]
pub async fn pdf_text(doc: u32, page: usize, docs: State<'_, Docs>) -> Result<Vec<TextLine>, String> {
    let rx = docs.get(doc)?.text(page);
    tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or_default()).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pdf_links(doc: u32, page: usize, docs: State<'_, Docs>) -> Result<Vec<Link>, String> {
    let rx = docs.get(doc)?.links(page);
    tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or_default()).await.map_err(|e| e.to_string())
}

/// Streams the hits to `channel` page by page; a new search (or `pdf_cancel_search`) stops it.
#[tauri::command]
pub fn pdf_search(doc: u32, query: String, channel: Channel<SearchEvent>, docs: State<Docs>) -> Result<(), String> {
    docs.get(doc)?.search(&query, Box::new(move |event| drop(channel.send(event))));
    Ok(())
}

#[tauri::command]
pub fn pdf_cancel_search(doc: u32, docs: State<Docs>) -> Result<(), String> {
    docs.get(doc)?.cancel_search();
    Ok(())
}

/// `jaye://` requests: parsed here, drawn on the document's thread, answered from a waiting thread.
pub fn protocol<R: Runtime>(ctx: UriSchemeContext<'_, R>, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let reply = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header("Content-Type", "image/png")
            // Dev pages come from http://localhost:5177, a different origin.
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "no-store")
            .body(body)
            .unwrap()
    };
    let Some((doc, page, width, thumb)) = parse_page_path(request.uri().path(), request.uri().query()) else {
        return responder.respond(reply(StatusCode::NOT_FOUND, vec![]));
    };
    let Ok(doc) = ctx.app_handle().state::<Docs>().get(doc) else {
        return responder.respond(reply(StatusCode::NOT_FOUND, vec![]));
    };
    let rx = doc.render(page, width, thumb);
    std::thread::spawn(move || {
        responder.respond(match rx.recv().ok().flatten() {
            Some(bmp) => reply(StatusCode::OK, bmp.to_png()),
            None => reply(StatusCode::NO_CONTENT, vec![]),
        })
    });
}

/// `/pdf/<doc>/<page>/<width>` and `thumb` in the query.
fn parse_page_path(path: &str, query: Option<&str>) -> Option<(u32, usize, u32, bool)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "pdf" {
        return None;
    }
    let doc = parts.next()?.parse().ok()?;
    let page = parts.next()?.parse().ok()?;
    let width = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((doc, page, width, query.is_some_and(|q| q.split('&').any(|p| p == "thumb"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_paths() {
        assert_eq!(parse_page_path("/pdf/3/12/1240", None), Some((3, 12, 1240, false)));
        assert_eq!(parse_page_path("/pdf/3/0/160", Some("thumb")), Some((3, 0, 160, true)));
        assert_eq!(parse_page_path("/pdf/3/0", None), None);
        assert_eq!(parse_page_path("/pdf/3/0/160/x", None), None);
        assert_eq!(parse_page_path("/pdf/a/0/160", None), None);
        assert_eq!(parse_page_path("/img/3/0/160", None), None);
    }
}
