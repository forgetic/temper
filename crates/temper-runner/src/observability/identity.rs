//! Stable, provider-neutral observability identities for work items.
//!
//! [`WorkItemIdentity`] survives the move to `temper-log` as the runner-internal
//! carrier of a work item's role/queue/decision-id correlation (used for
//! role-decision routing and for building the design's `artifact.ref` join key
//! via [`work_item_ref`](crate::observability::work_item_ref)). Its old
//! JSON-rendering (`to_json`) is gone — the machine projection is now real
//! `tracing` fields emitted by `temper_log::emit::*`.

use temper_forge::{ItemNumber, RepositoryId};
use temper_workflow::{ArtifactKindId, ArtifactSource, QueueId, RoleId};

/// Artifact target type used in observability identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservabilityArtifactType {
    /// Forge issue artifact.
    Issue,
    /// Forge pull request artifact.
    PullRequest,
}

impl ObservabilityArtifactType {
    /// Stable provider-neutral string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::PullRequest => "pull_request",
        }
    }

    fn from_source(source: ArtifactSource) -> Self {
        match source {
            ArtifactSource::Issue { .. } => Self::Issue,
            ArtifactSource::PullRequest { .. } => Self::PullRequest,
        }
    }
}

/// Stable identity for one workflow work item or role-decision request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkItemIdentity {
    /// Deterministic id derived only from provider-neutral coordinates.
    pub work_item_id: String,
    /// Deterministic decision id for correlating role responders with Temper.
    pub decision_id: String,
    /// Optional tick id when a caller has one available.
    pub tick_id: Option<String>,
    /// Backend-neutral repository id used by the Forge trait.
    pub repo: RepositoryId,
    /// Workflow role handling the item.
    pub role: RoleId,
    /// Queue that selected the item.
    pub queue: QueueId,
    /// Forge artifact type.
    pub artifact_type: ObservabilityArtifactType,
    /// Repository-local artifact number.
    pub artifact_number: ItemNumber,
    /// Workflow artifact kind resolved by classification.
    pub artifact_kind: ArtifactKindId,
}

impl WorkItemIdentity {
    /// Builds a deterministic identity from workflow and Forge coordinates.
    pub fn new(
        repo: &RepositoryId,
        role: &RoleId,
        queue: &QueueId,
        target: ArtifactSource,
        artifact_kind: &ArtifactKindId,
    ) -> Self {
        let artifact_type = ObservabilityArtifactType::from_source(target);
        let artifact_number = artifact_number(target);
        let work_item_id = work_item_id(
            repo.as_str(),
            role.as_str(),
            queue.as_str(),
            artifact_type,
            artifact_number,
            artifact_kind.as_str(),
        );
        let decision_id = format!("decision/{work_item_id}");
        Self {
            work_item_id,
            decision_id,
            tick_id: None,
            repo: repo.clone(),
            role: role.clone(),
            queue: queue.clone(),
            artifact_type,
            artifact_number,
            artifact_kind: artifact_kind.clone(),
        }
    }

    /// Returns the same identity associated with a caller-supplied tick id.
    pub fn with_tick_id(mut self, tick_id: impl Into<String>) -> Self {
        let tick_id = tick_id.into();
        self.decision_id = format!(
            "decision/{}/{}",
            length_prefixed("tick", &tick_id),
            self.work_item_id
        );
        self.tick_id = Some(tick_id);
        self
    }
}

fn artifact_number(source: ArtifactSource) -> ItemNumber {
    match source {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

fn work_item_id(
    repo: &str,
    role: &str,
    queue: &str,
    artifact_type: ObservabilityArtifactType,
    artifact_number: ItemNumber,
    artifact_kind: &str,
) -> String {
    format!(
        "work-item/{}/{}/kind:{}/queue:{}/role:{}",
        length_prefixed("repo", repo),
        artifact_segment(artifact_type, artifact_number),
        length_value(artifact_kind),
        length_value(queue),
        length_value(role)
    )
}

fn artifact_segment(
    artifact_type: ObservabilityArtifactType,
    artifact_number: ItemNumber,
) -> String {
    format!(
        "artifact:{}:{}",
        artifact_type.as_str(),
        artifact_number.get()
    )
}

fn length_prefixed(label: &str, value: &str) -> String {
    format!("{label}:{}:{value}", value.len())
}

fn length_value(value: &str) -> String {
    format!("{}:{value}", value.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_item_identity_renders_stable_fields() {
        let identity = WorkItemIdentity::new(
            &RepositoryId::new("forgejo:acme/service"),
            &RoleId::new("banana"),
            &QueueId::new("todo"),
            ArtifactSource::Issue {
                number: ItemNumber::new(7),
            },
            &ArtifactKindId::new("task"),
        );

        assert_eq!(identity.artifact_type.as_str(), "issue");
        assert_eq!(identity.artifact_number.get(), 7);
        assert_eq!(
            identity.work_item_id,
            "work-item/repo:20:forgejo:acme/service/artifact:issue:7/kind:4:task/queue:4:todo/role:6:banana"
        );
        assert_eq!(
            identity.decision_id,
            "decision/work-item/repo:20:forgejo:acme/service/artifact:issue:7/kind:4:task/queue:4:todo/role:6:banana"
        );
        assert!(identity.tick_id.is_none());
    }

    #[test]
    fn work_item_identity_can_include_tick_id() {
        let identity = WorkItemIdentity::new(
            &RepositoryId::new("repo-1"),
            &RoleId::new("engineer"),
            &QueueId::new("ready"),
            ArtifactSource::PullRequest {
                number: ItemNumber::new(2),
            },
            &ArtifactKindId::new("implementation_pr"),
        )
        .with_tick_id("tick-42");

        assert_eq!(identity.tick_id.as_deref(), Some("tick-42"));
        assert!(identity.decision_id.contains("tick:7:tick-42"));
        assert_eq!(
            identity.artifact_type,
            ObservabilityArtifactType::PullRequest
        );
    }
}
