//! Jay-E's window. No tray: closing the window closes the app (docs/ARQUITETURA.md, section 2).

// Prevents an additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod files;

use tauri::{DragDropEvent, Manager, WindowEvent};

fn main() {
    tauri::Builder::default()
        // First plugin: a second launch ("Open with" on another file) exits at once and hands its
        // files to the running Jay-E.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            files::announce(app, files::paths_from_args(&args, cwd.as_ref()));
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let args: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            let opened = files::open(app.handle(), files::paths_from_args(&args, &cwd));
            app.manage(files::StartupFiles(opened.into()));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                files::announce(window.app_handle(), paths.clone());
            }
        })
        .invoke_handler(tauri::generate_handler![files::pick_files, files::startup_files])
        .run(tauri::generate_context!())
        .expect("error while running Jay-E");
}
