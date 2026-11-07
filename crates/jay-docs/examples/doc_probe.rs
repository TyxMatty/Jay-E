//! Reads documents and prints their outline; for DOCX also the trip DOCX → Markdown → DOCX:
//!   cargo run --release --example doc_probe -- <files>…
use jay_docs::doc::{self, Block, Document, plain};
use std::time::Instant;

fn outline(blocks: &[Block], indent: usize, out: &mut Vec<String>) {
    for b in blocks {
        let pad = "  ".repeat(indent);
        let short = |s: String| s.chars().take(60).collect::<String>().replace('\n', "⏎");
        match b {
            Block::Heading { level, inlines } => out.push(format!("{pad}H{level} {}", short(plain(inlines)))),
            Block::Paragraph { inlines, .. } => out.push(format!("{pad}P  {}", short(plain(inlines)))),
            Block::List { ordered, items } => {
                out.push(format!("{pad}{} ({} items)", if *ordered { "OL" } else { "UL" }, items.len()));
                for item in items {
                    outline(item, indent + 1, out);
                }
            }
            Block::Table { rows } => out.push(format!("{pad}TABLE {}x{}", rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0))),
            Block::Code { text } => out.push(format!("{pad}CODE {}", short(text.clone()))),
            Block::Quote { blocks } => {
                out.push(format!("{pad}QUOTE"));
                outline(blocks, indent + 1, out);
            }
            Block::PageBreak => out.push(format!("{pad}--- page ---")),
            Block::Rule => out.push(format!("{pad}--- rule ---")),
        }
    }
}

/// Headings and all text, for comparing two versions of a document.
fn essence(d: &Document) -> (Vec<String>, String) {
    fn walk(blocks: &[Block], heads: &mut Vec<String>, text: &mut String) {
        for b in blocks {
            match b {
                Block::Heading { level, inlines } => {
                    heads.push(format!("H{level} {}", plain(inlines)));
                    text.push_str(&plain(inlines));
                }
                Block::Paragraph { inlines, .. } => text.push_str(&plain(inlines)),
                Block::List { items, .. } => items.iter().for_each(|i| walk(i, heads, text)),
                Block::Table { rows } => rows.iter().flatten().for_each(|c| walk(c, heads, text)),
                Block::Code { text: t } => text.push_str(t),
                Block::Quote { blocks } => walk(blocks, heads, text),
                _ => {}
            }
        }
    }
    let (mut heads, mut text) = (vec![], String::new());
    walk(&d.blocks, &mut heads, &mut text);
    (heads, text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn main() {
    for path in std::env::args().skip(1) {
        let p = std::path::Path::new(&path);
        let (_, format) = jay_docs::detect(p).unwrap();
        let t = Instant::now();
        match doc::read(p, format) {
            Ok(d) => {
                let mut lines = vec![];
                outline(&d.blocks, 1, &mut lines);
                println!("== {} ({format}): {} blocks, {} images, {:.1} ms", p.file_name().unwrap().to_string_lossy(), d.blocks.len(), d.images.len(), t.elapsed().as_secs_f64() * 1e3);
                for l in lines.iter().take(40) {
                    println!("{l}");
                }
                if lines.len() > 40 {
                    println!("  … {} more", lines.len() - 40);
                }
                if format == "docx" {
                    let (md, _) = doc::markdown::write(&d, "x");
                    let back = doc::docx::read(&doc::docx::write(&doc::markdown::read(&md)).unwrap()).unwrap();
                    let (a, b) = (essence(&d), essence(&back));
                    println!("  DOCX → MD → DOCX: headings {}, text {}", if a.0 == b.0 { "same" } else { "DIFFERENT" }, if a.1 == b.1 { "same" } else { "DIFFERENT" });
                    if a.1 != b.1 {
                        let i = a.1.chars().zip(b.1.chars()).position(|(x, y)| x != y).unwrap_or(0);
                        println!("    first difference at char {i}: {:?} vs {:?}", a.1.chars().skip(i.saturating_sub(20)).take(50).collect::<String>(), b.1.chars().skip(i.saturating_sub(20)).take(50).collect::<String>());
                    }
                    if a.0 != b.0 {
                        println!("    {:?}\n    {:?}", a.0, b.0);
                    }
                }
            }
            Err(e) => println!("== {}: {e:?}", p.file_name().unwrap().to_string_lossy()),
        }
    }
}
