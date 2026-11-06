//! Converts files the way the app's batch does (one per core, never overwriting), and times it:
//!   cargo run --release --example convert -- <webp|avif|jpeg|png|…> <out-dir> <files>…
use jay_docs::image::{EncodeOptions, Edits, Format, convert_file};
use rayon::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let format: Format = serde_json::from_str(&format!("\"{}\"", args[1])).expect("format");
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let options = EncodeOptions { format, quality: 80, lossless: false, max_side: None };
    let t = Instant::now();
    let results: Vec<_> = args[3..].par_iter().map(|f| (f, convert_file(f.as_ref(), &Edits::default(), &options, &out))).collect();
    for (f, r) in &results {
        match r {
            Ok(p) => println!("ok   {} -> {} ({} KB)", f.rsplit(['/', '\\']).next().unwrap(), p.file_name().unwrap().to_string_lossy(), std::fs::metadata(p).unwrap().len() / 1024),
            Err(e) => println!("fail {}: {e:?}", f.rsplit(['/', '\\']).next().unwrap()),
        }
    }
    println!("{} files in {:.1} s", results.len(), t.elapsed().as_secs_f64());
}
