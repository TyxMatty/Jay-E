//! Text and Markdown commands over `jay_docs::text` and `jay_docs::markdown`. Every path the page
//! sends must be a file the person opened (files.rs).

use crate::files::OpenedPaths;
use jay_docs::text::{self, Stamp, TextError, TextFile};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// Images a Markdown file may show: next to it or below its folder, with an image extension.
const IMAGE_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "ico"];

fn opened(paths: &OpenedPaths, path: &Path) -> Result<(), TextError> {
    if paths.contains(path) { Ok(()) } else { Err(TextError::Io { message: "not an opened file".into() }) }
}

#[tauri::command]
pub async fn text_open(path: PathBuf, paths: State<'_, OpenedPaths>) -> Result<TextFile, TextError> {
    opened(&paths, &path)?;
    tauri::async_runtime::spawn_blocking(move || text::read(&path)).await.map_err(|e| TextError::Io { message: e.to_string() })?
}

/// For noticing changes made by other programs (the page asks when the window gets focus).
#[tauri::command]
pub fn text_stamp(path: PathBuf, paths: State<OpenedPaths>) -> Result<Stamp, TextError> {
    opened(&paths, &path)?;
    text::stamp(&path)
}

#[tauri::command]
pub async fn text_save(path: PathBuf, file: TextFile, force: bool, paths: State<'_, OpenedPaths>) -> Result<Stamp, TextError> {
    opened(&paths, &path)?;
    tauri::async_runtime::spawn_blocking(move || text::write(&path, &file, force)).await.map_err(|e| TextError::Io { message: e.to_string() })?
}

/// The preview's HTML. Local images are let through the asset protocol one by one.
#[tauri::command]
pub fn markdown_render(path: PathBuf, text: String, app: AppHandle, paths: State<OpenedPaths>) -> Result<String, TextError> {
    opened(&paths, &path)?;
    Ok(jay_docs::markdown::to_html(&text, |src| {
        let file = local_image(&path, src)?;
        app.asset_protocol_scope().allow_file(&file).ok()?;
        Some(asset_url(&file))
    }))
}

/// A standalone HTML file next to the Markdown (the person picks where), images kept as relative
/// links. Returns where it was saved, or `None` if the dialog was cancelled.
#[tauri::command]
pub async fn markdown_export_html(path: PathBuf, text: String, title: String, app: AppHandle) -> Result<Option<String>, TextError> {
    opened(&app.state::<OpenedPaths>(), &path)?;
    let body = jay_docs::markdown::to_html(&text, |src| local_image(&path, src).map(|_| src.to_owned()));
    let html = format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>{}</style>\n</head>\n<body>\n<main class=\"markdown\">\n{body}</main>\n</body>\n</html>\n",
        title.replace('&', "&amp;").replace('<', "&lt;"),
        EXPORT_CSS
    );
    let name = path.file_stem().map(|s| format!("{}.html", s.to_string_lossy())).unwrap_or_else(|| "export.html".into());
    let mut dialog = app.dialog().file().set_file_name(name).add_filter("HTML", &["html"]);
    if let Some(dir) = path.parent() {
        dialog = dialog.set_directory(dir);
    }
    let Some(target) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::write(&target, html)?;
    Ok(Some(target.to_string_lossy().into_owned()))
}

/// The Markdown being edited as a DOCX where the person picks (images in the Markdown are links to
/// files, so they don't go in). `None` if the dialog was cancelled.
#[tauri::command]
pub async fn markdown_export_docx(path: PathBuf, text: String, app: AppHandle) -> Result<Option<String>, jay_docs::doc::DocError> {
    if !app.state::<OpenedPaths>().contains(&path) {
        return Err(jay_docs::doc::DocError::Io { message: "not an opened file".into() });
    }
    let document = jay_docs::doc::markdown::read(&text);
    crate::doc::save_as(&app, &path, &document, "docx").await
}

/// `src` relative to the Markdown file, inside its folder, an image that exists.
fn local_image(markdown: &Path, src: &str) -> Option<PathBuf> {
    if src.contains(':') || src.starts_with('/') || src.starts_with('\\') {
        return None; // a scheme (https:, data:), a drive or a rooted path: not a file beside it
    }
    let src = percent_decode(src.split(['?', '#']).next()?);
    let dir = markdown.parent()?.canonicalize().ok()?;
    let file = dir.join(&src).canonicalize().ok()?;
    let ext = file.extension()?.to_str()?.to_ascii_lowercase();
    let ok = file.starts_with(&dir) && file.is_file() && IMAGE_EXTENSIONS.contains(&ext.as_str());
    // canonicalize() gives \\?\C:\…; the asset scope and the protocol want C:\…
    let plain = file.to_string_lossy().strip_prefix(r"\\?\").map(PathBuf::from);
    ok.then(|| plain.unwrap_or(file))
}

/// What `convertFileSrc` makes of a path: `http://asset.localhost/` on Windows, `asset://localhost/`
/// elsewhere, then the path with everything but `A-Za-z0-9-_.!~*'()` percent-encoded.
fn asset_url(path: &Path) -> String {
    let base = if cfg!(windows) { "http://asset.localhost/" } else { "asset://localhost/" };
    let mut out = String::from(base);
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The exported page's look: readable, light, prints well. Kept short on purpose.
pub(crate) const EXPORT_CSS: &str = "body{margin:0;background:#fff;color:#1f2328;font:16px/1.6 system-ui,-apple-system,'Segoe UI',sans-serif}\
.markdown{max-width:820px;margin:0 auto;padding:32px 24px}\
h1,h2{border-bottom:1px solid #d1d9e0;padding-bottom:.3em}\
code,pre{font-family:ui-monospace,'Cascadia Code',Consolas,monospace;font-size:.9em;background:#f6f8fa;border-radius:6px}\
code{padding:.15em .35em}pre{padding:14px 16px;overflow:auto}pre code{padding:0;background:none}\
blockquote{margin:0;padding:0 1em;color:#59636e;border-left:.25em solid #d1d9e0}\
table{border-collapse:collapse}th,td{border:1px solid #d1d9e0;padding:6px 13px}\
img{max-width:100%}li:has(>input[type=checkbox]){list-style:none}.math-error{color:#cf222e}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_beside_the_file_only() {
        let dir = std::env::temp_dir().join(format!("jay-e-img-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let md = dir.join("note.md");
        std::fs::write(&md, "x").unwrap();
        std::fs::write(dir.join("sub").join("a b.png"), "x").unwrap();
        std::fs::write(dir.join("secret.txt"), "x").unwrap();
        let found = local_image(&md, "sub/a%20b.png").unwrap();
        assert!(!found.to_string_lossy().starts_with(r"\\?\"), "{found:?}");
        assert!(local_image(&md, "sub/a b.png?v=1").is_some());
        assert!(local_image(&md, "secret.txt").is_none(), "not an image");
        assert!(local_image(&md, "../outside.png").is_none());
        assert!(local_image(&md, "C:/Windows/x.png").is_none());
        assert!(local_image(&md, "https://e.com/a.png").is_none());
        assert!(local_image(&md, "missing.png").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn asset_urls_match_convert_file_src() {
        let url = asset_url(Path::new(r"C:\Users\me\Fotos\a b.png"));
        if cfg!(windows) {
            assert_eq!(url, "http://asset.localhost/C%3A%5CUsers%5Cme%5CFotos%5Ca%20b.png");
        }
    }
}
