//! The model as plain text: headings and paragraphs separated by blank lines, list markers, table
//! cells separated by tabs.

use super::{Block, Document, plain};

pub fn write(doc: &Document) -> String {
    let mut out = String::new();
    blocks(&mut out, &doc.blocks, "");
    out.trim_end().to_owned() + "\n"
}

fn blocks(out: &mut String, list: &[Block], indent: &str) {
    for b in list {
        match b {
            Block::Heading { inlines, .. } | Block::Paragraph { inlines, .. } => {
                for line in plain(inlines).split('\n') {
                    out.push_str(&format!("{indent}{line}\n"));
                }
                out.push('\n');
            }
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = if *ordered { format!("{}. ", n + 1) } else { "• ".into() };
                    let mut inner = String::new();
                    blocks(&mut inner, item, &format!("{indent}{}", " ".repeat(marker.chars().count())));
                    let inner = inner.trim_end().trim_start().to_owned();
                    out.push_str(&format!("{indent}{marker}{inner}\n"));
                }
                out.push('\n');
            }
            Block::Table { rows } => {
                for row in rows {
                    let cells: Vec<String> = row.iter().map(|c| {
                        let mut s = String::new();
                        blocks(&mut s, c, "");
                        s.split_whitespace().collect::<Vec<_>>().join(" ")
                    }).collect();
                    out.push_str(&format!("{indent}{}\n", cells.join("\t")));
                }
                out.push('\n');
            }
            Block::Code { text } => out.push_str(&format!("{text}\n\n")),
            Block::Quote { blocks: inner } => blocks(out, inner, &format!("{indent}    ")),
            Block::PageBreak => out.push('\u{c}'),
            Block::Rule => out.push_str(&format!("{indent}----\n\n")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::markdown;

    #[test]
    fn plain_text() {
        let doc = markdown::read("# T\n\nA **b**.\n\n- x\n- y\n\n| 1 | 2 |\n|---|---|\n| 3 | 4 |\n");
        assert_eq!(write(&doc), "T\n\nA b.\n\n• x\n• y\n\n1\t2\n3\t4\n");
    }
}
