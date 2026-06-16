//! Process I/O behavior: environment forwarding, action execution, and the
//! timeout/exit/malformed-reply/secret-redaction failure surface.

use super::*;

use std::fs;
use std::time::Duration;

use crate::{Agent, WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION};
use temper_log::redact::REDACTED;

#[test]
fn process_agent_sends_request_filters_environment_and_executes_action() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
        let request_path = temp_path("request.json");
        let config = script_config(
        r#"cat > "$1"
if [ "${TEMPER_RUNNER_ROLE_DECISION_ALLOWED:-}" = "allowed-value" ] && [ -z "${TEMPER_RUNNER_ROLE_DECISION_BLOCKED:-}" ]; then
  printf '%s\n' '{"protocol_version":1,"action":"advance","reason":"env-ok"}'
else
  printf '%s\n' '{"protocol_version":1,"action":"no_action","reason":"env-leaked"}'
fi
"#,
        vec![request_path.to_string_lossy().into_owned()],
    )
    // The config carries resolved name→value pairs; only the allowed var is
    // forwarded. The blocked var is intentionally absent, proving the adapter
    // forwards exactly the configured environment and reads no ambient process
    // environment of its own.
    .with_env([("TEMPER_RUNNER_ROLE_DECISION_ALLOWED", "allowed-value")]);
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools(
            cx.clone(),
            "generic-agent-test",
            fixture.manifest.clone(),
            config,
            vec![bound_coding_workspace()],
        )
        .expect("process config validates");

        let changed = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect("service succeeds");

        assert!(changed);
        assert_eq!(labels(&fixture).await, vec!["done", "task"]);
        let captured: WorkflowRoleDecisionRequest =
            serde_json::from_str(&fs::read_to_string(&request_path).expect("request captured"))
                .expect("captured request parses");
        assert_eq!(
            captured.protocol_version,
            WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION
        );
        assert_eq!(captured.workflow_id, "generic-agent-test");
        assert_eq!(captured.work_item_context["role"], "banana");
        assert_eq!(
            captured.work_item_context["artifact"]["title"],
            "generic work"
        );
        let observability = captured.work_item_context["observability"]
            .as_object()
            .expect("observability context is an object");
        assert_eq!(observability["repo"], fixture.repo.to_string());
        assert_eq!(observability["role"], "banana");
        assert_eq!(observability["queue"], "todo");
        assert_eq!(observability["artifact_type"], "issue");
        assert_eq!(observability["artifact_number"], fixture.issue.number.get());
        assert_eq!(observability["artifact_kind"], "task");
        assert!(
            observability["work_item_id"]
                .as_str()
                .expect("work item id is a string")
                .contains("artifact:issue:1")
        );
        assert!(
            observability["decision_id"]
                .as_str()
                .expect("decision id is a string")
                .starts_with("decision/work-item/")
        );
        assert!(observability.get("tick_id").is_none());
        assert_eq!(captured.authorized_actions[0].action, "advance");
        assert_eq!(
            captured.available_external_tools[0].provider,
            "workspace-local"
        );
    })
}

#[test]
fn process_agent_treats_unauthorized_action_as_no_action() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
        let agent = agent(
            cx.clone(),
            fixture.manifest.clone(),
            inline_config(
                r#"printf '%s' '{"protocol_version":1,"action":"delete_everything","reason":"bad"}'"#,
            ),
        );

        let changed = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect("unauthorized action degrades to no-action");

        assert!(!changed);
        assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
    })
}

#[test]
fn process_agent_reports_timeout_exit_and_malformed_replies() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
        let cases = [
            (
                WorkflowRoleDecisionProcessConfig::new("/bin/sh")
                    .with_args(["-c".to_string(), "cat >/dev/null; sleep 1".to_string()])
                    .with_timeout(Duration::from_millis(20)),
                "timed out",
            ),
            (
                inline_config("printf 'bad news' >&2; exit 7"),
                "exited unsuccessfully",
            ),
            (
                inline_config(
                    r#"printf '%s' '{"protocol_version":1,"action":"advance"}{"protocol_version":1,"action":"advance"}'"#,
                ),
                "malformed JSON",
            ),
            (
                inline_config(
                    r#"printf '%s' '{"protocol_version":1,"action":"advance","extra":1}'"#,
                ),
                "unknown field",
            ),
            (
                inline_config(
                    r#"printf '%s' '{"protocol_version":1,"action":"advance","action":"advance"}'"#,
                ),
                "duplicate field",
            ),
            (
                inline_config(r#"printf '%s' '{"protocol_version":999,"action":"advance"}'"#),
                "version mismatch",
            ),
        ];

        for (config, expected) in cases {
            let error = agent(cx.clone(), fixture.manifest.clone(), config)
                .service(&fixture.item, &tools(&fixture))
                .await
                .expect_err("process failure is an agent error");
            assert!(
                error.to_string().contains(expected),
                "expected `{expected}` in `{error}`"
            );
        }
        assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
    })
}

#[test]
fn process_agent_redacts_secret_like_stderr() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
        let error = agent(
            cx.clone(),
            fixture.manifest.clone(),
            inline_config("printf 'token=super-secret' >&2; exit 7"),
        )
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect_err("process failure is an agent error");
        let rendered = error.to_string();

        assert!(rendered.contains(REDACTED));
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("token=super-secret"));
    })
}
