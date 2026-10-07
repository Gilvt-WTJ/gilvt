//! Keyboard → PTY byte encoding: legacy xterm sequences and the kitty keyboard protocol
//! (https://sw.kovidgoyal.net/kitty/keyboard-protocol/). Press and repeat events only.
//!
//! Returns `None` for keys that should instead reach the IME / text-input path
//! (plain printable characters) or that belong to the app (anything with Cmd).

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub cmd: bool,
}

impl Mods {
    fn any_except_shift(&self) -> bool {
        self.alt || self.ctrl
    }

    /// xterm / kitty modifier parameter: 1 + bitmask.
    fn param(&self) -> u8 {
        1 + (self.shift as u8) + ((self.alt as u8) << 1) + ((self.ctrl as u8) << 2)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KeyInput<'a> {
    /// gpui key name: "a", "enter", "tab", "escape", "backspace", "delete", "insert",
    /// "up", "down", "left", "right", "home", "end", "pageup", "pagedown", "space", "f1".."f12".
    pub key: &'a str,
    /// Text the key would type with current modifiers (gpui `key_char`).
    pub text: Option<&'a str>,
    pub mods: Mods,
    /// Auto-repeat of a held key.
    pub repeat: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyOptions {
    /// Treat Option as Meta (ESC prefix) instead of typing macOS special characters.
    pub option_as_meta: bool,
}

pub fn encode_key(input: &KeyInput, mode: TermMode, opts: &KeyOptions) -> Option<Vec<u8>> {
    if input.mods.cmd {
        return None;
    }
    if mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC) {
        return encode_kitty(input, mode, opts);
    }
    encode_legacy(input, mode, opts)
}

// ---------- legacy ----------

fn encode_legacy(input: &KeyInput, mode: TermMode, opts: &KeyOptions) -> Option<Vec<u8>> {
    let m = input.mods;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let esc_prefix = |mut bytes: Vec<u8>| {
        if m.alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    let named: Option<Vec<u8>> = match input.key {
        "enter" if m.shift && !m.alt && !m.ctrl => Some(b"\x1b\r".to_vec()),
        "enter" => Some(esc_prefix(b"\r".to_vec())),
        "tab" if m.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(esc_prefix(b"\t".to_vec())),
        "backspace" if m.ctrl => Some(esc_prefix(vec![0x08])),
        "backspace" => Some(esc_prefix(vec![0x7f])),
        "escape" => Some(esc_prefix(vec![0x1b])),
        "space" if m.ctrl => Some(esc_prefix(vec![0x00])),
        "space" if m.alt && opts.option_as_meta => Some(b"\x1b ".to_vec()),
        "up" | "down" | "right" | "left" | "home" | "end" => {
            let c = match input.key {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            Some(if m.param() > 1 {
                format!("\x1b[1;{}{c}", m.param()).into_bytes()
            } else if app_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            })
        }
        "insert" | "delete" | "pageup" | "pagedown" => {
            let n = match input.key {
                "insert" => 2,
                "delete" => 3,
                "pageup" => 5,
                _ => 6,
            };
            Some(tilde(n, m))
        }
        k if k.starts_with('f') && k.len() > 1 => function_key(k, m),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    // Character keys.
    let base = single_char(input.key)?;
    if m.ctrl {
        if let Some(c) = ctrl_byte(base) {
            return Some(esc_prefix(vec![c]));
        }
    }
    if m.alt && opts.option_as_meta {
        let ch = if m.shift { shifted(input) } else { base.to_string() };
        let mut out = vec![0x1b];
        out.extend(ch.as_bytes());
        return Some(out);
    }
    // Plain / shifted / macOS Option characters go through text input.
    None
}

fn tilde(n: u8, m: Mods) -> Vec<u8> {
    if m.param() > 1 {
        format!("\x1b[{n};{}~", m.param()).into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn function_key(key: &str, m: Mods) -> Option<Vec<u8>> {
    let n: u8 = key[1..].parse().ok()?;
    Some(match n {
        1..=4 => {
            let c = [b'P', b'Q', b'R', b'S'][(n - 1) as usize] as char;
            if m.param() > 1 {
                format!("\x1b[1;{}{c}", m.param()).into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        5 => tilde(15, m),
        6 => tilde(17, m),
        7 => tilde(18, m),
        8 => tilde(19, m),
        9 => tilde(20, m),
        10 => tilde(21, m),
        11 => tilde(23, m),
        12 => tilde(24, m),
        _ => return None,
    })
}

fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' | ' ' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '8' => 0x7f,
        _ => return None,
    })
}

fn shifted(input: &KeyInput) -> String {
    match input.text {
        Some(t) if !t.is_empty() && t.chars().all(|c| !c.is_control()) && !input.mods.alt => t.to_string(),
        _ => input.key.to_uppercase(),
    }
}

// ---------- kitty ----------

fn kitty_functional(key: &str) -> Option<(u32, char)> {
    // (number, final byte). Final '~' keys use the number; letter finals use number 1.
    Some(match key {
        "enter" => (13, 'u'),
        "tab" => (9, 'u'),
        "backspace" => (127, 'u'),
        "escape" => (27, 'u'),
        "space" => (32, 'u'),
        "insert" => (2, '~'),
        "delete" => (3, '~'),
        "pageup" => (5, '~'),
        "pagedown" => (6, '~'),
        "up" => (1, 'A'),
        "down" => (1, 'B'),
        "right" => (1, 'C'),
        "left" => (1, 'D'),
        "home" => (1, 'H'),
        "end" => (1, 'F'),
        "f1" => (1, 'P'),
        "f2" => (1, 'Q'),
        "f3" => (13, '~'),
        "f4" => (1, 'S'),
        "f5" => (15, '~'),
        "f6" => (17, '~'),
        "f7" => (18, '~'),
        "f8" => (19, '~'),
        "f9" => (20, '~'),
        "f10" => (21, '~'),
        "f11" => (23, '~'),
        "f12" => (24, '~'),
        _ => return None,
    })
}

fn encode_kitty(input: &KeyInput, mode: TermMode, opts: &KeyOptions) -> Option<Vec<u8>> {
    let m = input.mods;
    let all_as_esc = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let event_types = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let alternates = mode.contains(TermMode::REPORT_ALTERNATE_KEYS);
    let assoc_text = mode.contains(TermMode::REPORT_ASSOCIATED_TEXT);
    // Without event-type reporting, repeats are indistinguishable from presses.
    let event = if event_types && input.repeat { ":2" } else { "" };
    let mods_field = |param: u8| -> String {
        if param > 1 || !event.is_empty() {
            format!("{param}{event}")
        } else {
            String::new()
        }
    };

    if let Some((num, fin)) = kitty_functional(input.key) {
        let legacy_text_key = matches!(input.key, "enter" | "tab" | "backspace" | "space");
        // In disambiguate-only mode, unmodified Enter/Tab/Backspace/Space keep legacy bytes.
        if legacy_text_key && !all_as_esc && m.param() == 1 {
            return encode_legacy(input, TermMode::empty(), opts);
        }
        let mf = mods_field(m.param());
        return Some(match fin {
            'u' | '~' => {
                if mf.is_empty() {
                    format!("\x1b[{num}{fin}")
                } else {
                    format!("\x1b[{num};{mf}{fin}")
                }
            }
            _ => {
                if mf.is_empty() {
                    format!("\x1b[{fin}")
                } else {
                    format!("\x1b[1;{mf}{fin}")
                }
            }
        }
        .into_bytes());
    }

    let base = single_char(input.key)?;
    let text_only = !m.any_except_shift();
    if text_only && !all_as_esc {
        // Plain and shifted characters are sent as text via the IME path.
        return None;
    }
    if m.alt && !m.ctrl && !opts.option_as_meta && !all_as_esc {
        // Option composes a special character; let text input handle it.
        return None;
    }
    let code = base.to_lowercase().next().unwrap_or(base) as u32;
    let mut key_field = code.to_string();
    if alternates && m.shift {
        if let Some(shifted) = input.text.and_then(|t| t.chars().next()) {
            if shifted as u32 != code {
                key_field.push_str(&format!(":{}", shifted as u32));
            }
        }
    }
    let mut mf = mods_field(m.param());
    if assoc_text && text_only {
        if let Some(t) = input.text.filter(|t| !t.is_empty()) {
            let cps: Vec<String> = t.chars().map(|c| (c as u32).to_string()).collect();
            if mf.is_empty() {
                mf = "1".into();
            }
            mf.push(';');
            mf.push_str(&cps.join(":"));
        }
    }
    Some(if mf.is_empty() {
        format!("\x1b[{key_field}u")
    } else {
        format!("\x1b[{key_field};{mf}u")
    }
    .into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::TermMode;

    fn key(k: &str) -> KeyInput<'_> {
        KeyInput { key: k, text: None, mods: Mods::default(), repeat: false }
    }
    fn with(k: &str, text: Option<&'static str>, mods: Mods) -> KeyInput<'static> {
        KeyInput { key: Box::leak(k.to_string().into_boxed_str()), text, mods, repeat: false }
    }
    fn legacy(i: &KeyInput) -> Option<String> {
        encode_key(i, TermMode::empty(), &KeyOptions::default()).map(|b| String::from_utf8(b).unwrap())
    }
    fn legacy_meta(i: &KeyInput) -> Option<String> {
        encode_key(i, TermMode::empty(), &KeyOptions { option_as_meta: true }).map(|b| String::from_utf8(b).unwrap())
    }
    fn kitty(i: &KeyInput, mode: TermMode) -> Option<String> {
        encode_key(i, mode, &KeyOptions::default()).map(|b| String::from_utf8(b).unwrap())
    }
    const SHIFT: Mods = Mods { shift: true, alt: false, ctrl: false, cmd: false };
    const CTRL: Mods = Mods { shift: false, alt: false, ctrl: true, cmd: false };
    const ALT: Mods = Mods { shift: false, alt: true, ctrl: false, cmd: false };
    const CMD: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: true };
    const CTRL_SHIFT: Mods = Mods { shift: true, alt: false, ctrl: true, cmd: false };

    #[test]
    fn plain_text_goes_to_ime() {
        assert_eq!(legacy(&with("a", Some("a"), Mods::default())), None);
        assert_eq!(legacy(&with("a", Some("A"), SHIFT)), None);
    }

    #[test]
    fn cmd_is_never_encoded() {
        assert_eq!(legacy(&with("c", None, CMD)), None);
        assert_eq!(kitty(&with("c", None, CMD), TermMode::DISAMBIGUATE_ESC_CODES), None);
    }

    #[test]
    fn legacy_named_keys() {
        assert_eq!(legacy(&key("enter")).as_deref(), Some("\r"));
        assert_eq!(legacy(&with("enter", None, SHIFT)).as_deref(), Some("\x1b\r"));
        assert_eq!(legacy(&key("backspace")).as_deref(), Some("\x7f"));
        assert_eq!(legacy(&key("escape")).as_deref(), Some("\x1b"));
        assert_eq!(legacy(&with("tab", None, SHIFT)).as_deref(), Some("\x1b[Z"));
        assert_eq!(legacy(&key("up")).as_deref(), Some("\x1b[A"));
        assert_eq!(legacy(&with("left", None, ALT)).as_deref(), Some("\x1b[1;3D"));
        assert_eq!(legacy(&key("pageup")).as_deref(), Some("\x1b[5~"));
        assert_eq!(legacy(&with("delete", None, CTRL)).as_deref(), Some("\x1b[3;5~"));
        assert_eq!(legacy(&key("f1")).as_deref(), Some("\x1bOP"));
        assert_eq!(legacy(&key("f5")).as_deref(), Some("\x1b[15~"));
    }

    #[test]
    fn app_cursor_mode() {
        let s = encode_key(&key("up"), TermMode::APP_CURSOR, &KeyOptions::default()).unwrap();
        assert_eq!(s, b"\x1bOA");
    }

    #[test]
    fn ctrl_and_meta() {
        assert_eq!(legacy(&with("c", None, CTRL)).as_deref(), Some("\x03"));
        assert_eq!(legacy(&with("[", None, CTRL)).as_deref(), Some("\x1b"));
        assert_eq!(legacy(&with("space", None, CTRL)).as_deref(), Some("\x00"));
        // Option without meta: macOS character goes to text input.
        assert_eq!(legacy(&with("f", Some("ƒ"), ALT)), None);
        assert_eq!(legacy_meta(&with("f", Some("ƒ"), ALT)).as_deref(), Some("\x1bf"));
        assert_eq!(legacy_meta(&with("enter", None, ALT)).as_deref(), Some("\x1b\r"));
    }

    #[test]
    fn option_space_as_meta() {
        assert_eq!(legacy_meta(&with("space", Some("\u{a0}"), ALT)).as_deref(), Some("\x1b "));
        assert_eq!(legacy(&with("space", Some("\u{a0}"), ALT)), None);
    }

    #[test]
    fn kitty_disambiguate() {
        let m = TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(kitty(&with("a", Some("a"), Mods::default()), m), None);
        assert_eq!(kitty(&key("enter"), m).as_deref(), Some("\r"));
        assert_eq!(kitty(&with("enter", None, SHIFT), m).as_deref(), Some("\x1b[13;2u"));
        assert_eq!(kitty(&key("escape"), m).as_deref(), Some("\x1b[27u"));
        assert_eq!(kitty(&with("c", None, CTRL), m).as_deref(), Some("\x1b[99;5u"));
        assert_eq!(kitty(&with("c", None, CTRL_SHIFT), m).as_deref(), Some("\x1b[99;6u"));
        assert_eq!(kitty(&key("up"), m).as_deref(), Some("\x1b[A"));
        assert_eq!(kitty(&with("up", None, SHIFT), m).as_deref(), Some("\x1b[1;2A"));
        assert_eq!(kitty(&key("delete"), m).as_deref(), Some("\x1b[3~"));
    }

    #[test]
    fn kitty_all_keys_as_escapes() {
        let m = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(kitty(&with("a", Some("a"), Mods::default()), m).as_deref(), Some("\x1b[97u"));
        assert_eq!(kitty(&with("a", Some("A"), SHIFT), m).as_deref(), Some("\x1b[97;2u"));
        assert_eq!(kitty(&key("enter"), m).as_deref(), Some("\x1b[13u"));
        let alt = m | TermMode::REPORT_ALTERNATE_KEYS;
        assert_eq!(kitty(&with("a", Some("A"), SHIFT), alt).as_deref(), Some("\x1b[97:65;2u"));
        let text = m | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(kitty(&with("a", Some("a"), Mods::default()), text).as_deref(), Some("\x1b[97;1;97u"));
    }

    #[test]
    fn kitty_repeat_event_type() {
        let m = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
        let mut k = key("escape");
        k.repeat = true;
        assert_eq!(kitty(&k, m).as_deref(), Some("\x1b[27;1:2u"));
    }
}
