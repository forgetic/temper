// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
}

fn write_manifest_run_evidence(path: &Path, failed_assertion: bool) {
    let (assertion_status, result_status, description) = if failed_assertion {
        (
            "failed",
            "failed",
            "expected issue `source` state `open`, observed `closed`",
        )
    } else {
        (
            "passed",
            "passed",
            "source issue closed after the implementation PR merged",
        )
    };
    let artifact = json!({
        "schema": "temper.scenario.run-evidence",
        "version": 1,
        "scenario": {
            "name": "manifest-ingest",
            "source": "ephemeral",
            "source_description": "ephemeral validation bundle",
            "scenario_path": "/tmp/manifest-ingest",
            "manifest_path": "/tmp/manifest-ingest/scenario.toml",
            "runner_id": "manifest",
            "runner_selector": "runner.uses",
            "runner_selection": "runner: `manifest` selected by runner.uses",
            "tier": "live",
            "tier_description": "real Forgejo + host `forgejo-runner` CI + standalone Temper + Jig fake-LLM agents",
            "topology": {
                "kind": "single-repo-forgejo-standalone",
                "forge": "forgejo",
                "runner": "forgejo-actions-host",
                "temper": "standalone",
                "agent_model": "scripted-fake-llm"
            }
        },
        "final_state": {
            "issues": [{
                "number": 1,
                "id": "source",
                "title": "Seed issue",
                "state": "closed",
                "labels": ["code"]
            }],
            "pull_requests": [{
                "number": 1,
                "id": "implementation",
                "title": "Implement change",
                "state": "merged",
                "labels": [],
                "head_branch": "temper/impl",
                "merged_sha": "abc123"
            }],
            "repositories": [{
                "id": "service",
                "slug": "acme/service",
                "branches": [{
                    "name": "main",
                    "head_sha": "abc123",
                    "contains_engineer_diff": true
                }]
            }],
            "ci": {
                "completed_jobs": 1,
                "jobs": [{
                    "name": "ci",
                    "status": "completed",
                    "pull_request_number": 1,
                    "conclusion": "success"
                }]
            }
        },
        "provider": {
            "forgejo_url": "http://127.0.0.1:3000",
            "repo_slug": "acme/service",
            "issue_number": 1,
            "pr_number": 1,
            "head_branch": "temper/impl",
            "merged_sha": "abc123",
            "temper_binary": "target/debug/temper",
            "fake_llm_url": "http://127.0.0.1:4000"
        },
        "evidence_lines": [
            "Forgejo URL: http://127.0.0.1:3000",
            "implementation PR: #1 state=merged",
            "CI jobs: 1 completed job(s)"
        ],
        "assertions": {
            "status": assertion_status,
            "total": 1,
            "passed": if failed_assertion { 0 } else { 1 },
            "failed": if failed_assertion { 1 } else { 0 },
            "unsupported": 0,
            "results": [{
                "id": "source-finalized",
                "status": result_status,
                "description": description,
                "artifact": "issue:source",
                "details": [description]
            }]
        }
    });
    std::fs::write(path, serde_json::to_string_pretty(&artifact).unwrap()).expect("write evidence");
}

#[test]
fn validate_pr_ingests_manifest_run_evidence_without_rerunning_scenario() {
    let dir = tempfile::tempdir().expect("tempdir");
    let evidence = dir.path().join("run-evidence.json");
    write_manifest_run_evidence(&evidence, false);
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &evidence.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(
        markdown.contains("Run evidence artifact ingested; scenario run was not rerun"),
        "{markdown}"
    );
    assert!(
        markdown.contains(&format!("run evidence artifact: `{}`", evidence.display())),
        "{markdown}"
    );
    assert!(
        markdown.contains("scenario: `manifest-ingest`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("runner: `manifest` selected by runner.uses"),
        "{markdown}"
    );
    assert!(
        markdown.contains("runner evidence: implementation PR: #1 state=merged"),
        "{markdown}"
    );
    assert!(
        markdown.contains("Manifest assertion results were ingested from run evidence"),
        "{markdown}"
    );
    assert!(
        markdown.contains("manifest assertions: passed"),
        "{markdown}"
    );
    assert!(
        markdown.contains("assertion passed `source-finalized`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("execution topology: live (real Forgejo + host `forgejo-runner` CI + standalone Temper + Jig fake-LLM agents)"),
        "{markdown}"
    );
    assert!(!markdown.contains("requested scenario tier"), "{markdown}");
    assert!(
        !markdown.contains("tier accepted from evidence"),
        "{markdown}"
    );
}

#[test]
fn validate_pr_ingests_failing_assertion_results() {
    let dir = tempfile::tempdir().expect("tempdir");
    let evidence = dir.path().join("failing.run-evidence.json");
    write_manifest_run_evidence(&evidence, true);
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &evidence.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "failing assertion report should fail"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(
        markdown.contains("Manifest assertion results were ingested from run evidence"),
        "{markdown}"
    );
    assert!(
        markdown.contains("assertion failed `source-finalized`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("expected issue `source` state `open`, observed `closed`"),
        "{markdown}"
    );
    assert!(markdown.contains("- Verdict: failed"), "{markdown}");
}

#[test]
fn validate_pr_rejects_every_non_live_evidence_tier() {
    let dir = tempfile::tempdir().expect("tempdir");
    for tier in ["hermetic", "experimental"] {
        let evidence = dir.path().join(format!("{tier}.run-evidence.json"));
        write_manifest_run_evidence(&evidence, false);
        let mut artifact: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence).expect("read evidence fixture"))
                .expect("evidence JSON");
        artifact["scenario"]["tier"] = serde_json::Value::String(tier.to_string());
        std::fs::write(&evidence, serde_json::to_vec_pretty(&artifact).unwrap())
            .expect("rewrite evidence tier");

        let output = temper_scenario(&[
            "validate-pr",
            "--pr",
            "123",
            "--sha",
            "deadbeef",
            "--run-evidence",
            &evidence.to_string_lossy(),
            "--output-dir",
            &dir.path().join("reports").to_string_lossy(),
        ]);

        assert!(!output.status.success(), "tier {tier} must be rejected");
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
        assert!(
            markdown.contains(&format!(
                "run evidence scenario.tier must be `live`, got `{tier}`"
            )),
            "{markdown}"
        );
    }
}

#[test]
fn validate_pr_rejects_missing_and_malformed_run_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing-run-evidence.json");
    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &missing.to_string_lossy(),
        "--output-dir",
        &dir.path().join("reports").to_string_lossy(),
    ]);
    assert!(!output.status.success(), "missing evidence should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run evidence path does not exist"),
        "{stderr}"
    );

    let malformed = dir.path().join("malformed.run-evidence.json");
    std::fs::write(&malformed, "{not-json").expect("write malformed evidence");
    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &malformed.to_string_lossy(),
        "--output-dir",
        &dir.path().join("reports").to_string_lossy(),
    ]);
    assert!(!output.status.success(), "malformed evidence should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse run evidence artifact"), "{stderr}");
}
