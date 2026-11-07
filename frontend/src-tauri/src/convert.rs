//! The batch converter (stage 8): files of any kind Jay-E reads into one folder the person picks,
//! one at a time, each reported on its own. A file that fails (damaged, too big, unsupported) fails
//! alone and the queue goes on. Before starting, the page asks `convert_check` which outputs exist
//! and asks the person whether to replace them, keep both ("name (2).ext") or skip them.

use crate::files::OpenedPaths;
use jay_docs::Kind;
use jay_docs::image::{EncodeOptions, Format};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub struct Batch {
    /// The folder the person picked; the page never names one.
    folder: Mutex<Option<PathBuf>>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Deserialize)]
pub struct Item {
    path: PathBuf,
    /// The output's extension: an image format's (`png`, `jpg`, `webp`…), `docx`, `md`, `html`,
    /// `txt`, `mp4` or `m4a`.
    to: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Existing {
    Replace,
    Rename,
    Skip,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Failure {
    Image(jay_docs::image::ImageError),
    Doc(jay_docs::doc::DocError),
    Video(jay_docs::video::VideoError),
    Other { kind: &'static str, message: String },
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    Started { index: usize },
    Progress { index: usize, done: f64 },
    Done { index: usize, output: String },
    Skipped { index: usize },
    Failed { index: usize, error: Failure },
    Finished { cancelled: bool },
}

const IMAGES: [Format; 9] = [Format::Png, Format::Jpeg, Format::Webp, Format::Avif, Format::Bmp, Format::Tiff, Format::Gif, Format::Ico, Format::Qoi];

/// What a file of `kind` can become (the page offers the same, core/convert.js).
pub fn can_become(kind: Kind, to: &str) -> bool {
    match kind {
        Kind::Image => IMAGES.iter().any(|f| f.extension() == to),
        Kind::Document | Kind::Markdown => matches!(to, "docx" | "md" | "html" | "txt"),
        Kind::Video => matches!(to, "mp4" | "m4a"),
        Kind::Audio => to == "m4a",
        Kind::Pdf => to == "txt",
        _ => false,
    }
}

/// `<folder>/<name of the file>.<to>`.
fn target(folder: &Path, path: &Path, to: &str) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    folder.join(format!("{stem}.{to}"))
}

/// The first free `name (n).ext` beside `path`, not one of `taken`.
fn free_name(path: &Path, taken: &[PathBuf]) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists() && !taken.contains(p))
        .expect("a free name")
}

/// Where each item would go, and whether something is already there (a file, the original itself,
/// or an earlier item of the same batch).
fn plan(folder: &Path, items: &[Item]) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = vec![];
    for item in items {
        let t = target(folder, &item.path, &item.to);
        let taken = t.exists() || out.iter().any(|(o, _)| *o == t);
        out.push((t, taken));
    }
    out
}

/// Asks for the folder. Returns it, or `None` if the person cancelled.
#[tauri::command]
pub async fn convert_pick_folder(app: AppHandle) -> Option<String> {
    let folder = app.dialog().file().blocking_pick_folder().and_then(|p| p.into_path().ok())?;
    let shown = folder.to_string_lossy().into_owned();
    *app.state::<Batch>().folder.lock().unwrap() = Some(folder);
    Some(shown)
}

/// For each item, whether its output already exists in the picked folder.
#[tauri::command]
pub fn convert_check(items: Vec<Item>, batch: State<Batch>) -> Result<Vec<bool>, String> {
    let folder = batch.folder.lock().unwrap().clone().ok_or("no folder")?;
    Ok(plan(&folder, &items).into_iter().map(|(_, taken)| taken).collect())
}

/// Converts the items in order into the picked folder, reporting each on `channel`.
#[tauri::command]
pub fn convert_start(items: Vec<Item>, existing: Existing, quality: u8, channel: Channel<Event>, app: AppHandle) -> Result<(), String> {
    let opened = app.state::<OpenedPaths>();
    if let Some(item) = items.iter().find(|i| !opened.contains(&i.path)) {
        return Err(format!("not an opened file: {}", item.path.display()));
    }
    let batch = app.state::<Batch>();
    let folder = batch.folder.lock().unwrap().clone().ok_or("no folder")?;
    let cancel = Arc::new(AtomicBool::new(false));
    *batch.cancel.lock().unwrap() = Some(cancel.clone());
    std::thread::spawn(move || {
        let planned = plan(&folder, &items);
        let mut made: Vec<PathBuf> = vec![];
        for (index, (item, (mut out, taken))) in items.iter().zip(planned).enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if taken {
                match existing {
                    Existing::Skip => {
                        let _ = channel.send(Event::Skipped { index });
                        continue;
                    }
                    Existing::Rename => out = free_name(&out, &made),
                    // Never over the file being read.
                    Existing::Replace if same_file(&out, &item.path) => out = free_name(&out, &made),
                    Existing::Replace => {}
                }
            }
            let _ = channel.send(Event::Started { index });
            let progress_channel = channel.clone();
            let mut progress = move |done: f64| drop(progress_channel.send(Event::Progress { index, done }));
            // A decoder that panics on a damaged file fails that file, not the batch.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| convert_one(&item.path, &item.to, &out, quality, &mut progress, &cancel)))
                .unwrap_or_else(|_| Err(Failure::Other { kind: "invalid", message: "the file couldn't be read".into() }));
            let event = match result {
                Ok(()) => {
                    made.push(out.clone());
                    Event::Done { index, output: out.to_string_lossy().into_owned() }
                }
                Err(error) => Event::Failed { index, error },
            };
            let _ = channel.send(event);
        }
        let _ = channel.send(Event::Finished { cancelled: cancel.load(Ordering::Relaxed) });
    });
    Ok(())
}

/// Stops the batch: a video stops where it is, any other file finishes first.
#[tauri::command]
pub fn convert_cancel(batch: State<Batch>) {
    if let Some(flag) = batch.cancel.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// One file, written to `out` through a temporary file (an existing `out` is replaced at the end).
fn convert_one(path: &Path, to: &str, out: &Path, quality: u8, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), Failure> {
    let (kind, format) = jay_docs::detect(path).map_err(|e| Failure::Other { kind: "io", message: e.to_string() })?;
    if !can_become(kind, to) {
        return Err(Failure::Other { kind: "unsupported", message: format!("{format} → {to}") });
    }
    let write = |bytes: Vec<u8>| -> std::io::Result<()> {
        let tmp = out.with_extension(format!("{to}.jaye-tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, out).inspect_err(|_| drop(std::fs::remove_file(&tmp)))
    };
    match kind {
        Kind::Image => {
            let format = *IMAGES.iter().find(|f| f.extension() == to).expect("checked by can_become");
            let image = jay_docs::image::decode(path).map_err(Failure::Image)?;
            let bytes = jay_docs::image::encode(&image, &EncodeOptions { format, quality: quality.clamp(1, 100), lossless: false, max_side: None }).map_err(Failure::Image)?;
            write(bytes).map_err(|e| Failure::Other { kind: "io", message: e.to_string() })
        }
        Kind::Document | Kind::Markdown => {
            let document = jay_docs::doc::read(path, format).map_err(Failure::Doc)?;
            let format = if to == "md" { "markdown" } else { to };
            crate::doc::export_to(&document, format, out).map_err(Failure::Doc)
        }
        Kind::Pdf => {
            let pdf = jay_docs::pdf::PdfDocument::open(path, None).map_err(|e| {
                use jay_docs::pdf::OpenError::*;
                let kind = match e { TooBig => "toobig", Password => "password", Invalid => "invalid", Io(_) => "io" };
                // The page words the others by their kind; only a system error has its own text.
                Failure::Other { kind, message: if let Io(m) = e { m } else { String::new() } }
            })?;
            let pages = pdf.info.pages.len();
            let mut text = String::new();
            for page in 0..pages {
                let lines = pdf.text(page).recv().unwrap_or_default();
                text.extend(lines.iter().map(|l| format!("{}\n", l.text)));
                text.push('\n');
                progress((page + 1) as f64 / pages.max(1) as f64);
            }
            write(text.into_bytes()).map_err(|e| Failure::Other { kind: "io", message: e.to_string() })
        }
        #[cfg(windows)]
        Kind::Video | Kind::Audio => {
            let options = jay_docs::video::mf::ConvertOptions { audio_only: to == "m4a", max_height: None, video_kbps: 4000, audio_kbps: 160 };
            jay_docs::video::mf::convert(path, out, &options, progress, cancel).map_err(Failure::Video)
        }
        _ => {
            let _ = cancel;
            Err(Failure::Other { kind: "unsupported", message: format!("{format} → {to}") })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_each_kind_becomes() {
        assert!(can_become(Kind::Image, "webp") && can_become(Kind::Image, "jpg") && !can_become(Kind::Image, "jpeg"));
        assert!(can_become(Kind::Document, "md") && can_become(Kind::Markdown, "docx") && !can_become(Kind::Document, "pdf"));
        assert!(can_become(Kind::Video, "m4a") && !can_become(Kind::Audio, "mp4") && can_become(Kind::Pdf, "txt"));
        assert!(!can_become(Kind::Latex, "pdf") && !can_become(Kind::Unknown, "txt"));
    }

    #[test]
    fn a_batch_converts_around_a_damaged_file() {
        let dir = std::env::temp_dir().join(format!("jaye-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.png");
        let mut png = vec![];
        image::DynamicImage::new_rgba8(4, 3).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        std::fs::write(&good, &png).unwrap();
        let bad = dir.join("bad.png");
        std::fs::write(&bad, &png[..40]).unwrap();
        let md = dir.join("notes.md");
        std::fs::write(&md, "# Title\n\nSome *text*.\n").unwrap();
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let items = vec![Item { path: bad.clone(), to: "webp".into() }, Item { path: good.clone(), to: "webp".into() }, Item { path: md.clone(), to: "html".into() }, Item { path: md.clone(), to: "html".into() }];
        let plan = plan(&out, &items);
        assert_eq!(plan.iter().map(|(_, t)| *t).collect::<Vec<_>>(), [false, false, false, true]);
        let mut none = |_: f64| {};
        let stop = AtomicBool::new(false);
        assert!(matches!(convert_one(&bad, "webp", &plan[0].0, 80, &mut none, &stop), Err(Failure::Image(_))));
        assert!(!plan[0].0.exists() && !out.join("bad.webp.jaye-tmp").exists());
        convert_one(&good, "webp", &plan[1].0, 80, &mut none, &stop).unwrap();
        assert_eq!(image::open(&plan[1].0).unwrap().width(), 4);
        convert_one(&md, "html", &plan[2].0, 80, &mut none, &stop).unwrap();
        assert!(std::fs::read_to_string(&plan[2].0).unwrap().contains("<em>text</em>"));
        assert_eq!(free_name(&plan[3].0, &[]), out.join("notes (2).html"));
        assert_eq!(free_name(&plan[3].0, &[out.join("notes (2).html")]), out.join("notes (3).html"));
        assert!(matches!(convert_one(&md, "webp", &out.join("x.webp"), 80, &mut none, &stop), Err(Failure::Other { kind: "unsupported", .. })));
        assert!(same_file(&good, &dir.join(".").join("good.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
