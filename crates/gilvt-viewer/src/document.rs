//! Loading a file (or inline content) for preview.

use std::io;
use std::path::{Path, PathBuf};

/// Files larger than this are not rendered (only their metadata is shown).
pub const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// Contains NUL bytes or is not valid UTF-8.
    Binary,
    /// Larger than `MAX_TEXT_BYTES`; holds the size.
    TooLarge(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// `None` for inline content (`gilvt view -`).
    pub path: Option<PathBuf>,
    /// Display name: file name, or "stdin".
    pub name: String,
    /// Explicit type from `--as` (e.g. "rs", "md"); overrides the extension.
    pub type_hint: Option<String>,
    pub content: Content,
    pub size: u64,
}

fn classify(bytes: Vec<u8>) -> Content {
    let head = &bytes[..bytes.len().min(8000)];
    if head.contains(&0) {
        return Content::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Content::Text(text),
        Err(_) => Content::Binary,
    }
}

impl Document {
    /// Reads a regular file; at most `MAX_TEXT_BYTES + 1` bytes are read whatever its metadata says.
    pub fn load(path: &Path) -> io::Result<Document> {
        use std::io::Read;
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
        }
        let size = meta.len();
        let content = if size > MAX_TEXT_BYTES {
            Content::TooLarge(size)
        } else {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?.take(MAX_TEXT_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_TEXT_BYTES { Content::TooLarge(bytes.len() as u64) } else { classify(bytes) }
        };
        Ok(Document {
            path: Some(path.to_path_buf()),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()),
            type_hint: None,
            content,
            size,
        })
    }

    pub fn from_content(content: String, type_hint: Option<String>) -> Document {
        let size = content.len() as u64;
        Document { path: None, name: "stdin".into(), type_hint, content: classify(content.into_bytes()), size }
    }

    /// A document whose content is already in memory (the file as a snapshot had it); `path` names it.
    pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Document {
        let size = bytes.len() as u64;
        let content = if size > MAX_TEXT_BYTES { Content::TooLarge(size) } else { classify(bytes) };
        Document {
            path: Some(path.to_path_buf()),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()),
            type_hint: None,
            content,
            size,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match &self.content {
            Content::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Token used to pick a syntax: `--as` type, else the extension, else the file name.
    pub fn syntax_token(&self) -> String {
        if let Some(t) = &self.type_hint {
            return t.clone();
        }
        let p = Path::new(&self.name);
        p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| self.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_text() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("main.rs");
        std::fs::write(&p, "fn main() {}\n").unwrap();
        let d = Document::load(&p).unwrap();
        assert_eq!(d.name, "main.rs");
        assert_eq!(d.text(), Some("fn main() {}\n"));
        assert_eq!(d.syntax_token(), "rs");
        assert_eq!(d.size, 13);
    }

    #[test]
    fn detects_binary_and_invalid_utf8() {
        assert_eq!(classify(vec![b'a', 0, b'b']), Content::Binary);
        assert_eq!(classify(vec![0xff, 0xfe, b'a']), Content::Binary);
        assert_eq!(classify("中文".as_bytes().to_vec()), Content::Text("中文".into()));
    }

    #[test]
    fn large_files_are_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.log");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(MAX_TEXT_BYTES + 1).unwrap();
        assert_eq!(Document::load(&p).unwrap().content, Content::TooLarge(MAX_TEXT_BYTES + 1));
    }

    #[test]
    fn non_regular_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Document::load(dir.path()).is_err());
        // Character device reporting size 0 that never ends.
        assert!(Document::load(Path::new("/dev/zero")).is_err());
    }

    #[test]
    fn inline_content_uses_type_hint() {
        let d = Document::from_content("# hi".into(), Some("md".into()));
        assert_eq!(d.name, "stdin");
        assert_eq!(d.syntax_token(), "md");
        let d = Document::from_content("x".into(), None);
        assert_eq!(d.syntax_token(), "stdin");
    }

    #[test]
    fn extensionless_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Makefile");
        std::fs::write(&p, "all:\n").unwrap();
        assert_eq!(Document::load(&p).unwrap().syntax_token(), "Makefile");
    }
}
