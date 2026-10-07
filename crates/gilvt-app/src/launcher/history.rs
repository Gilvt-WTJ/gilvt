//! Past sessions for the 会话 palette (spec §3.3): one `History` global with the latest result of
//! `HistoryIndex::refresh`. The scan runs on the background executor, one at a time: a request while one runs
//! schedules exactly one more. The cached result is shown first (`HistoryIndex::entries`), the scan's after it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use gilvt_agent::{
    choose_title, AgentKind, HistoryEntry, HistoryIndex, ReviewSessionIndex, Title, TitleSource,
};
use gpui::{App, Global};

use crate::workspace;

fn chosen(e: &HistoryEntry, saved_name: Option<&str>) -> Title {
    choose_title(
        saved_name,
        e.custom_title.as_deref(),
        e.ai_title.as_deref(),
        &e.topic_prompt,
        &e.first_prompt,
    )
}

/// A session's title in lists and messages. From highest: the user's rename in gilvt, their own title in the
/// agent, the title the agent generated, the first informative prompt tidied up, the first prompt (see
/// `gilvt_agent::choose_title`).
pub fn title(e: &HistoryEntry, saved_name: Option<String>) -> String {
    chosen(e, saved_name.as_deref()).text
}

/// Where [`title`] came from.
pub fn title_source(e: &HistoryEntry, saved_name: Option<String>) -> TitleSource {
    chosen(e, saved_name.as_deref()).source
}

/// One refresh at a time; asks made meanwhile collapse into a single follow-up. Runs are numbered from 1.
#[derive(Debug, Default)]
struct Runs {
    running: bool,
    again: bool,
    /// The number of the latest run started (0: none yet).
    started: u64,
}

impl Runs {
    /// A refresh was asked for: `Some(run)` = start it now; None = one is running, another follows it.
    fn request(&mut self) -> Option<u64> {
        if self.running {
            self.again = true;
            return None;
        }
        self.running = true;
        self.started += 1;
        Some(self.started)
    }

    /// The running refresh ended: `Some(run)` = start the one asked for meanwhile.
    fn finished(&mut self) -> Option<u64> {
        self.running = false;
        if std::mem::take(&mut self.again) {
            self.request()
        } else {
            None
        }
    }
}

/// Sessions moved to the Trash, by transcript, with the latest run started before they were. A run that
/// began before the move may still list them; one begun after it does not (their files are gone).
#[derive(Debug, Default)]
struct Trashed(HashMap<PathBuf, u64>);

impl Trashed {
    fn add(&mut self, transcript: PathBuf, started: u64) {
        self.0.insert(transcript, started);
    }

    /// The entries of run `run`, without the ones trashed while it ran. `last` = the run's final result: the
    /// marks it made obsolete are dropped (a session restored from the Trash shows up again at the next run).
    fn filter(&mut self, run: u64, mut entries: Vec<HistoryEntry>, last: bool) -> Vec<HistoryEntry> {
        entries.retain(|e| self.0.get(&e.transcript).is_none_or(|&mark| run > mark));
        if last {
            self.0.retain(|_, mark| *mark >= run);
        }
        entries
    }

    fn filter_reviews(&mut self, run: u64, mut reviews: Vec<ReviewSessionIndex>, last: bool) -> Vec<ReviewSessionIndex> {
        reviews.retain(|review| self.0.get(&review.transcript).is_none_or(|&mark| run > mark));
        if last {
            self.0.retain(|_, mark| *mark >= run);
        }
        reviews
    }
}

pub struct History {
    generation: u64,
    entries: Arc<Vec<HistoryEntry>>,
    reviews: Arc<Vec<ReviewSessionIndex>>,
    /// Loaded by the first run (the cache file can be large); locked by the background scans.
    index: Arc<Mutex<Option<HistoryIndex>>>,
    state_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    runs: Runs,
    trashed: Trashed,
    /// Session directories the latest scan found gone (checked on the background thread, never in the UI).
    missing: Arc<HashSet<PathBuf>>,
    /// Bumped whenever `missing` changes, so views rebuild their rows.
    dirs_generation: u64,
}

/// History and review projections published by one cache/scan generation.
#[derive(Clone)]
pub struct HistorySnapshot {
    pub generation: u64,
    pub entries: Arc<Vec<HistoryEntry>>,
    pub reviews: Arc<Vec<ReviewSessionIndex>>,
    pub refreshing: bool,
}

impl Global for History {}

impl History {
    /// Sets up the global (`state_dir` holds `history.json`) and starts the first refresh.
    pub fn init(state_dir: Option<PathBuf>, cx: &mut App) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        cx.set_global(History {
            generation: 0,
            entries: Arc::default(),
            reviews: Arc::default(),
            index: Arc::default(),
            state_dir,
            home,
            runs: Runs::default(),
            trashed: Trashed::default(),
            missing: Arc::default(),
            dirs_generation: 0,
        });
        History::refresh(cx);
    }

    /// Past sessions, newest `last_active` first (empty until the first result).
    pub fn entries(&self) -> Arc<Vec<HistoryEntry>> {
        self.entries.clone()
    }

    /// Both projections from the same publication. Consumers must not combine separately sampled globals.
    pub fn snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            generation: self.generation,
            entries: self.entries.clone(),
            reviews: self.reviews.clone(),
            refreshing: self.runs.running,
        }
    }

    /// A scan is running (the palette can say so while it shows the older list).
    pub fn refreshing(&self) -> bool {
        self.runs.running
    }

    /// Whether a session directory exists. Directories the scan has not checked yet count as existing, so
    /// nothing is flagged before the first scan ends.
    pub fn dir_exists(&self, dir: &Path) -> bool {
        !self.missing.contains(dir)
    }

    /// Changes whenever [`History::dir_exists`] may answer differently.
    pub fn dirs_generation(&self) -> u64 {
        self.dirs_generation
    }

    /// The entry of session `id`, if the latest result lists it.
    pub fn find(&self, agent: AgentKind, id: &str) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.agent == agent && e.session_id == id)
    }

    /// Rescans in the background, then repaints the windows. Called at launch and on every ⌘⇧R.
    pub fn refresh(cx: &mut App) {
        if !cx.has_global::<History>() {
            return;
        }
        if let Some(run) = cx.global_mut::<History>().runs.request() {
            start(run, cx);
        }
    }

    /// These sessions' transcripts went to the Trash: out of the list now, out of the cache in the background.
    pub fn forget(transcripts: Vec<PathBuf>, cx: &mut App) {
        if transcripts.is_empty() || !cx.has_global::<History>() {
            return;
        }
        let h = cx.global_mut::<History>();
        h.generation += 1;
        for t in &transcripts {
            h.trashed.add(t.clone(), h.runs.started);
        }
        let kept: Vec<HistoryEntry> = h.entries.iter().filter(|e| !transcripts.contains(&e.transcript)).cloned().collect();
        h.entries = Arc::new(kept);
        h.reviews = Arc::new(h.reviews.iter().filter(|review| !transcripts.contains(&review.transcript)).cloned().collect());
        let index = h.index.clone();
        cx.background_executor()
            .spawn(async move {
                let mut index = index.lock().unwrap_or_else(|e| e.into_inner());
                // Not loaded yet: the first scan drops the vanished files itself.
                if let Some(index) = index.as_mut() {
                    transcripts.iter().for_each(|t| index.forget(t));
                }
            })
            .detach();
        cx.defer(workspace::notify_all);
    }
}

/// Run `run`: the cached list (first run only), then the scan's.
fn start(run: u64, cx: &mut App) {
    let h = cx.global::<History>();
    let (index, state_dir, home) = (h.index.clone(), h.state_dir.clone(), h.home.clone());
    cx.spawn(async move |cx| {
        let loading = index.clone();
        let cached = cx
            .background_executor()
            .spawn(async move {
                let mut slot = loading.lock().unwrap_or_else(|e| e.into_inner());
                if slot.is_some() {
                    return None;
                }
                let loaded = HistoryIndex::load(state_dir.as_deref());
                let entries = loaded.entries();
                let reviews = loaded.review_sessions();
                *slot = Some(loaded);
                Some((entries, reviews))
            })
            .await;
        if let Some((entries, reviews)) = cached {
            let _ = cx.update(|cx| publish(run, entries, reviews, false, cx));
        }
        let scanned = cx
            .background_executor()
            .spawn(async move {
                let home = home?;
                let (entries, reviews) = {
                    let mut slot = index.lock().unwrap_or_else(|e| e.into_inner());
                    let index = slot.as_mut()?;
                    (index.refresh(&home), index.review_sessions())
                };
                // Not under the index lock: a hung mount must not stall the next scan or `History::forget`.
                let missing = missing_dirs(&entries);
                Some((entries, reviews, missing))
            })
            .await;
        let _ = cx.update(|cx| {
            if let Some((entries, reviews, missing)) = scanned {
                set_missing(missing, cx);
                publish(run, entries, reviews, true, cx);
            }
            let next = cx.global_mut::<History>().runs.finished();
            match next {
                Some(next) => start(next, cx),
                // `refreshing()` changed.
                None => cx.defer(workspace::notify_all),
            }
        });
    })
    .detach();
}

/// The distinct directories of `entries` that are not directories on disk (blocking: background only).
fn missing_dirs(entries: &[HistoryEntry]) -> HashSet<PathBuf> {
    let dirs: HashSet<&Path> = entries.iter().map(|e| e.cwd.as_path()).collect();
    dirs.into_iter().filter(|d| !d.is_dir()).map(Path::to_path_buf).collect()
}

fn set_missing(missing: HashSet<PathBuf>, cx: &mut App) {
    let h = cx.global_mut::<History>();
    if *h.missing != missing {
        h.missing = Arc::new(missing);
        h.dirs_generation += 1;
    }
}

fn publish(run: u64, entries: Vec<HistoryEntry>, reviews: Vec<ReviewSessionIndex>, last: bool, cx: &mut App) {
    let reviews = {
        let h = cx.global_mut::<History>();
        h.generation += 1;
        h.entries = Arc::new(h.trashed.filter(run, entries, false));
        h.reviews = Arc::new(h.trashed.filter_reviews(run, reviews, last));
        h.reviews.clone()
    };
    if last {
        crate::review::ReviewService::publish_complete(&reviews, SystemTime::now(), cx);
    }
    cx.defer(workspace::notify_all);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn entry(transcript: &str, first_prompt: &str) -> HistoryEntry {
        HistoryEntry {
            agent: AgentKind::Claude,
            session_id: "s".into(),
            cwd: "/Users/u/proj".into(),
            transcript: transcript.into(),
            first_prompt: first_prompt.into(),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: SystemTime::UNIX_EPOCH,
            turns: 1,
            model: None,
            size: 0,
        }
    }

    fn paths(entries: &[HistoryEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.transcript.to_str().unwrap()).collect()
    }

    #[test]
    fn titles_prefer_the_rename() {
        let e = entry("/t/a.jsonl", "修一下登录");
        assert_eq!(title(&e, Some("登录修复".into())), "登录修复");
        assert_eq!(title(&e, Some("  ".into())), "修一下登录");
        assert_eq!(title(&e, None), "修一下登录");
        assert_eq!(title(&entry("/t/b.jsonl", ""), None), "（无提示词）");
    }

    #[test]
    fn titles_follow_the_agents_own_before_the_prompts() {
        let mut e = entry("/t/a.jsonl", "继续");
        e.topic_prompt = "整理 README 的结构。然后补测试".into();
        // No agent title: the first informative prompt, tidied (not 「继续」).
        assert_eq!(title(&e, None), "整理 README 的结构");
        assert_eq!(title_source(&e, None), TitleSource::Prompt);
        e.ai_title = Some("整理文档".into());
        assert_eq!((title(&e, None), title_source(&e, None)), ("整理文档".to_string(), TitleSource::Ai));
        e.custom_title = Some("我的名字".into());
        assert_eq!((title(&e, None), title_source(&e, None)), ("我的名字".to_string(), TitleSource::Custom));
        // A rename in gilvt beats both.
        assert_eq!(title(&e, Some("重命名".into())), "重命名");
        assert_eq!(title_source(&e, Some("重命名".into())), TitleSource::Saved);
        assert_eq!(title_source(&e, Some("  ".into())), TitleSource::Custom, "a blank rename is none");
    }

    #[test]
    fn missing_dirs_lists_each_gone_directory_once() {
        let here = std::env::temp_dir();
        let mut a = entry("/t/a.jsonl", "a");
        a.cwd = here.clone();
        let mut b = entry("/t/b.jsonl", "b");
        b.cwd = here.join("gilvt-no-such-dir-8f3a");
        let c = b.clone();
        let missing = missing_dirs(&[a, b.clone(), c]);
        assert_eq!(missing.into_iter().collect::<Vec<_>>(), [b.cwd]);
    }

    #[test]
    fn one_run_at_a_time_and_one_more_at_most() {
        let mut runs = Runs::default();
        assert_eq!(runs.request(), Some(1));
        assert!(runs.running);
        // Two asks during run 1 collapse into a single run 2.
        assert_eq!(runs.request(), None);
        assert_eq!(runs.request(), None);
        assert_eq!(runs.finished(), Some(2));
        assert!(runs.running);
        assert_eq!(runs.finished(), None);
        assert!(!runs.running);
        assert_eq!(runs.request(), Some(3));
    }

    #[test]
    fn a_run_begun_before_the_trash_does_not_bring_sessions_back() {
        let mut trashed = Trashed::default();
        let all = || vec![entry("/t/a.jsonl", "a"), entry("/t/b.jsonl", "b")];
        // Moved while run 4 was scanning (the latest started): its results still list a.
        trashed.add("/t/a.jsonl".into(), 4);
        assert_eq!(paths(&trashed.filter(4, all(), false)), ["/t/b.jsonl"]);
        assert_eq!(paths(&trashed.filter(4, all(), true)), ["/t/b.jsonl"]);
        // Run 5 began after the move: whatever it lists is on disk (restored from the Trash), and the mark is gone.
        assert_eq!(paths(&trashed.filter(5, all(), true)), ["/t/a.jsonl", "/t/b.jsonl"]);
        assert!(trashed.0.is_empty());
    }

    #[test]
    fn a_cached_list_keeps_the_marks() {
        let mut trashed = Trashed::default();
        trashed.add("/t/a.jsonl".into(), 0);
        // Run 1's cached list (the index was not loaded when a was trashed): shown, the mark stays until its scan.
        assert_eq!(paths(&trashed.filter(1, vec![entry("/t/a.jsonl", "a")], false)), ["/t/a.jsonl"]);
        assert_eq!(trashed.0.len(), 1);
        assert!(trashed.filter(1, vec![], true).is_empty());
        assert!(trashed.0.is_empty());
    }
}
