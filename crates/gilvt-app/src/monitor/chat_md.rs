//! The 监控官's answers as drawable lines (S2 §6.4): gilvt-markdown parses; this flattens the blocks into headings,
//! paragraphs, list items, quotes, code and rules, each a run of styled spans. Tables, diagrams and images show
//! their source. Pure: the chat panel draws the lines, DebugState lists the session links.

use gilvt_markdown::{Block, BlockKind, Inline};

pub const SESSION_LINK: &str = "gilvt://session/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// The destination as written.
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKind {
    Heading(u8),
    Paragraph,
    /// `marker`: 「•」 or 「3.」.
    Item { depth: usize, marker: String },
    Code,
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdLine {
    /// Nesting depth inside list items (paragraphs and code under an item are indented by it).
    pub indent: usize,
    /// Inside a block quote: draw the quote bar next to `kind`.
    pub quoted: bool,
    pub kind: LineKind,
    pub spans: Vec<Span>,
}

fn spans(inlines: &[Inline]) -> Vec<Span> {
    inlines
        .iter()
        .map(|i| Span { text: i.text.clone(), bold: i.style.bold, italic: i.style.italic, code: i.style.code, link: i.style.link.clone() })
        .collect()
}

fn text_span(text: impl Into<String>, code: bool) -> Vec<Span> {
    vec![Span { text: text.into(), bold: false, italic: false, code, link: None }]
}

fn push_block(b: &Block, depth: usize, out: &mut Vec<MdLine>) {
    let line = |kind: LineKind, spans: Vec<Span>| MdLine { indent: depth, quoted: false, kind, spans };
    match &b.kind {
        BlockKind::Heading { level, content, .. } => out.push(line(LineKind::Heading(*level), spans(content))),
        BlockKind::Paragraph(content) => out.push(line(LineKind::Paragraph, spans(content))),
        BlockKind::List { ordered, start, items } => {
            for (n, item) in items.iter().enumerate() {
                let marker = if *ordered { format!("{}.", *start + n as u64) } else { "•".to_string() };
                let mut blocks = item.blocks.iter();
                // The item's first paragraph shares the marker's line.
                let first = match blocks.next() {
                    Some(Block { kind: BlockKind::Paragraph(c), .. }) => spans(c),
                    Some(other) => {
                        out.push(line(LineKind::Item { depth, marker: marker.clone() }, Vec::new()));
                        push_block(other, depth + 1, out);
                        blocks.for_each(|b| push_block(b, depth + 1, out));
                        continue;
                    }
                    None => Vec::new(),
                };
                let mut first = first;
                if let Some(done) = item.task {
                    first.insert(0, Span { text: (if done { "☑ " } else { "☐ " }).into(), bold: false, italic: false, code: false, link: None });
                }
                out.push(line(LineKind::Item { depth, marker }, first));
                blocks.for_each(|b| push_block(b, depth + 1, out));
            }
        }
        BlockKind::Quote(inner) => {
            for b in inner {
                let mut tmp = Vec::new();
                push_block(b, depth, &mut tmp);
                out.extend(tmp.into_iter().map(|l| MdLine { quoted: true, ..l }));
            }
        }
        BlockKind::Code { text, .. } | BlockKind::Mermaid(text) | BlockKind::Html(text) => {
            for l in text.split('\n') {
                out.push(line(LineKind::Code, text_span(l, true)));
            }
        }
        BlockKind::Table(t) => {
            let row = |cells: &[Vec<Inline>]| cells.iter().map(|c| c.iter().map(|i| i.text.as_str()).collect::<String>()).collect::<Vec<_>>().join(" | ");
            out.push(line(LineKind::Code, text_span(row(&t.header), true)));
            for r in &t.rows {
                out.push(line(LineKind::Code, text_span(row(r), true)));
            }
        }
        BlockKind::Rule => out.push(line(LineKind::Rule, Vec::new())),
        BlockKind::Image { alt, src } => {
            let image = if crate::i18n::english() { format!("[Image: {alt}] {src}") } else { format!("[图片：{}] {src}", alt) };
            out.push(line(LineKind::Paragraph, text_span(image, false)))
        }
        BlockKind::FrontMatter(_) | BlockKind::Footnotes(_) => {}
    }
}

pub fn lines(md: &str) -> Vec<MdLine> {
    let mut out = Vec::new();
    for b in gilvt_markdown::parse(md) {
        push_block(&b, 0, &mut out);
    }
    out
}

/// The key of a `gilvt://session/<key>` link.
pub fn session_key(target: &str) -> Option<&str> {
    target.strip_prefix(SESSION_LINK).filter(|k| !k.is_empty())
}

/// The session links of `md` in reading order: (text, key).
pub fn session_links(md: &str) -> Vec<(String, String)> {
    lines(md)
        .iter()
        .flat_map(|l| &l.spans)
        .filter_map(|s| Some((s.text.clone(), session_key(s.link.as_deref()?)?.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &MdLine) -> String {
        l.spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn briefing_b_layout() {
        let md = "### 要你处理\n- [api-refactor](gilvt://session/agent:claude:a1)：批准 `rm -rf build`\n- [zsh · ~/repo](gilvt://session/pane:3)：看超时用例\n\n### 整体\n**web-login** 快完成了。";
        let l = lines(md);
        assert_eq!(l[0].kind, LineKind::Heading(3));
        assert_eq!(plain(&l[0]), "要你处理");
        assert_eq!(l[1].kind, LineKind::Item { depth: 0, marker: "•".into() });
        assert_eq!(l[1].spans[0], Span { text: "api-refactor".into(), bold: false, italic: false, code: false, link: Some("gilvt://session/agent:claude:a1".into()) });
        assert!(l[1].spans.iter().any(|s| s.code && s.text == "rm -rf build"));
        assert_eq!(l[3].kind, LineKind::Heading(3));
        assert!(l[4].spans[0].bold && l[4].spans[0].text == "web-login");
        assert_eq!(session_links(md), vec![("api-refactor".to_string(), "agent:claude:a1".to_string()), ("zsh · ~/repo".to_string(), "pane:3".to_string())]);
    }

    #[test]
    fn lists_quotes_code_and_rules() {
        let md = "1. 一\n   - 嵌套\n2. 二\n\n> 引用\n\n```sh\nmake test\n```\n\n---\n";
        let l = lines(md);
        let kinds: Vec<&LineKind> = l.iter().map(|x| &x.kind).collect();
        assert_eq!(kinds[0], &LineKind::Item { depth: 0, marker: "1.".into() });
        assert_eq!(kinds[1], &LineKind::Item { depth: 1, marker: "•".into() });
        assert_eq!(kinds[2], &LineKind::Item { depth: 0, marker: "2.".into() });
        assert_eq!((kinds[3], l[3].quoted), (&LineKind::Paragraph, true));
        assert_eq!((kinds[4], plain(&l[4]).as_str(), l[4].quoted), (&LineKind::Code, "make test", false));
        assert_eq!(kinds[5], &LineKind::Rule);
    }

    #[test]
    fn half_written_markdown_still_reads() {
        let l = lines("正在看 [web-lo");
        assert_eq!(plain(&l[0]), "正在看 [web-lo");
        assert!(session_links("**未闭合").is_empty());
    }

    #[test]
    fn only_session_links_count() {
        assert_eq!(session_key("gilvt://session/pane:3"), Some("pane:3"));
        assert_eq!(session_key("https://example.com"), None);
        assert_eq!(session_key("gilvt://session/"), None);
        assert!(session_links("[文档](https://example.com)").is_empty());
    }

    #[test]
    fn tables_and_diagrams_show_their_source() {
        let l = lines("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(l.iter().any(|x| plain(x).contains("1") && plain(x).contains("2")));
        let l = lines("```mermaid\ngraph TD; A-->B\n```");
        assert_eq!((l[0].kind.clone(), plain(&l[0])), (LineKind::Code, "graph TD; A-->B".to_string()));
    }

    #[test]
    fn quotes_keep_their_inner_kind() {
        let l = lines("> - 一\n>\n> ```\n> x\n> ```\n>\n> ---\n");
        assert_eq!(l[0].kind, LineKind::Item { depth: 0, marker: "•".into() });
        assert!(l.iter().all(|x| x.quoted));
        assert!(l.iter().any(|x| x.kind == LineKind::Code && plain(x) == "x"));
        assert!(l.iter().any(|x| x.kind == LineKind::Rule));
    }

    #[test]
    fn content_under_items_is_indented_and_code_is_per_line() {
        let l = lines("- 项\n\n  段落\n\n  ```\n  a\n  b\n  ```\n");
        let para = l.iter().find(|x| plain(x) == "段落").unwrap();
        assert_eq!((para.kind.clone(), para.indent), (LineKind::Paragraph, 1));
        let code: Vec<(String, usize)> = l.iter().filter(|x| x.kind == LineKind::Code).map(|x| (plain(x), x.indent)).collect();
        assert_eq!(code, [("a".to_string(), 1), ("b".to_string(), 1)]);
        assert_eq!(l[0].indent, 0);
    }

    #[test]
    fn task_items_get_a_box() {
        let l = lines("- [x] 做完\n- [ ] 待办\n");
        assert_eq!(plain(&l[0]), "☑ 做完");
        assert_eq!(plain(&l[1]), "☐ 待办");
    }
}
