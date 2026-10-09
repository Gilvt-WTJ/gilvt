//! The 「对比」 overlay's pure half (E2b-1 §3): the disk version (old) against the user's unsaved version (new).
//! Reading the disk, building rows, folds, keys and button routing live here; `chrome::compare_overlay`
//! draws it and `EditorView` runs the effects.

use std::collections::HashSet;
use std::io;
use std::path::Path;

use gilvt_editor::{decode_with, normalize_newlines, Decoded, EditorError, Encoding, LineEnding, MAX_FILE_SIZE};
pub use gilvt_viewer::diff::{diff_texts, Diff, LineKind, Row, SplitRow};
use gilvt_viewer::diff::{split_rows, unified_rows};

use super::view::{BarAction, KeyCmd};

/// At or above this many columns the overlay shows the two versions side by side.
pub const SPLIT_MIN_COLS: usize = 80;
/// Unchanged lines kept around each change; longer runs fold.
const CONTEXT: usize = 3;

pub const MISSING_TEXT: &str = "文件已被删除。";
pub const ESC_HINT: &str = "Esc 返回编辑";

pub fn header_text(name: &str) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("Compare · {name}  Disk Version ↔ Your Version (Unsaved)")
    } else {
        format!("对比 · {name}　磁盘版本 ↔ 你的版本（未保存）")
    }
}

/// The bar text when the overlay could not be opened: says it is the compare that failed, not a save.
pub fn open_error_message(e: &EditorError) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("Could not read the version on disk: {e}")
    } else {
        format!("无法读取磁盘版本：{e}")
    }
}

pub fn fold_label(len: usize) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("⋯ {} (click to expand)", crate::i18n::count(len, "unchanged line", "unchanged lines"))
    } else {
        format!("⋯ {len} 行未改动（点击展开）")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareRows {
    Same,
    Unified(Vec<Row>),
    Split(Vec<SplitRow>),
}

/// The open overlay. `rows` is derived from `diff`, `expanded` and `width_cols`; `scroll` is the first row shown.
#[derive(Clone, Debug)]
pub struct Compare {
    pub diff: Diff,
    pub rows: CompareRows,
    /// `start` indices of folds the user opened.
    pub expanded: HashSet<usize>,
    pub disk_missing: bool,
    pub scroll: usize,
    /// What `CompareRows::Same` shows ([`same_note`]).
    pub note: &'static str,
    /// The pane width (in cells) the rows were laid out for.
    pub width_cols: usize,
}

fn rows_for(diff: &Diff, width_cols: usize, expanded: &HashSet<usize>) -> CompareRows {
    if !diff.has_changes() {
        CompareRows::Same
    } else if width_cols >= SPLIT_MIN_COLS {
        CompareRows::Split(split_rows(diff, CONTEXT, expanded))
    } else {
        CompareRows::Unified(unified_rows(diff, CONTEXT, expanded))
    }
}

/// `disk` is the old side, `mine` the new: removed lines are on disk only, added lines are mine only.
pub fn build(disk: &str, mine: &str, width_cols: usize, expanded: &HashSet<usize>) -> (Diff, CompareRows) {
    let diff = diff_texts(disk, mine);
    let rows = rows_for(&diff, width_cols, expanded);
    (diff, rows)
}

/// What to say when the texts match (after newline normalization).
pub fn same_note(disk_le: LineEnding, mine_le: LineEnding, disk_enc: &str, mine_enc: &str) -> &'static str {
    if disk_le != mine_le {
        crate::i18n::text(
            "内容相同，只有换行符不同。",
            "The content is identical; only the line endings differ.",
        )
    } else if disk_enc != mine_enc {
        crate::i18n::text(
            "内容相同，只有编码不同。",
            "The content is identical; only the encoding differs.",
        )
    } else {
        crate::i18n::text("内容相同。", "The content is identical.")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareAction {
    UseDisk,
    KeepMine,
    Overwrite,
    Close,
}

/// The bottom buttons, left to right: label, action, danger.
pub fn buttons(disk_missing: bool) -> Vec<(&'static str, CompareAction, bool)> {
    if disk_missing {
        vec![(
            crate::i18n::text("保留我的，稍后再说", "Keep Mine for Now"),
            CompareAction::KeepMine,
            false,
        )]
    } else {
        vec![
            (
                crate::i18n::text("用磁盘版本", "Use Disk Version"),
                CompareAction::UseDisk,
                false,
            ),
            (
                crate::i18n::text("保留我的，稍后再说", "Keep Mine for Now"),
                CompareAction::KeepMine,
                false,
            ),
            (
                crate::i18n::text("仍然覆盖磁盘", "Overwrite Disk Anyway"),
                CompareAction::Overwrite,
                true,
            ),
        ]
    }
}

/// Every action closes the overlay first; then the banner's own action runs (so 仍然覆盖磁盘 keeps the
/// banner's `closing` flag through `plan`). `None`: only close (the banner stays).
pub fn apply_action(action: CompareAction) -> Option<BarAction> {
    match action {
        CompareAction::UseDisk => Some(BarAction::Reload),
        CompareAction::Overwrite => Some(BarAction::Overwrite),
        CompareAction::KeepMine | CompareAction::Close => None,
    }
}

/// What a key does while the overlay is open. Everything not listed is swallowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareKey {
    Action(CompareAction),
    /// Rows.
    Scroll(i64),
    /// Pages.
    Page(i64),
    Swallow,
}

pub fn compare_key(cmd: Option<KeyCmd>) -> CompareKey {
    match cmd {
        Some(KeyCmd::Escape) => CompareKey::Action(CompareAction::Close),
        Some(KeyCmd::Rows(d, _)) => CompareKey::Scroll(d),
        Some(KeyCmd::Page(d, _)) => CompareKey::Page(d),
        _ => CompareKey::Swallow,
    }
}

/// `scroll` moved by `delta` rows, kept on a row (`0..len`).
pub fn scrolled(scroll: usize, delta: i64, len: usize) -> usize {
    (scroll as i64 + delta).clamp(0, len.saturating_sub(1) as i64) as usize
}

/// Opens a closed fold (or closes an open one).
pub fn toggle_fold(expanded: &mut HashSet<usize>, start: usize) {
    if !expanded.remove(&start) {
        expanded.insert(start);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskText {
    Text(Decoded),
    Missing,
}

/// The file as it is on disk now, decoded with the buffer's explicit encoding (auto-detected without one).
/// Never fails on the bytes: what will not decode faithfully decodes lossily, and bytes the editor would
/// call binary are shown as lossy UTF-8. A missing file is `Missing`.
///
/// # Errors
/// `NotAFile` (a directory in its place), `TooLarge` above `MAX_FILE_SIZE` (the same cap as opening: this runs
/// on the UI thread, and the file is likely being rewritten by something else), and other read errors.
pub fn read_disk_for_compare(path: &Path, explicit: Option<Encoding>) -> Result<DiskText, EditorError> {
    read_disk_for_compare_with_limit(path, explicit, MAX_FILE_SIZE)
}

/// [`read_disk_for_compare`] with the size cap as a parameter. The cap is checked on the metadata before
/// reading, and the read itself stops one byte past it, so a file growing in between is refused too.
pub(crate) fn read_disk_for_compare_with_limit(path: &Path, explicit: Option<Encoding>, limit: u64) -> Result<DiskText, EditorError> {
    let missing = |e: &io::Error| e.kind() == io::ErrorKind::NotFound;
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if missing(&e) => return Ok(DiskText::Missing),
        Err(e) => return Err(EditorError::Io(e)),
    };
    if !meta.is_file() {
        return Err(EditorError::NotAFile);
    }
    if meta.len() > limit {
        return Err(EditorError::TooLarge { size: meta.len(), limit });
    }
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if missing(&e) => return Ok(DiskText::Missing),
        Err(e) => return Err(EditorError::Io(e)),
    };
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    io::Read::read_to_end(&mut io::Read::take(file, limit + 1), &mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(EditorError::TooLarge { size: bytes.len() as u64, limit });
    }
    let decoded = decode_with(&bytes, explicit, false)
        .or_else(|_| decode_with(&bytes, explicit, true))
        .map(|(d, _)| d)
        .unwrap_or_else(|_| {
            let raw = String::from_utf8_lossy(&bytes);
            let line_ending = if raw.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
            Decoded { text: normalize_newlines(&raw), encoding: explicit.unwrap_or(Encoding::Utf8), line_ending, mixed_line_endings: false }
        });
    Ok(DiskText::Text(decoded))
}

impl Compare {
    /// Reads the disk afresh and compares it with `mine` (the buffer's normalized text).
    ///
    /// # Errors
    /// See [`read_disk_for_compare`].
    pub fn open(
        path: &Path,
        explicit: Option<Encoding>,
        mine: &str,
        mine_le: LineEnding,
        mine_enc: &str,
        width_cols: usize,
    ) -> Result<Compare, EditorError> {
        let expanded = HashSet::new();
        let (diff, rows, note, disk_missing) = match read_disk_for_compare(path, explicit)? {
            DiskText::Missing => (Diff::default(), CompareRows::Unified(Vec::new()), "", true),
            DiskText::Text(d) => {
                let (diff, rows) = build(&d.text, mine, width_cols, &expanded);
                (diff, rows, same_note(d.line_ending, mine_le, d.encoding.name(), mine_enc), false)
            }
        };
        Ok(Compare { diff, rows, expanded, disk_missing, scroll: 0, note, width_cols })
    }

    pub fn row_count(&self) -> usize {
        match &self.rows {
            CompareRows::Same => 0,
            CompareRows::Unified(r) => r.len(),
            CompareRows::Split(r) => r.len(),
        }
    }

    fn rebuild(&mut self) {
        if !self.disk_missing {
            self.rows = rows_for(&self.diff, self.width_cols, &self.expanded);
        }
        self.scroll = self.scroll.min(self.row_count().saturating_sub(1));
    }

    pub fn toggle_fold(&mut self, start: usize) {
        toggle_fold(&mut self.expanded, start);
        self.rebuild();
    }

    /// Re-lays out for a new pane width (only when it crosses `SPLIT_MIN_COLS`).
    pub fn set_width(&mut self, width_cols: usize) {
        let split = |w| w >= SPLIT_MIN_COLS;
        if split(width_cols) != split(self.width_cols) {
            self.width_cols = width_cols;
            self.rebuild();
        }
    }

    pub fn scroll_by(&mut self, delta: i64) {
        self.scroll = scrolled(self.scroll, delta, self.row_count());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn identical_texts_are_the_same_and_the_note_names_the_difference() {
        let (_, rows) = build("a\nb\n", "a\nb\n", 120, &HashSet::new());
        assert!(matches!(rows, CompareRows::Same));
        assert_eq!(same_note(LineEnding::Lf, LineEnding::CrLf, "UTF-8", "UTF-8"), "内容相同，只有换行符不同。");
        assert_eq!(same_note(LineEnding::Lf, LineEnding::Lf, "UTF-8", "GBK"), "内容相同，只有编码不同。");
        assert_eq!(same_note(LineEnding::Lf, LineEnding::Lf, "UTF-8", "UTF-8"), "内容相同。");
    }

    #[test]
    fn width_picks_split_or_unified() {
        let disk = "a\nb\nc\n";
        let mine = "a\nB\nc\n";
        assert!(matches!(build(disk, mine, 80, &HashSet::new()).1, CompareRows::Split(_)));
        assert!(matches!(build(disk, mine, 79, &HashSet::new()).1, CompareRows::Unified(_)));
    }

    #[test]
    fn disk_is_old_and_mine_is_new() {
        let (diff, _) = build("tags: [review]\n", "tags: [review, mine]\n", 120, &HashSet::new());
        let removed: Vec<_> = diff.lines.iter().filter(|l| l.kind == LineKind::Removed).map(|l| l.text.as_str()).collect();
        let added: Vec<_> = diff.lines.iter().filter(|l| l.kind == LineKind::Added).map(|l| l.text.as_str()).collect();
        assert_eq!(removed, ["tags: [review]"]);
        assert_eq!(added, ["tags: [review, mine]"]);
    }

    #[test]
    fn long_unchanged_runs_fold_and_expand() {
        let disk: String = (0..40).map(|i| format!("l{i}\n")).collect();
        let mine = disk.replace("l39\n", "L39\n");
        let (_, rows) = build(&disk, &mine, 120, &HashSet::new());
        let CompareRows::Split(rows) = rows else { panic!() };
        let fold = rows.iter().find_map(|r| match r { SplitRow::Fold { start, len } => Some((*start, *len)), _ => None }).expect("a fold");
        let mut open = HashSet::new();
        open.insert(fold.0);
        let (_, rows2) = build(&disk, &mine, 120, &open);
        let CompareRows::Split(rows2) = rows2 else { panic!() };
        assert!(rows2.iter().all(|r| !matches!(r, SplitRow::Fold { .. })));
        assert!(rows2.len() > rows.len());
    }

    #[test]
    fn buttons_follow_the_disk_state() {
        let present: Vec<_> = buttons(false).iter().map(|b| b.0).collect();
        assert_eq!(present, ["用磁盘版本", "保留我的，稍后再说", "仍然覆盖磁盘"]);
        assert_eq!(buttons(true).iter().map(|b| b.0).collect::<Vec<_>>(), ["保留我的，稍后再说"]);
        assert!(buttons(false).iter().any(|b| b.2), "overwrite is the danger button");
    }

    // ---------- beyond the brief: disk reader, fold toggle, keys, button routing, scroll ----------

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn a_missing_file_reads_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(read_disk_for_compare(&dir.path().join("gone.md"), None), Ok(DiskText::Missing)));
    }

    #[test]
    fn the_explicit_encoding_decodes_the_disk_text() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(&dir, "gbk.txt", &[0xC4, 0xE3, 0xBA, 0xC3, b'\n']);
        let gbk = gilvt_editor::encoding_by_label("gbk").unwrap();
        let Ok(DiskText::Text(d)) = read_disk_for_compare(&p, Some(gbk)) else { panic!() };
        assert_eq!((d.text.as_str(), d.encoding.name()), ("你好\n", "GBK"));
    }

    #[test]
    fn undecodable_bytes_decode_lossy_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(&dir, "bad.txt", b"ok\xff\n");
        let Ok(DiskText::Text(d)) = read_disk_for_compare(&p, Some(Encoding::Utf8)) else { panic!() };
        assert!(d.text.starts_with("ok") && d.text.contains('\u{FFFD}'), "{:?}", d.text);
        // Even bytes the editor calls binary still give something to look at.
        let p = write(&dir, "bin.dat", b"a\0b\r\n");
        let Ok(DiskText::Text(d)) = read_disk_for_compare(&p, None) else { panic!() };
        assert_eq!(d.text, "a\0b\n");
    }

    #[test]
    fn crlf_on_disk_against_lf_mine_is_the_same_with_an_endings_note() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(&dir, "a.md", b"one\r\ntwo\r\n");
        let Ok(DiskText::Text(d)) = read_disk_for_compare(&p, None) else { panic!() };
        assert_eq!(d.line_ending, LineEnding::CrLf);
        let (_, rows) = build(&d.text, "one\ntwo\n", 120, &HashSet::new());
        assert!(matches!(rows, CompareRows::Same));
        assert_eq!(same_note(d.line_ending, LineEnding::Lf, d.encoding.name(), "UTF-8"), "内容相同，只有换行符不同。");
    }

    #[test]
    fn opening_builds_the_state_from_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(&dir, "a.md", b"a\nb\n");
        let c = Compare::open(&p, None, "a\nB\n", LineEnding::Lf, "UTF-8", 120).unwrap();
        assert!(!c.disk_missing && matches!(c.rows, CompareRows::Split(_)) && c.scroll == 0);
        // The disk is the old side: its line is the removed one, the user's line the added one.
        let text = |k| c.diff.lines.iter().filter(|l| l.kind == k).map(|l| l.text.as_str()).collect::<Vec<_>>();
        assert_eq!(text(LineKind::Removed), ["b"]);
        assert_eq!(text(LineKind::Added), ["B"]);
        let c = Compare::open(&dir.path().join("gone.md"), None, "a\n", LineEnding::Lf, "UTF-8", 120).unwrap();
        assert!(c.disk_missing && c.row_count() == 0);
        std::fs::write(&p, "a\nB\n").unwrap();
        let c = Compare::open(&p, None, "a\nB\n", LineEnding::Lf, "UTF-8", 40).unwrap();
        assert_eq!((matches!(c.rows, CompareRows::Same), c.note), (true, "内容相同。"));
        // A directory where the file was is a read error, not a deletion.
        assert!(Compare::open(dir.path(), None, "", LineEnding::Lf, "UTF-8", 40).is_err());
    }

    #[test]
    fn a_disk_file_over_the_limit_is_refused_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let at = write(&dir, "at.md", &[b'x'; 10]);
        let over = write(&dir, "over.md", &[b'x'; 11]);
        assert!(matches!(read_disk_for_compare_with_limit(&at, None, 10), Ok(DiskText::Text(d)) if d.text.len() == 10));
        assert!(matches!(
            read_disk_for_compare_with_limit(&over, None, 10),
            Err(EditorError::TooLarge { size: 11, limit: 10 })
        ));
        // Missing still wins over the size check.
        assert!(matches!(read_disk_for_compare_with_limit(&dir.path().join("gone"), None, 10), Ok(DiskText::Missing)));
    }

    #[test]
    fn a_directory_in_the_files_place_is_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(read_disk_for_compare(dir.path(), None), Err(EditorError::NotAFile)));
    }

    #[test]
    fn a_failed_open_reads_as_a_compare_problem() {
        assert_eq!(open_error_message(&EditorError::NotAFile), format!("无法读取磁盘版本：{}", EditorError::NotAFile));
        let e = EditorError::TooLarge { size: 11, limit: 10 };
        assert!(open_error_message(&e).starts_with("无法读取磁盘版本："));
    }

    #[test]
    fn esc_and_keep_mine_only_close() {
        // Esc maps to Close; Close and KeepMine run no banner action (the banner stays).
        let CompareKey::Action(esc) = compare_key(Some(KeyCmd::Escape)) else { panic!() };
        for a in [esc, CompareAction::KeepMine] {
            assert_eq!(apply_action(a), None, "{a:?}");
        }
    }

    #[test]
    fn toggling_a_fold_expands_it_and_rebuilds_the_rows() {
        let disk: String = (0..40).map(|i| format!("l{i}\n")).collect();
        let mine = disk.replace("l39\n", "L39\n");
        let (diff, rows) = build(&disk, &mine, 60, &HashSet::new());
        let mut c = Compare { diff, rows, expanded: HashSet::new(), disk_missing: false, scroll: 0, note: "", width_cols: 60 };
        let before = c.row_count();
        let CompareRows::Unified(ref r) = c.rows else { panic!() };
        let Some(Row::Fold { start, .. }) = r.first().copied() else { panic!("{r:?}") };
        c.toggle_fold(start);
        assert!(c.expanded.contains(&start));
        assert!(c.row_count() > before);
        let mut e = HashSet::new();
        toggle_fold(&mut e, 3);
        toggle_fold(&mut e, 3);
        assert!(e.is_empty(), "toggling twice folds it again");
    }

    #[test]
    fn a_width_change_across_80_columns_switches_the_layout() {
        let (diff, rows) = build("a\n", "b\n", 100, &HashSet::new());
        let mut c = Compare { diff, rows, expanded: HashSet::new(), disk_missing: false, scroll: 0, note: "", width_cols: 100 };
        c.set_width(70);
        assert!(matches!(c.rows, CompareRows::Unified(_)));
        c.set_width(90);
        assert!(matches!(c.rows, CompareRows::Split(_)));
    }

    #[test]
    fn keys_close_scroll_or_are_swallowed() {
        use crate::editor::model::HMove;
        assert_eq!(compare_key(Some(KeyCmd::Escape)), CompareKey::Action(CompareAction::Close));
        assert_eq!(compare_key(Some(KeyCmd::Rows(-1, false))), CompareKey::Scroll(-1));
        assert_eq!(compare_key(Some(KeyCmd::Rows(1, true))), CompareKey::Scroll(1));
        assert_eq!(compare_key(Some(KeyCmd::Page(1, false))), CompareKey::Page(1));
        assert_eq!(compare_key(Some(KeyCmd::Page(-1, true))), CompareKey::Page(-1));
        // Enter never picks a button: no accidental reload or overwrite.
        for k in [KeyCmd::Enter, KeyCmd::Backspace, KeyCmd::Delete, KeyCmd::Tab(false), KeyCmd::Move(HMove::Left, false)] {
            assert_eq!(compare_key(Some(k)), CompareKey::Swallow, "{k:?}");
        }
        assert_eq!(compare_key(None), CompareKey::Swallow);
    }

    #[test]
    fn button_actions_close_and_then_reuse_the_banner_plan() {
        use crate::editor::view::{plan, BarAction, Bar, Plan};
        assert_eq!(apply_action(CompareAction::Close), None);
        assert_eq!(apply_action(CompareAction::KeepMine), None);
        assert_eq!(apply_action(CompareAction::UseDisk), Some(BarAction::Reload));
        assert_eq!(apply_action(CompareAction::Overwrite), Some(BarAction::Overwrite));
        // Overwrite keeps the banner's closing flag.
        let m = |closing| Bar::Modified { closing };
        assert_eq!(plan(&m(true), BarAction::Overwrite), Plan::Save { overwrite: true, closing: true });
        assert_eq!(plan(&m(false), BarAction::Overwrite), Plan::Save { overwrite: true, closing: false });
        assert_eq!(plan(&m(true), BarAction::Reload), Plan::Reload);
    }

    #[test]
    fn scrolling_clamps_to_the_rows() {
        assert_eq!(scrolled(0, -3, 10), 0);
        assert_eq!(scrolled(5, 3, 10), 8);
        assert_eq!(scrolled(5, 30, 10), 9);
        assert_eq!(scrolled(0, 1, 0), 0);
    }
}
