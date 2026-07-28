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
        bundle.poll_backstop,
        Duration::from_secs(DEFAULT_DAEMON_POLL_BACKSTOP_SECS)
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
        ("codebase-memory-agent", ConvergenceStrategy::CodebaseMemory),
        (
            "implementation-pr-handoff",
            ConvergenceStrategy::ImplementationPrHandoff,
        ),
        (
            "plan-centric-feature-branch",
            ConvergenceStrategy::PlanFeatureLanding,
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
            bundle
                .execution
                .steps
                .iter()
                .any(|step| matches!(step.action, ManifestAction::SeedIssue { .. })),
            "{name} has a typed issue seed"
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
