// SPDX-License-Identifier: MPL-2.0

//! Provider payload shape helpers for Forgejo/Gitea/GitHub webhook intake.

use serde_json::Value;
use temper_forge::{ItemNumber, RepositoryPath};

use super::WebhookError;

pub(super) fn parse_repo(value: &Value) -> Result<RepositoryPath, WebhookError> {
    parse_repo_object(value.pointer("/repository"))
        .or_else(|| parse_repo_object(value.pointer("/run/repository")))
        .ok_or_else(|| WebhookError::BadPayload("payload has no repository owner/name".into()))
}

fn parse_repo_object(value: Option<&Value>) -> Option<RepositoryPath> {
    let repo = value?;
    if let Some(full) = repo.pointer("/full_name").and_then(Value::as_str) {
        if let Some((owner, name)) = full.split_once('/') {
            return Some(RepositoryPath::new(owner, name));
        }
    }

    let owner = repo
        .pointer("/owner/login")
        .or_else(|| repo.pointer("/owner/username"))
        .and_then(Value::as_str);
    let name = repo.pointer("/name").and_then(Value::as_str);
    match (owner, name) {
        (Some(owner), Some(name)) => Some(RepositoryPath::new(owner, name)),
        _ => None,
    }
}

pub(super) fn parse_item(value: &Value, event: &str) -> Option<ItemNumber> {
    value
        .pointer("/pull_request/number")
        .and_then(json_u64)
        .or_else(|| value.pointer("/issue/number").and_then(json_u64))
        .or_else(|| {
            if is_action_run_event(event) {
                action_run_payload_pr_number(value)
            } else {
                None
            }
        })
        .map(ItemNumber::new)
}

fn is_action_run_event(event: &str) -> bool {
    matches!(
        event,
        "action_run_failure" | "action_run_recover" | "action_run_success"
    )
}

pub(super) fn action_run_payload_pr_number(value: &Value) -> Option<u64> {
    action_run_event_payload(value)
        .as_ref()
        .and_then(event_payload_pr_number)
}

fn action_run_event_payload(value: &Value) -> Option<Value> {
    let payload = value.pointer("/run/event_payload")?;
    match payload {
        Value::String(raw) if !raw.trim().is_empty() => serde_json::from_str(raw).ok(),
        Value::Object(_) => Some(payload.clone()),
        _ => None,
    }
}

fn event_payload_pr_number(value: &Value) -> Option<u64> {
    value
        .pointer("/pull_request/number")
        .and_then(json_u64)
        .or_else(|| value.pointer("/number").and_then(json_u64))
}

pub(super) fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forgejo_action_run_success_repo_and_item() {
        let event_payload = serde_json::json!({
            "pull_request": { "number": 23 }
        })
        .to_string();
        let value = serde_json::json!({
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
        });

        assert_eq!(parse_repo(&value), Ok(RepositoryPath::new("ai", "temper")));
        assert_eq!(
            parse_item(&value, "action_run_success"),
            Some(ItemNumber::new(23))
        );
    }

    #[test]
    fn parses_nested_run_repository_owner_and_name() {
        let event_payload = serde_json::json!({ "number": 24 }).to_string();
        let value = serde_json::json!({
            "action": "success",
            "run": {
                "status": "success",
                "repository": {
                    "owner": { "login": "ai" },
                    "name": "temper"
                },
                "event_payload": event_payload
            }
        });

        assert_eq!(parse_repo(&value), Ok(RepositoryPath::new("ai", "temper")));
        assert_eq!(
            parse_item(&value, "action_run_success"),
            Some(ItemNumber::new(24))
        );
    }
}
