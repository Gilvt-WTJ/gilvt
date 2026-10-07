//! Every built-in theme's derived chrome meets the readability rules (spec §5); which colors fell back
//! is recorded in `fallbacks.snap` so a coefficient change shows its reach in the diff.
//! Regenerate the snapshot with `UPDATE_SNAPSHOT=1 cargo test -p gilvt-theme --test all_themes`.

use gilvt_theme::color::{contrast, delta_e, is_dark_background};
use gilvt_theme::ui::{derive, DISTINCT};
use gilvt_theme::{builtin, parse::parse};

const EPS: f32 = 1e-3;

#[test]
fn every_built_in_theme_meets_the_rules() {
    let mut snapshot = String::new();
    let mut failures = Vec::new();
    for (name, text) in builtin::all() {
        let p = parse(text).unwrap().to_palette();
        let dark = is_dark_background(p.background);
        let (u, notes) = derive(&p, dark);
        let has = |note: &str| notes.iter().any(|n| n == note);
        let unfixable = |token: &str| has(&format!("{token}:unfixable"));
        let statuses = [("attention", u.attention), ("error", u.error), ("running", u.running), ("done", u.done)];
        for (sname, s) in statuses {
            if !unfixable(&format!("{sname}.fg")) && (contrast(s.fg, u.panel) < 4.5 - EPS || contrast(s.fg, p.background) < 3.0 - EPS) {
                failures.push(format!("{name}: {sname}.fg"));
            }
            if !unfixable(&format!("{sname}.ring")) && contrast(s.ring, p.background) < 3.0 - EPS {
                failures.push(format!("{name}: {sname}.ring"));
            }
        }
        for i in 0..4 {
            for j in i + 1..4 {
                // Statuses are listed in priority order; the recorded, accepted outcome of a clash that
                // could not be resolved is `<lower-priority status>:clash-unresolved`.
                if has(&format!("{}:clash-unresolved", statuses[j].0)) {
                    continue;
                }
                let (a, b) = (statuses[i].1, statuses[j].1);
                if delta_e(a.fg, b.fg) < DISTINCT - EPS || delta_e(a.ring, b.ring) < DISTINCT - EPS {
                    failures.push(format!("{name}: {} vs {}", statuses[i].0, statuses[j].0));
                }
            }
        }
        for (token, c) in [("accent", u.accent), ("purple", u.purple)] {
            if !unfixable(token) && contrast(c, u.panel) < 4.5 - EPS {
                failures.push(format!("{name}: {token}"));
            }
        }
        if !notes.is_empty() {
            snapshot.push_str(&format!("{name}: {}\n", notes.join(" ")));
        }
    }
    assert!(failures.is_empty(), "{} violations:\n{}", failures.len(), failures.join("\n"));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fallbacks.snap");
    if std::env::var_os("UPDATE_SNAPSHOT").is_some() {
        std::fs::write(&path, &snapshot).unwrap();
    }
    let stored = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(stored, snapshot, "fallbacks changed; review and regenerate with UPDATE_SNAPSHOT=1");
}
