//! Syntax highlighting with syntect (pure-Rust regex engine), producing per-line colored spans.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Byte range within the line (newline excluded).
    pub range: Range<usize>,
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// What a piece of text is, independent of any color theme; the app maps classes to colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenClass {
    Comment, String, Number, Keyword, Function, Type, Constant, Escape, Key, Tag,
    Heading, Bold, Italic, Code, Link, Operator, Invalid,
}

impl TokenClass {
    pub const ALL: [TokenClass; 17] = {
        use TokenClass::*;
        [Comment, String, Number, Keyword, Function, Type, Constant, Escape, Key, Tag, Heading, Bold, Italic, Code, Link, Operator, Invalid]
    };

    pub fn name(self) -> &'static str {
        use TokenClass::*;
        match self {
            Comment => "comment", String => "string", Number => "number", Keyword => "keyword", Function => "function",
            Type => "type", Constant => "constant", Escape => "escape", Key => "key", Tag => "tag", Heading => "heading",
            Bold => "bold", Italic => "italic", Code => "code", Link => "link", Operator => "operator", Invalid => "invalid",
        }
    }
}

/// Scope prefix → class; within one scope the first match wins, so specific rules come before general ones.
const RULES: &[(&str, TokenClass)] = {
    use TokenClass::*;
    &[
        ("comment", Comment), ("constant.character.escape", Escape), ("string.other.link", Link), ("string", String),
        ("constant.numeric", Number), ("constant.language", Constant), ("markup.heading", Heading), ("markup.bold", Bold),
        ("markup.italic", Italic), ("markup.raw", Code), ("markup.inline.raw", Code), ("markup.underline.link", Link),
        ("entity.name.function", Function), ("support.function", Function), ("entity.name.tag.yaml", Key),
        ("entity.name.key", Key), ("support.type.property-name", Key), ("entity.name.tag", Tag),
        ("entity.name.type", Type), ("entity.name.class", Type), ("entity.name.struct", Type), ("entity.name.enum", Type),
        ("entity.name.trait", Type),
        // JSON keys are `string.quoted` inside a key context; `class_of` lets Key win over the string scope.
        ("meta.structure.dictionary.key", Key),
        // `storage` (below) catches `storage.type`: primitives like `i32` render as Keyword because Rust gives
        // `let` and `i32` the same `storage.type.rust` scope; names of declared types are `entity.name.*` (Type).
        ("support.type", Type), ("support.class", Type),
        ("keyword.operator", Operator), ("keyword", Keyword), ("storage", Keyword), ("invalid", Invalid),
    ]
};

/// The class of one scope name (`string.quoted.double.rust`), or `None` when no rule covers it.
pub fn classify(scope: &str) -> Option<TokenClass> {
    RULES.iter().find(|(p, _)| scope == *p || scope.strip_prefix(p).is_some_and(|r| r.starts_with('.'))).map(|(_, c)| *c)
}

/// syntect's defaults plus the TOML syntax it lacks (hand-written, `syntaxes/toml.sublime-syntax`).
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let mut b = SyntaxSet::load_defaults_newlines().into_builder();
        let toml = SyntaxDefinition::load_from_str(include_str!("../syntaxes/toml.sublime-syntax"), true, None).expect("embedded TOML syntax parses");
        b.add(toml);
        b.build()
    })
}

fn theme(appearance: Appearance) -> &'static Theme {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let set = THEMES.get_or_init(ThemeSet::load_defaults);
    let name = match appearance {
        Appearance::Light => "InspiredGitHub",
        Appearance::Dark => "base16-ocean.dark",
    };
    &set.themes[name]
}

/// Loads the syntax and theme sets (~0.1 s) so the first preview does not pay for it.
pub fn preload() {
    syntaxes();
    theme(Appearance::Light);
    theme(Appearance::Dark);
}

fn find_syntax(token: &str, first_line: &str) -> &'static SyntaxReference {
    let set = syntaxes();
    set.find_syntax_by_token(token)
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

/// Human-readable language name for `token` (e.g. "Rust"), or "Plain Text".
pub fn language_name(token: &str, text: &str) -> String {
    find_syntax(token, text.lines().next().unwrap_or("")).name.clone()
}

/// Highlights `text`; returns one span list per line (`text.lines()` order).
pub fn highlight(text: &str, token: &str, appearance: Appearance) -> Vec<Vec<Span>> {
    let syntax = find_syntax(token, text.lines().next().unwrap_or(""));
    let mut h = HighlightLines::new(syntax, theme(appearance));
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        let content_len = line.trim_end_matches(['\n', '\r']).len();
        let mut spans = Vec::new();
        let mut pos = 0;
        let regions = h.highlight_line(line, syntaxes()).unwrap_or_default();
        for (style, piece) in regions {
            let start = pos;
            pos += piece.len();
            let end = pos.min(content_len);
            if start >= end {
                continue;
            }
            spans.push(Span {
                range: start..end,
                fg: Color { r: style.foreground.r, g: style.foreground.g, b: style.foreground.b },
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            });
        }
        out.push(spans);
    }
    out
}

/// Spans for every line of `diff`: old and new texts are highlighted as whole files (so
/// multi-line strings and comments color correctly), then mapped back by line number.
pub fn highlight_diff(diff: &crate::diff::Diff, token: &str, appearance: Appearance) -> Vec<Vec<Span>> {
    use crate::diff::LineKind;
    let side = |keep: LineKind| -> String {
        let mut s = String::new();
        for l in diff.lines.iter().filter(|l| l.kind == LineKind::Context || l.kind == keep) {
            s.push_str(&l.text);
            s.push('\n');
        }
        s
    };
    let old = highlight(&side(LineKind::Removed), token, appearance);
    let new = highlight(&side(LineKind::Added), token, appearance);
    diff.lines
        .iter()
        .map(|l| {
            let spans = match l.kind {
                LineKind::Removed => l.old_no.and_then(|n| old.get(n as usize - 1)),
                _ => l.new_no.and_then(|n| new.get(n as usize - 1)),
            };
            spans.cloned().unwrap_or_default()
        })
        .collect()
}

/// Default text color of the theme (for gutters and plain regions).
pub fn default_fg(appearance: Appearance) -> Color {
    let c = theme(appearance).settings.foreground.unwrap_or(syntect::highlighting::Color::BLACK);
    Color { r: c.r, g: c.g, b: c.b }
}

pub const CHECKPOINT_EVERY: usize = 64;
pub const SPAN_CACHE_MAX: usize = 2048;
/// A longer line is not parsed (regex cost); it gets no color and the state carries on unchanged.
pub const MAX_LINE_BYTES: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassSpan {
    /// Byte range within the line (newline excluded).
    pub range: Range<usize>,
    pub class: TokenClass,
}

/// Which grammar the parser state belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The file's own syntax.
    Main,
    /// Inside Markdown's YAML front matter (`state` is a YAML parse state).
    FrontMatter,
}

/// The parser state between two lines.
#[derive(Clone)]
struct LineState {
    state: ParseState,
    stack: ScopeStack,
    mode: Mode,
}

impl LineState {
    fn new(syntax: &SyntaxReference, mode: Mode) -> LineState {
        LineState { state: ParseState::new(syntax), stack: ScopeStack::new(), mode }
    }
}

/// Incremental highlighter: parse state is checkpointed every `CHECKPOINT_EVERY` lines and per-line spans are cached,
/// so only the visible lines (plus at most 63 before them) are parsed after an edit.
pub struct Highlighter {
    syntax: &'static SyntaxReference,
    /// YAML, when `syntax` is Markdown (for its front matter).
    front_matter: Option<&'static SyntaxReference>,
    /// State before line `i * CHECKPOINT_EVERY`.
    checkpoints: Vec<LineState>,
    /// `(n, state before line n)` where the last fill stopped, so the next line down continues from there.
    frontier: Option<(usize, LineState)>,
    cache: HashMap<usize, Vec<ClassSpan>>,
    line_count: usize,
    scope_classes: HashMap<Scope, Option<TokenClass>>,
}

impl Highlighter {
    /// `token`: extension or file name; `first_line` is used for shebangs. Plain text gives `None`.
    pub fn new(token: &str, first_line: &str) -> Option<Highlighter> {
        let set = syntaxes();
        let syntax = set.find_syntax_by_token(token).or_else(|| set.find_syntax_by_first_line(first_line))?;
        if syntax.name == "Plain Text" {
            return None;
        }
        let front_matter = if syntax.name == "Markdown" { set.find_syntax_by_name("YAML") } else { None };
        Some(Highlighter {
            syntax,
            front_matter,
            checkpoints: vec![LineState::new(syntax, Mode::Main)],
            frontier: None,
            cache: HashMap::new(),
            line_count: 0,
            scope_classes: HashMap::new(),
        })
    }

    pub fn language(&self) -> &str {
        &self.syntax.name
    }

    pub fn set_line_count(&mut self, n: usize) {
        self.line_count = n;
    }

    /// Checkpoints and cached lines at or after `line` are dropped.
    pub fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate(line / CHECKPOINT_EVERY + 1);
        // The frontier is the state *before* its line, so it stays valid when that line itself changed.
        if self.frontier.as_ref().is_some_and(|(next, _)| *next > line) {
            self.frontier = None;
        }
        self.cache.retain(|l, _| *l < line);
    }

    /// Whether `line`'s spans are cached, i.e. asking for them parses nothing (for tests of invalidation).
    pub fn is_cached(&self, line: usize) -> bool {
        self.cache.contains_key(&line)
    }

    pub fn spans(&mut self, line: usize, text_of: &dyn Fn(usize) -> String) -> &[ClassSpan] {
        if line >= self.line_count {
            return &[];
        }
        if !self.cache.contains_key(&line) {
            self.fill(line, text_of);
        }
        self.cache.get(&line).map_or(&[], Vec::as_slice)
    }

    fn fill(&mut self, line: usize, text_of: &dyn Fn(usize) -> String) {
        let k = (line / CHECKPOINT_EVERY).min(self.checkpoints.len() - 1);
        let from_checkpoint = k * CHECKPOINT_EVERY;
        // Resume where the previous fill stopped when that is between the checkpoint and `line`: rows are asked
        // for top to bottom, and restarting at the checkpoint each time would re-parse the block quadratically.
        let (start, mut st) = match self.frontier.take() {
            Some((next_line, st)) if (from_checkpoint..=line).contains(&next_line) => (next_line, st),
            _ => (from_checkpoint, self.checkpoints[k].clone()),
        };
        for l in start..=line {
            if l % CHECKPOINT_EVERY == 0 && l / CHECKPOINT_EVERY == self.checkpoints.len() {
                self.checkpoints.push(st.clone());
            }
            let spans = self.advance(&mut st, l, &text_of(l));
            if self.cache.len() >= SPAN_CACHE_MAX {
                self.cache.clear();
            }
            self.cache.insert(l, spans);
        }
        self.frontier = Some((line + 1, st));
    }

    /// Parses line `l` from `st` (the state before it) and leaves `st` as the state after it. For Markdown this is
    /// also where YAML front matter is entered (line 0 is `---`) and left (a later `---` or `...` line); the
    /// delimiter lines get no class, and the Markdown after the closing one starts from a fresh state.
    fn advance(&mut self, st: &mut LineState, l: usize, text: &str) -> Vec<ClassSpan> {
        if let Some(yaml) = self.front_matter {
            let delimiter = text.trim_end();
            match st.mode {
                Mode::Main if l == 0 && delimiter == "---" => {
                    *st = LineState::new(yaml, Mode::FrontMatter);
                    return Vec::new();
                }
                Mode::FrontMatter if delimiter == "---" || delimiter == "..." => {
                    *st = LineState::new(self.syntax, Mode::Main);
                    return Vec::new();
                }
                _ => {}
            }
        }
        self.parse_line(&mut st.state, &mut st.stack, text)
    }

    fn parse_line(&mut self, state: &mut ParseState, stack: &mut ScopeStack, text: &str) -> Vec<ClassSpan> {
        if text.len() > MAX_LINE_BYTES {
            return Vec::new();
        }
        let mut with_nl = String::with_capacity(text.len() + 1);
        with_nl.push_str(text);
        with_nl.push('\n');
        let Ok(ops) = state.parse_line(&with_nl, syntaxes()) else { return Vec::new() };
        let content = text.len();
        let mut out: Vec<ClassSpan> = Vec::new();
        let (mut pos, mut cur) = (0, self.class_of(stack));
        for (off, op) in ops {
            let off = off.min(content);
            if off > pos {
                push_span(&mut out, pos..off, cur);
                pos = off;
            }
            let _ = stack.apply(&op);
            cur = self.class_of(stack);
        }
        if content > pos {
            push_span(&mut out, pos..content, cur);
        }
        out
    }

    /// The innermost scope on the stack that has a class decides, except that a Key context anywhere on the stack wins
    /// (a JSON key is a string scope nested in a key scope). So a Key scope must wrap only the key text.
    fn class_of(&mut self, stack: &ScopeStack) -> Option<TokenClass> {
        let classes = &mut self.scope_classes;
        let mut innermost = None;
        for s in stack.as_slice().iter().rev() {
            let c = *classes.entry(*s).or_insert_with(|| classify(&s.build_string()));
            if c == Some(TokenClass::Key) {
                return c;
            }
            innermost = innermost.or(c);
        }
        innermost
    }
}

fn push_span(out: &mut Vec<ClassSpan>, range: Range<usize>, class: Option<TokenClass>) {
    let Some(class) = class else { return };
    match out.last_mut() {
        Some(prev) if prev.class == class && prev.range.end == range.start => prev.range.end = range.end,
        _ => out.push(ClassSpan { range, class }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_map_to_classes_with_specific_rules_first() {
        use TokenClass::*;
        let c = classify;
        assert_eq!(c("comment.line.double-slash.rust"), Some(Comment));
        assert_eq!(c("string.quoted.double.rust"), Some(String));
        assert_eq!(c("constant.character.escape.rust"), Some(Escape));
        assert_eq!(c("constant.numeric.integer.decimal.rust"), Some(Number));
        assert_eq!(c("constant.language.boolean.rust"), Some(Constant));
        assert_eq!(c("markup.heading.markdown"), Some(Heading));
        assert_eq!(c("markup.raw.inline.markdown"), Some(Code));
        assert_eq!(c("markup.inline.raw.string.markdown"), Some(Code));
        assert_eq!(c("entity.name.function.rust"), Some(Function));
        assert_eq!(c("entity.name.tag.yaml"), Some(Key));
        assert_eq!(c("entity.name.tag.html"), Some(Tag));
        assert_eq!(c("support.type.property-name.json"), Some(Key));
        assert_eq!(c("support.type.rust"), Some(Type));
        assert_eq!(c("keyword.operator.assignment.rust"), Some(Operator));
        assert_eq!(c("keyword.control.rust"), Some(Keyword));
        assert_eq!(c("storage.type.rust"), Some(Keyword)); // `let` and `i32` share this scope in the Rust grammar
        assert_eq!(c("meta.structure.dictionary.key.json"), Some(Key));
        assert_eq!(c("storage.type.function.rust"), Some(Keyword));
        assert_eq!(c("storage.modifier.rust"), Some(Keyword));
        assert_eq!(c("invalid.illegal.rust"), Some(Invalid));
    }

    #[test]
    fn unrelated_and_prefix_lookalike_scopes_do_not_match() {
        assert_eq!(classify("source.rust"), None);
        assert_eq!(classify("meta.function.rust"), None);
        assert_eq!(classify("punctuation.definition.string.begin.rust"), None);
        assert_eq!(classify("commentary.x"), None); // a prefix must end at a `.` boundary
        assert_eq!(classify("stringy"), None);
        assert_eq!(classify(""), None);
    }

    #[test]
    fn class_names_are_snake_case_and_all_is_complete() {
        assert_eq!(TokenClass::ALL.len(), 17);
        let names: Vec<_> = TokenClass::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names[0], "comment");
        assert_eq!(names[16], "invalid");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 17);
        assert!(names.iter().all(|n| n.chars().all(|c| c.is_ascii_lowercase())));
    }

    #[test]
    fn toml_is_a_known_language_and_gets_colors() {
        assert_eq!(language_name("toml", ""), "TOML");
        let lines = highlight("name = \"x\" # c\n[server]\nport = 8080\n", "toml", Appearance::Dark);
        assert_eq!(lines.len(), 3);
        let distinct = |l: &Vec<Span>| {
            let mut c: Vec<_> = l.iter().map(|s| s.fg).collect();
            c.sort_by_key(|c| (c.r, c.g, c.b));
            c.dedup();
            c.len()
        };
        assert!(distinct(&lines[0]) >= 3, "key, string and comment differ: {:?}", lines[0]);
        assert!(distinct(&lines[2]) >= 2, "key and number differ: {:?}", lines[2]);
    }

    #[test]
    fn highlights_rust_keywords_differently_from_identifiers() {
        let lines = highlight("fn main() {\n    let x = 1;\n}\n", "rs", Appearance::Light);
        assert_eq!(lines.len(), 3);
        let first = &lines[0];
        let fn_span = first.iter().find(|s| s.range == (0..2)).expect("`fn` has its own span");
        let main_span = first.iter().find(|s| s.range.start == 3).expect("`main` span");
        assert_ne!(fn_span.fg, main_span.fg);
        // Spans cover the line without the newline.
        assert_eq!(first.last().unwrap().range.end, "fn main() {".len());
    }

    #[test]
    fn unknown_types_fall_back_to_plain_text() {
        assert_eq!(language_name("nosuchlang", "hello"), "Plain Text");
        assert_eq!(language_name("rs", ""), "Rust");
        assert_eq!(language_name("Makefile", "all:"), "Makefile");
        let lines = highlight("hello\nworld", "nosuchlang", Appearance::Dark);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1][0].range, 0..5);
    }

    #[test]
    fn shebang_detection() {
        assert_eq!(language_name("script", "#!/bin/bash"), "Bourne Again Shell (bash)");
    }

    #[test]
    fn diff_lines_get_spans_from_their_side() {
        let d = crate::diff::diff_texts("/* a\nb */\nlet x = 1;\n", "/* a\nb */\nlet y = 2;\n");
        let spans = highlight_diff(&d, "rs", Appearance::Light);
        assert_eq!(spans.len(), d.lines.len());
        // Line 2 is inside a block comment on both sides: one comment-colored span.
        let comment_color = spans[0][0].fg;
        assert!(spans[1].iter().all(|s| s.fg == comment_color));
        // The removed and added lines are highlighted as code, not comment.
        let removed = d.lines.iter().position(|l| l.kind == crate::diff::LineKind::Removed).unwrap();
        assert!(spans[removed].iter().any(|s| s.fg != comment_color));
    }

    #[test]
    fn crlf_is_excluded_from_spans() {
        let lines = highlight("a\r\nb\r\n", "txt", Appearance::Light);
        assert_eq!(lines[0].last().unwrap().range.end, 1);
    }

    fn lines_of(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }
    fn hl(token: &str, text: &str) -> (Highlighter, Vec<String>) {
        let lines = lines_of(text);
        let mut h = Highlighter::new(token, lines[0].as_str()).expect("known language");
        h.set_line_count(lines.len());
        (h, lines)
    }
    /// Classes of `line` as (text, class) pairs.
    fn classes(h: &mut Highlighter, lines: &[String], line: usize) -> Vec<(String, TokenClass)> {
        h.spans(line, &|i| lines[i].clone()).iter().map(|s| (lines[line][s.range.clone()].to_string(), s.class)).collect()
    }
    fn has(v: &[(String, TokenClass)], text: &str, class: TokenClass) -> bool {
        v.iter().any(|(t, c)| t == text && *c == class)
    }

    #[test]
    fn plain_text_has_no_highlighter() {
        assert!(Highlighter::new("txt", "hello").is_none());
        assert!(Highlighter::new("nosuchlang", "hello").is_none());
        assert!(Highlighter::new("script", "#!/bin/bash").is_some());
    }

    #[test]
    fn rust_classes() {
        use TokenClass::*;
        let (mut h, l) = hl("rs", "fn main() { let x = \"a\\n\"; 42 } // hi");
        assert_eq!(h.language(), "Rust");
        let v = classes(&mut h, &l, 0);
        assert!(has(&v, "fn", Keyword), "{v:?}");
        assert!(has(&v, "main", Function), "{v:?}");
        assert!(has(&v, "let", Keyword), "{v:?}");
        assert!(v.iter().any(|(t, c)| *c == String && t.contains('a')), "{v:?}");
        assert!(has(&v, "\\n", Escape), "{v:?}");
        assert!(has(&v, "42", Number), "{v:?}");
        assert!(v.iter().any(|(t, c)| *c == Comment && t.contains("hi")), "{v:?}");
    }

    #[test]
    fn markdown_fences_are_one_code_span_embedding_is_not_supported_by_the_bundled_grammar() {
        use TokenClass::*;
        let (mut h, l) = hl("md", "# Title\n\nuse `x` and **bold**\n\n```rust\nfn f() {}\n```");
        assert_eq!(h.language(), "Markdown");
        assert!(classes(&mut h, &l, 0).iter().any(|(_, c)| *c == Heading));
        let v = classes(&mut h, &l, 2);
        assert!(v.iter().any(|(t, c)| *c == Code && t.contains('x')), "{v:?}");
        assert!(v.iter().any(|(t, c)| *c == Bold && t.contains("bold")), "{v:?}");
        let v = classes(&mut h, &l, 5);
        // syntect's bundled Markdown grammar does not embed other languages: a fenced block is one Code span.
        assert!(has(&v, "fn f() {}", Code), "fenced block is Code: {v:?}");
    }

    #[test]
    fn markdown_front_matter_is_yaml_and_the_markdown_after_it_starts_fresh() {
        use TokenClass::*;
        let (mut h, l) = hl("md", "---  \nname: x\ndescription: \"y\" # c\n---\n# T\n\ntext");
        assert!(classes(&mut h, &l, 0).is_empty(), "opening delimiter has no class");
        assert!(has(&classes(&mut h, &l, 1), "name", Key));
        let v = classes(&mut h, &l, 2);
        assert!(has(&v, "description", Key), "{v:?}");
        assert!(v.iter().any(|(t, c)| *c == String && t.contains('y')), "{v:?}");
        assert!(v.iter().any(|(_, c)| *c == Comment), "{v:?}");
        assert!(classes(&mut h, &l, 3).is_empty(), "closing delimiter is not a setext heading");
        assert!(classes(&mut h, &l, 4).iter().any(|(_, c)| *c == Heading));
        // `...` also closes it.
        let (mut h, l) = hl("md", "---\na: 1\n...\n# T");
        assert!(classes(&mut h, &l, 2).is_empty());
        assert!(classes(&mut h, &l, 3).iter().any(|(_, c)| *c == Heading));
    }

    #[test]
    fn unclosed_front_matter_and_later_rules_are_sane() {
        use TokenClass::*;
        // Never closed: YAML to the end of the file.
        let (mut h, l) = hl("md", "---\na: 1\n# not a heading");
        assert!(has(&classes(&mut h, &l, 1), "a", Key));
        assert!(!classes(&mut h, &l, 2).iter().any(|(_, c)| *c == Heading));
        // `---` after line 0 is plain Markdown (a thematic break, or a setext underline), never front matter.
        let (mut h, l) = hl("md", "# T\n\n---\nname: x\n---");
        assert!(classes(&mut h, &l, 0).iter().any(|(_, c)| *c == Heading));
        assert!(!classes(&mut h, &l, 3).iter().any(|(_, c)| *c == Key));
        // Other languages are unaffected by a leading `---` (YAML documents start with it themselves).
        let (mut h, l) = hl("yaml", "---\nname: x");
        assert!(has(&classes(&mut h, &l, 1), "name", Key));
    }

    #[test]
    fn adding_and_removing_the_opening_delimiter_recolours_the_whole_file() {
        let mut lines = lines_of("name: x\n---\n# T\nbody");
        let mut h = Highlighter::new("md", "").unwrap();
        h.set_line_count(lines.len());
        let snapshot = |h: &mut Highlighter, lines: &Vec<String>| (0..lines.len()).map(|i| classes(h, lines, i)).collect::<Vec<_>>();
        let fresh = |lines: &Vec<String>| {
            let mut f = Highlighter::new("md", "").unwrap();
            f.set_line_count(lines.len());
            snapshot(&mut f, lines)
        };
        let before = snapshot(&mut h, &lines);
        lines.insert(0, "---".into());
        h.invalidate_from(0);
        h.set_line_count(lines.len());
        let with = snapshot(&mut h, &lines);
        assert_eq!(with, fresh(&lines));
        assert!(has(&with[1], "name", TokenClass::Key), "{:?}", with[1]);
        lines.remove(0);
        h.invalidate_from(0);
        h.set_line_count(lines.len());
        assert_eq!(snapshot(&mut h, &lines), before);
    }

    #[test]
    fn yaml_json_shell_toml() {
        use TokenClass::*;
        let (mut h, l) = hl("yaml", "name: code-review\ncount: 2 # c");
        assert!(has(&classes(&mut h, &l, 0), "name", Key));
        assert!(classes(&mut h, &l, 1).iter().any(|(_, c)| *c == Comment));
        let (mut h, l) = hl("json", "{\"a\": \"b\", \"n\": 1, \"t\": true}");
        let v = classes(&mut h, &l, 0);
        assert!(v.iter().any(|(t, c)| *c == Key && t.contains('a')), "{v:?}");
        assert!(v.iter().any(|(t, c)| *c == String && t.contains('b')), "{v:?}");
        assert!(has(&v, "1", Number) && has(&v, "true", Constant), "{v:?}");
        let (mut h, l) = hl("sh", "if [ -f x ]; then echo \"hi\"; fi # c");
        let v = classes(&mut h, &l, 0);
        assert!(has(&v, "if", Keyword) && has(&v, "fi", Keyword), "{v:?}");
        assert!(v.iter().any(|(t, c)| *c == String && t.contains("hi")), "{v:?}");
        assert!(v.iter().any(|(_, c)| *c == Comment));
        let (mut h, l) = hl("toml", "[a.b]\nname = \"x\\t\"\nport = 8080\nok = true\nd = 1979-05-27T07:32:00Z\n# c");
        assert_eq!(h.language(), "TOML");
        assert!(classes(&mut h, &l, 0).iter().any(|(_, c)| *c == Heading));
        let v = classes(&mut h, &l, 1);
        assert!(has(&v, "name", Key) && has(&v, "\\t", Escape), "{v:?}");
        assert!(has(&classes(&mut h, &l, 2), "8080", Number));
        assert!(has(&classes(&mut h, &l, 3), "true", Constant));
        assert!(has(&classes(&mut h, &l, 4), "1979-05-27T07:32:00Z", Number));
        assert!(classes(&mut h, &l, 5).iter().any(|(_, c)| *c == Comment));
    }

    #[test]
    fn a_block_comment_opened_then_closed_updates_the_lines_below() {
        use TokenClass::*;
        let mut lines = lines_of("fn a() {}\nlet x = 1;\nlet y = 2;");
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(lines.len());
        let snapshot = |h: &mut Highlighter, lines: &Vec<std::string::String>| -> Vec<Vec<(std::string::String, TokenClass)>> { (0..lines.len()).map(|i| classes(h, lines, i)).collect() };
        let before = snapshot(&mut h, &lines);
        assert!(has(&before[2], "let", Keyword));
        // Type `/*` at the start of line 0: everything below becomes a comment.
        lines[0] = format!("/*{}", lines[0]);
        h.invalidate_from(0);
        let during = snapshot(&mut h, &lines);
        assert!(during[2].iter().all(|(_, c)| *c == Comment) && !during[2].is_empty(), "{:?}", during[2]);
        // Delete it again: back to what it was.
        lines[0] = lines[0][2..].to_string();
        h.invalidate_from(0);
        assert_eq!(snapshot(&mut h, &lines), before);
    }

    #[test]
    fn editing_a_later_line_keeps_earlier_lines_cached_and_correct() {
        let mut lines: Vec<String> = (0..200).map(|i| format!("let v{i} = {i}; // c")).collect();
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(lines.len());
        let first = classes(&mut h, &lines, 10);
        lines[150] = "/* open".into();
        h.invalidate_from(150);
        assert_eq!(classes(&mut h, &lines, 10), first);
        assert!(classes(&mut h, &lines, 199).iter().all(|(_, c)| *c == TokenClass::Comment));
    }

    #[test]
    fn checkpoints_work_across_the_64_line_boundary() {
        // A block comment opens on line 3 and closes on line 130: lines 64 and 128 start inside it.
        let mut lines: Vec<String> = (0..140).map(|i| format!("let a{i} = 1;")).collect();
        lines[3] = "/*".into();
        lines[130] = "*/ let z = 2;".into();
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(lines.len());
        // Ask for a far line first (parses from the last checkpoint it can), then an earlier one.
        assert!(classes(&mut h, &lines, 135).iter().any(|(t, c)| t == "let" && *c == TokenClass::Keyword));
        assert!(classes(&mut h, &lines, 100).iter().all(|(_, c)| *c == TokenClass::Comment));
        assert!(classes(&mut h, &lines, 64).iter().all(|(_, c)| *c == TokenClass::Comment));
    }

    #[test]
    fn a_very_long_line_is_skipped_without_disturbing_the_next_line() {
        let long = format!("let s = \"{}\";", "x".repeat(MAX_LINE_BYTES + 10));
        let lines = vec![long, "let y = 1; /* c */".to_string()];
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(2);
        assert!(h.spans(0, &|i| lines[i].clone()).is_empty());
        let v = classes(&mut h, &lines, 1);
        assert!(has(&v, "let", TokenClass::Keyword), "{v:?}");
    }

    #[test]
    fn empty_documents_and_out_of_range_lines_are_fine() {
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(1);
        assert!(h.spans(0, &|_| String::new()).is_empty());
        assert!(h.spans(5, &|_| String::new()).is_empty());
        h.set_line_count(0);
        assert!(h.spans(0, &|_| String::new()).is_empty());
        h.invalidate_from(0);
    }

    /// Deterministic pseudo-random numbers (no rand dependency).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n
        }
    }

    #[test]
    fn incremental_results_equal_a_fresh_highlighter_after_random_edits() {
        const FRAGS: &[&str] = &["/*", "*/", "\"", "'a'", "//", "fn f() {}", "let x = 1;", "r#\"", "\"#", "", "    ", "}"];
        // 3000 lines: more than SPAN_CACHE_MAX, so the cache overflows and resets while we query.
        random_edits_match_a_fresh_highlighter("rs", FRAGS, 3000, false);
    }

    #[test]
    fn incremental_markdown_with_front_matter_equals_a_fresh_highlighter_after_random_edits() {
        const FRAGS: &[&str] = &["---", "...", "a: 1", "b: \"x\" # c", "# T", "text `x`", "```", "", "    code", "- item"];
        // Line 0 is toggled between `---` and text every few rounds, so front matter appears and disappears.
        random_edits_match_a_fresh_highlighter("md", FRAGS, 300, true);
    }

    fn random_edits_match_a_fresh_highlighter(token: &str, frags: &[&str], n: usize, toggle_line_0: bool) {
        let mut rng = Lcg(7);
        let mut lines: Vec<String> = (0..n).map(|_| frags[rng.next(frags.len())].to_string()).collect();
        let mut h = Highlighter::new(token, "").unwrap();
        h.set_line_count(lines.len());
        for round in 0..40 {
            let mut at = rng.next(lines.len());
            if toggle_line_0 && round % 4 == 0 {
                lines[0] = if lines[0] == "---" { "text".into() } else { "---".into() };
                at = 0;
            } else {
                match rng.next(3) {
                    0 => lines[at] = format!("{}{}", frags[rng.next(frags.len())], lines[at]),
                    1 => lines.insert(at, frags[rng.next(frags.len())].to_string()),
                    _ => {
                        if lines.len() > 1 {
                            lines.remove(at);
                        }
                    }
                }
            }
            h.invalidate_from(at);
            h.set_line_count(lines.len());
            let mut fresh = Highlighter::new(token, "").unwrap();
            fresh.set_line_count(lines.len());
            for _ in 0..30 {
                let l = rng.next(lines.len());
                assert_eq!(classes(&mut h, &lines, l), classes(&mut fresh, &lines, l), "round {round}, line {l}");
            }
            // A viewport's worth of lines in ascending order, as prepaint asks for them (exercises the frontier).
            let top = rng.next(lines.len());
            for l in top..(top + 50).min(lines.len()) {
                assert_eq!(classes(&mut h, &lines, l), classes(&mut fresh, &lines, l), "round {round}, line {l} (ascending)");
            }
        }
    }

    /// Calls of `text_of` while asking for `range` in ascending order; `invalidate` (if any) is applied after a
    /// first full pass over all lines.
    fn parses_for(range: std::ops::Range<usize>, invalidate: Option<usize>) -> usize {
        const TOTAL: usize = 400;
        let lines: Vec<String> = (0..TOTAL).map(|i| format!("let v{i} = {i}; // c")).collect();
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(TOTAL);
        if let Some(at) = invalidate {
            for l in 0..TOTAL {
                h.spans(l, &|i| lines[i].clone());
            }
            h.invalidate_from(at);
        }
        let calls = std::cell::Cell::new(0);
        let text_of = |i: usize| {
            calls.set(calls.get() + 1);
            lines[i].clone()
        };
        for l in range {
            h.spans(l, &text_of);
        }
        calls.get()
    }

    #[test]
    fn consecutive_lines_in_order_are_parsed_about_once_each() {
        // Quadratic re-parsing from the block's checkpoint would be 64·65/2 = 2080 calls for the first 64 lines.
        assert_eq!(parses_for(0..64, None), 64);
        assert_eq!(parses_for(0..200, None), 200);
        // After an edit at line 100, a 50-line viewport around it costs at most 63 lines before it plus itself.
        for top in [64, 90, 100, 120] {
            let n = parses_for(top..top + 50, Some(100));
            assert!(n < 50 + CHECKPOINT_EVERY, "viewport at {top}: {n}");
        }
    }

    #[test]
    fn an_edit_below_the_frontier_keeps_it_and_one_above_drops_it() {
        let mut lines: Vec<String> = (0..100).map(|i| format!("let v{i} = {i};")).collect();
        let mut h = Highlighter::new("rs", "").unwrap();
        h.set_line_count(lines.len());
        for l in 0..40 {
            h.spans(l, &|i| lines[i].clone());
        }
        // Edit line 20 (above the frontier at 40): line 40 must see the new comment, not the stale frontier state.
        lines[20] = "/*".into();
        h.invalidate_from(20);
        assert!(classes(&mut h, &lines, 40).iter().all(|(_, c)| *c == TokenClass::Comment));
        // Edit exactly the frontier line (41): the state before it is still valid and is reused.
        lines[41] = "*/ let z = 1;".into();
        h.invalidate_from(41);
        let v = classes(&mut h, &lines, 41);
        assert!(has(&v, "let", TokenClass::Keyword), "{v:?}");
    }
}
