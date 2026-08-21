use super::*;

#[test]
fn parses_closed_bounded_fixture() {
    let manifest = manifest_with_actions_history(201, 90_000, 120_000);
    let plan = ManifestExecutionPlan::from_manifest(&manifest).expect("bounded Actions fixture");
    assert!(plan.steps.iter().any(|step| {
        matches!(
            &step.action,
            ManifestAction::SeedActionsHistory { fixture }
                if fixture.repo_id == "service"
                    && fixture.source_issue_id == "intake"
                    && fixture.seeded_runs == 201
                    && fixture.payload_bytes == 90_000
                    && fixture.timeout == Duration::from_secs(120)
        )
    }));
}

#[test]
fn rejects_bounds_and_insufficient_inventory() {
    for (runs, payload, timeout, expected) in [
        (50, 90_000, 120_000, "seeded_runs"),
        (257, 90_000, 120_000, "seeded_runs"),
        (201, 64_000, 120_000, "payload_bytes"),
        (201, 99_000, 120_000, "payload_bytes"),
        (201, 90_000, 180_001, "timeout_ms"),
        (51, 65_536, 120_000, "must exceed"),
    ] {
        let error = ManifestExecutionPlan::from_manifest(&manifest_with_actions_history(
            runs, payload, timeout,
        ))
        .expect_err("invalid oversized Actions fixture must fail");
        assert!(error.contains(expected), "{expected}: {error}");
    }
}

#[test]
fn requires_seeded_issue_and_precedes_convergence() {
    let mut manifest = manifest_with_actions_history(201, 90_000, 120_000);
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("steps");
    let issue = steps
        .iter()
        .position(|step| step.get("action").and_then(TomlValue::as_str) == Some("issue.seed"))
        .expect("issue step");
    let history = steps
        .iter()
        .position(|step| {
            step.get("action").and_then(TomlValue::as_str)
                == Some("forgejo.actions.seed_oversized_history")
        })
        .expect("history step");
    steps.swap(issue, history);
    let error = ManifestExecutionPlan::from_manifest(&manifest)
        .expect_err("history cannot run before its source issue");
    assert!(error.contains("issue.seed binding `intake`"), "{error}");
}

fn manifest_with_actions_history(runs: i64, payload: i64, timeout: i64) -> TomlValue {
    let path = scenarios_root().join("basic-delivery/scenario.toml");
    let mut manifest =
        temper_scenario_core::load_resolved_manifest_toml(path).expect("resolved basic manifest");
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("steps");
    let convergence = steps
        .iter()
        .position(|step| {
            step.get("action").and_then(TomlValue::as_str) == Some("workflow.wait_convergence")
        })
        .expect("convergence");
    steps.insert(
        convergence,
        TomlValue::Table(toml::Table::from_iter([
            (
                "id".to_string(),
                TomlValue::String("seed-actions-history".to_string()),
            ),
            (
                "action".to_string(),
                TomlValue::String("forgejo.actions.seed_oversized_history".to_string()),
            ),
            ("repo".to_string(), TomlValue::String("service".to_string())),
            (
                "source_issue_id".to_string(),
                TomlValue::String("intake".to_string()),
            ),
            ("seeded_runs".to_string(), TomlValue::Integer(runs)),
            ("payload_bytes".to_string(), TomlValue::Integer(payload)),
            ("timeout_ms".to_string(), TomlValue::Integer(timeout)),
        ])),
    );
    manifest
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
