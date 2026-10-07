//! Side-channel scanner for OSC sequences that alacritty_terminal does not handle
//! (OSC 9 / 777 desktop notifications). The bytes are still passed to the real parser.

const MAX_OSC_LEN: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscSeq {
    pub code: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: Option<String>,
    pub body: String,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc(Vec<u8>),
    OscEsc(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct OscScanner {
    state: State,
}

impl OscScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds raw PTY bytes; returns every OSC sequence completed within them.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OscSeq> {
        self.feed_at(bytes).into_iter().map(|(_, s)| s).collect()
    }

    /// Like [`feed`](Self::feed), with where each sequence ends: the index in `bytes` just past its terminator.
    pub fn feed_at(&mut self, bytes: &[u8]) -> Vec<(usize, OscSeq)> {
        let mut out = Vec::new();
        let mut done = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            self.step(b, &mut done);
            out.extend(done.drain(..).map(|s| (i + 1, s)));
        }
        out
    }

    fn step(&mut self, b: u8, out: &mut Vec<OscSeq>) {
        let state = std::mem::take(&mut self.state);
        self.state = match state {
            State::Ground => {
                if b == 0x1b {
                    State::Esc
                } else {
                    State::Ground
                }
            }
            State::Esc => match b {
                b']' => State::Osc(Vec::new()),
                0x1b => State::Esc,
                _ => State::Ground,
            },
            State::Osc(mut buf) => match b {
                0x07 => {
                    out.extend(finish(buf));
                    State::Ground
                }
                0x1b => State::OscEsc(buf),
                0x18 | 0x1a => State::Ground,
                _ if buf.len() >= MAX_OSC_LEN => State::Ground,
                _ => {
                    buf.push(b);
                    State::Osc(buf)
                }
            },
            State::OscEsc(buf) => {
                if b == b'\\' {
                    out.extend(finish(buf));
                    State::Ground
                } else {
                    // Not a string terminator: the ESC started a new sequence.
                    self.state = State::Esc;
                    self.step(b, out);
                    return;
                }
            }
        };
    }
}

fn finish(buf: Vec<u8>) -> Option<OscSeq> {
    let split = buf.iter().position(|&c| c == b';').unwrap_or(buf.len());
    let code = std::str::from_utf8(&buf[..split]).ok()?.to_string();
    let payload = if split < buf.len() { buf[split + 1..].to_vec() } else { Vec::new() };
    Some(OscSeq { code, payload })
}

/// Interprets OSC 9 (`ESC ] 9 ; body`) and OSC 777 (`ESC ] 777 ; notify ; title ; body`).
pub fn parse_notification(seq: &OscSeq) -> Option<Notification> {
    let payload = String::from_utf8_lossy(&seq.payload);
    match seq.code.as_str() {
        // `9;4;...` is the ConEmu progress extension, not a notification.
        "9" if payload.starts_with("4;") => None,
        "9" if !payload.is_empty() => Some(Notification { title: None, body: payload.into_owned() }),
        "777" => {
            let mut parts = payload.splitn(3, ';');
            if parts.next()? != "notify" {
                return None;
            }
            let title = parts.next().unwrap_or_default().to_string();
            let body = parts.next().unwrap_or_default().to_string();
            Some(Notification { title: (!title.is_empty()).then_some(title), body })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bel_and_st_terminators() {
        let mut s = OscScanner::new();
        let seqs = s.feed(b"hi\x1b]9;done\x07 and \x1b]777;notify;T;B\x1b\\");
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0], OscSeq { code: "9".into(), payload: b"done".to_vec() });
        assert_eq!(seqs[1].code, "777");
    }

    #[test]
    fn split_across_reads() {
        let mut s = OscScanner::new();
        assert!(s.feed(b"\x1b]9;hel").is_empty());
        let seqs = s.feed(b"lo\x07");
        assert_eq!(seqs[0].payload, b"hello");
    }

    #[test]
    fn esc_inside_osc_that_is_not_st_restarts() {
        let mut s = OscScanner::new();
        let seqs = s.feed(b"\x1b]9;x\x1b]9;y\x07");
        assert_eq!(seqs, vec![OscSeq { code: "9".into(), payload: b"y".to_vec() }]);
    }

    #[test]
    fn csi_is_ignored() {
        let mut s = OscScanner::new();
        assert!(s.feed(b"\x1b[31mred\x1b[0m").is_empty());
    }

    #[test]
    fn notifications() {
        let n9 = parse_notification(&OscSeq { code: "9".into(), payload: b"Build ok".to_vec() });
        assert_eq!(n9, Some(Notification { title: None, body: "Build ok".into() }));
        let progress = parse_notification(&OscSeq { code: "9".into(), payload: b"4;1;50".to_vec() });
        assert_eq!(progress, None);
        let n777 = parse_notification(&OscSeq {
            code: "777".into(),
            payload: b"notify;Claude;Needs input".to_vec(),
        });
        assert_eq!(n777, Some(Notification { title: Some("Claude".into()), body: "Needs input".into() }));
        let title = parse_notification(&OscSeq { code: "2".into(), payload: b"x".to_vec() });
        assert_eq!(title, None);
    }

    #[test]
    fn feed_at_reports_where_each_sequence_ends() {
        let mut s = OscScanner::new();
        let bytes = b"ab\x1b]133;C\x07out\x1b]133;D;0\x1b\\tail";
        let got: Vec<(usize, String)> = s.feed_at(bytes).into_iter().map(|(end, seq)| (end, String::from_utf8(seq.payload).unwrap())).collect();
        assert_eq!(got, vec![(10, "C".to_string()), (24, "D;0".to_string())]);
        assert_eq!(&bytes[24..], b"tail");
    }
}
