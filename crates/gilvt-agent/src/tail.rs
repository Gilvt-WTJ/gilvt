//! Incremental reading of an append-only JSONL file (a transcript or rollout).

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Most bytes read per call; the rest waits for the next call.
const MAX_READ: u64 = 16 << 20;

/// Incremental reader: returns the complete new lines since the last call and keeps a partial last line
/// for later. A missing file reads as empty; a truncated or replaced file is read again from the start.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    /// Started mid-line ([`Tail::from_end`]): drop everything up to the next newline.
    skip_to_newline: bool,
    file_id: Option<(u64, u64)>,
}

impl Tail {
    /// Reads the file from its start.
    pub fn new(path: PathBuf) -> Tail {
        Tail { path, offset: 0, partial: Vec::new(), skip_to_newline: false, file_id: None }
    }

    /// Skips what the file holds now: only lines appended later are returned.
    pub fn from_end(path: PathBuf) -> Tail {
        let mut tail = Tail::new(path);
        if let Ok(mut file) = File::open(&tail.path) {
            if let Ok(meta) = file.metadata() {
                tail.offset = meta.len();
                tail.file_id = Some(file_id(&meta));
                tail.skip_to_newline = meta.len() > 0 && !ends_with_newline(&mut file, meta.len());
            }
        }
        tail
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_new(&mut self) -> io::Result<Vec<String>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        let id = file_id(&meta);
        if meta.len() < self.offset || self.file_id.is_some_and(|known| known != id) {
            self.offset = 0;
            self.partial.clear();
            self.skip_to_newline = false;
        }
        self.file_id = Some(id);
        if meta.len() == self.offset {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let read = file.take(MAX_READ).read_to_end(&mut self.partial)?;
        self.offset += read as u64;
        Ok(self.take_lines())
    }

    fn take_lines(&mut self) -> Vec<String> {
        let Some(last) = self.partial.iter().rposition(|&b| b == b'\n') else { return Vec::new() };
        let rest = self.partial.split_off(last + 1);
        let mut complete = std::mem::replace(&mut self.partial, rest);
        if self.skip_to_newline {
            let first = complete.iter().position(|&b| b == b'\n').expect("contains a newline");
            complete.drain(..=first);
            self.skip_to_newline = false;
        }
        complete
            .split(|&b| b == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .filter(|line| !line.is_empty())
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect()
    }
}

fn ends_with_newline(file: &mut File, len: u64) -> bool {
    let mut last = [0u8];
    file.seek(SeekFrom::Start(len - 1)).and_then(|_| file.read_exact(&mut last)).is_ok() && last[0] == b'\n'
}

#[cfg(unix)]
fn file_id(meta: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn file_id(_meta: &Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

    use super::*;

    fn append(path: &Path, text: &str) {
        OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn returns_complete_lines_and_keeps_the_partial_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut tail = Tail::new(path.clone());
        assert_eq!(tail.read_new().unwrap(), Vec::<String>::new(), "missing file");
        append(&path, "{\"a\":1}\n{\"b\":");
        assert_eq!(tail.read_new().unwrap(), ["{\"a\":1}"]);
        assert_eq!(tail.read_new().unwrap(), Vec::<String>::new());
        append(&path, "2}\r\n\n{\"c\":\"中文\"}\n");
        assert_eq!(tail.read_new().unwrap(), ["{\"b\":2}", "{\"c\":\"中文\"}"]);
        assert_eq!(tail.path(), path);
    }

    #[test]
    fn from_end_skips_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "old 1\nold 2\n");
        let mut tail = Tail::from_end(path.clone());
        assert_eq!(tail.read_new().unwrap(), Vec::<String>::new());
        append(&path, "new\n");
        assert_eq!(tail.read_new().unwrap(), ["new"]);
    }

    #[test]
    fn from_end_mid_line_drops_the_rest_of_that_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "old\n{\"half\":");
        let mut tail = Tail::from_end(path.clone());
        append(&path, "true}\nnext\n");
        assert_eq!(tail.read_new().unwrap(), ["next"]);
        // A missing file at construction starts at 0.
        let mut fresh = Tail::from_end(dir.path().join("later.jsonl"));
        append(&dir.path().join("later.jsonl"), "first\n");
        assert_eq!(fresh.read_new().unwrap(), ["first"]);
    }

    #[test]
    fn truncated_or_replaced_files_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "one\ntwo\n");
        let mut tail = Tail::new(path.clone());
        assert_eq!(tail.read_new().unwrap(), ["one", "two"]);
        std::fs::write(&path, "x\n").unwrap();
        assert_eq!(tail.read_new().unwrap(), ["x"]);
        // Replaced by a longer file (new inode): read from the start, not from the old offset.
        let other = dir.path().join("other.jsonl");
        std::fs::write(&other, "fresh 1\nfresh 2\nfresh 3\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(tail.read_new().unwrap(), ["fresh 1", "fresh 2", "fresh 3"]);
    }
}
