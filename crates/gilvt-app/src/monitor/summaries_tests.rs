use super::*;
use gilvt_monitor::policy::{Decision, MAX_FAILURES};
use gilvt_monitor::privacy::Exclusions;

fn obs(group: usize, done: bool, activity: u64, terminal: bool) -> Observed {
    Observed { group, done, activity, terminal }
}

#[test]
fn only_live_sessions_with_a_timeline_are_requestable() {
    use gilvt_agent::{AgentKind, Status};
    let mut s = Session::new((AgentKind::Claude, "a".to_string()), Instant::now());
    s.status = Status::Idle;
    assert!(agent_requestable(&s, true));
    assert!(!agent_requestable(&s, false), "no timeline yet");
    s.status = Status::Ended;
    assert!(!agent_requestable(&s, true), "an ended session's transcript is final");
}

#[test]
fn first_sight_of_a_session_is_no_trigger() {
    // At startup every old session is seen for the first time: none of them is news.
    assert_eq!(trigger_of(None, obs(0, false, 5, false)), None);
}

#[test]
fn first_sight_of_a_terminal_is_its_first_finished_command() {
    // A terminal is listed only once a command finished (blocks live in memory), so first sight is news.
    assert_eq!(trigger_of(None, obs(9, true, 1, true)), Some(Priority::Ended));
}

#[test]
fn becoming_needs_you_or_error_is_attention() {
    assert_eq!(trigger_of(Some(obs(2, false, 5, false)), obs(0, false, 5, false)), Some(Priority::Attention));
    assert_eq!(trigger_of(Some(obs(2, false, 5, false)), obs(1, true, 6, false)), Some(Priority::Attention));
    assert_eq!(trigger_of(Some(obs(0, false, 5, false)), obs(0, false, 6, false)), None, "still needs you");
}

#[test]
fn a_turn_end_is_ended() {
    assert_eq!(trigger_of(Some(obs(2, false, 5, false)), obs(3, true, 6, false)), Some(Priority::Ended));
    assert_eq!(trigger_of(Some(obs(3, true, 6, false)), obs(3, true, 6, false)), None);
}

#[test]
fn a_new_finished_command_is_ended() {
    assert_eq!(trigger_of(Some(obs(9, true, 3, true)), obs(9, true, 4, true)), Some(Priority::Ended));
    assert_eq!(trigger_of(Some(obs(9, true, 4, true)), obs(9, true, 4, true)), None);
}

#[test]
fn activity_orders_turns_then_steps_then_done() {
    let mut a = gilvt_agent::Turn::new_for_test("p");
    a.index = 3;
    a.steps = 4;
    a.outcome = gilvt_agent::TurnOutcome::Running;
    let mut b = a.clone();
    b.steps = 5;
    let mut c = b.clone();
    c.outcome = gilvt_agent::TurnOutcome::Done;
    let mut d = gilvt_agent::Turn::new_for_test("q");
    d.index = 4;
    let v = |t: gilvt_agent::Turn| activity_of_turns(&[std::sync::Arc::new(t)]);
    assert!(v(a.clone()) < v(b.clone()) && v(b) < v(c) && v(a) < v(d));
    assert_eq!(activity_of_turns(&[]), 0);
}

fn done(result: Result<String, ProviderError>) -> Done {
    Done { key: "agent:claude:x".into(), activity: 7, covers: Covers::Turns(1, 2), previous_goal: Some("old goal".into()), manual: false, result }
}

#[test]
fn success_replaces_the_summary_and_resets_failures() {
    let mut track = Track { failures: 2, ..Track::default() };
    let v = apply_done(None, &mut track, done(Ok("目标：新\n近期：做完了".into())), std::time::UNIX_EPOCH);
    assert_eq!(v.state, SumState::Ready);
    assert_eq!(v.summary.as_ref().unwrap().recent, "做完了");
    assert_eq!(v.activity, 7);
    assert_eq!((track.failures, track.covered), (0, Some(7)));
}

#[test]
fn failure_keeps_the_old_summary_and_pauses_after_three() {
    let old = SummaryView { summary: Some(Summary { goal: None, recent: "旧".into() }), generated_at: None, covers: None, activity: 3, state: SumState::Ready };
    let mut track = Track { failures: MAX_FAILURES - 2, ..Track::default() };
    let v = apply_done(Some(&old), &mut track, done(Err(ProviderError::Timeout)), std::time::UNIX_EPOCH);
    assert_eq!(v.state, SumState::Failed("超时".into()));
    assert_eq!(v.summary, old.summary);
    assert_eq!(v.activity, 3, "still covers what the old one covered");
    let v = apply_done(Some(&v), &mut track, done(Err(ProviderError::Timeout)), std::time::UNIX_EPOCH);
    assert_eq!(v.state, SumState::Paused("超时".into()));
}

#[test]
fn a_trigger_seen_while_a_summary_runs_is_deferred_until_it_finishes() {
    let t0 = std::time::Instant::now();
    let cfg = PolicyConfig { auto: true, interval: Duration::from_secs(120) };
    let mut track = Track { covered: Some(5), last_run: Some(t0), ..Track::default() };
    // A periodic summary of activity 6 is in flight when the turn ends (activity 7).
    assert!(!hold_while_running(&mut track, true, Some(Priority::Ended)), "not decided while running");
    assert!(!hold_while_running(&mut track, true, Some(Priority::Periodic)), "a lower trigger does not lower it");
    assert_eq!(track.deferred, Some(Priority::Ended));
    // It finished (covered 6); a later tick, past the interval, with no new trigger.
    track.covered = Some(6);
    let seen = Seen { activity: 7, running: false, trigger: None };
    assert_eq!(policy::decide(&cfg, &track, &seen, t0 + Duration::from_secs(121)), Decision::Now(Priority::Ended));
    assert!(hold_while_running(&mut track, false, None), "decided when nothing runs");
}

fn block(id: u64, cwd: &str, ended: bool) -> gilvt_term::CommandBlock {
    gilvt_term::CommandBlock {
        id,
        command: Some(format!("cmd{id}")),
        cwd: Some(PathBuf::from(cwd)),
        started: std::time::UNIX_EPOCH,
        ended: ended.then_some(std::time::UNIX_EPOCH + Duration::from_secs(2)),
        exit: ended.then_some(0),
        output_tail: Some(format!("out{id}")),
        end_cwd: None,
        start_line: None,
        end_line: None,
    }
}

fn secret() -> Exclusions {
    Exclusions::new(&["/secret".into()], None)
}

/// A finished block that started in `cwd` and left the shell in `end`.
fn moved(id: u64, cwd: &str, end: Option<&str>, command: &str) -> gilvt_term::CommandBlock {
    gilvt_term::CommandBlock { end_cwd: end.map(PathBuf::from), command: Some(command.into()), ..block(id, cwd, true) }
}

#[test]
fn commands_run_in_excluded_dirs_are_not_sent() {
    let excluded = secret();
    // Newest first, as `CommandLog::recent` gives them.
    let blocks = [
        moved(4, "/secret/a", Some("/work"), "cmd4"),
        moved(3, "/work", Some("/work"), "cmd3"),
        moved(2, "/secret", Some("/work/x"), "cmd2"),
        moved(1, "/work/x", Some("/work/x"), "cmd1"),
    ];
    let (activity, commands) = terminal_commands(&blocks, None, &excluded).unwrap();
    assert_eq!(activity, 3, "the newest finished command that is shown");
    assert_eq!(commands.iter().map(|c| c.command.as_str()).collect::<Vec<_>>(), ["cmd3", "cmd1"]);
    assert_eq!(commands[0].output_tail.as_deref(), Some("out3"));
    assert_eq!(commands[0].took, Some(Duration::from_secs(2)));
}

#[test]
fn a_command_that_ends_in_an_excluded_directory_is_not_sent() {
    let excluded = secret();
    // `cd /secret && cat notes` started in /work: the shell reported /secret after it.
    let blocks = [moved(3, "/work", None, "cmd3"), moved(2, "/work", None, "cmd2"), moved(1, "/work", Some("/secret"), "cmd1")];
    let shown = |current: &str| {
        let (activity, commands) = terminal_commands(&blocks, Some(std::path::Path::new(current)), &excluded).unwrap();
        (activity, commands.into_iter().map(|c| c.command).collect::<Vec<_>>())
    };
    // cmd2 ended where cmd3 started; cmd3, the newest, ended where the terminal is now.
    assert_eq!(shown("/work"), (3, vec!["cmd3".to_string(), "cmd2".to_string()]));
    assert_eq!(shown("/secret/x"), (2, vec!["cmd2".to_string()]), "the newest one went into /secret");
    // Without a recorded end, the next block's start directory tells it.
    let blocks = [moved(2, "/secret", None, "cmd2"), moved(1, "/work", None, "cmd1")];
    assert_eq!(terminal_commands(&blocks, Some(std::path::Path::new("/secret")), &excluded), None);
}

#[test]
fn a_command_naming_an_excluded_directory_is_not_sent() {
    let excluded = secret();
    let blocks = [moved(3, "/work", Some("/work"), "(cd /secret && make)"), moved(2, "/work", Some("/work"), "cat /secret/notes"), moved(1, "/work", Some("/work"), "ls /secretive")];
    let (activity, commands) = terminal_commands(&blocks, None, &excluded).unwrap();
    assert_eq!(activity, 1);
    assert_eq!(commands.iter().map(|c| c.command.as_str()).collect::<Vec<_>>(), ["ls /secretive"]);
}

#[test]
fn a_terminal_with_only_excluded_or_running_commands_is_skipped() {
    let excluded = secret();
    assert_eq!(terminal_commands(&[block(2, "/secret/a", true), block(1, "/secret", true)], None, &excluded), None);
    assert_eq!(terminal_commands(&[block(1, "/work", false)], None, &excluded), None, "nothing finished yet");
    assert_eq!(terminal_commands(&[], None, &excluded), None);
    let (activity, commands) = terminal_commands(&[block(2, "/work", false), block(1, "/work", true)], None, &excluded).unwrap();
    assert_eq!((activity, commands.len()), (1, 2), "a running command is shown; the finished one is the activity");
}

#[test]
fn exclusions_can_be_used_re_entrantly() {
    // `tick` calls `subjects(ex, cx)` inside `with_exclusions`, which asks `is_excluded` again.
    let cell = RefCell::new(Exclusions::new(&[], None));
    let raw = vec!["/secret".to_string()];
    let nested = with_refreshed(&cell, &raw, |outer| {
        with_refreshed(&cell, &raw, |inner| (outer.excluded(Some(Path::new("/secret/x"))), inner.excluded(Some(Path::new("/open")))))
    });
    assert_eq!(nested, (true, false));
}

fn empty() -> Summaries {
    let (done_tx, _done_rx) = async_channel::unbounded();
    Summaries {
        views: HashMap::new(),
        tracks: HashMap::new(),
        observed: HashMap::new(),
        queue: Queue::default(),
        running: HashSet::new(),
        manual: HashSet::new(),
        loaded: HashSet::new(),
        cache_dir: None,
        run_dir: None,
        done_tx,
        exclusions: RefCell::new(Exclusions::default()),
    }
}

fn job() -> Job {
    Job { activity: 1, covers: Covers::Commands(1), request: OneShot { instructions: String::new(), prompt: String::new() }, previous_goal: None }
}

fn ready() -> Arc<SummaryView> {
    Arc::new(SummaryView { summary: None, generated_at: None, covers: None, activity: 1, state: SumState::Ready })
}

#[test]
fn disabling_forgets_what_was_queued_and_seen() {
    let mut s = empty();
    s.manual.insert("pane:1".into());
    s.observed.insert("pane:1".into(), obs(9, true, 1, true));
    s.queue.push("pane:1".into(), Priority::Manual, job());
    s.disable();
    assert!(s.manual.is_empty() && s.observed.is_empty() && s.queue.is_empty());
}

#[test]
fn dropping_keys_forgets_them_but_keeps_the_rest() {
    let mut s = empty();
    for k in ["agent:claude:a", "agent:claude:b"] {
        s.views.insert(k.into(), ready());
        s.tracks.insert(k.into(), Track::default());
        s.observed.insert(k.into(), obs(3, true, 1, false));
        s.manual.insert(k.into());
        s.loaded.insert(k.into());
        s.queue.push(k.into(), Priority::Ended, job());
    }
    s.running.insert("agent:claude:a".into());
    s.drop_keys(&["agent:claude:a".to_string()]);
    let a = "agent:claude:a";
    assert!(!s.views.contains_key(a) && !s.tracks.contains_key(a) && !s.observed.contains_key(a));
    assert!(!s.manual.contains(a) && !s.queue.contains(a));
    assert!(!s.loaded.contains(a), "un-excluding loads the cache again");
    assert!(s.running.contains(a), "a run in flight still finishes (its result is then thrown away)");
    let b = "agent:claude:b";
    assert!(s.views.contains_key(b) && s.tracks.contains_key(b) && s.observed.contains_key(b));
    assert!(s.manual.contains(b) && s.loaded.contains(b) && s.queue.contains(b));
}

#[test]
fn sessions_under_new_exclusions_are_found() {
    use gilvt_agent::AgentKind;
    let ex = Exclusions::new(&["/secret".to_string()], None);
    let (a, b, c) = ((AgentKind::Claude, "a".to_string()), (AgentKind::Codex, "b".to_string()), (AgentKind::Claude, "c".to_string()));
    let all = [(&a, Some(Path::new("/secret/x"))), (&b, Some(Path::new("/open"))), (&c, None)];
    assert_eq!(excluded_keys(all.into_iter(), &ex), vec![agent_key(&a)]);
}

#[test]
fn a_result_for_a_key_excluded_meanwhile_is_not_kept() {
    let key = "agent:claude:x";
    let mut s = empty();
    s.running.insert(key.into());
    s.store(done(Ok("目标：新\n近期：做完了".into())), true);
    assert!(!s.running.contains(key));
    assert!(!s.views.contains_key(key) && !s.tracks.contains_key(key));
    s.running.insert(key.into());
    s.store(done(Ok("目标：新\n近期：做完了".into())), false);
    assert_eq!(s.views[key].state, SumState::Ready);
}
