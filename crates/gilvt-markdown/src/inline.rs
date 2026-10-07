//! Flattening comrak inline nodes into styled runs.

use comrak::nodes::{AstNode, NodeValue};

use crate::model::{Inline, Style};

/// Styled runs of `node`'s children. Images inside text show their alt text.
pub fn inlines<'a>(node: &'a AstNode<'a>) -> Vec<Inline> {
    let mut out = Vec::new();
    for child in node.children() {
        collect(child, &Style::default(), &mut out);
    }
    out
}

/// Concatenated text of `node`'s children, styles dropped.
pub fn plain_text<'a>(node: &'a AstNode<'a>) -> String {
    inlines(node).into_iter().map(|i| i.text).collect()
}

fn push(out: &mut Vec<Inline>, text: &str, style: &Style) {
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.style == *style => last.text.push_str(text),
        _ => out.push(Inline { text: text.to_string(), style: style.clone() }),
    }
}

fn collect<'a>(node: &'a AstNode<'a>, style: &Style, out: &mut Vec<Inline>) {
    let nested = |style: Style, out: &mut Vec<Inline>| {
        for child in node.children() {
            collect(child, &style, out);
        }
    };
    match &node.data.borrow().value {
        NodeValue::Text(text) => push(out, text, style),
        NodeValue::SoftBreak => push(out, " ", style),
        NodeValue::LineBreak => push(out, "\n", style),
        NodeValue::Code(code) => push(out, &code.literal, &Style { code: true, ..style.clone() }),
        NodeValue::HtmlInline(html) => push(out, html, style),
        NodeValue::Emph => nested(Style { italic: true, ..style.clone() }, out),
        NodeValue::Strong => nested(Style { bold: true, ..style.clone() }, out),
        NodeValue::Strikethrough => nested(Style { strike: true, ..style.clone() }, out),
        NodeValue::Link(link) => nested(Style { link: Some(link.url.clone()), ..style.clone() }, out),
        NodeValue::Image(_) => nested(style.clone(), out),
        NodeValue::FootnoteReference(r) => {
            let number = r.ix as usize;
            push(out, &number.to_string(), &Style { footnote: Some(number), ..style.clone() });
        }
        _ => nested(style.clone(), out),
    }
}
