use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gilvt_agent::{AgentKind, ReviewPage, TurnCursor};

use super::model::*;

fn key(id: &str) -> gilvt_agent::SessionKey {
    (AgentKind::Claude, id.to_string())
}

fn cursor(id: &str) -> TurnCursor {
    TurnCursor::Claude {
        prompt_uuid: id.to_string(),
    }
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// 2001-09-09 00:00:00 UTC.
const DAY: u64 = 999_993_600;

#[test]
fn the_layout_is_wide_from_960_points() {
    assert_eq!(layout(959.), Layout::Narrow);
    assert_eq!(layout(960.), Layout::Wide);
}

#[test]
fn only_a_wide_review_grows_the_frame() {
    assert_eq!(frame_width(1400., false), 640.);
    assert_eq!(frame_width(1400., true), 1040.);
    assert_eq!(frame_width(1000., true), 952.);
    assert_eq!(frame_width(800., true), 640.);
}

#[test]
fn list_letters_stay_with_the_search_box() {
    for letter in ["s", "z", "p", "f", "[", "]"] {
        assert_eq!(
            key_action(Scope::List, letter, Mods::NONE),
            None,
            "{letter}"
        );
    }
    assert_eq!(
        key_action(Scope::List, "space", Mods::NONE),
        Some(Action::Open)
    );
    assert_eq!(
        key_action(Scope::List, "escape", Mods::NONE),
        Some(Action::Close)
    );
}

#[test]
fn detail_letters_drive_the_review() {
    assert_eq!(
        key_action(Scope::Detail, "s", Mods::NONE),
        Some(Action::Skip)
    );
    assert_eq!(
        key_action(Scope::Detail, "z", Mods::NONE),
        Some(Action::OpenSnoozeMenu)
    );
    assert_eq!(
        key_action(Scope::Detail, "p", Mods::NONE),
        Some(Action::Pin)
    );
    assert_eq!(
        key_action(Scope::Detail, "f", Mods::NONE),
        Some(Action::ToggleFullHistory)
    );
    assert_eq!(
        key_action(Scope::Detail, "[", Mods::NONE),
        Some(Action::Page(Paging::Earlier))
    );
    assert_eq!(
        key_action(Scope::Detail, "]", Mods::NONE),
        Some(Action::Page(Paging::Later))
    );
    assert_eq!(
        key_action(Scope::Detail, "up", Mods::NONE),
        Some(Action::Scroll(-1))
    );
    assert_eq!(
        key_action(Scope::Detail, "down", Mods::NONE),
        Some(Action::Scroll(1))
    );
    assert_eq!(
        key_action(Scope::Detail, "escape", Mods::NONE),
        Some(Action::Back)
    );
}

#[test]
fn command_enter_reviews_next_and_plain_enter_goes_back_to_the_agent() {
    for scope in [Scope::List, Scope::Detail] {
        assert_eq!(
            key_action(scope, "enter", Mods::CMD),
            Some(Action::ReviewNext)
        );
        assert_eq!(
            key_action(scope, "enter", Mods::NONE),
            Some(Action::BackToAgent)
        );
    }
}

#[test]
fn the_snooze_menu_takes_one_two_three_and_escape() {
    assert_eq!(
        key_action(Scope::SnoozeMenu, "1", Mods::NONE),
        Some(Action::Snooze(SnoozeChoice::Hour))
    );
    assert_eq!(
        key_action(Scope::SnoozeMenu, "2", Mods::NONE),
        Some(Action::Snooze(SnoozeChoice::Later))
    );
    assert_eq!(
        key_action(Scope::SnoozeMenu, "3", Mods::NONE),
        Some(Action::Snooze(SnoozeChoice::Tomorrow))
    );
    assert_eq!(
        key_action(Scope::SnoozeMenu, "escape", Mods::NONE),
        Some(Action::CloseSnoozeMenu)
    );
    assert_eq!(key_action(Scope::SnoozeMenu, "s", Mods::NONE), None);
}

#[test]
fn the_next_item_follows_the_current_one_and_wraps_without_repeating_it() {
    let keys = [key("a"), key("b"), key("c")];
    assert_eq!(next_key(&keys, &key("a")), Some(key("b")));
    assert_eq!(next_key(&keys, &key("c")), Some(key("a")));
    assert_eq!(next_key(&[key("a")], &key("a")), None);
    assert_eq!(next_key(&[], &key("a")), None);
}

#[test]
fn a_current_item_that_left_the_queue_leaves_the_first_one_next() {
    let keys = [key("b"), key("c")];
    assert_eq!(next_key(&keys, &key("a")), Some(key("b")));
}

#[test]
fn snooze_one_hour_is_exactly_one_hour() {
    assert_eq!(
        snooze_until(SnoozeChoice::Hour, at(1_000_000), 0),
        at(1_003_600)
    );
}

#[test]
fn snooze_later_is_six_in_the_evening_local_time() {
    // 2001-09-09 01:46:40 UTC is 09:46:40 at UTC+8; 18:00 local is 10:00 UTC.
    let now = at(1_000_000_000);
    assert_eq!(
        snooze_until(SnoozeChoice::Later, now, 8 * 3600),
        at(DAY + 10 * 3600)
    );
}

#[test]
fn snooze_later_within_an_hour_of_six_is_two_hours_from_now() {
    // 17:30 local at UTC+0.
    let now = at(DAY + 17 * 3600 + 1800);
    assert_eq!(
        snooze_until(SnoozeChoice::Later, now, 0),
        now + Duration::from_secs(7200)
    );
}

#[test]
fn snooze_tomorrow_is_nine_the_next_local_morning() {
    let now = at(DAY + 23 * 3600);
    assert_eq!(
        snooze_until(SnoozeChoice::Tomorrow, now, 0),
        at(DAY + 86_400 + 9 * 3600)
    );
    // At UTC+8 that instant is already 07:00 on the next local day, so tomorrow is the day after.
    assert_eq!(
        snooze_until(SnoozeChoice::Tomorrow, now, 8 * 3600),
        at(DAY + 2 * 86_400 + 9 * 3600 - 8 * 3600)
    );
}

#[test]
fn opening_pages_depend_on_the_mode() {
    let reviewed = cursor("r");
    assert_eq!(
        open_page(Mode::Incremental, Some(&reviewed)),
        ReviewPage::After {
            cursor: Some(reviewed.clone())
        }
    );
    assert_eq!(
        open_page(Mode::Incremental, None),
        ReviewPage::After { cursor: None }
    );
    assert_eq!(open_page(Mode::Full, Some(&reviewed)), ReviewPage::Latest);
}

#[test]
fn paging_continues_from_the_edges_of_the_loaded_page() {
    let (first, last) = (cursor("f"), cursor("l"));
    assert_eq!(
        turn_page(Paging::Earlier, &first, &last),
        ReviewPage::Before {
            cursor: first.clone()
        }
    );
    assert_eq!(
        turn_page(Paging::Later, &first, &last),
        ReviewPage::After { cursor: Some(last) }
    );
}

#[test]
fn a_surface_remembers_expanded_turns_until_the_mode_or_session_changes() {
    let mut surface = Surface::new(key("a"));
    assert!(!surface.is_expanded(&cursor("t1")));
    surface.toggle_expanded(cursor("t1"));
    assert!(surface.is_expanded(&cursor("t1")));
    surface.toggle_expanded(cursor("t1"));
    assert!(!surface.is_expanded(&cursor("t1")));

    surface.toggle_expanded(cursor("t2"));
    surface.set_mode(Mode::Full);
    assert_eq!(surface.mode, Mode::Full);
    assert!(!surface.is_expanded(&cursor("t2")));

    surface.toggle_expanded(cursor("t3"));
    surface.snooze_menu = true;
    surface.reopen(key("b"));
    assert_eq!(surface.key, key("b"));
    assert_eq!(surface.mode, Mode::Incremental);
    assert!(!surface.snooze_menu);
    assert!(!surface.is_expanded(&cursor("t3")));
}

#[test]
fn the_scope_follows_the_surface() {
    let mut surface = Surface::new(key("a"));
    assert_eq!(surface.scope(), Scope::Detail);
    surface.snooze_menu = true;
    assert_eq!(surface.scope(), Scope::SnoozeMenu);
}

#[test]
fn review_next_needs_the_document_of_the_open_session() {
    let surface = Surface::new(key("a"));
    let snapshot = cursor("last");
    assert_eq!(
        surface.review_target(&key("a"), &snapshot, false, false),
        Some(snapshot.clone())
    );
    assert_eq!(
        surface.review_target(&key("b"), &snapshot, false, false),
        None
    );
}

#[test]
fn review_next_is_blocked_until_the_last_page_and_while_the_position_is_stale() {
    let surface = Surface::new(key("a"));
    let snapshot = cursor("last");
    // More turns after the shown page: confirming would mark turns that were never shown.
    assert_eq!(
        surface.review_target(&key("a"), &snapshot, true, false),
        None
    );
    // A stale saved position cannot be advanced; it needs the recovery actions instead.
    assert_eq!(
        surface.review_target(&key("a"), &snapshot, false, true),
        None
    );
}

#[test]
fn review_next_saves_the_snapshot_captured_when_the_review_was_opened() {
    let mut surface = Surface::new(key("a"));
    surface.opened_through = Some(cursor("at-open"));
    // A page reloaded after a history refresh reports a newer last turn; that one stays unreviewed.
    assert_eq!(
        surface.review_target(&key("a"), &cursor("after-refresh"), false, false),
        Some(cursor("at-open"))
    );
    surface.reopen(key("b"));
    assert_eq!(surface.opened_through, None, "a new session starts clean");
}

#[test]
fn modified_keys_are_never_review_shortcuts() {
    let cmd = Mods::CMD;
    let alt = Mods {
        alt: true,
        ..Mods::NONE
    };
    let control = Mods {
        control: true,
        ..Mods::NONE
    };
    for mods in [cmd, alt, control] {
        for key in [
            "s", "z", "p", "f", "[", "]", "up", "down", "escape", "b", "a",
        ] {
            assert_eq!(key_action(Scope::Detail, key, mods), None, "{key} {mods:?}");
        }
        assert_eq!(key_action(Scope::List, "space", mods), None, "{mods:?}");
        assert_eq!(key_action(Scope::SnoozeMenu, "1", mods), None, "{mods:?}");
    }
    // Only cmd+enter reviews next; alt/control + enter do nothing.
    assert_eq!(
        key_action(Scope::Detail, "enter", cmd),
        Some(Action::ReviewNext)
    );
    assert_eq!(key_action(Scope::Detail, "enter", alt), None);
    assert_eq!(key_action(Scope::Detail, "enter", control), None);
}

/// With the Pinyin input method as the input source gpui reports the `]` key as "】" (and `[` as "【"), so
/// punctuation can never be the only way to page. The letters E / L page in every input source.
#[test]
fn paging_keys_survive_a_chinese_input_method() {
    for key in ["e", "[", "【"] {
        assert_eq!(
            key_action(Scope::Detail, key, Mods::NONE),
            Some(Action::Page(Paging::Earlier)),
            "{key}"
        );
    }
    for key in ["l", "]", "】"] {
        assert_eq!(
            key_action(Scope::Detail, key, Mods::NONE),
            Some(Action::Page(Paging::Later)),
            "{key}"
        );
    }
    // Typing "e" / "l" into the queue's search box stays typing, and modified keys never page.
    for key in ["e", "l", "【", "】"] {
        assert_eq!(key_action(Scope::List, key, Mods::NONE), None, "{key}");
        assert_eq!(key_action(Scope::Detail, key, Mods::CMD), None, "{key}");
        assert_eq!(
            key_action(Scope::SnoozeMenu, key, Mods::NONE),
            None,
            "{key}"
        );
    }
}

#[test]
fn stale_position_recovery_keys_only_exist_inside_a_review() {
    assert_eq!(
        key_action(Scope::Detail, "b", Mods::NONE),
        Some(Action::BaselineHere)
    );
    assert_eq!(
        key_action(Scope::Detail, "a", Mods::NONE),
        Some(Action::ReviewAllVisible)
    );
    assert_eq!(key_action(Scope::List, "b", Mods::NONE), None);
    assert_eq!(key_action(Scope::List, "a", Mods::NONE), None);
    assert_eq!(key_action(Scope::SnoozeMenu, "b", Mods::NONE), None);
}

#[test]
fn a_stale_position_opens_on_the_latest_page() {
    let reviewed = cursor("gone");
    assert_eq!(
        open_page_for(Mode::Incremental, Some(&reviewed), true),
        ReviewPage::Latest,
        "the saved position is not in the transcript, so After{{cursor}} would fail"
    );
    assert_eq!(
        open_page_for(Mode::Incremental, Some(&reviewed), false),
        open_page(Mode::Incremental, Some(&reviewed))
    );
    assert_eq!(
        open_page_for(Mode::Full, Some(&reviewed), false),
        ReviewPage::Latest
    );
}

#[test]
fn save_failures_say_what_actually_went_wrong() {
    use std::io::ErrorKind;
    let io = save_notice(ErrorKind::PermissionDenied, "标记为已 Review", "denied");
    assert_eq!(io, "保存失败，没有标记为已 Review：denied");
    let rewritten = save_notice(ErrorKind::InvalidInput, "标记为已 Review", "x");
    assert!(
        rewritten.contains("改写") && !rewritten.contains("保存失败"),
        "{rewritten}"
    );
    let stale = save_notice(ErrorKind::InvalidData, "标记为已 Review", "x");
    assert!(
        stale.contains("失效") && !stale.contains("保存失败"),
        "{stale}"
    );
}

#[test]
fn long_text_is_capped_in_one_piece() {
    let (short, omitted) = capped_text("a\nb\nc", 5);
    assert_eq!((short.as_str(), omitted), ("a\nb\nc", 0));
    let (capped, omitted) = capped_text("1\n2\n3\n4\n5", 3);
    assert_eq!((capped.as_str(), omitted), ("1\n2\n3", 2));
    let (empty, omitted) = capped_text("", 3);
    assert_eq!((empty.as_str(), omitted), ("", 0));
    // A blank line inside the text is kept, so paragraphs stay apart.
    assert_eq!(capped_text("a\n\nb", 9).0, "a\n\nb");
}

#[test]
fn review_and_archive_is_disabled_while_the_session_runs_in_gilvt() {
    assert!(can_review_and_archive(true, false));
    assert!(!can_review_and_archive(true, true));
    assert!(
        !can_review_and_archive(false, false),
        "needs what 'reviewed, next' needs"
    );
}

#[test]
fn shift_command_e_reviews_and_archives_in_the_detail_only() {
    let chord = Mods {
        shift: true,
        ..Mods::CMD
    };
    assert_eq!(
        key_action(Scope::Detail, "e", chord),
        Some(Action::ReviewNextAndArchive)
    );
    assert_eq!(key_action(Scope::List, "e", chord), None);
    assert_eq!(key_action(Scope::SnoozeMenu, "e", chord), None);
    // ⌘E alone and ⇧E alone are not the chord (⇧E still pages like E).
    assert_eq!(key_action(Scope::Detail, "e", Mods::CMD), None);
    assert_eq!(
        key_action(
            Scope::Detail,
            "e",
            Mods {
                shift: true,
                ..Mods::NONE
            }
        ),
        Some(Action::Page(Paging::Earlier))
    );
}
