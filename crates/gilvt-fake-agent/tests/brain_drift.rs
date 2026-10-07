//! The fake brain must answer in the formats gilvt reads (S2 §9): if gilvt's parsers change, these fail first.

use gilvt_fake_agent::{brain, Agent};
use gilvt_monitor::provider::process::Output;
use gilvt_monitor::provider::{claude, codex, models, ProviderError};
use serde_json::json;

#[test]
fn gilvt_reads_the_fake_model_list() {
    let (m, next) = models::parse_model_page(&json!({"id": 2, "result": brain::model_list(None, None).unwrap()})).unwrap();
    assert_eq!(m[0].id, brain::CODEX_MODELS[0]);
    assert_eq!(next.as_deref(), Some("2"));
    let (m, next) = models::parse_model_page(&json!({"id": 3, "result": brain::model_list(None, Some("2")).unwrap()})).unwrap();
    assert_eq!(m[0].id, brain::CODEX_MODELS[1]);
    assert_eq!(next, None);
}

#[test]
fn gilvt_reads_a_missing_model_as_a_failure_not_as_auth() {
    let out = brain::missing_model(Agent::Claude, "missing-x");
    let r = claude::parse(&Output { code: Some(out.code), stdout: out.stdout, stderr: out.stderr });
    assert!(matches!(r, Err(ProviderError::Exited { .. })), "{r:?}");
    let out = brain::missing_model(Agent::Codex, "missing-x");
    let r = codex::parse(&Output { code: Some(out.code), stdout: out.stdout, stderr: out.stderr }, None);
    assert!(matches!(r, Err(ProviderError::Exited { .. })), "{r:?}");
}

#[test]
fn gilvt_reads_the_models_fail_error_as_a_protocol_failure() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(brain::CONTROL_FILE), "models-fail\n").unwrap();
    let error = brain::model_list(Some(home.path()), None).unwrap_err();
    let r = models::parse_model_page(&json!({"id": 2, "error": error}));
    assert!(matches!(r, Err(ProviderError::Protocol(ref m)) if m.contains("model list unavailable")), "{r:?}");
}
