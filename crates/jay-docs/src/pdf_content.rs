//! Editing what a PDF page itself draws (stage 10): its text lines, its pictures and the blanks of a
//! form printed on it, read from the page's content stream.
//!
//! The content stream is run through a small interpreter (graphics and text state, paths, `Do`),
//! enough to know where each piece of text and each picture lands. Changing text rewrites the
//! operators that show it: the new text goes in place of the first piece of the line (so it keeps
//! its place, color and clipping), the rest of the line's pieces go, and the old content stream is
//! pruned from the file, so the old text is really gone. The new text keeps the file's font when
//! every letter is one the page already shows with it (an embedded font may hold only those);
//! otherwise it goes in the standard font closest to it (Helvetica, Times or Courier).
//!
//! Only the page's own content is read: text and pictures inside Form XObjects aren't listed.

use crate::pdf_edit::{EditError, Geo, deref, flatten, geo, invalid, load, name, num, page_id, save, text_width, win_ansi};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Encoding, Object, ObjectId, Stream, StringFormat, dictionary};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

const MAX_LINES: usize = 5000;
const MAX_PICTURES: usize = 500;
const MAX_BLANKS: usize = 300;
/// The shortest printed line taken for a blank to fill in (half an inch).
const MIN_BLANK: f32 = 36.0;

/// A line of text on the page.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Line {
    pub index: usize,
    pub text: String,
    /// `[x, y, w, h]` on the shown page, in points.
    pub rect: [f32; 4],
    /// The text's height on the shown page, in points.
    pub size: f32,
    pub font: String,
}

/// A picture drawn by the page.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Picture {
    pub index: usize,
    pub rect: [f32; 4],
}

/// A place to fill in: a printed line or underscores ("line"), or a small square ("box").
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Blank {
    pub kind: String,
    pub rect: [f32; 4],
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct PageContent {
    pub lines: Vec<Line>,
    pub pictures: Vec<Picture>,
    pub blanks: Vec<Blank>,
}

// ---------------------------------------------------------------- matrices

type M = [f32; 6];
const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b` (PDF's row vectors: a point goes through `a` first).
fn mul(a: M, b: M) -> M {
    [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3], a[4] * b[0] + a[5] * b[2] + b[4], a[4] * b[1] + a[5] * b[3] + b[5]]
}

fn apply_m(m: M, [x, y]: [f32; 2]) -> [f32; 2] {
    [x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5]]
}

fn inverse(m: M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

fn operands(op: &Operation) -> Vec<f32> {
    op.operands.iter().filter_map(num).collect()
}

fn matrix_of(v: &[f32]) -> Option<M> {
    (v.len() == 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

// ---------------------------------------------------------------- fonts

struct Font<'a> {
    two_byte: bool,
    /// Width in 1/1000 em by character code.
    widths: HashMap<u32, f32>,
    default: f32,
    /// No width table at all (the standard 14): widths are guessed from the letters.
    guessed: bool,
    encoding: Option<Encoding<'a>>,
    embedded: bool,
    base: String,
}

fn font_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, obj).as_dict().ok()
}

fn has_file(doc: &Document, font: &Dictionary) -> bool {
    font.get(b"FontDescriptor").ok().and_then(|d| font_dict(doc, d)).is_some_and(|d| d.has(b"FontFile") || d.has(b"FontFile2") || d.has(b"FontFile3"))
}

fn read_font<'a>(doc: &'a Document, dict: &'a Dictionary) -> Font<'a> {
    let base = dict.get(b"BaseFont").ok().and_then(name).unwrap_or_default();
    let encoding = dict.get_font_encoding(doc).ok();
    let mut widths = HashMap::new();
    if dict.get(b"Subtype").ok().and_then(name).as_deref() == Some("Type0") {
        let descendant = dict.get(b"DescendantFonts").ok().map(|o| deref(doc, o)).and_then(|o| o.as_array().ok()).and_then(|a| a.first()).and_then(|o| font_dict(doc, o));
        let default = descendant.and_then(|d| d.get(b"DW").ok()).and_then(|o| num(deref(doc, o))).unwrap_or(1000.0);
        // /W: `c [w1 w2 …]` or `c_first c_last w`.
        if let Some(Ok(w)) = descendant.and_then(|d| d.get(b"W").ok()).map(|o| deref(doc, o).as_array()) {
            let mut i = 0;
            while i < w.len() && widths.len() < 70_000 {
                let Some(first) = num(deref(doc, &w[i])) else { break };
                match w.get(i + 1).map(|o| deref(doc, o)) {
                    Some(Object::Array(list)) => {
                        for (k, v) in list.iter().enumerate() {
                            if let Some(v) = num(deref(doc, v)) {
                                widths.insert(first as u32 + k as u32, v);
                            }
                        }
                        i += 2;
                    }
                    Some(last) => {
                        let (Some(last), Some(v)) = (num(last), w.get(i + 2).and_then(|o| num(deref(doc, o)))) else { break };
                        for c in first as u32..=(last as u32).min(first as u32 + 70_000) {
                            widths.insert(c, v);
                        }
                        i += 3;
                    }
                    None => break,
                }
            }
        }
        let embedded = descendant.is_some_and(|d| has_file(doc, d));
        return Font { two_byte: true, widths, default, guessed: false, encoding, embedded, base };
    }
    let first = dict.get(b"FirstChar").ok().and_then(|o| num(deref(doc, o))).unwrap_or(0.0) as u32;
    if let Some(Ok(w)) = dict.get(b"Widths").ok().map(|o| deref(doc, o).as_array()) {
        for (k, v) in w.iter().enumerate() {
            if let Some(v) = num(deref(doc, v)) {
                widths.insert(first + k as u32, v);
            }
        }
    }
    let missing = dict.get(b"FontDescriptor").ok().and_then(|d| font_dict(doc, d)).and_then(|d| d.get(b"MissingWidth").ok()).and_then(|o| num(deref(doc, o))).unwrap_or(0.0);
    let guessed = widths.is_empty();
    Font { two_byte: false, widths, default: missing, guessed, encoding, embedded: has_file(doc, dict), base }
}

impl Font<'_> {
    fn decode(&self, code: &[u8]) -> String {
        match &self.encoding {
            Some(e) => e.bytes_to_string(code).unwrap_or_else(|_| "\u{FFFD}".into()),
            None if !self.two_byte => code.iter().map(|&b| b as char).collect(),
            None => "\u{FFFD}".into(),
        }
    }

    /// The width of a code, in 1/1000 em.
    fn width(&self, code: u32, text: &str) -> f32 {
        if self.guessed {
            return text_width(text, 1000.0);
        }
        self.widths.get(&code).copied().unwrap_or(self.default)
    }
}

/// The standard 14 font closest to a font's name.
fn standard_for(base: &str) -> &'static str {
    let b = base.to_ascii_lowercase();
    let bold = ["bold", "black", "heavy", "semibold", "demi"].iter().any(|w| b.contains(w));
    let italic = b.contains("italic") || b.contains("oblique");
    let family = if ["courier", "mono", "consola", "code"].iter().any(|w| b.contains(w)) {
        0
    } else if ["times", "serif", "georgia", "garamond", "cambria", "roman", "book", "minion", "palatino"].iter().any(|w| b.contains(w)) && !b.contains("sans") {
        1
    } else {
        2
    };
    const NAMES: [[&str; 4]; 3] = [
        ["Courier", "Courier-Bold", "Courier-Oblique", "Courier-BoldOblique"],
        ["Times-Roman", "Times-Bold", "Times-Italic", "Times-BoldItalic"],
        ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique", "Helvetica-BoldOblique"],
    ];
    NAMES[family][usize::from(bold) + 2 * usize::from(italic)]
}

// ---------------------------------------------------------------- the interpreter

#[derive(Clone)]
struct Glyph {
    text: String,
    code: Vec<u8>,
    /// Baseline start and end on the shown page.
    a: [f32; 2],
    b: [f32; 2],
}

struct Piece {
    op: usize,
    font: Vec<u8>,
    /// The `Tf` size in effect.
    tf: f32,
    /// The text's height on the shown page.
    height: f32,
    horizontal: bool,
    glyphs: Vec<Glyph>,
}

struct Image {
    op: usize,
    ctm: M,
    rect: [f32; 4],
}

#[derive(Clone, Copy)]
struct State {
    ctm: M,
    font: usize,
    size: f32,
    tc: f32,
    tw: f32,
    th: f32,
    tl: f32,
    rise: f32,
}

enum Seg {
    Line([f32; 2], [f32; 2]),
    Rect([f32; 4]),
}

/// What the interpreter found on a page.
struct Scan {
    ops: Vec<Operation>,
    pieces: Vec<Piece>,
    images: Vec<Image>,
    /// Horizontal printed lines `(x0, x1, y)` and vertical ones, on the shown page.
    hlines: Vec<(f32, f32, f32)>,
    vlines: Vec<(f32, f32, f32)>,
    boxes: Vec<[f32; 4]>,
}

fn shown_bbox(g: &Geo, points: &[[f32; 2]]) -> [f32; 4] {
    let p: Vec<[f32; 2]> = points.iter().map(|&q| g.from_user(q)).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for q in &p {
        x0 = x0.min(q[0]);
        y0 = y0.min(q[1]);
        x1 = x1.max(q[0]);
        y1 = y1.max(q[1]);
    }
    [x0, y0, x1 - x0, y1 - y0]
}

fn scan(doc: &Document, page: ObjectId) -> Result<Scan, EditError> {
    let content = Content::decode(&doc.get_page_content(page)).map_err(|e| invalid(format!("the page's content can't be read ({e})")))?;
    let g = geo(doc, page);
    let font_dicts = doc.get_page_fonts(page).unwrap_or_default();
    let keys: Vec<Vec<u8>> = font_dicts.keys().cloned().collect();
    let fonts: Vec<Font> = font_dicts.values().map(|d| read_font(doc, d)).collect();
    let xobjects = xobjects(doc, page);

    let mut st = State { ctm: ID, font: usize::MAX, size: 0.0, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, rise: 0.0 };
    let mut stack: Vec<State> = vec![];
    let (mut tm, mut tlm) = (ID, ID);
    let mut path: Vec<Seg> = vec![];
    let mut current = [0.0f32; 2];
    let mut start = [0.0f32; 2];
    let mut scan = Scan { ops: vec![], pieces: vec![], images: vec![], hlines: vec![], vlines: vec![], boxes: vec![] };

    for (i, op) in content.operations.iter().enumerate() {
        let v = operands(op);
        let mut show: Vec<&Object> = vec![];
        match op.operator.as_str() {
            "q" if stack.len() < 64 => stack.push(st),
            "Q" => st = stack.pop().unwrap_or(st),
            "cm" => {
                if let Some(m) = matrix_of(&v) {
                    st.ctm = mul(m, st.ctm);
                }
            }
            "BT" => {
                tm = ID;
                tlm = ID;
            }
            "Tf" => {
                st.font = op.operands.first().and_then(name).and_then(|k| keys.iter().position(|q| q == k.as_bytes())).unwrap_or(usize::MAX);
                st.size = v.first().copied().unwrap_or(0.0);
            }
            "Tc" => st.tc = v.first().copied().unwrap_or(0.0),
            "Tw" => st.tw = v.first().copied().unwrap_or(0.0),
            "Tz" => st.th = v.first().copied().unwrap_or(100.0) / 100.0,
            "TL" => st.tl = v.first().copied().unwrap_or(0.0),
            "Ts" => st.rise = v.first().copied().unwrap_or(0.0),
            "Td" | "TD" if v.len() == 2 => {
                if op.operator == "TD" {
                    st.tl = -v[1];
                }
                tlm = mul([1.0, 0.0, 0.0, 1.0, v[0], v[1]], tlm);
                tm = tlm;
            }
            "Tm" => {
                if let Some(m) = matrix_of(&v) {
                    tlm = m;
                    tm = m;
                }
            }
            "T*" => {
                tlm = mul([1.0, 0.0, 0.0, 1.0, 0.0, -st.tl], tlm);
                tm = tlm;
            }
            "Tj" | "TJ" => show = op.operands.iter().collect(),
            "'" | "\"" => {
                if op.operator == "\"" && v.len() >= 2 {
                    st.tw = v[0];
                    st.tc = v[1];
                }
                tlm = mul([1.0, 0.0, 0.0, 1.0, 0.0, -st.tl], tlm);
                tm = tlm;
                show = op.operands.last().into_iter().collect();
            }
            "m" if v.len() == 2 => {
                current = apply_m(st.ctm, [v[0], v[1]]);
                start = current;
            }
            "l" if v.len() == 2 => {
                let p = apply_m(st.ctm, [v[0], v[1]]);
                path.push(Seg::Line(current, p));
                current = p;
            }
            "c" | "v" | "y" if v.len() >= 4 => current = apply_m(st.ctm, [v[v.len() - 2], v[v.len() - 1]]),
            "h" => {
                path.push(Seg::Line(current, start));
                current = start;
            }
            "re" if v.len() == 4 => {
                let corners = [[v[0], v[1]], [v[0] + v[2], v[1]], [v[0] + v[2], v[1] + v[3]], [v[0], v[1] + v[3]]].map(|p| apply_m(st.ctm, p));
                path.push(Seg::Rect(shown_bbox(&g, &corners)));
                current = corners[0];
                start = current;
            }
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                let o = op.operator.as_str();
                let stroke = matches!(o, "S" | "s" | "B" | "B*" | "b" | "b*");
                let fill = matches!(o, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                for seg in path.drain(..) {
                    match seg {
                        Seg::Rect([x, y, w, h]) => {
                            if stroke && (6.0..=28.0).contains(&w) && (6.0..=28.0).contains(&h) && (w - h).abs() <= 2.0 {
                                scan.boxes.push([x, y, w, h]);
                            } else if fill && h <= 1.5 && w >= MIN_BLANK {
                                scan.hlines.push((x, x + w, y + h / 2.0));
                            } else if fill && w <= 1.5 && h >= 6.0 {
                                scan.vlines.push((y, y + h, x + w / 2.0));
                            } else if stroke && w >= MIN_BLANK && h >= 6.0 {
                                // A frame's sides end the lines that run into it.
                                scan.vlines.push((y, y + h, x));
                                scan.vlines.push((y, y + h, x + w));
                            }
                        }
                        Seg::Line(a, b) if stroke => {
                            let (a, b) = (g.from_user(a), g.from_user(b));
                            if (a[1] - b[1]).abs() < 1.0 && (a[0] - b[0]).abs() >= MIN_BLANK {
                                scan.hlines.push((a[0].min(b[0]), a[0].max(b[0]), (a[1] + b[1]) / 2.0));
                            } else if (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() >= 6.0 {
                                scan.vlines.push((a[1].min(b[1]), a[1].max(b[1]), a[0]));
                            }
                        }
                        Seg::Line(..) => {}
                    }
                }
            }
            "Do" => {
                let image = op.operands.first().and_then(name).and_then(|k| xobjects.get(k.as_bytes())).is_some_and(|id| doc.get_object(*id).ok().and_then(|o| o.as_stream().ok()).is_some_and(|s| s.dict.get(b"Subtype").ok().and_then(name).as_deref() == Some("Image")));
                if image && scan.images.len() < MAX_PICTURES {
                    let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|p| apply_m(st.ctm, p));
                    scan.images.push(Image { op: i, ctm: st.ctm, rect: shown_bbox(&g, &corners) });
                }
            }
            _ => {}
        }
        if show.is_empty() || scan.pieces.len() >= MAX_LINES * 4 {
            continue;
        }
        let Some(font) = fonts.get(st.font) else { continue };
        let mut glyphs = vec![];
        for obj in show {
            let parts: Vec<&Object> = match obj {
                Object::Array(a) => a.iter().collect(),
                o => vec![o],
            };
            for part in parts {
                match part {
                    Object::String(bytes, _) => {
                        let step = if font.two_byte { 2 } else { 1 };
                        for code in bytes.chunks(step) {
                            let value = code.iter().fold(0u32, |a, &b| a * 256 + b as u32);
                            let text = font.decode(code);
                            let w0 = font.width(value, &text) / 1000.0;
                            let space = if step == 1 && code == [32] { st.tw } else { 0.0 };
                            let advance = (w0 * st.size + st.tc + space) * st.th;
                            let trm = mul([st.size * st.th, 0.0, 0.0, st.size, 0.0, st.rise], mul(tm, st.ctm));
                            let a = g.from_user(apply_m(trm, [0.0, 0.0]));
                            tm = mul([1.0, 0.0, 0.0, 1.0, advance, 0.0], tm);
                            let trm2 = mul([st.size * st.th, 0.0, 0.0, st.size, 0.0, st.rise], mul(tm, st.ctm));
                            let b = g.from_user(apply_m(trm2, [0.0, 0.0]));
                            glyphs.push(Glyph { text, code: code.to_vec(), a, b });
                        }
                    }
                    o => {
                        if let Some(adj) = num(o) {
                            let advance = -adj / 1000.0 * st.size * st.th;
                            tm = mul([1.0, 0.0, 0.0, 1.0, advance, 0.0], tm);
                        }
                    }
                }
            }
        }
        let trm = mul([st.size * st.th, 0.0, 0.0, st.size, 0.0, st.rise], mul(tm, st.ctm));
        let up = [trm[2], trm[3]];
        let height = (up[0] * up[0] + up[1] * up[1]).sqrt();
        let horizontal = glyphs.iter().all(|q| (q.a[1] - q.b[1]).abs() < 0.05 * height.max(1.0) && q.b[0] >= q.a[0] - 0.01);
        scan.pieces.push(Piece { op: i, font: keys.get(st.font).cloned().unwrap_or_default(), tf: st.size, height, horizontal, glyphs });
    }
    scan.ops = content.operations;
    Ok(scan)
}

/// The page's XObjects by name.
fn xobjects(doc: &Document, page: ObjectId) -> BTreeMap<Vec<u8>, ObjectId> {
    let mut out = BTreeMap::new();
    let Ok(page) = doc.get_dictionary(page) else { return out };
    let res = page.get(b"Resources").ok().and_then(|o| font_dict(doc, o));
    if let Some(Ok(x)) = res.and_then(|r| r.get(b"XObject").ok()).map(|o| deref(doc, o).as_dict()) {
        for (k, v) in x.iter() {
            if let Object::Reference(id) = v {
                out.insert(k.clone(), *id);
            }
        }
    }
    out
}

/// Pieces that sit on one row, one after the other, become a line.
fn lines_of(pieces: &[Piece]) -> Vec<Vec<usize>> {
    let mut lines: Vec<Vec<usize>> = vec![];
    let mut last: Option<(f32, f32, f32)> = None; // end x, baseline y, height
    for (i, p) in pieces.iter().enumerate() {
        let (Some(first), Some(end)) = (p.glyphs.first(), p.glyphs.last()) else { continue };
        let h = p.height.max(1.0);
        let joins = p.horizontal
            && last.is_some_and(|(x, y, lh)| {
                let h = h.max(lh);
                (first.a[1] - y).abs() < 0.3 * h && first.a[0] - x > -0.5 * h && first.a[0] - x < 1.5 * h
            });
        if joins {
            lines.last_mut().unwrap().push(i);
        } else {
            lines.push(vec![i]);
        }
        last = p.horizontal.then_some((end.b[0], end.b[1], h));
    }
    lines.retain(|l| l.iter().any(|&i| pieces[i].glyphs.iter().any(|g| !g.text.trim().is_empty())));
    lines.truncate(MAX_LINES);
    lines
}

fn line_text(pieces: &[Piece], line: &[usize]) -> String {
    let mut out = String::new();
    let mut prev: Option<(&Glyph, f32)> = None;
    for &i in line {
        for g in &pieces[i].glyphs {
            if let Some((p, h)) = prev {
                if pieces[i].horizontal && g.a[0] - p.b[0] > 0.2 * h && !p.text.ends_with(' ') && !g.text.starts_with(' ') {
                    out.push(' ');
                }
            }
            out.push_str(&g.text);
            prev = Some((g, pieces[i].height.max(1.0)));
        }
    }
    out.trim().to_owned()
}

fn line_rect(pieces: &[Piece], line: &[usize]) -> ([f32; 4], f32) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut size: f32 = 0.0;
    for &i in line {
        let h = pieces[i].height;
        size = size.max(h);
        for g in &pieces[i].glyphs {
            for p in [g.a, g.b] {
                x0 = x0.min(p[0]);
                x1 = x1.max(p[0]);
                y0 = y0.min(p[1] - 0.85 * h);
                y1 = y1.max(p[1] + 0.25 * h);
            }
        }
    }
    ([x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)], size)
}

// ---------------------------------------------------------------- reading

/// The text lines, pictures and blanks of page `page`.
pub fn page_content(bytes: &[u8], page: usize) -> Result<PageContent, EditError> {
    let doc = load(bytes)?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let id = page_id(&pages, page)?;
    let s = scan(&doc, id)?;
    let fonts = doc.get_page_fonts(id).unwrap_or_default();
    let lines: Vec<Line> = lines_of(&s.pieces)
        .iter()
        .enumerate()
        .map(|(index, l)| {
            let (rect, size) = line_rect(&s.pieces, l);
            let font = fonts.get(&s.pieces[l[0]].font).and_then(|d| d.get(b"BaseFont").ok()).and_then(name).unwrap_or_default();
            // Subset fonts are named "ABCDEF+Name".
            let font = font.split_once('+').map(|(_, f)| f.to_owned()).unwrap_or(font);
            Line { index, text: line_text(&s.pieces, l), rect, size, font }
        })
        .collect();
    let pictures = s.images.iter().enumerate().map(|(index, im)| Picture { index, rect: im.rect }).collect();
    Ok(PageContent { lines, pictures, blanks: blanks(&s) })
}

fn blanks(s: &Scan) -> Vec<Blank> {
    let mut lines = s.hlines.clone();
    // Runs of three or more underscores.
    for p in s.pieces.iter().filter(|p| p.horizontal) {
        let mut run: Vec<&Glyph> = vec![];
        for g in p.glyphs.iter().chain(std::iter::once(&Glyph { text: String::new(), code: vec![], a: [0.0; 2], b: [0.0; 2] })) {
            if g.text == "_" {
                run.push(g);
                continue;
            }
            if run.len() >= 3 {
                lines.push((run[0].a[0], run.last().unwrap().b[0], run[0].a[1] + 0.1 * p.height));
            }
            run.clear();
        }
    }
    // Written-on glyphs (not underscores or spaces), to drop rules under text.
    let ink: Vec<(f32, f32, f32)> = s.pieces.iter().flat_map(|p| p.glyphs.iter().filter(|g| !g.text.trim().is_empty() && g.text != "_").map(|g| (g.a[0].min(g.b[0]), g.a[0].max(g.b[0]), g.a[1]))).collect();
    let mut out: Vec<Blank> = vec![];
    for (x0, x1, y) in lines {
        let w = x1 - x0;
        // Part of a table or a frame: something vertical meets one of its ends.
        let framed = s.vlines.iter().any(|&(v0, v1, vx)| ((vx - x0).abs() < 3.0 || (vx - x1).abs() < 3.0) && y >= v0 - 3.0 && y <= v1 + 3.0);
        // Text already sitting on it (a heading's rule, a name already printed).
        let written = ink.iter().filter(|&&(a, b, gy)| b > x0 && a < x1 && gy <= y + 2.0 && gy >= y - 8.0).map(|&(a, b, _)| b.min(x1) - a.max(x0)).sum::<f32>() > 0.3 * w;
        let seen = out.iter().any(|b| b.kind == "line" && (b.rect[0] - x0).abs() < 2.0 && (b.rect[1] + b.rect[3] - y).abs() < 2.0 && (b.rect[2] - w).abs() < 4.0);
        if !framed && !written && !seen {
            out.push(Blank { kind: "line".into(), rect: [x0, y - 14.0, w, 14.0] });
        }
    }
    for &r in &s.boxes {
        if !out.iter().any(|b| b.kind == "box" && (b.rect[0] - r[0]).abs() < 1.0 && (b.rect[1] - r[1]).abs() < 1.0) {
            out.push(Blank { kind: "box".into(), rect: r });
        }
    }
    out.truncate(MAX_BLANKS);
    out
}

// ---------------------------------------------------------------- changing

/// One change to what a page draws, as the page sends it.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ContentOp {
    /// Line `line` (from `page_content`) says `text` now; empty removes it.
    ReplaceText { page: usize, line: usize, text: String },
    /// Picture `index` moved and sized to `[x, y, w, h]` on the shown page.
    MoveImage { page: usize, index: usize, rect: [f32; 4] },
    DeleteImage { page: usize, index: usize },
}

/// A page's resources dictionary, made the page's own and writable, and its `kind` subdictionary.
fn own_resource(doc: &mut Document, page: ObjectId, kind: &[u8]) -> Result<Dictionary, EditError> {
    let res = doc.get_dictionary(page).map_err(invalid)?.get(b"Resources").ok().cloned();
    let res = match res {
        Some(Object::Reference(id)) => doc.get_dictionary(id).map_err(invalid)?.clone(),
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    let sub = match res.get(kind).ok() {
        Some(Object::Reference(id)) => doc.get_dictionary(*id).map_err(invalid)?.clone(),
        Some(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    let mut res = res;
    res.set(kind.to_vec(), Object::Dictionary(sub.clone()));
    doc.get_dictionary_mut(page).map_err(invalid)?.set("Resources", Object::Dictionary(res));
    Ok(sub)
}

/// Adds `obj` to the page's `kind` resources under a new name, and gives the name.
fn add_resource(doc: &mut Document, page: ObjectId, kind: &[u8], prefix: &str, obj: Object) -> Result<Vec<u8>, EditError> {
    let mut sub = own_resource(doc, page, kind)?;
    let key = (1..).map(|i| format!("{prefix}{i}").into_bytes()).find(|k| !sub.has(k)).unwrap();
    sub.set(key.clone(), obj);
    let page_dict = doc.get_dictionary_mut(page).map_err(invalid)?;
    if let Ok(Object::Dictionary(res)) = page_dict.get_mut(b"Resources") {
        res.set(kind.to_vec(), Object::Dictionary(sub));
    }
    Ok(key)
}

/// Writes `ops` as the page's only content stream and drops what nothing uses any more (the old
/// stream, with the old text in it).
fn put_content(doc: &mut Document, page: ObjectId, ops: Vec<Operation>) -> Result<(), EditError> {
    let bytes = Content { operations: ops }.encode().map_err(invalid)?;
    let mut stream = Stream::new(Dictionary::new(), bytes);
    let _ = stream.compress();
    let id = doc.add_object(stream);
    doc.get_dictionary_mut(page).map_err(invalid)?.set("Contents", Object::Reference(id));
    doc.prune_objects();
    Ok(())
}

fn op(name: &str, operands: Vec<Object>) -> Operation {
    Operation::new(name, operands)
}

fn real(v: f32) -> Object {
    Object::Real(v)
}

fn matrix_ops(m: M) -> Vec<Object> {
    m.iter().map(|&v| real(v)).collect()
}

/// What stays of a showing operator when its text goes: its line move.
fn without_text(o: &Operation) -> Vec<Operation> {
    match o.operator.as_str() {
        "'" => vec![op("T*", vec![])],
        "\"" if o.operands.len() >= 3 => vec![op("Tw", vec![o.operands[0].clone()]), op("Tc", vec![o.operands[1].clone()]), op("T*", vec![])],
        _ => vec![],
    }
}

pub fn apply(bytes: &[u8], change: &ContentOp) -> Result<Vec<u8>, EditError> {
    let mut doc = load(bytes)?;
    let pages = flatten(&mut doc)?;
    let (ContentOp::ReplaceText { page, .. } | ContentOp::MoveImage { page, .. } | ContentOp::DeleteImage { page, .. }) = change;
    let id = page_id(&pages, *page)?;
    let s = scan(&doc, id)?;
    let mut ops = s.ops.clone();
    // Each original operator's replacement, by index.
    let mut swap: HashMap<usize, Vec<Operation>> = HashMap::new();
    // Drawn after everything else.
    let mut after: Vec<Operation> = vec![];
    match change {
        ContentOp::ReplaceText { line, text, .. } => {
            let lines = lines_of(&s.pieces);
            let line = lines.get(*line).ok_or_else(|| invalid("no such line"))?;
            for &i in line {
                swap.insert(s.pieces[i].op, without_text(&s.ops[s.pieces[i].op]));
            }
            let text = text.replace(['\r', '\n'], " ");
            let text = text.trim();
            if !text.is_empty() {
                let first = &s.pieces[line[0]];
                // The letters the page already shows in this font, and their codes.
                let mut known: HashMap<String, Vec<u8>> = HashMap::new();
                for p in s.pieces.iter().filter(|p| p.font == first.font) {
                    for g in &p.glyphs {
                        known.entry(g.text.clone()).or_insert_with(|| g.code.clone());
                    }
                }
                let gap = !known.contains_key(" ");
                let fonts = doc.get_page_fonts(id).unwrap_or_default();
                let font = fonts.get(&first.font).map(|d| read_font(&doc, d));
                // A font that isn't embedded has every letter of its encoding.
                let open = font.as_ref().filter(|f| !f.embedded && !f.two_byte);
                let code_of = |c: char| -> Option<Vec<u8>> {
                    let s = c.to_string();
                    known.get(&s).cloned().or_else(|| {
                        let f = open?;
                        let b = f.encoding.as_ref()?.string_to_bytes(&s);
                        (!b.is_empty() && f.decode(&b) == s).then_some(b)
                    })
                };
                let base = font.as_ref().map(|f| f.base.clone()).unwrap_or_default();
                let format = if font.as_ref().is_some_and(|f| f.two_byte) { StringFormat::Hexadecimal } else { StringFormat::Literal };
                let codes: Vec<(char, Option<Vec<u8>>)> = text.chars().map(|c| (c, code_of(c))).collect();
                drop(font);
                drop(fonts);
                // Runs of letters the font has, in it, and of the others in the standard font closest to it.
                let mut runs: Vec<(bool, String)> = vec![];
                for (c, code) in &codes {
                    let own = (*c == ' ' && gap) || code.is_some();
                    match runs.last_mut() {
                        Some((o, r)) if *o == own || *c == ' ' => r.push(*c),
                        _ => runs.push((own, c.to_string())),
                    }
                }
                let code_of = |c: char| codes.iter().find(|(k, _)| *k == c).and_then(|(_, b)| b.clone());
                let mut new = without_text(&s.ops[first.op]);
                let mut standard_key: Option<Vec<u8>> = None;
                for (own, run) in runs {
                    if own {
                        let mut parts = vec![];
                        let mut word = vec![];
                        for c in run.chars() {
                            match code_of(c) {
                                Some(code) => word.extend(code),
                                // No space letter (the file spaced words by moving): the same here.
                                None => {
                                    parts.push(Object::String(std::mem::take(&mut word), format));
                                    parts.push(Object::Integer(-250));
                                }
                            }
                        }
                        parts.push(Object::String(word, format));
                        new.push(op("TJ", vec![Object::Array(parts)]));
                    } else {
                        let key = match &standard_key {
                            Some(k) => k.clone(),
                            None => {
                                let font_id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => standard_for(&base), "Encoding" => "WinAnsiEncoding" });
                                let k = add_resource(&mut doc, id, b"Font", "JEf", Object::Reference(font_id))?;
                                standard_key = Some(k.clone());
                                k
                            }
                        };
                        new.push(op("Tf", vec![Object::Name(key), real(first.tf)]));
                        new.push(op("Tj", vec![Object::String(win_ansi(&run), StringFormat::Literal)]));
                        new.push(op("Tf", vec![Object::Name(first.font.clone()), real(first.tf)]));
                    }
                }
                swap.insert(first.op, new);
            }
        }
        ContentOp::MoveImage { index, rect, .. } => {
            let im = s.images.get(*index).ok_or_else(|| invalid("no such picture"))?;
            if rect.iter().any(|v| !v.is_finite()) || rect[2] < 1.0 || rect[3] < 1.0 {
                return Err(invalid("not a size"));
            }
            let g = geo(&doc, id);
            let (old, new) = (g.user_rect(im.rect), g.user_rect(*rect));
            let (sx, sy) = ((new[2] - new[0]) / (old[2] - old[0]).max(1e-3), (new[3] - new[1]) / (old[3] - old[1]).max(1e-3));
            let move_m: M = [sx, 0.0, 0.0, sy, new[0] - old[0] * sx, new[1] - old[1] * sy];
            if inverse(im.ctm).is_none() {
                return Err(invalid("the picture has no size"));
            }
            // Drawn again at the end, from the page's own state: where it was it may be clipped to
            // its old box (Word does that), and the new place is outside it.
            swap.insert(im.op, vec![]);
            after = vec![op("q", vec![]), op("cm", matrix_ops(mul(im.ctm, move_m))), s.ops[im.op].clone(), op("Q", vec![])];
        }
        ContentOp::DeleteImage { index, .. } => {
            let im = s.images.get(*index).ok_or_else(|| invalid("no such picture"))?;
            swap.insert(im.op, vec![]);
        }
    }
    ops = ops.into_iter().enumerate().flat_map(|(i, o)| swap.remove(&i).unwrap_or_else(|| vec![o])).collect();
    if !after.is_empty() {
        // The page's operators inside q … Q, so what's added starts from a clean state.
        ops.insert(0, op("q", vec![]));
        ops.push(op("Q", vec![]));
        ops.extend(after);
    }
    put_content(&mut doc, id, ops)?;
    save(&mut doc)
}

/// Puts `picture` (RGBA) in place of picture `index`, fitted inside its box, centered.
pub fn replace_image(bytes: &[u8], page: usize, index: usize, picture: &image::RgbaImage) -> Result<Vec<u8>, EditError> {
    let mut doc = load(bytes)?;
    let pages = flatten(&mut doc)?;
    let id = page_id(&pages, page)?;
    let s = scan(&doc, id)?;
    let im = s.images.get(index).ok_or_else(|| invalid("no such picture"))?;
    let (w, h) = picture.dimensions();
    if w == 0 || h == 0 {
        return Err(invalid("an empty picture"));
    }
    let rgb: Vec<u8> = picture.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut dict = dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 };
    if picture.pixels().any(|p| p[3] < 255) {
        let alpha: Vec<u8> = picture.pixels().map(|p| p[3]).collect();
        let mut mask = Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 }, alpha);
        let _ = mask.compress();
        dict.set("SMask", Object::Reference(doc.add_object(mask)));
    }
    let mut stream = Stream::new(dict, rgb);
    let _ = stream.compress();
    let image_id = doc.add_object(stream);
    let key = add_resource(&mut doc, id, b"XObject", "JEim", Object::Reference(image_id))?;

    // Fitted inside the old box on the shown page, keeping the new picture's shape.
    let [x, y, bw, bh] = im.rect;
    let scale = (bw / w as f32).min(bh / h as f32);
    let (fw, fh) = (w as f32 * scale, h as f32 * scale);
    let g = geo(&doc, id);
    let (old, new) = (g.user_rect(im.rect), g.user_rect([x + (bw - fw) / 2.0, y + (bh - fh) / 2.0, fw, fh]));
    let (sx, sy) = ((new[2] - new[0]) / (old[2] - old[0]).max(1e-3), (new[3] - new[1]) / (old[3] - old[1]).max(1e-3));
    let move_m: M = [sx, 0.0, 0.0, sy, new[0] - old[0] * sx, new[1] - old[1] * sy];
    let inv = inverse(im.ctm).ok_or_else(|| invalid("the picture has no size"))?;
    let cm = mul(mul(im.ctm, move_m), inv);
    let ops: Vec<Operation> = s
        .ops
        .into_iter()
        .enumerate()
        .flat_map(|(i, o)| if i == im.op { vec![op("q", vec![]), op("cm", matrix_ops(cm)), op("Do", vec![Object::Name(key.clone())]), op("Q", vec![])] } else { vec![o] })
        .collect();
    put_content(&mut doc, id, ops)?;
    save(&mut doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form page: a heading with a rule under it, "Name: ____", a signature line, a check box, a
    /// 2-row table, a picture, and a line in an "embedded" font that only has some letters.
    fn sample() -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let helv = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let file = doc.add_object(Stream::new(Dictionary::new(), vec![0; 16]));
        let desc = doc.add_object(dictionary! { "Type" => "FontDescriptor", "FontName" => "ABCDEF+Georgia-Bold", "FontFile2" => file });
        let widths: Vec<Object> = (32..=122).map(|_| Object::Integer(500)).collect();
        let sub = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "ABCDEF+Georgia-Bold", "FirstChar" => 32, "LastChar" => 122, "Widths" => widths, "Encoding" => "WinAnsiEncoding", "FontDescriptor" => desc });
        let pixels = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 2, "Height" => 1, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 }, vec![255, 0, 0, 0, 0, 255]));
        let content = [
            "BT /F1 18 Tf 72 740 Td (Application) Tj ET",
            "0.5 w 72 736 m 300 736 l S",
            "BT /F1 12 Tf 72 700 Td (Name: ) Tj (__________) Tj ET",
            "72 640 m 252 640 l S",
            "BT /F1 10 Tf 72 628 Td (Signature) Tj ET",
            "72 600 12 12 re S",
            "BT /F1 12 Tf 90 602 Td (I agree) Tj ET",
            "72 500 200 60 re S 72 530 m 272 530 l S",
            "q 100 0 0 50 300 600 cm /Im1 Do Q",
            "BT /F2 12 Tf 72 450 Td [(Bad) -250 (dog)] TJ ET",
        ]
        .join("\n");
        let content = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Resources" => dictionary! { "Font" => dictionary! { "F1" => helv, "F2" => sub }, "XObject" => dictionary! { "Im1" => pixels } } });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut out = vec![];
        doc.save_to(&mut out).unwrap();
        out
    }

    /// Every stream of the file, decompressed, as text: to see what's really in it.
    fn everything(bytes: &[u8]) -> String {
        let doc = Document::load_mem(bytes).unwrap();
        doc.objects.values().filter_map(|o| o.as_stream().ok()).map(|s| String::from_utf8_lossy(&s.decompressed_content().unwrap_or_else(|_| s.content.clone())).into_owned()).collect::<Vec<_>>().join("\n")
    }

    fn texts(bytes: &[u8]) -> Vec<String> {
        page_content(bytes, 0).unwrap().lines.into_iter().map(|l| l.text).collect()
    }

    fn page_fonts(bytes: &[u8]) -> Vec<String> {
        let doc = Document::load_mem(bytes).unwrap();
        let page = doc.get_pages()[&1];
        doc.get_page_fonts(page).unwrap().values().filter_map(|d| d.get(b"BaseFont").ok().and_then(name)).collect()
    }

    #[test]
    fn lines_pictures_and_blanks_are_found() {
        let c = page_content(&sample(), 0).unwrap();
        let texts: Vec<&str> = c.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["Application", "Name: __________", "Signature", "I agree", "Bad dog"]);
        let name = &c.lines[1];
        assert!((name.rect[0] - 72.0).abs() < 0.5 && (name.size - 12.0).abs() < 0.01, "{name:?}");
        assert_eq!(c.lines[4].font, "Georgia-Bold");
        // The picture: 100×50 at (300, 600) from the bottom = y 142 from the top.
        assert_eq!(c.pictures.len(), 1);
        let r = c.pictures[0].rect;
        assert!((r[0] - 300.0).abs() < 0.01 && (r[1] - 142.0).abs() < 0.01 && (r[2] - 100.0).abs() < 0.01 && (r[3] - 50.0).abs() < 0.01, "{r:?}");
        // Blanks: the underscores, the signature line and the box; not the heading's rule (text on
        // it) nor the table's middle line (it meets the frame).
        let kinds: Vec<(&str, i32, i32)> = c.blanks.iter().map(|b| (b.kind.as_str(), b.rect[0].round() as i32, (b.rect[1] + b.rect[3]).round() as i32)).collect();
        assert!(kinds.contains(&("line", 72, 152)), "signature line: {kinds:?}");
        assert!(kinds.iter().any(|&(k, x, y)| k == "line" && x > 100 && (y - 92).abs() <= 1), "underscores: {kinds:?}");
        assert!(kinds.contains(&("box", 72, 192)), "box: {kinds:?}");
        assert_eq!(c.blanks.len(), 3, "not the heading's rule nor the table's line: {kinds:?}");
    }

    #[test]
    fn text_is_replaced_in_place_and_the_old_text_is_gone() {
        let pdf = sample();
        let changed = apply(&pdf, &ContentOp::ReplaceText { page: 0, line: 0, text: "Request form".into() }).unwrap();
        assert_eq!(texts(&changed)[0], "Request form");
        assert!(!everything(&changed).contains("Application"), "the old text stays in the file");
        let (a, b) = (&page_content(&pdf, 0).unwrap().lines[0], &page_content(&changed, 0).unwrap().lines[0]);
        assert!((a.rect[0] - b.rect[0]).abs() < 0.01 && (a.rect[1] - b.rect[1]).abs() < 0.01 && a.size == b.size, "same place and size");
        // Helvetica isn't embedded: any letter goes, in the file's own font.
        assert_eq!(page_fonts(&changed).len(), 2);

        // An embedded font: letters it already shows keep it…
        let same = apply(&pdf, &ContentOp::ReplaceText { page: 0, line: 4, text: "dog  Bad".into() }).unwrap();
        assert_eq!(texts(&same)[4], "dog Bad");
        assert_eq!(page_fonts(&same).len(), 2, "kept the embedded font");
        // …a letter it doesn't have goes in the closest standard font.
        let other = apply(&pdf, &ContentOp::ReplaceText { page: 0, line: 4, text: "Good dog".into() }).unwrap();
        assert_eq!(texts(&other)[4], "Good dog");
        assert!(page_fonts(&other).contains(&"Times-Bold".to_owned()), "{:?}", page_fonts(&other));

        // Empty text removes the line; the rest stays.
        let gone = apply(&pdf, &ContentOp::ReplaceText { page: 0, line: 2, text: " ".into() }).unwrap();
        assert_eq!(texts(&gone), ["Application", "Name: __________", "I agree", "Bad dog"]);
        assert!(apply(&pdf, &ContentOp::ReplaceText { page: 0, line: 99, text: "x".into() }).is_err());
    }

    #[test]
    fn pictures_move_and_go() {
        let pdf = sample();
        let moved = apply(&pdf, &ContentOp::MoveImage { page: 0, index: 0, rect: [50.0, 60.0, 200.0, 80.0] }).unwrap();
        let r = page_content(&moved, 0).unwrap().pictures[0].rect;
        for (got, want) in r.iter().zip([50.0, 60.0, 200.0, 80.0]) {
            assert!((got - want).abs() < 0.01, "{r:?}");
        }
        assert!(apply(&pdf, &ContentOp::MoveImage { page: 0, index: 0, rect: [0.0, 0.0, 0.0, 10.0] }).is_err());
        let gone = apply(&pdf, &ContentOp::DeleteImage { page: 0, index: 0 }).unwrap();
        assert!(page_content(&gone, 0).unwrap().pictures.is_empty());
        assert!(!everything(&gone).contains("Do"));

        // A square picture in the 100×50 box is 50×50, centered; half clear, so it has a mask.
        let square = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 128]));
        let swapped = replace_image(&pdf, 0, 0, &square).unwrap();
        let r = page_content(&swapped, 0).unwrap().pictures[0].rect;
        assert!((r[0] - 325.0).abs() < 0.01 && (r[1] - 142.0).abs() < 0.01 && (r[2] - 50.0).abs() < 0.01 && (r[3] - 50.0).abs() < 0.01, "{r:?}");
        let doc = Document::load_mem(&swapped).unwrap();
        assert!(doc.objects.values().any(|o| o.as_stream().is_ok_and(|s| s.dict.has(b"SMask"))));
        // The page still draws.
        assert!(crate::pdf::PdfDocument::from_bytes(std::sync::Arc::new(swapped), "").is_ok());
    }

    /// A real PDF (`JAYE_TEST_PDF`, e.g. one Word exported): what's found, and a line changed and drawn
    /// to `JAYE_TEST_OUT` (a PNG) when set.
    #[test]
    #[ignore]
    fn real_pdf() {
        let Ok(path) = std::env::var("JAYE_TEST_PDF") else { return };
        let pdf = std::fs::read(path).unwrap();
        let c = page_content(&pdf, 0).unwrap();
        for l in &c.lines {
            println!("line {} {:?} {:?} {} {}", l.index, l.text, l.rect.map(|v| v.round()), l.size, l.font);
        }
        println!("pictures {:?}", c.pictures);
        println!("blanks {:?}", c.blanks);
        let Ok(text) = std::env::var("JAYE_TEST_TEXT") else { return };
        let (line, text) = text.split_once('=').unwrap();
        let changed = apply(&pdf, &ContentOp::ReplaceText { page: 0, line: line.parse().unwrap(), text: text.into() }).unwrap();
        let changed = if c.pictures.is_empty() { changed } else { apply(&changed, &ContentOp::MoveImage { page: 0, index: 0, rect: [300.0, 60.0, c.pictures[0].rect[2] * 2.0, c.pictures[0].rect[3] * 2.0] }).unwrap() };
        println!("after: {:?}", page_content(&changed, 0).unwrap().lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
        if let Ok(out) = std::env::var("JAYE_TEST_OUT") {
            std::fs::write(format!("{out}.pdf"), &changed).unwrap();
            let doc = crate::pdf::PdfDocument::from_bytes(std::sync::Arc::new(changed), "").unwrap();
            let b = doc.render(0, 900, false).recv().unwrap().unwrap();
            image::RgbaImage::from_raw(b.width, b.height, b.rgba).unwrap().save(out).unwrap();
        }
    }
}
