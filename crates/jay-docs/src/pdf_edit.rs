//! Editing PDFs (stage 7), with lopdf: the pages (order, rotation, deleting, inserting another PDF,
//! taking some out to a new file), annotations (highlight, drawing, note, text box) and form fields.
//!
//! Every change takes the file's bytes and gives new bytes; the viewer redraws from them, and saving
//! writes them. Annotations carry their own appearance (`/AP`), so every viewer draws them the same,
//! hayro (the reader, pdf.rs) included. Coordinates from the page are points from the top-left
//! corner of the page *as shown* (after its `/Rotate`); they become PDF user space here.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EditError {
    /// Password-protected files are only read.
    Encrypted,
    /// Every page can't go.
    Empty,
    Invalid { message: String },
}

fn invalid(e: impl std::fmt::Display) -> EditError {
    EditError::Invalid { message: e.to_string() }
}

/// One change, as the page sends it. Pages are indexes in the current order.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Turns pages by a multiple of 90° (clockwise when positive).
    Rotate { pages: Vec<usize>, by: i32 },
    Delete { pages: Vec<usize> },
    /// The pages in a new order: `order[i]` is the current index of the page that goes at `i`.
    Order { order: Vec<usize> },
    /// Rectangles `[x, y, w, h]` (usually the lines of selected text).
    Highlight { page: usize, rects: Vec<[f32; 4]>, color: String },
    /// Freehand lines.
    Ink { page: usize, strokes: Vec<Vec<[f32; 2]>>, color: String, width: f32 },
    /// A sticky note at a point.
    Note { page: usize, at: [f32; 2], text: String, color: String },
    /// Text written on the page.
    Text { page: usize, at: [f32; 2], text: String, size: f32, color: String },
    /// Removes the `index`th entry of the page's annotations (`annotations` lists them).
    RemoveAnnot { page: usize, index: usize },
    /// A form field's new value: text, "true"/"false" for a checkbox, the chosen state for radios.
    Fill { name: String, value: String },
}

/// Where the editor can draw and remove an annotation.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AnnotInfo {
    pub index: usize,
    pub subtype: String,
    /// `[x, y, w, h]` on the shown page.
    pub rect: [f32; 4],
    pub contents: String,
}

/// A form field and where its boxes are.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Field {
    pub name: String,
    /// "text", "check", "radio" or "choice".
    pub kind: String,
    pub value: String,
    pub multiline: bool,
    pub options: Vec<String>,
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Widget {
    pub page: usize,
    pub rect: [f32; 4],
    /// For checkboxes and radios: the name of this box's "on" state.
    pub on: Option<String>,
}

// ---------------------------------------------------------------- loading and saving

fn load(bytes: &[u8]) -> Result<Document, EditError> {
    let doc = Document::load_mem(bytes).map_err(invalid)?;
    if doc.is_encrypted() || doc.trailer.has(b"Encrypt") {
        return Err(EditError::Encrypted);
    }
    Ok(doc)
}

fn save(doc: &mut Document) -> Result<Vec<u8>, EditError> {
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(invalid)?;
    Ok(out)
}

/// What a value is, through references.
fn deref<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    match obj {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(&Object::Null),
        o => o,
    }
}

fn num(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn name(obj: &Object) -> Option<String> {
    match obj {
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

fn text(doc: &Document, obj: &Object) -> String {
    match deref(doc, obj) {
        o @ Object::String(..) => lopdf::decode_text_string(o).unwrap_or_default(),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => String::new(),
    }
}

/// The root of the page tree.
fn pages_root(doc: &Document) -> Result<ObjectId, EditError> {
    match doc.catalog().map_err(invalid)?.get(b"Pages").map_err(invalid)? {
        Object::Reference(id) => Ok(*id),
        _ => Err(invalid("no page tree")),
    }
}

/// The pages, in order, each holding what it inherited (size, resources, rotation) itself, all
/// children of the root: then reordering and deleting are just the root's `/Kids`.
fn flatten(doc: &mut Document) -> Result<Vec<ObjectId>, EditError> {
    let root = pages_root(doc)?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for &page in &pages {
        let mut inherited: Vec<(&[u8], Object)> = vec![];
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            let own = doc.get_dictionary(page).map_err(invalid)?.has(key);
            if own {
                continue;
            }
            // Up the tree until someone says it.
            let mut at = doc.get_dictionary(page).map_err(invalid)?.get(b"Parent").ok().and_then(|p| if let Object::Reference(id) = p { Some(*id) } else { None });
            let mut depth = 0;
            while let Some(id) = at {
                let Ok(dict) = doc.get_dictionary(id) else { break };
                if let Ok(v) = dict.get(key) {
                    inherited.push((key, v.clone()));
                    break;
                }
                at = dict.get(b"Parent").ok().and_then(|p| if let Object::Reference(id) = p { Some(*id) } else { None });
                depth += 1;
                if depth > 64 {
                    break;
                }
            }
        }
        let dict = doc.get_dictionary_mut(page).map_err(invalid)?;
        for (key, value) in inherited {
            dict.set(key, value);
        }
        dict.set("Parent", Object::Reference(root));
    }
    set_kids(doc, &pages)?;
    Ok(pages)
}

fn set_kids(doc: &mut Document, pages: &[ObjectId]) -> Result<(), EditError> {
    let root = pages_root(doc)?;
    let dict = doc.get_dictionary_mut(root).map_err(invalid)?;
    dict.set("Kids", Object::Array(pages.iter().map(|&p| Object::Reference(p)).collect()));
    dict.set("Count", Object::Integer(pages.len() as i64));
    Ok(())
}

fn page_id(pages: &[ObjectId], index: usize) -> Result<ObjectId, EditError> {
    pages.get(index).copied().ok_or_else(|| invalid("no such page"))
}

// ---------------------------------------------------------------- page geometry

/// A page's visible box and rotation, to go between the page as shown and user space.
#[derive(Debug, Clone, Copy)]
struct Geo {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    rot: i32,
}

fn rect_of(doc: &Document, obj: &Object) -> Option<[f32; 4]> {
    let Object::Array(a) = deref(doc, obj) else { return None };
    let v: Vec<f32> = a.iter().filter_map(|o| num(deref(doc, o))).collect();
    (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn geo(doc: &Document, page: ObjectId) -> Geo {
    let dict = doc.get_dictionary(page).ok();
    let media = dict.and_then(|d| d.get(b"MediaBox").ok()).and_then(|o| rect_of(doc, o)).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let crop = dict.and_then(|d| d.get(b"CropBox").ok()).and_then(|o| rect_of(doc, o)).unwrap_or(media);
    let rot = dict.and_then(|d| d.get(b"Rotate").ok()).and_then(|o| num(deref(doc, o))).unwrap_or(0.0) as i32;
    Geo { x0: crop[0], y0: crop[1], x1: crop[2], y1: crop[3], rot: rot.rem_euclid(360) / 90 * 90 }
}

impl Geo {
    /// From the shown page (top-left origin, after rotation) to user space.
    fn to_user(&self, [dx, dy]: [f32; 2]) -> [f32; 2] {
        match self.rot {
            90 => [self.x0 + dy, self.y0 + dx],
            180 => [self.x1 - dx, self.y0 + dy],
            270 => [self.x1 - dy, self.y1 - dx],
            _ => [self.x0 + dx, self.y1 - dy],
        }
    }

    fn from_user(&self, [ux, uy]: [f32; 2]) -> [f32; 2] {
        match self.rot {
            90 => [uy - self.y0, ux - self.x0],
            180 => [self.x1 - ux, uy - self.y0],
            270 => [self.y1 - uy, self.x1 - ux],
            _ => [ux - self.x0, self.y1 - uy],
        }
    }

    /// A user-space rectangle `[x0, y0, x1, y1]` as `[x, y, w, h]` on the shown page.
    /// The `cm` operands that draw in shown coordinates (top-left origin, y down).
    fn shown_matrix(&self) -> [f32; 6] {
        match self.rot {
            90 => [0.0, 1.0, 1.0, 0.0, self.x0, self.y0],
            180 => [-1.0, 0.0, 0.0, 1.0, self.x1, self.y0],
            270 => [0.0, -1.0, -1.0, 0.0, self.x1, self.y1],
            _ => [1.0, 0.0, 0.0, -1.0, self.x0, self.y1],
        }
    }

    /// A box `[x, y, w, h]` on the shown page as a user-space rectangle.
    fn user_rect(&self, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
        bbox(&[self.to_user([x, y]), self.to_user([x + w, y + h])], 0.0)
    }

    fn shown(&self, r: [f32; 4]) -> [f32; 4] {
        let a = self.from_user([r[0], r[1]]);
        let b = self.from_user([r[2], r[3]]);
        [a[0].min(b[0]), a[1].min(b[1]), (a[0] - b[0]).abs(), (a[1] - b[1]).abs()]
    }
}

fn bbox(points: &[[f32; 2]], pad: f32) -> [f32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    [x0 - pad, y0 - pad, x1 + pad, y1 + pad]
}

fn color(hex: &str) -> [f32; 3] {
    let h = hex.trim_start_matches('#');
    let c = |i: usize| h.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).unwrap_or(0) as f32 / 255.0;
    if h.len() == 6 { [c(0), c(2), c(4)] } else { [1.0, 0.85, 0.0] }
}

fn n(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn rect_obj(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|&v| Object::Real(v)).collect())
}

fn color_obj(c: [f32; 3]) -> Object {
    Object::Array(c.iter().map(|&v| Object::Real(v)).collect())
}

// ---------------------------------------------------------------- text for appearances

/// A string in PDF's WinAnsiEncoding (Latin letters, accents, common punctuation), others as '?'.
fn win_ansi(s: &str) -> Vec<u8> {
    const HIGH: [(char, u8); 27] = [
        ('€', 0x80), ('‚', 0x82), ('ƒ', 0x83), ('„', 0x84), ('…', 0x85), ('†', 0x86), ('‡', 0x87), ('ˆ', 0x88), ('‰', 0x89), ('Š', 0x8A), ('‹', 0x8B), ('Œ', 0x8C), ('Ž', 0x8E),
        ('‘', 0x91), ('’', 0x92), ('“', 0x93), ('”', 0x94), ('•', 0x95), ('–', 0x96), ('—', 0x97), ('˜', 0x98), ('™', 0x99), ('š', 0x9A), ('›', 0x9B), ('œ', 0x9C), ('ž', 0x9E), ('Ÿ', 0x9F),
    ];
    s.chars()
        .map(|c| match c as u32 {
            0x20..=0x7E | 0xA0..=0xFF => c as u8,
            _ => HIGH.iter().find(|(h, _)| *h == c).map(|(_, b)| *b).unwrap_or(b'?'),
        })
        .collect()
}

/// A PDF literal string `( … )` of WinAnsi bytes.
fn literal(s: &str) -> String {
    let mut out = String::from("(");
    for b in win_ansi(s) {
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7E => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push(')');
    out
}

/// Helvetica's widths (1/1000 em) for ASCII, near enough for others: to size text boxes.
fn text_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| if c == ' ' { 0.278 } else if c.is_ascii_uppercase() || c == 'm' || c == 'w' { 0.72 } else if "iljtf.,;:'!|".contains(c) { 0.28 } else { 0.556 }).sum::<f32>() * size
}

fn helvetica(doc: &mut Document) -> ObjectId {
    doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" })
}

/// A Form XObject drawn in user space over `bbox`.
fn appearance(doc: &mut Document, bbox: [f32; 4], resources: Dictionary, content: String) -> ObjectId {
    let dict = dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => rect_obj(bbox), "Resources" => resources };
    doc.add_object(Stream::new(dict, content.into_bytes()))
}

/// Adds an annotation to a page's `/Annots`.
fn add_annot(doc: &mut Document, page: ObjectId, mut annot: Dictionary) -> Result<(), EditError> {
    annot.set("Type", "Annot");
    annot.set("P", Object::Reference(page));
    annot.set("F", Object::Integer(4)); // printed
    annot.set("M", Object::string_literal(chrono_now()));
    let id = doc.add_object(annot);
    let existing = doc.get_dictionary(page).map_err(invalid)?.get(b"Annots").ok().cloned();
    match existing {
        Some(Object::Reference(arr)) => {
            if let Ok(Object::Array(a)) = doc.get_object_mut(arr) {
                a.push(Object::Reference(id));
            }
        }
        Some(Object::Array(mut a)) => {
            a.push(Object::Reference(id));
            doc.get_dictionary_mut(page).map_err(invalid)?.set("Annots", Object::Array(a));
        }
        _ => doc.get_dictionary_mut(page).map_err(invalid)?.set("Annots", Object::Array(vec![Object::Reference(id)])),
    }
    Ok(())
}

/// "D:YYYYMMDDHHmmSS" in UTC, for an annotation's date.
fn chrono_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

// ---------------------------------------------------------------- changes

/// The file with one change made.
pub fn apply(bytes: &[u8], op: &Op) -> Result<Vec<u8>, EditError> {
    let mut doc = load(bytes)?;
    let mut pages = flatten(&mut doc)?;
    match op {
        Op::Rotate { pages: which, by } => {
            for &i in which {
                let id = page_id(&pages, i)?;
                let now = doc.get_dictionary(id).map_err(invalid)?.get(b"Rotate").ok().and_then(|o| num(deref(&doc, o))).unwrap_or(0.0) as i32;
                doc.get_dictionary_mut(id).map_err(invalid)?.set("Rotate", Object::Integer(((now + by) / 90 * 90).rem_euclid(360) as i64));
            }
        }
        Op::Delete { pages: which } => {
            let gone: HashSet<usize> = which.iter().copied().collect();
            let keep: Vec<ObjectId> = pages.iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, &p)| p).collect();
            if keep.is_empty() {
                return Err(EditError::Empty);
            }
            set_kids(&mut doc, &keep)?;
            doc.prune_objects();
        }
        Op::Order { order } => {
            let mut seen = order.clone();
            seen.sort_unstable();
            if seen != (0..pages.len()).collect::<Vec<_>>() {
                return Err(invalid("not an order of the pages"));
            }
            pages = order.iter().map(|&i| pages[i]).collect();
            set_kids(&mut doc, &pages)?;
        }
        Op::Highlight { page, rects, color: hex } => {
            let id = page_id(&pages, *page)?;
            let g = geo(&doc, id);
            let c = color(hex);
            let mut quads = vec![];
            let mut all = vec![];
            let mut path = String::new();
            for &[x, y, w, h] in rects.iter().filter(|r| r[2] > 0.0 && r[3] > 0.0) {
                // Top-left, top-right, bottom-left, bottom-right, as readers expect.
                let corners = [g.to_user([x, y]), g.to_user([x + w, y]), g.to_user([x, y + h]), g.to_user([x + w, y + h])];
                for p in corners {
                    quads.push(Object::Real(p[0]));
                    quads.push(Object::Real(p[1]));
                    all.push(p);
                }
                let [tl, tr, bl, br] = corners;
                let _ = write!(path, "{} {} m {} {} l {} {} l {} {} l h f\n", n(tl[0]), n(tl[1]), n(tr[0]), n(tr[1]), n(br[0]), n(br[1]), n(bl[0]), n(bl[1]));
            }
            if all.is_empty() {
                return Err(invalid("nothing to highlight"));
            }
            let rect = bbox(&all, 0.0);
            let gs = doc.add_object(dictionary! { "Type" => "ExtGState", "BM" => "Multiply" });
            let ap = appearance(&mut doc, rect, dictionary! { "ExtGState" => dictionary! { "GS0" => Object::Reference(gs) } }, format!("/GS0 gs {} {} {} rg\n{path}", n(c[0]), n(c[1]), n(c[2])));
            add_annot(&mut doc, id, dictionary! { "Subtype" => "Highlight", "Rect" => rect_obj(rect), "QuadPoints" => Object::Array(quads), "C" => color_obj(c), "AP" => dictionary! { "N" => Object::Reference(ap) } })?;
        }
        Op::Ink { page, strokes, color: hex, width } => {
            let id = page_id(&pages, *page)?;
            let g = geo(&doc, id);
            let c = color(hex);
            let w = width.clamp(0.25, 50.0);
            let user: Vec<Vec<[f32; 2]>> = strokes.iter().filter(|s| !s.is_empty()).take(500).map(|s| s.iter().take(5000).map(|&p| g.to_user(p)).collect()).collect();
            let all: Vec<[f32; 2]> = user.iter().flatten().copied().collect();
            if all.is_empty() {
                return Err(invalid("nothing drawn"));
            }
            let rect = bbox(&all, w);
            let mut content = format!("{} {} {} RG {} w 1 J 1 j\n", n(c[0]), n(c[1]), n(c[2]), n(w));
            for s in &user {
                let _ = write!(content, "{} {} m ", n(s[0][0]), n(s[0][1]));
                for p in &s[1..] {
                    let _ = write!(content, "{} {} l ", n(p[0]), n(p[1]));
                }
                // A dot is a line to itself (round caps draw it).
                if s.len() == 1 {
                    let _ = write!(content, "{} {} l ", n(s[0][0]), n(s[0][1]));
                }
                content.push_str("S\n");
            }
            let ap = appearance(&mut doc, rect, Dictionary::new(), content);
            let ink = Object::Array(user.iter().map(|s| Object::Array(s.iter().flat_map(|p| [Object::Real(p[0]), Object::Real(p[1])]).collect())).collect());
            add_annot(&mut doc, id, dictionary! { "Subtype" => "Ink", "Rect" => rect_obj(rect), "InkList" => ink, "C" => color_obj(c), "BS" => dictionary! { "W" => Object::Real(w) }, "AP" => dictionary! { "N" => Object::Reference(ap) } })?;
        }
        Op::Note { page, at, text: body, color: hex } => {
            let id = page_id(&pages, *page)?;
            let g = geo(&doc, id);
            let c = color(hex);
            let rect = g.user_rect([at[0], at[1], 20.0, 20.0]);
            // A note: a square with a folded corner and lines, in the note's color.
            let [x, y] = [rect[0], rect[1]];
            let content = format!(
                "{} {} {} rg 0.25 0.25 0.25 RG 0.8 w {} {} m {} {} l {} {} l {} {} l {} {} l h B {} {} m {} {} l {} {} l S {} {} m {} {} l {} {} m {} {} l {} {} m {} {} l S",
                n(c[0]), n(c[1]), n(c[2]),
                n(x + 1.0), n(y + 1.0), n(x + 19.0), n(y + 1.0), n(x + 19.0), n(y + 14.0), n(x + 14.0), n(y + 19.0), n(x + 1.0), n(y + 19.0),
                n(x + 14.0), n(y + 19.0), n(x + 14.0), n(y + 14.0), n(x + 19.0), n(y + 14.0),
                n(x + 4.0), n(y + 12.0), n(x + 12.0), n(y + 12.0), n(x + 4.0), n(y + 8.5), n(x + 15.0), n(y + 8.5), n(x + 4.0), n(y + 5.0), n(x + 15.0), n(y + 5.0)
            );
            let ap = appearance(&mut doc, rect, Dictionary::new(), content);
            add_annot(
                &mut doc,
                id,
                dictionary! { "Subtype" => "Text", "Rect" => rect_obj(rect), "Contents" => lopdf::text_string(body), "Name" => "Comment", "C" => color_obj(c), "Open" => false, "AP" => dictionary! { "N" => Object::Reference(ap) } },
            )?;
        }
        Op::Text { page, at, text: body, size, color: hex } => {
            let id = page_id(&pages, *page)?;
            let g = geo(&doc, id);
            let c = color(hex);
            let size = size.clamp(4.0, 144.0);
            let lines: Vec<&str> = body.lines().take(200).collect();
            let lead = size * 1.2;
            let width = lines.iter().map(|l| text_width(l, size)).fold(0.0, f32::max) + 4.0;
            let height = lead * lines.len().max(1) as f32 + 4.0;
            let rect = g.user_rect([at[0], at[1], width, height]);
            let font = helvetica(&mut doc);
            // Drawn in the shown page's coordinates, so it reads upright on a turned page too.
            let m = g.shown_matrix();
            let mut content = format!("q {} {} {} {} {} {} cm BT /Helv {} Tf {} {} {} rg\n", n(m[0]), n(m[1]), n(m[2]), n(m[3]), n(m[4]), n(m[5]), n(size), n(c[0]), n(c[1]), n(c[2]));
            for (i, line) in lines.iter().enumerate() {
                let _ = writeln!(content, "1 0 0 -1 {} {} Tm {} Tj", n(at[0] + 2.0), n(at[1] + 2.0 + size * 0.8 + lead * i as f32), literal(line));
            }
            content.push_str("ET Q");
            let ap = appearance(&mut doc, rect, dictionary! { "Font" => dictionary! { "Helv" => Object::Reference(font) } }, content);
            let da = format!("/Helv {} Tf {} {} {} rg", n(size), n(c[0]), n(c[1]), n(c[2]));
            add_annot(
                &mut doc,
                id,
                dictionary! { "Subtype" => "FreeText", "Rect" => rect_obj(rect), "Contents" => lopdf::text_string(body), "DA" => Object::string_literal(da), "BS" => dictionary! { "W" => 0 }, "AP" => dictionary! { "N" => Object::Reference(ap) } },
            )?;
        }
        Op::RemoveAnnot { page, index } => {
            let id = page_id(&pages, *page)?;
            let annots = doc.get_dictionary(id).map_err(invalid)?.get(b"Annots").ok().cloned();
            match annots {
                Some(Object::Reference(arr)) => {
                    if let Ok(Object::Array(a)) = doc.get_object_mut(arr) {
                        if *index < a.len() {
                            a.remove(*index);
                        }
                    }
                }
                Some(Object::Array(mut a)) if *index < a.len() => {
                    a.remove(*index);
                    doc.get_dictionary_mut(id).map_err(invalid)?.set("Annots", Object::Array(a));
                }
                _ => return Err(invalid("no such annotation")),
            }
        }
        Op::Fill { name, value } => fill(&mut doc, &pages, name, value)?,
    }
    save(&mut doc)
}

/// The file with all of another PDF's pages put in at `at`.
pub fn insert(bytes: &[u8], other: &[u8], at: usize) -> Result<Vec<u8>, EditError> {
    let mut doc = load(bytes)?;
    let mut pages = flatten(&mut doc)?;
    let mut more = load(other)?;
    flatten(&mut more)?;
    more.renumber_objects_with(doc.max_id + 1);
    let skip: HashSet<ObjectId> = [more.trailer.get(b"Root").ok().and_then(|r| if let Object::Reference(id) = r { Some(*id) } else { None }), pages_root(&more).ok()].into_iter().flatten().collect();
    let new_pages: Vec<ObjectId> = more.get_pages().into_values().collect();
    let root = pages_root(&doc)?;
    for (id, obj) in std::mem::take(&mut more.objects) {
        if !skip.contains(&id) {
            doc.objects.insert(id, obj);
        }
    }
    doc.max_id = doc.max_id.max(more.max_id);
    for &p in &new_pages {
        let dict = doc.get_dictionary_mut(p).map_err(invalid)?;
        dict.set("Parent", Object::Reference(root));
        // Its form fields belonged to the other file's form: the boxes stay as drawn.
        if let Ok(Object::Array(annots)) = dict.get(b"Annots").cloned() {
            let _ = annots;
        }
    }
    let at = at.min(pages.len());
    pages.splice(at..at, new_pages);
    set_kids(&mut doc, &pages)?;
    save(&mut doc)
}

/// A new file with only these pages, in this order.
pub fn extract(bytes: &[u8], which: &[usize]) -> Result<Vec<u8>, EditError> {
    let mut doc = load(bytes)?;
    let pages = flatten(&mut doc)?;
    let keep = which.iter().map(|&i| page_id(&pages, i)).collect::<Result<Vec<_>, _>>()?;
    if keep.is_empty() {
        return Err(EditError::Empty);
    }
    set_kids(&mut doc, &keep)?;
    // Outlines and the form point at pages that may be gone.
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"Outlines");
        cat.remove(b"AcroForm");
        cat.remove(b"StructTreeRoot");
    }
    doc.prune_objects();
    save(&mut doc)
}

// ---------------------------------------------------------------- reading annotations and fields

/// Where each page's annotations are (not links, form boxes or pop-ups), page by page.
pub fn annotations(bytes: &[u8]) -> Result<Vec<Vec<AnnotInfo>>, EditError> {
    let doc = Document::load_mem(bytes).map_err(invalid)?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    Ok(pages
        .iter()
        .map(|&p| {
            let g = geo(&doc, p);
            let Some(Object::Array(list)) = doc.get_dictionary(p).ok().and_then(|d| d.get(b"Annots").ok()).map(|o| deref(&doc, o)) else { return vec![] };
            list.iter()
                .enumerate()
                .filter_map(|(index, a)| {
                    let Object::Dictionary(d) = deref(&doc, a) else { return None };
                    let subtype = d.get(b"Subtype").ok().and_then(name)?;
                    if matches!(subtype.as_str(), "Link" | "Widget" | "Popup") {
                        return None;
                    }
                    let rect = rect_of(&doc, d.get(b"Rect").ok()?)?;
                    Some(AnnotInfo { index, subtype, rect: g.shown(rect), contents: d.get(b"Contents").map(|c| text(&doc, c)).unwrap_or_default() })
                })
                .collect()
        })
        .collect())
}

/// The form's fields that can be filled (text, checkboxes, radios, lists), with their boxes.
pub fn fields(bytes: &[u8]) -> Result<Vec<Field>, EditError> {
    let doc = Document::load_mem(bytes).map_err(invalid)?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let mut on_page: HashMap<ObjectId, usize> = HashMap::new();
    for (i, &p) in pages.iter().enumerate() {
        if let Some(Object::Array(list)) = doc.get_dictionary(p).ok().and_then(|d| d.get(b"Annots").ok()).map(|o| deref(&doc, o)) {
            for a in list {
                if let Object::Reference(id) = a {
                    on_page.insert(*id, i);
                }
            }
        }
    }
    let mut out = vec![];
    for (id, field) in terminal_fields(&doc) {
        let d = match doc.get_dictionary(id) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let ft = inherited(&doc, id, b"FT").and_then(|o| name(&o)).unwrap_or_default();
        let flags = inherited(&doc, id, b"Ff").and_then(|o| num(&o)).unwrap_or(0.0) as u32;
        let kind = match ft.as_str() {
            "Tx" => "text",
            "Btn" if flags & (1 << 16) != 0 => continue, // push button
            "Btn" if flags & (1 << 15) != 0 => "radio",
            "Btn" => "check",
            "Ch" => "choice",
            _ => continue,
        };
        let value = inherited(&doc, id, b"V").map(|v| text(&doc, &v)).unwrap_or_default();
        let options = match d.get(b"Opt").ok().map(|o| deref(&doc, o)) {
            Some(Object::Array(a)) => a
                .iter()
                .map(|o| match deref(&doc, o) {
                    Object::Array(pair) => pair.get(1).map(|v| text(&doc, v)).unwrap_or_default(),
                    other => text(&doc, other),
                })
                .collect(),
            _ => vec![],
        };
        let widgets: Vec<ObjectId> = match d.get(b"Kids").ok().map(|o| deref(&doc, o)) {
            Some(Object::Array(kids)) => kids.iter().filter_map(|k| if let Object::Reference(r) = k { Some(*r) } else { None }).collect(),
            _ => vec![id],
        };
        let widgets = widgets
            .into_iter()
            .filter_map(|w| {
                let wd = doc.get_dictionary(w).ok()?;
                let page = on_page.get(&w).copied()?;
                let rect = rect_of(&doc, wd.get(b"Rect").ok()?)?;
                let on = on_state(&doc, wd);
                Some(Widget { page, rect: geo(&doc, pages[page]).shown(rect), on })
            })
            .collect::<Vec<_>>();
        if widgets.is_empty() {
            continue;
        }
        out.push(Field { name: field, kind: kind.into(), value, multiline: kind == "text" && flags & (1 << 12) != 0, options, widgets });
    }
    Ok(out)
}

/// Field ids with no child fields (only widgets, or none), with their full names ("a.b.c").
fn terminal_fields(doc: &Document) -> Vec<(ObjectId, String)> {
    let mut out = vec![];
    let Some(Object::Array(roots)) = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).and_then(|f| match deref(doc, f) {
        Object::Dictionary(d) => d.get(b"Fields").ok().map(|o| deref(doc, o)),
        _ => None,
    }) else {
        return out;
    };
    let mut stack: Vec<(ObjectId, String, usize)> = roots.iter().rev().filter_map(|r| if let Object::Reference(id) = r { Some((*id, String::new(), 0)) } else { None }).collect();
    let mut seen = HashSet::new();
    while let Some((id, prefix, depth)) = stack.pop() {
        if depth > 32 || !seen.insert(id) {
            continue;
        }
        let Ok(d) = doc.get_dictionary(id) else { continue };
        let own = d.get(b"T").map(|t| text(doc, t)).unwrap_or_default();
        let full = match (prefix.is_empty(), own.is_empty()) {
            (_, true) => prefix.clone(),
            (true, false) => own,
            (false, false) => format!("{prefix}.{own}"),
        };
        let kids: Vec<ObjectId> = match d.get(b"Kids").ok().map(|o| deref(doc, o)) {
            Some(Object::Array(k)) => k.iter().filter_map(|r| if let Object::Reference(id) = r { Some(*id) } else { None }).collect(),
            _ => vec![],
        };
        // Kids that have names are fields; nameless kids are this field's boxes.
        let child_fields: Vec<ObjectId> = kids.iter().copied().filter(|k| doc.get_dictionary(*k).is_ok_and(|kd| kd.has(b"T"))).collect();
        if child_fields.is_empty() {
            out.push((id, full));
        } else {
            for k in child_fields.into_iter().rev() {
                stack.push((k, full.clone(), depth + 1));
            }
        }
    }
    out
}

/// A field value, from the field or the fields above it.
fn inherited(doc: &Document, mut id: ObjectId, key: &[u8]) -> Option<Object> {
    for _ in 0..32 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(deref(doc, v).clone());
        }
        id = match d.get(b"Parent").ok()? {
            Object::Reference(p) => *p,
            _ => return None,
        };
    }
    None
}

/// The name of a checkbox or radio box's "on" appearance (anything but /Off).
fn on_state(doc: &Document, widget: &Dictionary) -> Option<String> {
    let Object::Dictionary(ap) = deref(doc, widget.get(b"AP").ok()?) else { return None };
    let Object::Dictionary(normal) = deref(doc, ap.get(b"N").ok()?) else { return None };
    normal.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).find(|k| k != "Off")
}

fn fill(doc: &mut Document, pages: &[ObjectId], target: &str, value: &str) -> Result<(), EditError> {
    let _ = pages;
    let (id, _) = terminal_fields(doc).into_iter().find(|(_, n)| n == target).ok_or_else(|| invalid("no such field"))?;
    let ft = inherited(doc, id, b"FT").and_then(|o| name(&o)).unwrap_or_default();
    let flags = inherited(doc, id, b"Ff").and_then(|o| num(&o)).unwrap_or(0.0) as u32;
    let widgets: Vec<ObjectId> = match doc.get_dictionary(id).map_err(invalid)?.get(b"Kids").ok().cloned() {
        Some(Object::Array(kids)) => kids.iter().filter_map(|k| if let Object::Reference(r) = k { Some(*r) } else { None }).collect(),
        _ => vec![id],
    };
    match ft.as_str() {
        "Btn" => {
            // Checkbox: "true"/"false"; radio: the state of the chosen box.
            let radio = flags & (1 << 15) != 0;
            let mut chosen = None;
            for &w in &widgets {
                let on = doc.get_dictionary(w).ok().and_then(|d| on_state(doc, d));
                let pick = match (&on, radio) {
                    (Some(state), true) => state == value,
                    (Some(_), false) => value == "true",
                    _ => false,
                };
                let state = if pick { on.clone().unwrap_or_else(|| "Off".into()) } else { "Off".into() };
                if pick {
                    chosen = on.clone();
                }
                doc.get_dictionary_mut(w).map_err(invalid)?.set("AS", Object::Name(state.into_bytes()));
            }
            doc.get_dictionary_mut(id).map_err(invalid)?.set("V", Object::Name(chosen.unwrap_or_else(|| "Off".into()).into_bytes()));
        }
        "Tx" | "Ch" => {
            doc.get_dictionary_mut(id).map_err(invalid)?.set("V", lopdf::text_string(value));
            // The box shows the new text: a fresh appearance with the field's font and size.
            let da = inherited(doc, id, b"DA").map(|d| text(doc, &d)).or_else(|| {
                doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).and_then(|f| match deref(doc, f) {
                    Object::Dictionary(d) => d.get(b"DA").ok().map(|v| text(doc, v)),
                    _ => None,
                })
            });
            let (font, size, rest) = parse_da(da.as_deref().unwrap_or("/Helv 0 Tf 0 g"));
            let dr = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).and_then(|f| match deref(doc, f) {
                Object::Dictionary(d) => d.get(b"DR").ok().map(|v| deref(doc, v).clone()),
                _ => None,
            });
            let has_font = matches!(&dr, Some(Object::Dictionary(d)) if matches!(d.get(b"Font").ok().map(|f| deref(doc, f)), Some(Object::Dictionary(fd)) if fd.has(font.as_bytes())));
            let resources = match (dr, has_font) {
                (Some(Object::Dictionary(d)), true) => d,
                _ => {
                    let f = helvetica(doc);
                    dictionary! { "Font" => dictionary! { font.as_str() => Object::Reference(f) } }
                }
            };
            let multiline = flags & (1 << 12) != 0;
            for &w in &widgets {
                let Some(rect) = doc.get_dictionary(w).ok().and_then(|d| d.get(b"Rect").ok().cloned()).and_then(|r| rect_of(doc, &r)) else { continue };
                let (bw, bh) = (rect[2] - rect[0], rect[3] - rect[1]);
                let size = if size > 0.0 { size } else { (bh * 0.6).clamp(4.0, 12.0) };
                let mut content = format!("/Tx BMC q 1 1 {} {} re W n BT /{font} {} Tf {rest} ", n(bw - 2.0), n(bh - 2.0), n(size));
                if multiline {
                    let _ = write!(content, "2 {} Td {} TL ", n(bh - 2.0 - size), n(size * 1.15));
                    for (i, line) in value.lines().enumerate() {
                        let _ = write!(content, "{}{} Tj ", if i > 0 { "T* " } else { "" }, literal(line));
                    }
                } else {
                    let _ = write!(content, "2 {} Td {} Tj ", n((bh - size) / 2.0 + size * 0.22), literal(value));
                }
                content.push_str("ET Q EMC");
                let ap = appearance(doc, [0.0, 0.0, bw, bh], resources.clone(), content);
                doc.get_dictionary_mut(w).map_err(invalid)?.set("AP", dictionary! { "N" => Object::Reference(ap) });
            }
            // Other readers redraw it too.
            let form = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok().cloned());
            match form {
                Some(Object::Reference(f)) => {
                    if let Ok(d) = doc.get_dictionary_mut(f) {
                        d.set("NeedAppearances", true);
                    }
                }
                Some(Object::Dictionary(mut d)) => {
                    d.set("NeedAppearances", true);
                    doc.catalog_mut().map_err(invalid)?.set("AcroForm", Object::Dictionary(d));
                }
                _ => {}
            }
        }
        _ => return Err(invalid("this field can't be filled")),
    }
    Ok(())
}

/// A field's default appearance: font name, size (0 = fit) and the color operators.
fn parse_da(da: &str) -> (String, f32, String) {
    let parts: Vec<&str> = da.split_whitespace().collect();
    let tf = parts.iter().position(|p| *p == "Tf");
    let (font, size) = match tf {
        Some(i) if i >= 2 => (parts[i - 2].trim_start_matches('/').to_owned(), parts[i - 1].parse().unwrap_or(0.0)),
        _ => ("Helv".to_owned(), 0.0),
    };
    let font: String = font.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    // The color: what's after Tf, if it's only numbers and g/rg/k.
    let rest = tf.map(|i| parts[i + 1..].join(" ")).unwrap_or_default();
    let rest = if rest.split_whitespace().all(|p| p.parse::<f32>().is_ok() || matches!(p, "g" | "rg" | "k")) { rest } else { "0 g".into() };
    (if font.is_empty() { "Helv".into() } else { font }, size, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small PDF: `n` pages, each saying its number, the second turned 90°; a text field and a
    /// checkbox on the first.
    fn sample(n: usize) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let mut kids = vec![];
        let mut first = None;
        for i in 0..n {
            let content = doc.add_object(Stream::new(Dictionary::new(), format!("BT /F1 24 Tf 72 700 Td (Page {}) Tj ET", i + 1).into_bytes()));
            let mut page = dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content };
            if i == 1 {
                page.set("Rotate", 90);
            }
            let id = doc.add_object(page);
            first.get_or_insert(id);
            kids.push(Object::Reference(id));
        }
        // Size and fonts inherited from the tree, as many files do.
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } }),
        );
        let on = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] }, b"0 0 12 12 re f".to_vec()));
        let off = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] }, Vec::new()));
        let name = doc.add_object(dictionary! { "FT" => "Tx", "T" => Object::string_literal("name"), "Type" => "Annot", "Subtype" => "Widget", "Rect" => vec![100.into(), 600.into(), 300.into(), 620.into()], "DA" => Object::string_literal("/Helv 12 Tf 0 g") });
        let check = doc.add_object(dictionary! { "FT" => "Btn", "T" => Object::string_literal("agree"), "Type" => "Annot", "Subtype" => "Widget", "Rect" => vec![100.into(), 560.into(), 112.into(), 572.into()], "V" => "Off", "AS" => "Off", "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } } });
        doc.get_dictionary_mut(first.unwrap()).unwrap().set("Annots", vec![Object::Reference(name), Object::Reference(check)]);
        let helv = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let form = dictionary! { "Fields" => vec![Object::Reference(name), Object::Reference(check)], "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv } } };
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => form });
        doc.trailer.set("Root", catalog);
        let mut out = vec![];
        doc.save_to(&mut out).unwrap();
        out
    }

    fn texts(bytes: &[u8]) -> Vec<String> {
        let doc = Document::load_mem(bytes).unwrap();
        doc.get_pages().into_keys().map(|n| doc.extract_text(&[n]).unwrap_or_default().trim().to_owned()).collect()
    }

    fn rotation(bytes: &[u8], i: usize) -> i64 {
        let doc = Document::load_mem(bytes).unwrap();
        let id = doc.get_pages().into_values().nth(i).unwrap();
        match doc.get_dictionary(id).unwrap().get(b"Rotate") {
            Ok(Object::Integer(r)) => *r,
            _ => 0,
        }
    }

    #[test]
    fn pages_reorder_rotate_delete_insert_and_extract() {
        let pdf = sample(3);
        assert_eq!(texts(&pdf), ["Page 1", "Page 2", "Page 3"]);
        let moved = apply(&pdf, &Op::Order { order: vec![2, 0, 1] }).unwrap();
        assert_eq!(texts(&moved), ["Page 3", "Page 1", "Page 2"]);
        assert!(apply(&pdf, &Op::Order { order: vec![0, 0, 1] }).is_err());
        let turned = apply(&moved, &Op::Rotate { pages: vec![0, 2], by: -90 }).unwrap();
        assert_eq!((rotation(&turned, 0), rotation(&turned, 2)), (270, 0), "page 2 was at 90");
        let fewer = apply(&turned, &Op::Delete { pages: vec![1] }).unwrap();
        assert_eq!(texts(&fewer), ["Page 3", "Page 2"]);
        assert_eq!(apply(&fewer, &Op::Delete { pages: vec![0, 1] }), Err(EditError::Empty));
        let merged = insert(&fewer, &sample(2), 1).unwrap();
        assert_eq!(texts(&merged), ["Page 3", "Page 1", "Page 2", "Page 2"]);
        let out = extract(&merged, &[3, 0]).unwrap();
        assert_eq!(texts(&out), ["Page 2", "Page 3"]);
        // hayro reads every result.
        for bytes in [&moved, &turned, &fewer, &merged, &out] {
            let doc = crate::pdf::PdfDocument::from_bytes(std::sync::Arc::new(bytes.to_vec()), "").unwrap();
            assert!(doc.render(0, 200, true).recv().unwrap().is_some());
        }
    }

    #[test]
    fn rotated_pages_map_both_ways() {
        let g = |rot| Geo { x0: 0.0, y0: 0.0, x1: 612.0, y1: 792.0, rot };
        for rot in [0, 90, 180, 270] {
            for p in [[10.0, 20.0], [300.0, 5.0]] {
                let back = g(rot).from_user(g(rot).to_user(p));
                assert!((back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3, "{rot} {p:?} {back:?}");
            }
        }
        // Not turned: the top-left corner shown is the top of user space.
        assert_eq!(g(0).to_user([0.0, 0.0]), [0.0, 792.0]);
        // Turned 90° clockwise: user space's bottom-left is shown at the top-left.
        assert_eq!(g(90).to_user([0.0, 0.0]), [0.0, 0.0]);
    }

    #[test]
    fn annotations_are_added_drawn_and_removed() {
        let pdf = sample(2);
        let a = apply(&pdf, &Op::Highlight { page: 0, rects: vec![[72.0, 70.0, 100.0, 24.0]], color: "#ffd400".into() }).unwrap();
        let b = apply(&a, &Op::Ink { page: 0, strokes: vec![vec![[100.0, 300.0], [150.0, 320.0], [200.0, 300.0]]], color: "#e53935".into(), width: 2.0 }).unwrap();
        let c = apply(&b, &Op::Note { page: 1, at: [50.0, 50.0], text: "Revisar isto".into(), color: "#ffeb3b".into() }).unwrap();
        let d = apply(&c, &Op::Text { page: 0, at: [72.0, 400.0], text: "Olá, ação (teste)\nlinha 2".into(), size: 14.0, color: "#1a73e8".into() }).unwrap();
        let list = annotations(&d).unwrap();
        let kinds: Vec<&str> = list[0].iter().map(|a| a.subtype.as_str()).collect();
        assert_eq!(kinds, ["Highlight", "Ink", "FreeText"], "form widgets aren't listed");
        assert_eq!(list[1][0].subtype, "Text");
        assert_eq!(list[1][0].contents, "Revisar isto");
        // Where it was put, on the page as shown (page 1 is turned).
        let hl = list[0][0].rect;
        assert!((hl[0] - 72.0).abs() < 0.01 && (hl[1] - 70.0).abs() < 0.01 && (hl[2] - 100.0).abs() < 0.01, "{hl:?}");
        let note = list[1][0].rect;
        assert!((note[0] - 50.0).abs() < 0.01 && (note[1] - 50.0).abs() < 0.01, "{note:?}");
        // The free text's appearance has the accents in WinAnsi and the parentheses escaped.
        assert!(literal("ação (x)").contains("a\\347\\343o \\(x\\)"));
        // Drawn by hayro.
        let doc = crate::pdf::PdfDocument::from_bytes(std::sync::Arc::new(d.clone()), "").unwrap();
        assert!(doc.render(0, 300, true).recv().unwrap().is_some());
        // Removing the ink keeps the others.
        let e = apply(&d, &Op::RemoveAnnot { page: 0, index: list[0][1].index }).unwrap();
        let kinds: Vec<String> = annotations(&e).unwrap()[0].iter().map(|a| a.subtype.clone()).collect();
        assert_eq!(kinds, ["Highlight", "FreeText"]);
    }

    #[test]
    fn forms_are_listed_and_filled() {
        let pdf = sample(1);
        // JAYE_PDF_SAMPLE=<path> writes a three-page sample with the form, to try the app with.
        if let Ok(path) = std::env::var("JAYE_PDF_SAMPLE") {
            std::fs::write(path, sample(3)).unwrap();
        }
        let f = fields(&pdf).unwrap();
        assert_eq!(f.iter().map(|x| (x.name.as_str(), x.kind.as_str())).collect::<Vec<_>>(), [("name", "text"), ("agree", "check")]);
        assert_eq!(f[1].widgets[0].on.as_deref(), Some("Yes"));
        assert_eq!(f[0].widgets[0].rect, [100.0, 172.0, 200.0, 20.0]);
        let a = apply(&pdf, &Op::Fill { name: "name".into(), value: "Maria Souza".into() }).unwrap();
        let b = apply(&a, &Op::Fill { name: "agree".into(), value: "true".into() }).unwrap();
        let f = fields(&b).unwrap();
        assert_eq!((f[0].value.as_str(), f[1].value.as_str()), ("Maria Souza", "Yes"));
        let doc = Document::load_mem(&b).unwrap();
        let widget = doc.get_pages().into_values().next().map(|p| doc.get_dictionary(p).unwrap().get(b"Annots").unwrap().clone()).unwrap();
        let Object::Array(w) = widget else { panic!() };
        let Object::Reference(name_id) = w[0] else { panic!() };
        let ap = doc.get_dictionary(name_id).unwrap().get(b"AP").unwrap();
        let Object::Dictionary(ap) = ap else { panic!() };
        let Object::Reference(n) = ap.get(b"N").unwrap() else { panic!() };
        let Object::Stream(s) = doc.get_object(*n).unwrap() else { panic!() };
        assert!(String::from_utf8_lossy(&s.content).contains("(Maria Souza) Tj"));
        assert!(apply(&b, &Op::Fill { name: "nope".into(), value: "x".into() }).is_err());
        assert_eq!(parse_da("/Helv 0 Tf 0 g"), ("Helv".into(), 0.0, "0 g".into()));
    }
}
