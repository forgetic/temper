// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

#[cfg(target_os = "linux")]
#[test]
fn codebase_memory_recovery_is_dry_run_first_plan_bound_and_idempotent() {
    let fixture = tempfile::tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let source = fixture.path().join("trusted-source");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&source).expect("source");
    let script = fixture.path().join("fake-recovery-provider.py");
    let log = fixture.path().join("calls.jsonl");
    let state = fixture.path().join("deleted");
    let config = fixture.path().join("config.toml");
    std::fs::write(&script, FAKE_RECOVERY_PROVIDER).expect("write provider");
    std::fs::write(
        &config,
        format!(
            "schema_version = 1\n\
             [paths]\n\
             workspace_dir = {workspace:?}\n\
             [engine]\n\
             repos = [\"ai/temper\"]\n\
             roles = [\"engineer\"]\n\
             [worker]\n\
             capabilities = [\"ai/temper:engineer\"]\n\
             [agent.tools.codebase_memory]\n\
             mode = \"auto\"\n\
             command = \"python3\"\n\
             args = [\"-u\", {script:?}, {workspace:?}, {log:?}, {state:?}]\n\
             roles = [\"engineer\"]\n\
             index = \"background\"\n\
             startup_timeout_secs = 2\n\
             index_timeout_secs = 2\n\
             [agent.tools.codebase_memory.retention]\n\
             enabled = true\n\
             max_obsolete_projects = 0\n\
             max_age_days = 1\n\
             maintenance_interval_secs = 60\n\
             maintenance_timeout_secs = 5\n\
             inventory_page_size = 10\n\
             max_inventory_pages = 2\n\
             max_deletions_per_run = 2\n",
            workspace = workspace.display().to_string(),
            script = script.display().to_string(),
            log = log.display().to_string(),
            state = state.display().to_string(),
        ),
    )
    .expect("write config");

    let config_arg = config.to_string_lossy();
    let dry_run = successful_json(run(
        &[
            &config_arg,
            "maintenance",
            "codebase-memory",
            "--repository",
            "ai/temper",
        ],
        fixture.path(),
    ));
    assert_eq!(dry_run["mode"], "dry-run");
    assert_eq!(
        dry_run["retention"]["proposed"].as_array().unwrap().len(),
        1
    );
    assert!(
        dry_run["retention"]["deleted"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(calls_named(&log, "delete_project"), 0);
    let plan = dry_run["plan_id"].as_str().expect("plan id");

    let refused = run(
        &[
            &config_arg,
            "maintenance",
            "codebase-memory",
            "--apply",
            "--plan",
            &format!("sha256:{}", "0".repeat(64)),
        ],
        fixture.path(),
    );
    assert!(!refused.status.success());
    assert_eq!(calls_named(&log, "delete_project"), 0);

    let missing_source = fixture.path().join("missing-source");
    let missing_source_arg = missing_source.to_string_lossy();
    let failed_preflight = run(
        &[
            &config_arg,
            "maintenance",
            "codebase-memory",
            "--apply",
            "--plan",
            plan,
            "--repository",
            "ai/temper",
            "--rebuild-from",
            &missing_source_arg,
        ],
        fixture.path(),
    );
    assert!(!failed_preflight.status.success());
    assert_eq!(calls_named(&log, "delete_project"), 0);

    let source_arg = source.to_string_lossy();
    let applied = successful_json(run(
        &[
            &config_arg,
            "maintenance",
            "codebase-memory",
            "--apply",
            "--plan",
            plan,
            "--repository",
            "ai/temper",
            "--rebuild-from",
            &source_arg,
        ],
        fixture.path(),
    ));
    assert_eq!(applied["mode"], "apply");
    assert_eq!(applied["preflight_verified"], true);
    assert_eq!(applied["retention"]["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(applied["stable_project"]["ready"], true);
    assert_eq!(applied["stable_project"]["rebuild_completed"], true);
    assert_eq!(applied["stable_project"]["safe_probe_succeeded"], true);
    assert_eq!(calls_named(&log, "delete_project"), 1);
    assert_eq!(calls_named(&log, "index_repository"), 1);
    assert_eq!(calls_named(&log, "search_code"), 2);

    let rerun = successful_json(run(
        &[&config_arg, "maintenance", "codebase-memory"],
        fixture.path(),
    ));
    assert!(
        rerun["retention"]["proposed"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(calls_named(&log, "delete_project"), 1);
}

#[cfg(target_os = "linux")]
fn run(args: &[&str], env_root: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper"));
    command
        .arg("--config")
        .arg(args[0])
        .args(["--format", "json"])
        .args(&args[1..])
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .env_remove("CREDENTIALS_DIRECTORY");
    command.output().expect("run temper")
}

#[cfg(target_os = "linux")]
fn successful_json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON stdout")
}

#[cfg(target_os = "linux")]
fn calls_named(path: &Path, expected: &str) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call["name"] == expected)
        .count()
}

#[cfg(target_os = "linux")]
const FAKE_RECOVERY_PROVIDER: &str = r#"import json
import os
import sys

workspace, log_path, state_path = sys.argv[1:4]
candidate_path = os.path.join(workspace, "engineer", "obsolete", "temper")

def schema(properties, required=()):
    return {"type": "object", "properties": properties, "required": list(required)}

TOOLS = [
    {"name": "list_projects", "inputSchema": schema({"limit": {"type": "integer"}, "cursor": {"type": "string"}}, ["limit"])},
    {"name": "delete_project", "inputSchema": schema({"project": {"type": "string"}}, ["project"])},
    {"name": "index_status", "inputSchema": schema({"project": {"type": "string"}}, ["project"])},
    {"name": "search_code", "inputSchema": schema({"project": {"type": "string"}, "query": {"type": "string"}}, ["query"])},
    {"name": "index_repository", "inputSchema": schema({"repo_path": {"type": "string"}, "name": {"type": "string"}}, ["repo_path"])},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def result(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": {"content": [{"type": "text", "text": json.dumps(value)}], "isError": False}})

for raw in sys.stdin:
    request = json.loads(raw)
    if "id" not in request:
        continue
    request_id = request["id"]
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.9.0"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        arguments = params.get("arguments", {})
        with open(log_path, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"name": name, "arguments": arguments}, sort_keys=True) + "\n")
        if name == "list_projects":
            projects = [] if os.path.exists(state_path) else [{"project": "ephemeral-1", "repo_path": candidate_path, "updated_at_unix_secs": 1, "ownership": "temper", "estimated_bytes": 1024, "status": "stale"}]
            result(request_id, {"cache_instance_id": "cache-a", "cache_bytes": 1024 if projects else 0, "projects": projects})
        elif name == "index_status":
            project = arguments.get("project", "")
            status = "ready" if project.startswith("temper-v1-") else "stale"
            result(request_id, {"project": project, "status": status})
        elif name == "delete_project":
            with open(state_path, "w", encoding="utf-8") as handle:
                handle.write(arguments.get("project", ""))
            result(request_id, {"deleted": arguments.get("project", "")})
        elif name == "index_repository":
            result(request_id, {"project": arguments.get("name", ""), "status": "ready"})
        elif name == "search_code":
            result(request_id, {"matches": []})
        else:
            result(request_id, {})
"#;
