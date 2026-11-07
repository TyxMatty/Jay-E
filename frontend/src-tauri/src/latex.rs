//! LaTeX commands over `jay_docs::latex`: compiling a .tex (saved, or the editor's text) into a PDF
//! in the cache, which opens in the PDF viewer beside the editor. The engine is Jay-E's own
//! executable run again with `latex::ENGINE_ARG` (see `main`).

use crate::files::{self, OpenedFile, OpenedPaths};
use jay_docs::latex::{self, Diagnostic, LatexError};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// The running compile of each .tex, to stop it when a newer one starts.
#[derive(Default)]
pub struct Compiles(Mutex<HashMap<PathBuf, Arc<AtomicBool>>>);

/// The cache folder for one .tex's output (the same .tex always gets the same folder).
fn out_dir(app: &AppHandle, source: &Path) -> Result<PathBuf, LatexError> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    let cache = app.path().app_cache_dir().map_err(|e| LatexError::Io { message: e.to_string() })?;
    Ok(cache.join("latex").join(format!("{:016x}", h.finish())))
}

#[derive(Serialize)]
pub struct Compiled {
    /// The PDF, opened like a file the person opened (so the PDF viewer may read it).
    pdf: Option<OpenedFile>,
    diagnostics: Vec<Diagnostic>,
    seconds: f64,
}

/// Compiles `path`, or `text` (its unsaved version) when given. A newer compile of the same file
/// stops this one.
#[tauri::command]
pub async fn latex_compile(path: PathBuf, text: Option<String>, app: AppHandle, compiles: State<'_, Compiles>) -> Result<Compiled, LatexError> {
    if !app.state::<OpenedPaths>().contains(&path) {
        return Err(LatexError::Io { message: "not an opened file".into() });
    }
    let engine = std::env::current_exe()?;
    let out = out_dir(&app, &path)?;
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(previous) = compiles.0.lock().unwrap().insert(path.clone(), cancel.clone()) {
        previous.store(true, Ordering::Relaxed);
    }
    let source = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || latex::compile(&engine, &source, text.as_deref(), &out, &cancel))
        .await
        .map_err(|e| LatexError::Io { message: e.to_string() })??;
    let pdf = result.pdf.and_then(|p| files::open(&app, [p]).into_iter().next());
    Ok(Compiled { pdf, diagnostics: result.diagnostics, seconds: result.seconds })
}

#[tauri::command]
pub fn latex_cancel(path: PathBuf, compiles: State<Compiles>) {
    if let Some(flag) = compiles.0.lock().unwrap().get(&path) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Copies the last compiled PDF where the person picks. `None` if cancelled.
#[tauri::command]
pub async fn latex_save_pdf(path: PathBuf, app: AppHandle) -> Result<Option<String>, LatexError> {
    if !app.state::<OpenedPaths>().contains(&path) {
        return Err(LatexError::Io { message: "not an opened file".into() });
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let pdf = out_dir(&app, &path)?.join(format!("{stem}.pdf"));
    if !pdf.is_file() {
        return Err(LatexError::Io { message: "compile it first".into() });
    }
    let mut dialog = app.dialog().file().set_file_name(format!("{stem}.pdf")).add_filter("PDF", &["pdf"]);
    if let Some(dir) = path.parent() {
        dialog = dialog.set_directory(dir);
    }
    let Some(target) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::copy(&pdf, &target)?;
    Ok(Some(target.to_string_lossy().into_owned()))
}
