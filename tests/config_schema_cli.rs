// SPDX-License-Identifier: MPL-2.0

use std::process::{Command, Output};

use serde_json::{Value, json};

fn temper(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("tempdir");
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
        .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
        .env("HOME", dir.path().join("home"))
        .output()
        .expect("run temper")
}

fn parse_schema(output: Output) -> Value {
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    serde_json::from_str(&stdout).expect("valid JSON schema")
}

#[test]
fn config_schema_default_prints_valid_json_with_current_sections() {
    let schema = parse_schema(temper(&["config", "schema"]));

    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["schema_version"]));

    let properties = &schema["properties"];
    assert_eq!(properties["schema_version"]["type"], "integer");
    assert_eq!(properties["schema_version"]["const"], 1);

    for section in [
        "deployment",
        "workflow",
        "paths",
        "forge",
        "engine",
        "worker",
        "agent",
    ] {
        assert_eq!(properties[section]["type"], "object", "{section}");
        assert_eq!(
            properties[section]["additionalProperties"], false,
            "{section} should reject unknown fields"
        );
    }

    assert_eq!(
        properties["deployment"]["properties"]["name"]["type"],
        "string"
    );
    assert_eq!(
        properties["deployment"]["properties"]["topology"]["enum"],
        json!(["standalone", "distributed"])
    );
    assert_eq!(
        properties["workflow"]["properties"]["file"]["type"],
        "string"
    );
    assert_eq!(
        properties["paths"]["properties"]["state_dir"]["type"],
        "string"
    );
    assert_eq!(
        properties["paths"]["properties"]["workspace_dir"]["type"],
        "string"
    );

    assert_eq!(properties["forge"]["properties"]["url"]["type"], "string");
    assert_eq!(properties["engine"]["properties"]["repos"]["type"], "array");
    assert_eq!(
        properties["engine"]["properties"]["repos"]["items"]["type"],
        "string"
    );
    assert_eq!(
        properties["worker"]["properties"]["capabilities"]["items"]["type"],
        "string"
    );
    let pools = &properties["worker"]["properties"]["pools"];
    assert_eq!(pools["type"], "array");
    assert_eq!(pools["items"]["type"], "object");
    assert_eq!(pools["items"]["additionalProperties"], false);
    assert_eq!(pools["items"]["properties"]["name"]["type"], "string");
    assert_eq!(
        pools["items"]["properties"]["roles"]["items"]["type"],
        "string"
    );
    assert_eq!(
        pools["items"]["properties"]["repos"]["items"]["type"],
        "string"
    );
    assert_eq!(
        pools["items"]["properties"]["max_concurrent_jobs"]["minimum"],
        1
    );
    assert_eq!(
        pools["items"]["properties"]["agent_profile"]["type"],
        "string"
    );
    assert_eq!(
        pools["items"]["properties"]["worker_token"]["type"],
        "string"
    );
}

#[test]
fn config_schema_json_format_prints_same_machine_readable_schema() {
    let default_schema = parse_schema(temper(&["config", "schema"]));
    let json_schema = parse_schema(temper(&["--format", "json", "config", "schema"]));

    assert_eq!(json_schema, default_schema);

    let providers = &json_schema["properties"]["agent"]["properties"]["providers"];
    assert_eq!(providers["type"], "object");
    let provider_profile = &providers["additionalProperties"];
    assert_eq!(provider_profile["type"], "object");
    assert_eq!(provider_profile["additionalProperties"], false);
    assert_eq!(provider_profile["properties"]["url"]["type"], "string");
    assert_eq!(
        provider_profile["properties"]["models"]["additionalProperties"],
        false
    );
    assert_eq!(
        provider_profile["properties"]["models"]["properties"]["main"]["type"],
        "string"
    );
    assert_eq!(
        provider_profile["properties"]["models"]["properties"]["investigate"]["type"],
        "string"
    );

    let profiles = &json_schema["properties"]["agent"]["properties"]["profiles"];
    assert_eq!(profiles["type"], "object");
    let agent_profile = &profiles["additionalProperties"];
    assert_eq!(agent_profile["type"], "object");
    assert_eq!(agent_profile["additionalProperties"], false);
    assert_eq!(
        agent_profile["properties"]["command"]["items"]["type"],
        "string"
    );
    assert_eq!(
        agent_profile["properties"]["provider"]["enum"],
        json!(["anthropic", "deepseek", "chatgpt"])
    );
    assert_eq!(agent_profile["properties"]["model"]["type"], "string");
    assert_eq!(
        agent_profile["properties"]["investigate_model"]["type"],
        "string"
    );
    assert_eq!(
        agent_profile["properties"]["provider_url"]["type"],
        "string"
    );
    assert_eq!(
        agent_profile["properties"]["max_iterations"]["minimum"],
        1
    );
    assert_eq!(
        agent_profile["properties"]["subagents"]["type"],
        "boolean"
    );
    assert_eq!(
        agent_profile["properties"]["credential"]["type"],
        "string"
    );
}
