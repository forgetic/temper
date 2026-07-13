use serde_json::json;
use temper_forge::ItemNumber;

use super::*;

fn target(kind: HintArtifactKind, number: u64) -> HintTarget {
    HintTarget::Artifact {
        kind,
        number: ItemNumber::new(number),
    }
}

#[test]
fn payload_families_preserve_artifact_namespace() {
    let cases = [
        (
            "issues",
            json!({"issue":{"number":1}}),
            target(HintArtifactKind::Issue, 1),
            ChangeKind::Edited,
        ),
        (
            "pull_request",
            json!({"pull_request":{"number":2}}),
            target(HintArtifactKind::PullRequest, 2),
            ChangeKind::Edited,
        ),
        (
            "issue_comment",
            json!({"issue":{"number":3}}),
            target(HintArtifactKind::Issue, 3),
            ChangeKind::Comment,
        ),
        (
            "issue_comment",
            json!({"issue":{"number":4,"pull_request":{}}}),
            target(HintArtifactKind::PullRequest, 4),
            ChangeKind::Comment,
        ),
        (
            "pull_request_comment",
            json!({"issue":{"number":4}}),
            target(HintArtifactKind::PullRequest, 4),
            ChangeKind::Comment,
        ),
        (
            "pull_request_review",
            json!({"pull_request":{"number":5}}),
            target(HintArtifactKind::PullRequest, 5),
            ChangeKind::Review,
        ),
        (
            "status",
            json!({"pull_request":{"number":6}}),
            target(HintArtifactKind::PullRequest, 6),
            ChangeKind::Ci,
        ),
        (
            "status",
            json!({"commit":{"sha":"abc"}}),
            HintTarget::Repository,
            ChangeKind::Ci,
        ),
        (
            "push",
            json!({"ref":"refs/heads/main"}),
            HintTarget::Repository,
            ChangeKind::Push,
        ),
        (
            "issue_dependency",
            json!({"issue":{"number":7}}),
            target(HintArtifactKind::Issue, 7),
            ChangeKind::Dependency,
        ),
        (
            "mystery",
            json!({"issue":{"number":8}}),
            HintTarget::Repository,
            ChangeKind::Unknown,
        ),
        (
            "comment",
            json!({"issue":{"number":9},"pull_request":{"number":10}}),
            HintTarget::Repository,
            ChangeKind::Unknown,
        ),
    ];

    for (event, mut value, expected_target, expected_change) in cases {
        value
            .as_object_mut()
            .unwrap()
            .insert("repository".to_string(), json!({"full_name":"ai/temper"}));
        let hint = parse_change_hint(&serde_json::to_vec(&value).unwrap(), event).unwrap();
        assert_eq!(hint.target, expected_target, "event {event}");
        assert_eq!(hint.change, expected_change, "event {event}");
    }
}

fn workflow_body(heartbeat: &str, expires: &str, target_branch: &str, prose: &str) -> String {
    format!(
        "{prose}\n\n<!-- temper:workflow\n{{\"kind\":\"code\",\"target_branch\":\"{target_branch}\",\"lease\":{{\"role\":\"engineer\",\"worker\":\"worker\",\"claimed_at\":\"2026-07-13T12:00:00Z\",\"heartbeat_at\":\"{heartbeat}\",\"expires_at\":\"{expires}\"}}}}\n-->"
    )
}

fn verified(event: &str, value: Value) -> VerifiedWebhook {
    let body = serde_json::to_vec(&value).unwrap();
    let secret = "secret";
    let headers = BTreeMap::from([
        ("x-forgejo-event".to_string(), event.to_string()),
        (
            "x-forgejo-signature".to_string(),
            webhook_signature(secret, &body),
        ),
    ]);
    parse_verified_webhook(&headers, &body, secret).unwrap()
}

fn edited_issue(old_body: Option<&str>, new_body: Option<&str>, extra_change: bool) -> Value {
    let mut changes = serde_json::Map::new();
    changes.insert(
        "body".to_string(),
        old_body.map_or(Value::Null, |old| json!({"from":old})),
    );
    if extra_change {
        changes.insert("title".to_string(), json!({"from":"old"}));
    }
    json!({
        "action":"edited",
        "repository":{"full_name":"ai/temper"},
        "sender":{"login":"temper-bot"},
        "issue":{"number":319,"body":new_body},
        "changes":changes
    })
}

#[test]
fn suppresses_only_proven_heartbeat_body_delta() {
    let old = workflow_body(
        "2026-07-13T12:00:00Z",
        "2026-07-13T12:05:00Z",
        "feature/base",
        "Prose",
    );
    let heartbeat = workflow_body(
        "2026-07-13T12:01:00Z",
        "2026-07-13T12:06:00Z",
        "feature/base",
        "Prose",
    );
    let extra_metadata = workflow_body(
        "2026-07-13T12:01:00Z",
        "2026-07-13T12:06:00Z",
        "feature/changed",
        "Prose",
    );
    let prose_edit = workflow_body(
        "2026-07-13T12:01:00Z",
        "2026-07-13T12:06:00Z",
        "feature/base",
        "Edited prose",
    );

    let cases = [
        (
            edited_issue(Some(&old), Some(&heartbeat), false),
            WebhookDisposition::SuppressHeartbeat,
        ),
        (
            edited_issue(Some(&old), Some(&extra_metadata), false),
            WebhookDisposition::Schedule,
        ),
        (
            edited_issue(Some(&old), Some(&prose_edit), false),
            WebhookDisposition::Schedule,
        ),
        (
            edited_issue(Some(&old), Some(&heartbeat), true),
            WebhookDisposition::Schedule,
        ),
        (
            edited_issue(None, Some(&heartbeat), false),
            WebhookDisposition::Schedule,
        ),
        (
            edited_issue(Some("malformed"), Some(&heartbeat), false),
            WebhookDisposition::Schedule,
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(verified("issues", value).disposition, expected);
    }

    let bot_label = json!({
        "action":"labeled",
        "repository":{"full_name":"ai/temper"},
        "sender":{"login":"temper-bot"},
        "issue":{"number":319},
        "label":{"name":"ready"}
    });
    let bot_state = json!({
        "action":"closed",
        "repository":{"full_name":"ai/temper"},
        "sender":{"login":"temper-bot"},
        "issue":{"number":319,"state":"closed"}
    });
    assert_eq!(
        verified("issues", bot_label).disposition,
        WebhookDisposition::Schedule
    );
    assert_eq!(
        verified("issues", bot_state).disposition,
        WebhookDisposition::Schedule
    );
}

#[test]
fn webhook_accepted_log_line_includes_typed_target() {
    let hint = ChangeHint::pull_request(
        RepositoryPath::new("ai", "temper"),
        ItemNumber::new(147),
        ChangeKind::Review,
    );

    assert_eq!(
        webhook_accepted_log_line(&hint),
        "engine: webhook accepted repo=ai/temper target=pull_request#147 change=Review"
    );
}

#[test]
fn webhook_wake_scan_log_line_includes_enqueued_count() {
    let repo = RepositoryPath::new("ai", "temper");

    assert_eq!(
        webhook_wake_scan_log_line(&repo, 3),
        "engine: webhook wake scan repo=ai/temper enqueued=3"
    );
}
