//! Comparing two versions of a document (stage 10): paragraph by paragraph, and word by word inside
//! a paragraph that changed. Only the text is compared, not the formatting.

use super::{Block, Document, plain};
use serde::Serialize;

/// The most cells a comparison table may have (old × new); past it, the middle is shown as all
/// removed then all added.
const MAX_CELLS: usize = 40_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Same,
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Piece {
    pub kind: Kind,
    pub text: String,
}

/// A paragraph of the comparison: `words` are its pieces (one, for a paragraph that's the same,
/// added or removed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Paragraph {
    pub kind: Kind,
    pub words: Vec<Piece>,
}

/// The text of every paragraph, heading, list item, cell and code block, in order.
pub fn paragraphs(doc: &Document) -> Vec<String> {
    fn walk(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Heading { inlines, .. } | Block::Paragraph { inlines, .. } => out.push(plain(inlines)),
                Block::List { items, .. } => items.iter().for_each(|i| walk(i, out)),
                Block::Table { rows } => rows.iter().flatten().for_each(|c| walk(c, out)),
                Block::Code { text } => out.push(text.clone()),
                Block::Quote { blocks } => walk(blocks, out),
                Block::PageBreak | Block::Rule => {}
            }
        }
    }
    let mut out = vec![];
    walk(&doc.blocks, &mut out);
    out.into_iter().map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()
}

/// The longest common subsequence of `a` and `b`, as steps: `Same(i, j)`, `Removed(i)`, `Added(j)`.
#[derive(Debug, PartialEq)]
enum Step {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

fn diff<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Step> {
    // The common start and end first: most edits change a little in the middle.
    let start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    let mut steps: Vec<Step> = (0..start).map(|i| Step::Same(i, i)).collect();
    let (n, m) = (ma.len(), mb.len());
    if n * m > MAX_CELLS {
        steps.extend((0..n).map(|i| Step::Removed(start + i)));
        steps.extend((0..m).map(|j| Step::Added(start + j)));
    } else {
        // lcs[i][j]: the common length of ma[i..] and mb[j..].
        let w = m + 1;
        let mut lcs = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * w + j] = if ma[i] == mb[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && ma[i] == mb[j] {
                steps.push(Step::Same(start + i, start + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
                steps.push(Step::Removed(start + i));
                i += 1;
            } else {
                steps.push(Step::Added(start + j));
                j += 1;
            }
        }
    }
    steps.extend((0..end).map(|k| Step::Same(a.len() - end + k, b.len() - end + k)));
    steps
}

/// Words and the spaces between them, so pieces join back into the text.
fn words(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if start < i {
                out.push(&text[start..i]);
            }
            out.push(&text[i..i + c.len_utf8()]);
            start = i + c.len_utf8();
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn push(pieces: &mut Vec<Piece>, kind: Kind, text: &str) {
    match pieces.last_mut() {
        Some(p) if p.kind == kind => p.text.push_str(text),
        _ => pieces.push(Piece { kind, text: text.to_owned() }),
    }
}

/// The two paragraphs word by word.
fn changed(old: &str, new: &str) -> Vec<Piece> {
    let (a, b) = (words(old), words(new));
    let mut pieces = vec![];
    for step in diff(&a, &b) {
        match step {
            Step::Same(_, j) => push(&mut pieces, Kind::Same, b[j]),
            Step::Removed(i) => push(&mut pieces, Kind::Removed, a[i]),
            Step::Added(j) => push(&mut pieces, Kind::Added, b[j]),
        }
    }
    pieces
}

/// `old` against `new`. Removed paragraphs right before added ones are paired up as changed.
pub fn compare(old: &Document, new: &Document) -> Vec<Paragraph> {
    let (a, b) = (paragraphs(old), paragraphs(new));
    let one = |kind, text: &String| Paragraph { kind, words: vec![Piece { kind, text: text.clone() }] };
    let mut out = vec![];
    let (mut removed, mut added): (Vec<usize>, Vec<usize>) = (vec![], vec![]);
    let flush = |removed: &mut Vec<usize>, added: &mut Vec<usize>, out: &mut Vec<Paragraph>| {
        let pairs = removed.len().min(added.len());
        for k in 0..pairs {
            out.push(Paragraph { kind: Kind::Changed, words: changed(&a[removed[k]], &b[added[k]]) });
        }
        out.extend(removed[pairs..].iter().map(|&i| one(Kind::Removed, &a[i])));
        out.extend(added[pairs..].iter().map(|&j| one(Kind::Added, &b[j])));
        removed.clear();
        added.clear();
    };
    for step in diff(&a, &b) {
        match step {
            Step::Same(_, j) => {
                flush(&mut removed, &mut added, &mut out);
                out.push(one(Kind::Same, &b[j]));
            }
            Step::Removed(i) => removed.push(i),
            Step::Added(j) => added.push(j),
        }
    }
    flush(&mut removed, &mut added, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Inline;

    fn doc(paragraphs: &[&str]) -> Document {
        Document { blocks: paragraphs.iter().map(|p| Block::Paragraph { inlines: vec![Inline::text(*p)], align: Default::default() }).collect(), images: vec![] }
    }

    #[test]
    fn paragraphs_and_words() {
        let old = doc(&["Title", "The quick brown fox.", "Gone paragraph.", "End."]);
        let new = doc(&["Title", "The slow brown fox jumps.", "End.", "New last line."]);
        let c = compare(&old, &new);
        let kinds: Vec<Kind> = c.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, [Kind::Same, Kind::Changed, Kind::Removed, Kind::Same, Kind::Added]);
        let words: Vec<(Kind, &str)> = c[1].words.iter().map(|p| (p.kind, p.text.as_str())).collect();
        assert_eq!(words[..4], [(Kind::Same, "The "), (Kind::Removed, "quick"), (Kind::Added, "slow"), (Kind::Same, " brown ")]);
        // The new text comes back from what isn't removed.
        let rebuilt: String = c[1].words.iter().filter(|p| p.kind != Kind::Removed).map(|p| p.text.as_str()).collect();
        assert_eq!(rebuilt, "The slow brown fox jumps.");
        let old_back: String = c[1].words.iter().filter(|p| p.kind != Kind::Added).map(|p| p.text.as_str()).collect();
        assert_eq!(old_back, "The quick brown fox.");
        assert!(compare(&old, &old).iter().all(|p| p.kind == Kind::Same));
    }
}
