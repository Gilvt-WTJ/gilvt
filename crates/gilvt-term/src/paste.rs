use alacritty_terminal::term::TermMode;

/// Bytes to write for a paste. Bracketed paste wraps the text and strips any embedded end marker
/// so pasted content cannot break out of the bracket.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let body = text.replace("\x1b[201~", "");
        let mut out = Vec::with_capacity(body.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed() {
        let out = encode_paste("a\nb\x1b[201~c", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~a\nbc\x1b[201~");
    }

    #[test]
    fn plain_normalizes_newlines() {
        assert_eq!(encode_paste("a\r\nb\nc", TermMode::empty()), b"a\rb\rc");
    }
}
