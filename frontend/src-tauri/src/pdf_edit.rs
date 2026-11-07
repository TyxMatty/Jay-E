//! Editing an open PDF (jay_docs::pdf_edit): each change makes new bytes, opened as a new document
//! for the viewer (the old one closes); undo goes back a version; saving writes the current one over
//! the file, unless another program changed it since it was read.

use crate::pdf::Docs;
use jay_docs::pdf::PdfDocument;
use jay_docs::pdf_edit::{self, AnnotInfo, EditError, Field, Op};
use jay_docs::text::Stamp;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

/// Versions kept for undo; fewer for big files.
const UNDO_STEPS: usize = 30;
const BIG_FILE: usize = 50 << 20;

/// An edited PDF: its file, the stamp it had when read or saved, and its versions (the last is
/// what's shown); `saved` is the version on disk.
struct Session {
    path: PathBuf,
    stamp: Stamp,
    versions: Vec<Arc<Vec<u8>>>,
    saved: usize,
}

/// Edited PDFs, by the viewer's current document id.
#[derive(Default)]
pub struct PdfEdits {
    open: Mutex<HashMap<u32, Session>>,
    /// The file each viewer document came from (pdf_open records it).
    pub paths: Mutex<HashMap<u32, PathBuf>>,
}

#[derive(Serialize)]
pub struct Edited {
    doc: u32,
    pages: Vec<[f32; 2]>,
    can_undo: bool,
    dirty: bool,
}

fn err(e: impl std::fmt::Display) -> EditError {
    EditError::Invalid { message: e.to_string() }
}

fn stamp(path: &Path) -> Result<Stamp, EditError> {
    jay_docs::text::stamp(path).map_err(|e| err(format!("{e:?}")))
}

/// The session of `doc`, started from the file on first use.
fn take(edits: &PdfEdits, doc: u32) -> Result<Session, EditError> {
    if let Some(s) = edits.open.lock().unwrap().remove(&doc) {
        return Ok(s);
    }
    let path = edits.paths.lock().unwrap().get(&doc).cloned().ok_or_else(|| err("document closed"))?;
    let stamp = stamp(&path)?;
    let bytes = std::fs::read(&path).map_err(err)?;
    Ok(Session { path, stamp, versions: vec![Arc::new(bytes)], saved: 0 })
}

/// Shows `session`'s current version as a new document in place of `doc`.
fn show(app: &AppHandle, doc: u32, session: Session) -> Result<Edited, EditError> {
    let bytes = session.versions.last().cloned().ok_or_else(|| err("no version"))?;
    let pdf = PdfDocument::from_bytes(bytes, "").map_err(|e| err(format!("{e:?}")))?;
    let pages = pdf.info.pages.clone();
    let docs = app.state::<Docs>();
    let id = docs.add(pdf);
    docs.close(doc);
    let edits = app.state::<PdfEdits>();
    let path = session.path.clone();
    edits.paths.lock().unwrap().remove(&doc);
    edits.paths.lock().unwrap().insert(id, path);
    let edited = Edited { doc: id, pages, can_undo: session.versions.len() > 1, dirty: session.versions.len() - 1 != session.saved };
    edits.open.lock().unwrap().insert(id, session);
    Ok(edited)
}

/// Applies `change` to the current version and shows the result.
async fn change(app: AppHandle, doc: u32, change: impl FnOnce(&[u8]) -> Result<Vec<u8>, EditError> + Send + 'static) -> Result<Edited, EditError> {
    let edits = app.state::<PdfEdits>();
    let mut session = take(&edits, doc)?;
    let current = session.versions.last().cloned().ok_or_else(|| err("no version"))?;
    let result = tauri::async_runtime::spawn_blocking(move || change(&current)).await.map_err(err)?;
    match result {
        Ok(bytes) => {
            session.versions.push(Arc::new(bytes));
            let keep = if session.versions.last().is_some_and(|b| b.len() > BIG_FILE) { 3 } else { UNDO_STEPS };
            while session.versions.len() > keep {
                session.versions.remove(0);
                // The saved version fell out of the history: none of the kept ones is on disk.
                session.saved = session.saved.checked_sub(1).unwrap_or(usize::MAX);
            }
            show(&app, doc, session)
        }
        Err(e) => {
            edits.open.lock().unwrap().insert(doc, session);
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn pdf_edit(doc: u32, op: Op, app: AppHandle) -> Result<Edited, EditError> {
    change(app, doc, move |bytes| pdf_edit::apply(bytes, &op)).await
}

#[tauri::command]
pub fn pdf_edit_undo(doc: u32, app: AppHandle) -> Result<Edited, EditError> {
    let edits = app.state::<PdfEdits>();
    let mut session = take(&edits, doc)?;
    if session.versions.len() > 1 {
        session.versions.pop();
    }
    show(&app, doc, session)
}

/// Puts another PDF's pages in at `at` (the person picks the file). `None` if they cancel.
#[tauri::command]
pub async fn pdf_edit_insert(doc: u32, at: usize, app: AppHandle) -> Result<Option<Edited>, EditError> {
    let dir = app.state::<PdfEdits>().paths.lock().unwrap().get(&doc).and_then(|p| p.parent().map(Path::to_path_buf));
    let mut dialog = app.dialog().file().add_filter("PDF", &["pdf"]);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.blocking_pick_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let other = std::fs::read(&file).map_err(err)?;
    change(app, doc, move |bytes| pdf_edit::insert(bytes, &other, at)).await.map(Some)
}

/// Saves the chosen pages as a new PDF where the person picks. Returns its path, `None` if cancelled.
#[tauri::command]
pub async fn pdf_edit_extract(doc: u32, pages: Vec<usize>, app: AppHandle) -> Result<Option<String>, EditError> {
    let edits = app.state::<PdfEdits>();
    let session = take(&edits, doc)?;
    let current = session.versions.last().cloned().ok_or_else(|| err("no version"))?;
    let path = session.path.clone();
    edits.open.lock().unwrap().insert(doc, session);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let mut dialog = app.dialog().file().set_file_name(format!("{stem} (pages).pdf")).add_filter("PDF", &["pdf"]);
    if let Some(dir) = path.parent() {
        dialog = dialog.set_directory(dir);
    }
    let Some(target) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let bytes = tauri::async_runtime::spawn_blocking(move || pdf_edit::extract(&current, &pages)).await.map_err(err)??;
    write_atomic(&target, &bytes)?;
    Ok(Some(target.to_string_lossy().into_owned()))
}

/// Writes the current version over the file. `force` saves even if another program changed it.
#[tauri::command]
pub async fn pdf_edit_save(doc: u32, force: bool, app: AppHandle) -> Result<Edited, EditError> {
    let edits = app.state::<PdfEdits>();
    let mut session = take(&edits, doc)?;
    let result = (|| {
        if !force && stamp(&session.path)? != session.stamp {
            return Err(EditError::Invalid { message: "changed".into() });
        }
        let bytes = session.versions.last().cloned().ok_or_else(|| err("no version"))?;
        write_atomic(&session.path, &bytes)?;
        session.stamp = stamp(&session.path)?;
        session.saved = session.versions.len() - 1;
        Ok(())
    })();
    let edited = Edited { doc, pages: vec![], can_undo: session.versions.len() > 1, dirty: session.versions.len() - 1 != session.saved };
    edits.open.lock().unwrap().insert(doc, session);
    result.map(|_| edited)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), EditError> {
    let tmp = path.with_extension("pdf.jaye-tmp");
    std::fs::write(&tmp, bytes).map_err(err)?;
    std::fs::rename(&tmp, path).inspect_err(|_| drop(std::fs::remove_file(&tmp))).map_err(err)
}

/// The bytes the viewer shows for `doc`: the edited version, or the file.
fn current(app: &AppHandle, doc: u32) -> Result<Arc<Vec<u8>>, EditError> {
    let edits = app.state::<PdfEdits>();
    if let Some(s) = edits.open.lock().unwrap().get(&doc) {
        return s.versions.last().cloned().ok_or_else(|| err("no version"));
    }
    let path = edits.paths.lock().unwrap().get(&doc).cloned().ok_or_else(|| err("document closed"))?;
    std::fs::read(path).map(Arc::new).map_err(err)
}

#[tauri::command]
pub async fn pdf_annotations(doc: u32, app: AppHandle) -> Result<Vec<Vec<AnnotInfo>>, EditError> {
    let bytes = current(&app, doc)?;
    tauri::async_runtime::spawn_blocking(move || pdf_edit::annotations(&bytes)).await.map_err(err)?
}

#[tauri::command]
pub async fn pdf_fields(doc: u32, app: AppHandle) -> Result<Vec<Field>, EditError> {
    let bytes = current(&app, doc)?;
    tauri::async_runtime::spawn_blocking(move || pdf_edit::fields(&bytes)).await.map_err(err)?
}

/// Forgets a closed document's edits (pdf_close).
pub fn forget(app: &AppHandle, doc: u32) {
    app.state::<PdfEdits>().open.lock().unwrap().remove(&doc);
    app.state::<PdfEdits>().paths.lock().unwrap().remove(&doc);
}
