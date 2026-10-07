//! Bytes ↔ text: which encoding and line ending a file uses, and how to write it back unchanged.
//!
//! Inside the editor text always uses `\n`. `decode` normalizes `\r\n` and lone `\r` to `\n` and remembers the
//! dominant ending; `encode` turns `\n` back into it and re-encodes with the file's own encoding.

use crate::error::EditorError;

/// Control bytes (other than tab, newline, carriage return, form feed and escape) above this share of the
/// file make it "not text".
const MAX_CONTROL_RATIO: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// The label for a status bar: `LF`, `CRLF`, `CR`.
    pub fn name(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    /// UTF-8 with a byte order mark, which is written back.
    Utf8Bom,
    /// UTF-16 always carries a BOM here (files without one are indistinguishable from binary).
    Utf16Le,
    Utf16Be,
    /// Anything `chardetng` recognizes: GBK, Shift_JIS, windows-1252, …
    Legacy(&'static encoding_rs::Encoding),
}

impl Encoding {
    /// The label for a status bar.
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Legacy(e) => e.name(),
        }
    }
}

/// A decoded file: normalized text plus what is needed to write it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    /// More than one kind of line ending was present; `line_ending` is the most common.
    pub mixed_line_endings: bool,
}

/// Replaces `\r\n` and lone `\r` with `\n`.
pub fn normalize_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The dominant line ending of `text` (ties: LF, then CRLF, then CR) and whether it is mixed.
fn detect_line_ending(text: &str) -> (LineEnding, bool) {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let cr = text.matches('\r').count() - crlf;
    let kinds = [lf, crlf, cr].iter().filter(|&&n| n > 0).count();
    let ending = if lf >= crlf && lf >= cr {
        LineEnding::Lf
    } else if crlf >= cr {
        LineEnding::CrLf
    } else {
        LineEnding::Cr
    };
    (ending, kinds > 1)
}

fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    let control = bytes.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)).count();
    control as f64 > bytes.len() as f64 * MAX_CONTROL_RATIO
}

/// Detects the encoding and line ending of `bytes` and returns normalized text.
///
/// A BOM decides first (UTF-8 / UTF-16). Without one, NUL bytes or many control bytes mean `Binary`; valid
/// UTF-8 is UTF-8; anything else goes to `chardetng`. Bytes that do not decode cleanly in its guess are
/// `UnsupportedEncoding`, and so is a legacy file that would not encode back to exactly the same bytes
/// (duplicate code points, characters the encoder cannot produce): the editor refuses to open it rather than
/// risk silently rewriting or being unable to save text the user never touched.
pub fn decode(bytes: &[u8]) -> Result<Decoded, EditorError> {
    decode_with(bytes, None, false).map(|(d, _)| d)
}

/// Like `decode`, but `forced` skips detection and decodes with that encoding (a BOM matching it is
/// stripped; NUL bytes are not "binary" for UTF-16), and `allow_lossy` turns "cannot be opened faithfully"
/// into a successful decode that replaces what it cannot read. The returned flag says whether that happened
/// (such text must never be saved back). With `forced = None` and `allow_lossy = false` this is `decode`.
///
/// # Errors
/// `Binary`, and `UnsupportedEncoding` for bytes invalid in the encoding or a legacy text that would not
/// encode back to the same bytes, unless `allow_lossy`.
pub fn decode_with(bytes: &[u8], forced: Option<Encoding>, allow_lossy: bool) -> Result<(Decoded, bool), EditorError> {
    let mut lossy = false;
    let (encoding, text) = if let Some(enc) = forced {
        let (e, text, l) = decode_forced(bytes, enc, allow_lossy)?;
        lossy = l;
        (e, text)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let text = std::str::from_utf8(rest).map_err(|_| EditorError::UnsupportedEncoding)?;
        (Encoding::Utf8Bom, text.to_string())
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (Encoding::Utf16Le, decode_utf16(encoding_rs::UTF_16LE, rest)?)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (Encoding::Utf16Be, decode_utf16(encoding_rs::UTF_16BE, rest)?)
    } else if looks_binary(bytes) {
        return Err(EditorError::Binary);
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        (Encoding::Utf8, text.to_string())
    } else {
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
        detector.feed(bytes, true);
        let guess = detector.guess(None, chardetng::Utf8Detection::Deny);
        match guess.decode_without_bom_handling_and_without_replacement(bytes) {
            Some(text) => {
                // Refuse files that would not save back byte for byte: encoding_rs decodes some duplicate code
                // points (Shift_JIS NEC extensions, GBK/Big5 extras) but encodes them differently or not at all.
                let (back, _, unmappable) = guess.encode(&text);
                if unmappable || back.as_ref() != bytes {
                    if !allow_lossy {
                        return Err(EditorError::UnsupportedEncoding);
                    }
                    lossy = true;
                }
                (Encoding::Legacy(guess), text.into_owned())
            }
            None if allow_lossy => {
                lossy = true;
                (Encoding::Legacy(guess), guess.decode_without_bom_handling(bytes).0.into_owned())
            }
            None => return Err(EditorError::UnsupportedEncoding),
        }
    };
    let (line_ending, mixed_line_endings) = detect_line_ending(&text);
    Ok((Decoded { text: normalize_newlines(&text), encoding, line_ending, mixed_line_endings }, lossy))
}

/// Decodes `bytes` as `enc`, returning the encoding, the text and whether it is lossy.
///
/// The text must re-encode to exactly `bytes` (BOM included) with the resulting encoding, so an editable
/// buffer always saves the untouched file back unchanged; otherwise it is lossy (`allow_lossy`) or refused.
/// Forcing UTF-8 picks the variant that matches the file's BOM.
fn decode_forced(bytes: &[u8], enc: Encoding, allow_lossy: bool) -> Result<(Encoding, String, bool), EditorError> {
    let enc = match enc {
        Encoding::Utf8 | Encoding::Utf8Bom => {
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                Encoding::Utf8Bom
            } else {
                Encoding::Utf8
            }
        }
        other => other,
    };
    // A BOM that matches the encoding is stripped; Utf8Bom / Utf16 write one back (encode() does).
    let body = match enc {
        Encoding::Utf8Bom => &bytes[3..],
        Encoding::Utf16Le => bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes),
        Encoding::Utf16Be => bytes.strip_prefix(&[0xFE, 0xFF]).unwrap_or(bytes),
        Encoding::Utf8 | Encoding::Legacy(_) => bytes,
    };
    if !matches!(enc, Encoding::Utf16Le | Encoding::Utf16Be) && looks_binary(bytes) {
        return Err(EditorError::Binary);
    }
    let rs: &'static encoding_rs::Encoding = match enc {
        Encoding::Utf8 | Encoding::Utf8Bom => encoding_rs::UTF_8,
        Encoding::Utf16Le => encoding_rs::UTF_16LE,
        Encoding::Utf16Be => encoding_rs::UTF_16BE,
        Encoding::Legacy(l) => l,
    };
    match rs.decode_without_bom_handling_and_without_replacement(body) {
        Some(text) => {
            let text = text.into_owned();
            // Exact round trip, the same rule as auto-detection (the text is not yet line-ending-normalized).
            if encode(&text, enc, LineEnding::Lf).map_or(true, |back| back != bytes) {
                return if allow_lossy { Ok((enc, text, true)) } else { Err(EditorError::UnsupportedEncoding) };
            }
            Ok((enc, text, false))
        }
        None if allow_lossy => Ok((enc, rs.decode_without_bom_handling(body).0.into_owned(), true)),
        None => Err(EditorError::UnsupportedEncoding),
    }
}

/// The encoding named by a web-style `label` ("GBK", "shift_jis", "utf-8", "utf-16le", ...), if there is one.
pub fn encoding_by_label(label: &str) -> Option<Encoding> {
    let rs = encoding_rs::Encoding::for_label(label.trim().as_bytes())?;
    Some(if rs == encoding_rs::UTF_8 {
        Encoding::Utf8
    } else if rs == encoding_rs::UTF_16LE {
        Encoding::Utf16Le
    } else if rs == encoding_rs::UTF_16BE {
        Encoding::Utf16Be
    } else {
        Encoding::Legacy(rs)
    })
}

/// The encodings a picker offers first, in this order.
pub fn common_encodings() -> Vec<Encoding> {
    vec![
        Encoding::Utf8,
        Encoding::Utf8Bom,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
        Encoding::Legacy(encoding_rs::GBK),
        Encoding::Legacy(encoding_rs::SHIFT_JIS),
        Encoding::Legacy(encoding_rs::EUC_KR),
        Encoding::Legacy(encoding_rs::BIG5),
        Encoding::Legacy(encoding_rs::WINDOWS_1252),
        Encoding::Legacy(encoding_rs::GB18030),
    ]
}

fn decode_utf16(enc: &'static encoding_rs::Encoding, bytes: &[u8]) -> Result<String, EditorError> {
    enc.decode_without_bom_handling_and_without_replacement(bytes)
        .map(|t| t.into_owned())
        .ok_or(EditorError::UnsupportedEncoding)
}

/// Turns normalized `text` back into file bytes: `\n` becomes `line_ending`, then the text is encoded.
/// A character the encoding cannot hold is `Unrepresentable` with its line and column (in chars).
pub fn encode(text: &str, encoding: Encoding, line_ending: LineEnding) -> Result<Vec<u8>, EditorError> {
    let rewritten = match line_ending {
        LineEnding::Lf => std::borrow::Cow::Borrowed(text),
        other => std::borrow::Cow::Owned(text.replace('\n', other.as_str())),
    };
    match encoding {
        Encoding::Utf8 => Ok(rewritten.into_owned().into_bytes()),
        Encoding::Utf8Bom => {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(rewritten.as_bytes());
            Ok(out)
        }
        Encoding::Utf16Le => Ok(encode_utf16(&rewritten, [0xFF, 0xFE], u16::to_le_bytes)),
        Encoding::Utf16Be => Ok(encode_utf16(&rewritten, [0xFE, 0xFF], u16::to_be_bytes)),
        Encoding::Legacy(enc) => {
            let (bytes, _, unmappable) = enc.encode(&rewritten);
            if unmappable {
                return Err(first_unrepresentable(text, enc));
            }
            Ok(bytes.into_owned())
        }
    }
}

fn encode_utf16(text: &str, bom: [u8; 2], unit: fn(u16) -> [u8; 2]) -> Vec<u8> {
    let mut out = bom.to_vec();
    for u in text.encode_utf16() {
        out.extend_from_slice(&unit(u));
    }
    out
}

fn first_unrepresentable(text: &str, enc: &'static encoding_rs::Encoding) -> EditorError {
    let (mut line, mut col) = (0, 0);
    for c in text.chars() {
        let mut buf = [0u8; 4];
        let (_, _, unmappable) = enc.encode(c.encode_utf8(&mut buf));
        if unmappable {
            return EditorError::Unrepresentable { line, col };
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    // Unreachable in practice: enc.encode() already reported an unmappable character above.
    EditorError::UnsupportedEncoding
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_with_and_without_a_bom_round_trips() {
        let plain = decode("héllo\n世界".as_bytes()).unwrap();
        assert_eq!((plain.encoding, plain.text.as_str()), (Encoding::Utf8, "héllo\n世界"));
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("héllo".as_bytes());
        let d = decode(&with_bom).unwrap();
        assert_eq!((d.encoding, d.text.as_str()), (Encoding::Utf8Bom, "héllo"));
        assert_eq!(encode(&d.text, d.encoding, d.line_ending).unwrap(), with_bom, "the BOM is written back");
    }

    #[test]
    fn utf16_needs_a_bom_and_round_trips_both_byte_orders() {
        for (enc, bom) in [(Encoding::Utf16Le, [0xFF, 0xFE]), (Encoding::Utf16Be, [0xFE, 0xFF])] {
            let bytes = encode("a\n世😀", enc, LineEnding::Lf).unwrap();
            assert_eq!(&bytes[..2], &bom);
            let d = decode(&bytes).unwrap();
            assert_eq!((d.encoding, d.text.as_str()), (enc, "a\n世😀"));
            assert_eq!(encode(&d.text, d.encoding, d.line_ending).unwrap(), bytes);
        }
        // An odd number of bytes after the BOM, and a lone surrogate, are not UTF-16.
        assert!(matches!(decode(&[0xFF, 0xFE, 0x61]), Err(EditorError::UnsupportedEncoding)));
        assert!(matches!(decode(&[0xFF, 0xFE, 0x00, 0xD8]), Err(EditorError::UnsupportedEncoding)));
    }

    #[test]
    fn legacy_encodings_are_detected_and_round_trip() {
        let text = "这是一个用 GBK 编码保存的中文文件。\n它有好几行，足够让编码探测器认出来。\n今天天气很好，我们一起去公园散步吧。\n";
        let bytes = encode(text, Encoding::Legacy(encoding_rs::GBK), LineEnding::Lf).unwrap();
        let d = decode(&bytes).unwrap();
        assert_eq!(d.encoding, Encoding::Legacy(encoding_rs::GBK));
        assert_eq!(d.text, text);
        assert_eq!(encode(&d.text, d.encoding, d.line_ending).unwrap(), bytes);
    }

    #[test]
    fn nul_bytes_and_control_noise_are_binary() {
        assert!(matches!(decode(b"abc\0def"), Err(EditorError::Binary)));
        let noisy: Vec<u8> = (0..200).map(|i| if i % 10 == 0 { 0x01 } else { b'a' }).collect();
        assert!(matches!(decode(&noisy), Err(EditorError::Binary)));
        // Tabs, newlines and ESC (colored logs) are text.
        assert!(decode(b"a\tb\n\x1b[31mred\x1b[0m\r\n").is_ok());
    }

    #[test]
    fn line_endings_are_detected_normalized_and_restored() {
        for (raw, ending) in [("a\nb\n", LineEnding::Lf), ("a\r\nb\r\n", LineEnding::CrLf), ("a\rb\r", LineEnding::Cr)] {
            let d = decode(raw.as_bytes()).unwrap();
            assert_eq!((d.text.as_str(), d.line_ending, d.mixed_line_endings), ("a\nb\n", ending, false));
            assert_eq!(encode(&d.text, d.encoding, d.line_ending).unwrap(), raw.as_bytes());
        }
        let mixed = decode(b"a\r\nb\r\nc\nd").unwrap();
        assert_eq!((mixed.text.as_str(), mixed.line_ending, mixed.mixed_line_endings), ("a\nb\nc\nd", LineEnding::CrLf, true));
        assert_eq!(normalize_newlines("x\r\ny\rz"), "x\ny\nz");
    }

    #[test]
    fn a_character_the_encoding_cannot_hold_is_reported_with_its_place() {
        let latin = Encoding::Legacy(encoding_rs::WINDOWS_1252);
        let err = encode("ab\ncafé 世", latin, LineEnding::Lf).unwrap_err();
        assert!(matches!(err, EditorError::Unrepresentable { line: 1, col: 5 }), "{err:?}");
        assert!(encode("café", latin, LineEnding::Lf).is_ok());
    }

    #[test]
    fn unrepresentable_character_is_reported_in_editor_coordinates_for_all_line_endings() {
        let text = "ab\ncafé 世";
        let latin = Encoding::Legacy(encoding_rs::WINDOWS_1252);
        for line_ending in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
            let err = encode(text, latin, line_ending).unwrap_err();
            assert!(
                matches!(err, EditorError::Unrepresentable { line: 1, col: 5 }),
                "for {:?} ending: {:?}",
                line_ending.name(),
                err
            );
        }
    }

    #[test]
    fn a_legacy_file_that_would_not_save_back_byte_for_byte_is_refused() {
        // Shift_JIS with the NEC-selected IBM extension 0xED 0x40: encoding_rs decodes it as 纊 but encodes
        // that as 0xFA 0x5C, so saving an unedited file would change bytes.
        let sjis = |t: &str| encoding_rs::SHIFT_JIS.encode(t).0.into_owned();
        let mut bytes = sjis("これはシフトJISで保存された日本語のテキストファイルです。\n複数行あるので、文字コードの判定がしやすくなります。\n");
        bytes.extend_from_slice(&[0xED, 0x40]);
        bytes.extend(sjis("\n今日はとても天気が良いので、公園へ散歩に行きましょう。\n"));
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
        detector.feed(&bytes, true);
        assert_eq!(detector.guess(None, chardetng::Utf8Detection::Deny), encoding_rs::SHIFT_JIS, "the sample must be guessed as Shift_JIS");
        assert!(matches!(decode(&bytes), Err(EditorError::UnsupportedEncoding)));
    }

    #[test]
    fn labels_and_the_common_list() {
        assert_eq!(encoding_by_label("gbk").map(|e| e.name()), Some("GBK"));
        assert_eq!(encoding_by_label("Shift_JIS").map(|e| e.name()), Some("Shift_JIS"));
        assert_eq!(encoding_by_label("utf-8"), Some(Encoding::Utf8));
        assert!(encoding_by_label("no-such-encoding").is_none());
        let names: Vec<_> = common_encodings().iter().map(|e| e.name()).collect();
        assert_eq!(
            names,
            ["UTF-8", "UTF-8 BOM", "UTF-16 LE", "UTF-16 BE", "GBK", "Shift_JIS", "EUC-KR", "Big5", "windows-1252", "gb18030"]
        );
        assert_eq!(encoding_by_label("utf-16le"), Some(Encoding::Utf16Le));
        assert_eq!(encoding_by_label("utf-16be"), Some(Encoding::Utf16Be));
    }

    #[test]
    fn a_forced_encoding_decodes_and_round_trips() {
        let gbk = encoding_by_label("gbk").unwrap();
        let bytes = [0xC4, 0xE3, 0xBA, 0xC3]; // 你好 in GBK
        let (d, lossy) = decode_with(&bytes, Some(gbk), false).unwrap();
        assert_eq!((d.text.as_str(), lossy), ("你好", false));
        assert_eq!(d.encoding, gbk);
    }

    #[test]
    fn a_forced_encoding_that_cannot_round_trip_needs_allow_lossy() {
        let sjis = encoding_by_label("shift_jis").unwrap();
        let nec = [0xED, 0x40]; // NEC-selected IBM extension: decodes as 纊 but encodes to 0xFA 0x5C
        assert!(matches!(decode_with(&nec, Some(sjis), false), Err(EditorError::UnsupportedEncoding)));
        let (d, lossy) = decode_with(&nec, Some(sjis), true).unwrap();
        assert!(lossy);
        assert_eq!(d.encoding, sjis);
    }

    #[test]
    fn invalid_bytes_in_a_forced_utf8_are_lossy_only_when_allowed() {
        let bytes = [b'a', 0xFF, b'b'];
        assert!(matches!(decode_with(&bytes, Some(Encoding::Utf8), false), Err(EditorError::UnsupportedEncoding)));
        let (d, lossy) = decode_with(&bytes, Some(Encoding::Utf8), true).unwrap();
        assert!(lossy && d.text.contains('\u{FFFD}'));
    }

    #[test]
    fn forced_utf16_without_bom_is_not_binary_but_only_opens_lossy() {
        let bytes = [b'h', 0, b'i', 0]; // contains NUL, no BOM: saving would prepend a BOM
        assert!(matches!(decode_with(&bytes, Some(Encoding::Utf16Le), false), Err(EditorError::UnsupportedEncoding)));
        let (d, lossy) = decode_with(&bytes, Some(Encoding::Utf16Le), true).unwrap();
        assert_eq!((d.text.as_str(), lossy), ("hi", true));
        assert!(matches!(decode_with(&bytes, Some(Encoding::Utf8), false), Err(EditorError::Binary)));
    }

    #[test]
    fn a_forced_utf16_is_editable_only_when_the_file_round_trips() {
        let good = encode("hi\r\nyo", Encoding::Utf16Le, LineEnding::Lf).unwrap(); // FF FE + units
        let (d, lossy) = decode_with(&good, Some(Encoding::Utf16Le), false).unwrap();
        assert_eq!((d.text.as_str(), lossy, d.encoding), ("hi\nyo", false, Encoding::Utf16Le), "mixed endings stay allowed");
        // The opposite byte order mark is not stripped, so the text is wrong and cannot round-trip.
        let be = encode("hi", Encoding::Utf16Be, LineEnding::Lf).unwrap();
        assert!(matches!(decode_with(&be, Some(Encoding::Utf16Le), false), Err(EditorError::UnsupportedEncoding)));
        assert!(decode_with(&be, Some(Encoding::Utf16Le), true).unwrap().1);
        // ASCII read as UTF-16 decodes to other characters and would gain a BOM.
        assert!(matches!(decode_with(b"abcd", Some(Encoding::Utf16Le), false), Err(EditorError::UnsupportedEncoding)));
        assert!(decode_with(b"abcd", Some(Encoding::Utf16Le), true).unwrap().1);
    }

    #[test]
    fn forcing_utf8_picks_the_variant_that_matches_the_bom() {
        let (d, lossy) = decode_with(&[0xEF, 0xBB, 0xBF, b'a'], Some(Encoding::Utf8), false).unwrap();
        assert_eq!((d.encoding, d.text.as_str(), lossy), (Encoding::Utf8Bom, "a", false));
        let (d, lossy) = decode_with(b"a", Some(Encoding::Utf8Bom), false).unwrap();
        assert_eq!((d.encoding, d.text.as_str(), lossy), (Encoding::Utf8, "a", false));
    }

    #[test]
    fn auto_detect_falls_back_to_lossy_when_the_guess_cannot_decode_strictly() {
        // A GBK text followed by 0xFF, which is not valid GBK: chardetng still guesses GBK.
        let mut bytes = encoding_rs::GBK.encode("这是一个用 GBK 编码保存的中文文件。\n它有好几行，足够让编码探测器认出来。\n今天天气很好，我们一起去公园散步吧。\n").0.into_owned();
        bytes.extend_from_slice(&[0xFF, b'\n']);
        assert!(matches!(decode_with(&bytes, None, false), Err(EditorError::UnsupportedEncoding)));
        let (d, lossy) = decode_with(&bytes, None, true).unwrap();
        assert!(lossy && d.encoding == Encoding::Legacy(encoding_rs::GBK));
        assert!(d.text.contains('\u{FFFD}'), "this went through the replacement decode: {:?}", d.text);
    }

    #[test]
    fn decode_is_decode_with_nothing_forced() {
        assert_eq!(decode(b"abc\r\n").unwrap(), decode_with(b"abc\r\n", None, false).unwrap().0);
    }
}
