//! A Claude transcript (`<id>.jsonl`) → [`HistoryEntry`]. Real prompts are what M3a's
//! `crate::claude::parse_record` reports as `PromptSubmit`: no `!` commands, slash-command echoes,
//! task notifications or hand-backs.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{disk_size, for_each_record, for_each_record_from, HistoryEntry, ParsedSession, Scan};
use crate::event::{str_field, AgentKind, Event};
use crate::review::ReviewScanner;

pub(super) fn parse(path: &Path) -> io::Result<Option<ParsedSession>> {
    let Some(session_id) = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let mut scan = Scan::default();
    let mut cwd: Option<PathBuf> = None;
    let mut entrypoint_seen = false;
    let mut sdk = false;
    let (mut sidechain, mut main_thread) = (false, false);
    let mut review = ReviewScanner::new(AgentKind::Claude);
    let read = for_each_record(path, |r, start, end, raw| {
        // Only the first one counts: a session continued with `claude -p --resume` gets `sdk-cli` records
        // after its interactive ones, and stays an interactive session.
        if !entrypoint_seen {
            if let Some(entrypoint) = str_field(r, "entrypoint") {
                entrypoint_seen = true;
                if entrypoint.starts_with("sdk") {
                    sdk = true;
                    return false;
                }
            }
        }
        if cwd.is_none() {
            cwd = str_field(r, "cwd")
                .filter(|c| !c.is_empty())
                .map(PathBuf::from);
        }
        match r.get("isSidechain").and_then(Value::as_bool) {
            Some(true) => sidechain = true,
            Some(false) => main_thread = true,
            None => {}
        }
        scan.timestamp(r);
        scan.title_record(r);
        for event in crate::claude::parse_record(r) {
            match event {
                Event::PromptSubmit { text } => scan.prompt(&text),
                Event::Model { name } => scan.model = Some(name),
                _ => {}
            }
        }
        review.apply(r, start, end, raw);
        true
    })?;
    let Some(cwd) = cwd else { return Ok(None) };
    if sdk || (sidechain && !main_thread) {
        return Ok(None);
    }
    let transcript_size = disk_size(path);
    let size = transcript_size + disk_size(&path.with_extension(""));
    let Some(history) = scan.entry(AgentKind::Claude, session_id.to_string(), cwd, path, size)?
    else {
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

/// Replays from the last completed turn boundary after a verified append. The earlier projection remains
/// untouched, while an open turn that existed at the previous refresh is rebuilt from disk.
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
        // Titles can sit anywhere in the file; the ones before the restart point come from the old entry
        // and any newer record read below replaces them.
        custom_title: previous_history.custom_title.clone(),
        ai_title: previous_history.ai_title.clone(),
        turns: restart as u32,
        started: previous_history.started,
        last: Some(previous_history.last_active),
        model: previous_history.model.clone(),
    };
    let mut review =
        ReviewScanner::resume(AgentKind::Claude, previous_review.turns[..restart].to_vec());
    let read = for_each_record_from(path, last.start_offset, |r, start, end, raw| {
        scan.timestamp(r);
        scan.title_record(r);
        for event in crate::claude::parse_record(r) {
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
    let size = transcript_size + disk_size(&path.with_extension(""));
    let Some(history) = scan.entry(
        AgentKind::Claude,
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
            "Claude transcript changed before the appended records",
        ));
    }
    Ok(Some(ParsedSession { history, review }))
}
