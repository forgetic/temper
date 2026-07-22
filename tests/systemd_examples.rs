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
    assert_eq!(
        pools.len(),
        4,
        "checked-in example should cover every split pool plus local standalone"
    );
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

    let standalone = successful_json(
        &[
            "--config",
            &config,
            "--secrets",
            &credentials,
            "--format",
            "json",
            "check",
            "--component",
            "standalone",
        ],
        dir.path(),
    );
    assert_eq!(standalone["status"], "ok");
    assert_eq!(standalone["online"], false);

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
        "worker-local-token",
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

fn systemd_duration_secs(value: &str) -> u64 {
    for (suffix, multiplier) in [("min", 60_u64), ("s", 1_u64)] {
        if let Some(number) = value.strip_suffix(suffix) {
            return number
                .parse::<u64>()
                .unwrap_or_else(|error| panic!("invalid systemd duration `{value}`: {error}"))
                .checked_mul(multiplier)
                .unwrap_or_else(|| panic!("systemd duration `{value}` overflows seconds"));
        }
    }
    panic!("unsupported systemd duration `{value}`");
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
    assert_eq!(service_files.len(), 3, "do not add a separate trigger unit");

    let standalone_path = examples.join("temper-standalone.service");
    let engine_path = examples.join("temper-engine.service");
    let worker_path = examples.join("temper-worker@.service");
    let standalone = SystemdUnit::parse(&standalone_path);
    let engine = SystemdUnit::parse(&engine_path);
    let worker = SystemdUnit::parse(&worker_path);
    let credential = "credentials.toml:/etc/temper/credentials.toml";
    assert_eq!(
        standalone.values("Service", "LoadCredential"),
        vec![credential]
    );
    assert_eq!(engine.values("Service", "LoadCredential"), vec![credential]);
    assert_eq!(worker.values("Service", "LoadCredential"), vec![credential]);

    let standalone_command = standalone.values("Service", "ExecStart");
    assert_eq!(standalone_command.len(), 1);
    assert_eq!(
        standalone_command[0],
        "/usr/local/bin/temper --config /etc/temper/config.toml serve standalone --id %H-standalone"
    );

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
    assert_eq!(worker.values("Service", "Delegate"), vec!["yes"]);
    assert_eq!(worker.values("Service", "KillMode"), vec!["control-group"]);
    assert_eq!(worker.values("Service", "TimeoutStopSec"), vec!["5min"]);

    assert_eq!(standalone.values("Service", "Delegate"), vec!["yes"]);
    assert_eq!(
        standalone.values("Service", "KillMode"),
        vec!["control-group"]
    );
    assert_eq!(standalone.values("Service", "Restart"), vec!["on-failure"]);
    let stop_timeout = standalone.values("Service", "TimeoutStopSec");
    assert_eq!(stop_timeout.len(), 1);
    let stop_timeout_secs = systemd_duration_secs(stop_timeout[0]);
    let config = parse_toml(&examples.join("config.example.toml"));
    let shutdown_budget_secs = table(&config, "deployment")
        .get("standalone_shutdown_budget_secs")
        .and_then(TomlValue::as_integer)
        .and_then(|value| u64::try_from(value).ok())
        .expect("positive standalone shutdown budget");
    const DOCUMENTED_SAFETY_MARGIN_SECS: u64 = 15;
    assert!(
        stop_timeout_secs > shutdown_budget_secs,
        "systemd must not terminate Temper at or before its internal deadline"
    );
    assert_eq!(
        stop_timeout_secs - shutdown_budget_secs,
        DOCUMENTED_SAFETY_MARGIN_SECS,
        "standalone unit must retain its documented 15-second safety margin"
    );

    for path in [standalone_path, engine_path, worker_path] {
        let unit = read(&path);
        for forbidden in ["temper daemon", "trigger-forgejo", "serve trigger"] {
            assert!(
                !unit.contains(forbidden),
                "{} contains legacy trigger/runtime onboarding `{forbidden}`",
                path.display()
            );
        }
    }

    let deployment_docs = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/how-to/deploy-with-systemd.md"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/reference/production-worker.md"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/systemd/README.md"),
    ];
    for path in deployment_docs {
        let documentation = read(&path);
        for required in [
            "Delegate=yes",
            "KillMode=control-group",
            "cgroup-v2",
            "pidfd",
            "subreaper",
            "recursive-empty",
            "standalone_shutdown_budget_secs",
            "15-second safety margin",
            "KillMode=process",
        ] {
            assert!(
                documentation.contains(required),
                "{} lacks containment contract `{required}`",
                path.display()
            );
        }
        for obsolete in [
            "Process-group cleanup is the normal",
            "process group supervisor",
            "process groups are descendant-complete",
        ] {
            assert!(
                !documentation.contains(obsolete),
                "{} retains obsolete descendant claim `{obsolete}`",
                path.display()
            );
        }
    }
}
