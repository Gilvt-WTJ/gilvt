//! Paths as shell words, the way iTerm2 inserts dropped files.

use std::fmt::Write;

/// Special to bash / zsh anywhere in a word.
const SPECIAL: &[char] =
    &[' ', '\\', '\'', '"', '`', '$', '!', '&', '|', ';', '<', '>', '(', ')', '[', ']', '{', '}', '*', '?', '#', '^'];
/// Special only at the start of a word: `~` (home) and zsh's `=` (command path) expansions.
const SPECIAL_AT_START: &[char] = &['~', '='];

/// Shell word for a path, in bash / zsh syntax: backslash-escape spaces and shell metacharacters
/// like iTerm2 does (keep non-ASCII such as 中文 as is). Used for both ⌥⏎ and Finder drops. A
/// leading `-` gets `./` so the path is not read as an option. Control characters (a newline would
/// end the command line; C1 included) and bidi overrides / isolates (which reorder how the line
/// reads) become their UTF-8 bytes as `$'\xNN…'`: macOS's bash 3.2 has no `$'\uXXXX'`, and zsh
/// rejects it outside a UTF-8 locale.
pub fn shell_escape(path: &str) -> String {
    let mut word = String::with_capacity(path.len() + 8);
    if path.starts_with('-') {
        word.push_str("./");
    }
    for (i, c) in path.chars().enumerate() {
        if c.is_control() || is_bidi_control(c) {
            word.push_str("$'");
            for b in c.encode_utf8(&mut [0; 4]).bytes() {
                let _ = write!(word, "\\x{b:02x}");
            }
            word.push('\'');
            continue;
        }
        if SPECIAL.contains(&c) || (i == 0 && SPECIAL_AT_START.contains(&c)) {
            word.push('\\');
        }
        word.push(c);
    }
    word
}

/// Embeddings / overrides (U+202A–U+202E) and isolates (U+2066–U+2069).
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Text inserted for a set of paths: escaped words joined by ' ', plus one trailing space
/// (empty for no paths).
pub fn insertion(paths: &[String]) -> String {
    paths.iter().map(|p| shell_escape(p) + " ").collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_metacharacters() {
        assert_eq!(shell_escape("/a/b.txt"), "/a/b.txt");
        assert_eq!(shell_escape("/a/has space.txt"), r"/a/has\ space.txt");
        assert_eq!(shell_escape(r#"it's "q".md"#), r#"it\'s\ \"q\".md"#);
        assert_eq!(shell_escape("$HOME!x"), r"\$HOME\!x");
        assert_eq!(shell_escape("a&b|c;d"), r"a\&b\|c\;d");
        assert_eq!(shell_escape("f(1).rs"), r"f\(1\).rs");
        assert_eq!(shell_escape("*?[ab]{c}"), r"\*\?\[ab\]\{c\}");
        assert_eq!(shell_escape("<in>`x`#^"), r"\<in\>\`x\`\#\^");
        assert_eq!(shell_escape(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn tilde_and_equals_only_at_the_start() {
        assert_eq!(shell_escape("~draft.md"), r"\~draft.md");
        assert_eq!(shell_escape("=cmd"), r"\=cmd");
        assert_eq!(shell_escape("/a/~b=c"), "/a/~b=c");
    }

    #[test]
    fn keeps_non_ascii() {
        assert_eq!(shell_escape("/文档/说明.md"), "/文档/说明.md");
        assert_eq!(shell_escape("中文 名.md"), r"中文\ 名.md");
    }

    #[test]
    fn leading_dash_and_control_characters() {
        assert_eq!(shell_escape("-rf"), "./-rf");
        assert_eq!(shell_escape("a\nb\tc"), r"a$'\x0a'b$'\x09'c");
        assert_eq!(shell_escape("a\u{7f}b"), r"a$'\x7f'b");
    }

    #[test]
    fn unicode_controls_and_bidi_characters() {
        assert_eq!(shell_escape("a\u{85}b\u{9b}c"), r"a$'\xc2\x85'b$'\xc2\x9b'c");
        assert_eq!(shell_escape("x\u{202e}fdp.exe"), r"x$'\xe2\x80\xae'fdp.exe");
        let bidi = ['\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'];
        let escaped = bidi.map(|c| shell_escape(&c.to_string()));
        let expected = [r"\xe2\x80\xaa", r"\xe2\x80\xab", r"\xe2\x80\xac", r"\xe2\x80\xad", r"\xe2\x81\xa6", r"\xe2\x81\xa7", r"\xe2\x81\xa8", r"\xe2\x81\xa9"];
        assert_eq!(escaped, expected.map(|b| format!("$'{b}'")));
        // Neighbours of the ranges, and other format characters, stay as they are.
        assert_eq!(shell_escape("\u{2029}\u{202f}\u{2065}\u{206a}\u{200d}"), "\u{2029}\u{202f}\u{2065}\u{206a}\u{200d}");
    }

    #[test]
    fn words_read_back_as_the_path_in_bash_and_zsh() {
        let path = "-a b/$x!'\"*~\n\u{85}\u{202e}中文 \u{2066}.md";
        let word = shell_escape(path);
        // macOS's /bin/bash is 3.2; zsh runs without a UTF-8 locale.
        for shell in ["/bin/bash", "/bin/zsh"].into_iter().filter(|s| std::path::Path::new(s).exists()) {
            let out = std::process::Command::new(shell)
                .args(["-c", &format!("printf %s {word}")])
                .env_remove("LANG")
                .env_remove("LC_ALL")
                .env_remove("LC_CTYPE")
                .output()
                .unwrap();
            assert!(out.status.success(), "{shell}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("./{path}"), "{shell}");
        }
    }

    #[test]
    fn insertion_joins_with_a_trailing_space() {
        assert_eq!(insertion(&["/a b".into()]), r"/a\ b ");
        assert_eq!(insertion(&["x.rs".into(), "/y z/中.md".into()]), r"x.rs /y\ z/中.md ");
        assert_eq!(insertion(&[]), "");
    }
}
