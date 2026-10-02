//! What the rules are asked, and what they answer (engine-model.md, section
//! 7), in their own terms; the top level translates to and from them.
//!
//! The engine asks before it starts a run ([`Run`]), before it makes a write
//! ([`Write`]), and before it acts on a person's request ([`Request`]), with
//! the facts each check reads inside what it asks: the spend so far, CI and
//! reviews on a pull request's exact head, a person's permission on the
//! repository, as the forge reports them. A plan's [`Gate`]s on the step
//! come with a run or a write, as extra conditions: each adds findings, and
//! none takes one away, so a gate never loosens a rule.
//!
//! Every check answers a [`Decision`], the strictest of what it found, and
//! writes each [`Finding`] (why it is not simply allowed) into the bounded
//! output its caller provides: `Refuse` is stricter than any acceptance, and
//! an acceptance by a person of a higher permission is stricter than one of a
//! lower.

use alloc::boxed::Box;

/// A repository, as the rules name it: one of the deployment's, by its place
/// in the configured list, or one that is not. Writes go only to the
/// deployment's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Repository {
    Deployment(u32),
    Elsewhere,
}

/// A person's permission on a repository, as the forge reports it, weakest
/// first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Permission {
    None,
    Read,
    Write,
    Admin,
}

/// What a check answers, least strict first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Decision {
    Allow,
    /// It waits for a person's acceptance, by someone with at least
    /// `permission` on the repository.
    Accept {
        permission: Permission,
    },
    Refuse,
}

/// A run the engine is about to start.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    /// What the run may spend: its charter's budget, in the deployment's unit
    /// of spend.
    pub budget: u64,
    /// What its goal's runs have spent so far, if it is under a goal, and what
    /// the deployment's runs have.
    pub goal_spent: Option<u64>,
    pub deployment_spent: u64,
    /// What it may do to repositories.
    pub grants: Box<[Grant]>,
}

/// What a run may do to a repository.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Grant {
    /// Read its checkout: any repository the forge lets the engine read.
    Read { repository: Repository },
    /// Push its change to `branch`.
    Push { repository: Repository, branch: Box<[u8]> },
}

/// A write the engine is about to make, an outcome's or its own.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    /// An item's own: create an issue, comment, edit the engine's own
    /// comment, set labels, close an item.
    Item { repository: Repository },
    /// Open a pull request into `base`.
    Open { repository: Repository, base: Box<[u8]> },
    /// Merge a pull request.
    Land(Landing),
    /// Delete `branch`.
    Delete { repository: Repository, branch: Box<[u8]> },
    /// Create the items of a plan of `steps`, whose estimate is `spend`, in
    /// `repository`.
    Plan { repository: Repository, steps: u32, spend: u64 },
    /// Put or delete a note's page, in `repository`'s wiki, of `scope`.
    Note { repository: Repository, scope: Scope },
}

/// A pull request to merge, and what the forge says of its exact head.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Landing {
    pub repository: Repository,
    /// The branch it lands on.
    pub base: Box<[u8]>,
    /// The head it is merged at, exactly.
    pub head: [u8; 32],
    /// CI as the forge last reported it, and the head it ran on.
    pub ci: Ci,
    pub ci_head: [u8; 32],
    /// Each reviewer's latest review.
    pub reviews: Box<[Review]>,
}

/// Where a pull request's CI stands.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ci {
    /// None ran, or none on the head in question.
    None,
    Pending,
    Passed,
    Failed,
}

/// A person's latest review of a pull request.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    /// The forge's name for the person.
    pub person: u64,
    /// Their permission on the repository.
    pub permission: Permission,
    /// The head they reviewed.
    pub head: [u8; 32],
    pub stance: Stance,
}

/// What a review says.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Stance {
    Approve,
    RequestChanges,
}

/// What a note is about, narrowest first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Scope {
    Goal,
    Repository,
    Deployment,
}

/// A person's request, on the web or on the forge, about an item of
/// `repository`, with their permission there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Request {
    pub repository: Repository,
    pub act: Act,
    pub permission: Permission,
}

/// What a person asks to do.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Act {
    /// Open a session.
    Open,
    /// Write to an item: a message to it, or to its run.
    Steer,
    /// Accept or reject a proposal, which waits for a person with at least
    /// `permission` (its [`Decision::Accept`]).
    Accept {
        permission: Permission,
    },
    Reject {
        permission: Permission,
    },
    /// Stop a run.
    Cancel,
    /// Release a held item.
    Release,
    /// Watch a run or an item.
    Watch,
}

/// A condition a plan adds to a step, on top of the rules.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Gate {
    /// The step writes nothing to code: no push, pull request, landing or
    /// branch deleted.
    ReadOnly,
    /// Its landing needs green CI on the exact head, on any branch.
    Ci,
    /// Its landing needs an approving review on the exact head by a person
    /// with at least `permission`, and none asking for changes, on any
    /// branch.
    Review { permission: Permission },
    /// Its run may spend at most `most`.
    Spend { most: u64 },
    /// Its goal's runs may spend at most `most` in all: the plan's budget.
    GoalSpend { most: u64 },
}

/// Why a check does not simply allow, and what that answers
/// ([`Finding::decision`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Finding {
    /// More than the limits let in: refused at the entrance, unchecked.
    Oversized(Oversized),
    /// A write, or a push granted, to a repository that is not the
    /// deployment's: refused.
    Elsewhere,
    /// A push granted to a protected branch, or a protected branch deleted:
    /// only a landing reaches one. Refused.
    Protected,
    /// A step the plan made read-only writes code, or is granted a push:
    /// refused.
    ReadOnly,
    /// A landing that needs green CI on its exact head, where CI there is
    /// `ci`: refused.
    Unchecked {
        ci: Ci,
    },
    /// A landing that a person with at least `permission` asked changes for
    /// on its exact head: refused.
    ChangesRequested {
        permission: Permission,
    },
    /// A landing with no approving review on its exact head by a person with
    /// at least `permission`: it waits for one.
    Unreviewed {
        permission: Permission,
    },
    /// A plan of more steps, or a costlier one, than the deployment lets pass
    /// unasked: it waits for a person with at least `permission`.
    PlanSize {
        permission: Permission,
    },
    PlanSpend {
        permission: Permission,
    },
    /// A note scoped wider than a goal, where the deployment wants a person
    /// with at least `permission` to accept it.
    WideNote {
        permission: Permission,
    },
    /// Spending past a bound: refused.
    Overspent(Bound),
    /// A person whose permission falls short of what the act needs:
    /// refused.
    Unpermitted {
        needs: Permission,
        has: Permission,
    },
}

/// What was more than the limits let in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Oversized {
    Grants,
    Reviews,
    Gates,
    /// A branch name.
    Branch,
}

/// A bound on spending.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Bound {
    /// The deployment's, per run, per goal and in all.
    Run,
    Goal,
    Deployment,
    /// A plan's: on the step's run ([`Gate::Spend`]), on its goal's runs
    /// ([`Gate::GoalSpend`]).
    Step,
    Plan,
}

impl Finding {
    /// What the finding answers.
    #[must_use]
    pub const fn decision(self) -> Decision {
        match self {
            Finding::Oversized(_)
            | Finding::Elsewhere
            | Finding::Protected
            | Finding::ReadOnly
            | Finding::Unchecked { .. }
            | Finding::ChangesRequested { .. }
            | Finding::Overspent(_)
            | Finding::Unpermitted { .. } => Decision::Refuse,
            Finding::Unreviewed { permission }
            | Finding::PlanSize { permission }
            | Finding::PlanSpend { permission }
            | Finding::WideNote { permission } => Decision::Accept { permission },
        }
    }
}
