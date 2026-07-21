use super::support::target_branch::*;
use super::support::*;

#[test]
fn missing_target_branch_is_created_from_default_before_work_branch_checkout() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/plan-centric-delivery";
        let main_head = branch_head(&fixture, "acme/service", "main");
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(single_repo_assign(
                "pr-for-code-155",
                "agent/pr-for-code-155",
                "main",
                target_branch,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            main_head,
            "new target branch is materialized from the default branch"
        );
        assert_eq!(
            agent.observed_head_sha(),
            main_head,
            "work branch starts from the freshly created target branch"
        );
    });
}

#[test]
fn existing_target_branch_is_reused_without_resetting_it() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/existing-plan-branch";
        let target_head = seed_feature_branch(&fixture, "acme/service", target_branch);
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(single_repo_assign(
                "pr-for-code-155-existing",
                "agent/pr-for-code-155-existing",
                "main",
                target_branch,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155-existing");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            target_head,
            "existing target branch must not be reset to the default branch"
        );
        assert_eq!(
            agent.observed_head_sha(),
            target_head,
            "work branch starts from the existing target branch head"
        );
    });
}

#[test]
fn reused_dirty_read_only_target_is_still_quarantined() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/reused-read-only";
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);
        let assignment = || {
            assign_with_context(
                "reused-read-only",
                read_only_job_context("agent/reused-read-only", "reused-read-only")
                    .with_base_branch(target_branch),
            )
        };

        expect_verdict(executor.execute(assignment()).await);
        let checkout = fixture
            .workspace_root
            .join("architect/reused-read-only/service");
        fs::write(checkout.join("architect-note.txt"), "preserve this work\n")
            .expect("write genuine local edit");

        let message = expect_failure_class(
            executor.execute(assignment()).await,
            FailureClass::Permanent,
        );

        assert!(message.contains("quarantined during inspect-read-only"));
        let quarantine = checkout.with_file_name("service.temper-quarantine");
        let manifest_path = quarantine.join("temper-recovery.json");
        let manifest: Value = serde_json::from_str(
            &fs::read_to_string(&manifest_path).expect("read quarantine recovery manifest"),
        )
        .expect("parse quarantine recovery manifest");
        assert!(
            !manifest["recovery_notes"]
                .as_array()
                .expect("recovery notes array")
                .is_empty(),
            "new quarantine manifest should carry phase-aware operator notes"
        );
        assert_quarantine_failure_guidance(&message, &manifest);
        assert!(
            message.contains("read-only checkout contains staged, tracked, or untracked edits"),
            "failure should include the underlying preparation failure: {message}"
        );
        assert!(
            manifest.to_string().contains("architect-note.txt"),
            "manifest should preserve the dirty path"
        );
        assert!(!checkout.exists());
    });
}

#[test]
fn reused_writable_target_reports_the_same_delimited_quarantine_guidance() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/quarantine-guidance";
        fixture.seed_pr_head_branch(branch);
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);
        let assignment = || pr_fix_assign(branch, "quarantine-guidance");

        expect_failure_class(
            executor.execute(assignment()).await,
            FailureClass::Permanent,
        );
        let checkout = fixture
            .workspace_root
            .join("engineer/quarantine-guidance/service");
        git([
            "-C",
            path_str(&checkout),
            "checkout",
            "-b",
            "unexpected-operator-branch",
        ]);
        fs::write(checkout.join("operator-note.txt"), "preserve this too\n")
            .expect("write local edit on unexpected branch");

        let message = expect_failure_class(
            executor.execute(assignment()).await,
            FailureClass::Permanent,
        );
        assert!(message.contains("quarantined during inspect-branch"));
        assert!(message.contains(
            "expected branch `agent/quarantine-guidance`, found `unexpected-operator-branch`"
        ));

        let quarantine = checkout.with_file_name("service.temper-quarantine");
        let manifest_path = quarantine.join("temper-recovery.json");
        let mut manifest: Value = serde_json::from_str(
            &fs::read_to_string(&manifest_path).expect("read quarantine recovery manifest"),
        )
        .expect("parse quarantine recovery manifest");
        assert!(
            !manifest["recovery_notes"]
                .as_array()
                .expect("recovery notes array")
                .is_empty(),
            "new quarantine manifest should carry phase-aware operator notes"
        );
        assert_quarantine_failure_guidance(&message, &manifest);

        manifest
            .as_object_mut()
            .expect("manifest object")
            .remove("recovery_notes");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize legacy manifest"),
        )
        .expect("rewrite manifest without recovery notes");
        let legacy_message = expect_failure_class(
            executor.execute(assignment()).await,
            FailureClass::Permanent,
        );
        assert_quarantine_failure_guidance(&legacy_message, &manifest);
        assert!(legacy_message.contains("recovery notes: (none recorded in manifest)"));
    });
}

fn assert_quarantine_failure_guidance(message: &str, manifest: &Value) {
    const COMMANDS_BEGIN: &str = "--- BEGIN RUNNABLE RECOVERY COMMANDS ---";
    const COMMANDS_END: &str = "--- END RUNNABLE RECOVERY COMMANDS ---";

    let field = |name: &str| {
        manifest[name]
            .as_str()
            .unwrap_or_else(|| panic!("manifest field {name} should be a string"))
    };
    assert!(
        message.contains(&format!(
            "workspace {} quarantined during {} at {}",
            field("repository"),
            field("failure_phase"),
            field("quarantine_path")
        )),
        "failure did not publish manifest identity and location:\n{message}\nmanifest: {manifest}"
    );
    assert!(message.contains(&format!("underlying failure: {}", field("failure"))));

    let begin = message.find(COMMANDS_BEGIN).expect("commands begin marker");
    let end = message.find(COMMANDS_END).expect("commands end marker");
    assert!(begin < end, "recovery command markers are out of order");
    let before_commands = &message[..begin];
    let command_block = message[begin + COMMANDS_BEGIN.len()..end].trim_matches('\n');
    let expected_commands = manifest["recovery_commands"]
        .as_array()
        .expect("recovery commands array")
        .iter()
        .map(|command| command.as_str().expect("recovery command string"))
        .collect::<Vec<_>>();
    assert_eq!(command_block.lines().collect::<Vec<_>>(), expected_commands);

    let notes = manifest
        .get("recovery_notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|note| note.as_str().expect("recovery note string"));
    for note in notes {
        assert!(
            before_commands.contains(note),
            "recovery note should be rendered before the command section: {note}"
        );
        assert!(
            !command_block.contains(note),
            "recovery prose must not be mixed into runnable commands: {note}"
        );
    }
}

#[test]
fn each_writable_repo_materializes_target_branch_independently() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        seed_repo_from_service_main(&fixture, "acme/lib");
        let target_branch = "feature/coordinated-target";
        let service_main = branch_head(&fixture, "acme/service", "main");
        let lib_main = branch_head(&fixture, "acme/lib", "main");
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let outcome = executor
            .execute(coordinated_assign(
                "pr-for-code-155-coordinated",
                "agent/pr-for-code-155-coordinated",
                target_branch,
                true,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155-coordinated");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            service_main
        );
        assert_eq!(branch_head(&fixture, "acme/lib", target_branch), lib_main);
    });
}

#[test]
fn read_only_sibling_does_not_create_target_branch() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        seed_repo_from_service_main(&fixture, "acme/lib");
        let target_branch = "feature/writable-only-target";
        let service_main = branch_head(&fixture, "acme/service", "main");
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let outcome = executor
            .execute(coordinated_assign(
                "pr-for-code-155-readonly",
                "agent/pr-for-code-155-readonly",
                target_branch,
                false,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155-readonly");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            service_main
        );
        assert_no_branch(&fixture, "acme/lib", target_branch);
    });
}

#[test]
fn missing_target_and_default_branch_reports_clear_diagnostics() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/missing-target-and-default";
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let outcome = executor
            .execute(single_repo_assign(
                "pr-for-code-155-missing-default",
                "agent/pr-for-code-155-missing-default",
                "missing-default",
                target_branch,
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Transient);
        assert!(
            message.contains("target branch `feature/missing-target-and-default` is missing"),
            "message should name the missing target branch: {message}"
        );
        assert!(
            message.contains("default branch `missing-default` could not be fetched"),
            "message should explain the default branch fetch failure: {message}"
        );
        assert_no_origin_branch(&fixture, target_branch);
        assert_no_origin_branch(&fixture, "agent/pr-for-code-155-missing-default");
    });
}
