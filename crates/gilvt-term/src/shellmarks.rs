//! Shell integration sequences: OSC 7 (working directory) and OSC 133 (prompt / command marks).

use std::path::PathBuf;

use crate::osc::OscSeq;

/// A working directory reported by the shell with OSC 7 (`ESC ] 7 ; file://host/path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedCwd {
    pub host: String,
    pub path: PathBuf,
}

impl ReportedCwd {
    /// Whether the directory is on this machine (not inside an ssh session).
    pub fn is_local(&self) -> bool {
        is_local_host(&self.host, &local_hostnames())
    }
}

/// OSC 133 semantic prompt marks (FinalTerm protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMark {
    /// `A`: a prompt is about to be drawn.
    PromptStart,
    /// `B`: the prompt ended; user input starts.
    InputStart,
    /// `C`: the command was submitted; its output starts.
    CommandStart,
    /// `D[;exit]`: the command finished.
    CommandEnd(Option<i32>),
}

fn percent_decode(s: &str) -> Option<String> {
    String::from_utf8(percent_decode_bytes(s)?).ok()
}

fn percent_decode_bytes(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

pub fn parse_cwd(seq: &OscSeq) -> Option<ReportedCwd> {
    if seq.code != "7" {
        return None;
    }
    let url = std::str::from_utf8(&seq.payload).ok()?;
    let rest = url.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let host = rest[..slash].to_string();
    let path = percent_decode(&rest[slash..])?;
    Some(ReportedCwd { host, path: PathBuf::from(path) })
}

pub fn parse_prompt_mark(seq: &OscSeq) -> Option<PromptMark> {
    if seq.code != "133" {
        return None;
    }
    let payload = std::str::from_utf8(&seq.payload).ok()?;
    let mut parts = payload.split(';');
    Some(match parts.next()? {
        "A" => PromptMark::PromptStart,
        "B" => PromptMark::InputStart,
        "C" => PromptMark::CommandStart,
        "D" => PromptMark::CommandEnd(parts.next().and_then(|c| c.parse().ok())),
        _ => return None,
    })
}

fn is_local_host(host: &str, hostnames: &[String]) -> bool {
    host.is_empty()
        || ["localhost", "127.0.0.1", "::1", "[::1]"].into_iter().chain(hostnames.iter().map(String::as_str)).any(|h| h.eq_ignore_ascii_case(host))
}

/// This machine's full and short host names, looked up per call (the name can change at runtime).
fn local_hostnames() -> Vec<String> {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0 {
        return Vec::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let full = String::from_utf8_lossy(&buf[..end]).into_owned();
    // Shells report either the full name or the short name ($HOST vs hostname -s).
    let short = full.split('.').next().unwrap_or_default().to_string();
    vec![short, full]
}

/// Most bytes of a command line kept from an OSC 133;C `cmdline_url` (a longer one ends with `…`).
pub const CMDLINE_MAX: usize = 4096;

/// The command line an OSC 133;C mark carries as `cmdline_url=<percent-encoded UTF-8>` (kitty's name for it).
/// None for other marks, without the parameter, or when it does not decode. The shell hooks cut the line at
/// a byte count, so an incomplete UTF-8 sequence at the very end is dropped and the line ends with `…`.
pub fn parse_cmdline(seq: &OscSeq) -> Option<String> {
    if seq.code != "133" {
        return None;
    }
    let payload = std::str::from_utf8(&seq.payload).ok()?;
    let mut parts = payload.split(';');
    if parts.next()? != "C" {
        return None;
    }
    let raw = parts.find_map(|p| p.strip_prefix("cmdline_url="))?;
    let line = match String::from_utf8(percent_decode_bytes(raw)?) {
        Ok(line) => line,
        Err(e) if e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            // Valid up to `valid` by construction.
            format!("{}…", String::from_utf8(bytes).ok()?)
        }
        Err(_) => return None,
    };
    Some(truncate_bytes(&line, CMDLINE_MAX))
}

/// `s` cut to at most `max` bytes on a char boundary, ending with `…` when cut.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max - '…'.len_utf8();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(code: &str, payload: &str) -> OscSeq {
        OscSeq { code: code.into(), payload: payload.as_bytes().to_vec() }
    }

    #[test]
    fn cwd_with_host_and_escapes() {
        let c = parse_cwd(&seq("7", "file://mac.local/Users/me/My%20Repo/%E4%B8%AD")).unwrap();
        assert_eq!(c.host, "mac.local");
        assert_eq!(c.path, PathBuf::from("/Users/me/My Repo/中"));
    }

    #[test]
    fn cwd_without_host_is_local() {
        let c = parse_cwd(&seq("7", "file:///tmp")).unwrap();
        assert_eq!(c.path, PathBuf::from("/tmp"));
        assert!(c.is_local());
        assert!(!ReportedCwd { host: "remote-box-xyz".into(), path: "/".into() }.is_local());
    }

    #[test]
    fn local_host_names() {
        let names = vec!["Mac".to_string(), "Mac.local".to_string()];
        for host in ["", "localhost", "LocalHost", "127.0.0.1", "::1", "[::1]", "mac", "MAC.LOCAL"] {
            assert!(is_local_host(host, &names), "{host}");
        }
        for host in ["remote", "mac.example.com", "10.0.0.1"] {
            assert!(!is_local_host(host, &names), "{host}");
        }
        let own = local_hostnames();
        if let Some(full) = own.last() {
            assert!(ReportedCwd { host: full.to_uppercase(), path: "/".into() }.is_local());
        }
    }

    #[test]
    fn rejects_non_file_urls_and_bad_escapes() {
        assert_eq!(parse_cwd(&seq("7", "http://x/y")), None);
        assert_eq!(parse_cwd(&seq("7", "file://host")), None);
        assert_eq!(parse_cwd(&seq("7", "file:///a%zz")), None);
        assert_eq!(parse_cwd(&seq("2", "file:///tmp")), None);
    }

    #[test]
    fn prompt_marks() {
        assert_eq!(parse_prompt_mark(&seq("133", "A")), Some(PromptMark::PromptStart));
        assert_eq!(parse_prompt_mark(&seq("133", "B")), Some(PromptMark::InputStart));
        assert_eq!(parse_prompt_mark(&seq("133", "C")), Some(PromptMark::CommandStart));
        assert_eq!(parse_prompt_mark(&seq("133", "D;2")), Some(PromptMark::CommandEnd(Some(2))));
        assert_eq!(parse_prompt_mark(&seq("133", "D")), Some(PromptMark::CommandEnd(None)));
        assert_eq!(parse_prompt_mark(&seq("133", "A;aid=1")), Some(PromptMark::PromptStart));
        assert_eq!(parse_prompt_mark(&seq("133", "Z")), None);
    }

    #[test]
    fn cmdline_from_c_mark() {
        assert_eq!(parse_cmdline(&seq("133", "C;cmdline_url=make%20test")).as_deref(), Some("make test"));
        assert_eq!(parse_cmdline(&seq("133", "C")), None, "no parameter");
        assert_eq!(parse_cmdline(&seq("133", "D;0")), None, "only C carries it");
        assert_eq!(parse_cmdline(&seq("7", "C;cmdline_url=x")), None);
        assert_eq!(parse_cmdline(&seq("133", "C;cmdline_url=%zz")), None, "bad escape");
        // The mark itself still parses with the parameter present.
        assert_eq!(parse_prompt_mark(&seq("133", "C;cmdline_url=ls")), Some(PromptMark::CommandStart));
    }

    #[test]
    fn cmdline_with_semicolons_and_cjk() {
        // `;` must arrive encoded (it separates OSC parameters); CJK as UTF-8 escapes.
        let s = seq("133", "C;cmdline_url=echo%20%22a%20b%3Bc%22%20%E4%B8%AD");
        assert_eq!(parse_cmdline(&s).as_deref(), Some("echo \"a b;c\" 中"));
    }

    #[test]
    fn cmdline_is_truncated_on_a_char_boundary() {
        let long = "中".repeat(2000); // 6000 bytes
        let encoded: String = long.bytes().map(|b| format!("%{b:02X}")).collect();
        let got = parse_cmdline(&seq("133", &format!("C;cmdline_url={encoded}"))).unwrap();
        assert!(got.len() <= CMDLINE_MAX, "{}", got.len());
        assert!(got.ends_with('…'));
        assert!(got.trim_end_matches('…').chars().all(|c| c == '中'));
    }

    #[test]
    fn cmdline_cut_inside_a_utf8_sequence_keeps_the_valid_prefix() {
        // The hooks cut at a byte count, possibly inside a character: 中 is E4 B8 AD.
        let s = seq("133", "C;cmdline_url=echo%20%E4%B8%AD%E4%B8");
        assert_eq!(parse_cmdline(&s).as_deref(), Some("echo 中…"));
        let s = seq("133", "C;cmdline_url=%E4");
        assert_eq!(parse_cmdline(&s).as_deref(), Some("…"));
        // Invalid bytes elsewhere are still rejected.
        assert_eq!(parse_cmdline(&seq("133", "C;cmdline_url=a%FFb")), None);
        assert_eq!(parse_cmdline(&seq("133", "C;cmdline_url=%E4%B8x")), None, "broken sequence followed by more text");
    }
}
