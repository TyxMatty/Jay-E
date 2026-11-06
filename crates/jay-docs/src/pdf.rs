//! PDF with hayro (pure Rust, `unsafe` forbidden; MIT/Apache). Spike S1 in docs/ARQUITETURA.md
//! measured it against PDFium: same speed and look on the test PDFs, and it reads truncated files.
//!
//! hayro's types borrow the document and aren't `Send`, so each open PDF gets two worker threads
//! that own a parsed copy: one draws pages, the other extracts text, links and searches (a long
//! search never holds up drawing). Callers talk to them through channels.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::kurbo::{Affine, BezPath, Point, Rect};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask,
    TransformExt, interpret_page,
};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier};
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use serde::Serialize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Larger files aren't read (the whole file stays in memory while open).
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// The widest page bitmap; wider zooms are stretched by the page.
/// shortcut: one bitmap per page, capped; tiles (docs/ARQUITETURA.md, section 5) when zooming past ~400% matters.
pub const MAX_RENDER_WIDTH: u32 = 4096;
/// And at most this many pixels (a very tall page would otherwise be huge).
pub const MAX_RENDER_PIXELS: u32 = 4096 * 6144;
/// Pages around the visible ones that are still drawn (the rest is skipped, see `set_visible`).
const VISIBLE_MARGIN: usize = 2;
/// Bound on name-tree and outline walks: documents can be deep or cyclic.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenError {
    TooBig,
    /// Needs a password, or the one given is wrong.
    Password,
    Invalid,
    Io(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooBig => write!(f, "file too big (max {} MB)", MAX_FILE_BYTES >> 20),
            Self::Password => write!(f, "password required"),
            Self::Invalid => write!(f, "not a readable PDF"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Page sizes in points (1/72 in), after the page's rotation.
#[derive(Debug, Clone, Serialize)]
pub struct PdfInfo {
    pub pages: Vec<[f32; 2]>,
}

/// RGBA, 8 bits, row by row from the top. Pages are drawn on white, so alpha is always 255.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Bitmap {
    /// PNG with the Up filter and fdeflate's fastest mode: a text page of 1350 px goes from 10 MB
    /// to ~0.8 MB in ~6 ms (spike S1), which beats moving the raw pixels to the page.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len() / 8);
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fastest);
        encoder.set_filter(png::Filter::Up);
        // Writing into a Vec only fails on a size mismatch, which `render` never makes.
        encoder.write_header().and_then(|mut w| w.write_image_data(&self.rgba)).expect("PNG of a page");
        out
    }
}

/// A line of text in page points from the top-left. `xs[i]` is where char `i` of `text` starts,
/// relative to `x`; selection and search highlights use it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub xs: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LinkTarget {
    /// Zero-based page, and the point from the top to scroll to, when the link says.
    Page { page: usize, top: Option<f32> },
    Uri { uri: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Link {
    /// [x, y, w, h] in page points from the top-left.
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

/// Matches found on one page, each as [x, y, w, h].
#[derive(Debug, Clone, Serialize)]
pub struct SearchHits {
    pub page: usize,
    pub rects: Vec<[f32; 4]>,
}

/// What a search reports as it goes: hits page by page, then `Done`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum SearchEvent {
    Hits(SearchHits),
    /// `searched` pages so far, sent every few pages so the page can show progress.
    Progress { searched: usize },
    Done { searched: usize },
}

pub type SearchSink = Box<dyn FnMut(SearchEvent) + Send>;

enum Job {
    Render { page: usize, width: u32, thumb: bool, reply: Sender<Option<Bitmap>> },
    Text { page: usize, reply: Sender<Vec<TextLine>> },
    Links { page: usize, reply: Sender<Vec<Link>> },
    Search { query: String, generation: u64, sink: SearchSink },
}

/// An open PDF. Dropping it stops its threads.
pub struct PdfDocument {
    pub info: PdfInfo,
    render: Mutex<Sender<Job>>,
    text: Mutex<Sender<Job>>,
    shared: Arc<Shared>,
}

/// State the callers change while the workers run.
struct Shared {
    first_visible: AtomicUsize,
    last_visible: AtomicUsize,
    search_generation: AtomicU64,
}

impl PdfDocument {
    pub fn open(path: &Path, password: Option<&str>) -> Result<Self, OpenError> {
        let len = std::fs::metadata(path).map_err(|e| OpenError::Io(e.to_string()))?.len();
        if len > MAX_FILE_BYTES {
            return Err(OpenError::TooBig);
        }
        let data = Arc::new(std::fs::read(path).map_err(|e| OpenError::Io(e.to_string()))?);
        Self::from_bytes(data, password.unwrap_or(""))
    }

    pub fn from_bytes(data: Arc<Vec<u8>>, password: &str) -> Result<Self, OpenError> {
        let shared = Arc::new(Shared { first_visible: AtomicUsize::new(0), last_visible: AtomicUsize::new(usize::MAX), search_generation: AtomicU64::new(0) });
        // Both parse at the same time; only the drawing one is waited for. Text jobs queue until
        // the other is ready (it reads the same bytes, so it opens too).
        let (render, ready) = spawn_worker(data.clone(), password.to_owned(), shared.clone())?;
        let (text, _) = spawn_worker(data, password.to_owned(), shared.clone())?;
        let info = ready.recv().map_err(|_| OpenError::Invalid)??;
        Ok(Self { info, render: Mutex::new(render), text: Mutex::new(text), shared })
    }

    /// The page at `width` pixels (capped, see `MAX_RENDER_WIDTH`). `None` if the page doesn't exist,
    /// failed to draw, or (unless `thumb`) left the visible range before its turn.
    pub fn render(&self, page: usize, width: u32, thumb: bool) -> Receiver<Option<Bitmap>> {
        let (reply, rx) = channel();
        let _ = self.render.lock().unwrap().send(Job::Render { page, width, thumb, reply });
        rx
    }

    pub fn text(&self, page: usize) -> Receiver<Vec<TextLine>> {
        let (reply, rx) = channel();
        let _ = self.text.lock().unwrap().send(Job::Text { page, reply });
        rx
    }

    pub fn links(&self, page: usize) -> Receiver<Vec<Link>> {
        let (reply, rx) = channel();
        let _ = self.text.lock().unwrap().send(Job::Links { page, reply });
        rx
    }

    /// The pages on screen: page drawings far from them are skipped, newest requests first.
    pub fn set_visible(&self, first: usize, last: usize) {
        self.shared.first_visible.store(first, Ordering::Relaxed);
        self.shared.last_visible.store(last, Ordering::Relaxed);
    }

    /// Searches every page (case-insensitive), reporting to `sink`. A new search or
    /// `cancel_search` stops the previous one.
    pub fn search(&self, query: &str, sink: SearchSink) {
        let generation = self.shared.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.text.lock().unwrap().send(Job::Search { query: query.to_owned(), generation, sink });
    }

    pub fn cancel_search(&self) {
        self.shared.search_generation.fetch_add(1, Ordering::SeqCst);
    }
}

/// Starts a thread that parses `data` and serves jobs. The receiver says when parsing is done.
fn spawn_worker(data: Arc<Vec<u8>>, password: String, shared: Arc<Shared>) -> Result<(Sender<Job>, Receiver<Result<PdfInfo, OpenError>>), OpenError> {
    let (tx, rx) = channel::<Job>();
    let (ready_tx, ready_rx) = channel();
    std::thread::Builder::new()
        .name("jay-docs-pdf".into())
        .spawn(move || {
            let pdf = match catch_unwind(|| Pdf::new_with_password(data, &password)) {
                Ok(Ok(pdf)) => pdf,
                Ok(Err(LoadPdfError::Decryption(DecryptionError::PasswordProtected))) => return drop(ready_tx.send(Err(OpenError::Password))),
                _ => return drop(ready_tx.send(Err(OpenError::Invalid))),
            };
            let pages = pdf.pages();
            if pages.is_empty() {
                return drop(ready_tx.send(Err(OpenError::Invalid)));
            }
            let sizes = pages.iter().map(|p| p.render_dimensions().into()).collect::<Vec<(f32, f32)>>();
            let _ = ready_tx.send(Ok(PdfInfo { pages: sizes.iter().map(|&(w, h)| [w, h]).collect() }));
            Worker { pdf: &pdf, pages, cache: RenderCache::new(), text_cache: vec![None; sizes.len()], shared }.run(rx);
        })
        .map_err(|e| OpenError::Io(e.to_string()))?;
    Ok((tx, ready_rx))
}

struct Worker<'a> {
    pdf: &'a Pdf,
    pages: &'a [Page<'a>],
    cache: RenderCache<'a>,
    text_cache: Vec<Option<Vec<TextLine>>>,
    shared: Arc<Shared>,
}

impl<'a> Worker<'a> {
    fn run(&mut self, rx: Receiver<Job>) {
        // A running search goes on between other jobs, one page at a time.
        let mut search: Option<(String, u64, SearchSink, usize)> = None;
        loop {
            let mut jobs: Vec<Job> = if search.is_some() { rx.try_iter().collect() } else { rx.recv().into_iter().chain(rx.try_iter()).collect() };
            if jobs.is_empty() && search.is_none() {
                return; // every sender is gone: the document was closed
            }
            // Newest first: the pages on screen now were asked last.
            jobs.reverse();
            for job in jobs {
                match job {
                    Job::Render { page, width, thumb, reply } => {
                        let wanted = thumb || self.near_visible(page);
                        let _ = reply.send(if wanted { guard(|| self.render(page, width)).flatten() } else { None });
                    }
                    Job::Text { page, reply } => {
                        let _ = reply.send(self.text(page));
                    }
                    Job::Links { page, reply } => {
                        let _ = reply.send(guard(|| self.links(page)).unwrap_or_default());
                    }
                    Job::Search { query, generation, sink } => search = Some((query, generation, sink, 0)),
                }
            }
            if let Some((query, generation, sink, next)) = &mut search {
                if self.shared.search_generation.load(Ordering::SeqCst) != *generation {
                    search = None;
                } else if *next >= self.pages.len() {
                    sink(SearchEvent::Done { searched: *next });
                    search = None;
                } else {
                    let rects = find_in_lines(&self.text(*next), query);
                    if !rects.is_empty() {
                        sink(SearchEvent::Hits(SearchHits { page: *next, rects }));
                    }
                    *next += 1;
                    if *next % 25 == 0 {
                        sink(SearchEvent::Progress { searched: *next });
                    }
                }
            }
        }
    }

    fn near_visible(&self, page: usize) -> bool {
        let first = self.shared.first_visible.load(Ordering::Relaxed);
        let last = self.shared.last_visible.load(Ordering::Relaxed);
        page + VISIBLE_MARGIN >= first && page <= last.saturating_add(VISIBLE_MARGIN)
    }

    fn render(&self, index: usize, width: u32) -> Option<Bitmap> {
        let page = self.pages.get(index)?;
        let (pw, ph) = page.render_dimensions();
        let (width, _) = fit_bitmap(pw, ph, width);
        let scale = width as f32 / pw;
        let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
        let pixmap = render(page, &self.cache, &InterpreterSettings::default(), &RenderSettings::default(), &settings);
        Some(Bitmap { width: pixmap.width() as u32, height: pixmap.height() as u32, rgba: pixmap.data_as_u8_slice().to_vec() })
    }

    fn text(&mut self, index: usize) -> Vec<TextLine> {
        let Some(page) = self.pages.get(index) else { return vec![] };
        if self.text_cache[index].is_none() {
            let glyphs = guard(|| {
                let cache = InterpreterCache::new();
                let (w, h) = page.render_dimensions();
                let mut ctx = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, w as f64, h as f64), &cache, page.xref(), InterpreterSettings::default());
                let mut device = TextDevice(vec![]);
                interpret_page(page, &mut ctx, &mut device);
                device.0
            });
            self.text_cache[index] = Some(lines_from_glyphs(&glyphs.unwrap_or_default()));
        }
        self.text_cache[index].clone().unwrap_or_default()
    }

    fn links(&self, index: usize) -> Vec<Link> {
        let Some(page) = self.pages.get(index) else { return vec![] };
        let Some(annots) = page.raw().get::<Array>(b"Annots") else { return vec![] };
        let to_page = page.initial_transform(true).to_kurbo();
        let mut links = vec![];
        for annot in annots.iter::<Dict>().take(10_000) {
            if annot.get::<Name>(b"Subtype").as_ref().map(Name::as_str) != Some("Link") {
                continue;
            }
            let Some(r) = annot.get::<hayro::hayro_syntax::object::Rect>(b"Rect") else { continue };
            let target = match annot.get::<Dict>(b"A") {
                Some(action) => self.action_target(&action),
                None => annot.get::<Object>(b"Dest").and_then(|d| self.dest_target(d)),
            };
            if let Some(target) = target {
                let rect = to_page.transform_rect_bbox(Rect::new(r.x0, r.y0, r.x1, r.y1));
                links.push(Link { rect: [rect.x0 as f32, rect.y0 as f32, rect.width() as f32, rect.height() as f32], target });
            }
        }
        links
    }

    fn action_target(&self, action: &Dict<'a>) -> Option<LinkTarget> {
        match action.get::<Name>(b"S")?.as_str() {
            "GoTo" => self.dest_target(action.get::<Object>(b"D")?),
            "URI" => {
                let uri = action.get::<hayro::hayro_syntax::object::String>(b"URI")?;
                let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_owned();
                // Only web and mail links leave the app; never file: or javascript:.
                let lower = uri.to_ascii_lowercase();
                ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s)).then_some(LinkTarget::Uri { uri })
            }
            _ => None,
        }
    }

    /// An explicit destination array, or a name to look up in the catalog.
    fn dest_target(&self, dest: Object<'a>) -> Option<LinkTarget> {
        match dest {
            Object::Array(array) => self.explicit_dest(&array),
            Object::Name(name) => self.named_dest(name.as_str().as_bytes()),
            Object::String(s) => self.named_dest(s.as_bytes()),
            Object::Dict(d) => self.dest_target(d.get::<Object>(b"D")?),
            _ => None,
        }
    }

    /// `[page /XYZ left top zoom]` and the other fit modes: the page, and the top when given.
    fn explicit_dest(&self, array: &Array<'a>) -> Option<LinkTarget> {
        let mut items = array.raw_iter();
        let page = match items.next()? {
            MaybeRef::Ref(r) => {
                let id: ObjectIdentifier = r.into();
                self.pages.iter().position(|p| p.raw().obj_id() == Some(id))?
            }
            // Some writers put a page number instead of a reference.
            MaybeRef::NotRef(Object::Number(n)) => n.as_f64() as usize,
            _ => return None,
        };
        let kind = match items.next() {
            Some(MaybeRef::NotRef(Object::Name(n))) => n.as_str().to_owned(),
            _ => String::new(),
        };
        let number = |o: Option<MaybeRef<Object<'a>>>| match o {
            Some(MaybeRef::NotRef(Object::Number(n))) => Some(n.as_f64()),
            _ => None,
        };
        // The top in PDF space: XYZ's 2nd number, FitH/FitBH's 1st.
        let top = match kind.as_str() {
            "XYZ" => {
                let _left = items.next();
                number(items.next())
            }
            "FitH" | "FitBH" => number(items.next()),
            _ => None,
        };
        let top = top.and_then(|t| {
            let p = self.pages.get(page)?;
            let y = (p.initial_transform(true).to_kurbo() * Point::new(0.0, t)).y;
            Some(y.max(0.0) as f32)
        });
        (page < self.pages.len()).then_some(LinkTarget::Page { page, top })
    }

    fn named_dest(&self, name: &[u8]) -> Option<LinkTarget> {
        let xref = self.pdf.xref();
        let catalog = xref.get::<Dict>(xref.root_id())?;
        // PDF 1.1: /Dests dictionary in the catalog.
        if let Some(dest) = catalog.get::<Dict>(b"Dests").and_then(|d| d.get::<Object>(name)) {
            return self.dest_target(dest);
        }
        // PDF 1.2+: the /Dests name tree.
        let tree = catalog.get::<Dict>(b"Names")?.get::<Dict>(b"Dests")?;
        self.dest_target(find_in_name_tree(&tree, name, 0)?)
    }
}

fn find_in_name_tree<'a>(node: &Dict<'a>, key: &[u8], depth: usize) -> Option<Object<'a>> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(names) = node.get::<Array>(b"Names") {
        let mut items = names.iter::<Object>();
        while let (Some(k), Some(v)) = (items.next(), items.next()) {
            if let Object::String(s) = k
                && s.as_bytes() == key
            {
                return Some(v);
            }
        }
    }
    let kids = node.get::<Array>(b"Kids")?;
    kids.iter::<Dict>().take(10_000).find_map(|kid| find_in_name_tree(&kid, key, depth + 1))
}

/// Width and height of a page bitmap for `width` pixels asked, within the caps.
fn fit_bitmap(page_w: f32, page_h: f32, width: u32) -> (u32, u32) {
    let mut w = width.clamp(1, MAX_RENDER_WIDTH) as f32;
    let mut h = w * page_h / page_w.max(1.0);
    if w * h > MAX_RENDER_PIXELS as f32 {
        let k = (MAX_RENDER_PIXELS as f32 / (w * h)).sqrt();
        w *= k;
        h *= k;
    }
    (w.max(1.0) as u32, h.max(1.0) as u32)
}

/// Runs `f`, turning a panic in the PDF code (a malformed file) into `None`.
fn guard<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

/// A glyph as drawn: its text, box and baseline, in page points from the top-left.
#[derive(Debug, Clone)]
struct PlacedGlyph {
    text: String,
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
    baseline: f32,
    size: f32,
}

/// Collects the glyphs of a page instead of drawing them.
struct TextDevice(Vec<PlacedGlyph>);

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            let text = match glyph.as_unicode() {
                Some(BfString::Char(c)) => c.to_string(),
                Some(BfString::String(s)) => s,
                None => continue,
            };
            // Glyph space is 1000 units per em; the transform takes it to page points.
            let t: Affine = props.transform * glyph.transform();
            let advance = match &**glyph {
                Glyph::Outline(o) => o.advance_width().unwrap_or(500.0),
                Glyph::Type3(_) => 500.0,
            } as f64;
            let origin = t * Point::ZERO;
            let end = t * Point::new(advance, 0.0);
            let up = t * Point::new(0.0, 1000.0);
            let size = ((up.x - origin.x).hypot(up.y - origin.y)) as f32;
            if !size.is_finite() || size <= 0.0 {
                continue;
            }
            // Ascent 0.8 em above the baseline, descent 0.2 below (fonts rarely say better).
            let (x0, x1) = (origin.x.min(end.x) as f32, origin.x.max(end.x) as f32);
            self.0.push(PlacedGlyph { text, x0, x1, top: origin.y as f32 - 0.8 * size, bottom: origin.y as f32 + 0.2 * size, baseline: origin.y as f32, size });
        }
    }
}

/// Groups glyphs in drawing order into lines: a new line when the baseline moves or the text goes
/// back to the left; a space where the gap between glyphs is wider than 0.12 em (TeX sets word
/// gaps by positioning, as narrow as ~0.17 em in tight lines; kerning stays under ~0.05 em).
fn lines_from_glyphs(glyphs: &[PlacedGlyph]) -> Vec<TextLine> {
    let mut lines: Vec<TextLine> = vec![];
    let mut last: Option<&PlacedGlyph> = None;
    for g in glyphs {
        let same_line = last.is_some_and(|p| (g.baseline - p.baseline).abs() < 0.5 * p.size.min(g.size) && g.x0 > p.x0 - 0.5 * p.size);
        if !same_line {
            lines.push(TextLine { text: String::new(), x: g.x0, y: g.top, w: 0.0, h: g.bottom - g.top, xs: vec![] });
        }
        let line = lines.last_mut().unwrap();
        if let Some(p) = last.filter(|_| same_line)
            && g.x0 - p.x1 > 0.12 * g.size
            && !line.text.ends_with(' ')
            && !g.text.starts_with(' ')
        {
            line.xs.push(p.x1 - line.x);
            line.text.push(' ');
        }
        // A glyph for several chars (a ligature like "fi") shares its width between them.
        let n = g.text.chars().count().max(1) as f32;
        for (i, c) in g.text.chars().enumerate() {
            line.xs.push(g.x0 - line.x + (g.x1 - g.x0) * i as f32 / n);
            line.text.push(c);
        }
        let top = line.y.min(g.top);
        let bottom = (line.y + line.h).max(g.bottom);
        line.y = top;
        line.h = bottom - top;
        line.w = line.w.max(g.x1 - line.x);
        last = Some(g);
    }
    lines.retain(|l| !l.text.trim().is_empty());
    lines
}

/// Case-insensitive matches of `query` in `lines`, one rect per match (matches don't cross lines).
pub fn find_in_lines(lines: &[TextLine], query: &str) -> Vec<[f32; 4]> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let needle: Vec<char> = query.chars().map(fold).collect();
    if needle.is_empty() {
        return vec![];
    }
    let mut rects = vec![];
    for line in lines {
        let hay: Vec<char> = line.text.chars().map(fold).collect();
        let mut i = 0;
        while i + needle.len() <= hay.len() {
            if hay[i..i + needle.len()] == needle[..] {
                let end = i + needle.len();
                let x0 = line.xs.get(i).copied().unwrap_or(0.0);
                let x1 = line.xs.get(end).copied().unwrap_or(line.w);
                rects.push([line.x + x0, line.y, x1 - x0, line.h]);
                i = end;
            } else {
                i += 1;
            }
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A PDF with `n` pages of Helvetica text, a link from page 1 to page 2, and a named destination.
    fn sample(n: usize) -> Vec<u8> {
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R /Names << /Dests << /Names [(chapter2) [6 0 R /XYZ 0 700 0]] >> >> >>".into(),
            String::new(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        let mut kids = vec![];
        for p in 0..n {
            let id = objs.len() + 1;
            kids.push(format!("{id} 0 R"));
            let annots = if p == 0 {
                " /Annots [<< /Type /Annot /Subtype /Link /Rect [50 700 150 720] /Dest [6 0 R /XYZ 0 800 0] >> \
                 << /Type /Annot /Subtype /Link /Rect [50 600 150 620] /A << /S /URI /URI (https://example.com/a) >> >> \
                 << /Type /Annot /Subtype /Link /Rect [50 500 150 520] /A << /S /URI /URI (file:///C:/x) >> >> \
                 << /Type /Annot /Subtype /Link /Rect [50 400 150 420] /Dest (chapter2) >>]"
            } else {
                ""
            };
            objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R{annots} >>", id + 1));
            let content = format!("BT /F1 20 Tf 50 750 Td (Hello Page {}) Tj ET BT /F1 10 Tf 50 100 Td (second line) Tj ET", p + 1);
            objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        objs[1] = format!("<< /Type /Pages /Count {n} /Kids [{}] >>", kids.join(" "));
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = vec![];
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        out
    }

    fn open(n: usize) -> PdfDocument {
        PdfDocument::from_bytes(Arc::new(sample(n)), "").unwrap()
    }

    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn opens_and_draws() {
        let doc = open(3);
        assert_eq!(doc.info.pages, vec![[600.0, 800.0]; 3]);
        let bmp = doc.render(0, 300, false).recv_timeout(WAIT).unwrap().unwrap();
        assert_eq!((bmp.width, bmp.height), (300, 400));
        assert_eq!(bmp.rgba.len(), 300 * 400 * 4);
        // White paper with dark text somewhere.
        assert_eq!(&bmp.rgba[..4], &[255, 255, 255, 255]);
        assert!(bmp.rgba.chunks(4).any(|p| p[0] < 100));
        let png = bmp.to_png();
        assert!(png.starts_with(b"\x89PNG") && png.len() < bmp.rgba.len() / 4, "{} bytes", png.len());
        assert!(doc.render(3, 300, false).recv_timeout(WAIT).unwrap().is_none(), "no page 4");
    }

    #[test]
    fn bitmaps_are_capped() {
        assert_eq!(fit_bitmap(600.0, 800.0, 100_000), (4096, 5461));
        let (w, h) = fit_bitmap(100.0, 10_000.0, 4096);
        assert!(w * h <= MAX_RENDER_PIXELS && w < 4096);
        assert_eq!(fit_bitmap(600.0, 800.0, 0), (1, 1));
    }

    #[test]
    fn far_pages_are_skipped_but_thumbnails_are_not() {
        let doc = open(20);
        doc.set_visible(0, 1);
        assert!(doc.render(10, 100, false).recv_timeout(WAIT).unwrap().is_none());
        assert!(doc.render(10, 100, true).recv_timeout(WAIT).unwrap().is_some());
        assert!(doc.render(3, 100, false).recv_timeout(WAIT).unwrap().is_some(), "within the margin");
    }

    #[test]
    fn text_lines_with_positions() {
        let doc = open(1);
        let lines = doc.text(0).recv_timeout(WAIT).unwrap();
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["Hello Page 1", "second line"]);
        let first = &lines[0];
        // 50 pt from the left; baseline 750 pt from the bottom = 50 from the top, 20 pt text.
        assert!((first.x - 50.0).abs() < 0.5, "{first:?}");
        assert!((first.y - (50.0 - 16.0)).abs() < 0.5 && (first.h - 20.0).abs() < 0.5, "{first:?}");
        assert_eq!(first.xs.len(), first.text.chars().count());
        assert!(first.xs.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn links_inside_and_out() {
        let doc = open(2);
        let links = doc.links(0).recv_timeout(WAIT).unwrap();
        assert_eq!(links.len(), 3, "the file: link is dropped: {links:?}");
        assert_eq!(links[0].target, LinkTarget::Page { page: 1, top: Some(0.0) });
        assert_eq!(links[0].rect, [50.0, 80.0, 100.0, 20.0]);
        assert_eq!(links[1].target, LinkTarget::Uri { uri: "https://example.com/a".into() });
        assert_eq!(links[2].target, LinkTarget::Page { page: 1, top: Some(100.0) }, "named destination");
    }

    #[test]
    fn search_reports_every_page_then_done() {
        let doc = open(60);
        let (tx, rx) = channel();
        doc.search("PAGE 5", Box::new(move |e| drop(tx.send(e))));
        let events: Vec<SearchEvent> = rx.iter().take_while(|e| !matches!(e, SearchEvent::Done { .. })).collect();
        let pages: Vec<usize> = events.iter().filter_map(|e| if let SearchEvent::Hits(h) = e { Some(h.page) } else { None }).collect();
        // "Page 5" and "Page 50".."Page 59".
        assert_eq!(pages, [4, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58]);
        assert!(events.iter().any(|e| matches!(e, SearchEvent::Progress { searched: 50 })));
    }

    #[test]
    fn a_new_search_stops_the_old_one() {
        let doc = open(200);
        let (tx, rx) = channel();
        doc.search("line", Box::new(move |e| drop(tx.send(e))));
        doc.cancel_search();
        // Either nothing or a few pages, never Done.
        std::thread::sleep(Duration::from_millis(500));
        assert!(rx.try_iter().all(|e| !matches!(e, SearchEvent::Done { .. })));
    }

    #[test]
    fn find_is_case_insensitive_and_places_the_rect() {
        let line = TextLine { text: "Olá Mundo".into(), x: 10.0, y: 5.0, w: 90.0, h: 12.0, xs: (0..9).map(|i| i as f32 * 10.0).collect() };
        assert_eq!(find_in_lines(std::slice::from_ref(&line), "MUNDO"), vec![[50.0, 5.0, 50.0, 12.0]]);
        assert_eq!(find_in_lines(std::slice::from_ref(&line), "olá"), vec![[10.0, 5.0, 30.0, 12.0]]);
        assert!(find_in_lines(&[line], "").is_empty());
    }

    #[test]
    fn broken_files() {
        assert_eq!(PdfDocument::from_bytes(Arc::new(b"%PDF-1.7 garbage".to_vec()), "").err(), Some(OpenError::Invalid));
        // A truncated file still opens the pages it has (hayro rebuilds the xref).
        let mut half = sample(40);
        half.truncate(half.len() / 2);
        let doc = PdfDocument::from_bytes(Arc::new(half), "").unwrap();
        assert!(!doc.info.pages.is_empty() && doc.info.pages.len() < 40);
    }
}
