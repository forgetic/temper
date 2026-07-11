// SPDX-License-Identifier: MPL-2.0

//! Authoritative successful-verdict validation against fresh Forge state.

use temper_forge::{Forge, ItemNumber, RepositoryPath};
use temper_protocol_worker::{JobContext, JobResult};
use temper_verdict::{SourceMetadata, validate_verdict_result};
use temper_workflow::{
    ArtifactKindId, ArtifactTarget, Classifier, VerdictId, parse_metadata_block,
};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::verdict_contract::{
    child_kind_has_reachable_queue, derive_verdict_contracts, source_metadata_from_workflow,
};

pub(crate) enum VerdictCheck {
    Valid,
    Stale,
    Retryable(String),
    Rejected(String),
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(crate) async fn validate_successful_verdict(
        &self,
        job: &InFlightJob,
        result: &JobResult,
    ) -> VerdictCheck {
        let Some(verdict) = result.verdict.as_deref() else {
            return VerdictCheck::Valid;
        };
        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                return VerdictCheck::Rejected(format!("invalid in-flight JobContext: {error}"));
            }
        };
        let Some(action) = context.action.as_deref() else {
            return VerdictCheck::Rejected("in-flight JobContext has no action".to_string());
        };
        let Some(role) = self
            .compiled
            .roles()
            .iter()
            .find(|role| role.id.as_str() == job.role)
        else {
            return VerdictCheck::Rejected(format!("workflow has no role `{}`", job.role));
        };
        let Some(tool) = role.tools.iter().find(|tool| tool.name == action) else {
            return VerdictCheck::Rejected(format!(
                "workflow role `{}` has no action `{action}`",
                job.role
            ));
        };
        let verdict_id = VerdictId::new(verdict.trim());
        let Some(routed) = tool.outcomes.get(&verdict_id) else {
            return VerdictCheck::Rejected(format!(
                "action `{action}` does not declare verdict `{verdict}`"
            ));
        };
        let contracts = derive_verdict_contracts(self.workflow.as_ref(), tool);
        let contract = contracts.get(verdict.trim());

        let (source_body, source_metadata) = match job.artifact.kind.as_str() {
            "issue" => match self.fresh_issue_for_validation(job, &context).await {
                Ok(Some(body)) => body,
                Ok(None) => return VerdictCheck::Stale,
                Err(outcome) => return outcome,
            },
            "pull_request" => match self.fresh_pull_request_for_validation(job, &context).await {
                Ok(Some(body)) => body,
                Ok(None) => return VerdictCheck::Stale,
                Err(outcome) => return outcome,
            },
            other => {
                return VerdictCheck::Rejected(format!(
                    "unsupported verdict source artifact `{other}`"
                ));
            }
        };

        if let Err(error) = validate_verdict_result(result, &contracts, &source_metadata) {
            return VerdictCheck::Rejected(format!(
                "action `{action}`, verdict `{verdict}`, routed transition `{routed}`: {error}"
            ));
        }
        if contract.is_some_and(|contract| contract.min_children > 0) {
            if let Err(reason) = self
                .validate_child_inputs(job, result, &source_body, contract.expect("checked"))
                .await
            {
                return reason;
            }
        }
        VerdictCheck::Valid
    }

    async fn fresh_issue_for_validation(
        &self,
        job: &InFlightJob,
        context: &JobContext,
    ) -> Result<Option<(String, SourceMetadata)>, VerdictCheck> {
        let (repo, number) = self.validation_coordinates(job).await?;
        let issue = self
            .forge
            .get_issue_by_number(&repo, number)
            .await
            .map_err(|error| {
                VerdictCheck::Retryable(format!("read fresh source issue: {error}"))
            })?;
        let Some(issue) = issue else {
            return Ok(None);
        };
        let metadata = parse_metadata_block(&issue.body)
            .map_err(|error| {
                VerdictCheck::Rejected(format!("source workflow metadata is malformed: {error}"))
            })?
            .map(source_metadata_from_workflow)
            .unwrap_or_default();
        let source_kind = ArtifactKindId::new(&context.artifact_kind);
        if !matches!(
            Classifier::new(self.workflow.as_ref()).classify_issue(&issue),
            Ok(classified) if classified.kind == source_kind
        ) {
            return Ok(None);
        }
        Ok(Some((issue.body, metadata)))
    }

    async fn fresh_pull_request_for_validation(
        &self,
        job: &InFlightJob,
        context: &JobContext,
    ) -> Result<Option<(String, SourceMetadata)>, VerdictCheck> {
        let (repo, number) = self.validation_coordinates(job).await?;
        let pull = self
            .forge
            .get_pull_request_by_number(&repo, number)
            .await
            .map_err(|error| {
                VerdictCheck::Retryable(format!("read fresh source pull request: {error}"))
            })?;
        let Some(pull) = pull else {
            return Ok(None);
        };
        let metadata = parse_metadata_block(&pull.body)
            .map_err(|error| {
                VerdictCheck::Rejected(format!("source workflow metadata is malformed: {error}"))
            })?
            .map(source_metadata_from_workflow)
            .unwrap_or_default();
        let source_kind = ArtifactKindId::new(&context.artifact_kind);
        if !matches!(
            Classifier::new(self.workflow.as_ref()).classify_pull_request(&pull),
            Ok(classified) if classified.kind == source_kind
        ) {
            return Ok(None);
        }
        Ok(Some((pull.body, metadata)))
    }

    async fn validation_coordinates(
        &self,
        job: &InFlightJob,
    ) -> Result<(temper_forge::RepositoryId, ItemNumber), VerdictCheck> {
        let (owner, name) = job.repo.split_once('/').ok_or_else(|| {
            VerdictCheck::Rejected(format!("malformed repository path `{}`", job.repo))
        })?;
        let repository = self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
            .map_err(|error| VerdictCheck::Retryable(format!("read source repository: {error}")))?
            .ok_or(VerdictCheck::Stale)?;
        let number = job
            .artifact
            .item
            .as_u64()
            .map(ItemNumber::new)
            .ok_or_else(|| {
                VerdictCheck::Rejected("source artifact number is not numeric".to_string())
            })?;
        Ok((repository.id, number))
    }

    async fn validate_child_inputs(
        &self,
        job: &InFlightJob,
        result: &JobResult,
        _source_body: &str,
        contract: &temper_verdict::VerdictContract,
    ) -> Result<(), VerdictCheck> {
        for child in &result.children {
            let metadata = parse_metadata_block(&child.body).map_err(|error| {
                VerdictCheck::Rejected(format!(
                    "child `{}` workflow metadata is malformed: {error}",
                    child.slug
                ))
            })?;
            let explicit = child
                .kind
                .as_deref()
                .map(str::trim)
                .filter(|kind| !kind.is_empty());
            let metadata_kind = metadata
                .as_ref()
                .and_then(|metadata| metadata.kind.as_ref());
            if explicit.is_some_and(|kind| {
                metadata_kind.is_some_and(|metadata_kind| metadata_kind.as_str() != kind)
            }) {
                return Err(VerdictCheck::Rejected(format!(
                    "child `{}` declares conflicting payload and body artifact kinds",
                    child.slug
                )));
            }
            let kind = explicit
                .map(ArtifactKindId::new)
                .or_else(|| metadata_kind.cloned())
                .unwrap_or_else(|| ArtifactKindId::new("code"));
            let Some(declared) = self.workflow.artifact_kind(&kind) else {
                return Err(VerdictCheck::Rejected(format!(
                    "child `{}` names unknown artifact kind `{kind}`",
                    child.slug
                )));
            };
            if declared.target != ArtifactTarget::Issue
                || (!contract.allowed_child_kinds.is_empty()
                    && !contract
                        .allowed_child_kinds
                        .iter()
                        .any(|allowed| allowed == kind.as_str()))
            {
                return Err(VerdictCheck::Rejected(format!(
                    "child `{}` kind `{kind}` is not allowed by the routed workflow relation",
                    child.slug
                )));
            }
            if !child_kind_has_reachable_queue(self.workflow.as_ref(), &kind) {
                return Err(VerdictCheck::Rejected(format!(
                    "child `{}` kind `{kind}` has no reachable workflow queue/action",
                    child.slug
                )));
            }
            if let Some(target) = child.target_repo.as_deref() {
                self.validate_child_target_repo(job, &child.slug, target)
                    .await?;
            }
        }
        Ok(())
    }

    async fn validate_child_target_repo(
        &self,
        job: &InFlightJob,
        slug: &str,
        target: &str,
    ) -> Result<(), VerdictCheck> {
        let Some((owner, name)) = target.split_once('/') else {
            return Err(VerdictCheck::Rejected(format!(
                "child `{slug}` has malformed target repository `{target}`"
            )));
        };
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return Err(VerdictCheck::Rejected(format!(
                "child `{slug}` has malformed target repository `{target}`"
            )));
        }
        match self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
        {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(VerdictCheck::Rejected(format!(
                "child `{slug}` target repository `{target}` does not exist"
            ))),
            Err(error) => Err(VerdictCheck::Retryable(format!(
                "look up child `{slug}` target repository for job `{}`: {error}",
                job.job_id
            ))),
        }
    }
}
