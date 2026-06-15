// SPDX-License-Identifier: MPL-2.0

//! Workspace-manifest assembly and artifact snapshots for the work-item feed.
//!
//! A coding job's [`WorkspaceManifest`] is the primary writable repo plus any
//! additional repos a coordinating issue declares in a `temper:workspace`
//! metadata block (ADR 0023). Snapshots capture the current issue/PR state the
//! worker-side agent reads, skipping artifacts that have reached a terminal
//! state.

use temper_forge_model::{
    Forge, ForgeError, Issue, IssueState, ItemNumber, PullRequest, PullRequestState, Repository,
    RepositoryId, RepositoryPath,
};
use temper_runner::ScanError;
use temper_worker_protocol::{JobArtifactSnapshot, RepoAccess, WorkspaceManifest, WorkspaceRepo};
use temper_workflow::ArtifactSource;

use crate::workflow_meta::default_base_branch;

/// The workspace directory a repo is checked out into: its `name` segment, so
/// the flat sibling layout matches the inter-repo path dependencies (ADR 0023).
fn repo_dir(repo_path: &str) -> String {
    repo_path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(repo_path)
        .to_string()
}

/// One repo of a `temper:workspace` declaration on a coordinating issue.
struct WorkspaceRepoDecl {
    repo: String,
    access: RepoAccess,
    depends_on: Vec<String>,
}

/// Parses an optional `temper:workspace` metadata block from an issue body:
///
/// ```text
/// <!-- temper:workspace
/// {"repos":[{"repo":"ai/temper","access":"writable"},
///           {"repo":"ai/smith","access":"writable","depends_on":["ai/temper"]},
///           {"repo":"ai/skein","access":"read_only"}]}
/// -->
/// ```
///
/// `depends_on` lists other repos whose PR must land first (coordinated landing
/// order). Returns `None` when no (well-formed) block is present, in which case
/// the job gets a degenerate single-repo manifest.
fn parse_workspace_decl(body: &str) -> Option<Vec<WorkspaceRepoDecl>> {
    const OPEN: &str = "<!-- temper:workspace";
    let start = body.find(OPEN)?;
    let rest = &body[start + OPEN.len()..];
    let end = rest.find("-->")?;
    let json = rest[..end].trim();
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let repos = value.get("repos")?.as_array()?;

    let mut decls = Vec::new();
    for entry in repos {
        let repo = entry.get("repo")?.as_str()?.to_string();
        let access = match entry.get("access").and_then(serde_json::Value::as_str) {
            Some("read_only") => RepoAccess::ReadOnly,
            _ => RepoAccess::Writable,
        };
        let depends_on = entry
            .get("depends_on")
            .and_then(serde_json::Value::as_array)
            .map(|deps| {
                deps.iter()
                    .filter_map(|dep| dep.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        decls.push(WorkspaceRepoDecl {
            repo,
            access,
            depends_on,
        });
    }
    Some(decls)
}

/// Builds the [`WorkspaceManifest`] for a coding job: the primary writable repo
/// first, then each additional declared repo (resolving its default branch from
/// the Forge). All writable repos share the one coordination branch; read-only
/// repos carry no branch hint and are never pushed (ADR 0023). `branch_hint` is
/// derived from the coordinating issue and shared by every writable repo.
pub(super) async fn build_workspace_manifest<F: Forge + ?Sized>(
    forge: &F,
    primary: &Repository,
    primary_path: &str,
    coordination_key: &str,
    branch_hint: &str,
    body: &str,
) -> Result<WorkspaceManifest, ScanError> {
    let declared = parse_workspace_decl(body);
    let primary_base = default_base_branch(primary);
    let mut repos = vec![WorkspaceRepo {
        repo: primary_path.to_string(),
        dir: repo_dir(primary_path),
        access: RepoAccess::Writable,
        default_branch: primary_base.clone(),
        base_branch: primary_base,
        branch_hint: Some(branch_hint.to_string()),
        // The primary's landing order is taken from its own declaration entry,
        // applied below.
        depends_on: Vec::new(),
    }];

    for decl in declared.into_iter().flatten() {
        // The primary is always present (and writable) regardless of how the
        // declaration lists it; fold its declared landing order onto the
        // existing primary entry rather than adding a duplicate.
        if decl.repo == primary_path {
            repos[0].depends_on = decl.depends_on;
            continue;
        }
        let Some((owner, name)) = decl.repo.split_once('/') else {
            return Err(ScanError::Forge(ForgeError::NotFound(format!(
                "malformed workspace repo path {}",
                decl.repo
            ))));
        };
        let repository = forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await?
            .ok_or_else(|| {
                ScanError::Forge(ForgeError::NotFound(format!(
                    "workspace repository {}",
                    decl.repo
                )))
            })?;
        let base = default_base_branch(&repository);
        let branch_hint = match decl.access {
            RepoAccess::Writable => Some(branch_hint.to_string()),
            RepoAccess::ReadOnly => None,
        };
        repos.push(WorkspaceRepo {
            repo: decl.repo,
            dir: repo_dir(&repository.name),
            access: decl.access,
            default_branch: base.clone(),
            base_branch: base,
            branch_hint,
            depends_on: decl.depends_on,
        });
    }

    Ok(WorkspaceManifest {
        coordination_key: coordination_key.to_string(),
        repos,
    })
}

pub(super) fn target_number(target: ArtifactSource) -> ItemNumber {
    match target {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

pub(super) async fn terminal_checked_snapshot<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
) -> Result<Option<JobArtifactSnapshot>, ScanError> {
    match target {
        ArtifactSource::Issue { number } => {
            let issue = forge
                .get_issue_by_number(repo, number)
                .await?
                .ok_or_else(|| ScanError::Forge(ForgeError::NotFound(format!("issue {number}"))))?;
            if issue.state != IssueState::Open {
                return Ok(None);
            }
            Ok(Some(snapshot_from_issue(issue)))
        }
        ArtifactSource::PullRequest { number } => {
            let pull_request = forge
                .get_pull_request_by_number(repo, number)
                .await?
                .ok_or_else(|| {
                    ScanError::Forge(ForgeError::NotFound(format!("pull request {number}")))
                })?;
            if pull_request.state != PullRequestState::Open {
                return Ok(None);
            }
            Ok(Some(snapshot_from_pull_request(pull_request)))
        }
    }
}

fn snapshot_from_issue(issue: Issue) -> JobArtifactSnapshot {
    JobArtifactSnapshot {
        number: issue.number.get(),
        title: issue.title,
        body: issue.body,
        labels: issue.labels,
        state: format!("{:?}", issue.state),
    }
}

fn snapshot_from_pull_request(pull_request: PullRequest) -> JobArtifactSnapshot {
    JobArtifactSnapshot {
        number: pull_request.number.get(),
        title: pull_request.title,
        body: pull_request.body,
        labels: pull_request.labels,
        state: format!("{:?}", pull_request.state),
    }
}
