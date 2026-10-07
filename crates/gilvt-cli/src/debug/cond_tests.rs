use serde_json::{json, Value};

use super::*;

fn state() -> Value {
    json!({
        "version": 1,
        "front": false,
        "dock_badge": null,
        "ratio": 1.5,
        "windows": [{
            "key": true,
            "title": "bash — user",
            "tags": ["a", "b", 3],
            "sidebar": {"rows": [
                {"name": "新会话", "status": "awaiting_answer", "pane": 3, "muted": false},
                {"name": "build", "status": "idle", "pane": 4, "muted": true},
                {"name": "say \"hi\"", "status": "idle", "pane": 5, "muted": false},
            ]},
        }, {
            "key": false,
            "title": "second",
            "sidebar": {"rows": []},
        }],
    })
}

fn holds(src: &str) -> bool {
    parse(src).unwrap_or_else(|e| panic!("{src}: {e}")).eval(&state())
}

#[test]
fn keys_and_indexes() {
    assert!(holds("version == 1"));
    assert!(!holds("version == 2"));
    assert!(holds(r#"windows[0].title == "bash — user""#));
    assert!(holds(r#"windows[1].title == "second""#));
    assert!(holds("windows[0].sidebar.rows[1].pane == 4"));
    assert!(!holds("windows[5].key == true"), "out of range: no candidate");
    assert!(!holds("nope == 1"), "missing key: no candidate");
    assert!(!holds("version.x == 1"), "key of a number: no candidate");
    assert!(!holds("windows.key == true"), "key of an array: no candidate");
    assert!(!holds("front[0] == false"), "index of a bool: no candidate");
}

#[test]
fn fan_out_is_any_element() {
    assert!(holds("windows[*].key == false"));
    assert!(holds("windows[*].key == true"));
    assert!(holds(r#"windows[*].sidebar.rows[*].status == "idle""#));
    assert!(!holds(r#"windows[*].sidebar.rows[*].status == "thinking""#));
    assert!(!holds("front[*] == false"), "fan-out of a non-array: nothing");
    assert!(!holds("windows[0].sidebar[*] exists"), "objects are not fanned out");
}

#[test]
fn filters_select_elements_by_a_field() {
    assert!(holds(r#"windows[0].sidebar.rows[?name=="新会话"].status == "awaiting_answer""#));
    assert!(holds(r#"windows[*].sidebar.rows[?pane==4].muted == true"#));
    assert!(holds(r#"windows[?key==true].title == "bash — user""#));
    assert!(holds(r#"windows[?key == false].title == "second""#), "spaces around ==");
    assert!(holds(r#"windows[0].sidebar.rows[?name=="say \"hi\""].pane == 5"#), "escapes in the filter literal");
    assert!(!holds(r#"windows[0].sidebar.rows[?name=="nobody"] exists"#));
    assert!(!holds(r#"windows[0].sidebar.rows[?missing==1] exists"#));
    assert!(!holds(r#"front[?a==1] exists"#));
}

#[test]
fn contains_filters_match_substrings() {
    assert!(holds(r#"windows[0].sidebar.rows[?name contains "hi"].pane == 5"#));
    assert!(holds(r#"windows[?title contains "user"].key == true"#));
    assert!(holds(r#"windows[?tags contains 3].title == "bash — user""#), "an element of an array");
    assert!(holds(r#"windows[0].sidebar.rows[?status contains "idle"] exists count=2"#));
    assert!(!holds(r#"windows[0].sidebar.rows[?name contains "zzz"] exists"#));
    assert!(!holds(r#"windows[0].sidebar.rows[?pane contains 3] exists"#), "a number contains nothing");
}

#[test]
fn equality_and_inequality() {
    assert!(holds("front == false"));
    assert!(holds("front != true"));
    assert!(holds("dock_badge == null"));
    assert!(!holds("dock_badge != null"));
    assert!(holds("ratio == 1.5"));
    assert!(holds("version == 1.0"), "numbers compare by value");
    assert!(holds(r#"windows[1].sidebar.rows == []"#));
    assert!(holds(r#"windows[0].tags == ["a", "b", 3.0]"#));
    // `!=` needs a candidate, and every candidate must differ.
    assert!(holds(r#"windows[*].sidebar.rows[*].status != "thinking""#));
    assert!(!holds(r#"windows[*].sidebar.rows[*].status != "idle""#), "two rows are idle");
    assert!(!holds("nope != 1"), "no candidate: != does not hold");
    assert!(!holds(r#"windows[1].sidebar.rows[*].status != "x""#), "empty fan-out");
}

#[test]
fn contains_substrings_and_elements() {
    assert!(holds(r#"windows[0].title contains "user""#));
    assert!(holds(r#"windows[0].title contains "—""#));
    assert!(!holds(r#"windows[0].title contains "zsh""#));
    assert!(holds(r#"windows[0].tags contains "b""#));
    assert!(holds("windows[0].tags contains 3"));
    assert!(!holds(r#"windows[0].tags contains "c""#));
    assert!(!holds("version contains 1"), "a number contains nothing");
    assert!(!holds(r#"windows[0].title contains 3"#), "a string contains only strings");
    assert!(holds(r#"windows[*].title contains "sec""#));
}

#[test]
fn counts() {
    assert!(holds(r#"windows[*].sidebar.rows[*].status == "idle" count=2"#));
    assert!(!holds(r#"windows[*].sidebar.rows[*].status == "idle" count=1"#));
    assert!(holds(r#"windows[*].sidebar.rows[*].status == "thinking" count=0"#));
    assert!(holds(r#"windows[*].sidebar.rows[*].status != "idle" count=1"#));
    assert!(holds(r#"windows[*].title contains "e" count=2"#));
    assert!(holds("windows[*] exists count=2"));
    assert!(holds("windows[0].sidebar.rows[*] exists count=3"));
    assert!(holds("nope exists count=0"));
    assert!(holds("nope == 1 count=0"));
}

#[test]
fn exists_and_not_exists() {
    assert!(holds("windows exists"));
    assert!(holds("dock_badge exists"), "null exists");
    assert!(!holds("dock_badge !exists"));
    assert!(holds("nope !exists"));
    assert!(holds("windows[2] !exists"));
    assert!(holds(r#"windows[0].sidebar.rows[?name=="新会话"] exists"#));
}

#[test]
fn clauses_all_hold() {
    assert!(holds(r#"version == 1 && front == false"#));
    assert!(!holds(r#"version == 1 && front == true"#));
    assert!(holds(r#"windows[0].title != " && " && version == 1"#), "&& inside a string does not split");
    assert!(holds(r#"windows[*] exists count=2 && dock_badge == null && nope !exists"#));
}

#[test]
fn unicode_keys_and_values() {
    let v = json!({"名字": {"状态": "等待 && 回答"}, "列表": [{"键": "值"}]});
    assert!(parse(r#"名字.状态 == "等待 && 回答""#).unwrap().eval(&v));
    assert!(parse(r#"名字.状态 contains "等待""#).unwrap().eval(&v));
    assert!(parse(r#"列表[?键=="值"] exists"#).unwrap().eval(&v));
    assert!(parse(r#"名字.状态 == "\u7b49\u5f85 && \u56de\u7b54""#).unwrap().eval(&v), "\\u escapes");
}

#[test]
fn a_path_may_start_with_a_segment() {
    let v = json!([{"a": 1}, {"a": 2}]);
    assert!(parse("[1].a == 2").unwrap().eval(&v));
    assert!(parse("[*].a == 1 count=1").unwrap().eval(&v));
    assert!(parse("[?a==2] exists").unwrap().eval(&v));
}

fn error(src: &str) -> ParseError {
    match parse(src) {
        Ok(c) => panic!("{src} parsed: {c:?}"),
        Err(e) => e,
    }
}

#[test]
fn parse_errors_point_at_the_problem() {
    let cases: &[(&str, usize, &str)] = &[
        ("", 0, "expected a path"),
        ("   ", 3, "expected a path"),
        ("version", 7, "expected an operator"),
        ("version ~= 1", 8, "expected an operator"),
        ("version ==", 10, "expected a JSON value"),
        ("version == nope", 11, "expected a JSON value"),
        (r#"version == "open"#, 11, "expected a JSON value"),
        ("version == 1 extra", 13, "expected && or count=N"),
        ("version == 1count=1", 11, "expected a JSON value"),
        ("version == 1 count=", 13, "count=N needs a whole number"),
        ("version == 1 count=-1", 13, "count=N needs a whole number"),
        ("version == 1 &&", 15, "expected a path"),
        ("version == 1 && && front == false", 16, "expected a path"),
        ("version == 1 &", 13, "expected && or count=N"),
        ("windows[", 8, "expected an index, * or ?field==value"),
        ("windows[x]", 8, "expected an index, * or ?field==value"),
        ("windows[0", 9, "expected ]"),
        ("windows[*x]", 9, "expected ]"),
        ("windows[?]", 9, "expected a field name"),
        ("windows[?a]", 10, "expected == or contains"),
        ("windows[?a!=1]", 10, "expected == or contains"),
        ("windows[?a==]", 12, "expected a JSON value"),
        ("windows[?a==1", 13, "expected ]"),
        ("windows..key == 1", 8, "expected a key"),
        ("windows. == 1", 8, "expected a key"),
        ("windows] == 1", 7, "expected an operator"),
        ("nope !exists count=1", 13, "count=N cannot follow !exists"),
        ("a == 1 && b ~ 2", 12, "expected an operator"),
        (r#"名字 ~ 1"#, 3, "expected an operator"),
    ];
    for &(src, col, msg) in cases {
        let e = error(src);
        assert_eq!((e.column, e.message.contains(msg)), (col, true), "{src:?}: {e:?}");
    }
}

#[test]
fn a_parse_error_shows_a_caret() {
    let e = error("version ~= 1");
    assert_eq!(e.to_string(), "expected an operator (==, !=, contains, exists, !exists)\n  version ~= 1\n          ^");
}

#[test]
fn operators_need_a_word_boundary() {
    assert_eq!(error("windows existsx").column, 8);
    assert_eq!(error("title containsx 1").column, 6);
    // `==` needs no space.
    assert!(holds("version==1"));
}

#[test]
fn blanks_before_a_closing_bracket() {
    assert!(holds("windows[0 ].key == true"));
    assert!(holds("windows[* ].key exists count=2"));
    assert!(holds(r#"windows[0].sidebar.rows[?pane==4 ].name == "build""#));
}

#[test]
fn equality_is_strict_but_numbers_compare_by_value() {
    assert!(!holds("windows[0].key == 1"), "a bool never equals a number");
    assert!(!holds("windows[0].sidebar.rows[?muted==0] exists"));
    assert!(holds("windows[0].sidebar.rows[?muted==false] exists count=2"));
    assert!(holds("ratio == 1.5"));
    assert!(!holds("windows[0].tags contains true"));
    assert!(holds(r#"windows[0].tags == ["a", "b", 3]"#));
    assert!(!holds(r#"windows[0].tags == ["a", "b", true]"#));
}

#[test]
fn a_path_alone() {
    let p = parse_path(" windows[*].title ").unwrap();
    let st = state();
    assert_eq!(p.candidates(&st), [&json!("bash — user"), &json!("second")]);
    for bad in [".windows", "windows..key", "windows.", "windows[0]key", "windows[0] key", "", "a b"] {
        assert!(parse_path(bad).is_err(), "{bad:?}");
    }
}
