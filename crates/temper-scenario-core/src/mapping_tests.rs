// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{
    FeatureScenarioBaseComparison, FeatureScenarioResolveError, ForgeIssueKey,
    ResolveFeatureScenarioRequest, check_scenario, parse_manifest_str, resolve_feature_scenario,
    scenario_content_digest,
};

#[test]
fn parses_typed_feature_mapping_and_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join("jig")).expect("jig dir");
    fs::write(dir.path().join("jig/proof.json"), "{}\n").expect("jig");
    let manifest = parse_manifest_str(
        &manifest("proof", "active", "new", "ai/temper#778"),
        dir.path(),
    )
    .expect("mapped manifest");

    let mapping = manifest.feature_mapping.expect("mapping");
    assert_eq!(mapping.feature.to_string(), "ai/temper#778");
    assert_eq!(mapping.plan.expect("plan").to_string(), "ai/temper#779");
    assert_eq!(mapping.source_branch, "feature/778-exact-head-validation");
    let contract = manifest.feature_contract.expect("contract");
    assert_eq!(contract.runtime_budget_seconds, 600);
    assert_eq!(contract.jig_script_path, "jig/proof.json");
}

#[test]
fn rejects_invalid_mapping_metadata_and_unbounded_contract() {
    let source = manifest("proof", "active", "later", "not-an-issue")
        .replace(
            "runtime_budget_seconds = 600",
            "runtime_budget_seconds = 99999",
        )
        .replace(
            "source_branch = \"feature/778-exact-head-validation\"",
            "source_branch = \"--upload-pack=oops\"",
        );
    let diagnostics = parse_manifest_str(&source, ".").expect_err("invalid mapping");
    let rendered = diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("owner/name#number"), "{rendered}");
    assert!(rendered.contains("unknown change intent"), "{rendered}");
    assert!(rendered.contains("safe Git branch"), "{rendered}");
    assert!(rendered.contains("1 through 3600"), "{rendered}");
}

#[test]
fn inherited_mapping_cannot_implicitly_map_a_renamed_or_copied_scenario() {
    let workspace = tempfile::tempdir().expect("workspace");
    fs::write(workspace.path().join("Cargo.toml"), "[workspace]\n").expect("cargo");
    let scenarios = workspace.path().join("scenarios");
    write_scenario(&scenarios, "base", "base", "active", "new", "ai/temper#778");
    let child = scenarios.join("copy");
    fs::create_dir_all(&child).expect("child");
    fs::write(
        child.join("scenario.toml"),
        "name = \"copy\"\nintent = \"A renamed inherited copy.\"\n[fixtures]\nextends = \"scenarios/base\"\n[runner]\nuses = \"manifest\"\n",
    )
    .expect("child manifest");

    let report = check_scenario(child);
    assert!(!report.is_valid());
    let rendered = report
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("validation: must be declared by this scenario and cannot be inherited"),
        "{rendered}"
    );
    assert!(rendered.contains("feature_contract"), "{rendered}");
}

#[test]
fn resolves_one_new_mapping_with_stable_audit_identity() {
    let repo = TestRepo::new();
    let base = repo.commit("base");
    repo.write_scenario("proof", "active", "new", "ai/temper#778");
    let head = repo.commit("add proof");

    let request = repo.request("ai/temper#778", &base);
    let resolved = resolve_feature_scenario(&request).expect("resolve mapping");

    assert_eq!(resolved.mapping_id, "ai/temper#778:proof");
    assert_eq!(resolved.scenario_path, "scenarios/proof");
    assert_eq!(resolved.manifest_path, "scenarios/proof/scenario.toml");
    assert_eq!(resolved.head_sha, head);
    assert_eq!(resolved.landing_base_sha, base);
    assert_eq!(resolved.base_comparison, FeatureScenarioBaseComparison::New);
    assert!(resolved.content_changed_from_base);
    assert!(resolved.digest.starts_with("sha256:"));

    let report = check_scenario(repo.path().join("scenarios/proof"));
    let context = resolved.validator_context(report.manifest.as_ref().expect("manifest"));
    assert_eq!(context.mapping_id.as_deref(), Some("ai/temper#778:proof"));
    assert_eq!(context.feature.as_deref(), Some("ai/temper#778"));
    assert_eq!(context.commit.as_deref(), Some(head.as_str()));
    assert_eq!(context.digest.as_deref(), Some(resolved.digest.as_str()));
}

#[test]
fn rejects_zero_duplicate_inactive_and_unsafe_mappings_clearly() {
    let missing = TestRepo::new();
    let base = missing.commit("base");
    let error = resolve_feature_scenario(&missing.request("ai/temper#778", &base))
        .expect_err("missing mapping");
    assert!(matches!(error, FeatureScenarioResolveError::Missing { .. }));

    let duplicate = TestRepo::new();
    let base = duplicate.commit("base");
    duplicate.write_scenario("zeta", "active", "new", "ai/temper#778");
    duplicate.write_scenario("alpha", "active", "new", "ai/temper#778");
    duplicate.commit("duplicates");
    let error = resolve_feature_scenario(&duplicate.request("ai/temper#778", &base))
        .expect_err("duplicate mappings");
    match error {
        FeatureScenarioResolveError::Ambiguous { paths, .. } => {
            assert_eq!(paths, "scenarios/alpha, scenarios/zeta");
        }
        other => panic!("unexpected error: {other}"),
    }

    let inactive = TestRepo::new();
    let base = inactive.commit("base");
    inactive.write_scenario("proof", "draft", "new", "ai/temper#778");
    inactive.commit("draft proof");
    let error = resolve_feature_scenario(&inactive.request("ai/temper#778", &base))
        .expect_err("inactive mapping");
    assert!(matches!(
        error,
        FeatureScenarioResolveError::Inactive { .. }
    ));

    let unsafe_repo = TestRepo::new();
    let base = unsafe_repo.commit("base");
    unsafe_repo.write_scenario_named("renamed", "copied", "active", "new", "ai/temper#778");
    unsafe_repo.commit("unsafe name");
    let error = resolve_feature_scenario(&unsafe_repo.request("ai/temper#778", &base))
        .expect_err("name/path mismatch");
    assert!(matches!(error, FeatureScenarioResolveError::Unsafe { .. }));
}

#[test]
fn requires_landing_base_change_intent_and_digest_match() {
    let updated = TestRepo::new();
    updated.write_scenario("proof", "active", "new", "ai/temper#778");
    let base = updated.commit("existing mapping");
    let path = updated.path().join("scenarios/proof/scenario.toml");
    let source = fs::read_to_string(&path)
        .expect("manifest")
        .replace("change = \"new\"", "change = \"updated\"");
    fs::write(&path, source).expect("update intent");
    fs::write(
        updated.path().join("scenarios/proof/update.md"),
        "deliberate update\n",
    )
    .expect("update content");
    updated.commit("update proof");
    let resolved = resolve_feature_scenario(&updated.request("ai/temper#778", &base))
        .expect("deliberately updated mapping");
    assert_eq!(
        resolved.base_comparison,
        FeatureScenarioBaseComparison::Updated
    );
    assert!(resolved.content_changed_from_base);

    let wrong_intent = TestRepo::new();
    let base = wrong_intent.commit("base");
    wrong_intent.write_scenario("proof", "active", "updated", "ai/temper#778");
    wrong_intent.commit("new path wrong intent");
    let error = resolve_feature_scenario(&wrong_intent.request("ai/temper#778", &base))
        .expect_err("wrong new intent");
    assert!(matches!(
        error,
        FeatureScenarioResolveError::NewIntent { .. }
    ));

    let unchanged = TestRepo::new();
    unchanged.write_scenario("proof", "active", "updated", "ai/temper#778");
    let base = unchanged.commit("existing mapping");
    let error = resolve_feature_scenario(&unchanged.request("ai/temper#778", &base))
        .expect_err("unchanged mapping");
    assert!(matches!(
        error,
        FeatureScenarioResolveError::Unchanged { .. }
    ));

    let mismatch = TestRepo::new();
    let base = mismatch.commit("base");
    mismatch.write_scenario("proof", "active", "new", "ai/temper#778");
    mismatch.commit("add proof");
    let mut request = mismatch.request("ai/temper#778", &base);
    request.expected_digest = Some(format!("sha256:{}", "0".repeat(64)));
    let error = resolve_feature_scenario(&request).expect_err("digest mismatch");
    assert!(matches!(
        error,
        FeatureScenarioResolveError::DigestMismatch { .. }
    ));
}

#[test]
fn digest_is_independent_of_directory_enumeration_and_checkout_path() {
    let left = tempfile::tempdir().expect("left");
    let right = tempfile::tempdir().expect("right");
    write_scenario(
        left.path(),
        "proof",
        "proof",
        "active",
        "new",
        "ai/temper#778",
    );
    write_scenario(
        right.path(),
        "proof",
        "proof",
        "active",
        "new",
        "ai/temper#778",
    );
    fs::write(left.path().join("proof/z-last.txt"), "z\n").expect("left z");
    fs::write(left.path().join("proof/a-first.txt"), "a\n").expect("left a");
    fs::write(right.path().join("proof/a-first.txt"), "a\n").expect("right a");
    fs::write(right.path().join("proof/z-last.txt"), "z\n").expect("right z");

    let left_report = check_scenario(left.path().join("proof"));
    let right_report = check_scenario(right.path().join("proof"));
    let left_digest = scenario_content_digest(&left_report).expect("left digest");
    let right_digest = scenario_content_digest(&right_report).expect("right digest");
    assert_eq!(left_digest, right_digest);

    fs::write(
        right.path().join("proof/jig/proof.json"),
        "{\"changed\":true}\n",
    )
    .expect("change jig");
    let changed = scenario_content_digest(&check_scenario(right.path().join("proof")))
        .expect("changed digest");
    assert_ne!(left_digest, changed);
}

struct TestRepo {
    dir: tempfile::TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("repo");
        git(dir.path(), &["init", "-q"]);
        git(
            dir.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        git(dir.path(), &["config", "user.name", "Temper Tests"]);
        fs::create_dir(dir.path().join("scenarios")).expect("scenarios");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write_scenario(&self, name: &str, status: &str, change: &str, feature: &str) {
        self.write_scenario_named(name, name, status, change, feature);
    }

    fn write_scenario_named(
        &self,
        directory: &str,
        name: &str,
        status: &str,
        change: &str,
        feature: &str,
    ) {
        write_scenario(
            &self.path().join("scenarios"),
            directory,
            name,
            status,
            change,
            feature,
        );
    }

    fn commit(&self, message: &str) -> String {
        git(self.path(), &["add", "."]);
        git(
            self.path(),
            &["commit", "-q", "--allow-empty", "-m", message],
        );
        git_output(self.path(), &["rev-parse", "HEAD"])
    }

    fn request(&self, feature: &str, base: &str) -> ResolveFeatureScenarioRequest {
        ResolveFeatureScenarioRequest::new(
            self.path(),
            "scenarios",
            feature.parse::<ForgeIssueKey>().expect("feature"),
            base,
        )
    }
}

fn write_scenario(
    root: &Path,
    directory: &str,
    name: &str,
    status: &str,
    change: &str,
    feature: &str,
) {
    let scenario = root.join(directory);
    fs::create_dir_all(scenario.join("jig")).expect("scenario dirs");
    fs::write(
        scenario.join("scenario.toml"),
        manifest(name, status, change, feature),
    )
    .expect("manifest");
    fs::write(scenario.join(format!("jig/{name}.json")), "{}\n").expect("jig");
}

fn manifest(name: &str, status: &str, change: &str, feature: &str) -> String {
    format!(
        "schema = \"temper.scenario.v1\"\nname = \"{name}\"\nstatus = \"{status}\"\nstability = \"provisional\"\nintent = \"Prove mapped behavior.\"\n\n[runner]\nuses = \"manifest\"\n\n[validation]\nfeature = \"{feature}\"\nplan = \"ai/temper#779\"\nsource_branch = \"feature/778-exact-head-validation\"\nchange = \"{change}\"\n\n[feature_contract]\nclaim = \"The feature works.\"\nstimulus = \"Run the feature workflow.\"\nobservable = \"Structured facts.\"\nassertion = \"Required facts pass.\"\nruntime_budget_seconds = 600\njig_script_path = \"jig/{name}.json\"\n"
    )
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}
