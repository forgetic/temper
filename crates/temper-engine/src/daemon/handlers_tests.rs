// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::daemon::wake_coordinator::WakeLane;
use crate::webhook::webhook_signature;
use crate::{RoleFeedMode, RoleFeedTarget, WebhookConfig};
use std::collections::BTreeMap;
use temper_forge::{ChangeKind, HintArtifactKind, ItemNumber, RepositoryId, RepositoryPath};
use temper_workflow::RoleId;

#[test]
fn verified_webhook_acks_before_wake_scan_finishes() {
    let secret = "secret";
    let body = br#"{"repository":{"full_name":"ai/temper"}}"#.to_vec();
    let signature = webhook_signature(secret, &body);
    let mut machine = DaemonMachine::new(Duration::from_secs(10), 30_000);
    machine.webhook = Some(WebhookConfig {
        secret: secret.to_string(),
        targets: vec![RoleFeedTarget {
            repo: RepositoryId::new("forgejo:ai/temper"),
            path: RepositoryPath::new("ai", "temper"),
            role: RoleId::new("engineer"),
            mode: RoleFeedMode::Wake,
        }],
    });
    let (reply, _response) = temper_engine_io::oneshot();

    let requests = machine.handle_http(
        HttpRequestData {
            method: "POST".to_string(),
            uri: "/forgejo/webhook".to_string(),
            headers: vec![
                ("x-forgejo-event".to_string(), "push".to_string()),
                ("x-forgejo-signature".to_string(), signature),
            ],
            body,
        },
        HttpResponder::from_oneshot(reply),
    );

    assert_eq!(requests.len(), 3);
    assert!(matches!(&requests[0], DaemonRequest::Log(line) if line.contains("webhook accepted")));
    assert!(matches!(
        &requests[1],
        DaemonRequest::Respond { response, .. }
            if response.status == 202 && response.body.is_empty()
    ));
    assert!(matches!(&requests[2], DaemonRequest::StartWakeTimer { .. }));
    assert_eq!(
        machine
            .wake_coordinator
            .repository_state(&RepositoryPath::new("ai", "temper"))
            .expect("configured webhook repository")
            .pending
            .len(),
        1,
        "the configured role lane owns the repository timer"
    );
}

#[test]
fn proven_heartbeat_is_acknowledged_before_suppression_accounting() {
    let secret = "secret";
    let old = r#"Prose

<!-- temper:workflow
{"lease":{"role":"engineer","worker":"worker","claimed_at":"2026-07-13T12:00:00Z","heartbeat_at":"2026-07-13T12:00:00Z","expires_at":"2026-07-13T12:05:00Z"}}
-->"#;
    let new = r#"Prose

<!-- temper:workflow
{"lease":{"role":"engineer","worker":"worker","claimed_at":"2026-07-13T12:00:00Z","heartbeat_at":"2026-07-13T12:01:00Z","expires_at":"2026-07-13T12:06:00Z"}}
-->"#;
    let body = serde_json::to_vec(&serde_json::json!({
        "action": "edited",
        "repository": {"full_name": "ai/temper"},
        "issue": {"number": 319, "body": new},
        "changes": {"body": {"from": old}}
    }))
    .unwrap();
    let signature = webhook_signature(secret, &body);
    let mut machine = DaemonMachine::new(Duration::from_secs(10), 30_000);
    machine.webhook = Some(WebhookConfig {
        secret: secret.to_string(),
        targets: Vec::new(),
    });
    let (reply, _response) = temper_engine_io::oneshot();

    let requests = machine.handle_http(
        HttpRequestData {
            method: "POST".to_string(),
            uri: "/forgejo/webhook".to_string(),
            headers: vec![
                ("x-forgejo-event".to_string(), "issues".to_string()),
                ("x-forgejo-signature".to_string(), signature),
            ],
            body,
        },
        HttpResponder::from_oneshot(reply),
    );

    assert_eq!(requests.len(), 3);
    assert!(matches!(
        &requests[1],
        DaemonRequest::Respond { response, .. } if response.status == 202
    ));
    assert!(matches!(
        &requests[2],
        DaemonRequest::Log(line) if line.contains("reason=lease_heartbeat")
    ));
    assert!(!requests.iter().any(|request| matches!(
        request,
        DaemonRequest::StartWakeTimer { .. } | DaemonRequest::RunWake { .. }
    )));
}

#[test]
fn saturation_log_request_and_state_share_configured_value_and_waiting_order() {
    let mut machine = DaemonMachine::with_role_limits(
        BTreeMap::from([("engineer".to_string(), 0)]),
        Duration::ZERO,
        30_000,
    );
    machine.handle_enqueue(
        "job-1".to_string(),
        "engineer".to_string(),
        "forgejo:acme/widgets".to_string(),
        Artifact {
            item: serde_json::json!(41),
            kind: "issue".to_string(),
        },
        serde_json::json!({}),
    );
    let requests = machine.handle_enqueue(
        "job-2".to_string(),
        "engineer".to_string(),
        "acme/api".to_string(),
        Artifact {
            item: serde_json::json!(9),
            kind: "pull_request".to_string(),
        },
        serde_json::json!({}),
    );

    let (log_concurrency, log_waiting) = requests
        .iter()
        .find_map(|request| match request {
            DaemonRequest::RoleSaturated {
                concurrency,
                waiting,
                ..
            } => Some((*concurrency, waiting.clone())),
            _ => None,
        })
        .expect("zero limit emits role saturation");
    let snapshot = DaemonStateSnapshot::from_core(&machine.core);
    let state = snapshot
        .role_saturation
        .iter()
        .find(|state| state.role == "engineer")
        .expect("zero limit appears in state");

    assert_eq!(log_concurrency, 0);
    assert_eq!(state.concurrency, 0);
    assert_eq!(
        log_waiting,
        vec!["acme/widgets#41".to_string(), "acme/api PR#9".to_string()]
    );
    assert_eq!(state.waiting, log_waiting);
}

#[test]
fn forgejo_action_run_success_webhook_is_accepted() {
    let secret = "secret";
    let event_payload = serde_json::json!({
        "pull_request": { "number": 23 }
    })
    .to_string();
    let body = serde_json::to_vec(&serde_json::json!({
        "action": "success",
        "run": {
            "id": 706,
            "status": "success",
            "started": "2026-06-29T16:48:49+01:00",
            "stopped": "2026-06-29T16:51:00+01:00",
            "repository": { "full_name": "ai/temper" },
            "event_payload": event_payload
        },
        "prior_status": "running"
    }))
    .expect("body serializes");
    let signature = webhook_signature(secret, &body);
    let mut machine = DaemonMachine::new(Duration::from_secs(10), 30_000);
    machine.webhook = Some(WebhookConfig {
        secret: secret.to_string(),
        targets: vec![RoleFeedTarget {
            repo: RepositoryId::new("forgejo:ai/temper"),
            path: RepositoryPath::new("ai", "temper"),
            role: RoleId::new("engineer"),
            mode: RoleFeedMode::Wake,
        }],
    });
    let (reply, _response) = temper_engine_io::oneshot();

    let requests = machine.handle_http(
        HttpRequestData {
            method: "POST".to_string(),
            uri: "/forgejo/webhook".to_string(),
            headers: vec![
                (
                    "x-forgejo-event".to_string(),
                    "action_run_success".to_string(),
                ),
                ("x-forgejo-signature".to_string(), signature),
            ],
            body,
        },
        HttpResponder::from_oneshot(reply),
    );

    assert_eq!(requests.len(), 3);
    assert!(matches!(
        &requests[1],
        DaemonRequest::Respond { response, .. }
            if response.status == 202 && response.body.is_empty()
    ));
    assert!(matches!(&requests[2], DaemonRequest::StartWakeTimer { .. }));
    let state = machine
        .wake_coordinator
        .repository_state(&RepositoryPath::new("ai", "temper"))
        .expect("configured webhook repository");
    let lane = WakeLane::Role(RoleId::new("engineer"));
    let scope = state
        .pending
        .scope(&lane)
        .expect("role lane receives CI target");
    assert_eq!(
        scope.targets(),
        Some(&BTreeMap::from([(
            (HintArtifactKind::PullRequest, ItemNumber::new(23)),
            ChangeKind::Ci,
        )]))
    );
}
