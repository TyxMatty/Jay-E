//! HTML into the model (stage 10, for the converter). The page is first cleaned by ammonia down to
//! the tags the model has (no scripts, styles or attributes but links and pictures), then read as
//! a tree. Pictures come only from `data:` addresses: nothing is fetched.

use super::xml::{self, Child, Node};
use super::{Block, Document, Image, Inline, Style, merge_runs};
use std::collections::{HashMap, HashSet};

const BLOCKS: [&str; 22] = ["p", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "ul", "ol", "li", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "blockquote", "hr", "div", "body"];

pub fn read(html: &str) -> Document {
    let tags: HashSet<&str> = BLOCKS.iter().copied().chain(["br", "code", "b", "strong", "i", "em", "u", "s", "strike", "del", "sup", "sub", "a", "img"]).filter(|t| *t != "body").collect();
    let clean = ammonia::Builder::default()
        .tags(tags)
        .clean_content_tags(["script", "style", "title", "template", "noscript", "textarea", "select"].into())
        .generic_attributes(HashSet::new())
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"])), ("img", HashSet::from(["src", "width", "height"]))]))
        .url_schemes(["http", "https", "mailto", "data"].into())
        .link_rel(None)
        .clean(html)
        .to_string();
    // What ammonia writes is HTML: closed void tags and numeric spaces make it XML.
    let xml = format!("<body>{}</body>", close_void(&clean).replace("&nbsp;", "&#160;"));
    let mut doc = Document::default();
    if let Ok(root) = xml::parse(xml.as_bytes()) {
        blocks_of(&root, &mut doc.blocks, &mut doc.images);
    }
    doc
}

/// `<br>`, `<hr>` and `<img …>` as `<br/>`, `<hr/>`, `<img …/>` (quotes respected).
fn close_void(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let void = ["<br", "<hr", "<img"].iter().any(|t| rest.starts_with(t) && rest[t.len()..].starts_with([' ', '>', '/']));
        let mut quote = None;
        let end = rest.char_indices().skip(1).find(|&(_, c)| match quote {
            Some(q) if c == q => {
                quote = None;
                false
            }
            Some(_) => false,
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                false
            }
            None => c == '>',
        });
        let Some((end, _)) = end else {
            out.push_str(rest);
            return out;
        };
        let tag = rest[..end].trim_end_matches('/');
        out.push_str(tag);
        out.push_str(if void { "/>" } else { ">" });
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn is_block(name: &str) -> bool {
    BLOCKS.contains(&name)
}

/// The children of a block container: runs of inline content become paragraphs.
fn blocks_of(node: &Node, out: &mut Vec<Block>, images: &mut Vec<Image>) {
    let mut run: Vec<Inline> = vec![];
    for child in &node.children {
        match child {
            Child::Node(n) if is_block(&n.name) => {
                paragraph(std::mem::take(&mut run), out);
                block(n, out, images);
            }
            _ => inline(child, Style::default(), &mut run, images),
        }
    }
    paragraph(run, out);
}

/// Inlines as a paragraph, spaces at its ends trimmed; nothing if it's empty.
fn trimmed(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut inlines = merge_runs(inlines);
    if let Some(Inline::Text { text, .. }) = inlines.first_mut() {
        *text = text.trim_start().to_owned();
    }
    if let Some(Inline::Text { text, .. }) = inlines.last_mut() {
        *text = text.trim_end().to_owned();
    }
    merge_runs(inlines)
}

fn paragraph(inlines: Vec<Inline>, out: &mut Vec<Block>) {
    let inlines = trimmed(inlines);
    if !inlines.is_empty() {
        out.push(Block::Paragraph { inlines, align: Default::default() });
    }
}

fn inlines_of(node: &Node, images: &mut Vec<Image>) -> Vec<Inline> {
    let mut out = vec![];
    for c in &node.children {
        inline(c, Style::default(), &mut out, images);
    }
    trimmed(out)
}

fn block(n: &Node, out: &mut Vec<Block>, images: &mut Vec<Image>) {
    match n.name.as_str() {
        h if h.len() == 2 && h.starts_with('h') && h != "hr" => {
            let level = h[1..].parse().unwrap_or(1);
            out.push(Block::Heading { level, inlines: inlines_of(n, images) });
        }
        "p" => paragraph(inlines_of(n, images), out),
        "pre" => out.push(Block::Code { text: n.text().trim_end_matches('\n').to_owned() }),
        "hr" => out.push(Block::Rule),
        "ul" | "ol" => {
            let items = n
                .nodes()
                .map(|item| {
                    let mut blocks = vec![];
                    blocks_of(item, &mut blocks, images);
                    blocks
                })
                .filter(|b| !b.is_empty())
                .collect();
            out.push(Block::List { ordered: n.name == "ol", items });
        }
        "table" => {
            let mut rows = vec![];
            rows_of(n, &mut rows, images);
            if !rows.is_empty() {
                out.push(Block::Table { rows });
            }
        }
        "blockquote" => {
            let mut blocks = vec![];
            blocks_of(n, &mut blocks, images);
            out.push(Block::Quote { blocks });
        }
        _ => blocks_of(n, out, images), // div, li or a cell out of place: its content
    }
}

fn rows_of(n: &Node, rows: &mut Vec<Vec<Vec<Block>>>, images: &mut Vec<Image>) {
    for child in n.nodes() {
        match child.name.as_str() {
            "tr" => rows.push(
                child
                    .nodes()
                    .map(|cell| {
                        let mut blocks = vec![];
                        blocks_of(cell, &mut blocks, images);
                        blocks
                    })
                    .collect(),
            ),
            "thead" | "tbody" | "tfoot" => rows_of(child, rows, images),
            _ => {}
        }
    }
}

fn inline(child: &Child, style: Style, out: &mut Vec<Inline>, images: &mut Vec<Image>) {
    let n = match child {
        Child::Text(t) => {
            // Runs of white space are one space, as a browser shows them.
            let after_space = out.last().is_none_or(|i| matches!(i, Inline::Break) || matches!(i, Inline::Text { text, .. } if text.ends_with(' ')));
            let mut text = String::with_capacity(t.len());
            for c in t.chars() {
                if c.is_whitespace() && c != '\u{a0}' {
                    if !(if text.is_empty() { after_space } else { text.ends_with(' ') }) {
                        text.push(' ');
                    }
                } else {
                    text.push(c);
                }
            }
            out.push(Inline::Text { text, style });
            return;
        }
        Child::Node(n) => n,
    };
    let mut s = style;
    match n.name.as_str() {
        "br" => return out.push(Inline::Break),
        "img" => {
            if let Some(image) = n.attr("src").and_then(data_url) {
                let size = |k: &str| n.attr(k).and_then(|v| v.trim_end_matches("px").parse::<f32>().ok()).map(|px| px * 0.75);
                images.push(image);
                out.push(Inline::Image { index: images.len() - 1, width: size("width"), height: size("height") });
            }
            return;
        }
        "a" => {
            let mut inner = vec![];
            for c in &n.children {
                inline(c, style, &mut inner, images);
            }
            match n.attr("href") {
                Some(href) if !href.starts_with("data:") => out.push(Inline::Link { href: href.to_owned(), inlines: merge_runs(inner) }),
                _ => out.extend(inner),
            }
            return;
        }
        "b" | "strong" | "th" => s.bold = true,
        "i" | "em" => s.italic = true,
        "u" => s.underline = true,
        "s" | "strike" | "del" => s.strike = true,
        "code" => s.code = true,
        "sup" => s.sup = true,
        "sub" => s.sub = true,
        _ => {}
    }
    for c in &n.children {
        inline(c, s, out, images);
    }
}

/// A picture in a `data:image/…;base64,` address.
fn data_url(src: &str) -> Option<Image> {
    let rest = src.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    if !head.ends_with(";base64") {
        return None;
    }
    let bytes = crate::image::base64_decode(&data.split_whitespace().collect::<String>())?;
    let mime = super::image_mime(&bytes)?;
    Some(Image { mime: mime.to_owned(), bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_becomes_the_model() {
        let html = r#"<!doctype html><html><head><title>Hidden</title><style>p{color:red}</style><script>alert(1)</script></head>
            <body><h1>The   title</h1><p>Some <b>bold</b> and <a href="https://x.org" onclick="evil()">a link</a>.<br>Next&nbsp;line</p>
            <ul><li>one</li><li><p>two</p></li></ul>
            <table><thead><tr><th>A</th><th>B</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>
            <div>loose text<blockquote>said</blockquote></div><hr><pre>code
  kept</pre>
            <img src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" width="40"><img src="https://x.org/a.png"></body></html>"#;
        let doc = read(html);
        let b = &doc.blocks;
        assert_eq!(b[0], Block::Heading { level: 1, inlines: vec![Inline::text("The title")] });
        let Block::Paragraph { inlines, .. } = &b[1] else { panic!("{b:?}") };
        assert_eq!(inlines[1], Inline::Text { text: "bold".into(), style: Style { bold: true, ..Default::default() } });
        assert_eq!(inlines[3], Inline::Link { href: "https://x.org".into(), inlines: vec![Inline::text("a link")] });
        assert_eq!(inlines[5], Inline::Break);
        assert_eq!(inlines[6], Inline::text("Next\u{a0}line"));
        assert!(matches!(&b[2], Block::List { ordered: false, items } if items.len() == 2));
        assert!(matches!(&b[3], Block::Table { rows } if rows.len() == 2 && rows[1].len() == 2));
        assert_eq!(b[4], Block::Paragraph { inlines: vec![Inline::text("loose text")], align: Default::default() });
        assert!(matches!(&b[5], Block::Quote { .. }));
        assert_eq!(b[6], Block::Rule);
        assert_eq!(b[7], Block::Code { text: "code\n  kept".into() });
        // The data: picture is in; the one on the internet isn't fetched.
        assert_eq!(doc.images.len(), 1);
        assert!(matches!(&b[8], Block::Paragraph { inlines, .. } if inlines == &[Inline::Image { index: 0, width: Some(30.0), height: None }]));
        assert_eq!(b.len(), 9);
        let all = format!("{doc:?}");
        assert!(!all.contains("Hidden") && !all.contains("alert") && !all.contains("color:red") && !all.contains("evil"));
    }

    #[test]
    fn void_tags_close_but_quotes_are_kept() {
        assert_eq!(close_void(r#"a<br>b<img src="x>y"><hr/><b>c</b>"#), r#"a<br/>b<img src="x>y"/><hr/><b>c</b>"#);
    }
}
