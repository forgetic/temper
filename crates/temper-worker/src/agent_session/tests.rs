// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::{Value, json};
use temper_protocol_activity::{
    ModelFailureBoundaryV1, ModelFailureCategoryV1, ModelFailureDispositionV1,
    ModelFailureEventKindV1,
};

fn store(root: &Path) -> AgentSessionStore {
    AgentSessionStore::for_workspace_root(root, "engineer", "pr-for-code-7")
        .expect("store")
        .with_recovery_policy(policy())
}

fn policy() -> SessionRecoveryPolicy {
    SessionRecoveryPolicy {
        session_failure_limit: 1,
        fresh_session_limit: 1,
        provider_deferral_limit: 2,
        provider_deferral_delay_ms: 100,
        recovery_slo_ms: 1_000,
    }
}

fn diagnostic(disposition: ModelFailureDispositionV1) -> ModelFailureV1 {
    match disposition {
        ModelFailureDispositionV1::Retryable => ModelFailureV1 {
            provider: "fixture-provider".to_string(),
            model: "fixture-model".to_string(),
            category: ModelFailureCategoryV1::Provider,
            disposition,
            boundary: ModelFailureBoundaryV1::Http,
            event_kind: ModelFailureEventKindV1::HttpResponse,
            status_present: true,
            code_present: true,
            retryable: true,
            http_status: Some(503),
            provider_request_id: Some("request-749".to_string()),
            provider_error_code: Some("unavailable".to_string()),
            message: "Provider failure details were redacted.".to_string(),
            detail_redacted: true,
        },
        ModelFailureDispositionV1::NonRetryable => ModelFailureV1 {
            provider: "fixture-provider".to_string(),
            model: "fixture-model".to_string(),
            category: ModelFailureCategoryV1::Authentication,
            disposition,
            boundary: ModelFailureBoundaryV1::Http,
            event_kind: ModelFailureEventKindV1::HttpResponse,
            status_present: true,
            code_present: true,
            retryable: false,
            http_status: Some(401),
            provider_request_id: Some("request-749".to_string()),
            provider_error_code: Some("invalid_api_key".to_string()),
            message: "Provider failure details were redacted.".to_string(),
            detail_redacted: true,
        },
        ModelFailureDispositionV1::Unknown => ModelFailureV1::unknown(
            "fixture-provider",
            "fixture-model",
            ModelFailureBoundaryV1::Sse,
            ModelFailureEventKindV1::StreamError,
        ),
    }
}

fn save_initial(store: &AgentSessionStore) {
    store
        .save_sync(&AgentSessionState::new("session-first"))
        .unwrap();
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
        AgentSessionLedger::new_with_policy(state, policy())
    );
    assert!(store.delete_sync().expect("delete"));
    assert!(!store.delete_sync().expect("delete missing"));
}

#[test]
fn valid_v1_and_v2_records_migrate_atomically_to_v3() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    let path = store.path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "role": "engineer",
            "coordination_key": "pr-for-code-7",
            "state": AgentSessionState::new("legacy-session"),
        }))
        .unwrap(),
    )
    .unwrap();
    let v1 = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(v1.active_session.session_id, "legacy-session");
    assert_eq!(v1.recovery_policy, policy());
    assert_eq!(read_json(&path)["version"], AGENT_SESSION_STORE_VERSION);

    let unknown = legacy_unknown_json();
    std::fs::write(&path, serde_json::to_vec_pretty(&unknown).unwrap()).unwrap();
    let v2 = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(v2.cumulative_terminal_failures, 1);
    assert_eq!(v2.current_session_number, 2);
    assert_eq!(
        v2.latest_model_failure.as_ref().unwrap().disposition,
        ModelFailureDispositionV1::Unknown,
        "legacy unclassified non-retryable evidence must migrate as unknown"
    );
    assert_eq!(
        v2.recovery_decision.as_ref().unwrap().action,
        SessionRecoveryActionV1::RotateSession
    );
    assert_eq!(read_json(&path)["version"], AGENT_SESSION_STORE_VERSION);

    // V2 retained predecessor/latest evidence after a successful reset even
    // though that evidence no longer belonged to the active failure epoch.
    // Migration keeps the bounded predecessor but projects the authoritative
    // success boundary into an empty V3 epoch.
    let mut reset_v2 = legacy_unknown_json();
    reset_v2["ledger"]["failure_epoch"] = json!(2);
    reset_v2["ledger"]["rotation_consumed"] = json!(false);
    reset_v2["ledger"]["consecutive_terminal_count"] = json!(0);
    reset_v2["ledger"]
        .as_object_mut()
        .unwrap()
        .remove("accounted_attempt_id");
    reset_v2["ledger"]
        .as_object_mut()
        .unwrap()
        .remove("recovery_decision");
    std::fs::write(&path, serde_json::to_vec_pretty(&reset_v2).unwrap()).unwrap();
    let reset = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(reset.failure_epoch, 2);
    assert_eq!(reset.cumulative_terminal_failures, 0);
    assert!(reset.latest_model_failure.is_none());
    assert!(reset.prior_session.is_some());
    assert_eq!(read_json(&path)["version"], AGENT_SESSION_STORE_VERSION);
}

#[test]
fn v2_migration_replace_failure_preserves_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path()).with_replace_failure();
    let path = store.path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = serde_json::to_vec_pretty(&legacy_unknown_json()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    assert!(store.load_ledger_sync().is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn malformed_mismatched_unsupported_and_corrupt_records_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    let path = store.path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut corrupt_v2 = legacy_unknown_json();
    corrupt_v2["ledger"]["active_session"]["session_id"] = json!("session-old");
    let cases = [
        b"{not-json".to_vec(),
        serde_json::to_vec(&json!({"version": 99})).unwrap(),
        serde_json::to_vec(&json!({
            "version": 1,
            "role": "engineer",
            "coordination_key": "pr-for-code-7",
            "state": AgentSessionState::new("legacy-session"),
            "unexpected": "must fail closed",
        }))
        .unwrap(),
        serde_json::to_vec(&json!({
            "version": 1,
            "role": "reviewer",
            "coordination_key": "other",
            "state": AgentSessionState::new("foreign-session"),
        }))
        .unwrap(),
        serde_json::to_vec(&corrupt_v2).unwrap(),
    ];
    for bytes in cases {
        std::fs::write(&path, &bytes).unwrap();
        assert!(store.load_ledger_sync().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn unknown_failures_rotate_once_then_defer_with_cumulative_counts() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);

    let rotation = store
        .account_model_failure_at_sync(
            "attempt-1",
            &diagnostic(ModelFailureDispositionV1::Unknown),
            1_000,
        )
        .unwrap();
    assert_eq!(rotation.action, SessionRecoveryActionV1::RotateSession);
    assert_eq!(rotation.failure_count, 1);
    assert_eq!(rotation.session_number, 1);
    assert_eq!(rotation.session_failure_count, 1);
    assert!(rotation.immediate_retry_exhausted);
    let second_session = rotation.new_session_id.clone().unwrap();

    let deferred = store
        .account_model_failure_at_sync(
            "attempt-2",
            &diagnostic(ModelFailureDispositionV1::Unknown),
            1_100,
        )
        .unwrap();
    assert_eq!(deferred.action, SessionRecoveryActionV1::ProviderDeferred);
    assert_eq!(deferred.failure_count, 2);
    assert_eq!(deferred.session_number, 2);
    assert_eq!(deferred.session_failure_count, 1);
    assert_eq!(deferred.deferral_count, 1);
    assert_eq!(deferred.deferral_generation, 1);
    assert_eq!(deferred.not_before_unix_ms, Some(1_200));
    assert_eq!(deferred.current_session_id, second_session);

    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(ledger.cumulative_terminal_failures, 2);
    assert_eq!(ledger.session_terminal_failures, 1);
    assert_eq!(ledger.current_session_number, 2);
    assert_eq!(ledger.fresh_sessions_used, 1);
    assert_eq!(ledger.deferral_generation, 1);
}

#[test]
fn actionable_non_retryable_failure_parks_directly_without_rotation() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);

    let parked = store
        .account_model_failure_at_sync(
            "attempt-auth",
            &diagnostic(ModelFailureDispositionV1::NonRetryable),
            1_000,
        )
        .unwrap();
    assert_eq!(parked.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(parked.new_session_id, None);
    assert!(!parked.immediate_retry_exhausted);
    let ledger = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(ledger.active_session.session_id, "session-first");
    assert_eq!(ledger.fresh_sessions_used, 0);
    assert!(ledger.prior_session.is_none());
}

#[test]
fn unknown_provider_recovery_is_bounded_by_deferral_budget_and_slo() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);
    let unknown = diagnostic(ModelFailureDispositionV1::Unknown);
    store
        .account_model_failure_at_sync("rotate", &unknown, 1_000)
        .unwrap();
    for (attempt, now, generation) in [("defer-1", 1_100, 1), ("defer-2", 1_200, 2)] {
        let decision = store
            .account_model_failure_at_sync(attempt, &unknown, now)
            .unwrap();
        assert_eq!(decision.action, SessionRecoveryActionV1::ProviderDeferred);
        assert_eq!(decision.deferral_generation, generation);
    }
    let budget_park = store
        .account_model_failure_at_sync("budget-park", &unknown, 1_300)
        .unwrap();
    assert_eq!(budget_park.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(budget_park.failure_count, 4);
    assert_eq!(budget_park.deferral_generation, 2);

    let other = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "slo-key")
        .unwrap()
        .with_recovery_policy(policy());
    other
        .save_sync(&AgentSessionState::new("slo-session"))
        .unwrap();
    other
        .account_model_failure_at_sync("slo-rotate", &unknown, 5_000)
        .unwrap();
    let slo_park = other
        .account_model_failure_at_sync("slo-park", &unknown, 6_000)
        .unwrap();
    assert_eq!(slo_park.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(slo_park.epoch_elapsed_ms, 1_000);
    assert_eq!(slo_park.slo_deadline_unix_ms, Some(6_000));

    let no_extension = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "slo-retry")
        .unwrap()
        .with_recovery_policy(SessionRecoveryPolicy {
            session_failure_limit: 2,
            fresh_session_limit: 1,
            provider_deferral_limit: 2,
            provider_deferral_delay_ms: 50,
            recovery_slo_ms: 100,
        });
    no_extension
        .save_sync(&AgentSessionState::new("slo-retry-session"))
        .unwrap();
    assert_eq!(
        no_extension
            .account_model_failure_at_sync("within-slo", &unknown, 10_000)
            .unwrap()
            .action,
        SessionRecoveryActionV1::RetryCurrentSession
    );
    let elapsed = no_extension
        .account_model_failure_at_sync("at-slo", &unknown, 10_100)
        .unwrap();
    assert_eq!(elapsed.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(elapsed.new_session_id, None, "SLO must prevent rotation");
}

#[test]
fn replay_returns_identical_decision_without_increment_or_generation_change() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);
    let unknown = diagnostic(ModelFailureDispositionV1::Unknown);
    store
        .account_model_failure_at_sync("rotate", &unknown, 1_000)
        .unwrap();
    let first = store
        .account_model_failure_at_sync("same", &unknown, 1_100)
        .unwrap();
    let before = std::fs::read(store.path()).unwrap();
    let replay = store
        .account_model_failure_at_sync(
            "same",
            &diagnostic(ModelFailureDispositionV1::NonRetryable),
            9_999,
        )
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn only_authoritative_success_clears_deferral_and_starts_a_new_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);
    let unknown = diagnostic(ModelFailureDispositionV1::Unknown);
    store
        .account_model_failure_at_sync("rotate", &unknown, 1_000)
        .unwrap();
    store
        .account_model_failure_at_sync("defer", &unknown, 1_100)
        .unwrap();

    let mut active = store.load_sync().unwrap().unwrap();
    active.state = Some(json!({"preserved": true}));
    store.save_sync(&active).unwrap();
    let unchanged = store.load_ledger_sync().unwrap().unwrap();
    assert_eq!(unchanged.deferral_generation, 1);
    assert_eq!(unchanged.cumulative_terminal_failures, 2);

    let reset = store.reset_after_success_sync().unwrap();
    assert_eq!(reset.failure_epoch, 2);
    assert_eq!(reset.cumulative_terminal_failures, 0);
    assert_eq!(reset.session_terminal_failures, 0);
    assert_eq!(reset.deferral_count, 0);
    assert_eq!(reset.deferral_generation, 0);
    assert_eq!(reset.not_before_unix_ms, None);
    assert_eq!(reset.latest_model_failure, None);
    assert!(
        reset.prior_session.is_some(),
        "bounded predecessor is retained"
    );
}

#[test]
fn overflow_and_atomic_replace_failure_leave_the_record_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);
    let rotation = store
        .account_model_failure_at_sync(
            "rotate",
            &diagnostic(ModelFailureDispositionV1::Unknown),
            1_000,
        )
        .unwrap();
    let mut ledger = store.load_ledger_sync().unwrap().unwrap();
    ledger.cumulative_terminal_failures = u32::MAX;
    ledger
        .prior_session
        .as_mut()
        .unwrap()
        .cumulative_terminal_failures = u32::MAX;
    ledger.recovery_decision.as_mut().unwrap().failure_count = u32::MAX;
    store.save_ledger_sync(&ledger).unwrap();
    let before_overflow = std::fs::read(store.path()).unwrap();
    assert!(
        store
            .account_model_failure_at_sync(
                "overflow",
                &diagnostic(ModelFailureDispositionV1::Unknown),
                1_100,
            )
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before_overflow);

    // Restore a simple valid ledger and inject the failure immediately before
    // atomic persist. The old bytes remain authoritative.
    store.delete_sync().unwrap();
    save_initial(&store);
    let before_replace = std::fs::read(store.path()).unwrap();
    let failing = store.clone().with_replace_failure();
    assert!(
        failing
            .account_model_failure_at_sync(
                "replace",
                &diagnostic(ModelFailureDispositionV1::Unknown),
                1_000,
            )
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before_replace);
    assert_eq!(rotation.failure_count, 1);
}

#[test]
fn cancellation_preserves_the_ledger_without_accounting_an_attempt() {
    temper_worker_io::block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        save_initial(&store);
        let before = std::fs::read(store.path()).unwrap();
        let cancellation = JobCancellation::default();
        cancellation.cancel();
        assert!(matches!(
            store
                .account_model_failure_controlled(
                    "cancelled",
                    &diagnostic(ModelFailureDispositionV1::Unknown),
                    &cancellation,
                )
                .await,
            Err(AgentSessionStoreError::Cancelled)
        ));
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
    });
}

#[test]
fn ledger_decisions_preserve_dirty_tracked_and_untracked_workspace_files() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    let repo = temp.path().join("engineer/pr-for-code-7/service");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("tracked.txt"), "dirty tracked\n").unwrap();
    std::fs::write(repo.join("untracked.txt"), "valuable untracked\n").unwrap();
    std::fs::write(repo.join(".git/index"), "sentinel index\n").unwrap();
    save_initial(&store);
    let unknown = diagnostic(ModelFailureDispositionV1::Unknown);
    store
        .account_model_failure_at_sync("rotate", &unknown, 1_000)
        .unwrap();
    store
        .account_model_failure_at_sync("defer", &unknown, 1_100)
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
fn corrupt_retained_predecessor_and_early_park_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(temp.path());
    save_initial(&store);
    let unknown = diagnostic(ModelFailureDispositionV1::Unknown);
    store
        .account_model_failure_at_sync("rotate", &unknown, 1_000)
        .unwrap();
    store.reset_after_success_sync().unwrap();
    let mut corrupt_history = read_json(&store.path());
    corrupt_history["ledger"]["prior_session"]["failed_attempt_id"] = json!("unsafe\nattempt");
    let bytes = serde_json::to_vec_pretty(&corrupt_history).unwrap();
    std::fs::write(store.path(), &bytes).unwrap();
    assert!(store.load_ledger_sync().is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), bytes);

    let early = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "early-park")
        .unwrap()
        .with_recovery_policy(SessionRecoveryPolicy {
            session_failure_limit: 2,
            ..policy()
        });
    early
        .save_sync(&AgentSessionState::new("early-session"))
        .unwrap();
    early
        .account_model_failure_at_sync("retry", &unknown, 2_000)
        .unwrap();
    let mut corrupt_decision = read_json(&early.path());
    corrupt_decision["ledger"]["recovery_decision"]["action"] = json!("park_for_human");
    let bytes = serde_json::to_vec_pretty(&corrupt_decision).unwrap();
    std::fs::write(early.path(), &bytes).unwrap();
    assert!(early.load_ledger_sync().is_err());
    assert_eq!(std::fs::read(early.path()).unwrap(), bytes);
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

    let first = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "key-1").unwrap();
    let second = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "key-2").unwrap();
    first
        .save_sync(&AgentSessionState::new("session-1"))
        .unwrap();
    std::fs::create_dir_all(second.path().parent().unwrap()).unwrap();
    std::fs::copy(first.path(), second.path()).unwrap();
    let bytes = std::fs::read(second.path()).unwrap();
    assert!(matches!(
        second.load_sync().unwrap_err(),
        AgentSessionStoreError::KeyMismatch { .. }
    ));
    assert_eq!(std::fs::read(second.path()).unwrap(), bytes);
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn legacy_unknown_json() -> Value {
    json!({
        "version": 2,
        "role": "engineer",
        "coordination_key": "pr-for-code-7",
        "ledger": {
            "active_session": AgentSessionState::new("session-new"),
            "prior_session": {
                "session": AgentSessionState::new("session-old"),
                "failed_attempt_id": "attempt-legacy",
                "consecutive_terminal_count": 1,
                "model_failure": {
                    "provider": "legacy-provider",
                    "model": "legacy-model",
                    "category": "redacted_unknown",
                    "retryable": false,
                    "message": "Provider failure details were redacted.",
                    "detail_redacted": true
                }
            },
            "failure_epoch": 1,
            "consecutive_terminal_count": 0,
            "rotation_consumed": true,
            "latest_model_failure": {
                "provider": "legacy-provider",
                "model": "legacy-model",
                "category": "redacted_unknown",
                "retryable": false,
                "message": "Provider failure details were redacted.",
                "detail_redacted": true
            },
            "accounted_attempt_id": "attempt-legacy",
            "recovery_decision": {
                "attempt_id": "attempt-legacy",
                "failure_epoch": 1,
                "failure_count": 1,
                "action": "rotate_session",
                "current_session_id": "session-old",
                "new_session_id": "session-new",
                "evidence_location": ".temper-agent-session/state.json"
            }
        }
    })
}
