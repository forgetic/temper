// SPDX-License-Identifier: MPL-2.0

//! Deterministic, bounded initial artifact context collection.
//!
//! Repository authorization comes exclusively from a startup-built
//! [`ConfiguredRepositoryCatalog`]. The resolver classifies the primary under
//! the validated workflow, follows declared parent relations, and treats every
//! non-primary read failure as a diagnostic so job dispatch can continue with a
//! useful partial bundle.

mod bounds;
mod catalog;
mod extras;
mod forge;
mod lineage;
mod markdown;
mod projection;
mod retrieval;

#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use temper_forge::{Forge, RepositoryId};
use temper_protocol_worker::{
    ArtifactContextBundle, ArtifactContextDiagnosticCode, ArtifactSnapshot,
};
use temper_workflow::{ArtifactSource, ClassifiedArtifact, ValidatedWorkflow};

pub use catalog::ConfiguredRepositoryCatalog;
pub use forge::ArtifactContextForge;
pub(crate) use projection::{SnapshotInput, project_snapshot};
pub use retrieval::{
    ArtifactContextService, DEFAULT_RELATED_DEPTH, DEFAULT_RELATED_RESULTS, MAX_COMMENT_BYTES,
    MAX_FORGE_RESPONSE_BYTES, MAX_ITEM_BODY_BYTES, MAX_ITEM_COMMENTS, MAX_RELATED_DEPTH,
    MAX_RELATED_RESULTS, validate_context_operation,
};

/// Shared startup-constructed resolver used by every dispatch path.
///
/// Keeping the Forge handle, validated workflow, repository allow-list, URL
/// parsing policy, and collection bounds together prevents poll, webhook, and
/// recovery dispatches from drifting into subtly different graph reads.
#[derive(Clone)]
pub struct ArtifactContextBundleService {
    forge: Arc<dyn Forge>,
    workflow: Arc<ValidatedWorkflow>,
    catalog: ConfiguredRepositoryCatalog,
    policy: ArtifactContextPolicy,
}

impl ArtifactContextBundleService {
    pub fn new(
        forge: Arc<dyn Forge>,
        workflow: Arc<ValidatedWorkflow>,
        catalog: ConfiguredRepositoryCatalog,
        policy: ArtifactContextPolicy,
    ) -> Self {
        Self {
            forge,
            workflow,
            catalog,
            policy,
        }
    }

    /// Resolves the bundle selected for the already-resolved workflow action.
    /// Aggregate child and implementation-PR summaries are intentionally
    /// collected only for plan validation; all actions receive mandatory
    /// ancestry and safe markdown references.
    pub async fn resolve(
        &self,
        repository: &RepositoryId,
        source: ArtifactSource,
        action: &str,
    ) -> Result<ArtifactContextBundle, ArtifactContextError> {
        resolve_initial_artifact_context_for_action_with_policy(
            self.forge.as_ref(),
            &self.catalog,
            self.workflow.as_ref(),
            repository,
            source,
            action,
            self.policy,
        )
        .await
    }

    /// Resolves an action-selected bundle while reusing the exact primary
    /// representation validated by an item-scoped scan.
    pub async fn resolve_with_primary(
        &self,
        repository: &RepositoryId,
        action: &str,
        primary: ArtifactSnapshot,
        classified: ClassifiedArtifact,
    ) -> Result<ArtifactContextBundle, ArtifactContextError> {
        resolve_initial_artifact_context_for_action_with_primary(
            self.forge.as_ref(),
            &self.catalog,
            self.workflow.as_ref(),
            repository,
            action,
            primary,
            classified,
            self.policy,
        )
        .await
    }

    pub fn catalog(&self) -> &ConfiguredRepositoryCatalog {
        &self.catalog
    }
}

pub const DEFAULT_LINEAGE_DEPTH: usize = 8;
pub const DEFAULT_FULL_SNAPSHOTS: usize = 16;
pub const DEFAULT_SUMMARIES: usize = 100;
pub const DEFAULT_BODY_BYTES: usize = 256 * 1024;
pub const DEFAULT_BUNDLE_BYTES: usize = 512 * 1024;

/// Centralized artifact-context limits. Defaults are the production policy;
/// explicit values make boundary behavior cheap to test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactContextPolicy {
    pub lineage_depth: usize,
    pub full_snapshots: usize,
    pub summaries: usize,
    pub body_bytes: usize,
    pub bundle_bytes: usize,
}

impl Default for ArtifactContextPolicy {
    fn default() -> Self {
        Self {
            lineage_depth: DEFAULT_LINEAGE_DEPTH,
            full_snapshots: DEFAULT_FULL_SNAPSHOTS,
            summaries: DEFAULT_SUMMARIES,
            body_bytes: DEFAULT_BODY_BYTES,
            bundle_bytes: DEFAULT_BUNDLE_BYTES,
        }
    }
}

/// Hard enrichment failure. Only primary lookup/classification and an
/// unauthorized primary repository produce this error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactContextError {
    Primary(String),
}

impl fmt::Display for ArtifactContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary(message) => {
                write!(formatter, "primary artifact context failed: {message}")
            }
        }
    }
}

impl Error for ArtifactContextError {}

/// Resolves a production-policy initial bundle.
pub async fn resolve_initial_artifact_context<F: Forge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository: &RepositoryId,
    source: ArtifactSource,
) -> Result<ArtifactContextBundle, ArtifactContextError> {
    resolve_initial_artifact_context_with_policy(
        forge,
        catalog,
        workflow,
        repository,
        source,
        ArtifactContextPolicy::default(),
    )
    .await
}

/// Resolves an initial bundle under explicit limits.
pub async fn resolve_initial_artifact_context_with_policy<F: Forge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository: &RepositoryId,
    source: ArtifactSource,
    policy: ArtifactContextPolicy,
) -> Result<ArtifactContextBundle, ArtifactContextError> {
    resolve_initial_artifact_context_selected(
        forge, catalog, workflow, repository, source, policy, true,
    )
    .await
}

/// Resolves a bundle after workflow action selection. Only `validate_plan`
/// needs potentially broad child/implementation evidence; other actions avoid
/// those aggregate list queries while retaining ancestry and references.
pub async fn resolve_initial_artifact_context_for_action_with_policy<F: Forge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository: &RepositoryId,
    source: ArtifactSource,
    action: &str,
    policy: ArtifactContextPolicy,
) -> Result<ArtifactContextBundle, ArtifactContextError> {
    resolve_initial_artifact_context_selected(
        forge,
        catalog,
        workflow,
        repository,
        source,
        policy,
        action == "validate_plan",
    )
    .await
}

/// Resolves an action-selected bundle from an already validated primary
/// snapshot. Lineage, declared dependency, and reference expansion retain the
/// ordinary bounds, but the coordinating artifact is not fetched again.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_initial_artifact_context_for_action_with_primary<F: Forge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository: &RepositoryId,
    action: &str,
    primary: ArtifactSnapshot,
    classified: ClassifiedArtifact,
    policy: ArtifactContextPolicy,
) -> Result<ArtifactContextBundle, ArtifactContextError> {
    let source = classified.source;
    resolve_initial_artifact_context_selected_with_primary(
        forge,
        catalog,
        workflow,
        repository,
        source,
        policy,
        action == "validate_plan",
        Some((primary, classified)),
    )
    .await
}

async fn resolve_initial_artifact_context_selected<F: Forge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository: &RepositoryId,
    source: ArtifactSource,
    policy: ArtifactContextPolicy,
    include_validation_aggregates: bool,
) -> Result<ArtifactContextBundle, ArtifactContextError> {
    resolve_initial_artifact_context_selected_with_primary(
        forge,
        catalog,
        workflow,
        repository,
        source,
        policy,
        include_validation_aggregates,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resolve_initial_artifact_context_selected_with_primary<F: Forge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository: &RepositoryId,
    source: ArtifactSource,
    policy: ArtifactContextPolicy,
    include_validation_aggregates: bool,
    primary: Option<(ArtifactSnapshot, ClassifiedArtifact)>,
) -> Result<ArtifactContextBundle, ArtifactContextError> {
    let collection = match primary {
        Some((snapshot, classified)) => {
            lineage::collect_lineage_from_primary(
                forge, catalog, workflow, repository, snapshot, classified, policy,
            )
            .await?
        }
        None => {
            lineage::collect_lineage(forge, catalog, workflow, repository, source, policy).await?
        }
    };
    let mut references = extras::collect_references(forge, catalog, workflow, &collection).await;
    let mut validation = if include_validation_aggregates {
        extras::collect_validation_aggregates(forge, catalog, workflow, &collection).await
    } else {
        extras::Extras {
            summaries: Vec::new(),
            diagnostics: Vec::new(),
        }
    };
    let mut bundle = lineage::ordered_bundle(collection);
    bundle.diagnostics.append(&mut validation.diagnostics);
    bundle.diagnostics.append(&mut references.diagnostics);

    let total_summaries = validation.summaries.len() + references.summaries.len();
    if validation.summaries.len() >= policy.summaries {
        validation.summaries.truncate(policy.summaries);
        references.summaries.clear();
    } else {
        references
            .summaries
            .truncate(policy.summaries - validation.summaries.len());
    }
    bundle.validation_scope = validation.summaries;
    bundle.optional_references = references.summaries;
    projection::attach_available_child_states(&mut bundle);
    if total_summaries > policy.summaries {
        bundle.diagnostics.push(lineage::diagnostic(
            ArtifactContextDiagnosticCode::CountExceeded,
            "validation/reference summary limit reached",
            Some(bundle.primary.artifact.clone()),
        ));
    }
    bundle.truncation.depth_exceeded = bundle
        .diagnostics
        .iter()
        .any(|item| item.code == ArtifactContextDiagnosticCode::DepthExceeded);
    bundle.truncation.count_exceeded = bundle
        .diagnostics
        .iter()
        .any(|item| item.code == ArtifactContextDiagnosticCode::CountExceeded);
    bounds::enforce_bounds(&mut bundle, policy);
    Ok(bundle)
}
