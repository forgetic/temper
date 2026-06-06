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
    assert_eq!(output.labels, vec!["implementation", "needs-reviewer"]);
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
fn workspace_result_parses_full_protocol() {
    let result = WorkspaceResult::parse(
        r#"{
            "verdict": "ready_code",
            "summary": "one-line summary",
            "body": "rewritten issue body",
            "review_body": "review prose",
            "labels": ["implementation"],
            "children": [{"title": "child"}]
        }"#,
    )
    .expect("valid protocol parses")
    .expect("non-empty file yields a result");

    assert_eq!(result.verdict.as_deref(), Some("ready_code"));
    assert_eq!(result.summary.as_deref(), Some("one-line summary"));
    assert_eq!(result.body.as_deref(), Some("rewritten issue body"));
    assert_eq!(result.review_body.as_deref(), Some("review prose"));
    assert_eq!(result.labels, Some(vec!["implementation".to_string()]));
    assert_eq!(result.children.len(), 1);
    assert_eq!(
        result.verdict_id(),
        Some(temper_workflow::VerdictId::new("ready_code"))
    );
}

#[test]
fn workspace_result_empty_file_is_absent() {
    assert_eq!(WorkspaceResult::parse("   \n").expect("blank parses"), None);
    assert_eq!(WorkspaceResult::parse("").expect("empty parses"), None);

    // An empty JSON object is a present-but-inert result: no verdict, no
    // overrides — the head path stays in effect.
    let result = WorkspaceResult::parse("{}")
        .expect("empty object parses")
        .expect("empty object is a present result");
    assert_eq!(result, WorkspaceResult::default());
    assert_eq!(result.verdict_id(), None);
}

#[test]
fn workspace_result_rejects_unknown_fields() {
    let error =
        WorkspaceResult::parse(r#"{"verdct": "typo"}"#).expect_err("unknown field is rejected");
    assert!(error.contains("failed to parse coding workspace result file"));
}

#[test]
fn no_result_file_keeps_head_path_unchanged() {
    let repo = TestRepo::new("no-result-head");
    // The command writes a real diff and never touches the result file.
    let workspace = local_workspace(
        &repo.path,
        "mkdir -p docs && printf 'real change\n' > docs/product-change.md",
    );

    let output = workspace
        .produce(request())
        .expect("head path still opens a PR");

    assert!(output.verdict.is_none());
    assert_eq!(output.branch, "agent/pr-for-code-7");
    assert_eq!(output.changed_files, vec!["docs/product-change.md"]);
    assert_eq!(output.labels, vec!["implementation", "needs-reviewer"]);
    let head = git(&repo.path, &["log", "--oneline", "-1"]).expect("git log succeeds");
    assert!(head.contains("Implement pr-for-code-7"));
    assert!(changed_files(&repo.path)
        .expect("status succeeds")
        .is_empty());
}

#[test]
fn empty_result_file_keeps_head_path_unchanged() {
    let repo = TestRepo::new("empty-result-head");
    // Writing an empty result file is equivalent to writing none at all.
    let workspace = local_workspace(
        &repo.path,
        "mkdir -p docs && printf 'real change\n' > docs/product-change.md && \
         : > \"$TEMPER_CODING_WORKSPACE_RESULT\"",
    );

    let output = workspace
        .produce(request())
        .expect("empty result keeps the head path");

    assert!(output.verdict.is_none());
    assert_eq!(output.changed_files, vec!["docs/product-change.md"]);
    let head = git(&repo.path, &["log", "--oneline", "-1"]).expect("git log succeeds");
    assert!(head.contains("Implement pr-for-code-7"));
}

#[test]
fn verdict_result_skips_commit_push_and_tolerates_empty_tree() {
    let repo = TestRepo::new("verdict-approve");
    let head_before = git(&repo.path, &["rev-parse", "HEAD"]).expect("HEAD resolves");
    // Approve with no diff: the canonical reviewer path.
    let workspace = local_workspace(
        &repo.path,
        "printf '{\"verdict\":\"approve\"}' > \"$TEMPER_CODING_WORKSPACE_RESULT\"",
    );

    let output = workspace
        .produce(request())
        .expect("verdict path tolerates an empty tree");

    assert_eq!(
        output.verdict,
        Some(temper_workflow::VerdictId::new("approve"))
    );
    assert!(output.branch.is_empty(), "verdict output carries no branch");
    assert!(output.changed_files.is_empty());
    assert_eq!(output.base_branch, "main");

    // No commit and no push happened: HEAD is unchanged.
    let head_after = git(&repo.path, &["rev-parse", "HEAD"]).expect("HEAD resolves");
    assert_eq!(head_before, head_after, "verdict path makes no commit");
}

#[test]
fn verdict_result_carries_body_and_review_body() {
    let repo = TestRepo::new("verdict-content");
    let workspace = local_workspace(
        &repo.path,
        "printf '%s' '{\"verdict\":\"ready_code\",\"body\":\"new body\",\
         \"review_body\":\"looks good\"}' > \"$TEMPER_CODING_WORKSPACE_RESULT\"",
    );

    let output = workspace.produce(request()).expect("verdict path succeeds");

    assert_eq!(
        output.verdict,
        Some(temper_workflow::VerdictId::new("ready_code"))
    );
    assert_eq!(output.body.as_deref(), Some("new body"));
    assert_eq!(output.review_body.as_deref(), Some("looks good"));
}

#[test]
fn head_path_applies_labels_and_summary_override() {
    let repo = TestRepo::new("head-override");
    let workspace = local_workspace(
        &repo.path,
        "mkdir -p docs && printf 'real change\n' > docs/product-change.md && \
         printf '%s' '{\"summary\":\"custom summary\",\"labels\":[\"docs\",\"fast-track\"]}' \
         > \"$TEMPER_CODING_WORKSPACE_RESULT\"",
    );

    let output = workspace
        .produce(request())
        .expect("override flows into the head output");

    assert!(output.verdict.is_none());
    assert_eq!(output.summary, "custom summary");
    assert_eq!(output.labels, vec!["docs", "fast-track"]);
    // Override does not bypass the head flow: a real PR branch is still produced.
    assert_eq!(output.branch, "agent/pr-for-code-7");
    assert_eq!(output.changed_files, vec!["docs/product-change.md"]);
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
