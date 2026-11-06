//! Markdown to HTML that is safe to put in the page: CommonMark + GFM (tables, task lists,
//! strikethrough, footnotes) with `pulldown-cmark`, cleaned by `ammonia`, and `$…$` / `$$…$$` math
//! turned into MathML Core by `math-core`, which WebView2 draws without a JS library.
//!
//! Each top-level block is preceded by an empty `<span data-line="N">`, the source line it starts
//! on, so the preview can follow the editor's scroll.

use ammonia::Builder;
use math_core::{LatexToMathML, MathCoreConfig, MathDisplay};
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};
use std::collections::HashSet;

/// Renders `markdown`. `image_src` maps each image's address to the one the page may load (or
/// `None` to leave the image out): the caller decides which files can be shown.
pub fn to_html(markdown: &str, mut image_src: impl FnMut(&str) -> Option<String>) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let line_starts: Vec<usize> = std::iter::once(0).chain(markdown.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_of = |offset: usize| line_starts.partition_point(|&s| s <= offset);

    let converter = LatexToMathML::new(MathCoreConfig::default(), vec![]).ok();
    let mut math: Vec<String> = vec![];
    let mut images: HashSet<String> = HashSet::new();
    let mut depth = 0usize;
    let mut events: Vec<Event> = vec![];
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match &event {
            Event::Start(_) => {
                if depth == 0 {
                    events.push(Event::Html(format!("<span data-line=\"{}\"></span>", line_of(range.start)).into()));
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        match event {
            Event::InlineMath(tex) | Event::DisplayMath(tex) => {
                let display = if matches!(markdown.get(range.clone()), Some(s) if s.starts_with("$$")) { MathDisplay::Block } else { MathDisplay::Inline };
                let mathml = converter.as_ref().and_then(|c| c.convert_with_local_state(&tex, display).ok()).map(|r| r.mathml);
                // Math goes in after cleaning (ammonia would drop MathML): a placeholder for now.
                events.push(Event::Html(format!("<span data-math=\"{}\"></span>", math.len()).into()));
                math.push(mathml.unwrap_or_else(|| format!("<code class=\"math-error\">{}</code>", escape(&tex))));
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                let dest = image_src(&dest_url).unwrap_or_default();
                images.insert(dest.clone());
                events.push(Event::Start(Tag::Image { link_type, dest_url: CowStr::from(dest), title, id }));
            }
            Event::End(TagEnd::Image) => events.push(Event::End(TagEnd::Image)),
            other => events.push(other),
        }
    }
    let mut raw = String::with_capacity(markdown.len() * 3 / 2);
    html::push_html(&mut raw, events.into_iter());

    let mut clean = sanitizer(images).clean(&raw).to_string();
    for (i, m) in math.iter().enumerate() {
        clean = clean.replacen(&format!("<span data-math=\"{i}\"></span>"), m, 1);
    }
    clean
}

/// ammonia's defaults (no scripts, no event handlers, no `style`, safe URL schemes) plus what this
/// renderer emits: line markers, math placeholders, task-list checkboxes and the footnote classes.
/// Images keep only an address `image_src` gave, so an `<img>` written as HTML can't load anything.
fn sanitizer(images: HashSet<String>) -> Builder<'static> {
    let mut b = Builder::default();
    b.add_tags(["input", "span"])
        .add_tag_attributes("span", ["data-line", "data-math"])
        .add_tag_attributes("input", ["type", "checked", "disabled"])
        .add_tag_attributes("ol", ["start"])
        .add_allowed_classes("div", ["footnote-definition"])
        .add_allowed_classes("sup", ["footnote-reference"])
        .add_allowed_classes("code", ["math-error"])
        .attribute_filter(move |element, attribute, value| match (element, attribute) {
            // Only disabled checkboxes: the preview isn't a form.
            ("input", "type") if value != "checkbox" => None,
            ("img", "src") if !images.contains(value) => None,
            _ => Some(value.into()),
        });
    b.add_tag_attributes("img", HashSet::from(["src", "alt", "title"]));
    b
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(md: &str) -> String {
        to_html(md, |src| Some(src.to_owned()))
    }

    #[test]
    fn commonmark_and_gfm() {
        let html = render("# Title\n\nSome **bold** and ~~gone~~.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n");
        for part in ["<h1>Title</h1>", "<strong>bold</strong>", "<del>gone</del>", "<table>", "<td>1</td>", "type=\"checkbox\"", "checked"] {
            assert!(html.contains(part), "{part} missing in {html}");
        }
    }

    #[test]
    fn top_level_blocks_carry_their_source_line() {
        let html = render("# One\n\npara\non two lines\n\n- item\n- item\n\n```\ncode\n```\n");
        let lines: Vec<&str> = html.match_indices("data-line=\"").map(|(i, _)| &html[i + 11..i + 12]).collect();
        assert_eq!(lines, ["1", "3", "6", "9"], "{html}");
    }

    #[test]
    fn scripts_and_handlers_never_survive() {
        let html = render(
            "<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[x](javascript:alert(1))\n\n<a href=\"#\" onclick=\"x()\">a</a>\n\n<iframe src=\"https://e.com\"></iframe>\n\n<input type=\"text\" value=\"form\">\n\n<div style=\"position:fixed\">s</div>",
        );
        for bad in ["<script", "onerror", "javascript:", "onclick", "<iframe", "type=\"text\"", "style="] {
            assert!(!html.contains(bad), "{bad} survived in {html}");
        }
    }

    #[test]
    fn math_becomes_mathml_and_stays_safe() {
        let html = render("Inline $x^2$ and\n\n$$\\frac{1}{2}$$\n");
        assert!(html.contains("<math") && html.contains("<msup>") && html.contains("<mfrac>"), "{html}");
        assert!(html.contains("display=\"block\""), "display math: {html}");
        // Markup inside math is text, never HTML.
        let evil = render("$\\text{<img src=x onerror=alert(1)>}$");
        assert!(!evil.contains("<img"), "{evil}");
        // Broken math shows its source as code.
        let broken = render("$\\frac{1}$");
        assert!(broken.contains("math-error") && broken.contains("\\frac{1}"), "{broken}");
    }

    #[test]
    fn images_go_through_the_callback() {
        let html = to_html("![a](pic.png) ![b](https://tracker.example/p.gif)", |src| (!src.starts_with("http")).then(|| format!("http://asset.localhost/{src}")));
        assert!(html.contains("src=\"http://asset.localhost/pic.png\""), "{html}");
        assert!(!html.contains("tracker"), "{html}");
        // Raw HTML images don't go through the callback, so they lose their address.
        let raw = to_html("<img src=\"C:/secret.png\"> <img src=\"http://asset.localhost/x.png\">", |_| None);
        assert!(!raw.contains("secret") && !raw.contains("asset.localhost"), "{raw}");
    }
}
