//! A Codex rollout → [`HistoryEntry`]. The first line (`session_meta`) names the session and decides the
//! exclusions; prompts are `event_msg` `user_message`s, as in the timeline.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    codex_siblings, disk_size, for_each_record, for_each_record_from, HistoryEntry, ParsedSession,
    Scan,
};
use crate::event::{str_field, AgentKind, Event};
use crate::review::ReviewScanner;

/// Identity of the session, from its first line.
struct Meta {
    session_id: String,
    cwd: PathBuf,
}

pub(super) fn parse(path: &Path) -> io::Result<Option<ParsedSession>> {
    let mut meta: Option<Option<Meta>> = None;
    let mut scan = Scan::default();
    let mut review = ReviewScanner::new(AgentKind::Codex);
    let read = for_each_record(path, |r, start, end, raw| {
        if meta.is_none() {
            meta = Some(session_meta(r));
            if meta.as_ref().is_some_and(Option::is_none) {
                return false;
            }
        }
        scan.timestamp(r);
        for event in crate::codex::parse_record(r) {
            match event {
                Event::PromptSubmit { text } => scan.prompt(&text),
                Event::Model { name } => scan.model = Some(name),
                _ => {}
            }
        }
        review.apply(r, start, end, raw);
        true
    })?;
    let Some(Some(Meta { session_id, cwd })) = meta else {
        return Ok(None);
    };
    let transcript_size = disk_size(path);
    let size = transcript_size
        + codex_siblings(path)
            .iter()
            .map(|p| disk_size(p))
            .sum::<u64>();
    let Some(history) = scan.entry(AgentKind::Codex, session_id, cwd, path, size)? else {
        return Ok(None);
    };
    let key = (history.agent, history.session_id.clone());
    let review = review.finish(
        key,
        path.to_path_buf(),
        transcript_size,
        read.scanned_through,
        read.incomplete_tail,
    );
    Ok(Some(ParsedSession { history, review }))
}

pub(super) fn parse_appended(
    path: &Path,
    previous_history: &HistoryEntry,
    previous_review: &crate::review::ReviewSessionIndex,
) -> io::Result<Option<ParsedSession>> {
    let Some(last) = previous_review.turns.last() else {
        return parse(path);
    };
    let restart = previous_review.turns.len() - 1;
    let expected = last.clone();
    let mut scan = Scan {
        first_prompt: Some(previous_history.first_prompt.clone()),
        topic_prompt: Some(previous_history.topic_prompt.clone()).filter(|t| !t.is_empty()),
        custom_title: None,
        ai_title: None,
        turns: restart as u32,
        started: previous_history.started,
        last: Some(previous_history.last_active),
        model: previous_history.model.clone(),
    };
    let mut review =
        ReviewScanner::resume(AgentKind::Codex, previous_review.turns[..restart].to_vec());
    let read = for_each_record_from(path, last.start_offset, |r, start, end, raw| {
        scan.timestamp(r);
        for event in crate::codex::parse_record(r) {
            match event {
                Event::PromptSubmit { text } => scan.prompt(&text),
                Event::Model { name } => scan.model = Some(name),
                _ => {}
            }
        }
        review.apply(r, start, end, raw);
        true
    })?;
    let transcript_size = disk_size(path);
    let size = transcript_size
        + codex_siblings(path)
            .iter()
            .map(|sibling| disk_size(sibling))
            .sum::<u64>();
    let Some(history) = scan.entry(
        AgentKind::Codex,
        previous_history.session_id.clone(),
        previous_history.cwd.clone(),
        path,
        size,
    )?
    else {
        return Ok(None);
    };
    let review = review.finish(
        (history.agent, history.session_id.clone()),
        path.to_path_buf(),
        transcript_size,
        read.scanned_through,
        read.incomplete_tail,
    );
    if review.turns.get(restart) != Some(&expected) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Codex transcript changed before the appended records",
        ));
    }
    Ok(Some(ParsedSession { history, review }))
}

/// Id and cwd of an interactive session's `session_meta`. Subagent threads (`source` is an object such as
/// `{"subagent": {"thread_spawn": …}}`) and `codex exec` runs are left out; other string sources (`cli`,
/// `vscode`, `unknown`) are kept.
fn session_meta(r: &Value) -> Option<Meta> {
    if str_field(r, "type") != Some("session_meta") {
        return None;
    }
    let p = r.get("payload")?;
    let source = str_field(p, "source")?;
    if source == "exec" || str_field(p, "originator") == Some("codex_exec") {
        return None;
    }
    let session_id = str_field(p, "id")
        .or_else(|| str_field(p, "session_id"))
        .filter(|s| !s.is_empty())?;
    let cwd = str_field(p, "cwd").filter(|c| !c.is_empty())?;
    Some(Meta {
        session_id: session_id.to_string(),
        cwd: PathBuf::from(cwd),
    })
}
