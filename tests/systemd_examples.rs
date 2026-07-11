// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/systemd")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn parse_toml(path: &Path) -> TomlValue {
    toml::from_str(&read(path)).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn table<'a>(value: &'a TomlValue, key: &str) -> &'a toml::map::Map<String, TomlValue> {
    value
        .get(key)
        .and_then(TomlValue::as_table)
        .unwrap_or_else(|| panic!("missing TOML table `{key}`"))
}

fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .env_remove("CREDENTIALS_DIRECTORY")
        .output()
        .expect("run temper")
}

fn successful_json(args: &[&str], env_root: &Path) -> JsonValue {
    let output = temper(args, env_root);
    assert!(
        output.status.success(),
        "args: {args:?}\nstatus: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid command JSON")
}

fn config_and_credentials_args() -> (String, String) {
    let examples = example_dir();
    (
        examples.join("config.example.toml").display().to_string(),
        examples
            .join("credentials.example.toml")
            .display()
            .to_string(),
    )
}

#[test]
fn bundle_files_parse_and_every_named_reference_has_a_placeholder() {
    let examples = example_dir();
    let config = parse_toml(&examples.join("config.example.toml"));
    let credentials = parse_toml(&examples.join("credentials.example.toml"));
    assert_eq!(
        config.get("schema_version").and_then(TomlValue::as_integer),
        Some(1)
    );
    assert_eq!(
        credentials
            .get("schema_version")
            .and_then(TomlValue::as_integer),
        Some(1)
    );

    let workflow_relative = table(&config, "workflow")
        .get("file")
        .and_then(TomlValue::as_str)
        .expect("workflow.file string");
    assert_eq!(workflow_relative, "workflow.example.yaml");
    let workflow_path = examples.join(workflow_relative);
    let workflow = temper_workflow::load_workflow(&workflow_path)
        .unwrap_or_else(|error| panic!("parse and validate {}: {error}", workflow_path.display()));
    assert_eq!(workflow.name(), "systemd-operator-example");

    let mut references = BTreeSet::new();
    let engine = table(&config, "engine");
    for key in ["forge_token", "webhook_secret"] {
        references.insert(
            engine
                .get(key)
                .and_then(TomlValue::as_str)
                .unwrap_or_else(|| panic!("engine.{key} string")),
        );
    }
    let worker = table(&config, "worker");
    let pools = worker
        .get("pools")
        .and_then(TomlValue::as_array)
        .expect("worker.pools array");
    assert_eq!(pools.len(), 3, "checked-in example should cover every pool");
    for pool in pools {
        references.insert(
            pool.get("worker_token")
                .and_then(TomlValue::as_str)
                .expect("pool worker_token string"),
        );
    }
    let profiles = table(&config, "agent")
        .get("profiles")
        .and_then(TomlValue::as_table)
        .expect("agent.profiles table");
    for profile in profiles.values() {
        references.insert(
            profile
                .get("credential")
                .and_then(TomlValue::as_str)
                .expect("profile credential string"),
        );
    }

    let named_secrets = table(&credentials, "secrets");
    for reference in references {
        let placeholder = named_secrets
            .get(reference)
            .unwrap_or_else(|| panic!("missing placeholder for named secret `{reference}`"));
        let rendered = placeholder.to_string();
        assert!(
            rendered.contains("replace-me"),
            "named secret `{reference}` is not an obvious placeholder: {rendered}"
        );
    }
}

#[test]
fn offline_engine_and_every_pool_check_pass_and_redact_secrets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let examples = example_dir();
    let (config, credentials) = config_and_credentials_args();

    let engine = successful_json(
        &[
            "--config",
            &config,
            "--secrets",
            &credentials,
            "--format",
            "json",
            "check",
            "--component",
            "engine",
        ],
        dir.path(),
    );
    assert_eq!(engine["status"], "ok");
    assert_eq!(engine["online"], false);

    let parsed_config = parse_toml(&examples.join("config.example.toml"));
    let pools = table(&parsed_config, "worker")["pools"]
        .as_array()
        .expect("worker.pools array");
    for pool in pools {
        let name = pool["name"].as_str().expect("pool name string");
        let report = successful_json(
            &[
                "--config",
                &config,
                "--secrets",
                &credentials,
                "--format",
                "json",
                "check",
                "--component",
                "worker",
                "--pool",
                name,
            ],
            dir.path(),
        );
        assert_eq!(report["status"], "ok", "pool `{name}`: {report}");
        assert_eq!(report["pool"], name);
        assert_eq!(report["online"], false);
    }

    let paths = successful_json(
        &[
            "--config",
            &config,
            "--secrets",
            &credentials,
            "--format",
            "json",
            "config",
            "paths",
        ],
        dir.path(),
    );
    assert_eq!(
        paths["workflow_file"],
        examples.join("workflow.example.yaml").display().to_string()
    );

    let show = temper(
        &[
            "--config",
            &config,
            "--secrets",
            &credentials,
            "config",
            "show",
        ],
        dir.path(),
    );
    assert!(
        show.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let show = String::from_utf8(show.stdout).expect("config show utf8");
    for name in [
        "engine-forge-token",
        "webhook-secret",
        "worker-architects-token",
        "worker-engineers-token",
        "worker-reviewers-token",
        "planning-provider-credentials",
        "coding-provider-credentials",
    ] {
        assert!(
            show.contains(name),
            "missing redacted reference `{name}`: {show}"
        );
    }
    let parsed_credentials = parse_toml(&examples.join("credentials.example.toml"));
    let mut placeholders = Vec::new();
    collect_placeholders(&parsed_credentials, &mut placeholders);
    assert!(
        placeholders.len() >= 15,
        "unexpected placeholder coverage: {placeholders:?}"
    );
    for secret in placeholders {
        assert!(
            !show.contains(&secret),
            "secret placeholder leaked: {secret}"
        );
    }
}

fn collect_placeholders(value: &TomlValue, output: &mut Vec<String>) {
    match value {
        TomlValue::String(value) if value.contains("replace-me") => output.push(value.clone()),
        TomlValue::Array(values) => {
            for value in values {
                collect_placeholders(value, output);
            }
        }
        TomlValue::Table(values) => {
            for value in values.values() {
                collect_placeholders(value, output);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Default)]
struct SystemdUnit {
    sections: BTreeMap<String, Vec<(String, String)>>,
}

impl SystemdUnit {
    fn parse(path: &Path) -> Self {
        let mut unit = Self::default();
        let mut section = None::<String>;
        for (index, raw) in read(path).lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let name = line[1..line.len() - 1].to_string();
                unit.sections.entry(name.clone()).or_default();
                section = Some(name);
                continue;
            }
            let (key, value) = line.split_once('=').unwrap_or_else(|| {
                panic!(
                    "{}:{} is not a systemd assignment: {line}",
                    path.display(),
                    index + 1
                )
            });
            let section = section.as_ref().unwrap_or_else(|| {
                panic!("{}:{} assignment before section", path.display(), index + 1)
            });
            unit.sections
                .get_mut(section)
                .expect("current section exists")
                .push((key.to_string(), value.to_string()));
        }
        for required in ["Unit", "Service", "Install"] {
            assert!(
                unit.sections.contains_key(required),
                "{} lacks [{required}]",
                path.display()
            );
        }
        unit
    }

    fn values<'a>(&'a self, section: &str, key: &str) -> Vec<&'a str> {
        self.sections
            .get(section)
            .into_iter()
            .flatten()
            .filter_map(|(candidate, value)| (candidate == key).then_some(value.as_str()))
            .collect()
    }
}

#[test]
fn units_use_public_serve_commands_and_the_bundled_credentials() {
    let examples = example_dir();
    let service_files = std::fs::read_dir(&examples)
        .expect("read systemd examples")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "service")
        })
        .collect::<Vec<_>>();
    assert_eq!(service_files.len(), 2, "do not add a separate trigger unit");

    let engine_path = examples.join("temper-engine.service");
    let worker_path = examples.join("temper-worker@.service");
    let engine = SystemdUnit::parse(&engine_path);
    let worker = SystemdUnit::parse(&worker_path);
    let credential = "credentials.toml:/etc/temper/credentials.toml";
    assert_eq!(engine.values("Service", "LoadCredential"), vec![credential]);
    assert_eq!(worker.values("Service", "LoadCredential"), vec![credential]);

    let engine_command = engine.values("Service", "ExecStart");
    assert_eq!(engine_command.len(), 1);
    assert!(
        engine_command[0]
            .contains("temper --config /etc/temper/config.toml serve engine --id %H-engine"),
        "{}",
        engine_command[0]
    );
    let worker_command = worker.values("Service", "ExecStart");
    assert_eq!(worker_command.len(), 1);
    assert!(
        worker_command[0]
            .contains("temper --config /etc/temper/config.toml serve worker --pool %i --id %H-%i"),
        "{}",
        worker_command[0]
    );

    for path in [engine_path, worker_path] {
        let unit = read(&path);
        for forbidden in ["temper daemon", "trigger-forgejo", "serve trigger"] {
            assert!(
                !unit.contains(forbidden),
                "{} contains legacy trigger/runtime onboarding `{forbidden}`",
                path.display()
            );
        }
    }
}
