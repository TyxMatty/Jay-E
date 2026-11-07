//! The model as HTML: the viewer's page and the HTML export. Everything is escaped here (the text
//! comes from untrusted files) and links keep only http(s) and mailto.

use super::{Align, Block, Document, Inline, Style};

/// `image_src(index)` gives each image's address (a `jaye://` URL on screen, a data: URL in a file).
pub fn to_html(doc: &Document, image_src: &dyn Fn(usize) -> String) -> String {
    let mut out = String::with_capacity(4096);
    blocks(&mut out, &doc.blocks, image_src);
    out
}

fn blocks(out: &mut String, blocks_: &[Block], img: &dyn Fn(usize) -> String) {
    for b in blocks_ {
        block(out, b, img);
    }
}

fn block(out: &mut String, b: &Block, img: &dyn Fn(usize) -> String) {
    match b {
        Block::Heading { level, inlines: i } => {
            let l = (*level).clamp(1, 6);
            out.push_str(&format!("<h{l}>"));
            inlines(out, i, img);
            out.push_str(&format!("</h{l}>\n"));
        }
        Block::Paragraph { inlines: i, align } => {
            let style = match align {
                Align::Start => "",
                Align::Center => " style=\"text-align:center\"",
                Align::End => " style=\"text-align:end\"",
                Align::Justify => " style=\"text-align:justify\"",
            };
            out.push_str(&format!("<p{style}>"));
            if i.is_empty() {
                out.push_str("<br>"); // an empty paragraph still takes a line, as in Word
            }
            inlines(out, i, img);
            out.push_str("</p>\n");
        }
        Block::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>\n"));
            for item in items {
                out.push_str("<li>");
                // A one-paragraph item without the <p>, so lists stay tight.
                match item.as_slice() {
                    [Block::Paragraph { inlines: i, .. }] => inlines(out, i, img),
                    [Block::Paragraph { inlines: i, .. }, rest @ ..] => {
                        inlines(out, i, img);
                        blocks(out, rest, img);
                    }
                    _ => blocks(out, item, img),
                }
                out.push_str("</li>\n");
            }
            out.push_str(&format!("</{tag}>\n"));
        }
        Block::Table { rows } => {
            out.push_str("<table>\n");
            for row in rows {
                out.push_str("<tr>");
                for cell in row {
                    out.push_str("<td>");
                    match cell.as_slice() {
                        [Block::Paragraph { inlines: i, .. }] => inlines(out, i, img),
                        _ => blocks(out, cell, img),
                    }
                    out.push_str("</td>");
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</table>\n");
        }
        Block::Code { text } => out.push_str(&format!("<pre><code>{}</code></pre>\n", escape(text))),
        Block::Quote { blocks: b } => {
            out.push_str("<blockquote>\n");
            blocks(out, b, img);
            out.push_str("</blockquote>\n");
        }
        Block::PageBreak => out.push_str("<hr class=\"page-break\">\n"),
        Block::Rule => out.push_str("<hr>\n"),
    }
}

fn inlines(out: &mut String, list: &[Inline], img: &dyn Fn(usize) -> String) {
    for i in list {
        match i {
            Inline::Text { text, style } => styled(out, text, *style),
            Inline::Link { href, inlines: inner } => {
                if safe_href(href) {
                    out.push_str(&format!("<a href=\"{}\">", escape(href)));
                    inlines(out, inner, img);
                    out.push_str("</a>");
                } else {
                    inlines(out, inner, img);
                }
            }
            Inline::Image { index, width, height } => {
                let mut size = String::new();
                if let Some(w) = width {
                    size.push_str(&format!(" width=\"{}\"", (w * 96.0 / 72.0).round()));
                }
                if let Some(h) = height {
                    size.push_str(&format!(" height=\"{}\"", (h * 96.0 / 72.0).round()));
                }
                out.push_str(&format!("<img src=\"{}\" alt=\"\"{size}>", escape(&img(*index))));
            }
            Inline::Break => out.push_str("<br>"),
        }
    }
}

fn styled(out: &mut String, text: &str, s: Style) {
    let tags: Vec<&str> = [(s.bold, "strong"), (s.italic, "em"), (s.underline, "u"), (s.strike, "s"), (s.code, "code"), (s.sup, "sup"), (s.sub, "sub")]
        .into_iter()
        .filter_map(|(on, tag)| on.then_some(tag))
        .collect();
    for t in &tags {
        out.push_str(&format!("<{t}>"));
    }
    out.push_str(&escape(text));
    for t in tags.iter().rev() {
        out.push_str(&format!("</{t}>"));
    }
}

pub(crate) fn safe_href(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s))
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_escaped_and_links_are_filtered() {
        let doc = Document {
            blocks: vec![
                Block::Heading { level: 9, inlines: vec![Inline::text("<script>x</script>")] },
                Block::Paragraph {
                    inlines: vec![
                        Inline::Link { href: "javascript:alert(1)".into(), inlines: vec![Inline::text("bad")] },
                        Inline::Link { href: "https://e.com/?a=1&b=\"2\"".into(), inlines: vec![Inline::text("good")] },
                    ],
                    align: Align::Center,
                },
                Block::List { ordered: true, items: vec![vec![Block::Paragraph { inlines: vec![Inline::Text { text: "a".into(), style: Style { bold: true, italic: true, ..Default::default() } }], align: Align::Start }]] },
            ],
            images: vec![],
        };
        let html = to_html(&doc, &|_| String::new());
        assert!(html.contains("<h6>&lt;script&gt;x&lt;/script&gt;</h6>"), "{html}");
        assert!(!html.contains("javascript"), "{html}");
        assert!(html.contains("<a href=\"https://e.com/?a=1&amp;b=&quot;2&quot;\">good</a>"), "{html}");
        assert!(html.contains("<p style=\"text-align:center\">"), "{html}");
        assert!(html.contains("<ol>\n<li><strong><em>a</em></strong></li>"), "{html}");
    }
}
