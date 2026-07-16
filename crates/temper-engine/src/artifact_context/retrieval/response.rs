// SPDX-License-Identifier: MPL-2.0

//! Item loading and serialized response bounds for on-demand Forge reads.

use temper_forge::{Comment, ItemNumber, RepositoryId};
use temper_protocol_context::{
    ArtifactContextTruncation, ArtifactType, ForgeContextErrorCode, ForgeGetItemResult,
    ForgeItemComment, ForgeListRelatedResult,
};

use super::{MAX_COMMENT_BYTES, MAX_INNER_RESPONSE_BYTES};
use crate::artifact_context::catalog::ConfiguredRepositoryCatalog;
use crate::artifact_context::forge::ArtifactContextForge;
use crate::artifact_context::lineage::{ForgeItem, fetch, key};
use crate::artifact_context::projection::{drop_optional_child, drop_optional_child_state};

pub(super) async fn load_item<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    repository: &RepositoryId,
    number: u64,
    artifact_type: Option<ArtifactType>,
) -> Result<ForgeItem, ForgeContextErrorCode> {
    let number = ItemNumber::new(number);
    if let Some(artifact_type) = artifact_type {
        return fetch(forge, repository, artifact_type, number)
            .await
            .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
            .ok_or(ForgeContextErrorCode::NotFound);
    }
    if let Some(issue) = forge
        .issue(repository, number)
        .await
        .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
    {
        return Ok(ForgeItem::Issue(issue));
    }
    forge
        .pull_request(repository, number)
        .await
        .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
        .map(|pull_request| ForgeItem::PullRequest(Box::new(pull_request)))
        .ok_or(ForgeContextErrorCode::NotFound)
}

pub(super) async fn load_comments<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    item: &ForgeItem,
) -> Result<Vec<Comment>, ForgeContextErrorCode> {
    match item {
        ForgeItem::Issue(issue) => forge.issue_comments(&issue.id).await,
        ForgeItem::PullRequest(pull_request) => forge.pull_request_comments(&pull_request.id).await,
    }
    .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)
}

pub(super) fn bounded_comment(
    comment: Comment,
    truncation: &mut ArtifactContextTruncation,
) -> ForgeItemComment {
    let mut body = comment.body;
    if truncate_utf8(&mut body, MAX_COMMENT_BYTES) {
        truncation.content_truncated = true;
    }
    ForgeItemComment {
        id: comment.id.to_string(),
        author_id: comment.author_id.to_string(),
        body,
        created_at: comment.created_at.to_rfc3339(),
        updated_at: comment.updated_at.to_rfc3339(),
    }
}

pub(super) fn resolve_repository(
    catalog: &ConfiguredRepositoryCatalog,
    requested: &str,
) -> Result<(RepositoryId, temper_protocol_context::ArtifactRepository), ForgeContextErrorCode> {
    if let Some((id, repository)) = catalog.by_path(requested) {
        return Ok((id, repository.clone()));
    }
    let id = RepositoryId::new(requested.to_string());
    catalog
        .by_id(&id)
        .cloned()
        .map(|repository| (id, repository))
        .ok_or(ForgeContextErrorCode::NotAuthorized)
}

pub(super) fn validate_identity(repo: &str, number: u64) -> Result<(), ForgeContextErrorCode> {
    if repo.trim().is_empty() || repo.len() > 512 || number == 0 {
        Err(ForgeContextErrorCode::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn truncate_utf8(value: &mut String, maximum: usize) -> bool {
    if value.len() <= maximum {
        return false;
    }
    let mut boundary = maximum;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    true
}

pub(super) fn enforce_item_response_bound(
    result: &mut ForgeGetItemResult,
) -> Result<(), ForgeContextErrorCode> {
    while serialized_len(result) > MAX_INNER_RESPONSE_BYTES && !result.comments.is_empty() {
        result.comments.pop();
        result.truncation.count_exceeded = true;
    }
    while serialized_len(result) > MAX_INNER_RESPONSE_BYTES
        && (drop_optional_child_state(&mut result.item) || drop_optional_child(&mut result.item))
    {
        result.truncation.count_exceeded = true;
    }
    if serialized_len(result) > MAX_INNER_RESPONSE_BYTES && !result.item.body.is_empty() {
        let excess = serialized_len(result).saturating_sub(MAX_INNER_RESPONSE_BYTES);
        let target = result.item.body.len().saturating_sub(excess.max(1));
        truncate_utf8(&mut result.item.body, target);
        result.truncation.content_truncated = true;
    }
    if serialized_len(result) > MAX_INNER_RESPONSE_BYTES {
        return Err(ForgeContextErrorCode::LimitExceeded);
    }
    Ok(())
}

pub(super) fn enforce_related_response_bound(
    result: &mut ForgeListRelatedResult,
) -> Result<(), ForgeContextErrorCode> {
    while serialized_len(result) > MAX_INNER_RESPONSE_BYTES && !result.items.is_empty() {
        let removed = result.items.pop().expect("checked non-empty");
        let removed_key = key(&removed.artifact);
        result
            .edges
            .retain(|edge| key(&edge.source) != removed_key && key(&edge.target) != removed_key);
        result.truncation.count_exceeded = true;
    }
    if serialized_len(result) > MAX_INNER_RESPONSE_BYTES {
        return Err(ForgeContextErrorCode::LimitExceeded);
    }
    Ok(())
}

fn serialized_len<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .expect("Forge context DTO always serializes")
        .len()
}

#[cfg(test)]
mod tests {
    use temper_protocol_context::{
        ArtifactReference, ArtifactRepository, ArtifactSnapshot, ArtifactWorkflowContext,
        WorkflowChildIdentity,
    };

    use super::*;

    #[test]
    fn item_response_drops_optional_children_before_authored_body() {
        let authored = "mandatory authored body".repeat(1_024);
        let mut result = ForgeGetItemResult {
            item: ArtifactSnapshot {
                artifact: ArtifactReference {
                    repository: ArtifactRepository {
                        id: "forge:ai/temper".into(),
                        path: "ai/temper".into(),
                    },
                    artifact_type: ArtifactType::Issue,
                    number: 1,
                },
                title: "primary".into(),
                body: authored.clone(),
                labels: Vec::new(),
                state: "open".into(),
                workflow_kind: Some("plan".into()),
                workflow: Some(ArtifactWorkflowContext {
                    kind: Some("plan".into()),
                    children: vec![WorkflowChildIdentity {
                        repository_id: "forge:ai/temper".into(),
                        number: 2,
                        title: "x".repeat(MAX_INNER_RESPONSE_BYTES),
                        state: Some("open".into()),
                    }],
                    ..Default::default()
                }),
            },
            comments: Vec::new(),
            truncation: ArtifactContextTruncation::default(),
        };

        enforce_item_response_bound(&mut result).unwrap();

        assert_eq!(result.item.body, authored);
        assert!(result.item.workflow.as_ref().unwrap().children.is_empty());
        assert!(result.truncation.count_exceeded);
        assert!(!result.truncation.content_truncated);
        assert!(serialized_len(&result) <= MAX_INNER_RESPONSE_BYTES);
    }
}
