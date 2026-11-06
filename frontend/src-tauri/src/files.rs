//! Opening files: the dialog, a drop on the window, or the command line ("Open with", a second
//! launch). The page never names a path: Rust gets it, detects the type, and lets the asset protocol
//! serve that one file (its scope starts empty, tauri.conf.json).

use jay_docs::Kind;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// The event the page listens to for files opened by a drop or a second launch.
pub const OPENED_EVENT: &str = "files-opened";

#[derive(Debug, Clone, Serialize)]
pub struct OpenedFile {
    path: String,
    name: String,
    kind: Kind,
    format: &'static str,
    size: u64,
    /// Why it can't be shown (unreadable, a folder…). The page shows it instead of a viewer.
    error: Option<String>,
}

/// Files from the command line of the first launch, until the page asks for them.
#[derive(Default)]
pub struct StartupFiles(pub Mutex<Vec<OpenedFile>>);

pub fn open(app: &AppHandle, paths: impl IntoIterator<Item = PathBuf>) -> Vec<OpenedFile> {
    paths.into_iter().map(|path| open_one(app, path)).collect()
}

fn open_one(app: &AppHandle, path: PathBuf) -> OpenedFile {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut file = OpenedFile { path: path.to_string_lossy().into_owned(), name, kind: Kind::Unknown, format: "", size: 0, error: None };
    let result = std::fs::metadata(&path).and_then(|meta| {
        if !meta.is_file() {
            return Err(std::io::Error::other("not a file"));
        }
        file.size = meta.len();
        jay_docs::detect(&path)
    });
    match result {
        Ok((kind, format)) => {
            file.kind = kind;
            file.format = format;
            if let Err(e) = app.asset_protocol_scope().allow_file(&path) {
                file.error = Some(e.to_string());
            }
        }
        Err(e) => file.error = Some(e.to_string()),
    }
    file
}

/// The paths in a command line: everything after the program that isn't a `--flag`, relative to `cwd`.
pub fn paths_from_args(args: &[String], cwd: &Path) -> Vec<PathBuf> {
    args.iter().skip(1).filter(|a| !a.starts_with("--")).map(|a| cwd.join(a)).collect()
}

/// A drop on the window or a second launch: tell the page.
pub fn announce(app: &AppHandle, paths: Vec<PathBuf>) {
    let files = open(app, paths);
    if !files.is_empty() {
        let _ = app.emit(OPENED_EVENT, files);
    }
}

#[tauri::command]
pub async fn pick_files(app: AppHandle) -> Vec<OpenedFile> {
    // async: the blocking dialog runs off the main thread.
    let picked = app.dialog().file().blocking_pick_files().unwrap_or_default();
    open(&app, picked.into_iter().filter_map(|p| p.into_path().ok()))
}

#[tauri::command]
pub fn startup_files(state: State<StartupFiles>) -> Vec<OpenedFile> {
    std::mem::take(&mut *state.0.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_paths() {
        let args: Vec<String> = ["jay-e.exe", "notes.md", "--flag", "C:\\docs\\a.pdf"].map(String::from).into();
        let cwd = Path::new("C:\\work");
        assert_eq!(paths_from_args(&args, cwd), [cwd.join("notes.md"), PathBuf::from("C:\\docs\\a.pdf")]);
        assert!(paths_from_args(&["jay-e.exe".into()], cwd).is_empty());
    }
}
