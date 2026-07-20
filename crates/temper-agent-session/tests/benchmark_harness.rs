// SPDX-License-Identifier: MPL-2.0

//! End-to-end direct benchmark coverage through the real `temper-agent` process.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use temper_benchmark_cli::{
    BenchmarkModeV1, DiffArtifactV1, HarnessRunOptions, ValidationArtifactV1, run_harness,
};
use temper_protocol_agent::WorkspaceResult;

const HARNESS_TEST: &str = "harness_runner_drives_real_agent_jig_submit_gate_and_fresh_workspaces";

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) =
        temper_agent_session::dispatch_linux_supervisor_helper(std::env::args_os().skip(1))
    {
        return status;
    }

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.iter().any(|argument| argument == "--list") {
        // The custom early-main harness speaks libtest's listing protocol so
        // nextest can execute it while process-containment helper invocations
        // still bypass libtest argument parsing.
        #[cfg(target_os = "linux")]
        if !arguments.iter().any(|argument| argument == "--ignored") {
            println!("{HARNESS_TEST}: test");
        }
        return ExitCode::SUCCESS;
    }

    #[cfg(target_os = "linux")]
    harness_runner_drives_real_agent_jig_submit_gate_and_fresh_workspaces();
    ExitCode::SUCCESS
}

#[cfg(target_os = "linux")]
fn harness_runner_drives_real_agent_jig_submit_gate_and_fresh_workspaces() {
    let temporary = tempfile::tempdir().unwrap();
    let repository = temporary.path().join("fixture/repo");
    fs::create_dir_all(repository.join(".temper")).unwrap();
    fs::write(repository.join("README.md"), "# benchmark fixture\n").unwrap();
    fs::write(
        repository.join(".temper/pre-push.toml"),
        r#"version = 1

[pre_push]
required = true
cwd = "repo"

[[pre_push.commands]]
id = "output-exists"
argv = ["sh", "-c", "test -f OUTPUT.md && printf gate-ok"]
timeout_secs = 10
"#,
    )
    .unwrap();
    write_context(temporary.path());
    write_jig_script(temporary.path());
    fs::write(
        temporary.path().join("benchmark.toml"),
        r#"schema = "temper.benchmark.v1"
name = "real-agent-harness"
fixture = "fixture"
workspace_context = "context.json"
capture = "diagnostic"
validation_command_prefixes = [["sh", "-c", "test -f repo/OUTPUT.md"]]
post_run_commands = [["sh", "-c", "test -f repo/OUTPUT.md && printf post-ok"]]
jig_script = "jig.json"
repetitions = 1

[annotations]
provider_region = "loopback"
cache_warmth = "cold"
"#,
    )
    .unwrap();

    let output_dir = temporary.path().join("artifacts");
    let aggregate = run_harness(&HarnessRunOptions {
        benchmark: temporary.path().join("benchmark.toml"),
        agent_bin: Path::new(env!("CARGO_BIN_EXE_temper-agent")).to_path_buf(),
        output_dir: output_dir.clone(),
        repetitions: Some(2),
    })
    .unwrap();

    assert_eq!(aggregate.benchmark.as_deref(), Some("real-agent-harness"));
    assert_eq!(aggregate.mode, Some(BenchmarkModeV1::Harness));
    assert_eq!(aggregate.outcomes.total, 2);
    assert_eq!(aggregate.outcomes.succeeded, 2);
    assert_eq!(
        aggregate.runs[0]
            .summary
            .workspace_result
            .as_ref()
            .unwrap()
            .title
            .as_deref(),
        Some("Implement benchmark output")
    );
    assert!(
        fs::read_to_string(output_dir.join("aggregate.md"))
            .unwrap()
            .contains("not representative LLM performance")
    );

    for repetition in ["001", "002"] {
        let root = output_dir.join("repetitions").join(repetition);
        let result: WorkspaceResult =
            serde_json::from_slice(&fs::read(root.join("workspace-result.json")).unwrap()).unwrap();
        assert_eq!(result.verdict, None);
        assert_eq!(result.title.as_deref(), Some("Implement benchmark output"));

        let validation: ValidationArtifactV1 =
            serde_json::from_slice(&fs::read(root.join("validation.json")).unwrap()).unwrap();
        let proof = validation
            .accepted_submit
            .as_ref()
            .expect("accepted submit proof");
        assert!(proof.response.accepted);
        assert_eq!(proof.response.gates.len(), 1);
        assert!(proof.fingerprint_current_after_session);
        assert_eq!(validation.post_run_commands.len(), 1);
        assert_eq!(validation.post_run_commands[0].argv[0], "sh");
        assert_eq!(validation.post_run_commands[0].status, "passed");
        assert_eq!(validation.post_run_commands[0].stdout_tail, "post-ok");

        let diff: DiffArtifactV1 =
            serde_json::from_slice(&fs::read(root.join("diff.json")).unwrap()).unwrap();
        assert_eq!(diff.statistics.files_changed, 2);
        assert_eq!(diff.statistics.tracked_files, 1);
        assert_eq!(diff.statistics.untracked_files, 1);
        assert_eq!(diff.statistics.insertions, 2);
        assert_eq!(diff.statistics.deletions, 1);
        assert!(
            diff.repositories[0]
                .files
                .iter()
                .any(|file| file.path == "README.md" && file.tracked)
        );
        assert!(
            diff.repositories[0]
                .files
                .iter()
                .any(|file| file.path == "OUTPUT.md" && !file.tracked)
        );

        let run: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("run.json")).unwrap()).unwrap();
        assert_eq!(run["benchmark"]["mode"], "harness");
        assert_eq!(
            run["workspace_result"]["title"],
            "Implement benchmark output"
        );
        assert_eq!(run["validation"]["command_count"], 2);
        assert_eq!(run["validation"]["succeeded"], 2);
        assert_eq!(run["host"]["provider_region"], "loopback");
        assert_eq!(run["host"]["observed_models"][0]["provider"], "deepseek");
        assert!(run["metrics"]["model"]["calls"].as_u64().unwrap() >= 4);
        assert!(
            fs::read_to_string(root.join("trace.export.jsonl"))
                .unwrap()
                .contains("model.call.started")
        );
        assert!(
            fs::read_to_string(root.join("run.md"))
                .unwrap()
                .contains("not representative LLM performance")
        );
    }
}

fn write_context(root: &Path) {
    fs::write(
        root.join("context.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "repos": [{
                "id": "repo-1",
                "owner": "acme",
                "name": "fixture",
                "default_branch": "main",
                "dir": "repo",
                "access": "writable",
                "base_branch": "main",
                "branch_hint": "benchmark/fixture"
            }],
            "work_item": {
                "role": "engineer",
                "queue": "code_ready",
                "kind": "code",
                "target": "Issue { number: ItemNumber(1) }",
                "context": "{\"title\":\"Create OUTPUT.md\"}"
            },
            "action": "open_pr",
            "correlation_key": "real-agent-harness",
            "checkout": "writable"
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_jig_script(root: &Path) {
    fs::write(
        root.join("jig.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "sequence": [
                {
                    "turns": [{
                        "tool_call": {
                            "id": "call_update_readme",
                            "name": "write",
                            "args": {
                                "path": "repo/README.md",
                                "content": "changed benchmark fixture\n"
                            }
                        }
                    }],
                    "stop": "tool_calls"
                },
                {
                    "turns": [{
                        "tool_call": {
                            "id": "call_write_output",
                            "name": "write",
                            "args": {
                                "path": "repo/OUTPUT.md",
                                "content": "benchmark output\n"
                            }
                        }
                    }],
                    "stop": "tool_calls"
                },
                {
                    "turns": [{
                        "tool_call": {
                            "id": "call_submit_output",
                            "name": "submit_for_pr",
                            "args": {"summary": "benchmark fixture is ready"}
                        }
                    }],
                    "stop": "tool_calls"
                },
                {
                    "text": "{\"title\":\"Implement benchmark output\",\"body\":\"# Implementation report\\n\\nCreated OUTPUT.md.\",\"summary\":\"Created OUTPUT.md and passed submit gate\"}"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}
