//! Dogfood-style workspace proof against a throwaway Forgejo.
//!
//! The test does not call an LLM. It proves the production local-git coding
//! workspace can push a real branch to Forgejo, open a PR through the Forge
//! interface, and produce a changed-file list that the dogfood PR diff guard
//! classifies as meaningful.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;

use temper_forge::{BranchRef, CreateIssue, CreatePullRequest};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_production::coding_workspace::LocalGitCodingWorkspace;
use temper_production::forgejo_rest;
use temper_production::pr_diff_guard::{safety_for_files, DiffSafety};
use temper_runner::{
    CodingWorkspace, CodingWorkspaceGuidance, CodingWorkspaceRepository, CodingWorkspaceRequest,
    CodingWorkspaceWorkItem, CODING_WORKSPACE_TOOL_ID,
};
use temper_testing::forgejo_server::{provision, ForgejoServer};
use temper_workflow::{
    render_metadata_block, ArtifactKindId, ArtifactRef, ArtifactSource, QueueId, RoleId,
    WorkflowMetadata,
};

#[test]
#[ignore = "boots a real Forgejo; run with --ignored"]
fn local_git_workspace_pushes_meaningful_pr_diff() {
    let server = ForgejoServer::start().expect("forgejo server boots");
    let provisioned = block_on(provision(&server)).expect("provisioning succeeds");
    let engineer = provisioned
        .role(&RoleId::new("engineer"))
        .expect("engineer identity is provisioned")
        .clone();
    let forge = ForgejoForge::new(
        ForgejoConfig::new(server.base_url(), &engineer.token)
            .with_default_repo(&provisioned.owner, &provisioned.name)
            .with_web_ui_credentials(&engineer.user, &engineer.password),
    );

    let issue = block_on(forge.create_issue(
        &provisioned.repository,
        CreateIssue {
            title: "Prove coding workspace PR path".into(),
            body: "A ready code issue for the workspace proof.".into(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("code issue creates");

    let temp = TempDir::new("temper-forgejo-workspace-pr");
    let checkout = temp.path.join("checkout");
    let remote = format!(
        "{}/{}/{}.git",
        server.base_url().trim_end_matches('/'),
        provisioned.owner,
        provisioned.name
    );
    git(
        temp.path(),
        &["clone", &remote, checkout.to_str().expect("utf8 path")],
    );
    git(
        &checkout,
        &["config", "user.email", "engineer@example.invalid"],
    );
    git(&checkout, &["config", "user.name", "Temper Engineer"]);
    let credentials = write_git_credentials(
        temp.path(),
        server.base_url(),
        &engineer.user,
        &engineer.password,
    );
    git(
        &checkout,
        &[
            "config",
            "credential.helper",
            &format!("store --file={}", credentials.display()),
        ],
    );

    let repo = block_on(forge.get_repository(&provisioned.repository))
        .expect("repository lookup succeeds")
        .expect("repository exists");
    let workspace = LocalGitCodingWorkspace::new(
        &checkout,
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "mkdir -p src && printf 'workspace proof\\n' > src/banner.txt".to_string(),
        ],
    );
    let output = block_on(workspace.produce_head(CodingWorkspaceRequest {
        repository: CodingWorkspaceRepository {
            id: repo.id.clone(),
            owner: repo.owner.clone(),
            name: repo.name.clone(),
            default_branch: repo.default_branch.clone(),
        },
        work_item: CodingWorkspaceWorkItem {
            role: RoleId::new("engineer"),
            queue: QueueId::new("code_ready"),
            kind: ArtifactKindId::new("code"),
            target: ArtifactSource::Issue {
                number: issue.number,
            },
            context_json: "{\"issue\":\"workspace proof\"}".to_string(),
        },
        base_branch: repo.default_branch.clone(),
        branch_hint: format!("agent/pr-for-code-{}", issue.number.get()),
        correlation_key: format!("pr-for-code-{}", issue.number.get()),
        guidance: CodingWorkspaceGuidance {
            role_guidance: Some("Make a real product change.".to_string()),
            tool_guidance: Some(format!("Use {CODING_WORKSPACE_TOOL_ID} for the PR head.")),
            tool_constraints: vec!["No bookkeeping-only diffs.".to_string()],
        },
    }))
    .expect("workspace produces and pushes a head");
    assert_eq!(output.changed_files, vec!["src/banner.txt"]);

    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(issue.number)],
        ..WorkflowMetadata::default()
    };
    let pr = block_on(forge.create_pull_request(
        &provisioned.repository,
        CreatePullRequest {
            title: format!("Implement #{}: {}", issue.number.get(), issue.title),
            body: format!(
                "Workspace-produced implementation.\n\n{}",
                render_metadata_block(&metadata)
            ),
            source: BranchRef {
                repository_id: provisioned.repository.clone(),
                branch: output.branch,
            },
            target: BranchRef {
                repository_id: provisioned.repository.clone(),
                branch: output.base_branch,
            },
            labels: output.labels,
            assignees: Vec::new(),
        },
    ))
    .expect("PR opens from workspace branch");

    let files = block_on(async {
        let client = forgejo_rest::http_client().expect("HTTP client builds");
        forgejo_rest::list_pull_request_files(
            &client,
            server.base_url(),
            &engineer.token,
            &provisioned.owner,
            &provisioned.name,
            pr.number.get(),
        )
        .await
    })
    .expect("PR files list is readable");
    assert_eq!(
        safety_for_files(files.clone()),
        DiffSafety::Meaningful { files }
    );
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("timestamp has nanos")
        ));
        std::fs::create_dir_all(&path).expect("temp dir creates");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_git_credentials(root: &Path, base_url: &str, user: &str, password: &str) -> PathBuf {
    let without_scheme = base_url
        .trim_end_matches('/')
        .strip_prefix("http://")
        .expect("throwaway Forgejo uses http");
    let credentials = root.join("git-credentials");
    std::fs::write(
        &credentials,
        format!(
            "http://{}:{}@{}\n",
            percent_encode_userinfo(user),
            percent_encode_userinfo(password),
            without_scheme
        ),
    )
    .expect("credential file writes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))
            .expect("credential file is private");
    }
    credentials
}

fn percent_encode_userinfo(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    if !output.status.success() {
        panic!(
            "git {} failed with {}; stderr: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}
