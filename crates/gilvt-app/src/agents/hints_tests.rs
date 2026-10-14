use super::*;
use crate::agents::foreground::Via;
use gilvt_agent::AgentKind::{Claude, Codex};

fn l(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_child_of_a_wrapper_was_bypassed() {
    let via = Via::Descendant { parent: "ttadk".into() };
    assert_eq!(cause(&via, Claude, Some("ttadk claude"), &l(&["claude"]), &l(&["codex"]), false), Some(Cause::Bypassed { parent: "ttadk".into() }));
}

#[test]
fn a_leader_started_by_an_unlisted_word_is_unlisted() {
    assert_eq!(
        cause(&Via::Leader, Codex, Some("cc exec hi"), &l(&["claude"]), &l(&["codex"]), false),
        Some(Cause::Unlisted { word: "cc".into(), kind: Codex })
    );
    assert_eq!(cause(&Via::Leader, Claude, Some("claude hi"), &l(&["claude"]), &l(&["codex"]), false), None, "listed: something else went wrong");
}

#[test]
fn prefixes_and_assignments_are_skipped_or_undecidable() {
    assert_eq!(first_word("FOO=1 BAR='a b' cc x"), Some("cc".into()));
    assert_eq!(first_word("GILVT_AGENT_HOOKED=1 claude @scenario:x"), Some("claude".into()));
    for c in ["env cc", "time cc", "nice cc", "nohup cc", "command cc", "exec cc", "sudo cc", "   "] {
        assert_eq!(first_word(c), None, "{c}");
    }
}

#[test]
fn no_hint_when_wrappers_are_off_or_unknown() {
    let via = Via::Descendant { parent: "node".into() };
    assert_eq!(cause(&via, Claude, None, &l(&["claude"]), &[], true), None, "GILVT_NO_AGENT_WRAPPERS");
    assert_eq!(cause(&Via::Leader, Claude, None, &l(&["claude"]), &[], false), None, "no command line");
}

#[test]
fn ttadk_hint_copies_its_config_line() {
    let shims = Some(Path::new("/L/gilvt/shims"));
    let h = hint(&Cause::Bypassed { parent: "ttadk".into() }, Claude, shims, &l(&["claude", "cc"]));
    assert_eq!(h.copy.as_deref(), Some(r#""claude": "/L/gilvt/shims/claude""#));
    assert!(h.text.contains("ttadk"));
    let g = hint(&Cause::Bypassed { parent: "perl".into() }, Codex, shims, &l(&["codex"]));
    assert_eq!(g.copy.as_deref(), Some("/L/gilvt/shims/codex"));
    let u = hint(&Cause::Unlisted { word: "cc".into(), kind: Claude }, Claude, None, &l(&["claude"]));
    assert_eq!(u.copy, None);
}

#[test]
fn the_copied_shim_is_one_that_is_installed() {
    let shims = Some(Path::new("/L/gilvt/shims"));
    // Only names in the list have a shim: `claude_commands = ["cc"]` leaves no `<shims>/claude`.
    let h = hint(&Cause::Bypassed { parent: "ttadk".into() }, Claude, shims, &l(&["cc"]));
    assert_eq!(h.copy.as_deref(), Some(r#""claude": "/L/gilvt/shims/cc""#));
    assert_eq!(hint(&Cause::Bypassed { parent: "perl".into() }, Codex, shims, &l(&["cw"])).copy.as_deref(), Some("/L/gilvt/shims/cw"));
    assert_eq!(hint(&Cause::Bypassed { parent: "ttadk".into() }, Claude, shims, &[]).copy, None, "no names: no shim to point at");
    assert_eq!(hint(&Cause::Bypassed { parent: "ttadk".into() }, Claude, None, &l(&["claude"])).copy, None, "no shims directory");
    assert_eq!(ttadk_line(Codex, Path::new("/L/s/cw")), r#""codex": "/L/s/cw""#);
}

#[test]
fn dismissed_hints_persist() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Dismissed::load(d.path());
    let c = Cause::Unlisted { word: "cc".into(), kind: Claude };
    assert!(!s.contains(&c));
    s.insert(&c);
    s.save(d.path()).unwrap();
    assert!(Dismissed::load(d.path()).contains(&c));
    let mut s = Dismissed::load(d.path());
    s.clear();
    s.save(d.path()).unwrap();
    assert!(!Dismissed::load(d.path()).contains(&c));
}

#[test]
fn a_path_or_unusable_name_gets_no_hint() {
    for c in ["./cc hi", "~/bin/cc", "/usr/local/bin/cc x"] {
        assert_eq!(cause(&Via::Leader, Claude, Some(c), &l(&["claude"]), &l(&["codex"]), false), None, "{c}: 加入 could not help");
    }
    assert_eq!(cause(&Via::Leader, Claude, Some("cc hi"), &l(&["claude"]), &l(&["codex"]), false), Some(Cause::Unlisted { word: "cc".into(), kind: Claude }));
}
