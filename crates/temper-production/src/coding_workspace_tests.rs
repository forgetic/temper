use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use temper_forge::RepositoryId;
use temper_workflow::{ArtifactKindId, ArtifactSource, ExternalToolId, QueueId, RoleId};

struct TestRepo {
    path: PathBuf,
}

impl TestRepo {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "temper-coding-workspace-{name}-{}-{}",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("timestamp has nanos")
        ));
        fs::create_dir_all(&path).expect("temp repo dir creates");
        git(&path, &["init"]).expect("git init succeeds");
        git(&path, &["checkout", "-B", "main"]).expect("main branch exists");
        git(&path, &["config", "user.email", "temper@example.invalid"])
            .expect("git user email configured");
        git(&path, &["config", "user.name", "Temper Test"]).expect("git user name configured");
        fs::write(path.join("README.md"), "# Fixture\n").expect("README writes");
        git(&path, &["add", "README.md"]).expect("git add succeeds");
        git(&path, &["commit", "-m", "initial"]).expect("initial commit succeeds");
        Self { path }
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn request() -> CodingWorkspaceRequest {
    CodingWorkspaceRequest {
        repository: temper_runner::CodingWorkspaceRepository {
            id: RepositoryId::new("repo-1"),
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
        },
        work_item: temper_runner::CodingWorkspaceWorkItem {
            role: RoleId::new("engineer"),
            queue: QueueId::new("code_ready"),
            kind: ArtifactKindId::new("code"),
            target: ArtifactSource::Issue {
                number: temper_forge::ItemNumber::new(7),
            },
            context_json: "{\"artifact\":{\"title\":\"Implement docs\"}}".to_string(),
        },
        base_branch: "main".to_string(),
        branch_hint: "agent/pr-for-code-7".to_string(),
        correlation_key: "pr-for-code-7".to_string(),
        guidance: temper_runner::CodingWorkspaceGuidance {
            role_guidance: Some("Make a real product change.".to_string()),
            tool_guidance: Some("Use docs/product-change.md for this fixture.".to_string()),
            tool_constraints: vec!["No .temper-only diffs.".to_string()],
        },
    }
}

fn local_workspace(path: &Path, script: &str) -> LocalGitCodingWorkspace {
    LocalGitCodingWorkspace::new(
        path,
        vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()],
    )
    .with_push(false)
}

#[test]
fn local_git_workspace_accepts_product_code_or_docs_diff() {
    let repo = TestRepo::new("product");
    let workspace = local_workspace(
        &repo.path,
        "mkdir -p docs && printf 'real change\n' > docs/product-change.md",
    );

    let output = workspace
        .produce(request())
        .expect("product diff is accepted");

    assert_eq!(output.branch, "agent/pr-for-code-7");
    assert_eq!(output.base_branch, "main");
    assert_eq!(output.changed_files, vec!["docs/product-change.md"]);
    assert_eq!(
        output.labels,
        vec!["implementation", "needs-reviewer", "needs-merge"]
    );
    let head = git(&repo.path, &["log", "--oneline", "-1"]).expect("git log succeeds");
    assert!(head.contains("Implement pr-for-code-7"));
    assert!(changed_files(&repo.path)
        .expect("status succeeds")
        .is_empty());
}

#[test]
fn local_git_workspace_rejects_synthetic_only_diff() {
    let repo = TestRepo::new("synthetic");
    let workspace = local_workspace(
        &repo.path,
        "mkdir -p .temper-ci && printf 'marker\n' > .temper-ci/ok.txt",
    );

    let error = workspace
        .produce(request())
        .expect_err("bookkeeping-only diff is rejected");

    assert!(error.contains("no meaningful product diff"));
    assert!(error.contains(".temper-ci/ok.txt"));
}

#[test]
fn env_binding_is_absent_until_workspace_root_is_configured() {
    let workspace = LocalGitCodingWorkspace::from_env(|_| None).expect("empty env is valid");

    assert!(workspace.is_none());
}

#[test]
fn env_binding_requires_command_when_root_is_configured() {
    let error = LocalGitCodingWorkspace::from_env(|key| match key {
        WORKSPACE_ROOT_ENV => Some("/tmp/workspace".to_string()),
        _ => None,
    })
    .expect_err("partial workspace env fails");

    assert!(error.contains(WORKSPACE_COMMAND_ENV));
}

#[test]
fn env_binding_parses_labels_and_push_flag() {
    let workspace = LocalGitCodingWorkspace::from_env(|key| match key {
        WORKSPACE_ROOT_ENV => Some("/tmp/workspace".to_string()),
        WORKSPACE_COMMAND_ENV => Some("echo ok".to_string()),
        WORKSPACE_PUSH_ENV => Some("0".to_string()),
        WORKSPACE_PR_LABELS_ENV => Some("implementation,custom".to_string()),
        _ => None,
    })
    .expect("workspace env parses")
    .expect("workspace is configured");

    assert!(!workspace.push);
    assert_eq!(workspace.pr_labels, vec!["implementation", "custom"]);
    assert_eq!(workspace.command[0], "/bin/sh");
    assert_eq!(workspace.command[2], "echo ok");
}

#[test]
fn coding_workspace_tool_id_constant_matches_workflow_convention() {
    assert_eq!(
        temper_runner::CODING_WORKSPACE_TOOL_ID,
        ExternalToolId::new("coding_workspace").as_str()
    );
}
