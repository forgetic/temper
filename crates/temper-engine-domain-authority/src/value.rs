//! Owned authority data (domain/authority.md, sections 3 and 4).

use alloc::boxed::Box;

use skein_lib::Wall;

/// What a task may do. Each collection is bounded before the root admits it.
/// Root-supplied permission value, ordered componentwise; checked inputs must fit the configured
/// collection and byte limits. (domain/authority.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Authority {
    /// Permitted configured tool families. (domain/authority.md, sections 3–5).
    pub tools: Tools,
    /// Resource grants, bounded by `Limits::grants` at admission. (domain/authority.md, sections
    /// 3–5).
    pub grants: Box<[Grant]>,
    /// Executor, lifetime task-count and depth ceiling. (domain/authority.md, sections 3–5).
    pub delegation: Delegation,
    /// Spend and wall-time ceiling, rather than current available funding. (domain/authority.md,
    /// sections 3–5).
    pub budget: Budget,
    /// Permitted note scopes. (domain/authority.md, sections 3–5).
    pub notes: Scopes,
}

/// Up to 64 tool families, one bit each. Configuration assigns the bits,
/// including each connector's read family; the root translates them.
/// Configuration-selected tool-family bits; authority interprets no connector-specific family
/// names. (domain/authority.md, sections 3–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tools(
    /** Up to 64 configured family bits; inclusion is a bit-set subset test. (domain/authority.md, sections 3–5). */
    pub u64,
);

/// Note scopes relative to the task: goal, repository, project, deployment.
/// Set of the four supported note scopes relative to the task; admission rejects other bits.
/// (domain/authority.md, sections 3–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Scopes(
    /** Scope bits: goal 1, repository 2, project 4 and deployment 8. (domain/authority.md, sections 3–5). */ pub u8,
);

impl Scopes {
    /// Goal-relative note scope. (domain/authority.md, sections 3–5).
    pub const GOAL: Scopes = Scopes(1);

    /// Repository-relative note scope. (domain/authority.md, sections 3–5).
    pub const REPOSITORY: Scopes = Scopes(2);

    /// Project-relative note scope. (domain/authority.md, sections 3–5).
    pub const PROJECT: Scopes = Scopes(4);

    /// Deployment-relative note scope. (domain/authority.md, sections 3–5).
    pub const DEPLOYMENT: Scopes = Scopes(8);
}

/// An effect or read of one connector's kind on the resources a pattern covers.
/// Permission for one connector's kind and the resources covered by its pattern.
/// (domain/authority.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Grant {
    /// Connector whose resource naming and kind order apply. (domain/authority.md, sections 3–5).
    pub connector: u16,
    /// Granted kind, including configured implied kinds. (domain/authority.md, sections 3–5).
    pub kind: u16,
    /// Covered resources, bounded by segment and byte limits. (domain/authority.md, sections 3–5).
    pub pattern: Pattern,
}

/// A connector's resource path. Bytes and segment boundaries are literal:
/// empty segments are permitted, with no parsing or normalization here.
/// Owned literal resource segments; callers admit segment count and bytes before value queries
/// that do not perform admission themselves. (domain/authority.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Name {
    /// Literal ordered segments, bounded by `Limits::segments` and `Limits::segment_bytes` when
    /// admitted. (domain/authority.md, sections 3–5).
    pub segments: Box<[Box<[u8]>]>,
}

/// Exact base segments, followed by the terminal's matching rule.
/// Owned resource coverage value; base and terminal bytes have the configured name limits.
/// (domain/authority.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pattern {
    /// Exact base segments, bounded by the configured segment and byte limits.
    /// (domain/authority.md, sections 3–5).
    pub segments: Box<[Box<[u8]>]>,
    /// Terminal coverage rule after the base. (domain/authority.md, sections 3–5).
    pub last: Last,
}

/// Terminal coverage after the exact base; empty terminal bytes are literal and do not remove the
/// required extra segment. (domain/authority.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Last {
    /// Only the name the base segments spell, with no descendants.
    /// Covers exactly the base name, with no additional segments. (domain/authority.md, sections
    /// 3–5).
    None,
    /// One additional segment exactly equal to this, and all descendants.
    /// Requires this exact additional segment and includes all its descendants.
    /// (domain/authority.md, sections 3–5).
    Exact(
        /** Literal additional segment, bounded by `Limits::segment_bytes` at admission. (domain/authority.md, sections 3–5). */
         Box<[u8]>,
    ),
    /// One additional segment beginning with this, and all descendants.
    /// Requires an additional segment beginning with this prefix and includes all its descendants.
    /// (domain/authority.md, sections 3–5).
    Open(
        /** Prefix of the additional segment, bounded by `Limits::segment_bytes`; empty covers any additional segment. (domain/authority.md, sections 3–5). */
         Box<[u8]>,
    ),
}

/// Executors local to authority, numbered by configuration or project policy.
/// Configuration or project-policy identifier for a permitted executor, with no executor state
/// retained here. (domain/authority.md, sections 3–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    /// An agent running a configured charter. (domain/authority.md, sections 3–5).
    Charter(/** Configured agent charter number. (domain/authority.md, sections 3–5). */ u32),
    /// A configured procedure executor. (domain/authority.md, sections 3–5).
    Procedure(/** Configured procedure number. (domain/authority.md, sections 3–5). */ u32),
    /// A person executor addressed to a project role. (domain/authority.md, sections 3–5).
    Role(/** Project-policy person-role number. (domain/authority.md, sections 3–5). */ u32),
}

/// Lifetime descendant capacity and executor permissions; direct creation also consumes a task slot
/// and a depth level. (domain/authority.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Delegation {
    /// Permitted executors, bounded by `Limits::executors` at admission; duplicates do not widen
    /// permission. (domain/authority.md, sections 3–5).
    pub kinds: Box<[Executor]>,
    /// How many tasks the subtree may make over its life.
    /// Lifetime number of tasks the subtree may create; current remaining capacity is supplied
    /// separately. (domain/authority.md, sections 3–5).
    pub tasks: u32,
    /// How deep below the task its subtree may go.
    /// Maximum descendant depth; zero permits no delegation. (domain/authority.md, sections 3–5).
    pub depth: u32,
}

/// Spend authority and latest permitted wall time; current spend and reservations are separate
/// accounting inputs. (domain/authority.md, sections 3–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    /// In the deployment's unit.
    /// Maximum allotment in the deployment's unit; authority does not choose that unit.
    /// (domain/authority.md, sections 3–5).
    pub spend: u64,
    /// Latest wall time it may end; absence is later than every time.
    /// Latest ending wall time; `None` is later than every finite deadline. (domain/authority.md,
    /// sections 3–5).
    pub deadline: Option<Wall>,
}
