//! Configuration held in force (domain/authority.md, sections 6, 8 and 10).

use alloc::boxed::Box;

use crate::{Authority, Implies, Pattern};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Rules {
    pub ceiling: Authority,
    pub period_spend: u64,
    pub minimum_run_spend: u64,
    pub maximum_run_spend: u64,
    pub implies: Implies,
    pub requirements: Box<[Requirement]>,
    pub landing: Box<[LandingRule]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Policy {
    pub ceiling: Authority,
    pub period_spend: u64,
    pub roles: Box<[Role]>,
    pub requirements: Box<[Requirement]>,
    pub landing: Box<[LandingRule]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Role {
    pub number: u32,
    pub authority: Authority,
    pub period_spend: u64,
    pub requests: Requests,
    pub decides: Proposals,
}

/// An effect of this exact kind, on a matching resource, needs each named
/// connector fact to have passed at its pinned state. Kinds of effect are
/// matched exactly: granting an implied kind never removes its requirements.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Requirement {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
    pub facts: Box<[u16]>,
}

/// A verdict may carry only over the clean updates verified by the root.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Freshness {
    Exact,
    Clean,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Gate {
    pub number: u32,
    pub blocking: bool,
    pub freshness: Freshness,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Approval {
    pub role: u32,
    pub people: u32,
    pub freshness: Freshness,
}

/// Requirements on one connector's exact landing kind and branch pattern
/// (domain/authority.md, 10; forge.md, 8.3). These add to change gates.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LandingRule {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
    pub ci: bool,
    pub up_to_date: bool,
    pub gates: Box<[Gate]>,
    pub approvals: Box<[Approval]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RequestKind {
    Create,
    Allot,
    Accept,
    Amend,
    Cancel,
    Release,
    Move,
    Watch,
    Policy,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Requests(pub u16);

impl Requests {
    pub const ALL: Requests = Requests(511);

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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProposalKind {
    Batch,
    Effect,
    Widen,
    Amend,
    Escalation,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Proposals(pub u8);

impl Proposals {
    pub const ALL: Proposals = Proposals(31);

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
