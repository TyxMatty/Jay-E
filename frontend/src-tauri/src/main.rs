//! Jay-E's window. No tray: closing the window closes the app (docs/ARQUITETURA.md, section 2).

// Prevents an additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod convert;
mod doc;
mod files;
mod image;
mod latex;
mod pdf;
mod pdf_edit;
#[cfg(windows)]
mod taskbar_icon;
mod text;
mod update;
mod video;

use tauri::{DragDropEvent, Manager, WindowEvent};

/// A web or mail link from a PDF, in the default browser. Only http(s) and mailto, checked again
/// here because the page sends it.
#[tauri::command]
fn open_link(uri: String) -> Result<(), String> {
    let lower = uri.to_ascii_lowercase();
    if !["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s)) {
        return Err("not a web link".into());
    }
    tauri_plugin_opener::open_url(uri, None::<&str>).map_err(|e| e.to_string())
}

fn main() {
    // `jay-e.exe --jaye-tex ...`: the LaTeX engine's child process (src/latex.rs), no window.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(jay_docs::latex::ENGINE_ARG) {
        std::process::exit(jay_docs::latex::engine_main(&args[2..]));
    }
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
        .plugin(tauri_plugin_opener::init())
        .manage(files::OpenedPaths::default())
        .manage(pdf::Docs::default())
        .manage(pdf_edit::PdfEdits::default())
        .manage(image::Images::default())
        .manage(doc::Documents::default())
        .manage(convert::Batch::default())
        .manage(doc::Editors::default())
        .manage(video::VideoJobs::default())
        .manage(latex::Compiles::default())
        .manage(update::Downloaded::default())
        .register_asynchronous_uri_scheme_protocol("jaye", pdf::protocol)
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
        .invoke_handler(tauri::generate_handler![
            files::pick_files,
            files::startup_files,
            pdf::pdf_open,
            pdf::pdf_close,
            pdf::pdf_visible,
            pdf::pdf_text,
            pdf::pdf_links,
            pdf::pdf_search,
            pdf::pdf_cancel_search,
            pdf_edit::pdf_edit,
            pdf_edit::pdf_edit_undo,
            pdf_edit::pdf_edit_insert,
            pdf_edit::pdf_edit_extract,
            pdf_edit::pdf_edit_save,
            pdf_edit::pdf_annotations,
            pdf_edit::pdf_fields,
            text::text_open,
            text::text_stamp,
            text::text_save,
            text::markdown_render,
            text::markdown_export_html,
            text::markdown_export_docx,
            doc::doc_open,
            doc::doc_close,
            doc::doc_export,
            doc::docx_edit_open,
            doc::docx_edit_save,
            doc::docx_edit_add_image,
            doc::docx_edit_paste_image,
            doc::docx_edit_export,
            video::video_info,
            video::video_cut,
            video::video_join,
            video::video_convert,
            video::video_cancel,
            video::video_frame,
            latex::latex_compile,
            latex::latex_cancel,
            latex::latex_save_pdf,
            image::image_open,
            image::image_close,
            image::image_set_edits,
            image::image_save_as,
            image::image_convert,
            image::image_convert_cancel,
            convert::convert_pick_folder,
            convert::convert_check,
            convert::convert_start,
            convert::convert_cancel,
            update::update_check,
            update::update_download,
            update::update_install,
            open_link,
            #[cfg(windows)]
            taskbar_icon::set_taskbar_icon,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jay-E");
}
