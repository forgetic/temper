//! Raw, serde-loadable user-defined interaction spec types.
//!
//! These structs mirror authored JSON/YAML/TOML-shaped configuration and do no
//! semantic checking during deserialization beyond rejecting unknown fields.
//! Call [`RawInteractionSpec::validate`] to obtain a
//! [`crate::validated::ValidatedInteractionSpec`].

use serde::{Deserialize, Serialize};

use crate::transcript::DEFAULT_RECENT_TURN_LIMIT;
use crate::validate::{validate, InteractionSpecValidationErrors};
use crate::validated::ValidatedInteractionSpec;

/// Raw interaction specification as loaded from user-authored configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawInteractionSpec {
    /// Stable id for this interaction spec document.
    pub id: String,
    /// Responder process declarations available to profiles.
    #[serde(default)]
    pub responders: Vec<RawResponderDeclaration>,
    /// Interactive profile declarations.
    #[serde(default)]
    pub profiles: Vec<RawInteractiveProfile>,
}

impl RawInteractionSpec {
    /// Validates this raw spec into a normalized interaction spec.
    pub fn validate(&self) -> Result<ValidatedInteractionSpec, InteractionSpecValidationErrors> {
        validate(self)
    }
}

/// Raw interactive profile declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawInteractiveProfile {
    /// Stable profile id passed to responder requests.
    pub id: String,
    /// Transcript issue policy for this profile.
    pub transcript: RawTranscriptPolicy,
    /// Human and agent participant display policy.
    pub participants: RawParticipants,
    /// Responder id used for this profile.
    pub responder: String,
    /// Proposal kinds this profile's responder may return.
    #[serde(default)]
    pub proposal_kinds: Vec<RawProposalKindDeclaration>,
    /// Transport commands exposed for this profile.
    #[serde(default)]
    pub commands: Vec<RawTransportCommandDeclaration>,
    /// Accepted-proposal actions authorized for this profile.
    #[serde(default)]
    pub acceptance_actions: Vec<RawAcceptanceActionDeclaration>,
}

/// Human and agent participants for transcript rendering and responder context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawParticipants {
    /// Human participant display declaration.
    pub human: RawParticipantDeclaration,
    /// Agent participant display declaration.
    pub agent: RawParticipantDeclaration,
}

/// Display information for one participant role.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawParticipantDeclaration {
    /// Display name safe to show in transcripts and frontend snapshots.
    pub display_name: String,
}

/// Transcript policy for one interactive profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawTranscriptPolicy {
    /// Transcript target kind. Phase 1 supports `issue`.
    #[serde(default = "default_transcript_target")]
    pub target: String,
    /// Prefix used when creating transcript issue titles.
    pub title_prefix: String,
    /// Labels applied to transcript issues.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Label policy for resuming transcript issues. Phase 1 supports `exact`.
    #[serde(default = "default_transcript_label_policy")]
    pub label_policy: String,
    /// Hidden marker namespace for transcript and acceptance correlation markers.
    pub marker_namespace: String,
    /// Maximum number of recent transcript turns supplied to the responder.
    #[serde(default = "default_recent_turn_limit")]
    pub recent_turn_limit: usize,
}

fn default_transcript_target() -> String {
    "issue".to_string()
}

fn default_transcript_label_policy() -> String {
    "exact".to_string()
}

fn default_recent_turn_limit() -> usize {
    DEFAULT_RECENT_TURN_LIMIT
}

/// External process responder declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawResponderDeclaration {
    /// Stable responder id referenced by profiles.
    pub id: String,
    /// Protocol/version intent. Phase 1 supports `process-v1`.
    pub protocol: String,
    /// Whether a deployment must bind this responder before serving the profile.
    #[serde(default)]
    pub required: bool,
}

/// Proposal kind and payload contract declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawProposalKindDeclaration {
    /// Stable proposal kind id.
    pub id: String,
    /// Payload contract. Phase 1 supports `issue_draft`.
    pub payload: String,
}

/// Transport-facing command declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawTransportCommandDeclaration {
    /// Stable command id.
    pub id: String,
    /// Aliases exposed by a transport, such as `/file`.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Action this command requests.
    pub action: RawTransportCommandAction,
}

/// Action requested by a transport command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawTransportCommandAction {
    /// Accept a proposal of the declared kind through an acceptance action.
    pub accept_proposal: RawAcceptProposalCommandAction,
}

/// Accept-proposal command action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawAcceptProposalCommandAction {
    /// Proposal kind the command accepts.
    pub kind: String,
    /// Acceptance action id to execute after explicit human acceptance.
    pub acceptance_action: String,
}

/// Accepted-proposal action declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawAcceptanceActionDeclaration {
    /// Stable acceptance action id.
    pub id: String,
    /// Proposal kind this action accepts.
    pub proposal_kind: String,
    /// Explicit acceptance policy.
    pub acceptance: RawAcceptancePolicy,
    /// Idempotency key template used by later acceptance execution.
    pub idempotency_key: String,
    /// Closed list of effects this action may perform.
    #[serde(default)]
    pub effects: Vec<RawAcceptanceEffect>,
}

/// Explicit human acceptance policy for one action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawAcceptancePolicy {
    /// Policy kind. Phase 1 supports `explicit`.
    pub policy: String,
    /// Commands allowed to trigger the explicit acceptance path.
    #[serde(default)]
    pub commands: Vec<String>,
}

/// One effect in an acceptance action's closed effect list.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawAcceptanceEffect {
    /// Effect kind. Phase 1 supports `create_issue`.
    pub kind: String,
    /// Issue title template for `create_issue`.
    #[serde(default)]
    pub title: String,
    /// Issue body template for `create_issue`.
    #[serde(default)]
    pub body_template: String,
    /// Labels applied to created issues for `create_issue`.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Marker namespace used to make issue creation idempotent.
    #[serde(default)]
    pub marker_namespace: String,
    /// Optional transcript backlink metadata for `create_issue`.
    #[serde(default)]
    pub backlink: Option<RawBacklinkPolicy>,
}

/// A templated backlink inserted by a later acceptance executor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawBacklinkPolicy {
    /// Human-facing backlink label, such as `Transcript`.
    pub label: String,
    /// URL template, commonly `${conversation.transcript_url}`.
    pub url: String,
}
