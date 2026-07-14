// SPDX-License-Identifier: MPL-2.0

use std::process::Command;

use serde_json::Value;

use crate::support::temper;

fn write_trace_bundle(root: &std::path::Path, include_token: bool) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n\
         [observability.agent_traces]\n\
         read_token = \"trace-reader\"\n",
    )
    .expect("config");
    let token = if include_token {
        "[secrets]\ntrace-reader = \"never-print-this-trace-token\"\n"
    } else {
        ""
    };
    std::fs::write(
        bundle.join("credentials.toml"),
        format!("schema_version = 1\n[forge.users.engineer]\ntoken = \"forge-token\"\n{token}"),
    )
    .expect("credentials");
    bundle
}

#[test]
fn config_show_reports_trace_policy_roots_and_redacted_token_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_trace_bundle(dir.path(), true);
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(&["--config", &bundle_arg, "config", "show"], dir.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("show utf8");
    assert!(text.contains("[observability.agent_traces]"), "{text}");
    assert!(text.contains("capture       = metadata"), "{text}");
    assert!(text.contains("trace-reader (available)"), "{text}");
    assert!(text.contains("transcript_queries = enabled"), "{text}");
    assert!(text.contains("agent-traces/journal"), "{text}");
    assert!(text.contains("agent-traces/worker-spool"), "{text}");
    assert!(!text.contains("never-print-this-trace-token"), "{text}");
}

#[test]
fn read_token_finding_is_engine_scoped_and_json_is_redacted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_trace_bundle(dir.path(), false);
    let bundle_arg = bundle.to_string_lossy();

    let engine = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--component",
            "engine",
        ],
        dir.path(),
    );
    assert!(!engine.status.success());
    let engine_json: Value = serde_json::from_slice(&engine.stdout).expect("engine json");
    assert!(engine_json["findings"].as_array().is_some_and(|findings| {
        findings.iter().any(|finding| {
            finding["scope"] == "secrets"
                && finding["message"].as_str().is_some_and(|message| {
                    message.contains("observability.agent_traces.read_token")
                })
        })
    }));

    let worker = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--component",
            "worker",
        ],
        dir.path(),
    );
    let worker_text = String::from_utf8(worker.stdout).expect("worker json utf8");
    assert!(
        !worker_text.contains("observability.agent_traces.read_token"),
        "{worker_text}"
    );
    assert!(!worker_text.contains("never-print-this-trace-token"));
}

#[test]
fn no_durable_state_warning_appears_in_human_and_json_check_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_trace_bundle(dir.path(), true);
    let bundle_arg = bundle.to_string_lossy().into_owned();

    let run = |format: &str| {
        Command::new(env!("CARGO_BIN_EXE_temper"))
            .args([
                "--config",
                bundle_arg.as_str(),
                "--format",
                format,
                "check",
                "--component",
                "engine",
            ])
            .env_clear()
            .output()
            .expect("run temper check without state env")
    };

    let human = run("human");
    assert!(
        human.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human_text = String::from_utf8(human.stdout).expect("human utf8");
    assert!(
        human_text.contains("agent tracing is disabled for the engine"),
        "{human_text}"
    );
    assert!(
        !human_text.contains("never-print-this-trace-token"),
        "{human_text}"
    );

    let json = run("json");
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let value: Value = serde_json::from_slice(&json.stdout).expect("json output");
    assert!(value["findings"].as_array().is_some_and(|findings| {
        findings.iter().any(|finding| {
            finding["scope"] == "engine"
                && finding["category"] == "path"
                && finding["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("agent tracing is disabled"))
        })
    }));
    assert!(!String::from_utf8_lossy(&json.stdout).contains("never-print-this-trace-token"));
}
