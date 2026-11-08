//! The image editor's side in Rust: open images stay decoded here, the page sends the list of edits,
//! and `jaye://` hands back what to show as fast PNGs:
//!
//! - `img/<id>/<rev>/view/<width>`: the whole edited image, from a ≤ 2048 px preview.
//! - `img/<id>/<rev>/region/<x>/<y>/<w>/<h>/<width>`: a part of the full-size edited image (in its
//!   pixels), for zooming into a big photo without sending all of it.
//!
//! `rev` only busts the page's cache; the current edits are the ones drawn.

use crate::files::OpenedPaths;
use image::{DynamicImage, GenericImageView};
use jay_docs::image::{self as img, EncodeOptions, Edits, ImageError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::http::{Response, StatusCode};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

/// The preview's longer side: enough for a big screen at fit; zooming further uses regions.
const PREVIEW_SIDE: u32 = 2048;

struct OpenImage {
    path: PathBuf,
    full: Arc<DynamicImage>,
    preview: Arc<DynamicImage>,
    edits: Edits,
    rev: u32,
    /// The edits applied to `preview` and `full`, made when first asked for this `rev`.
    edited_preview: Option<(u32, Arc<DynamicImage>)>,
    edited_full: Option<(u32, Arc<DynamicImage>)>,
}

impl OpenImage {
    fn edited(&mut self, full: bool) -> Arc<DynamicImage> {
        let (source, cache) = if full { (&self.full, &mut self.edited_full) } else { (&self.preview, &mut self.edited_preview) };
        match cache {
            Some((rev, image)) if *rev == self.rev => image.clone(),
            _ => {
                let image = if self.edits.is_empty() { source.clone() } else { Arc::new(img::apply(source, &self.edits)) };
                *cache = Some((self.rev, image.clone()));
                image
            }
        }
    }
}

#[derive(Default)]
pub struct Images {
    open: Mutex<HashMap<u32, Arc<Mutex<OpenImage>>>>,
    next: AtomicU32,
    /// The running batch conversion, if any, and its cancel flag.
    convert_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl Images {
    fn get(&self, id: u32) -> Result<Arc<Mutex<OpenImage>>, ImageError> {
        self.open.lock().unwrap().get(&id).cloned().ok_or(ImageError::Io { message: "image closed".into() })
    }
}

fn opened(paths: &OpenedPaths, path: &std::path::Path) -> Result<(), ImageError> {
    if paths.contains(path) { Ok(()) } else { Err(ImageError::Io { message: "not an opened file".into() }) }
}

#[derive(Serialize)]
pub struct Opened {
    id: u32,
    width: u32,
    height: u32,
}

#[tauri::command]
pub async fn image_open(path: PathBuf, paths: State<'_, OpenedPaths>, images: State<'_, Images>) -> Result<Opened, ImageError> {
    opened(&paths, &path)?;
    let source = path.clone();
    let (full, preview) = tauri::async_runtime::spawn_blocking(move || {
        let full = img::decode(&source)?;
        let preview = if full.width().max(full.height()) > PREVIEW_SIDE { full.thumbnail(PREVIEW_SIDE, PREVIEW_SIDE) } else { full.clone() };
        Ok::<_, ImageError>((full, preview))
    })
    .await
    .map_err(|e| ImageError::Io { message: e.to_string() })??;
    let (width, height) = full.dimensions();
    let id = images.next.fetch_add(1, Ordering::Relaxed) + 1;
    let open = OpenImage { path, full: Arc::new(full), preview: Arc::new(preview), edits: Edits::default(), rev: 0, edited_preview: None, edited_full: None };
    images.open.lock().unwrap().insert(id, Arc::new(Mutex::new(open)));
    Ok(Opened { id, width, height })
}

#[tauri::command]
pub fn image_close(id: u32, images: State<Images>) {
    images.open.lock().unwrap().remove(&id);
}

#[derive(Serialize)]
pub struct Edited {
    rev: u32,
    width: u32,
    height: u32,
}

/// Replaces the edit list; returns the new revision and the edited size.
#[tauri::command]
pub fn image_set_edits(id: u32, edits: Edits, images: State<Images>) -> Result<Edited, ImageError> {
    let image = images.get(id)?;
    let mut image = image.lock().unwrap();
    let (width, height) = edits.size(image.full.width(), image.full.height());
    image.edits = edits;
    image.rev += 1;
    Ok(Edited { rev: image.rev, width, height })
}

#[derive(Serialize)]
pub struct Histogram {
    r: Vec<u32>,
    g: Vec<u32>,
    b: Vec<u32>,
    l: Vec<u32>,
}

/// The histogram the tone tools work from: the preview after the crop, rotations and flips, before
/// any tone or color change.
#[tauri::command]
pub async fn image_histogram(id: u32, images: State<'_, Images>) -> Result<Histogram, ImageError> {
    let image = images.get(id)?;
    let (preview, ops) = {
        let image = image.lock().unwrap();
        (image.preview.clone(), image.edits.ops.clone())
    };
    let shaped = img::apply(&preview, &Edits { ops, ..Default::default() });
    let [r, g, b, l] = img::histogram(&shaped);
    Ok(Histogram { r, g, b, l })
}

/// Encodes the edited image and saves it where the person picks. `None` if they cancel.
#[tauri::command]
pub async fn image_save_as(id: u32, options: EncodeOptions, app: AppHandle) -> Result<Option<String>, ImageError> {
    let image = app.state::<Images>().get(id)?;
    let (path, edited) = {
        let mut image = image.lock().unwrap();
        (image.path.clone(), image.edited(true))
    };
    let ext = options.format.extension();
    let name = format!("{}.{ext}", path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into()));
    let mut dialog = app.dialog().file().set_file_name(name).add_filter(ext.to_uppercase(), &[ext]);
    if let Some(dir) = path.parent() {
        dialog = dialog.set_directory(dir);
    }
    // The save dialog itself asks before replacing an existing file.
    let Some(target) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = img::encode(&edited, &options)?;
        let tmp = target.with_extension(format!("{ext}.jaye-tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &target).inspect_err(|_| drop(std::fs::remove_file(&tmp)))?;
        Ok(Some(target.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| ImageError::Io { message: e.to_string() })?
}

#[derive(Deserialize)]
pub struct ConvertItem {
    path: PathBuf,
    #[serde(default)]
    edits: Edits,
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum ConvertEvent {
    /// The folder the person picked; the files go there.
    Started { folder: String },
    Done { index: usize, output: String },
    Failed { index: usize, error: ImageError },
    Finished { cancelled: bool },
}

/// Converts `items` into a folder the person picks, several at a time, reporting each file.
/// Returns false if the folder dialog was cancelled.
#[tauri::command]
pub async fn image_convert(items: Vec<ConvertItem>, options: EncodeOptions, channel: Channel<ConvertEvent>, app: AppHandle) -> Result<bool, ImageError> {
    for item in &items {
        opened(&app.state::<OpenedPaths>(), &item.path)?;
    }
    let first_dir = items.first().and_then(|i| i.path.parent().map(PathBuf::from));
    let mut dialog = app.dialog().file();
    if let Some(dir) = first_dir {
        dialog = dialog.set_directory(dir);
    }
    let Some(folder) = dialog.blocking_pick_folder().and_then(|p| p.into_path().ok()) else { return Ok(false) };
    let cancel = Arc::new(AtomicBool::new(false));
    *app.state::<Images>().convert_cancel.lock().unwrap() = Some(cancel.clone());
    let _ = channel.send(ConvertEvent::Started { folder: folder.to_string_lossy().into_owned() });
    tauri::async_runtime::spawn_blocking(move || {
        use rayon::prelude::*;
        // One file per core (rayon's pool); AVIF encoding inside uses the same pool.
        items.par_iter().enumerate().for_each(|(index, item)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let event = match img::convert_file(&item.path, &item.edits, &options, &folder) {
                Ok(out) => ConvertEvent::Done { index, output: out.to_string_lossy().into_owned() },
                Err(error) => ConvertEvent::Failed { index, error },
            };
            let _ = channel.send(event);
        });
        let _ = channel.send(ConvertEvent::Finished { cancelled: cancel.load(Ordering::Relaxed) });
    });
    Ok(true)
}

/// Stops starting new files in the running conversion (the ones in progress finish).
#[tauri::command]
pub fn image_convert_cancel(images: State<Images>) {
    if let Some(flag) = images.convert_cancel.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// `jaye://localhost/img/…` (see the module docs). Called from main.rs's protocol handler.
pub fn respond<R: Runtime>(app: &AppHandle<R>, parts: &[&str]) -> Response<Vec<u8>> {
    let reply = |status: StatusCode, body: Vec<u8>| crate::pdf::response(status, body);
    let Some(request) = parse(parts) else { return reply(StatusCode::NOT_FOUND, vec![]) };
    let Ok(image) = app.state::<Images>().get(request.id) else { return reply(StatusCode::NOT_FOUND, vec![]) };
    let rendered = match request.what {
        What::View { width } => {
            let edited = image.lock().unwrap().edited(false);
            scale_to(&edited, width)
        }
        What::Region { x, y, w, h, width } => {
            let edited = image.lock().unwrap().edited(true);
            let (iw, ih) = edited.dimensions();
            let (x, y) = (x.min(iw.saturating_sub(1)), y.min(ih.saturating_sub(1)));
            let (w, h) = (w.clamp(1, iw - x), h.clamp(1, ih - y));
            scale_to(&edited.crop_imm(x, y, w, h), width)
        }
    };
    let rgba = rendered.to_rgba8();
    reply(StatusCode::OK, img::fast_png(rgba.width(), rgba.height(), rgba.as_raw()))
}

/// Shrinks to `width` pixels wide (never enlarges: the page stretches).
fn scale_to(image: &DynamicImage, width: u32) -> DynamicImage {
    if width == 0 || width >= image.width() {
        return image.clone();
    }
    let height = ((image.height() as u64 * width as u64) / image.width() as u64).max(1) as u32;
    image.resize_exact(width, height, image::imageops::FilterType::Triangle)
}

struct Request {
    id: u32,
    what: What,
}

enum What {
    View { width: u32 },
    Region { x: u32, y: u32, w: u32, h: u32, width: u32 },
}

/// `[<id>, <rev>, "view", <width>]` or `[<id>, <rev>, "region", x, y, w, h, <width>]`.
fn parse(parts: &[&str]) -> Option<Request> {
    let n = |i: usize| parts.get(i)?.parse::<u32>().ok();
    let id = n(0)?;
    n(1)?; // the revision, only for the cache
    let what = match (*parts.get(2)?, parts.len()) {
        ("view", 4) => What::View { width: n(3)? },
        ("region", 8) => What::Region { x: n(3)?, y: n(4)?, w: n(5)?, h: n(6)?, width: n(7)? },
        _ => return None,
    };
    Some(Request { id, what })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert!(matches!(parse(&["3", "7", "view", "800"]), Some(Request { id: 3, what: What::View { width: 800 } })));
        assert!(matches!(parse(&["3", "7", "region", "10", "20", "30", "40", "300"]), Some(Request { what: What::Region { x: 10, y: 20, w: 30, h: 40, width: 300 }, .. })));
        assert!(parse(&["3", "7", "view"]).is_none());
        assert!(parse(&["3", "x", "view", "800"]).is_none());
        assert!(parse(&["3", "7", "region", "1", "2", "3", "4"]).is_none());
    }

    #[test]
    fn scaling_never_enlarges() {
        let img = DynamicImage::new_rgba8(100, 50);
        assert_eq!(scale_to(&img, 40).dimensions(), (40, 20));
        assert_eq!(scale_to(&img, 400).dimensions(), (100, 50));
        assert_eq!(scale_to(&img, 0).dimensions(), (100, 50));
    }
}
