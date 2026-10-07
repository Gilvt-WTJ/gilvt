//! Mouse reporting (X10/normal and SGR) and alternate-scroll translation.

use alacritty_terminal::term::TermMode;

use crate::keys::Mods;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    /// Motion; `Some(button)` while a button is held (drag).
    Motion(Option<Button>),
}

/// True when the running program wants mouse events instead of local selection.
pub fn reporting_enabled(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

pub fn encode_mouse(button: Button, action: Action, col: usize, row: usize, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    if !reporting_enabled(mode) {
        return None;
    }
    let (btn, motion) = match action {
        Action::Motion(held) => {
            let allowed = mode.contains(TermMode::MOUSE_MOTION) || (held.is_some() && mode.contains(TermMode::MOUSE_DRAG));
            if !allowed {
                return None;
            }
            (held.unwrap_or(button), true)
        }
        _ => (button, false),
    };
    if matches!(btn, Button::WheelUp | Button::WheelDown) && action == Action::Release {
        return None;
    }
    let mut code: u32 = match btn {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    if motion {
        code += 32;
        if matches!(action, Action::Motion(None)) {
            code = 35; // motion with no button
        }
    }
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let fin = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{fin}", col + 1, row + 1).into_bytes());
    }
    // X10 encoding: release has no button information.
    if action == Action::Release {
        code = 3 + (code & !3);
    }
    if col >= 223 || row >= 223 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', 32 + code as u8, 32 + col as u8 + 1, 32 + row as u8 + 1])
}

/// In the alternate screen without mouse reporting, wheel scrolls become arrow keys.
pub fn alternate_scroll(lines: i32, mode: TermMode) -> Option<Vec<u8>> {
    if lines == 0
        || reporting_enabled(mode)
        || !mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
    {
        return None;
    }
    let app = mode.contains(TermMode::APP_CURSOR);
    let seq: &[u8] = match (lines > 0, app) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    Some(seq.repeat(lines.unsigned_abs() as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };

    #[test]
    fn disabled_without_mode() {
        assert_eq!(encode_mouse(Button::Left, Action::Press, 0, 0, NONE, TermMode::empty()), None);
    }

    #[test]
    fn sgr_press_release() {
        let m = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(Button::Left, Action::Press, 4, 2, NONE, m).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(encode_mouse(Button::Left, Action::Release, 4, 2, NONE, m).unwrap(), b"\x1b[<0;5;3m");
        assert_eq!(encode_mouse(Button::WheelDown, Action::Press, 0, 0, NONE, m).unwrap(), b"\x1b[<65;1;1M");
    }

    #[test]
    fn drag_requires_drag_mode() {
        let click = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(Button::Left, Action::Motion(Some(Button::Left)), 1, 1, NONE, click), None);
        let drag = click | TermMode::MOUSE_DRAG;
        assert_eq!(
            encode_mouse(Button::Left, Action::Motion(Some(Button::Left)), 1, 1, NONE, drag).unwrap(),
            b"\x1b[<32;2;2M"
        );
    }

    #[test]
    fn x10_encoding() {
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(encode_mouse(Button::Right, Action::Press, 0, 0, NONE, m).unwrap(), vec![0x1b, b'[', b'M', 34, 33, 33]);
        assert_eq!(encode_mouse(Button::Right, Action::Release, 0, 0, NONE, m).unwrap(), vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn alternate_scroll_arrows() {
        let m = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(alternate_scroll(2, m).unwrap(), b"\x1b[A\x1b[A");
        assert_eq!(alternate_scroll(-1, m | TermMode::APP_CURSOR).unwrap(), b"\x1bOB");
        assert_eq!(alternate_scroll(1, TermMode::ALTERNATE_SCROLL), None);
    }
}
