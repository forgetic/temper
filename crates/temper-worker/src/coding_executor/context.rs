use temper_protocol_agent::{
    WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
};
use temper_protocol_worker::{
    ArtifactContextBundle, JobArtifactSnapshot, PullRequestFreshness as WorkerPullRequestFreshness,
    RepoAccess, WorkspaceManifest,
};
use temper_verdict::{SourceMetadata, VerdictContracts};

/// Assembles the typed [`WorkspaceContext`] the agent turn receives, listing
/// every manifest repo with its sibling dir and access (ADR 0023).
///
/// The [`OutOfProcessRunner`](crate::out_of_process_runner::OutOfProcessRunner)
/// serializes this to the JSON document the agent reads from
/// `$TEMPER_CODING_WORKSPACE_CONTEXT`; the struct (and thus the wire shape) is
/// owned by `temper-protocol-agent`. `work_item.context` stays a pretty-printed
/// JSON *string* of the artifact, surfaced to the model verbatim.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_workspace_context(
    role: &str,
    queue: &str,
    action: &str,
    artifact_kind: &str,
    manifest: &WorkspaceManifest,
    artifact: &JobArtifactSnapshot,
    artifact_context: Option<&ArtifactContextBundle>,
    artifact_wire_kind: &str,
    checkout: &str,
    allowed_verdicts: &[String],
    verdict_contracts: &VerdictContracts,
    source_metadata: &SourceMetadata,
    guidance: Option<&str>,
    pull_request_freshness: Option<&WorkerPullRequestFreshness>,
) -> WorkspaceContext {
    let (artifact_type, target_kind) = match artifact_wire_kind {
        "pull_request" => ("pull_request", "PullRequest"),
        _ => ("issue", "Issue"),
    };
    let primary_repo = manifest
        .repos
        .first()
        .map(|repo| repo.repo.clone())
        .unwrap_or_default();
    let work_item_context = serde_json::json!({
        "repository": primary_repo,
        "role": role,
        "queue": queue,
        "action": action,
        "kind": artifact_kind,
        "artifact": {
            "type": artifact_type,
            "number": artifact.number,
            "title": artifact.title.as_str(),
            "body": artifact.body.as_str(),
            "labels": &artifact.labels,
            "state": artifact.state.as_str(),
        }
    });
    // `to_string_pretty` on an in-memory `Value` is infallible; fall back to the
    // compact form rather than failing the job on the impossible error path.
    let work_item_context = serde_json::to_string_pretty(&work_item_context)
        .unwrap_or_else(|_| work_item_context.to_string());

    let repos = manifest
        .repos
        .iter()
        .map(|repo| {
            let (owner, name) = repo.owner_name().unwrap_or(("", ""));
            WorkspaceRepository {
                id: repo.repo.clone(),
                owner: owner.to_string(),
                name: name.to_string(),
                default_branch: repo.default_branch.clone(),
                dir: repo.dir.clone(),
                access: match repo.access {
                    RepoAccess::Writable => "writable",
                    RepoAccess::ReadOnly => "read_only",
                }
                .to_string(),
                base_branch: repo.base_branch.clone(),
                branch_hint: repo.branch_hint.clone(),
            }
        })
        .collect();

    WorkspaceContext {
        repos,
        work_item: WorkspaceWorkItem {
            role: role.to_string(),
            queue: queue.to_string(),
            kind: artifact_kind.to_string(),
            target: format!(
                "{target_kind} {{ number: ItemNumber({}) }}",
                artifact.number
            ),
            context: work_item_context,
        },
        artifact_context: artifact_context.cloned(),
        action: action.to_string(),
        correlation_key: manifest.coordination_key.clone(),
        checkout: Some(checkout.to_string()),
        allowed_verdicts: allowed_verdicts.to_vec(),
        verdict_contracts: verdict_contracts.clone(),
        source_metadata: source_metadata.clone(),
        guidance: WorkspaceGuidance {
            role_guidance: guidance.map(str::to_string),
            ..WorkspaceGuidance::default()
        },
        pull_request_freshness: pull_request_freshness.map(agent_pull_request_freshness),
        agent_session: None,
    }
}

fn agent_pull_request_freshness(
    freshness: &WorkerPullRequestFreshness,
) -> temper_protocol_agent::PullRequestFreshness {
    temper_protocol_agent::PullRequestFreshness {
        repository_id: freshness.repository_id.clone(),
        repo: freshness.repo.clone(),
        role: freshness.role.clone(),
        queue: freshness.queue.clone(),
        action: freshness.action.clone(),
        number: freshness.number,
        pull_request_id: freshness.pull_request_id.clone(),
        head_sha: freshness.head_sha.clone(),
        queue_condition: freshness.queue_condition.clone(),
        queue_labels: freshness.queue_labels.clone(),
    }
}
