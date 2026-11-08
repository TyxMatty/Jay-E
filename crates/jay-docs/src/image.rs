//! Images: decode (pure Rust, with limits: files are untrusted, docs/REFERENCIAS.md 4.2), edit
//! without touching the original (a list of operations applied on the way out, as in Kdenlive), and
//! encode to the common formats.
//!
//! Edits are resolution independent (crops in fractions of the image), so the same list draws the
//! screen preview and the full-size file. Adjustments use the CSS filter formulas.

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

/// Images with more pixels aren't decoded (a 100 MP photo is fine; 300 MP is ~1.2 GB in memory).
pub const MAX_PIXELS: u64 = 300_000_000;
/// Nor wider or taller than this, whatever the file claims.
pub const MAX_SIDE: u32 = 65_535;
/// Memory the decoder may allocate (decompression bombs stop here).
pub const MAX_DECODE_BYTES: u64 = 3 << 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageError {
    /// Larger than the limits above.
    TooBig {
        width: u32,
        height: u32,
    },
    /// A format the Rust decoders don't read (AVIF, HEIC, SVG…).
    Unsupported,
    Invalid {
        message: String,
    },
    Io {
        message: String,
    },
}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            message: e.to_string(),
        }
    }
}

impl From<image::ImageError> for ImageError {
    fn from(e: image::ImageError) -> Self {
        match e {
            image::ImageError::Unsupported(_) => Self::Unsupported,
            image::ImageError::Limits(_) => Self::TooBig {
                width: 0,
                height: 0,
            },
            image::ImageError::IoError(e) => e.into(),
            other => Self::Invalid {
                message: other.to_string(),
            },
        }
    }
}

/// Decodes a file, upright (EXIF orientation applied, as the browser does).
pub fn decode(path: &Path) -> Result<DynamicImage, ImageError> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(ImageError::TooBig { width, height });
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// A geometric edit. Crop and the corners are in fractions (0..1) of the image as it is at that point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    RotateRight,
    RotateLeft,
    FlipH,
    FlipV,
    Crop { x: f32, y: f32, w: f32, h: f32 },
    /// Straighten: turned by `degrees` (clockwise, at most 45 either way), keeping the largest
    /// picture of the same shape inside, so no empty corners show.
    Rotate { degrees: f32 },
    /// Perspective crop: these four points (top-left, top-right, bottom-right, bottom-left) become
    /// the picture's corners, as when a photo of a page is taken at an angle.
    Perspective { corners: [[f32; 2]; 4] },
}

/// Tone and color. The sliders are -1..1, 0 = unchanged. Brightness and contrast scale by 1 + value,
/// saturation too (CSS filters). Missing fields (an older list of edits) are "unchanged".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjust {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// ±2 stops: the values are multiplied by 2^(2 × exposure).
    pub exposure: f32,
    /// Lifts (+) or darkens (−) the dark parts, by how dark each pixel is.
    pub shadows: f32,
    /// Brightens (+) or brings back (−) the bright parts, by how bright each pixel is.
    pub highlights: f32,
    /// Warmer (+, yellow) or cooler (−, blue).
    pub temperature: f32,
    /// Magenta (+) or green (−).
    pub tint: f32,
    /// Saturation that spares what is already saturated (and so skin tones).
    pub vibrance: f32,
    /// Black and white (Rec. 709 luma), last.
    pub grayscale: bool,
    pub levels: Levels,
    /// The tone curve: points (in, out) in 0..1, sorted by `in`; fewer than two means none.
    pub curve: Vec<[f32; 2]>,
    /// A color look from a `.cube` file (3D LUT), by its path; the app only sends files the person
    /// picked. A file that can't be read leaves the colors as they are.
    pub lut: Option<String>,
    /// Filters, 0..1 (0 = off), sized to the picture so the preview and the file look the same.
    pub denoise: f32,
    pub sharpen: f32,
    pub blur: f32,
    /// Darker (+) or lighter (−) corners.
    pub vignette: f32,
}

/// Levels per channel (R, G, B): what becomes black and white, in 0..1, and the midtones' gamma.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Levels {
    pub black: [f32; 3],
    pub white: [f32; 3],
    pub gamma: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self { black: [0.0; 3], white: [1.0; 3], gamma: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Edits {
    pub ops: Vec<Op>,
    pub adjust: Adjust,
    /// Drawing over the picture (pen and highlighter, and the arrows and shapes made of lines), on
    /// top of everything else.
    #[serde(default)]
    pub strokes: Vec<Stroke>,
    /// Blurred or pixelated parts (to hide something) and text, drawn before the strokes.
    #[serde(default)]
    pub areas: Vec<Area>,
}

/// A rectangle of the picture as it was after the first `at` operations (top-left and size in
/// fractions), carried through the later ones like a stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub at: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(flatten)]
    pub kind: AreaKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AreaKind {
    Blur,
    Pixelate,
    /// A picture laid on top (text the page drew): a PNG, base64.
    Stamp { png: String },
}

/// A stamp's PNG can't be bigger than this (base64 characters) or than 4096 pixels a side.
const MAX_STAMP_BASE64: usize = 8 << 20;

/// One brush stroke. Points and width are fractions of the image as it was when drawn (after the
/// first `at` operations), so a rotation, flip or crop made later carries the drawing along.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub at: usize,
    /// `#rrggbb`.
    pub color: String,
    /// The line's width as a fraction of the image's longer side.
    pub width: f32,
    /// 1 for a pen, less for a highlighter (the picture shows through).
    pub opacity: f32,
    pub points: Vec<[f32; 2]>,
}

/// More strokes or points than this are ignored (the list comes from the page).
const MAX_STROKES: usize = 5_000;
const MAX_POINTS: usize = 50_000;

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.adjust == Adjust::default() && self.strokes.is_empty() && self.areas.is_empty()
    }

    /// The size after the edits, for an image of `width`×`height`.
    pub fn size(&self, width: u32, height: u32) -> (u32, u32) {
        self.ops.iter().fold((width, height), |(w, h), op| op_size(op, w, h))
    }
}

/// The size after `op`, on an image of `w`×`h`.
fn op_size(op: &Op, w: u32, h: u32) -> (u32, u32) {
    match *op {
        Op::RotateRight | Op::RotateLeft => (h, w),
        Op::FlipH | Op::FlipV => (w, h),
        Op::Crop { x, y, w: cw, h: ch } => crop_rect(w, h, x, y, cw, ch).map(|(_, _, a, b)| (a, b)).unwrap_or((w, h)),
        Op::Rotate { degrees } => straighten_size(degrees, w, h),
        Op::Perspective { corners } => match Homography::of(corners, w, h) {
            Some(_) => perspective_size(corners, w, h),
            None => (w, h),
        },
    }
}

/// Where a point (in fractions) of the `w`×`h` image goes after `op`.
fn op_point(op: &Op, w: u32, h: u32, p: [f32; 2]) -> [f32; 2] {
    let (fw, fh) = (w as f32, h as f32);
    match *op {
        Op::RotateRight => [1.0 - p[1], p[0]],
        Op::RotateLeft => [p[1], 1.0 - p[0]],
        Op::FlipH => [1.0 - p[0], p[1]],
        Op::FlipV => [p[0], 1.0 - p[1]],
        Op::Crop { x, y, w: cw, h: ch } => match crop_rect(w, h, x, y, cw, ch) {
            Some((l, t, pw, ph)) => [(p[0] * fw - l as f32) / pw as f32, (p[1] * fh - t as f32) / ph as f32],
            None => p,
        },
        Op::Rotate { degrees } => {
            let (ow, oh) = straighten_size(degrees, w, h);
            let (sin, cos) = straighten_angle(degrees).sin_cos();
            let (dx, dy) = ((p[0] - 0.5) * fw, (p[1] - 0.5) * fh);
            [(dx * cos - dy * sin) / ow as f32 + 0.5, (dx * sin + dy * cos) / oh as f32 + 0.5]
        }
        Op::Perspective { corners } => match Homography::of(corners, w, h) {
            Some(hm) => hm.to_unit(p[0] * fw, p[1] * fh).map(|uv| [uv[0], uv[1]]).unwrap_or(p),
            None => p,
        },
    }
}

/// The straighten angle in radians, kept within ±45°.
fn straighten_angle(degrees: f32) -> f32 {
    if degrees.is_finite() { degrees.clamp(-45.0, 45.0).to_radians() } else { 0.0 }
}

/// The largest picture of the same shape inside the `w`×`h` image turned by `degrees`.
fn straighten_size(degrees: f32, w: u32, h: u32) -> (u32, u32) {
    let (sin, cos) = straighten_angle(degrees).abs().sin_cos();
    let (fw, fh) = (w as f32, h as f32);
    let s = (fw / (fw * cos + fh * sin)).min(fh / (fw * sin + fh * cos));
    (((fw * s).round() as u32).max(1), ((fh * s).round() as u32).max(1))
}

/// A perspective crop's size: the average lengths of its opposite edges, in pixels.
fn perspective_size(corners: [[f32; 2]; 4], w: u32, h: u32) -> (u32, u32) {
    let px = corners.map(|c| [c[0].clamp(0.0, 1.0) * w as f32, c[1].clamp(0.0, 1.0) * h as f32]);
    let len = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let width = (len(px[0], px[1]) + len(px[3], px[2])) / 2.0;
    let height = (len(px[0], px[3]) + len(px[1], px[2])) / 2.0;
    ((width.round() as u32).clamp(1, MAX_SIDE), (height.round() as u32).clamp(1, MAX_SIDE))
}

/// The projective map from the unit square to a quadrilateral (in pixels), Heckbert's formulas.
struct Homography {
    m: [f32; 9],
}

impl Homography {
    /// For corners in fractions of a `w`×`h` image; None if they don't make a usable shape.
    fn of(corners: [[f32; 2]; 4], w: u32, h: u32) -> Option<Self> {
        if corners.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        let [p0, p1, p2, p3] = corners.map(|c| [c[0].clamp(0.0, 1.0) * w as f32, c[1].clamp(0.0, 1.0) * h as f32]);
        let (dx1, dx2, dx3) = (p1[0] - p2[0], p3[0] - p2[0], p0[0] - p1[0] + p2[0] - p3[0]);
        let (dy1, dy2, dy3) = (p1[1] - p2[1], p3[1] - p2[1], p0[1] - p1[1] + p2[1] - p3[1]);
        let den = dx1 * dy2 - dx2 * dy1;
        let (g, hh) = if dx3.abs() < 1e-6 && dy3.abs() < 1e-6 { (0.0, 0.0) } else { ((dx3 * dy2 - dx2 * dy3) / den, (dx1 * dy3 - dx3 * dy1) / den) };
        let m = [p1[0] - p0[0] + g * p1[0], p3[0] - p0[0] + hh * p3[0], p0[0], p1[1] - p0[1] + g * p1[1], p3[1] - p0[1] + hh * p3[1], p0[1], g, hh, 1.0];
        // A shape with no area (or folded over itself) has no inverse worth using.
        let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
        (m.iter().all(|v| v.is_finite()) && det.abs() > 1e-3).then_some(Self { m })
    }

    /// The point of the quadrilateral (in pixels) at `u`, `v` of the square.
    fn from_unit(&self, u: f32, v: f32) -> [f32; 2] {
        let m = &self.m;
        let z = m[6] * u + m[7] * v + 1.0;
        [(m[0] * u + m[1] * v + m[2]) / z, (m[3] * u + m[4] * v + m[5]) / z]
    }

    /// Where a pixel of the quadrilateral is in the square (the inverse map).
    fn to_unit(&self, x: f32, y: f32) -> Option<[f32; 2]> {
        let m = &self.m;
        // The adjugate of the 3×3 matrix is enough: the scale cancels in the division.
        let a = [m[4] - m[5] * m[7], m[2] * m[7] - m[1], m[1] * m[5] - m[2] * m[4], m[5] * m[6] - m[3], m[0] - m[2] * m[6], m[2] * m[3] - m[0] * m[5], m[3] * m[7] - m[4] * m[6], m[1] * m[6] - m[0] * m[7], m[0] * m[4] - m[1] * m[3]];
        let z = a[6] * x + a[7] * y + a[8];
        (z.abs() > 1e-9).then(|| [(a[0] * x + a[1] * y + a[2]) / z, (a[3] * x + a[4] * y + a[5]) / z])
    }
}

/// A new `ow`×`oh` picture whose pixel (x, y) comes from `source(x, y)` in `image` (bilinear,
/// the edge repeated past the border).
fn warp(image: &DynamicImage, ow: u32, oh: u32, source: impl Fn(f32, f32) -> [f32; 2] + Sync) -> DynamicImage {
    let src = image.to_rgba8();
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let mut out = RgbaImage::new(ow, oh);
    out.par_chunks_mut(ow as usize * 4).enumerate().for_each(|(y, row)| {
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let [fx, fy] = source(x as f32 + 0.5, y as f32 + 0.5);
            let (fx, fy) = (fx - 0.5, fy - 0.5);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let at = |dx: i64, dy: i64| src.get_pixel((x0 as i64 + dx).clamp(0, sw - 1) as u32, (y0 as i64 + dy).clamp(0, sh - 1) as u32).0;
            let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
            for i in 0..4 {
                let top = a[i] as f32 + (b[i] as f32 - a[i] as f32) * tx;
                let bottom = c[i] as f32 + (d[i] as f32 - c[i] as f32) * tx;
                px[i] = (top + (bottom - top) * ty).round() as u8;
            }
        }
    });
    DynamicImage::ImageRgba8(out)
}

/// A crop in fractions as whole pixels inside the image, at least 1×1; None if it's empty.
fn crop_rect(w: u32, h: u32, x: f32, y: f32, cw: f32, ch: f32) -> Option<(u32, u32, u32, u32)> {
    let clamp = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let (x0, y0) = (clamp(x), clamp(y));
    let (x1, y1) = (clamp(x + cw), clamp(y + ch));
    let px = |f: f32, size: u32| (f * size as f32).round() as u32;
    let (left, top) = (
        px(x0, w).min(w.saturating_sub(1)),
        px(y0, h).min(h.saturating_sub(1)),
    );
    let (right, bottom) = (
        px(x1, w).max(left + 1).min(w),
        px(y1, h).max(top + 1).min(h),
    );
    (x1 > x0 && y1 > y0).then_some((left, top, right - left, bottom - top))
}

/// Applies the edits to a copy of `image`.
pub fn apply(image: &DynamicImage, edits: &Edits) -> DynamicImage {
    let mut out = image.clone();
    // The size before each operation, to carry strokes through the ones made after them.
    let mut sizes = Vec::with_capacity(edits.ops.len());
    for op in &edits.ops {
        sizes.push((out.width(), out.height()));
        out = match *op {
            Op::RotateRight => out.rotate90(),
            Op::RotateLeft => out.rotate270(),
            Op::FlipH => out.fliph(),
            Op::FlipV => out.flipv(),
            Op::Crop { x, y, w, h } => match crop_rect(out.width(), out.height(), x, y, w, h) {
                Some((l, t, cw, ch)) => out.crop_imm(l, t, cw, ch),
                None => out,
            },
            Op::Rotate { degrees } if straighten_angle(degrees) != 0.0 => {
                let (w, h) = (out.width(), out.height());
                let (ow, oh) = straighten_size(degrees, w, h);
                let (sin, cos) = straighten_angle(degrees).sin_cos();
                // Each new pixel, turned back into the old picture.
                warp(&out, ow, oh, |x, y| {
                    let (ox, oy) = (x - ow as f32 / 2.0, y - oh as f32 / 2.0);
                    [ox * cos + oy * sin + w as f32 / 2.0, -ox * sin + oy * cos + h as f32 / 2.0]
                })
            }
            Op::Rotate { .. } => out,
            Op::Perspective { corners } => match Homography::of(corners, out.width(), out.height()) {
                Some(hm) => {
                    let (ow, oh) = perspective_size(corners, out.width(), out.height());
                    warp(&out, ow, oh, |x, y| hm.from_unit(x / ow as f32, y / oh as f32))
                }
                None => out,
            },
        };
    }
    if edits.adjust != Adjust::default() {
        let mut rgba = out.into_rgba8();
        adjust(&mut rgba, &edits.adjust);
        if let Some(lut) = edits.adjust.lut.as_deref().and_then(cached_lut) {
            lut.apply(&mut rgba);
        }
        filters(&mut rgba, &edits.adjust);
        out = DynamicImage::ImageRgba8(rgba);
    }
    if !edits.areas.is_empty() {
        let mut rgba = out.into_rgba8();
        // Blur and pixelate first, so text written on a hidden part stays readable.
        let (hide, stamps): (Vec<&Area>, Vec<&Area>) = edits.areas.iter().take(MAX_STROKES).partition(|a| !matches!(a.kind, AreaKind::Stamp { .. }));
        for area in hide.into_iter().chain(stamps) {
            draw_area(&mut rgba, area, &edits.ops, &sizes);
        }
        out = DynamicImage::ImageRgba8(rgba);
    }
    if !edits.strokes.is_empty() {
        let mut rgba = out.into_rgba8();
        for stroke in edits.strokes.iter().take(MAX_STROKES) {
            draw_stroke(&mut rgba, stroke, &edits.ops, &sizes);
        }
        out = DynamicImage::ImageRgba8(rgba);
    }
    out
}

/// `area` carried through the operations made after it (its middle moves like a point, its size in
/// pixels turns with quarter turns; a stamp turns and flips with the picture), then applied.
fn draw_area(image: &mut RgbaImage, area: &Area, ops: &[Op], sizes: &[(u32, u32)]) {
    if ![area.x, area.y, area.w, area.h].iter().all(|v| v.is_finite()) || area.w <= 0.0 || area.h <= 0.0 {
        return;
    }
    let mut stamp = match &area.kind {
        AreaKind::Stamp { png } if png.len() <= MAX_STAMP_BASE64 => match base64_decode(png).and_then(|b| image::load_from_memory_with_format(&b, ImageFormat::Png).ok()) {
            Some(img) if img.width() <= 4096 && img.height() <= 4096 => Some(img),
            _ => return,
        },
        AreaKind::Stamp { .. } => return,
        _ => None,
    };
    let at = area.at.min(ops.len());
    let (w0, h0) = sizes.get(at).copied().unwrap_or((image.width(), image.height()));
    let mut center = [area.x + area.w / 2.0, area.y + area.h / 2.0];
    let mut size = [area.w * w0 as f32, area.h * h0 as f32];
    for (op, &(w, h)) in ops.iter().zip(sizes).skip(at) {
        center = op_point(op, w, h, center);
        match op {
            Op::RotateRight | Op::RotateLeft => {
                size = [size[1], size[0]];
                stamp = stamp.map(|s| if matches!(op, Op::RotateRight) { s.rotate90() } else { s.rotate270() });
            }
            Op::FlipH => stamp = stamp.map(|s| s.fliph()),
            Op::FlipV => stamp = stamp.map(|s| s.flipv()),
            _ => {}
        }
    }
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let x0 = (center[0] * iw - size[0] / 2.0).round().max(0.0) as u32;
    let y0 = (center[1] * ih - size[1] / 2.0).round().max(0.0) as u32;
    let x1 = ((center[0] * iw + size[0] / 2.0).round().max(0.0) as u32).min(image.width());
    let y1 = ((center[1] * ih + size[1] / 2.0).round().max(0.0) as u32).min(image.height());
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (rw, rh) = (x1 - x0, y1 - y0);
    match &area.kind {
        AreaKind::Blur => {
            // Strong enough that small text can't be read back.
            let part = image::imageops::crop_imm(image, x0, y0, rw, rh).to_image();
            let blurred = image::imageops::fast_blur(&part, (rw.min(rh) as f32 / 4.0).max(3.0));
            image::imageops::replace(image, &blurred, x0 as i64, y0 as i64);
        }
        AreaKind::Pixelate => {
            let block = ((rw.max(rh) as f32 / 12.0).round() as u32).max(4);
            for by in (y0..y1).step_by(block as usize) {
                for bx in (x0..x1).step_by(block as usize) {
                    let (ex, ey) = ((bx + block).min(x1), (by + block).min(y1));
                    let mut sum = [0u64; 4];
                    for y in by..ey {
                        for x in bx..ex {
                            let p = image.get_pixel(x, y).0;
                            (0..4).for_each(|i| sum[i] += p[i] as u64);
                        }
                    }
                    let n = ((ex - bx) * (ey - by)) as u64;
                    let avg = Rgba(sum.map(|v| (v / n) as u8));
                    for y in by..ey {
                        for x in bx..ex {
                            image.put_pixel(x, y, avg);
                        }
                    }
                }
            }
        }
        AreaKind::Stamp { .. } => {
            let Some(stamp) = stamp else { return };
            // Placed in the whole carried rectangle (part of it may be off the picture).
            let (sw, sh) = ((size[0].round() as u32).max(1), (size[1].round() as u32).max(1));
            if sw > MAX_SIDE || sh > MAX_SIDE {
                return;
            }
            let scaled = stamp.resize_exact(sw, sh, FilterType::Triangle).to_rgba8();
            let left = (center[0] * iw - size[0] / 2.0).round() as i64;
            let top = (center[1] * ih - size[1] / 2.0).round() as i64;
            image::imageops::overlay(image, &scaled, left, top);
        }
    }
}

/// Standard base64 (with or without padding); None if it isn't.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let bytes: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let body = bytes.strip_suffix(b"==").or_else(|| bytes.strip_suffix(b"=")).unwrap_or(&bytes);
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for chunk in body.chunks(4) {
        let v: Vec<u8> = chunk.iter().map(|&c| value(c)).collect::<Option<_>>()?;
        if v.len() == 1 {
            return None;
        }
        let n = v.iter().fold(0u32, |n, &d| (n << 6) | d as u32) << (6 * (4 - v.len()));
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..v.len() - 1]);
    }
    Some(out)
}

/// `stroke` carried through the operations made after it, then painted: a round brush,
/// antialiased, one coverage per pixel (so a stroke crossing itself isn't darker there).
fn draw_stroke(image: &mut RgbaImage, stroke: &Stroke, ops: &[Op], sizes: &[(u32, u32)]) {
    let hex = stroke.color.strip_prefix('#').filter(|h| h.len() == 6).and_then(|h| u32::from_str_radix(h, 16).ok());
    let (Some(rgb), true) = (hex, stroke.width.is_finite() && stroke.opacity.is_finite()) else { return };
    let color = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8].map(|c| c as f32);
    let opacity = stroke.opacity.clamp(0.0, 1.0);
    let mut points: Vec<[f32; 2]> = stroke.points.iter().take(MAX_POINTS).filter(|p| p[0].is_finite() && p[1].is_finite()).copied().collect();
    if points.is_empty() || opacity == 0.0 {
        return;
    }
    let mut width = stroke.width.clamp(0.0, 1.0);
    for (op, &(w, h)) in ops.iter().zip(sizes).skip(stroke.at.min(ops.len())) {
        for p in &mut points {
            *p = op_point(op, w, h, *p);
        }
        // The pixels keep (about) their size: the same width in pixels is a bigger part of a
        // smaller picture.
        let (ow, oh) = op_size(op, w, h);
        width *= w.max(h) as f32 / ow.max(oh) as f32;
    }
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let px: Vec<[f32; 2]> = points.iter().map(|p| [p[0] * iw, p[1] * ih]).collect();
    let r = (width * iw.max(ih) / 2.0).max(0.5);
    // The stroke's box, and its coverage there.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &px {
        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
    }
    let bx0 = (x0 - r - 1.0).floor().max(0.0) as u32;
    let by0 = (y0 - r - 1.0).floor().max(0.0) as u32;
    let bx1 = ((x1 + r + 1.0).ceil().max(0.0) as u32).min(image.width());
    let by1 = ((y1 + r + 1.0).ceil().max(0.0) as u32).min(image.height());
    if bx0 >= bx1 || by0 >= by1 {
        return;
    }
    let bw = (bx1 - bx0) as usize;
    let mut cover = vec![0f32; bw * (by1 - by0) as usize];
    let segments: Vec<([f32; 2], [f32; 2])> = if px.len() == 1 { vec![(px[0], px[0])] } else { px.windows(2).map(|s| (s[0], s[1])).collect() };
    for (a, b) in segments {
        let sx0 = ((a[0].min(b[0]) - r - 1.0).floor().max(bx0 as f32)) as u32;
        let sy0 = ((a[1].min(b[1]) - r - 1.0).floor().max(by0 as f32)) as u32;
        let sx1 = ((a[0].max(b[0]) + r + 1.0).ceil() as u32).min(bx1);
        let sy1 = ((a[1].max(b[1]) + r + 1.0).ceil() as u32).min(by1);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        for y in sy0..sy1 {
            for x in sx0..sx1 {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = if len2 > 0.0 { (((cx - a[0]) * dx + (cy - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let d = (cx - a[0] - t * dx).hypot(cy - a[1] - t * dy);
                let c = (r + 0.5 - d).clamp(0.0, 1.0);
                let i = (y - by0) as usize * bw + (x - bx0) as usize;
                if c > cover[i] {
                    cover[i] = c;
                }
            }
        }
    }
    for y in by0..by1 {
        for x in bx0..bx1 {
            let k = cover[(y - by0) as usize * bw + (x - bx0) as usize] * opacity;
            if k > 0.0 {
                let px = image.get_pixel_mut(x, y);
                for i in 0..3 {
                    px[i] = (px[i] as f32 + (color[i] - px[i] as f32) * k).round() as u8;
                }
                // Drawing on a transparent part shows: it becomes as opaque as the ink.
                px[3] = px[3].max((k * 255.0).round() as u8);
            }
        }
    }
}

/// Reduce noise, sharpen, blur and vignette, in that order. Radii are fractions of the longer side.
fn filters(image: &mut RgbaImage, a: &Adjust) {
    let amount = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let side = image.width().max(image.height()) as f32;
    let (denoise, sharpen, blur) = (amount(a.denoise), amount(a.sharpen), amount(a.blur));
    if denoise > 0.0 {
        // Edge-aware smoothing: each value moves toward its neighbourhood's average, but less where
        // it differs a lot from it (an edge, not grain).
        let smooth = image::imageops::fast_blur(image, (side * 0.0015).max(0.8));
        let limit = 6.0 + 30.0 * denoise;
        blend(image, &smooth, |v, s| {
            let d = s - v;
            v + d * denoise * (-(d / limit).powi(2)).exp()
        });
    }
    if sharpen > 0.0 {
        // Unsharp mask: the difference from a slightly blurred copy, added back.
        let soft = image::imageops::fast_blur(image, (side * 0.001).max(0.7));
        blend(image, &soft, |v, s| v + (v - s) * sharpen * 2.0);
    }
    if blur > 0.0 {
        *image = image::imageops::fast_blur(image, side * 0.02 * blur);
    }
    let vignette = if a.vignette.is_finite() { a.vignette.clamp(-1.0, 1.0) } else { 0.0 };
    if vignette != 0.0 {
        let (w, h) = (image.width() as f32, image.height() as f32);
        let width = image.width() as usize;
        image.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
            for (x, px) in row.chunks_exact_mut(4).enumerate() {
                // 0 in the middle, 1 in the corners; nothing changes inside the inner 40%.
                let (dx, dy) = ((x as f32 + 0.5) / w - 0.5, (y as f32 + 0.5) / h - 0.5);
                let r = ((dx * dx + dy * dy).sqrt() / 0.5f32.hypot(0.5) - 0.4).max(0.0) / 0.6;
                let k = vignette * r * r * (3.0 - 2.0 * r) * 0.8;
                for v in &mut px[..3] {
                    let f = *v as f32;
                    *v = (if k > 0.0 { f * (1.0 - k) } else { f + (255.0 - f) * -k }).round().clamp(0.0, 255.0) as u8;
                }
            }
        });
    }
}

/// Each color value from `f(value, other)`, alpha kept.
fn blend(image: &mut RgbaImage, other: &RgbaImage, f: impl Fn(f32, f32) -> f32 + Sync) {
    image.par_chunks_mut(4 * 1024).zip(other.par_chunks(4 * 1024)).for_each(|(a, b)| {
        for (pa, pb) in a.chunks_exact_mut(4).zip(b.chunks_exact(4)) {
            for i in 0..3 {
                pa[i] = f(pa[i] as f32, pb[i] as f32).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

/// A 3D color table from a `.cube` file (Adobe/Resolve format): `LUT_3D_SIZE n`, optional
/// `DOMAIN_MIN`/`DOMAIN_MAX`, then n³ lines `r g b` with red changing fastest.
#[derive(Debug)]
pub struct Lut {
    size: usize,
    min: [f32; 3],
    max: [f32; 3],
    table: Vec<[f32; 3]>,
}

/// Bigger tables are refused (65 is the usual largest; 128³ is already 2 million colors).
const MAX_LUT_SIZE: usize = 128;

impl Lut {
    pub fn parse(text: &str) -> Result<Self, ImageError> {
        let bad = |why: &str| ImageError::Invalid { message: format!(".cube: {why}") };
        let (mut size, mut min, mut max) = (0usize, [0.0f32; 3], [1.0f32; 3]);
        let mut table = Vec::new();
        let three = |rest: std::str::SplitWhitespace| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.map(|t| t.parse().ok()).collect::<Option<_>>()?;
            (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
        };
        for line in text.lines() {
            let line = line.trim();
            let mut words = line.split_whitespace();
            match words.next() {
                None => {}
                Some(w) if w.starts_with('#') || w == "TITLE" || w == "LUT_IN_VIDEO_RANGE" || w == "LUT_OUT_VIDEO_RANGE" => {}
                Some("LUT_1D_SIZE") => return Err(bad("1D tables aren't supported")),
                Some("LUT_3D_SIZE") => {
                    size = words.next().and_then(|n| n.parse().ok()).filter(|n| (2..=MAX_LUT_SIZE).contains(n)).ok_or_else(|| bad("size"))?;
                    table.reserve(size * size * size);
                }
                Some("DOMAIN_MIN") => min = three(words).ok_or_else(|| bad("DOMAIN_MIN"))?,
                Some("DOMAIN_MAX") => max = three(words).ok_or_else(|| bad("DOMAIN_MAX"))?,
                Some(_) => {
                    let rgb = three(line.split_whitespace()).ok_or_else(|| bad("a line that isn't three numbers"))?;
                    if size == 0 || table.len() == size * size * size {
                        return Err(bad("more colors than LUT_3D_SIZE"));
                    }
                    table.push(rgb);
                }
            }
        }
        if size == 0 || table.len() != size * size * size || (0..3).any(|i| max[i] <= min[i]) {
            return Err(bad("incomplete table"));
        }
        Ok(Self { size, min, max, table })
    }

    /// Every pixel through the table (trilinear between its points), alpha kept.
    fn apply(&self, image: &mut RgbaImage) {
        let n = self.size;
        let at = |r: usize, g: usize, b: usize| self.table[r + g * n + b * n * n];
        image.par_chunks_mut(4 * 1024).for_each(|chunk| {
            for px in chunk.chunks_exact_mut(4) {
                let pos: [f32; 3] = std::array::from_fn(|i| ((px[i] as f32 / 255.0 - self.min[i]) / (self.max[i] - self.min[i])).clamp(0.0, 1.0) * (n - 1) as f32);
                let lo = pos.map(|v| (v.floor() as usize).min(n - 2));
                let t: [f32; 3] = std::array::from_fn(|i| pos[i] - lo[i] as f32);
                let mut out = [0.0f32; 3];
                for corner in 0..8 {
                    let (dr, dg, db) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
                    let wgt = [dr, dg, db].iter().zip(t).map(|(&d, ti)| if d == 1 { ti } else { 1.0 - ti }).product::<f32>();
                    let c = at(lo[0] + dr, lo[1] + dg, lo[2] + db);
                    for i in 0..3 {
                        out[i] += c[i] * wgt;
                    }
                }
                for i in 0..3 {
                    px[i] = (out[i] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        });
    }
}

/// Reads a `.cube` file (at most 100 MB) and checks it.
pub fn load_lut(path: &Path) -> Result<Lut, ImageError> {
    if fs::metadata(path)?.len() > 100 << 20 {
        return Err(ImageError::Invalid { message: ".cube: too big".into() });
    }
    Lut::parse(&String::from_utf8_lossy(&fs::read(path)?))
}

/// The tables already read, by path: a slider move redraws without reading the file again.
fn cached_lut(path: &str) -> Option<std::sync::Arc<Lut>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Lut>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(lut) = cache.lock().unwrap().get(path) {
        return Some(lut.clone());
    }
    let lut = Arc::new(load_lut(Path::new(path)).ok()?);
    cache.lock().unwrap().insert(path.to_owned(), lut.clone());
    Some(lut)
}

/// On 8-bit sRGB, alpha untouched, in this order: exposure, levels, curve, temperature and tint (one
/// lookup table per channel); shadows and highlights (by the pixel's luma); CSS `brightness()
/// contrast()` (a table) and `saturate()`; vibrance; black and white.
fn adjust(image: &mut RgbaImage, a: &Adjust) {
    let s1 = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
    let b = 1.0 + s1(a.brightness);
    let c = 1.0 + s1(a.contrast);
    let s = 1.0 + s1(a.saturation);
    let gain = 2f32.powf(2.0 * s1(a.exposure));
    let (temp, tint) = (s1(a.temperature), s1(a.tint));
    let white_balance = [1.0 + 0.2 * temp + 0.05 * tint, 1.0 - 0.15 * tint, 1.0 - 0.2 * temp + 0.05 * tint];
    let curve = Curve::new(&a.curve);
    let l = a.levels;
    let gamma = if l.gamma.is_finite() { l.gamma.clamp(0.1, 10.0) } else { 1.0 };
    // Exposure, levels, curve and white balance, per channel (0..1).
    let tone: [Vec<f32>; 3] = std::array::from_fn(|ch| {
        let (lo, hi) = (l.black[ch].clamp(0.0, 1.0), l.white[ch].clamp(0.0, 1.0));
        let span = (hi - lo).max(1.0 / 255.0);
        (0..256)
            .map(|v| {
                let mut x = (v as f32 / 255.0 * gain).min(1.0);
                x = ((x - lo) / span).clamp(0.0, 1.0);
                if gamma != 1.0 {
                    x = x.powf(1.0 / gamma);
                }
                curve.at(x) * white_balance[ch]
            })
            .collect()
    });
    let (shadows, highlights, vibrance) = (s1(a.shadows), s1(a.highlights), s1(a.vibrance));
    // Brightness then contrast, on 0..1, to 0..255.
    let bc = |x: f32| ((x * b - 0.5) * c + 0.5).clamp(0.0, 1.0) * 255.0;
    // The saturate() matrix from the Filter Effects spec.
    let m = [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ];
    let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    image.par_chunks_mut(4 * 1024).for_each(|chunk| {
        for px in chunk.chunks_exact_mut(4) {
            let mut p = [tone[0][px[0] as usize], tone[1][px[1] as usize], tone[2][px[2] as usize]];
            if shadows != 0.0 || highlights != 0.0 {
                let y = luma(p).clamp(0.0, 1.0);
                let lift = shadows * 0.4 * (1.0 - y).powi(3) + highlights * 0.4 * y.powi(3);
                p = p.map(|v| v + lift);
            }
            let [r, g, bl] = p.map(bc);
            let mut out = m.map(|row| row[0] * r + row[1] * g + row[2] * bl);
            if vibrance != 0.0 {
                let max = out[0].max(out[1]).max(out[2]);
                let min = out[0].min(out[1]).min(out[2]);
                let k = 1.0 + vibrance * (1.0 - (max - min) / 255.0);
                let y = luma(out);
                out = out.map(|v| y + (v - y) * k);
            }
            if a.grayscale {
                out = [luma(out); 3];
            }
            for i in 0..3 {
                px[i] = out[i].round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

/// A smooth tone curve through the points that never overshoots between them (monotone cubic,
/// Fritsch–Carlson); flat past the first and last point. Fewer than two points: unchanged.
struct Curve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    slopes: Vec<f32>,
}

impl Curve {
    fn new(points: &[[f32; 2]]) -> Self {
        let mut pts: Vec<[f32; 2]> = points.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]).collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4);
        let (xs, ys): (Vec<f32>, Vec<f32>) = pts.iter().map(|p| (p[0], p[1])).unzip();
        let n = xs.len();
        if n < 2 {
            return Self { xs: vec![], ys: vec![], slopes: vec![] };
        }
        let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
        let mut slopes = vec![0.0; n];
        slopes[0] = d[0];
        slopes[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            slopes[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
        }
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                slopes[i] = 0.0;
                slopes[i + 1] = 0.0;
                continue;
            }
            let (a, b) = (slopes[i] / d[i], slopes[i + 1] / d[i]);
            let h = a.hypot(b);
            if h > 3.0 {
                slopes[i] = 3.0 / h * a * d[i];
                slopes[i + 1] = 3.0 / h * b * d[i];
            }
        }
        Self { xs, ys, slopes }
    }

    fn at(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if n < 2 {
            return x;
        }
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = self.xs.partition_point(|&v| v <= x) - 1;
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i] + (t3 - 2.0 * t2 + t) * h * self.slopes[i] + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1] + (t3 - t2) * h * self.slopes[i + 1]
    }
}

/// How many pixels have each value, per channel (R, G, B) and for the luma, out of 256 bins:
/// what the Levels panel draws and the automatic levels read.
pub fn histogram(image: &DynamicImage) -> [Vec<u32>; 4] {
    let rgba = image.to_rgba8();
    let mut out: [Vec<u32>; 4] = std::array::from_fn(|_| vec![0; 256]);
    for px in rgba.pixels() {
        if px[3] == 0 {
            continue; // transparent pixels have no tone
        }
        let [r, g, b, _] = px.0;
        out[0][r as usize] += 1;
        out[1][g as usize] += 1;
        out[2][b as usize] += 1;
        let y = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as usize;
        out[3][y.min(255)] += 1;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Avif,
    Bmp,
    Tiff,
    Gif,
    Ico,
    Qoi,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Bmp => "bmp",
            Self::Tiff => "tif",
            Self::Gif => "gif",
            Self::Ico => "ico",
            Self::Qoi => "qoi",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodeOptions {
    pub format: Format,
    /// 1..100, for JPEG, lossy WebP and AVIF.
    pub quality: u8,
    /// WebP without loss (bigger files).
    pub lossless: bool,
    /// Shrink so the longer side is at most this (never enlarges).
    pub max_side: Option<u32>,
}

/// Encodes `image` (already edited) as a file's bytes.
pub fn encode(image: &DynamicImage, options: &EncodeOptions) -> Result<Vec<u8>, ImageError> {
    let mut image = std::borrow::Cow::Borrowed(image);
    let mut max_side = options.max_side;
    if options.format == Format::Ico {
        max_side = Some(max_side.unwrap_or(256).min(256)); // ICO can't hold more than 256×256
    }
    if let Some(max) = max_side.filter(|&m| m > 0 && image.width().max(image.height()) > m) {
        image = std::borrow::Cow::Owned(image.resize(max, max, FilterType::Lanczos3));
    }
    let quality = options.quality.clamp(1, 100);
    let mut out = Vec::new();
    match options.format {
        Format::Jpeg => {
            // No alpha in JPEG: flatten on white.
            let rgb = flatten_on_white(&image);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .encode_image(&rgb)?;
        }
        Format::Webp => {
            let rgba = image.to_rgba8();
            let encoder =
                webp::Encoder::new(&rgba, webp::PixelLayout::Rgba, rgba.width(), rgba.height());
            let memory = if options.lossless {
                encoder.encode_lossless()
            } else {
                encoder.encode(quality as f32)
            };
            out.extend_from_slice(&memory);
        }
        Format::Avif => {
            use rgb::FromSlice;
            let rgba = image.to_rgba8();
            let encoded = ravif::Encoder::new()
                .with_quality(quality as f32)
                .with_speed(6)
                .encode_rgba(ravif::Img::new(
                    rgba.as_raw().as_rgba(),
                    rgba.width() as usize,
                    rgba.height() as usize,
                ))
                .map_err(|e| ImageError::Invalid {
                    message: e.to_string(),
                })?;
            out = encoded.avif_file;
        }
        Format::Gif => {
            // GIF is 8-bit color: the encoder quantizes.
            image
                .to_rgba8()
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Gif)?;
        }
        Format::Png | Format::Bmp | Format::Tiff | Format::Ico | Format::Qoi => {
            let format = match options.format {
                Format::Png => ImageFormat::Png,
                Format::Bmp => ImageFormat::Bmp,
                Format::Tiff => ImageFormat::Tiff,
                Format::Ico => ImageFormat::Ico,
                _ => ImageFormat::Qoi,
            };
            // Each encoder takes 8-bit RGB(A); 16-bit and float sources are converted. ICO always
            // gets RGBA: readers (image's own included) expect the PNG inside an icon to have alpha.
            let rgba = image.color().has_alpha() || options.format == Format::Ico;
            let converted = if rgba {
                DynamicImage::ImageRgba8(image.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(image.to_rgb8())
            };
            converted.write_to(&mut Cursor::new(&mut out), format)?;
        }
    }
    Ok(out)
}

fn flatten_on_white(image: &DynamicImage) -> image::RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        image::Rgb([0, 1, 2].map(|i| ((p[i] as u32 * a + 255 * (255 - a)) / 255) as u8))
    })
}

/// Writes `bytes` as `<dir>/<stem>.<ext>`, or `<stem> (2).<ext>`… if that exists: nothing is ever
/// overwritten. Written to a temporary file first, so a failed write leaves no half file.
pub fn write_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf, ImageError> {
    let tmp = dir.join(format!(".{stem}.{ext}.jaye-tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    for n in 1..10_000 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let target = dir.join(name);
        // hard_link fails if the target exists, so two jobs can't take the same name.
        match fs::hard_link(&tmp, &target) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(ImageError::Io {
        message: "no free file name".into(),
    })
}

/// Decodes `src`, applies `edits`, encodes and writes the result in `dir`.
pub fn convert_file(
    src: &Path,
    edits: &Edits,
    options: &EncodeOptions,
    dir: &Path,
) -> Result<PathBuf, ImageError> {
    let image = apply(&decode(src)?, edits);
    let bytes = encode(&image, options)?;
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    write_new(dir, &stem, options.format.extension(), &bytes)
}

/// RGBA as PNG with the Up filter and fdeflate's fastest mode, for the page (not for files): a
/// 1350 px text page goes from 10 MB to ~0.8 MB in ~6 ms (spike S1), quicker to hand over than
/// raw pixels through the WebView2 protocol.
pub fn fast_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 8);
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fastest);
    encoder.set_filter(png::Filter::Up);
    // Writing into a Vec only fails on a size mismatch, which callers never make.
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .expect("PNG of a bitmap");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageEncoder, Rgba};

    fn dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "jay-docs-image-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 40×20: left half red, right half blue, top-left pixel white.
    fn sample() -> DynamicImage {
        let mut img = RgbaImage::from_fn(40, 20, |x, _| {
            if x < 20 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn geometry() {
        let img = sample();
        let px = |i: &DynamicImage, x, y| i.get_pixel(x, y).0;
        let right = apply(
            &img,
            &Edits {
                ops: vec![Op::RotateRight],
                ..Default::default()
            },
        );
        assert_eq!(right.dimensions(), (20, 40));
        assert_eq!(
            px(&right, 19, 0),
            [255, 255, 255, 255],
            "top-left goes top-right"
        );
        let flipped = apply(
            &img,
            &Edits {
                ops: vec![Op::FlipH],
                ..Default::default()
            },
        );
        assert_eq!(px(&flipped, 39, 0), [255, 255, 255, 255]);
        // Crop the right half: all blue.
        let edits = Edits {
            ops: vec![Op::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            }],
            ..Default::default()
        };
        let crop = apply(&img, &edits);
        assert_eq!(crop.dimensions(), (20, 20));
        assert_eq!(edits.size(40, 20), (20, 20));
        assert!(crop.to_rgba8().pixels().all(|p| p.0 == [0, 0, 255, 255]));
        // The same edits at another resolution crop the same part.
        let big = sample().resize_exact(400, 200, FilterType::Nearest);
        assert_eq!(apply(&big, &edits).dimensions(), (200, 200));
        // Rotate then crop: the crop is in the rotated image's fractions.
        let both = Edits {
            ops: vec![
                Op::RotateRight,
                Op::Crop {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 0.5,
                },
            ],
            ..Default::default()
        };
        assert_eq!(apply(&img, &both).dimensions(), (20, 20));
        assert_eq!(both.size(40, 20), (20, 20));
    }

    #[test]
    fn crops_never_leave_the_image() {
        assert_eq!(
            crop_rect(100, 50, -1.0, -1.0, 5.0, 5.0),
            Some((0, 0, 100, 50))
        );
        assert_eq!(
            crop_rect(100, 50, 0.999, 0.999, 0.0001, 0.0001),
            Some((99, 49, 1, 1))
        );
        assert_eq!(crop_rect(100, 50, 0.5, 0.5, 0.0, 0.3), None);
        assert_eq!(
            crop_rect(100, 50, f32::NAN, 0.0, 1.0, 1.0),
            None,
            "a broken crop is ignored"
        );
    }

    #[test]
    fn adjustments_follow_css_filters() {
        let gray =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([100, 150, 200, 128])));
        let none = apply(&gray, &Edits::default());
        assert_eq!(none.get_pixel(0, 0).0, [100, 150, 200, 128]);
        // brightness(1.5): ×1.5, alpha kept.
        let bright = apply(
            &gray,
            &Edits {
                adjust: Adjust {
                    brightness: 0.5,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(bright.get_pixel(0, 0).0, [150, 225, 255, 128]);
        // saturate(0): the luma gray.
        let gray_out = apply(
            &gray,
            &Edits {
                adjust: Adjust {
                    saturation: -1.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .get_pixel(0, 0)
        .0;
        assert_eq!(gray_out[0], gray_out[1]);
        assert_eq!(gray_out[1], gray_out[2]);
        // contrast(0): everything mid-gray.
        let flat = apply(
            &gray,
            &Edits {
                adjust: Adjust {
                    contrast: -1.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .get_pixel(0, 0)
        .0;
        assert_eq!(&flat[..3], &[128, 128, 128]);
    }

    #[test]
    fn tone_and_color() {
        let px = |a: Adjust, rgb: [u8; 3]| apply(&DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([rgb[0], rgb[1], rgb[2], 200]))), &Edits { adjust: a, ..Default::default() }).get_pixel(0, 0).0;
        // An older list of edits (only the three CSS sliders) still reads, the rest unchanged.
        let old: Adjust = serde_json::from_str(r#"{"brightness":0,"contrast":0,"saturation":0}"#).unwrap();
        assert_eq!(old, Adjust::default());
        // Exposure +0.5 = one stop: ×2.
        assert_eq!(px(Adjust { exposure: 0.5, ..Default::default() }, [50, 100, 120]), [100, 200, 240, 200]);
        // Levels: 0.2 becomes black and 0.6 white; a mid value in between is stretched.
        let levels = Levels { black: [0.2; 3], white: [0.6; 3], gamma: 1.0 };
        let stretched = px(Adjust { levels, ..Default::default() }, [51, 153, 102]);
        assert!(stretched[..2] == [0, 255] && (127..=128).contains(&stretched[2]), "{stretched:?}");
        // Gamma 2 brightens the midtones: 0.25 → 0.5.
        assert_eq!(px(Adjust { levels: Levels { gamma: 2.0, ..Default::default() }, ..Default::default() }, [64, 64, 64])[0], 128);
        // A curve through (0.5, 0.75) lifts the middle and keeps the ends.
        let curve = vec![[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]];
        let lifted = px(Adjust { curve: curve.clone(), ..Default::default() }, [0, 128, 255]);
        assert_eq!((lifted[0], lifted[2]), (0, 255));
        assert!((190..=192).contains(&lifted[1]), "{lifted:?}");
        // Monotone: never goes down between the points.
        let c = Curve::new(&curve);
        assert!((0..100).all(|i| c.at(i as f32 / 100.0) <= c.at((i + 1) as f32 / 100.0)));
        // Warmer: more red, less blue.
        let warm = px(Adjust { temperature: 0.5, ..Default::default() }, [100, 100, 100]);
        assert!(warm[0] > 100 && warm[2] < 100 && warm[1] == 100, "{warm:?}");
        // Shadows lift the dark more than the light.
        let dark = px(Adjust { shadows: 1.0, ..Default::default() }, [20, 20, 20])[0] as i32 - 20;
        let light = px(Adjust { shadows: 1.0, ..Default::default() }, [220, 220, 220])[0] as i32 - 220;
        assert!(dark > 50 && light < 5, "{dark} {light}");
        // Vibrance boosts a dull color more than a vivid one.
        let dull = px(Adjust { vibrance: 1.0, ..Default::default() }, [120, 100, 100]);
        let vivid = px(Adjust { vibrance: 1.0, ..Default::default() }, [250, 10, 10]);
        assert!(dull[0] as i32 - dull[1] as i32 > 30 && vivid[0] == 255);
        // Black and white: one gray, alpha kept.
        let bw = px(Adjust { grayscale: true, ..Default::default() }, [200, 50, 10]);
        assert!(bw[0] == bw[1] && bw[1] == bw[2] && bw[3] == 200);
    }

    #[test]
    fn filters_and_color_looks() {
        let run = |img: &RgbaImage, a: Adjust| apply(&DynamicImage::ImageRgba8(img.clone()), &Edits { adjust: a, ..Default::default() }).into_rgba8();
        // A hard edge: black on the left, white on the right.
        let edge = RgbaImage::from_fn(64, 8, |x, _| if x < 32 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) });
        let blurred = run(&edge, Adjust { blur: 0.5, ..Default::default() });
        assert!(blurred.get_pixel(31, 4)[0] > 40 && blurred.get_pixel(32, 4)[0] < 215, "blur softens the edge");
        // Sharpening a soft edge makes it steeper.
        let soft = image::imageops::fast_blur(&edge, 2.0);
        let sharp = run(&soft, Adjust { sharpen: 1.0, ..Default::default() });
        let step = |i: &RgbaImage| i.get_pixel(33, 4)[0] as i32 - i.get_pixel(30, 4)[0] as i32;
        assert!(step(&sharp) > step(&soft), "{} vs {}", step(&sharp), step(&soft));
        // Reducing noise smooths grain but keeps the edge.
        let noisy = RgbaImage::from_fn(64, 8, |x, y| {
            let base = if x < 32 { 60 } else { 200 };
            let grain = if (x * 7 + y * 13) % 3 == 0 { 10 } else { 0 };
            Rgba([base + grain, base + grain, base + grain, 255])
        });
        let clean = run(&noisy, Adjust { denoise: 1.0, ..Default::default() });
        let spread = |i: &RgbaImage| (2..28).map(|x| i.get_pixel(x, 4)[0]).max().unwrap() - (2..28).map(|x| i.get_pixel(x, 4)[0]).min().unwrap();
        assert!(spread(&clean) < spread(&noisy) && clean.get_pixel(40, 4)[0] > 180);
        // Vignette: corners darker, the middle untouched.
        let gray = RgbaImage::from_pixel(40, 40, Rgba([128, 128, 128, 255]));
        let v = run(&gray, Adjust { vignette: 1.0, ..Default::default() });
        assert!(v.get_pixel(0, 0)[0] < 60 && v.get_pixel(20, 20)[0] == 128);
        // A .cube that swaps red and blue, and one that's broken.
        let mut cube = String::from("TITLE \"swap\"\n# comment\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    cube.push_str(&format!("{b} {g} {r}\n"));
                }
            }
        }
        let lut = Lut::parse(&cube).unwrap();
        let mut px = RgbaImage::from_pixel(1, 1, Rgba([200, 100, 50, 77]));
        lut.apply(&mut px);
        assert_eq!(px.get_pixel(0, 0).0, [50, 100, 200, 77]);
        assert!(Lut::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err() && Lut::parse("LUT_1D_SIZE 4\n").is_err() && Lut::parse("LUT_3D_SIZE 99999\n").is_err());
        // A path that isn't a table changes nothing (and doesn't fail the edit).
        let missing = run(&gray, Adjust { lut: Some("C:/no/such.cube".into()), ..Default::default() });
        assert_eq!(missing.get_pixel(5, 5)[0], 128);
    }

    #[test]
    fn strokes_are_drawn_and_follow_later_edits() {
        let white = DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 50, Rgba([255, 255, 255, 255])));
        let line = |at: usize, opacity: f32| Stroke { at, color: "#ff0000".into(), width: 0.04, opacity, points: vec![[0.1, 0.5], [0.4, 0.5]] };
        let draw = |ops: Vec<Op>, strokes: Vec<Stroke>| apply(&white, &Edits { ops, strokes, ..Default::default() }).into_rgba8();
        // A red pen line across the left part, 4 px thick, nothing elsewhere.
        let pen = draw(vec![], vec![line(0, 1.0)]);
        assert_eq!(pen.get_pixel(25, 25).0, [255, 0, 0, 255]);
        assert_eq!(pen.get_pixel(25, 20).0, [255, 255, 255, 255]);
        assert_eq!(pen.get_pixel(70, 25).0, [255, 255, 255, 255]);
        // A highlighter lets the picture through.
        assert_eq!(draw(vec![], vec![line(0, 0.5)]).get_pixel(25, 25).0, [255, 128, 128, 255]);
        // Drawn, then the picture flipped: the line goes to the right part.
        let flipped = draw(vec![Op::FlipH], vec![line(0, 1.0)]);
        assert_eq!((flipped.get_pixel(75, 25)[1], flipped.get_pixel(25, 25)[1]), (0, 255));
        // Drawn after the flip (at = 1): it stays where it was drawn.
        assert_eq!(draw(vec![Op::FlipH], vec![line(1, 1.0)]).get_pixel(25, 25)[1], 0);
        // Rotated right after drawing: the horizontal line becomes vertical, in the image's upper part.
        let turned = draw(vec![Op::RotateRight], vec![line(0, 1.0)]);
        assert_eq!(turned.dimensions(), (50, 100));
        assert_eq!((turned.get_pixel(25, 25)[1], turned.get_pixel(10, 25)[1]), (0, 255));
        // Cropped to the left half: still under the same pixels, and as thick as before.
        let cropped = draw(vec![Op::Crop { x: 0.0, y: 0.0, w: 0.5, h: 1.0 }], vec![line(0, 1.0)]);
        assert_eq!((cropped.get_pixel(25, 25)[1], cropped.get_pixel(25, 23)[1], cropped.get_pixel(25, 22)[1]), (0, 0, 255));
        // A bad color draws nothing.
        assert_eq!(draw(vec![], vec![Stroke { color: "red".into(), ..line(0, 1.0) }]).get_pixel(25, 25).0, [255, 255, 255, 255]);
        // An older list (no strokes) still reads.
        let old: Edits = serde_json::from_str(r#"{"ops":[],"adjust":{}}"#).unwrap();
        assert!(old.strokes.is_empty() && old.is_empty());
    }

    #[test]
    fn straighten_and_perspective() {
        // A 200×100 picture: left half red, right half blue.
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(200, 100, |x, _| if x < 100 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) }));
        let run = |ops: Vec<Op>| apply(&img, &Edits { ops, ..Default::default() }).into_rgba8();
        // 10°: smaller, same shape, no empty (transparent) corners.
        let turned = run(vec![Op::Rotate { degrees: 10.0 }]);
        let (w, h) = turned.dimensions();
        assert!(w < 200 && h < 100 && (w as f32 / h as f32 - 2.0).abs() < 0.05, "{w}×{h}");
        assert_eq!(Edits { ops: vec![Op::Rotate { degrees: 10.0 }], ..Default::default() }.size(200, 100), (w, h));
        assert!([(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)].iter().all(|&(x, y)| turned.get_pixel(x, y)[3] == 255));
        // Clockwise: the top-left corner now shows red from lower down, the middle seam tilts right going down.
        let seam = |y: u32| (0..w).find(|&x| turned.get_pixel(x, y)[2] > 128).unwrap();
        assert!(seam(h - 5) < seam(5), "clockwise moves the bottom of the seam left");
        // 0° and NaN change nothing; 90° is held to 45°.
        assert_eq!(run(vec![Op::Rotate { degrees: 0.0 }]).dimensions(), (200, 100));
        assert_eq!(run(vec![Op::Rotate { degrees: f32::NAN }]).dimensions(), (200, 100));
        assert_eq!(straighten_size(90.0, 200, 100), straighten_size(45.0, 200, 100));
        // Perspective: the right half's corners become the whole picture: all blue, 100×100.
        let half = run(vec![Op::Perspective { corners: [[0.5, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 1.0]] }]);
        assert_eq!(half.dimensions(), (100, 100));
        assert!(half.pixels().all(|p| p[2] > 200 && p[0] < 50));
        // A tilted quadrilateral maps its corners to the corners, and back.
        let quad = [[0.1, 0.2], [0.8, 0.05], [0.95, 0.9], [0.2, 0.8]];
        let hm = Homography::of(quad, 200, 100).unwrap();
        for (corner, uv) in quad.iter().zip([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) {
            let p = hm.from_unit(uv[0], uv[1]);
            assert!((p[0] - corner[0] * 200.0).abs() < 0.01 && (p[1] - corner[1] * 100.0).abs() < 0.01);
            let back = hm.to_unit(p[0], p[1]).unwrap();
            assert!((back[0] - uv[0]).abs() < 1e-3 && (back[1] - uv[1]).abs() < 1e-3);
        }
        // Corners in a line make no shape: the picture is left alone.
        assert_eq!(run(vec![Op::Perspective { corners: [[0.1, 0.1], [0.5, 0.5], [0.9, 0.9], [0.3, 0.3]] }]).dimensions(), (200, 100));
        // A stroke drawn before a perspective crop of the right half moves with it.
        let stroke = Stroke { at: 0, color: "#00ff00".into(), width: 0.02, opacity: 1.0, points: vec![[0.75, 0.5]] };
        let drawn = apply(&img, &Edits { ops: vec![Op::Perspective { corners: [[0.5, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 1.0]] }], strokes: vec![stroke], ..Default::default() }).into_rgba8();
        assert_eq!(drawn.get_pixel(50, 50)[1], 255);
    }

    #[test]
    fn areas_hide_and_stamps_follow_the_picture() {
        // Stripes: hard to the eye when blurred, a single color when pixelated.
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(100, 50, |x, _| if x % 2 == 0 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }));
        let area = |kind: AreaKind| Area { at: 0, x: 0.2, y: 0.2, w: 0.4, h: 0.6, kind };
        let run = |ops: Vec<Op>, areas: Vec<Area>| apply(&img, &Edits { ops, areas, ..Default::default() }).into_rgba8();
        let blurred = run(vec![], vec![area(AreaKind::Blur)]);
        let mid = |i: &RgbaImage, x: u32| i.get_pixel(x, 25)[0] as i32;
        assert!((mid(&blurred, 40) - mid(&blurred, 41)).abs() < 40, "stripes gone inside");
        assert_eq!((mid(&blurred, 90), mid(&blurred, 91)), (0, 255), "untouched outside");
        let pix = run(vec![], vec![area(AreaKind::Pixelate)]);
        assert!(pix.get_pixel(24, 15) == pix.get_pixel(25, 15) && mid(&pix, 90) == 0);
        // A stamp: a 4×2 red PNG laid on the right half's middle, then the picture flipped.
        let mut png = Vec::new();
        RgbaImage::from_pixel(4, 2, Rgba([255, 0, 0, 255])).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let b64: String = png.chunks(3).flat_map(|c| {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            (0..4).map(move |i| if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
        }).collect();
        assert_eq!(base64_decode(&b64).unwrap(), png);
        assert!(base64_decode("@@@@").is_none());
        let stamp = Area { at: 0, x: 0.6, y: 0.4, w: 0.2, h: 0.2, kind: AreaKind::Stamp { png: b64 } };
        let on = run(vec![], vec![stamp.clone()]);
        assert_eq!(on.get_pixel(70, 25).0, [255, 0, 0, 255]);
        let flipped = run(vec![Op::FlipH], vec![stamp.clone()]);
        assert_eq!((flipped.get_pixel(29, 25)[1], flipped.get_pixel(70, 25)[1]), (0, 255).into());
        // Turned with the picture: 20×10 px becomes 10×20.
        let turned = run(vec![Op::RotateRight], vec![stamp]);
        let red: Vec<(u32, u32)> = turned.enumerate_pixels().filter(|(_, _, p)| p[0] == 255 && p[1] == 0).map(|(x, y, _)| (x, y)).collect();
        let (xs, ys): (Vec<u32>, Vec<u32>) = red.iter().copied().unzip();
        assert_eq!((xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1, ys.iter().max().unwrap() - ys.iter().min().unwrap() + 1), (10, 20));
        // A list from before areas existed still reads.
        let old: Edits = serde_json::from_str(r#"{"ops":[],"adjust":{},"strokes":[]}"#).unwrap();
        assert!(old.areas.is_empty());
    }

    #[test]
    fn histogram_counts_each_channel() {
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 10, 255]));
        img.put_pixel(1, 0, Rgba([0, 0, 0, 0]));
        let h = histogram(&DynamicImage::ImageRgba8(img));
        assert_eq!((h[0][255], h[1][0], h[2][10]), (1, 1, 1), "the transparent pixel isn't counted");
        assert_eq!(h[3][55], 1); // 0.2126 × 255 + 0.0722 × 10
    }

    #[test]
    fn every_format_encodes_and_reads_back() {
        let d = dir();
        // With and without alpha: some encoders take a different path for each.
        for img in [sample(), DynamicImage::ImageRgb8(sample().to_rgb8())] {
            for format in [
                Format::Png,
                Format::Jpeg,
                Format::Webp,
                Format::Avif,
                Format::Bmp,
                Format::Tiff,
                Format::Gif,
                Format::Ico,
                Format::Qoi,
            ] {
                let bytes = encode(
                    &img,
                    &EncodeOptions {
                        format,
                        quality: 80,
                        lossless: false,
                        max_side: None,
                    },
                )
                .unwrap();
                let path = write_new(&d, "out", format.extension(), &bytes).unwrap();
                if format == Format::Avif {
                    // No AVIF decoder in Rust here: check the container.
                    assert_eq!(&bytes[4..12], b"ftypavif", "{format:?}");
                    continue;
                }
                let back = decode(&path).unwrap();
                assert_eq!(back.dimensions(), (40, 20), "{format:?}");
                let p = back.to_rgba8().get_pixel(30, 10).0;
                assert!(
                    p[2] > 200 && p[0] < 60,
                    "{format:?}: blue stays blue, got {p:?}"
                );
            }
        }
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn max_side_shrinks_but_never_enlarges() {
        let img = sample();
        let opts = |max| EncodeOptions {
            format: Format::Png,
            quality: 90,
            lossless: false,
            max_side: Some(max),
        };
        let d = dir();
        let small = write_new(&d, "s", "png", &encode(&img, &opts(10)).unwrap()).unwrap();
        assert_eq!(decode(&small).unwrap().dimensions(), (10, 5));
        let same = write_new(&d, "b", "png", &encode(&img, &opts(4000)).unwrap()).unwrap();
        assert_eq!(decode(&same).unwrap().dimensions(), (40, 20));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn never_overwrites() {
        let d = dir();
        fs::write(d.join("a.png"), "mine").unwrap();
        let p = write_new(&d, "a", "png", b"new").unwrap();
        assert_eq!(p.file_name().unwrap(), "a (2).png");
        assert_eq!(fs::read_to_string(d.join("a.png")).unwrap(), "mine");
        assert!(fs::read_dir(&d).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".jaye-tmp")
        }));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn decompression_bombs_are_refused() {
        let d = dir();
        // A 1×1 PNG whose header says 60 000 × 60 000 (3.6 GP): refused from the header, nothing allocated.
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[0], 1, 1, image::ExtendedColorType::L8)
            .unwrap();
        png[16..24].copy_from_slice(&[0, 0, 0xea, 0x60, 0, 0, 0xea, 0x60]);
        let crc = crc32fast::hash(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let p = d.join("bomb.png");
        fs::write(&p, &png).unwrap();
        assert!(
            matches!(decode(&p), Err(ImageError::TooBig { .. })),
            "{:?}",
            decode(&p)
        );
        // Garbage is an error, not a panic.
        fs::write(d.join("x.png"), b"\x89PNG\r\n\x1a\nnope").unwrap();
        assert!(matches!(
            decode(&d.join("x.png")),
            Err(ImageError::Invalid { .. } | ImageError::Io { .. })
        ));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn exif_orientation_is_applied() {
        // A JPEG 40×20 with EXIF orientation 6 (rotate 90° clockwise) opens as 20×40.
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&sample().to_rgb8())
            .unwrap();
        let exif: &[u8] =
            b"Exif\0\0MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";
        let mut with_exif = jpeg[..2].to_vec();
        with_exif.extend([0xff, 0xe1]);
        with_exif.extend(((exif.len() + 2) as u16).to_be_bytes());
        with_exif.extend(exif);
        with_exif.extend(&jpeg[2..]);
        let d = dir();
        let p = d.join("rot.jpg");
        fs::write(&p, with_exif).unwrap();
        assert_eq!(decode(&p).unwrap().dimensions(), (20, 40));
        fs::remove_dir_all(&d).unwrap();
    }
}
