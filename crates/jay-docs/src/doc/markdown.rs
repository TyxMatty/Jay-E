//! Markdown to the model (with pulldown-cmark, as the editor's preview) and back (CommonMark + GFM
//! tables). Images in a Markdown file are links to files beside it: reading keeps no image bytes;
//! writing names the images `<stem>-img<N>.<ext>` and returns them for the caller to save.

use super::{Align, Block, Document, Inline, Style, merge_runs, plain};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

pub fn read(markdown: &str) -> Document {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let mut r = Reader::default();
    for event in Parser::new_ext(markdown, options) {
        r.event(event);
    }
    while r.stack.len() > 1 {
        r.close();
    }
    Document { blocks: r.stack.pop().map(|f| f.blocks).unwrap_or_default(), images: vec![] }
}

/// A container being filled: the document, a list item, a quote, a table cell.
#[derive(Default)]
struct Frame {
    kind: FrameKind,
    blocks: Vec<Block>,
}

#[derive(Default, PartialEq)]
enum FrameKind {
    #[default]
    Root,
    Quote,
    Item,
    Cell,
}

#[derive(Default)]
struct Reader {
    stack: Vec<Frame>,
    /// Inlines of the paragraph or heading being read, with link nesting.
    inlines: Vec<Vec<Inline>>,
    links: Vec<String>,
    style: Style,
    heading: Option<u8>,
    in_code: Option<String>,
    /// Lists being read: ordered, items done.
    lists: Vec<(bool, Vec<Vec<Block>>)>,
    table: Option<Vec<Vec<Vec<Block>>>>,
}

impl Reader {
    fn frame(&mut self) -> &mut Frame {
        if self.stack.is_empty() {
            self.stack.push(Frame::default());
        }
        self.stack.last_mut().unwrap()
    }

    fn push_block(&mut self, b: Block) {
        self.frame().blocks.push(b);
    }

    fn close(&mut self) {
        let frame = self.stack.pop().unwrap_or_default();
        match frame.kind {
            FrameKind::Quote => self.push_block(Block::Quote { blocks: frame.blocks }),
            FrameKind::Item => {
                if let Some((_, items)) = self.lists.last_mut() {
                    items.push(frame.blocks);
                }
            }
            FrameKind::Cell => {
                if let Some(row) = self.table.as_mut().and_then(|t| t.last_mut()) {
                    row.push(frame.blocks);
                }
            }
            FrameKind::Root => self.stack.push(frame),
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.in_code {
            code.push_str(text);
            return;
        }
        // Text straight in a list item or cell (a tight list) starts a paragraph.
        if self.inlines.is_empty() {
            self.inlines.push(vec![]);
        }
        self.inlines.last_mut().unwrap().push(Inline::Text { text: text.to_owned(), style: self.style });
    }

    /// Ends a paragraph started implicitly by text in a tight list item or a cell.
    fn flush_loose(&mut self) {
        if let Some(inlines) = self.inlines.pop() {
            let inlines = merge_runs(inlines);
            if !inlines.is_empty() {
                self.push_block(Block::Paragraph { inlines, align: Align::Start });
            }
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => self.inlines.push(vec![]),
                Tag::Heading { level, .. } => {
                    self.heading = Some(match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    });
                    self.inlines.push(vec![]);
                }
                Tag::BlockQuote(_) => {
                    self.flush_loose();
                    self.stack.push(Frame { kind: FrameKind::Quote, blocks: vec![] });
                }
                Tag::CodeBlock(_) => self.in_code = Some(String::new()),
                Tag::List(start) => {
                    self.flush_loose();
                    self.lists.push((start.is_some(), vec![]));
                }
                Tag::Item => self.stack.push(Frame { kind: FrameKind::Item, blocks: vec![] }),
                Tag::Table(_) => self.table = Some(vec![]),
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = &mut self.table {
                        t.push(vec![]);
                    }
                }
                Tag::TableCell => self.stack.push(Frame { kind: FrameKind::Cell, blocks: vec![] }),
                Tag::Emphasis => self.style.italic = true,
                Tag::Strong => self.style.bold = true,
                Tag::Strikethrough => self.style.strike = true,
                Tag::Link { dest_url, .. } => {
                    self.links.push(dest_url.to_string());
                    self.inlines.push(vec![]);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    let inlines = merge_runs(self.inlines.pop().unwrap_or_default());
                    self.push_block(Block::Paragraph { inlines, align: Align::Start });
                }
                TagEnd::Heading(_) => {
                    let inlines = merge_runs(self.inlines.pop().unwrap_or_default());
                    let level = self.heading.take().unwrap_or(1);
                    self.push_block(Block::Heading { level, inlines });
                }
                TagEnd::BlockQuote(_) => {
                    self.flush_loose();
                    self.close();
                }
                TagEnd::CodeBlock => {
                    let text = self.in_code.take().unwrap_or_default();
                    self.push_block(Block::Code { text: text.strip_suffix('\n').unwrap_or(&text).to_owned() });
                }
                TagEnd::Item => {
                    self.flush_loose();
                    self.close();
                }
                TagEnd::List(_) => {
                    if let Some((ordered, items)) = self.lists.pop() {
                        self.push_block(Block::List { ordered, items });
                    }
                }
                TagEnd::TableCell => {
                    self.flush_loose();
                    self.close();
                }
                TagEnd::Table => {
                    if let Some(rows) = self.table.take() {
                        self.push_block(Block::Table { rows });
                    }
                }
                TagEnd::Emphasis => self.style.italic = false,
                TagEnd::Strong => self.style.bold = false,
                TagEnd::Strikethrough => self.style.strike = false,
                TagEnd::Link => {
                    let inner = merge_runs(self.inlines.pop().unwrap_or_default());
                    let href = self.links.pop().unwrap_or_default();
                    if self.inlines.is_empty() {
                        self.inlines.push(vec![]);
                    }
                    self.inlines.last_mut().unwrap().push(Inline::Link { href, inlines: inner });
                }
                _ => {}
            },
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                let style = Style { code: true, ..self.style };
                if self.inlines.is_empty() {
                    self.inlines.push(vec![]);
                }
                self.inlines.last_mut().unwrap().push(Inline::Text { text: t.to_string(), style });
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => {
                if let Some(i) = self.inlines.last_mut() {
                    i.push(Inline::Break);
                }
            }
            Event::Rule => {
                self.flush_loose();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "☒ " } else { "☐ " }),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- writing

/// The document as Markdown, and the images it refers to as (file name, bytes).
pub fn write(doc: &Document, image_stem: &str) -> (String, Vec<(String, Vec<u8>)>) {
    let mut w = Writer { images: vec![], doc, stem: image_stem };
    let mut out = String::new();
    w.blocks(&mut out, &doc.blocks, "");
    let text = out.trim_end().to_owned() + "\n";
    (text, w.images)
}

struct Writer<'a> {
    images: Vec<(String, Vec<u8>)>,
    doc: &'a Document,
    stem: &'a str,
}

impl Writer<'_> {
    fn blocks(&mut self, out: &mut String, blocks: &[Block], indent: &str) {
        for (i, b) in blocks.iter().enumerate() {
            if i > 0 {
                out.push_str(indent.trim_end());
                out.push('\n');
            }
            self.block(out, b, indent);
        }
    }

    fn block(&mut self, out: &mut String, b: &Block, indent: &str) {
        match b {
            Block::Heading { level, inlines } => out.push_str(&format!("{indent}{} {}\n", "#".repeat(*level as usize), self.inlines(inlines).replace('\n', " "))),
            Block::Paragraph { inlines, .. } => {
                let text = self.inlines(inlines);
                // A paragraph that looks like a heading or a list would be read back as one.
                let text = if text.starts_with(['#', '-', '+', '>']) || text.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) { format!("\\{text}") } else { text };
                for line in text.split('\n') {
                    out.push_str(&format!("{indent}{line}\n"));
                }
            }
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = if *ordered { format!("{}. ", n + 1) } else { "- ".to_owned() };
                    let inner_indent = format!("{indent}{}", " ".repeat(marker.len()));
                    let mut item_text = String::new();
                    // Tight: no blank line between an item's paragraph and its nested list.
                    for (j, b) in item.iter().enumerate() {
                        if j > 0 && !matches!(b, Block::List { .. }) {
                            item_text.push_str(inner_indent.trim_end());
                            item_text.push('\n');
                        }
                        self.block(&mut item_text, b, &inner_indent);
                    }
                    let item_text = item_text.strip_prefix(inner_indent.as_str()).unwrap_or(&item_text);
                    out.push_str(&format!("{indent}{marker}{item_text}"));
                }
            }
            Block::Table { rows } => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                if cols == 0 {
                    return;
                }
                for (r, row) in rows.iter().enumerate() {
                    let cells: Vec<String> = (0..cols)
                        .map(|c| {
                            let cell = row.get(c).map(Vec::as_slice).unwrap_or(&[]);
                            let text: Vec<String> = cell.iter().map(|b| self.inline_of(b)).collect();
                            text.join(" ").replace('|', "\\|").replace('\n', " ")
                        })
                        .collect();
                    out.push_str(&format!("{indent}| {} |\n", cells.join(" | ")));
                    if r == 0 {
                        out.push_str(&format!("{indent}|{}\n", " --- |".repeat(cols)));
                    }
                }
            }
            Block::Code { text } => {
                let fence = if text.contains("```") { "~~~~" } else { "```" };
                out.push_str(&format!("{indent}{fence}\n"));
                for line in text.split('\n') {
                    out.push_str(&format!("{indent}{line}\n"));
                }
                out.push_str(&format!("{indent}{fence}\n"));
            }
            Block::Quote { blocks } => {
                let mut inner = String::new();
                self.blocks(&mut inner, blocks, "");
                for line in inner.trim_end().split('\n') {
                    out.push_str(&format!("{indent}> {line}\n").replace("> \n", ">\n"));
                }
            }
            Block::PageBreak | Block::Rule => out.push_str(&format!("{indent}---\n")),
        }
    }

    /// A block as one line (for table cells).
    fn inline_of(&mut self, b: &Block) -> String {
        match b {
            Block::Heading { inlines, .. } | Block::Paragraph { inlines, .. } => self.inlines(inlines),
            Block::Code { text } => format!("`{text}`"),
            other => plain(&match other {
                Block::List { items, .. } => items.iter().flatten().filter_map(|b| if let Block::Paragraph { inlines, .. } = b { Some(inlines.clone()) } else { None }).flatten().collect(),
                _ => vec![],
            }),
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) -> String {
        let mut out = String::new();
        for i in inlines {
            match i {
                Inline::Text { text, style } => out.push_str(&styled(text, *style)),
                Inline::Link { href, inlines } => out.push_str(&format!("[{}](<{}>)", self.inlines(inlines), href.replace('>', "%3E"))),
                Inline::Image { index, .. } => {
                    if let Some(img) = self.doc.images.get(*index) {
                        let ext = img.mime.rsplit('/').next().unwrap_or("bin").replace("jpeg", "jpg").replace("svg+xml", "svg");
                        let name = format!("{}-img{}.{ext}", self.stem, index + 1);
                        if !self.images.iter().any(|(n, _)| *n == name) {
                            self.images.push((name.clone(), img.bytes.clone()));
                        }
                        out.push_str(&format!("![](<{name}>)"));
                    }
                }
                Inline::Break => out.push_str("\\\n"),
            }
        }
        out
    }
}

/// Text with Markdown emphasis, its own special characters escaped. Spaces stay outside the
/// markers ("**bold** " not "**bold **", which CommonMark wouldn't read as bold).
fn styled(text: &str, s: Style) -> String {
    if s.code {
        let tick = if text.contains('`') { "``" } else { "`" };
        return format!("{tick}{text}{tick}");
    }
    let escaped: String = text.chars().flat_map(|c| if "\\*_[]`<>|~".contains(c) { vec!['\\', c] } else { vec![c] }).collect();
    let marks = format!("{}{}{}", if s.bold { "**" } else { "" }, if s.italic { "*" } else { "" }, if s.strike { "~~" } else { "" });
    if marks.is_empty() || escaped.trim().is_empty() {
        return escaped;
    }
    let start = escaped.len() - escaped.trim_start().len();
    let end = escaped.trim_end().len();
    let closing: String = marks.chars().rev().collect::<String>().replace("~~", "~~");
    format!("{}{marks}{}{closing}{}", &escaped[..start], &escaped[start..end], &escaped[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_common_structures() {
        let doc = read("# Title\n\nSome **bold**, *it* and `code` with a [link](https://e.com).\n\n- a\n- b\n  1. b1\n\n> quoted\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\n```\nfn x() {}\n```\n");
        let bold = Style { bold: true, ..Default::default() };
        assert_eq!(doc.blocks[0], Block::Heading { level: 1, inlines: vec![Inline::text("Title")] });
        let Block::Paragraph { inlines, .. } = &doc.blocks[1] else { panic!() };
        assert_eq!(inlines[1], Inline::Text { text: "bold".into(), style: bold });
        assert!(matches!(&inlines[5], Inline::Text { style: Style { code: true, .. }, .. }));
        assert!(matches!(&inlines[7], Inline::Link { href, .. } if href == "https://e.com"));
        let Block::List { ordered: false, items } = &doc.blocks[2] else { panic!("{:?}", doc.blocks[2]) };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1][1], Block::List { ordered: true, .. }), "{:?}", items[1]);
        assert!(matches!(&doc.blocks[3], Block::Quote { .. }));
        let Block::Table { rows } = &doc.blocks[4] else { panic!() };
        assert_eq!(rows.len(), 2);
        assert_eq!(doc.blocks[5], Block::Code { text: "fn x() {}".into() });
    }

    #[test]
    fn writing_then_reading_keeps_the_document() {
        let source = "# Title\n\nSome **bold**, *it*, ~~gone~~ and `code` with a [link](https://e.com/a_b).\n\n- a\n- b\n  1. b1\n  2. b2\n- c\n\n> quoted\n\n| x | y |\n| --- | --- |\n| 1 | 2 |\n\n```\nfn x() {}\n```\n\nChars \\*not bold\\* and 1\\. not a list.\n";
        let doc = read(source);
        let (md, images) = write(&doc, "x");
        assert!(images.is_empty());
        assert_eq!(read(&md), doc, "\n{md}");
    }

    #[test]
    fn images_become_files() {
        let doc = Document {
            blocks: vec![Block::Paragraph { inlines: vec![Inline::Image { index: 0, width: None, height: None }], align: Align::Start }],
            images: vec![super::super::Image { mime: "image/jpeg".into(), bytes: vec![1, 2, 3] }],
        };
        let (md, images) = write(&doc, "report");
        assert_eq!(md, "![](<report-img1.jpg>)\n");
        assert_eq!(images, vec![("report-img1.jpg".to_owned(), vec![1, 2, 3])]);
    }
}
