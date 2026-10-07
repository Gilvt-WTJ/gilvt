//! Keys: decoding terminal input bytes, and the raw-mode terminal the binary reads them from.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// A key press, as far as the fake cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Esc,
    Up,
    Down,
    CtrlC,
    CtrlD,
    /// Ctrl-U: clears the line being typed.
    CtrlU,
    /// A bracketed paste (`ESC [200~ … ESC [201~`): the text between the markers.
    Paste(String),
    /// Input closed (or a terminating signal arrived): no more keys will come.
    Eof,
}

/// Bytes → keys. A lone ESC is held back until the next byte, or until [`Decoder::flush`] (the caller's
/// timeout): arrow keys arrive as `ESC [ A` in one read, a bare Esc is followed by nothing.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
}

impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.pending.extend_from_slice(bytes);
        let mut keys = Vec::new();
        loop {
            match self.next_key() {
                Some((key, used)) => {
                    self.pending.drain(..used);
                    keys.extend(key);
                }
                None => return keys,
            }
        }
    }

    /// Whether bytes are held back (an ESC that may start a sequence, or part of a UTF-8 char).
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The held-back bytes as keys: a lone ESC is Esc; a broken sequence is dropped.
    pub fn flush(&mut self) -> Vec<Key> {
        let keys = if self.pending.first() == Some(&0x1b) { vec![Key::Esc] } else { Vec::new() };
        self.pending.clear();
        keys
    }

    /// The first complete key in `pending` and the bytes it used; `None` when more bytes are needed.
    fn next_key(&self) -> Option<(Option<Key>, usize)> {
        let b = &self.pending;
        let first = *b.first()?;
        let key = |k: Key, n: usize| Some((Some(k), n));
        match first {
            0x1b => match b.get(1) {
                None => None,
                Some(b'[') if b[2..].starts_with(b"200~") => {
                    // Held back until the end marker arrives; the body is taken whole, escapes included.
                    let body = &b[6..];
                    let close = body.windows(6).position(|w| w == b"\x1b[201~")?;
                    Some((Some(Key::Paste(String::from_utf8_lossy(&body[..close]).into_owned())), 6 + close + 6))
                }
                Some(b'[' | b'O') => {
                    let end = b[2..].iter().position(|c| (0x40..=0x7e).contains(c))? + 2;
                    let k = match (b[1], b[end]) {
                        (_, b'A') if end == 2 => Some(Key::Up),
                        (_, b'B') if end == 2 => Some(Key::Down),
                        _ => None,
                    };
                    Some((k, end + 1))
                }
                // ESC ESC …: the first one was a bare Esc.
                Some(_) => key(Key::Esc, 1),
            },
            b'\r' | b'\n' => key(Key::Enter, 1),
            0x7f | 0x08 => key(Key::Backspace, 1),
            0x03 => key(Key::CtrlC, 1),
            0x04 => key(Key::CtrlD, 1),
            0x15 => key(Key::CtrlU, 1),
            c if c < 0x20 => Some((None, 1)),
            c if c < 0x80 => key(Key::Char(c as char), 1),
            c => {
                let len = match c {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => return Some((None, 1)),
                };
                if b.len() < len {
                    return None;
                }
                match std::str::from_utf8(&b[..len]).ok().and_then(|s| s.chars().next()) {
                    Some(ch) => key(Key::Char(ch), len),
                    None => Some((None, 1)),
                }
            }
        }
    }
}

/// How long a lone ESC waits for the rest of a sequence.
const ESC_WAIT: Duration = Duration::from_millis(40);
/// Longest single `poll`: signals and deadlines are checked at least this often.
const POLL_SLICE: Duration = Duration::from_millis(100);

static SIGNAL: AtomicI32 = AtomicI32::new(0);
static ORIGINAL: OnceLock<libc::termios> = OnceLock::new();

extern "C" fn on_signal(sig: libc::c_int) {
    SIGNAL.store(sig, Ordering::SeqCst);
    if let Some(t) = ORIGINAL.get() {
        // SAFETY: tcsetattr is async-signal-safe; `t` is a valid termios read at startup.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, t) };
    }
}

/// The terminating signal received (SIGINT / SIGTERM / SIGHUP), 0 when none.
pub fn signal_received() -> i32 {
    SIGNAL.load(Ordering::SeqCst)
}

/// Stdin in raw mode (restored on drop and on SIGINT / SIGTERM / SIGHUP), or plain stdin when it is not a
/// terminal (then keys are read from whatever bytes arrive, e.g. a pipe).
pub struct Terminal {
    raw: bool,
    decoder: Decoder,
    queue: std::collections::VecDeque<Key>,
    eof: bool,
}

impl Terminal {
    pub fn open() -> Terminal {
        // SAFETY: plain libc calls on fd 0 with a valid out-pointer.
        let raw = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::isatty(0) == 1 && libc::tcgetattr(0, &mut t) == 0 {
                let _ = ORIGINAL.set(t);
                let mut r = t;
                libc::cfmakeraw(&mut r);
                // Keep output post-processing so `\n` still returns the carriage.
                r.c_oflag |= libc::OPOST | libc::ONLCR;
                libc::tcsetattr(0, libc::TCSANOW, &r) == 0
            } else {
                false
            }
        };
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: installing an extern "C" handler that only touches atomics and tcsetattr.
            unsafe { libc::signal(sig, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
        }
        Terminal { raw, decoder: Decoder::default(), queue: Default::default(), eof: false }
    }

    pub fn is_raw(&self) -> bool {
        self.raw
    }

    /// The next key, waiting at most `timeout` (`None`: until one arrives). `Some(Key::Eof)` once input is
    /// closed or a terminating signal arrived; after that, timed reads just wait out their timeout.
    pub fn read_key(&mut self, timeout: Option<Duration>) -> Option<Key> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            if let Some(k) = self.queue.pop_front() {
                return Some(k);
            }
            if signal_received() != 0 || self.eof {
                if !self.eof {
                    self.eof = true;
                    return Some(Key::Eof);
                }
                match deadline {
                    None => return Some(Key::Eof),
                    Some(d) => {
                        std::thread::sleep(d.saturating_duration_since(Instant::now()));
                        return None;
                    }
                }
            }
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if left == Some(Duration::ZERO) {
                return None;
            }
            let wait = match (left, self.decoder.has_pending()) {
                (_, true) => ESC_WAIT,
                (Some(l), false) => l.min(POLL_SLICE),
                (None, false) => POLL_SLICE,
            };
            match self.poll_read(wait) {
                Some(bytes) if bytes.is_empty() => {
                    self.queue.extend(self.decoder.flush());
                    self.eof = true;
                    self.queue.push_back(Key::Eof);
                }
                Some(bytes) => {
                    let keys = self.decoder.feed(&bytes);
                    self.queue.extend(keys);
                }
                None if self.decoder.has_pending() => {
                    let keys = self.decoder.flush();
                    self.queue.extend(keys);
                }
                None => {}
            }
        }
    }

    /// Bytes available within `wait` (empty: end of input), `None` on timeout or interruption.
    fn poll_read(&self, wait: Duration) -> Option<Vec<u8>> {
        let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd; a bounded read into a stack buffer.
        unsafe {
            if libc::poll(&mut fds, 1, wait.as_millis() as libc::c_int) <= 0 {
                return None;
            }
            let mut buf = [0u8; 256];
            let n = libc::read(0, buf.as_mut_ptr().cast(), buf.len());
            match n {
                n if n > 0 => Some(buf[..n as usize].to_vec()),
                0 => Some(Vec::new()),
                _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => None,
                _ => Some(Vec::new()),
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let (true, Some(t)) = (self.raw, ORIGINAL.get()) {
            // SAFETY: restoring the termios read at startup.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, t) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_keys() {
        let mut d = Decoder::default();
        assert_eq!(d.feed(b"y\r\x7f\x03\x04\x15\n"), vec![Key::Char('y'), Key::Enter, Key::Backspace, Key::CtrlC, Key::CtrlD, Key::CtrlU, Key::Enter]);
        assert_eq!(d.feed(b"\x1b[A\x1b[B\x1bOA"), vec![Key::Up, Key::Down, Key::Up]);
        assert_eq!(d.feed(b"\x1b[3~\x1b[1;5C"), vec![], "other sequences are swallowed whole");
        assert_eq!(d.feed(b"\t"), vec![]);
    }

    #[test]
    fn decodes_a_bracketed_paste() {
        let mut d = Decoder::default();
        assert_eq!(d.feed(b"a\x1b[200~cd \xe4\xb8"), vec![Key::Char('a')], "held until the end marker");
        assert_eq!(d.feed(b"\xad\x1b[A\n\x1b[201~b"), vec![Key::Paste("cd 中\x1b[A\n".into()), Key::Char('b')]);
    }

    #[test]
    fn a_lone_escape_waits_for_the_timeout() {
        let mut d = Decoder::default();
        assert_eq!(d.feed(b"\x1b"), vec![]);
        assert!(d.has_pending());
        assert_eq!(d.flush(), vec![Key::Esc]);
        assert_eq!(d.feed(b"\x1b\x1b"), vec![Key::Esc]);
        assert_eq!(d.flush(), vec![Key::Esc]);
        assert_eq!(d.feed(b"\x1b["), vec![], "an unfinished sequence");
        assert_eq!(d.feed(b"B"), vec![Key::Down]);
    }

    #[test]
    fn decodes_utf8_split_across_reads() {
        let mut d = Decoder::default();
        let cat = "猫".as_bytes();
        assert_eq!(d.feed(&cat[..2]), vec![]);
        assert_eq!(d.feed(&cat[2..]), vec![Key::Char('猫')]);
        assert_eq!(d.feed("é🐱".as_bytes()), vec![Key::Char('é'), Key::Char('🐱')]);
        assert_eq!(d.feed(&[0xff, b'a']), vec![Key::Char('a')], "invalid bytes are dropped");
        assert!(d.flush().is_empty());
    }
}
