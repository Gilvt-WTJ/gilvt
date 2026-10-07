//! End-to-end tests against a real PTY. Each test runs a tiny shell script.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use gilvt_term::snapshot::take_snapshot;
use gilvt_term::{Palette, SessionOptions, TermEvent, TermMode, TermSession, TermSize};

fn spawn(script: &str) -> TermSession {
    let mut opts = SessionOptions::new(TermSize { cols: 40, rows: 5, cell_width: 8, cell_height: 16 });
    opts.program = Some("/bin/sh".into());
    opts.args = vec!["-c".into(), script.into()];
    TermSession::spawn(opts).expect("spawn pty")
}

/// Drains events until `pred` holds for the screen, or panics after 5s.
fn wait_until(session: &TermSession, mut pred: impl FnMut(&TermSession, &[TermEvent]) -> bool) -> Vec<TermEvent> {
    let rx = session.events();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    loop {
        while let Ok(ev) = rx.try_recv() {
            seen.push(ev);
        }
        if pred(session, &seen) {
            return seen;
        }
        assert!(Instant::now() < deadline, "timed out; events: {seen:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn first_row(session: &TermSession) -> String {
    let snap = take_snapshot(&*session.term().lock(), &Palette::dark(), None);
    snap.row_text(0).0.trim_end().to_string()
}

#[test]
fn renders_child_output() {
    let s = spawn("printf 'hello gilvt'; sleep 2");
    wait_until(&s, |s, _| first_row(s) == "hello gilvt");
}

#[test]
fn reports_env() {
    let s = spawn("printf \"$TERM_PROGRAM:$COLORTERM\"; sleep 2");
    wait_until(&s, |s, _| first_row(s) == "gilvt:truecolor");
}

#[test]
fn osc_notification_reaches_channel() {
    let s = spawn("printf '\\033]777;notify;Claude;Needs input\\007'; sleep 2");
    wait_until(&s, |_, evs| {
        evs.iter().any(|e| matches!(e, TermEvent::Notification(n) if n.body == "Needs input"))
    });
}

#[test]
fn modes_are_tracked() {
    // Bracketed paste, kitty keyboard push (flags=1), synchronized output begin/end.
    let s = spawn("printf '\\033[?2004h\\033[>1u\\033[?2026hSYNC\\033[?2026l'; sleep 2");
    wait_until(&s, |s, _| {
        let mode = *s.term().lock().mode();
        mode.contains(TermMode::BRACKETED_PASTE) && mode.contains(TermMode::DISAMBIGUATE_ESC_CODES) && first_row(s) == "SYNC"
    });
}

#[test]
fn input_is_echoed_back() {
    // `R` is printed only after echo is disabled, so input written afterwards is not echoed.
    let s = spawn("stty -echo; printf R; read line; printf \"got:$line\"; sleep 2");
    wait_until(&s, |s, _| first_row(s) == "R");
    s.write_input(b"abc\r".to_vec());
    wait_until(&s, |s, _| first_row(s) == "Rgot:abc");
}

#[test]
fn device_status_report_is_answered() {
    // `printf '\033[6n'` asks for the cursor position; the reply comes back as PtyWrite.
    let s = spawn("printf 'x\\033[6n'; sleep 2");
    let evs = wait_until(&s, |_, evs| evs.iter().any(|e| matches!(e, TermEvent::PtyWrite(_))));
    let reply = evs.iter().find_map(|e| match e { TermEvent::PtyWrite(t) => Some(t.clone()), _ => None }).unwrap();
    assert_eq!(reply, "\x1b[1;2R");
    assert!(s.handle_reply(&TermEvent::PtyWrite(reply), &Palette::dark()));
}

#[test]
fn child_exit_is_reported() {
    let s = spawn("exit 3");
    wait_until(&s, |_, evs| evs.iter().any(|e| matches!(e, TermEvent::ChildExit(Some(3)))));
}

#[test]
fn foreground_cwd_follows_cd() {
    let s = spawn("cd /tmp && sleep 3");
    wait_until(&s, |s, _| s.cwd().and_then(|p| p.canonicalize().ok()) == Some("/private/tmp".into()));
}

#[test]
fn search_finds_older_output() {
    let mut s = spawn("printf 'alpha\\nbeta\\ngamma'; sleep 2");
    wait_until(&s, |s, _| {
        let snap = take_snapshot(&*s.term().lock(), &Palette::dark(), None);
        snap.row_text(2).0.trim_end() == "gamma"
    });
    let m = s.search("beta").expect("match");
    assert_eq!(m.start().line.0, 1);
    assert_eq!(m.start().column.0, 0);
    // Search wraps around: the only match is found again.
    let again = s.search_step(gilvt_term::Direction::Left).expect("wrapped match");
    assert_eq!(again.start().line.0, 1);
    s.end_search();
    assert!(s.search_match().is_none());
}

#[test]
fn clear_history_drops_scrollback() {
    let mut s = spawn("seq 1 50; sleep 2");
    wait_until(&s, |s, _| s.term().lock().grid().history_size() > 0);
    s.search("7");
    s.clear_history();
    assert_eq!(s.term().lock().grid().history_size(), 0);
    assert!(s.search_match().is_none());
}

#[test]
fn absolute_line_survives_scrolling_and_clear_history() {
    let mut s = spawn("printf 'first\\n'; sleep 0.5; seq 1 40; sleep 3");
    wait_until(&s, |s, _| first_row(s) == "first");
    // The cursor is on the row below "first".
    let below = s.absolute_cursor_line().expect("main screen");
    assert_eq!(below, 1);
    wait_until(&s, |s, _| s.term().lock().grid().history_size() >= 30);
    assert_eq!(s.scroll_to_absolute(0), gilvt_term::ScrollOutcome::Shown { viewport_row: 0 });
    assert_eq!(first_row(&s), "first");
    assert_eq!(s.absolute_rows_visible(0, 3), Some(0..3));
    s.clear_history();
    assert_eq!(s.scroll_to_absolute(0), gilvt_term::ScrollOutcome::Evicted);
    assert!(s.absolute_cursor_line().expect("main screen") > below);
}

#[test]
fn raw_key_mode_follows_the_tty() {
    // Cooked until the script switches the tty to raw (what zle / readline / TUIs do), and `R` marks the switch.
    let s = spawn("printf C; sleep 0.5; stty raw -echo; printf R; sleep 2");
    wait_until(&s, |s, _| first_row(s) == "C");
    assert!(!s.reads_raw_keys());
    wait_until(&s, |s, _| first_row(s) == "CR");
    assert!(s.reads_raw_keys());
}
