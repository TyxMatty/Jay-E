//! Images: decode (pure Rust, with limits: files are untrusted, docs/REFERENCIAS.md 4.2), edit
//! without touching the original (a list of operations applied on the way out, as in Kdenlive), and
//! encode to the common formats.
//!
//! Edits are resolution independent (crops in fractions of the image), so the same list draws the
//! screen preview and the full-size file. Adjustments use the CSS filter formulas.

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage};
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

/// A geometric edit. Crop is in fractions (0..1) of the image as it is at that point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    RotateRight,
    RotateLeft,
    FlipH,
    FlipV,
    Crop { x: f32, y: f32, w: f32, h: f32 },
}

/// -1..1, 0 = unchanged. Brightness and contrast scale by 1 + value, saturation too (CSS filters).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Adjust {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Edits {
    pub ops: Vec<Op>,
    pub adjust: Adjust,
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.adjust == Adjust::default()
    }

    /// The size after the edits, for an image of `width`×`height`.
    pub fn size(&self, width: u32, height: u32) -> (u32, u32) {
        self.ops
            .iter()
            .fold((width, height), |(w, h), op| match *op {
                Op::RotateRight | Op::RotateLeft => (h, w),
                Op::FlipH | Op::FlipV => (w, h),
                Op::Crop { x, y, w: cw, h: ch } => crop_rect(w, h, x, y, cw, ch)
                    .map(|(_, _, a, b)| (a, b))
                    .unwrap_or((w, h)),
            })
    }
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
    for op in &edits.ops {
        out = match *op {
            Op::RotateRight => out.rotate90(),
            Op::RotateLeft => out.rotate270(),
            Op::FlipH => out.fliph(),
            Op::FlipV => out.flipv(),
            Op::Crop { x, y, w, h } => match crop_rect(out.width(), out.height(), x, y, w, h) {
                Some((l, t, cw, ch)) => out.crop_imm(l, t, cw, ch),
                None => out,
            },
        };
    }
    if edits.adjust != Adjust::default() {
        let mut rgba = out.into_rgba8();
        adjust(&mut rgba, edits.adjust);
        out = DynamicImage::ImageRgba8(rgba);
    }
    out
}

/// CSS `brightness() contrast() saturate()` in that order, on 8-bit sRGB, alpha untouched.
fn adjust(image: &mut RgbaImage, a: Adjust) {
    let b = 1.0 + a.brightness.clamp(-1.0, 1.0);
    let c = 1.0 + a.contrast.clamp(-1.0, 1.0);
    let s = 1.0 + a.saturation.clamp(-1.0, 1.0);
    // Brightness then contrast, per channel: a lookup table.
    let lut: Vec<f32> = (0..256)
        .map(|v| (((v as f32 / 255.0) * b - 0.5) * c + 0.5).clamp(0.0, 1.0) * 255.0)
        .collect();
    // The saturate() matrix from the Filter Effects spec.
    let m = [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ];
    image.par_chunks_mut(4 * 1024).for_each(|chunk| {
        for px in chunk.chunks_exact_mut(4) {
            let [r, g, bl] = [
                lut[px[0] as usize],
                lut[px[1] as usize],
                lut[px[2] as usize],
            ];
            for (i, row) in m.iter().enumerate() {
                px[i] = (row[0] * r + row[1] * g + row[2] * bl)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    });
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
