//! Times opening, drawing and text of a PDF, and prints its first lines and links:
//!   cargo run --release --example pdf_probe -- file.pdf [page]
use jay_docs::pdf::PdfDocument;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("file.pdf");
    let page: usize = std::env::args().nth(2).and_then(|p| p.parse().ok()).unwrap_or(0);
    let t = Instant::now();
    let doc = PdfDocument::open(path.as_ref(), None).unwrap();
    println!("{} pages, open {:.1} ms", doc.info.pages.len(), t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let bmp = doc.render(page, 1240, false).recv().unwrap().unwrap();
    println!("page {} at {}x{}: {:.1} ms", page + 1, bmp.width, bmp.height, t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let lines = doc.text(page).recv().unwrap();
    println!("{} lines of text: {:.1} ms", lines.len(), t.elapsed().as_secs_f64() * 1e3);
    for l in lines.iter().take(8) {
        println!("  [{:.0},{:.0} {:.0}x{:.0}] {}", l.x, l.y, l.w, l.h, l.text);
    }
    for l in doc.links(page).recv().unwrap().iter().take(5) {
        println!("  link {:?} -> {:?}", l.rect, l.target);
    }
}
