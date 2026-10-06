//! Configuration held in force (domain/authority.md, sections 6, 8 and 10).

use alloc::boxed::Box;

use crate::{Authority, Implies, Pattern};

/// Root-owned deployment configuration admitted by `Domain::new`; hard ceilings apply to every
/// later check. (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Rules {
    /// Deployment authority ceiling, admitted under `Limits`.
    pub ceiling: Authority,
    /// Deployment spend ceiling for a period; this value does not keep a period ledger.
    pub period_spend: u64,
    /// Exclusive lower bound of an offered run budget, strictly below the maximum.
    pub minimum_run_spend: u64,
    /// Inclusive run cap, no greater than the deployment authority's spend.
    pub maximum_run_spend: u64,
    /// Validated connector-scoped kind preorder, bounded by `Limits::implications`.
    pub implies: Implies,
    /// Deployment effect requirements, bounded by `Limits::requirements`.
    pub requirements: Box<[Requirement]>,
    /// Deployment landing rules, bounded by `Limits::landing_rules`.
    pub landing: Box<[LandingRule]>,
}

/// Root-submitted project policy; policy events validate its bounded values before replacing the
/// current entry. (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Policy {
    /// Optional final escalation recipient role, distinct from acceptance permissions. When present
    /// it must name a configured role allowed to accept escalations; the current root requires it.
    /// Disabled authority-only policies route no escalation fallback.
    pub escalation_role: Option<u32>,
    /// Project authority ceiling, no greater than the deployment ceiling.
    pub ceiling: Authority,
    /// Project period ceiling, no greater than deployment period spend.
    pub period_spend: u64,
    /// Distinct numbered roles, bounded by `Limits::roles` and the project ceiling.
    pub roles: Box<[Role]>,
    /// Project requirements added to deployment requirements, bounded by `Limits::requirements`.
    pub requirements: Box<[Requirement]>,
    /// Project landing rules added to deployment and change gates, bounded by
    /// `Limits::landing_rules`.
    pub landing: Box<[LandingRule]>,
}

/// Numbered project permission and funding ceiling; role membership is supplied by people through
/// the root. (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Role {
    pub number: u32,
    /// Authority this role may give, admitted below the project ceiling.
    pub authority: Authority,
    /// Role's period funding ceiling, no greater than the project's.
    pub period_spend: u64,
    pub requests: Requests,
    /// Proposal kinds this role may accept.
    pub decides: Proposals,
}

/// An effect of this exact kind, on a matching resource, needs each named
/// connector fact to have passed at its pinned state. Kinds of effect are
/// matched exactly: granting an implied kind never removes its requirements.
/// Facts required for the exact effect kind and matching resources, independent of grant
/// implications. (domain/authority.md, section 10).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Requirement {
    pub connector: u16,
    pub kind: u16,
    /// Resources to which it applies, bounded by segment and byte limits.
    pub pattern: Pattern,
    /// Required connector fact kinds, bounded by `Limits::facts` per requirement.
    pub facts: Box<[u16]>,
}

/// A verdict may carry only over the clean updates verified by the root.
/// Which root-verified heads a gate verdict or human approval may cover; CI always pins the exact
/// head. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Freshness {
    /// Accept only the landing head itself.
    Exact,
    /// Also accept predecessor heads connected entirely by root-verified clean updates.
    Clean,
}

/// Numbered landing gate supplied by configuration or the change; advisory gates do not block
/// landing. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Gate {
    /// Gate identity resolved by the root for the named change.
    pub number: u32,
    /// Whether missing, pending or failed valid verdicts affect admission.
    pub blocking: bool,
    /// Accepted verdict-head relationship to the landing head.
    pub freshness: Freshness,
}

/// Required distinct authenticated reviewers of a root-verified project role. (domain/authority.md,
/// section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Approval {
    /// Reviewer role verified by the root; project-policy approval roles must exist in that policy.
    pub role: u32,
    /// Positive distinct-person count, admitted no greater than `Limits::reviews`.
    pub people: u32,
    /// Accepted review-head relationship to the landing head.
    pub freshness: Freshness,
}

/// Requirements on one connector's exact landing kind and branch pattern
/// (domain/authority.md, 10; domain/forge.md, 8.3). These add to change gates.
/// Additional conditions on one exact connector landing kind and branch pattern; every applicable
/// rule applies. (domain/authority.md, section 10).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LandingRule {
    pub connector: u16,
    /// Exact landing effect kind; implications do not skip this rule.
    pub kind: u16,
    /// Landing-branch resources covered by this rule.
    pub pattern: Pattern,
    pub ci: bool,
    /// Require containment of the snapshot's landing-branch tip.
    pub up_to_date: bool,
    /// Additional gates, bounded by `Limits::gates` per rule.
    pub gates: Box<[Gate]>,
    /// Required role approvals, bounded by `Limits::approvals` per rule.
    pub approvals: Box<[Approval]>,
}

/// Kinds of person request checked against a role's request bit set; execution remains with the
/// root. (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RequestKind {
    /// Create a task batch.
    Create,
    /// Give a complete future allotment.
    Allot,
    /// Accept one proposed action.
    Accept,
    /// Give authority newly needed by an amendment.
    Amend,
    /// Cancel a task.
    Cancel,
    /// Release a held task.
    Release,
    /// Move a task with the authority to fund it.
    Move,
    /// Watch live state.
    Watch,
    /// Change project policy.
    Policy,
}

/// Role's request-permission bit set; policy admission rejects bits outside `ALL`.
/// (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Requests(
    /** Bits 1 through 256 for Create, Allot, Accept, Amend, Cancel, Release, Move, Watch and Policy respectively. */
    pub u16,
);

impl Requests {
    /// All defined bits of this permission set; not a grant to a particular role.
    pub const ALL: Requests = Requests(511);

    /// Pure membership query for `kind`; returns one boolean and emits no child output or mutation.
    #[must_use]
    pub fn allows(self, kind: RequestKind) -> bool {
        let bit = match kind {
            RequestKind::Create => 1,
            RequestKind::Allot => 2,
            RequestKind::Accept => 4,
            RequestKind::Amend => 8,
            RequestKind::Cancel => 16,
            RequestKind::Release => 32,
            RequestKind::Move => 64,
            RequestKind::Watch => 128,
            RequestKind::Policy => 256,
        };
        self.0 & bit != 0
    }
}

/// Kinds of proposal a role may accept; permission does not replace the fresh action check.
/// (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProposalKind {
    /// A batch of delegated tasks.
    Batch,
    /// One pinned connector effect.
    Effect,
    /// Widen a task's authority.
    Widen,
    /// An amendment requiring more authority.
    Amend,
    /// Release an escalated task, with additional authority if required.
    Escalation,
}

/// Role's proposal-decision bit set; policy admission rejects bits outside `ALL`.
/// (domain/authority.md, sections 6 and 8.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Proposals(/** Bits 1, 2, 4, 8 and 16 for Batch, Effect, Widen, Amend and Escalation respectively. */ pub u8);

impl Proposals {
    /// All defined bits of this permission set; not a grant to a particular role.
    pub const ALL: Proposals = Proposals(31);

    /// Pure membership query for `kind`; returns one boolean and emits no child output or mutation.
    #[must_use]
    pub fn allows(self, kind: ProposalKind) -> bool {
        let bit = match kind {
            ProposalKind::Batch => 1,
            ProposalKind::Effect => 2,
            ProposalKind::Widen => 4,
            ProposalKind::Amend => 8,
            ProposalKind::Escalation => 16,
        };
        self.0 & bit != 0
    }
}
