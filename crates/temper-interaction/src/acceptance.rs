//! Generic explicit proposal acceptance execution.
//!
//! Responders only return inert proposals. This module owns the narrow runtime
//! that turns one explicitly selected proposal into declared Forge effects from a
//! compiled profile manifest. It reloads current Forge state before mutating and
//! uses hidden correlation markers rendered from each acceptance action's
//! idempotency-key template.

use temper_forge::{
    CreateComment, CreateIssue, Forge, Issue, IssueQuery, Repository, User, UserId,
};

use crate::ids::AcceptanceActionId;
use crate::transcript::append_marker;
use crate::types::{ConversationId, ProposalId, ProposalKind};
use crate::validated::{AcceptanceEffect, AddTranscriptCommentEffect, CreateIssueEffect};
use crate::{
    AcceptanceManifest, CompiledProfileManifest, InteractionError, IssueProposal, Proposal,
};

/// Request consumed by [`AcceptanceExecutor`] for one explicit proposal
/// acceptance command.
#[derive(Clone, Copy)]
pub struct AcceptanceRequest<'a> {
    /// Compiled profile manifest that declares proposal kinds and effects.
    pub profile: &'a CompiledProfileManifest,
    /// Repository that owns the transcript and accepted targets.
    pub repository: &'a Repository,
    /// Current cached transcript issue; the executor reloads it before writes.
    pub transcript_issue: &'a Issue,
    /// Durable conversation id stored in the transcript marker.
    pub conversation_id: &'a ConversationId,
    /// Human-facing transcript URL for templates and target bodies.
    pub transcript_url: &'a str,
    /// Human whose explicit command requested acceptance, if known.
    pub requested_by: Option<&'a User>,
    /// Durable proposals currently available for this conversation.
    pub proposals: &'a [Proposal],
    /// Proposal selected by the human.
    pub proposal_id: &'a ProposalId,
    /// Optional explicit action id selected by a transport command manifest.
    /// When absent, exactly one acceptance action for the proposal kind must
    /// exist in the profile.
    pub acceptance_action: Option<&'a AcceptanceActionId>,
}

/// Result of a generic proposal acceptance transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptanceOutcome {
    /// Whether the primary target was newly created by this call.
    pub created: bool,
    /// Typed accepted target reference.
    pub target: AcceptedTarget,
}

/// Typed reference to the target produced by an accepted proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceptedTarget {
    /// A Forge issue target.
    Issue(Issue),
}

impl AcceptedTarget {
    /// Returns the target issue when this outcome produced one.
    pub fn issue(&self) -> Option<&Issue> {
        match self {
            Self::Issue(issue) => Some(issue),
        }
    }
}

/// Result of accepting an issue proposal. Kept as a compatibility DTO for older
/// product-chat callers that need the full Forge issue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueAcceptanceOutcome {
    /// Existing or newly-created issue.
    pub issue: Issue,
    /// Whether this acceptance call created the issue.
    pub created: bool,
}

/// Manifest-driven proposal acceptance executor.
pub struct AcceptanceExecutor<'a, F: Forge + ?Sized> {
    forge: &'a F,
}

impl<'a, F: Forge + ?Sized> AcceptanceExecutor<'a, F> {
    /// Builds an executor over the Forge handle authorized to apply effects.
    pub fn new(forge: &'a F) -> Self {
        Self { forge }
    }

    /// Executes one explicit proposal acceptance transaction.
    pub async fn accept(
        &self,
        request: AcceptanceRequest<'_>,
    ) -> Result<AcceptanceOutcome, InteractionError> {
        let repository = self
            .forge
            .get_repository(&request.repository.id)
            .await?
            .ok_or_else(|| InteractionError::RepositoryNotFound {
                owner: request.repository.owner.clone(),
                name: request.repository.name.clone(),
            })?;
        let transcript_issue = self
            .forge
            .get_issue(&request.transcript_issue.id)
            .await?
            .ok_or_else(|| InteractionError::TranscriptNotFound {
                number: request.transcript_issue.number.get(),
            })?;

        let proposal = request
            .proposals
            .iter()
            .find(|proposal| &proposal.id == request.proposal_id)
            .ok_or_else(|| InteractionError::ProposalNotFound {
                id: request.proposal_id.clone(),
                available: request
                    .proposals
                    .iter()
                    .map(|proposal| proposal.id.clone())
                    .collect(),
            })?;
        let proposal_manifest = request.profile.proposal(&proposal.kind).ok_or_else(|| {
            InteractionError::UnsupportedProposalKind {
                id: proposal.id.clone(),
                kind: proposal.kind.clone(),
            }
        })?;
        proposal_manifest.validate_payload(proposal)?;
        let action = select_acceptance_action(
            request.profile,
            &proposal.id,
            &proposal.kind,
            request.acceptance_action,
        )?;
        let idempotency_key = render_template(
            &action.idempotency_key_template,
            &TemplateContext::new(request, proposal, action, None, None),
        )?;
        validate_marker_value("idempotency_key", &idempotency_key)?;

        let mut accepted_issue = None;
        let mut created_issue = false;
        for effect in &action.effects {
            match effect {
                AcceptanceEffect::CreateIssue(effect) => {
                    if accepted_issue.is_some() {
                        return Err(InteractionError::InvalidConfig {
                            field: "acceptance_actions",
                            message:
                                "only one create_issue effect is supported per acceptance action"
                                    .into(),
                        });
                    }
                    let outcome = self
                        .execute_create_issue(
                            &repository,
                            request,
                            proposal,
                            action,
                            effect,
                            &idempotency_key,
                        )
                        .await?;
                    created_issue = outcome.created;
                    accepted_issue = Some(outcome.issue);
                }
                AcceptanceEffect::AddTranscriptComment(effect) => {
                    self.execute_transcript_comment(
                        &transcript_issue,
                        request,
                        proposal,
                        action,
                        effect,
                        &idempotency_key,
                    )
                    .await?;
                }
            }
        }

        let issue = accepted_issue.ok_or_else(|| InteractionError::InvalidConfig {
            field: "acceptance_actions",
            message: "accepted proposal action must produce an issue target".into(),
        })?;
        Ok(AcceptanceOutcome {
            created: created_issue,
            target: AcceptedTarget::Issue(issue),
        })
    }

    async fn execute_create_issue(
        &self,
        repository: &Repository,
        request: AcceptanceRequest<'_>,
        proposal: &Proposal,
        action: &AcceptanceManifest,
        effect: &CreateIssueEffect,
        idempotency_key: &str,
    ) -> Result<IssueAcceptanceOutcome, InteractionError> {
        let marker = render_acceptance_marker(
            effect.marker_namespace(),
            effect_marker_key(effect.marker_key(), action),
            idempotency_key,
        );
        if let Some(existing) = find_issue_by_marker(self.forge, repository, &marker).await? {
            return Ok(IssueAcceptanceOutcome {
                issue: existing,
                created: false,
            });
        }

        let context = TemplateContext::new(
            request,
            proposal,
            action,
            Some(idempotency_key),
            Some(&marker),
        );
        let title = render_template(effect.title(), &context)?;
        let mut body = render_template(effect.body_template(), &context)?;
        if let Some(backlink) = effect.backlink() {
            let label = render_template(backlink.label(), &context)?;
            let url = render_template(backlink.url(), &context)?;
            let line = format!("{}: {}", label.trim(), url.trim());
            if !line.trim().is_empty() && !body.contains(&line) {
                body = append_visible_line(body, &line);
            }
        }
        if !body.contains(&marker) {
            body = append_marker(&body, &marker);
        }
        let labels = render_non_empty_values(effect.labels(), &context, "labels")?;
        let assignees = render_non_empty_values(effect.assignees(), &context, "assignees")?
            .into_iter()
            .map(UserId::new)
            .collect();

        let issue = self
            .forge
            .create_issue(
                &repository.id,
                CreateIssue {
                    title,
                    body,
                    labels,
                    assignees,
                },
            )
            .await?;
        Ok(IssueAcceptanceOutcome {
            issue,
            created: true,
        })
    }

    async fn execute_transcript_comment(
        &self,
        transcript_issue: &Issue,
        request: AcceptanceRequest<'_>,
        proposal: &Proposal,
        action: &AcceptanceManifest,
        effect: &AddTranscriptCommentEffect,
        idempotency_key: &str,
    ) -> Result<(), InteractionError> {
        let marker = render_acceptance_marker(
            effect.marker_namespace(),
            effect_marker_key(effect.marker_key(), action),
            idempotency_key,
        );
        let comments = self.forge.list_issue_comments(&transcript_issue.id).await?;
        if comments
            .iter()
            .any(|comment| comment.body.contains(&marker))
        {
            return Ok(());
        }
        let context = TemplateContext::new(
            request,
            proposal,
            action,
            Some(idempotency_key),
            Some(&marker),
        );
        let mut body = render_template(effect.body_template(), &context)?;
        if !body.contains(&marker) {
            body = append_marker(&body, &marker);
        }
        self.forge
            .add_issue_comment(&transcript_issue.id, CreateComment { body })
            .await?;
        Ok(())
    }
}

fn select_acceptance_action<'a>(
    profile: &'a CompiledProfileManifest,
    proposal_id: &ProposalId,
    proposal_kind: &ProposalKind,
    requested: Option<&AcceptanceActionId>,
) -> Result<&'a AcceptanceManifest, InteractionError> {
    if let Some(requested) = requested {
        let action = profile.acceptance_action(requested).ok_or_else(|| {
            InteractionError::InvalidConfig {
                field: "acceptance_action",
                message: format!("acceptance action `{requested}` is not declared"),
            }
        })?;
        if &action.proposal_kind != proposal_kind {
            return Err(InteractionError::UnsupportedProposalKind {
                id: proposal_id.clone(),
                kind: proposal_kind.clone(),
            });
        }
        return Ok(action);
    }

    let mut matches = profile
        .acceptance_actions
        .iter()
        .filter(|action| &action.proposal_kind == proposal_kind);
    let Some(action) = matches.next() else {
        return Err(InteractionError::UnsupportedProposalKind {
            id: proposal_id.clone(),
            kind: proposal_kind.clone(),
        });
    };
    if matches.next().is_some() {
        return Err(InteractionError::InvalidConfig {
            field: "acceptance_actions",
            message: format!(
                "multiple acceptance actions accept proposal kind `{proposal_kind}`; transport must select one"
            ),
        });
    }
    Ok(action)
}

/// Finds an issue whose body contains a hidden marker.
pub async fn find_issue_by_marker<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    marker: &str,
) -> Result<Option<Issue>, InteractionError> {
    let issues = forge
        .list_issues(&repository.id, IssueQuery::default())
        .await?;
    Ok(issues.into_iter().find(|issue| issue.body.contains(marker)))
}

/// Renders the generic hidden acceptance marker used for idempotency.
pub fn render_acceptance_marker(
    marker_namespace: &str,
    marker_key: &str,
    idempotency_key: &str,
) -> String {
    format!("<!-- temper:{marker_namespace}-{marker_key}={idempotency_key} -->")
}

fn effect_marker_key<'a>(marker_key: Option<&'a str>, action: &'a AcceptanceManifest) -> &'a str {
    marker_key.unwrap_or_else(|| action.id.as_str())
}

fn validate_marker_value(field: &'static str, value: &str) -> Result<(), InteractionError> {
    if !value.trim().is_empty() && !value.contains('\n') && !value.contains("-->") {
        Ok(())
    } else {
        Err(InteractionError::InvalidConfig {
            field,
            message: "rendered marker value must be non-empty and fit in one HTML comment".into(),
        })
    }
}

/// Renders the body used by the deprecated issue-intake helper.
pub fn render_filed_issue_body(
    draft: &IssueProposal,
    transcript_url: &str,
    marker: &str,
    requested_by: Option<&str>,
) -> String {
    let mut body = draft.body.trim_end().to_string();
    body.push_str("\n\n---\n");
    body.push_str(&format!("Transcript: {transcript_url}\n"));
    if let Some(human) = requested_by.filter(|value| !value.trim().is_empty()) {
        body.push_str(&format!("requested-by: {human}\n"));
    }
    body.push('\n');
    body.push_str(marker);
    body
}

#[derive(Clone, Copy)]
struct TemplateContext<'a> {
    request: AcceptanceRequest<'a>,
    proposal: &'a Proposal,
    action: &'a AcceptanceManifest,
    idempotency_key: Option<&'a str>,
    effect_marker: Option<&'a str>,
}

impl<'a> TemplateContext<'a> {
    fn new(
        request: AcceptanceRequest<'a>,
        proposal: &'a Proposal,
        action: &'a AcceptanceManifest,
        idempotency_key: Option<&'a str>,
        effect_marker: Option<&'a str>,
    ) -> Self {
        Self {
            request,
            proposal,
            action,
            idempotency_key,
            effect_marker,
        }
    }
}

fn render_template(
    template: &str,
    context: &TemplateContext<'_>,
) -> Result<String, InteractionError> {
    let mut rendered = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(InteractionError::InvalidConfig {
                field: "template",
                message: format!("unterminated template variable in `{template}`"),
            });
        };
        let variable = &after[..end];
        rendered.push_str(&template_value(variable, context)?);
        rest = &after[end + 1..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn template_value(
    variable: &str,
    context: &TemplateContext<'_>,
) -> Result<String, InteractionError> {
    match variable {
        "conversation.id" => Ok(context.request.conversation_id.to_string()),
        "conversation.transcript_url" => Ok(context.request.transcript_url.to_string()),
        "proposal.id" => Ok(context.proposal.id.to_string()),
        "proposal.kind" => Ok(context.proposal.kind.to_string()),
        "proposal.title" => Ok(context.proposal.title.clone()),
        "proposal.summary" => Ok(context.proposal.summary.clone().unwrap_or_default()),
        "human.handle" => Ok(context
            .request
            .requested_by
            .map(|user| user.handle.clone())
            .unwrap_or_default()),
        "acceptance.action_id" => Ok(context.action.id.to_string()),
        "idempotency.key" => Ok(context.idempotency_key.unwrap_or_default().to_string()),
        "effect.marker" => Ok(context.effect_marker.unwrap_or_default().to_string()),
        value if value.starts_with("proposal.payload.") => json_path_string(
            &context.proposal.payload,
            value.trim_start_matches("proposal.payload."),
        ),
        other => Err(InteractionError::InvalidConfig {
            field: "template",
            message: format!("unsupported template variable `{other}`"),
        }),
    }
}

fn json_path_string(value: &serde_json::Value, path: &str) -> Result<String, InteractionError> {
    let mut current = value;
    for segment in path.split('.') {
        current = current
            .get(segment)
            .ok_or_else(|| InteractionError::InvalidConfig {
                field: "template",
                message: format!("proposal payload has no `{path}` field"),
            })?;
    }
    Ok(match current {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(value) => value.clone(),
        other => other.to_string(),
    })
}

fn render_non_empty_values(
    values: &[String],
    context: &TemplateContext<'_>,
    field: &'static str,
) -> Result<Vec<String>, InteractionError> {
    values
        .iter()
        .map(|value| {
            let rendered = render_template(value, context)?.trim().to_string();
            if rendered.is_empty() {
                Err(InteractionError::InvalidConfig {
                    field,
                    message: "rendered value must not be empty".into(),
                })
            } else {
                Ok(rendered)
            }
        })
        .collect()
}

fn append_visible_line(mut body: String, line: &str) -> String {
    if body.trim().is_empty() {
        line.to_string()
    } else {
        body.push_str("\n\n---\n");
        body.push_str(line);
        body
    }
}
