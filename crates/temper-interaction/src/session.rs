use std::sync::Arc;

use serde_json::{Map, Value};
use temper_forge::{CreateComment, Forge, Issue, ItemNumber, Repository, RepositoryPath, User};

use crate::CompiledProfileManifest;
use crate::InteractionError;
use crate::acceptance::{
    AcceptanceExecutor, AcceptanceRequest, AcceptedTarget, IssueAcceptanceOutcome,
};
use crate::agent::InteractiveResponder;
use crate::proposal::Proposal;
use crate::proposal_state::render_agent_reply_comment_with_proposals;
use crate::transcript::{
    ForgeTranscript, ForgeTranscriptConfig, ForgeTranscriptOpenOptions, issue_url,
    open_forge_transcript, trim_turns,
};
use crate::types::{ConversationReply, ConversationRequest, ConversationTurn, ProposalId};
use crate::validated::AcceptanceEffect;

/// Open options for a Forge-backed interactive session.
#[derive(Clone, Debug, PartialEq)]
pub struct ForgeSessionOpenOptions {
    /// Base Forge URL used to render transcript and issue links.
    pub base_url: String,
    /// Repository that owns transcript and intake issues.
    pub repo_path: RepositoryPath,
    /// Existing transcript issue to resume, or `None` to create one.
    pub transcript_issue: Option<ItemNumber>,
    /// Profile-specific responder context.
    ///
    /// When this is a JSON object, the session adds `repository` and
    /// `transcript_url` fields if they are not already present.
    pub context: Value,
}

impl ForgeSessionOpenOptions {
    /// Builds open options with empty profile context.
    pub fn new(base_url: impl Into<String>, repo_path: RepositoryPath) -> Self {
        Self {
            base_url: base_url.into(),
            repo_path,
            transcript_issue: None,
            context: Value::Object(Map::new()),
        }
    }
}

/// Transport-neutral session configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeSessionConfig {
    /// Transcript issue policy and responder profile settings.
    pub transcript: ForgeTranscriptConfig,
    /// Compiled profile manifest that declares proposal kinds, commands, and
    /// explicit acceptance actions.
    pub profile: CompiledProfileManifest,
}

impl ForgeSessionConfig {
    /// Builds a validated session configuration from manifest data.
    pub fn new(
        transcript: ForgeTranscriptConfig,
        profile: CompiledProfileManifest,
    ) -> Result<Self, InteractionError> {
        for action in &profile.acceptance_actions {
            for effect in &action.effects {
                if let AcceptanceEffect::CreateIssue(effect) = effect
                    && effect
                        .labels()
                        .iter()
                        .any(|label| transcript.transcript_labels.contains(label))
                {
                    return Err(InteractionError::InvalidConfig {
                        field: "create_issue.labels",
                        message: "must differ from transcript labels".into(),
                    });
                }
            }
        }
        Ok(Self {
            transcript,
            profile,
        })
    }

    /// Builds session configuration from a compiled profile manifest.
    pub fn from_profile_manifest(
        manifest: &CompiledProfileManifest,
    ) -> Result<Self, InteractionError> {
        let transcript = ForgeTranscriptConfig::from_profile_manifest(manifest);
        Self::new(transcript, manifest.clone())
    }
}

/// Forge-backed interactive session that owns durable turns and proposal cache.
pub struct ForgeInteractionSession<H, A, R>
where
    H: Forge + ?Sized,
    A: Forge + ?Sized,
    R: InteractiveResponder + ?Sized,
{
    human_forge: Arc<H>,
    agent_forge: Arc<A>,
    responder: Arc<R>,
    config: ForgeSessionConfig,
    base_url: String,
    repo_path: RepositoryPath,
    context: Value,
    transcript: ForgeTranscript,
    latest_proposals: Vec<Proposal>,
}

impl<H, A, R> ForgeInteractionSession<H, A, R>
where
    H: Forge + ?Sized,
    A: Forge + ?Sized,
    R: InteractiveResponder + ?Sized,
{
    /// Opens or creates a Forge-backed interactive session.
    pub async fn open(
        human_forge: Arc<H>,
        agent_forge: Arc<A>,
        responder: Arc<R>,
        config: ForgeSessionConfig,
        options: ForgeSessionOpenOptions,
    ) -> Result<Self, InteractionError> {
        let transcript = open_forge_transcript(
            human_forge.as_ref(),
            agent_forge.as_ref(),
            ForgeTranscriptOpenOptions {
                repo_path: options.repo_path.clone(),
                transcript_issue: options.transcript_issue,
            },
            &config.transcript,
        )
        .await?;
        let latest_proposals = transcript.latest_proposals().to_vec();
        Ok(Self {
            human_forge,
            agent_forge,
            responder,
            config,
            base_url: options.base_url,
            repo_path: options.repo_path,
            context: options.context,
            transcript,
            latest_proposals,
        })
    }

    /// URL of the transcript issue.
    pub fn transcript_url(&self) -> String {
        issue_url(
            &self.base_url,
            &self.repo_path,
            self.transcript.issue().number,
        )
    }

    /// Renders a URL for an issue in this session's repository.
    pub fn issue_url_for(&self, number: ItemNumber) -> String {
        issue_url(&self.base_url, &self.repo_path, number)
    }

    /// Transcript issue backing this session.
    pub fn transcript_issue(&self) -> &Issue {
        self.transcript.issue()
    }

    /// Durable conversation id.
    pub fn conversation_id(&self) -> &crate::ConversationId {
        self.transcript.conversation_id()
    }

    /// Repository that owns the transcript and accepted targets.
    pub fn repository(&self) -> &Repository {
        self.transcript.repository()
    }

    /// Human Forge user used for human comments.
    pub fn human_user(&self) -> &User {
        self.transcript.human_user()
    }

    /// Agent Forge user used for agent comments.
    pub fn agent_user(&self) -> &User {
        self.transcript.agent_user()
    }

    /// Recent turns currently cached by the session.
    pub fn turns(&self) -> &[ConversationTurn] {
        self.transcript.turns()
    }

    /// Latest responder proposals awaiting explicit acceptance.
    pub fn latest_proposals(&self) -> &[Proposal] {
        &self.latest_proposals
    }

    /// Appends a human turn, dispatches the responder, persists the agent reply,
    /// and updates the latest proposal cache.
    pub async fn send_human_turn(
        &mut self,
        body: &str,
    ) -> Result<ConversationReply, InteractionError> {
        self.human_forge
            .add_issue_comment(
                &self.transcript.issue().id,
                CreateComment { body: body.into() },
            )
            .await?;
        self.transcript.push_turn(
            ConversationTurn::new(self.config.transcript.human_participant.clone(), body),
            self.config.transcript.recent_turn_limit,
        );

        let request = ConversationRequest {
            profile_id: self.config.transcript.profile_id.clone(),
            conversation_id: self.transcript.conversation_id().clone(),
            turns: recent_turns(
                self.transcript.turns(),
                self.config.transcript.recent_turn_limit,
            ),
            context: self.request_context(),
        };
        let reply = self.responder.respond(&request).await?;
        reply.validate()?;
        self.validate_reply_proposals(&reply)?;
        let comment_body = render_agent_reply_comment_with_proposals(
            &reply,
            &self.config.transcript.marker_namespace,
        )?;
        self.agent_forge
            .add_issue_comment(
                &self.transcript.issue().id,
                CreateComment {
                    body: comment_body.clone(),
                },
            )
            .await?;
        self.transcript.push_turn(
            ConversationTurn::new(
                self.config.transcript.agent_participant.clone(),
                crate::proposal_state::strip_proposal_snapshot_marker(
                    &self.config.transcript.marker_namespace,
                    &comment_body,
                ),
            ),
            self.config.transcript.recent_turn_limit,
        );
        self.latest_proposals = reply.proposals.clone();
        Ok(reply)
    }

    /// Idempotently accepts a cached proposal by id and returns the issue target.
    pub async fn accept_issue_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<IssueAcceptanceOutcome, InteractionError> {
        self.accept_issue_proposal_with_action(proposal_id, None)
            .await
    }

    /// Idempotently accepts a cached proposal through an optional command-selected
    /// acceptance action.
    pub async fn accept_issue_proposal_with_action(
        &self,
        proposal_id: &ProposalId,
        acceptance_action: Option<&crate::AcceptanceActionId>,
    ) -> Result<IssueAcceptanceOutcome, InteractionError> {
        let outcome = AcceptanceExecutor::new(self.agent_forge.as_ref())
            .accept(AcceptanceRequest {
                profile: &self.config.profile,
                repository: self.transcript.repository(),
                transcript_issue: self.transcript.issue(),
                conversation_id: self.transcript.conversation_id(),
                transcript_url: &self.transcript_url(),
                requested_by: Some(self.transcript.human_user()),
                proposals: &self.latest_proposals,
                proposal_id,
                acceptance_action,
            })
            .await?;
        let AcceptedTarget::Issue(issue) = outcome.target;
        Ok(IssueAcceptanceOutcome {
            issue,
            created: outcome.created,
        })
    }

    fn validate_reply_proposals(&self, reply: &ConversationReply) -> Result<(), InteractionError> {
        for proposal in &reply.proposals {
            let manifest = self
                .config
                .profile
                .proposal(&proposal.kind)
                .ok_or_else(|| InteractionError::UnsupportedProposalKind {
                    id: proposal.id.clone(),
                    kind: proposal.kind.clone(),
                })?;
            manifest.validate_payload(proposal)?;
        }
        Ok(())
    }

    fn request_context(&self) -> Value {
        let mut context = self.context.clone();
        if let Value::Object(map) = &mut context {
            map.entry("repository".to_string()).or_insert_with(|| {
                Value::String(format!("{}/{}", self.repo_path.owner, self.repo_path.name))
            });
            map.entry("transcript_url".to_string())
                .or_insert_with(|| Value::String(self.transcript_url()));
        }
        context
    }
}

fn recent_turns(turns: &[ConversationTurn], limit: usize) -> Vec<ConversationTurn> {
    let mut turns = turns.to_vec();
    trim_turns(&mut turns, limit);
    turns
}
