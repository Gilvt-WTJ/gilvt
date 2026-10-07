//! Inputs a person will hit that the design does not spell out. Each test pins what a reasonable person
//! expects: nothing panics, nothing is silently lost, and a file you did not edit is saved byte for byte.

use std::fs;

use gilvt_editor::{encoding_by_label, Buffer, EditorError, Encoding, LineEnding, OpenOptions, Selection};

fn temp_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, bytes).unwrap();
    (dir, path)
}

#[test]
fn an_empty_file_and_an_empty_buffer_survive_every_command() {
    let (_dir, path) = temp_file(b"");
    let mut b = Buffer::open(&path).unwrap();
    assert_eq!((b.text().as_str(), b.line_count(), b.encoding()), ("", 1, Encoding::Utf8));
    b.move_left(false);
    b.move_right(true);
    b.move_up(false);
    b.move_down(true);
    b.move_word_left(false);
    b.move_word_right(false);
    b.move_line_start(false);
    b.move_line_end(false);
    b.select_all();
    b.backspace().unwrap();
    b.delete_forward().unwrap();
    b.delete_word_back().unwrap();
    b.delete_word_forward().unwrap();
    b.delete_to_line_end().unwrap();
    b.delete_line().unwrap();
    assert!(b.undo().is_none() && b.redo().is_none());
    assert!(!b.dirty(), "none of that changed anything");
    b.save().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"", "an untouched empty file stays empty");
    assert_eq!(b.line(0), "");
}

#[test]
fn a_file_that_is_not_valid_utf8_is_saved_back_byte_for_byte_when_unedited() {
    // A log cut in the middle of a multi-byte character; whatever encoding it is guessed to be, saving
    // without an edit must not change a byte.
    let cut = b"first line\nsecond line has a cut: \xe4\xb8";
    let (_dir, path) = temp_file(cut);
    let mut b = Buffer::open(&path).unwrap();
    assert!(matches!(b.encoding(), Encoding::Legacy(_)), "{:?}", b.encoding());
    b.save().unwrap();
    assert_eq!(fs::read(&path).unwrap(), cut.to_vec());
}

#[test]
fn mixed_line_endings_are_flagged_and_unified_to_the_dominant_one_on_save() {
    let (_dir, path) = temp_file(b"a\r\nb\r\nc\nd\r\n");
    let mut b = Buffer::open(&path).unwrap();
    assert!(b.mixed_line_endings());
    assert_eq!(b.line_ending(), LineEnding::CrLf);
    b.move_doc_end(false);
    b.insert("e").unwrap();
    b.save().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\nc\r\nd\r\ne");
}

#[test]
fn a_very_long_single_line_stays_correct() {
    let line = "x".repeat(200_000);
    let mut b = Buffer::from_text(&line);
    b.move_doc_end(false);
    b.move_left(false);
    assert_eq!(b.selection(), Selection::caret(199_999));
    b.insert("é").unwrap();
    b.move_line_start(false);
    b.move_word_right(false);
    assert_eq!(b.selection().head, 200_001, "one word-like run to the end of the line");
    assert_eq!(b.line_count(), 1);
    assert_eq!(b.position(200_001).col, 200_001);
}

#[test]
fn pasted_text_with_any_line_ending_becomes_plain_newlines() {
    let mut b = Buffer::from_text("");
    b.insert("one\r\ntwo\rthree\nfour").unwrap();
    assert_eq!(b.text(), "one\ntwo\nthree\nfour");
    assert_eq!(b.line_count(), 4);
    b.replace(0..b.len_chars(), "a\r\n").unwrap();
    assert_eq!((b.text().as_str(), b.line_count()), ("a\n", 2));
}

#[test]
fn a_directory_or_missing_path_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(Buffer::open(dir.path()), Err(EditorError::NotAFile)));
    assert!(Buffer::open(&dir.path().join("missing.txt")).is_err());
}

#[test]
fn a_gbk_file_chosen_by_hand_edits_and_saves_back_in_gbk() {
    let original = [0xC4, 0xE3, 0xBA, 0xC3, b'\n']; // 你好\n in GBK
    let (_dir, path) = temp_file(&original);
    let gbk = encoding_by_label("gbk").unwrap();
    // Auto-detection does not pick GBK for these few bytes, so choosing it by hand is what makes the difference.
    let auto = Buffer::open(&path);
    assert!(!matches!(&auto, Ok(a) if a.encoding() == Encoding::Legacy(encoding_rs::GBK)), "auto: {:?}", auto.map(|a| a.text()));
    let mut b = Buffer::open_with(&path, OpenOptions { encoding: Some(gbk), read_only: false }).unwrap();
    assert_eq!(b.text(), "你好\n");
    b.insert("新").unwrap();
    b.save().unwrap();
    let mut expected = encoding_rs::GBK.encode("新").0.into_owned();
    expected.extend_from_slice(&original);
    assert_eq!(fs::read(&path).unwrap(), expected);
}
