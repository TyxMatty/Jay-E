//! A small XML tree for the readers: element and attribute names are local (the `w:` or `text:`
//! prefix dropped), text is unescaped. Bounded: depth and node count, as the files are untrusted.

use super::{DocError, invalid};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

const MAX_DEPTH: usize = 512;
const MAX_NODES: usize = 5_000_000;

#[derive(Debug, Default)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Child>,
    /// Where the element is in the parsed bytes, start tag to end tag (the DOCX editor copies
    /// what it didn't change from there, byte for byte).
    pub span: std::ops::Range<usize>,
}

#[derive(Debug)]
pub enum Child {
    Node(Node),
    Text(String),
}

impl Node {
    /// An attribute by local name ("val" matches `w:val`).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter_map(|c| if let Child::Node(n) = c { Some(n) } else { None })
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.nodes().find(|n| n.name == name)
    }

    /// The first descendant with this name, depth first.
    pub fn find(&self, name: &str) -> Option<&Node> {
        self.nodes().find_map(|n| if n.name == name { Some(n) } else { n.find(name) })
    }

    /// All the text inside, in order.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for c in &self.children {
            match c {
                Child::Text(t) => out.push_str(t),
                Child::Node(n) => out.push_str(&n.text()),
            }
        }
        out
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_owned()
}

/// Parses `bytes` into its root element.
pub fn parse(bytes: &[u8]) -> Result<Node, DocError> {
    let mut reader = Reader::from_reader(bytes);
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut nodes = 0usize;
    let mut buf = Vec::new();
    loop {
        let at = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buf).map_err(|e| invalid(format!("XML: {e}")))?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                nodes += 1;
                if stack.len() > MAX_DEPTH || nodes > MAX_NODES {
                    return Err(DocError::TooBig);
                }
                let mut node = Node { name: local(start.name().as_ref()), span: at..end, ..Default::default() };
                for attr in start.attributes().flatten() {
                    let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_else(|_| attr.value.to_string());
                    node.attrs.push((local(attr.key.as_ref()), value));
                }
                if matches!(event, Event::Empty(_)) {
                    stack.last_mut().ok_or_else(|| invalid("XML: unbalanced"))?.children.push(Child::Node(node));
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                let mut node = stack.pop().ok_or_else(|| invalid("XML: unbalanced"))?;
                node.span.end = end;
                stack.last_mut().ok_or_else(|| invalid("XML: unbalanced"))?.children.push(Child::Node(node));
            }
            Event::Text(text) => push_text(&mut stack, &text.xml10_content()),
            Event::CData(data) => push_text(&mut stack, &data.into_inner()),
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => match r.xml10_content().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        _ => String::new(), // no DTD entities: never expanded (billion laughs)
                    },
                };
                push_text(&mut stack, &resolved);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    let mut document = stack.pop().ok_or_else(|| invalid("XML: unbalanced"))?;
    let root = document.children.drain(..).find_map(|c| if let Child::Node(n) = c { Some(n) } else { None });
    root.ok_or_else(|| invalid("XML: no root element"))
}

fn push_text(stack: &mut [Node], text: &str) {
    if let Some(top) = stack.last_mut() {
        match top.children.last_mut() {
            Some(Child::Text(t)) => t.push_str(text),
            _ => top.children.push(Child::Text(text.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_attributes_and_text() {
        let root = parse(br#"<?xml version="1.0"?><w:document xmlns:w="x"><w:p w:val="a&amp;b"><w:t>A &lt; B &#233;</w:t><w:t><![CDATA[<raw>]]></w:t></w:p></w:document>"#).unwrap();
        assert_eq!(root.name, "document");
        let p = root.child("p").unwrap();
        assert_eq!(p.attr("val"), Some("a&b"));
        assert_eq!(p.text(), "A < B é<raw>");
        assert!(root.find("t").is_some());
    }

    #[test]
    fn spans_cover_each_element() {
        let src = br#"<?xml version="1.0"?><a><b x="1">hi</b><c/></a>"#;
        let root = parse(src).unwrap();
        let s = |n: &Node| std::str::from_utf8(&src[n.span.clone()]).unwrap().to_owned();
        assert_eq!(s(&root), r#"<a><b x="1">hi</b><c/></a>"#);
        assert_eq!(s(root.child("b").unwrap()), r#"<b x="1">hi</b>"#);
        assert_eq!(s(root.child("c").unwrap()), "<c/>");
    }

    #[test]
    fn entities_from_a_doctype_are_not_expanded() {
        let bomb = br#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]><x>&b;</x>"#;
        assert_eq!(parse(bomb).unwrap().text(), "");
    }

    #[test]
    fn too_deep_is_refused() {
        let deep = "<a>".repeat(MAX_DEPTH + 2) + &"</a>".repeat(MAX_DEPTH + 2);
        assert_eq!(parse(deep.as_bytes()).err(), Some(DocError::TooBig));
        assert!(parse(b"<a><b></a>").is_err() || parse(b"<a><b></a>").is_ok(), "malformed never panics");
    }
}
