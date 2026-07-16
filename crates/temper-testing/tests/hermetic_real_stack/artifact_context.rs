use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use temper_forge_model::{CreateIssue, Forge, UpdateIssue, UserId};
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{HermeticIssueSpec, HermeticRealStackBuilder, HermeticRepoSpec};
use temper_workflow::{ArtifactKindId, ArtifactRef, WorkflowMetadata, render_metadata_block};

#[test]
fn hermetic_real_stack_delivers_bundle_and_services_repeated_forge_reads() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let observed_bundle = Arc::new(AtomicUsize::new(0));
        let observed_bundle_for_script = Arc::clone(&observed_bundle);
        let script = Script::rule(move |view| match view.prior_tool_results {
            0 => {
                let prompt = view
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(prompt.contains("Artifact context bundle (version 1):"));
                let primary = prompt_section(&prompt, "Primary artifact:", "Mandatory lineage:");
                assert!(primary.contains("issue acme/service#1 — Plan lineage delivery"));
                assert!(primary.contains("Plan lineage boundary marker"));
                assert!(!primary.contains("Cross-repository architecture parent"));
                assert!(!primary.contains("Markdown-only background"));

                let lineage =
                    prompt_section(&prompt, "Mandatory lineage:", "Validation summaries:");
                assert!(lineage.contains("Cross-repository architecture parent"));
                assert!(lineage.contains("kind=design labels=design, ready"));
                assert!(!lineage.contains("Plan lineage delivery"));
                assert!(!lineage.contains("Markdown-only background"));

                assert_eq!(
                    prompt_section(
                        &prompt,
                        "Validation summaries:",
                        "Optional body-omitted references:",
                    ),
                    "- No validation dependencies or implementations were collected."
                );
                let optional = prompt_section(
                    &prompt,
                    "Optional body-omitted references:",
                    "Diagnostics and truncation:",
                );
                assert!(optional.contains("issue acme/service#2 — Markdown-only background"));
                assert!(optional.contains("relation=related source=issue acme/service#1"));
                assert!(!optional.contains("Cross-repository architecture parent"));
                assert!(!optional.contains("Plan lineage delivery"));
                assert!(prompt.contains("Forge context follow-up:"));
                assert!(prompt.contains("`forge_get_item`"));
                assert!(prompt.contains("`forge_list_related`"));
                observed_bundle_for_script.fetch_add(1, Ordering::SeqCst);
                tool_call(
                    "get_primary",
                    "forge_get_item",
                    serde_json::json!({
                        "repo": "acme/service",
                        "number": 1,
                        "type": "issue",
                        "include_comments": false
                    }),
                )
            }
            1 => tool_call(
                "list_parent_once",
                "forge_list_related",
                serde_json::json!({
                    "repo": "acme/service",
                    "number": 1,
                    "type": "issue",
                    "relations": ["parent"],
                    "depth": 1,
                    "limit": 10
                }),
            ),
            2 => tool_call(
                "list_parent_again",
                "forge_list_related",
                serde_json::json!({
                    "repo": "acme/service",
                    "number": 1,
                    "type": "issue",
                    "relations": ["parent"],
                    "depth": 1,
                    "limit": 10
                }),
            ),
            3 => tool_call(
                "deny_unconfigured_repo",
                "forge_get_item",
                serde_json::json!({
                    "repo": "acme/not-configured",
                    "number": 1,
                    "type": "issue",
                    "include_comments": false
                }),
            ),
            4 => {
                let transcript = view
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(transcript.contains("Plan lineage delivery"));
                assert!(transcript.contains("not_authorized"));
                tool_call(
                    "write_evidence",
                    "write",
                    serde_json::json!({
                        "path": "service/LINEAGE_E2E.md",
                        "content": "bundle and bounded reads reached the agent\n"
                    }),
                )
            }
            5 => tool_call(
                "submit_lineage_evidence",
                "submit_for_pr",
                serde_json::json!({"summary": "Verified lineage delivery and bounded reads."}),
            ),
            _ => Reply::text(
                r#"{"title":"Verify lineage delivery","body":"Bundle and bounded reads reached the native agent.","summary":"Verified lineage delivery and bounded reads."}"#,
            ),
        });

        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .add_repo(HermeticRepoSpec::new("acme", "platform"))
            .issue(HermeticIssueSpec::ready_code(
                "Plan lineage delivery",
                "Plan lineage boundary marker: preserve this legacy artifact body at the agent boundary.",
            ))
            .fake_model_script(script)
            .max_iterations(10)
            .build(&handle)
            .await
            .expect("lineage real stack builds");

        let platform_id = stack
            .repo_id("acme/platform")
            .expect("configured secondary repository")
            .clone();
        let parent = stack
            .forge()
            .create_issue(
                &platform_id,
                CreateIssue {
                    title: "Cross-repository architecture parent".to_string(),
                    body: format!(
                        "Parent context from the configured platform repository.\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            kind: Some(ArtifactKindId::new("design")),
                            ..Default::default()
                        })
                    ),
                    labels: vec!["design".to_string(), "ready".to_string()],
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .expect("cross-repository parent created");
        let markdown_reference = stack
            .forge()
            .create_issue(
                stack.primary_repo_id(),
                CreateIssue {
                    title: "Markdown-only background".to_string(),
                    body: "This body must be omitted from the initial bundle.".to_string(),
                    labels: vec!["docs".to_string()],
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .expect("markdown reference created");
        let source = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("source read succeeds")
            .expect("source exists");
        stack
            .forge()
            .update_issue(
                &source.id,
                UpdateIssue {
                    body: Some(format!(
                        "Plan lineage boundary marker: preserve this legacy artifact body at the agent boundary.\nSee #{} for background.\n{}",
                        markdown_reference.number.get(),
                        render_metadata_block(&WorkflowMetadata {
                            kind: Some(ArtifactKindId::new("code")),
                            parents: vec![ArtifactRef::in_repo(platform_id, parent.number)],
                            ..Default::default()
                        })
                    )),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("source lineage metadata updated");

        let run = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("lineage real stack completes");
        assert_eq!(run.job_result.status, ResultStatus::Success);
        assert_eq!(observed_bundle.load(Ordering::SeqCst), 1);
        let branch = &run.job_result.repos[0].branch.name;
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), branch, "LINEAGE_E2E.md")
                .expect("agent evidence file was pushed"),
            "bundle and bounded reads reached the agent\n"
        );
    });
}

fn prompt_section<'a>(prompt: &'a str, heading: &str, next_heading: &str) -> &'a str {
    prompt
        .split_once(heading)
        .unwrap_or_else(|| panic!("missing prompt heading {heading}"))
        .1
        .split_once(next_heading)
        .unwrap_or_else(|| panic!("missing following prompt heading {next_heading}"))
        .0
        .trim()
}

fn tool_call(id: &str, name: &str, args: serde_json::Value) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            args,
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}
