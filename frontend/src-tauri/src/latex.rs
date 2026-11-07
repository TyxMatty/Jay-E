//! LaTeX commands over `jay_docs::latex`: Tectonic's install state and download (on request), and
//! compiling a .tex (saved, or the editor's text) into a PDF in the cache, which opens in the PDF
//! viewer beside the editor.

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

fn data_dir(app: &AppHandle) -> Result<PathBuf, LatexError> {
    app.path().app_local_data_dir().map_err(|e| LatexError::Io { message: e.to_string() })
}

/// The cache folder for one .tex's output (the same .tex always gets the same folder).
fn out_dir(app: &AppHandle, source: &Path) -> Result<PathBuf, LatexError> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    let cache = app.path().app_cache_dir().map_err(|e| LatexError::Io { message: e.to_string() })?;
    Ok(cache.join("latex").join(format!("{:016x}", h.finish())))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    installed: bool,
    version: &'static str,
    download_mb: u64,
}

#[tauri::command]
pub fn latex_status(app: AppHandle) -> Result<Status, LatexError> {
    Ok(Status { installed: latex::tectonic_path(&data_dir(&app)?).is_file(), version: latex::TECTONIC_VERSION, download_mb: latex::TECTONIC_ZIP_BYTES.div_ceil(1 << 20) })
}

/// Downloads Tectonic (the page asks the person first).
#[tauri::command]
pub async fn latex_install(app: AppHandle) -> Result<(), LatexError> {
    let dir = data_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || latex::install(&dir).map(|_| ())).await.map_err(|e| LatexError::Io { message: e.to_string() })?
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
    let tectonic = latex::tectonic_path(&data_dir(&app)?);
    let out = out_dir(&app, &path)?;
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(previous) = compiles.0.lock().unwrap().insert(path.clone(), cancel.clone()) {
        previous.store(true, Ordering::Relaxed);
    }
    let source = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || latex::compile(&tectonic, &source, text.as_deref(), &out, &cancel))
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
