//! Transcript following for the app's inspector (spec R2a §4.4): polls a file's size every 250 ms and pushes
//! what was appended, cut at line ends. inotify comes with R3.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub const FOLLOW_POLL: Duration = Duration::from_millis(250);
/// Largest chunk cut at a line end.
pub const MAX_CHUNK: usize = 256 * 1024;
/// Longest line sent (whole, as one chunk); longer ones are skipped (R2a plan deviation 3).
pub const MAX_LINE: usize = 8 * 1024 * 1024;
/// Follows per daemon.
pub const MAX_FOLLOWS: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    Append { offset: u64, bytes: Vec<u8> },
    /// The file shrank or was replaced: the app starts over from offset 0. `size` is the file's length now: what
    /// is pushed again below it is history for the app, not live events (R2a plan deviation 15).
    Reset { size: u64 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Chunk {
    /// Push this many bytes from the start.
    Send(usize),
    /// No complete line yet.
    Wait,
    /// `limit` bytes without a line end: a line over the limit, skip to its end.
    Skip,
}

/// How much of `buf` (which starts at a line start) to push now: up to the last line end within `max`; else a
/// longer line whole once it is complete; `Skip` when `buf` holds `limit` bytes without any line end.
pub fn split_chunk(buf: &[u8], max: usize, limit: usize) -> Chunk {
    if let Some(i) = buf[..buf.len().min(max)].iter().rposition(|&b| b == b'\n') {
        return Chunk::Send(i + 1);
    }
    match buf.iter().position(|&b| b == b'\n') {
        Some(i) => Chunk::Send(i + 1),
        None if buf.len() >= limit => Chunk::Skip,
        None => Chunk::Wait,
    }
}

/// A transcript the app may follow: absolute, no `..`, `.jsonl`, under `home` (also after resolving symlinks).
pub fn check_path(path: &Path, home: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{}: not an absolute path", path.display()));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err(format!("{}: not a .jsonl file", path.display()));
    }
    if !path.starts_with(home) {
        return Err(format!("{}: not under $HOME", path.display()));
    }
    if let (Ok(real), Ok(real_home)) = (path.canonicalize(), home.canonicalize()) {
        if !real.starts_with(&real_home) {
            return Err(format!("{}: not under $HOME", path.display()));
        }
    }
    Ok(path.to_path_buf())
}

pub struct Follower {
    path: PathBuf,
    /// The next byte to push (a line start, unless `skipping`).
    offset: u64,
    id: Option<(u64, u64)>,
    /// Inside a line over [`MAX_LINE`]: drop bytes up to its end.
    skipping: bool,
}

impl Follower {
    pub fn new(path: PathBuf, from: u64) -> Follower {
        Follower { path, offset: from, id: None, skipping: false }
    }

    /// The file's size now (0 when missing): what the app counts as history (`Following { size }`).
    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.path).map_or(0, |m| m.len())
    }

    pub fn poll(&mut self) -> io::Result<Vec<Push>> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        let id = (meta.dev(), meta.ino());
        let mut out = Vec::new();
        if meta.len() < self.offset || self.id.is_some_and(|known| known != id) {
            out.push(Push::Reset { size: meta.len() });
            self.offset = 0;
            self.skipping = false;
        }
        self.id = Some(id);
        let end = meta.len().min(self.offset + MAX_LINE as u64);
        if end <= self.offset {
            return Ok(out);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = vec![0; (end - self.offset) as usize];
        file.read_exact(&mut buf)?;
        let mut at = 0usize;
        if self.skipping {
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    at = i + 1;
                    self.skipping = false;
                }
                None => {
                    self.offset = end;
                    return Ok(out);
                }
            }
        }
        loop {
            match split_chunk(&buf[at..], MAX_CHUNK, MAX_LINE) {
                Chunk::Send(n) => {
                    out.push(Push::Append { offset: self.offset + at as u64, bytes: buf[at..at + n].to_vec() });
                    at += n;
                }
                Chunk::Wait => break,
                Chunk::Skip => {
                    self.skipping = true;
                    at = buf.len();
                    break;
                }
            }
        }
        self.offset += at as u64;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn appends(p: &[Push]) -> Vec<(u64, String)> {
        p.iter().filter_map(|x| match x { Push::Append { offset, bytes } => Some((*offset, String::from_utf8_lossy(bytes).into_owned())), _ => None }).collect()
    }

    #[test]
    fn chunks_end_at_line_ends() {
        assert!(matches!(split_chunk(b"a\nb\nc", 100, 1000), Chunk::Send(4)));
        assert!(matches!(split_chunk(b"abc", 100, 1000), Chunk::Wait), "no complete line yet");
        assert!(matches!(split_chunk(b"aaaa\nbbbb\n", 6, 1000), Chunk::Send(5)), "the last line end within max");
    }

    #[test]
    fn an_oversized_line_is_sent_whole() {
        let mut buf = vec![b'x'; 300];
        buf.push(b'\n');
        buf.extend_from_slice(b"y\n");
        assert!(matches!(split_chunk(&buf, 256, 1000), Chunk::Send(301)), "a line over max goes as one chunk once complete");
    }

    #[test]
    fn a_line_over_the_limit_is_skipped_and_following_lines_still_arrive() {
        assert!(matches!(split_chunk(&[b'x'; 1000], 256, 1000), Chunk::Skip));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![b'x'; MAX_LINE + 10]).unwrap();
        f.write_all(b"\n{\"after\":1}\n").unwrap();
        let mut fl = Follower::new(path, 0);
        let mut got = Vec::new();
        for _ in 0..4 { got.extend(appends(&fl.poll().unwrap())); }
        assert_eq!(got, vec![((MAX_LINE + 11) as u64, "{\"after\":1}\n".to_string())]);
    }

    #[test]
    fn follows_appends_and_resets_on_truncation_or_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "one\ntw").unwrap();
        let mut fl = Follower::new(path.clone(), 0);
        assert_eq!(fl.size(), 6, "size() is the file's length now (what Following reports), not the follow position");
        assert_eq!(appends(&fl.poll().unwrap()), vec![(0, "one\n".to_string())]);
        assert_eq!(fl.size(), 6, "still the file length after a poll");
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"o\n").unwrap();
        assert_eq!(appends(&fl.poll().unwrap()), vec![(4, "two\n".to_string())]);
        std::fs::write(&path, "x\n").unwrap();
        let p = fl.poll().unwrap();
        assert_eq!(p[0], Push::Reset { size: 2 }, "shorter: reset, carrying the new length (the app treats those bytes as history)");
        assert_eq!(appends(&p), vec![(0, "x\n".to_string())]);
        let other = dir.path().join("t.jsonl");
        std::fs::write(&other, "x\ny\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        let p = fl.poll().unwrap();
        assert_eq!(p[0], Push::Reset { size: 4 }, "another inode: reset");
        assert_eq!(appends(&p), vec![(0, "x\ny\n".to_string())]);
    }

    #[test]
    fn from_continues_where_the_app_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let mut fl = Follower::new(path, 4);
        assert_eq!(appends(&fl.poll().unwrap()), vec![(4, "two\n".to_string())]);
    }

    #[test]
    fn paths_must_be_jsonl_under_home() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        assert!(check_path(&h.join(".claude/projects/p/s.jsonl"), h).is_ok());
        assert!(check_path(&h.join("notes.txt"), h).is_err());
        assert!(check_path(Path::new("/etc/passwd.jsonl"), h).is_err());
        assert!(check_path(&h.join("../other/s.jsonl"), h).is_err());
        assert!(check_path(Path::new("rel/s.jsonl"), h).is_err());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.jsonl"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("x.jsonl"), h.join("link.jsonl")).unwrap();
        assert!(check_path(&h.join("link.jsonl"), h).is_err(), "a symlink out of $HOME");
    }
}
