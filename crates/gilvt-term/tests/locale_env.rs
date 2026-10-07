//! Exercises the real `detect_child_lang()` path in its own process (so the `OnceLock` cache is
//! fresh), verifying that a child spawned with no LANG/LC_ALL/LC_CTYPE in gilvt's own environment
//! still gets a usable UTF-8 locale via the `defaults read -g AppleLocale` fallback in
//! `locale.rs`.

use std::time::{Duration, Instant};

use gilvt_term::snapshot::take_snapshot;
use gilvt_term::{Palette, SessionOptions, TermSession, TermSize};

fn first_row(session: &TermSession) -> String {
    let snap = take_snapshot(&*session.term().lock(), &Palette::dark(), None);
    snap.row_text(0).0.trim_end().to_string()
}

#[test]
fn child_gets_a_utf8_locale() {
    // This test is the only one in this binary, so it is safe to mutate the process environment.
    std::env::remove_var("LANG");
    std::env::remove_var("LC_ALL");
    std::env::remove_var("LC_CTYPE");

    let mut opts = SessionOptions::new(TermSize { cols: 40, rows: 5, cell_width: 8, cell_height: 16 });
    opts.program = Some("/bin/sh".into());
    opts.args = vec!["-c".into(), "printf \"$LANG\"; sleep 2".into()];
    let s = TermSession::spawn(opts).expect("spawn pty");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if first_row(&s).ends_with(".UTF-8") {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for a UTF-8 LANG; last row: {:?}", first_row(&s));
        std::thread::sleep(Duration::from_millis(10));
    }
}
