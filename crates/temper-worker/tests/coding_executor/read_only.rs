// SPDX-License-Identifier: MPL-2.0

use super::support::target_branch::seed_feature_branch;
use super::support::*;

#[test]
fn read_only_job_returns_verdict_and_body() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context(
                    "triage-7",
                    read_only_job_context("agent/triage-7", "triage-7"),
                ))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(body.as_deref(), Some("rewritten"));
        assert_eq!(summary.as_deref(), Some("did triage"));
        assert!(children.is_empty());
        assert_no_origin_branch(&fixture, "agent/triage-7");
    });
}

#[test]
fn read_only_job_materializes_missing_target_base_without_head_push() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/plan-7";
        let main_head = git_output([
            "-C",
            path_str(&fixture.origin),
            "rev-parse",
            "refs/heads/main",
        ]);
        let agent = AgentBehavior::ReadOnlyVerdict.runner();
        let executor = fixture.executor(agent.clone(), true);
        let context =
            read_only_job_context("agent/plan-7", "plan-7").with_base_branch(target_branch);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context("plan-7", context))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(body.as_deref(), Some("rewritten"));
        assert_eq!(summary.as_deref(), Some("did triage"));
        assert!(children.is_empty());
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                &format!("refs/heads/{target_branch}"),
            ]),
            main_head,
            "missing target branch should be created from the default branch"
        );
        assert_origin_branch_exists(&fixture, target_branch);
        assert_prepared_read_only_checkout(&fixture, "plan-7", target_branch, &main_head, &agent);
        assert_no_origin_branch(&fixture, "agent/plan-7");
    });
}

#[test]
fn read_only_job_uses_existing_target_without_quarantine_or_reset() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/existing-plan-7";
        let target_head = seed_feature_branch(&fixture, "acme/service", target_branch);
        let agent = AgentBehavior::ReadOnlyVerdict.runner();
        let executor = fixture.executor(agent.clone(), true);
        let context = read_only_job_context("agent/existing-plan-7", "existing-plan-7")
            .with_base_branch(target_branch);

        let (verdict, _, _, _) = expect_verdict(
            executor
                .execute(assign_with_context("existing-plan-7", context))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                &format!("refs/heads/{target_branch}"),
            ]),
            target_head,
            "materialization must not reset an existing target branch"
        );
        assert_prepared_read_only_checkout(
            &fixture,
            "existing-plan-7",
            target_branch,
            &target_head,
            &agent,
        );
        assert_no_origin_branch(&fixture, "agent/existing-plan-7");
    });
}

#[test]
fn read_only_job_with_diff_still_returns_verdict_without_push() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdictWithDiff.runner(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context(
                    "triage-with-diff-7",
                    read_only_job_context("agent/triage-with-diff-7", "triage-with-diff-7"),
                ))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(body.as_deref(), Some("rewritten"));
        assert_eq!(summary.as_deref(), Some("did triage"));
        assert!(children.is_empty());
        assert_no_origin_branch(&fixture, "agent/triage-with-diff-7");
        assert_workspace_clean(&fixture, "architect", "triage-with-diff-7");
    });
}

#[test]
fn workflow_native_validator_resolves_exact_mapping_and_derives_verdict() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let source_branch = "feature/778-exact-head-validation";
        let expected_head = seed_native_validation_branch(&fixture, source_branch);
        let command = native_validator_command(&fixture, false);
        let executor = fixture
            .executor(ForbiddenValidationAgent, true)
            .with_native_validator_command(command);
        let context = native_validation_job_context(
            "agent/validation-7",
            "native-validation-7",
            source_branch,
        );

        let outcome = executor
            .execute(assign_with_context("native-validation-7", context))
            .await;
        let JobOutcome::Verdict {
            verdict,
            title,
            body,
            details,
            ..
        } = outcome
        else {
            panic!("expected native validation verdict, got {outcome:?}");
        };
        assert_eq!(verdict, "validated");
        assert_eq!(
            title.as_deref(),
            Some(format!("Land validated feature head {}", &expected_head[..12]).as_str())
        );
        assert!(body.is_some_and(|body| body.contains("# Validation report")));
        let evidence = details
            .as_ref()
            .and_then(|details| details.get("validator_result"))
            .expect("worker-owned typed evidence");
        assert_eq!(evidence["exact_head_sha"], expected_head);
        assert_eq!(evidence["mapping_id"], "acme/service#778:exact-head-proof");
        assert_workspace_clean(&fixture, "tester", "native-validation-7");
    });
}

#[test]
fn workflow_native_validator_rejects_and_cleans_checkout_mutation() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let source_branch = "feature/778-exact-head-validation";
        seed_native_validation_branch(&fixture, source_branch);
        let command = native_validator_command(&fixture, true);
        let executor = fixture
            .executor(ForbiddenValidationAgent, true)
            .with_native_validator_command(command);
        let context = native_validation_job_context(
            "agent/validation-mutated-7",
            "native-validation-mutated-7",
            source_branch,
        );

        let outcome = executor
            .execute(assign_with_context("native-validation-mutated-7", context))
            .await;
        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("mutated its read-only checkout"),
            "{message}"
        );
        assert_workspace_clean(&fixture, "tester", "native-validation-mutated-7");
    });
}

#[test]
fn workflow_native_validator_routes_failed_required_assertion_without_agent() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let source_branch = "feature/778-exact-head-validation";
        seed_native_validation_branch(&fixture, source_branch);
        let command = native_validator_command_with(&fixture, false, "failed", "failed", 1);
        let executor = fixture
            .executor(ForbiddenValidationAgent, true)
            .with_native_validator_command(command);
        let context = native_validation_job_context(
            "agent/validation-failed-7",
            "native-validation-failed-7",
            source_branch,
        );

        let JobOutcome::Verdict {
            verdict, children, ..
        } = executor
            .execute(assign_with_context("native-validation-failed-7", context))
            .await
        else {
            panic!("failed native validation should route a verdict");
        };
        assert_eq!(verdict, "needs_followup");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].kind.as_deref(), Some("code"));
        assert_workspace_clean(&fixture, "tester", "native-validation-failed-7");
    });
}

#[derive(Clone, Copy)]
struct ForbiddenValidationAgent;

impl AgentRunner for ForbiddenValidationAgent {
    async fn run(
        &self,
        _job_id: &str,
        _context: &WorkspaceContext,
        _cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        panic!("bound workflow-native validation must not invoke a model agent")
    }
}

fn native_validator_command(
    fixture: &Fixture,
    mutate: bool,
) -> temper_worker::NativeValidatorCommand {
    native_validator_command_with(fixture, mutate, "passed", "satisfied", 0)
}

fn native_validator_command_with(
    fixture: &Fixture,
    mutate: bool,
    verdict: &str,
    assertion_status: &str,
    exit_code: i32,
) -> temper_worker::NativeValidatorCommand {
    let script = fixture
        .workspace_root
        .parent()
        .expect("fixture root")
        .join(if mutate {
            "mutating-native-validator.py"
        } else {
            "native-validator.py"
        });
    fs::write(
        &script,
        format!(
            r##"import hashlib
import json
import os
from pathlib import Path

root = Path.cwd()
out = Path(os.environ["TEMPER_VALIDATION_OUTPUT"])
out.mkdir(parents=True, exist_ok=True)
binary = root / "validator-bin"
data = binary.read_bytes()
if {mutate}:
    (root / "validator-mutation.txt").write_text("forbidden\n")
plan = int(os.environ["TEMPER_VALIDATION_PLAN"].rsplit("#", 1)[1])
payload = {{
    "schema": "temper.validator.result.v2",
    "target": {{
        "kind": "plan",
        "repo": os.environ["TEMPER_VALIDATION_REPO"],
        "ref": {{"issue_number": plan}}
    }},
    "verdict": "{verdict}",
    "feature": os.environ["TEMPER_VALIDATION_FEATURE"],
    "plan": os.environ["TEMPER_VALIDATION_PLAN"],
    "mapping_id": os.environ["TEMPER_VALIDATION_MAPPING"],
    "scenario_name": os.environ["TEMPER_VALIDATION_SCENARIO_NAME"],
    "scenario_path": os.environ["TEMPER_VALIDATION_SCENARIO_PATH"],
    "source_branch": os.environ["TEMPER_VALIDATION_SOURCE_BRANCH"],
    "exact_head_sha": os.environ["TEMPER_VALIDATION_HEAD"],
    "resolved_content_digest": os.environ["TEMPER_VALIDATION_CONTENT_DIGEST"],
    "standalone_binary": {{
        "path": "validator-bin",
        "sha256": hashlib.sha256(data).hexdigest(),
        "size_bytes": len(data)
    }},
    "duration_ms": 25,
    "retained_paths": ["validator-bin"],
    "acceptance_criteria": [{{
        "description": "The mapped exact-head scenario passed.",
        "status": "{assertion_status}",
        "evidence_refs": ["scenario-run"]
    }}],
    "evidence": [{{
        "id": "scenario-run",
        "kind": "scenario_run",
        "summary": "The mapped live scenario completed with structured facts.",
        "artifact_path": "validator-bin"
    }}]
}}
(out / "validator-result.json").write_text(json.dumps(payload))
raise SystemExit({exit_code})
"##,
            mutate = if mutate { "True" } else { "False" },
            verdict = verdict,
            assertion_status = assertion_status,
            exit_code = exit_code,
        ),
    )
    .expect("write native validator fixture");
    temper_worker::NativeValidatorCommand::new("python3", [script.as_os_str()])
}

fn seed_native_validation_branch(fixture: &Fixture, branch: &str) -> String {
    let seed = tempfile::tempdir().expect("validation seed");
    let checkout = seed.path().join("service");
    git_output(["clone", path_str(&fixture.origin), path_str(&checkout)]);
    git_output(["-C", path_str(&checkout), "checkout", "-b", branch]);
    let scenario = checkout.join("scenarios/exact-head-proof");
    fs::create_dir_all(scenario.join("jig")).expect("scenario directory");
    fs::write(
        scenario.join("scenario.toml"),
        format!(
            "schema = \"temper.scenario.v1\"\nname = \"exact-head-proof\"\nstatus = \"active\"\nstability = \"provisional\"\nintent = \"Prove exact-head validation.\"\n\n[runner]\nuses = \"manifest\"\n\n[validation]\nfeature = \"acme/service#778\"\nplan = \"acme/service#7\"\nsource_branch = \"{branch}\"\nchange = \"new\"\n\n[feature_contract]\nclaim = \"Exact evidence gates landing.\"\nstimulus = \"Run mapped validation.\"\nobservable = \"Structured evidence.\"\nassertion = \"Required evidence passes.\"\nruntime_budget_seconds = 600\njig_script_path = \"jig/exact-head-proof.json\"\n"
        ),
    )
    .expect("scenario manifest");
    fs::write(scenario.join("jig/exact-head-proof.json"), "{}\n").expect("jig script");
    fs::write(checkout.join("validator-bin"), "binary-from-feature-head\n")
        .expect("validator binary");
    git_output(["-C", path_str(&checkout), "add", "."]);
    git_output([
        "-C",
        path_str(&checkout),
        "-c",
        "user.name=Validator Seed",
        "-c",
        "user.email=validator@example.test",
        "commit",
        "-m",
        "add exact-head proof",
    ]);
    let head = git_output(["-C", path_str(&checkout), "rev-parse", "HEAD"]);
    git_output(["-C", path_str(&checkout), "push", "origin", branch]);
    head
}

#[test]
fn read_only_breakdown_verdict_passes_children_through() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyBreakdownVerdict.runner(), true);
        let mut context = read_only_job_context("agent/breakdown-7", "breakdown-7");
        context.allowed_verdicts = vec!["needs_breakdown".to_string()];

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context("breakdown-7", context))
                .await,
        );

        assert_eq!(verdict, "needs_breakdown");
        assert_eq!(body, None);
        assert_eq!(summary.as_deref(), Some("planned breakdown"));
        assert_eq!(
            children,
            vec![
                JobChild {
                    slug: "api-schema".to_string(),
                    title: "Define the API schema".to_string(),
                    body: "Write the shared API schema.".to_string(),
                    kind: None,
                    labels: vec!["code".to_string(), "ready".to_string()],
                    depends_on: Vec::new(),
                    target_repo: None,
                },
                JobChild {
                    slug: "web-client".to_string(),
                    title: "Implement the web client".to_string(),
                    body: "Build the web client against the API schema.".to_string(),
                    kind: None,
                    labels: Vec::new(),
                    depends_on: vec!["api-schema".to_string()],
                    target_repo: Some("acme/other".to_string()),
                },
            ]
        );
        assert_no_origin_branch(&fixture, "agent/breakdown-7");
    });
}

#[test]
fn worker_rejects_agent_result_that_violates_verdict_contract() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);
        let mut context = read_only_job_context("agent/contract-7", "contract-7");
        context.verdict_contracts.insert(
            "ready_code".to_string(),
            temper_verdict::VerdictContract {
                min_children: 1,
                allowed_child_kinds: vec!["code".to_string()],
                ..Default::default()
            },
        );

        let outcome = executor
            .execute(assign_with_context("contract-7", context))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Protocol);
        assert!(message.contains("violates its workflow verdict contract"));
        assert!(message.contains("requires at least 1 child product(s), received 0"));
        assert_no_origin_branch(&fixture, "agent/contract-7");
    });
}

#[test]
fn worker_rejects_child_missing_required_workflow_metadata() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyBreakdownVerdict.runner(), true);
        let mut context = read_only_job_context("agent/metadata-7", "metadata-7");
        context.allowed_verdicts = vec!["needs_breakdown".to_string()];
        context.verdict_contracts.insert(
            "needs_breakdown".to_string(),
            temper_verdict::VerdictContract {
                min_children: 1,
                allowed_child_kinds: vec!["code".to_string()],
                required_child_metadata: vec!["target_branch".to_string()],
                ..Default::default()
            },
        );

        let outcome = executor
            .execute(assign_with_context("metadata-7", context))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Protocol);
        assert!(message.contains("workflow metadata `target_branch`"));
        assert_no_origin_branch(&fixture, "agent/metadata-7");
    });
}

#[test]
fn read_only_job_without_verdict_is_permanent() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

        let outcome = executor
            .execute(assign_with_context(
                "triage-no-verdict-7",
                read_only_job_context("agent/triage-no-verdict-7", "triage-no-verdict-7"),
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("read-only job returned no verdict"),
            "unexpected message: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/triage-no-verdict-7");
    });
}

#[test]
fn read_only_job_with_undeclared_verdict_is_permanent() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::UndeclaredVerdict.runner(), true);

        let outcome = executor
            .execute(assign_with_context(
                "triage-undeclared-7",
                read_only_job_context("agent/triage-undeclared-7", "triage-undeclared-7"),
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("needs_breakdown"),
            "message should name the emitted verdict: {message}"
        );
        assert!(
            message.contains("ready_code") && message.contains("needs_design"),
            "message should name the allowed vocabulary: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/triage-undeclared-7");
    });
}

fn assert_prepared_read_only_checkout(
    fixture: &Fixture,
    coordination_key: &str,
    expected_branch: &str,
    expected_head: &str,
    agent: &FakeAgentRunner,
) {
    let checkout = fixture
        .workspace_root
        .join("architect")
        .join(coordination_key)
        .join("service");
    assert_eq!(
        git_output(["-C", path_str(&checkout), "branch", "--show-current"]),
        expected_branch
    );
    assert_eq!(
        git_output(["-C", path_str(&checkout), "rev-parse", "HEAD"]),
        expected_head
    );
    assert_eq!(
        agent.observed_head_sha(),
        expected_head,
        "agent should start"
    );
    assert_workspace_clean(fixture, "architect", coordination_key);
    assert!(
        !checkout
            .with_file_name("service.temper-quarantine")
            .exists(),
        "fresh checkout must not be quarantined"
    );
}
