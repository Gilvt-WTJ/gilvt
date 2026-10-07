//! Markdown source → block model, via comrak's GFM parser.

use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use comrak::{Anchorizer, Arena, Options};

use crate::inline::{inlines, plain_text};
use crate::model::{Block, BlockKind, Footnote, Lines, ListItem};
use crate::table::table;

fn options() -> Options<'static> {
    let mut options = Options::default();
    let ext = &mut options.extension;
    ext.table = true;
    ext.tasklist = true;
    ext.strikethrough = true;
    ext.autolink = true;
    ext.footnotes = true;
    ext.front_matter_delimiter = Some("---".into());
    options
}

pub fn parse(source: &str) -> Vec<Block> {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, source, &options());
    let mut parser = Parser { source: source.lines().collect(), anchors: Anchorizer::new() };
    let mut blocks = Vec::new();
    let mut footnotes = Vec::new();
    for node in root.children() {
        let ast = node.data.borrow();
        match &ast.value {
            // comrak moves referenced definitions to the end, ordered by number, and drops the rest.
            NodeValue::FootnoteDefinition(def) => footnotes.push((
                Footnote { number: footnotes.len() + 1, label: def.name.clone(), blocks: parser.blocks(node) },
                parser.lines(ast.sourcepos),
            )),
            _ => blocks.extend(parser.block(node)),
        }
    }
    let (items, ranges): (Vec<Footnote>, Vec<Lines>) = footnotes.into_iter().unzip();
    if let (Some(start), Some(end)) = (ranges.iter().map(|l| l.start).min(), ranges.iter().map(|l| l.end).max()) {
        blocks.push(Block { kind: BlockKind::Footnotes(items), lines: Lines { start, end } });
    }
    blocks
}

struct Parser<'s> {
    source: Vec<&'s str>,
    anchors: Anchorizer,
}

impl Parser<'_> {
    /// comrak's end position can take in trailing blank lines (indented code, list items, unclosed
    /// fences) or point at column 0 of the next line; trim those so ranges hold only the block.
    fn lines(&self, pos: Sourcepos) -> Lines {
        let start = pos.start.line as u32;
        let mut end = pos.end.line as u32;
        let blank = |line: u32| self.source.get(line as usize - 1).is_none_or(|l| l.trim().is_empty());
        while end > start && blank(end) {
            end -= 1;
        }
        Lines { start, end }
    }

    fn blocks<'a>(&mut self, node: &'a AstNode<'a>) -> Vec<Block> {
        node.children().filter_map(|child| self.block(child)).collect()
    }

    fn block<'a>(&mut self, node: &'a AstNode<'a>) -> Option<Block> {
        let ast = node.data.borrow();
        let kind = match &ast.value {
            NodeValue::Heading(h) => {
                let content = inlines(node);
                let anchor = self.anchors.anchorize(&content.iter().map(|i| i.text.as_str()).collect::<String>());
                BlockKind::Heading { level: h.level, content, anchor }
            }
            NodeValue::Paragraph => match lone_image(node) {
                Some((src, alt)) => BlockKind::Image { src, alt },
                None => BlockKind::Paragraph(inlines(node)),
            },
            NodeValue::List(list) => BlockKind::List {
                ordered: list.list_type == comrak::nodes::ListType::Ordered,
                start: list.start as u64,
                items: node.children().map(|item| self.item(item)).collect(),
            },
            NodeValue::BlockQuote => BlockKind::Quote(self.blocks(node)),
            NodeValue::CodeBlock(code) => {
                let text = code.literal.strip_suffix('\n').unwrap_or(&code.literal).to_string();
                match code.info.split_whitespace().next() {
                    Some("mermaid") => BlockKind::Mermaid(text),
                    lang => BlockKind::Code { lang: lang.map(str::to_string), text },
                }
            }
            NodeValue::Table(info) => BlockKind::Table(table(node, info)),
            NodeValue::ThematicBreak => BlockKind::Rule,
            NodeValue::FrontMatter(text) => BlockKind::FrontMatter(front_matter(text)),
            NodeValue::HtmlBlock(html) => {
                BlockKind::Html(html.literal.strip_suffix('\n').unwrap_or(&html.literal).to_string())
            }
            _ => return None,
        };
        Some(Block { kind, lines: self.lines(ast.sourcepos) })
    }

    fn item<'a>(&mut self, node: &'a AstNode<'a>) -> ListItem {
        let ast = node.data.borrow();
        let task = match &ast.value {
            NodeValue::TaskItem(t) => Some(t.symbol.is_some()),
            _ => None,
        };
        ListItem { task, blocks: self.blocks(node), lines: self.lines(ast.sourcepos) }
    }
}

/// `(src, alt)` if the paragraph is exactly one image.
fn lone_image<'a>(node: &'a AstNode<'a>) -> Option<(String, String)> {
    let only = node.first_child().filter(|c| c.next_sibling().is_none())?;
    match &only.data.borrow().value {
        NodeValue::Image(link) => Some((link.url.clone(), plain_text(only))),
        _ => None,
    }
}

/// Top-level `key: value` lines between the `---` delimiters; anything else is ignored.
fn front_matter(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines().skip(1).take_while(|l| l.trim_end() != "---") {
        if line.starts_with(char::is_whitespace) || line.starts_with(['#', '-']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = unquote(value.trim());
        if !key.trim().is_empty() && !value.is_empty() {
            pairs.push((key.trim().to_string(), value.to_string()));
        }
    }
    pairs
}

fn unquote(s: &str) -> &str {
    ['"', '\'']
        .into_iter()
        .find_map(|q| s.strip_prefix(q)?.strip_suffix(q))
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Align, Inline, Style};

    fn lines(blocks: &[Block]) -> Vec<(u32, u32)> {
        blocks.iter().map(|b| (b.lines.start, b.lines.end)).collect()
    }

    fn plain(text: &str) -> Inline {
        Inline { text: text.into(), style: Style::default() }
    }

    fn styled(text: &str, style: Style) -> Inline {
        Inline { text: text.into(), style }
    }

    #[test]
    fn block_kinds_and_lines() {
        let src = "# Title\n\nPara one\nstill para\n\n---\n\n```rust\nfn main() {}\n\n```\n\n```mermaid\ngraph TD\n```\n\n> quote\n> more\n\n<div>\nhi\n</div>\n\n![alt *text*](a.png)\n";
        let blocks = parse(src);
        assert_eq!(lines(&blocks), [(1, 1), (3, 4), (6, 6), (8, 11), (13, 15), (17, 18), (20, 22), (24, 24)]);
        assert_eq!(blocks[1].kind, BlockKind::Paragraph(vec![plain("Para one still para")]));
        assert_eq!(blocks[2].kind, BlockKind::Rule);
        assert_eq!(blocks[3].kind, BlockKind::Code { lang: Some("rust".into()), text: "fn main() {}\n".into() });
        assert_eq!(blocks[4].kind, BlockKind::Mermaid("graph TD".into()));
        let BlockKind::Quote(inner) = &blocks[5].kind else { panic!("{:?}", blocks[5]) };
        assert_eq!(lines(inner), [(17, 18)]);
        assert_eq!(blocks[6].kind, BlockKind::Html("<div>\nhi\n</div>".into()));
        assert_eq!(blocks[7].kind, BlockKind::Image { src: "a.png".into(), alt: "alt text".into() });
    }

    #[test]
    fn code_blocks() {
        let blocks = parse("```\nplain\n```\n\n``` python  extra\nx\n```\n\n    indented\n\n\n    code\n\n\nafter\n");
        assert_eq!(blocks[0].kind, BlockKind::Code { lang: None, text: "plain".into() });
        assert_eq!(blocks[1].kind, BlockKind::Code { lang: Some("python".into()), text: "x".into() });
        // comrak ends an indented block on the trailing blank lines; the range must not.
        assert_eq!(blocks[2].kind, BlockKind::Code { lang: None, text: "indented\n\n\ncode".into() });
        assert_eq!(lines(&blocks), [(1, 3), (5, 7), (9, 12), (15, 15)]);
    }

    #[test]
    fn unclosed_fence_at_end_of_file() {
        let blocks = parse("text\n\n~~~\nunclosed\n\n");
        assert_eq!(lines(&blocks), [(1, 1), (3, 4)]);
        assert_eq!(blocks[1].kind, BlockKind::Code { lang: None, text: "unclosed\n".into() });
    }

    #[test]
    fn headings_get_unique_github_anchors() {
        let blocks = parse("# Hello, World!\n\n## Hello World\n\n### Hello World\n\nSetext `code`\n------\n\n# 中文 标题\n\n# hello-world-1\n");
        let anchors: Vec<(u8, &str)> = blocks
            .iter()
            .map(|b| match &b.kind {
                BlockKind::Heading { level, anchor, .. } => (*level, anchor.as_str()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            anchors,
            [(1, "hello-world"), (2, "hello-world-1"), (3, "hello-world-2"), (2, "setext-code"), (1, "中文-标题"), (1, "hello-world-1-1")]
        );
        assert_eq!(lines(&blocks)[3], (7, 8));
    }

    #[test]
    fn inline_styles() {
        let blocks = parse("a *b **c** d* `e` ~~f~~ [g](h) <http://i.j> x  \ny\nz ![pic](p.png) <b>raw</b>\n");
        let BlockKind::Paragraph(inl) = &blocks[0].kind else { panic!() };
        let italic = Style { italic: true, ..Style::default() };
        let link = |url: &str| Style { link: Some(url.into()), ..Style::default() };
        assert_eq!(
            inl,
            &[
                plain("a "),
                styled("b ", italic.clone()),
                styled("c", Style { bold: true, italic: true, ..Style::default() }),
                styled(" d", italic),
                plain(" "),
                styled("e", Style { code: true, ..Style::default() }),
                plain(" "),
                styled("f", Style { strike: true, ..Style::default() }),
                plain(" "),
                styled("g", link("h")),
                plain(" "),
                styled("http://i.j", link("http://i.j")),
                plain(" x\ny z pic <b>raw</b>"),
            ]
        );
    }

    #[test]
    fn nested_and_task_lists() {
        let src = "3. one\n   - nested\n     continued\n   - [x] done\n\n4. two\n\n   more\n\n\n- [ ] todo\n- \n";
        let blocks = parse(src);
        assert_eq!(lines(&blocks), [(1, 8), (11, 12)]);
        let BlockKind::List { ordered: true, start: 3, items } = &blocks[0].kind else { panic!("{:?}", blocks[0]) };
        assert_eq!(items.iter().map(|i| (i.lines.start, i.lines.end)).collect::<Vec<_>>(), [(1, 4), (6, 8)]);
        assert_eq!(lines(&items[1].blocks), [(6, 6), (8, 8)]);
        let BlockKind::List { ordered: false, items: nested, .. } = &items[0].blocks[1].kind else { panic!() };
        assert_eq!(nested.iter().map(|i| (i.task, i.lines.start, i.lines.end)).collect::<Vec<_>>(), [(None, 2, 3), (Some(true), 4, 4)]);
        assert_eq!(nested[1].blocks[0].kind, BlockKind::Paragraph(vec![plain("done")]));
        let BlockKind::List { items: tasks, .. } = &blocks[1].kind else { panic!() };
        assert_eq!(tasks.iter().map(|i| i.task).collect::<Vec<_>>(), [Some(false), None]);
        assert!(tasks[1].blocks.is_empty());
    }

    #[test]
    fn tables() {
        let src = "| Name | Qty | Share | Note |\n|:-----|----:|:-----:|------|\n| a | 1,200 | +12% | x |\n| b | $3.50 |  | 9 |\n| c |\n";
        let blocks = parse(src);
        assert_eq!(lines(&blocks), [(1, 5)]);
        let BlockKind::Table(t) = &blocks[0].kind else { panic!() };
        assert_eq!(t.align, [Align::Left, Align::Right, Align::Center, Align::None]);
        assert_eq!(t.numeric, [false, true, true, false]);
        assert_eq!(t.header[1], [plain("Qty")]);
        assert_eq!(t.rows.len(), 3);
        assert!(t.rows.iter().all(|r| r.len() == 4), "short rows are padded");
        assert!(t.rows[2][1].is_empty());
    }

    #[test]
    fn wide_rows_are_truncated_and_empty_columns_are_not_numeric() {
        let blocks = parse("| a | b |\n|---|---|\n|  | 2 | 3 |\n");
        let BlockKind::Table(t) = &blocks[0].kind else { panic!() };
        assert_eq!(t.rows[0].len(), 2);
        assert_eq!(t.numeric, [false, true]);
    }

    #[test]
    fn front_matter() {
        let src = "---\ntitle: \"Hello: World\"\ntags:\n  - a\n# comment\ndate: 2026-09-24\n---\n\n# H\n";
        let blocks = parse(src);
        assert_eq!(lines(&blocks), [(1, 7), (9, 9)]);
        assert_eq!(
            blocks[0].kind,
            BlockKind::FrontMatter(vec![("title".into(), "Hello: World".into()), ("date".into(), "2026-09-24".into())])
        );
    }

    #[test]
    fn front_matter_only_at_line_one() {
        let blocks = parse("\n---\na: 1\n---\n");
        assert!(!blocks.iter().any(|b| matches!(b.kind, BlockKind::FrontMatter(_))), "{blocks:?}");
    }

    #[test]
    fn footnotes_are_numbered_by_first_reference_and_last() {
        let src = "Uses[^b] and[^a] and[^b].\n\n[^a]: Alpha\n    continued.\n\n[^b]: Beta\n\n[^unused]: Never\n\n# After\n";
        let blocks = parse(src);
        assert_eq!(lines(&blocks), [(1, 1), (10, 10), (3, 6)]);
        let BlockKind::Paragraph(inl) = &blocks[0].kind else { panic!() };
        let note = |n| Style { footnote: Some(n), ..Style::default() };
        assert_eq!(
            inl,
            &[plain("Uses"), styled("1", note(1)), plain(" and"), styled("2", note(2)), plain(" and"), styled("1", note(1)), plain(".")]
        );
        let BlockKind::Footnotes(notes) = &blocks[2].kind else { panic!() };
        let summary: Vec<_> = notes.iter().map(|f| (f.number, f.label.as_str(), lines(&f.blocks))).collect();
        assert_eq!(summary, [(1, "b", vec![(6, 6)]), (2, "a", vec![(3, 4)])]);
    }

    #[test]
    fn no_footnotes_block_without_references() {
        let blocks = parse("text\n\n[^x]: orphan\n");
        assert_eq!(lines(&blocks), [(1, 1)]);
    }

    #[test]
    fn link_reference_definitions_are_not_blocks() {
        let blocks = parse("See [it].\n\n[it]: http://x\n");
        assert_eq!(lines(&blocks), [(1, 1)]);
        let BlockKind::Paragraph(inl) = &blocks[0].kind else { panic!() };
        assert_eq!(inl[1], styled("it", Style { link: Some("http://x".into()), ..Style::default() }));
    }

    #[test]
    fn empty_document() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n").is_empty());
    }
}
