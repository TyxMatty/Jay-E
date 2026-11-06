//! Spike S3: decode a photo, then time each output format at quality 80.
//!   cargo run --release --example image_bench -- photo.jpg [make-100mp.jpg]
//! With a second path, also writes a ~100 MP JPEG made from the photo and times decoding it.
use image::GenericImageView;
use jay_docs::image::{EncodeOptions, Format, decode, encode};
use std::time::Instant;

fn ms(t: Instant) -> String {
    format!("{:.0} ms", t.elapsed().as_secs_f64() * 1e3)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let t = Instant::now();
    let img = decode(args[1].as_ref()).unwrap();
    let (w, h) = img.dimensions();
    let raw = (w * h * 3) as f64 / 1024.0;
    println!("decode {w}x{h}: {}", ms(t));
    for format in [
        Format::Jpeg,
        Format::Webp,
        Format::Avif,
        Format::Png,
        Format::Qoi,
        Format::Gif,
    ] {
        let t = Instant::now();
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
        println!(
            "{format:?}: {} , {:.0} KB ({:.1}% of raw RGB)",
            ms(t),
            bytes.len() as f64 / 1024.0,
            bytes.len() as f64 / 1024.0 / raw * 100.0
        );
    }
    if let Some(big) = args.get(2) {
        let t = Instant::now();
        let scale = (100e6 / (w as f64 * h as f64)).sqrt();
        let up = img.resize_exact(
            (w as f64 * scale) as u32,
            (h as f64 * scale) as u32,
            image::imageops::FilterType::Triangle,
        );
        let bytes = encode(
            &up,
            &EncodeOptions {
                format: Format::Jpeg,
                quality: 85,
                lossless: false,
                max_side: None,
            },
        )
        .unwrap();
        std::fs::write(big, &bytes).unwrap();
        println!(
            "made {}x{} JPEG ({:.1} MB): {}",
            up.width(),
            up.height(),
            bytes.len() as f64 / 1e6,
            ms(t)
        );
        let t = Instant::now();
        let back = decode(big.as_ref()).unwrap();
        println!("decode 100 MP: {}", ms(t));
        let t = Instant::now();
        let preview = back.thumbnail(2048, 2048);
        println!(
            "preview {}x{}: {}",
            preview.width(),
            preview.height(),
            ms(t)
        );
    }
}
