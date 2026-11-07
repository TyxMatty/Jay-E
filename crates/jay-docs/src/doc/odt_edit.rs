//! Editing an ODT in place (stage 7), with the same page and the same changes as the DOCX editor
//! (edit.rs): the page is built from the file's own page layout, styles, lists, tables and pictures;
//! saving rewrites only the paragraphs that changed and copies everything else.
//!
//! OpenDocument keeps the look in styles: a changed paragraph or piece of text gets a new automatic
//! style (`JP…`, `JT…`) that copies the one it had and changes only what the toolbar knows, so
//! properties Jay-E doesn't show survive. Lists are nested `text:list`s: a paragraph that joins or
//! leaves a list has the list closed and opened again around it (`text:continue-numbering`).
//! Spaces follow ODF: runs of spaces collapse unless written as `text:s`.
//!
//! content.xml and styles.xml (the header and footer) are kept one after the other in `xml`, as
//! edit.rs does with document.xml and its header parts.

use super::edit::{
    Editable, EditPara, NewMargin, EditTable, Edits, Geo, Item, Level, Margin, NewImage, PProps, PageBox, RProps, Set, StyleChoice, View, check_namespaces, cls, default_level, esc, hex, num, safe_link, shift,
    start_tag_end, visible, xml_text,
};
use super::odt::length_pt;
use super::xml::{self, Node};
use super::{DocError, Image, image_mime, invalid, zip_entry};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{Cursor, Write};
use std::ops::Range;

const NS: [(&str, &str); 4] = [
    ("office", "urn:oasis:names:tc:opendocument:xmlns:office:1.0"),
    ("text", "urn:oasis:names:tc:opendocument:xmlns:text:1.0"),
    ("style", "urn:oasis:names:tc:opendocument:xmlns:style:1.0"),
    ("table", "urn:oasis:names:tc:opendocument:xmlns:table:1.0"),
];
const DRAWING_NS: [(&str, &str); 4] = [
    ("draw", "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"),
    ("svg", "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"),
    ("xlink", "http://www.w3.org/1999/xlink"),
    ("fo", "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"),
];

// ---------------------------------------------------------------- styles

/// A style as written (`raw`, so a copy keeps what Jay-E doesn't read) and what it says.
#[derive(Debug, Clone, Default)]
struct OStyle {
    family: String,
    parent: Option<String>,
    display: String,
    next: Option<String>,
    outline: Option<String>,
    /// In content.xml's automatic styles (0) or styles.xml's (1); named styles: `None`.
    auto: Option<usize>,
    p: PProps,
    r: RProps,
    /// Graphic properties of frame styles (wrap, position).
    h_pos: Option<String>,
    raw: String,
}

fn twips(len: &str) -> Option<i32> {
    length_pt(len).map(|pt| (pt * 20.0).round() as i32)
}

fn read_style(s: &Node, src: &str, fonts: &HashMap<String, String>, auto: Option<usize>) -> OStyle {
    let pp = s.child("paragraph-properties");
    let tp = s.child("text-properties");
    let gp = s.child("graphic-properties");
    let mut p = PProps::default();
    if let Some(pp) = pp {
        p.align = pp.attr("text-align").map(|a| {
            match a {
                "center" => "center",
                "end" | "right" => "right",
                "justify" => "both",
                _ => "left",
            }
            .to_owned()
        });
        p.before = pp.attr("margin-top").and_then(twips);
        p.after = pp.attr("margin-bottom").and_then(twips);
        p.left = pp.attr("margin-left").and_then(twips);
        p.right = pp.attr("margin-right").and_then(twips);
        p.first = pp.attr("text-indent").and_then(twips);
        p.contextual = pp.attr("contextual-spacing").map(|v| v == "true");
        p.line = pp.attr("line-height").and_then(|l| match l.strip_suffix('%') {
            Some(pct) => pct.parse::<f32>().ok().map(|v| ((v * 2.4).round() as i32, "auto".to_owned())),
            None => twips(l).map(|t| (t, "exact".to_owned())),
        });
    }
    let mut r = RProps::default();
    if let Some(tp) = tp {
        r.font = tp.attr("font-name").and_then(|f| fonts.get(f).cloned()).or_else(|| tp.attr("font-family").map(|f| f.trim_matches(['\'', '"']).to_owned()));
        r.size = tp.attr("font-size").and_then(length_pt);
        r.bold = tp.attr("font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
        r.italic = tp.attr("font-style").map(|v| v == "italic" || v == "oblique");
        r.underline = tp.attr("text-underline-style").map(|v| v != "none");
        r.strike = tp.attr("text-line-through-style").map(|v| v != "none");
        r.color = tp.attr("color").and_then(|c| hex(c.trim_start_matches('#')));
        r.bg = tp.attr("background-color").map(|c| hex(c.trim_start_matches('#')).unwrap_or_default());
        r.vert = tp.attr("text-position").map(|v| {
            if v.starts_with("super") || v.trim_start().starts_with(|c: char| c.is_ascii_digit() && c != '0') {
                "superscript"
            } else if v.starts_with("sub") || v.trim_start().starts_with('-') {
                "subscript"
            } else {
                "baseline"
            }
            .to_owned()
        });
        r.caps = tp.attr("text-transform").map(|v| v == "uppercase");
        r.small_caps = tp.attr("font-variant").map(|v| v == "small-caps");
    }
    OStyle {
        family: s.attr("family").unwrap_or("paragraph").to_owned(),
        parent: s.attr("parent-style-name").map(str::to_owned),
        display: s.attr("display-name").map(str::to_owned).unwrap_or_else(|| s.attr("name").unwrap_or("").replace("_20_", " ")),
        next: s.attr("next-style-name").map(str::to_owned),
        outline: s.attr("default-outline-level").map(str::to_owned),
        auto,
        p,
        r,
        h_pos: gp.and_then(|g| g.attr("horizontal-pos")).map(str::to_owned),
        raw: src[s.span.clone()].to_owned(),
    }
}

struct Styles {
    /// By (family, name).
    defs: HashMap<(String, String), OStyle>,
    /// The default paragraph style's properties (`style:default-style`).
    default_p: PProps,
    default_r: RProps,
    named: Vec<String>,
}

impl Styles {
    fn get(&self, family: &str, name: &str) -> Option<&OStyle> {
        self.defs.get(&(family.to_owned(), name.to_owned()))
    }

    /// The properties of a style and its parents, defaults under them.
    fn resolve(&self, family: &str, name: &str) -> (PProps, RProps) {
        let mut chain = vec![];
        let mut at = Some(name.to_owned());
        while let Some(cur) = at.take() {
            if chain.len() > 32 || chain.contains(&cur) {
                break;
            }
            if let Some(s) = self.get(family, &cur) {
                at = s.parent.clone();
                chain.push(cur);
            }
        }
        let (mut p, mut r) = (self.default_p.clone(), self.default_r.clone());
        for n in chain.iter().rev() {
            let s = &self.defs[&(family.to_owned(), n.clone())];
            p.over(&s.p);
            r.over(&s.r);
        }
        (p, r)
    }

    /// The named style an automatic one is based on (or the style itself).
    fn named(&self, family: &str, name: &str) -> String {
        match self.get(family, name) {
            Some(s) if s.auto.is_some() => s.parent.clone().unwrap_or_default(),
            _ => name.to_owned(),
        }
    }
}

/// An ODF list style's levels as the editor draws them (indents in twips, as in edit.rs).
fn read_list_style(l: &Node) -> Vec<Level> {
    let mut levels: Vec<Level> = (0..10).map(|i| default_level(i, true)).collect();
    for lvl in l.nodes() {
        let Some(i) = lvl.attr("level").and_then(|v| v.parse::<usize>().ok()).filter(|&i| (1..=10).contains(&i)).map(|i| i - 1) else { continue };
        let props = lvl.child("list-level-properties");
        let align = props.and_then(|p| p.child("list-level-label-alignment"));
        let (left, hanging) = match align {
            Some(a) => (a.attr("margin-left").and_then(twips).unwrap_or(720 * (i as i32 + 1)), a.attr("text-indent").and_then(twips).map(|t| -t).unwrap_or(360)),
            None => {
                let before = props.and_then(|p| p.attr("space-before")).and_then(twips).unwrap_or(0);
                let label = props.and_then(|p| p.attr("min-label-width")).and_then(twips).unwrap_or(360);
                (before + label, label)
            }
        };
        let level = &mut levels[i];
        level.left = left;
        level.hanging = hanging;
        match lvl.name.as_str() {
            "list-level-style-bullet" => {
                level.fmt = "bullet".into();
                level.text = lvl.attr("bullet-char").map(|c| if ('\u{E000}'..='\u{F8FF}').contains(&c.chars().next().unwrap_or(' ')) { "•".to_owned() } else { c.to_owned() }).unwrap_or_else(|| "•".into());
            }
            "list-level-style-number" => {
                level.fmt = match lvl.attr("num-format").unwrap_or("1") {
                    "a" => "lowerLetter",
                    "A" => "upperLetter",
                    "i" => "lowerRoman",
                    "I" => "upperRoman",
                    "" => "none",
                    _ => "decimal",
                }
                .into();
                level.start = lvl.attr("start-value").and_then(|v| v.parse().ok()).unwrap_or(1);
                let shown: usize = lvl.attr("display-levels").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, i + 1);
                let numbers: Vec<String> = (i + 1 - shown..=i).map(|k| format!("%{}", k + 1)).collect();
                level.text = format!("{}{}{}", lvl.attr("num-prefix").unwrap_or(""), numbers.join("."), lvl.attr("num-suffix").unwrap_or(""));
            }
            _ => {
                level.fmt = "none".into();
                level.text = String::new();
            }
        }
    }
    levels
}

/// The master page's header and footer elements, in the order ODF wants them, with their variants.
const MARGINS: [(&str, &str, &str); 6] = [
    ("header", "header", "default"),
    ("header-left", "header", "even"),
    ("header-first", "header", "first"),
    ("footer", "footer", "default"),
    ("footer-left", "footer", "even"),
    ("footer-first", "footer", "first"),
];

// ---------------------------------------------------------------- the session

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Para,
    Table,
    Row,
    Cell,
    Block,
    Root,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Direct {
    style: String,
    num: String,
    align: String,
    ind: String,
    line: String,
}

/// One level of the lists around a paragraph: the `text:list` start tag and the `text:list-item`.
#[derive(Debug, Clone)]
struct ListLevel {
    list_tag: String,
    item: Range<usize>,
}

struct Unit {
    kind: Kind,
    part: usize,
    span: Range<usize>,
    parent: Option<usize>,
    /// Paragraphs: "p" or "h", the style name in the file (automatic or named), what the page is
    /// told, bookmarks and the like, and the lists it's in (outermost first).
    tag: String,
    style: String,
    direct: Direct,
    marks: Vec<Range<usize>>,
    lists: Vec<ListLevel>,
    /// Cells and roots: where a first paragraph goes, and whether the cell is `<…/>`.
    content_start: usize,
    empty: bool,
    /// Tables: their `table:table-column`s and how many columns each stands for.
    columns: Vec<(Range<usize>, usize)>,
    /// Rows in `table:table-header-rows` (repeated on each page): that element's start and end tags.
    header_group: Option<(Range<usize>, Range<usize>)>,
}

pub struct Session {
    /// The first master page (in styles.xml): where a new header or footer goes, and its children.
    master: Option<Range<usize>>,
    master_children: Vec<(String, Range<usize>)>,
    file: Vec<u8>,
    xml: String,
    parts: Vec<(String, Range<usize>)>,
    /// Per part: the `office:automatic-styles` element (new styles go in it).
    autos: Vec<Option<Range<usize>>>,
    units: Vec<Unit>,
    atoms: Vec<String>,
    /// Style names of the spans around each original piece of text, outermost first.
    runs: Vec<Vec<String>>,
    links: Vec<(String, String)>,
    styles: Styles,
    names: HashSet<String>,
    images: Vec<Image>,
    view: View,
}

struct Ctx<'a> {
    xml: &'a str,
    part: usize,
    styles: &'a Styles,
    zip: zip::ZipArchive<Cursor<Vec<u8>>>,
    image_index: HashMap<String, usize>,
    img_url: &'a dyn Fn(usize) -> String,
    list_styles: &'a HashMap<String, Vec<Level>>,
    /// List instances: (style, levels) by "style#n"; continuing lists share an instance.
    numbering: HashMap<String, Vec<Level>>,
    last_list: HashMap<String, String>,
    out: Session,
}

/// The class of a style on the page; automatic styles of styles.xml get their own prefix.
fn class_of(family: &str, name: &str, styles: &Styles) -> String {
    match styles.get(family, name).and_then(|s| s.auto) {
        Some(1) => format!("s-{}", cls(name)),
        Some(_) => format!("a-{}", cls(name)),
        None => cls(name),
    }
}

impl Session {
    pub fn open(bytes: &[u8], img_url: &dyn Fn(usize) -> String) -> Result<Session, DocError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|e| invalid(e.to_string()))?;
        let content = String::from_utf8(zip_entry(&mut zip, "content.xml")?.ok_or_else(|| invalid("not an OpenDocument file (no content.xml)"))?).map_err(|_| invalid("content.xml is not UTF-8"))?;
        let styles_xml = zip_entry(&mut zip, "styles.xml")?.map(String::from_utf8).transpose().map_err(|_| invalid("styles.xml is not UTF-8"))?.unwrap_or_default();
        let root = xml::parse(content.as_bytes())?;
        let tag = &content[root.span.start..start_tag_end(&content, root.span.start)];
        for (prefix, uri) in NS {
            if !tag.contains(&format!("xmlns:{prefix}=\"{uri}\"")) {
                return Err(invalid("this OpenDocument file uses unusual XML prefixes; Jay-E can show it but not edit it"));
            }
        }
        let mut all = content.clone();
        let mut parts = vec![("content.xml".to_owned(), 0..content.len())];
        let mut sroot = None;
        if !styles_xml.is_empty() {
            let mut n = xml::parse(styles_xml.as_bytes())?;
            shift(&mut n, all.len());
            all.push_str(&styles_xml);
            parts.push(("styles.xml".to_owned(), content.len()..all.len()));
            sroot = Some(n);
        }

        // Fonts, styles and list styles, from both parts.
        let mut fonts = HashMap::new();
        for r in [Some(&root), sroot.as_ref()].into_iter().flatten() {
            if let Some(decls) = r.child("font-face-decls") {
                for f in decls.nodes() {
                    if let (Some(name), Some(family)) = (f.attr("name"), f.attr("font-family")) {
                        fonts.insert(name.to_owned(), family.trim_matches(['\'', '"']).to_owned());
                    }
                }
            }
        }
        let mut styles = Styles { defs: HashMap::new(), default_p: PProps::default(), default_r: RProps { font: Some("Liberation Serif".into()), size: Some(12.0), color: Some("auto".into()), ..Default::default() }, named: vec![] };
        let mut list_styles: HashMap<String, Vec<Level>> = HashMap::new();
        let mut names = HashSet::new();
        let mut autos = vec![root.child("automatic-styles").map(|a| a.span.clone()), sroot.as_ref().and_then(|s| s.child("automatic-styles")).map(|a| a.span.clone())];
        autos.truncate(parts.len());
        let groups: Vec<(&Node, Option<usize>)> = [
            sroot.as_ref().and_then(|s| s.child("styles")).map(|g| (g, None)),
            sroot.as_ref().and_then(|s| s.child("automatic-styles")).map(|g| (g, Some(1))),
            root.child("automatic-styles").map(|g| (g, Some(0))),
        ]
        .into_iter()
        .flatten()
        .collect();
        for (group, auto) in groups {
            for s in group.nodes() {
                if let Some(name) = s.attr("name") {
                    names.insert(name.to_owned());
                }
                match s.name.as_str() {
                    "style" => {
                        let st = read_style(s, &all, &fonts, auto);
                        let name = s.attr("name").unwrap_or("").to_owned();
                        if auto.is_none() && st.family == "paragraph" {
                            styles.named.push(name.clone());
                        }
                        styles.defs.insert((st.family.clone(), name), st);
                    }
                    "default-style" if s.attr("family") == Some("paragraph") => {
                        let st = read_style(s, &all, &fonts, None);
                        styles.default_p = st.p;
                        styles.default_r.over(&st.r);
                    }
                    "list-style" => {
                        list_styles.insert(s.attr("name").unwrap_or("").to_owned(), read_list_style(s));
                    }
                    _ => {}
                }
            }
        }

        // The page: the first master page's layout, with its header and footer.
        let master = sroot.as_ref().and_then(|s| s.child("master-styles")).and_then(|m| m.nodes().find(|n| n.name == "master-page"));
        let layout = master.and_then(|m| m.attr("page-layout-name")).and_then(|name| sroot.as_ref()?.child("automatic-styles")?.nodes().find(|n| n.name == "page-layout" && n.attr("name") == Some(name)));
        let lp = layout.and_then(|l| l.child("page-layout-properties"));
        let len = |n: Option<&Node>, a: &str, d: f32| n.and_then(|n| n.attr(a)).and_then(length_pt).unwrap_or(d);
        let band = |kind: &str| {
            let hp = layout.and_then(|l| l.child(kind)).and_then(|h| h.child("header-footer-properties"));
            len(hp, "min-height", 0.0).max(len(hp, "height", 0.0)) + len(hp, if kind == "header-style" { "margin-bottom" } else { "margin-top" }, 0.0)
        };
        let header = master.and_then(|m| m.child("header"));
        let footer = master.and_then(|m| m.child("footer"));
        let (mt, mb) = (len(lp, "margin-top", 56.7), len(lp, "margin-bottom", 56.7));
        let page = PageBox {
            w: len(lp, "page-width", 595.3),
            h: len(lp, "page-height", 841.9),
            top: mt + if header.is_some() { band("header-style") } else { 0.0 },
            right: len(lp, "margin-right", 56.7),
            bottom: mb + if footer.is_some() { band("footer-style") } else { 0.0 },
            left: len(lp, "margin-left", 56.7),
            header: mt,
            footer: mb,
        };

        let default_style = if styles.get("paragraph", "Standard").is_some() { "Standard".to_owned() } else { styles.named.first().cloned().unwrap_or_default() };
        let view = View {
            html: String::new(),
            css: String::new(),
            styles: vec![],
            char_styles: HashMap::new(),
            default_style: default_style.clone(),
            fonts: vec![],
            numbering: HashMap::new(),
            bullet_num: None,
            number_num: None,
            page,
            table_style: None,
            margins: vec![],
            title_pg: master.is_some_and(|m| m.child("header-first").is_some() || m.child("footer-first").is_some()),
            even_odd: master.is_some_and(|m| m.child("header-left").is_some() || m.child("footer-left").is_some()),
        };
        let out = Session { master: master.map(|m| m.span.clone()), master_children: master.map(|m| m.nodes().map(|c| (c.name.clone(), c.span.clone())).collect()).unwrap_or_default(), file: bytes.to_vec(), xml: String::new(), parts, autos, units: vec![], atoms: vec![], runs: vec![], links: vec![], styles: Styles { defs: HashMap::new(), default_p: PProps::default(), default_r: RProps::default(), named: vec![] }, names, images: vec![], view };
        let mut ctx = Ctx { xml: &all, part: 0, styles: &styles, zip, image_index: HashMap::new(), img_url, list_styles: &list_styles, numbering: HashMap::new(), last_list: HashMap::new(), out };
        let text = root.child("body").and_then(|b| b.child("text")).ok_or_else(|| invalid("not a text document (no office:text)"))?;
        let mut html = String::new();
        ctx.container(text, None, &mut html, &[], 0);
        let body_start = text.child("sequence-decls").map(|d| d.span.end).unwrap_or_else(|| start_tag_end(&all, text.span.start));
        for (element, kind, variant) in MARGINS {
            let Some(node) = master.and_then(|m| m.child(element)) else { continue };
            ctx.part = 1;
            let u = ctx.unit(Kind::Root, node, None);
            let mut part_html = String::new();
            ctx.container(node, Some(u), &mut part_html, &[], 0);
            ctx.out.view.margins.push(Margin { kind: kind.into(), variant: variant.into(), u, html: part_html });
        }
        let numbering = std::mem::take(&mut ctx.numbering);
        let mut session = ctx.out;
        session.units.push(Unit { kind: Kind::Block, part: 0, span: 0..0, parent: None, tag: String::new(), style: String::new(), direct: Direct::default(), marks: vec![], lists: vec![], content_start: body_start, empty: false, columns: vec![], header_group: None });

        // CSS: the page, then every paragraph and text style with its parents resolved.
        let mut css = format!(
            ".dx-page{{width:{}pt;min-height:{}pt;padding:{}pt {}pt {}pt {}pt;",
            num(page.w - page.left - page.right),
            num(page.h - page.top - page.bottom),
            num(page.top),
            num(page.right),
            num(page.bottom),
            num(page.left)
        );
        let mut base = RProps { font: Some("Liberation Serif".into()), size: Some(12.0), color: Some("auto".into()), ..Default::default() };
        base.over(&styles.default_r);
        base.css(&mut css);
        css.push_str("}\n");
        let mut fonts_used: HashSet<String> = fonts.values().cloned().collect();
        let mut keys: Vec<&(String, String)> = styles.defs.keys().collect();
        keys.sort();
        for (family, name) in keys {
            let (p, r) = styles.resolve(family, name);
            let mut rule = String::new();
            match family.as_str() {
                "paragraph" => {
                    let mut p = p;
                    p.num = None;
                    let contextual = p.contextual == Some(true);
                    p.css(&mut rule);
                    r.css(&mut rule);
                    let c = class_of(family, name, &styles);
                    let _ = writeln!(css, ".dx-page .dx-p-{c}{{{rule}}}");
                    // No space between paragraphs of the same style, as ODF's contextual spacing says.
                    if contextual {
                        let _ = writeln!(css, ".dx-page .dx-p-{c}+.dx-p-{c}{{padding-top:0}}.dx-page .dx-p-{c}:has(+.dx-p-{c}){{padding-bottom:0}}");
                    }
                }
                "text" => {
                    r.css(&mut rule);
                    let _ = writeln!(css, ".dx-page .dx-c-{}{{{rule}}}", class_of(family, name, &styles));
                }
                _ => {}
            }
            if let Some(f) = &r.font {
                fonts_used.insert(f.clone());
            }
        }
        // The style menu: the usual named styles, and the ones the document uses.
        let used: HashSet<String> = session.units.iter().filter(|u| u.kind == Kind::Para).map(|u| u.direct.style.clone()).collect();
        let usual = ["Standard", "Text_20_body", "Heading_20_1", "Heading_20_2", "Heading_20_3", "Title", "Subtitle", "Quotations", "Preformatted_20_Text", "List"];
        for name in &styles.named {
            let s = &styles.defs[&("paragraph".to_owned(), name.clone())];
            let (_, r) = styles.resolve("paragraph", name);
            let mut run = base.clone();
            run.over(&r);
            session.view.styles.push(StyleChoice { id: name.clone(), name: s.display.clone(), quick: usual.contains(&name.as_str()) || used.contains(name), run, num: None, next: s.next.clone().filter(|n| n != name) });
        }
        for ((family, name), s) in &styles.defs {
            if family == "text" && s.auto.is_none() {
                session.view.char_styles.insert(name.clone(), styles.resolve("text", name).1);
            }
        }
        let mut fonts_used: Vec<String> = fonts_used.into_iter().collect();
        fonts_used.sort();
        session.view.fonts = fonts_used;
        session.view.html = html;
        session.view.css = css;
        session.view.bullet_num = numbering.iter().filter(|(_, l)| l[0].fmt == "bullet").map(|(k, _)| k.clone()).min();
        session.view.number_num = numbering.iter().filter(|(_, l)| l[0].fmt != "bullet" && l[0].fmt != "none").map(|(k, _)| k.clone()).min();
        session.view.numbering = numbering;
        session.styles = styles;
        session.xml = all;
        Ok(session)
    }

    fn body(&self) -> usize {
        self.units.len() - 1
    }
}

impl Ctx<'_> {
    fn unit(&mut self, kind: Kind, node: &Node, parent: Option<usize>) -> usize {
        let start = start_tag_end(self.xml, node.span.start);
        let empty = self.xml[node.span.start..start].ends_with("/>");
        self.out.units.push(Unit { kind, part: self.part, span: node.span.clone(), parent, tag: String::new(), style: String::new(), direct: Direct::default(), marks: vec![], lists: vec![], content_start: start, empty, columns: vec![], header_group: None });
        self.out.units.len() - 1
    }

    /// The paragraphs, lists, tables and sections of the body, a cell or a header.
    fn container(&mut self, node: &Node, parent: Option<usize>, html: &mut String, lists: &[ListLevel], depth: usize) {
        if depth > super::MAX_DEPTH {
            return;
        }
        for child in node.nodes() {
            match child.name.as_str() {
                "p" | "h" => self.paragraph(child, parent, html, lists),
                "list" => self.list(child, parent, html, lists, depth),
                "table" => self.table(child, parent, html, depth),
                // Sections don't show: their paragraphs belong to the container around them.
                "section" => self.container(child, parent, html, lists, depth + 1),
                "sequence-decls" | "variable-decls" | "user-field-decls" | "soft-page-break" | "bookmark" | "bookmark-start" | "bookmark-end" => {}
                _ => {
                    let u = self.unit(Kind::Block, child, parent);
                    let _ = write!(html, "<div class=\"dx-block\" contenteditable=\"false\" data-u=\"{u}\">{}</div>", esc(&child.text()));
                }
            }
        }
    }

    fn list(&mut self, list: &Node, parent: Option<usize>, html: &mut String, lists: &[ListLevel], depth: usize) {
        let tag = self.xml[list.span.start..start_tag_end(self.xml, list.span.start)].to_owned();
        // The list's numbering: a new instance, or the one it continues.
        if lists.is_empty() {
            let style = list.attr("style-name").unwrap_or("").to_owned();
            let continues = list.attr("continue-numbering") == Some("true") || list.attr("continue-list").is_some();
            let key = match self.last_list.get(&style).filter(|_| continues) {
                Some(k) => k.clone(),
                None => {
                    let key = format!("{style}#{}", self.numbering.len() + 1);
                    let levels = self.list_styles.get(&style).cloned().unwrap_or_else(|| (0..10).map(|i| default_level(i, true)).collect());
                    self.numbering.insert(key.clone(), levels);
                    key
                }
            };
            self.last_list.insert(style, key);
        }
        for item in list.nodes().filter(|n| n.name == "list-item" || n.name == "list-header") {
            let mut inner = lists.to_vec();
            inner.push(ListLevel { list_tag: if tag.ends_with("/>") { format!("{}>", tag.trim_end_matches("/>")) } else { tag.clone() }, item: item.span.clone() });
            self.container(item, parent, html, &inner, depth + 1);
        }
    }

    fn table(&mut self, tbl: &Node, parent: Option<usize>, html: &mut String, depth: usize) {
        let t = self.unit(Kind::Table, tbl, parent);
        // Column widths, repeated as the file says.
        let mut widths: Vec<Option<f32>> = vec![];
        for col in tbl.nodes().flat_map(|n| if n.name == "table-columns" || n.name == "table-column-group" { n.nodes().collect::<Vec<_>>() } else { vec![n] }).filter(|n| n.name == "table-column") {
            let w = col.attr("style-name").and_then(|s| self.styles.defs.get(&("table-column".to_owned(), s.to_owned()))).and_then(|s| {
                let raw = xml::parse(s.raw.as_bytes()).ok()?;
                raw.child("table-column-properties")?.attr("column-width").and_then(length_pt)
            });
            let n: usize = col.attr("number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1).min(1024);
            widths.extend(std::iter::repeat_n(w, n));
            self.out.units[t].columns.push((col.span.clone(), n));
        }
        let bordered = tbl.find("table-cell").and_then(|c| c.attr("style-name")).and_then(|s| self.styles.defs.get(&("table-cell".to_owned(), s.to_owned()))).is_some_and(|s| s.raw.contains("fo:border") && !s.raw.contains("fo:border=\"none\""));
        let _ = write!(html, "<table class=\"dx-table{}\" data-u=\"{t}\"><tbody>", if bordered { " is-bordered" } else { "" });
        let rows: Vec<(&Node, Option<&Node>)> = tbl
            .nodes()
            .flat_map(|n| match n.name.as_str() {
                "table-header-rows" => n.nodes().map(|r| (r, Some(n))).collect::<Vec<_>>(),
                "table-rows" | "table-row-group" => n.nodes().map(|r| (r, None)).collect(),
                _ => vec![(n, None)],
            })
            .filter(|(n, _)| n.name == "table-row")
            .collect();
        for (tr, group) in rows {
            let r = self.unit(Kind::Row, tr, Some(t));
            if let Some(g) = group {
                let open = g.span.start..start_tag_end(self.xml, g.span.start);
                self.out.units[r].header_group = Some((open, g.span.end - "</table:table-header-rows>".len()..g.span.end));
            }
            let _ = write!(html, "<tr data-u=\"{r}\"{}>", if group.is_some() { " data-header=\"1\"" } else { "" });
            let mut col = 0;
            for tc in tr.nodes().filter(|n| n.name == "table-cell" || n.name == "covered-table-cell") {
                if tc.name == "covered-table-cell" {
                    col += 1;
                    continue;
                }
                let c = self.unit(Kind::Cell, tc, Some(r));
                let span: usize = tc.attr("number-columns-spanned").and_then(|v| v.parse().ok()).unwrap_or(1);
                let width: f32 = (col..col + span).filter_map(|i| widths.get(i).copied().flatten()).sum();
                col += span;
                let _ = write!(html, "<td data-u=\"{c}\"{}{}>", if span > 1 { format!(" colspan=\"{span}\"") } else { String::new() }, if width > 0.0 { format!(" style=\"width:{}pt\"", num(width)) } else { String::new() });
                let before = html.len();
                self.container(tc, Some(c), html, &[], depth + 1);
                if html.len() == before {
                    // An empty cell still needs a line for the cursor.
                    html.push_str("<p class=\"dx-p\" data-style=\"\" data-num=\"\" data-align=\"\" data-ind=\"\" data-line=\"\"><br></p>");
                }
                html.push_str("</td>");
            }
            html.push_str("</tr>");
        }
        html.push_str("</tbody></table>");
    }

    fn paragraph(&mut self, p: &Node, parent: Option<usize>, html: &mut String, lists: &[ListLevel]) {
        let u = self.unit(Kind::Para, p, parent);
        let style = p.attr("style-name").unwrap_or("").to_owned();
        let styles = self.styles;
        let own = styles.get("paragraph", &style).filter(|s| s.auto.is_some());
        let named = if style.is_empty() { String::new() } else { styles.named("paragraph", &style) };
        let numkey = (!lists.is_empty()).then(|| {
            // The instance key of the outermost list (`list` set it as we came in).
            let outer = xml::parse(format!("{}</text:list>", lists[0].list_tag).as_bytes()).ok();
            let style = outer.as_ref().and_then(|o| o.attr("style-name")).unwrap_or("").to_owned();
            format!("{}:{}", self.last_list.get(&style).cloned().unwrap_or_default(), lists.len() - 1)
        });
        let direct = Direct {
            style: if named == self.out.view.default_style { String::new() } else { named.clone() },
            num: numkey.clone().unwrap_or_default(),
            align: own.and_then(|s| s.p.align.clone()).unwrap_or_default(),
            ind: own.and_then(|s| s.p.left).map(|v| v.to_string()).unwrap_or_default(),
            line: own.and_then(|s| s.p.line.clone()).filter(|(_, rule)| rule == "auto").map(|(v, _)| v.to_string()).unwrap_or_default(),
        };
        let mut css = String::new();
        if let Some(key) = &numkey {
            let (k, lvl) = key.rsplit_once(':').unwrap_or((key, "0"));
            if let Some(level) = self.numbering.get(k).and_then(|l| l.get(lvl.parse::<usize>().unwrap_or(0))) {
                let _ = write!(css, "margin-left:{}pt;text-indent:{}pt;", num(level.left as f32 / 20.0), num(-level.hanging as f32 / 20.0));
            }
        }
        {
            let unit = &mut self.out.units[u];
            unit.tag = p.name.clone();
            unit.style = style.clone();
            unit.direct = direct.clone();
            unit.lists = lists.to_vec();
        }
        let class = if style.is_empty() { cls(&self.out.view.default_style) } else { class_of("paragraph", &style, styles) };
        let _ = write!(
            html,
            "<p class=\"dx-p dx-p-{class}\" data-u=\"{u}\" data-style=\"{}\" data-num=\"{}\" data-align=\"{}\" data-ind=\"{}\" data-line=\"{}\"{}>",
            esc(&direct.style),
            esc(&direct.num),
            esc(&direct.align),
            esc(&direct.ind),
            esc(&direct.line),
            if css.is_empty() { String::new() } else { format!(" style=\"{css}\"") }
        );
        let start = html.len();
        let mut space = true;
        self.inlines(p, u, html, &mut vec![], &mut space, 0);
        if !visible(&html[start..]) {
            html.push_str("<br>");
        }
        html.push_str("</p>");
    }

    /// Text, spans, links, line breaks and the things shown but not edited, in a paragraph.
    fn inlines(&mut self, node: &Node, u: usize, html: &mut String, spans: &mut Vec<String>, space: &mut bool, depth: usize) {
        if depth > 16 {
            return;
        }
        for c in &node.children {
            match c {
                xml::Child::Text(t) => {
                    // ODF white space: runs collapse to one space; none at a paragraph's start.
                    let mut text = String::new();
                    for ch in t.chars() {
                        if matches!(ch, ' ' | '\t' | '\n' | '\r') {
                            if !*space {
                                text.push(' ');
                                *space = true;
                            }
                        } else {
                            text.push(ch);
                            *space = false;
                        }
                    }
                    self.text(html, spans, &text);
                }
                xml::Child::Node(n) => match n.name.as_str() {
                    "span" => {
                        spans.push(n.attr("style-name").unwrap_or("").to_owned());
                        self.inlines(n, u, html, spans, space, depth + 1);
                        spans.pop();
                    }
                    "s" => {
                        let count: usize = n.attr("c").and_then(|c| c.parse().ok()).unwrap_or(1).min(1000);
                        self.text(html, spans, &" ".repeat(count));
                        *space = true;
                    }
                    "tab" => {
                        self.text(html, spans, "\t");
                        *space = false;
                    }
                    "line-break" => {
                        html.push_str("<br>");
                        *space = false;
                    }
                    "a" => {
                        let href = n.attr("href").unwrap_or("").to_owned();
                        let tag = self.xml[n.span.start..start_tag_end(self.xml, n.span.start)].to_owned();
                        self.out.links.push((tag, href.clone()));
                        let _ = write!(html, "<a href=\"{}\" data-h=\"{}\">", esc(&href), self.out.links.len() - 1);
                        self.inlines(n, u, html, spans, space, depth + 1);
                        html.push_str("</a>");
                    }
                    "soft-page-break" => {}
                    "bookmark" | "bookmark-start" | "bookmark-end" | "reference-mark" | "reference-mark-start" | "reference-mark-end" | "alphabetical-index-mark" | "toc-mark" | "toc-mark-start" | "toc-mark-end" => {
                        self.out.units[u].marks.push(n.span.clone());
                    }
                    "frame" => {
                        let (inner, attrs) = self.frame(n);
                        self.atom(html, n, &inner, &attrs);
                        *space = false;
                    }
                    "annotation" | "annotation-end" => self.atom(html, n, "", " hidden"),
                    "note" => {
                        let mark = n.child("note-citation").map(|c| c.text()).unwrap_or_else(|| "*".into());
                        self.atom(html, n, &format!("<sup>{}</sup>", esc(&mark)), "");
                    }
                    // Fields and the rest: their text as it shows, kept as they are.
                    _ => {
                        let t = n.text();
                        let hidden = if t.is_empty() { " hidden" } else { "" };
                        self.atom(html, n, &esc(&t), hidden);
                        *space = t.ends_with(' ');
                    }
                },
            }
        }
    }

    fn text(&mut self, html: &mut String, spans: &[String], text: &str) {
        if text.is_empty() {
            return;
        }
        self.out.runs.push(spans.to_vec());
        let k = self.out.runs.len() - 1;
        let inner = spans.iter().rev().find(|s| !s.is_empty());
        let class = inner.map(|s| class_of("text", s, self.styles));
        // A named character style lets the page compare the text with it.
        let named = inner.filter(|s| self.styles.get("text", s).is_some_and(|st| st.auto.is_none()));
        let outer: String = spans.iter().filter(|s| !s.is_empty() && Some(*s) != inner).map(|s| format!(" dx-c-{}", class_of("text", s, self.styles))).collect();
        let _ = write!(
            html,
            "<span data-r=\"{k}\"{}{}>{}</span>",
            class.map(|c| format!(" class=\"dx-c-{c}{outer}\"")).unwrap_or_default(),
            named.map(|s| format!(" data-cs=\"{}\"", esc(s))).unwrap_or_default(),
            esc(text)
        );
    }

    fn atom(&mut self, html: &mut String, n: &Node, inner: &str, attrs: &str) {
        self.out.atoms.push(self.xml[n.span.clone()].to_owned());
        let a = self.out.atoms.len() - 1;
        let _ = write!(html, "<span class=\"dx-atom\" contenteditable=\"false\" data-a=\"{a}\"{attrs}>{inner}</span>");
    }

    /// A frame's picture (or a box), and where it is: inline, or floating from its paragraph or page.
    fn frame(&mut self, f: &Node) -> (String, String) {
        let (w, h) = (f.attr("width").and_then(length_pt), f.attr("height").and_then(length_pt));
        let size = match (w, h) {
            (Some(w), Some(h)) => format!("width:{}pt;height:{}pt;", num(w), num(h)),
            _ => String::new(),
        };
        let mut attrs = String::new();
        if let (Some(w), Some(h)) = (w, h) {
            let _ = write!(attrs, " data-img=\"1\" data-w=\"{}\" data-h=\"{}\"", num(w), num(h));
            let anchor = f.attr("anchor-type").unwrap_or("paragraph");
            if anchor != "as-char" {
                let graphic = f.attr("style-name").and_then(|s| self.styles.get("graphic", s));
                let (hr, vr) = if anchor == "page" { ("page", "page") } else { ("margin", "paragraph") };
                let _ = write!(attrs, " data-float=\"1\" data-hr=\"{hr}\" data-vr=\"{vr}\"");
                match (f.attr("x").and_then(length_pt), graphic.and_then(|g| g.h_pos.as_deref())) {
                    (_, Some(pos @ ("center" | "right"))) => {
                        let _ = write!(attrs, " data-ha=\"{pos}\"");
                    }
                    (x, _) => {
                        let _ = write!(attrs, " data-hx=\"{}\"", num(x.unwrap_or(0.0)));
                    }
                }
                let _ = write!(attrs, " data-vy=\"{}\"", num(f.attr("y").and_then(length_pt).unwrap_or(0.0)));
            }
        }
        let href = f.child("image").and_then(|i| i.attr("href")).map(str::to_owned);
        let index = href.and_then(|path| {
            let path = path.trim_start_matches("./").to_owned();
            if let Some(&i) = self.image_index.get(&path) {
                return Some(i);
            }
            let bytes = zip_entry(&mut self.zip, &path).ok()??;
            let mime = image_mime(&bytes)?.to_owned();
            self.out.images.push(Image { mime, bytes });
            self.image_index.insert(path, self.out.images.len() - 1);
            Some(self.out.images.len() - 1)
        });
        let inner = match index {
            Some(i) => format!("<img src=\"{}\" alt=\"\" style=\"{size}\">", esc(&(self.img_url)(i))),
            None => format!("<span class=\"dx-shape\" style=\"{size}\"></span>"),
        };
        (inner, attrs)
    }
}

// ---------------------------------------------------------------- raw XML editing

/// An element's start tag with attributes set (`Some`) or removed (`None`); the rest kept.
fn retag(raw: &str, set: &[(&str, Option<String>)]) -> String {
    let end = start_tag_end(raw, 0);
    let tag = &raw[..end];
    let closed = tag.ends_with("/>");
    let mut reader = quick_xml::Reader::from_str(tag);
    let (name, mut attrs) = match reader.read_event() {
        Ok(quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e)) => (
            String::from(e.name().as_ref()),
            e.attributes().flatten().map(|a| (String::from(a.key.as_ref()), a.value.to_string())).collect::<Vec<(String, String)>>(),
        ),
        _ => return raw.to_owned(),
    };
    // Replaced where it was, added at the end, or removed.
    for (k, v) in set {
        match (attrs.iter().position(|(a, _)| a == k), v) {
            (Some(i), Some(v)) => attrs[i].1 = esc(v),
            (Some(i), None) => drop(attrs.remove(i)),
            (None, Some(v)) => attrs.push(((*k).to_owned(), esc(v))),
            (None, None) => {}
        }
    }
    let mut out = format!("<{name}");
    for (k, v) in attrs {
        let _ = write!(out, " {k}=\"{v}\"");
    }
    out.push_str(if closed { "/>" } else { ">" });
    out.push_str(&raw[end..]);
    out
}

/// `raw` with its child `qname`'s start tag changed (an empty child made first if it has none).
fn retag_child(raw: &str, qname: &str, set: &[(&str, Option<String>)]) -> String {
    let local = qname.rsplit(':').next().unwrap_or(qname);
    let Ok(node) = xml::parse(raw.as_bytes()) else { return raw.to_owned() };
    match node.child(local) {
        Some(c) => {
            let mut out = raw[..c.span.start].to_owned();
            out.push_str(&retag(&raw[c.span.clone()], set));
            out.push_str(&raw[c.span.end..]);
            out
        }
        None => {
            let fresh = retag(&format!("<{qname}/>"), set);
            let end = start_tag_end(raw, 0);
            if raw[..end].ends_with("/>") {
                format!("{}>{fresh}</{}>", raw[..end].trim_end_matches("/>"), node_name(raw))
            } else {
                format!("{}{fresh}{}", &raw[..end], &raw[end..])
            }
        }
    }
}

fn node_name(raw: &str) -> &str {
    raw[1..].split([' ', '>', '/']).next().unwrap_or("")
}

fn pt(v: f32) -> String {
    format!("{}pt", num(v))
}

// ---------------------------------------------------------------- saving

struct Out<'a> {
    edits: Vec<(usize, usize, usize, String)>,
    seq: usize,
    /// New automatic styles per part, and the ones made already (by content, for reuse).
    styles: Vec<String>,
    styles_parts: Vec<usize>,
    made: HashMap<(usize, String), String>,
    images: &'a [NewImage],
    media: Vec<(usize, String)>,
    /// The part being written (styles go into its automatic styles).
    part: usize,
    counter: usize,
    /// Edits into `<…/>` cells: their content, written when the cell is reopened.
    into_empty: HashMap<usize, Vec<(usize, String)>>,
}

impl Out<'_> {
    fn edit(&mut self, range: Range<usize>, text: String) {
        self.seq += 1;
        self.edits.push((range.start, range.end, 1_000_000 + self.seq, text));
    }

    fn insert(&mut self, at: usize, order: usize, text: String) {
        self.edits.push((at, at, order, text));
    }
}

impl Session {
    fn fresh_name(&self, out: &mut Out, prefix: &str) -> String {
        loop {
            out.counter += 1;
            let name = format!("{prefix}{}", out.counter);
            if !self.names.contains(&name) {
                return name;
            }
        }
    }

    /// Registers an automatic style (`body` has `NAME` where its name goes); the same one twice
    /// is written once.
    fn auto_style(&self, out: &mut Out, prefix: &str, body: String) -> String {
        if let Some(name) = out.made.get(&(out.part, body.clone())) {
            return name.clone();
        }
        let name = self.fresh_name(out, prefix);
        out.styles.push(body.replace("\u{0}NAME\u{0}", &esc(&name)));
        out.styles_parts.push(out.part);
        out.made.insert((out.part, body), name.clone());
        name
    }

    pub fn save(&self, edits: &Edits, images: &[NewImage]) -> Result<Vec<u8>, DocError> {
        let present: HashSet<usize> = edits.present.iter().copied().filter(|&u| u < self.units.len()).collect();
        let mut out = Out { edits: vec![], seq: 0, styles: vec![], styles_parts: vec![], made: HashMap::new(), images, media: vec![], part: 0, counter: 0, into_empty: HashMap::new() };
        let missing = |u: usize| !present.contains(&u);
        let ancestors = |u: usize| std::iter::successors(self.units[u].parent, |&p| self.units[p].parent);
        let mut deleted = HashSet::new();
        for (u, unit) in self.units.iter().enumerate().take(self.body()) {
            if !missing(u) || matches!(unit.kind, Kind::Cell | Kind::Root) || ancestors(u).any(|a| missing(a) && !matches!(self.units[a].kind, Kind::Cell | Kind::Root)) {
                continue;
            }
            if unit.kind == Kind::Para && ancestors(u).any(|a| matches!(self.units[a].kind, Kind::Cell | Kind::Root) && missing(a)) {
                continue;
            }
            deleted.insert(u);
        }
        for (t, unit) in self.units.iter().enumerate() {
            if unit.kind == Kind::Table && !deleted.contains(&t) {
                let rows: Vec<usize> = (0..self.units.len()).filter(|&r| self.units[r].kind == Kind::Row && self.units[r].parent == Some(t)).collect();
                if !rows.is_empty() && rows.iter().all(|r| deleted.contains(r)) {
                    rows.iter().for_each(|r| {
                        deleted.remove(r);
                    });
                    deleted.insert(t);
                }
            }
        }
        let mut cuts: Vec<(usize, usize)> = vec![];
        for rc in &edits.removed_cells {
            if rc.cell < self.body() && self.units[rc.cell].kind == Kind::Cell && missing(rc.cell) {
                deleted.insert(rc.cell);
                if let Some(t) = ancestors(rc.cell).find(|&a| self.units[a].kind == Kind::Table) {
                    cuts.push((t, rc.column));
                }
            }
        }
        let gone = |u: usize| deleted.contains(&u) || ancestors(u).any(|a| deleted.contains(&a));
        for &u in &deleted {
            if !ancestors(u).any(|a| deleted.contains(&a)) {
                out.edit(self.units[u].span.clone(), String::new());
            }
        }

        let mut written_in: HashMap<usize, usize> = HashMap::new();
        for p in &edits.paras {
            let container = p.container.filter(|&c| c < self.units.len() && matches!(self.units[c].kind, Kind::Cell | Kind::Root) && !gone(c)).unwrap_or(self.body());
            out.part = self.units[container].part;
            let original = p.u.filter(|&u| u < self.body() && self.units[u].kind == Kind::Para && !gone(u));
            let base = original.or(p.base.filter(|&b| b < self.body() && self.units[b].kind == Kind::Para));
            let para = self.paragraph_xml(p, original, base, &mut out)?;
            match original {
                Some(u) => {
                    let unit = &self.units[u];
                    // In or out of a list: the lists around it close before it and open again after.
                    let text = if p.num != unit.direct.num && !unit.lists.is_empty() { format!("{}{}{}", close(&unit.lists), self.wrap(&para, &p.num, &mut out)?, reopen(&unit.lists)) } else { self.wrap(&para, if p.num != unit.direct.num { &p.num } else { "" }, &mut out)? };
                    out.edit(unit.span.clone(), text);
                }
                None => {
                    let after = p.after.filter(|&a| a < self.body() && matches!(self.units[a].kind, Kind::Para | Kind::Table | Kind::Block) && self.units[a].parent.unwrap_or(self.body()) == container);
                    match after.map(|a| &self.units[a]).filter(|a| !a.lists.is_empty()) {
                        // After a list item: a new item of the same list, or out of it at that point.
                        Some(a) => {
                            let at = a.lists.last().map(|l| l.item.end).unwrap_or(a.span.end);
                            let text = if p.num == a.direct.num { format!("<text:list-item>{para}</text:list-item>") } else { format!("{}{}{}", close_after_item(&a.lists), self.wrap(&para, &p.num, &mut out)?, reopen_after_item(&a.lists)) };
                            out.insert(at, p.order, text);
                        }
                        None => {
                            let wrapped = self.wrap(&para, &p.num, &mut out)?;
                            self.put(&mut out, after, container, p.order, wrapped);
                        }
                    }
                    *written_in.entry(container).or_default() += 1;
                }
            }
        }
        for t in &edits.tables {
            let container = t.container.filter(|&c| c < self.units.len() && matches!(self.units[c].kind, Kind::Cell | Kind::Root) && !gone(c)).unwrap_or(self.body());
            out.part = self.units[container].part;
            let xml = self.table_xml(t, &mut out)?;
            let after = t.after.filter(|&a| a < self.body() && matches!(self.units[a].kind, Kind::Para | Kind::Table | Kind::Block) && self.units[a].parent.unwrap_or(self.body()) == container);
            self.put(&mut out, after, container, t.order, xml);
            *written_in.entry(container).or_default() += 1;
        }
        self.table_changes(edits, &cuts, &gone, &mut out)?;
        self.new_margins(edits, &mut out)?;
        // A cell must keep a paragraph.
        for (c, unit) in self.units.iter().enumerate() {
            if unit.kind != Kind::Cell || gone(c) || written_in.contains_key(&c) {
                continue;
            }
            let kept = (0..self.body()).any(|u| self.units[u].kind == Kind::Para && self.units[u].parent == Some(c) && !deleted.contains(&u));
            let had = (0..self.body()).any(|u| self.units[u].kind == Kind::Para && self.units[u].parent == Some(c));
            if had && !kept {
                self.put(&mut out, None, c, 0, "<text:p/>".into());
            }
        }
        // Self-closed cells that got content are written open.
        for (c, inner) in std::mem::take(&mut out.into_empty) {
            let mut inner = inner;
            inner.sort_by_key(|(o, _)| *o);
            let raw = &self.xml[self.units[c].span.clone()];
            let open = format!("{}>", raw.trim_end_matches("/>").trim_end());
            out.edit(self.units[c].span.clone(), format!("{open}{}</table:table-cell>", inner.into_iter().map(|(_, x)| x).collect::<String>()));
        }

        // New automatic styles, into each part's automatic-styles.
        for (i, part_auto) in self.autos.iter().enumerate() {
            let mine: String = out.styles.iter().zip(&out.styles_parts).filter(|(_, p)| **p == i).map(|(s, _)| s.as_str()).collect();
            if mine.is_empty() {
                continue;
            }
            match part_auto {
                Some(span) => {
                    let raw = &self.xml[span.clone()];
                    if raw.ends_with("/>") {
                        out.edit(span.clone(), format!("<office:automatic-styles>{mine}</office:automatic-styles>"));
                    } else {
                        let at = span.end - "</office:automatic-styles>".len();
                        out.insert(at, usize::MAX, mine);
                    }
                }
                None => {
                    // No automatic styles yet: right before the body (content) or the master styles.
                    let range = &self.parts[i].1;
                    let anchor = if i == 0 { "<office:body" } else { "<office:master-styles" };
                    let at = self.xml[range.clone()].find(anchor).map(|p| range.start + p).ok_or_else(|| invalid("no place for styles"))?;
                    out.insert(at, usize::MAX, format!("<office:automatic-styles>{mine}</office:automatic-styles>"));
                }
            }
        }

        out.edits.sort_by_key(|(s, e, seq, _)| (*s, e > s, *seq));
        let mut changed = vec![];
        for (name, range) in &self.parts {
            let mut xml = String::with_capacity(range.len());
            let mut at = range.start;
            for (start, end, _, text) in out.edits.iter().filter(|(s, ..)| range.contains(s)) {
                if *start < at || *end > range.end {
                    return Err(invalid("overlapping edits"));
                }
                xml.push_str(&self.xml[at..*start]);
                xml.push_str(text);
                at = *end;
            }
            xml.push_str(&self.xml[at..range.end]);
            if xml != self.xml[range.clone()] {
                let xml = tidy_lists(xml);
                xml::parse(xml.as_bytes())?;
                check_namespaces(&xml)?;
                changed.push((name.clone(), xml));
            }
        }
        if changed.is_empty() && out.media.is_empty() {
            // (A new header or footer changes styles.xml: never empty.)
            return Ok(self.file.clone());
        }
        self.package(changed, &out)
    }

    /// Puts something new after a unit, or first in a container (open self-closed cells first).
    fn put(&self, out: &mut Out, after: Option<usize>, container: usize, order: usize, text: String) {
        match after {
            Some(a) => out.insert(self.units[a].span.end, order, text),
            None if self.units[container].empty => out.into_empty.entry(container).or_default().push((order, text)),
            None => out.insert(self.units[container].content_start, order, text),
        }
    }

    /// A paragraph in the lists its `num` asks for ("style#n:level", "new-bullet:level"…), or alone.
    fn wrap(&self, para: &str, num: &str, out: &mut Out) -> Result<String, DocError> {
        let Some((key, level)) = num.rsplit_once(':') else { return Ok(para.to_owned()) };
        let level: usize = level.parse::<usize>().unwrap_or(0).min(9);
        let style = match key {
            "new-bullet" | "new-number" => self.new_list_style(key == "new-bullet", out),
            _ => key.split('#').next().unwrap_or("").to_owned(),
        };
        if style.is_empty() {
            return Ok(para.to_owned());
        }
        let mut s = format!("<text:list text:style-name=\"{}\" text:continue-numbering=\"true\"><text:list-item>", esc(&style));
        for _ in 0..level {
            s.push_str("<text:list><text:list-item>");
        }
        s.push_str(para);
        for _ in 0..level {
            s.push_str("</text:list-item></text:list>");
        }
        s.push_str("</text:list-item></text:list>");
        Ok(s)
    }

    fn new_list_style(&self, bullet: bool, out: &mut Out) -> String {
        let mut body = "<text:list-style style:name=\"\u{0}NAME\u{0}\">".to_owned();
        for i in 0..10u32 {
            let l = default_level(i, bullet);
            let indent = format!(
                "<style:list-level-properties text:list-level-position-and-space-mode=\"label-alignment\"><style:list-level-label-alignment text:label-followed-by=\"listtab\" fo:margin-left=\"{}\" fo:text-indent=\"-18pt\"/></style:list-level-properties>",
                pt(l.left as f32 / 20.0)
            );
            if bullet {
                let _ = write!(body, "<text:list-level-style-bullet text:level=\"{}\" text:bullet-char=\"{}\">{indent}</text:list-level-style-bullet>", i + 1, l.text);
            } else {
                let fmt = match l.fmt.as_str() {
                    "lowerLetter" => "a",
                    "lowerRoman" => "i",
                    _ => "1",
                };
                let _ = write!(body, "<text:list-level-style-number text:level=\"{}\" style:num-suffix=\".\" style:num-format=\"{fmt}\">{indent}</text:list-level-style-number>", i + 1);
            }
        }
        body.push_str("</text:list-style>");
        self.auto_style(out, if bullet { "JLB" } else { "JLN" }, body)
    }

    /// The paragraph's style name: the one it had, or a new automatic one with what changed.
    fn paragraph_style(&self, p: &EditPara, base: Option<&Unit>, out: &mut Out) -> String {
        let was = base.map(|b| b.direct.clone()).unwrap_or_default();
        let named = if p.style.is_empty() { self.view.default_style.clone() } else { p.style.clone() };
        let changed = p.align != was.align || p.ind != was.ind || p.line != was.line;
        let current = base.map(|b| b.style.clone()).unwrap_or_default();
        let own = base.and_then(|b| self.styles.get("paragraph", &b.style)).filter(|s| s.auto.is_some());
        if !changed && p.style == was.style {
            return if current.is_empty() { named } else { current };
        }
        if !changed && own.is_none() {
            return named;
        }
        let raw = own.map(|s| s.raw.clone()).unwrap_or_else(|| "<style:style style:family=\"paragraph\"></style:style>".to_owned());
        let mut body = retag(&raw, &[("style:name", Some("\u{0}NAME\u{0}".into())), ("style:parent-style-name", Some(named))]);
        let align = match p.align.as_str() {
            "center" => Some("center"),
            "right" => Some("end"),
            "both" => Some("justify"),
            "left" => Some("start"),
            _ => None,
        };
        let ind = p.ind.parse::<f32>().ok().map(|t| pt(t / 20.0));
        let line = p.line.parse::<f32>().ok().map(|v| format!("{}%", num(v / 2.4)));
        body = retag_child(&body, "style:paragraph-properties", &[("fo:text-align", align.map(str::to_owned)), ("fo:margin-left", ind), ("fo:line-height", line)]);
        self.auto_style(out, "JP", body.replace("&#0;NAME&#0;", "\u{0}NAME\u{0}"))
    }

    /// The span styles of a piece of text: as they were, or a new automatic style with its look.
    fn span_styles(&self, r: Option<usize>, set: Option<&Set>, out: &mut Out) -> Vec<String> {
        let chain = r.and_then(|r| self.runs.get(r)).cloned().unwrap_or_default();
        let Some(set) = set else { return chain.into_iter().filter(|s| !s.is_empty()).collect() };
        let inner = chain.iter().rev().find(|s| !s.is_empty()).cloned();
        let own = inner.as_ref().and_then(|s| self.styles.get("text", s));
        let (raw, parent) = match own {
            Some(s) if s.auto.is_some() => (s.raw.clone(), None),
            Some(_) => ("<style:style style:family=\"text\"></style:style>".to_owned(), inner.clone()),
            None => ("<style:style style:family=\"text\"></style:style>".to_owned(), None),
        };
        let mut body = retag(&raw, &[("style:name", Some("\u{0}NAME\u{0}".into()))]);
        if let Some(parent) = parent {
            body = retag(&body, &[("style:parent-style-name", Some(parent))]);
        }
        let font = set.font.as_ref().map(|f| format!("'{}'", f.replace(['\'', '"'], "")));
        let on = |v: Option<bool>, yes: &str, no: &str| v.map(|b| if b { yes.to_owned() } else { no.to_owned() });
        let props: Vec<(&str, Option<String>)> = vec![
            ("style:font-name", None),
            ("fo:font-family", font),
            ("fo:font-size", set.size.map(pt)),
            ("fo:font-weight", on(set.bold, "bold", "normal")),
            ("fo:font-style", on(set.italic, "italic", "normal")),
            ("style:text-underline-style", on(set.underline, "solid", "none")),
            ("style:text-underline-width", set.underline.filter(|u| *u).map(|_| "auto".into())),
            ("style:text-underline-color", set.underline.filter(|u| *u).map(|_| "font-color".into())),
            ("style:text-line-through-style", on(set.strike, "solid", "none")),
            ("fo:color", set.color.as_deref().and_then(|c| if c == "auto" { None } else { hex(c) }).map(|c| format!("#{c}"))),
            ("fo:background-color", set.bg.as_deref().and_then(hex).map(|c| format!("#{c}"))),
            ("style:text-position", set.vert.as_deref().and_then(|v| match v { "superscript" => Some("super 58%".to_owned()), "subscript" => Some("sub 58%".to_owned()), _ => None })),
        ];
        body = retag_child(&body, "style:text-properties", &props);
        vec![self.auto_style(out, "JT", body.replace("&#0;NAME&#0;", "\u{0}NAME\u{0}"))]
    }

    fn paragraph_xml(&self, p: &EditPara, original: Option<usize>, base: Option<usize>, out: &mut Out) -> Result<String, DocError> {
        let base_unit = base.map(|b| &self.units[b]);
        let style = self.paragraph_style(p, base_unit, out);
        let named = self.styles.named("paragraph", &style);
        let outline = self.styles.get("paragraph", &named).and_then(|s| s.outline.clone());
        let mut xml = match &outline {
            Some(level) => format!("<text:h text:style-name=\"{}\" text:outline-level=\"{}\">", esc(&style), esc(level)),
            None => format!("<text:p text:style-name=\"{}\">", esc(&style)),
        };
        if let Some(u) = original {
            for m in &self.units[u].marks {
                xml.push_str(&self.xml[m.clone()]);
            }
        }
        let mut space = true;
        self.items_xml(&p.items, &mut xml, out, &mut space, 0);
        xml.push_str(if outline.is_some() { "</text:h>" } else { "</text:p>" });
        Ok(xml)
    }

    fn items_xml(&self, items: &[Item], xml: &mut String, out: &mut Out, space: &mut bool, depth: usize) {
        for item in items {
            match item {
                Item::Text { text, r, set } => {
                    let spans = self.span_styles(*r, set.as_ref(), out);
                    for s in &spans {
                        let _ = write!(xml, "<text:span text:style-name=\"{}\">", esc(s));
                    }
                    xml.push_str(&odf_text(text, space));
                    for _ in &spans {
                        xml.push_str("</text:span>");
                    }
                }
                Item::Atom { a, geo } => {
                    if let Some(raw) = self.atoms.get(*a) {
                        match geo.as_ref().filter(|_| raw.contains("<draw:frame")) {
                            Some(g) => xml.push_str(&self.reshape(raw, g, out)),
                            None => xml.push_str(raw),
                        }
                        *space = false;
                    }
                }
                Item::Br => {
                    xml.push_str("<text:line-break/>");
                    *space = true;
                }
                Item::Image { n, geo } => {
                    if out.part == 0 {
                        if let Some(img) = self.image_xml(*n, geo.as_ref(), out) {
                            xml.push_str(&img);
                            *space = false;
                        }
                    }
                }
                Item::Link { href, h, items } => {
                    let same = h.and_then(|h| self.links.get(h)).filter(|(_, target)| target == href);
                    let open = match same {
                        Some((tag, _)) => Some(tag.clone()),
                        None if depth == 0 && safe_link(href) => Some(format!("<text:a xlink:type=\"simple\" xlink:href=\"{}\">", esc(href))),
                        None => None,
                    };
                    match open {
                        Some(tag) => {
                            xml.push_str(&tag);
                            self.items_xml(items, xml, out, space, depth + 1);
                            xml.push_str("</text:a>");
                        }
                        None => self.items_xml(items, xml, out, space, depth + 1),
                    }
                }
            }
        }
    }

    /// A frame with a new size, and if moved, floating from its paragraph at that place.
    fn reshape(&self, raw: &str, g: &Geo, out: &mut Out) -> String {
        let mut set = vec![("svg:width", Some(pt(g.w.max(1.0)))), ("svg:height", Some(pt(g.h.max(1.0))))];
        if g.moved {
            set.push(("text:anchor-type", Some("paragraph".into())));
            set.push(("svg:x", Some(pt(g.x - self.view.page.left))));
            set.push(("svg:y", Some(pt(g.y))));
            set.push(("draw:style-name", Some(self.float_style(raw, out))));
        }
        retag(raw, &set)
    }

    /// A graphic style that lets a frame sit where it was put, over the text.
    fn float_style(&self, frame: &str, out: &mut Out) -> String {
        let current = xml::parse(frame.as_bytes()).ok().and_then(|f| f.attr("style-name").map(str::to_owned));
        let own = current.as_ref().and_then(|s| self.styles.get("graphic", s));
        let (raw, parent) = match own {
            Some(s) if s.auto.is_some() => (s.raw.clone(), None),
            _ => ("<style:style style:family=\"graphic\"></style:style>".to_owned(), current.clone().or_else(|| Some("Graphics".into()))),
        };
        let mut body = retag(&raw, &[("style:name", Some("\u{0}NAME\u{0}".into()))]);
        if let Some(parent) = parent.filter(|p| self.styles.get("graphic", p).is_some()) {
            body = retag(&body, &[("style:parent-style-name", Some(parent))]);
        }
        body = retag_child(
            &body,
            "style:graphic-properties",
            &[
                ("style:wrap", Some("run-through".into())),
                ("style:run-through", Some("foreground".into())),
                ("style:horizontal-pos", Some("from-left".into())),
                ("style:horizontal-rel", Some("paragraph".into())),
                ("style:vertical-pos", Some("from-top".into())),
                ("style:vertical-rel", Some("paragraph".into())),
            ],
        );
        self.auto_style(out, "JFr", body.replace("&#0;NAME&#0;", "\u{0}NAME\u{0}"))
    }

    fn image_xml(&self, n: usize, geo: Option<&Geo>, out: &mut Out) -> Option<String> {
        let img = out.images.get(n)?;
        let path = match out.media.iter().find(|(m, _)| *m == n) {
            Some((_, p)) => p.clone(),
            None => {
                let p = format!("Pictures/jaye-{}-{}.{}", std::process::id(), out.media.len() + 1, img.ext);
                out.media.push((n, p.clone()));
                p
            }
        };
        let (w, h) = geo.map(|g| (g.w, g.h)).unwrap_or((img.w, img.h));
        let root = &self.xml[..self.parts[0].1.end.min(self.xml.find("<office:body").unwrap_or(0))];
        let ns: String = DRAWING_NS.iter().filter(|(p, u)| !root.contains(&format!("xmlns:{p}=\"{u}\""))).map(|(p, u)| format!(" xmlns:{p}=\"{u}\"")).collect();
        let frame = format!(
            "<draw:frame{ns} draw:name=\"JayE image {n}\" text:anchor-type=\"as-char\" svg:width=\"{}\" svg:height=\"{}\" draw:z-index=\"0\"><draw:image xlink:href=\"{path}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>",
            pt(w),
            pt(h)
        );
        Some(match geo.filter(|g| g.moved) {
            Some(g) => self.reshape(&frame, g, out),
            None => frame,
        })
    }

    fn children_of(&self, u: usize, kind: Kind) -> Vec<usize> {
        (0..self.body()).filter(|&c| self.units[c].kind == kind && self.units[c].parent == Some(u)).collect()
    }

    /// A new cell: the template's start tag without its spanning, and its paragraphs.
    fn cell_xml(&self, template: Option<usize>, paras: &[EditPara], out: &mut Out) -> Result<String, DocError> {
        let tag = template
            .map(|t| {
                let u = &self.units[t];
                let raw = &self.xml[u.span.start..start_tag_end(&self.xml, u.span.start)];
                let raw = if raw.ends_with("/>") { format!("{}>", raw.trim_end_matches("/>").trim_end()) } else { raw.to_owned() };
                retag(&raw, &[("table:number-columns-spanned", None), ("table:number-rows-spanned", None), ("office:value", None), ("calcext:value-type", None)])
            })
            .unwrap_or_else(|| "<table:table-cell office:value-type=\"string\">".to_owned());
        let mut xml = tag;
        for p in paras {
            xml.push_str(&self.paragraph_xml(p, None, None, out)?);
        }
        if paras.is_empty() {
            xml.push_str("<text:p/>");
        }
        xml.push_str("</table:table-cell>");
        Ok(xml)
    }

    /// Rows and cells added to the file's tables; the column list kept in step.
    fn table_changes(&self, edits: &Edits, cuts: &[(usize, usize)], gone: &dyn Fn(usize) -> bool, out: &mut Out) -> Result<(), DocError> {
        let valid = |u: usize, kind: Kind| u < self.body() && self.units[u].kind == kind && !gone(u);
        for r in &edits.rows {
            if !valid(r.table, Kind::Table) {
                continue;
            }
            let rows: Vec<usize> = (0..self.body()).filter(|&x| self.units[x].kind == Kind::Row && self.units[x].parent == Some(r.table)).collect();
            let Some(template) = Some(r.template).filter(|t| rows.contains(t)).or_else(|| rows.first().copied()) else { continue };
            out.part = self.units[r.table].part;
            let tu = &self.units[template];
            let mut xml = self.xml[tu.span.start..start_tag_end(&self.xml, tu.span.start)].to_owned();
            let tcells = self.children_of(template, Kind::Cell);
            for (i, paras) in r.cells.iter().enumerate() {
                xml.push_str(&self.cell_xml(tcells.get(i).or(tcells.last()).copied(), paras, out)?);
            }
            xml.push_str("</table:table-row>");
            let at = r.after.filter(|a| rows.contains(a)).map(|a| self.units[a].span.end).unwrap_or(tu.span.start);
            out.insert(at, r.order, xml);
        }
        // Net columns added (+) or taken out (−), per column entry of each table.
        let mut delta: HashMap<(usize, usize), i64> = HashMap::new();
        let entry = |t: usize, col: usize| {
            let mut covered = 0;
            for (i, (_, n)) in self.units[t].columns.iter().enumerate() {
                covered += n;
                if col < covered {
                    return Some(i);
                }
            }
            self.units[t].columns.len().checked_sub(1)
        };
        let mut grown = HashSet::new();
        for c in &edits.cells {
            if !valid(c.row, Kind::Row) {
                continue;
            }
            let table = std::iter::successors(self.units[c.row].parent, |&p| self.units[p].parent).find(|&a| self.units[a].kind == Kind::Table);
            let cells = self.children_of(c.row, Kind::Cell);
            let template = Some(c.template).filter(|&t| t < self.body() && self.units[t].kind == Kind::Cell).or_else(|| cells.first().copied());
            out.part = self.units[c.row].part;
            let xml = self.cell_xml(template, &c.paras, out)?;
            let at = match c.after.filter(|a| cells.contains(a)) {
                Some(a) => self.units[a].span.end,
                None => cells.first().map(|&f| self.units[f].span.start).unwrap_or(self.units[c.row].content_start),
            };
            out.insert(at, c.order, xml);
            if let Some(t) = table {
                if grown.insert((t, c.column)) {
                    if let Some(i) = entry(t, c.column) {
                        *delta.entry((t, i)).or_default() += 1;
                    }
                }
            }
        }
        let mut cut = HashSet::new();
        for &(t, col) in cuts {
            if cut.insert((t, col)) {
                if let Some(i) = entry(t, col) {
                    *delta.entry((t, i)).or_default() -= 1;
                }
            }
        }
        for ((t, i), d) in delta {
            let (span, n) = &self.units[t].columns[i];
            let left = *n as i64 + d;
            let raw = &self.xml[span.clone()];
            let text = if left <= 0 { String::new() } else { retag(raw, &[("table:number-columns-repeated", (left > 1).then(|| left.to_string()))]) };
            out.edit(span.clone(), text);
        }
        Ok(())
    }

    /// Headers and footers made where the file had none, and the first-page and even-page ones
    /// taken out when turned off (in ODF they exist or not); rows that start or stop repeating.
    fn new_margins(&self, edits: &Edits, out: &mut Out) -> Result<(), DocError> {
        for h in &edits.row_headers {
            let Some(row) = self.units.get(h.row).filter(|u| u.kind == Kind::Row && u.header_group.is_some() != h.on) else { continue };
            match &row.header_group {
                Some((open, close)) => {
                    out.edit(open.clone(), String::new());
                    out.edit(close.clone(), String::new());
                }
                None => {
                    out.insert(row.span.start, 0, "<table:table-header-rows>".into());
                    out.insert(row.span.end, 0, "</table:table-header-rows>".into());
                }
            }
        }
        let Some(master) = &self.master else { return Ok(()) };
        let off: Vec<&str> = [(edits.title_pg, self.view.title_pg, "first"), (edits.even_odd, self.view.even_odd, "even")].iter().filter(|(new, was, _)| *new == Some(false) && *was).map(|(_, _, v)| *v).collect();
        for (name, span) in &self.master_children {
            if MARGINS.iter().any(|(e, _, v)| e == name && off.contains(v)) {
                out.edit(span.clone(), String::new());
            }
        }
        let mut made: Vec<(usize, String)> = vec![];
        for NewMargin { kind, variant, paras } in &edits.new_margins {
            let Some(i) = MARGINS.iter().position(|(_, k, v)| k == kind && v == variant) else { continue };
            let element = MARGINS[i].0;
            if paras.is_empty() || self.master_children.iter().any(|(n, _)| n == element) || off.contains(&variant.as_str()) {
                continue;
            }
            out.part = 1;
            let mut xml = format!("<style:{element}>");
            for p in paras {
                xml.push_str(&self.paragraph_xml(p, None, None, out)?);
            }
            let _ = write!(xml, "</style:{element}>");
            made.push((i, xml));
        }
        // ODF has no switch for these: turned on with nothing written, an empty header keeps it on.
        for (on, was, variant) in [(edits.title_pg, self.view.title_pg, "first"), (edits.even_odd, self.view.even_odd, "even")] {
            let mine: Vec<usize> = (0..MARGINS.len()).filter(|&i| MARGINS[i].2 == variant).collect();
            if on == Some(true) && !was && !mine.iter().any(|&i| made.iter().any(|(j, _)| *j == i)) {
                made.push((mine[0], format!("<style:{0}><text:p/></style:{0}>", MARGINS[mine[0]].0)));
            }
        }
        if made.is_empty() {
            return Ok(());
        }
        made.sort_by_key(|(i, _)| *i);
        let raw = &self.xml[master.clone()];
        let open_end = start_tag_end(&self.xml, master.start);
        if raw.ends_with("/>") && open_end == master.end {
            let open = format!("{}>", raw.trim_end_matches("/>").trim_end());
            let inner: String = made.into_iter().map(|(_, x)| x).collect();
            out.edit(master.clone(), format!("{open}{inner}</style:master-page>"));
            return Ok(());
        }
        // Each after the last existing element that comes before it in ODF's order.
        for (i, xml) in made {
            let at = self
                .master_children
                .iter()
                .filter(|(n, _)| MARGINS.iter().position(|(e, _, _)| e == n).is_some_and(|k| k < i))
                .map(|(_, sp)| sp.end)
                .max()
                .unwrap_or(open_end);
            out.insert(at, i, xml);
        }
        Ok(())
    }

    fn table_xml(&self, t: &EditTable, out: &mut Out) -> Result<String, DocError> {
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0).clamp(1, 63);
        let cell = self.auto_style(out, "JCell", "<style:style style:name=\"\u{0}NAME\u{0}\" style:family=\"table-cell\"><style:table-cell-properties fo:padding=\"0.1cm\" fo:border=\"0.5pt solid #000000\"/></style:style>".into());
        let name = self.fresh_name(out, "JayE Table ");
        let mut xml = format!("<table:table table:name=\"{}\"><table:table-column table:number-columns-repeated=\"{cols}\"/>", esc(&name));
        for row in t.rows.iter().take(1000) {
            xml.push_str("<table:table-row>");
            for i in 0..cols {
                let _ = write!(xml, "<table:table-cell table:style-name=\"{}\" office:value-type=\"string\">", esc(&cell));
                let paras = row.get(i).map(Vec::as_slice).unwrap_or(&[]);
                for p in paras {
                    let para = self.paragraph_xml(p, None, None, out)?;
                    xml.push_str(&para);
                }
                if paras.is_empty() {
                    xml.push_str("<text:p/>");
                }
                xml.push_str("</table:table-cell>");
            }
            xml.push_str("</table:table-row>");
        }
        xml.push_str("</table:table>");
        Ok(xml)
    }

    fn package(&self, parts: Vec<(String, String)>, out: &Out) -> Result<Vec<u8>, DocError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(self.file.as_slice())).map_err(|e| invalid(e.to_string()))?;
        let mut replaced: HashMap<String, Vec<u8>> = parts.into_iter().map(|(n, x)| (n, x.into_bytes())).collect();
        if !out.media.is_empty() {
            let manifest = String::from_utf8(zip_entry(&mut zip, "META-INF/manifest.xml")?.ok_or_else(|| invalid("no manifest"))?).map_err(|_| invalid("manifest"))?;
            let end = manifest.rfind("</manifest:manifest>").ok_or_else(|| invalid("manifest"))?;
            let mut entries = String::new();
            for (n, path) in &out.media {
                let img = &out.images[*n];
                let mime = if img.ext == "jpeg" { "image/jpeg".to_owned() } else { format!("image/{}", img.ext) };
                let _ = write!(entries, "<manifest:file-entry manifest:full-path=\"{path}\" manifest:media-type=\"{mime}\"/>");
                replaced.insert(path.clone(), img.bytes.clone());
            }
            let mut manifest = manifest;
            manifest.insert_str(end, &entries);
            replaced.insert("META-INF/manifest.xml".into(), manifest.into_bytes());
        }
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut done = HashSet::new();
        for i in 0..zip.len() {
            let entry = zip.by_index_raw(i).map_err(|e| invalid(e.to_string()))?;
            let name = entry.name().map_err(|e| invalid(e.to_string()))?.into_owned();
            match replaced.get(&name) {
                Some(data) => {
                    drop(entry);
                    writer.start_file(name.as_str(), opts).map_err(|e| invalid(e.to_string()))?;
                    writer.write_all(data)?;
                }
                // The mimetype stays first and stored, as ODF requires (raw copy keeps both).
                None => writer.raw_copy_file(entry).map_err(|e| invalid(e.to_string()))?,
            }
            done.insert(name);
        }
        for (name, data) in &replaced {
            if !done.contains(name) {
                writer.start_file(name.as_str(), opts).map_err(|e| invalid(e.to_string()))?;
                writer.write_all(data)?;
            }
        }
        Ok(writer.finish().map_err(|e| invalid(e.to_string()))?.into_inner())
    }
}

/// Text in ODF: a space after a space (or at the paragraph's start) as `text:s`, tabs as `text:tab`.
fn odf_text(text: &str, space: &mut bool) -> String {
    let mut out = String::new();
    let mut pending = 0usize;
    let flush = |out: &mut String, pending: &mut usize| {
        if *pending > 0 {
            if *pending == 1 {
                out.push_str("<text:s/>");
            } else {
                let _ = write!(out, "<text:s text:c=\"{pending}\"/>");
            }
            *pending = 0;
        }
    };
    let mut chunk = String::new();
    for c in text.chars() {
        match c {
            ' ' if *space => {
                out.push_str(&xml_text(&chunk));
                chunk.clear();
                pending += 1;
            }
            ' ' => {
                flush(&mut out, &mut pending);
                chunk.push(' ');
                *space = true;
            }
            '\t' | '\n' => {
                out.push_str(&xml_text(&chunk));
                chunk.clear();
                flush(&mut out, &mut pending);
                out.push_str(if c == '\t' { "<text:tab/>" } else { "<text:line-break/>" });
                *space = c == '\n';
            }
            _ => {
                if pending > 0 {
                    out.push_str(&xml_text(&chunk));
                    chunk.clear();
                    flush(&mut out, &mut pending);
                }
                chunk.push(c);
                *space = false;
            }
        }
    }
    out.push_str(&xml_text(&chunk));
    flush(&mut out, &mut pending);
    out
}

/// Splitting lists leaves empty items and empty continuing lists: they go.
fn tidy_lists(mut xml: String) -> String {
    loop {
        let before = xml.len();
        xml = xml.replace("<text:list-item></text:list-item>", "");
        // An emptied list Jay-E reopened (it continues numbering and has nothing in it).
        let mut out = String::with_capacity(xml.len());
        let mut rest = xml.as_str();
        while let Some(i) = rest.find("<text:list ") {
            let end = start_tag_end(rest, i);
            let tag = &rest[i..end];
            if tag.contains("text:continue-numbering=\"true\"") && !tag.ends_with("/>") && rest[end..].starts_with("</text:list>") {
                out.push_str(&rest[..i]);
                rest = &rest[end + "</text:list>".len()..];
            } else {
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
        }
        out.push_str(rest);
        xml = out;
        if xml.len() == before {
            return xml;
        }
    }
}

/// Closing the lists around a paragraph (innermost first)…
fn close(lists: &[ListLevel]) -> String {
    "</text:list-item></text:list>".repeat(lists.len())
}

/// …and opening them again after it, the numbering going on.
fn reopen(lists: &[ListLevel]) -> String {
    lists.iter().map(|l| format!("{}<text:list-item>", continuing(&l.list_tag))).collect()
}

/// The same, right after the innermost item has closed.
fn close_after_item(lists: &[ListLevel]) -> String {
    let mut s = "</text:list>".to_owned();
    s.push_str(&"</text:list-item></text:list>".repeat(lists.len().saturating_sub(1)));
    s
}

fn reopen_after_item(lists: &[ListLevel]) -> String {
    let mut s: String = lists[..lists.len().saturating_sub(1)].iter().map(|l| format!("{}<text:list-item>", continuing(&l.list_tag))).collect();
    if let Some(last) = lists.last() {
        s.push_str(&continuing(&last.list_tag));
    }
    s
}

/// A list's start tag that goes on with the numbering (no `xml:id`, which must stay unique).
fn continuing(tag: &str) -> String {
    retag(tag, &[("xml:id", None), ("text:id", None), ("text:continue-list", None), ("text:continue-numbering", Some("true".into()))])
}

impl Editable for Session {
    fn view(&self) -> &View {
        &self.view
    }
    fn images(&self) -> &[Image] {
        &self.images
    }
    fn save(&self, edits: &Edits, images: &[NewImage]) -> Result<Vec<u8>, DocError> {
        Session::save(self, edits, images)
    }
    fn format(&self) -> &'static str {
        "odt"
    }
}

#[cfg(test)]
mod tests {
    use super::super::edit::{EditCell, EditRow};
    use super::*;

    const CONTENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3"><office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="center" fo:keep-with-next="always"/></style:style><style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold" fo:letter-spacing="0.1pt"/></style:style><text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="•"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment fo:margin-left="1.27cm" fo:text-indent="-0.635cm"/></style:list-level-properties></text:list-level-style-bullet></text:list-style></office:automatic-styles><office:body><office:text><text:sequence-decls/><text:h text:style-name="Heading_20_1" text:outline-level="1">Title</text:h><text:p text:style-name="P1">Centered <text:span text:style-name="T1">bold</text:span>  two<text:s/>spaces<text:bookmark text:name="b1"/></text:p><text:list text:style-name="L1"><text:list-item><text:p text:style-name="Standard">one</text:p></text:list-item><text:list-item><text:p text:style-name="Standard">two</text:p></text:list-item><text:list-item><text:p text:style-name="Standard">three</text:p></text:list-item></text:list><table:table table:name="T"><table:table-column table:number-columns-repeated="2"/><table:table-row><table:table-cell office:value-type="string"><text:p>cell</text:p></table:table-cell><table:table-cell/></table:table-row></table:table><text:p text:style-name="Standard">last</text:p></office:text></office:body></office:document-content>"#;

    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"><office:font-face-decls><style:font-face style:name="Liberation Sans" svg:font-family="'Liberation Sans'" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"/></office:font-face-decls><office:styles><style:default-style style:family="paragraph"><style:text-properties fo:font-size="12pt"/></style:default-style><style:style style:name="Standard" style:family="paragraph"/><style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Standard" style:default-outline-level="1"><style:text-properties style:font-name="Liberation Sans" fo:font-size="18pt" fo:font-weight="bold"/></style:style></office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2cm" fo:margin-right="2cm"/><style:header-style><style:header-footer-properties fo:min-height="0.6cm" fo:margin-bottom="0.25cm"/></style:header-style></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p text:style-name="Standard">Company</text:p></style:header></style:master-page></office:master-styles></office:document-styles>"#;

    fn sample() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("mimetype", stored).unwrap();
        w.write_all(b"application/vnd.oasis.opendocument.text").unwrap();
        w.start_file("content.xml", opts).unwrap();
        w.write_all(CONTENT.as_bytes()).unwrap();
        w.start_file("styles.xml", opts).unwrap();
        w.write_all(STYLES.as_bytes()).unwrap();
        w.start_file("META-INF/manifest.xml", opts).unwrap();
        w.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/></manifest:manifest>"#).unwrap();
        w.finish().unwrap().into_inner()
    }

    fn open(bytes: &[u8]) -> Session {
        Session::open(bytes, &|n| format!("img/{n}")).unwrap()
    }

    fn entry(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        String::from_utf8(zip_entry(&mut zip, name).unwrap().unwrap()).unwrap()
    }

    fn all(s: &Session) -> Vec<usize> {
        (0..s.units.len()).collect()
    }

    fn para(s: &Session, text: &str) -> usize {
        (0..s.body()).find(|&u| s.units[u].kind == Kind::Para && s.xml[s.units[u].span.clone()].contains(text)).unwrap()
    }

    fn edit(u: usize, s: &Session, items: Vec<Item>) -> EditPara {
        let d = s.units[u].direct.clone();
        EditPara { order: 0, u: Some(u), base: None, after: None, container: None, style: d.style, num: d.num, align: d.align, ind: d.ind, line: d.line, items }
    }

    fn text(t: &str, r: Option<usize>) -> Item {
        Item::Text { text: t.into(), r, set: None }
    }

    fn save(s: &Session, paras: Vec<EditPara>) -> Vec<u8> {
        s.save(&Edits { present: all(s), paras, ..Default::default() }, &[]).unwrap()
    }

    #[test]
    fn opens_with_its_look_and_saves_unchanged() {
        let bytes = sample();
        let s = open(&bytes);
        let v = &s.view;
        assert!(v.html.contains("data-style=\"Heading_20_1\"") && v.html.contains("data-align=\"center\""), "{}", v.html);
        // Collapsed spaces read as ODF says: "  two" is one space, text:s another.
        assert!(v.html.contains("> two</span>") || v.html.contains("two"), "{}", v.html);
        assert!(v.html.contains(">  </span>") || v.html.contains(">two</span>") || v.html.contains(" two"));
        assert!(v.css.contains(".dx-page{width:481.89pt;"), "{}", v.css);
        assert!(v.css.contains("font-family:'Liberation Sans',sans-serif;font-size:18pt;font-weight:700"), "{}", v.css);
        assert!(v.margins.iter().any(|m| m.kind == "header" && m.html.contains("Company")));
        let key = v.bullet_num.clone().unwrap();
        assert!(key.starts_with("L1#"));
        assert_eq!(v.numbering[&key][0].left, 720);
        assert!(v.html.contains(&format!("data-num=\"{key}:0\"")));
        assert_eq!(save(&s, vec![]), bytes);
    }

    #[test]
    fn text_and_formatting_get_automatic_styles() {
        let bytes = sample();
        let s = open(&bytes);
        let p = para(&s, "Centered");
        let bold_run = 1;
        let set = Set { italic: Some(true), color: Some("c00000".into()), size: Some(14.0), ..Default::default() };
        let mut e = edit(p, &s, vec![text("Centered ", Some(0)), Item::Text { text: "bold".into(), r: Some(bold_run), set: Some(set) }, text("  and  spaces", None)]);
        e.align = "right".into();
        let saved = save(&s, vec![e]);
        let c = entry(&saved, "content.xml");
        check_namespaces(&c).unwrap();
        // The paragraph's own style copied (keep-with-next kept), aligned right.
        assert!(c.contains("<style:style style:name=\"JP1\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:text-align=\"end\" fo:keep-with-next=\"always\"/></style:style>"), "{c}");
        // The bold run's style copied: still bold and spaced, now italic, red, 14 pt.
        assert!(c.contains("fo:font-weight=\"bold\" fo:letter-spacing=\"0.1pt\"") && c.contains("fo:font-style=\"italic\"") && c.contains("fo:color=\"#c00000\"") && c.contains("fo:font-size=\"14pt\""), "{c}");
        assert!(c.contains("<text:p text:style-name=\"JP1\"><text:bookmark text:name=\"b1\"/>Centered <text:span text:style-name=\"JT"), "{c}");
        assert!(c.contains("<text:s/>and <text:s/>spaces</text:p>"), "{c}");
        // Untouched parts as they were.
        assert!(c.contains("<text:h text:style-name=\"Heading_20_1\" text:outline-level=\"1\">Title</text:h>"));
        assert_eq!(entry(&saved, "styles.xml"), entry(&bytes, "styles.xml"));
        // Jay-E reads it back.
        let doc = crate::doc::odt::read(&saved).unwrap();
        assert!(crate::doc::text::write(&doc).contains("Centered bold  and  spaces"), "{}", crate::doc::text::write(&doc));
    }

    #[test]
    fn lists_split_join_and_continue() {
        let bytes = sample();
        let s = open(&bytes);
        let key = s.view.bullet_num.clone().unwrap();
        // "two" leaves the list; "last" joins it; a new item after "three".
        let two = para(&s, ">two<");
        let mut a = edit(two, &s, vec![text("two", None)]);
        a.num = String::new();
        let last = para(&s, ">last<");
        let mut b = edit(last, &s, vec![text("last", None)]);
        b.num = format!("{key}:0");
        let three = para(&s, ">three<");
        let mut c = edit(three, &s, vec![text("four", None)]);
        c.u = None;
        c.base = Some(three);
        c.after = Some(three);
        let saved = save(&s, vec![a, b, c]);
        let x = entry(&saved, "content.xml");
        check_namespaces(&x).unwrap();
        assert!(x.contains("one</text:p></text:list-item></text:list-item></text:list><text:p text:style-name=\"Standard\">two</text:p><text:list text:style-name=\"L1\" text:continue-numbering=\"true\"><text:list-item><text:list-item>") || x.contains("</text:list><text:p text:style-name=\"Standard\">two</text:p><text:list text:style-name=\"L1\" text:continue-numbering=\"true\">"), "{x}");
        assert!(x.contains("three</text:p></text:list-item><text:list-item><text:p text:style-name=\"Standard\">four</text:p></text:list-item>"), "{x}");
        assert!(x.contains("<text:list text:style-name=\"L1\" text:continue-numbering=\"true\"><text:list-item><text:p text:style-name=\"Standard\">last</text:p></text:list-item></text:list>"), "{x}");
        assert!(!x.contains("<text:list-item></text:list-item>"), "{x}");
        let doc = crate::doc::odt::read(&saved).unwrap();
        let plain = crate::doc::text::write(&doc);
        for t in ["one", "two", "three", "four", "last"] {
            assert!(plain.contains(t), "{plain}");
        }
    }

    #[test]
    fn header_cells_tables_and_pictures() {
        let bytes = sample();
        let s = open(&bytes);
        // The header is in styles.xml.
        let header = s.view.margins.iter().find(|m| m.kind == "header" && m.variant == "default").cloned().unwrap();
        let company = para(&s, "Company");
        let mut h = edit(company, &s, vec![text("Company Ltd.", None)]);
        h.container = Some(header.u);
        // A paragraph into the self-closed cell.
        let cells: Vec<usize> = (0..s.body()).filter(|&u| s.units[u].kind == Kind::Cell).collect();
        let mut into = edit(para(&s, "cell"), &s, vec![text("new", None)]);
        into.u = None;
        into.container = Some(cells[1]);
        // A table and a picture.
        let png = {
            let mut v = Vec::new();
            image::DynamicImage::new_rgb8(40, 20).write_to(&mut Cursor::new(&mut v), image::ImageFormat::Png).unwrap();
            v
        };
        let img = super::super::edit::prepare_image(&png).unwrap();
        let last = para(&s, ">last<");
        let mut l = edit(last, &s, vec![text("last", None), Item::Image { n: 0, geo: Some(Geo { moved: true, x: 120.0, y: 10.0, w: 30.0, h: 15.0 }) }]);
        l.order = 1;
        let cell = |t: &str| vec![{
            let mut p = edit(last, &s, vec![text(t, None)]);
            p.u = None;
            p
        }];
        let table = EditTable { order: 2, after: Some(last), container: None, cols: vec![], rows: vec![vec![cell("a"), cell("b")]] };
        let saved = s.save(&Edits { present: all(&s), paras: vec![h, into, l], tables: vec![table], ..Default::default() }, &[img]).unwrap();
        let st = entry(&saved, "styles.xml");
        assert!(st.contains("<style:header><text:p text:style-name=\"Standard\">Company Ltd.</text:p></style:header>"), "{st}");
        let c = entry(&saved, "content.xml");
        check_namespaces(&c).unwrap();
        assert!(c.contains("<table:table-cell office:value-type=\"string\"><text:p>cell</text:p></table:table-cell><table:table-cell><text:p text:style-name=\"Standard\">new</text:p></table:table-cell>"), "{c}");
        assert!(c.contains("text:anchor-type=\"paragraph\"") && c.contains("svg:x=\"63.31pt\"") && c.contains("svg:width=\"30pt\""), "{c}");
        assert!(c.contains("style:wrap=\"run-through\""), "{c}");
        assert!(c.contains("<table:table table:name=\"JayE Table ") && c.contains("fo:border=\"0.5pt solid #000000\""), "{c}");
        let manifest = entry(&saved, "META-INF/manifest.xml");
        assert!(manifest.contains("manifest:full-path=\"Pictures/jaye-"), "{manifest}");
        // The mimetype is still the first entry, stored.
        let mut zip = zip::ZipArchive::new(Cursor::new(saved.clone())).unwrap();
        let first = zip.by_index(0).unwrap();
        assert_eq!((first.name().unwrap().into_owned(), first.compression()), ("mimetype".to_owned(), zip::CompressionMethod::Stored));
        drop(first);
        let doc = crate::doc::odt::read(&saved).unwrap();
        assert_eq!(doc.images.len(), 1);
    }

    #[test]
    fn rows_columns_and_a_new_footer() {
        let bytes = sample();
        let s = open(&bytes);
        let table = (0..s.body()).find(|&u| s.units[u].kind == Kind::Table).unwrap();
        let row = s.children_of(table, Kind::Row)[0];
        let cells = s.children_of(row, Kind::Cell);
        let para = |t: &str| {
            let mut p = edit(para(&s, ">last<"), &s, vec![text(t, None)]);
            p.u = None;
            p
        };
        let new_row = EditRow { order: 1, table, after: Some(row), template: row, cells: vec![vec![para("r2c1")], vec![], vec![para("r2c3")]] };
        let new_cell = EditCell { order: 2, row, after: Some(cells[1]), template: cells[1], column: 2, paras: vec![para("r1c3")] };
        let saved = s.save(&Edits { present: all(&s), rows: vec![new_row], cells: vec![new_cell], new_margins: vec![NewMargin { kind: "footer".into(), variant: "default".into(), paras: vec![para("Page footer")] }], ..Default::default() }, &[]).unwrap();
        let c = entry(&saved, "content.xml");
        check_namespaces(&c).unwrap();
        assert!(c.contains("<table:table-column table:number-columns-repeated=\"3\"/>"), "{c}");
        assert!(c.contains("<table:table-cell/><table:table-cell><text:p text:style-name=\"Standard\">r1c3</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type=\"string\"><text:p text:style-name=\"Standard\">r2c1</text:p></table:table-cell><table:table-cell><text:p/></table:table-cell>"), "{c}");
        let st = entry(&saved, "styles.xml");
        assert!(st.contains("</style:header><style:footer><text:p text:style-name=\"Standard\">Page footer</text:p></style:footer></style:master-page>"), "{st}");
        let again = open(&saved);
        assert!(again.view.margins.iter().any(|m| m.kind == "footer" && m.html.contains("Page footer")));
        let doc = crate::doc::odt::read(&saved).unwrap();
        assert!(crate::doc::text::write(&doc).contains("r1c3"));
        // A column taken out again: one fewer in the column list.
        let t2 = (0..again.body()).find(|&u| again.units[u].kind == Kind::Table).unwrap();
        let gone: Vec<usize> = again.children_of(t2, Kind::Row).iter().map(|&r| again.children_of(r, Kind::Cell)[2]).collect();
        let present: Vec<usize> = all(&again).into_iter().filter(|u| !gone.contains(u) && !gone.iter().any(|g| again.units[*u].parent == Some(*g))).collect();
        let removed = gone.iter().map(|&cell| super::super::edit::RemovedCell { cell, column: 2 }).collect();
        let back = again.save(&Edits { present, removed_cells: removed, ..Default::default() }, &[]).unwrap();
        let c2 = entry(&back, "content.xml");
        assert!(c2.contains("<table:table-column table:number-columns-repeated=\"2\"/>") && !c2.contains("r1c3") && !c2.contains("r2c3"), "{c2}");
    }

    #[test]
    fn first_and_left_page_headers_and_repeated_rows() {
        let bytes = sample();
        let s = open(&bytes);
        assert!(!s.view.title_pg && !s.view.even_odd);
        let para = |t: &str| {
            let mut p = edit(para(&s, ">last<"), &s, vec![text(t, None)]);
            p.u = None;
            p
        };
        let table = (0..s.body()).find(|&u| s.units[u].kind == Kind::Table).unwrap();
        let row = s.children_of(table, Kind::Row)[0];
        let margins = vec![NewMargin { kind: "header".into(), variant: "first".into(), paras: vec![para("Cover")] }, NewMargin { kind: "footer".into(), variant: "even".into(), paras: vec![para("Even")] }];
        let saved = s.save(&Edits { present: all(&s), new_margins: margins, title_pg: Some(true), even_odd: Some(true), row_headers: vec![super::super::edit::RowHeader { row, on: true }], ..Default::default() }, &[]).unwrap();
        let st = entry(&saved, "styles.xml");
        assert!(st.contains("<style:header><text:p text:style-name=\"Standard\">Company</text:p></style:header><style:header-first><text:p text:style-name=\"Standard\">Cover</text:p></style:header-first><style:footer-left><text:p text:style-name=\"Standard\">Even</text:p></style:footer-left></style:master-page>"), "{st}");
        let c = entry(&saved, "content.xml");
        assert!(c.contains("<table:table-header-rows><table:table-row>") && c.contains("</table:table-row></table:table-header-rows>"), "{c}");
        let again = open(&saved);
        assert!(again.view.title_pg && again.view.even_odd && again.view.html.contains("data-header=\"1\""));
        // Off again: the first-page header goes; the header row stops repeating.
        let t2 = (0..again.body()).find(|&u| again.units[u].kind == Kind::Table).unwrap();
        let r2 = again.children_of(t2, Kind::Row)[0];
        let first = again.view.margins.iter().find(|m| m.variant == "first").unwrap().u;
        let present: Vec<usize> = all(&again).into_iter().filter(|&u| u != first && again.units[u].parent != Some(first)).collect();
        let off = again.save(&Edits { present, title_pg: Some(false), row_headers: vec![super::super::edit::RowHeader { row: r2, on: false }], ..Default::default() }, &[]).unwrap();
        let st2 = entry(&off, "styles.xml");
        assert!(!st2.contains("header-first") && st2.contains("footer-left"), "{st2}");
        assert!(!entry(&off, "content.xml").contains("table-header-rows"));
        // On with nothing written: still on when opened again.
        let on = s.save(&Edits { present: all(&s), even_odd: Some(true), ..Default::default() }, &[]).unwrap();
        assert!(entry(&on, "styles.xml").contains("<style:header-left><text:p/></style:header-left>") && open(&on).view.even_odd);
    }

    #[test]
    fn odf_spaces() {
        let mut space = true;
        assert_eq!(odf_text(" a  b\tc", &mut space), "<text:s/>a <text:s/>b<text:tab/>c");
        let mut space = false;
        assert_eq!(odf_text("x   ", &mut space), "x <text:s text:c=\"2\"/>");
        assert_eq!(retag("<a b=\"1\" c=\"2\"><x/></a>", &[("b", None), ("d", Some("3&".into()))]), "<a c=\"2\" d=\"3&amp;\"><x/></a>");
        assert_eq!(retag_child("<s:s n=\"1\"/>", "s:p", &[("k", Some("v".into()))]), "<s:s n=\"1\"><s:p k=\"v\"/></s:s>");
    }
}
