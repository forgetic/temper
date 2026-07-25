// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::json;
use temper_protocol_activity::ModelFailureCategoryV1;

fn store(root: &Path) -> AgentSessionStore {
    AgentSessionStore::for_workspace_root(root, "engineer", "pr-for-code-7").expect("store")
}

fn diagnostic(retryable: bool) -> ModelFailureV1 {
    ModelFailureV1 {
        provider: "fixture-provider".to_string(),
        model: "fixture-model".to_string(),
        category: ModelFailureCategoryV1::Provider,
        retryable,
        http_status: Some(503),
        provider_request_id: Some("request-749".to_string()),
        provider_error_code: Some("unavailable".to_string()),
        message: "Provider is unavailable.".to_string(),
        detail_redacted: false,
    }
}

#[test]
fn save_load_and_delete_session_by_role_and_coordination_key() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store(temp.path());
    let mut state = AgentSessionState::new("session-1");
    state.state = Some(json!({ "provider": "test" }));

    store.save_sync(&state).expect("save");
    assert_eq!(store.load_sync().expect("load"), Some(state.clone()));
    assert_eq!(
        store.load_ledger_sync().unwrap().unwrap(),
        AgentSessionLedger::new(state)
    );
    assert!(store.delete_sync().expect("delete"));
    assert_eq!(store.load_sync().expect("load after delete"), None);
    assert!(!store.delete_sync().expect("delete missing"));
}

#[test]
fn valid_v1_record_migrates_deterministically() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    let state = AgentSessionState::new("legacy-session");
    let path = store.path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "role": "engineer",
            "coordination_key": "pr-for-code-7",
            "state": state,
        }))
        .unwrap(),
    )
    .unwrap();

    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(ledger, AgentSessionLedger::new(state));
    let migrated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(migrated["version"], AGENT_SESSION_STORE_VERSION);
    assert_eq!(
        migrated["ledger"]["active_session"]["session_id"],
        "legacy-session"
    );
}

#[test]
fn missing_state_can_create_initial_ledger() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    assert_eq!(store.load_ledger_sync().unwrap(), None);
    store
        .save_sync(&AgentSessionState::new("initial-session"))
        .unwrap();
    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(ledger.failure_epoch, 1);
    assert_eq!(ledger.consecutive_terminal_count, 0);
    assert!(!ledger.rotation_consumed);
}

#[test]
fn malformed_unsupported_and_mismatched_records_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    let path = store.path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let cases = [
        (b"{not-json".to_vec(), "malformed"),
        (
            serde_json::to_vec(&json!({"version": 99})).unwrap(),
            "unsupported",
        ),
        (
            serde_json::to_vec(&json!({
                "version": 1,
                "role": "reviewer",
                "coordination_key": "other",
                "state": AgentSessionState::new("foreign-session"),
            }))
            .unwrap(),
            "mismatched",
        ),
    ];
    for (bytes, label) in cases {
        std::fs::write(&path, &bytes).unwrap();
        assert!(store.load_ledger_sync().is_err(), "{label} must fail");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "{label} was overwritten"
        );
    }
}

#[test]
fn semantically_inconsistent_v2_ledgers_fail_closed_without_changing_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();

    let read_document = || -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap()
    };
    let initial = read_document();

    store
        .account_model_failure_sync("attempt-retry", &diagnostic(true))
        .unwrap();
    let retry = read_document();

    store
        .account_model_failure_sync("attempt-rotate", &diagnostic(false))
        .unwrap();
    let rotation = read_document();

    store
        .account_model_failure_sync("attempt-park", &diagnostic(false))
        .unwrap();
    let park = read_document();

    store.reset_after_success_sync().unwrap();
    let success_reset = read_document();

    let mut cases = Vec::new();

    let mut document = initial.clone();
    document["ledger"]["consecutive_terminal_count"] = json!(1);
    cases.push(("unaccounted nonzero count", document));

    let mut document = initial;
    document["ledger"]["latest_model_failure"] = serde_json::to_value(diagnostic(true)).unwrap();
    cases.push(("initial epoch with reset-only evidence", document));

    let mut document = retry.clone();
    document["ledger"]["latest_model_failure"] = serde_json::Value::Null;
    cases.push(("accounted decision without latest diagnostic", document));

    let mut document = retry.clone();
    document["ledger"]["recovery_decision"]["failure_epoch"] = json!(2);
    cases.push(("decision from another failure epoch", document));

    let mut document = retry.clone();
    document["ledger"]["consecutive_terminal_count"] = json!(2);
    cases.push(("decision count differs from ledger", document));

    let mut document = retry.clone();
    document["ledger"]["latest_model_failure"]["retryable"] = json!(false);
    cases.push(("non-retryable diagnostic with retry action", document));

    let mut document = retry.clone();
    document["ledger"]["recovery_decision"]["new_session_id"] = json!("forged-session");
    cases.push(("retry action with a new session", document));

    let mut unrotated_success_reset = retry.clone();
    {
        let ledger = unrotated_success_reset["ledger"].as_object_mut().unwrap();
        ledger.insert("failure_epoch".to_string(), json!(2));
        ledger.insert("consecutive_terminal_count".to_string(), json!(0));
        ledger.remove("accounted_attempt_id");
        ledger.remove("recovery_decision");
    }
    unrotated_success_reset["ledger"]["latest_model_failure"]["retryable"] = json!(false);
    cases.push((
        "success reset retains impossible non-retryable evidence without a rotation",
        unrotated_success_reset,
    ));

    let mut document = retry;
    document["ledger"]["recovery_decision"]["evidence_location"] = json!("other/session.json");
    cases.push(("decision points at different evidence", document));

    let mut document = rotation.clone();
    document["ledger"]["prior_session"]["failed_attempt_id"] = json!("different-attempt");
    cases.push(("rotation attempt differs from archived failure", document));

    let mut document = rotation.clone();
    document["ledger"]["latest_model_failure"]["provider"] = json!("different-provider");
    cases.push((
        "rotation diagnostic differs from archived failure",
        document,
    ));

    let mut document = rotation.clone();
    document["ledger"]["consecutive_terminal_count"] = json!(1);
    cases.push(("rotation did not reset active-session count", document));

    let mut document = rotation;
    let prior_session_id = document["ledger"]["prior_session"]["session"]["session_id"].clone();
    document["ledger"]["active_session"]["session_id"] = prior_session_id;
    cases.push(("active and prior session ids are identical", document));

    let mut document = park.clone();
    document["ledger"]["rotation_consumed"] = json!(false);
    cases.push(("park without consumed rotation", document));

    let mut document = park.clone();
    document["ledger"]["recovery_decision"]["prior_session_id"] = json!("wrong-prior");
    cases.push(("park names a different prior session", document));

    let mut document = park.clone();
    document["ledger"]["latest_model_failure"]["retryable"] = json!(true);
    cases.push(("park before retryable budget exhaustion", document));

    let mut document = park;
    document["ledger"]["recovery_decision"]["new_session_id"] = json!("third-session");
    cases.push(("park action creates a new session", document));

    let mut document = success_reset.clone();
    document["ledger"]["consecutive_terminal_count"] = json!(1);
    cases.push(("success reset retains a nonzero count", document));

    let mut document = success_reset.clone();
    document["ledger"]["rotation_consumed"] = json!(true);
    cases.push(("success reset retains consumed rotation", document));

    let mut document = success_reset.clone();
    document["ledger"]["prior_session"]["failed_attempt_id"] = json!("unsafe\nattempt");
    cases.push(("reset history has an invalid archived attempt", document));

    let mut document = success_reset.clone();
    document["ledger"]["prior_session"]["session"]["session_id"] = json!("unsafe session id");
    cases.push(("reset history has an invalid archived session", document));

    let mut document = success_reset;
    document["ledger"]["latest_model_failure"] = serde_json::Value::Null;
    cases.push(("retained prior session without diagnostic", document));

    for (label, document) in cases {
        let bytes = serde_json::to_vec_pretty(&document).unwrap();
        std::fs::write(store.path(), &bytes).unwrap();
        assert!(
            matches!(
                store.load_ledger_sync().expect_err(label),
                AgentSessionStoreError::InvalidLedger { .. }
            ),
            "{label} must be rejected as an invalid ledger"
        );
        assert_eq!(
            std::fs::read(store.path()).unwrap(),
            bytes,
            "{label} was rewritten while being rejected"
        );
    }
}

#[test]
fn retryable_failures_retry_twice_then_rotate_once_and_park_second_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();

    for count in 1..=2 {
        let decision = store
            .account_model_failure_sync(&format!("attempt-{count}"), &diagnostic(true))
            .unwrap();
        assert_eq!(
            decision.action,
            SessionRecoveryActionV1::RetryCurrentSession
        );
        assert_eq!(decision.failure_count, count);
        assert_eq!(decision.current_session_id, "session-first");
    }
    let rotation = store
        .account_model_failure_sync("attempt-3", &diagnostic(true))
        .unwrap();
    assert_eq!(rotation.action, SessionRecoveryActionV1::RotateSession);
    assert_eq!(rotation.failure_count, 3);
    let second_session = rotation.new_session_id.clone().unwrap();
    assert_ne!(second_session, "session-first");

    for count in 1..=2 {
        let decision = store
            .account_model_failure_sync(&format!("attempt-second-{count}"), &diagnostic(true))
            .unwrap();
        assert_eq!(
            decision.action,
            SessionRecoveryActionV1::RetryCurrentSession
        );
        assert_eq!(decision.failure_count, count);
        assert_eq!(decision.current_session_id, second_session);
    }
    let parked = store
        .account_model_failure_sync("attempt-second-3", &diagnostic(true))
        .unwrap();
    assert_eq!(parked.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(parked.failure_count, 3);
    assert_eq!(parked.current_session_id, second_session);
    assert_eq!(parked.prior_session_id.as_deref(), Some("session-first"));
    assert_eq!(parked.new_session_id, None);
}

#[test]
fn non_retryable_failure_rotates_immediately_then_parks_without_third_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();

    let rotation = store
        .account_model_failure_sync("attempt-1", &diagnostic(false))
        .unwrap();
    assert_eq!(rotation.action, SessionRecoveryActionV1::RotateSession);
    let second_session = rotation.new_session_id.unwrap();
    let parked = store
        .account_model_failure_sync("attempt-2", &diagnostic(false))
        .unwrap();
    assert_eq!(parked.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(parked.current_session_id, second_session);

    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(ledger.active_session.session_id, second_session);
    assert_eq!(
        ledger.prior_session.unwrap().session.session_id,
        "session-first"
    );
}

#[test]
fn accounting_is_attempt_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();

    let first = store
        .account_model_failure_sync("attempt-same", &diagnostic(false))
        .unwrap();
    let replay = store
        .account_model_failure_sync("attempt-same", &diagnostic(true))
        .unwrap();
    assert_eq!(replay, first);
    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(
        ledger.active_session.session_id,
        first.new_session_id.unwrap()
    );
    assert_eq!(ledger.prior_session.unwrap().consecutive_terminal_count, 1);
}

#[test]
fn authoritative_success_starts_new_epoch_and_retains_bounded_history() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();
    store
        .account_model_failure_sync("attempt-1", &diagnostic(false))
        .unwrap();

    let reset = store.reset_after_success_sync().unwrap();
    assert_eq!(reset.failure_epoch, 2);
    assert_eq!(reset.consecutive_terminal_count, 0);
    assert!(!reset.rotation_consumed);
    assert!(reset.prior_session.is_some());
    assert!(reset.latest_model_failure.is_some());
    assert_eq!(reset.accounted_attempt_id, None);
    assert_eq!(reset.recovery_decision, None);

    let next = store
        .account_model_failure_sync("attempt-2", &diagnostic(false))
        .unwrap();
    assert_eq!(next.failure_epoch, 2);
    assert_eq!(next.action, SessionRecoveryActionV1::RotateSession);
}

#[test]
fn authoritative_success_allows_attempt_ids_to_be_reused_in_a_new_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();
    store
        .account_model_failure_sync("attempt-reused", &diagnostic(false))
        .unwrap();
    store.reset_after_success_sync().unwrap();

    let next = store
        .account_model_failure_sync("attempt-reused", &diagnostic(true))
        .expect("a reset epoch accounts the attempt independently");
    assert_eq!(next.failure_epoch, 2);
    assert_eq!(next.action, SessionRecoveryActionV1::RetryCurrentSession);
    assert_eq!(next.failure_count, 1);
}

#[test]
fn atomic_save_failure_does_not_publish_partial_rotation() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();
    let before = std::fs::read(store.path()).unwrap();
    let mut failing_store = store.clone();
    failing_store.fail_before_replace = true;

    assert!(
        failing_store
            .account_model_failure_sync("attempt-fail", &diagnostic(false))
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(ledger.active_session.session_id, "session-first");
    assert_eq!(ledger.consecutive_terminal_count, 0);
    assert!(!ledger.rotation_consumed);
}

#[test]
fn ledger_decisions_do_not_touch_coordination_workspace_files() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    let repo = temp.path().join("engineer/pr-for-code-7/service");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("tracked.txt"), "dirty tracked\n").unwrap();
    std::fs::write(repo.join("untracked.txt"), "valuable untracked\n").unwrap();
    std::fs::write(repo.join(".git/index"), "sentinel index\n").unwrap();
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();

    store
        .account_model_failure_sync("attempt-1", &diagnostic(false))
        .unwrap();
    store
        .account_model_failure_sync("attempt-2", &diagnostic(false))
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
        "dirty tracked\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("untracked.txt")).unwrap(),
        "valuable untracked\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".git/index")).unwrap(),
        "sentinel index\n"
    );
}

#[test]
fn session_store_uses_one_safe_path_component_and_has_no_cross_key_leakage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let escaped =
        AgentSessionStore::for_workspace_root(temp.path(), "engineer", "../../escape/nested")
            .expect("store");
    assert!(escaped.path().starts_with(temp.path()));
    assert!(
        escaped.path().ends_with(
            "engineer/%2E%2E%2F%2E%2E%2Fescape%2Fnested/.temper-agent-session/state.json"
        )
    );
    assert!(AgentSessionStore::for_workspace_root(temp.path(), "../bad", "key").is_err());
    assert!(AgentSessionStore::for_workspace_root(temp.path(), "engineer", "  ").is_err());

    let first = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "key-1")
        .expect("first store");
    let second = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "key-2")
        .expect("second store");
    first
        .save_sync(&AgentSessionState::new("session-1"))
        .expect("save first");
    assert_eq!(second.load_sync().expect("load second"), None);

    let second_path = second.path();
    std::fs::create_dir_all(second_path.parent().expect("session parent")).unwrap();
    std::fs::copy(first.path(), &second_path).unwrap();
    let bytes = std::fs::read(&second_path).unwrap();
    assert!(matches!(
        second.load_sync().expect_err("mismatch rejected"),
        AgentSessionStoreError::KeyMismatch { .. }
    ));
    assert_eq!(std::fs::read(second_path).unwrap(), bytes);
}
