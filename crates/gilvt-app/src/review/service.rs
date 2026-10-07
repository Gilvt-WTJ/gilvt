use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use gilvt_agent::{
    ReviewDocument, ReviewPage, ReviewSessionIndex, ReviewState, SessionKey, TurnCursor,
};
use gpui::{App, Global};

use super::ReviewStore;

/// App-wide owner of persisted review cursors. History publishes only complete refresh generations here.
pub struct ReviewService {
    store: ReviewStore,
    last_error: Option<String>,
    detail: DetailState,
    /// Moves on every store change, so projections of the stored states can tell they went stale.
    revision: u64,
}

#[derive(Clone, Debug, Default)]
struct DetailState {
    request: u64,
    key: Option<SessionKey>,
    loading: bool,
    document: Option<Arc<ReviewDocument>>,
    error: Option<String>,
}

impl Global for ReviewService {}

impl ReviewService {
    pub fn init(state_dir: Option<PathBuf>, cx: &mut App) {
        let store = state_dir
            .as_deref()
            .map(ReviewStore::open)
            .unwrap_or_else(ReviewStore::in_memory);
        let last_error = store.load_error().map(str::to_string);
        cx.set_global(ReviewService {
            store,
            last_error,
            detail: DetailState::default(),
            revision: 0,
        });
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn state(&self, key: &SessionKey) -> ReviewState {
        self.store.state(key)
    }

    pub fn states(
        &self,
        sessions: impl IntoIterator<Item = SessionKey>,
    ) -> HashMap<SessionKey, ReviewState> {
        sessions
            .into_iter()
            .map(|key| {
                let state = self.store.state(&key);
                (key, state)
            })
            .collect()
    }

    pub fn initialized(&self) -> bool {
        self.store.baseline_complete()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn detail(&self) -> Option<Arc<ReviewDocument>> {
        self.detail.document.clone()
    }

    pub fn detail_error(&self) -> Option<&str> {
        self.detail.error.as_deref()
    }

    pub fn detail_loading(&self) -> bool {
        self.detail.loading
    }

    /// Loads a review page off the UI thread. Only the newest request may publish its result.
    pub fn request_detail(index: ReviewSessionIndex, page: ReviewPage, cx: &mut App) {
        if !cx.has_global::<ReviewService>() {
            return;
        }
        let request = cx
            .global_mut::<ReviewService>()
            .begin_detail(index.key.clone());
        cx.spawn(async move |cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    ReviewDocument::load(&index, page)
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = cx.update(|cx| {
                if cx
                    .global_mut::<ReviewService>()
                    .finish_detail(request, loaded)
                {
                    cx.defer(crate::workspace::notify_all);
                }
            });
        })
        .detach();
    }

    pub fn advance_reviewed(
        &mut self,
        index: &ReviewSessionIndex,
        cursor: TurnCursor,
        now: SystemTime,
    ) -> io::Result<bool> {
        let ordered = index
            .turns
            .iter()
            .map(|turn| turn.cursor.clone())
            .collect::<Vec<_>>();
        self.persist(|store| store.advance_reviewed(&index.key, cursor, &ordered, now))
    }

    pub fn reset_reviewed(
        &mut self,
        key: &SessionKey,
        cursor: Option<TurnCursor>,
        now: SystemTime,
    ) -> io::Result<()> {
        self.persist(|store| store.reset_reviewed(key, cursor, now))
    }

    pub fn set_snoozed_until(
        &mut self,
        key: &SessionKey,
        until: Option<SystemTime>,
    ) -> io::Result<()> {
        self.persist(|store| store.set_snoozed_until(key, until))
    }

    pub fn set_pinned(&mut self, key: &SessionKey, pinned: bool) -> io::Result<()> {
        self.persist(|store| store.set_pinned(key, pinned))
    }

    pub fn set_archived(&mut self, key: &SessionKey, turns: u32, at: SystemTime) -> io::Result<()> {
        self.persist(|store| store.set_archived(key, turns, at))
    }

    pub fn clear_archived(&mut self, key: &SessionKey) -> io::Result<()> {
        self.persist(|store| store.clear_archived(key))
    }

    /// Called only after a complete disk refresh, never for the startup cache publication.
    pub fn publish_complete(sessions: &[ReviewSessionIndex], now: SystemTime, cx: &mut App) {
        if !cx.has_global::<ReviewService>() {
            return;
        }
        let service = cx.global_mut::<ReviewService>();
        let result = if !service.store.baseline_complete() {
            service.store.initialize_baseline(
                now,
                sessions.iter().map(|session| {
                    (
                        session.key.clone(),
                        session.last_completed().map(|turn| turn.cursor.clone()),
                    )
                }),
            )
        } else {
            let initialized_at = service.store.initialized_at();
            service
                .store
                .reconcile_baseline(sessions.iter().map(|session| {
                    let cursor = initialized_at.and_then(|initialized_at| {
                        session
                            .turns
                            .iter()
                            .filter(|turn| {
                                turn.completed_at
                                    .is_none_or(|completed_at| completed_at <= initialized_at)
                            })
                            .last()
                            .map(|turn| turn.cursor.clone())
                    });
                    (session.key.clone(), cursor)
                }))
        };
        service.last_error = result.err().map(|error| error.to_string());
        service.revision += 1;
    }

    fn begin_detail(&mut self, key: SessionKey) -> u64 {
        self.detail.request += 1;
        self.detail.key = Some(key);
        self.detail.loading = true;
        self.detail.document = None;
        self.detail.error = None;
        self.detail.request
    }

    fn finish_detail(&mut self, request: u64, result: Result<Arc<ReviewDocument>, String>) -> bool {
        if request != self.detail.request {
            return false;
        }
        self.detail.loading = false;
        match result {
            Ok(document) => {
                self.detail.key = Some(document.key.clone());
                self.detail.document = Some(document);
                self.detail.error = None;
            }
            Err(error) => {
                self.detail.document = None;
                self.detail.error = Some(error);
            }
        }
        true
    }

    fn persist<T>(
        &mut self,
        change: impl FnOnce(&mut ReviewStore) -> io::Result<T>,
    ) -> io::Result<T> {
        match change(&mut self.store) {
            Ok(value) => {
                self.last_error = None;
                self.revision += 1;
                Ok(value)
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::{AgentKind, ReviewCompatibility};

    fn service() -> ReviewService {
        ReviewService {
            store: ReviewStore::in_memory(),
            last_error: None,
            detail: DetailState::default(),
            revision: 0,
        }
    }

    fn document(id: &str) -> Arc<ReviewDocument> {
        Arc::new(ReviewDocument {
            key: (AgentKind::Claude, id.into()),
            snapshot_through: TurnCursor::Claude {
                prompt_uuid: "p".into(),
            },
            snapshot_size: 1,
            turns: Vec::new(),
            has_earlier: false,
            has_later: false,
            compatibility: ReviewCompatibility::default(),
            bytes_read: 0,
        })
    }

    #[test]
    fn the_revision_moves_only_when_the_store_changes() {
        let mut service = service();
        let key = (AgentKind::Claude, "s".to_string());
        let start = service.revision();
        service.set_pinned(&key, true).unwrap();
        assert_eq!(service.revision(), start + 1);
        service.set_snoozed_until(&key, None).unwrap();
        assert_eq!(service.revision(), start + 2);
        // Reading never counts as a change.
        let _ = service.state(&key);
        assert_eq!(service.revision(), start + 2);
    }

    #[test]
    fn an_old_detail_result_cannot_replace_the_new_selection() {
        let mut service = service();
        let old = service.begin_detail((AgentKind::Claude, "old".into()));
        let new = service.begin_detail((AgentKind::Codex, "new".into()));
        assert!(!service.finish_detail(old, Ok(document("old"))));
        assert!(service.detail_loading());
        assert!(service.detail().is_none());
        assert!(service.finish_detail(new, Ok(document("new"))));
        assert_eq!(service.detail().unwrap().key.1, "new");
    }
}
