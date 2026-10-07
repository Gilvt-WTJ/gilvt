//! The session's net diff behind the summary line (tasks spec §4.4): the newest repository's first
//! `before` to its last Done `after`, limited to the files some task touched.

use super::tasks::{range_label, range_of, Member};
use super::*;

pub(super) fn session(members: &[Member], i: &Inputs) -> Option<Net> {
    // The newest repository a record names; only its members count. (`fallback_root` plays no part: a
    // record without a `repo_root` has no snapshots.)
    let repo = members.iter().rev().find_map(|m| m.rec?.repo_root.clone())?;
    let mine: Vec<Member> = members.iter().copied().filter(|m| repo_of(m) == Some(repo.as_str())).collect();
    let others = members.iter().any(|m| repo_of(m).is_some_and(|r| r != repo));
    let (bi, ai, repo_root, before, after) = range_of(&mine, None)?;
    let (from, to) = (mine[bi].number(), mine[ai].number());
    let range = CardRange {
        repo_root: repo_root.clone(),
        before,
        after,
        label: range_label(crate::i18n::text("本会话", "This session"), from, to, false),
        scope: crate::i18n::text("本会话", "This session"),
    };
    let side = |t: Option<SystemTime>, n: Option<u32>, end: &str| {
        let turn = n.map_or_else(
            || end.to_string(),
            |n| {
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("Turn {n} {end}")
                } else {
                    format!("第 {n} 轮{end}")
                }
            },
        );
        t.map_or(turn.clone(), |t| format!("{} {turn}", (i.clock)(t)))
    };
    let range_label = format!(
        "{} → {}",
        side(mine[bi].started(), from, crate::i18n::text("前", "before")),
        side(mine[ai].ended(), to, crate::i18n::text("后", "after"))
    );
    // The paths some turn named (its recorded changes, or the running turn's live list), old paths included.
    let named = mine.iter().filter_map(|m| m.rec?.changes.as_deref()).flatten().chain(i.live.into_iter().flatten());
    let touched: HashSet<&str> = named.flat_map(|c| std::iter::once(c.path.as_str()).chain(c.old_path.as_deref())).collect();
    let (files, excluded, computing, failed) = match (i.diff)(&range) {
        None => (Vec::new(), 0, true, None),
        Some(Err(why)) => (Vec::new(), 0, false, Some(why)),
        Some(Ok(all)) => {
            let (keep, drop): (Vec<&FileChange>, Vec<&FileChange>) =
                all.iter().partition(|c| touched.contains(c.path.as_str()) || c.old_path.as_deref().is_some_and(|o| touched.contains(o)));
            (keep.into_iter().map(|c| file_row(c, Some(&repo_root))).collect(), drop.len(), false, None)
        }
    };
    let only_repo = others.then(|| Path::new(&repo).file_name().map_or(repo.clone(), |n| n.to_string_lossy().into_owned()));
    Some(Net { range, range_label, files, excluded, only_repo, computing, failed })
}

fn repo_of<'a>(m: &Member<'a>) -> Option<&'a str> {
    m.rec.and_then(|r| r.repo_root.as_deref())
}

/// The net's files and line counts once it is in (even none: the row and its 「未计入」 note still show);
/// otherwise — no net, still computing, or failed — the Done cards' distinct files without counts.
pub(super) fn summary(cards: &[ArtCard], net: Option<&Net>) -> Option<Summary> {
    if let Some(n) = net.filter(|n| !n.computing && n.failed.is_none()) {
        return Some(Summary {
            files: n.files.len(),
            added: Some(n.files.iter().map(|f| f.added).sum()),
            removed: Some(n.files.iter().map(|f| f.removed).sum()),
            computing: false,
        });
    }
    let files: HashSet<&str> = cards.iter().filter(|c| c.state == CardState::Done).flat_map(|c| c.files.iter().map(|f| f.path.as_str())).collect();
    (!files.is_empty()).then_some(Summary { files: files.len(), added: None, removed: None, computing: net.is_some_and(|n| n.computing) })
}
