//! RTF to the model: a small reader of the format's groups and control words. Headings come from
//! the stylesheet's names ("heading 1") or `\outlinelevel`; lists from `\ls`/`\ilvl`, numbered when
//! the text Word writes for old readers (`\listtext`) has a number; tables from `\intbl`, `\cell`,
//! `\row`. Bytes `\'hh` are Windows-1252. Not read: pictures, fonts, colors, footnotes, other code
//! pages (their text comes out as 1252).

use super::{Align, Block, DocError, Document, Inline, MAX_DEPTH, Style, invalid, merge_runs};
use std::collections::HashMap;

#[derive(Clone, Copy, Default)]
struct State {
    style: Style,
    /// Text in this group is not content (font table, picture, field instruction…).
    skip: bool,
    /// Inside `\listtext`/`\pntext`: noted to tell numbered lists from bullets, not shown.
    list_text: bool,
    /// Inside the stylesheet: style names are collected.
    stylesheet: bool,
    /// `\ucN`: how many fallback characters follow a `\uN`.
    uc: u8,
}

#[derive(Default)]
struct Para {
    align: Align,
    style: Option<i32>,
    outline: Option<u8>,
    list: Option<(i32, usize)>,
    in_table: bool,
}

#[derive(Default)]
struct Reader {
    stack: Vec<State>,
    state: State,
    para: Para,
    inlines: Vec<Inline>,
    text: String,
    /// Stylesheet: style number → name; the entry being read.
    styles: HashMap<i32, String>,
    style_entry: (Option<i32>, String),
    list_text: String,
    blocks: Vec<Block>,
    /// The list being filled: (list id, numbered, level → items).
    list: Option<ListBuild>,
    row: Vec<Vec<Block>>,
    cell: Vec<Block>,
    rows: Vec<Vec<Vec<Block>>>,
    skip_chars: u8,
    page_break: bool,
}

struct ListBuild {
    id: i32,
    /// Open levels, outermost first: (numbered, items).
    levels: Vec<(bool, Vec<Vec<Block>>)>,
}

/// Destinations whose text isn't document content.
const SKIP: [&str; 27] = [
    "fonttbl", "colortbl", "info", "pict", "header", "headerl", "headerr", "headerf", "footer", "footerl", "footerr", "footerf", "footnote", "fldinst", "listtable", "listoverridetable", "rsidtbl", "generator", "xmlnstbl", "themedata",
    "colorschememapping", "latentstyles", "datastore", "object", "shp", "nonshppict", "bkmkstart",
];

pub fn read(bytes: &[u8]) -> Result<Document, DocError> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(invalid("not RTF"));
    }
    let mut r = Reader { state: State { uc: 1, ..Default::default() }, ..Default::default() };
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'{' => {
                if r.stack.len() >= MAX_DEPTH * 4 {
                    return Err(DocError::TooBig);
                }
                // Text so far keeps the style it was typed in.
                r.flush_text();
                r.stack.push(r.state);
                i += 1;
            }
            b'}' => {
                r.end_group();
                i += 1;
            }
            b'\\' => i = r.control(bytes, i + 1),
            b'\r' | b'\n' => i += 1,
            _ => {
                r.char(super::super::text::from_1252(b));
                i += 1;
            }
        }
    }
    r.end_paragraph(false);
    r.flush_table();
    r.flush_list();
    Ok(Document { blocks: r.blocks, images: vec![] })
}

impl Reader {
    fn end_group(&mut self) {
        self.flush_text();
        if self.state.stylesheet && !self.stack.last().is_some_and(|s| s.stylesheet) {
            // Leaving the stylesheet.
        } else if self.state.stylesheet {
            self.end_style_entry();
        }
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    fn end_style_entry(&mut self) {
        if let (Some(n), name) = std::mem::take(&mut self.style_entry) {
            self.styles.insert(n, name.trim().trim_end_matches(';').trim().to_ascii_lowercase());
        }
    }

    fn char(&mut self, c: char) {
        if self.skip_chars > 0 {
            self.skip_chars -= 1;
            return;
        }
        if self.state.stylesheet {
            self.style_entry.1.push(c);
            return;
        }
        if self.state.list_text {
            self.list_text.push(c);
            return;
        }
        if !self.state.skip {
            self.text.push(c);
        }
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.inlines.push(Inline::Text { text, style: self.state.style });
        }
    }

    /// Reads a control word or symbol after a backslash; returns the next index.
    fn control(&mut self, bytes: &[u8], mut i: usize) -> usize {
        let Some(&first) = bytes.get(i) else { return i };
        if !first.is_ascii_alphabetic() {
            match first {
                b'\'' => {
                    let hex = bytes.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
                    if let Some(b) = hex {
                        self.char(super::super::text::from_1252(b));
                    }
                    return i + 3;
                }
                b'*' => self.state.skip = true, // an optional destination we don't know
                b'~' => self.char('\u{a0}'),
                b'_' => self.char('\u{2011}'),
                b'-' => {} // optional hyphen
                b'\\' | b'{' | b'}' => self.char(first as char),
                b'\n' | b'\r' => self.paragraph_mark(),
                _ => {}
            }
            return i + 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() && i - start < 32 {
            i += 1;
        }
        let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
        let num_start = i;
        if bytes.get(i) == Some(&b'-') {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() && i - num_start < 11 {
            i += 1;
        }
        let param: Option<i32> = std::str::from_utf8(&bytes[num_start..i]).ok().and_then(|s| s.parse().ok());
        if bytes.get(i) == Some(&b' ') {
            i += 1;
        }
        self.word(word, param);
        i
    }

    fn word(&mut self, word: &str, param: Option<i32>) {
        let on = param != Some(0);
        if SKIP.contains(&word) {
            self.flush_text();
            self.state.skip = true;
            return;
        }
        match word {
            "stylesheet" => {
                self.state.stylesheet = true;
                self.state.skip = true;
            }
            "s" if self.state.stylesheet => {
                self.end_style_entry();
                self.style_entry.0 = param;
            }
            "listtext" | "pntext" => {
                self.flush_text();
                self.state.list_text = true;
                self.list_text.clear();
            }
            "par" => self.paragraph_mark(),
            "pard" => {
                self.para = Para::default();
            }
            "plain" => {
                self.flush_text();
                self.state.style = Style::default();
            }
            "b" | "i" | "ul" | "ulnone" | "strike" | "striked" | "super" | "sub" | "nosupersub" => {
                self.flush_text();
                let s = &mut self.state.style;
                match word {
                    "b" => s.bold = on,
                    "i" => s.italic = on,
                    "ul" => s.underline = on,
                    "ulnone" => s.underline = false,
                    "strike" | "striked" => s.strike = on,
                    "super" => (s.sup, s.sub) = (on, false),
                    "sub" => (s.sub, s.sup) = (on, false),
                    _ => (s.sup, s.sub) = (false, false),
                }
            }
            "qc" => self.para.align = Align::Center,
            "qr" => self.para.align = Align::End,
            "qj" => self.para.align = Align::Justify,
            "ql" => self.para.align = Align::Start,
            "s" => self.para.style = param,
            "outlinelevel" => self.para.outline = param.and_then(|p| u8::try_from(p).ok()).filter(|&l| l < 9).map(|l| l + 1),
            "ls" => self.para.list = Some((param.unwrap_or(0), self.para.list.map(|l| l.1).unwrap_or(0))),
            "ilvl" => {
                let level = param.unwrap_or(0).clamp(0, 8) as usize;
                self.para.list = Some((self.para.list.map(|l| l.0).unwrap_or(-1), level));
            }
            "intbl" => self.para.in_table = true,
            "cell" | "nestcell" => {
                self.end_paragraph(true);
                let cell = std::mem::take(&mut self.cell);
                self.row.push(cell);
            }
            "row" | "nestrow" => {
                let row = std::mem::take(&mut self.row);
                if !row.is_empty() {
                    self.rows.push(row);
                }
            }
            "line" => {
                self.flush_text();
                self.inlines.push(Inline::Break);
            }
            "tab" => self.char('\t'),
            "page" => {
                self.end_paragraph(false);
                self.page_break = true;
                self.flush_list();
                self.blocks.push(Block::PageBreak);
            }
            "uc" => self.state.uc = param.unwrap_or(1).clamp(0, 10) as u8,
            "u" => {
                if let Some(n) = param {
                    let code = if n < 0 { n + 65_536 } else { n } as u32;
                    if let Some(c) = char::from_u32(code) {
                        self.char(c);
                    }
                    self.skip_chars = self.state.uc;
                }
            }
            "emdash" => self.char('—'),
            "endash" => self.char('–'),
            "bullet" => self.char('•'),
            "lquote" => self.char('‘'),
            "rquote" => self.char('’'),
            "ldblquote" => self.char('“'),
            "rdblquote" => self.char('”'),
            "emspace" | "enspace" | "qmspace" => self.char(' '),
            _ => {}
        }
    }

    fn paragraph_mark(&mut self) {
        self.end_paragraph(false);
    }

    /// Ends the paragraph: into the cell being read, the list, or the document.
    fn end_paragraph(&mut self, cell_end: bool) {
        self.flush_text();
        let inlines = merge_runs(std::mem::take(&mut self.inlines));
        let list_text = std::mem::take(&mut self.list_text);
        let style_name = self.para.style.and_then(|s| self.styles.get(&s)).cloned().unwrap_or_default();
        let level = style_name.strip_prefix("heading ").and_then(|l| l.trim().parse::<u8>().ok()).or(self.para.outline);
        if self.para.in_table || cell_end {
            if !inlines.is_empty() || cell_end {
                self.cell.push(Block::Paragraph { inlines, align: self.para.align });
            }
            return;
        }
        self.flush_table();
        if inlines.is_empty() {
            return; // RTF writers end documents and groups with empty \par
        }
        let block = match level {
            Some(l) => Block::Heading { level: l.clamp(1, 6), inlines },
            None => Block::Paragraph { inlines, align: self.para.align },
        };
        match (self.para.list, &block) {
            (Some((id, level)), Block::Paragraph { .. }) => {
                let numbered = list_text.chars().any(|c| c.is_ascii_alphanumeric());
                self.push_list_item(id, level, numbered, block);
            }
            _ => {
                self.flush_list();
                self.blocks.push(block);
            }
        }
    }

    fn push_list_item(&mut self, id: i32, level: usize, numbered: bool, block: Block) {
        if self.list.as_ref().is_some_and(|l| l.id != id) {
            self.flush_list();
        }
        let list = self.list.get_or_insert_with(|| ListBuild { id, levels: vec![] });
        while list.levels.len() > level + 1 {
            close_level(list);
        }
        while list.levels.len() < level + 1 {
            list.levels.push((numbered, vec![]));
        }
        list.levels[level].1.push(vec![block]);
    }

    fn flush_list(&mut self) {
        if let Some(mut list) = self.list.take() {
            while list.levels.len() > 1 {
                close_level(&mut list);
            }
            if let Some((ordered, items)) = list.levels.pop() {
                self.blocks.push(Block::List { ordered, items });
            }
        }
    }

    fn flush_table(&mut self) {
        if !self.row.is_empty() {
            let row = std::mem::take(&mut self.row);
            self.rows.push(row);
        }
        if !self.rows.is_empty() {
            self.flush_list();
            let rows = std::mem::take(&mut self.rows);
            self.blocks.push(Block::Table { rows });
        }
    }
}

/// Closes the deepest level into the last item of its parent.
fn close_level(list: &mut ListBuild) {
    if let Some((ordered, items)) = list.levels.pop() {
        let nested = Block::List { ordered, items };
        if let Some((_, parent)) = list.levels.last_mut() {
            match parent.last_mut() {
                Some(item) => item.push(nested),
                None => parent.push(vec![nested]),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_like_rtf() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Calibri;}}{\colortbl;\red0\green0\blue0;}
{\stylesheet{\s0 Normal;}{\s1\b heading 1;}}
{\*\generator Writer;}
{\pard\s1\b T\'edtulo\par}
\pard\qc Texto com {\b negrito} e \u8364?uro\par
{\listtext 1.\tab}\pard\ls1\ilvl0 um\par
{\listtext 2.\tab}\pard\ls1\ilvl0 dois\par
{\listtext \'95\tab}\pard\ls1\ilvl1 sub\par
\pard\intbl A\cell B\cell\row
\pard\intbl C\cell D\cell\row
\pard fim\par}";
        let doc = read(rtf).unwrap();
        let bold = Style { bold: true, ..Default::default() };
        assert_eq!(doc.blocks[0], Block::Heading { level: 1, inlines: vec![Inline::Text { text: "Título".into(), style: bold }] });
        assert_eq!(doc.blocks[1], Block::Paragraph { inlines: vec![Inline::text("Texto com "), Inline::Text { text: "negrito".into(), style: bold }, Inline::text(" e €uro")], align: Align::Center });
        let Block::List { ordered: true, items } = &doc.blocks[2] else { panic!("{:?}", doc.blocks[2]) };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1][1], Block::List { ordered: false, .. }), "{:?}", items[1]);
        let Block::Table { rows } = &doc.blocks[3] else { panic!("{:?}", doc.blocks[3]) };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], vec![Block::Paragraph { inlines: vec![Inline::text("D")], align: Align::Start }]);
        assert_eq!(doc.blocks[4], Block::Paragraph { inlines: vec![Inline::text("fim")], align: Align::Start });
    }

    #[test]
    fn hostile_rtf() {
        assert!(read(b"hello").is_err());
        let deep = format!("{{\\rtf1 {}x{}}}", "{".repeat(100_000), "}".repeat(100_000));
        assert_eq!(read(deep.as_bytes()).err(), Some(DocError::TooBig));
        // Truncated in the middle of a control word, a hex escape and a group: no panic.
        for s in [&b"{\\rtf1 abc\\"[..], b"{\\rtf1 abc\\'4", b"{\\rtf1 {\\b abc", b"{\\rtf1 \\u-", b"{\\rtf1 \\u99999999999"] {
            let _ = read(s);
        }
    }
}
