//! Random edit sequences checked against a model that is too simple to be wrong: a `Vec<char>` and two
//! stacks of text snapshots. Seeds are fixed, so a failure reproduces; the message names the seed and step.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gilvt_editor::{Buffer, Selection};

/// xorshift64*: no dependency, same sequence on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: [&str; 9] = ["a", "xy", "\n", "世", "é", "e\u{301}", "👨\u{200d}👩", "  ", "foo_bar"];

fn random_text(rng: &mut Rng) -> String {
    (0..rng.below(4)).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

/// One undo step as the model remembers it: the text and the selection on both sides of the replacement.
struct Step {
    text_before: Vec<char>,
    selection_before: Selection,
    text_after: Vec<char>,
    selection_after: Selection,
}

#[test]
fn replace_undo_redo_match_a_snapshot_model() {
    for seed in 1..=25u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let initial: Vec<char> = "first line\nsecond 世界 line\n".chars().collect();
        let mut buffer = Buffer::from_text(&initial.iter().collect::<String>());
        let mut model = initial.clone();
        let mut undo: Vec<Step> = Vec::new();
        let mut redo: Vec<Step> = Vec::new();

        for step in 0..400 {
            let ctx = format!("seed {seed} step {step}");
            match rng.below(10) {
                0..=6 => {
                    let len = model.len();
                    let (a, b) = (rng.below(len + 3), rng.below(len + 3));
                    let (start, end) = (a.min(b).min(len), a.max(b).min(len));
                    let text = random_text(&mut rng);
                    let selection_before = buffer.selection();
                    buffer.replace(a.min(b)..a.max(b), &text).unwrap();
                    let inserted: Vec<char> = text.chars().collect();
                    if start == end && inserted.is_empty() {
                        continue_after_check(&buffer, &model, &ctx);
                        continue;
                    }
                    let text_before = model.clone();
                    model.splice(start..end, inserted.iter().copied());
                    let selection_after = Selection::caret(start + inserted.len());
                    assert_eq!(buffer.selection(), selection_after, "{ctx}: caret after replace");
                    undo.push(Step { text_before, selection_before, text_after: model.clone(), selection_after });
                    redo.clear();
                }
                7 | 8 => {
                    let had = !undo.is_empty();
                    assert_eq!(buffer.undo().is_some(), had, "{ctx}: undo availability");
                    if let Some(rec) = undo.pop() {
                        model = rec.text_before.clone();
                        assert_eq!(buffer.selection(), rec.selection_before, "{ctx}: selection restored by undo");
                        redo.push(rec);
                    }
                }
                _ => {
                    let had = !redo.is_empty();
                    assert_eq!(buffer.redo().is_some(), had, "{ctx}: redo availability");
                    if let Some(rec) = redo.pop() {
                        model = rec.text_after.clone();
                        assert_eq!(buffer.selection(), rec.selection_after, "{ctx}: selection restored by redo");
                        undo.push(rec);
                    }
                }
            }
            continue_after_check(&buffer, &model, &ctx);
            assert_eq!(buffer.dirty(), !undo.is_empty(), "{ctx}: dirty follows the undo depth");
        }
        // Walk to the very top of the history (steps undone during the run may still be ahead), then undo to
        // the bottom and redo to the top again.
        while buffer.redo().is_some() {}
        let top = buffer.text();
        while buffer.undo().is_some() {}
        assert_eq!(buffer.text(), initial.iter().collect::<String>(), "seed {seed}: undo everything");
        assert!(!buffer.dirty());
        while buffer.redo().is_some() {}
        assert_eq!(buffer.text(), top, "seed {seed}: redo everything");
    }
}

/// The checks every step shares: the buffer's text, length and line count equal the model's.
fn continue_after_check(buffer: &Buffer, model: &[char], ctx: &str) {
    let expected: String = model.iter().collect();
    assert_eq!(buffer.text(), expected, "{ctx}: text");
    assert_eq!(buffer.line_count(), expected.matches('\n').count() + 1, "{ctx}: line count");
    assert_eq!(buffer.len_chars(), model.len(), "{ctx}: length");
}

#[test]
fn any_mix_of_editing_commands_undoes_back_to_the_start() {
    for seed in 1..=25u64 {
        let mut rng = Rng(seed.wrapping_mul(0xD1B54A32D192ED03) | 1);
        let now = Arc::new(AtomicU64::new(0));
        let initial = "alpha beta\ngamma 世界\n\nlast";
        let mut b = Buffer::from_text(initial);
        let clock = now.clone();
        b.set_clock(move || clock.load(Ordering::SeqCst));
        for step in 0..300 {
            // Sometimes a pause (new undo step), mostly quick keystrokes (merging).
            now.fetch_add(if rng.below(5) == 0 { 2000 } else { 50 }, Ordering::SeqCst);
            let ctx = format!("seed {seed} step {step}");
            let r = match rng.below(14) {
                0..=3 => b.insert(PIECES[rng.below(4)]),
                4 => b.insert(&random_text(&mut rng)),
                5 => b.newline(),
                6 | 7 => b.backspace(),
                8 => b.delete_forward(),
                9 => b.delete_word_back(),
                10 => b.delete_to_line_end(),
                11 => b.delete_line(),
                12 => {
                    match rng.below(6) {
                        0 => b.move_left(rng.below(2) == 0),
                        1 => b.move_right(rng.below(2) == 0),
                        2 => b.move_up(rng.below(2) == 0),
                        3 => b.move_down(rng.below(2) == 0),
                        4 => b.move_word_left(rng.below(2) == 0),
                        _ => b.move_word_right(rng.below(2) == 0),
                    }
                    continue;
                }
                _ => {
                    b.select_all();
                    continue;
                }
            };
            r.unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let sel = b.selection();
            assert!(sel.anchor <= b.len_chars() && sel.head <= b.len_chars(), "{ctx}: selection in range");
        }
        let top = b.text();
        while b.undo().is_some() {}
        assert_eq!(b.text(), initial, "seed {seed}: undoing every step restores the start");
        assert!(!b.dirty(), "seed {seed}");
        while b.redo().is_some() {}
        assert_eq!(b.text(), top, "seed {seed}: redoing every step restores the end");
    }
}

#[test]
fn reload_as_edit_undoes_back_to_the_users_version() {
    for seed in 1..=40u64 {
        let mut rng = Rng(seed.wrapping_mul(0xA24BAED4963EE407) | 1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        let initial = "first line\nsecond 世界 line\n";
        std::fs::write(&path, initial).unwrap();
        let mut b = Buffer::open(&path).unwrap();
        for _ in 0..rng.below(8) {
            let len = b.len_chars();
            let (x, y) = (rng.below(len + 3), rng.below(len + 3));
            b.replace(x.min(y)..x.max(y), &random_text(&mut rng)).unwrap();
        }
        let mine = b.text();
        let disk = format!("disk {seed}\n{}\n{}", random_text(&mut rng), random_text(&mut rng));
        std::fs::write(&path, &disk).unwrap();
        b.reload_as_edit().unwrap();
        assert_eq!(b.text(), disk, "seed {seed}: text is the disk text");
        assert!(!b.dirty(), "seed {seed}: reloaded text counts as saved");
        assert!(b.undo().is_some(), "seed {seed}");
        assert_eq!(b.text(), mine, "seed {seed}: one undo returns to the user's version");
        assert!(b.dirty(), "seed {seed}");
        while b.undo().is_some() {}
        assert_eq!(b.text(), initial, "seed {seed}: undo to the bottom");
        while b.redo().is_some() {}
        assert_eq!(b.text(), disk, "seed {seed}: redo to the top");
    }
}
