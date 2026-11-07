//! Opens MP4s, cuts and joins them, with timings:
//!   cargo run --release --example video_probe -- cut <in.mp4> <start> <end> <out.mp4>
//!   cargo run --release --example video_probe -- join <out.mp4> <in.mp4> [<in.mp4>…]
//!   cargo run --release --example video_probe -- info <in.mp4>…
//!   cargo run --release --example video_probe -- convert <in> <out.mp4|out.m4a> [max-height]
use jay_docs::video::mp4::{Movie, join};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let no = AtomicBool::new(false);
    match a[1].as_str() {
        "info" => {
            for f in &a[2..] {
                let t = Instant::now();
                match Movie::open(f.as_ref()) {
                    Ok(m) => println!("{f}: {:.2} s, {} key frames (first {:?}), open {:.1} ms", m.duration(), m.key_frames().len(), &m.key_frames()[..m.key_frames().len().min(4)], t.elapsed().as_secs_f64() * 1e3),
                    Err(e) => println!("{f}: {e:?}"),
                }
            }
        }
        "cut" => {
            let t = Instant::now();
            let m = Movie::open(a[2].as_ref()).unwrap();
            let opened = t.elapsed().as_secs_f64();
            let (s, e) = m.cut(a[2].as_ref(), a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].as_ref(), &no).unwrap();
            println!("cut {:.3}–{:.3} s of a {:.1} s file in {:.0} ms (open {:.0} ms), {:.1} MB", s, e, m.duration(), t.elapsed().as_secs_f64() * 1e3, opened * 1e3, std::fs::metadata(&a[5]).unwrap().len() as f64 / 1e6);
        }
        "join" => {
            let t = Instant::now();
            let movies: Vec<Movie> = a[3..].iter().map(|f| Movie::open(f.as_ref()).unwrap()).collect();
            let parts: Vec<(&Path, &Movie)> = a[3..].iter().map(|f| Path::new(f.as_str())).zip(movies.iter()).collect();
            join(&parts, a[2].as_ref(), &no).unwrap();
            println!("joined {} files in {:.1} s, {:.1} MB", parts.len(), t.elapsed().as_secs_f64(), std::fs::metadata(&a[2]).unwrap().len() as f64 / 1e6);
        }
        #[cfg(windows)]
        "convert" => {
            use jay_docs::video::mf::{ConvertOptions, convert, probe};
            let options = ConvertOptions { audio_only: a[3].ends_with(".m4a"), max_height: a.get(4).and_then(|h| h.parse().ok()), video_kbps: 2500, audio_kbps: 160 };
            let t = Instant::now();
            let mut last = 0.0;
            match convert(a[2].as_ref(), a[3].as_ref(), &options, &mut |p| last = p, &no) {
                Ok(()) => {
                    let (d, v, au) = probe(a[3].as_ref()).unwrap();
                    println!("converted in {:.1} s (progress reached {:.0}%): {:.2} s, video {v}, audio {au}, {:.1} MB", t.elapsed().as_secs_f64(), last * 100.0, d, std::fs::metadata(&a[3]).unwrap().len() as f64 / 1e6);
                }
                Err(e) => println!("convert failed after {:.1} s: {e:?}", t.elapsed().as_secs_f64()),
            }
        }
        #[cfg(windows)]
        "frame" => {
            let t = Instant::now();
            let (w, h, rgba, at) = jay_docs::video::mf::frame(a[2].as_ref(), a[3].parse().unwrap()).unwrap();
            image::RgbaImage::from_raw(w, h, rgba).unwrap().save(&a[4]).unwrap();
            println!("frame {w}x{h} at {at:.3} s in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        }
        _ => panic!("info | cut | join | convert | frame"),
    }
}
