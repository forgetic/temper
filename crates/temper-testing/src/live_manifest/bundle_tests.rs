use super::*;

#[test]
fn loads_checked_in_live_manifest_bundle() {
    let bundle = ScenarioBundle::load(default_basic_delivery_scenario_path())
        .expect("checked-in live manifest bundle loads");
    assert_eq!(
        bundle.execution.convergence,
        ConvergenceStrategy::SinglePullRequest
    );
    assert!(
        bundle
            .jig_script_path()
            .ends_with("jig/basic-delivery.json")
    );
    assert_eq!(bundle.repo.slug, "acme/service");
    assert_eq!(bundle.repo.default_branch, "main");
    assert_eq!(
        bundle.intake.title,
        "Service banner should identify the environment"
    );
    assert_eq!(bundle.timeout, Duration::from_secs(600));
    assert_eq!(
        bundle.poll_cadence,
        Duration::from_secs(DEFAULT_DAEMON_POLL_BACKSTOP_SECS)
    );
    assert_eq!(
        bundle.poll_backstop,
        Duration::from_secs(DEFAULT_DAEMON_POLL_BACKSTOP_SECS)
    );
    assert_eq!(
        bundle.ci_poll_cadence,
        Duration::from_secs(DEFAULT_CI_POLL_CADENCE_SECS)
    );
    assert_eq!(
        bundle.mechanical_cadence,
        Duration::from_secs(DEFAULT_MECHANICAL_CADENCE_SECS)
    );
    assert_eq!(bundle.observability.log_format, "json");
    assert_eq!(bundle.observability.rust_log, "temper=debug");
    assert!(bundle.repo.seed_path.join("README.md").is_file());
    assert!(
        bundle
            .repo
            .seed_path
            .join(".forgejo/workflows/ci.yml")
            .is_file()
    );
    assert_eq!(
        bundle.repo.ci_source,
        fs::read_to_string(&bundle.repo.ci_seed_path).unwrap()
    );
    jig_core::ScriptFile::load(bundle.jig_script_path()).expect("scenario-owned Jig script parses");
    bundle
        .validate_workflow()
        .expect("scenario workflow remains canonical");
}

#[test]
fn all_live_bundles_resolve_typed_actions_and_owned_jig_scripts() {
    for (name, convergence) in [
        ("basic-delivery", ConvergenceStrategy::SinglePullRequest),
        (
            "forgejo-v16-api-ci",
            ConvergenceStrategy::ImplementationPrTerminalCi,
        ),
        (
            "forgejo-exact-head-ci-repair",
            ConvergenceStrategy::CiPollExactHeadRepair,
        ),
        ("codebase-memory-agent", ConvergenceStrategy::CodebaseMemory),
        (
            "implementation-pr-handoff",
            ConvergenceStrategy::ImplementationPrHandoff,
        ),
        (
            "plan-centric-feature-branch",
            ConvergenceStrategy::PlanFeatureLanding,
        ),
        (
            "history-independent-terminal-recovery",
            ConvergenceStrategy::HistoryIndependentTerminalRecovery,
        ),
    ] {
        let bundle = ScenarioBundle::load(scenarios_root().join(name))
            .unwrap_or_else(|error| panic!("load {name}: {error}"));
        assert_eq!(bundle.execution.convergence, convergence, "{name}");
        assert!(
            bundle
                .jig_script_path()
                .starts_with(scenarios_root().join(name)),
            "{name} must own its Jig script: {}",
            bundle.jig_script_path().display()
        );
        jig_core::ScriptFile::load(bundle.jig_script_path())
            .unwrap_or_else(|error| panic!("parse {name} Jig script: {error}"));
        assert!(
            bundle.execution.steps.iter().any(|step| {
                matches!(
                    step.action,
                    ManifestAction::SeedIssue { .. } | ManifestAction::SeedTerminalHistory { .. }
                )
            }),
            "{name} has a typed issue seed"
        );
    }
}

#[test]
fn forgejo_v16_api_ci_bundle_owns_two_job_terminal_workflow() {
    let bundle = ScenarioBundle::load(scenarios_root().join("forgejo-v16-api-ci"))
        .expect("Forgejo v16 API CI bundle");

    assert_eq!(
        bundle.execution.convergence,
        ConvergenceStrategy::ImplementationPrTerminalCi
    );
    assert!(bundle.repo.ci_source.contains("successful-job:"));
    assert!(bundle.repo.ci_source.contains("status-only-failure:"));
    assert!(bundle.repo.ci_source.contains("exit 1"));
    assert_eq!(
        bundle.repo.ci_source,
        fs::read_to_string(&bundle.repo.ci_seed_path).expect("seeded CI workflow")
    );
}

#[test]
fn codebase_memory_bundle_requires_delayed_graph_readiness_and_bounded_fallback() {
    let bundle = ScenarioBundle::load(scenarios_root().join("codebase-memory-remediation"))
        .expect("codebase-memory bundle");
    let mcp = bundle
        .execution
        .steps
        .iter()
        .find(|step| step.id == "start-fake-codebase-memory-mcp")
        .expect("MCP fixture action");
    assert!(matches!(
        &mcp.action,
        ManifestAction::StartCodebaseMemoryMcp {
            fixture: Some(fixture),
            safe_tools,
            readiness_delay_ms: 750,
            forced_systemic_failure: Some(ForcedSystemicFailureFixture { tool, after_calls: 1 }),
            ..
        } if fixture == "stable-rebind" && safe_tools == &vec![
            "search_graph".to_string(),
            "get_code_snippet".to_string(),
            "list_projects".to_string(),
            "index_status".to_string(),
        ] && tool == "search_graph"
    ));
    assert!(bundle.execution.steps.iter().any(|step| {
        matches!(
            &step.action,
            ManifestAction::ConfigureAgentTools {
                index,
                tool_timeout_secs: Some(2),
                ..
            } if index == "background"
        )
    }));
    let jig = std::fs::read_to_string(bundle.jig_script_path()).expect("scenario-owned Jig");
    assert!(jig.contains("graph_select_retry_worker"));
    assert!(jig.contains("read_rebound_graph_selected_source"));
    assert!(jig.contains("fallback_grep_retry_worker"));
    assert!(!jig.contains("MCP-FIXTURE-SECRET"));
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
    assert_eq!(
        fs::read_to_string(bundle.repo.seed_path.join(".gitignore")).expect("fixture ignore"),
        "/target/\n"
    );
    assert!(
        bundle
            .repo
            .seed_path
            .join("tests/retry_affinity.rs")
            .is_file()
    );
}

#[test]
fn exact_head_repair_bundle_owns_protected_failure_proof_and_three_cadences() {
    let bundle = ScenarioBundle::load(scenarios_root().join("forgejo-exact-head-ci-repair"))
        .expect("exact-head CI repair bundle");

    assert_eq!(
        bundle.execution.convergence,
        ConvergenceStrategy::CiPollExactHeadRepair
    );
    assert_eq!(bundle.ci_poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.poll_cadence, Duration::from_secs(600));
    assert_eq!(bundle.mechanical_cadence, Duration::from_secs(600));
    let failure = bundle
        .ci_failure_evidence
        .expect("protected failure evidence fixture");
    assert_eq!(failure.issuer, "temper-live-evidence");
    assert_eq!(failure.protected_producers, ["forgejo-actions-protected"]);
    assert!(bundle.repo.ci_source.contains("sh -n \"$source_path\""));
    assert!(bundle.repo.ci_source.contains("exit \"$ordinary_status\""));
    assert!(!bundle.repo.ci_source.contains("actions/checkout"));
}

#[test]
fn failure_evidence_manifest_vocabulary_is_closed_and_validated() {
    let valid = TomlValue::Table(toml::Table::from_iter([
        (
            "issuer".to_string(),
            TomlValue::String("issuer-1".to_string()),
        ),
        (
            "protected_producers".to_string(),
            TomlValue::Array(vec![TomlValue::String("producer-1".to_string())]),
        ),
    ]));
    assert!(
        bundle_with_live_harness([("ci_failure_evidence", valid)])
            .expect("valid failure evidence fixture")
            .ci_failure_evidence
            .is_some()
    );

    for invalid in [
        TomlValue::Table(toml::Table::from_iter([(
            "unknown".to_string(),
            TomlValue::String("value".to_string()),
        )])),
        TomlValue::Table(toml::Table::from_iter([
            (
                "issuer".to_string(),
                TomlValue::String("bad identity!".to_string()),
            ),
            ("protected_producers".to_string(), TomlValue::Array(vec![])),
        ])),
    ] {
        let error = bundle_with_live_harness([("ci_failure_evidence", invalid)])
            .expect_err("invalid failure evidence fixture");
        assert!(
            error.contains("live_harness.ci_failure_evidence"),
            "{error}"
        );
    }
}

#[test]
fn convergence_strategy_is_required_instead_of_inferred_from_bundle_identity() {
    let path = scenarios_root().join("implementation-pr-handoff/scenario.toml");
    let mut manifest =
        temper_scenario_core::load_resolved_manifest_toml(path).expect("resolved handoff manifest");
    let wait = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .and_then(|steps| {
            steps.iter_mut().find(|step| {
                step.get("action").and_then(TomlValue::as_str) == Some("workflow.wait_convergence")
            })
        })
        .and_then(TomlValue::as_table_mut)
        .expect("wait step");
    wait.remove("strategy");

    let error = ManifestExecutionPlan::from_manifest(&manifest)
        .expect_err("identity must not supply convergence behavior");
    assert!(error.contains("strategy is required"), "{error}");
}

#[test]
fn reordering_runtime_actions_fails_the_missing_prerequisite() {
    let path = scenarios_root().join("basic-delivery/scenario.toml");
    let mut manifest =
        temper_scenario_core::load_resolved_manifest_toml(path).expect("resolved basic manifest");
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("steps");
    let launch = steps
        .iter()
        .position(|step| {
            step.get("action").and_then(TomlValue::as_str) == Some("temper.launch_standalone")
        })
        .expect("launch");
    let seed = steps
        .iter()
        .position(|step| step.get("action").and_then(TomlValue::as_str) == Some("repo.seed"))
        .expect("seed");
    steps.swap(launch, seed);

    let error = ManifestExecutionPlan::from_manifest(&manifest)
        .expect_err("repo seed cannot run before standalone provisioning");

    assert!(
        error.contains("seed-service-repo") && error.contains("temper.launch_standalone"),
        "{error}"
    );
}

#[test]
fn issue_bindings_and_existing_pr_parameters_are_runtime_actions() {
    let bundle = ScenarioBundle::load(scenarios_root().join("implementation-pr-handoff"))
        .expect("handoff bundle");
    let refresh = bundle
        .execution
        .steps
        .iter()
        .find(|step| step.id == "seed-refresh-source-issue")
        .expect("refresh issue action");
    assert!(matches!(
        &refresh.action,
        ManifestAction::SeedIssue {
            issue_id,
            repo_id,
            binding: Some(binding),
            after_pr_binding: Some(after),
        } if issue_id == "refresh"
            && repo_id == "service"
            && binding == "refresh"
            && after == "create"
    ));
    let pull = bundle
        .execution
        .steps
        .iter()
        .find(|step| step.id == "seed-stale-refresh-pr")
        .expect("existing PR action");
    assert!(matches!(
        &pull.action,
        ManifestAction::SeedPullRequest {
            repo_id,
            source_issue_id,
            title,
            body,
            metadata_kind,
            correlation_key,
        } if repo_id == "service"
            && source_issue_id == "refresh"
            && title == "Old generated title"
            && body == "Old report that must be replaced."
            && metadata_kind == "implementation_pr"
            && correlation_key == "$correlation:refresh"
    ));
}

#[test]
fn live_ci_poll_cadence_accepts_boundaries_and_is_independent() {
    for (value, expected) in [
        (TomlValue::Integer(1), Duration::from_secs(1)),
        (
            TomlValue::String("300s".to_string()),
            Duration::from_secs(300),
        ),
    ] {
        let bundle = bundle_with_live_harness([
            ("ci_poll_cadence", value),
            ("poll_cadence", TomlValue::Integer(77)),
            ("poll_backstop", TomlValue::Integer(88)),
            ("mechanical_cadence", TomlValue::Integer(33)),
        ])
        .expect("valid dedicated CI cadence");
        assert_eq!(bundle.ci_poll_cadence, expected);
        assert_eq!(bundle.poll_cadence, Duration::from_secs(77));
        assert_eq!(bundle.poll_backstop, Duration::from_secs(88));
        assert_eq!(bundle.mechanical_cadence, Duration::from_secs(33));
    }
}

#[test]
fn live_ci_poll_cadence_rejects_every_invalid_value_class() {
    for value in [
        TomlValue::Integer(0),
        TomlValue::Integer(-1),
        TomlValue::Integer(301),
        TomlValue::Float(1.5),
        TomlValue::String("soon".to_string()),
        TomlValue::String("1.5s".to_string()),
        TomlValue::String("18446744073709551616s".to_string()),
    ] {
        let error = bundle_with_live_harness([("ci_poll_cadence", value)])
            .expect_err("invalid dedicated CI cadence");
        assert!(
            error.contains("live_harness.ci_poll_cadence"),
            "field-specific diagnostic: {error}"
        );
    }
}

fn bundle_with_live_harness<const N: usize>(
    values: [(&str, TomlValue); N],
) -> Result<ScenarioBundle, String> {
    let scenario_path = scenarios_root().join("basic-delivery");
    let manifest_path = scenario_path.join("scenario.toml");
    let mut manifest = temper_scenario_core::load_resolved_manifest_toml(&manifest_path)
        .expect("resolved basic manifest");
    let root = manifest.as_table_mut().expect("manifest table");
    let live = root
        .entry("live_harness")
        .or_insert_with(|| TomlValue::Table(toml::Table::new()))
        .as_table_mut()
        .expect("live harness table");
    for (key, value) in values {
        live.insert(key.to_string(), value);
    }
    ScenarioBundle::from_manifest(scenario_path, manifest_path, manifest)
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}

fn default_basic_delivery_scenario_path() -> PathBuf {
    scenarios_root().join("basic-delivery")
}
