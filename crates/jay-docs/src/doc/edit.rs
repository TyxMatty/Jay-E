//! Editing a DOCX in place (stage 7), keeping everything Jay-E doesn't touch.
//!
//! `Session::open` turns word/document.xml into HTML for the editor, with the file's own look:
//! page size and margins, fonts, sizes, colors, spacing and indents, from the document, its styles,
//! its theme and its numbering. Every paragraph, table, row and cell is a *unit* (`data-u`) that
//! remembers where it is in the XML.
//!
//! `Session::save` gets back only the paragraphs that changed (and which units are still on the
//! page), and patches the XML: a changed paragraph keeps its own properties (`w:pPr`), bookmarks,
//! images and fields, and each piece of text keeps the run properties it came from, with only the
//! ones the toolbar knows (font, size, bold, color…) overridden. The rest of document.xml and every
//! other part of the package are copied byte for byte (docs/ARQUITETURA.md, "ida e volta sem perda").

use super::xml::{self, Node};
use super::{DocError, Image, image_mime, invalid, zip_entry};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{Cursor, Write};
use std::ops::Range;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const HYPERLINK_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
const NUMBERING_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const NUMBERING_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";

/// The order Word requires inside `w:pPr` and `w:rPr` (ECMA-376 CT_PPr / CT_RPr).
const PPR_ORDER: [&str; 36] = [
    "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr", "widowControl", "numPr", "suppressLineNumbers", "pBdr", "shd", "tabs", "suppressAutoHyphens", "kinsoku", "wordWrap",
    "overflowPunct", "topLinePunct", "autoSpaceDE", "autoSpaceDN", "bidi", "adjustRightInd", "snapToGrid", "spacing", "ind", "contextualSpacing", "mirrorIndents", "suppressOverlap", "jc",
    "textDirection", "textAlignment", "textboxTightWrap", "outlineLvl", "divId", "cnfStyle", "rPr", "sectPr", "pPrChange",
];
const RPR_ORDER: [&str; 40] = [
    "rStyle", "rFonts", "b", "bCs", "i", "iCs", "caps", "smallCaps", "strike", "dstrike", "outline", "shadow", "emboss", "imprint", "noProof", "snapToGrid", "vanish", "webHidden", "color",
    "spacing", "w", "kern", "position", "sz", "szCs", "highlight", "u", "effect", "bdr", "shd", "fitText", "vertAlign", "rtl", "cs", "em", "lang", "eastAsianLayout", "specVanish", "oMath",
    "rPrChange",
];

/// Word's 16 highlight colors.
const HIGHLIGHTS: [(&str, &str); 16] = [
    ("yellow", "ffff00"),
    ("green", "00ff00"),
    ("cyan", "00ffff"),
    ("magenta", "ff00ff"),
    ("blue", "0000ff"),
    ("red", "ff0000"),
    ("darkBlue", "000080"),
    ("darkCyan", "008080"),
    ("darkGreen", "008000"),
    ("darkMagenta", "800080"),
    ("darkRed", "800000"),
    ("darkYellow", "808000"),
    ("darkGray", "808080"),
    ("lightGray", "c0c0c0"),
    ("black", "000000"),
    ("white", "ffffff"),
];

// ---------------------------------------------------------------- properties

/// Run properties Jay-E shows and edits; `None` = not said at this level.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RProps {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// Hex without '#', lowercase; "auto" for automatic.
    pub color: Option<String>,
    /// Hex background (highlight or shading); "" for none.
    pub bg: Option<String>,
    /// "superscript", "subscript" or "baseline".
    pub vert: Option<String>,
    pub caps: Option<bool>,
    pub small_caps: Option<bool>,
}

impl RProps {
    pub(super) fn over(&mut self, o: &RProps) {
        macro_rules! take { ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* } }
        take!(font, size, bold, italic, underline, strike, color, bg, vert, caps, small_caps);
    }

    pub(super) fn css(&self, out: &mut String) {
        if let Some(f) = &self.font {
            let _ = write!(out, "font-family:{};", font_css(f));
        }
        if let Some(s) = self.size {
            let _ = write!(out, "font-size:{}pt;", num(s));
        }
        if let Some(b) = self.bold {
            out.push_str(if b { "font-weight:700;" } else { "font-weight:400;" });
        }
        if let Some(i) = self.italic {
            out.push_str(if i { "font-style:italic;" } else { "font-style:normal;" });
        }
        if self.underline.is_some() || self.strike.is_some() {
            let mut lines = vec![];
            if self.underline == Some(true) {
                lines.push("underline");
            }
            if self.strike == Some(true) {
                lines.push("line-through");
            }
            let _ = write!(out, "text-decoration-line:{};", if lines.is_empty() { "none".into() } else { lines.join(" ") });
        }
        match self.color.as_deref() {
            Some("auto") => out.push_str("color:#000000;"),
            Some(c) => {
                let _ = write!(out, "color:#{c};");
            }
            None => {}
        }
        if let Some(bg) = self.bg.as_deref().filter(|b| !b.is_empty()) {
            let _ = write!(out, "background-color:#{bg};");
        }
        match self.vert.as_deref() {
            Some("superscript") => out.push_str("vertical-align:super;font-size:smaller;"),
            Some("subscript") => out.push_str("vertical-align:sub;font-size:smaller;"),
            _ => {}
        }
        if let Some(c) = self.caps {
            out.push_str(if c { "text-transform:uppercase;" } else { "text-transform:none;" });
        }
        if let Some(c) = self.small_caps {
            out.push_str(if c { "font-variant:small-caps;" } else { "font-variant:normal;" });
        }
    }
}

/// Paragraph properties Jay-E shows; twips (1/20 pt) for lengths.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct PProps {
    pub(super) align: Option<String>,
    pub(super) before: Option<i32>,
    pub(super) after: Option<i32>,
    /// (value, rule): "auto" in 240ths of a line, else twips.
    pub(super) line: Option<(i32, String)>,
    pub(super) left: Option<i32>,
    pub(super) right: Option<i32>,
    /// First-line indent; negative for hanging.
    pub(super) first: Option<i32>,
    pub(super) num: Option<(String, u32)>,
    /// No space between paragraphs of the same style ("contextualSpacing").
    pub(super) contextual: Option<bool>,
}

impl PProps {
    pub(super) fn over(&mut self, o: &PProps) {
        macro_rules! take { ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* } }
        take!(align, before, after, line, left, right, first, num, contextual);
    }

    pub(super) fn css(&self, out: &mut String) {
        if let Some(a) = &self.align {
            let _ = write!(out, "text-align:{};", css_align(a));
        }
        if let Some(b) = self.before {
            let _ = write!(out, "padding-top:{}pt;", num(b as f32 / 20.0));
        }
        if let Some(a) = self.after {
            let _ = write!(out, "padding-bottom:{}pt;", num(a as f32 / 20.0));
        }
        if let Some((v, rule)) = &self.line {
            if rule == "auto" {
                let _ = write!(out, "line-height:{};", num(*v as f32 / 240.0 * 1.15));
            } else {
                let _ = write!(out, "line-height:{}pt;", num(*v as f32 / 20.0));
            }
        }
        if let Some(l) = self.left {
            let _ = write!(out, "margin-left:{}pt;", num(l as f32 / 20.0));
        }
        if let Some(r) = self.right {
            let _ = write!(out, "margin-right:{}pt;", num(r as f32 / 20.0));
        }
        if let Some(f) = self.first {
            let _ = write!(out, "text-indent:{}pt;", num(f as f32 / 20.0));
        }
    }
}

fn css_align(jc: &str) -> &'static str {
    match jc {
        "center" => "center",
        "right" | "end" => "right",
        "both" | "distribute" | "justify" => "justify",
        _ => "left",
    }
}

/// A number for CSS: up to 2 decimals, no trailing zeros.
pub(super) fn num(v: f32) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A font name that can't break out of CSS, quoted, with a generic fallback.
fn font_css(name: &str) -> String {
    let clean: String = name.chars().filter(|c| c.is_alphanumeric() || " -_.".contains(*c)).take(64).collect();
    let lower = clean.to_ascii_lowercase();
    let generic = if ["consolas", "courier new", "courier", "cascadia code", "cascadia mono", "lucida console"].contains(&lower.as_str()) {
        "monospace"
    } else if lower.contains("times") || lower.contains("georgia") || lower.contains("cambria") || lower.contains("garamond") || lower.contains("serif") && !lower.contains("sans") {
        "serif"
    } else {
        "sans-serif"
    };
    // Single quotes: this goes inside a style="…" attribute.
    format!("'{clean}',{generic}")
}

fn on(node: &Node) -> bool {
    !matches!(node.attr("val"), Some("false" | "0" | "off" | "none"))
}

pub(super) fn hex(v: &str) -> Option<String> {
    (v.len() == 6 && v.chars().all(|c| c.is_ascii_hexdigit())).then(|| v.to_ascii_lowercase())
}

struct Theme {
    major: Option<String>,
    minor: Option<String>,
}

fn read_rpr(rpr: &Node, theme: &Theme) -> RProps {
    let mut p = RProps::default();
    for c in rpr.nodes() {
        match c.name.as_str() {
            "rFonts" => {
                let themed = |a: &str| match c.attr(a) {
                    Some(t) if t.starts_with("major") => theme.major.clone(),
                    Some(t) if t.starts_with("minor") => theme.minor.clone(),
                    _ => None,
                };
                p.font = themed("asciiTheme").or_else(|| c.attr("ascii").map(str::to_owned)).or_else(|| themed("hAnsiTheme")).or_else(|| c.attr("hAnsi").map(str::to_owned)).or(p.font);
            }
            "sz" => p.size = c.attr("val").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 2.0),
            "b" => p.bold = Some(on(c)),
            "i" => p.italic = Some(on(c)),
            "u" => p.underline = Some(on(c)),
            "strike" | "dstrike" => p.strike = Some(on(c)),
            "caps" => p.caps = Some(on(c)),
            "smallCaps" => p.small_caps = Some(on(c)),
            "color" => p.color = c.attr("val").and_then(|v| if v == "auto" { Some("auto".into()) } else { hex(v) }),
            "highlight" => p.bg = Some(HIGHLIGHTS.iter().find(|(n, _)| Some(*n) == c.attr("val")).map(|(_, h)| (*h).to_owned()).unwrap_or_default()),
            "shd" => {
                if p.bg.as_deref().is_none_or(str::is_empty) {
                    if let Some(fill) = c.attr("fill").and_then(hex) {
                        p.bg = Some(fill);
                    }
                }
            }
            "vertAlign" => p.vert = c.attr("val").map(str::to_owned),
            _ => {}
        }
    }
    p
}

fn read_ppr(ppr: &Node) -> PProps {
    let mut p = PProps::default();
    let twips = |n: &Node, a: &str| n.attr(a).and_then(|v| v.parse::<f32>().ok()).map(|v| v as i32);
    for c in ppr.nodes() {
        match c.name.as_str() {
            "jc" => p.align = c.attr("val").map(str::to_owned),
            "spacing" => {
                p.before = twips(c, "before").or(p.before);
                p.after = twips(c, "after").or(p.after);
                if let Some(line) = twips(c, "line") {
                    p.line = Some((line, c.attr("lineRule").unwrap_or("auto").to_owned()));
                }
            }
            "ind" => {
                p.left = twips(c, "left").or_else(|| twips(c, "start")).or(p.left);
                p.right = twips(c, "right").or_else(|| twips(c, "end")).or(p.right);
                if let Some(h) = twips(c, "hanging") {
                    p.first = Some(-h);
                } else if let Some(f) = twips(c, "firstLine") {
                    p.first = Some(f);
                }
            }
            "contextualSpacing" => p.contextual = Some(on(c)),
            "numPr" => {
                if let Some(id) = c.child("numId").and_then(|n| n.attr("val")) {
                    let lvl = c.child("ilvl").and_then(|l| l.attr("val")?.parse().ok()).unwrap_or(0);
                    p.num = Some((id.to_owned(), lvl));
                }
            }
            _ => {}
        }
    }
    p
}

// ---------------------------------------------------------------- styles and numbering

#[derive(Default)]
struct StyleDef {
    kind: String,
    name: String,
    based_on: Option<String>,
    next: Option<String>,
    ppr: PProps,
    rpr: RProps,
    quick: bool,
    hidden: bool,
}

struct Styles {
    defs: HashMap<String, StyleDef>,
    /// Order in styles.xml, for the menu.
    order: Vec<String>,
    doc_r: RProps,
    doc_p: PProps,
    default_para: Option<String>,
}

impl Styles {
    fn read(styles: Option<&Node>, theme: &Theme) -> Self {
        let mut s = Styles { defs: HashMap::new(), order: vec![], doc_r: RProps::default(), doc_p: PProps::default(), default_para: None };
        let Some(root) = styles else { return s };
        if let Some(d) = root.child("docDefaults") {
            if let Some(r) = d.child("rPrDefault").and_then(|r| r.child("rPr")) {
                s.doc_r = read_rpr(r, theme);
            }
            if let Some(p) = d.child("pPrDefault").and_then(|p| p.child("pPr")) {
                s.doc_p = read_ppr(p);
            }
        }
        for st in root.nodes().filter(|n| n.name == "style") {
            let Some(id) = st.attr("styleId") else { continue };
            let def = StyleDef {
                kind: st.attr("type").unwrap_or("paragraph").to_owned(),
                name: st.child("name").and_then(|n| n.attr("val")).unwrap_or(id).to_owned(),
                based_on: st.child("basedOn").and_then(|b| b.attr("val")).map(str::to_owned),
                next: st.child("next").and_then(|b| b.attr("val")).map(str::to_owned),
                ppr: st.child("pPr").map(read_ppr).unwrap_or_default(),
                rpr: st.child("rPr").map(|r| read_rpr(r, theme)).unwrap_or_default(),
                quick: st.child("qFormat").is_some(),
                hidden: st.child("semiHidden").is_some() || st.child("hidden").is_some(),
            };
            if def.kind == "paragraph" && matches!(st.attr("default"), Some("1" | "true" | "on")) {
                s.default_para = Some(id.to_owned());
            }
            s.order.push(id.to_owned());
            s.defs.insert(id.to_owned(), def);
        }
        s
    }

    /// The style's properties with its `basedOn` chain under it.
    fn resolve(&self, id: &str) -> (PProps, RProps) {
        let mut chain = vec![];
        let mut at = Some(id.to_owned());
        while let Some(cur) = at.take() {
            if chain.len() > 32 || chain.contains(&cur) {
                break;
            }
            if let Some(def) = self.defs.get(&cur) {
                at = def.based_on.clone();
                chain.push(cur);
            }
        }
        let (mut p, mut r) = (PProps::default(), RProps::default());
        for id in chain.iter().rev() {
            let def = &self.defs[id];
            p.over(&def.ppr);
            r.over(&def.rpr);
        }
        (p, r)
    }
}

/// A numbering level as the editor draws it.
#[derive(Debug, Clone, Serialize)]
pub struct Level {
    /// "decimal", "lowerLetter", "upperLetter", "lowerRoman", "upperRoman", "bullet", "none"…
    pub fmt: String,
    /// `%1.` style template, or the bullet itself.
    pub text: String,
    pub start: u32,
    pub left: i32,
    pub hanging: i32,
}

fn read_numbering(root: Option<&Node>) -> HashMap<String, Vec<Level>> {
    let Some(root) = root else { return HashMap::new() };
    let abstracts: HashMap<&str, Vec<Level>> = root
        .nodes()
        .filter(|n| n.name == "abstractNum")
        .filter_map(|a| {
            let mut levels: Vec<Level> = a.nodes().filter(|l| l.name == "lvl").take(9).map(read_level).collect();
            while levels.len() < 9 {
                levels.push(default_level(levels.len() as u32, true));
            }
            Some((a.attr("abstractNumId")?, levels))
        })
        .collect();
    root.nodes()
        .filter(|n| n.name == "num")
        .filter_map(|n| {
            let mut levels = abstracts.get(n.child("abstractNumId")?.attr("val")?)?.clone();
            for o in n.nodes().filter(|o| o.name == "lvlOverride") {
                let i: usize = o.attr("ilvl")?.parse().ok()?;
                if let (Some(level), Some(start)) = (levels.get_mut(i), o.child("startOverride").and_then(|s| s.attr("val")?.parse().ok())) {
                    level.start = start;
                }
            }
            Some((n.attr("numId")?.to_owned(), levels))
        })
        .collect()
}

fn read_level(l: &Node) -> Level {
    let fmt = l.child("numFmt").and_then(|f| f.attr("val")).unwrap_or("decimal").to_owned();
    let raw = l.child("lvlText").and_then(|t| t.attr("val")).unwrap_or("").to_owned();
    let font = l.child("rPr").and_then(|r| r.child("rFonts")).and_then(|f| f.attr("ascii").or(f.attr("hAnsi"))).unwrap_or("").to_ascii_lowercase();
    let text = if fmt == "bullet" {
        // Symbol and Wingdings bullets are private-use characters: shown as the closest Unicode one.
        match raw.chars().next() {
            Some('o') if font.contains("courier") => "◦".into(),
            Some('§' | '\u{F0A7}') => "▪".into(),
            Some('\u{F0D8}' | 'Ø') => "➢".into(),
            Some('\u{F0FC}' | 'ü') => "✓".into(),
            Some(c) if ('\u{F000}'..='\u{F0FF}').contains(&c) || font.contains("symbol") || font.contains("wingdings") => "•".into(),
            Some(_) => raw.chars().take(3).collect(),
            None => String::new(),
        }
    } else {
        raw.chars().take(32).collect()
    };
    let ind = l.child("pPr").and_then(|p| p.child("ind"));
    let tw = |a: &str| ind.and_then(|i| i.attr(a)?.parse::<f32>().ok()).map(|v| v as i32);
    Level {
        fmt,
        text,
        start: l.child("start").and_then(|s| s.attr("val")?.parse().ok()).unwrap_or(1),
        left: tw("left").or_else(|| tw("start")).unwrap_or(0),
        hanging: tw("hanging").unwrap_or(0),
    }
}

pub(super) fn default_level(i: u32, bullet: bool) -> Level {
    let (fmt, text) = if bullet {
        ("bullet", ["•", "◦", "▪"][i as usize % 3].to_owned())
    } else {
        (["decimal", "lowerLetter", "lowerRoman"][i as usize % 3], format!("%{}.", i + 1))
    };
    Level { fmt: fmt.into(), text, start: 1, left: 720 * (i as i32 + 1), hanging: 360 }
}

fn new_abstract(id: u32, bullet: bool) -> String {
    let mut out = format!("<w:abstractNum w:abstractNumId=\"{id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
    for i in 0..9 {
        let l = default_level(i, bullet);
        let _ = write!(
            out,
            "<w:lvl w:ilvl=\"{i}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{}\"/><w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
            l.fmt, l.text, l.left
        );
    }
    out.push_str("</w:abstractNum>");
    out
}

// ---------------------------------------------------------------- the session

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Para,
    Table,
    Row,
    Cell,
    Block,
    /// A header or footer (`w:hdr`, `w:ftr`): a container like a cell.
    Root,
}

struct Unit {
    kind: Kind,
    /// Which XML part it's in (`Session::parts`; 0 is document.xml).
    part: usize,
    span: Range<usize>,
    parent: Option<usize>,
    /// Paragraphs: the start tag, `w:pPr`, and their direct values as sent to the page.
    start_tag: Range<usize>,
    ppr: Option<Range<usize>>,
    direct: Direct,
    /// Bookmarks, comment ranges…: kept when the paragraph is rewritten.
    marks: Vec<Range<usize>>,
    /// Field characters and instructions (atom keys), and whether they open and close here.
    fields: Vec<usize>,
    fields_balanced: bool,
    has_sect: bool,
    /// Cells and the body: where a first paragraph goes.
    content_start: usize,
    /// Tables: their `w:gridCol`s.
    grid: Vec<Range<usize>>,
}

/// What a paragraph says itself, as strings ("" = nothing), compared on save.
#[derive(Debug, Clone, Default, PartialEq)]
struct Direct {
    style: String,
    num: String,
    align: String,
    ind: String,
    line: String,
}

/// The paragraph style choices for the toolbar.
#[derive(Debug, Clone, Serialize)]
pub struct StyleChoice {
    pub id: String,
    pub name: String,
    /// In Word's quick style gallery (the menu lists these; others show when the cursor is in one).
    pub quick: bool,
    /// The style's run properties with the document defaults (what text looks like in it).
    pub run: RProps,
    /// The style's own list, if it has one ("List Bullet").
    pub num: Option<String>,
    /// The style of the paragraph Enter makes after this one (Word's "Style for following
    /// paragraph": a heading is followed by normal text).
    pub next: Option<String>,
}

/// What the page needs to edit: the HTML of the body, the CSS of the styles and page, and the
/// menus' choices.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub html: String,
    pub css: String,
    pub styles: Vec<StyleChoice>,
    /// Character style id → its run properties.
    pub char_styles: HashMap<String, RProps>,
    pub default_style: String,
    pub fonts: Vec<String>,
    pub numbering: HashMap<String, Vec<Level>>,
    /// An existing bullet and numbered list to reuse from the toolbar, if the file has them.
    pub bullet_num: Option<String>,
    pub number_num: Option<String>,
    /// Page size and margins (points), for placing floating pictures.
    pub page: PageBox,
    /// The style new tables get ("Table Grid"), if the file has it.
    pub table_style: Option<String>,
    /// The default header and footer, edited like the body (`u` is their container).
    pub header: Option<Margin>,
    pub footer: Option<Margin>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Margin {
    pub u: usize,
    pub html: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct PageBox {
    pub w: f32,
    pub h: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    /// Distance from the page's edge to the header and to the footer.
    pub header: f32,
    pub footer: f32,
}

pub struct Session {
    file: Vec<u8>,
    /// document.xml, then the default header and footer, one after the other; every span points in
    /// here, and `parts` says where each part is.
    xml: String,
    parts: Vec<(String, Range<usize>)>,
    units: Vec<Unit>,
    /// Raw XML of things shown but not edited (images, fields, footnote marks…), by `data-a`.
    atoms: Vec<String>,
    /// Raw `w:rPr` children of each original run, by `data-r`.
    runs: Vec<Vec<(String, String)>>,
    /// Original hyperlinks: start tag and target, by `data-h`.
    links: Vec<(String, String)>,
    rels: HashMap<String, String>,
    /// Whether document.xml declares the `r:` prefix new hyperlinks use.
    has_r_ns: bool,
    /// The body's last section (where a new header's reference goes), and where the body ends.
    body_sect: Option<Range<usize>>,
    body_end: usize,
    pub images: Vec<Image>,
    pub view: View,
}

struct Ctx<'a> {
    xml: &'a str,
    part: usize,
    styles: &'a Styles,
    theme: &'a Theme,
    numbering: &'a HashMap<String, Vec<Level>>,
    rels: &'a HashMap<String, String>,
    zip: zip::ZipArchive<Cursor<Vec<u8>>>,
    image_index: HashMap<String, usize>,
    img_url: &'a dyn Fn(usize) -> String,
    fonts: HashSet<String>,
    out: Session,
}

pub(super) fn cls(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

pub(super) fn esc(s: &str) -> String {
    super::html::escape(s)
}

pub(super) fn start_tag_end(xml: &str, at: usize) -> usize {
    xml[at..].find('>').map(|i| at + i + 1).unwrap_or(at)
}

impl Session {
    /// Opens a DOCX for editing. `img_url(n)` is where the page loads image `n` from.
    pub fn open(bytes: &[u8], img_url: &dyn Fn(usize) -> String) -> Result<Session, DocError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|e| invalid(e.to_string()))?;
        let doc_bytes = zip_entry(&mut zip, "word/document.xml")?.ok_or_else(|| invalid("not a Word document (no word/document.xml)"))?;
        let xml = String::from_utf8(doc_bytes).map_err(|_| invalid("document.xml is not UTF-8"))?;
        let root = xml::parse(xml.as_bytes())?;
        // New elements are written with the usual prefixes: the file has to use them.
        let tag = &xml[root.span.start..start_tag_end(&xml, root.span.start)];
        if !tag.contains(&format!("xmlns:w=\"{W_NS}\"")) || !tag.starts_with("<w:document") {
            return Err(invalid("this Word file uses unusual XML prefixes; Jay-E can show it but not edit it"));
        }
        let rels: HashMap<String, String> = match zip_entry(&mut zip, "word/_rels/document.xml.rels")? {
            Some(x) => xml::parse(&x)?.nodes().filter_map(|r| Some((r.attr("Id")?.to_owned(), r.attr("Target")?.to_owned()))).collect(),
            None => HashMap::new(),
        };
        let theme = match zip_entry(&mut zip, "word/theme/theme1.xml")? {
            Some(x) => {
                let t = xml::parse(&x)?;
                let face = |n: &str| t.find(n).and_then(|f| f.child("latin")).and_then(|l| l.attr("typeface")).filter(|f| !f.is_empty()).map(str::to_owned);
                Theme { major: face("majorFont"), minor: face("minorFont") }
            }
            None => Theme { major: None, minor: None },
        };
        let styles_root = zip_entry(&mut zip, "word/styles.xml")?.map(|x| xml::parse(&x)).transpose()?;
        let styles = Styles::read(styles_root.as_ref(), &theme);
        let numbering_root = zip_entry(&mut zip, "word/numbering.xml")?.map(|x| xml::parse(&x)).transpose()?;
        let numbering = read_numbering(numbering_root.as_ref());
        let body = root.child("body").ok_or_else(|| invalid("no w:body"))?;

        // The default header and footer of the last section, appended after document.xml.
        let mut all = xml.clone();
        let mut parts = vec![("word/document.xml".to_owned(), 0..xml.len())];
        let mut margins: Vec<(&str, Node, HashMap<String, String>)> = vec![];
        for (kind, reference) in [("header", "headerReference"), ("footer", "footerReference")] {
            let Some(target) = body.child("sectPr").and_then(|s| s.nodes().find(|n| n.name == reference && n.attr("type").is_none_or(|t| t == "default"))).and_then(|r| rels.get(r.attr("id")?)) else { continue };
            let name = format!("word/{}", target.trim_start_matches("/word/").trim_start_matches('/'));
            let Some(bytes) = zip_entry(&mut zip, &name)? else { continue };
            let Ok(text) = String::from_utf8(bytes) else { continue };
            let Ok(mut node) = xml::parse(text.as_bytes()) else { continue };
            if !text[node.span.start..].starts_with(&format!("<w:{}", if kind == "header" { "hdr" } else { "ftr" })) {
                continue;
            }
            let offset = all.len();
            shift(&mut node, offset);
            all.push_str(&text);
            parts.push((name.clone(), offset..all.len()));
            let rels_name = match name.rsplit_once('/') {
                Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
                None => format!("_rels/{name}.rels"),
            };
            let part_rels = match zip_entry(&mut zip, &rels_name)? {
                Some(x) => xml::parse(&x)?.nodes().filter_map(|r| Some((r.attr("Id")?.to_owned(), r.attr("Target")?.to_owned()))).collect(),
                None => HashMap::new(),
            };
            margins.push((kind, node, part_rels));
        }

        let view = View {
            html: String::new(),
            css: String::new(),
            styles: vec![],
            char_styles: HashMap::new(),
            default_style: styles.default_para.clone().unwrap_or_default(),
            fonts: vec![],
            numbering: numbering.clone(),
            bullet_num: None,
            number_num: None,
            page: PageBox::default(),
            table_style: None,
            header: None,
            footer: None,
        };
        let has_r_ns = tag.contains(&format!("xmlns:r=\"{R_NS}\""));
        let body_sect = body.child("sectPr").map(|s| s.span.clone());
        let body_end = body.span.end - "</w:body>".len();
        let out = Session { file: bytes.to_vec(), xml: String::new(), parts, units: vec![], atoms: vec![], runs: vec![], links: vec![], rels: rels.clone(), has_r_ns, body_sect, body_end, images: vec![], view };
        let mut ctx = Ctx { xml: &all, part: 0, styles: &styles, theme: &theme, numbering: &numbering, rels: &rels, zip, image_index: HashMap::new(), img_url, fonts: HashSet::new(), out };

        let mut html = String::new();
        ctx.container(body, None, &mut html, 0);
        let body_start = start_tag_end(&xml, body.span.start);
        for (i, (kind, node, part_rels)) in margins.iter().enumerate() {
            // Its pictures and links are in its own relationships.
            ctx.rels = part_rels;
            ctx.part = i + 1;
            let u = ctx.unit(Kind::Root, node, None);
            let mut part_html = String::new();
            ctx.container(node, Some(u), &mut part_html, 0);
            let margin = Some(Margin { u, html: part_html });
            if *kind == "header" {
                ctx.out.view.header = margin;
            } else {
                ctx.out.view.footer = margin;
            }
        }
        let mut session = ctx.out;
        let mut fonts = ctx.fonts;
        session.units.push(Unit {
            kind: Kind::Block,
            part: 0,
            span: 0..0,
            parent: None,
            start_tag: 0..0,
            ppr: None,
            direct: Direct::default(),
            marks: vec![],
            fields: vec![],
            fields_balanced: true,
            has_sect: false,
            content_start: body_start,
            grid: vec![],
        });

        // The page: size and margins from the last section.
        let sect = body.child("sectPr");
        let tw = |n: Option<&Node>, a: &str, d: f32| n.and_then(|n| n.attr(a)?.parse::<f32>().ok()).unwrap_or(d) / 20.0;
        let (pg, mar) = (sect.and_then(|s| s.child("pgSz")), sect.and_then(|s| s.child("pgMar")));
        let page = PageBox {
            w: tw(pg, "w", 11906.0),
            h: tw(pg, "h", 16838.0),
            top: tw(mar, "top", 1440.0),
            right: tw(mar, "right", 1440.0),
            bottom: tw(mar, "bottom", 1440.0),
            left: tw(mar, "left", 1440.0),
            header: tw(mar, "header", 720.0),
            footer: tw(mar, "footer", 720.0),
        };
        session.view.page = page;
        let mut css = format!(
            ".dx-page{{width:{}pt;min-height:{}pt;padding:{}pt {}pt {}pt {}pt;",
            num(page.w - page.left - page.right),
            num(page.h - page.top - page.bottom),
            num(page.top),
            num(page.right),
            num(page.bottom),
            num(page.left)
        );
        let mut base_r = RProps { font: Some("Times New Roman".into()), size: Some(10.0), color: Some("auto".into()), ..Default::default() };
        base_r.over(&styles.doc_r);
        base_r.css(&mut css);
        css.push_str("}\n");
        for id in &styles.order {
            let def = &styles.defs[id];
            let (mut p, r) = styles.resolve(id);
            let mut rule = String::new();
            match def.kind.as_str() {
                "paragraph" => {
                    let mut pp = styles.doc_p.clone();
                    pp.over(&p);
                    p = pp;
                    p.num = None;
                    p.css(&mut rule);
                    r.css(&mut rule);
                    let _ = writeln!(css, ".dx-page .dx-p-{}{{{rule}}}", cls(id));
                    if p.contextual == Some(true) {
                        let c = cls(id);
                        let _ = writeln!(css, ".dx-page .dx-p-{c}+.dx-p-{c}{{padding-top:0}}.dx-page .dx-p-{c}:has(+.dx-p-{c}){{padding-bottom:0}}");
                    }
                    let mut run = base_r.clone();
                    run.over(&r);
                    if let Some(f) = &run.font {
                        fonts.insert(f.clone());
                    }
                    let num = styles.resolve(id).0.num.map(|(n, _)| n).filter(|n| n != "0");
                    let shown = (def.quick && !def.hidden) || Some(id) == styles.default_para.as_ref();
                    let next = def.next.clone().filter(|n| n != id && styles.defs.contains_key(n));
                    session.view.styles.push(StyleChoice { id: id.clone(), name: def.name.clone(), quick: shown, run, num, next });
                }
                // A table style's paragraph settings win over normal text's inside its cells (as
                // in Word: "Table Grid" has no space after).
                "table" => {
                    p.css(&mut rule);
                    r.css(&mut rule);
                    if !rule.is_empty() {
                        let normal = styles.default_para.as_deref().map(cls).unwrap_or_else(|| "_default".into());
                        let _ = writeln!(css, ".dx-page .dx-t-{} td>p.dx-p-{normal}{{{rule}}}", cls(id));
                    }
                }
                "character" => {
                    r.css(&mut rule);
                    let _ = writeln!(css, ".dx-page .dx-c-{}{{{rule}}}", cls(id));
                    session.view.char_styles.insert(id.clone(), r);
                }
                _ => {}
            }
        }
        // Paragraphs with no style of their own get the default one's look (the base already has
        // the document defaults).
        if styles.default_para.is_none() {
            let mut rule = String::new();
            styles.doc_p.css(&mut rule);
            let _ = writeln!(css, ".dx-page .dx-p-{}{{{rule}}}", "_default");
            session.view.styles.push(StyleChoice { id: String::new(), name: "Normal".into(), quick: true, run: base_r.clone(), num: None, next: None });
        }
        if let Some(f) = &base_r.font {
            fonts.insert(f.clone());
        }
        let mut fonts: Vec<String> = fonts.into_iter().collect();
        fonts.sort();
        session.view.fonts = fonts;
        session.view.table_style = styles.order.iter().find(|id| styles.defs[*id].kind == "table" && styles.defs[*id].name.eq_ignore_ascii_case("table grid")).cloned();
        session.view.html = html;
        session.view.css = css;
        session.view.bullet_num = numbering.iter().filter(|(_, l)| l[0].fmt == "bullet").map(|(id, _)| id.clone()).min_by_key(|id| id.parse::<u32>().unwrap_or(u32::MAX));
        session.view.number_num = numbering.iter().filter(|(_, l)| l[0].fmt == "decimal").map(|(id, _)| id.clone()).min_by_key(|id| id.parse::<u32>().unwrap_or(u32::MAX));
        session.xml = all;
        Ok(session)
    }

    /// The unit id of the body (the container of top-level paragraphs).
    fn body(&self) -> usize {
        self.units.len() - 1
    }
}

impl Ctx<'_> {
    fn unit(&mut self, kind: Kind, node: &Node, parent: Option<usize>) -> usize {
        let start = start_tag_end(self.xml, node.span.start);
        self.out.units.push(Unit {
            kind,
            part: self.part,
            span: node.span.clone(),
            parent,
            start_tag: node.span.start..start,
            ppr: None,
            direct: Direct::default(),
            marks: vec![],
            fields: vec![],
            fields_balanced: true,
            has_sect: false,
            content_start: node.child("tcPr").map(|p| p.span.end).unwrap_or(start),
            grid: node.child("tblGrid").map(|g| g.nodes().filter(|c| c.name == "gridCol").map(|c| c.span.clone()).collect()).unwrap_or_default(),
        });
        self.out.units.len() - 1
    }

    /// The paragraphs, tables and other blocks of the body or a cell.
    fn container(&mut self, node: &Node, parent: Option<usize>, html: &mut String, depth: usize) {
        if depth > super::MAX_DEPTH {
            return;
        }
        for child in node.nodes() {
            match child.name.as_str() {
                "p" => self.paragraph(child, parent, html),
                "tbl" => self.table(child, parent, html, depth),
                "tcPr" | "sectPr" | "bookmarkStart" | "bookmarkEnd" | "proofErr" | "permStart" | "permEnd" | "commentRangeStart" | "commentRangeEnd" => {}
                _ => {
                    // Content controls, custom XML…: shown, kept, not edited.
                    let u = self.unit(Kind::Block, child, parent);
                    let _ = write!(html, "<div class=\"dx-block\" contenteditable=\"false\" data-u=\"{u}\">{}</div>", esc(&child.text()));
                }
            }
        }
    }

    fn table(&mut self, tbl: &Node, parent: Option<usize>, html: &mut String, depth: usize) {
        let t = self.unit(Kind::Table, tbl, parent);
        let tpr = tbl.child("tblPr");
        let style = tpr.and_then(|p| p.child("tblStyle")).and_then(|s| s.attr("val")).unwrap_or("");
        let bordered = tpr.and_then(|p| p.child("tblBorders")).is_some_and(|b| b.nodes().any(|e| e.attr("val").is_some_and(|v| v != "none" && v != "nil"))) || style.to_ascii_lowercase().contains("grid");
        let _ = write!(html, "<table class=\"dx-table dx-t-{}{}\" data-u=\"{t}\"><tbody>", cls(style), if bordered { " is-bordered" } else { "" });
        for tr in tbl.nodes().filter(|n| n.name == "tr") {
            let r = self.unit(Kind::Row, tr, Some(t));
            let _ = write!(html, "<tr data-u=\"{r}\">");
            for tc in tr.nodes().filter(|n| n.name == "tc") {
                let c = self.unit(Kind::Cell, tc, Some(r));
                let pr = tc.child("tcPr");
                let span = pr.and_then(|p| p.child("gridSpan")).and_then(|g| g.attr("val")?.parse::<u32>().ok()).unwrap_or(1);
                let width = pr.and_then(|p| p.child("tcW")).filter(|w| w.attr("type") == Some("dxa")).and_then(|w| w.attr("w")?.parse::<f32>().ok());
                let shade = pr.and_then(|p| p.child("shd")).and_then(|s| s.attr("fill")).and_then(hex);
                let mut style = String::new();
                if let Some(w) = width {
                    let _ = write!(style, "width:{}pt;", num(w / 20.0));
                }
                if let Some(s) = shade {
                    let _ = write!(style, "background-color:#{s};");
                }
                let _ = write!(html, "<td data-u=\"{c}\"{}{}>", if span > 1 { format!(" colspan=\"{span}\"") } else { String::new() }, if style.is_empty() { String::new() } else { format!(" style=\"{style}\"") });
                self.container(tc, Some(c), html, depth + 1);
                html.push_str("</td>");
            }
            html.push_str("</tr>");
        }
        html.push_str("</tbody></table>");
    }

    fn paragraph(&mut self, p: &Node, parent: Option<usize>, html: &mut String) {
        let u = self.unit(Kind::Para, p, parent);
        let ppr = p.child("pPr");
        let direct_p = ppr.map(read_ppr).unwrap_or_default();
        let style = ppr.and_then(|p| p.child("pStyle")).and_then(|s| s.attr("val")).unwrap_or("").to_owned();
        let style_id = if style.is_empty() { self.styles.default_para.clone().unwrap_or_default() } else { style.clone() };
        let style_num = self.styles.resolve(&style_id).0.num;
        let num = direct_p.num.clone().or(style_num).filter(|(id, _)| id != "0" && self.numbering.contains_key(id));
        let spacing = ppr.and_then(|p| p.child("spacing"));
        let direct = Direct {
            style: style.clone(),
            num: num.as_ref().map(|(id, l)| format!("{id}:{l}")).unwrap_or_default(),
            align: direct_p.align.clone().unwrap_or_default(),
            ind: ppr.and_then(|p| p.child("ind")).and_then(|i| i.attr("left").or(i.attr("start"))).unwrap_or("").to_owned(),
            line: spacing.filter(|s| s.attr("lineRule").is_none_or(|r| r == "auto")).and_then(|s| s.attr("line")).unwrap_or("").to_owned(),
        };
        {
            let unit = &mut self.out.units[u];
            unit.ppr = ppr.map(|p| p.span.clone());
            unit.has_sect = ppr.and_then(|p| p.child("sectPr")).is_some();
            unit.direct = direct.clone();
        }

        // Inline CSS: the paragraph's own properties, and a list's indent.
        let mut own = direct_p.clone();
        own.num = None;
        if let Some((id, lvl)) = &num {
            if let Some(level) = self.numbering.get(id).and_then(|l| l.get(*lvl as usize)) {
                own.left = own.left.or(Some(level.left));
                own.first = own.first.or(Some(-level.hanging));
            }
        }
        let mut css = String::new();
        own.css(&mut css);
        let class = if style_id.is_empty() { "_default".to_owned() } else { cls(&style_id) };
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
        let mut field_depth = 0i32;
        let len = html.len();
        self.inlines(p, u, html, &mut field_depth, 0);
        self.out.units[u].fields_balanced = field_depth == 0;
        // An empty paragraph still needs a line to put the cursor on.
        if !visible(&html[len..]) {
            html.push_str("<br>");
        }
        html.push_str("</p>");
    }

    fn inlines(&mut self, parent: &Node, u: usize, html: &mut String, field_depth: &mut i32, depth: usize) {
        if depth > 8 {
            return;
        }
        for c in parent.nodes() {
            match c.name.as_str() {
                "pPr" => {}
                "r" => self.run(c, u, html, field_depth),
                "hyperlink" => {
                    let target = c.attr("id").and_then(|id| self.rels.get(id)).cloned().or_else(|| c.attr("anchor").map(|a| format!("#{a}"))).unwrap_or_default();
                    let tag = self.xml[c.span.start..start_tag_end(self.xml, c.span.start)].to_owned();
                    let tag = if tag.ends_with("/>") { format!("{}>", tag.trim_end_matches("/>")) } else { tag };
                    self.out.links.push((tag, target.clone()));
                    let _ = write!(html, "<a href=\"{}\" data-h=\"{}\">", esc(&target), self.out.links.len() - 1);
                    self.inlines(c, u, html, field_depth, depth + 1);
                    html.push_str("</a>");
                }
                // Wrappers whose runs are ordinary text (an insertion is accepted when edited).
                "ins" | "moveTo" | "smartTag" | "customXml" => self.inlines(c, u, html, field_depth, depth + 1),
                "proofErr" => {}
                "bookmarkStart" | "bookmarkEnd" | "commentRangeStart" | "commentRangeEnd" | "permStart" | "permEnd" | "moveFromRangeStart" | "moveFromRangeEnd" | "moveToRangeStart" | "moveToRangeEnd" => {
                    self.out.units[u].marks.push(c.span.clone());
                }
                // Shown as they are and kept: deletions, simple fields, content controls, math.
                "del" | "moveFrom" => {
                    self.atom(html, c.span.clone(), &format!("<del>{}</del>", esc(&deleted_text(c))), false);
                }
                _ => {
                    self.atom(html, c.span.clone(), &esc(&c.text()), false);
                }
            }
        }
    }

    fn atom(&mut self, html: &mut String, span: Range<usize>, inner: &str, hidden: bool) -> usize {
        self.out.atoms.push(self.xml[span].to_owned());
        let a = self.out.atoms.len() - 1;
        let _ = write!(html, "<span class=\"dx-atom\" contenteditable=\"false\" data-a=\"{a}\"{}>{inner}</span>", if hidden { " hidden" } else { "" });
        a
    }

    fn run(&mut self, r: &Node, u: usize, html: &mut String, field_depth: &mut i32) {
        let rpr = r.child("rPr");
        let raw = rpr.map(|p| p.nodes().map(|c| (c.name.clone(), self.xml[c.span.clone()].to_owned())).collect()).unwrap_or_default();
        self.out.runs.push(raw);
        let k = self.out.runs.len() - 1;
        let direct = rpr.map(|p| read_rpr(p, self.theme)).unwrap_or_default();
        if let Some(f) = &direct.font {
            self.fonts.insert(f.clone());
        }
        let char_style = rpr.and_then(|p| p.child("rStyle")).and_then(|s| s.attr("val")).filter(|s| self.styles.defs.contains_key(*s));
        let mut css = String::new();
        direct.css(&mut css);
        let open = format!(
            "<span data-r=\"{k}\"{}{}>",
            char_style.map(|s| format!(" class=\"dx-c-{}\" data-cs=\"{}\"", cls(s), esc(s))).unwrap_or_default(),
            if css.is_empty() { String::new() } else { format!(" style=\"{css}\"") }
        );
        let rpr_raw = rpr.map(|p| self.xml[p.span.clone()].to_owned()).unwrap_or_default();
        let mut text = String::new();
        let flush = |html: &mut String, text: &mut String| {
            if !text.is_empty() {
                let _ = write!(html, "{open}{}</span>", esc(text));
                text.clear();
            }
        };
        for c in r.nodes() {
            match c.name.as_str() {
                "rPr" | "lastRenderedPageBreak" => {}
                "t" => text.push_str(&c.text()),
                "tab" => text.push('\t'),
                "noBreakHyphen" => text.push('\u{2011}'),
                "softHyphen" => text.push('\u{AD}'),
                "br" | "cr" if c.attr("type").is_none_or(|t| t == "textWrapping") => {
                    flush(html, &mut text);
                    html.push_str("<br>");
                }
                _ => {
                    flush(html, &mut text);
                    let whole = format!("<w:r>{rpr_raw}{}</w:r>", &self.xml[c.span.clone()]);
                    let mut extra = String::new();
                    let (inner, hidden, field) = match c.name.as_str() {
                        "fldChar" => {
                            match c.attr("fldCharType") {
                                Some("begin") => *field_depth += 1,
                                Some("end") => *field_depth -= 1,
                                _ => {}
                            }
                            (String::new(), true, true)
                        }
                        "instrText" | "delInstrText" | "fldData" => (String::new(), true, true),
                        "br" => ("<span class=\"dx-pagebreak\"></span>".to_owned(), false, false),
                        "drawing" | "pict" | "object" | "AlternateContent" => {
                            extra = drawing_attrs(c);
                            (self.picture(c), false, false)
                        }
                        "footnoteReference" | "endnoteReference" => (format!("<sup>{}</sup>", esc(c.attr("id").unwrap_or("*"))), false, false),
                        "commentReference" | "annotationRef" => (String::new(), true, false),
                        "sym" => (esc(&c.attr("char").and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32).map(|ch| if ('\u{F000}'..='\u{F0FF}').contains(&ch) { '•' } else { ch }).unwrap_or('•').to_string()), false, false),
                        _ => (esc(&c.text()), c.text().is_empty(), false),
                    };
                    self.out.atoms.push(whole);
                    let a = self.out.atoms.len() - 1;
                    let _ = write!(html, "<span class=\"dx-atom\" contenteditable=\"false\" data-a=\"{a}\"{extra}{}>{inner}</span>", if hidden { " hidden" } else { "" });
                    if field {
                        self.out.units[u].fields.push(a);
                    }
                }
            }
        }
        flush(html, &mut text);
    }

    /// The `<img>` for a drawing, or a box where Jay-E can't show it (a shape, a chart).
    fn picture(&mut self, node: &Node) -> String {
        let extent = node.find("extent");
        let pt = |a: &str| extent.and_then(|e| e.attr(a)?.parse::<f32>().ok()).map(|emu| emu / 12_700.0);
        let size = match (pt("cx"), pt("cy")) {
            (Some(w), Some(h)) => format!("width:{}pt;height:{}pt;", num(w), num(h)),
            _ => String::new(),
        };
        let id = node.find("blip").and_then(|b| b.attr("embed")).or_else(|| node.find("imagedata").and_then(|i| i.attr("id")));
        let index = id.and_then(|id| self.rels.get(id)).cloned().and_then(|target| {
            let path = format!("word/{}", target.trim_start_matches("/word/").trim_start_matches('/'));
            if let Some(&i) = self.image_index.get(&path) {
                return Some(i);
            }
            let bytes = zip_entry(&mut self.zip, &path).ok()??;
            let mime = image_mime(&bytes)?.to_owned();
            self.out.images.push(Image { mime, bytes });
            self.image_index.insert(path, self.out.images.len() - 1);
            Some(self.out.images.len() - 1)
        });
        match index {
            Some(i) => format!("<img src=\"{}\" alt=\"\" style=\"{size}\">", esc(&(self.img_url)(i))),
            None => format!("<span class=\"dx-shape\" style=\"{size}\"></span>"),
        }
    }
}

/// Moves every span of a part parsed alone to where the part is in `Session::xml`.
pub(super) fn shift(node: &mut Node, by: usize) {
    node.span = node.span.start + by..node.span.end + by;
    for c in &mut node.children {
        if let xml::Child::Node(n) = c {
            shift(n, by);
        }
    }
}

/// A DrawingML picture's size (`data-w`/`data-h`, points) and, when it floats (`wp:anchor`), where
/// it is: `data-hr`/`data-vr` say from what ("page", "margin", "paragraph"…), `data-hx`/`data-vy`
/// how far (points) or `data-ha`/`data-va` how it's aligned. The page places it (ui/docx-editor.js).
fn drawing_attrs(node: &Node) -> String {
    if node.name != "drawing" {
        return String::new();
    }
    let Some(shape) = node.child("inline").or_else(|| node.child("anchor")) else { return String::new() };
    let extent = shape.child("extent");
    let pt = |a: &str| extent.and_then(|e| e.attr(a)?.parse::<f32>().ok()).map(|emu| emu / 12_700.0);
    let (Some(w), Some(h)) = (pt("cx"), pt("cy")) else { return String::new() };
    let mut out = format!(" data-img=\"1\" data-w=\"{}\" data-h=\"{}\"", num(w), num(h));
    if shape.name == "anchor" {
        let behind = matches!(shape.attr("behindDoc"), Some("1" | "true"));
        let _ = write!(out, " data-float=\"1\"{}", if behind { " data-behind=\"1\"" } else { "" });
        for (axis, name, off, align) in [("h", "positionH", "hx", "ha"), ("v", "positionV", "vy", "va")] {
            let Some(pos) = shape.child(name) else { continue };
            let _ = write!(out, " data-{axis}r=\"{}\"", esc(pos.attr("relativeFrom").unwrap_or("page")));
            if let Some(v) = pos.child("posOffset").and_then(|o| o.text().trim().parse::<f32>().ok()) {
                let _ = write!(out, " data-{off}=\"{}\"", num(v / 12_700.0));
            } else if let Some(a) = pos.child("align") {
                let _ = write!(out, " data-{align}=\"{}\"", esc(a.text().trim()));
            }
        }
    }
    out
}

/// A picture's run XML with a new size and, if it was moved, a new place: floating, from the
/// page's left edge (`x`) and the top of its paragraph (`y`), over the text. Points.
fn reshape(raw: &str, geo: &Geo) -> Option<String> {
    let run = xml::parse(raw.as_bytes()).ok()?;
    let drawing = run.child("drawing")?;
    let shape = drawing.child("inline").or_else(|| drawing.child("anchor"))?;
    let prefix = |n: &Node| raw[n.span.start + 1..].split([':', ' ', '>', '/']).next().map(str::to_owned).unwrap_or_default();
    let wp = prefix(shape);
    let emu = |pt: f32| ((pt.clamp(0.0, 22_000.0)) * 12_700.0).round() as i64;
    let (cx, cy) = (emu(geo.w.max(1.0)), emu(geo.h.max(1.0)));
    let mut edits: Vec<(Range<usize>, String)> = vec![];
    // The size, in the frame and in the picture's own transform.
    if let Some(ext) = shape.child("extent") {
        edits.push((ext.span.clone(), format!("<{wp}:extent cx=\"{cx}\" cy=\"{cy}\"/>")));
    }
    if let Some(xfrm) = shape.child("graphic").and_then(|g| g.find("xfrm")) {
        if let Some(ext) = xfrm.child("ext") {
            let a = prefix(ext);
            edits.push((ext.span.clone(), format!("<{a}:ext cx=\"{cx}\" cy=\"{cy}\"/>")));
        }
    }
    let pos_h = format!("<{wp}:positionH relativeFrom=\"page\"><{wp}:posOffset>{}</{wp}:posOffset></{wp}:positionH>", emu(geo.x));
    let pos_v = format!("<{wp}:positionV relativeFrom=\"paragraph\"><{wp}:posOffset>{}</{wp}:posOffset></{wp}:positionV>", (geo.y.clamp(-22_000.0, 22_000.0) * 12_700.0).round() as i64);
    if geo.moved && shape.name == "anchor" {
        for (name, new) in [("positionH", &pos_h), ("positionV", &pos_v)] {
            if let Some(n) = shape.child(name) {
                edits.push((n.span.clone(), new.clone()));
            }
        }
    } else if geo.moved {
        // Inline becomes floating: the same picture in a wp:anchor, over the text.
        let keep = |name: &str| shape.child(name).map(|n| raw[n.span.clone()].to_owned()).unwrap_or_default();
        let effect = shape.child("effectExtent").map(|n| raw[n.span.clone()].to_owned()).unwrap_or_else(|| format!("<{wp}:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>"));
        // Namespaces the inline element declared itself (python-docx puts a: and pic: there).
        let tag = &raw[shape.span.start..start_tag_end(raw, shape.span.start)];
        let ns: String = tag.split_whitespace().filter(|a| a.starts_with("xmlns")).map(|a| format!(" {}", a.trim_end_matches(['>', '/']))).collect();
        let anchor = format!(
            "<{wp}:anchor{ns} distT=\"0\" distB=\"0\" distL=\"114300\" distR=\"114300\" simplePos=\"0\" relativeHeight=\"251659264\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><{wp}:simplePos x=\"0\" y=\"0\"/>{pos_h}{pos_v}<{wp}:extent cx=\"{cx}\" cy=\"{cy}\"/>{effect}<{wp}:wrapNone/>{}{}{}</{wp}:anchor>",
            keep("docPr"),
            keep("cNvGraphicFramePr"),
            reshaped_graphic(raw, shape, cx, cy)?
        );
        let mut out = raw.to_owned();
        out.replace_range(shape.span.clone(), &anchor);
        return Some(out);
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = raw.to_owned();
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    Some(out)
}

/// The `a:graphic` of a picture with its transform's size changed.
fn reshaped_graphic(raw: &str, shape: &Node, cx: i64, cy: i64) -> Option<String> {
    let graphic = shape.child("graphic")?;
    let mut out = raw[graphic.span.clone()].to_owned();
    if let Some(ext) = graphic.find("xfrm").and_then(|x| x.child("ext")) {
        let a = raw[ext.span.start + 1..].split(':').next().unwrap_or("a");
        let start = ext.span.start - graphic.span.start;
        out.replace_range(start..start + ext.span.len(), &format!("<{a}:ext cx=\"{cx}\" cy=\"{cy}\"/>"));
    }
    Some(out)
}

/// Whether some HTML shows anything (text, an image, an atom that isn't hidden).
pub(super) fn visible(html: &str) -> bool {
    html.contains("<img") || html.contains("dx-shape") || html.contains("dx-pagebreak") || {
        let mut inside = false;
        html.chars().any(|c| match c {
            '<' => {
                inside = true;
                false
            }
            '>' => {
                inside = false;
                false
            }
            _ => !inside,
        })
    }
}

fn deleted_text(node: &Node) -> String {
    let mut out = String::new();
    fn walk(n: &Node, out: &mut String) {
        for c in n.nodes() {
            match c.name.as_str() {
                "delText" | "t" => out.push_str(&c.text()),
                _ => walk(c, out),
            }
        }
    }
    walk(node, &mut out);
    out
}

// ---------------------------------------------------------------- saving

/// A document open for editing, DOCX or ODT: the page it shows, its pictures, and saving.
pub trait Editable: Send + Sync {
    fn view(&self) -> &View;
    fn images(&self) -> &[Image];
    fn save(&self, edits: &Edits, images: &[NewImage]) -> Result<Vec<u8>, DocError>;
    /// "docx" or "odt" (for exporting what was saved).
    fn format(&self) -> &'static str;
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
        "docx"
    }
}

/// Opens a Word or OpenDocument text file for editing, by what's inside (ODT has a `mimetype`).
pub fn open_editor(bytes: &[u8], img_url: &dyn Fn(usize) -> String) -> Result<Box<dyn Editable>, DocError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| invalid(e.to_string()))?;
    let odt = zip_entry(&mut zip, "mimetype")?.is_some_and(|m| m.starts_with(b"application/vnd.oasis.opendocument.text"));
    Ok(if odt { Box::new(super::odt_edit::Session::open(bytes, img_url)?) } else { Box::new(Session::open(bytes, img_url)?) })
}

/// The page's changes: every unit still on it, and each paragraph that changed or is new.
#[derive(Debug, Default, Deserialize)]
pub struct Edits {
    pub present: Vec<usize>,
    pub paras: Vec<EditPara>,
    /// Tables made on the page.
    #[serde(default)]
    pub tables: Vec<EditTable>,
    /// Rows and cells added to the file's tables, and cells taken out with their column.
    #[serde(default)]
    pub rows: Vec<EditRow>,
    #[serde(default)]
    pub cells: Vec<EditCell>,
    #[serde(default)]
    pub removed_cells: Vec<RemovedCell>,
    /// A header or footer made where the file had none.
    #[serde(default)]
    pub new_header: Option<Vec<EditPara>>,
    #[serde(default)]
    pub new_footer: Option<Vec<EditPara>>,
}

/// A row added to a table of the file: after `after` (or before `template`), its cells shaped like
/// `template`'s.
#[derive(Debug, Deserialize)]
pub struct EditRow {
    #[serde(default)]
    pub order: usize,
    pub table: usize,
    pub after: Option<usize>,
    pub template: usize,
    pub cells: Vec<Vec<EditPara>>,
}

/// A cell added to a row of the file (a new column): after `after` (or first), shaped like
/// `template`, at grid column `column`.
#[derive(Debug, Deserialize)]
pub struct EditCell {
    #[serde(default)]
    pub order: usize,
    pub row: usize,
    pub after: Option<usize>,
    pub template: usize,
    pub column: usize,
    pub paras: Vec<EditPara>,
}

/// A cell deleted with its column (`column` in the grid).
#[derive(Debug, Deserialize)]
pub struct RemovedCell {
    pub cell: usize,
    pub column: usize,
}

/// A table made on the page: where it goes (as for a new paragraph), its column widths (points) and
/// its rows of cells of paragraphs.
#[derive(Debug, Deserialize)]
pub struct EditTable {
    #[serde(default)]
    pub order: usize,
    pub after: Option<usize>,
    pub container: Option<usize>,
    pub cols: Vec<f32>,
    pub rows: Vec<Vec<Vec<EditPara>>>,
}

/// A picture added on the page, ready to go into word/media.
#[derive(Debug, Clone)]
pub struct NewImage {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    /// Its size in points at 96 dpi.
    pub w: f32,
    pub h: f32,
}

/// A picture file as Word takes it: PNG, JPEG, GIF and BMP as they are, anything else Jay-E
/// reads (WebP, TIFF…) as PNG.
pub fn prepare_image(bytes: &[u8]) -> Result<NewImage, DocError> {
    let kind = image_mime(bytes).ok_or_else(|| invalid("not a picture Jay-E can read"))?;
    let (ext, bytes) = match kind {
        "image/png" => ("png", bytes.to_vec()),
        "image/jpeg" => ("jpeg", bytes.to_vec()),
        "image/gif" => ("gif", bytes.to_vec()),
        "image/bmp" => ("bmp", bytes.to_vec()),
        _ => {
            let decoded = image::load_from_memory(bytes).map_err(|e| invalid(e.to_string()))?;
            let mut png = Vec::new();
            decoded.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| invalid(e.to_string()))?;
            ("png", png)
        }
    };
    let (w, h) = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| invalid(e.to_string()))?.into_dimensions().map_err(|e| invalid(e.to_string()))?;
    Ok(NewImage { bytes, ext, w: w as f32 * 0.75, h: h as f32 * 0.75 })
}

#[derive(Debug, Deserialize)]
pub struct EditPara {
    /// Its place among the new things on the page (keeps two inserted at one spot in order).
    #[serde(default)]
    pub order: usize,
    /// The original paragraph, or `None` for a new one.
    pub u: Option<usize>,
    /// A new paragraph's properties come from this one (the one Enter was pressed in).
    pub base: Option<usize>,
    /// A new paragraph goes after this unit (same container), or first in `container`.
    pub after: Option<usize>,
    /// The cell it's in; `None` for the body.
    pub container: Option<usize>,
    pub style: String,
    pub num: String,
    pub align: String,
    pub ind: String,
    pub line: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Item {
    /// `set`: `None` keeps the run's properties as they were (the text only changed); else the
    /// whole look the page shows, where `None` fields are what the style gives.
    Text {
        text: String,
        r: Option<usize>,
        #[serde(default)]
        set: Option<Set>,
    },
    Atom {
        a: usize,
        #[serde(default)]
        geo: Option<Geo>,
    },
    Br,
    /// A picture added on the page (`n` in the list given to `save`).
    Image {
        n: usize,
        #[serde(default)]
        geo: Option<Geo>,
    },
    Link {
        href: String,
        h: Option<usize>,
        items: Vec<Item>,
    },
}

/// A picture resized (`w`, `h`) or moved (`moved`: floating at `x` from the page's left edge and `y`
/// from the top of its paragraph), in points.
#[derive(Debug, Deserialize)]
pub struct Geo {
    #[serde(default)]
    pub moved: bool,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The look of some text where it differs from its styles: `Some` is written on the run, `None`
/// removes the run's own setting (so the style's shows).
#[derive(Debug, Default, Deserialize)]
pub struct Set {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<String>,
    pub bg: Option<String>,
    pub vert: Option<String>,
}

/// Children of a `w:pPr`/`w:rPr` as (local name, raw XML), set and written in schema order.
struct Props(Vec<(String, String)>);

impl Props {
    fn parse(raw: &str) -> Props {
        match xml::parse(raw.as_bytes()) {
            Ok(n) => Props(n.nodes().map(|c| (c.name.clone(), raw[c.span.clone()].to_owned())).collect()),
            Err(_) => Props(vec![]),
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, r)| r.as_str())
    }

    fn set(&mut self, name: &str, raw: Option<String>) {
        self.0.retain(|(n, _)| n != name);
        if let Some(raw) = raw {
            self.0.push((name.to_owned(), raw));
        }
    }

    fn write(mut self, tag: &str, order: &[&str]) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        let rank = |n: &str| order.iter().position(|o| *o == n).unwrap_or(order.len());
        self.0.sort_by_key(|(n, _)| rank(n));
        let mut out = format!("<w:{tag}>");
        for (_, raw) in self.0 {
            out.push_str(&raw);
        }
        let _ = write!(out, "</w:{tag}>");
        out
    }
}

/// An element's attributes from raw XML, with some replaced or removed.
fn with_attrs(raw: Option<&str>, name: &str, set: &[(&str, Option<String>)]) -> Option<String> {
    let mut attrs: Vec<(String, String)> = raw
        .and_then(|r| {
            let mut reader = quick_xml::Reader::from_str(r);
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e)) => {
                    Some(e.attributes().flatten().map(|a| (String::from(a.key.as_ref()), a.value.to_string())).collect())
                }
                _ => None,
            }
        })
        .unwrap_or_default();
    for (k, v) in set {
        let key = format!("w:{k}");
        attrs.retain(|(a, _)| *a != key);
        if let Some(v) = v {
            attrs.push((key, esc(v)));
        }
    }
    if attrs.is_empty() {
        return None;
    }
    let mut out = format!("<w:{name}");
    for (k, v) in attrs {
        let _ = write!(out, " {k}=\"{v}\"");
    }
    out.push_str("/>");
    Some(out)
}

pub(super) fn xml_text(s: &str) -> String {
    s.chars().filter(|c| matches!(*c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)).collect::<String>().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The links the page may create: web, mail and places in the document.
pub(super) fn safe_link(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || (href.starts_with('#') && href.len() > 1)
}

struct Out<'a> {
    edits: Vec<(usize, usize, usize, String)>,
    seq: usize,
    images: &'a [NewImage],
    /// Pictures written: (n, relationship id, part name).
    media: Vec<(usize, String, String)>,
    next_drawing: u32,
    /// Writing into document.xml (not a header or footer, whose links and pictures live in their
    /// own relationships: new ones aren't made there).
    main: bool,
    /// Headers and footers made here: (part name, XML, relationship id, "header" | "footer").
    new_parts: Vec<(String, String, String, &'static str)>,
    /// Relationships added (id, target), lists created (kind → numId).
    new_rels: Vec<(String, String)>,
    new_lists: Vec<(bool, String)>,
}

impl Out<'_> {
    fn edit(&mut self, range: Range<usize>, text: String) {
        self.seq += 1;
        self.edits.push((range.start, range.end, 1_000_000 + self.seq, text));
    }

    /// New things at one place go in the page's order.
    fn insert(&mut self, at: usize, order: usize, text: String) {
        self.edits.push((at, at, order, text));
    }
}

impl Session {
    /// The DOCX with the page's changes. Unchanged, it is the same file.
    pub fn save(&self, edits: &Edits, images: &[NewImage]) -> Result<Vec<u8>, DocError> {
        let present: HashSet<usize> = edits.present.iter().copied().filter(|&u| u < self.units.len()).collect();
        let next_drawing = self.xml.match_indices("docPr id=\"").filter_map(|(i, m)| self.xml[i + m.len()..].split('"').next()?.parse::<u32>().ok()).max().unwrap_or(0);
        let mut out = Out { edits: vec![], seq: 0, new_rels: vec![], new_lists: vec![], images, media: vec![], next_drawing, main: true, new_parts: vec![] };
        let mut numbering_xml = None::<String>;
        let mut next_ids = None::<(u32, u32)>;

        let missing = |u: usize| !present.contains(&u);
        // A unit is gone if it's missing and no missing table, row or block around it covers it.
        let ancestors = |u: usize| std::iter::successors(self.units[u].parent, |&p| self.units[p].parent);
        let mut deleted = HashSet::new();
        for (u, unit) in self.units.iter().enumerate().take(self.body()) {
            if !missing(u) || matches!(unit.kind, Kind::Cell | Kind::Root) || ancestors(u).any(|a| missing(a) && !matches!(self.units[a].kind, Kind::Cell | Kind::Root)) {
                continue;
            }
            // A paragraph whose cell is gone stays (cells are never removed).
            if unit.kind == Kind::Para && ancestors(u).any(|a| self.units[a].kind == Kind::Cell && missing(a)) {
                continue;
            }
            deleted.insert(u);
        }
        // Cells taken out with their column go too (other missing cells stay: the page lost them).
        let mut grid_cuts: HashSet<(usize, usize)> = HashSet::new();
        for rc in &edits.removed_cells {
            if rc.cell < self.body() && self.units[rc.cell].kind == Kind::Cell && missing(rc.cell) {
                deleted.insert(rc.cell);
                if let Some(t) = ancestors(rc.cell).find(|&a| self.units[a].kind == Kind::Table) {
                    grid_cuts.insert((t, rc.column));
                }
            }
        }
        // A table with no rows left goes whole.
        for (t, unit) in self.units.iter().enumerate() {
            if unit.kind == Kind::Table && !deleted.contains(&t) && !ancestors(t).any(|a| deleted.contains(&a)) {
                let rows: Vec<usize> = (0..self.units.len()).filter(|&r| self.units[r].kind == Kind::Row && self.units[r].parent == Some(t)).collect();
                if !rows.is_empty() && rows.iter().all(|r| deleted.contains(r)) {
                    rows.iter().for_each(|r| { deleted.remove(r); });
                    deleted.insert(t);
                }
            }
        }
        let gone = |u: usize| deleted.contains(&u) || ancestors(u).any(|a| deleted.contains(&a));

        for &u in &deleted {
            let unit = &self.units[u];
            if ancestors(u).any(|a| deleted.contains(&a)) {
                continue;
            }
            let mut keep = String::new();
            if unit.kind == Kind::Para {
                // Bookmarks and comment ranges stay where the paragraph was; a section break, or a
                // field that goes on in other paragraphs, keeps an empty paragraph.
                let marks: String = unit.marks.iter().map(|m| &self.xml[m.clone()]).collect();
                if unit.has_sect || !unit.fields_balanced {
                    let ppr = if unit.has_sect { unit.ppr.clone().map(|r| self.xml[r].to_owned()).unwrap_or_default() } else { String::new() };
                    let fields: String = if unit.fields_balanced { String::new() } else { unit.fields.iter().map(|&a| self.atoms[a].as_str()).collect() };
                    keep = format!("<w:p>{ppr}{marks}{fields}</w:p>");
                } else {
                    keep = marks;
                }
            }
            out.edit(unit.span.clone(), keep);
        }

        let mut written_in: HashMap<usize, usize> = HashMap::new();
        for p in &edits.paras {
            let container = p.container.filter(|&c| c < self.units.len() && matches!(self.units[c].kind, Kind::Cell | Kind::Root) && !gone(c)).unwrap_or(self.body());
            out.main = self.units[container].part == 0;
            let original = p.u.filter(|&u| u < self.body() && self.units[u].kind == Kind::Para && !gone(u));
            let base = original.or(p.base.filter(|&b| b < self.body() && self.units[b].kind == Kind::Para));
            let xml = self.paragraph_xml(p, original, base, &mut out, &mut numbering_xml, &mut next_ids)?;
            match original {
                Some(u) => out.edit(self.units[u].span.clone(), xml),
                None => {
                    let at = self.insert_at(p.after, container);
                    out.insert(at, p.order, xml);
                    *written_in.entry(container).or_default() += 1;
                }
            }
        }
        for t in &edits.tables {
            let container = t.container.filter(|&c| c < self.units.len() && matches!(self.units[c].kind, Kind::Cell | Kind::Root) && !gone(c)).unwrap_or(self.body());
            out.main = self.units[container].part == 0;
            let xml = self.table_xml(t, &mut out, &mut numbering_xml, &mut next_ids)?;
            let at = self.insert_at(t.after, container);
            out.insert(at, t.order, xml);
            *written_in.entry(container).or_default() += 1;
        }
        self.table_changes(edits, &grid_cuts, &gone, &mut out, &mut numbering_xml, &mut next_ids)?;
        self.new_margins(edits, &mut out, &mut numbering_xml, &mut next_ids)?;
        // A cell must keep a paragraph.
        for (c, unit) in self.units.iter().enumerate() {
            if !matches!(unit.kind, Kind::Cell | Kind::Root) || gone(c) || written_in.contains_key(&c) {
                continue;
            }
            let kept = (0..self.body()).any(|u| self.units[u].kind == Kind::Para && self.units[u].parent == Some(c) && !deleted.contains(&u));
            if !kept {
                out.edit(unit.content_start..unit.content_start, "<w:p/>".into());
            }
        }

        // Apply the edits in order (none overlap: units nest, and only the outermost changes).
        // At one position, insertions go before a change that starts there.
        out.edits.sort_by_key(|(s, e, seq, _)| (*s, e > s, *seq));
        let mut changed_parts = vec![];
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
                // Must still be XML Jay-E (and Word) can read, with every prefix declared.
                xml::parse(xml.as_bytes())?;
                check_namespaces(&xml)?;
                changed_parts.push((name.clone(), xml));
            }
        }
        if changed_parts.is_empty() && out.new_rels.is_empty() && numbering_xml.is_none() && out.media.is_empty() && out.new_parts.is_empty() {
            return Ok(self.file.clone());
        }
        self.package(changed_parts, &out, numbering_xml)
    }

    /// A child element's XML inside a unit (`w:trPr` of a row, `w:tcPr` of a cell), or "".
    fn child_raw(&self, u: usize, name: &str) -> String {
        let raw = &self.xml[self.units[u].span.clone()];
        xml::parse(raw.as_bytes()).ok().and_then(|n| n.child(name).map(|c| raw[c.span.clone()].to_owned())).unwrap_or_default()
    }

    fn children_of(&self, u: usize, kind: Kind) -> Vec<usize> {
        (0..self.body()).filter(|&c| self.units[c].kind == kind && self.units[c].parent == Some(u)).collect()
    }

    /// A new cell's properties: the template's, without its merging.
    fn new_tcpr(&self, template: Option<usize>) -> String {
        let raw = template.map(|t| self.child_raw(t, "tcPr")).unwrap_or_default();
        let mut props = Props::parse(&raw);
        for k in ["gridSpan", "hMerge", "vMerge"] {
            props.set(k, None);
        }
        props.write("tcPr", &["cnfStyle", "tcW", "gridSpan", "hMerge", "vMerge", "tcBorders", "shd", "noWrap", "tcMar", "textDirection", "tcFitText", "vAlign", "hideMark"])
    }

    fn cell_xml(&self, template: Option<usize>, paras: &[EditPara], out: &mut Out, numbering: &mut Option<String>, next_ids: &mut Option<(u32, u32)>) -> Result<String, DocError> {
        let mut xml = format!("<w:tc>{}", self.new_tcpr(template));
        for p in paras {
            xml.push_str(&self.paragraph_xml(p, None, None, out, numbering, next_ids)?);
        }
        if paras.is_empty() {
            xml.push_str("<w:p/>");
        }
        xml.push_str("</w:tc>");
        Ok(xml)
    }

    /// Rows and cells added to the file's tables, and the grid kept in step with the columns.
    fn table_changes(&self, edits: &Edits, cuts: &HashSet<(usize, usize)>, gone: &dyn Fn(usize) -> bool, out: &mut Out, numbering: &mut Option<String>, next_ids: &mut Option<(u32, u32)>) -> Result<(), DocError> {
        let valid = |u: usize, kind: Kind| u < self.body() && self.units[u].kind == kind && !gone(u);
        for r in &edits.rows {
            if !valid(r.table, Kind::Table) {
                continue;
            }
            let rows = self.children_of(r.table, Kind::Row);
            let Some(template) = Some(r.template).filter(|t| rows.contains(t)).or_else(|| rows.first().copied()) else { continue };
            out.main = self.units[r.table].part == 0;
            let tcells = self.children_of(template, Kind::Cell);
            let mut xml = format!("<w:tr>{}", self.child_raw(template, "trPr"));
            for (i, paras) in r.cells.iter().enumerate() {
                xml.push_str(&self.cell_xml(tcells.get(i).or(tcells.last()).copied(), paras, out, numbering, next_ids)?);
            }
            xml.push_str("</w:tr>");
            let at = r.after.filter(|a| rows.contains(a)).map(|a| self.units[a].span.end).unwrap_or(self.units[template].span.start);
            out.insert(at, r.order, xml);
        }
        let mut grows: HashSet<(usize, usize)> = HashSet::new();
        for c in &edits.cells {
            if !valid(c.row, Kind::Row) {
                continue;
            }
            let Some(table) = self.units[c.row].parent else { continue };
            let cells = self.children_of(c.row, Kind::Cell);
            let template = Some(c.template).filter(|&t| t < self.body() && self.units[t].kind == Kind::Cell).or_else(|| cells.first().copied());
            out.main = self.units[table].part == 0;
            let xml = self.cell_xml(template, &c.paras, out, numbering, next_ids)?;
            let at = match c.after.filter(|a| cells.contains(a)) {
                Some(a) => self.units[a].span.end,
                None => cells.first().map(|&f| self.units[f].span.start).unwrap_or(self.units[c.row].content_start),
            };
            out.insert(at, c.order, xml);
            grows.insert((table, c.column));
        }
        // One grid column per new column, a copy of its neighbor's; one less per column taken out.
        for &(t, col) in &grows {
            let grid = &self.units[t].grid;
            if let Some(last) = grid.len().checked_sub(1) {
                let copy = self.xml[grid[col.min(last)].clone()].to_owned();
                let at = if col <= last { grid[col].start } else { grid[last].end };
                out.insert(at, 0, copy);
            }
        }
        for &(t, col) in cuts {
            if let Some(g) = self.units[t].grid.get(col) {
                out.edit(g.clone(), String::new());
            }
        }
        Ok(())
    }

    /// A header or footer made where the file had none: its own part, related and referenced from
    /// the last section.
    fn new_margins(&self, edits: &Edits, out: &mut Out, numbering: &mut Option<String>, next_ids: &mut Option<(u32, u32)>) -> Result<(), DocError> {
        let mut names: HashSet<String> = zip::ZipArchive::new(Cursor::new(self.file.as_slice())).map_err(|e| invalid(e.to_string()))?.file_names().flatten().map(|n| n.into_owned()).collect();
        for (kind, paras, exists) in [("header", &edits.new_header, self.view.header.is_some()), ("footer", &edits.new_footer, self.view.footer.is_some())] {
            let Some(paras) = paras.as_ref().filter(|p| !p.is_empty() && !exists) else { continue };
            out.main = false;
            let tag = if kind == "header" { "hdr" } else { "ftr" };
            let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:{tag} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\">");
            for p in paras {
                xml.push_str(&self.paragraph_xml(p, None, None, out, numbering, next_ids)?);
            }
            let _ = write!(xml, "</w:{tag}>");
            let name = (1..).map(|n| format!("word/{kind}{n}.xml")).find(|n| !names.contains(n)).unwrap_or_default();
            names.insert(name.clone());
            let rid = (1..).map(|n| format!("rIdJayE{kind}{n}")).find(|id| !self.rels.contains_key(id)).unwrap_or_default();
            let ns = if self.has_r_ns { String::new() } else { format!(" xmlns:r=\"{R_NS}\"") };
            let reference = format!("<w:{kind}Reference{ns} w:type=\"default\" r:id=\"{rid}\"/>");
            match &self.body_sect {
                Some(sect) if self.xml[sect.clone()].ends_with("/>") => out.edit(sect.clone(), format!("<w:sectPr>{reference}</w:sectPr>")),
                Some(sect) => out.insert(start_tag_end(&self.xml, sect.start), 0, reference),
                None => out.insert(self.body_end, usize::MAX, format!("<w:sectPr>{reference}</w:sectPr>")),
            }
            out.new_parts.push((name, xml, rid, kind));
        }
        Ok(())
    }

    /// Where something new goes: after a unit of the same container, or first in it.
    fn insert_at(&self, after: Option<usize>, container: usize) -> usize {
        after
            .filter(|&a| a < self.body() && matches!(self.units[a].kind, Kind::Para | Kind::Table | Kind::Block) && self.units[a].parent.unwrap_or(self.body()) == container)
            .map(|a| self.units[a].span.end)
            .unwrap_or(self.units[container].content_start)
    }

    fn table_xml(&self, t: &EditTable, out: &mut Out, numbering: &mut Option<String>, next_ids: &mut Option<(u32, u32)>) -> Result<String, DocError> {
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0).clamp(1, 63);
        let twips = |pt: f32| (pt.clamp(10.0, 1600.0) * 20.0).round() as i32;
        let content = self.view.page.w - self.view.page.left - self.view.page.right;
        let widths: Vec<i32> = (0..cols).map(|i| twips(t.cols.get(i).copied().unwrap_or(content / cols as f32))).collect();
        let mut xml = String::from("<w:tbl><w:tblPr>");
        match &self.view.table_style {
            Some(style) => {
                let _ = write!(xml, "<w:tblStyle w:val=\"{}\"/><w:tblW w:w=\"0\" w:type=\"auto\"/>", esc(style));
            }
            None => {
                xml.push_str("<w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
                for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
                    let _ = write!(xml, "<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>");
                }
                xml.push_str("</w:tblBorders>");
            }
        }
        xml.push_str("<w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>");
        for w in &widths {
            let _ = write!(xml, "<w:gridCol w:w=\"{w}\"/>");
        }
        xml.push_str("</w:tblGrid>");
        for row in t.rows.iter().take(1000) {
            xml.push_str("<w:tr>");
            for (i, w) in widths.iter().enumerate() {
                let _ = write!(xml, "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/></w:tcPr>");
                let cell = row.get(i).map(Vec::as_slice).unwrap_or(&[]);
                for p in cell {
                    xml.push_str(&self.paragraph_xml(p, None, None, out, numbering, next_ids)?);
                }
                if cell.is_empty() {
                    xml.push_str("<w:p/>");
                }
                xml.push_str("</w:tc>");
            }
            xml.push_str("</w:tr>");
        }
        xml.push_str("</w:tbl>");
        Ok(xml)
    }

    /// A picture added on the page, inline (or floating, if it was moved), stored once in word/media.
    fn image_xml(&self, n: usize, geo: Option<&Geo>, out: &mut Out) -> Option<String> {
        let img = out.images.get(n)?;
        let rid = match out.media.iter().find(|(m, _, _)| *m == n) {
            Some((_, rid, _)) => rid.clone(),
            None => {
                let rid = self.new_id("rIdJayEImg", out.media.len() + 1);
                let name = format!("media/jaye-{}-{}.{}", std::process::id(), out.media.len() + 1, img.ext);
                out.media.push((n, rid.clone(), name));
                rid
            }
        };
        out.next_drawing += 1;
        let (w, h) = geo.map(|g| (g.w, g.h)).unwrap_or((img.w, img.h));
        let emu = |pt: f32| (pt.clamp(1.0, 22_000.0) * 12_700.0).round() as u64;
        let mut xml = super::docx::drawing(&rid, out.next_drawing, emu(w), emu(h));
        // Prefixes this document may not declare at its root.
        let root = &self.xml[..self.xml.find("<w:body").unwrap_or(0)];
        let mut ns = String::new();
        for (prefix, uri) in [("wp", "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"), ("r", R_NS)] {
            if !root.contains(&format!("xmlns:{prefix}=\"{uri}\"")) {
                let _ = write!(ns, " xmlns:{prefix}=\"{uri}\"");
            }
        }
        if !ns.is_empty() {
            xml = xml.replacen("<w:drawing>", &format!("<w:drawing{ns}>"), 1);
        }
        match geo.filter(|g| g.moved) {
            Some(g) => reshape(&xml, g),
            None => Some(xml),
        }
    }

    fn new_id(&self, base: &str, from: usize) -> String {
        (from..).map(|n| format!("{base}{n}")).find(|id| !self.rels.contains_key(id)).unwrap_or_default()
    }

    fn paragraph_xml(&self, p: &EditPara, original: Option<usize>, base: Option<usize>, out: &mut Out, numbering: &mut Option<String>, next_ids: &mut Option<(u32, u32)>) -> Result<String, DocError> {
        let tag = match original {
            Some(u) => {
                let t = &self.xml[self.units[u].start_tag.clone()];
                if t.ends_with("/>") { format!("{}>", t.trim_end_matches("/>").trim_end()) } else { t.to_owned() }
            }
            None => "<w:p>".into(),
        };
        let base_unit = base.map(|b| &self.units[b]);
        let mut ppr = Props::parse(base_unit.and_then(|b| b.ppr.clone()).map(|r| &self.xml[r]).unwrap_or(""));
        let was = base_unit.map(|b| b.direct.clone()).unwrap_or_default();
        // A new paragraph doesn't repeat a section break.
        if original.is_none() {
            ppr.set("sectPr", None);
        }
        if p.style != was.style {
            ppr.set("pStyle", (!p.style.is_empty()).then(|| format!("<w:pStyle w:val=\"{}\"/>", esc(&p.style))));
        }
        if p.align != was.align {
            let jc = match p.align.as_str() {
                "" => None,
                "center" | "right" | "both" | "left" => Some(p.align.clone()),
                _ => None,
            };
            ppr.set("jc", jc.map(|v| format!("<w:jc w:val=\"{v}\"/>")));
        }
        if p.ind != was.ind {
            let left = p.ind.parse::<i32>().ok().map(|v| v.clamp(-31680, 31680).to_string());
            let ind = with_attrs(ppr.get("ind"), "ind", &[("left", left), ("start", None)]);
            ppr.set("ind", ind);
        }
        if p.line != was.line {
            let line = p.line.parse::<i32>().ok().map(|v| v.clamp(60, 2400).to_string());
            let rule = line.as_ref().map(|_| "auto".to_owned());
            let spacing = with_attrs(ppr.get("spacing"), "spacing", &[("line", line), ("lineRule", rule)]);
            ppr.set("spacing", spacing);
        }
        if p.num != was.num {
            let style_id = if p.style.is_empty() { self.view.default_style.clone() } else { p.style.clone() };
            let style_num = self.view.styles.iter().find(|s| s.id == style_id).and_then(|s| s.num.clone());
            let numpr = match p.num.split_once(':') {
                Some((id, lvl)) => {
                    let lvl = lvl.parse::<u32>().unwrap_or(0).min(8);
                    let id = match id {
                        "new-bullet" | "new-number" => self.new_list(id == "new-bullet", out, numbering, next_ids)?,
                        id if self.view.numbering.contains_key(id) => id.to_owned(),
                        _ => return Err(invalid("unknown list")),
                    };
                    Some(format!("<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{id}\"/></w:numPr>"))
                }
                // Out of a list the style puts it in: numId 0 says "no list".
                None if style_num.is_some() => Some("<w:numPr><w:numId w:val=\"0\"/></w:numPr>".into()),
                None => None,
            };
            ppr.set("numPr", numpr);
        }
        let mut xml = tag;
        xml.push_str(&ppr.write("pPr", &PPR_ORDER));

        let mut atoms = vec![];
        collect_atoms(&p.items, &mut atoms);
        if let Some(u) = original {
            let unit = &self.units[u];
            for m in &unit.marks {
                xml.push_str(&self.xml[m.clone()]);
            }
            // Fields that go on into other paragraphs are always kept; one that is all here
            // goes back to plain text if part of it was deleted.
            if !unit.fields_balanced {
                for &a in unit.fields.iter().filter(|a| !atoms.contains(a)) {
                    xml.push_str(&self.atoms[a]);
                }
            }
        }
        let drop_fields: HashSet<usize> = match original {
            Some(u) if self.units[u].fields_balanced && self.units[u].fields.iter().any(|a| !atoms.contains(a)) => self.units[u].fields.iter().copied().collect(),
            _ => HashSet::new(),
        };
        self.items_xml(&p.items, &drop_fields, &mut xml, out, 0);
        xml.push_str("</w:p>");
        Ok(xml)
    }

    fn items_xml(&self, items: &[Item], drop_fields: &HashSet<usize>, xml: &mut String, out: &mut Out, depth: usize) {
        for item in items {
            match item {
                Item::Text { text, r, set } => {
                    let rpr = self.rpr_xml(r.and_then(|r| self.runs.get(r)), set.as_ref());
                    let mut chunk = String::new();
                    let flush = |chunk: &mut String, xml: &mut String| {
                        if !chunk.is_empty() {
                            let _ = write!(xml, "<w:t xml:space=\"preserve\">{}</w:t>", xml_text(chunk));
                            chunk.clear();
                        }
                    };
                    xml.push_str("<w:r>");
                    xml.push_str(&rpr);
                    for c in text.chars() {
                        let special = match c {
                            '\t' => "<w:tab/>",
                            '\n' => "<w:br/>",
                            '\u{2011}' => "<w:noBreakHyphen/>",
                            '\u{AD}' => "<w:softHyphen/>",
                            _ => {
                                chunk.push(c);
                                continue;
                            }
                        };
                        flush(&mut chunk, xml);
                        xml.push_str(special);
                    }
                    flush(&mut chunk, xml);
                    xml.push_str("</w:r>");
                }
                Item::Atom { a, geo } => {
                    if let Some(raw) = self.atoms.get(*a).filter(|_| !drop_fields.contains(a)) {
                        match geo.as_ref().and_then(|g| reshape(raw, g)) {
                            Some(changed) => xml.push_str(&changed),
                            None => xml.push_str(raw),
                        }
                    }
                }
                Item::Br => xml.push_str("<w:r><w:br/></w:r>"),
                Item::Image { n, geo } => {
                    if let Some(img) = self.image_xml(*n, geo.as_ref(), out).filter(|_| out.main) {
                        xml.push_str(&img);
                    }
                }
                Item::Link { href, h, items } => {
                    let same = h.and_then(|h| self.links.get(h)).filter(|(_, target)| target == href);
                    let open = match same {
                        Some((tag, _)) => Some(tag.clone()),
                        None if depth == 0 && out.main && safe_link(href) => Some(match href.strip_prefix('#') {
                            Some(anchor) => format!("<w:hyperlink w:anchor=\"{}\">", esc(anchor)),
                            None => {
                                let id = self.new_rel(href, out);
                                let ns = if self.has_r_ns { String::new() } else { format!(" xmlns:r=\"{R_NS}\"") };
                                format!("<w:hyperlink r:id=\"{id}\"{ns} w:history=\"1\">")
                            }
                        }),
                        None => None,
                    };
                    match open {
                        Some(tag) => {
                            xml.push_str(&tag);
                            self.items_xml(items, drop_fields, xml, out, depth + 1);
                            xml.push_str("</w:hyperlink>");
                        }
                        None => self.items_xml(items, drop_fields, xml, out, depth + 1),
                    }
                }
            }
        }
    }

    fn rpr_xml(&self, base: Option<&Vec<(String, String)>>, set: Option<&Set>) -> String {
        let mut rpr = Props(base.cloned().unwrap_or_default());
        let Some(set) = set else { return rpr.write("rPr", &RPR_ORDER) };
        let flag = |on: bool, name: &str| if on { format!("<w:{name}/>") } else { format!("<w:{name} w:val=\"0\"/>") };
        let font = set.font.as_ref().map(|f| f.chars().filter(|c| !c.is_control()).take(64).collect::<String>());
        let fonts = with_attrs(rpr.get("rFonts"), "rFonts", &[("ascii", font.clone()), ("hAnsi", font.clone()), ("cs", font), ("asciiTheme", None), ("hAnsiTheme", None), ("cstheme", None)]);
        rpr.set("rFonts", fonts);
        let half = set.size.map(|s| ((s * 2.0).round() as i32).clamp(2, 3276));
        rpr.set("sz", half.map(|h| format!("<w:sz w:val=\"{h}\"/>")));
        rpr.set("szCs", half.map(|h| format!("<w:szCs w:val=\"{h}\"/>")));
        rpr.set("b", set.bold.map(|b| flag(b, "b")));
        rpr.set("bCs", None);
        rpr.set("i", set.italic.map(|i| flag(i, "i")));
        rpr.set("iCs", None);
        // A double or wavy underline that is still there stays as it is.
        if set.underline != Some(true) || rpr.get("u").is_none_or(|u| u.contains("\"none\"")) {
            rpr.set("u", set.underline.map(|u| format!("<w:u w:val=\"{}\"/>", if u { "single" } else { "none" })));
        }
        if set.strike != Some(true) || rpr.get("strike").or(rpr.get("dstrike")).is_none() {
            rpr.set("dstrike", None);
            rpr.set("strike", set.strike.map(|s| flag(s, "strike")));
        }
        let color = set.color.as_deref().and_then(|c| if c == "auto" { Some("auto".to_owned()) } else { hex(c) });
        rpr.set("color", color.map(|c| format!("<w:color w:val=\"{c}\"/>")));
        rpr.set("highlight", None);
        rpr.set("shd", None);
        if let Some(bg) = set.bg.as_deref().and_then(hex) {
            match HIGHLIGHTS.iter().find(|(_, h)| *h == bg) {
                Some((name, _)) => rpr.set("highlight", Some(format!("<w:highlight w:val=\"{name}\"/>"))),
                None => rpr.set("shd", Some(format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{bg}\"/>"))),
            }
        }
        let vert = set.vert.as_deref().filter(|v| matches!(*v, "superscript" | "subscript"));
        rpr.set("vertAlign", vert.map(|v| format!("<w:vertAlign w:val=\"{v}\"/>")));
        rpr.write("rPr", &RPR_ORDER)
    }

    fn new_rel(&self, target: &str, out: &mut Out) -> String {
        if let Some((id, _)) = out.new_rels.iter().find(|(_, t)| t == target) {
            return id.clone();
        }
        let mut n = out.new_rels.len() + 1;
        let id = loop {
            let id = format!("rIdJayE{n}");
            if !self.rels.contains_key(&id) {
                break id;
            }
            n += 1;
        };
        out.new_rels.push((id.clone(), target.to_owned()));
        id
    }

    /// A list for the toolbar's bullet or number button, made once per save.
    fn new_list(&self, bullet: bool, out: &mut Out, numbering: &mut Option<String>, next_ids: &mut Option<(u32, u32)>) -> Result<String, DocError> {
        if let Some((_, id)) = out.new_lists.iter().find(|(b, _)| *b == bullet) {
            return Ok(id.clone());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(self.file.as_slice())).map_err(|e| invalid(e.to_string()))?;
        if numbering.is_none() {
            *numbering = Some(match zip_entry(&mut zip, "word/numbering.xml")? {
                Some(x) => String::from_utf8(x).map_err(|_| invalid("numbering.xml is not UTF-8"))?,
                None => format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"{W_NS}\"></w:numbering>"),
            });
        }
        let text = numbering.as_mut().unwrap();
        if !text.contains("<w:numbering") {
            return Err(invalid("numbering.xml uses unusual XML prefixes"));
        }
        let (abs, num) = next_ids.get_or_insert_with(|| {
            let max = |attr: &str| text.match_indices(&format!("w:{attr}=\"")).filter_map(|(i, m)| text[i + m.len()..].split('"').next()?.parse::<u32>().ok()).max().unwrap_or(0);
            (max("abstractNumId") + 1, max("numId") + 1)
        });
        let (abs_id, num_id) = (*abs, *num);
        *abs += 1;
        *num += 1;
        // Abstract definitions come before the w:num elements.
        let first_num = text.find("<w:num ").or_else(|| text.find("<w:num>")).or_else(|| text.rfind("</w:numbering>")).ok_or_else(|| invalid("numbering.xml"))?;
        text.insert_str(first_num, &new_abstract(abs_id, bullet));
        let end = text.rfind("</w:numbering>").ok_or_else(|| invalid("numbering.xml"))?;
        text.insert_str(end, &format!("<w:num w:numId=\"{num_id}\"><w:abstractNumId w:val=\"{abs_id}\"/></w:num>"));
        out.new_lists.push((bullet, num_id.to_string()));
        Ok(num_id.to_string())
    }

    /// The new package: changed parts rewritten, every other entry copied as it was.
    fn package(&self, parts: Vec<(String, String)>, out: &Out, numbering: Option<String>) -> Result<Vec<u8>, DocError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(self.file.as_slice())).map_err(|e| invalid(e.to_string()))?;
        let had_numbering = zip.by_name("word/numbering.xml").is_ok();
        let mut replaced: HashMap<String, Vec<u8>> = HashMap::new();
        for (name, xml) in parts {
            replaced.insert(name, xml.into_bytes());
        }
        let mut rel_add = String::new();
        let mut types_add = String::new();
        let types_now = String::from_utf8(zip_entry(&mut zip, "[Content_Types].xml")?.ok_or_else(|| invalid("no [Content_Types].xml"))?).map_err(|_| invalid("[Content_Types].xml"))?;
        for (n, rid, name) in &out.media {
            let img = &out.images[*n];
            let _ = write!(rel_add, "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{name}\"/>");
            replaced.insert(format!("word/{name}"), img.bytes.clone());
            let ext = img.ext;
            if !types_now.to_ascii_lowercase().contains(&format!("extension=\"{ext}\"")) && !types_add.contains(&format!("\"{ext}\"")) {
                let mime = if ext == "jpeg" { "image/jpeg".to_owned() } else { format!("image/{ext}") };
                let _ = write!(types_add, "<Default Extension=\"{ext}\" ContentType=\"{mime}\"/>");
            }
        }
        for (name, xml, rid, kind) in &out.new_parts {
            let target = name.trim_start_matches("word/");
            let _ = write!(rel_add, "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}\" Target=\"{target}\"/>");
            let _ = write!(types_add, "<Override PartName=\"/{name}\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml\"/>");
            replaced.insert(name.clone(), xml.clone().into_bytes());
        }
        for (id, target) in &out.new_rels {
            let _ = write!(rel_add, "<Relationship Id=\"{id}\" Type=\"{HYPERLINK_REL}\" Target=\"{}\" TargetMode=\"External\"/>", esc(target));
        }
        if let Some(n) = numbering {
            if !had_numbering {
                let _ = write!(rel_add, "<Relationship Id=\"rIdJayENumbering\" Type=\"{NUMBERING_REL}\" Target=\"numbering.xml\"/>");
                let _ = write!(types_add, "<Override PartName=\"/word/numbering.xml\" ContentType=\"{NUMBERING_TYPE}\"/>");
            }
            replaced.insert("word/numbering.xml".into(), n.into_bytes());
        }
        if !types_add.is_empty() {
            let end = types_now.rfind("</Types>").ok_or_else(|| invalid("[Content_Types].xml"))?;
            let mut types = types_now.clone();
            types.insert_str(end, &types_add);
            replaced.insert("[Content_Types].xml".into(), types.into_bytes());
        }
        if !rel_add.is_empty() {
            let rels = match zip_entry(&mut zip, "word/_rels/document.xml.rels")? {
                Some(r) => String::from_utf8(r).map_err(|_| invalid("document.xml.rels"))?,
                None => "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>".into(),
            };
            let end = rels.rfind("</Relationships>").ok_or_else(|| invalid("document.xml.rels"))?;
            let mut rels = rels;
            rels.insert_str(end, &rel_add);
            replaced.insert("word/_rels/document.xml.rels".into(), rels.into_bytes());
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

/// Every element and attribute prefix is declared (Word refuses the file otherwise).
pub(super) fn check_namespaces(xml: &str) -> Result<(), DocError> {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    let mut reader = quick_xml::reader::NsReader::from_str(xml);
    loop {
        match reader.read_resolved_event().map_err(|e| invalid(format!("XML: {e}")))? {
            (ResolveResult::Unknown(prefix), Event::Start(_) | Event::Empty(_)) => return Err(invalid(format!("undeclared XML prefix {}", prefix))),
            (_, Event::Start(e) | Event::Empty(e)) => {
                for a in e.attributes().flatten() {
                    if let (ResolveResult::Unknown(prefix), _) = reader.resolver().resolve_attribute(a.key) {
                        return Err(invalid(format!("undeclared XML prefix {}", prefix)));
                    }
                }
            }
            (_, Event::Eof) => return Ok(()),
            _ => {}
        }
    }
}

fn collect_atoms(items: &[Item], out: &mut Vec<usize>) {
    for i in items {
        match i {
            Item::Atom { a, .. } => out.push(*a),
            Item::Link { items, .. } => collect_atoms(items, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Align, Block, Document, Inline, Style};

    fn sample() -> Vec<u8> {
        let bold = Style { bold: true, ..Default::default() };
        let doc = Document {
            blocks: vec![
                Block::Heading { level: 1, inlines: vec![Inline::text("Title")] },
                Block::Paragraph { inlines: vec![Inline::text("Plain, "), Inline::Text { text: "bold".into(), style: bold }, Inline::text(" and a "), Inline::Link { href: "https://example.com".into(), inlines: vec![Inline::text("link")] }], align: Align::Start },
                Block::List { ordered: false, items: vec![vec![Block::Paragraph { inlines: vec![Inline::text("one")], align: Align::Start }]] },
                Block::Table { rows: vec![vec![vec![Block::Paragraph { inlines: vec![Inline::text("cell")], align: Align::Start }]]] },
                Block::Paragraph { inlines: vec![Inline::text("last")], align: Align::Center },
            ],
            images: vec![],
        };
        crate::doc::docx::write(&doc).unwrap()
    }

    fn open(bytes: &[u8]) -> Session {
        Session::open(bytes, &|n| format!("img/{n}")).unwrap()
    }

    fn entry(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        String::from_utf8(zip_entry(&mut zip, name).unwrap().unwrap()).unwrap()
    }

    fn all_units(s: &Session) -> Vec<usize> {
        (0..s.units.len()).collect()
    }

    fn edits(s: &Session, paras: Vec<EditPara>) -> Edits {
        Edits { present: all_units(s), paras, tables: vec![], rows: vec![], cells: vec![], removed_cells: vec![], new_header: None, new_footer: None }
    }

    fn para(s: &Session, text: &str) -> usize {
        (0..s.body()).find(|&u| s.units[u].kind == Kind::Para && s.xml[s.units[u].span.clone()].contains(text)).unwrap()
    }

    fn edit(u: Option<usize>, s: &Session, items: Vec<Item>) -> EditPara {
        let d = u.map(|u| s.units[u].direct.clone()).unwrap_or_default();
        EditPara { order: 0, u, base: None, after: None, container: None, style: d.style, num: d.num, align: d.align, ind: d.ind, line: d.line, items }
    }

    fn text(t: &str, r: Option<usize>) -> Item {
        Item::Text { text: t.into(), r, set: None }
    }

    #[test]
    fn saving_without_edits_is_the_same_file() {
        let bytes = sample();
        let s = open(&bytes);
        assert!(s.view.html.contains("Title") && s.view.html.contains("<table"));
        assert!(s.view.css.contains(".dx-page{width:453.6pt;min-height:700.2pt;padding:70.85pt"), "{}", s.view.css);
        let saved = s.save(&edits(&s, vec![]), &[]).unwrap();
        assert_eq!(saved, bytes);
    }

    #[test]
    fn an_edited_paragraph_changes_and_nothing_else() {
        let bytes = sample();
        let s = open(&bytes);
        let p = para(&s, "Plain");
        let set = Set { bold: Some(true), color: Some("ff0000".into()), font: Some("Arial".into()), size: Some(14.0), ..Default::default() };
        let items = vec![text("New text ", Some(0)), Item::Text { text: "red\tbold".into(), r: None, set: Some(set) }];
        let saved = s.save(&edits(&s, vec![edit(Some(p), &s, items)]), &[]).unwrap();
        let before = entry(&bytes, "word/document.xml");
        let after = entry(&saved, "word/document.xml");
        assert!(after.contains("New text ") && !after.contains("Plain, "));
        assert!(after.contains("<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/><w:b/><w:color w:val=\"ff0000\"/><w:sz w:val=\"28\"/><w:szCs w:val=\"28\"/></w:rPr>"), "{after}");
        assert!(after.contains("<w:tab/>"));
        // The other paragraphs are byte for byte the same.
        for t in ["Title", "one", "cell", "last"] {
            let u = para(&s, t);
            assert!(after.contains(&s.xml[s.units[u].span.clone()]), "{t}");
        }
        assert_eq!(entry(&saved, "word/styles.xml"), entry(&bytes, "word/styles.xml"));
        assert_ne!(before, after);
        // And Jay-E's reader sees it.
        let doc = crate::doc::docx::read(&saved).unwrap();
        assert!(crate::doc::text::write(&doc).contains("New text red"));
    }

    #[test]
    fn new_deleted_and_emptied() {
        let bytes = sample();
        let s = open(&bytes);
        let last = para(&s, "last");
        let cell_p = para(&s, "cell");
        let cell = s.units[cell_p].parent.unwrap();
        let title = para(&s, "Title");
        let present: Vec<usize> = all_units(&s).into_iter().filter(|&u| u != title && u != cell_p).collect();
        // Enter copies the paragraph's attributes to the new one.
        let mut new = edit(Some(last), &s, vec![text("after last", None)]);
        new.u = None;
        new.base = Some(last);
        new.after = Some(last);
        let saved = s.save(&Edits { present, ..edits(&s, vec![new]) }, &[]).unwrap();
        let after = entry(&saved, "word/document.xml");
        assert!(!after.contains(">Title<") && !after.contains(">cell<"));
        assert!(after.find("last").unwrap() < after.find("after last").unwrap());
        assert!(after.contains("<w:jc w:val=\"center\"/></w:pPr><w:r><w:t xml:space=\"preserve\">after last"), "new paragraph keeps its base's properties: {after}");
        // The emptied cell got a paragraph back.
        assert!(after.contains("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>"), "{after}");
        let _ = cell;
    }

    #[test]
    fn lists_alignment_and_new_numbering() {
        let bytes = sample();
        let s = open(&bytes);
        let p = para(&s, "last");
        let mut e = edit(Some(p), &s, vec![text("last", None)]);
        e.num = "new-number:0".into();
        e.align = "right".into();
        e.line = "360".into();
        let saved = s.save(&edits(&s, vec![e]), &[]).unwrap();
        let after = entry(&saved, "word/document.xml");
        assert!(after.contains("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"2\"/></w:numPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/><w:jc w:val=\"right\"/>"), "{after}");
        let numbering = entry(&saved, "word/numbering.xml");
        assert!(numbering.contains("<w:num w:numId=\"2\"><w:abstractNumId w:val=\"2\"/></w:num>"), "{numbering}");
        let reopened = open(&saved);
        assert_eq!(reopened.view.numbering["2"][0].fmt, "decimal");
    }

    #[test]
    fn unbolding_a_bold_run_removes_its_bold() {
        let bytes = sample();
        let s = open(&bytes);
        let p = para(&s, "Plain");
        let items = vec![text("Plain, ", Some(1)), Item::Text { text: "bold".into(), r: Some(2), set: Some(Set::default()) }];
        let saved = s.save(&edits(&s, vec![edit(Some(p), &s, items)]), &[]).unwrap();
        let after = entry(&saved, "word/document.xml");
        assert!(after.contains("<w:r><w:t xml:space=\"preserve\">bold</w:t></w:r>"), "{after}");
    }

    const INLINE_PIC: &str = r#"<w:r><w:rPr><w:noProof/></w:rPr><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="1270000" cy="635000"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="7" name="Picture 7"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic><a:graphicData uri="x"><pic:pic><pic:blipFill><a:blip r:embed="rId9"/></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;

    #[test]
    fn pictures_resize_and_float_where_they_are_dropped() {
        assert!(drawing_attrs(&xml::parse(INLINE_PIC.as_bytes()).unwrap().child("drawing").unwrap()).contains("data-w=\"100\" data-h=\"50\""));
        // Resized in place: both sizes change, it stays inline.
        let resized = reshape(INLINE_PIC, &Geo { moved: false, x: 0.0, y: 0.0, w: 200.0, h: 100.0 }).unwrap();
        assert_eq!(resized.matches("cx=\"2540000\" cy=\"1270000\"").count(), 2, "{resized}");
        assert!(resized.contains("<wp:inline"));
        // Moved: a floating anchor at that place, over the text, the picture kept.
        let moved = reshape(INLINE_PIC, &Geo { moved: true, x: 300.0, y: -20.0, w: 100.0, h: 50.0 }).unwrap();
        assert!(moved.contains("<wp:anchor ") && !moved.contains("wp:inline"), "{moved}");
        assert!(moved.contains("<wp:positionH relativeFrom=\"page\"><wp:posOffset>3810000</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>-254000</wp:posOffset></wp:positionV>"), "{moved}");
        assert!(moved.contains("<wp:wrapNone/><wp:docPr id=\"7\" name=\"Picture 7\"/><wp:cNvGraphicFramePr>") && moved.contains("r:embed=\"rId9\""));
        xml::parse(moved.as_bytes()).unwrap();
        let attrs = drawing_attrs(&xml::parse(moved.as_bytes()).unwrap().child("drawing").unwrap());
        assert!(attrs.contains("data-float=\"1\" data-hr=\"page\" data-hx=\"300\" data-vr=\"paragraph\" data-vy=\"-20\""), "{attrs}");
        // An inline that declares its own prefixes keeps them when it floats.
        let own = INLINE_PIC.replace("<wp:inline distT", "<wp:inline xmlns:a=\"urn:a\" xmlns:pic=\"urn:pic\" distT");
        let floated = reshape(&own, &Geo { moved: true, x: 1.0, y: 1.0, w: 10.0, h: 10.0 }).unwrap();
        assert!(floated.starts_with("<w:r><w:rPr><w:noProof/></w:rPr><w:drawing><wp:anchor xmlns:a=\"urn:a\" xmlns:pic=\"urn:pic\" distT"), "{floated}");
        assert!(check_namespaces(&floated.replacen("<w:r>", "<w:r xmlns:w=\"w\" xmlns:wp=\"wp\" xmlns:r=\"r\">", 1)).is_ok());
        assert!(check_namespaces("<w:p xmlns:w=\"w\"><a:b/></w:p>").is_err());
        assert!(check_namespaces("<w:p xmlns:w=\"w\"><w:b r:id=\"1\"/></w:p>").is_err());
        // Moved again: only the place changes.
        let again = reshape(&moved, &Geo { moved: true, x: 10.0, y: 5.0, w: 100.0, h: 50.0 }).unwrap();
        assert!(again.contains("<wp:posOffset>127000</wp:posOffset>") && again.matches("<wp:anchor ").count() == 1);
    }

    #[test]
    fn a_new_picture_and_a_new_table() {
        let bytes = sample();
        let s = open(&bytes);
        let png = {
            let mut v = Vec::new();
            image::DynamicImage::new_rgb8(64, 32).write_to(&mut Cursor::new(&mut v), image::ImageFormat::Png).unwrap();
            v
        };
        let img = prepare_image(&png).unwrap();
        assert_eq!((img.ext, img.w, img.h), ("png", 48.0, 24.0));
        let last = para(&s, "last");
        let mut e = edit(Some(last), &s, vec![text("last ", None), Item::Image { n: 0, geo: None }]);
        e.order = 1;
        let cell = |t: &str| vec![{
            let mut p = edit(None, &s, vec![text(t, None)]);
            p.align = String::new();
            p
        }];
        let table = EditTable { order: 2, after: Some(last), container: None, cols: vec![], rows: vec![vec![cell("a"), cell("b")], vec![cell("c"), vec![]]] };
        let saved = s.save(&Edits { tables: vec![table], ..edits(&s, vec![e]) }, &[img]).unwrap();
        let after = entry(&saved, "word/document.xml");
        check_namespaces(&after).unwrap();
        assert!(after.contains("<a:blip r:embed=\"rIdJayEImg1\"/>") && after.contains("<wp:extent cx=\"609600\" cy=\"304800\"/>"), "{after}");
        let rels = entry(&saved, "word/_rels/document.xml.rels");
        assert!(rels.contains("Id=\"rIdJayEImg1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/jaye-"), "{rels}");
        let mut zip = zip::ZipArchive::new(Cursor::new(saved.clone())).unwrap();
        assert!(zip.file_names().flatten().any(|n| n.starts_with("word/media/jaye-") && n.ends_with(".png")));
        drop(zip.by_name("word/document.xml"));
        // The table comes right after the paragraph, two by two, the empty cell with a paragraph.
        let table_at = after.rfind("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/>").expect(&after);
        assert!(after.find(">last </w:t>").unwrap() < table_at);
        assert_eq!(after[table_at..].matches("<w:gridCol w:w=\"4536\"/>").count(), 2, "{after}");
        assert!(after.contains("<w:tc><w:tcPr><w:tcW w:w=\"4536\" w:type=\"dxa\"/></w:tcPr><w:p/></w:tc>"));
        let doc = crate::doc::docx::read(&saved).unwrap();
        assert!(matches!(&doc.blocks[..], [.., Block::Table { rows }] if rows.len() == 2), "{:?}", doc.blocks);
        assert_eq!(doc.images.len(), 1);
    }

    /// The sample with a default header (one paragraph) and its relationship.
    fn with_header() -> Vec<u8> {
        let bytes = sample();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            let name = f.name().unwrap().into_owned();
            let mut data = String::new();
            std::io::Read::read_to_string(&mut f, &mut data).unwrap();
            if name == "word/document.xml" {
                data = data.replace("<w:sectPr>", "<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdHdr\"/>");
            } else if name == "word/_rels/document.xml.rels" {
                data = data.replace("</Relationships>", "<Relationship Id=\"rIdHdr\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\"/></Relationships>");
            }
            w.start_file(name.as_str(), opts).unwrap();
            w.write_all(data.as_bytes()).unwrap();
        }
        w.start_file("word/header1.xml", opts).unwrap();
        w.write_all(format!("<?xml version=\"1.0\"?><w:hdr xmlns:w=\"{W_NS}\"><w:p><w:r><w:t>Company</w:t></w:r></w:p></w:hdr>").as_bytes()).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn the_header_is_edited_in_its_own_part() {
        let bytes = with_header();
        let s = open(&bytes);
        let header = s.view.header.clone().unwrap();
        assert!(header.html.contains("Company"));
        let p = para(&s, "Company");
        assert_eq!(s.units[p].part, 1);
        let mut e = edit(Some(p), &s, vec![text("Company Ltd. ", None), Item::Link { href: "https://x.example".into(), h: None, items: vec![text("site", None)] }]);
        e.container = Some(header.u);
        let saved = s.save(&edits(&s, vec![e]), &[]).unwrap();
        let hdr = entry(&saved, "word/header1.xml");
        assert!(hdr.contains(">Company Ltd. </w:t>") && hdr.contains(">site</w:t>") && !hdr.contains("hyperlink"), "{hdr}");
        // document.xml and its relationships are as they were.
        assert_eq!(entry(&saved, "word/document.xml"), entry(&bytes, "word/document.xml"));
        assert_eq!(entry(&saved, "word/_rels/document.xml.rels"), entry(&bytes, "word/_rels/document.xml.rels"));
        // Saving with no edits is still the same file.
        assert_eq!(s.save(&edits(&s, vec![]), &[]).unwrap(), bytes);
    }

    #[test]
    fn rows_and_columns_in_a_table_of_the_file() {
        let bytes = sample();
        let s = open(&bytes);
        let table = (0..s.body()).find(|&u| s.units[u].kind == Kind::Table).unwrap();
        let row = s.children_of(table, Kind::Row)[0];
        let cell = s.children_of(row, Kind::Cell)[0];
        let para = |t: &str| {
            let mut p = edit(None, &s, vec![text(t, None)]);
            p.align = String::new();
            p
        };
        // A row below, a column after the first cell.
        let new_row = EditRow { order: 1, table, after: Some(row), template: row, cells: vec![vec![para("r2c1")], vec![para("r2c2")]] };
        let new_cell = EditCell { order: 2, row, after: Some(cell), template: cell, column: 1, paras: vec![para("r1c2")] };
        let saved = s.save(&Edits { rows: vec![new_row], cells: vec![new_cell], ..edits(&s, vec![]) }, &[]).unwrap();
        let x = entry(&saved, "word/document.xml");
        check_namespaces(&x).unwrap();
        assert!(x.contains(">cell</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p><w:r><w:t xml:space=\"preserve\">r1c2</w:t>"), "{x}");
        assert!(x.contains("</w:tr><w:tr><w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p><w:r><w:t xml:space=\"preserve\">r2c1"), "{x}");
        assert_eq!(x.matches("<w:gridCol w:w=\"9072\"/>").count(), 2, "the grid grew with the column: {x}");
        let doc = crate::doc::docx::read(&saved).unwrap();
        assert!(matches!(&doc.blocks[3], Block::Table { rows } if rows.len() == 2 && rows[0].len() == 2), "{:?}", doc.blocks[3]);
        // Taking the new column out again (on the saved file): cell and grid column go.
        let s2 = open(&saved);
        let table2 = (0..s2.body()).find(|&u| s2.units[u].kind == Kind::Table).unwrap();
        let row2 = s2.children_of(table2, Kind::Row)[0];
        let second = s2.children_of(row2, Kind::Cell)[1];
        let present: Vec<usize> = all_units(&s2).into_iter().filter(|&u| u != second && s2.units[u].parent != Some(second)).collect();
        let back = s2.save(&Edits { present, removed_cells: vec![RemovedCell { cell: second, column: 1 }], ..edits(&s2, vec![]) }, &[]).unwrap();
        let y = entry(&back, "word/document.xml");
        assert!(!y.contains("r1c2") && y.contains("r2c2") && y.matches("<w:gridCol").count() == 1, "{y}");
    }

    #[test]
    fn a_header_where_there_was_none() {
        let bytes = sample();
        let s = open(&bytes);
        assert!(s.view.header.is_none());
        let mut p = edit(None, &s, vec![text("New header", None)]);
        p.align = String::new();
        let saved = s.save(&Edits { new_header: Some(vec![p]), ..edits(&s, vec![]) }, &[]).unwrap();
        let x = entry(&saved, "word/document.xml");
        assert!(x.contains("<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdJayEheader1\"/><w:pgSz"), "{x}");
        assert!(entry(&saved, "word/header1.xml").contains(">New header</w:t>"));
        assert!(entry(&saved, "word/_rels/document.xml.rels").contains("Id=\"rIdJayEheader1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\""));
        assert!(entry(&saved, "[Content_Types].xml").contains("PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\""));
        let again = open(&saved);
        assert!(again.view.header.unwrap().html.contains("New header"));
    }

    #[test]
    fn a_new_link_gets_a_relationship() {
        let bytes = sample();
        let s = open(&bytes);
        let p = para(&s, "last");
        let items = vec![Item::Link { href: "https://jay.example/".into(), h: None, items: vec![text("site", None)] }, Item::Link { href: "javascript:alert(1)".into(), h: None, items: vec![text(" no", None)] }];
        let saved = s.save(&edits(&s, vec![edit(Some(p), &s, items)]), &[]).unwrap();
        let after = entry(&saved, "word/document.xml");
        assert!(after.contains("<w:hyperlink r:id=\"rIdJayE1\" w:history=\"1\">") && !after.contains("javascript"), "{after}");
        assert!(entry(&saved, "word/_rels/document.xml.rels").contains("Id=\"rIdJayE1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://jay.example/\""));
    }

    #[test]
    fn the_existing_link_and_text_runs_round_trip() {
        let bytes = sample();
        let s = open(&bytes);
        let p = para(&s, "Plain");
        assert!(s.view.html.contains("data-h=\"0\""));
        let items = vec![text("Plain, ", Some(1)), text("bold", Some(2)), Item::Link { href: "https://example.com".into(), h: Some(0), items: vec![text("link", Some(4))] }];
        let saved = s.save(&edits(&s, vec![edit(Some(p), &s, items)]), &[]).unwrap();
        let after = entry(&saved, "word/document.xml");
        assert!(after.contains(&format!("{}<w:r><w:rPr><w:rStyle w:val=\"Hyperlink\"/></w:rPr><w:t xml:space=\"preserve\">link</w:t></w:r></w:hyperlink>", s.links[0].0)), "{after}");
        assert!(after.contains("<w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">bold"));
    }
}
