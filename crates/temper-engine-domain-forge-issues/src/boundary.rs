//! Inputs, committed projection and keyed requests (domain/forge.md, section 12).
//!
//! The root supplies the goal's current plan and accepted milestones. The
//! top keeps `Projected` beside the goal; this module holds no state and
//! never reads an issue back from the forge.
use alloc::boxed::Box;
use skein_lib::{Duration, Wall};

/// One task-list line from the current plan.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlanItem {
    pub text: Box<str>,
    pub done: bool,
}

/// Stable identity of a milestone in the goal's history.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MilestoneKey {
    /// The plan was accepted.
    PlanAccepted,
    /// A plan revision, identified by its history number.
    Revision(u64),
    /// A change landed.
    ChangeLanded(u64),
    /// A change was held.
    ChangeHeld(u64),
    /// A report the design agreed was worth keeping.
    Report(u64),
    /// The goal finished.
    Finished,
}

/// One comment, already selected and worded by the root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Milestone {
    pub key: MilestoneKey,
    pub text: Box<str>,
}

/// Current goal facts from the root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GoalView {
    pub goal: u64,
    pub repository: u64,
    pub title: Box<str>,
    pub goal_text: Box<str>,
    pub plan: Box<[PlanItem]>,
    pub milestones: Box<[Milestone]>,
    pub finished: Option<Box<str>>,
}

/// Keys for effects, scoped by goal and repository at the top.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    /// Create the issue.
    Open,
    /// Rewrite the body at this local projection revision.
    Body(u64),
    /// Add the named milestone comment.
    Milestone(MilestoneKey),
    /// Close the issue after its final comment.
    Close,
}

/// One write for the top's durable outbox.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Create a goal issue in its home repository.
    Open { goal: u64, repository: u64, key: Key, title: Box<str>, body: Box<[u8]> },
    /// Replace the issue's body.
    Body { goal: u64, repository: u64, key: Key, body: Box<[u8]> },
    /// Add one milestone comment.
    Comment { goal: u64, repository: u64, key: Key, body: Box<str> },
    /// Close the goal issue.
    Close { goal: u64, repository: u64, key: Key },
}

/// Committed projection, including writes already enqueued by the top.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Projected {
    pub opened: bool,
    pub digest: Option<[u8; 32]>,
    pub last_body: Option<Wall>,
    pub body_revision: u64,
    pub milestones: Box<[MilestoneKey]>,
    pub closed: bool,
}

impl Default for Projected {
    fn default() -> Self {
        Self { opened: false, digest: None, last_body: None, body_revision: 0, milestones: Box::new([]), closed: false }
    }
}

/// Bounds and the minimum interval between body writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub plan_items: u32,
    pub milestones: u32,
    pub title_bytes: u32,
    pub body_bytes: u32,
    pub comment_bytes: u32,
    pub interval: Duration,
}

/// What one projection step asks the top to do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decision {
    /// Commit and release this keyed effect.
    Effect(Effect),
    /// Recheck when a body rewrite becomes due.
    Wait(Wall),
    /// Projection matches the committed goal facts.
    None,
    /// Input or projection exceeded its bounds, or history contradicted itself.
    Hold,
}

/// The new committed projection and its next decision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Projection {
    pub projected: Projected,
    pub decision: Decision,
}
