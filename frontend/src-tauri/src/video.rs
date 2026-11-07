//! Video commands: cut and join by stream copy (jay_docs::video::mp4), convert and grab frames
//! through Media Foundation (jay_docs::video::mf). Results go where the person picks (the save
//! dialog asks before replacing) and open in a new tab.

use crate::files::{self, OpenedPaths};
use jay_docs::video::VideoError;
use jay_docs::video::mp4::{Movie, join};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// The running conversion's cancel flag (one at a time).
#[derive(Default)]
pub struct VideoJobs(Mutex<Option<Arc<AtomicBool>>>);

fn opened(app: &AppHandle, path: &Path) -> Result<(), VideoError> {
    if app.state::<OpenedPaths>().contains(path) { Ok(()) } else { Err(VideoError::Io { message: "not an opened file".into() }) }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, VideoError> + Send + 'static) -> Result<T, VideoError> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| VideoError::Io { message: e.to_string() })?
}

/// The save dialog, starting beside `source` with `name`. `None` if cancelled.
fn pick_target(app: &AppHandle, source: &Path, name: String, ext: &str) -> Option<PathBuf> {
    let mut dialog = app.dialog().file().set_file_name(name).add_filter(ext.to_uppercase(), &[ext]);
    if let Some(dir) = source.parent() {
        dialog = dialog.set_directory(dir);
    }
    dialog.blocking_save_file().and_then(|p| p.into_path().ok())
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// MP4/MOV the stream-copy cut can read.
    cuttable: bool,
    key_frames: Vec<f64>,
    /// Why it can't be cut, when it can't.
    reason: Option<String>,
}

#[tauri::command]
pub async fn video_info(path: PathBuf, app: AppHandle) -> Result<Info, VideoError> {
    opened(&app, &path)?;
    blocking(move || {
        Ok(match Movie::open(&path) {
            Ok(m) => Info { cuttable: true, key_frames: m.key_frames(), reason: None },
            Err(e) => Info { cuttable: false, key_frames: vec![], reason: Some(format!("{e:?}")) },
        })
    })
    .await
}

#[derive(Serialize)]
pub struct Saved {
    path: String,
    start: f64,
    end: f64,
}

/// Cuts `[start, end)` seconds without re-encoding, from the key frame at or before `start`.
#[tauri::command]
pub async fn video_cut(path: PathBuf, start: f64, end: f64, app: AppHandle) -> Result<Option<Saved>, VideoError> {
    opened(&app, &path)?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp4".into());
    let Some(target) = pick_target(&app, &path, format!("{} (cut).{ext}", stem(&path)), &ext) else { return Ok(None) };
    let (source, out) = (path.clone(), target.clone());
    let (start, end) = blocking(move || Movie::open(&source)?.cut(&source, start, end, &out, &AtomicBool::new(false))).await?;
    files::announce(&app, vec![target.clone()]);
    Ok(Some(Saved { path: target.to_string_lossy().into_owned(), start, end }))
}

/// Joins `paths` in order (same codecs and settings) without re-encoding.
#[tauri::command]
pub async fn video_join(paths: Vec<PathBuf>, app: AppHandle) -> Result<Option<String>, VideoError> {
    for p in &paths {
        opened(&app, p)?;
    }
    let first = paths.first().cloned().ok_or_else(|| VideoError::Invalid { message: "nothing to join".into() })?;
    let ext = first.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp4".into());
    let Some(target) = pick_target(&app, &first, format!("{} (joined).{ext}", stem(&first)), &ext) else { return Ok(None) };
    let out = target.clone();
    blocking(move || {
        let movies = paths.iter().map(|p| Movie::open(p)).collect::<Result<Vec<_>, _>>()?;
        let parts: Vec<(&Path, &Movie)> = paths.iter().map(PathBuf::as_path).zip(movies.iter()).collect();
        join(&parts, &out, &AtomicBool::new(false))
    })
    .await?;
    files::announce(&app, vec![target.clone()]);
    Ok(Some(target.to_string_lossy().into_owned()))
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum ConvertEvent {
    Progress { done: f64 },
    Finished { path: String },
    Failed { error: VideoError },
}

/// Converts to MP4 (H.264 + AAC) or M4A where the person picks, reporting to `channel`.
/// Returns false if the dialog was cancelled.
#[cfg(windows)]
#[tauri::command]
pub async fn video_convert(path: PathBuf, options: jay_docs::video::mf::ConvertOptions, channel: Channel<ConvertEvent>, app: AppHandle, jobs: State<'_, VideoJobs>) -> Result<bool, VideoError> {
    opened(&app, &path)?;
    let ext = if options.audio_only { "m4a" } else { "mp4" };
    let Some(target) = pick_target(&app, &path, format!("{} (converted).{ext}", stem(&path)), ext) else { return Ok(false) };
    let cancel = Arc::new(AtomicBool::new(false));
    *jobs.0.lock().unwrap() = Some(cancel.clone());
    let handle = app.clone();
    std::thread::spawn(move || {
        let progress_channel = channel.clone();
        let mut progress = move |done: f64| drop(progress_channel.send(ConvertEvent::Progress { done }));
        let event = match jay_docs::video::mf::convert(&path, &target, &options, &mut progress, &cancel) {
            Ok(()) => {
                files::announce(&handle, vec![target.clone()]);
                ConvertEvent::Finished { path: target.to_string_lossy().into_owned() }
            }
            Err(error) => ConvertEvent::Failed { error },
        };
        let _ = channel.send(event);
    });
    Ok(true)
}

#[tauri::command]
pub fn video_cancel(jobs: State<VideoJobs>) {
    if let Some(flag) = jobs.0.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Saves the frame at `seconds` as a PNG where the person picks.
#[cfg(windows)]
#[tauri::command]
pub async fn video_frame(path: PathBuf, seconds: f64, app: AppHandle) -> Result<Option<String>, VideoError> {
    opened(&app, &path)?;
    let label = format!("{:02}m{:05.2}s", (seconds / 60.0) as u32, seconds % 60.0).replace('.', "_");
    let Some(target) = pick_target(&app, &path, format!("{} {label}.png", stem(&path)), "png") else { return Ok(None) };
    let out = target.clone();
    blocking(move || {
        let (w, h, rgba, _) = jay_docs::video::mf::frame(&path, seconds)?;
        let png = jay_docs::image::encode(&image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, rgba).ok_or(VideoError::TooBig)?), &jay_docs::image::EncodeOptions { format: jay_docs::image::Format::Png, quality: 100, lossless: true, max_side: None })
            .map_err(|e| VideoError::Invalid { message: format!("{e:?}") })?;
        std::fs::write(&out, png)?;
        Ok(())
    })
    .await?;
    files::announce(&app, vec![target.clone()]);
    Ok(Some(target.to_string_lossy().into_owned()))
}
