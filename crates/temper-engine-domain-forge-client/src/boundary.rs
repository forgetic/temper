//! Values exchanged with the connector top (domain/connectors.md, sections 2–4).
//!
//! The client keeps live-resource rows and repository scan positions, while
//! the top keeps durable outbox entries. It never knows tasks beyond numbers.
//! `Event::Make` starts or recovers an entry after its commit; `Progress`
//! updates the top's copy before a write call; `Outcome` settles it.
use crate::api::{Answer, Error, Op, Read, Repository};
use crate::api::{Commit, Write};
use alloc::boxed::Box;
use skein_lib::{Time, Token, Wall};
/// The effect's own conditions, beyond the checks the raw API supports.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Condition {
    /// No extra effect condition is needed.
    None,
    /// The pull request must still target this base.
    Merge { base: Box<[u8]> },
    /// The update must start at these head and base commits.
    Update { head: Commit, base: Commit },
    /// The review must apply at this head.
    Review { head: Commit },
}
/// A requested write together with its application condition.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Effect {
    pub write: Write,
    pub condition: Condition,
}
/// How Forgejo v16.0.5 can recover a write whose answer was lost
/// (jig's domain/connectors.md, section 4.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Recovery {
    /// Forgejo refuses another creation with the same identity.
    Keyed,
    /// Forgejo checks the state named by the write as it applies it.
    Conditional,
    /// Repeating the write produces the same final state.
    Idempotent,
    /// Forgejo offers neither a deduplicating key nor an atomic condition.
    Unrecoverable,
}
/// The recovery class declared by each forge effect kind. A client-side
/// preflight does not make a write conditional: a late copy can pass it.
#[must_use]
#[expect(clippy::match_same_arms, reason = "each forge kind declares its own recovery reason")]
pub fn recovery(write: &Write) -> Recovery {
    match write {
        // Forgejo refuses a second pull only while one for those branches is open.
        Write::OpenPull { .. } => Recovery::Conditional,
        // A branch may be deleted; Forgejo then keeps no proof it was created.
        Write::CreateBranch { .. } => Recovery::Unrecoverable,
        // Forgejo checks the requested pull head when applying a merge.
        Write::Merge { .. } => Recovery::Conditional,
        // Issue creation has no server-enforced key; a marker only finds copies.
        Write::CreateIssue { .. } => Recovery::Unrecoverable,
        // A comment marker can be searched, but does not refuse a late copy.
        Write::Post { .. } => Recovery::Unrecoverable,
        // A review marker and a client-side head check do not deduplicate it.
        Write::Review { .. } => Recovery::Unrecoverable,
        // Forgejo's update checks neither the old head nor an operation id.
        Write::Update { .. } => Recovery::Unrecoverable,
        // Repeating an edit writes the same title and body.
        Write::Edit { .. } => Recovery::Idempotent,
        // Repeating the reviewer set writes the same members.
        Write::SetReviewers { .. } => Recovery::Idempotent,
        // Repeating close leaves the item closed.
        Write::Close { .. } => Recovery::Idempotent,
        // Repeating reopen leaves the item open.
        Write::Reopen { .. } => Recovery::Idempotent,
        // The context on a commit is a set value.
        Write::Status { .. } => Recovery::Idempotent,
        // Repeating deletion leaves the named branch absent.
        Write::DeleteBranch { .. } => Recovery::Idempotent,
    }
}
/// First-write search position. Never advanced by retries or restoration.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Position {
    pub at: Time,
    pub comment: u64,
    pub review: u64,
    pub review_page: u32,
}
/// The last write attempt, saved before its call can start. Absolute wall
/// expiry crosses process restarts; the original monotonic deadline is used
/// when its origin remains coherent. Neither expiry is restarted by a find.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Attempt {
    pub sent: Time,
    pub deadline: Time,
    pub wall: Wall,
    pub expires: Wall,
}
/// The root knows whether its monotonic origin survived this restart.
/// Wall recovery assumes an absolute clock coherent with the saved attempt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RecoveryClock {
    /// The saved monotonic origin is coherent.
    Monotonic,
    /// Recovery uses the saved absolute expiry.
    Wall,
}
/// An outbox entry durably owned by the connector top.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Entry {
    pub number: u64,
    pub task: u64,
    pub repository: Repository,
    pub effect: Effect,
    pub start: Option<Position>,
    pub attempt: Option<Attempt>,
    pub failures: u32,
}
/// A live forge object watched by the client.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Resource {
    pub repository: Repository,
    pub what: What,
}
/// The kind and identity of a live forge resource.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum What {
    /// The repository itself.
    Repository,
    /// A named branch.
    Branch(Box<[u8]>),
    /// A numbered pull request.
    Pull(u64),
    /// A numbered issue.
    Issue(u64),
}
/// One resource the parent asks the client to keep up.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Watch {
    pub resource: Resource,
    /// Participating objects deliver people's comments and reviews.
    /// Owned pulls never read people's forge verdicts.
    pub participating: bool,
}
/// A durable live-resource row and its delivery positions.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LiveRecord {
    pub watch: Watch,
    pub position: Position,
    pub listed: Time,
    /// Durable scan positions advance in the same decision as delivered news.
    pub comment_after: u64,
    pub comment_page: u32,
    pub comment_scanning: bool,
    pub comment_repair: bool,
    pub review_page: u32,
    pub comments: Box<[Delivery]>,
    pub reviews: Box<[Delivery]>,
    pub cached: Box<Cached>,
    pub pushed: Option<Commit>,
    pub echoes: Box<[Echo]>,
}
/// Latest delivered/recognized protocol revision of one inbox row.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Delivery {
    pub id: u64,
    pub revision: u64,
}
/// A forge row made by this deployment and awaiting recognition.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Echo {
    /// A comment this deployment posted.
    Comment(u64),
    /// A review this deployment posted.
    Review(u64),
}
/// The durable listing cursor and clock for one repository.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RepositoryRecord {
    pub repository: Repository,
    pub mark: Option<Time>,
    pub clock: Time,
}
/// The latest bounded values read for a live resource.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cached {
    pub item: Option<crate::api::Summary>,
    pub pull: Option<crate::api::Pull>,
    pub tip: Option<Commit>,
}
/// A durable client working-set record saved by the parent.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    /// A live-resource row.
    Live(LiveRecord),
    /// A repository scan row.
    Repository(RepositoryRecord),
}
/// The key under which a client working-set record is saved.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    /// The key for a live-resource row.
    Live(Resource),
    /// The key for a repository scan row.
    Repository(Repository),
}
/// What a settled write made at the forge.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Made {
    /// The created issue or pull request number.
    Created(u64),
    /// The posted comment number.
    Commented(u64),
    /// The posted review number.
    Reviewed(u64),
    /// The resulting merge commit.
    Merged(Commit),
    /// The head after an update.
    Updated(Commit),
    /// The created branch head.
    Branch(Commit),
    /// The requested state was set.
    Set,
}
/// The result of executing one committed outbox entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// The effect was made or found again.
    Made { made: Made, found: bool },
    /// The effect failed without a known write.
    Failed(Error),
    /// A known effect landed, but verification could not confirm its intended state.
    Raced { made: Made, why: Error },
    /// The write may still land and will be sought again.
    Uncertain,
    /// The write may still land, but Forgejo cannot make its retry safe.
    Held,
    /// The top withdrew the unsent effect.
    Withdrawn,
}
/// A request or answer delivered by the connector top.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Event {
    /// The complete union of resources named by live tasks. Admission is
    /// atomic; the top owns naming and task references.
    Keep { owner: Token, watches: Box<[Watch]> },
    /// Read sooner after a provider hint.
    Hint { hint: crate::api::Hint },
    /// Mark whether a writer slot is taken.
    Writer { resource: Resource, taken: bool },
    /// Record a head pushed by a task’s run.
    Pushed { resource: Resource, commit: Commit },
    /// The parent's outbox decision is durable. Numbers are commit order.
    Make { entry: Entry },
    /// Cancel an entry before it reaches the forge.
    Withdraw { entry: u64 },
    /// Restore a durable working-set record.
    Restore { record: Stored },
    /// Finish restoring the working set and select a recovery clock.
    Restored { clock: RecoveryClock },
    /// Hold restored writes while the connector reads its live resources.
    PauseOutbox,
    /// Read every restored live resource before procedures resume.
    ReadAfresh,
    /// Reconcile restored outbox entries after the fresh reads.
    SettleOutbox,
    /// A page read for a decision. Always enters the fresh class.
    Read { owner: Token, repository: Repository, read: Read },
    /// Exactly one terminal for each Call, including calls with no HTTP cost.
    Answered { call: Token, cost: u32, result: Result<Answer, Error> },
}
/// A decision or API call emitted to the connector top.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Request {
    /// Report whether a complete watch set was admitted.
    Kept { owner: Token, result: Result<(), Error> },
    /// Report a fresh observation of a live resource.
    Changed { resource: Resource, result: Result<Answer, Error> },
    /// Report a branch move outside its current writer.
    Drift { resource: Resource, expected: Commit, observed: Commit },
    /// Save a client working-set record in the parent’s decision.
    Save { record: Stored },
    /// Updated execution position for an entry durably owned by the top.
    Progress { entry: Entry },
    /// Erase a client working-set record in the parent’s decision.
    Erase { key: Key },
    /// Report an entry’s execution outcome for the top to commit.
    Outcome { entry: u64, task: u64, outcome: Outcome },
    /// Send a bounded forge call after its decision commits.
    Call { call: Token, repository: Repository, op: Op },
    /// Exactly one logical terminal, including admission refusal.
    Read { owner: Token, result: Result<Answer, Error> },
    /// Every restored live resource has completed its first fresh pass.
    ReadAfreshDone,
    /// Every restored uncertain entry has been looked for at least once.
    OutboxDone,
}
