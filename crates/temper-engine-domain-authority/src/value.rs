//! Owned authority data (domain/authority.md, sections 3 and 4).

use alloc::boxed::Box;

use skein_lib::Wall;

/// What a task may do. Each collection is bounded before the root admits it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Authority {
    pub tools: Tools,
    pub grants: Box<[Grant]>,
    pub delegation: Delegation,
    pub budget: Budget,
    pub notes: Scopes,
}

/// Up to 64 tool families, one bit each. Configuration assigns the bits,
/// including each connector's read family; the root translates them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tools(pub u64);

/// Note scopes relative to the task: goal, repository, project, deployment.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Scopes(pub u8);

impl Scopes {
    pub const GOAL: Scopes = Scopes(1);
    pub const REPOSITORY: Scopes = Scopes(2);
    pub const PROJECT: Scopes = Scopes(4);
    pub const DEPLOYMENT: Scopes = Scopes(8);
}

/// An effect or read of one connector's kind on the resources a pattern covers.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Grant {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
}

/// A connector's resource path. Bytes and segment boundaries are literal:
/// empty segments are permitted, with no parsing or normalization here.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Name {
    pub segments: Box<[Box<[u8]>]>,
}

/// Exact base segments, followed by the terminal's matching rule.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pattern {
    pub segments: Box<[Box<[u8]>]>,
    pub last: Last,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Last {
    /// Only the name the base segments spell, with no descendants.
    None,
    /// One additional segment exactly equal to this, and all descendants.
    Exact(Box<[u8]>),
    /// One additional segment beginning with this, and all descendants.
    Open(Box<[u8]>),
}

/// Executors local to authority, numbered by configuration or project policy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    Charter(u32),
    Procedure(u32),
    Role(u32),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Delegation {
    pub kinds: Box<[Executor]>,
    /// How many tasks the subtree may make over its life.
    pub tasks: u32,
    /// How deep below the task its subtree may go.
    pub depth: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    /// In the deployment's unit.
    pub spend: u64,
    /// Latest wall time it may end; absence is later than every time.
    pub deadline: Option<Wall>,
}
