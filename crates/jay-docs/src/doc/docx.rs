//! DOCX (Office Open XML) to the model and back.
//!
//! Reading follows LibreOffice's writerfilter split (docs/REFERENCIAS.md 1.2) in a small way: the
//! XML is walked once, and paragraphs, runs and tables become blocks and inlines. Headings come from
//! the style's *name* ("heading 1", also in Word's localized files, where the style id is
//! "Ttulo1") or its outline level; lists from numbering.xml (bullet or not); tracked insertions are
//! kept and deletions dropped (as accepted). Not read: text boxes, footnotes, headers and footers,
//! merged cells, fonts and colors.

use super::xml::{self, Node};
use super::{Align, Block, DocError, Document, Image, Inline, MAX_DEPTH, Style, image_mime, invalid, merge_runs, zip_entry};
use std::collections::HashMap;
use std::io::{Cursor, Write};

const MONOSPACE: [&str; 6] = ["consolas", "courier new", "courier", "cascadia code", "cascadia mono", "lucida console"];

struct Ctx {
    /// Relationship id → target (a hyperlink URL or a part path under word/).
    rels: HashMap<String, String>,
    /// Style id → heading level 1..=6, from the style's name or outline level.
    heading_styles: HashMap<String, u8>,
    /// Style ids that mean code (a monospace paragraph style).
    code_styles: Vec<String>,
    /// Style id → (numId, level), for list styles such as "List Number".
    list_styles: HashMap<String, (String, usize)>,
    /// numId → for each level, whether it's numbered (vs bullets).
    numbering: HashMap<String, Vec<bool>>,
    images: Vec<Image>,
    /// Part path → index in `images`, so an image used twice is stored once.
    image_index: HashMap<String, usize>,
    zip: zip::ZipArchive<Cursor<Vec<u8>>>,
}

pub fn read(bytes: &[u8]) -> Result<Document, DocError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|e| invalid(e.to_string()))?;
    let document = zip_entry(&mut zip, "word/document.xml")?.ok_or_else(|| invalid("not a Word document (no word/document.xml)"))?;
    let rels = match zip_entry(&mut zip, "word/_rels/document.xml.rels")? {
        Some(x) => xml::parse(&x)?.nodes().filter_map(|r| Some((r.attr("Id")?.to_owned(), r.attr("Target")?.to_owned()))).collect(),
        None => HashMap::new(),
    };
    let (heading_styles, code_styles, list_styles) = match zip_entry(&mut zip, "word/styles.xml")? {
        Some(x) => read_styles(&xml::parse(&x)?),
        None => Default::default(),
    };
    let numbering = match zip_entry(&mut zip, "word/numbering.xml")? {
        Some(x) => read_numbering(&xml::parse(&x)?),
        None => HashMap::new(),
    };
    let mut ctx = Ctx { rels, heading_styles, code_styles, list_styles, numbering, images: vec![], image_index: HashMap::new(), zip };
    let root = xml::parse(&document)?;
    let body = root.child("body").ok_or_else(|| invalid("no w:body"))?;
    let blocks = read_blocks(body, &mut ctx, 0);
    Ok(Document { blocks, images: ctx.images })
}

type StyleMaps = (HashMap<String, u8>, Vec<String>, HashMap<String, (String, usize)>);

fn read_styles(styles: &Node) -> StyleMaps {
    let mut headings = HashMap::new();
    let mut code = vec![];
    let mut lists = HashMap::new();
    // First by name and outline level, then a style based on a heading is that heading.
    let mut based_on = vec![];
    for s in styles.nodes().filter(|n| n.name == "style") {
        let Some(id) = s.attr("styleId") else { continue };
        let name = s.child("name").and_then(|n| n.attr("val")).unwrap_or("").to_ascii_lowercase();
        let level = name
            .strip_prefix("heading ")
            .and_then(|l| l.parse::<u8>().ok())
            .or_else(|| (name == "title").then_some(1))
            .or_else(|| s.child("pPr").and_then(|p| p.child("outlineLvl")).and_then(|o| o.attr("val")?.parse::<u8>().ok()).filter(|&l| l < 9).map(|l| l + 1));
        if let Some(l) = level {
            headings.insert(id.to_owned(), l.min(6));
        } else if let Some(parent) = s.child("basedOn").and_then(|b| b.attr("val")) {
            based_on.push((id.to_owned(), parent.to_owned()));
        }
        if let Some(num) = s.child("pPr").and_then(|p| p.child("numPr")) {
            if let Some(id) = num.child("numId").and_then(|n| n.attr("val")) {
                let lvl = num.child("ilvl").and_then(|l| l.attr("val")?.parse().ok()).unwrap_or(0);
                lists.insert(s.attr("styleId").unwrap_or_default().to_owned(), (id.to_owned(), lvl));
            }
        }
        let font = s.child("rPr").and_then(|r| r.child("rFonts")).and_then(|f| f.attr("ascii")).unwrap_or("").to_ascii_lowercase();
        if s.attr("type") == Some("paragraph") && (MONOSPACE.contains(&font.as_str()) || name.contains("code") || name == "html preformatted") {
            code.push(id.to_owned());
        }
    }
    for (id, parent) in based_on {
        if let Some(&l) = headings.get(&parent) {
            headings.insert(id, l);
        }
    }
    (headings, code, lists)
}

fn read_numbering(numbering: &Node) -> HashMap<String, Vec<bool>> {
    let abstracts: HashMap<&str, Vec<bool>> = numbering
        .nodes()
        .filter(|n| n.name == "abstractNum")
        .filter_map(|a| {
            let levels = a.nodes().filter(|l| l.name == "lvl").map(|l| !matches!(l.child("numFmt").and_then(|f| f.attr("val")), Some("bullet" | "none") | None)).collect();
            Some((a.attr("abstractNumId")?, levels))
        })
        .collect();
    numbering
        .nodes()
        .filter(|n| n.name == "num")
        .filter_map(|n| Some((n.attr("numId")?.to_owned(), abstracts.get(n.child("abstractNumId")?.attr("val")?)?.clone())))
        .collect()
}

/// A paragraph's list membership: (level, numbered).
type ListMark = Option<(usize, bool)>;

fn read_blocks(parent: &Node, ctx: &mut Ctx, depth: usize) -> Vec<Block> {
    let mut out = Lists::default();
    if depth > MAX_DEPTH {
        return vec![];
    }
    for node in parent.nodes() {
        match node.name.as_str() {
            "p" => {
                let (blocks, mark) = read_paragraph(node, ctx);
                for (i, b) in blocks.into_iter().enumerate() {
                    // Only the paragraph itself goes in the list; a page break after it doesn't.
                    out.push(b, if i == 0 { mark } else { None });
                }
            }
            "tbl" => {
                let rows = node
                    .nodes()
                    .filter(|r| r.name == "tr")
                    .map(|r| r.nodes().filter(|c| c.name == "tc").map(|c| read_blocks(c, ctx, depth + 1)).collect())
                    .collect();
                out.push(Block::Table { rows }, None);
            }
            // Content controls and tracked insertions wrap ordinary content.
            "sdt" => node.child("sdtContent").into_iter().flat_map(|c| read_blocks(c, ctx, depth + 1)).for_each(|b| out.push(b, None)),
            "ins" | "customXml" => read_blocks(node, ctx, depth + 1).into_iter().for_each(|b| out.push(b, None)),
            _ => {}
        }
    }
    out.finish()
}

/// The paragraph (or heading, or code line), followed by a page break if it had one.
fn read_paragraph(p: &Node, ctx: &mut Ctx) -> (Vec<Block>, ListMark) {
    let ppr = p.child("pPr");
    let style = ppr.and_then(|p| p.child("pStyle")).and_then(|s| s.attr("val")).unwrap_or("");
    let outline = ppr.and_then(|p| p.child("outlineLvl")).and_then(|o| o.attr("val")?.parse::<u8>().ok()).filter(|&l| l < 9).map(|l| l + 1);
    let level = ctx.heading_styles.get(style).copied().or(outline);
    let align = match ppr.and_then(|p| p.child("jc")).and_then(|j| j.attr("val")) {
        Some("center") => Align::Center,
        Some("right" | "end") => Align::End,
        Some("both" | "distribute") => Align::Justify,
        _ => Align::Start,
    };
    // The paragraph's own numbering, else its style's ("List Number", "List Bullet").
    let direct = ppr.and_then(|p| p.child("numPr")).and_then(|n| Some((n.child("numId")?.attr("val")?.to_owned(), n.child("ilvl").and_then(|l| l.attr("val")?.parse().ok()))));
    let list = direct.or_else(|| ctx.list_styles.get(style).map(|(id, l)| (id.clone(), Some(*l)))).and_then(|(id, lvl)| {
        let id = id.as_str();
        let lvl: usize = lvl.unwrap_or(0);
        // numId 0 means "no list".
        let numbered = ctx.numbering.get(id)?.get(lvl).copied().unwrap_or(false);
        (id != "0").then_some((lvl.min(8), numbered))
    });
    let mut page_break = ppr.and_then(|p| p.child("pageBreakBefore")).is_some_and(on);
    let mut inlines = vec![];
    read_inlines(p, ctx, &mut inlines, &mut page_break, 0);
    let inlines = merge_runs(inlines);
    // A paragraph that only holds a page break is the break (Word's "Insert › Page Break").
    if page_break && inlines.is_empty() {
        return (vec![Block::PageBreak], None);
    }
    let block = match level {
        Some(level) if list.is_none() => Block::Heading { level: level.min(6), inlines },
        _ if ctx.code_styles.iter().any(|c| c == style) => Block::Code { text: super::plain(&inlines) },
        _ => Block::Paragraph { inlines, align },
    };
    let mut blocks = vec![block];
    if page_break {
        blocks.push(Block::PageBreak);
    }
    (blocks, if level.is_some() { None } else { list })
}

fn read_inlines(parent: &Node, ctx: &mut Ctx, out: &mut Vec<Inline>, page_break: &mut bool, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for node in parent.nodes() {
        match node.name.as_str() {
            "r" => read_run(node, ctx, out, page_break),
            "hyperlink" => {
                let mut inner = vec![];
                read_inlines(node, ctx, &mut inner, page_break, depth + 1);
                match node.attr("id").and_then(|id| ctx.rels.get(id)) {
                    Some(href) => out.push(Inline::Link { href: href.clone(), inlines: merge_runs(inner) }),
                    None => out.extend(inner), // an anchor inside the document: keep the text
                }
            }
            // Tracked insertions, smart tags, simple fields and content controls: their runs count.
            "ins" | "smartTag" | "fldSimple" | "customXml" | "sdt" | "sdtContent" | "moveTo" => read_inlines(node, ctx, out, page_break, depth + 1),
            _ => {}
        }
    }
}

fn on(node: &Node) -> bool {
    !matches!(node.attr("val"), Some("false" | "0" | "off" | "none"))
}

fn read_run(r: &Node, ctx: &mut Ctx, out: &mut Vec<Inline>, page_break: &mut bool) {
    let rpr = r.child("rPr");
    let flag = |name: &str| rpr.and_then(|p| p.child(name)).is_some_and(on);
    let font = rpr.and_then(|p| p.child("rFonts")).and_then(|f| f.attr("ascii")).unwrap_or("").to_ascii_lowercase();
    let vert = rpr.and_then(|p| p.child("vertAlign")).and_then(|v| v.attr("val"));
    let style = Style {
        bold: flag("b"),
        italic: flag("i"),
        underline: flag("u"),
        strike: flag("strike") || flag("dstrike"),
        code: MONOSPACE.contains(&font.as_str()),
        sup: vert == Some("superscript"),
        sub: vert == Some("subscript"),
    };
    for node in r.nodes() {
        match node.name.as_str() {
            "t" => out.push(Inline::Text { text: node.text(), style }),
            "tab" => out.push(Inline::Text { text: "\t".into(), style }),
            "noBreakHyphen" => out.push(Inline::Text { text: "-".into(), style }),
            "br" if node.attr("type") == Some("page") => *page_break = true,
            "br" | "cr" => out.push(Inline::Break),
            "drawing" | "pict" | "object" => out.extend(read_image(node, ctx)),
            // Office 2010 drawings: the newer Choice only, not its Fallback copy.
            "AlternateContent" => {
                if let Some(choice) = node.child("Choice") {
                    out.extend(read_image(choice, ctx));
                }
            }
            _ => {}
        }
    }
}

/// The picture inside a drawing (DrawingML `a:blip r:embed`, or VML `v:imagedata r:id`).
fn read_image(node: &Node, ctx: &mut Ctx) -> Option<Inline> {
    let id = node.find("blip").and_then(|b| b.attr("embed")).or_else(|| node.find("imagedata").and_then(|i| i.attr("id")))?;
    let target = ctx.rels.get(id)?.clone();
    let path = format!("word/{}", target.trim_start_matches("/word/").trim_start_matches('/'));
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
    // wp:extent in EMU (914 400 per inch, 12 700 per point).
    let extent = node.find("extent");
    let pt = |a: &str| extent.and_then(|e| e.attr(a)?.parse::<f32>().ok()).map(|emu| emu / 12_700.0);
    Some(Inline::Image { index, width: pt("cx"), height: pt("cy") })
}

/// Builds nested lists from paragraphs marked with (level, numbered).
#[derive(Default)]
struct Lists {
    out: Vec<Block>,
    /// Open lists, outermost first: (level, numbered, items).
    open: Vec<(usize, bool, Vec<Vec<Block>>)>,
}

impl Lists {
    fn push(&mut self, block: Block, mark: ListMark) {
        let Some((level, numbered)) = mark else {
            self.close_to(0);
            self.out.push(block);
            return;
        };
        // Close deeper lists, and a same-level list of the other kind.
        while self.open.last().is_some_and(|(l, n, _)| *l > level || (*l == level && *n != numbered)) {
            self.close_one();
        }
        if self.open.last().is_none_or(|(l, _, _)| *l < level) {
            self.open.push((level, numbered, vec![]));
        }
        self.open.last_mut().unwrap().2.push(vec![block]);
    }

    fn close_one(&mut self) {
        let Some((_, numbered, items)) = self.open.pop() else { return };
        let list = Block::List { ordered: numbered, items };
        match self.open.last_mut() {
            // A nested list belongs to the last item of its parent.
            Some((_, _, parent)) => match parent.last_mut() {
                Some(item) => item.push(list),
                None => parent.push(vec![list]),
            },
            None => self.out.push(list),
        }
    }

    fn close_to(&mut self, _: usize) {
        while !self.open.is_empty() {
            self.close_one();
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.close_to(0);
        self.out
    }
}

// ---------------------------------------------------------------- writing

/// The model as a DOCX that Word and LibreOffice open: styles for the headings, code and quotes,
/// numbering for lists, a grid for tables, images in word/media.
pub fn write(doc: &Document) -> Result<Vec<u8>, DocError> {
    let mut w = Writer::default();
    for b in &doc.blocks {
        w.block(b, doc, 0);
    }
    let body = std::mem::take(&mut w.body);
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document {NS}><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1417\" w:right=\"1417\" w:bottom=\"1417\" w:left=\"1417\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let mut rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rIdNumbering\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
    );
    for (id, target) in &w.links {
        rels.push_str(&format!("<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>", esc(target)));
    }
    for (id, name) in &w.media_rels {
        rels.push_str(&format!("<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>"));
    }
    rels.push_str("</Relationships>");
    let mut nums = String::new();
    for (id, ordered) in w.lists.iter().enumerate() {
        let abs = if *ordered { 1 } else { 0 };
        let restart = if *ordered { (0..9).map(|l| format!("<w:lvlOverride w:ilvl=\"{l}\"><w:startOverride w:val=\"1\"/></w:lvlOverride>")).collect() } else { String::new() };
        nums.push_str(&format!("<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{abs}\"/>{restart}</w:num>", id + 1));
    }
    let numbering = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering {NS}>{}{}{nums}</w:numbering>", abstract_num(0, false), abstract_num(1, true));

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, data: &[u8]| -> Result<(), DocError> {
        zip.start_file(name, opts).map_err(|e| invalid(e.to_string()))?;
        zip.write_all(data)?;
        Ok(())
    };
    add("[Content_Types].xml", CONTENT_TYPES.as_bytes())?;
    add("_rels/.rels", ROOT_RELS.as_bytes())?;
    add("word/document.xml", document.as_bytes())?;
    add("word/styles.xml", STYLES.as_bytes())?;
    add("word/numbering.xml", numbering.as_bytes())?;
    add("word/_rels/document.xml.rels", rels.as_bytes())?;
    for (name, bytes) in &w.media {
        add(&format!("word/media/{name}"), bytes)?;
    }
    Ok(zip.finish().map_err(|e| invalid(e.to_string()))?.into_inner())
}

#[derive(Default)]
struct Writer {
    body: String,
    /// Hyperlink relationship id → URL.
    links: Vec<(String, String)>,
    /// Image relationship id → media file name, and the files.
    media_rels: Vec<(String, String)>,
    media: Vec<(String, Vec<u8>)>,
    /// Image index → (relationship id, width px, height px), written once.
    images: HashMap<usize, Option<(String, u32, u32)>>,
    /// One numbering instance per list (numbered or not), so each numbered list starts at 1.
    lists: Vec<bool>,
    next_drawing: u32,
}

impl Writer {
    fn block(&mut self, b: &Block, doc: &Document, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match b {
            Block::Heading { level, inlines } => self.paragraph(&format!("<w:pStyle w:val=\"Heading{}\"/>", level.clamp(&1, &6)), inlines, doc),
            Block::Paragraph { inlines, align } => {
                let jc = match align {
                    Align::Start => "",
                    Align::Center => "<w:jc w:val=\"center\"/>",
                    Align::End => "<w:jc w:val=\"right\"/>",
                    Align::Justify => "<w:jc w:val=\"both\"/>",
                };
                self.paragraph(jc, inlines, doc)
            }
            Block::List { ordered, items } => self.list(*ordered, items, doc, 0, depth),
            Block::Table { rows } => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                self.body.push_str("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"5000\" w:type=\"pct\"/></w:tblPr><w:tblGrid>");
                for _ in 0..cols {
                    self.body.push_str(&format!("<w:gridCol w:w=\"{}\"/>", 9072 / cols));
                }
                self.body.push_str("</w:tblGrid>");
                for row in rows {
                    self.body.push_str("<w:tr>");
                    for i in 0..cols {
                        self.body.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr>");
                        let cell = row.get(i).map(Vec::as_slice).unwrap_or(&[]);
                        for b in cell {
                            self.block(b, doc, depth + 1);
                        }
                        // A cell must end with a paragraph.
                        if !matches!(cell.last(), Some(Block::Paragraph { .. } | Block::Heading { .. } | Block::Code { .. })) {
                            self.body.push_str("<w:p/>");
                        }
                        self.body.push_str("</w:tc>");
                    }
                    self.body.push_str("</w:tr>");
                }
                self.body.push_str("</w:tbl>");
            }
            Block::Code { text } => {
                for line in text.split('\n') {
                    self.body.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"Code\"/></w:pPr>{}</w:p>", text_runs(line, "")));
                }
            }
            Block::Quote { blocks } => {
                for b in blocks {
                    match b {
                        Block::Paragraph { inlines, .. } => self.paragraph("<w:pStyle w:val=\"Quote\"/>", inlines, doc),
                        other => self.block(other, doc, depth + 1),
                    }
                }
            }
            Block::PageBreak => self.body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
            Block::Rule => self.body.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr></w:pPr></w:p>"),
        }
    }

    fn list(&mut self, ordered: bool, items: &[Vec<Block>], doc: &Document, level: usize, depth: usize) {
        self.lists.push(ordered);
        let num = self.lists.len();
        for item in items {
            let mut first = true;
            for b in item {
                match b {
                    // The item's first paragraph carries the bullet; later ones are indented text.
                    Block::Paragraph { inlines, .. } if first => {
                        self.paragraph(&format!("<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num}\"/></w:numPr>", level.min(8)), inlines, doc);
                    }
                    Block::Paragraph { inlines, .. } => self.paragraph(&format!("<w:ind w:left=\"{}\"/>", 720 * (level + 1)), inlines, doc),
                    Block::List { ordered, items } => self.list(*ordered, items, doc, level + 1, depth + 1),
                    other => self.block(other, doc, depth + 1),
                }
                first = false;
            }
        }
    }

    fn paragraph(&mut self, ppr: &str, inlines: &[Inline], doc: &Document) {
        let mut runs = String::new();
        self.inlines(&mut runs, inlines, "", doc);
        self.body.push_str(&format!("<w:p>{}{runs}</w:p>", if ppr.is_empty() { String::new() } else { format!("<w:pPr>{ppr}</w:pPr>") }));
    }

    fn inlines(&mut self, out: &mut String, inlines: &[Inline], extra_rpr: &str, doc: &Document) {
        for i in inlines {
            match i {
                Inline::Text { text, style } => {
                    let mut rpr = String::from(extra_rpr);
                    for (on, tag) in [(style.bold, "<w:b/>"), (style.italic, "<w:i/>"), (style.underline, "<w:u w:val=\"single\"/>"), (style.strike, "<w:strike/>")] {
                        if on {
                            rpr.push_str(tag);
                        }
                    }
                    if style.code {
                        rpr.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/>");
                    }
                    if style.sup || style.sub {
                        rpr.push_str(if style.sup { "<w:vertAlign w:val=\"superscript\"/>" } else { "<w:vertAlign w:val=\"subscript\"/>" });
                    }
                    out.push_str(&text_runs(text, &rpr));
                }
                Inline::Link { href, inlines } if super::html::safe_href(href) => {
                    let id = format!("rIdLink{}", self.links.len() + 1);
                    self.links.push((id.clone(), href.clone()));
                    out.push_str(&format!("<w:hyperlink r:id=\"{id}\">"));
                    self.inlines(out, inlines, "<w:rStyle w:val=\"Hyperlink\"/>", doc);
                    out.push_str("</w:hyperlink>");
                }
                Inline::Link { inlines, .. } => self.inlines(out, inlines, extra_rpr, doc),
                Inline::Image { index, width, height } => {
                    if let Some((rid, pw, ph)) = self.image(*index, doc) {
                        // Points when the source said; else the pixels at 96 dpi; never wider than the page.
                        let (mut w, mut h) = match (width, height) {
                            (Some(w), Some(h)) => (*w, *h),
                            _ => (pw as f32 * 0.75, ph as f32 * 0.75),
                        };
                        if w > 453.0 {
                            h *= 453.0 / w;
                            w = 453.0;
                        }
                        self.next_drawing += 1;
                        out.push_str(&drawing(&rid, self.next_drawing, (w * 12_700.0) as u64, (h * 12_700.0) as u64));
                    }
                }
                Inline::Break => out.push_str("<w:r><w:br/></w:r>"),
            }
        }
    }

    /// The image in word/media, converted to PNG if Word can't show its format.
    fn image(&mut self, index: usize, doc: &Document) -> Option<(String, u32, u32)> {
        if let Some(done) = self.images.get(&index) {
            return done.clone();
        }
        let result = (|| {
            let img = doc.images.get(index)?;
            let (ext, bytes) = match img.mime.as_str() {
                "image/png" => ("png", img.bytes.clone()),
                "image/jpeg" => ("jpeg", img.bytes.clone()),
                "image/gif" => ("gif", img.bytes.clone()),
                "image/bmp" => ("bmp", img.bytes.clone()),
                _ => {
                    let decoded = image::load_from_memory(&img.bytes).ok()?;
                    let mut png = Vec::new();
                    decoded.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
                    ("png", png)
                }
            };
            let (w, h) = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions().unwrap_or((300, 200));
            let name = format!("image{}.{ext}", self.media.len() + 1);
            let id = format!("rIdImg{}", self.media.len() + 1);
            self.media.push((name.clone(), bytes));
            self.media_rels.push((id.clone(), name));
            Some((id, w, h))
        })();
        self.images.insert(index, result.clone());
        result
    }
}

/// Runs for `text`: tabs and newlines become their own elements.
fn text_runs(text: &str, rpr: &str) -> String {
    let rpr = if rpr.is_empty() { String::new() } else { format!("<w:rPr>{rpr}</w:rPr>") };
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str(&format!("<w:r>{rpr}<w:br/></w:r>"));
        }
        for (j, part) in line.split('\t').enumerate() {
            if j > 0 {
                out.push_str(&format!("<w:r>{rpr}<w:tab/></w:r>"));
            }
            if !part.is_empty() {
                out.push_str(&format!("<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(part)));
            }
        }
    }
    out
}

fn esc(s: &str) -> String {
    // Also drop the control chars XML 1.0 forbids (an RTF or a broken file can carry them).
    s.chars().filter(|&c| c >= ' ' || c == '\t' || c == '\n').collect::<String>().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn abstract_num(id: u32, ordered: bool) -> String {
    let mut s = format!("<w:abstractNum w:abstractNumId=\"{id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
    for l in 0..9 {
        let (fmt, text) = if ordered { ("decimal", format!("%{}.", l + 1)) } else { ("bullet", ["•", "◦", "▪"][l % 3].to_owned()) };
        s.push_str(&format!(
            "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
            720 * (l + 1)
        ));
    }
    s.push_str("</w:abstractNum>");
    s
}

pub(crate) fn drawing(rid: &str, id: u32, cx: u64, cy: u64) -> String {
    format!(
        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{id}\" name=\"Picture {id}\"/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"Picture {id}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )
}

const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"";

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/><Default Extension=\"gif\" ContentType=\"image/gif\"/><Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/></Types>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";

const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:cs=\"Calibri\"/><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"259\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"360\" w:after=\"120\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"36\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"240\" w:after=\"80\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"30\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\" w:after=\"60\"/><w:outlineLvl w:val=\"2\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"26\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading4\"><w:name w:val=\"heading 4\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"3\"/></w:pPr><w:rPr><w:b/><w:i/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading5\"><w:name w:val=\"heading 5\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"4\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading6\"><w:name w:val=\"heading 6\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"5\"/></w:pPr><w:rPr><w:i/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Code\"><w:name w:val=\"Code\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F3F3F3\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/><w:sz w:val=\"20\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:ind w:left=\"720\"/></w:pPr><w:rPr><w:i/><w:color w:val=\"595959\"/></w:rPr></w:style>\
<w:style w:type=\"character\" w:styleId=\"Hyperlink\"><w:name w:val=\"Hyperlink\"/><w:rPr><w:color w:val=\"0563C1\"/><w:u w:val=\"single\"/></w:rPr></w:style>\
<w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/></w:tblBorders><w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>\
</w:styles>";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Document {
        let bold = Style { bold: true, ..Default::default() };
        let png = {
            let mut v = Vec::new();
            image::DynamicImage::new_rgb8(40, 20).write_to(&mut Cursor::new(&mut v), image::ImageFormat::Png).unwrap();
            v
        };
        Document {
            blocks: vec![
                Block::Heading { level: 1, inlines: vec![Inline::text("Título & <tags>")] },
                Block::Paragraph {
                    inlines: vec![
                        Inline::text("Plain, "),
                        Inline::Text { text: "bold".into(), style: bold },
                        Inline::text(" and a "),
                        Inline::Link { href: "https://example.com/a?b=1&c=2".into(), inlines: vec![Inline::text("link")] },
                        Inline::Break,
                        Inline::text("tab\there"),
                    ],
                    align: Align::Justify,
                },
                Block::List {
                    ordered: false,
                    items: vec![
                        vec![Block::Paragraph { inlines: vec![Inline::text("one")], align: Align::Start }],
                        vec![
                            Block::Paragraph { inlines: vec![Inline::text("two")], align: Align::Start },
                            Block::List { ordered: true, items: vec![vec![Block::Paragraph { inlines: vec![Inline::text("two.a")], align: Align::Start }]] },
                        ],
                    ],
                },
                Block::Table {
                    rows: vec![
                        vec![vec![Block::Paragraph { inlines: vec![Inline::text("A")], align: Align::Start }], vec![Block::Paragraph { inlines: vec![Inline::text("B")], align: Align::Start }]],
                        vec![vec![Block::Paragraph { inlines: vec![Inline::Image { index: 0, width: None, height: None }], align: Align::Start }], vec![]],
                    ],
                },
                Block::Heading { level: 2, inlines: vec![Inline::text("Two")] },
                Block::Code { text: "fn main() {}".into() },
                Block::PageBreak,
                Block::Paragraph { inlines: vec![Inline::text("after the break")], align: Align::Start },
            ],
            images: vec![Image { mime: "image/png".into(), bytes: png }],
        }
    }

    #[test]
    fn writes_what_it_reads() {
        let doc = sample();
        let bytes = write(&doc).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back.blocks.len(), doc.blocks.len(), "{:#?}", back.blocks);
        assert_eq!(back.blocks[0], doc.blocks[0]);
        // Runs merge on the way back; the content is the same.
        assert_eq!(back.blocks[1], doc.blocks[1]);
        assert_eq!(back.blocks[2], doc.blocks[2], "nested lists");
        let Block::Table { rows } = &back.blocks[3] else { panic!("{:?}", back.blocks[3]) };
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[1][0][0], Block::Paragraph { inlines, .. } if matches!(inlines[0], Inline::Image { index: 0, width: Some(w), .. } if (w - 30.0).abs() < 0.1)));
        assert_eq!(back.images.len(), 1);
        assert_eq!(back.blocks[5], doc.blocks[5], "code");
        assert_eq!(back.blocks[6], Block::PageBreak);
        assert_eq!(back.blocks[7], doc.blocks[7]);
    }

    #[test]
    fn broken_and_hostile_files() {
        assert!(matches!(read(b"not a zip"), Err(DocError::Invalid { .. })));
        // A ZIP without word/document.xml.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("a.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"x").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(matches!(read(&bytes), Err(DocError::Invalid { .. })));
    }

    #[test]
    fn localized_heading_styles_and_word_lists() {
        // Styles as a Portuguese Word writes them: id "Ttulo1", name "heading 1"; a bullet list.
        let document = r#"<w:document xmlns:w="w"><w:body>
            <w:p><w:pPr><w:pStyle w:val="Ttulo1"/></w:pPr><w:r><w:t>Capítulo</w:t></w:r></w:p>
            <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="7"/></w:numPr></w:pPr><w:r><w:rPr><w:b w:val="0"/></w:rPr><w:t>item</w:t></w:r></w:p>
            <w:p><w:r><w:t xml:space="preserve">kept </w:t></w:r><w:ins><w:r><w:t>inserted</w:t></w:r></w:ins><w:del><w:r><w:delText>deleted</w:delText></w:r></w:del></w:p>
        </w:body></w:document>"#;
        let styles = r#"<w:styles xmlns:w="w"><w:style w:type="paragraph" w:styleId="Ttulo1"><w:name w:val="heading 1"/></w:style></w:styles>"#;
        let numbering = r#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="3"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="7"><w:abstractNumId w:val="3"/></w:num></w:numbering>"#;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in [("word/document.xml", document), ("word/styles.xml", styles), ("word/numbering.xml", numbering)] {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        let doc = read(&zip.finish().unwrap().into_inner()).unwrap();
        assert_eq!(doc.blocks[0], Block::Heading { level: 1, inlines: vec![Inline::text("Capítulo")] });
        assert_eq!(doc.blocks[1], Block::List { ordered: false, items: vec![vec![Block::Paragraph { inlines: vec![Inline::text("item")], align: Align::Start }]] });
        assert_eq!(doc.blocks[2], Block::Paragraph { inlines: vec![Inline::text("kept inserted")], align: Align::Start });
    }
}
