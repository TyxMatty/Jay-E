//! ODT (OpenDocument text) to the model. Bold, italic and the rest live in styles (automatic ones in
//! content.xml, named ones in styles.xml), followed through `parent-style-name`; headings are
//! `text:h` with their outline level; lists are already nested in the XML, numbered or not by their
//! list style. Not read: frames of text, notes, sections' columns, merged cells.

use super::xml::{self, Node};
use super::{Align, Block, DocError, Document, Image, Inline, MAX_DEPTH, Style, image_mime, invalid, merge_runs, zip_entry};
use std::collections::HashMap;
use std::io::Cursor;

#[derive(Default, Clone)]
struct StyleProps {
    parent: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
    mono: Option<bool>,
    align: Option<Align>,
    page_break: bool,
}

struct Ctx {
    styles: HashMap<String, StyleProps>,
    /// List style name → numbered or not, per level (1-based in the XML, 0-based here).
    list_styles: HashMap<String, Vec<bool>>,
    images: Vec<Image>,
    image_index: HashMap<String, usize>,
    zip: zip::ZipArchive<Cursor<Vec<u8>>>,
}

pub fn read(bytes: &[u8]) -> Result<Document, DocError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|e| invalid(e.to_string()))?;
    let content = zip_entry(&mut zip, "content.xml")?.ok_or_else(|| invalid("not an OpenDocument file (no content.xml)"))?;
    let content = xml::parse(&content)?;
    let mut ctx = Ctx { styles: HashMap::new(), list_styles: HashMap::new(), images: vec![], image_index: HashMap::new(), zip };
    if let Some(styles) = zip_entry(&mut ctx.zip, "styles.xml")? {
        let styles = xml::parse(&styles)?;
        for group in ["styles", "automatic-styles"] {
            if let Some(g) = styles.child(group) {
                read_styles(g, &mut ctx);
            }
        }
    }
    if let Some(g) = content.child("automatic-styles") {
        read_styles(g, &mut ctx);
    }
    let text = content.child("body").and_then(|b| b.child("text")).ok_or_else(|| invalid("not a text document (no office:text)"))?;
    let blocks = read_blocks(text, &mut ctx, 0, None);
    Ok(Document { blocks, images: ctx.images })
}

fn read_styles(group: &Node, ctx: &mut Ctx) {
    for s in group.nodes() {
        let Some(name) = s.attr("name") else { continue };
        match s.name.as_str() {
            "style" => {
                let t = s.child("text-properties");
                let p = s.child("paragraph-properties");
                let ta = |a: &str| t.and_then(|t| t.attr(a));
                let position = ta("text-position").unwrap_or("");
                let font = ta("font-name").unwrap_or("").to_ascii_lowercase();
                let props = StyleProps {
                    parent: s.attr("parent-style-name").map(String::from),
                    bold: ta("font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)),
                    italic: ta("font-style").map(|v| v == "italic" || v == "oblique"),
                    underline: ta("text-underline-style").map(|v| v != "none"),
                    strike: ta("text-line-through-style").map(|v| v != "none"),
                    sup: (!position.is_empty()).then(|| position.starts_with("super") || position.trim_start().starts_with(|c: char| c.is_ascii_digit() && c != '0')),
                    sub: (!position.is_empty()).then(|| position.starts_with("sub") || position.trim_start().starts_with('-')),
                    mono: (!font.is_empty()).then(|| ["mono", "courier", "consolas", "code"].iter().any(|m| font.contains(m))),
                    align: p.and_then(|p| p.attr("text-align")).map(|a| match a {
                        "center" => Align::Center,
                        "end" | "right" => Align::End,
                        "justify" => Align::Justify,
                        _ => Align::Start,
                    }),
                    page_break: p.and_then(|p| p.attr("break-before")) == Some("page"),
                };
                ctx.styles.insert(name.to_owned(), props);
            }
            "list-style" => {
                let mut levels = vec![false; 10];
                for l in s.nodes() {
                    let level: usize = l.attr("level").and_then(|v| v.parse().ok()).unwrap_or(1);
                    if (1..=10).contains(&level) {
                        levels[level - 1] = l.name == "list-level-style-number";
                    }
                }
                ctx.list_styles.insert(name.to_owned(), levels);
            }
            _ => {}
        }
    }
}

impl Ctx {
    /// A style's property, looking up its parents (bounded: styles can be cyclic).
    fn prop<T: Copy>(&self, name: Option<&str>, get: impl Fn(&StyleProps) -> Option<T>) -> Option<T> {
        let mut name = name?;
        for _ in 0..32 {
            let s = self.styles.get(name)?;
            if let Some(v) = get(s) {
                return Some(v);
            }
            name = s.parent.as_deref()?;
        }
        None
    }

    fn style(&self, name: Option<&str>, base: Style) -> Style {
        let f = |get: fn(&StyleProps) -> Option<bool>, d: bool| self.prop(name, get).unwrap_or(d);
        Style {
            bold: f(|s| s.bold, base.bold),
            italic: f(|s| s.italic, base.italic),
            underline: f(|s| s.underline, base.underline),
            strike: f(|s| s.strike, base.strike),
            code: f(|s| s.mono, base.code),
            sup: f(|s| s.sup, base.sup),
            sub: f(|s| s.sub, base.sub),
        }
    }

    fn style_name_is(&self, name: Option<&str>, f: impl Fn(&str) -> bool) -> bool {
        let mut name = name;
        for _ in 0..32 {
            let Some(n) = name else { return false };
            if f(n) {
                return true;
            }
            name = self.styles.get(n).and_then(|s| s.parent.as_deref());
        }
        false
    }
}

/// `list` is the list style and level a list inside these blocks continues.
fn read_blocks(parent: &Node, ctx: &mut Ctx, depth: usize, list: Option<(&str, usize)>) -> Vec<Block> {
    let mut out = vec![];
    if depth > MAX_DEPTH {
        return out;
    }
    for node in parent.nodes() {
        match node.name.as_str() {
            "h" => {
                let level = node.attr("outline-level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(1).clamp(1, 6);
                let style = node.attr("style-name");
                if ctx.prop(style, |s| s.page_break.then_some(true)).is_some() {
                    out.push(Block::PageBreak);
                }
                out.push(Block::Heading { level, inlines: inlines_of(node, ctx, style) });
            }
            "p" => {
                let style = node.attr("style-name");
                if ctx.prop(style, |s| s.page_break.then_some(true)).is_some() && !out.is_empty() {
                    out.push(Block::PageBreak);
                }
                let inlines = inlines_of(node, ctx, style);
                if ctx.style_name_is(style, |n| n.contains("Preformatted") || n.contains("Source_20_Text")) {
                    out.push(Block::Code { text: super::plain(&inlines) });
                } else {
                    out.push(Block::Paragraph { inlines, align: ctx.prop(style, |s| s.align).unwrap_or_default() });
                }
            }
            "list" => {
                let (style_name, level) = match (node.attr("style-name"), list) {
                    (Some(s), _) => (s.to_owned(), list.map(|(_, l)| l + 1).unwrap_or(0)),
                    (None, Some((s, l))) => (s.to_owned(), l + 1),
                    (None, None) => (String::new(), 0),
                };
                let ordered = ctx.list_styles.get(&style_name).and_then(|l| l.get(level)).copied().unwrap_or(false);
                let items = node
                    .nodes()
                    .filter(|n| n.name == "list-item" || n.name == "list-header")
                    .map(|item| read_blocks(item, ctx, depth + 1, Some((&style_name, level))))
                    .collect();
                out.push(Block::List { ordered, items });
            }
            "table" => {
                let mut rows = vec![];
                collect_rows(node, ctx, depth, &mut rows);
                out.push(Block::Table { rows });
            }
            "section" | "index-body" | "table-of-content" | "illustration-index" | "alphabetical-index" | "deletion" | "tracked-changes" => {
                if !matches!(node.name.as_str(), "deletion" | "tracked-changes") {
                    out.extend(read_blocks(node, ctx, depth + 1, list));
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_rows(node: &Node, ctx: &mut Ctx, depth: usize, rows: &mut Vec<Vec<Vec<Block>>>) {
    for n in node.nodes() {
        match n.name.as_str() {
            "table-row" => rows.push(n.nodes().filter(|c| c.name == "table-cell").map(|c| read_blocks(c, ctx, depth + 1, None)).collect()),
            "table-header-rows" | "table-rows" | "table-row-group" => collect_rows(n, ctx, depth + 1, rows),
            _ => {}
        }
    }
}

fn inlines_of(node: &Node, ctx: &mut Ctx, style: Option<&str>) -> Vec<Inline> {
    let mut out = vec![];
    let base = ctx.style(style, Style::default());
    read_inlines(node, ctx, base, &mut out, 0);
    // ODF collapses white space like HTML: trim the paragraph's ends.
    if let Some(Inline::Text { text, .. }) = out.first_mut() {
        *text = text.trim_start().to_owned();
    }
    if let Some(Inline::Text { text, .. }) = out.last_mut() {
        *text = text.trim_end().to_owned();
    }
    merge_runs(out)
}

fn read_inlines(node: &Node, ctx: &mut Ctx, style: Style, out: &mut Vec<Inline>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut last_space = false;
    for child in &node.children {
        match child {
            xml::Child::Text(t) => {
                // Runs of white space are one space (text:s carries the real ones).
                let mut s = String::with_capacity(t.len());
                for c in t.chars() {
                    if c.is_whitespace() {
                        if !last_space {
                            s.push(' ');
                        }
                        last_space = true;
                    } else {
                        s.push(c);
                        last_space = false;
                    }
                }
                out.push(Inline::Text { text: s, style });
            }
            xml::Child::Node(n) => {
                last_space = false;
                match n.name.as_str() {
                    "span" => {
                        let s = ctx.style(n.attr("style-name"), style);
                        read_inlines(n, ctx, s, out, depth + 1);
                    }
                    "a" => {
                        let mut inner = vec![];
                        read_inlines(n, ctx, style, &mut inner, depth + 1);
                        match n.attr("href") {
                            Some(href) => out.push(Inline::Link { href: href.to_owned(), inlines: merge_runs(inner) }),
                            None => out.extend(inner),
                        }
                    }
                    "s" => out.push(Inline::Text { text: " ".repeat(n.attr("c").and_then(|c| c.parse().ok()).unwrap_or(1).min(1000)), style }),
                    "tab" => out.push(Inline::Text { text: "\t".into(), style }),
                    "line-break" => out.push(Inline::Break),
                    "frame" => out.extend(read_image(n, ctx)),
                    "ruby" | "ruby-base" | "meta" | "bookmark-ref" | "reference-ref" | "sequence" | "date" | "time" | "page-number" | "page-count" | "chapter" | "title" | "subject" | "author-name" => {
                        read_inlines(n, ctx, style, out, depth + 1)
                    }
                    _ => {}
                }
            }
        }
    }
}

fn read_image(frame: &Node, ctx: &mut Ctx) -> Option<Inline> {
    let image = frame.child("image")?;
    let href = image.attr("href")?;
    if href.contains(':') || href.starts_with("..") {
        return None; // a linked file or URL, not inside the package
    }
    let path = href.trim_start_matches("./").to_owned();
    let index = match ctx.image_index.get(&path) {
        Some(&i) => i,
        None => {
            let bytes = zip_entry(&mut ctx.zip, &path).ok()??;
            let mime = image_mime(&bytes)?.to_owned();
            ctx.images.push(Image { mime, bytes });
            ctx.image_index.insert(path, ctx.images.len() - 1);
            ctx.images.len() - 1
        }
    };
    Some(Inline::Image { index, width: frame.attr("width").and_then(length_pt), height: frame.attr("height").and_then(length_pt) })
}

/// An ODF length ("2.5cm", "1in", "12pt", "20mm") in points.
fn length_pt(s: &str) -> Option<f32> {
    let n = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let v: f32 = n.parse().ok()?;
    let unit = &s[n.len()..];
    Some(match unit {
        "cm" => v * 72.0 / 2.54,
        "mm" => v * 72.0 / 25.4,
        "in" => v * 72.0,
        "pt" => v,
        "pc" => v * 12.0,
        "px" => v * 0.75,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn odt(content: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("mimetype", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        zip.write_all(b"application/vnd.oasis.opendocument.text").unwrap();
        zip.start_file("content.xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_styles_lists_tables_and_links() {
        let content = r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:style="s" xmlns:fo="f" xmlns:table="tb" xmlns:xlink="x">
<office:automatic-styles>
  <style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
  <style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:text-align="center"/></style:style>
  <text:list-style style:name="L1"><text:list-level-style-number text:level="1"/><text:list-level-style-bullet text:level="2"/></text:list-style>
</office:automatic-styles>
<office:body><office:text>
  <text:h text:outline-level="2">Título</text:h>
  <text:p text:style-name="P1">Um   <text:span text:style-name="T1">negrito</text:span><text:s text:c="2"/>e <text:a xlink:href="https://e.com">link</text:a></text:p>
  <text:list text:style-name="L1"><text:list-item><text:p>um</text:p><text:list><text:list-item><text:p>sub</text:p></text:list-item></text:list></text:list-item></text:list>
  <table:table><table:table-header-rows><table:table-row><table:table-cell><text:p>A</text:p></table:table-cell><table:table-cell><text:p>B</text:p></table:table-cell></table:table-row></table:table-header-rows></table:table>
</office:text></office:body></office:document-content>"#;
        let doc = read(&odt(content)).unwrap();
        assert_eq!(doc.blocks[0], Block::Heading { level: 2, inlines: vec![Inline::text("Título")] });
        let bold = Style { bold: true, ..Default::default() };
        assert_eq!(
            doc.blocks[1],
            Block::Paragraph {
                inlines: vec![Inline::text("Um "), Inline::Text { text: "negrito".into(), style: bold }, Inline::text("  e "), Inline::Link { href: "https://e.com".into(), inlines: vec![Inline::text("link")] }],
                align: Align::Center
            }
        );
        let Block::List { ordered: true, items } = &doc.blocks[2] else { panic!("{:?}", doc.blocks[2]) };
        assert!(matches!(&items[0][1], Block::List { ordered: false, .. }), "level 2 is bullets: {:?}", items[0]);
        assert!(matches!(&doc.blocks[3], Block::Table { rows } if rows.len() == 1 && rows[0].len() == 2));
    }

    #[test]
    fn lengths() {
        assert_eq!(length_pt("1in"), Some(72.0));
        assert!((length_pt("2.54cm").unwrap() - 72.0).abs() < 0.01);
        assert_eq!(length_pt("12"), None);
        assert_eq!(length_pt("abc"), None);
    }

    #[test]
    fn not_an_odt() {
        assert!(matches!(read(b"PK nope"), Err(DocError::Invalid { .. })));
    }
}
