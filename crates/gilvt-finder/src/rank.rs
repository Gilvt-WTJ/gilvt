//! Fuzzy ranking of a listing: nucleo's fzf-style path score plus bonuses for matches in the file
//! name, changed files, files under the pane's cwd and recently opened files.

use std::cmp::Ordering;
use std::collections::HashSet;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::Listing;

// Scale: nucleo scores 16 per matched char, adds 8-18 at word and path boundaries and for runs, and
// subtracts 3 + 1/char per gap; a clean 4-char match starting at a boundary scores about 110.
// The name bonuses decide between equally clean matches in the file name and in the directory. The
// context bonuses add up to at most CHANGED + UNDER_CWD + RECENT = 36: enough to reorder matches of
// the same shape, not to beat one that is cleaner by a boundary, a run or a gap.

/// The whole query matches within the file name.
const NAME_MATCH: u32 = 16;
/// ... and that match includes the file name's first char.
const NAME_START: u32 = 16;
/// The file has uncommitted changes.
const CHANGED: u32 = 12;
/// The file is under the pane's cwd (when that is below the root).
const UNDER_CWD: u32 = 8;
/// The most recently opened file; each older one gets 1 less, down to RECENT_MIN.
const RECENT: u32 = 16;
const RECENT_MIN: u32 = 8;

/// Ranking context.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context<'a> {
    /// Pane cwd relative to the root ('/'-separated, "" = root); files under it get a bonus.
    pub cwd_rel: &'a str,
    /// Recently opened (relative paths), most recent first; earlier = bigger bonus.
    pub recent: &'a [String],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Relative to the root.
    pub path: String,
    pub score: u32,
    /// Char indices within the file name (last path component) to highlight.
    pub name_matches: Vec<usize>,
    /// Char indices within the directory part (everything before the file name, trailing '/' included).
    pub dir_matches: Vec<usize>,
    pub changed: bool,
}

/// Reusable matcher (nucleo_matcher::Matcher is not cheap to create).
pub struct Finder {
    matcher: Matcher,
    chars: Vec<char>,
    indices: Vec<u32>,
}

impl Default for Finder {
    fn default() -> Finder {
        Finder::new()
    }
}

struct Ranked {
    score: u32,
    len: usize,
    index: usize,
    in_name: bool,
}

impl Finder {
    pub fn new() -> Finder {
        Finder { matcher: Matcher::new(Config::DEFAULT.match_paths()), chars: Vec::new(), indices: Vec::new() }
    }

    /// Top `limit` hits for `query` (fzf syntax: space-separated terms, `^` `$` `'` `!`; smart case),
    /// best first; equal scores go to the shorter path, then alphabetical order. Empty query: changed
    /// files, then recent, then the rest by path, all with score 0.
    pub fn search(&mut self, listing: &Listing, query: &str, cx: &Context, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        if pattern.atoms.is_empty() {
            return unfiltered(listing, cx, limit);
        }
        let bonus = Bonus::new(listing, cx);
        let mut ranked = Vec::new();
        for (index, path) in listing.files.iter().enumerate() {
            let hay = utf32(path, &mut self.chars);
            let Some(score) = pattern.score(hay, &mut self.matcher) else { continue };
            let name = hay.slice(name_start(hay)..);
            self.indices.clear();
            let in_name = pattern.indices(name, &mut self.matcher, &mut self.indices).is_some();
            let name_bonus = match (in_name, self.indices.contains(&0)) {
                (false, _) => 0,
                (true, false) => NAME_MATCH,
                (true, true) => NAME_MATCH + NAME_START,
            };
            ranked.push(Ranked { score: score + name_bonus + bonus.of(index, path), len: hay.len(), index, in_name });
        }
        let order = |a: &Ranked, b: &Ranked| -> Ordering {
            b.score
                .cmp(&a.score)
                .then(a.len.cmp(&b.len))
                .then_with(|| listing.files[a.index].cmp(&listing.files[b.index]))
        };
        if ranked.len() > limit {
            ranked.select_nth_unstable_by(limit - 1, order);
            ranked.truncate(limit);
        }
        ranked.sort_unstable_by(order);
        ranked.iter().map(|r| self.hit(listing, &pattern, r)).collect()
    }

    /// Highlights where the ranking found the match: in the file name when the query matches there.
    fn hit(&mut self, listing: &Listing, pattern: &Pattern, r: &Ranked) -> Hit {
        let path = &listing.files[r.index];
        let hay = utf32(path, &mut self.chars);
        let start = name_start(hay);
        self.indices.clear();
        let searched = if r.in_name { hay.slice(start..) } else { hay };
        pattern.indices(searched, &mut self.matcher, &mut self.indices);
        let offset = if r.in_name { start } else { 0 };
        let mut matched: Vec<usize> = self.indices.iter().map(|&i| i as usize + offset).collect();
        matched.sort_unstable();
        matched.dedup();
        let split = matched.partition_point(|&i| i < start);
        Hit {
            path: path.clone(),
            score: r.score,
            name_matches: matched[split..].iter().map(|i| i - start).collect(),
            dir_matches: matched[..split].to_vec(),
            changed: listing.changed.contains(path),
        }
    }
}

/// Context bonuses.
struct Bonus {
    /// Changed and recent bonuses by file index; empty when there are none.
    by_index: Vec<u32>,
    /// "<cwd_rel>/", when the cwd is below the root.
    cwd_prefix: Option<String>,
}

impl Bonus {
    /// Looks the few changed and recent files up once, so ranking does not hash every path.
    fn new(listing: &Listing, cx: &Context) -> Bonus {
        let mut by_index = Vec::new();
        let mut add = |path: &str, bonus: u32| {
            if let Ok(i) = listing.files.binary_search_by(|f| f.as_str().cmp(path)) {
                by_index.resize(listing.files.len(), 0);
                by_index[i] += bonus;
            }
        };
        for path in &listing.changed {
            add(path, CHANGED);
        }
        let mut seen = HashSet::new();
        for (age, path) in cx.recent.iter().enumerate().filter(|(_, p)| seen.insert(p.as_str())) {
            add(path, RECENT.saturating_sub(age as u32).max(RECENT_MIN));
        }
        let cwd = cx.cwd_rel.trim_matches('/');
        Bonus { by_index, cwd_prefix: (!cwd.is_empty()).then(|| format!("{cwd}/")) }
    }

    fn of(&self, index: usize, path: &str) -> u32 {
        let under_cwd = match &self.cwd_prefix {
            Some(prefix) if path.starts_with(prefix.as_str()) => UNDER_CWD,
            _ => 0,
        };
        self.by_index.get(index).copied().unwrap_or(0) + under_cwd
    }
}

/// Empty-query order: changed files by path, then recent ones by recency, then the rest by path.
fn unfiltered(listing: &Listing, cx: &Context, limit: usize) -> Vec<Hit> {
    let mut changed: Vec<&String> = listing.changed.iter().collect();
    changed.sort_unstable();
    let recent = cx.recent.iter().filter(|p| listing.files.binary_search(p).is_ok());
    let mut seen = HashSet::new();
    changed
        .into_iter()
        .chain(recent)
        .chain(&listing.files)
        .filter(|p| seen.insert(p.as_str()))
        .take(limit)
        .map(|p| Hit {
            path: p.clone(),
            score: 0,
            name_matches: Vec::new(),
            dir_matches: Vec::new(),
            changed: listing.changed.contains(p),
        })
        .collect()
}

/// Haystack indexed by char (nucleo's `Utf32Str::new` indexes by grapheme), so match indices are char
/// indices.
fn utf32<'a>(s: &'a str, chars: &'a mut Vec<char>) -> Utf32Str<'a> {
    if s.is_ascii() {
        return Utf32Str::Ascii(s.as_bytes());
    }
    chars.clear();
    chars.extend(s.chars());
    Utf32Str::Unicode(chars)
}

/// Char index where the file name starts.
fn name_start(hay: Utf32Str) -> usize {
    let slash = match hay {
        Utf32Str::Ascii(bytes) => bytes.iter().rposition(|&b| b == b'/'),
        Utf32Str::Unicode(chars) => chars.iter().rposition(|&c| c == '/'),
    };
    slash.map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn listing(files: &[&str], changed: &[&str]) -> Listing {
        let mut files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
        files.sort();
        Listing { files, changed: changed.iter().map(|c| c.to_string()).collect(), ..Listing::default() }
    }

    fn paths(listing: &Listing, query: &str, cx: &Context) -> Vec<String> {
        Finder::new().search(listing, query, cx, 50).into_iter().map(|h| h.path).collect()
    }

    #[test]
    fn file_name_start_beats_directory_middle() {
        let l = listing(&["src/domain/x.rs", "src/main.rs"], &[]);
        assert_eq!(paths(&l, "main", &Context::default()), ["src/main.rs", "src/domain/x.rs"]);
        // Both at a path boundary: the file name wins.
        let l = listing(&["rank/mod.rs", "src/rank.rs"], &[]);
        assert_eq!(paths(&l, "rank", &Context::default()), ["src/rank.rs", "rank/mod.rs"]);
        // Name start beats a match inside the name.
        let l = listing(&["a/frank.rs", "a/rank.rs"], &[]);
        assert_eq!(paths(&l, "rank", &Context::default()), ["a/rank.rs", "a/frank.rs"]);
    }

    #[test]
    fn ties_go_to_the_shorter_path_then_alphabetical() {
        let l = listing(&["zz/util.rs", "util.rs", "aa/util.rs"], &[]);
        assert_eq!(paths(&l, "util", &Context::default()), ["util.rs", "aa/util.rs", "zz/util.rs"]);
    }

    #[test]
    fn each_bonus_reorders_equal_matches() {
        let files = ["a/util.rs", "b/util.rs", "c/util.rs"];
        let none = Context::default();
        assert_eq!(paths(&listing(&files, &[]), "util", &none), files);
        assert_eq!(paths(&listing(&files, &["c/util.rs"]), "util", &none)[0], "c/util.rs");
        assert_eq!(paths(&listing(&files, &[]), "util", &Context { cwd_rel: "c", recent: &[] })[0], "c/util.rs");
        // "c" is no prefix of "cc/…", and a trailing slash is tolerated.
        let l = listing(&["a/util.rs", "cc/util.rs", "c/sub/util.rs"], &[]);
        assert_eq!(paths(&l, "util", &Context { cwd_rel: "c/", recent: &[] })[0], "c/sub/util.rs");
        let recent = ["c/util.rs".to_string(), "b/util.rs".to_string()];
        let cx = Context { cwd_rel: "", recent: &recent };
        assert_eq!(paths(&listing(&files, &[]), "util", &cx), ["c/util.rs", "b/util.rs", "a/util.rs"]);
    }

    #[test]
    fn bonuses_do_not_beat_a_clearly_better_match() {
        let l = listing(&["docs/finder.md", "fine/header.rs"], &["fine/header.rs"]);
        let recent = ["fine/header.rs".to_string()];
        let cx = Context { cwd_rel: "fine", recent: &recent };
        assert_eq!(paths(&l, "finder", &cx), ["docs/finder.md", "fine/header.rs"]);
    }

    #[test]
    fn empty_query_order() {
        let l = listing(&["a", "b", "c", "d", "e", "f"], &["e", "d"]);
        let recent = ["c".to_string(), "missing".to_string(), "d".to_string(), "a".to_string()];
        let cx = Context { cwd_rel: "", recent: &recent };
        let hits = Finder::new().search(&l, "  ", &cx, 50);
        let order: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(order, ["d", "e", "c", "a", "b", "f"]);
        assert!(hits[0].changed && hits[1].changed && !hits[2].changed);
        assert_eq!(Finder::new().search(&l, "", &cx, 3).len(), 3);
    }

    #[test]
    fn highlights_name_and_directory() {
        let l = listing(&["src/main.rs", "文档/说明 书.md"], &["src/main.rs"]);
        let mut finder = Finder::new();
        let cx = Context::default();

        let hit = &finder.search(&l, "mr", &cx, 50)[0];
        assert_eq!((hit.path.as_str(), hit.changed), ("src/main.rs", true));
        assert_eq!((hit.name_matches.as_slice(), hit.dir_matches.as_slice()), (&[0, 5][..], &[][..]));

        let hit = &finder.search(&l, "srcmain", &cx, 50)[0];
        assert_eq!((hit.name_matches.as_slice(), hit.dir_matches.as_slice()), (&[0, 1, 2, 3][..], &[0, 1, 2][..]));

        let hit = &finder.search(&l, "说书", &cx, 50)[0];
        assert_eq!((hit.path.as_str(), hit.changed), ("文档/说明 书.md", false));
        assert_eq!((hit.name_matches.as_slice(), hit.dir_matches.as_slice()), (&[0, 3][..], &[][..]));

        let hit = &finder.search(&l, "文档说", &cx, 50)[0];
        assert_eq!((hit.name_matches.as_slice(), hit.dir_matches.as_slice()), (&[0][..], &[0, 1][..]));
    }

    #[test]
    fn smart_case_terms_and_limit() {
        let l = listing(&["src/main.rs", "src/Main.java", "tests/main.rs"], &[]);
        assert_eq!(paths(&l, "Main", &Context::default()), ["src/Main.java"]);
        assert_eq!(paths(&l, "tests main", &Context::default()), ["tests/main.rs"]);
        assert_eq!(paths(&l, "main !java", &Context::default()), ["src/main.rs", "tests/main.rs"]);
        assert_eq!(paths(&l, "zzz", &Context::default()), Vec::<String>::new());
        assert_eq!(Finder::new().search(&l, "main", &Context::default(), 2).len(), 2);
        assert!(Finder::new().search(&l, "main", &Context::default(), 0).is_empty());
    }

    #[test]
    fn searches_100k_paths_quickly() {
        let mut files = Vec::with_capacity(100_000);
        for i in 0..100_000 {
            files.push(format!(
                "crates/c{}/src/m{}/f{}_{}.rs",
                i % 50,
                i % 997,
                i,
                ["util", "view", "model", "test"][i % 4]
            ));
        }
        files.sort();
        let changed = files.iter().step_by(1000).cloned().collect();
        let l = Listing { files, changed, ..Listing::default() };
        let recent: Vec<String> = l.files.iter().take(20).cloned().collect();
        let cx = Context { cwd_rel: "crates/c7", recent: &recent };
        let mut finder = Finder::new();
        for query in ["u", "view", "c7m12f", "srcmodel"] {
            // Best of three: one run slowed by a loaded machine (parallel test binaries) is noise, but a
            // real regression makes every run slow.
            let mut best = None;
            for _ in 0..3 {
                let start = Instant::now();
                let hits = finder.search(&l, query, &cx, 50);
                let elapsed = start.elapsed();
                assert_eq!(hits.len(), 50);
                best = Some(best.map_or(elapsed, |b: std::time::Duration| b.min(elapsed)));
            }
            let best = best.expect("three runs");
            println!("100k paths, {query:?}: best of 3 in {best:?}");
            assert!(best.as_millis() < 50, "{query:?} took {best:?} (best of 3)");
        }
    }
}
